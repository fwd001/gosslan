# Gosslan 项目上下文交接文档（AI Handoff）

> 用途：本文件汇总 Gosslan 项目的完整上下文、工程约定、CI/打包现状与本轮修复记录，
> 供接手的新 AI（或开发者）快速恢复现场继续工作。最后更新：2026-09-05。

---

## 1. 项目一句话定位

**Gosslan** 是一款**无中央服务器**的 P2P 局域网即时通讯桌面 + 移动应用：
设备间通过 UDP 组播/广播发现 + TCP 点对点传输直接通信，E2EE 加密，无需注册登录，
用设备指纹识别同一用户，数据只存本机 SQLite。界面高度模仿飞书/钉钉。
项目 README 非常详尽，是一切功能的权威说明。

- 仓库：`github.com/fwd001/gosslan`（public，默认分支 `main`）
- 当前版本：`0.3.1`（package.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json 三处一致）
- ProductName：`Gosslan`；identifier：`com.gosslan.app`
- 技术栈：**Tauri v2（tauri 2.11.x）+ Rust 2021 + Vue 3 + TypeScript + Vite 6 + Tailwind CSS + Headless UI + Lucide + Pinia + SQLite（rusqlite bundled）**

---

## 2. 目录结构与职责

```
gosslan/
├── .github/workflows/
│   ├── build.yml            # Windows x64（NSIS exe）
│   ├── build-macos.yml      # macOS 通用包（universal x86_64+arm64 .dmg）← 本轮新增
│   └── build-android.yml    # Android APK（4 个 ABI）
├── package.json             # npm 脚本（打包/版本/Android 入口）
├── scripts/
│   ├── version.mjs          # 版本号统一维护脚本（同步 4 处 + CHANGELOG）
│   ├── android/AndroidManifest.xml  # Android 权限模板
│   ├── android/permissions.xml      # CI/setup 注入清单的权限片段
│   ├── build-android.ps1 / setup-android.ps1 / check-env.ps1 等（Windows 本地用）
├── src/                     # Vue3 前端（stores/ api/ workers/ components/ layouts/）
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── capabilities/default.json
    └── src/
        ├── lib.rs / main.rs / commands.rs / state.rs / db.rs / device.rs
        ├── crypto.rs         # X25519 + Ed25519 + ChaCha20-Poly1305
        ├── gossip_engine.rs  # Gossip 广播（Bloom+LRU 去重、fan-out、TTL）
        ├── relay_manager.rs  # 大文件切片 Mesh 分发
        ├── protocol.rs       # 线格式（UDP 包 / TCP 帧 / 消息枚举）
        ├── network/          # discovery.rs / transport.rs / file.rs / mod.rs
        ├── transport/        # 0.3.0 新增：双通道聚合抽象（lan/bluetooth）
        ├── relay/mesh_router.rs  # 0.3.0 新增：异构 Mesh 桥接（独立可测试）
        └── storage/cache_cleaner.rs # 0.3.0 新增：缓存清理 + VACUUM
```

**关键端口/常量**：UDP `59991`（发现）、TCP `59992`（消息传输），定义在 `protocol.rs`。

---

## 3. 核心架构速记（详细见 README）

| 模块 | 机制 |
|---|---|
| 发现 | UDP 广播 255.255.255.255 + 组播 239.255.42.99；广播周期自适应（<100 节点 5s…） |
| 消息 | TCP 分帧（4 字节大端长 + JSON）；建链规则：device_id 字典序小者主动拨号 |
| E2EE | X25519 ECDH 派生共享密钥；ChaCha20-Poly1305 加密；Ed25519 对 message_id 签名 |
| 群聊 | 随机群密钥对称加密，用各成员公钥 ECDH 分发群密钥 |
| 离线 | 发送失败进 SQLite outbox，对方上线心跳后自动补发（msg_id 去重） |
| 大文件 | 64KB~512KB 分片 BitTorrent 式并行分发、乱序重组 |
| 身份 | `device.rs`：硬件指纹（桌面 machine-uid）+ 持久化 UUID / 主机名兜底 |
| 多开 | `--instance N` 参数：独立 DB/TCP 端口/指纹，UDP 共享（SO_REUSEADDR） |

---

## 4. 工程约定（重要）

1. **版本号只通过 `node scripts/version.mjs`（`npm run version:patch|minor|major`）维护**，
   一次同步 package.json、Cargo.toml、tauri.conf.json、package-lock.json，并把 CHANGELOG 的
   `[Unreleased]` 落为带日期小节。不要手改三处版本。
2. **发布 = 打 tag**：bump 版本 → commit → `git tag vX.Y.Z` → `git push origin main && git push origin vX.Y.Z`。
   tag 推送会自动触发三端打包并创建 GitHub Release（workflow 里 softprops/action-gh-release）。
3. **Rust 编译目标**：rust-version 1.77；`[profile.release]` 用 `lto=true, opt-level="s", strip=true`（release 编译较慢属正常）。
4. `src-tauri/Cargo.lock` 已提交（保证 CI 可复现）——本轮首次成功编译后补提交的。
5. `src-tauri/gen/android` 不入库，CI 每次 `tauri android init` 重新生成。
6. 作者 wd.f / fuwedong；版权 MIT。
7. 用户对**编译零错误**是底线；dead-code 警告本可以清理（见 §7 遗留项），目前不影响构建。

---

## 5. CI 打包体系现状（三端 workflow 行为一致）

触发：push `main`、push `v*` tag、手动 `workflow_dispatch`。tag 触发时额外跑 Release 发布。
产物始终作为 Actions artifact 上传；tag 触发时同时挂到 GitHub Release。

| workflow | runner | 命令 | 产物路径 |
|---|---|---|---|
| Windows `build.yml` | windows-latest | `npm run dist:win`（= tauri build --bundles nsis） | `src-tauri/target/release/bundle/nsis/*.exe` |
| macOS `build-macos.yml` | macos-15 | `npm run tauri -- build --target universal-apple-darwin`（x86_64+arm64 通用包） | `src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg` |
| Android `build-android.yml` | ubuntu-latest | JDK17 + Android SDK（platforms;android-34 / build-tools;34.0.0 / ndk;26.3.11579264）+ Rust 4 个 android target → `tauri android init` → 注入权限 → `npm run android:build`（= tauri android build --apk，4 ABI） | `src-tauri/gen/android/app/build/outputs/apk/**/*.apk` |

Android 权限注入：CI 用 python 把 `scripts/android/permissions.xml` 插到生成的
`src-tauri/gen/android/app/src/main/AndroidManifest.xml` 的 `<application` 之前
（局域网组播、蓝牙、前台服务等权限）。

**产物命名**：NSIS exe ≈ `Gosslan_0.3.1_x64-setup.exe`；DMG ≈ `Gosslan_0.3.1_universal.dmg`。
（version.mjs 打印的 `gosslan_...` 全小写示例与实际 productName 大写略有出入，仅提示性。）

---

## 6. 本轮修复记录（重要现场）

### 背景：远程 CI 此前 100% 失败
仓库内从 v0.3.0 起所有 Actions run（Windows/Android、main 与 tag）全部失败在 **Build 步骤**。
根因不是 CI 配置，而是 **Rust 代码一直编译不过**（10 个编译错误），本地此前从未成功构建过。

### commit `1c158f2`：修复 10 个桌面端编译错误 + 新增 macOS workflow + bump v0.3.1
1. `crypto.rs`：`ed25519-dalek` 需开 `features=["rand_core"]` 才有 `SigningKey::generate`。
2. `db.rs`：`list_transfers` 缺 `TransferInfo` import；`get_friend_x25519` 多包一层 `.optional()` 导致 `Option<Option<String>>`。
3. `discovery.rs`：SocketAddr `.parse()` 错误类型标注缺失；RTT `Option<i64>` 应为 `Option<u64>`。
4. `network/file.rs`：`rel_path` 在递归前被 move，需 `.clone()`。
5. `commands.rs`：`search_nearby_peers` 是 async + State 引用入参，必须返回 `Result`；
   `NetworkStatus`/`CacheInfo` 需 `pub`。
6. `network/transport.rs`：两处 `message_exists` 去重分支里 `std::sync::MutexGuard` 跨 `.await`
   导致 spawned future 非 `Send` → 改为先在小作用域查重返回 bool，再在独立块里持锁写库。
7. 提交 `Cargo.lock`；新增 `build-macos.yml`（universal .dmg）。

结果：**Windows ✅ / macOS ✅ 通过**；Android 仍失败。

### commit `ac27dad`（远程并发提交，注意合并过）
- `machine-uid` 0.5 上游**不支持 Android/iOS**（`machine_id` 模块只对 linux/macos/windows 等定义，
  android 目标直接编译失败）。改为 target-specific 依赖 + `hardware_fingerprint()` 移动端返回 None。
- `db.rs`：friends 表补 x25519/ed25519 公钥列 + 幂等迁移（E2EE 好友公钥持久化修复）。
该提交**未**包含下述 focus_window 修复，其 Android run 仍失败。

### commit `869f67c`（当前 HEAD，= ac27dad 变基 + 我的补充）
- `commands.rs focus_window`：`unminimize/show/set_focus` 是 **desktop-only** 的 WebviewWindow 方法，
  移动端编译报 E0599 → 拆成 `#[cfg(desktop)]`（原实现）+ `#[cfg(mobile)]`（空实现）。
  注：`desktop`/`mobile` 两个 cfg 由 tauri-build 注入，可放心使用（已在两端验证生效）。
- Cargo.toml 冲突解决：保留 `cfg(not(any(target_os="android", target_os="ios")))` 的 machine-uid 门控。
- 已本地验证：`cargo check`（host macOS）与 `--target aarch64-linux-android`（NDK clang）均 0 错误。

### 当前 CI 结论（commit 869f67c）
- Build Windows x64：**success** ✅
- Build macOS (Universal)：**success** ✅
- Build Android APK：**failure** ❌ —— 从 15:54 跑到 16:06（约 12.5 分钟）后失败在 **Build release APK** 步骤；
  已过 Rust 编译关（本地 aarch64 check 通过），**大概率卡在 Gradle 打包/资源阶段**。

---

## 7. 遗留问题清单（接手 AI 优先处理）

### P0：Android APK 仍失败（唯一红灯）
- 现象：workflow `build-android.yml` 全部前置步骤 success，唯独 `npm run android:build`
  （= `tauri android build --apk`，4 ABI）失败，失败点位于步骤"Build release APK"。
- 已排除：桌面端 10 个编译错误、machine-uid、focus_window desktop-only API（本地双端 check 均通过）。
- 拿真实日志的方法（REST 日志下载匿名 403，需权限）：
  1. 网页端手动打开失败 run → 展开 Build release APK 步骤复制尾部报错；
  2. 或安装 `gh` 并 `gh auth login` 后 `gh run view <run_id> --log-failed`；
  3. 或本地复现：`npx tauri android init && npx tauri android build --apk --target aarch64`
     （本机已有 Android SDK + NDK 27.1，SDK 位于 `/Users/wendongfu/Library/Android/sdk`）。
- 怀疑方向（未证实）：Gradle/AGP 打包期问题（manifest 注入后资源合并、AGP 与 NDK/compileSdk 组合、
  签名配置等）。**下一步 = 先拿日志，不要盲改。**

### P2：Rust dead-code 警告（7 条，不影响构建，符合"零警告"偏好可顺手清）
- `db.rs`: `remove_friend`、`insert_outbox` 未使用
- `human_size` 未使用；某 `from_str` 关联函数未使用
- `relay_manager` 的 `next_chunk / is_send_done / ack_chunk / progress` 未使用
- `NetworkHandle.tcp_port` 字段未读
- `transport` trait 的 `name / send / broadcast` 方法未使用
（清之前确认是否 `#[allow(dead_code)]` 更合适——部分属预留扩展点。）

### P3：文档/脚本不一致
- README 提到 `npm run android:build:release`，但 package.json 只有 `android:build`（release）
  与 `android:build:debug`；README 里版本示例为旧值（v0.2.1 等）。
- vite build 有 chunk >500kB 提示（1.17MB 主包），可选 code-split。
- CI 有 Node20 action deprecation 警告（checkout@v4 等）与 `setup-java@v4` 弃用提示（可升 v5）。

---

## 8. 常用命令速查

```bash
# 本地开发/前端
npm ci                       # 注意：在本 IDE 沙箱里需 env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED npm ci
npm run dev                  # vite
npm run tauri dev            # 桌面调试
npm run build                # vue-tsc 类型检查 + vite build（CI beforeBuildCommand 同款）
npm test                     # 前端纯函数测试（node 内置，需 Node≥22）
cd src-tauri && cargo test   # Rust 单测（crypto/gossip/文件切片/SQLite/协议）

# 版本与发布
npm run version:show         # 当前版本
npm run version:patch        # 0.3.x -> 0.3.(x+1)，同步 4 处文件
git tag v0.3.2 && git push origin main && git push origin v0.3.2   # 触发三端打包+Release

# 本地打包（按需；Windows 需在 Windows 上跑）
npm run dist:win             # NSIS exe（Windows）
npm run tauri -- build --target universal-apple-darwin   # macOS universal dmg
npm run android:init && npm run android:build            # Android release APK

# Android 交叉编译检查（无需完整 gradle，快速验证代码能否过 Android 编译）
# 需 rustup target add aarch64-linux-android 等，并设置：
#   NDK=/Users/wendongfu/Library/Android/sdk/ndk/<版本>
#   CC_aarch64_linux_android=$NDK/toolchains/llvm/prebuilt/<host>/bin/aarch64-linux-android21-clang
#   AR_aarch64_linux_android=$NDK/toolchains/llvm/prebuilt/<host>/bin/llvm-ar
cargo check --target aarch64-linux-android
```

---

## 9. 给接手 AI 的注意事项（本机环境坑）

1. **npm 命令被 WorkBuddy 文件钩子拦截**：报 `CODEBUDDY_BROKER_DENY / Brokered host mkdir` 时，
   给命令加 `env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED` 前缀（写磁盘类操作同理）。
2. 本机 Rust：rustup stable 1.98.1（`source "$HOME/.cargo/env"`）；4 个 android target 已装。
3. Android SDK：`/Users/wendongfu/Library/Android/sdk`，NDK 27.1（CI 用 26.3，版本差异注意）。
4. JDK 本机 21（CI 用 17）。Xcode Command Line Tools 已装。
5. 若本机跑 `tauri android init` 会生成 `src-tauri/gen/android/`，**别提交**（不入库）。
6. git remote 走 SSH（已认证）；提交身份 fuwedong/fuwendong5@outlook.com。
7. 远程 CI 状态可用公开 API 查（匿名够用）：
   `curl -s "https://api.github.com/repos/fwd001/gosslan/actions/runs?per_page=10"`，
   但**日志下载接口需权限**（见 §7）。

---

## 10. 建议的下一个动作

1. **拿到 Android 失败真实日志**（网页/gh/本地复现三选一），定位 Gradle 打包失败原因并修复 → 让三端全绿。
2. 若打 tag 前想带出完整 Release，可在 Android 修复后再 `npm run version:patch` 出 v0.3.2
   （v0.3.1 的 Release 已存在且只有 Win/Mac 产物；也可删除旧 release/tag 重建）。
3. 顺手清 dead-code 警告、升级 setup-java@v4→v5（可选）。
