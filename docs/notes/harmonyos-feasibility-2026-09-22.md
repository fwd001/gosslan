# 鸿蒙（HarmonyOS NEXT / OpenHarmony）适配可行性审计 · 2026-09-22

> **性质**：纯调研审计。**本轮未改任何代码，未改任何构建配置**，只产出本文档。
> **口径**：每条结论标了来源。`已核实` = 我本轮从一手来源（rust-lang/rust 源码、openharmony/docs 原文、
> tauri-apps 仓库分支、本仓库源码行号）直接读到；`二手未核实` = 来自搜索/他人转述，我没拿到一手页面，
> 落地前必须自己再验。华为 developer 站是 JS 单页应用，直接 curl 只拿到 1749 字节空壳，
> 所以**所有分发/审核条款都是二手的**。

---

## 0. 结论先行

| 问题 | 答案 |
|---|---|
| 能不能做 | **手机：不能按现在的产品形态做**；**鸿蒙 PC / 2-in-1：能做，且成本比预想的低** |
| 卡在哪 | 不是编译，是**后台常驻**。鸿蒙三方应用切后台即被挂起，挂起后**网络资源不可用**，而"没有官方消息通道"的 mesh 客户端活着的全部意义就是后台收消息 |
| Rust 核心要不要重写 | **基本不用重写**。密码学全是纯 Rust（无 ring / 无 aws-lc / 无 openssl），socket2 / libc / cc 都已有 OHOS 支持，Rust 的 OHOS 目标还是 Tier 2 |
| 前端要不要重写 | **不用重写**（ArkWeb = Chromium M114）。但 **127 个 `invoke` 没有任何降级路径**，要换的是桥，不是 UI |
| 真正的成本大头 | ① 蓝牙通道要在 ArkTS 侧重写（btleplug 无 OHOS 后端）② "打开应用才收得到消息"这个语义要不要接受 ③ 上架合规 |
| 支持侧载么 | **没有 Android 式侧载**。所有安装路径都是"签名证书 + Profile"体系，个人开发者能自用的通道天花板是**约 100 台设备**（二手未核实） |
| 上架要什么 | 聊天/通讯类目的硬门槛含《安全评估报告》及平台通过截图，可能还需《增值电信业务经营许可证》（二手未核实）。**"无账号、无手机号、纯设备指纹 + 强制 E2EE" 与这一组要求正面冲突** |

---

## 1. 一条决定所有判断的技术事实：OHOS 的 `target_os` 是 `linux`

`已核实`（rust-lang/rust `master`，`compiler/rustc_target/src/spec/`）：

- `base/linux_ohos.rs`：`TargetOptions { env: Env::Ohos, ..base::linux::opts() }`
- `targets/aarch64_unknown_linux_ohos.rs`：`tier: Some(2)`、`host_tools: Some(true)`、`std: Some(true)`，另有 `tls_model: Emulated`、`has_thread_local: false`

即三元组 `aarch64-unknown-linux-ohos` 上 **`cfg(target_os = "linux")` 与 `cfg(unix)` 都为真**，
唯一能区分鸿蒙的是 **`cfg(target_env = "ohos")`**。

### 这条事实对本项目的三个具体后果

**后果 1：桌面专属依赖会被无声编进来。**

- `src-tauri/Cargo.toml:58-59` `notify-rust` 的 target 列表含 `target_os = "linux"` → OHOS 命中，
  连带 `zbus` / `dbus`（鸿蒙没有 D-Bus）。
- `src-tauri/Cargo.toml:64-65` `machine-uid` 的门禁是 `cfg(not(any(target_os="android", target_os="ios")))`
  → 安卓当年靠"排除 android"躲过的坑，鸿蒙**躲不过**，因为它不是 android。
  而 `src/device.rs:79` 确实会调 `machine_uid::get()`，`:110` 把它混进设备属性。
- `src/notifications.rs:73-81` 的 `any(macos, windows, linux, ...)` 桌面分支与 `:198` 的移动端分支
  是同一组列表的正反两面 → OHOS 会走**桌面 notify-rust 路径**，而不是插件路径。

**后果 2：`cfg(unix)` 优化会自动生效。** `src/network/discovery.rs:203,263` 的
`#[cfg(unix)] sock.set_reuse_port(true)` 在鸿蒙上会真的执行 —— 编译没问题，
**内核是否放行未知**（见 §6 待验项）。

**后果 3（好消息）：Tauri 官方分支已经处理过这件事。**
`已核实`（tauri-apps `feat/open-harmony` 分支原文）：

```rust
// crates/tauri-build/src/lib.rs:507
let mobile = target_os == "ios" || target_os == "android" || target_env == "ohos";
// crates/tauri-utils/src/lib.rs:270 等处
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]   // ← 正是为了绕开后果 1
```

也就是说：**鸿蒙被判定成 mobile**，于是本仓库为 Android 做的那套 `#[cfg(mobile)]` 桩
（`commands/logs.rs` 约 28 处，以及 `commands/logs_tests.rs:136-172` 那条
"每个 `#[cfg(desktop)]` 命令必须有 `#[cfg(mobile)]` 桩"的门禁测试）**直接复用**，
托盘 / window-state / 桌面菜单全部被裁掉。Android 化不是白做的。

> 但 `machine-uid`、`notify-rust`、`notifications.rs` 三处是**我们自己**的 cfg 判断，
> Tauri 管不着，必须各自补 `not(target_env = "ohos")`。这是本次审计找到的、
> 唯一一个"编译期就会红，且修法明确"的第一方清单。

---

## 2. 鸿蒙平台能力事实（一手来源：openharmony/docs 原文 + rust-lang/rust 源码 + tauri-apps 分支）

### 2.1 后台常驻 —— **这是不可跨越的那条线**

`task-management/background-task-overview.md:12`：

> "After being suspended, the application process cannot use software resources (such as common
> events and timers) or hardware resources (such as **CPU, network**, GPS, and Bluetooth)."

`task-management/continuous-task.md` 表 1 的**全部**长时任务类型：
`dataTransfer` / `audioPlayback` / `audioRecording` / `location` / `bluetoothInteraction` /
`multiDeviceConnection` / `wifiInteraction` / `voip`(13+) / `taskKeeping` / `avPlaybackAndRecord`(22+) /
`specialScenarioProcessing`(22+)。

逐条对我们：

- **没有一种对应"监听端口 / 接收消息"**。
- `wifiInteraction` 标注 **for system applications only** —— 系统应用专用。
- `taskKeeping` 从 API 21 起对 **2-in-1 设备**开放，非 2-in-1 需要 ACL 权限
  `ohos.permission.KEEP_BACKGROUND_RUNNING_SYSTEM`（三方基本拿不到）。
  → **这一条正是"鸿蒙 PC 能做、手机不能做"的分界。**
- `voip` 的例子写死了 "Chat applications ... **during audio and video calls**" —— 只有真在通话中才算数。
- `dataTransfer` 有两条附加约束（`:39`）：必须持续上报进度，**首次上报后超过 10 分钟无进度即取消**，
  且通知必须是 live view；用它跑空载保活属于"声明与实际不符"。
- 一致性校验（`:14`、`:73-79`）：申请了长时任务但没做对应业务 → 回后台即挂起；
  后台负载长期高于该类型典型值 → 挂起或终止；**用户把通知划掉，任务自动终止**。

`二手未核实`：华为 FAQ（faqs-network-95）称切后台 **2 秒冻结网络、12 秒释放**，
且明说三方无法用长时/短时任务支撑"长时间网络空载或低频心跳保活"。
数字没拿到一手，但**定性结论与上面一手文档一致**。

短时任务（`transient-task.md`）：24 小时约 10 分钟额度、单次最多 3 分钟 —— 救不了。

**推论**：Gosslan 在 Windows/macOS/Android 上的价值主张是"进程驻留托盘，后台继续收发消息、
继续替别人中继"。鸿蒙手机上这个前提**不成立**。可选只有两种：
(a) 接受"前台才在线"语义 —— 冷启动时一次性 flush outbox + 重连（这条链路项目已经有了，
`outbox` 表 + `flush_outbox`，属于降级而非新建）；
(b) 引入一个持有华为 Push Token 的服务端 —— 直接违背 ADR-0020「无账号、无注册、官方不运营」，
且 Push Kit 本身就要求服务端调 REST API（`二手未核实`：push-gettingstart 流程含
"服务端基于服务账号生成鉴权令牌"），IM 类目还要单独申请自分类权益，否则降级为
每天 2~5 条的资讯营销额度。

### 2.2 局域网能力 —— **前台完全够用**

`reference/apis-network-kit/js-apis-socket.md`：

- `:1059` `UDPExtraOptions.broadcast: boolean`，默认 **false** → **255.255.255.255 广播可发**，显式打开即可
  （与我们在 Windows 上踩过的 directed broadcast 结论同向：见 `discovery.rs:148-162`）。
- `:1103,:1183` `MulticastSocket`(11+) `addMembership` / `dropMembership` / `setMulticastTTL` /
  `setLoopbackMode` / `setReuseAddress` → **组播加入有原生 API**，对应 `discovery.rs:338-345`。
- `:3075` `TCPSocketServer.listen`(10+) → **可以监听 59992 接受局域网入连**，这是点对点模型的地基。
- 全文出现的权限**只有 `ohos.permission.INTERNET`**（normal / system_grant）。
  没有"局域网发现专用权限"，也没找到端口防火墙条款。
  → 网上流传的"鸿蒙局域网要额外申请权限"，本轮**未找到依据**。
- 接口枚举：`connection.getConnectionProperties` / `wifiManager.getIpInfo`（`GET_WIFI_INFO`，normal）
  可用，对应 `if_addrs` 那套网卡评分逻辑（`discovery.rs:67-104`）。

另：鸿蒙官方推荐 LAN 发现走 **`@ohos.net.mdns`**(11+)。它是唯一"被祝福"的通道，
且与 Bonjour/Avahi/JmDNS 互通 —— 但那是**协议变更**，牵动 INV-P01~P24，不在适配范围内，只记录。

`已核实`（rustc 目标规格）+ `二手未核实`（OHOS NDK 实际行为）：
NDK 侧是 **musl + clang**，标准 BSD socket 可用，且 `getSocketFd`(TCP 10+/UDP 23+) 能把 fd 交给 Rust。

### 2.3 蓝牙 —— **API 有，但我们的实现没有**

`security/AccessToken/permissions-for-all-user.md:23-29`：
`ohos.permission.ACCESS_BLUETOOTH`，level normal、**user_grant**，
明文覆盖 "advertising and scanning for Bluetooth Low Energy devices"。
GATT server（`createGattServer`/`addService`/`notifyCharacteristicChanged`）与
advertiser（`startAdvertising`）都在 connectivity-kit 文档里。→ 能力上**对等于 Android 那套**
（`gen/android/.../BlePeripheral.kt`）。

但 `src-tauri/vendor/btleplug/Cargo.toml:290-315` 的后端矩阵是
linux→`bluez-async`+`dbus`(`:290,293`)、windows→windows-rs(`:296`)、
apple→objc2-core-bluetooth(`:315`)、android→jni/droidplug(`:284`)。
OHOS 会命中 **linux 分支 → 找 D-Bus → 失败**。
所以蓝牙腿**必须在 ArkTS 侧重写**，对标 `src/transport/ble_android.rs` 另出一个 `ble_ohos.rs`，
`transport/mod.rs:15-24` 的 `all(feature="bluetooth", target_os=...)` 三选也要扩成四选。

（注：`bluetoothInteraction` 长时任务只覆盖"正在用蓝牙传文件时切后台"，救不了常驻。）

### 2.4 ArkWeb —— **现成 Vue 产物能跑**

`web/web-component-overview.md:54-61`：ArkWeb 基于 Chromium；
**OpenHarmony 4.1–5.1 = M114**，**6.0 = M132（默认，推荐）**。
鸿蒙 NEXT 5.x ⇒ Chromium 114（≈2023 年中）。

对照前端实际用到的能力（`src/` 全量 grep）：
`navigator.clipboard.write/writeText`（含 `ClipboardItem` 贴图，`MessageItem.vue:682-690`）、
`ResizeObserver`（自研 `VirtualList.vue:124-143`）、`window.visualViewport`（键盘顶起，
`useAppStore.ts:266-277`）、`matchMedia`、`URL.createObjectURL`+`Blob`、`backdrop-filter`、
容器查询（`style.css:622-641`，已在 `@supports` 里）—— **全部早于 M114**。
未用 Worker/WASM/WebRTC/IndexedDB/ServiceWorker，未用 `:has()` 和 CSS 嵌套。
构建目标 `vite.config.ts:72` 非 Windows 走 `safari13`，产物本身也吃得下 M114。

桥接能力：`registerJavaScriptProxy` / `runJavaScriptExt` / `createWebMessagePorts` /
`onInterceptRequest` / `setWebSchemeHandler`(12) + NDK 侧 `OH_ArkWeb_SetSchemeHandler`。
→ 可以自建 URL scheme 把 Rust 的字节流喂给 `<img>`/预览，等价于我们现在没有的 asset protocol。

两个坑：
1. **默认 UA 冻结为 `... Chrome/114.0.0.0 ... ArkWeb/4.1.6.1 Mobile`**，且 UA 不含 "Mobile" 时
   `viewport` meta 失效 —— 必须显式设 `metaViewport: true`。
2. `src/utils/platform.ts:20,41,61` 只有 `isMacUA/isAndroidUA/isIOSUA` 三个嗅探，
   `:84` 是**平台优先、宽度兜底**的布局判定（`useAppStore.ts:723` 取 `matchMedia`，`:734` 消费）。
   UA 里带 "Mobile" 会误判成 Android 布局 —— 要么加第四个 key，要么改成纯宽度判定。

### 2.5 Rust 侧生态 —— **比预期乐观得多**

| 依赖（本仓库） | OHOS 现状 | 来源 |
|---|---|---|
| `x25519-dalek` / `ed25519-dalek` / `chacha20poly1305` / `sha2`（`Cargo.toml:45,50-53`） | 纯 Rust，无 C/汇编 | `已核实`：`Cargo.lock` 共 563 包，其中 **`ring` / `aws-lc-rs` / `aws-lc-sys` / `openssl-sys` / `rustls` / `quinn` / `nix` 计数全部为 0** |
| `libc` | 有完整 `ohos` 模块，且**按 musl 处理** | `已核实` `libc/build.rs:251-258` `let musl = target_env=="musl" \|\| target_env=="ohos"` |
| `socket2 0.5.10`（`Cargo.toml:40`） | OHOS 编译修复**自 0.5.6 起已在**（PR #491），我们锁的版本更新 | `已核实` socket2 CHANGELOG |
| `cc`（编 SQLite amalgamation 用） | 支持 `linux + ohos` | `已核实` `cc-rs/src/lib.rs:3674` |
| `rusqlite 0.31` `bundled`（`:41`） | C 工具链路径通，需真编一遍确认 | 推论自上一条，**未实编** |
| `tokio 1` `full` / `mio` | 未见 OHOS 专项说明；`target_os=linux` 会走 epoll 路径 | **`二手未核实`** |
| `getrandom` **0.2.17 / 0.3.4 / 0.4.3 三个大版本同时在树里** | 各自的 OHOS 熵后端要分别确认 | **未核实**，风险点 |
| `if-addrs 0.13` / `hostname 0.3`（`:46,47`） | 走 libc 的 `getifaddrs`/`gethostname`，OHOS musl 有无需实测 | **未核实** |
| `btleplug`（vendor 目录） | **无 OHOS 后端** | `已核实` `vendor/btleplug/Cargo.toml` |
| `jni 0.22`（`Cargo.toml:118`，Android 专用） | 不适用，OHOS 是 N-API 不是 JNI | 结构性事实 |

**Tauri 官方鸿蒙支持状态**（`已核实`，GitHub 分支/commit）：

- `tauri-apps/tauri` `feat/open-harmony` @ `e3bf6eb`，最后提交 **2026-07-30**（Legend-Master，社区）
- `tauri-apps/wry` `feat/open-harmony` @ `6aaf4b8`，同 **2026-07-30**，文件 `src/ohos/mod.rs`
- `tauri-apps/tao` `feat/open-harmony` @ `813572f`
- `dev` / `next` 分支对 `ohos` **零命中** → **没有任何已发布版本支持鸿蒙**
- 分支把 `crates/tauri-cli/src/mobile/open_harmony/` 放在 **mobile** 目录下，
  且 `tauri-build` 调 `napi_build_ohos::setup()` → **Rust 是以 N-API `.so` 的形式被 ArkTS 进程加载**，
  不是独立进程。这决定了线程模型：`mobile_entry_point` 那套，和 Android 一样。

维护者 FabianLars 在 2025-11-18 表示 2025 年底/2026 初合并 wry 侧"非常不现实"（`二手未核实`，issue #7287）。
**结论：可以拿来当参考实现，不能作为发布依赖。**

---

## 3. 适配工作量分解（按层，含"要不要新写"）

### L0 编译期门禁修复（**必做，且是唯一能立刻做完的一块**）

三处第一方 cfg，各自加 `not(target_env = "ohos")`：
`Cargo.toml:58-59`(notify-rust)、`Cargo.toml:64-65`(machine-uid)、
`src/notifications.rs:73-81` 与 `:198`（正反两面必须一起改，理由见该文件 `:70-72` 的原注释：
两个入口各写一份平台分支一定会漂移）。

另需注意 `src/open_path.rs:44` 的兜底分支是
`#[cfg(all(not(target_os="macos"), not(target_os="android")))]` → OHOS 会落到
`tauri_plugin_opener`（桌面实现 fork `xdg-open`）。**这是"编得过但行为静默错"的那类**，
比编译失败更危险。

### L1 工程脚手架

- `cargo tauri ohos init` 等价物 → 生成 `src-tauri/gen/ohos/`（ArkTS 工程、`module.json5`、
  `build-profile.json5`）。对标现有 `gen/android/`。
- `scripts/check-mobile.sh` / `scripts/verify.mjs:283-294` / `scripts/package.mjs:215-216`
  现在写死 `aarch64-linux-android` + NDK clang + `CC_aarch64_linux_android`。
  需要平行的一组 OHOS 变量：OHOS NDK `native/llvm/bin/clang -target aarch64-linux-ohos
  --sysroot=native/sysroot -D__MUSL__`，以及 `.cargo/config.toml` 的
  `target.aarch64-unknown-linux-ohos.linker`。参照 `已核实` 的 rustc 官方 openharmony.md 搭建章节。
- 现有 `[profile.release] panic="abort"`（`Cargo.toml:129,137`）是被 `mobile_entry_point` 的
  `stop_unwind` 绑死的（见该处注释），OHOS 走同一约束，**不能顺手改**。

### L2 Rust 核心

| 模块 | 判断 |
|---|---|
| `crypto.rs` `protocol.rs` `gossip_engine.rs` `file_relay.rs` `db.rs` `ble_framing.rs` | **零改动**，纯 Rust + 纯逻辑 |
| `network/discovery.rs` | 广播/组播语义有原生 API 对等，但 socket 由 ArkTS 还是 Rust 持有要先定（见 L3）；`set_reuse_port` 待实测 |
| `network/transport.rs` | TCP 监听可复用；`cfg(windows)` 分支不受影响 |
| `state.rs:1022` 路径 | 只用 `app.path().app_data_dir()`，鸿蒙沙箱路径由 Tauri 分支给，**需验可写目录与容量** |
| `transport/ble_android.rs` | **必须新写 `ble_ohos.rs`**，走 N-API 到 `@ohos.bluetooth.ble` |
| `tray.rs` / `menu.rs` / `macos_*.rs` / `user_dirs.rs` | `#[cfg(desktop/macos)]` 自动裁掉，不动 |
| `commands/logs.rs` 约 28 处 desktop/mobile 成对桩 | 已有桩可复用，**只需按 `logs_tests.rs:136-172` 那条门禁补 OHOS 该走哪一侧** |
| `device.rs` | ADR-0021 已把设备 ID 改成首启随机生成，不再从机器码派生 → **鸿蒙上反而干净**，只需让 `machine_uid_opt()` 在 ohos 下返回 `None` |

### L3 架构决策（**必须人拍板，AI 不要替我定**）

Rust 与鸿蒙网络栈的边界在哪：

- **选项 A：Rust 自己开 socket**（musl `socket()`）。改动最小，逻辑全复用。
  风险：三方应用直接 syscall 是否被网络管控拦、fd 生命周期与 `onForeground` 重建连（文档明确
  socket 在冻结时会被 abort，必须 close 后重连）。
- **选项 B：ArkTS 持有 socket，N-API 回调喂 Rust**。最贴合鸿蒙规范（`UDPSocket`/`MulticastSocket`/
  `TCPSocketServer` 都是 ArkTS 一等 API），但要为 discovery/transport 新写一层 ArkTS 宿主 +
  一份 Rust 侧 trait 实现 —— 好在 `src/transport/mod.rs` 已有 `Transport` trait 抽象，
  加一个实现是既有形状，不是新形状。
- 我倾向 **先 A、失败退 B**，因为 A 的验证成本是"一次交叉编译 + 一次真机对连"，而 B 是几周。

### L4 前端

- **桥替换**：`src/api/index.ts` **127 个 invoke 名（其中 121 个在 `api/index.ts`）+ 33 个事件，零降级路径**
  （全仓无 `isTauri` / `window.__TAURI__` 守卫，只有 `TitleBar.vue:106-110` 那种 try/catch）。
  需要一层 shim：把 `invoke(cmd, args)` 转发到 ArkWeb `registerJavaScriptProxy` 注入的对象，
  事件反向用 `runJavaScriptExt` 推。**这是纯机械工作，但面积就是 127 + 33。**
  注意 `commands/helpers.rs:194`、`favorites.rs:243,333` 用了 `tauri::ipc::Response::new(bytes)`
  **裸二进制出参**，`group_todo_media.rs:75` 用 `InvokeBody::Raw` **裸二进制入参**（图片预览/媒体上传走这条），
  shim 必须支持 ArrayBuffer 双向，不能只搬 JSON。
- **UI 能力缺口**：`ChatWindow.vue:898` 的 `getCurrentWebview().onDragDropEvent`（拿真实文件绝对路径，
  且 `:896` 已是"移动端直接 return"）与 `MessageComposer.vue:548` 的 `read_clipboard_file_paths`
  在 ArkWeb 里**没有对等物**；项目已明确拒绝 HTML5 DnD（`ChatWindow.vue:871` 注释）。→ 移动端本来就是"点了再传"，
  按 Android 的移动选择器（`commands/mobile_picker.rs`）走即可，属于复用不是新建。
- 多窗口：`logs.html` / `settings.html` / `todos.html` 三个辅助窗口由 Rust 建
  （`commands/logs.rs:406-575`），ArkWeb 侧对等是 `multiWindowAccess` + 新开 Web 组件，
  或者干脆在鸿蒙上**只做单窗口 + 页内路由**（建议，砍 3 个入口的复杂度）。
- 托盘、Dock 角标、原生菜单、关窗驻留 —— 鸿蒙上没有对等概念，UI 要给出等价的
  "怎样才算退出"。**这是产品语义问题不是技术问题。**

### L5 合规与安全说明

- 强制 E2EE + 无服务器 + 无账号 = 无法履行内容审核义务（见 §5），这三条是产品定位本身，
  不要为了上架去改。
- 权限用途说明：鸿蒙 `module.json5` 每个 `user_grant` 权限要写 reason 字符串 + 隐私政策链接，
  比 Android 的 `permissions.xml`（`scripts/android/permissions.xml`）更严。

---

## 4. 路线对比（我的建议，不是结论）

| 路线 | 后台收消息 | 蓝牙 | 复用率 | 判断 |
|---|---|---|---|---|
| **鸿蒙手机（NEXT 5.x）** | ❌ 挂起即断网，无对应长时任务类型 | 需重写 | Rust 核心 ~85%，产品语义 0% | **不做，或做成"打开才在线"的阉割版** |
| **鸿蒙 PC / 2-in-1** | ✅ `taskKeeping` 对三方开放（API 21+） | 需重写 | Rust 核心 ~85% | **这才是有价值的目标平台**，社区 Tauri 鸿蒙工作也集中在 PC |
| **ArkWeb 壳（放弃 Tauri）** | 同上受限 | 需重写 | 前端 100%，Rust 靠 N-API | 只有在 Tauri 分支证明不可靠时的退路，桥要自己维护 |
| **纯 Web / 元服务** | ❌ 拿不到 socket | ❌ | — | 不考虑，架构不允许 |

**建议的下一步不是"开始做鸿蒙"，而是花半天做一个 spike 回答一个问题**：
`cargo check --lib --target aarch64-unknown-linux-ohos` 在 L0 三处改完之后能不能过。
过不了的清单才是真实成本。**在拿到这个清单之前，本文档所有工作量估计都不应当作承诺。**

---

## 5. 分发：侧载与上架

> ⚠️ 本节除标注外全部 **`二手未核实`** —— developer.huawei.com 抓不到一手正文。
> 数字（台数、人数、天数）任何一条落地前都要在 AGC 控制台自己确认。

### 5.1 侧载：没有 Android 式侧载

鸿蒙的模型是**每个包内必须带签名证书(.cer) + Profile(.p7b)**，Profile 记录包名、权限、设备白名单。
官方文档里**不存在**"允许安装未知来源应用"这个开关。可行通道：

1. **DevEco / `hdc install` 调试安装**：需调试证书 + 调试 Profile，设备先用 **UDID 在 AGC 注册**，
   取 UDID 要求开 USB 调试 → 必须先开开发者模式。传说的上限：**一个账号 100 台设备 /
   一个应用 100 个 Profile / 单个 Profile 100 台**。→ **适合我自己和小组真机验证，规模上不去。**
2. **指定设备发布（原"内部测试"）**：发布证书 + 指定设备 Profile，**不需提交应用市场审核**，
   包放自己的服务器，测试者点链接装。上限仍是 **100 台**。
   → 唯一"不过审就能发"的官方通道，但每个用户都要向我提交 UDID。
3. **AppGallery 邀请测试 / 公开测试**：邀请测试约 10000 人、公开测试约 1000 万次下载，
   **但都要过审核** —— 合规成本与正式上架基本相同，省不掉 §5.2 任何一项。
4. **企业 In-house**：企业主体 + In-house 专用证书，业务定位限"内部成员"，可挂 MDM。
   面向消费者的聊天应用走这条属明显越界。
5. **第三方侧载工具**（如 `likuai2010/auto-installer`）：现实中 Telegram 就是这么装进去的，
   但要求用户自己有实名华为账号 + 开发者模式 + PC，且随时可能被收紧。不能当产品分发方案。

**对本项目的直接含义**：**GitHub Releases 放个 .hap 让人自己装这条路不存在。**
这和现在 `release-artifacts/android/*.apk` 的做法根本不同 —— Android 侧载是默认可行的，鸿蒙不是。

### 5.2 正式上架需要的工作（大陆，2026）

| 项 | 内容 | 对本项目的可行性 |
|---|---|---|
| 开发者账号实名 | 个人=身份证（约 1–2 工作日）；企业=营业执照+法人认证 | 个人主体**理论上**可选聊天类目（审核指南 11.4 只禁金融/医疗等强管控类目） |
| **APP 备案** | 工信部 2023 通知，由**服务器接入商**办，需勾"鸿蒙"平台 + 填鸿蒙包名；商店侧做备案校验 | **本项目无服务器 → 可能命中官方"单机应用（未通过公共互联网提供互联网信息服务）"豁免**。但只要建议用户自接公网中继（ADR-0020），这个豁免口径就危险。**这条值得单独问华为。** |
| 软著 / 电子版权 | 应用名称须与软著证书一致 | 纸质 30–60 个工作日（最长板）；电子版权数天 |
| 隐私合规 | 隐私政策 + 权限用途 + SDK 逐个明示 + **商店跑自动化个人信息保护合规检测** | 我们零采集零上报，**这条是我们少见的优势** |
| **类目资质（决定性）** | 聊天/社区/通讯 要求：①《安全评估报告》②该报告在全国互联网安全管理服务平台的提交结果截图且"通过" ③涉及互联网信息服务还需《增值电信业务经营许可证》 | **《安全评估》依据的法规把"聊天室、通讯群组"明确纳入管辖**；增值电信许可**个人主体拿不到**。这是最硬的一堵墙 |
| UGC 身份核验 | 审核指南 4.15：核验用户账号身份并留痕 + 关键词过滤 + 举报入口 + 违规停服 | **"无账号、无手机号、纯设备指纹" 与这一条正面冲突。无公开案例支持可通过** |
| 未成年人保护 | 网络社交须设时间/权限/消费管理，含陌生人社交不得以未成年人为目标 | 可加，但要写进隐私政策与 UI |
| 审核时效 | 常规约 24 小时，可加急 | 不是瓶颈，前面几项才是 |

**端到端关键路径约 10–16 周**，且软著 → 应用名、备案 → 鸿蒙包名、安全评估通过截图 → 资质提交
三组是**串行**依赖。

**先例**：`二手未核实` 有称 LocalSend（开源局域网传输）鸿蒙版通过 AppGallery **邀请测试**链接分发
（2026-01）。**未找到任何端到端加密 IM 正式上架鸿蒙商店的案例，也未找到华为针对
"加密通讯/无服务器应用"的书面拒绝依据**（禁止清单只覆盖 root、跨境 VPN、动态 IP 代理、非法侵入类工具）。
先例不存在 ≠ 不能做，但**不要假设能通过**。

### 5.3 我的判断

- **不要以"上 AppGallery"为目标启动这件事**。目标定成"鸿蒙上能跑起来给 20 个朋友用"，
  路径就是 §5.1 第 1/2 条（≤100 台 + 自托管 .hap + 链接），**审批成本为零**。
- 真要走正式上架，**主体、资质、实名核验三项必须产品级改动**，
  那意味着放弃"无账号 + 强制 E2EE + 官方不运营服务器"三个定位中至少两个 ——
  那是**另一个产品**，AI_RULES §3 的范围边界应当明确排除。

---

## 6. 未核实项清单（下次要验的，按优先级）

1. `cargo check --lib --target aarch64-unknown-linux-ohos` 的真实报错清单（L0 三处改完之后）。
2. `getrandom` 0.2.17 / 0.3.4 / 0.4.3 三个版本各自的 OHOS 熵后端（树里同时存在三个大版本）。
3. `tokio`/`mio` 的 epoll 路径在 OHOS 是否编译通过；`if-addrs` 的 `getifaddrs` 在 OHOS musl 是否可用。
4. `rusqlite bundled` 的 SQLite amalgamation 用 OHOS clang 编出来能不能跑（`cc` 支持已核实，运行时未验）。
5. Rust 直接 `socket()` 发 255.255.255.255 / 加入 239.255.42.99 组播，OHOS 内核是否放行；
   `set_reuse_port` 在 `discovery.rs:203,263` 会不会失败。
6. 后台网络冻结的**具体秒数**（华为 faqs-network-95，SPA 抓不到）。
7. 侧载/内测的**台数上限**、软著真实时长、备案时长（AGC 控制台核实）。
8. 聊天类目"安全评估报告"是否接受纯局域网、无公网服务的应用口径 —— 这条决定 §5.3 第二条。
9. `feat/open-harmony` 分支在**商用 NEXT 手机**（而非 OpenHarmony 开发板）上是否真能起窗口。
10. ArkWeb M114 对 `document.execCommand`（`MessageComposer.vue:224,580` 富文本输入在用，
    靠它保 undo 栈）的实际行为。

---

## 7. 本轮改动范围声明

- 新增：`docs/notes/harmonyos-feasibility-2026-09-22.md`（本文件）。
- **未改**：任何 `.rs` / `.ts` / `.vue` / `Cargo.toml` / `tauri.conf.json` / `scripts/*` / CI。
- **未改**：`README.md`、`AI_RULES.md`、`docs/AI_ENGINEERING_INDEX.md` 三处索引 ——
  本文档属于调研笔记（与 `docs/notes/` 里 `bitchat-comparison.md`、`community-repos-review.md` 同类），
  不是 AI 约束文档。**是否要把它登记进索引、以及是否要在 `AI_RULES.md` §3「当前不做」里
  给鸿蒙一个明确位置，是需要我（用户）拍板的事，AI 不要顺手加。**
