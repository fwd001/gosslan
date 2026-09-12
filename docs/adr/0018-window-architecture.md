# ADR-0018: Window Architecture（一窗一入口 / 后端真相源 / 事件带载荷）

- Status: **Accepted**（2026-09-12，用户明确要求把这三条定下来：「避免以后被改回去」）
- Date: 2026-09-12
- Owners: Gosslan
- Related:
  - 本轮实现：① `396ac7d`（`settings-changed` 带补丁 + `emit_filter`）、
    ② `c97afc8`（运行状态单一快照）、`faa07ad`/`da716b3`（一窗一入口的收尾与打包）
  - 前置决策：ADR-0009（Rust↔TypeScript 契约）、ADR-0015（BLE transport，JNI 桥）
  - 参考实现：`clash-verge-rev`（一个 `index.html` 服务所有窗口 + Rust 侧
    `emit_to_window` 定向发事件）、Tauri 2.x 的 `Emitter::emit_filter` / `filter_target`
  - CHANGELOG: `[4.2.0]`

---

## 1. Context

Gosslan 有两个以上窗口：主窗口、独立「设置」窗口、独立「运行日志」窗口（桌面端）。
每个窗口是**一个独立 WebView**：自己的 JavaScript 上下文、自己的 Pinia store、自己的
渲染循环。它们之间**没有共享内存**，唯一的同步手段是 Tauri 的 IPC（命令调用 + 事件）。

这套结构在过去两周里被真机反馈反复打中，形成四类固定缺陷：

1. **同一件事两份状态**：局域网"开着没有"在界面上有两份前端表示
   （`channels[lan].enabled` 来自 `get_channel_status`，`online` 来自 `get_network_status`），
   由不同命令与事件维护 ⇒ 用户实测「添加好友里把局域网打开，设置里还是关的」。
2. **事件无载荷 ⇒ 每个窗口全量重拉**：`settings-changed` 曾是 `emit(event, ())`，
   各窗口收到后都要 `get_settings + get_device_info + get_share_dir` 三连拉；
   `runtime-changed` 同样无载荷。
3. **事件回发给发起窗口 ⇒ 自我回灌与事件乒乓**：发起窗口收到自己的事件、读到的是写入前的
   旧快照（"点了主题又跳回去"）；更糟的是重拉路径末尾会 `pushUiLanguage()` →
   `set_ui_language()`，而那条命令**也发** `settings-changed` ⇒ 两个窗口互相触发形成高频 IPC 环。
4. **窗口加载错文档**：设置/日志窗口曾经加载主窗口的 `index.html`，再由前端按窗口 label
   把聊天三栏挂起来换成设置页 ⇒ 用户看到「第二次打开设置，窗口先刷成主聊天窗口、又立马变成
   设置界面」「点一下要等很久」。

## 2. Decision

### 2.1 一窗一入口（one document per window）

每个窗口有**自己的 HTML 文档与自己的入口模块**：`index.html` + `src/entries/main.ts`、
`settings.html` + `src/entries/settings.ts`、`logs.html` + `src/entries/logs.ts`。
三者只共享 `src/boot/boot.ts`（主题注入、错误上报、窗口挂载助手）与 `src/style.css`。

**禁止**"一个文档 + 前端按窗口 label 换布局"：那会把三个窗口的首帧成本都变成最慢那个的，
并且让"窗口是哪个"这件事在两个地方各写一份（Rust 的 label 与前端的分支）。

护栏：`每个窗口都有自己的 HTML 与入口（不再共用一个 index.html）`、
`每个窗口只带自己的骨架`、`每个窗口都声明自己的标题`、`每个窗口都必须加载应用样式`。

### 2.2 后端是唯一真相源（backend is the single source of truth）

凡是**多个窗口都能看到的事实**，只能存在后端一份；前端不得自己攒第二份。

- 同一个事实**只有一个读命令**：例如运行状态只有 `get_runtime_snapshot`，
  `get_channel_status` / `get_network_status` 已删除（它们的全部信息都在快照里）。
- 前端每个事实**只有一个写入入口**：例如通道状态 / 在线 / 绑定 IP 只能经
  `applyRuntimeSnapshot()` 写入。
- 派生只允许发生在**渲染层**（computed），不允许在前端再存一份"镜像状态"。

护栏：`runtime_state_has_a_single_source`（Rust）、事件契约测试里的
`运行状态只能有一个快照 + 一个带载荷的事件`（前端）。

### 2.3 常驻单例窗口（resident singleton windows）

设置/日志窗口**懒创建、只隐藏不销毁**（`ensure_aux_window` + `AUX_WINDOWS_RESIDENT` +
`install_hide_on_close`）。关闭 = 隐藏；再次打开 = `show + set_focus`，因此没有重载闪屏。

代价：窗口不会重新加载 ⇒ **常驻窗口必须自己刷新会变的数据**。约定：
- 偏好类数据由 `settings-changed` 事件同步（见 2.4）；
- 环境类数据（设备信息 / 网卡 / 目录 / 运行状态）在窗口**重新获得焦点**时由
  `refreshEnvironment()` 一次性并行拉取（四项互不依赖，用 `allSettled`，任一项失败不影响其它）。
- 窗口标签是**单例键**：同一标签重复打开只是聚焦，不会出现两个设置窗口。

### 2.4 事件带载荷 + 定向发送（payload + targeted）

窗口间同步事件必须满足三条：

1. **带载荷**：载荷是接收方**可以直接应用**的最小完整信息，而不是"某件事变了"。
   - `settings-changed` → `{changed: [...], origin, settings: {…只含变了的键…}}`
     接收方零 IPC 应用；`changed: ["*"]` 表示全量（"恢复默认"这类删键操作）。
   - `runtime-changed` → `RuntimeSnapshot`（通道 / 在线 / 绑定地址 / 蓝牙事实 / 节点数）。
   - `data-cleared` → `{origin}`（破坏性操作后，其它窗口重建自己的列表）。
2. **不发回发起窗口**：用 `emit_filter(...)` 按窗口标签过滤，发起窗口**根本收不到**。
   发起窗口需要新值就直接用**命令的返回值**（`set_channel_enabled` / `start_network` /
   `save_settings` 等都返回/应用新状态）。
   ⇒ 由此**删除**了 `settingsDirty` / `lastLocalWriteAt` / grace 一整套"防自我回灌"状态机。
3. **不重发**：事件处理里不得再调"会发同一个事件"的命令（`set_ui_language` 就是因此
   去掉了自己的 emit），否则就是 1.3 的乒乓环。

护栏：`设置变更必须带补丁 + 不回发给发起窗口`、`运行状态事件必须带快照、且不回发发起窗口`、
`每个改设置的后端命令都必须传 origin`、`清空数据必须广播`（均在 `scripts/verify-guards.py`
里各有一条非空转用例）。

### 2.5 为什么**不用** BroadcastChannel

`BroadcastChannel` 能在同源 WebView 之间直接传消息，看起来能省掉后端中转。我们不用它：

1. **它绕过后端 ⇒ 制造第二真相源**。谁开通道、谁在广播，真相在 Rust（`AppState` + 平台
   运行时）。前端互相广播"我以为开了"只会在两边都错的时候一起错，而且错得一致、更难查。
2. **它无法与后端状态原子**。例：打开局域网必须是"改后端 + 拿到新运行状态"，
   `BroadcastChannel` 只能广播"我点了"，另一个窗口再去问后端 —— 那正是 1.1 的两份状态。
3. **它到不了原生侧**。托盘菜单、macOS 菜单栏、Android 生命周期都在 Rust 侧；
   跨进程/跨平台同步必须走 Tauri 事件。两套通道 = 同一件事两条路径。
4. **它没有目标过滤与类型约束**。Tauri 的 `emit_filter` 能"排除发起窗口"，
   载荷还能被 TS 类型约束（`SettingsChanged` / `RuntimeSnapshot`）；
   `BroadcastChannel` 只能"全发"，回灌问题得靠前端自己加守卫（我们刚删掉的那套）。
5. **它不解决首帧**。窗口首次加载时没有任何历史消息，仍然要向后端拉一次 ——
   既然总要有一条后端通道，就不需要第二条。

> 参考对照：`clash-verge-rev` 用"一个文档 + Rust 事件 + `emit_to_window`"；我们采纳它的
> **后端为真相源 + 定向发事件**，但在"文档"上刻意做得更细：我们的三个窗口骨架差异极大
> （聊天三栏 / 设置分区 / 日志表格），共用一个文档会让每个窗口都背别人的首帧成本。
> 这与 2.1 并不矛盾 —— 两条路线的共同点是**窗口与文档一一对应、事件由后端定向发**。

## 3. Consequences

**收益**
- 不可能再出现"同一件事两份状态"：想加一个跨窗口可见的事实，就必须在后端加一个命令/事件，
  前端没有别的地方可放。
- 跨窗口同步从"每个窗口三连拉"变成"零 IPC 应用载荷"；实测顺带消灭了事件乒乓环。
- 少一整套前端状态机（`settingsDirty` / grace 窗口），少两个命令
  （`get_channel_status` / `get_network_status`），少一个类型（`NetworkStatus`）。

**代价与边界**
- 快照必须**保持精简**：完整节点列表（`peers`）仍走独立的 `peers-updated`
  （它最多 3/s、只在脏时推，见 `emit_peers`），快照里只带 `peerCount`；
  否则每次通道开关都会搬一遍全表。
- "谁需要被通知"要显式设计：新增跨窗口事实时，必须同时回答
  「哪个命令返回它 / 哪个事件带它 / 谁不该收到」。

## 4. Status

Accepted。三条护栏（2.1 / 2.2 / 2.4）都已落地为**非空转**测试；
若将来有人把某个窗口改回"共用文档"、把某个事实改回"两份前端状态"、
或把某个事件改回"无载荷广播"，`verify-guards.py` 与 `npm test` 会直接报出来。
