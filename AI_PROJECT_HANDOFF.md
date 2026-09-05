# Gosslan 项目全景与开发指南（AI Handoff）

> **本文件的用途**：为三类读者提供完整项目上下文——
> ① 用 AI 编程工具（Claude Code / CodeBuddy / Cursor 等）继续开发的人；
> ② 想通读源码理解设计的开发者；
> ③ fork 后想二次开发的人。
>
> 内容包括：全部功能、架构与代码导读、协议与加密状态机、工程约定、测试口径、
> 历史演进与未来设想。最后更新：**2026-09-05（v0.11.0）**。

---

## 1. 项目一句话定位

**Gosslan**（gossip + LAN）是一款**无中央服务器**的 P2P 局域网即时通讯应用
（Windows / macOS / Android 三端）：

- 设备间通过 **UDP 组播/广播发现 + TCP 点对点传输**直接通信，无注册登录、无账号体系
- 所有单聊/群聊消息**强制端到端加密**（X25519 + ChaCha20-Poly1305），中继无法查看
- 用**设备指纹**识别同一用户，数据**只存本机 SQLite**
- 界面高度模仿**飞书 / 钉钉**，交互优先「乐观更新 + 失败可感知」

- 仓库：`github.com/fwd001/gosslan`（public，默认分支 `main`）
- 当前版本：**0.11.0**（package.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json / Cargo.lock 四处一致）
- 应用标识：`com.gosslan.app`，ProductName：`Gosslan`，CSS 前缀 `--gosslan-*`
- 技术栈：**Tauri v2 + Rust 2021 + Vue 3 + TypeScript + Vite + Tailwind CSS + Headless UI + Lucide + Pinia + SQLite（rusqlite bundled）**
- 作者：wd.f（fuwedong），MIT 协议

---

## 2. 完整功能清单（v0.11.0 现状）

### 2.1 网络与消息核心

| 功能 | 说明 | 版本 |
|---|---|---|
| UDP 发现 | 广播 255.255.255.255 + 组播 239.255.42.99（:59991）；自适应周期（<100 节点 5s / ≥100 10s / ≥500 20s）+ 0–2s 抖动 | 0.1.0 / 0.2.0 |
| 按需探测 | 打开「添加好友」时才群发 `who_has`，在线节点单播回复 announce；启动不持续扫描 | 0.3.0 |
| TCP 传输 | 分帧 = 4 字节大端长度 + JSON（:59992）；device_id 字典序小者主动拨号 | 0.1.0 |
| Gossip 广播 | Epidemic 泛洪，Bloom Filter + LRU 去重，fanout + TTL 衰减；信封 SHA-256 message_id + Ed25519 签名 | 0.2.0 |
| E2EE | **恒开且不可关闭**（v0.11.0 起）：X25519 ECDH 派生密钥 + ChaCha20-Poly1305 AEAD；详见 §5 | 0.2.0→0.11.0 |
| 群聊 | 随机群密钥对称加密；群密钥用各成员公钥 ECDH 单独加密分发（`GroupKey`） | 0.2.0 |
| 离线补发 | 消息一律写 outbox（INSERT OR IGNORE 幂等），**Ack 到达才删行**；对方上线建链/心跳触发 `flush_outbox`；接收方按 msg_id 去重 | 0.4.0 / 0.7.0 |
| 已读回执 | `ReadReceipt`（合并式 last_read_ts）；对方打开会话 → 我方消息绿勾；窗口聚焦时自动补发回执 | 0.6.0 |
| 心跳探活 | 每 5s 向已建链节点发 Heartbeat，静默断连及时清理 | 0.4.0 |
| 大文件 | **BitTorrent 式切片中继**：64KB–512KB 分片，并行分发到空闲中继二次转发，乱序重组；直连/中继**自动路由**（`send_file_auto`） | 0.2.0 / 0.4.0 |
| 共享目录 | 设置本地共享文件夹，好友点对点浏览目录树并下载（防目录穿越） | 0.1.0 |
| 多开 | `--instance N`：独立 DB / TCP 端口 / 指纹，UDP 共享（SO_REUSEADDR + unix 上 SO_REUSEPORT） | 0.3.0 / 0.4.1 |

### 2.2 聊天 UI / UX

| 功能 | 说明 |
|---|---|
| 三栏响应式布局 | PC 三栏（导航 / 会话列表 / 聊天区），移动端单栏滑动切换 |
| 消息类型 | 文本 / 代码（highlight.js 自动检测语言 + 折叠）/ 图片（粘贴板直发）/ 文件（内嵌进度条）/ 系统消息 |
| 长文本折叠 | >280 字符默认折叠 5 行，「展开全文 / 收起 / 复制」 |
| 消息回执图标 | 我发的消息**气泡左侧**挂状态：转圈=发送中，空心圆=已送达未读，**绿勾=已读**，红叉=失败（v0.10.0 移到左侧） |
| 消息时间 | 默认 `MM-DD HH:mm`，hover 切秒级 `YYYY-MM-DD HH:mm:ss`；同分钟合并消息仅 hover 显示 |
| 连续消息合并 | 同发送者 5 分钟内省略头像/昵称；≥5 分钟显示时间分割线；同分钟消息合并显示 |
| 虚拟滚动 | 自研 VirtualList：纵向滚动、scrollToIndex 任意定位、触顶分页加载历史（每页 100 条，上限 10 页）、未读定位（「以下是未读消息」分割线）、「回到最新」悬浮按钮 |
| 聊天样式 | 6 套配色预设（明暗双主题，对比度 ≥4.5:1）+ 3 档字号 + 消息合并开关；**跨设备同步**：对方按「我的配色」渲染我发的消息 |
| 复制 | 气泡 hover 复制按钮；**禁用**了文本右键自定义菜单（v0.10.0） |
| 删除聊天记录 | 会话列表项**右下角 X 按钮**（hover 浮现）→ 二次确认 Modal → 事务删除本地消息 + 会话行（幂等，不影响对方/好友关系） |
| 删除好友 | 联系人右键菜单；保留聊天记录，公钥随行移除，可重新添加 |
| 在线状态 | 好友头像绿点/灰点角标 + 离线置灰；聊天头部「对方在线/离线」 |
| 会话选中态 | 飞书式左侧主色指示条 + 名称高亮 |
| 系统通知 | 后台/非当前会话时原生通知；**点击通知唤起窗口并跳转到发送者会话**；好友申请等生命周期事件由 Rust 直接触发 |
| 桌面托盘 | **点 X 隐藏窗口驻留托盘**（不退出进程，后台继续收发）；托盘菜单「显示主窗口 / 退出」，**只有退出才结束进程**；单击托盘图标恢复；macOS Dock 点击恢复（`RunEvent::Reopen`） |
| E2EE 徽标 | 聊天窗口顶部恒显**绿锁**「端到端加密」 |
| 外观设置 | 主题色 / 字体（CSS 变量动态注入）/ 深色模式 / 网卡选择，即点即存 |
| 缓存管理 | 保留时长（3/7/30 天/永久）+ 磁盘配额自动清理 + VACUUM；「立即清理」 |

### 2.3 安全

| 功能 | 说明 |
|---|---|
| E2EE 恒开 | 单聊 X25519 静态 ECDH + ChaCha20-Poly1305；群聊群密钥；Gossip 信封 Ed25519 验签（防伪造/防篡改） |
| 公钥同步 | announce / FriendAccept / 好友建链路径自动同步公钥并持久化到 friends 表 |
| 公钥缺失兜底 | 发送时查 friends → peers → 主动 `who_has` 探测等 1.2s 重试 → 仍缺则报明确错误 |
| 解密失败兜底 | 收到 `enc1:` 解密失败（缺公钥/公钥已更新）→ 写入系统消息提示，**不静默丢消息** |
| 设备指纹 | 前缀 `gosslan-`（桌面 machine-uid，移动端 UUID/主机名兜底）；私钥持久化本地 SQLite，重启身份不变 |
| 隐私 | 数据只存本机；SQLite 不存 BLOB（图片/文件落 Cache 目录）；共享目录路径规范化校验 |

---

## 3. 目录结构与代码导读

```
gosslan/
├── .github/workflows/
│   ├── build.yml            # Windows x64（NSIS exe）
│   ├── build-macos.yml      # macOS 通用包（universal dmg）
│   └── build-android.yml    # Android APK（4 ABI）
├── package.json             # npm 脚本
├── scripts/
│   ├── version.mjs          # 版本号统一维护（同步 4 处 + CHANGELOG [Unreleased] 落日志）
│   └── android/…            # Android 权限模板（CI 注入）
├── docs/
│   ├── protocol-design.md   # ★ 协议对标 BeeBEEP/bitchat + 演进路线（设想文档）
│   ├── performance.md       # 500–1000 节点性能优化记录
│   ├── setup-windows.md     # Windows/Android 环境配置与打包
│   └── overview.md          # 2026-09-04 重命名/性能优化改动概览（历史）
├── src/                     # Vue 3 前端
│   ├── types.ts             # 与 Rust serde 结构一一对应的类型
│   ├── api/index.ts         # Tauri invoke封装 + 全部事件监听（bindEvents）
│   ├── stores/
│   │   ├── useAppStore.ts   # 设备/主题/深色/聊天样式/对端样式/toast/响应式
│   │   └── useChatStore.ts  # 好友/会话/消息合并/发送/回执/文件传输/通知（核心 store）
│   ├── utils/
│   │   ├── messages.ts      # 纯函数：mergeMessages 去重排序 / 会话未读统计 / preview
│   │   ├── chatStyle.ts     # 聊天样式预设解析
│   │   └── color.ts / cn.ts
│   ├── layouts/ResponsiveLayout.vue
│   └── components/
│       ├── ChatWindow.vue        # 聊天区（头部徽标/消息列表/输入区）
│       ├── MessageItem.vue       # 单条消息（气泡/回执/时间/复制/折叠）
│       ├── ConversationList.vue  # 会话+联系人列表（删除会话 X / 右键删好友）
│       ├── VirtualList.vue       # 虚拟滚动（绝对定位 + 触顶加载）
│       ├── CodeBlock.vue / BaseModal.vue / SettingsPanel.vue
│       ├── AddFriendModal.vue / GroupCreateModal.vue / ShareDirectory.vue
│       └── NavRail.vue / TopologyBar.vue
└── src-tauri/src/
    ├── lib.rs               # Tauri Builder：插件/状态/托盘 setup/invoke_handler 注册
    ├── main.rs
    ├── commands.rs          # Tauri 命令层（40+ 命令：send_message / delete_conversation / …）
    ├── state.rs             # AppState：identity/db/peers/gossip/group_keys/outbox 等
    ├── db.rs                # SQLite 存储层（SCHEMA 常量 + 全部 CRUD + 事务）
    ├── device.rs            # 设备指纹（gosslan- 前缀；桌面 machine-uid，移动端兜底）
    ├── crypto.rs            # ★ E2EE 原语：Identity / shared_secret / seal / open / 签名验签
    ├── protocol.rs          # 线格式：UDP 包 / TCP 帧 / Message 枚举 / GossipEnvelope
    ├── gossip_engine.rs     # Bloom+LRU 去重 / fanout / 信封构建与验签
    ├── relay_manager.rs     # 大文件切片 + 并行分发 + 乱序重组
    ├── schema.sql           # 建表脚本（与 db.rs SCHEMA 保持一致，文档用）
    ├── tray.rs              # 桌面托盘（#[cfg(desktop)] 门控；CloseRequested→隐藏）
    ├── network/
    │   ├── mod.rs           # 网络启动/停止
    │   ├── discovery.rs     # UDP 广播+组播 + 自适应周期 + who_has 探测
    │   ├── transport.rs     # ★ TCP 消息处理主循环（gossip/直发/回执/群密钥/outbox 补发）
    │   └── file.rs          # 文件直传 + 共享目录枚举
    ├── transport/           # 双通道聚合抽象（Transport trait；lan 已实现，bluetooth 为接口契约）
    ├── relay/mesh_router.rs # 异构 Mesh 桥接 + TTL + RingBuffer（接口就绪待蓝牙接线）
    └── storage/cache_cleaner.rs
```

**关键端口/常量**：UDP `59991`（发现）、TCP `59992`（消息），定义在 `protocol.rs`。

---

## 4. 核心机制详解

### 4.1 发现与建链

- announce 携带 device_id / 昵称 / TCP 端口 / X25519 + Ed25519 公钥；收到即 upsert peers 表并同步 friends 表公钥。
- 每对节点由 device_id 字典序较小一方主动拨号，避免重复建链竞态。
- 在线状态：peers 表 + 心跳（5s）判定；前端 `friends.online` 由 peers-updated 事件驱动。

### 4.2 消息投递的三条路径（统一 msg_id 去重）

1. **直发** `ChatMessage`（TCP 帧，内容可能带 `enc1:` 前缀）
2. **Gossip 广播** `Gossip{envelope}`（加密载荷或明文 base64）
3. **outbox 补发**（对方上线建链 Hello / 心跳触发 `flush_outbox`）

三处共用 Gossip 信封的确定性 SHA-256 message_id；接收方 `message_exists` 按 msg_id 幂等去重，
收到重复只回 Ack 不重复入库。发送方一律写 outbox（Ack 才删），防半开 TCP 静默丢包。

### 4.3 E2EE 状态机（v0.11.0 恒开）

```
发送方 send_message:
  查对端 X25519 公钥: friends 表 → peers 表 → 主动 who_has 探测 + 等 1.2s → 再查
    ├─ 拿到 → shared = X25519(自己私钥, 对方公钥)
    │         Gossip 载荷 = base64(seal(shared, JSON{kind,content}))
    │         直发内容  = "enc1:" + base64(seal(shared, content))
    │         信封 encrypted = true（build_envelope 默认）
    └─ 没有 → Err("尚未获取 {id} 的公钥：对方可能离线或处于不同子网…")

接收方 handle ChatMessage:
  content 有 "enc1:" 前缀?
    ├─ 查发送方公钥: friends 表 → peers 表
    │   ├─ 解密成功 → 明文落库
    │   └─ 失败/缺公钥 → 落库为 system 消息 "[加密消息] …"（不静默丢弃）
    └─ 无前缀 → 明文直接落库

接收方 handle Gossip:
  env.encrypted == false → base64 明文
  env.encrypted == true  → Chat: X25519(自己私钥, env.sender_pubkey) 解密
                            Group: group_id 群密钥对称解密
```

要点：
- **接收方解密只需要发送方公钥**（信封自带 `sender_pubkey`），与本机是否「开启加密」无关——这就是 v0.11.0 恒开后不存在兼容性问题的原因。
- 群密钥：创建群时生成随机 32B key，持久化 `settings` 表 `gk:<group_id>`；`GroupKey` 消息用各成员公钥 ECDH 加密分发；`distribute_group_key` 可补发（新成员上线）。
- 私钥以 base64 存 `settings` 表（`x25519_secret` / `ed25519_secret`），重启身份不变。

### 4.4 已读回执链路

1. 对方打开会话 / 窗口重新可见 → 前端防抖 600ms 调 `mark_read(conv_id)`
2. Rust 查该会话 `MAX(ts)` → 发 `ReadReceipt{last_read_ts}` 给对方
3. 对方收到 → 我发的、ts ≤ last_read_ts 的消息全部置 `read` → **绿勾**
4. 状态流转：`sending`（转圈）→ `delivered`（Ack 到达，空心圆）→ `read`（绿勾）；失败 = 红叉

### 4.5 前端消息管线

- 收到消息 → `enqueueMessage` 批量队列 → rAF（窗口可见）/ setTimeout（不可见）冲刷 →
  主线程同步合并（O(n) Set 去重 + 排序；**刻意不用 Web Worker**——WKWebView 生产构建下 Worker 可能加载失败导致消息全部卡住，v0.5.1 教训）。
- 乐观发送：先上屏 `sending` 态 → invoke 成功替换真实记录 → 失败置 `failed` + toast。
- 交互约定：**一切操作先假定成功、失败可感知**（删除会话/好友/发送均乐观 + 失败回滚）。

### 4.6 桌面托盘（tray.rs）

- `#[cfg(desktop)]` 门控（Android 编译不引入）；Cargo 需 `tauri` 的 `tray-icon` feature。
- `CloseRequested → api.prevent_close() + hide()`；托盘菜单「退出」才 `app.exit(0)`。
- **容错**：托盘初始化失败时不拦截关闭（保持默认退出），避免「窗口关不掉且无托盘可恢复」。
- macOS `RunEvent::Reopen`（Dock 点击、无可见窗口）同样恢复。

---

## 5. 工程约定（贡献者必读）

### 5.1 开发流程（作者明确要求）

**新功能一律：先计划 → 再设计测试 → 后实现 → 用测试验证。**
计划要简短列出任务分解与测试设计，不要直接开写代码。

### 5.2 版本与发布（每次交付缺一不可）

1. 变更先写进 `CHANGELOG.md` 的 `## [Unreleased]` 小节
2. `node scripts/version.mjs minor|patch`（新功能=minor，修复=patch；0.x 破坏性变更允许进 minor，日志标注 ⚠️）
   —— 自动同步 package.json / Cargo.toml / tauri.conf.json / package-lock，并把 `[Unreleased]` 落为带日期小节
3. `cargo check` 刷新 `Cargo.lock`（已提交，保证 CI 可复现）
4. `git tag vX.Y.Z && git push origin main && git push origin vX.Y.Z`
   —— tag 触发三端 workflow 并自动创建 GitHub Release（apk/dmg/exe 三件套）
5. push 后用 `git ls-remote origin main vX.Y.Z` 核对 main 与 tag 同 commit（沙箱 git 链式命令有假输出）

### 5.3 代码风格偏好

- 编译零错误底线；警告零容忍（新代码不得引入新警告）
- 原生 UI 组件优先，避免自定义实现；短命名；简洁注释（中文，解释「为什么」而非「是什么」）
- 第三方依赖引入前先评估体积、适配性、依赖污染成本；重构必须保持现有业务功能不变
- 前端类型与 Rust serde 结构一一对应（`src/types.ts`），camelCase invoke 参数自动转 snake_case

### 5.4 平台门控

- `#[cfg(desktop)]` / `#[cfg(mobile)]` 由 tauri-build 注入，放心使用（focus_window、托盘已验证）
- machine-uid 仅桌面目标（Cargo.toml target 门控）；Android 交叉编译检查见 §6

---

## 6. 测试与验证口径

| 层 | 命令 | 现状 |
|---|---|---|
| 前端纯函数 | `npm test`（node --test，需 Node ≥22） | 22/22 |
| 前端类型+构建 | `npm run build`（vue-tsc + vite build） | 通过 |
| Rust 单测 | `cd src-tauri && cargo test --lib` | 35/35（crypto/gossip/db/协议/分片） |
| 双端编译 | `cargo check` + `cargo check --target aarch64-linux-android` | 0 error |
| 协议级 E2E | `src-tauri/examples/e2e_peer.rs`（无 GUI 直连真实实例，15+ 断言） | 手动跑 |
| 全功能 dev 验证 | `scripts/e2e-dev.sh`（单机双实例，29 项断言） | 手动跑 |
| 桌面冒烟 | `npx tauri build --debug` → 启动 .app 验证托盘/启动日志 | 手动跑 |

Android 交叉编译检查（无需 Gradle，快速验证代码能否过 Android 编译）：

```bash
export NDK=$HOME/Library/Android/sdk/ndk/27.1.12297006
BIN=$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
export CC_aarch64_linux_android=$BIN/aarch64-linux-android21-clang
export AR_aarch64_linux_android=$BIN/llvm-ar
cargo check --target aarch64-linux-android
```

---

## 7. 历史演进时间线（版本 → 核心内容）

| 版本 | 日期 | 核心内容 |
|---|---|---|
| 0.1.0 | 09-03 | 基础 P2P：UDP 发现、TCP 分帧、好友、群聊、文件直传、共享目录 |
| 0.2.0 | 09-04 | E2EE（X25519/Ed25519/ChaCha20）、Gossip、大文件切片中继、群密钥、响应式布局、改名 Lanct→Gosslan、UI 换 Tailwind+Headless UI+Lucide |
| 0.3.0 | 09-04 | 按需 who_has 探测、系统通知、Transport 抽象（lan/bluetooth）、Mesh 中继路由、缓存清理、`--instance` 多开 |
| 0.3.1 | 09-04 | **修复 10 个 Rust 编译错误**（CI 此前 100% 失败的根因）、macOS workflow、machine-uid Android 门控 |
| 0.4.x | 09-04/05 | 统一文件自动路由、离线 outbox 激活、心跳探活、分页加载、会话选中态、e2e_peer 协议级验证工具、UDP 阻塞 socket 修复、SO_REUSEPORT 多开修复、统一 msg_id |
| 0.5.x | 09-05 | 虚拟滚动定位/未读分割线、长文本折叠、聊天样式跨设备同步、删除好友、**移除 Web Worker（Mac 消息不刷新根因）**、发送失败 toast |
| 0.6.0 | 09-05 | 消息回执链路（Ack + ReadReceipt + 绿勾）、在线状态角标、乐观交互推广、输入区重排、**确立发布三件套约定** |
| 0.7.0 | 09-05 | 代码块升级（语言检测/折叠）、**outbox 可靠性修复（Mac→Windows 丢消息根因：一律入队 + Ack 才删）**、乐观时间戳钳制、rAF 后台滞留修复、协议对标文档 |
| 0.8.0 | 09-05 | E2EE 开关（默认关）+ 直发链路加密统一（`enc1:`）+ 信封 encrypted 标志、**设备指纹前缀改 `gosslan-`（破坏性）**、设置页指纹显示 |
| 0.9.0 | 09-05 | **系统托盘：关窗驻留、仅托盘退出**（tray.rs，cfg(desktop)） |
| 0.10.0 | 09-05 | **删除聊天记录**（X 按钮 + 二次确认）、回执移到气泡左侧、时间 MM-DD HH:mm、移除右键复制菜单、**E2EE 健壮性**（公钥探测重试 / 解密失败写系统消息 / E2EE 关时不查公钥） |
| 0.11.0 | 09-05 | **E2EE 恒开且不可关闭**：移除开关，绿锁恒显，设置页只留说明 |

---

## 8. 演进设想（未来方向）

详细的协议对标分析（BeeBEEP / bitchat）与优先级见 **[docs/protocol-design.md](docs/protocol-design.md)**，
这里是摘要与产品层面的补充：

### 8.1 协议安全（P0–P1）

1. **Noise XX 会话**（`snow` crate）：当前静态 X25519 派生长期密钥，无前向保密；
   升级为建链握手派生会话密钥是安全收益最大的一步（TCP 通道先行，蓝牙直接复用会话层）
2. **好友指纹安全码 / QR 当面校验**：添加好友完成页展示指纹比对，防中间人（TOFU 增强）
3. **mDNS 第三发现通道**：覆盖跨子网 / 隔离广播域（BeeBEEP 实践）

### 8.2 传输扩展（P2）

4. **BLE 无配对通道**：`transport/bluetooth.rs` 接口契约已就位（btleplug），每设备同时
   GATT Central + Peripheral；需要通用分片协议（MTU 限制）+ 二进制帧头 + log₂(degree) fanout
5. **Store-and-Forward 多跳暂存**：`relay/mesh_router.rs` 的 RingBuffer 已就绪待接线
6. **通用分片协议**：消息级 (frag_id, seq, total)，蓝牙前置

### 8.3 产品方向

7. **QUIC 传输**：`transport.rs` 分帧读写两原语可替换为 `quinn`，消息层零改动
8. **服务端中继（可选）**：JSON 帧协议可跑 WebSocket，轻量中继打通跨网段 / 公网（电脑↔手机）
9. **E2EE 部分-加密场景**：恒开后旧版本（<0.10.0）对端收到 `enc1:` 会静默丢弃——如果仍需
   支持旧对端，考虑升级提示；另可做「密钥轮换」（设备重装后指纹不变但公钥变化的场景提示）
10. **账号体系（远期）**：device_id 之上叠加账号绑定与多设备同步（当前设计刻意无账号）
11. **UI 细化**：消息引用/回复、表情回应、群成员管理、深色模式下图片预览优化

### 8.4 已知限制（fork 者注意）

- 公钥缺失时无法发送（E2EE 恒开的固有代价）：从未上线过的好友无法收到消息；错误提示已做兜底
- `enc1:` 解密依赖「发送方公钥」，对方重装应用（私钥重建、公钥变化）后，我方需等对方重新
  announce 才能解密——期间消息会显示为系统提示（不丢失，但看不到内容）
- Android 侧 `transport/bluetooth.rs` 仅为接口契约，未接线
- Web Worker 已移除（WKWebView 兼容问题），大批量消息合并在主线程（实测微秒级，无忧）

---

## 9. 常用命令速查

```bash
# 开发
npm run dev                  # 仅前端 vite
npm run tauri dev            # 桌面调试（需 Rust）
npm run android:dev          # Android 真机调试

# 测试
npm test                                        # 前端 22 用例
cd src-tauri && cargo test --lib                # Rust 35 用例
cargo check --target aarch64-linux-android      # Android 编译检查（env 见 §6）

# 版本与发布（三件套约定，见 §5.2）
npm run version:show / patch / minor / major
git tag vX.Y.Z && git push origin main && git push origin vX.Y.Z

# 打包
npm run dist:win                # Windows NSIS（需在 Windows 上）
npm run tauri -- build --target universal-apple-darwin   # macOS universal
npm run android:init && npm run android:build            # Android release APK
npm run dist:win:portable       # Windows 便携版 zip（Windows）
npm run multi:run               # 单机多开 3 实例模拟多节点（Windows）
npm run env:check / env:install # Windows 环境检查/安装（PowerShell）

# 协议级验证
cd src-tauri && cargo build --example e2e_peer
GOSSLAN_AUTOSTART=1 ./target/debug/gosslan &    # headless 启动实例
# 另起 e2e_peer 对连（详见 examples/e2e_peer.rs 顶部说明）
```

CI：push `main` / push `v*` tag / 手动触发。tag 额外发布 Release。
产物：Windows NSIS exe、macOS universal dmg、Android 4-ABI APK。

---

## 10. 本机开发环境备忘（原作者 macOS 环境，fork 者可跳过）

1. npm 命令在 WorkBuddy 沙箱需前缀：`env -u NODE_OPTIONS -u CODEBUDDY_BROKERED_FS_HOOK_ENABLED`
2. rustup stable（`source "$HOME/.cargo/env"`）；4 个 android target 已装
3. Android SDK：`~/Library/Android/sdk`，NDK 27.1（CI 用 26.3）；JDK 21（CI 用 17）
4. `src-tauri/gen/android/` 不入库（CI 每次 `tauri android init` 重新生成）
5. git remote 走 SSH；提交身份 fuwedong / fuwendong5@outlook.com
6. CI 状态匿名可查：`https://api.github.com/repos/fwd001/gosslan/actions/runs`（日志下载需权限）
7. 匿名 GitHub API 限流 60/h，超限时用 WebFetch 读网页版 releases 页

---

## 11. 文档索引

| 文档 | 内容 |
|---|---|
| `README.md` | 项目门面：功能特性、快速开始、架构简介 |
| `AI_PROJECT_HANDOFF.md` | **本文件**：给 AI 编程/源码阅读/fork 者的完整上下文 |
| `CHANGELOG.md` | 全部版本历史（每版 Added/Fixed/Changed 明细） |
| `docs/protocol-design.md` | 协议对标（BeeBEEP/bitchat）+ 差距分析 + 演进优先级 |
| `docs/performance.md` | 500–1000 节点性能优化设计 |
| `docs/setup-windows.md` | Windows/Android 环境配置、权限清单、打包命令 |
| `src-tauri/src/schema.sql` | 数据库 Schema（与 db.rs SCHEMA 一致） |
