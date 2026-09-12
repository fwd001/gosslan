# Changelog

本项目遵循[语义化版本 SemVer](https://semver.org/lang/zh-CN/)：

- **major**：破坏性变更 / 架构级重构（不向后兼容）
- **minor**：新增功能（向下兼容）
- **patch**：Bug 修复与细节优化

版本号统一由 `npm run version:patch|minor|major` 维护，一次改动同步 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json` 五处，并把本文件 `[Unreleased]` 小节落为带日期的版本小节。

## [4.1.0] - 2026-09-12
### Changed (默认昵称：不再用设备用户名，改为「形容词 + 动物 + 设备短码」的英文名)
用户要求：「默认用户名可以不用设备的用户名吗？用一串英文，可以加设备识别号的前几位或后几位；
长度合适，让用户不改也好看，也有想改的欲望。」

- **规则**：`<Adjective> <Animal> <3 位 base36>`，例如 `Lively Puma W1U`。由 `device_id` 的 SHA-256
  **确定性派生**：同一台设备每次启动同名（随机数会让重启后名字变化，好友列表就认不出谁是谁）；
  短码来自设备标识的哈希，**不含设备信息**（旧规则直接拿 hostname 当昵称：既不好看，也把设备名写了出去）。
- **长度**：词表只用 ≤6 字母的词 ⇒ 总长 ≤17 字符，列表里不会被截断成省略号。
- **一次性迁移**：仅当已存名字是"旧默认的产物"（空串 / "Gosslan 用户" / "Gosslan User" /
  恰好等于本机 hostname）才替换；**用户自取的名字一律不动**。
- **"恢复默认"走同一条规则**：新增命令 `default_nickname`，前端不再写死 i18n 文案
  （否则"恢复默认"与首次安装得到的名字会不一致）；顺带删掉两处写死的默认名。

**护栏**：`nickname.rs` 4 条单测（确定性 + 三段式/纯 ASCII/长度上限并断言词表无长词、不同设备短码不同、
base36 补零大写、旧默认名识别且不误伤自取名字）。

**顺带修**：`scripts/version.mjs` 发布后**补回 `## [Unreleased]

### Changed (出一个 Mac 生产包也纳入常规流程；并加一条只打 .app 的脚本)
用户要求：「后面每次打完安卓的包，再打一个 Mac 的生产包，我本地测试」。

- 新增 `npm run dist:mac:app`：只出 `.app`（`--bundles app`）。
  为什么要这条：`npm run dist:mac` 会继续打 `.dmg`，而 dmg 那步要跑 `hdiutil` + AppleScript
  设置窗口布局 —— 在受管沙箱里会失败（本轮实测：`bundle_dmg.sh` 退出码非 0），
  虽然 `.app` 其实已经产出成功。以后本机自测用 `dist:mac:app`，要发布 dmg 时在**自己的终端**里跑
  `npm run dist:mac`。
- 产物：`src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Gosslan.app`
  （release + `bluetooth` feature；`open` 或拖进 /Applications 即可测）。

## [4.1.6] - 2026-09-12

## [4.1.5] - 2026-09-12

### Fixed (安卓「点『添加好友』/『设置』立刻闪退」—— Android 框架 API 被从非主线程调用)
用户实测 4.1.3：**打开应用不闪，一进「添加好友」或「设置」立刻闪退**。

两个入口的唯一共同新代码是 `ensureBluetoothOn()`（打开这些界面时按需申请「附近的设备」权限）。
根因形态：Rust 命令跑在 **tokio 工作线程**上，JNI 直接调进 Kotlin 后，
`ActivityCompat.requestPermissions` / `openGattServer` / `startAdvertising` 这些
**Android 框架 API 只能在主线程（有 Looper 的线程）调用**；从工作线程调用会抛 Java 异常，
而 Java 层的未捕获异常由系统处理器**直接杀掉进程** —— 它不是 Rust panic，
所以 panic hook 也抓不到、日志里什么都没有（与"什么都拿不到"的现象完全吻合）。

修法（Kotlin 侧，`BlePeripheral.kt`）：新增主线程跳板
- `onMainSync { }`（带返回值、最多等 3s）用于 `start()`，保留它原来的 `Boolean` 契约；
- `onMain { }`（异步 post）用于 `requestAllPermissions()`、`stop()`；
- 全部 try/catch 兜底并 `nativeOnWarning(...)` ⇒ 平台调用失败**最多是"通道没开"，绝不让应用消失**。

教训（写进注释）：**任何触碰 Android 框架/Activity 的 JNI 入口都必须回到主线程**。
顺带把"日志进 logcat + 启动路标"（上一提交）保留，下次即使还有别的崩点也能自己浮出来。

门禁：`npm test` 344/0；`vue-tsc` 0；`check-mobile.sh --bluetooth` PASS / 0 warning；
APK 构建通过（Kotlin 编译是该改动的实际验证）。

## [4.1.4] - 2026-09-12

### Added / Changed (安卓崩溃可诊断：日志进 logcat + 启动路标；启动路径彻底不申请权限)
用户实测 v4.1.2「打开还是闪退，连日志都拿不到」。在拿到 logcat 之前，先把"能自己缩小范围"的
两件事做掉：

- **日志同时写 logcat**（`log -t gosslan ...`）：release 包既不能 `run-as`、Rust 的 stdout/stderr
  也不进 logcat，此前崩溃现场对用户和我们都是黑的。现在 `adb logcat -s gosslan` 就能看到
  应用自己的日志（含 panic hook 那条 `panic @ 文件:行:列：消息`）。
  开销控制：warn/error 一律打，info 只打 `boot` 通道（启动路标）。
- **启动路标**：`AppState::init` 前后、`tray::setup` 前后各打一行 `boot` 日志 ——
  下次"打开就闪退"时，最后一条路标直接告诉我们崩在哪一步。
- **移动端启动路径彻底不申请权限**（上一轮已把蓝牙运行时改成按需，这一轮连权限申请也改成按需）：
  打开「添加好友」或网络设置时才申请。启动路径至此**不含任何平台专有调用**。

顺带修一处**只有安卓会现形**的编译问题：给 `tray::setup` 加路标时把 `#[cfg(desktop)]`
拆开了（属性只作用于紧跟其后的**一条**语句），导致 `tray::setup` 掉出 cfg ⇒ 移动端 E0433。
已改成整块包 `#[cfg(desktop)]`。`check-mobile.sh --bluetooth` 正是为这类问题存在的门禁。

门禁：`cargo test --lib` 399/0；`npm test` 344/0；`vue-tsc` 0；`vite build` 通过；
`check-mobile.sh --bluetooth` **PASS / 0 warning**。

## [4.1.3] - 2026-09-12

## [4.1.2] - 2026-09-12

## [4.1.1] - 2026-09-12`**（此前发布一次就把这一节吃掉，
下一次记账无处可写 —— 这个坑本轮咬了我两次）。

## [4.0.0] - 2026-09-12

### Added (Phase 8：BitChat 中继 —— 外部 mesh 的不透明帧，Gosslan 只当中继)
依据 ADR-0017（用户裁定：本版不做旧版兼容 ⇒ 不需要能力门控/双读），验收只有三条：
**收得到 · 去得掉重 · TTL 递减后转发**。

- **线格式**新增 `Message::OpaqueExternal { id, ttl, payload }`（原样字节 base64），
  去重用它自己的 `id`，**不进** Gosslan 的 `message_id` 体系。
- **收到即喂同一条流水线**：`handle_message` 新分支 → `MeshRouter::on_receive`
  （全局去重 + TTL 递减 + 源节点排除，与业务帧同一套；路由器不解析载荷，P-A03）
  → `Forward{frame}` 时用**路由器给出的 ttl**（已递减）按 fan-out 发给邻居（排除来源）。
- **不做的事**（照 ADR 写死）：不解密、不落库、不建 BitChat 用户/channel、不进 gossip 引擎。
- **健壮性底线**：新增纯函数 `validate_opaque_external`（id ≤128 且字符安全、ttl ∈ 1..=16、
  payload 合法 base64 且解码后 1..=256 KiB）—— 畸形/超限帧**只丢这一帧、不断链**。

**护栏**：`phase8_acceptance_receive_dedup_and_ttl_forward`（验收三条一次跑通）、
`opaque_external_validation_bounds`、`opaque_external_round_trips_through_wire_format`。
验证：`cargo test --lib` **395 / 0**；`npm test` **344 / 0**；`vue-tsc` 0；`vite build` 通过。

## [3.0.1] - 2026-09-12
### Changed
- 版本发布 v3.0.1（本次未预先填写更新说明，明细见 tag v3.0.0...v3.0.1 的提交记录）

## [3.0.0] - 2026-09-12

### Fixed (安卓发附件/图片总是失败 —— 选择器给的是 `content://` URI，不是文件路径)
用户实测：「发文件总是失败，但文字、代码都能发」。

**根因（读上游源码确认）**：Android 的文件选择器返回 **`content://` URI**；
`tauri-plugin-dialog` 的 Kotlin 侧直接把它交给前端（插件里的 `getPathFromUri` 是**没人调用**的死代码），
于是 `std::fs::metadata("content://…")` 必然失败 ⇒ 发送失败。桌面端本来就是真实路径 —— 只有安卓会这样。

**修法**：新增命令 `import_picked_file`，选择器返回值**统一先过它**：
不是 URI 就原样返回（桌面零开销）；是 URI 就经 `tauri-plugin-fs`（安卓走 ContentResolver）
**流式复制**进 `cache/imports/` 再发送 —— 后面的图片预览/缩略图/断点重传都不用改。
文件名优先取 URI 里编码的真实名字（`…%3ADownload%2Freport.pdf` → `report.pdf`），
相册那种只有数字 id 的按**文件头嗅探**补类型（jpg/png/gif/webp/bmp/pdf/zip，其余 bin），
所以"从相册选图片"仍会作为**图片**消息发出；名字一律消毒（阻断路径穿越）。

**护栏**：三个纯函数单测（URI 解名 / 内容嗅探 / 消毒）+ 前端接线守卫（必须先 `api.importPickedFile`）。
验证：`cargo test --lib` **392/0**、`npm test` **337/0**、`vue-tsc` 0、`check-mobile.sh` PASS/0 warning。
⚠️ 仍需真机确认：相册图片 + 下载里的文档各发一次；"另存为/打开收到的文件"是同一个选择器的**保存方向**，
可能需要同样处理（下一步）。

### Fixed (好友申请：双方互加后，那条申请还挂在「新朋友」里)
用户 2026-09-12 真机实测：「如果两个人已经互相加上好友了（可能双方都给对方发送了加好友申请），
其中一个人点了确定，另一个人点进『新朋友』列表……如果该好友已在好友列表的话，那条好友申请
就应该自动清除掉」。

**根因**：同一件事（同意好友 ⇒ 忘掉这条申请）在**两条路径**上行为不一致 ——
跨跳路径 `GossipKind::FriendAccept` 清了 `pending_requests`，而**直连路径
`Message::FriendAccept` 只加了好友、忘了清**。于是"有时候会清、有时候不清"，
全看这条回执走的是哪条路（同一局域网内直连时必现）。

**修法（三层，缺一层都可能再漏）**：
1. **路径统一**：抽出 `transport::forget_pending_request(state, peer)`，直连 / 跨跳 /
   `respond_friend_request` 的同意路径**全部**走它（同一个助手，不可能再各写一遍）。
2. **兜底判据**：`get_pending_requests` 按 friends 表过滤并顺手收敛内存态
   （`is_actionable_request`：人已经是好友 ⇒ 申请不再"待处理"）。判据抽成**纯函数并有单测** ——
   它原先散落在各条路径里，正是漏清的原因。
3. **前端按事实过滤**：`chat.pendingRequests` 改为 computed，用新的纯函数
   `actionableRequests(原始列表, 好友 id 集合)` 过滤。这样**四个读它的地方**
   （会话列表红点、通讯录「新的朋友」、窄导航徽标、添加好友页的「同意/拒绝」行）一处生效，
   而且无论这条申请是"我同意的 / 对方同意的 / 重启后重新拉取的 / 对方走别的消息把我加上的"，
   只要 `friends` 里有这个人，那一行就立刻消失 —— 不依赖某条回执有没有送达。

**护栏（都做过非空转验证，`verify-guards.py` 现 **27** 条）**：
新增 `src/utils/friendRequests.test.ts`（纯函数 4 例 + "store 必须走它"的接线守卫）、
`commands::tests::pending_request_from_an_existing_friend_is_not_actionable`、
Rust 源码规则 `every_friend_accept_path_forgets_the_pending_request`（两条路径各一次，少一条即 FAIL）
与 `pending_requests_exclude_existing_friends`。

验证：`cargo test --lib` **389 / 0**；`npm test` **334 / 0**；`vue-tsc` 0；`vite build` 通过；
`cargo check --all-targets` 0 warning；`check-mobile.sh` PASS / 0 warning。

### Fixed (安卓真机实测四处：触屏定位 / 通道不同步 / 新的朋友点不开 / 蓝牙要手动开)
用户 2026-09-12 安卓实测报告：①「回到最新」按钮不在右下角；② 添加好友里的局域网开关与设置页的
不同步；③ 收到好友申请后点「新的朋友」打不开界面；④ 蓝牙通道打不开、且希望手机上默认就开着
（参考 BitChat：不用配对、不用配置、进去就能连）。

- **① 「回到最新」按钮在触屏上掉出右下角** —— 根因是 **CSS 特异性**：按钮写的是
  `tap-safe absolute bottom-4 right-5`，而 `src/style.css` 在 `@tailwind utilities` **之后**、
  `@media (pointer: coarse)` 里的 `.tap-safe { position: relative }` 与 Tailwind 的 `.absolute`
  **特异性相同** ⇒ 触屏设备上 position 被改成 `relative`，按钮回到文档流。
  桌面 `pointer: fine` 不走这条媒体查询，所以**只有安卓/触屏复现**（这就是它看起来像随机 bug 的原因）。
  修法：把 `.tap-safe` 的规则包进 `:where()`（特异性 0），任何定位工具类都能正常生效；
  并给 `checkStyleCascade` 加了第 ③ 条级联判据（该块内声明 `position` 的选择器必须是 `:where(...)`），
  配一个复现用的坏样例单测 —— 面向未来：以后往这个块里加任何"只扩大命中区"的类都不会再压掉别人。
- **② 局域网开关两处不同步** —— 同一个概念有两份前端状态：`channels[lan].enabled`（后端真实运行
  状态）与 `app.online`（另一份快照）。「添加好友」页改的是前者，设置页显示的却是后者，
  而且没人去刷新它。修法：两处 UI 一律走 `app.setChannelEnabled`，且**设置页的开关值也改用通道
  状态**；store 的 `setChannelEnabled` 现在**同时**刷新 `refreshChannels()` 与
  `refreshNetworkStatus()`（`online`/`boundIp`）。新增 `channelState.test.ts` 把这三条路径钉住。
- **③ 安卓点「新的朋友」没反应** —— 申请页渲染在**右侧主面板**里，而移动端靠 `mobileView` 平移
  切换面板；`openRequests()` 没切过去，用户还停在会话列表上，于是"点了像没反应"。
  修法：移动端打开申请页时 `mobileView = "chat"`，关闭时回到 `"list"`（与 `openFriendProfile` /
  `openSearchHistory` 同一处理）。
- **④ 蓝牙：状态是假的 + 手机上要手动开** ——
  - **假状态**：`get_channel_status` 里的蓝牙 `running`/`peers` 取自 `TransportManager` 中那个
    "尚未接线"的占位 `BluetoothTransport`（running 恒 `false`、peers 恒 0）⇒ 界面永远显示未运行，
    用户点了开关也看不出变化。现在改为读**真实运行时**（`network::ble::runtime_state`），
    并且 **起不来就显示为关**（不再是"偏好写了就算开"，用户能再点一次重试）。
  - **手机默认开启、零配置**：新增 `db::get_bt_enabled`（缺省值 = `cfg!(mobile)`，
    与 `get_lan_enabled` 一样立刻持久化；桌面维持默认关，不悄悄开射频），
    且移动端启动拿到「附近的设备」权限后会自动把蓝牙通道真正拉起来（失败退避重试一次，
    仍失败则交给「添加好友」页的就地开关，那里有明确的失败原因与权限指引）。
- **顺手把「找不到设备」变成可自诊**：「添加好友」的空态现在直接说事实 ——
  两条通道都关 / 局域网在跑但没人应答（附"同网段、别开 VPN/访客网络"）/ 蓝牙正在扫描（首次要等几秒，
  列表会自己刷新）；蓝牙直连的节点（无 IP，来自双向 Hello 验签）显示「蓝牙直连」而不是留一行空白；
  通道开关改用开关给出的**目标值**（原先用 `!ch.enabled` 取反，状态过期时会反向操作）。

**仍未验证（需要真机环境）**：局域网在你们网络下到底能不能发现（组播是否被 AP 拦、是否同一网段）、
蓝牙射频行为（能否扫到/连上/握手）、以及手机默认开启后的实际观感。这些只能由你在设备上确认；
出问题时「运行日志」里会有 `ble`/`discovery` 通道的记录（`who_has_sent`、`+ble-link` 等）。

### Fixed (桌面独立窗口：慢、会闪成聊天界面、连点会开出第二个 —— 架构上把三个窗口彻底分开)
用户实测三连：「点设置/日志，窗口出来得很慢，像卡了一下」「第二次打开设置，窗口会先刷成主聊天
窗口、再立马变成设置界面」「按钮没防抖，连点几下不该开出第二个，它应该还是那一个」。

**根因（读代码确认，不是一个 bug 而是三个叠在一起）**：
1. **三个窗口共用一个 `index.html` + 一个 Vue 应用**，由 `App.vue` 按窗口 label 决定渲染哪一屏。
   它的模板是 `…v-else-if="isSettingsWindow && settingsReady"` / `v-else` ⇒ 设置窗口在
   "数据就绪之前"的窗口期**落到了 `v-else`，也就是把整棵聊天三栏布局挂了起来** ——
   这就是"先闪成主聊天窗口"。而且每次打开都要白等一整棵聊天组件树（dev 下是几百个模块请求）。
2. **窗口关闭即销毁**：每次打开都要重建 WebView + 重新加载前端 + 重跑 `app.init()`。
3. **并发打开没有串行**：`WebviewWindowBuilder::build()` 的重复 label 检查在
   `tauri/src/manager/window.rs::prepare_window` 里做，而窗口被登记进 manager 是在主线程创建
   **完成之后** —— 两个并发调用（连点）会双双通过检查，后者还会覆盖 manager 的记录。
   前端也只有 `void api.openSettingsWindow()`，没有单飞/防抖。

**修法（一次做干净，不留分支与拷贝）**：
- **一个窗口一个文档 + 一个入口**：`index.html` → `src/entries/main.ts`（聊天）、
  `settings.html` → `src/entries/settings.ts`、`logs.html` → `src/entries/logs.ts`。
  设置/日志窗口**从第一帧到结束都不会碰到聊天代码**（构建产物实测：`settings.html`
  不再引用 `assets/main-*.js`）。Rust 侧 `WebviewUrl::App("settings.html"|"logs.html")`，
  不再注入 `__GOSSLAN_WINDOW__`。
  实测收益：主入口 bundle **458KB → 310KB**，设置窗口只额外加载 3.25KB 的入口 chunk。
- **共用启动逻辑抽成一份**：`src/boot/boot.ts`（错误上报、骨架撤除、标题、装配、挂载顺序）
  + `src/boot/theme-boot.js`（首帧主题/语言/平台）+ `src/boot/skeleton.css`（骨架样式），
  后两者由 `vite.config.ts` 的 `gosslan:inline-boot` 插件内联进三个 HTML —— **三份拷贝变一份事实来源**。
  每个窗口的骨架写在各自的 HTML 里（设置窗口只有设置骨架），标题由各自的
  `data-title-zh/en` 声明（Tauri 会把 document title 同步到窗口标题，Rust 不再维护第二份文案）。
- **`App.vue` 只服务主窗口**：窗口 label 分支、`settingsReady` 占位 hack 全部删除 ——
  "设置窗口渲染成聊天界面"这个 bug 在结构上不可能再发生。
- **后端单例 + 串行创建**：新增 `ensure_aux_window`（快路径 show+focus；慢路径拿
  `AUX_WINDOW_CREATE_LOCK` 后**再查一次**才 build），所有独立窗口都必须走它。
- **关闭即隐藏（常驻）**：`install_hide_on_close` 把标题栏 × 与 `close_*_window` 都拦成
  `hide()` ⇒ 第二次起打开是 `show()`，也就是用户要的"点一下立马就开"。
  开关是 `commands.rs` 里的 `AUX_WINDOWS_RESIDENT`（改成 `false` 即回到关闭销毁）。
- **前端单飞 + 防抖 + pending 反馈**：新增 `src/utils/windowLaunch.ts`（纯判据）
  与 `src/composables/useWindowLauncher.ts`（模块级单例状态，窄导航 / 移动端底栏 / 原生菜单
  三处共用）；按钮在打开期间显示 `aria-busy` + 半透明，冷启动那一下用户能立刻看到"点到了"。

**护栏（都是主机可跑，且逐条做过非空转验证）**：
- `aux_windows_open_their_own_document`：Rust 里每个 `WebviewUrl::App(...)` 目标文件必须存在、
  必须指向自己的入口，且**独立窗口不得再共用 `index.html`**；
- `aux_window_open_is_singleton_serialized_and_resident`：打开命令必须走 `ensure_aux_window`、
  不得自己查窗口存在性，helper 必须双重检查 + 接上 hide-on-close；
- 前端 `windowEntries.test.ts`：三个 HTML ↔ 三个入口 ↔ 三套骨架一一对应（设置/日志不得带
  聊天骨架、不得 import 聊天代码），`App.vue` 不得再有 label 分支；
- 前端 `windowLaunch.test.ts`：单飞/防抖判据 + "两个开窗按钮必须走 `launchAuxWindow`"接线守卫。
- `scripts/verify-guards.py`：`--only window` 4 条新用例（现共 **19 条**）。

### Fixed (Android **release** 包：三个"只有 release 才现形"的问题 —— 之前那份包是装不上 / 蓝牙会废的)
- **① R8 把 Rust 按名字调用的 Kotlin 方法改名了**：`isMinifyEnabled = true` 时，`BlePeripheral` 的
  `stop/start/send/isConnected/payloadMtu/requestAllPermissions/hasRequiredPermissions` 全被改名成
  `a/b/c/d/e`（`dexdump` 实测），而 JNI 只按「名字 + 签名」查找 ⇒ release 真机包上**蓝牙外设整条
  路径会在运行期 `NoSuchMethodError`**；debug 包不混淆，所以开发期完全看不见。
  修法：新增 `scripts/android/proguard-gosslan.pro`（版本库里的单一事实来源），构建前由
  `inject-android-signing.mjs` 注入 `app/proguard-rules.pro`；**新增主机可跑护栏
  `release_keeps_every_kotlin_method_called_from_rust`** —— 规则漏方法 / 多留废弃方法 / 两处规则漂移
  三种漂移都会 FAIL，且已逐条做非空转验证（删 `send` 行 → FAIL 并指名；塞 `legacyMethodGone` → FAIL；
  恢复 → PASS）。修完实测 `dexdump`：7 个名字全部保留。
- **② release 包根本没有签名**（真机上是"应用未安装"）：`app/build.gradle.kts` 里没有任何
  signingConfig，AGP 对 release 产出的就是未签名 APK —— 而发布脚本此前**没有**跑
  `inject-android-signing.mjs`（只有旧的 `android:build` 跑了）。修法：脚本在构建前强制注入
  （有 `ANDROID_KEYSTORE_BASE64` 用真 keystore，否则回退 debug 签名保证内测可装），并在打包后
  **硬校验** `apksigner verify` + 包里只有目标 ABI 的 `.so`，不通过就整条构建红掉。
- **③ GitHub Actions 会把上面的坑原样发出去**：CI 只装 `platforms;android-34`，而 `tauri android init`
  生成的工程是 `compileSdk = 36`（必然失败）；且 CI 走同一个发布脚本（同样没注入签名/清单）。
  修法：CI 装 `platforms;android-36` + `build-tools;36.0.0` + `ndk;27.1.12297006`（与本机验证过的一致）、
  JDK 升 21、删掉重复的 python 权限注入（统一由注入脚本负责）、产物连 `.sha256` 一起上传/发布。

### Changed (Android 出包链路：产物位置、命名、校验)
- 产物从 `dist/android/` 改到 **`release-artifacts/android/`**：安卓构建会先跑 `vite build`，而它会
  **清空 `dist/`**（第一版就踩过：`mkdir` 完紧接着被删，`cp` 报 "No such file or directory"）。
- 文件名带构建类型：`gosslan-<版本>-<abi>-<release|debug>.apk`（此前 release/debug 同名，分不清手上
  装的是哪一份；实测 release **12MB** vs debug **216MB**），并在旁边生成同名 `.sha256`。
- 只认**本次构建新产出**的 APK（marker 时间戳 + `find -print -quit`），不再 `ls -t | head -1` 去赌
  构建目录里没有残留的 universal / 另一个 ABI 的旧包。
- 新增三条产物校验（都在出包脚本里，失败即整条构建红掉）：
  - **包内前端 = 当前 `dist`**：前端资源是被嵌进 `libgosslan_lib.so` 的，所以"前端改了但 Rust
    没重编"在产物层面完全看不出来 —— 用 bundle 的内容哈希文件名在 `.so` 里搜一遍（分块搜，debug
    的 `.so` 有 200MB+），对不上就报"包里会是旧界面"。
  - Gradle 判定"输入内容未变"而跳过打包时**允许复用**已有产物，但会打印一行说明并照常做上面的校验
    （实测：只改前端压缩配置、`.so` 内容一致时 `packageRelease` 是 UP-TO-DATE，APK 的 mtime 不变；
    旧写法会误报"本次构建没有新产出 APK"）。
  - **体积异常自检**：Gradle 增量打包偶尔在 APK 里留下**未被中央目录引用**的旧数据
    （实测 debug 包 226MB → **444MB**，多出的 218MB 是上一版 `.so` 残骸，能装但白胖一倍）→ 超过
    5MB 就警告并给出处置办法（删 `build/outputs/apk` 后重打）。
- 支持只出单个 ABI：`bash scripts/build-android-releases.sh --abi arm64-v8a`
  （或 `npm run android:build:test -- --abi arm64-v8a`）。
- 构建会**改脏工作区**的问题一并解决：`MainActivity.kt`（运行时权限申请）、`AndroidManifest.xml`
  （竖屏 + 权限清单）、`build.gradle.kts`（release 签名）与 `proguard-rules.pro`（R8 keep）的注入结果
  都落到版本库；注入脚本保持幂等，专门兜底 `tauri android init` 重生工程之后的 CI / 新机器。

### Changed (打包策略：按架构分别出包，不再打 universal)
- **Android 按 ABI 出两份包**（GitHub 发布就挂这两份）：`arm64-v8a` 给现代手机、`armeabi-v7a` 给老设备。新增 `scripts/build-android-releases.sh`（`npm run android:build:test` / `android:build:release`），对每个 ABI 各跑一次 `tauri android build --target <abi>`，产物按 ABI 改名落到 `release-artifacts/android/`。
  - universal 包把两/四份 `.so` 拼在一起，而真机只用到一份（实测 universal debug **423MB**）；单 ABI 包体积约为其 1/3。
  - ⚠️ **不用** Gradle 的 `splits.abi`：Tauri 的 Android 插件会给每个 ABI 设 `ndk.abiFilters`，AGP 禁止两者并存，配置阶段直接失败（`Conflicting configuration … in ndk abiFilters cannot be present when splits abi filters are set`）。
- **macOS 分架构出包**：`npm run dist:mac`（`aarch64-apple-darwin`，Apple 芯片 —— 日常开发/自测/打包都用它）与 `npm run dist:mac:intel`（`x86_64-apple-darwin`，发布给老 Intel Mac 时才需要）。**不打 universal**（会把两份二进制拼起来，体积翻倍）。
- **Windows 不需要拆**：`npm run dist:win`（NSIS，x86_64）本来就小，维持现状。
- 测试口径：**一律用最新版本，不为旧版本做任何兼容**。

### Fixed (Android：关掉蓝牙开关后手机仍在广播 —— JNI 签名写错)
- **真实缺陷**（本轮 code review 抓到，`d2fad5e`）：Kotlin 的 `fun stop()` 是 **Unit** 方法（JNI 描述符 `()V`），Rust 侧却按 `()Z` 调用。**JNI 不做任何编译期检查** —— 这只会在运行期抛 `NoSuchMethodError`，且只有真机才现形：用户关掉「蓝牙通道」后手机**仍在广播**（耗电 + 隐私），日志里一个字都没有。
- 修法：`stop()` 改走 `()V` 的 void 调用，失败时**主动上报 Warning**（"停止 BLE 外设失败（可能仍在广播）"）；引入 `kotlin_method!("名字", "描述符")` 登记宏把两者写在一处；顺带修正一条**永远发不出来的日志**（桥就绪的 Notice 原先在 `bootstrap` 里发，而那时 events 通道还没建立，现改在 `start()` 里发）。
- **新增主机可跑护栏 `android_jni_signatures_match_kotlin`**：解析 `BlePeripheral.kt` 里 `fun` 的形参/返回类型推出 JNI 描述符，与 Rust 侧登记**逐字比对**；并检查每个 `extern fn` 在 Kotlin 里确有同名 `external fun`（否则 JVM 会 `UnsatisfiedLinkError`）。
- **非空转验证**：把 `stop` 改回 `()Z` → 护栏 FAIL 并给出具体差异；恢复 → 全绿。已加入 `scripts/verify-guards.py`（现覆盖 **10 条**护栏，一条命令全跑）。
- 验证：`cargo test --lib` **379 passed / 0 fail / 0 warning**；`cargo check --all-targets` 0 warning；Android `check-mobile.sh --bluetooth` PASS / 0 warning。
- ⚠️ 仍未验证：真机射频行为（需你的设备）。另外本轮确认了 E2E 两个前置能编出来（`cargo build` + `build --example e2e_peer`），但**没有替你跑 `scripts/e2e-dev.sh`** —— 它会真的启动 GUI 实例（在你桌面上弹窗口），该由你决定何时跑（手册 §0.5）。

### Added (Android 外设角色的 Rust↔Kotlin 桥 —— 7-f 完成，手机也能"被连"了)
- **Rust 侧 JNI 桥**（`transport/ble_android.rs`）：缓存 `JavaVM` 与 Kotlin 类的全局引用、把 Kotlin 回调上来的**分片**重组成整帧（复用 `ble_framing`，与 macOS 同一份实现）、把网络层要发的帧按 MTU 分片后调 Kotlin 的 `send`。
  - **`bootstrap` 的鸡生蛋问题**：JNI 的 `FindClass` 依赖"调用方的类加载器"，从 tokio 线程里找不到 App 的类 ⇒ `MainActivity.onCreate` 调一次 `BlePeripheral.bootstrap(context)`，由它在 App 代码还在栈上时把 `JavaVM` + 类引用交给 Rust。
  - **符号 + 注册双保险**：`native_method!` 的 `extern` 直接导出 JNI 符号（`bootstrap` 靠名字解析），其余四个再 `register_native_methods` 显式注册 —— 签名写错会**当场**以 `NoSuchMethodError` 暴露，而不是真机收发时静默失效。
  - **没开 feature 必须安全**：Kotlin 用 `try/catch UnsatisfiedLinkError` 包住 `nativeBootstrap()`，否则默认构建会在启动路径崩溃。
- **接口同形**：`ble_android.rs` 的 `start/stop/PeripheralServer/PeripheralWriter/PeripheralEvent` 与 macOS 版逐一对应 ⇒ `network/ble.rs` 的事件循环/握手/路由/读写循环**两平台共用一份**，只有 import 按平台切换。依赖只加了 `jni = "0.22"`（optional，Android 专属；btleplug 的 droidplug 本来就用同一个版本，**没有引入新的第三方 crate**）。
- 验证：`cargo test --lib` 378 / 0 warning；`cargo check --all-targets` 0 warning；**Android target `--features bluetooth` 0 warning**；`--features bluetooth` 的完整 APK 构建通过（JNI 符号链接进 `.so`、Kotlin 一并编译）。
- ⚠️ **仍未验证**：真机上的广播/连接/GATT 读写（需用户设备）；iOS 侧同类实现（`CBPeripheralManager`，与 macOS 同款代码）尚未接；Windows 做外设（WinRT `GattServiceProvider`）未做。

### Fixed (🔴 卡死：61 个命令仍在 macOS 主线程上跑 —— 清除数据/恢复/添加好友时整个应用冻住)
- **用户反馈**：「点设置里的清除数据或恢复，整个设置窗口就卡死；点加号 → 添加好友，主窗口卡死」，并重申**渲染与响应速度高于一切**（`6324d05`）。
- **根因（读上游源码确认）**：`tauri-macros` 的 `body_blocking` 把**同步**命令**内联调用**在 IPC 处理器里，只有 `ExecutionContext::Async` 才走 `respond_async_serialized` → `async_runtime::spawn`；而 wry 的 IPC 回调跑在 **AppKit 消息循环（macOS 主线程）**。所以同步命令 = 在 UI 主线程执行，**卡的是整个进程、所有窗口**。而「清除数据」是一次长事务（全表 `DELETE`，可能数秒）并一直握着 `db` 互斥锁 ⇒ **长事务持锁 → 同步读在主线程等锁 → 全部窗口冻住**。`clear_all_data` 本身早就是 async，但它的**读者不是**（`get_settings`/`get_friends`/`get_pending_requests`/`get_transfers`/`get_logs`/`list_interfaces`/`reset_settings`/`open_settings_window`…）。
- **修法**：把所有会碰重资源的命令改成 `#[tauri::command(async)]` / `async fn`，共 **61 个**（数据库、文件系统、日志、剪贴板、网卡枚举、阻塞睡眠）；只保留纯窗口操作（minimize/maximize/fullscreen/close/圆角）同步。其中 19 个（`send_message`/`mark_read`/`send_file`/`respond_friend_request`/`start_network`/`stop_network`/`set_channel_enabled`…）是**被新守卫逼出来的**，全是高频路径。
- **守卫从"名字清单"改成"规则"**：上一轮的 `heavy_commands_run_off_the_main_thread` 是名字清单，只能盯住写清单时的 12 个 —— 正因为如此这 61 个才漏了过去。新增 `blocking_commands_run_off_the_main_thread` 直接解析 `commands.rs`，逐个命令取函数体（手写扫描跳过字符串/注释/生命周期，避免 `format!("{}")` 造成花括号错配），碰标记即要求 off-main-thread，**一次报出全部违规并附原因**。
- **非空转验证**：去掉真实命令 `get_settings` 的 `(async)` → 守卫 FAIL 并点名；恢复后全绿、无残留。
- 验证：`cargo test --lib` 378 / 0 warning；`--features bluetooth` 385 / 0 warning；`cargo check --all-targets` 0 warning；Android `check-mobile.sh --bluetooth` PASS / 0 warning。前端无需改动（命令名与返回类型未变，async 对 `invoke` 透明）。
  ⚠️ 说明：清除大量数据时**读操作会排在长事务后面**（界面保持可交互，数据在操作完成后刷新）—— 这是刻意的原子性取舍（全清或全不清），不是卡顿。

### Fixed (设置窗口的头像与名字显示空白/默认值)
- **根因是挂载与取数的顺序**（`9d4d055`）：子组件在 `setup` 阶段就把 store 的值**快照**进 ref（`ProfileSection` 的 `watch(..., { immediate: true })` 读 `app.device`），而 `app.init()` 是在 `App.vue` 的 `onMounted` 里才 `await` 的 —— **先挂载、后拿数据**。主窗口看不出问题（设置分区打开时才挂载），独立设置窗口一开场就把**空昵称 / null 头像**写进了 ref，数据到位后没人再同步，于是头像与名字一直是空白/默认。
- 修法两处互补：① `App.vue` 新增 `settingsReady`，独立设置窗口的内容**等 `app.init()` 完成后再挂载**（这一处同时修掉外观/语言/网络/存储等分区的同类问题；等待期间显示窗口自己的首屏骨架）；② `ProfileSection` 补对 `app.device.nickname/avatar` 的 watch，覆盖**运行中**的变更（「恢复默认」会把昵称恢复默认、头像清空并广播）。用户正在输入时 `device` 不变，故不会覆盖未保存的编辑。
- 验证：`npm test` 291 / 0 fail；`vue-tsc` 0 错误；`vite build` 通过。

### Added (Android 外设角色的 Kotlin 侧 —— 手机也能"被连"了，7-f 第一步)
- **新增 `BlePeripheral.kt`**（`gen/android/app/src/main/java/com/gosslan/app/`）：`BluetoothLeAdvertiser` + `BluetoothGattServer` 的完整实现（`e66fd8b`）。只做 central 的手机**永远不可能被发现**（btleplug 只能主动连，ADR-0015 §3.1）；手机做了外设之后 `Windows(central) ──BLE──▶ 手机(peripheral)` 才成立，手机与 Windows 之间不必再经 Mac 中转。
- 与 macOS 实现（`bluetooth_peripheral.rs`）**行为契约一致**：同一套 UUID、同样"广播里只放服务 UUID"、同样的写/通知语义与 native 回调（frame / unlinked / notice / warning）。两处**有意的差异**：① Android 的 `onConnectionStateChange` 会**真的**告诉我们对端断开（CoreBluetooth 外设角色没有这个回调）；② 必须显式给 TX 挂 **CCCD 描述符**，客户端才能开启通知（CoreBluetooth 隐式处理）。
- 🔴 **补 `BLUETOOTH_ADVERTISE` 权限**（真实缺口）：Android 12+ 把蓝牙拆成 SCAN / CONNECT / **ADVERTISE** 三个运行时权限，缺 ADVERTISE 时 `startAdvertising` 直接抛 `SecurityException` —— 现象正是"手机能扫别人、别人永远发现不了手机"。Kotlin 侧在用户打开「蓝牙通道」时申请并给出可操作提示。
- **本机真的把 APK 建出来了**（不只 `cargo check`）：`ANDROID_USER_HOME=<workspace>/target/android-home npm run android:build:debug`（沙盒不能写 `~/.android`，重定向后 Gradle 8.14 + AGP 出包）。第一次构建**抓到一处 Kotlin 编译错误**（API 33 的 `notifyCharacteristicChanged` 返回状态码 `Int`，与旧重载的 `Boolean` 不同 → 两分支类型不一致），已修；`aapt2 dump permissions` 确认最终 APK 含 `BLUETOOTH_ADVERTISE` / `SCAN` / `CONNECT`。
- ⚠️ **现状**：Kotlin 侧就绪并通过编译，**Rust 侧 JNI 桥接尚未实现**（注册 native 回调 + 调用 `start`/`send`），因此本类在真机上还不会被触发；真机广播/连接/GATT 读写与 iOS 侧同类实现均待做。

### Fixed (BLE 外设：蓝牙被关掉/广播失败不再静默)
- 两处"看代码看不出来"的静默故障（`81606cd`，自查上一轮落地的外设代码时发现）：
  1. **系统蓝牙被关 / 权限被撤时订阅状态会一直是旧的**：CoreBluetooth 会清空本地 GATT 数据库并断开所有 central，但**不会**回调 `didUnsubscribeFromCharacteristic:` ⇒ `is_subscribed` 仍返回 true，写任务要等 `updateValue` 失败（**最长 8s**）才收尾，而且**日志里一个字都没有**。现在：离开 `PoweredOn` 即作废全部订阅与半截消息、唤醒等待中的写任务，并为每个已订阅的 central 各发一条 `Unlinked`，让网络层**立刻**拆链路。
  2. **广播启动失败只在首次状态回调时才会被报出来**：那个 oneshot 在第一次 `peripheralManagerDidUpdateState:` 里就被 `take()`，之后（如用户关掉蓝牙再打开、重新广播失败）错误**被静默丢弃** —— 现象正是"蓝牙开着却没人能发现我们"，与手册排障表里"看日志"完全对不上。现在改走常驻的 `events` 通道，**每次**都记 `logger.warn` 并给出可操作建议。
- 同时：回到 `PoweredOn` 时**重新** `addService` + `startAdvertising`（CoreBluetooth 清过库，不重新发布就再也不会有人能连上我们）；新增纯函数 `state_label`（"蓝牙已关闭" vs "未授权（去系统设置…）"，两者的处理建议完全不同）与 `detach_targets`（离开 `PoweredOn` ⇒ 全部订阅视为失效）；`did_unsubscribe` 拆开两层锁。
- 验证：`cargo test --lib --features bluetooth` **385 passed / 0 fail / 0 warning**（+2 护栏）；`cargo test --lib` 378 / 0 warning；`cargo check --all-targets` 0 warning；Android `check-mobile.sh --bluetooth` PASS / 0 warning；前端 291 / vue-tsc 0 / vite build 通过。
- **非空转验证**：把 `detach_targets` 改成永远返回空 ⇒ 新护栏 FAIL；恢复后全绿、无残留标记。
  ⚠️ 真机上"关蓝牙 → 链路立刻消失且日志里有原因"仍需实测（本机无法触发 CoreBluetooth 状态切换）。

### Fixed (8 处点按目标 < 44pt 的触屏隐患 + 护栏 ⑧)
- iOS HIG 的最小点按目标是 **44×44pt**，而项目里图标按钮普遍 24–32px（桌面鼠标没问题，**手指容易点不中甚至误触相邻项**）。项目早有 `.tap-safe`（`:pointer: coarse` 下把热区垂直撑 +16px），但靠自觉使用；本轮实测 35 个小尺寸可交互元素里**仍有 8 处漏网**（`54123e0`）：
  - `FriendProfile` 移动端**返回键**（32×32，手机上最主要的返回入口）；
  - `NetworkSection` 删除「跨网段端点」（28×28，破坏性操作且与整行相邻）；
  - `AppearanceSection` 自定义主题**取色控件**（24×28）；
  - `LogViewer` 4 个工具按钮 + `SettingsWindow` 折叠项（桌面为主，但 Windows 触屏笔记本属粗指针）。
- **新增静态护栏 ⑧ `findSmallTapTargets`**：可交互元素（原生可交互标签或带 `@click`）+ `h-5..h-8`/`w-5..w-8` + 无 `tap-safe` → 报出；提示里写清能力边界（`tap-safe` 只补垂直 ±8px ⇒ h-7→44、h-8→48；`h-5`→36 仍不达标，必须调大）。逃生阀 `tap-target-ok`。
- **非空转验证**：去掉真实移动端返回键的 `tap-safe` → 全库扫描用例 FAIL 并精确指到 `FriendProfile.vue:50`；恢复后全绿。
- 至此触屏/键盘三件套齐备：⑥ 能点（键盘够得着）+ ⑦ 看得见（焦点环不被静默覆盖）+ ⑧ 点得中（≥44pt）。
- 验证：`npm test` **291 passed / 0 fail**（284 → 291）；`vue-tsc` 0 错误；`vite build` 通过；后端未改。
  ⚠️ `.tap-safe` 的实际手感（热区够不够、会不会与相邻按钮重叠）只能在真机触屏上确认。

### Fixed (7 处输入框的焦点环被 `outline-none` 静默盖掉 + 护栏 ⑦)
- **全局焦点环其实一条都没生效**：`style.css` 的焦点环写在 `:where(button, a, input, textarea, select, [tabindex], [contenteditable]):focus-visible` 里，而 `:where()` 让整条选择器**特异性变成 0**；Tailwind 的 `.outline-none`（`outline: 2px solid transparent; outline-offset: 2px`）是 0,1,0 ⇒ **只要元素带 `outline-none`，焦点环必定被覆盖**（透明 2px = 看不见）。源码注释里"必须包含 `[contenteditable]`，否则消息输入框看不到焦点"的**本意是对的，但因为特异性加了也不生效**（`754167b`）。
- **修掉 7 处**（全是 `outline-none` 且无替代指示）：`MessageComposer`（最高频的消息输入框）、`ConversationList` 搜索、`LogViewer` 搜索、`ChatSearchDialog` 搜索、`GroupCreateModal` 群名、`RenameGroupModal` 群名、`AddFriendModal` 搜索 —— 统一**删掉 `outline-none`**，让项目本来就设计好的全局焦点环生效。
- **两处合法例外显式声明**：`ContextMenu`（`role="menu" tabindex="-1"`）与 `ImageLightbox`（`role="dialog" tabindex="-1"`）的**容器**，焦点由内部条目承担，给弹出菜单/全屏遮罩画环只会变噪声 ⇒ 加 `focus-ring-ok` 文件级逃生阀并写明理由。（`style.css` 的 `.gosslan-select` 同为 `outline: none`，但自带 `.gosslan-select:focus { border-color }` 替代指示，合规。）
- **新增静态护栏 ⑦ `findOutlineNoneWithoutFocusRing`**：带 `outline-none` 的开标签必须自带 `focus:` / `focus-visible:` 的 `ring|border|outline|bg|shadow` 之一。
- **非空转验证**：把 `outline-none` 加回真实的消息输入框 → 全库扫描用例 FAIL 并精确指到 `MessageComposer.vue:598`；移除后全绿。
- 验证：`npm test` **284 passed / 0 fail**（278 → 284）；`vue-tsc` 0 错误；`vite build` 通过；后端未改。
  ⚠️ 桌面端点击这些输入框时会开始出现焦点环（`<input>` 聚焦即匹配 `:focus-visible`）—— 这是补上"设计好但没生效"的可见焦点，观感需真机确认（手册键盘验收那条已覆盖）。

### Fixed (5 处「能点但键盘够不着」的元素 + 静态护栏)
- **`div @click` = 只有鼠标/手指能用的按钮**：触屏能用、鼠标能用，但**键盘 Tab 不到、回车没反应**，读屏软件也只念成一段普通文本 —— 与"为 iOS 上架铺路 / 去网页感"直接冲突（原生控件天生带这些语义）。用静态扫描复核出 5 处真缺陷并全修（`16d8e91`）：
  - `GroupCreateModal` 好友选择行、`GroupMemberPanel` 可添加好友行 → 改**真按钮**（前者带 `aria-pressed` 开关语义）；
  - `MessageFileBubble`（点开文件）、`MessageImageBubble`（点开大图）→ 补 `:role`/`:tabindex`（**可用时才可聚焦**，避免把加载中的气泡做成"Tab 得到却点不动"的空按钮）+ 回车/空格处理；图片气泡另加 `aria-label`；
  - `AboutSection` 指纹（点击复制）→ 补 `role`/`tabindex`/键盘处理，**保留 `select-text`** 因此不换成 `<button>`。
  模态里的两行统一加 `type="button"`，不会误触发表单提交。
- **新增静态护栏 ⑥ `findTappableWithoutKeyboard`**：扫非交互标签上的**真实动作型** `@click`，要求同标签内有 `role` / `tabindex` / `@keydown|@keyup`（静态与 `:` 绑定都认）。两个**刻意排除**的写法（真实存在，非臆测）：`aria-hidden="true"` 的遮罩层（ActionSheet 的遮罩，Escape 由 Headless UI 的 Dialog 负责）、只有修饰符的 `@click.stop`（EmojiPicker 用来阻止冒泡，不是按钮）。
- **非空转验证**：往真实组件注入 `<div class="cursor-pointer" @click="…">` → 全库扫描用例 FAIL 并精确指到行号；移除后全绿、无残留标记。
- 验证：`npm test` **278 passed / 0 fail**（270 → 278）；`vue-tsc` 0 错误；`vite build` 通过；后端未改。
  ⚠️ 键盘可达性需真键盘走一遍（手册新增验收步骤）；静态护栏只能保证"语义补上了"，不能保证焦点顺序与视觉焦点环在所有页面都好看。

### Fixed (macOS 沙盒：用户选的目录重启后失访 —— 共享目录"变空"、收到的文件写不进去)
- **两处用户自选目录改用 security-scoped bookmark 保活**（`bce66f0` + `50cdc0d`）。App Sandbox 下用户在目录选择器里挑的目录，系统**只把访问权授予本次进程**；我们此前只把**路径字符串**存进数据库 ⇒ 重启后路径还在、权限没了：
  - **共享目录**：`read_dir` 失败 ⇒ 「共享目录」列表直接变空、对方拉不到文件；
  - **文件接收目录**：收到的文件**写不进去**（用户把接收目录改到自定义位置后重启即触发）。
  两者都**没有任何报错弹窗**，是最容易被误判成"网络问题"的一类故障。
- **修法**：存 security-scoped bookmark，启动时 `URLByResolvingBookmarkData:` 解析（**解析即隐式开始访问**，因此不再额外调 `startAccessingSecurityScopedResource` 免得引用计数只加不减），过期则用解析出来的 URL 续期并写回；**书签优先于数据库里的路径**（书签记的是"资源"，用户在 Finder 里移动/重命名目录后解析出的路径比旧路径更新），书签坏了则退回路径 —— 绝不能因为书签失效就让用户重选一次。
- **两级书签策略**：先试安全作用域书签，被拒则退回**普通书签**（不带沙盒授权，但能跟踪目录移动，且未沙盒环境里它就是完整可用的），两者都失败才退回"只存路径"。解析侧对称（选项必须与创建时一致）。这条兜底同时让本机的未沙盒测试二进制能**真实验证** objc2 调用姿势。
- 🔴 **补 `com.apple.security.files.bookmarks.app-scope`**：沙盒里缺这条权限，`NSURLBookmarkCreationWithSecurityScope` 会被系统拒绝 —— 也就是说"书签代码写了"在真机上依然不生效。⚠️ 已装旧版的 Mac 必须**重装**这一版。
- 新文件 `macos_bookmark.rs`（书签读写）、`user_dirs.rs`（两个目录共用的持久化 + 纯决策函数 `pick`）；`state.rs` 启动、`commands::set_share_dir` / `set_downloads_dir` 接线。
- 单测 +12：书签**真往返**（本机未沙盒也走通创建→解析并比对路径）、坏输入干净报错不 panic、目录被删后解析不得当成可用、端到端 `store → load` 两种模式、坏书签被清理且退回路径、书签优先/路径兜底/空路径拒绝、**两个目录互不串键**。
- **非空转验证**：① `pick` 改成"路径优先" → 2 条 FAIL；② 书签建失败时连路径也不落库 → 2 条 FAIL；③ `RECEIVE` 的键改成与 `SHARE` 相同 → 串键护栏 FAIL；恢复后全绿、无残留标记。
- 验证：`cargo test --lib` **378 passed / 0 failed / 0 warning**；`--features bluetooth` 383 passed / 0 warning；`cargo check --all-targets` 0 warning；`scripts/check-mobile.sh [--bluetooth]` Android 双 PASS / 0 warning；前端未改（npm 270 / vue-tsc 0 / vite build 通过）。
  ⚠️ 沙盒授权本身只能在**打包版**上验：手册 §4 顶部新增"选共享目录/接收目录 → ⌘Q 完全退出 → 重开 → 目录仍然可用"的验收步骤（`npm run tauri dev` 未沙盒，测不出区别）。

### Added (BLE 外设角色：手机不必与 Mac 同一 Wi-Fi 也能连入)
- **macOS 上新增 BLE peripheral（GATT server）角色**（`b317c27`）。`btleplug` 只能当 central（其 README 原文 "host/central mode only"），只能主动扫/连、**不能**被连 —— 所以只做 central 的 Mac 在蓝牙上永远不可被发现，"手机与电脑不在同一局域网也能加入"这条产品目标根本无法落地。现在 Mac 同时具备两种角色：

  ```text
  手机（Android/iOS，central）──BLE──▶ Mac（peripheral）──局域网──▶ Windows
  ```

  手机侧**零新增原生代码**（仍走 `btleplug` central 路径），Mac 负责把消息中继给同局域网的 PC。
- **线格式零改动**：广播里只放服务 UUID，特征 UUID、分片/重组、Hello 握手、验签、去重判据全部复用 central 侧那一套 —— 没有新增任何线上协议，因此不需要 ADR-0017 的能力门控。新增依赖 `objc2-core-bluetooth`（macOS target 专属 + optional，纳入 `bluetooth` feature），默认构建与其它平台完全不受影响（`cargo metadata`/Android `cargo check` 均已验证）。
- **`network/ble.rs` 里只多了一个"谁先连谁"的分支**：外设侧首帧就是对端的 Hello，验签通过后再回我们的 Hello。写/读循环泛型化为 `FrameSink`/`FrameSource` 两个私有 trait（`BleWriter`/`BleReader` 与新增的 `PeripheralSink`/`ChannelSource` 各实现一次），于是「取消息 → 序列化 → 发送 → 失败即收尾」与「收帧 → 解析 → `handle_message`」各**只有一份**实现，去重判据仍是同一个 `should_accept_inbound_public`。
- **三处必须写清的细节**（否则真机上必踩）：① BLE 外设角色**没有** "central 断开" 回调（只有取消订阅），旧链路可能早就死了而我们不知道 ⇒ 同一 BLE 端点的旧链路必须让位给新连接，否则该设备重连时会永远撞在去重判据上形成黑洞；② 路由先就位、再回 Hello（对端收到 Hello 会立刻冲刷待发队列，通道先挂上这批帧才不会被"注册还没完成"的缝隙吞掉）；③ `updateValue` 返回 `false`（对端接收窗口满）不是错误，等 `peripheralManagerIsReadyToUpdateSubscribers:` 再重试（8s 上限），未订阅时等订阅信号 —— 等待一律 `timeout + 50ms` 兜底，**绝不忙等**。
- **不阻断渲染**：delegate 回调走主队列，但回调里只做「拷字节 + 查表 + 发通道」，验签/写库/加解密全在 tokio 侧；CoreBluetooth 对象通过显式 `SendObj` 断言跨线程使用（依据 Apple 文档：manager 方法可从任意线程调用、回调串行派发到构造时给的队列），`Retained<NSData>` 等非 `Send` 对象一律在任何 `await` 之前析构，`start()` 全程同步（否则 future 会被染成 `!Send`，一路炸到 `#[tauri::command]`）。
- 单测 +3：广播净荷受 31 字节 legacy 上限约束且**故意不放**本地名；对端 `maximumUpdateValueLength` 异常值必须退回默认而**绝不返回 0**；用 clamp 出的 MTU 分片能被对端同一套重组器逐字节还原。**非空转验证**：把 MTU 下限判据改成 1 → FAIL；把 128 位 UUID 记成 16 字节 → FAIL；恢复后 PASS。
- 验证：`cargo test --lib --features bluetooth` **371 passed / 0 failed / 0 warning**；`cargo test --lib` 366 passed / 0 warning；`cargo check --all-targets` 干净；`bash scripts/check-mobile.sh --bluetooth` Android target **PASS / 0 warning**；前端未改（npm 270 / vue-tsc 0 / vite build 通过）。
  ⚠️ **射频行为未验证**（本机无第二台设备、无头运行会被 CoreBluetooth 授权弹窗挡住）：广播能否被手机发现、真实吞吐必须在真机跑，步骤见 `.workbuddy/audit/2026-09-12-真机测试手册.md` §5。手机/Windows 自己做外设（ADR-0015 的 7-f）仍未做，因此手机 ↔ Windows 之间只能经 Mac 中转。

### Fixed (macOS 沙盒缺蓝牙权限：开了开关却一个设备都发现不了)
- **`entitlements.plist` 补 `com.apple.security.device.bluetooth`，并显式声明 `bundle.macOS.infoPlist`**（`42c1108`）。两个都是"运行期才暴露、且现象具有误导性"的打包缺口：App Sandbox 下没有蓝牙权限时，CoreBluetooth 的 manager 状态会一直停在 Unauthorized —— **central（扫描/连接）也一起失效**，现象是"打开了蓝牙开关、一个设备也发现不了"，极易被误判成"对面没开蓝牙"；macOS 11+ 同样要求 Info.plist 里有 `NSBluetoothAlwaysUsageDescription`，之前只有 iOS 那一侧显式配置，现在 macOS 侧也显式指向同一个 `Info.plist`，不再依赖"自动探测同名文件"这种隐式行为。⚠️ 已装过旧版本的设备必须**重装**这一版才会带上新 entitlement。

### Changed (窄导航栏通讯录图标与选中态 + 输入框工具栏对齐)
- **通讯录图标换成 `Contact`**（用户反馈「最左侧那一栏通讯录的图标跟上面的聊天图标不像是一整套」）：原 `Users` 是「宽而扁」的双人剪影，与近正方形的聊天气泡并排时外接框与视觉重量都不一致；`Contact`（通讯录卡片）同为方形容器，两者并排才像一套。
- **导航栏选中态改为「图标 + 底色块」双通道**：原先只有图标变色、选中时还把图标填色（`fill=currentColor`）—— 填色对双人图标会变成两块墨团，且只靠颜色表达「现在在哪一栏」。现在用既有但从未被使用的 `--gosslan-rail-active`（浅色 `#cbd5e1` / 深色 `#253246`）作底色块 + `--gosslan-rail-text-active` 作图标色，图标**保持线性不填充**，与底部工具图标同一套描边语言。
- **导航栏上下两组按钮统一为 44px / 20px 图标**：中部导航原本 44px、底部工具原本 40px，两组点击热区与视觉重量不一致 → 统一为 44px（图标 20px、线宽 1.9）。
- **输入框工具栏三处修复**（用户反馈「底下工具栏这一行的内容，左右两边在视觉上不在同一条线上」「很像个网页」）：
  1. **垂直对齐**：行改 `h-7 items-center`，左右两组同高。原先发送键（`h-7 + px-4 + 13px` 文本）与图标按钮（28px 见方 + 18px 图标）是两种不同高度的行盒，`items-center` 居中两个不同行盒 ⇒ 看不出同一条中线。
  2. **两侧留白对称**：卡片 `px-3` → `px-4`，并在工具栏行加 `-mx-1`（4px）—— 编辑器文字左边缘与左侧第一个图标的**热区**边缘取同一起点，右侧发送键边缘与文字右边界对称，同时 28px 按钮的热区不越出卡片。
  3. **统一规格去"网页感"**：图标按钮 28×28 / 图标 16px / 线宽 1.75（原 18px + 默认 2，偏粗偏大）；按钮间距 8 网格（`gap-1.5`）；**发送键改为圆角实心主按钮**（`rounded-full` + `bg-primary` + 白字 + `font-medium`），无草稿时是低对比占位态。
- 验证：`npm test` **229 passed / 0 failed**；`npx vue-tsc --noEmit` 0 错误；`npx vite build` 通过。
  ⚠️ 均为观感变更，**需真机目视**（本机无头浏览器不可用）：重点看导航栏两组图标的整体感与选中态是否清楚、工具栏左右是否在同一中线、发送键有/无草稿两态。

### Changed (聊天气泡更紧凑 + 正文更清晰，含度量联动护栏)
- **气泡高度收敛、正文字重提高**（用户 2026-09-12 反馈：「气泡高度太高了，不如微信里的和谐；字重又太细了，一眼看上去不够清晰」）：文本气泡从 `px-3 py-2 leading-relaxed`（上下内边距 16px、行高 1.625）改为 `px-3 py-1.5 leading-normal`（12px、1.5），正文加 `font-medium`（500）。若隐若现的"太细"来自 400 字重在浅色画布上的笔画对比不足，500 提升辨识度又不会像 600 那样变成标题感。
- **同步虚拟列表高度度量**（关键，否则相邻消息会互相遮挡）：`previewMetrics.ts` 的 `TEXT_LINE_RATIO` 1.625 → 1.5、`TEXT_BUBBLE_PADDING` 16 → 12；`MessageItem` 里未知 kind 的兜底气泡同样改为 `py-1.5 leading-normal`（它与 `MessageTextBubble` 共用同一套高度常量）。
- **新增护栏 ⑤ `checkBubbleMetricsCoupling`**：把「组件真实排版」与「虚拟列表估算常量」这一对**必须成对演化**的值钉在一起 —— 解析气泡根元素的 `leading-*` / `py-*`，与度量文件里的 `TEXT_LINE_RATIO` / `TEXT_BUBBLE_PADDING` 交叉核对，不一致就报出并**给出应改的数值**。Tailwind 未覆盖 leadings（已确认 `tailwind.config.js` 只 extend 了 colors/fontFamily），故可静态判定。
- **非空转验证**：把 `TEXT_LINE_RATIO` 临时改回 1.625（marker 已删）→ 全库扫描用例**精确 FAIL**，恢复后 229 全绿。
- 验证：`npm test` **229 passed / 0 failed**（224 → 229）；`npx vue-tsc --noEmit` 0 错误。
  ⚠️ 属观感改动，**需真机目视**（本机无头浏览器不可用，见 §七之十二）：重点看长消息滚动时相邻气泡不遮挡、6 套配色下正文可读性。

### Fixed (M3-0b：连接健康拆开「读活性」与「写活性」)
- **半开 TCP 不再永久被判健康**（ADR-0014 §3.1 / §7 的前置补丁）。M3-0 的健康记录只有一个 `last_seen_ms`，**写成功与读成功写同一个字段**，而心跳每 5s 会给每条连接写成功一次 ⇒ 一条对端已消失、本机内核仍接受写入的链路会**永久保持「健康」**；选路若据此过滤，就会一直选中这条死路 —— 正是 ADR-0014 §7 列的失效场景。
- **拆成两个字段**：`last_write_seen_ms`（诊断口径，**不参与**判定）与 `last_read_seen_ms`（唯一「对端活着」的证据）。`is_healthy` 只看读活性 + 连续失败。`writer_loop` 写成功只刷写活性；`reader_loop` 每读到一帧刷读活性。
- **建链播种读活性**（`seed_read_seen`）：刚建好、尚未收到任何帧的连接必须算健康，否则 `should_dial` 会反复重拨（ADR-0014 §3.1 注意 ①）。播种是**一次性**的，超过阈值同样过期 —— 不是永久豁免。
- **行为零变化**：`online_state()` 在生产路径仍只用于 `[mesh] +conn` 的 `online=` 日志字段，没有任何决策读它，因此本补丁不影响现有收发。
- ⚠️ **阈值提醒（留给 M3-b）**：`PeerManager::new(10_000, 3)` 的读活性阈值是 10s，而双向心跳 5s 一次 ⇒ 容错仅一个心跳周期。今天无影响（只有日志在读），但 M3-b 一开始用健康信号做选路/在线判定，建议放宽到 ≥3 个心跳周期，否则抖动会被误判成链路故障并触发无谓换路（ADR-0014 §3.1 已记）。
- 单测 +5：**只写不读不算健康**（核心不变量）、写 19 次仍因读活性过期而判不健康、建链播种即健康且会过期、边界内仍健康、连续失败阈值；Peer 层再加 2 条（只写不读 → Offline、播种 → Online）。
- 验证：`cargo test --lib` **322 passed / 0 failed**（317 → 322）；`cargo check --all-targets` **0 warning**。

### Fixed (已读回执不再压小图片 + 选中会话可删除)
- **图片消息尺寸固定，已读头像不再把它压小**（用户 2026-09-12 反馈：「群聊里对方已读，后面会有一个已读列表和已读的小头像，那个头像会让图片稍微缩小一下……图片发出来之后，大小应该是固定的」）。根因：`MessageImageBubble` 的容器原先只有 `max-w-full`，**没有定宽** —— 图片宽度于是变成「父容器剩余宽度」的函数；而图片**没有内在宽度下限**，被压缩后不会回流，就永久变小。群聊已读回执（最多 3 个头像 + `+N`）是同一 flex 行的兄弟节点，因此**回执一出现就占宽、把图片压小**。改为容器**定宽 `w-52`（13rem，与加载骨架同宽）**：图片尺寸从此与兄弟节点无关，`max-w-full` 仅作为窄窗口下的安全下限保留；`img` 同宽 + 骨架 `w-full`，加载前后也不跳变。代价（已知并接受）：竖长图会在 13rem 框内留白，换「发出后尺寸恒定」。
- **选中的会话无法删除**（用户反馈：「选中的聊天框没法删除，自己应该是可以删除的」）。根因：删除按钮写成 `v-if="!active"` —— **选中态整个按钮不渲染**。改为**选中行常显**（`active ? 'flex' : 'hidden group-hover/conv:flex'`），并给它不透明底色 + 面板描边，避免压在摘要文字上糊在一起。删除后「顺位到下一条 / 无会话时兜底界面」**原本已由既有逻辑处理**：`deleteConversation` 会清空 `activeConv`，`ResponsiveLayout` 已有 watcher「`activeConv` 为空且列表非空 → 打开第一条」，列表为空时由 `ConversationList` 的空态提示接管。

### Fixed (群文件进度条改为「在线成员」口径)
- **在线成员都收到 = 100%，离线成员不再拖住进度条**（用户 2026-09-12 反馈：3 人群里 1 人离线，两个文件都发完却有一个「一直卡在 50%」）。此前发送方气泡进度取的是**全体 recipient 的 `max(progress)`**，离线成员永远停在 0 ⇒ 进度被永久冻在某个百分比；完成确认又把「有任意一人完成」直接写成进度 `1.0`，与离线成员数无关。
- **新口径**：进度 = **发送那一刻在线的成员**各自的字节进度的**平均**。分母在发送时**冻结**（`AppState::group_file_online_targets`），因此离线成员之后上线补发**不会**让进度条倒退（用户明确要求「补发不算在进度条里」）；发送时无人在线则恒为 0。
- **离线成员仍然可见、可补发**：气泡上的「已发送给 N 人 · M 人待上线」照旧按全体 recipient 统计，`flush_pending_group_files` 的离线补发链路一个字未改 —— 变的只是**进度条**这一项展示口径。
- 内核抽成**纯函数** `group_file_progress_from(recipients, online, fallback)`（+5 单测）：在线全到 → 1.0；**对照组**同一数据在全体口径下是 2/3（证明差异来自分母而非巧合）；离线者补发到 0.5 → 仍 1.0（不回退）；在线未全到 → 取平均；发送时无人在线 → 0（`fallback` 也被压到 0）；快照丢失 → 退回全体口径；`fallback` 越界夹紧。
- ⚠️ 调用顺序约束（已写进注释）：聚合函数**自己取 `state.db` 锁**，调用方必须在**未持有 db 锁**时调用（std Mutex 不可重入，注释里标了死锁原因）—— 完成确认路径为此显式 `drop(dbc)` 后再算。
- 验证：`cargo test --lib` **317 passed / 0 failed**（312 → 317）；`cargo check --all-targets` **0 warning**。
  ⚠️ E2E 未跑：本机会话的沙箱不允许写 `~/Library/Application Support/...`（`sqlite3` 打不开测试库），`scripts/e2e-dev.sh` 在第 3 步即报「数据库尚未初始化」。

### Fixed (截断文本补齐 hover title)
- **被 `truncate` 截断的文本补 `title`**（用户反馈「名字显示不下变成 `...`，鼠标悬停看不到完整名字」）：会话列表（名字 + 摘要）、好友列表（名字 + 在线态）、好友申请、群成员面板（成员名 + 可添加好友）、好友资料页（头部 + 大标题）、添加好友搜索结果、分享目录文件名、聊天头部标题、输入框 @候选、转发目标列表、文件气泡备注、已读成员列表、引用预览条、运行日志标题、路由端点地址、诊断面板事件。
- **新增静态护栏**（`designGuards` ④ `findTruncationWithoutTitle`）：全库扫描「带 `truncate` 类的真实 `class` 属性、同一开标签内既无 `title` / `:title` 也无 `aria-label` / `:aria-label`」的元素，精确报 `文件:行号`。这类缺陷**编译通过、测试全绿、代码看着正常**，只有真去 hover 才发现 —— 属最该由机器盯住的一类。支持跨行开标签；逃生阀为文件内 `truncate-title-ok` 注释（如父级已有整行 `aria-label` 且文案短到不可能截断）。**非空转验证**：截断但无 title → 报出；补 `:title`/`aria-label` → 通过；注释里提到 `title` → 仍报出（与 ③ 同源的假通过陷阱）；跨行 → 抓到；`truncate-title-ok` → 跳过。

### Fixed (跨网段中继稳定性)
- **定向 Gossip 帧到达目标后不再转发**：`handle_gossip` 第 4 步转发前新增 `is_target` 判定——`env.target == 本机` 时只消费不转发。此前目标节点会把自己是目标的定向帧（FriendRequest/FriendAccept 等）再洪泛给其他邻居，邻居又按 target 定向转发回来，形成冗余中转与回环，真机表现为「同网段好友申请一直中转、清掉还冒出来」。
- **单聊送达确认跨跳（`GossipKind::ChatAck`）**：接收方在 `handle_gossip` 单聊分支持久化后回发定向 Gossip 送达确认（明文 `{"msg_id":...}`，`target`=原始发送方）。此前单聊消息走 Gossip 多跳到达，但 Ack 只走 `try_send` 直连，跨 Tailscale 无直连时送达确认永远到不了发送方，消息状态卡在 `sent` 一直转圈。发送方按 `outbox(msg_id, sender)` 命中才接受，防伪造送达。
- **单聊已读回执跨跳（`GossipKind::ChatReadReceipt`）**：`mark_read` / `flush_pending_reads` 改走 `send_read_receipt_route`——有直连走 `Message::ReadReceipt`，无直连（跨跳）走定向 Gossip。修复跨网段聊天「双方都看到了却始终没有已读回执」。
- **拒绝好友申请不再被发送失败阻塞**：`respond_friend_request` 拒绝分支 `try_send(...).await?` 改为 `let _ = ...`——此前跨跳无直连时拒绝回执发不出去会导致 `pending_requests` 不删除、申请「清掉又冒出来」；现在本地清理与回执发送解耦，并补发 `friend-rejected` 事件。
- **回执/确认的身份绑定**：`sender_trusted` 对 `ChatAck` / `ChatReadReceipt` 不允许 TOFU，未在 peers 表时回退到 friends 表持久化的 ed25519 公钥做身份绑定（进程重启后 peers 内存态为空时不误拒跨跳回执）。

### Changed (Phase 6 网络层演进)
- **Routed 端点 `device_id` 改为可选**：`RoutedEndpoint` 的 `device_id` 由 `String` 改为 `Option<String>`；JSON 序列化时 `None` 不写入该键（`skip_serializing_if`）；旧格式 `{"device_id":"...","address":"..."}` 完全兼容，可直接被新代码反序列化。`scripts/t2-learn-id.sh` 端到端验证（向后兼容见 `discovery/routed::tests::device_id_is_optional_and_backward_compatible` 单测）。
- **主动拨号时无 peer_id 不再要求预配置身份**：`connect_to_peer` 接受 `known_id: Option<&str>`。`None` 路径遵循 §8 的 `IP:PORT → TCP → Hello → Node ID → Identity → 建立 Peer`：先发自身 Hello → 读对端回发的 Hello（被动方在「握手补全」中负责回发）→ 验签 → 学到真实身份后再登记链路 / 注册 mesh Connection / flush 待发队列。`HANDSHAKE_TIMEOUT = 5s`（大于正常握手，但覆盖「对端是未升级的旧版本、不会回发 Hello」兜底）。
- **`AppState::has_endpoint_addr(&SocketAddr)`**：身份未知时只能按端点判「要不要拨号」，否则 10s 周期重试会重复建链。方向性说明：主动方记录的 endpoint 是**对端的监听地址**（与配置一致），被动方记录的是**临时源端口**，故不会误判。
- **`add_routed_endpoint` 接受 `device_id: Option<String>`、`remove_routed_endpoint` 仅按地址匹配**：去重按地址而非 `(device_id, address)` —— 同一物理地址无论是否带 id 都是同一个端点；空字符串与 `None` 等价。
- **Routed 拨号任务的去重**：`spawn` 的拨号循环原本在循环内做 `has_endpoint` 检查；现在统一移到 `connect_to_peer` 里（按端点去重，且 `Some(id)` 时按 peer+endpoint、`None` 时按 endpoint）。理由：避免「同一判断两处实现、行为不一致」（项目踩过的坑）。

### Verification (Phase 6 Step 2)
- **T2-A 决定性验证（`scripts/t2-learn-id.sh`）**：实例1 (`--instance 1`) 配置 `[{"address":"127.0.0.1:60012"}]`（无 device_id），实例2 (`--instance 2`) 标准启动 → **7/7 PASS**：实例1 日志含 `[transport] 握手学到对端身份 peer=...-i2` + `[routed] 已连上 peer=<握手学>` + 实例1 DB `conversation_clocks` 出现 `...-i2` 行 + 实例2 DB 出现 `...-i1` 行（双向 observe_clock）+ 实例2 日志含 `[transport] 握手补全`（被动方回 Hello）+ 实例1 mesh 层 `[mesh] +conn peer=...-i2`。
- **护栏非空转验证**：在 `connect_to_peer` 的 `None` 分支临时注入「身份未知直接返回 Failed」回到旧行为，重跑 T2-A → **6/6 核心判据全部按预期 FAIL**（仅配置验证 PASS），证明判据**不是空转**。判定包括对话时钟表（已加 `DELETE FROM conversation_clocks WHERE conv_id LIKE '%-i1' OR '%-i2'` 防上次残留）。（基线 marker 已删。）
- **全门**：`cargo test --lib` 288 passed / 0 failed / 0 warning；`bash scripts/e2e-dev.sh` 30/0/1（功能零回归）；`npm test` 196 pass / 0 fail。
- **未触碰 Frozen Core**：msg_id / E2EE / Outbox / Ack / SQLite / 好友 / 文件 / 通知 / Chat UI 全部零改动。
- **后续**：「Routed 配置 UI」（让用户从好友列表选人 + 只填地址）排期独立；BLE 跨网段发现独立推进。

### Added (运行日志系统)
- **应用级运行日志**（`src-tauri/src/logging.rs`）：内存有界 ring buffer（500 条）+ 落盘文件（`logs/gosslan.log`，单文件 512 KB 超限轮转 `.old.log`，磁盘上界约 1 MB，惰性清理不另起后台任务）。生产环境（Windows release 无控制台）此前关键诊断日志只走 `eprintln!` 到 stderr 而全部丢失，现在统一进日志系统。
- **「运行日志」页**（`LogViewer.vue`）：桌面端走独立窗口（`open_log_window` 动态创建，label="logs"，系统标题栏、关闭即销毁），移动端走全屏页面（带返回）。支持滑动浏览、按级别着色（INFO/WARN/ERROR）、一键复制（时间正序）、清空（两段式确认）、自动刷新（2s 可关）。
- **入口**：桌面 NavRail 底部 + 移动端底部导航各加「日志」按钮（`ScrollText` 图标）。
- **日志规范**：写进 `logging.rs` 模块头注释——只记「可能出错」与关键状态跃迁，Info/Warn/Error 三档；不记消息正文 / 密钥等敏感内容；target 用子系统名（transport / lan / routed / mesh / friend / presence / link …）。
- **迁移现有诊断日志**：transport（握手/连接/拨号/presence/friend/link）与 network / commands / lib 启动阶段的关键 `eprintln!` 统一迁到 logger（级别、target 归一）。无 `state` 上下文的边界处（`set_abortive_close`、`await_tasks`、`tray::setup`）保留 `eprintln!`。

### Changed (P1 单聊定向化)
- **单聊消息定向投递**（`send_message`）：单聊 `GossipKind::Chat` 加 `target = 接收方`（参与签名，重签），投递改为「目标直连 → 只发它；否则广播靠中间节点按 target 定向转发」。此前单聊消息无条件 `broadcast_gossip` 全网广播，直连场景也放大到全网。投递失败**不返回 Err**（消息已落 outbox 兜底，链路竞态由 flush_outbox 补发），避免前端误判「发送失败」而重发。
- **接收端单聊消费加 target 判断**（`handle_gossip`）：`target` 存在且非本机 → 中间节点只转发不消费（防御性；即便不判断，中间节点也因 ECDH 解不开而不会落库，但明确判断语义更清晰）。
- 送达确认（ChatAck）与已读回执（ChatReadReceipt）此前已定向，本次对齐；outbox 补发走 `Message::ChatMessage` 直发、群聊无 target 广播，均不受影响。

### Changed (P1 M2 双向建链兜底)
- **小 ID 兜底拨号**（`ensure_link`）：此前只由「device_id 字典序较大」的一方拨号，小 ID 一方被动等。若大 ID 一方因单向可达（不对称 NAT/防火墙）拨不过来、或长期离线，小 ID 永远连不上。现在小 ID 在「对端在线却迟迟连不上」（首次发现超过 10s 仍无连接）时兜底主动拨号，补齐「谁能连上谁建链」的对等性；对称场景仍是大 ID 先拨（避免两端同时拨号产生重复连接）。
- 抽纯函数 `should_dial(my_id, peer_id, first_seen, now)` + 三个单测钉住「大 ID 恒拨 / 小 ID 阈值内等待 / 小 ID 超阈值兜底」，护栏非空转验证（临时禁用兜底 → 测试 FAIL）。
- `Peer.connected_since` 语义修正重命名为 `first_seen`（其值本就是「首次发现时间」而非「建链时间」，此前从未被读取）；前端 types 同步。

### Fixed (P1-2 镜像重复连接修正)
- **同一对节点不再稳定停留 2 条镜像 TCP**（`ensure_link`）：拨号判据从「**这个端点**连上了吗」提升为「和这个 peer **有连接吗**」。根因是端点表示不对称 —— 接受侧 `handle_incoming` 记录的 `Link.endpoint` 是 TCP **源地址（临时端口）**，而 `ensure_link` 拿到的是 announce 自报的**监听地址**，两者永不相等 ⇒ 被动方（小 ID）永远认为「没连上」，10s 后兜底拨号反向再拨一条，形成镜像重复连接（连接与读写任务翻倍、心跳双份，并让「断一条仍在线」的多路径判据变成假阳性）。镜像连接**不带来任何送达补偿**：`try_send` 只把消息交给 mpsc（返回 Ok 不代表 TCP 写出成功），所以它纯属浪费。
- **语义边界明确化**：`ensure_link` 只负责**连通性**（和看得见的 peer 建立联系），不负责**多路径** —— 多路径由各 Transport 自己的驱动产生（Routed 由配置驱动直接走 `connect_to_peer`、BLE 由 BLE 发现驱动，都不经过 `ensure_link`）。将来若需要「同一路径的多条连接」（如多网卡冗余），按**连接健康度**收敛，而不是放宽这一条。
- 抽纯函数 `should_dial(my_id, peer_id, has_endpoint, has_any_link, first_seen, now)`，决策顺序「已连该端点 → 已有任意连接 → 大 ID 恒拨 → 小 ID 超阈值兜底」。新增单测 `should_dial_skips_when_any_connection_already_exists` 钉住核心场景；**护栏非空转验证**：临时退回旧判据（只按端点）→ 该测试 FAIL（3 passed / 1 failed），证明它精确钉住了 P1-2 行为（marker 已删）。

### Fixed (未读徽标数字未垂直居中)
- **徽标数字在圆内偏下**（用户反馈「上宽下窄」）：2x 截图逐像素测量，圆 32 设备像素、数字墨迹 15–16、**上间隙 10 / 下间隙 7**（导航栏与会话列表三处徽标结果一致）⇒ 字形偏下 1.5 设备像素 = 0.75 CSS px。根因是 `items-center` 居中的是**行盒**，而字体的 ascent/descent 不对称、数字又没有下伸部，字形天然不在行盒正中。修法用**布局补偿**而非 transform（后者会让小字号文本在变换空间里栅格化而发虚）：固定高度盒加 `padding-bottom: 1.5px`，把行盒上移 0.75px（`(16 − 1.5 − 11) / 2 = 1.75`，未补偿时 2.5），正好抵消。
- **收敛为唯一实现 `UnreadBadge.vue`**：原先 5 处手写副本，其中 `ResponsiveLayout.vue` 的 2 处**漏了 `leading-none`** —— 同一种徽标在不同位置基线不一致（宏观「数字没居中」肉眼可见，读代码却看不出来）。同时消掉 3 份重复的「99+ 上限」逻辑。
- **新增静态护栏**（`designGuards`）：① 全库扫描禁止再手写徽标（带 `min-w-4` + danger 底色的 class 即为手写，精确报 `文件:行号`）；② 组件必须保留 `pb-[1.5px]` 与配套的 `leading-none`。**两条都做了非空转验证**（改坏 → 测试按预期 FAIL）。护栏刻意只解析真实 `class` 属性：首版用 `src.includes()` 扫全文，结果"因为注释里提到类名"而假通过，已修复并把该假通过固化成反面用例。

### Added (运行日志文本过滤)
- **日志界面文本过滤**：工具栏下方新增独立过滤条（不塞进工具栏 —— 那里已有 4 个按钮，移动端会被挤爆）。**字面包含**匹配、大小写不敏感、不做模糊/分词；命中处用与会话搜索同一套 `highlightText` 加颜色标记（`<mark>`，明暗主题各一套配色）。过滤时显示「匹配 n / 总数」，带一键清除；「没有匹配」与「暂无日志」是两种不同的空态文案。
- **判据是「所见即所匹配」**：只在界面上真实渲染出来的文本（时间 HH:MM:SS · 级别 · target · 消息）上匹配，不把未显示的日期部分纳入 —— 否则会出现「保留了这一行但整行没有高亮」的困惑。
- **过滤逻辑抽为可测纯函数 `utils/logFilter.ts`**：新增 9 条单测钉住语义（空词不过滤 / 子串命中 / 大小写不敏感 / **跨词不连续不算命中** / 正则元字符按字面处理 / 保持顺序）；另补 6 条 `utils/highlight.ts` 单测（转义安全 + 正则元字符不是模式 + 每处都标记）。

### Changed (M3-0 连接级健康信号，ADR-0014 §3.1)
- **`ConnectionHealth` 首次真正被喂上数据**：此前 `mark_seen` / `mark_failure` 只有 `mesh/peer.rs` 内部与单测在调用、`register_connection` 只 `merge` 出 `default()` 健康值，于是 `PeerManager::online_state()` 在生产路径**恒返回 Offline**（模型在、数据空 —— 与 Phase 2 review 抓到的 `upsert_connection` health 覆盖 bug 属同一类陷阱）。本步把它接上，且**纯旁路、行为零变化**：只写不读，选路仍照旧。
- **三个成功打点 + 一个失败打点**（全部复用现有帧，**零新协议**）：① 建链即打一次（否则「已建立但还没收发」的连接会被健康判据算作不健康，M3 选路会因此退化成「按固定顺序挑」甚至反复重拨 —— ADR-0014 §3.1 硬性注意 ①）；② `writer_loop` 每次写帧成功（心跳每 5s 一次 ⇒ 无业务消息时也至少每 5s 刷新）；③ `reader_loop` 每收到一帧（比「写成功」**更强**：对端确实活着，是半开 TCP 下唯一能区分真活/假活的信号）；④ 写失败记一次失败。RTT 恒为 `None` —— `Heartbeat` 是单向的、无回包，ADR-0014 明确本阶段不做 RTT，这里也不假装有数据。
- **`[mesh] +conn` 日志新增 `online=` 字段**：`ConnectionHealth` 是内存态，这是 mesh 健康信号在生产路径**唯一的外部可观测点**；没有它就只能靠读代码相信「信号接上了」。
- **验证（决定性 + 非空转）**：`bash scripts/t4-mirror-dial.sh` 实跑，三条连接（含真实局域网节点）全部 `online=1`；临时去掉建链打点 → 三条全部 `online=0`。后者同时**实测证实**了「M3-0 之前 `online_state()` 恒 Offline」这一 review 结论。
- `docs/adr/0014-multi-path-connection-selection.md` 状态 Proposed → **Accepted**（用户 2026-09-12 审核通过）。

### Added (M3-a 选路纯函数，ADR-0014 §3.2)
- **`mesh::selection::pick_link`**：多路径选路的**纯函数**（候选连接 → 该用哪一条）。策略：① 按活性过滤不健康连接 ② 路径优先级 **LAN > Routed > Bluetooth** ③ 同优先级用**建链顺序**打破平局（稳定可复现，不引入随机性）④ 全部不健康时**退回第一条**而非返回 `None`（保持可用优于报错，与改造前「首个成功即返回」的兜底一致）。
- **本步行为零变化**：函数先就位，只被单测调用，**没有接进 `try_send`** —— 按 ADR-0014 §9 Risks 把「策略」与「接线」拆开提交，接线（M3-b）出问题时可二分定位。
- **不做 RTT 排序**：`Heartbeat` 单向、无可靠往返测量来源，而 LAN 与 Tailscale 的差距由路径优先级已能区分（ADR-0014 §2）。
- `path_rank` 用**显式 match** 而非枚举声明顺序 —— 枚举顺序是巧合，以后往中间插一个变体就会静默改变选路优先级（已加单测钉住语义顺序）。
- `PeerManager` 新增 `health_timeout_ms()` / `max_failures()` 访问器：让选路复用**同一个**健康阈值，避免阈值散落两处（本项目踩过「同一判断两处实现、行为不一致」的坑）。
- 单测 +11（空集合 / 单条 / 全不健康兜底 / LAN>Routed>Bluetooth / 顺序打乱仍选 LAN / 不健康 LAN 不阻塞健康 Routed（failover 核心）/ 同优先级取先出现 / 过期不算健康 / 连续失败超阈值不算健康 / 恰好等于阈值仍算健康 / 优先级语义顺序）。**非空转验证**：临时把 LAN 降级 → 4 条优先级护栏按预期 FAIL。
- 验证：`cargo test --lib` **311 passed** / 0 warning；E2E 30/0/1（行为零变化）。

### Fixed (群成员变更不同步 + 缺系统消息)
- **群主移人后，其余成员的成员表不变小、也看不到任何提示**（用户反馈）。根因是**两头都断**：发送侧 `group_remove_member` 只把 `GroupMemberRemoved` 发给**被移除者本人**；接收侧 `handle_group_member_removed` 开头就是 `if to != 本机 { return }` —— 压根没有「别人被移出」这个分支。
  - 发送侧：现在**同时广播给其余成员**（被移除者本人仍单独通知以便清理本地群）；
  - 接收侧：补齐 `RemoveOther` 分支 —— 校验发起方确为群创建者后，同步本地成员表、清掉指向该成员的待补发群消息、落一条群内系统消息；
  - 群主自己也插一条系统消息（别人各自插入），文案「「X」已被移出群聊」/ “X” has been removed from the group。
- **主动退群也补了群内系统消息**：`leave_group` 原本已正确广播 `GroupMemberLeft`（成员表能同步），但接收端只更新成员表、不插系统消息，群里看不到「「X」退出了群聊」。现在两侧都有（同时清掉指向他的待补发群消息）。
- 成员变更的分支选择抽成纯函数 `member_removed_action`（三分支真值表），**非空转验证**：把 `RemoveOther` 改回 `Ignore` → 护栏按预期 FAIL（marker 已删）。
- 待测边界：**「其余成员是否真的收到并同步」需要 ≥3 台设备**（群主 + 被移出者 + 另一个成员），本机 E2E 只有 1 实例 + 1 对端，覆盖不到；已用单测钉住分支选择，集成留给真机回归。

## [2.1.2] - 2026-09-11

### Fixed
- **macOS 圆角外"淡淡一层颜色"（白主题淡白 / 黑主题淡黑）**：窗口背景色误用了 `--gosslan-bg`（浅 `#f1f5f9` / 深 `#0f172a`），而 body 实际底色是 `--gosslan-app-bg`（浅 `#edf1f6` / 深 `#0b1220`），两者差一档 → 圆角外透出与内容不同色的"淡淡一层"。修复：窗口背景色改用 `--gosslan-app-bg`，与内容零色差。真透明（透出桌面）需 `macos-private-api` 私有 API、会失去 App Store 上架资格，用户确认**保持可上架**，故不采用。
- **Windows 冷启动暗色下"闪一下白"**：窗口以 `visible: false` 创建，由前端挂载后调 `focus_window` 显示；但 `focus_window` 只 `show()` 没动背景色，`show()` 的第一帧会露出 WebView2 的默认背景色（`tauri.conf.json` 的 `backgroundColor` 写死浅色 `#edf1f6`），暗色主题用户在骨架合成前看到"骨架之前还有一帧白色"。修复：`focus_window` 在 `show()` 之前读后端 SQLite 的 `dark_mode`（"解析后的结果"，跟随系统时已按系统偏好算好），用 `set_background_color` 把窗口底色改成跟随主题（浅 `#edf1f6` / 深 `#0b1220`，与 body 的 `--gosslan-app-bg` 一致），第一帧即正确底色。命令消息 FIFO 顺序保证「先设色、后 show」，冷启动不再露浅色。
- **macOS 窗口圆角仍不生效 + 暗色下露白角**：此前在 `setup` 里给 contentView 设圆角，但 wry 在**窗口显示时才**用 `WryWebViewParent` 替换 NSWindow 的 contentView，setup 阶段的圆角被替换丢失；且窗口背景色写死浅色 `#edf1f6`，暗色主题下圆角外露出浅色边。修复：① 圆角改到 **WebView 加载完成后**设置（前端 `App.vue` onMounted 调新命令 `apply_macos_window_shape`），此时 contentView 已是 wry 的 parent_view；② 窗口背景色**运行时跟随主题**，消除暗色露白；③ `setHasShadow(false)` 留在 setup（NSWindow 级、不被替换）。新增 `macos_window::disable_shadow` / `apply_rounded_corners` 两函数。

## [2.1.1] - 2026-09-10

### Fixed
- **粘贴超长文本卡死输入框**：粘贴大段文字时 `execCommand("insertText")` 把整段（可能几十万字符）塞进 contenteditable，随后 input 事件里的 `innerText` 读取又强制同步 reflow，界面卡死。修复：① 粘贴前先 `slice(0, 50000)` 截断到硬上限（新增 `MAX_INPUT_LENGTH = 50_000`，与发送时的兜底截断共用同一常量）；② `syncDraftState` / `normalizeEmpty` 改用 `textContent` 替代 `innerText`（`innerText` 每次读取都触发 reflow，`textContent` 不触发布局）。发送序列化仍保留 `innerText`（只在发送时读一次，需保留 `<br>`→`\n` 换行语义）。
- **macOS 打开文件失败（App Sandbox 拦截 `/usr/bin/open`）**：`tauri-plugin-opener` 在 macOS 底层走 `open` crate → `Command::new("/usr/bin/open")`，而 App Sandbox 禁止沙盒应用 fork 外部可执行文件，故 Mac 端点开文件一律失败（Windows 端无沙盒正常）。修复：新增 `src-tauri/src/macos_open.rs`，macOS 改用 `NSWorkspace.openURL`（纯 Foundation API，沙盒允许），Windows/Linux 回落 opener；新增 `open_file_native` 命令并加 `path.exists()` 前置检查，文件不存在时返回明确错误（区分「文件不存在」与「无默认应用」）。
- **macOS 窗口四周无圆角**：跨平台用 `decorations: false` 自绘标题栏，关掉了 macOS 系统装饰（Windows 11 仍由 DWM 画圆角，所以 Mac 看起来直角、Win 看起来圆角，跨平台割裂）。修复：新增 `src-tauri/src/macos_window.rs`，运行时给窗口 `contentView` 的 layer 设 `cornerRadius: 10.0` + `masksToBounds`（公开 API；⚠️ NSWindow **没有** `setCornerRadius:`，容易误以为有——cornerRadius 是 CALayer 的属性）；同时 `setHasShadow: false`（系统阴影画在窗口外、是矩形，与圆角冲突）。**没**改 tauri.conf.json 的 `decorations`（单平台共用字段，改了会破坏 Windows 自绘）。

### Changed
- **PC 端窗口可缩到移动端宽度**：窗口最小宽度 `minWidth` 从 920 降到 **360**。`isMobile` 本就是响应式判定（`matchMedia("(max-width: 767px)")`，非平台判定），此前被 920 的 `minWidth` 挡住、PC 上永远触发不了移动端布局；现在缩窗口到 767px 以下即切换成移动端 UI（单列抽屉 + 底部导航），PC 上也能体验移动端形态。

## [2.1.0] - 2026-09-10

> 本轮为 **Apple HIG（2026 版）体验审计后的修复**，分三批 + 一次「App Store 上架前置」：
> **第一批**（改动小、收益确定）：辅助功能媒体适配、触摸端删除会话、Toast 可达性、`tap-safe`、图片 `alt`；
> **第二批**（需碰数据/结构）：搜索命中定位到消息、破坏性操作二次确认、空态行动入口；
> **第三批**（功能变更）：外观三态、macOS 原生菜单栏 + 快捷键、窗口控件系统行为、通知设置；
> **第四批**（为 iOS/macOS 上架铺路）：隐私清单、iOS 目的字符串、macOS 权限，以及第三批逻辑的单测覆盖。
> 未改任何消息投递、加密与存储语义。
> 审计报告见 `.workbuddy/artifacts/apple-hig-ux-audit-2026-09-10.md`，交付报告见 `p0-fixes-report-2026-09-10.md`。

### Added
- **macOS 原生菜单栏 + 跨平台快捷键**：自绘标题栏 + `decorations:false` 导致 macOS **没有系统菜单栏**，而 HIG 明确菜单栏是 Mac 应用的基础（⌘Q / ⌘, / ⌘W / ⌘M / 标准「编辑」项都靠它）。现在 `src-tauri/src/menu.rs` 在 macOS 建立应用菜单（关于 / 偏好设置 ⌘, / 服务 / 隐藏 / 退出 ⌘Q）、编辑（撤销/重做/剪切/复制/粘贴/全选）、会话（添加好友 ⌘N / 搜索 ⌘F）、窗口（最小化/关闭/缩放/全屏，`set_as_windows_menu_for_nsapp` 交给系统接管）。设计要点：
  - 自定义项**只发事件**、由前端执行，与快捷键走**同一条路径**（`window` 事件广播，见 `api/index.ts` 的 `APP_ACTION`），保证菜单与快捷键行为一致；
  - 菜单属"锦上添花"，**初始化失败不阻断启动**（与托盘不同，只打印日志）；
  - 跨平台快捷键 `useShortcuts`：⌘/Ctrl + `,`（设置）/ `F`（搜索）/ `N`（添加好友），**绝不碰** ⌘C/⌘V/⌘A/⌘X（那是系统编辑键）；组合态（中文输入法）放行。
- **通知设置**：新增后端键 `notify_enabled` / `notify_show_content`。设置页加「通知」分组：桌面通知开关（**打开时在用户动作上下文里请求权限**，被拒则保持关闭并提示，不再等某条消息到达才弹权限）+ 「通知显示消息内容」隐私开关（关掉后只提示"收到新消息"，锁屏/通知中心不泄正文）。
- **搜索命中可直接跳到那条消息**：此前搜到会话后点进去，用户还得自己在会话里翻——搜索只完成了一半。现在：
  - 后端 `SearchResult` 增加 `match_msg_id`（`db::search_messages_in_conv` 本就返回完整消息，只是此前没往外传）；
  - store 新增 `locateMessageInConv(convId, msgId)`：打开会话后若命中不在已加载窗口（默认最近 100 条），**逐页往前找**，上限沿用 `MAX_PAGES`（与手动上翻一致，不会为一句话翻遍整库）；找到后置 `locateRequest`，由 `ChatWindow` 滚动 + 高亮（复用既有的引用定位渲染路径）；
  - ⚠️ `locateRequest` **刻意不复用** `unreadJump`：后者会画「以下是未读消息」分割线，语义不同，复用会画错东西；
  - 返回**三态**（`found` / `not-found` / `error`）而不是布尔：翻到顶没找到、与翻页中途出错是两回事，要给不同的话——不能静默，也不能说错原因。
- **外观跟随系统（三态：跟随系统 / 浅色 / 深色）**：此前只有"深色模式"布尔开关，用户在 macOS / Android 系统里切换外观时 App **不跟随**——这是最容易被感知的"不像原生"之处（Apple HIG *Dark Mode* 要求 Respect the system appearance）。现在：
  - 新增后端设置键 `appearance_mode`（`system` | `light` | `dark`，缺省即 `system`），与既有 `dark_mode` **并存不冲突**：前者是**用户意图**，后者是**解析后的结果**（跟随系统时由前端按系统偏好算出来回写）。`save_settings` 对非法值直接忽略（宁可回落"跟随系统"也不写脏值），并已加入 `SETTINGS_KEYS` 使"恢复默认"能清干净
  - store 里 `dark` 由可写 ref 改为 **computed**（`appearance === "system" ? 系统偏好 : 强制值`）——既有大量 `app.dark` 读取处**零改动**，写入统一收敛到唯一入口 `applyAppearance()`
  - 跟随系统模式下监听 `prefers-color-scheme` 变化，系统切外观 App **即时**跟随，无需重启；强制模式下系统怎么变都不影响用户选择
  - **旧数据不丢**：≤2.0.3 只写了 `gosslan.dark` 布尔值，那是用户的一次**显式**选择；升级后按 `1→dark / 0→light` 迁移成显式模式，不会被静默改成"跟随系统"
  - `index.html` 首屏骨架的判定同步（骨架先于 bundle 执行，判定逻辑必须与 store 完全一致，否则启动瞬间会看到"骨架浅色 → 界面深色"闪一下）
- **错误文案收敛模块 `utils/errors.ts`**：把 IPC 抛出的异常转成「能读懂 + 可行动」的一句话。⚠️ 实测前提：本项目 Rust 侧**大部分错误本来就是写好的中文说明**（如「对方不是好友，请先扫描添加好友之后再继续聊天」），所以策略不是"一律换成通用文案"（那会抹掉有用信息），而是：命中需额外解释的模式 → 换成更有帮助的说法（如公钥缺失时说明"消息已保留、对方上线会自动补发"）；看起来已是给人读的 → 原样保留；其余（`os error 2` 这类 IO/库英文串）→ 换通用文案，**原文只进 console**，既不给用户看转储也不丢排查线索
- **自动化护栏 `utils/a11yLabels.ts` + 测试（含全库扫描）**：扫描 `src` 下全部 `.vue`，报出"纯图标且没有任何名字来源"的 `<button>`。判据刻意收紧（已有 `aria-label`/`aria-labelledby`（含 `:`/`v-bind:` 动态绑定）/`v-html`/可见文本或插值/带非空 `alt` 的 `<img>` 都跳过），避免误报
- **App Store 上架前置配置（macOS / iOS，均为 App Store 强制项，此前完全缺失）**：
  - **隐私清单 `src-tauri/PrivacyInfo.xcprivacy`**：声明不追踪、不采集数据（本应用纯 P2P、无服务器、无统计 SDK），并登记 Tauri 运行时用到的 required-reason API（`UserDefaults` / `FileTimestamp` / `SystemBootTime` / `DiskSpace`，各带正确的 reason 码），通过 `bundle.resources` 打入产物；
  - **iOS 目的字符串 `src-tauri/Info.plist`**：`NSLocalNetworkUsageDescription` —— 本应用靠 UDP 广播发现 + TCP 直连局域网，iOS 14+ 缺了它会在访问本地网络时被系统拦截甚至崩溃；通过 `bundle.iOS.infoPlist` 合并进默认 Info.plist（iOS 工程尚未生成，此为预置）；
  - **macOS 权限 `src-tauri/entitlements.plist`**：App Sandbox + `network.client/server` + 用户自选文件读写 + Downloads 读写，通过 `bundle.macOS.entitlements` 接入。
- **纯函数模块 `utils/appActions.ts` / `utils/shortcuts.ts` / `utils/notifications.ts`**：把「应用级动作名」「快捷键命中判定」「通知正文拼装」从 api/composable/store 里抽成零 `@/` 依赖的纯函数（Node 单测无法解析 `@/` 别名），并补齐单测。
- **状态恢复（各端统一，Apple HIG *State Restoration*）**：
  - **上次会话恢复**：打开会话即记 `gosslan.lastConv`（localStorage），重启后若该会话仍存在则自动打开——三端一致，回到上次离开的地方。
  - **窗口尺寸/位置恢复**（桌面 macOS/Windows）：接入官方 `tauri-plugin-window-state`，只持久化 `SIZE/POSITION/MAXIMIZED/FULLSCREEN`。⚠️ **刻意排除 `VISIBLE`**——本应用「关闭=隐藏到托盘」，若把可见性也持久化，会记成"关闭后是隐藏态"、重启就不显示窗口了；`DECORATIONS` 也排除（自绘标题栏由本项目管理）。
  - **移动端方向统一竖屏**：iOS `Info.plist` 补 `UISupportedInterfaceOrientations=Portrait`，与 Android 既有的 `screenOrientation="portrait"`（`scripts/inject-android-signing.mjs`）对齐——当前移动端横屏布局尚未适配（会破坏安全区/导航），等横屏就绪后再放开 iPad 多方向。
- **本地化骨架（App Store 全球上架阻断项）**：此前全中文硬编码、无任何 i18n。本轮建立：
  - `src/i18n/`：轻量字典（`zh-CN`/`en-US`）+ 响应式 `t()`（支持 `{name}` 插值）+ `applyLocale`/`isLocale`，自写而非引入 vue-i18n（文案量有限，避免新依赖）。
  - 后端新增 `language` 设置键（脏值忽略，回落中文），store 加 `language`/`setLanguage`（切语言即时生效 + 持久化）。
  - 设置页加「语言」切换入口（简体中文 / English），**覆盖系统 UI（导航栏）与设置页主框架 + 各分组标题/footer + 通知/共享/重置/清除 + 清除确认弹窗**。
  - 护栏 `src/i18n/index.test.ts`：**断言中英字典 key 集合完全一致**（漏翻译会变红）+ t() 翻译/插值/缺 key 回退。
  - ⚠️ 各分组**内部字段文案**（外观三态、气泡配色预设名、存储清理策略、网卡列表等）仍为增量待迁——切语言后这些字段暂不随动，后续批量补。
- **通知「标记已读」动作（移动端）**：Android/iOS 通知增加「标记已读」按钮（`registerActionTypes` + `actionTypeId`），点按不唤起窗口、直接标记该会话已读（发已读回执 + 清未读角标）。桌面端 Web Notification 不支持按钮，保持「点击打开」。
- **⌘/Ctrl + = / − 调整消息字号（Dynamic Type 精神）**：在 小/标准/大 三档间切换，直接改 store（`useShortcuts` 里处理，不走 window 事件），与设置页「字体大小」共用同一套 `CHAT_FONT_SIZES`。
- **macOS 滚动条恢复系统 overlay**：`html.platform-mac`（store init 按 `isMac` 标记）+ CSS 覆盖，macOS 上滚动条回到「滚动才浮出、不占布局」，Windows/Android 保留 6px 常显细滚动条。
- **移动端消息长按 → 底部 Action Sheet**：新建 `ActionSheet.vue`（底部滑出、遮罩、取消按钮、安全区），移动端长按消息唤出「复制/保存/引用/转发」等操作——此前移动端**没有右键、也没有长按**，消息操作在触屏上完全不可用（真实功能缺失）。
- **语言跟随系统（三态：跟随系统 / 简体中文 / English）**：此前语言默认写死中文。现在：
  - 后端 `language` 键扩展为 `system` | `zh-CN` | `en-US`（缺省即 `system`，脏值忽略）；
  - `src/i18n` 抽出 `detectSystemLocale()`（系统 `zh*` → 中文，其余 → 英文）+ `LanguagePreference` 三态，`locale` 改为 computed（`preference === "system" ? 系统语言 : 偏好`）；
  - 设置页语言切换改为三选一（「跟随系统」随当前语言翻译，「简体中文 / English」按国际惯例不自翻译）；
  - `index.html` 首帧脚本同步检测系统语言设置 `lang`（避免启动瞬间静态 `lang="zh-CN"` 与真界面不一致）；
  - 护栏 `src/i18n/index.test.ts` 扩展：`detectSystemLocale` 纯函数覆盖 + `refreshSystemLocale()` 重解析 + 显式偏好不受系统语言变化影响。
- **应用名本地化「相闻」/ "Gosslan"**：应用中文名「相闻」、英文名 "Gosslan"，桌面/主屏图标名按系统语言显示：
  - macOS：`src-tauri/infoplist/{en,zh-Hans}.lproj/InfoPlist.strings` 本地化 `CFBundleDisplayName`，经 `bundle.macOS.files` 精确放入 `Contents/Resources/<lang>.lproj/`（Tauri 不自动生成 InfoPlist.strings；官方推荐的 `resources` glob 也可行，这里用 `files` 显式映射更精确）；
  - iOS：`Info.plist` 设 `CFBundleDisplayName = "相闻"`（中文主市场默认），英文系统的本地化待 iOS 工程生成后补 `en.lproj/InfoPlist.strings`；
  - ⚠️ Windows 桌面快捷方式名 = `productName`（NSIS 不支持按系统语言），保持 "Gosslan" 不变（改 productName 会连带数据目录/bundle id，不推荐）。
- **英文翻译按 Apple 规范润色**：Title Case 一致性（`Mark as Read` / `Show Message Content`）、`&`→`and`、全大写强调 `NOT`→`not`、语法修正（`switch interface`→`switch the interface`）、描述用 sentence case、无障碍 label 更清晰（`My profile, {status}. Open settings.`）。
- **英文样式适配**：语言分段控件加 `flex-wrap + whitespace-nowrap`（英文「Follow System」较长，放不下换行而非溢出）；设置页 label/footer 均 flex + 自动换行，英文长文案安全。
- **README 参与贡献模块**：顶部加 release / contributors / license 徽章，License 前加「参与贡献」区块，用 `contrib.rocks` 动态展示提交量前 10 位贡献者头像。
- **聊天以外全库文案 i18n 全覆盖**：把上一轮只覆盖「系统 UI + 设置页主框架」的本地化，扩展到**除聊天消息内容外的全部用户可见文案**（44 个文件、约 500 个字典 key）：
  - 设置页 7 个 Section 内部字段（外观三态、主题色、字体、气泡配色预设名、字号档位、存储策略、网卡列表、资料、安全、关于）；
  - 所有 `title` / `aria-label`（标题栏窗口按钮、聊天头部、消息操作、回执、图片预览、文件气泡、输入区、会话/好友列表等）；
  - 所有弹窗 / 菜单 / toast / 空态 / placeholder / 状态标签（群组、好友、会话删除、共享目录、转发、诊断面板、移动端导航）；
  - `chatStyle.ts` 的气泡预设 `label` 与字号 `label` 改为 i18n key（纯数据模块零依赖，组件 `t(label)` 翻译）；
  - store/composable 的直接 toast（发送失败、权限、文件操作）统一走 `toastError` / `t()`。

### Fixed
- **触摸端无法删除会话**（真实功能缺失）：会话行的删除键写成 `hidden` + `group-hover:flex`，而 **Android 没有 hover 事件 → 该按钮永远不显示**，表现为"桌面能删、手机删不掉"（同一层的"删除好友"有长按兜底，聊天记录却没有）。新增全局工具类 `.hover-reveal` / `.hover-reveal-op`（`@media (hover: none)` 下退化为常显），并把「凡用 `group-hover` / `opacity-0` 揭示的元素都必须加其中之一」写进设计规范
- **读屏用户收不到失败反馈**：toast 是**唯一的失败反馈通道**（发送失败 / 删除失败都靠它），但容器没有 live region → 失败被静默。补 `role="status"` + `aria-live="polite"`（每条 `aria-atomic` 保证整句播报），装饰图标加 `aria-hidden`；错误停留时长 **3s → 6s**（读屏播报比扫一眼慢得多，原值常常没播完就消失）
- **错误提示直接暴露原始异常串**：31 处 `catch` 里写成 ``toast(`发送失败：${e}`)`` / `toast(String(e))`，会把 Rust 侧 `Err(String)` 原文（含 device_id、`os error 2` 之类）直接给用户看。统一改走 `app.toastError(e, "发送失败")`（14 个文件、31 处；按所在函数给了语义化前缀）
- **图标按钮在读屏下等于"无名按钮"**：全库 60 余处只写了 `title`，而 `title` 是**鼠标工具提示**、不是可访问名（触屏 VoiceOver/TalkBack 基本读不到）。为 **31 个纯图标按钮**补 `aria-label`（与 `title` 并存：一个给读屏、一个给鼠标），覆盖导航栏、标题栏窗口按钮、聊天头部、图片预览、群成员面板、输入区工具栏、申请列表等；动态值用 `:aria-label` 保持跟随
- **发送状态对读屏不可见**：回执是纯图标（转圈 / 空心圆 / 绿勾 / 红叉），读屏什么也读不到——而"发送中 / 已送达 / 已读"是聊天最核心的状态。给回执容器加 `role="img"` + 可访问名
- **列表行键盘不可达**：会话行 / 好友行是 `<div class="cursor-pointer">`，键盘用户无法 Tab 进入（`style.css` 注释里此前已自认"属后续项"）。补 `tabindex="0"` + `role="button"` + `:aria-label` + Enter/Space 激活；焦点环复用既有的全局 `:focus-visible` 规则，无需新增样式
- **未适配辅助功能媒体**：Apple 2026 HIG 明确要求适配「降低透明度 / 提高对比度」（macOS 27 另有 "Show Borders"），而项目大量使用毛玻璃且**完全没有**对应的降级。补两段媒体查询：`prefers-reduced-transparency: reduce` 时 `.frost` 退回不透明实底、`.glass`（模态遮罩）去模糊并加深压暗、`.vel-modal` 同理；`prefers-contrast: more` 时把**边界类** token（`border` / `divider` / `hover` / `window-ring`）提档。**刻意只动表面与边界、不动文字色**——文字可读性已由 `tokenContrast` 契约在亮/暗两套外观下逐对保证，在此再改会绕过那道护栏
- **44pt 最小点按目标规则"定义了却从未使用"**：`style.css` 早有 `.tap-safe`，但全库 **0 处引用**；而多数独立小图标按钮只有 24~32px，低于 Apple 的最小点按目标。本轮给 **15 处**补上（会话删除键、列表头的加号、聊天头部 4 个、申请行的同意/拒绝、共享目录的刷新/下载、群成员面板的转让/移出、文件气泡的下载、回执的重发、图片预览的关闭）。**刻意不为所有小按钮都加**，判据是「垂直方向有没有紧邻另一个可交互元素」：
  - ✗ **导航栏的图标栈**（`gap-2` 紧凑堆叠）：扩 8px 会盖到相邻按钮；
  - ✗ **输入区工具栏**（正下方是 `contenteditable` 正文区）：扩 8px 会抢走"点正文最后一行"的点击，反而更难用。
  这正是 `style.css` 里"只扩垂直、不给横向相邻控件加"那条告诫的延伸（触屏下多扩的命中区落在**非交互**空间才安全）。
- **`<img>` 全部声明 alt**：此前全库 17 处 `<img>` 无一带 alt，读屏可能念出文件名/URL。**分两类处理、不一刀切**：
  - **装饰性头像 15 处** → `alt=""`（HTML 规范里"这是装饰、读屏请跳过"的**正确**写法）——它们所在的行/按钮**已有可访问名或可见姓名**，再写一次人名只会让读屏重复念；
  - **内容图片 2 处** → 真实 alt：消息内图片 `alt="图片消息"`，图片预览用 `:alt="current?.name || '图片预览'"`（用文件名，取不到时回退）。
  - 判据是「**出现过** alt」而不是「alt 非空」——`alt=""` 合法，要禁的是"忘记写"。
- **破坏性操作的确认行为不一致**（同类的三处三种待遇）：
  - **右键删除好友此前"单击即删"**，而「删除聊天记录」和资料页的「删除好友」都有确认弹窗 → 右键那条最容易误触。现补二次确认（讲清"聊天记录保留 / 对方无法再发消息 / 可重新添加"）。
  - 这里**刻意不做"撤销"**：删除好友在后端不是可本地回滚的操作（对方可能已同步移除，重建关系要走一次好友申请），给一个做不到的"撤销"比不给更糟——所以选确认，而不是选一个假的甜点。
  - **「清除聊天数据」原本用 `window.confirm`**：那是 WebView 的系统对话框，样式与 App 完全脱节，在无边框窗口里尤其突兀。改为应用内 `BaseModal`（与其它破坏性确认同一套样式，并逐条列出"删什么 / 不删什么"）。
- **平台判定把 iOS 误判成 macOS**（iOS 上架前必须修掉）：`isMac` 用 `/Macintosh|Mac OS X/` 匹配 UA，而 iOS 的 UA 形如 `... (iPhone; CPU iPhone OS 17_0 like Mac OS X) ...`，其中 `like Mac OS X` 会命中 → iPhone/iPad 被当成 Mac（移动端错误显示红绿灯、快捷键误用 ⌘ 而非 ctrl）。改为只匹配桌面 macOS 独有的 `Macintosh`，并加 `navigator` 守卫（供 Node 单测 import）。
- **群成员面板两处仍用 `window.confirm`**（转让群主 / 退出群聊）：改为应用内 `BaseModal` 二次确认，与「清除聊天数据 / 删除好友」统一。⚠️ 剩余一处 `StorageSection` 的缓存策略确认仍用 `window.confirm` —— 它在 `watch` 里依赖**同步**弹窗 + 立即回滚的时序，改异步弹窗需重排该回滚逻辑，风险较高，留作后续。
- **截图粘贴时好时坏**：Windows 11 截图（Win+Shift+S）的剪贴板**同时**带一个临时文件引用（CF_HDROP 指向 Temp 下的 PNG），原逻辑「文件路径优先」会把截图误判成文件、去发那个可能已被清理的临时路径，导致"有时发得出去、有时发不出去"。修复两处：① `classifyPaste` 改为**图片优先于文件路径**；② `onPaste` 在**任何 `await` 之前**同步捕获图片 `File`（`files` 优先、`items.getAsFile()` 兜底）——Chromium/WebKit 会在 paste 事件返回后清空 clipboardData，先 `await` 再读 `items` 会拿到 `null`。
- **发送文件（尤其 .md）被渲染成代码块**：文件消息此前按 `subtype=code` 渲染成**内联代码预览块**（把 .js/.md 内容拉出来高亮显示），而不是文件卡片。现在**文件一律按文件卡片渲染**——代码块只来自「代码消息」（kind=code，输入框粘贴/发送的文本），文件不再依据扩展名变代码块。同时 `.md`（Markdown 是文档而非代码）从 `classify_file_subtype` 与文件卡片图标的 `code` 分类中移除，归为普通 `file`/文档图标。
- **群关系同步会凭空重建群聊会话**（产品语义错误）：`handle_group_key`（收到 GroupKey 后的群关系同步路径）在 `upsert_group` 之后**多调了一次 `ensure_conversation`**，导致用户清库/重装后仅凭群关系同步（群名/群成员/群密钥），之前加入过的群聊就会自动重新出现在聊天列表。这混淆了「群关系」与「聊天会话」——conversation 是**聊天活动驱动的会话索引**，只有收到新消息（`insert_message` + `touch_conversation`）时才应创建。修复：删掉这一处 `ensure_conversation`，保留 `upsert_group`（写 groups/group_members）与 `observe_clock`（群时钟推进）；群消息接收路径的 `touch_conversation` 不动，因此新群消息仍会正常创建会话。新增 db 层测试锁定不变量（群关系同步不建会话 / 新消息建会话 / 已有会话不被删除或重复创建）。

### Changed
- **空态只有陈述、没有下一步**：主聊天区、会话列表的「暂无会话 / 暂无好友」此前都只有一句话。新用户最常卡在"怎么加人"，现在空态直接给「添加好友」按钮（**搜索无结果时不给**——那是"换个词"的场景，不是"去加人"）。
- **`docs/design-guidelines.md` 新增 §10「系统与辅助功能跟随」**：把本轮确立的三条硬规则写进规范（辅助功能媒体必须响应、可访问名是准入项、悬停不能是唯一入口），并更新 §7.2 的"已知偏差"——「未新增跟随系统外观」已由本轮修复，从偏差表移除
- 设置页「外观」由二态开关改为**三选一分段控件**（跟随系统 / 浅色 / 深色）；导航栏的太阳/月亮按钮保留为快捷开关（语义明确为"切成显式的浅/深"，不再在城市与系统之间来回）
- **macOS 窗口控件补齐系统行为**：绿灯 option-click 进入/退出全屏（此前只有缩放，新增 `window_toggle_fullscreen`）；双击标题栏 = 缩放（仅 macOS，Windows 由 tao 原生处理）。⚠️ 系统偏好「双击标题栏的动作」无法从 WebView 读取，这里用系统默认的"缩放"，若需完全跟随需改 `decorations + titleBarStyle: Overlay`（属后续项）。
- **前端不再在 macOS 兜底 ⌘W**：此前靠 `TitleBar` 的 keydown 兜底，现由原生「窗口 → 关闭」菜单（配合 lib.rs 恢复的 NSWindow `Closable` 位）接管，避免双触发；Windows/Linux 的 Ctrl+W 兜底保留。
- **输入框移动端键盘提示**：`MessageComposer` 的 contenteditable 补 `enterkeyhint="send"`（回车即发送，iOS/Android 键盘显示"发送"而非"换行"），并按代码模式切换 `spellcheck` / `autocorrect` / `autocapitalize`（代码模式关闭纠错，避免改坏粘贴的代码）。

### 校验
- `npm test` **196/196**（原 121；新增 `errors` 8 例、`a11yLabels` 12 例、`templateBranches` 3 例、`appearance` 10 例、`designGuards` 12 例、`platform` 4 例、`shortcuts` 6 例、`notifications` 5 例、`clipboard` 1 例（图片优先回归）、`i18n` 14 例（含 `detectSystemLocale` 纯函数 + 跟随系统重解析））
- `npx vue-tsc --noEmit` 0 错误；`cargo check` 0 error / 0 warning；`cargo test --lib` **219/219**（新增 `markdown_is_a_document_not_code`）
- **降级 CSS 用无头浏览器实测计算值**（不靠推理）：`prefers-reduced-transparency` 下 `.frost` / `.glass` / `.vel-modal` 的 `backdrop-filter` 均为 `none`、`.glass` 背景变为 `rgba(0,0,0,0.62)`；`.hover-reveal` 的 `display` 确为 `flex`（证明 `!important` 压过了 Tailwind 的 `hidden`）；`prefers-contrast: more` 下 `--gosslan-border` = `#94a3b8`。⚠️ 这三段媒体查询**必须留在 `style.css` 末尾**——`.glass` / `.frost` 的定义在文件中更靠后，同优先级下"后定义者胜"，写在前面会被直接覆盖（首版即踩，已实测确认）

> **三批的完成情况**：第一批（辅助功能媒体适配、触摸端删除会话、Toast 可达性、`tap-safe`、图片 `alt`）✅；
> 第二批（搜索定位到消息、破坏性操作二次确认、空态行动入口）✅；
> 第三批（外观三态、macOS 原生菜单栏 + 快捷键、窗口控件系统行为、通知设置）✅。
> 第一批里的「抽 `IconButton` 组件」**有意未做**：图标按钮的可访问名已用 inline `aria-label` 补齐，
> 并加了全库扫描护栏；此情此景引入一个组件抽象违反 `AI_RULES §33`（不要无必要的抽象），
> 且会让 31 处改动的 diff 变大、收益为零。
> 路线图（P2 形态类：侧栏贴边/图标着色、字号覆盖整套 `--gosslan-text-*`、常显滚动条）未动。

## [2.0.3] - 2026-09-10

### Added
- **首屏骨架屏（消除启动白屏）**：此前 `index.html` 只有一个空 `#app`，样式与脚本要等 `main.ts` 执行后才生效——WebView 加载期间用户看到的是一整片纯白，容易误以为"卡了"。现在 `index.html` 内联了首屏骨架：**关键底色 + 三栏布局骨架（caption 38 / rail 64 / 列表 250 / 聊天区 / 输入条）+ 轻微呼吸动画**，并内联脚本在样式加载前先按 localStorage 恢复**亮暗主题与主色**（顺带消除"先白后暗"的闪烁）。真实数据就绪后淡出移除；另有 5s 兜底定时器，初始化异常也不会把骨架永久挡在界面上
- **聊天记录导出（纯文字单文件）**：此前**没有任何导出/备份入口**——磁盘吃紧或换机时只能看着聊天记录丢，「存储与缓存」页的清理又只管媒体，用户没有任何自救手段。现在设置页「存储」新增**导出聊天记录**：把全部会话的文字导出成**一个 Markdown 文件**（会话按最近活跃排序，逐条带时间与发送者；图片/文件消息只保留文件名，媒体本体不在里面）。刻意**不产出 HTML**：正文来自对端，导出成 HTML 再用浏览器打开等于自己造一条本机 XSS 通道；Markdown 在任何编辑器里都能读且没有执行语义。同时**不需要新增任何依赖**（`AI_RULES §25`），时区换算由前端给出偏移、Rust 侧纯整数运算

### Changed
- **深色模式按 Apple HIG 全局校准（主题关联性 + 文字可读性）**：此前深色只是"把浅色变量翻一遍"，有 11 处组件仍在写死 `emerald/neutral` 色阶、若干"与底色同值"的 token 让控件在暗色下隐形（对比 1.00 = 完全看不见）。本次逐项按可测对比度收敛到语义 token：
  - **新增暗色语义 token**：`--gosslan-success` / `--gosslan-success-ink`（填充与文字分开，绿在白底只有 2.5，文字必须深一档）/ `--gosslan-success-soft` / `--gosslan-status-offline` / `--gosslan-field`（输入框底）/ `--gosslan-hud`（toast 底）/ `--gosslan-card` `-ink` `-line`（中性文件/代码气泡）/ `--gosslan-accent-ink`（主题色当文字用）
  - **文字可读性（实测 WCAG 比值，改前 → 改后）**：绿色状态字 3.88 → **7.24**（暗）/ 3.77 → **5.48**（亮）；主题色文字 3.98 → **5.90**（暗）/ 3.68 → **6.30**（亮）；危险色文字 4.29 → **4.63**（暗）；代码块工具栏文字 2.79 → **4.59**（亮）；已读勾图标 2.32 → **5.01**（亮）。六套预置主题色在两种外观下都 ≥ 4.5
  - **主题关联性**：`--gosslan-accent-ink` 用 `color-mix` 派生（亮色压暗 28% 黑、暗色提亮 28% 白）——因为主色是用户可选、且由 `applyTheme` 以 inline style 写在 `<html>` 上，**暗色档无法用 `.dark { --gosslan-primary: … }` 覆盖**（inline 优先级更高）
  - **暗色下隐形/糊住的层**：搜索框底原本与列表栏同值（1.00）、右键菜单/表情面板的毛玻璃浮层与列表栏同值（1.00）、组件边框贴列表只有 1.28、toast 贴画布只有 1.15 → 分别抬到 **1.27 / 1.32 / 1.64 / 1.81**（对齐 Apple 深色 `tertiarySystemFill` 约 1.35 的"填充差"量级）；toast 底色在暗色下由"深灰"改为"抬亮一档"的中性灰（白字仍有 9.1:1）
  - **原生控件跟随主题**：补 `color-scheme: light/dark`，否则暗色下 `<select>` 仍弹白底下拉、滚动条/取色器永远是浅色
  - **硬编码色清零**：11 处 `bg-emerald-500` / `bg-neutral-400` / `text-emerald-500(600)` / `bg-emerald-500/10` 与 toast 的 `bg-neutral-800/90`、`useMessageDisplay` 的中性卡片色全部改为 token；`index.html` 首屏骨架的 `--boot-line` 同步更新（骨架无法用 CSS 变量，见规范 §4）
  - 校验方式：`npm test` 99/99、`npm run build` 通过，并用无头浏览器渲染 light/dark 双栏逐像素采样，确认每个 token 的**实际渲染值**与设计值一致（曾因此发现 `--gosslan-accent-ink` 的暗色档漏写）

- **亮色模式可读性按 Apple 标准校准（与上一轮暗色校准对称）**：上一轮只把暗色收敛到可测对比度，亮色沿用旧值未复核。本轮逐对实测后发现四处不达标，且都是**日常高频位置**：
  - **次要文字 `--gosslan-text-2` 不够**：设置页说明、会话摘要、占位符大量用 11~13px 小字，原 `#64748b` 落在卡片底只有 **4.11**、列表底 **4.34**（都低于正文要求的 4.5）→ 按项目既有 Slate 灰阶下走一档到 `#475569`：卡片 **6.54** / 列表 **6.92** / 面板 **7.58**
  - **危险色被当文字用**：11px 的「发送失败」、删除按钮、`[有人@我]`、错误提示在原 `#ff3b30` 上只有 **3.06~3.55** → 新增 **`--gosslan-danger-ink: #cc2418`**（面板 **5.48** / 列表 **5.00** / 卡片 **4.73**）；**填充档 `#ff3b30` 保持不变**，未读徽标、危险按钮实底、hover 底照旧走系统红
  - **警告色被当图标用**：群主皇冠、文件夹图标在原 `#ff9500` 上只有 **2.20**，连非文字的 3:1 都不到 → 新增 **`--gosslan-warning-ink: #c67600`**（面板 **3.50** / 列表 **3.20** / 卡片 **3.02**）；填充档不改
  - **离线状态点太淡**：`#a3a3a3` 在白底 **2.52** → 取 Apple `systemGray` **`#8e8e93`**（面板 **3.26**），并顺带与暗色档统一为同一个灰
  - 共 **20 处** `text-[var(--gosslan-danger/warning)]`（11 个文件）改走 `-ink` 档；10 处 `bg-[var(--gosslan-danger)]` 等**填充**用法原样保留 —— 这正是上一轮确立的「填充档 / 文字档分开」原则的对称落地
  - **新增自动化护栏**：`utils/tokenContrast.ts` + 测试直接读 `src/style.css`，按带 `why` 说明的契约表逐对核算 `:root` 与 `.dark`，并断言每个 `-ink` 档在两套外观都成对定义（漏写 `.dark` 会立刻变红）。测试数 114 → **121**
  - 校验：`npm test` 121/121；无头浏览器渲染「改前/改后」双栏逐像素采样，确认实际渲染值与设计值一致（改前 `#ff9500`/`#ff3b30`，改后 `#c67600`/`#cc2418`）
  - **有意保留的偏差已逐条登记**在 `docs/design-guidelines.md §3.3`：在线绿点 **2.54**（Apple 自家 `systemGreen` on white 仅 2.22，本应用已更亮）、未读徽标白字约 **3.5**（与 Apple 计数徽标同取舍）、亮色边框 **1.23**（亮色靠阴影表达层级，与暗色判据不同、不可互搬）

### Fixed
- **每条消息都被渲染两遍（文本/代码消息整条重复）**：`MessageItem` 的「图片已被清理」占位块写成了 `v-if`，而 `v-if / v-else-if / v-else` 是**按同层相邻性成链**的——这个 `v-if` 把「文本 → 代码 → 图片 → 文件 → 兜底」那条链**切断**了，于是链尾的兜底分支 `<div v-else>{{ message.content }}</div>` 变成一条独立链的 `v-else`，对**任何非图片/文件的消息都成立**。表现就是每条文本消息出现两个气泡；带表情的消息更明显：一个是表情气泡（`MessageTextBubble` 把 `[摊手]` 解析成表情图），另一个是兜底气泡（原样显示 `[摊手][害羞][色]` 文本）。现在把该块改回 `v-else-if`，让 7 个分支回到同一条链上，非图片/文件消息只命中第一支（渲染实测：一条消息 2 个气泡 → 1 个）
  - 该缺陷由提交 `fd02f62` 引入（当时为「媒体已被清理」加占位块时顺手写成了 `v-if`），**尚未发版**，仅存在于本地提交
  - **新增静态护栏防复发**：这类问题 `vue-tsc` 与既有单测都覆盖不到（模板分支结构不在二者覆盖面内），且修复只是一个词的差别、极易复发。新增 `src/utils/templateBranches.ts` + `templateBranches.test.ts`：解析 `<template>` 的兄弟分支链，报出「链被切断且新链末尾 `v-else` 会误命中旧链分支」与「孤儿 `v-else`」两类问题；判据刻意收紧（只在被切断的链本身是多分支时才报），因此 `v-if="mine"` 的回执、`v-if="fileDragOver"` 的拖拽提示层等**彼此独立的合法写法不会误报**。测试会用该检测器扫描 `src` 下全部 46 个 `.vue`，测试数 108 → **114**
- **「存储与缓存」整块是死 UI，数字永远为 0**：统计与清理策略都指向 `cache/` 目录，但 **P1 重构后图片/文件改落 downloads，已无任何代码往 `cache/` 写入**——于是「当前缓存 N 个文件」恒为 0、两个下拉与「立即清理」操作的都是空目录（连 footer 承诺的"删除最旧的图片/文件"也没真正发生过）。现在：
- **设置页底部文案过期**：仍写「清除聊天数据仅删除本机消息与会话」，但该操作还会**退出所有群聊**（此前只更正了二次确认弹窗里的文案，这行漏了）
- **气泡内表情偏小**：源图多为 96×96、少数 108×96，`object-contain` 下**实际墨迹比盒子还小**，1.15em 时与正文几乎同高，高分屏更显小。放大到 **1.3em**（14px 正文下 18.2px），混排更和谐
- **消息过长会被静默截断**：`send_message` / `send_group_message` 都用 `chars().take(50000)` **悄悄切掉超出部分**——用户以为整段发出去了，对方只收到前半段，本机也不留任何痕迹（违反 `AI_RULES` INV-005「不允许静默丢失」）。现在改为**超限直接报错拒发**，错误文案给出实际字符数与上限并建议分段/改用文件，与 `MAX_OUTGOING_IMAGE_BYTES`「超限一律报错拒发，绝不静默截断」的既有约定一致。上限仍是既有的 **5 万字符**（按字符计，UTF-8 下最大约 150 KB 落库），即**对正常消息零行为变化**
- **媒体被「存储清理」删掉后 UI 显示成坏图**：清理只删文件、不动消息，于是历史消息里的图片变成一个永远转圈/裂开的框、文件点开只报"打开文件失败"——用户会以为是对端发来的文件本身有问题，也不知道可以重新索取。现在新增 `media_present` 命令（复用 `read_file_preview` 的路径解析与安全边界，**只有确知文件已被删除**时才判定"已清理"），图片显示「图片已被清理 · 可向对方重新索取」占位，文件卡片显示「已被清理」并**去掉下载入口**（文件不会"等对方上线"自己回来）。在途的乐观消息一律按"存在"处理，绝不会被误标
- **暗色主题下启动仍有一片亮色闪**：窗口的 `backgroundColor` 是**静态**色值，只能在浅色/深色里二选一——改深色会让浅色用户闪一下黑。改为窗口以 `visible: false` 创建，由前端在挂载完成后立刻显示：此刻 `index.html` 的内联骨架（含主题判断）已在 DOM 且样式生效，**窗口露出来的第一帧就是骨架**。Rust 侧另有 4s 兜底（仅在确定处于隐藏态时才强制显示），前端初始化异常也不会留下"应用启动了却没有窗口"的死局
- **「添加好友」的扫描列表看不到第 201 个之后的节点**：列表此前硬截断在 200 行，超出的节点只能靠搜索找到——对设计规模（500–1000 节点）等于不可见。现在行上加了 `content-visibility: auto`（离屏行不参与布局与绘制），上限提到设计规模本身；保留上限只为挡住异常膨胀的节点表

---

## [2.0.2] - 2026-09-10

### Fixed

### Fixed
  - 统计改为**真实媒体目录**（接收的图片/文件，含历史遗留 cache 目录）+ **聊天数据库占用**，字段更名 `media_count/media_bytes/db_bytes`；「文件存储目录」行不再把长路径塞进副标题（改为截断 + 悬浮看全）
  - 保留时长 / 占用上限**真正作用于图片与文件**（恢复文案原本承诺的语义），并在改为非「永久/无限制」前**二次确认**，明确告知"历史消息里的对应图片/文件将无法再打开"；默认值不变（永久 + 无限制 = 不自动删除任何东西）
  - 「立即清理」结果文案区分情况：无可清理时明确说"当前设置下无需删除"，而不是丢一句"删除 0 个文件"
  - 已实测 13/14/16px 三档字号下，1.15em 与 1.3em 的**气泡渲染高度完全一致**（38.8 / 42.0 / 37.1px），因此不影响 VirtualList 的高度估算
- **Windows 标题栏窗口按钮的 hover 底与窗口圆角之间夹出一条缝**：上一版试图"对齐全局圆角"，给最小化/最大化/关闭三键加了 8px 圆角——结果**按钮自己的圆角曲线与窗口边界（外框 `rounded-xl` 12px）曲线不重合**，右上/右下角就夹出一条浅色月牙缝。现在**按钮不加任何圆角**，完全交给外框容器的 `rounded-xl + overflow-hidden` 裁切：两者曲线重合，hover 底与窗口边界严丝合缝（原生 Windows 的窗口按钮同样是整块矩形、由窗口圆角裁切）。组件注释已写明"不要在这里加圆角"及原因
- **表情面板里的表情太挤**：面板是 8 列 36px 格子，但**间隙只有 4px**（表情宽度的 1/9），整片网格看着"贴死"。现在间隙提到 **8px**（留白翻倍），面板宽度 340 → **360px** 与之精确配合；**表情尺寸与可视行数都不变**（8px 间隙下 300px 高度仍正好是 7 行）
  - ⚠️ 面板宽度与网格是**精确配合**的：内宽 `360 − 16(p-2) = 344 = 8 列 × 36(h-9) + 7 间隙 × 8(gap-2)`。列宽由 `1fr` 决定、格子却是固定 px，改格子或间隙时必须同步改面板宽度，否则格子会溢出列宽导致表情互相重叠
- **群里「@我」不高亮，且与「@其他人」样式不一致**：渲染端 @ 高亮用的成员名列表取自 `nicknameOf(我的 device_id)`——而我既不在自己的好友表、也不在 peers（那是"别的节点"），于是**回退成设备指纹**，别人 `@我的昵称` 匹配不上、只剩纯文本，跟 `@其他人` 的色块并列就很割裂。改为「自己」这一项直接取本机昵称
- **「完整文本」弹窗里的 @ 完全不高亮**：`linkify` 没传成员名 → 同一条消息在气泡里高亮、在弹窗里是纯文本。弹窗已接收成员名并复用同一套提及配色
- **@提及不够明显**：浅色气泡上 16% 淡底不够显眼，但**不能**靠加大底色——底色本身是 `currentColor` 的低透明度，加大它等于压低「文字/底色」对比度、跌破 `chatStyle.mentionHighlightColor` 守住的 ≥4.5:1 护栏。改为加一圈同色 1px 内描边：只强化边界、不动填充，对比度不受影响
- **气泡内表情显得拥挤**：表情 1.25em（14px 正文下 17.5px）比正文大一圈，气泡上下几乎没留白、连排表情还彼此贴死。改为 **1.15em + 横向 1px 间隙**
  - 选"缩小表情"而非"加大气泡内边距"：内边距会改变**所有**文本气泡的实际高度，必须同步 `previewMetrics.TEXT_BUBBLE_PADDING`，否则相邻消息错位；而表情尺寸受 `line-height` 约束，**不影响气泡高度与 VirtualList 的估算**
- **右键菜单可以同时展开多个（浮层未全局互斥）**：消息右键菜单的展开态原本由**每个消息项各自维护**，而右键触发的是 `contextmenu`、**不会触发 `click`**——靠 document click 关闭的兜底收不到通知，于是「先右键 A 再右键 B」会出现两个菜单并存；同类还有「消息菜单 + 好友菜单」「消息菜单 + 已读弹层」「消息菜单 + 表情面板」。现在把展开态收敛到全局注册表（`utils/popupRegistry.ts`：同一时刻只允许一个浮层展开），并统一接入**消息右键菜单 / 好友右键菜单 / 会话列表「+」下拉 / 群聊「已读成员」弹层 / 输入框表情面板**——任意一个打开都会自动收起其它浮层
  - 顺带修掉「点另一条消息的已读头像时，上一条的已读成员弹层不收起」
- **Windows 标题栏窗口按钮的 hover 底与窗口圆角之间夹出一条缝**：上一版试图"对齐全局圆角"，给最小化/最大化/关闭三键加了 8px 圆角——结果**按钮自己的圆角曲线与窗口边界（外框 `rounded-xl` 12px）曲线不重合**，右上/右下角就夹出一条浅色月牙缝。现在**按钮不加任何圆角**，完全交给外框容器的 `rounded-xl + overflow-hidden` 裁切：两者曲线重合，hover 底与窗口边界严丝合缝（原生 Windows 的窗口按钮同样是整块矩形、由窗口圆角裁切）。组件注释已写明"不要在这里加圆角"及原因
- **代码气泡的描边与尖角对不上**：代码卡片原来四边都有 1px 描边，右侧那条竖线正好贴在尖角旁，尖角像"贴上去的"。现在消息流里的代码卡片**去掉描边**——`CodeBlock` 新增 `attached` 语义（无描边 + 下圆角抹平，与下方操作条拼成一整块，操作条只留一条上分隔线）；尖角改取 `toolbar` 的等效色（尖角整段都落在 32px toolbar 高度内，取卡片本体色会比相邻 toolbar 亮一档、仍能看出台阶），做到融合无感。独立的「全文」弹窗仍是带描边的独立卡片
  - 同步修正 `previewMetrics` 的高度常量（卡片不再计边框）：截断高度 145→144、完整高度减 2px，保证 VirtualList 的估算与真实渲染仍然一致
- **群密钥不再因群主离线而死（群聊可用性）**：`group_add_member` 原先在加人时轮换群密钥，而 `handle_group_key` 只接受**群主**分发的密钥（这道校验用于防止成员伪造密钥劫持群聊，不能放宽），于是新密钥只存在于群主本机——群主一旦离线，其余成员永远拿不到，整群消息都无法解密。现在**加人不再轮换**（加人本无前向保密收益：新成员本来就没有旧密钥），改用当前密钥重发；轮换只保留在「移除成员」路径（撤权必需，且该时机群主必然在线）
- **身份密钥冲突改为用户可见告警**：同一 `device_id` 报出与已绑定值不同的公钥时（对方重装 / 有人冒名顶替），原先只在开发者诊断面板留痕，用户无感知。现在会在与该好友的会话里写入一条系统消息提示，并按 `device_id` 去重（announce 每 5s 一次，不去重会刷屏）。安全行为不变：**绝不覆盖**已绑定公钥
- **下载目录重名兜底会静默覆盖用户文件**：`unique_path` 在同名文件已达 999 个时直接返回原路径（必定已存在）→ 改为退回随机后缀，保证任何分支都不返回已存在的路径
- **误杀合法局域网地址**：删除 `handle_incoming` 中 `octets()[0] != 169 && octets()[1] != 254` 这处判断——它把「169.x 且 x.254」错写成两个独立条件（会误杀 `10.0.254.x`），且与 `is_virtual_ip()`（已覆盖 `169.254.0.0/16`）完全冗余

### Changed
- **互斥量中毒不再级联崩溃**：`std::sync::Mutex` 一律改用 `lock().unwrap_or_else(|e| e.into_inner())`（共 340 处）。此前任一处**持锁 panic** 会让互斥量中毒，之后所有加锁点都会跟着 panic，整个应用不可用；现在取回内部数据继续运行，把影响限制在最初那次 panic
- **前端消息缓存加上界**：消息缓存按会话累积且只增不减，聊天对象一多内存会持续增长。现在只保留活跃会话 + 最近使用的至多 3 个会话的内存副本（切回时从 SQLite 重新加载最新一页），淘汰判定抽为纯函数 `selectCachedConversations` 并有单测锁定
- **左右两栏头部的分隔线对不齐**：左栏列表头是 `px-3 py-2` + `h-9` 搜索框（**52px 且无底边线**），右栏 `ChatHeader` 是 `--gosslan-header-h`（56px）+ `border-b` → 永远差 4px，且右栏那条线在左栏没有对应物。左栏改用同一个 `--gosslan-header-h` 与同款 `border-b`，两栏分隔线合成一条（渲染实测同在 y=93.00）
- **群聊昵称与头像的视觉不在一条线上**：外层 flex 无 `items-*`（头像顶边 = 昵称行盒顶边），而 `text-[11px]` 继承行高 1.5 → **半行距把墨迹往下推约 3.7px**，看着"名字偏低"（中英文都有，实测 +3.0/+4.0px）。昵称行改 `leading-none` + `mb-[7px]`（总高仍 18px = `messageHeight.NICKNAME_ROW`，不影响虚拟列表估算），墨迹贴住行盒顶
- **切会话会先闪一句"暂无消息"**：消息尚未加载完时 `messages[convId]` 是空数组 → 走了空态分支。现在渲染加载骨架（过渡态而非空态），并给 `loadMessages` 补失败终态，避免骨架永久停留
- **设计系统 token 化（圆角 / 状态色 / 排版）**：圆角统一到 `--gosslan-radius-xs|sm|md|lg|xl|pill`（99 处 Tailwind 字面值 1:1 换算，零观感变化），并落实 iOS 的**同心圆角**规则（内圆角 = 外圆角 − 内边距；修掉菜单项 6px、表情格 6px、代码卡片顶部 8px 三处"内角外翻"）；危险/警告色改用语义 token（Apple 系统色，浅深两套，替换 61 处硬编码 `#e81123`/`red-*`/`amber-*`）
- **原生手感基线（触摸 / 按压 / 动效）**：关掉点击高亮、`touch-action: manipulation`（保留捏合缩放）、`overscroll-behavior` 防整页回弹；`:active` 里把过渡时长归零 → **按下瞬时反馈、松手平滑回落**（并列的可点 `<div>` 行也通过 `.cursor-pointer:active` 补上反馈）；新增 `prefers-reduced-motion` 支持与 `.tap-safe`（触屏小按钮垂直扩到 44px）
- **新增触觉反馈 `utils/haptics.ts`**：按 Apple 触觉词汇分档（light / selection / heavy / success / warning / error），**按下即反馈且不滥用**；接入发送消息、切换会话、长按菜单。Android 走 `navigator.vibrate`（已加 `VIBRATE` 权限），iOS/桌面自动 no-op
- **排版标尺（对齐 Apple HIG Text Styles）**：定 `--gosslan-text-title|body|callout|footnote|caption`（15/14/13/12/11px），**11px 为正文下限**；把 8/9/10px、13.5px 共 20 处散值收进标尺
- **新增 `docs/design-guidelines.md` 并接入 `AI_RULES.md` §42**：圆角 / 交互态 / 触觉 / 配色 / 排版 / UI 优先（不可阻断渲染）全部成文，新功能默认遵循
## [2.0.0] - 2026-09-10

> **2.0 版本：安全加固版 —— 修复「任意节点可冒用他人身份建链」的高危漏洞，并补齐群聊生命周期管理。**
> 单聊/群聊消息内容一直受 E2EE 保护，但**握手身份此前未做密码学校验**：局域网内任意设备只要
> 知道群主/好友的 `device_id`（announce 广播即可获得），就能冒名建立 TCP 链路，进而伪造
> 明文控制消息（如 `GroupMemberRemoved`）把别人的群从本地删除。本次为 `Hello` 握手加入
> Ed25519 签名与防重放校验，在建立链路前完成身份认证。同时新增「群主转让」与「退出群聊」，
> 解决群主换设备后群永久无法管理的问题。
>
> ⚠️ **破坏性变更（与 1.x 不兼容）**：`Hello` 握手新增签名要求，**2.0 与 1.x 无法互通**，
> 互聊设备需升级到同一版本。按 pre-release 规则不保留旧协议兼容分支。

### Added
- **群主转让与退出群聊**：新增命令 `transfer_group_creator`（仅当前群主，目标须为群成员）与 `leave_group`（群主须先转让，非群主可退并清理本地群记录/会话/群密钥）；新增协议 `GroupCreatorChanged` / `GroupMemberLeft` 及对应处理器（校验 `from` 为本地记录的当前群主/成员）；群成员面板新增「转让群主」与「退出群聊」入口。修复「群主换设备/卸载后群永久无法改名、加人、踢人」的僵尸群问题

### Security
- **TCP 握手身份认证（`Hello` 签名）**：`Hello` 新增 `nonce` 与 Ed25519 `sig`，签名覆盖 `device_id | tcp_port | nonce | x25519_pubkey | ed25519_pubkey`（`protocol::hello_signing_bytes`，经 serde_json 序列化避免字段拼接歧义）。接收方在**建立链路之前**验签：用 `friends`（权威）/ `peers` 中该 `device_id` 已绑定的 Ed25519 公钥校验，自报公钥必须与绑定值一致；两者皆无时才走 TOFU 用自报公钥验签。新增有界 FIFO 防重放（不依赖墙上时钟），验签失败直接拒绝连接。
  - **修复的漏洞**：此前 `Hello` 里的 `device_id` 完全未经验证即被用作链路标识，且 `handle_message` 对 `GroupMemberRemoved` / `GroupRename` / `FriendRemove` / `FriendAccept` 等**明文控制消息**只做 `from == peer_id` 的绑定校验 —— 局域网内任意设备只要在 `Hello` 中冒用群主/好友的 `device_id`（announce 广播即可获得），就能伪造 `GroupMemberRemoved` **把别人的群从本地删除**，或恶意改名、删除好友关系，并抢占链路造成失联。
  - ⚠️ **破坏性协议变更**：新旧版本的 `Hello` 互不兼容（新版本要求签名）。按 pre-release 规则不保留兼容分支；请确保互聊设备升级到同一版本。

### Fixed
- **设置页「清除数据」提示与实现不符**：提示文案仍写「不会退出群聊」，但 `clear_all_data` 早已会删除群记录/成员/群密钥（即退出所有群聊）。文案已更正，并明确列出「退出所有群聊（群聊会从列表移除）」。

## [1.2.0] - 2026-09-10

> **1.2 版本：抖音表情 + 图片相册预览 + 设置页 iOS 化重构，并修复局域网发现、群图片接收等真机回归。** 表情不再依赖 Unicode、改用 214 张抖音图片统一三端渲染；消息图片支持相册式浏览；设置页统一为 iOS 分组卡片风格；同时根治「头像撑爆 UDP 广播导致搜不到设备」与「群图片接收方加载失败」两个顽疾。

### Added
- **抖音表情**：214 个抖音评论区表情以图片形式统一三端渲染（不依赖 Unicode），输入框新增抖音式表情弹窗（上下滚动网格），选中插入 `[名字]` 语法；正文中 `[名字]` 与文字自然混合排版，正方形副格子 + `object-contain` 保持原图比例（兼容非正方形源图），只匹配已知表情名、不误吞 `[图片]`/`[代码]` 占位
- **图片相册预览**：点击消息里的图片，打开会话内全部图片的相册浏览，左右箭头 / 键盘 ←→ 切换，保留滚轮缩放 / 拖拽 / 保存 / Esc

### Changed
- **设置页 UI 重构（iOS 分组卡片风）**：抽出 `SettingsGroup` / `SettingsRow` 标准组件统一七个分区，灰底白卡 + 行分隔，分区按「个人资料 → 外观 → 聊天显示 → 网络 → 共享目录 → 存储 → 安全 → 关于 → 重置」优先级重排；头像改为可点 + hover 相机遮罩
- **标题栏平台化**：macOS 左侧红黄绿「红绿灯」（模拟系统样式，悬停整组显示符号），Windows/Linux 右侧三键（关闭键顶到窗口右缘、去掉空隙）

### Fixed
- **局域网发现失效（Message too long / EMSGSIZE）**：UDP announce 曾携带完整 base64 头像，超过 UDP 报文上限导致广播发送失败、节点互相搜不到；现从 announce 移除头像（发现只需 device_id/nickname/公钥/tcp_port），头像改由 TCP 建链后的 UserInfo 同步，且 `upsert_peer` 对 None 头像不覆盖、双向兼容旧版本
- **群图片接收方「加载失败」**：群文件 `GroupFileDone` 只 emit 带本地 path 的记录、从不更新 messages 表，`read_file_preview` 按 msg_id 反查 content 拿到 Offer 阶段无 path 的旧内容而失败；现 Done 阶段显式回填 content 的 path 与 status（与单聊 FileDone 一致）
- **头像存储爆炸风险**：头像此前无大小限制地以 base64 落库并广播；现前端限制 2MB + 中心裁剪 + 512×512 PNG 无损压缩，后端 `update_profile` 兜底拒绝超限头像
- **Cmd+W / Ctrl+W 关闭窗口**：macOS Cmd+W、Windows/Linux Ctrl+W 均可关闭窗口（隐藏到托盘），前端 keydown 兜底，不依赖系统原生菜单对无边框窗口的 Cmd+W 支持是否生效

## [1.1.0] - 2026-09-09

> **1.1 版本：夜间模式 + 微信式聊天 UI 全面优化，以及图片消息彻底移出 SQLite（P1）。** 图片不再以 base64 内联存储——粘贴/接收的图片经「本地文件 + 文件传输链路」流转，`kind` 保持 `image`，协议与 schema 零变更；同时修掉 macOS 图片粘贴无反应、附件图片无预览、发送方图片无回显三个真机回归。

### Added
- **夜间模式（深色主题）**：全项目暗色适配；暗色下「我的气泡」经 softenMineForDark 压暗，气泡/画布/文字对比度护栏卡死 ≥4.5:1
- **群聊 @提及**：输入框升级 contenteditable，@成员转为内联原子 token（主题色高亮、退格整删、光标原生管理）；新增 @成员选择器（模糊过滤/键盘导航）；被 @ 的会话在列表显示红色 [有人@我]
- **图片消息链路重构（P1）**：粘贴/拖拽的图片 data URL 仅作前端临时输入 → Rust `save_outgoing_image` 解码落盘本地文件（MIME 校验 + ≤8MiB + UUID 文件名）→ 复用既有 1:1 / 群文件传输链路 → SQLite 只存 JSON 元数据 `{name,path,size,subtype:"image"}`；新增 `delete_file` 清理发送失败孤儿文件
- **文件交互（微信式）**：文件气泡右键「复制文件」把 CF_HDROP 写入系统剪贴板（资源管理器可直接粘贴）；输入框粘贴真实文件直接发送（位图回退图片分支）

### Changed
- **微信式聊天 UI 全面优化**：气泡头像侧 CSS 尖角；文件气泡按类型着色图标（8 类）、整卡点击打开、下载态切换；图片气泡 loading/failed 占位；智能时间分割线（今天/昨天/星期/跨年）并删除消息底部常驻时间行
- **配色体系重构**：亮色改 luma 定标（二分 HSL 让任意主题色气泡亮度稳定在 207±2）、饱和度上限 0.62 雾感；派生统一走 HSL；头像颜色/取字全项目按昵称哈希统一

### Fixed
- **macOS Ctrl+V 图片粘贴无反应**：WKWebView/Safari 的 paste 事件里 items 为空、位图只经 `clipboardData.files` 暴露，此前按 items 判断误判为纯文本；现 `classifyPaste` 优先 files、items 兜底
- **附件图片无预览（仅图标）**：`read_file_preview` 在 macOS 上把 raw 字节 JSON 序列化成 `number[]`，`new Blob([number[]])` 被强转成 "137,80,78,…" 字符串导致图片损坏；现统一 `new Uint8Array(raw)` 归一成字节再消费
- **图片发送方无回显**：图片气泡 `<img loading="lazy">` 叠加 `hidden`（display:none）——懒加载图片在隐藏态永远进不了视口懒加载距离，`@load` 永不触发、骨架占位永不解除；现移除 `loading="lazy"`，图片即时解码显示
- **代码模式 Enter 无法发送**：代码消息按钮 `@mousedown` 抢占输入框焦点，Enter 激活按钮而非触发编辑器发送；现 `@mousedown.prevent` 保持编辑器焦点
- **macOS Cmd+W 无法关闭窗口**：`decorations:false` 使窗口缺失 Closable 位，系统「关闭窗口」菜单项（Cmd+W / performClose:）不可用；现窗口创建后补回 Closable 位，Cmd+W 恢复且关闭动作仍走托盘隐藏路径
- **e2e 测试基建修复**：新增 ensure_test_friend 用本次运行身份真实公钥预置好友关系；ensure_test_group 清理 group_files 残留；恢复图片粘贴回归用例

## [1.0.1] - 2026-09-09

> **1.0.0 之后的稳定性修复集合。** 聚焦两件事：Windows 上"重启/退出后再打开就永久掉线"的 LAN 顽疾，以及 1.0.0 UI 收敛留下的配色/头像一致性问题。

### Fixed
- **Windows 重启/退出后 TCP 59992 无法重新绑定导致永久掉线**：退出时监听 socket 与已建立连接没有及时释放，端口处于残留占用状态，重新启动后 `bind` 失败且无法自愈。现关闭流程显式 shutdown/close 监听与连接，并在绑定失败时按 `AddrInUse` 做退避重试与诊断上报（`transport.rs` / `mod.rs` / `tray.rs`），重启后可正常重新入网
- **Auto 模式下 UDP Discovery 绑定 `0.0.0.0` 导致对端收不到 announce**：多网卡（VPN / 虚拟适配器）环境下，广播与组播出口会被默认路由劫持到错误网卡，表现为"本机在线但节点列表恒为 0"，而 TCP 59992 却双向可达。现 Auto 模式绑定探测到的真实 LAN IP；同时不再吞掉 `set_multicast_if_v4` / `join_multicast_v4` 失败，启动即报错；`send_to` 失败记录 `broadcast_error` 事件，不再伪造 `broadcast_sent`；诊断中新增 `bound_ip` 展示实际 UDP 绑定地址（TCP 仍监听 `0.0.0.0:59992`，行为不变）
- **停止网络时端口释放不及时**：shutdown 信号在 accept / reader / discovery 的 `select!` 中不占优，停止后线程仍阻塞在读写上，端口要等超时才释放。现让 shutdown 分支优先，停止即时生效
- **文件气泡按钮配色错乱**：文件气泡中的"打开/另存"等按钮没有跟随气泡配色预设，深色/自定义配色下对比度不足。现统一走气泡配色变量
- **消息区对端头像不刷新**：会话头像更新后聊天区气泡仍用旧头像。现消息区与列表共用同一头像来源
- **默认头像底色不统一**：无头像用户此前每次渲染可能得到不同底色。现按名字哈希稳定取色，同一用户全局一致（连带会话列表、好友列表、群成员面板、转发弹窗、个人资料页）
- **聊天正文链接未自动识别**：纯文本 URL 不可点击。新增 `utils/linkify.ts`（含单测）自动识别并渲染为链接；引用块（`>` 引用）配色随之统一，跨气泡配色一致
- **文件气泡图标方块不跟随配色**：图标底板写死颜色，改用 `currentColor`，跨气泡配色自动继承

## [1.0.0] - 2026-09-08

> **正式版 1.0.0 发布。** 本版本在 UI 收敛之上，完成了 LAN Chat 稳定性与跨设备一致性：
> 群消息离线补发、文件离线补发与完成确认、每会话逻辑序号（Lamport）、传输优先级双队列、
> Android 安全区/竖屏/运行时权限、最新消息优先分页、文件进度条修复。

### Changed
- **视觉收敛：会话列表与聊天区分层、选中态改浅灰、去刺眼主色填充**（在上轮纯白极窄版之上纠偏）：
  - 三栏恢复**层次感**：侧栏浅灰 `--gosslan-rail #f2f3f5`、会话列表 `--gosslan-list #f4f5f7`、聊天区 `--gosslan-chat #fff`，告别"三块纯白靠细线"的空旷感
  - **选中态彻底去掉主题色填充**（用户反馈太丑）：`--gosslan-list-active` 由主色改为中性浅灰 `#e2e4e8`（深色 `#38393d`），会话/好友选中行不再整行变蓝/白字，文字保持深灰；侧栏激活项由主色整块改为**浅色胶囊 + 主色图标**（`--gosslan-rail-active` 改白/浅灰）
  - 侧栏加宽 52→**64px**，图标/头像加大（44px 触控块、图标 22px、本人头像 40px），激活项 `rounded-xl` + 轻投影，不再逼仄空旷
  - 列表头搜索框改**白底 + 细边框胶囊**（在浅灰栏上清晰可辨），聚焦描主色
  - 头像圆角 4→6px、气泡圆角 4→6px（更协调不锋利）
- **视觉改版：对齐微信 4.0 Windows 桌面版（按截图精确还原，自定义能力全部保留）**：此前在 `f223dbb` / `b2d6e0e` 基础上按用户截图校准的过渡版本（纯白三栏 + 52px 极窄栏 + 选中主色填充），已在本版本收敛为上述更耐看的层次化设计。**气泡配色预设、主题色取色器与色板、字号、紧凑模式等自定义项全部保留，出厂默认保持经典蓝**
- **顶部窗口条 & 聊天输入区再校准（微信 4.0）**：
  - 顶部拖拽条背景由纯白改为浅灰 app 底色 `--gosslan-app-bg`，高度收敛为 `--gosslan-title-h:30px`；盖在浅灰列表上自然融合、盖在白聊天区上呈微信那种"浅灰 caption 浮于内容上方"，不再是一条割裂的白色横杠；窗口按钮加宽到 44px 触控，关闭键 hover 走系统红 `#e81123`
  - 输入区图标改 `rounded-md`、放大到 19px、去 30px 空高挤得更紧；文本域去掉固定 72px 初高/`rows`，改 `min-h-7` 随内容自然生长（发送/清空仍即时自适应）；发送键由"大号纯白文字+高填充胶囊"改为微信式小圆角主色按钮，未输入时降为灰块禁用（主题色仍可自定义，语义不变）
- **前端组件拆分（纯结构重构，交互与视觉不变）**：`MessageItem`（668 行）拆为 `components/message/` 下的 Avatar / TextBubble / CodeBubble / FileBubble / ImageBubble / Receipt / ContentModal，并把「气泡配色与连续消息合并判定」「文件与附件预览」「复制反馈」抽成 `composables/useMessageDisplay`、`useMessageFile`、`useClipboard`；`ConversationList` 拆出会话行 / 好友行 / 好友申请 / 右键菜单，搜索抽为 `useConversationSearch`；`ChatWindow` 拆出 `ChatHeader` / `MessageComposer` / `RenameGroupModal`，行高估算抽为 `utils/messageHeight.ts`（与 `previewMetrics` 同源）；`SettingsPanel` 拆为 Profile / Appearance / ChatStyle / Network / Storage / Security / About 七个分区，并新增基础开关 `SettingsToggle`。行高估算与渲染仍共用同一套常量，虚拟列表定位与气泡高度不受影响

### Fixed
- **移动端列表↔聊天切换挤压内容**：原先靠宽度动画（`w-0` ↔ `w-full`）切换，动画期间列表内容被横向压缩。改为整屏抽屉 + `translate-x` 滑动
- **移动端底部导航与内容区未适配安全区**：导航本身有 `env(safe-area-inset-bottom)`，内容区却固定 `pb-16`，全面屏机型底部会被导航遮住。现内容区按 `calc(4rem + env(safe-area-inset-bottom))` 留位
- **移动端软键盘弹出时底部导航浮在键盘上方遮挡输入**：监听 `visualViewport`，键盘弹出即收起底部导航并收紧内容区留白
- **移动端进入好友资料页后无返回入口**：资料页加移动端返回条，可回到列表
- **移动端无法删除好友**：`contextmenu` 在触屏不会触发，现长按好友 500ms 呼出同一菜单（桌面右键行为不变）

## [0.13.1] - 2026-09-08
### Fixed
- **Android APK 构建失败（v0.13.0 起 CI 连续失败）**：自绘标题栏的窗口控制命令 `window_minimize` / `window_toggle_maximize` 调用了 Tauri 仅桌面端提供的 `WebviewWindow::minimize` / `maximize` / `unmaximize`，交叉编译到 `*-linux-android` 时报 E0599（`is_maximized` 与 `hide` 两端都有，因此只有这两个命令受影响）。现沿用仓库既有的 `focus_window` 做法补 `#[cfg(mobile)]` 实现：移动端无独立窗口概念，`window_minimize` 为 no-op、`window_toggle_maximize` 返回 `false`；两个命令在桌面/移动两端仍然都注册，桌面实现逐字未改。前端 `TitleBar.vue` 本身以 `v-if="!app.isMobile"` 只在桌面渲染，移动端行为无任何变化，桌面端功能不受影响

## [0.13.0] - 2026-09-07

### Fixed
- **聊天气泡辨识度与连续消息间距**：对方气泡统一使用更明显的蓝灰底色并增加边界；连续消息的时间标签移动到发送者消息组末尾，避免对方首条与第二条之间被额外撑开；虚拟列表通过真实 DOM 高度和 `ResizeObserver` 校正文本、代码、图片、文件及群聊回执布局，避免消息互相遮挡
- **未上线版本继续兼容明文聊天**：删除明文 `GroupMessage` 线路，直连聊天载荷不带 `enc1:` 时直接拒绝；用户聊天不再存在旧版本明文兼容分支
- **群聊已读只显示单一状态的问题**：新增成员级 `GroupReadReceipt` 和本地 `group_reads` 持久化；自己发送的群消息显示已读成员头像，最多展示 3 个头像，超出显示 `+N`，点击可展开完整成员列表
- **前端首屏包体过大**：改用 Highlight.js core + 按需语言注册，图片查看器改为异步加载，并按 Vue、Tauri、图标、代码高亮和通用依赖拆分 Rollup chunks；入口 JS 从约 1.27MB 降到约 124KB
- **文件接收的完整性与路径安全**：拒绝穿越文件名、非法/乱序/越界分片、超出声明大小的数据和不完整连接；断链会清理临时文件并报告失败，完成前同步临时文件后再原子改名
- **局域网消息身份与权限校验**：TCP 帧中的设备身份、好友关系、文件/共享目录请求和 Ack 目标均与当前连接绑定；Gossip 先验签再去重，避免伪造消息污染去重缓存；检测到已知设备公钥冲突时不再静默覆盖
- **Gossip 信封身份冒充与元数据篡改**：签名现在覆盖不可变信封字段（TTL 除外），并要求信封公钥与 Discovery/Hello 绑定的 device_id 一致；旧版仅签 message_id 的信封不再接受
- **持久化与前端竞态**：文本发送的消息和 outbox 进入同一事务；启动时损坏的身份密钥会重新持久化；文件失败事件、事件监听注册竞态和 Ack 先于文件气泡落地的状态竞态已修复
- **中继与共享目录边界**：拒绝空文件/非法中继分片，校验中继文件大小和落盘错误；共享目录枚举不再跟随符号链接
- **输入约束**：好友请求、群成员/群名、消息类型、分页范围、预览大小和文件类型增加服务端校验

## [0.12.0] - 2026-09-07
### Fixed
- **同一消息经 Direct + Gossip 双路径到达时重复计未读、重复弹通知**：`handle_gossip` 只做网络层内存去重（`gossip.is_new`），从不查业务库；而 `touch_conversation` 的 `unread = unread + ?` 是无条件累加。于是「Direct 先落库 → Gossip 后到」这一顺序下，数据库靠 `INSERT OR IGNORE` 只留一行，但**未读又 +1、`message-received` 又发一次**（前端 `mergeMessages` 会丢掉重复气泡，却仍会重复计未读并重复发系统通知）。现两条路径统一由新增的 `db::insert_message_if_new` 裁决——判定与插入合并在同一条 `INSERT OR IGNORE` 的受影响行数里完成（不是先查后写），只有 `fresh=true` 的一方执行 `touch_conversation(+1)` 与 `emit("message-received")`；Direct 的 Ack 与 fresh 无关（消息已在库中即代表成功接收），Gossip 的 TTL fan-out 在落库判定之前、完全不受本地是否已有影响，两层去重保持独立。新增 5 条回归测试覆盖 Direct→Gossip、Gossip→Direct、同一 msg_id 重复投递、8 线程并发同一 msg_id 只有一个首次插入者，以及「业务层判为重复」与「传播层仍判为首次见到、可继续 fan-out」两条件同时成立
- **落库裁决三态化：数据库真故障不再被误当成「重复」而照常 Ack**（与上一条同批修复的加固）：`insert_message_if_new` 明确区分 `Ok(true)` 新插入 / `Ok(false)` 唯一约束命中 / `Err` 数据库故障，调用方不得再用 `unwrap_or(false)` 把 `Err` 折叠成 `false`。Direct 分支据此在 `Err` 时**既不投递也绝不 Ack**——Ack 会让发送方删除 outbox 行，一次临时 SQLite 故障就此变成永久丢消息（与 P0-2 同源的红线）。Gossip 的单聊与群聊共用同一落库块，`GossipKind::Chat` 与 `GossipKind::Group` 均已分别覆盖测试。累计 9 条回归测试。另把 `Message::GroupMessage` 的 TCP 落库分支也套上同一裁决，至此三条落库路径（单聊直发 / Gossip / 群聊直发）副作用判定完全一致，不再存在无条件计未读并投递的群聊形态代码
- **E2EE 解密失败永久烧掉真实 msg_id（消息内容不可恢复）**：直连单聊 `ChatMessage` 解不开时（缺对方公钥 / 对方已换身份 / 密文损坏），旧实现把 `[加密消息] …` 占位文本**以原始 `msg_id` 写入 messages 并照常回 Ack**。后果是双重的：① Ack 让发送方删除 outbox 行，重发机会就此终止；② 之后携带同一 `msg_id` 的正确副本被 `INSERT OR IGNORE` 静默吞掉，明文永久不可恢复——即「对方重装/换密钥后发来的第一条消息永远看不到」。现改为：解不开即**不落库、不 Ack**（Ack 严格保持「已成功接收并持久化」语义），`message_exists` 判定提前到解密之前；outbox 行保留由 Hello/心跳继续补发，且补发前用**当前最新公钥**从本地明文重新密封（`msg_id` 保持不变），使「暂时缺公钥」「发送方换身份」「接收方换身份」三类场景全部自愈。新增 11 条回归测试覆盖正常解密、缺公钥、双方各自轮换、密文损坏/篡改、重复投递、Direct×Gossip 竞态、以及 outbox 身份只认 msg_id
- **局域网发现退化为广播热循环（三端 CPU/流量异常，疑似「界面卡顿」主因之一）**：自适应广播周期（5/10/20s）此前从未真正生效——广播循环每轮都重建 `tokio::time::Interval`，而新建 Interval 的首个 tick 立即就绪，等于把等待清零，`announce` 以约 1ms/轮的速率连发（实测同版本 tokio 复现）。后果是 UDP 广播+组播风暴、单核打满，以及与 UI 线程争抢 `peers` 互斥锁。现改为按固定 deadline 等待（`sleep_until`），周期只在一次广播真正发出后按节点数重算，`who_has` 探测分支不再改变节拍；`shutdown` / `probe` 分支语义保持不变。自适应分档与抖动边界、以及「不得在循环内重建计时器」这一时序前提均已补单测
- **快速连续发送时最后几条永久停在「发送中」（Ack / 真实记录丢失）**：`send()` 上屏的乐观记录用 `tmp-*` 占位 msg_id，真实 msg_id 只在 `invoke` 返回后由 `replaceMessage` 就地替换；而该记录走的是 rAF 批量队列，**invoke 完全可能先于批次落地返回**——此时 `messages.value[convId]` 里还找不到 `tmp-*`，旧实现直接 `return` 把真实记录**整条丢弃**，气泡就此停在 `sending`（转圈），后端库里却早已是 delivered/read：只有重开会话读库才会恢复正常。UI 繁忙时（连发 5 条）批次被推后，因此表现为「最后 1–2 条一直转圈」。现新增 `pendingReplace` 挂起未落地的替换，由 `applyIncoming` 在批次落地的唯一入口处经 `applyReplacements` 换成真实记录（不新增气泡、不改批量管线本身）。同时 `send_message` 改为**先写 outbox 再广播**（INV-003 原本就是这一顺序）：旧顺序下心跳的 `flush_outbox` 若正好落在广播与插队之间，这一轮直发缺席、Ack 要等下一个心跳（+5s）。另修两处状态回退：`loadMessages` 的会话快照可能早于 Ack/peer-read 落库，旧实现无条件覆盖会把已推进的状态退回「发送中」，现按 `preserveDeliveryStatus` 取两者更靠后者；重投消息带回的迟到 Ack 也不再能把 `read` 改回 `delivered`（`set_message_status` 与前端 `onMessageAcked` 同时收紧）。新增 5 条前端纯函数测试 + 1 条 DB 测试
- **已读回执只在用户再次点击会话时才生效（非实时）**：`mark_read` 的 `ReadReceipt` 是**一次性即时发送**——`let _ = try_send(...)`，未建链或半开链路时直接失败并被静默丢弃，此后没有任何补发路径，对方的绿勾只能等下一次 `mark_read`（即用户再点一次会话）才会更新。现按消息补发的既有机制对称处理：失败的回执暂存到 `state.pending_reads`（同一 peer 只保留最大 `last_read_ts`，已读是单调状态），由 `flush_pending_reads` 在建链 / Hello / 心跳这三个既有触发点冲刷，不新增表、不新增协议消息、不做定时全量刷新。会话内的自动标记已读（收到消息 → 防抖 600ms → `mark_read`）与窗口重新可见补标记链路经复核无误，未改动

### Changed
- **局域网通道默认开启（桌面端）**：此前只有设置 `GOSSLAN_AUTOSTART=1` 才在启动时联网，首次安装必须手动进设置页打开开关才能收发。现启动即按偏好自动开启，偏好复用现有 `settings` 表新增键 `lan_enabled`（**不引入新的配置系统**）：键不存在即视为开启 ⇒ 新装与从旧版本升级都自动联网；用户在设置页关闭后持久化为 `0`，重启保持关闭；「恢复默认设置」清除该键 ⇒ 回到默认开启。绑定地址沿用用户已选网卡 `settings.bind_ip`（自动开启**不**改写该选择），仅当该网卡已不存在（换网络）时回落 `0.0.0.0`；`set_channel_enabled("lan")` 原先硬编码 `0.0.0.0`（会覆盖用户已选网卡）也改走同一路径。`GOSSLAN_AUTOSTART=1` 行为保持不变（强制 `0.0.0.0`、且不改动已持久化的偏好），`examples/e2e_peer.rs` 依赖不受影响。移动端本轮保持手动开启不变

## [0.11.2] - 2026-09-05
### Fixed
- **消息排序错乱（同发送者消息堆叠，Mac↔Windows 时钟偏差）**：接收方落库直接采用发送方时钟的时间戳，设备间时钟不一致时消息会插到错误位置（视觉上同一人的消息堆在一起）。现接收侧对每条消息做双向钳制——不晚于本地当前时间（防对方时钟快、消息出现在「未来」）、不早于会话内最后一条消息（防对方时钟慢、消息插到历史之前）；毫秒级精度，同毫秒由自增 id 稳定排序兜底。直发单聊 / 群聊 / Gossip 三条落库路径统一处理
- **设置面板滚动区被滚动条挤压**：滚动容器右侧留白 12px→16px 并补左侧 4px，6px 宽滚动条不再遮挡内容 1–2 像素

### Changed
- `mark_read` 的会话最新时间查询复用新增的 `db::last_message_ts`（消除重复 SQL）

## [0.11.1] - 2026-09-05
### Changed
- **文档体系重构**：`AI_PROJECT_HANDOFF.md` 全面重写为「项目全景与开发指南」（面向 AI 编程 / 源码阅读 / fork 二次开发：完整功能清单、架构代码导读、E2EE 状态机、工程约定、测试口径、v0.1→v0.11 演进时间线、演进设想与已知限制）；README 功能特性表补齐 v0.5–v0.11 全部功能（回执/托盘/删除会话/E2EE 恒开等）、修正过时的 npm 脚本说明与版本示例

## [0.11.0] - 2026-09-05
### Changed
- **端到端加密（E2EE）恒开且不可关闭**：单聊 / 群聊消息始终经 X25519 + ChaCha20-Poly1305 加密，聊天窗口顶部恒显绿锁徽标；设置面板「安全」区的开关移除，改为说明文字（加密原理 + 需先获取对方公钥的行为说明）。发送侧不再读取 `e2ee_enabled` 设置（旧库残留键在「恢复默认」时清理），并消除「对端需同样开启才能互通」的限制——接收方解密只需发送方公钥，旧版关闭 E2EE 的对端也能正常解密

## [0.10.0] - 2026-09-05
### Added
- **删除聊天记录**：消息列表每条会话项右下角新增「×」按钮（hover 行时浮现），点击弹出二次确认弹窗（提示仅本地清理、不影响对方、不影响好友关系），确认后删除该会话全部本地消息与会话行；为乐观交互（先更新 UI，失败回滚 + toast），同时若被删会话是当前打开的会话则关闭并置空 `activeConv`，避免空窗口

### Fixed
- **E2EE 公钥缺失发送失败**：原逻辑「好友表无公钥即报错『未获取对方公钥』」会让 Mac→Windows 等跨网首次加密消息直接失败；现 E2EE 开启且好友表无公钥时，自动触发一次节点探测（who_has）等待 1.2 秒后重试（让对方/中继 announce 落库），仍缺失则返回更明确的错误指引（对方离线 / 临时关闭 E2EE 后明文发送）
- **E2EE 解密失败静默丢消息**：开启 E2EE 的一方发来的 `enc1:` 消息若接收方缺公钥或公钥已更新，原逻辑 `return` 直接静默吞掉消息；现改为写入一条系统消息「[加密消息] 尚未获取 {id} 的公钥... / 解密失败...」，让用户看到失败原因而非消息凭空消失
- **E2EE 关闭时仍要求对端公钥**：原代码无论 E2EE 开关都要求对方 X25519 公钥，导致关闭 E2EE 时也报「未获取对方公钥」；现已修正为关闭时不查公钥、不派生共享密钥（性能优先，明文路径无密码学开销）

### Changed
- **消息回执移到气泡左侧**：我发出的消息回执（转圈/空心圆/绿勾/红叉）从气泡**右侧**改到**左侧**，贴近头像方向更直观（与聊天人头像同侧）
- **消息时间默认显示 `MM-DD HH:mm`**：从「HH:mm」改为更显眼的日期+时分格式（hover 时仍切换到秒级 `YYYY-MM-DD HH:mm:ss`），同分钟合并的连续消息仍仅在 hover 时显示完整时间
- **移除文本右键「复制文本」菜单**：气泡 hover 已自带复制按钮，再加右键菜单冗余；现禁用文本右键自定义菜单，恢复系统默认行为

## [0.9.0] - 2026-09-05
### Added
- **系统托盘（关闭即最小化到托盘）**：Windows / macOS 点击窗口「×」不再退出进程，改为隐藏主窗口并驻留右下角（Windows）/ 菜单栏（macOS）托盘，后台继续收发消息与通知；托盘菜单提供「显示主窗口」与「退出」，**只有选择「退出」才真正结束进程**；左键单击托盘图标也可恢复窗口；macOS 点击 Dock 图标在无可见窗口时同样恢复窗口。托盘为桌面端专属（`#[cfg(desktop)]`，Cargo 开启 `tray-icon` feature）

## [0.8.0] - 2026-09-05
### Added
- **端到端加密（E2EE）开关 + 可视化**：设置面板新增「安全」区，E2EE 开关默认**关闭**（局域网可信场景性能优先）；开启后单聊/群聊载荷经 X25519 + ChaCha20-Poly1305 加密（信封新增 `encrypted` 标志，缺失时按已加密兼容旧版本）；开关影响说明内嵌设置页；聊天窗口顶部新增锁形徽标（绿锁「端到端加密」/ 灰开锁「未加密」）实时反映状态
- **修复直发链路明文不一致**：此前只有 Gossip 路径加密、直发 ChatMessage 帧为明文；现直发内容统一受 E2EE 开关控制（加密内容带 `enc1:` 前缀，接收方用发送方 X25519 公钥解密），关闭时两条路径同为明文

### Changed
- **设备指纹前缀改为 `gosslan-`**（原 `dev-`），设置页「关于」完整显示指纹（不再省略号截断，等宽字体可选中复制）。⚠️ 破坏性：升级后设备 ID 变化，需重新添加好友（本地聊天记录保留）
- 设置面板滚动条与内容间距优化（内容区加内边距，不再被滚动条挤压）

## [0.7.0] - 2026-09-05
### Added
- **代码块渲染升级**：highlight.js 自动检测语言（常用语言子集 + 置信度阈值），无法识别时兜底纯文本；代码自动换行、**移除横向滚动条**；超过 7 行自动折叠（渐隐 + 「展开全部 N 行」按钮）；标题栏显示语言与行数
- **悬浮完整时间戳**：鼠标悬停消息时，时间行从「HH:mm」切换为「YYYY-MM-DD HH:mm:ss」；同分钟合并的消息悬浮也可查看完整时间
- **协议演进文档 `docs/protocol-design.md`**：对标 BeeBEEP（发现冗余/每连接会话密钥/断点续传）与 bitchat（Noise XX/BLE 无配对 Mesh/Store-and-Forward）的差距分析与演进路线（安全/完整/及时三维度），为蓝牙第二通道与协议抽象化提供依据

### Fixed
- **消息可靠性（Mac→Windows 丢消息根因）**：`send_message` 改为**一律写 outbox 兜底**（原逻辑仅「无直连链路」入队，链路存在但已失效/半开 TCP 时 broadcast 静默丢包且无补发）；`flush_outbox` 改为**只补发、不删行**（原「try_send 返回 Ok 即删」在半开链路上同样丢消息），outbox 行仅在收到对方 Ack 时删除，接收方按 msg_id 幂等去重
- **消息排序**：乐观消息时间戳用「发送时刻」并以会话内最新消息时间做下限钳制，消除设备间时钟偏差导致的排序错乱
- **后台消息滞留**：消息合并批处理的 rAF 调度在窗口不可见时会被浏览器暂停，改为不可见时退回 setTimeout；窗口重新可见时冲刷滞留批次
- **已读回执时机**：窗口从后台恢复时自动补发当前会话已读回执（对齐 bitchat「send read receipts on focus」实践），消除「只有长时间后的第一条消息有回执」的现象
- 文本气泡悬停复制按钮加 `whitespace-nowrap`，修复短气泡内图标换行挤压

### Changed
- **乐观交互推广**：好友同意/拒绝、删除好友改为「先更新界面、失败回滚 + toast」；文件发送拿到 transfer_id 后立即乐观上屏气泡（大小/进度随传输记录回填），消息发送此前已乐观化——所有聊天与局域网交互优先「先假定成功、失败可感知」
- **输入区重排**：代码模式/发送文件按钮与「Enter 发送 · Shift+Enter 换行 · 支持粘贴图片」提示移到输入框下方一行，发送按钮改为「图标+文字」置于行尾；输入区底部间距加宽（pb-4/px-4）
- 消息回执（转圈/空心圆/绿勾/红叉）从时间行移到**气泡侧面**挂载
- 同一分钟内同一发送者的连续消息自动合并紧凑显示（不依赖紧凑模式开关）
- 首屏分页由 300 条调整为 **100 条**，上滑按需加载更多（最多 10 页）

## [0.6.0] - 2026-09-05
### Added
- **消息发送状态回执链路**：发送中转圈 → 空心圆（对方已收到/未读）→ 绿色对勾（对方已读），失败红叉 + toast 提示；协议新增 `ReadReceipt(from, to, last_read_ts)`，`mark_read` 改为 async 并向单聊对端发送已读回执；对端将「我发出的、ts ≤ last_read_ts」的消息置为已读并推送 `peer-read` 事件；会话打开期间收到新消息自动去抖标记已读并发送回执
- **文件消息内嵌传输进度条**：发送/接收中实时显示百分比，完成后自动消失（消除"发送像卡死"的感知）
- **对方在线状态全链路可视**：会话列表单聊头像角标（绿点=在线可连接 / 灰点=离线）、聊天窗口头部对方头像 + 同款角标与「对方在线/离线」文字、联系人列表离线头像置灰
- **本人在线状态迁位**：从原聊天区顶部拓扑栏移入「设置 → 个人资料」与左侧导航头像角标（绿点），明确标识是"我"的状态

### Changed
- 移除聊天区顶部拓扑栏（节点数等信息保留在「设置 → 网络通道」；消除平均延迟"—"占位符的误读）
- 输入框体验：内容换行自动增高（上限约 5 行后滚动）、底部留白修正、打开会话自动聚焦输入框（移动端不弹软键盘）
- 消息发送改为**乐观上屏**：点击发送立即显示，后端确认后替换为真实记录

## [0.5.1] - 2026-09-05
### Fixed
- **Mac 上发送/接收消息不刷新（关键）**：消息合并走 Web Worker，在 Tauri 生产构建（WKWebView 自定义协议）下 Worker 可能加载失败——`mergeInWorker` 的 Promise 永不 resolve，发送与接收的消息全部卡在合并步骤不显示（须重开会话走查库路径才恢复）。修复：移除消息路径上的 Worker（合并为 O(n) Set 去重，微秒级），改主线程同步合并，保留 rAF 批量节流
- **发送失败静默无反馈**（表现为「点发送没反应」）：`sendMsg` 增加错误捕获与 toast 提示，失败时保留草稿

## [0.5.0] - 2026-09-05
### Added
- **聊天滚动与定位**：消息区仅纵向滚动（禁横向溢出）；滚动事件 rAF 节流 + passive 监听；`scrollToIndex` 支持跳到任意消息（任意方向无布局抖动）；打开有未读的会话自动定位到**第一条未读**（显示「以下是未读消息」分割线，向上加载历史时分割线索引随偏移）；离开底部显示「回到最新」悬浮按钮
- **长文本折叠**：超过 280 字符的消息默认折叠为 5 行，底部带「展开全文 / 收起」与「复制」按钮
- **聊天样式配置（即点即存 + 跨设备同步）**：6 套可读性配色预设（经典蓝 / 薄荷绿 / 暖阳橙 / 青瓷 / 樱花粉 / 石墨灰，明暗双主题、对比度 ≥ 4.5:1）+ 3 档字体大小 + 消息合并开关；改动立即生效并持久化（重启保留）；变更时经 `ChatStyle` 消息广播到所有已连接节点，**对方按「我的配色」渲染我发的消息**（持久化对端样式表）
- **删除好友**：联系人列表右键 → 删除好友（保留聊天记录，公钥随行移除）；对方仍出现在扫描列表可重新添加；新增 `remove_friend` 命令
- 群聊优化：连续消息合并（5 分钟内同发送者省略头像/昵称/时间戳）、≥5 分钟显示时间分割线、群消息显示发送者昵称
- E2E 新增「聊天样式同步」协议用例；VirtualList 向上加载历史时锚定旧首条消息（视口不跳动）

### Changed
- 消息气泡颜色 / 文字颜色改为样式预设驱动（覆盖原 `--gosslan-bubble-*` 变量的用法）
- `reset_settings` 额外清除聊天样式与对端样式表

## [0.4.2] - 2026-09-05
### Added
- **单机 dev 全功能验证流程**：`scripts/e2e-dev.sh` 一键脚本——无需第二台设备、不依赖 UDP 广播（who_has 单播 + TCP loopback），启动 headless 实例后运行协议级对端，覆盖除网络发现外的全部聊天功能（29 项断言全通过）
- `e2e_peer --full` 扩展模式：代码 / 图片 / 1MB 大文本消息、乱序消息、群消息、心跳保活、UserInfo 资料同步、好友申请（等待 UI 人工同意，SKIP 语义）、共享目录树、**下载方向文件传输**（app 主动发送路径，即「发送文件卡死」修复的回归验证），并补齐对应 SQLite 落库校验（ts 保真、长度完整、群会话行）
- 测试报告支持 PASS / FAIL / SKIP 三态（人工交互项不计失败）

### Fixed
- e2e-dev.sh 在 pkill 后旧进程未退完时 sqlite3 预置 share_dir 偶发失败——加重试

## [0.4.1] - 2026-09-05
### Fixed
- **UDP 发现 socket 阻塞（关键）**：`bind_udp_reusable` 把 socket2 创建的**阻塞** socket 直接交给 tokio——debug 构建直接 panic（发现任务静默死亡），release 构建虽不 panic 但阻塞 fd 挂在 kqueue/epoll 上会卡死 worker 线程（界面卡顿帮凶）。修复：转 tokio 前显式 `set_nonblocking(true)`
- **macOS/Linux 多开互发现失败**：UDP 同端口多绑定在 unix 上必须 `SO_REUSEPORT`（`SO_REUSEADDR` 仅 Windows 有效），socket2 需开 `all` feature；缺它第二个实例 network 启动即报 "Address already in use"
- **跨路径消息重复隐患**：Gossip 落库的 msg_id 带 `g-` 前缀而 outbox 补发为 UUID，两者不同导致极端竞态下重复消息。统一为 Gossip 信封的确定性 SHA-256 ID（本地记录 / Gossip 投递 / outbox 补发三处共用），接收方 `message_exists` 跨路径去重

### Added
- **协议级 E2E 验证工具**：`src-tauri/examples/e2e_peer.rs`——无 GUI 直连真实运行实例，覆盖 UDP 发现、TCP 建链、直连消息 + 去重、Gossip E2EE（X25519+AEAD+Ed25519）、文件传输全链路、outbox 离线补发，并校验 SQLite 落库与文件落盘（15 项断言，全部通过）
- `GOSSLAN_AUTOSTART=1` 环境变量：启动即自动开启局域网通道，支持 headless 多实例联调
- `lib.rs` 开放 `crypto` / `protocol` 模块供测试对端复用

## [0.4.0] - 2026-09-04
### Added
- **统一文件发送（自动路由）**：新增 `send_file_auto` 命令——有直连 TCP 链路走直连分片流，无直连自动切换切片中继，均不可达给出明确错误；前端「直接发送 / 中继发送」合并为一个按钮，无需用户选择路线
- **消息触顶分页加载**：虚拟列表滚动到顶部自动加载更早历史（每页 300 条，上限 10 页），带会话切换竞态守卫
- **会话选中态（飞书式）**：会话列表 / 联系人列表选中项左侧主色指示条 + 名称高亮；新增 `ensure_conversation` 命令，打开新好友会话时后端自动补建会话行（修复左侧无高亮项）
- **离线消息队列激活**：全网无连接时消息进入 SQLite outbox，对方上线建链后自动补发（原 `insert_outbox` 为死代码从未启用）
- **TCP 心跳探活**：每 5s 向所有已建链节点发送 Heartbeat，静默断连及时清理（在线状态修正）

### Fixed
- **文字消息发送失败**：好友公钥在加好友后才落库导致 ECDH 失败——`announce`/`FriendAccept` 路径始终同步好友公钥，`send_message` 回退在线节点表取公钥
- **发送文件界面卡死**：切片读取改为 `spawn_blocking`（不再阻塞 async runtime）；发送/接收两侧进度事件 250ms 节流；前端进度事件不再全量 `refreshTransfers()`（消除大文件 IPC 风暴）
- **间歇性连通性故障**：旧连接断开时误删新连接的发送端（按 key 而非按 channel 身份移除），现用 `same_channel` 精确清理；`connect_to_peer` 移除占位 channel 竞态
- **新会话不显示**：收到未知会话的消息时自动从后端刷新会话列表（原逻辑直接跳过）
- **历史遗留单测失败**：`outbox.msg_id` 补唯一索引（含旧库迁移 + 重复行清理）、`reassemble_out_of_order` 改用手工切片绕开最小分片钳制，31 个 Rust 单测全绿

### Changed
- **设置即点即保存**：昵称失焦 / 回车自动保存，缓存策略（保留时长 / 磁盘上限）改动即时持久化；移除「保存资料」「保存策略」按钮
- CI 注入的 `scripts/android/permissions.xml` 移除误带的 XML 声明（曾致 Android Gradle 清单解析失败、APK 打包全红）
- 会话 / 联系人列表行增加 `v-memo`，长列表渲染仅在相关字段变化时更新

## [0.3.0] - 2026-09-04
### Added
- 好友搜索流程：按需 `who_has` 群发探测（仅在打开「添加好友」时触发，启动不持续扫描）；昵称 / IP / 设备 ID 实时过滤；已是好友显示「已加好友」禁用态；500–1000 节点下列表截断渲染 + 搜索
- 全平台系统通知：应用处于后台或非当前会话时触发原生通知（含昵称 + 摘要）；点击通知唤起并聚焦窗口，自动跳转到发送者会话并清零未读
- 新增后端命令 `search_nearby_peers`、`focus_window`
- **双通道聚合传输**：`transport/` 定义 `Transport` 抽象接口 + `TransportManager`（局域网 / 蓝牙独立开关、智能分流、状态汇总），`lan.rs` 适配现有网络层，`bluetooth.rs` 提供 BLE/RFCOMM 接口契约（后端待接入）
- **异构 Mesh 中继**：`relay/mesh_router.rs` 跨链路桥接寻址、TTL 衰减、有界 RingBuffer 限流暂存（可独立测试）
- **轻量存储与缓存清理**：`storage/cache_cleaner.rs` 保留时长（3/7/30 天/永久）+ 磁盘配额自动清理 + SQLite VACUUM；缓存策略设置与「立即清理」命令
- 新增命令 `get_channel_status`、`set_channel_enabled`、`get_cache_info`、`set_cache_policy`、`clean_cache_now`

### Changed
- 聊天/群聊/文件消息的原生通知由 Rust 侧迁移到前端统一做 gating（后台 / 非当前会话），避免重复通知；好友申请类通知仍由 Rust 直接触发
- 新增 `--instance N` 多开启动参数（独立数据库/TCP 端口/设备指纹，UDP 共享 + SO_REUSEADDR），支持单机模拟多节点压测
- 新增跨平台环境配置指南 `docs/setup-windows.md` 与一键脚本（`env:check` / `env:install` / `android:build` / `dist:win:portable` / `multi:run`）

## [0.2.0] - 2026-09-04
### Added
- E2EE 端到端加密：X25519（ECDH 密钥交换）+ Ed25519（签名校验）+ ChaCha20-Poly1305（AEAD）
- Gossip 去中心化广播：Bloom Filter 概率去重 + LRU 精确去重 + Epidemic fan-out + TTL
- 大文件切片中继：64KB–512KB Chunk，BitTorrent 式 Mesh 并行分发、乱序重组
- 群聊群密钥机制：组密钥对称加密，密钥用各成员公钥 ECDH 单独加密分发
- 响应式布局：PC 三栏 / 移动端单栏滑动切换，虚拟滚动，Web Worker 异步解密
- 局域网拓扑状态栏：节点数 / 中继数 / 平均时延
- 项目更名 Lanct → **Gosslan**

### Changed
- UI 组件库由 Ant Design Vue 全面替换为 Tailwind CSS + Headless UI + Lucide
- 面向 500–1000 节点规模做性能优化：节点列表事件节流合并、自适应广播周期、去重参数适配、SQLite WAL 调优

## [0.1.0] - 2026-09-03
### Added
- 基础 P2P 局域网即时通讯：UDP 广播发现、TCP 分帧传输、好友关系、群聊、文件直传、共享目录
