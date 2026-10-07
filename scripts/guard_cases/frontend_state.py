#!/usr/bin/env python3
"""护栏非空转用例分册：前端状态与事件（stores / utils / api / IPC 事件扫描）。

本册 19 条 / 316 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
`CASES` 字面量里）按**锚定的被守物**切出来。块文本逐字未搬动过一字 ⇒
恒等判据＝`verify-guards.py --list` 的输出排序后与拆前**逐字节相同**（条数与用例名都不是"我觉得一样"）。

⚠️ 三条硬规矩（都是这仓自己踩出来的形状）：
1. 加一条护栏就加进**对应这一域**的本册；域由 `file=` 锚点决定，不按"谁方便找"。
2. 分册必须被 `guard_cases/__init__.py` 的 `MODULES` 点名 —— 那里有一条**起跑前就会炸**的对账：
   目录里的模块集合 != 名单 ⇒ `ImportError`。漏点名的后果不是报错而是**那几条护栏从此不跑**（假绿）。
3. `check-doc-numbers.mjs` 的判据 E 现在按 `scripts/guard_cases/*.py` 里的 Case 构造行现算条数，
   与契约图上那一格对账；数到 0 它自己 throw（尺子坏了要比被测物先响）。

路由规则按**路径段**匹配，不许用裸 substring：`TodoCardBubble.vue` 里含 "ble"、`FriendProfile.vue`
里含 "file" —— 第一版就是这么被错分进蓝牙册与文件册的（各 2 条），改成段/主干相等才干净。
"""

from __future__ import annotations

from .base import ROOT, TAURI, Case, cargo, npm

CASES: list[Case] = [
    Case(
        name="自动拉起蓝牙必须尊重用户的关闭偏好（退出重进不能又打开）",
        why="真机 2026-09-14：电脑端设置里关掉蓝牙，退出重进又被 ensureBluetoothOn 自动拉起。"
            "判据必须用持久化偏好 preferred，而不是运行时 enabled/running —— 启动瞬间必然没在跑，"
            "只看运行状态就会把用户的关闭选择覆盖掉。",
        file=ROOT / "src" / "stores" / "useAppStore.ts",
        injections=[(
            "      if (!ch.preferred) return;",
            "      // 关闭偏好判断被移除（护栏注入）",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="偏好",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="桌面通知必须走后端命令（不能依赖被插件替换的 window.Notification）",
        why="Tauri 的 notification 插件把 window.Notification 换成转发到 plugin:notification|notify，"
            "onclick 永远不触发、且把真正的 toast 错误 spawn 掉丢了 —— Windows 同事『收不到通知』查无实据。"
            "现在统一 api.notifyDesktop（失败可返回/记录），并修复隐藏窗口下 hasFocus 仍为 true 的漏通知。",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[(
            "void api.notifyDesktop(title, body, convId).catch(() => {",
            "void Promise.resolve().catch(() => {",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="notify_desktop",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="在线状态必须包含有活跃链路的节点（不能只看节点表）",
        why="用户 2026-09-14：局域网直连上了，好友在线状态却不实时。前端原来只按\"在不在 peers 表\""
            "判在线，而有链路但广播没收到（防火墙/组播限制）或刚被 sweep 的节点会被判离线。"
            "2026-10-03：这段判定从 store 私有函数搬到 utils/friendOnline.ts（原先被抄成两份，"
            "searchNearbyPeers 那份漏了 linkedIds），注入锚点随之跟到新家 —— "
            "要盯的是\"判定必须含 linkedIds\"，不是它住在哪个文件。",
        file=ROOT / "src" / "utils" / "friendOnline.ts",
        injections=[(
            "f.online = onlineIds.has(f.device_id) || linkedIds.has(f.device_id)",
            "f.online = onlineIds.has(f.device_id)",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="活跃链路",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="通道开关必须乐观更新（否则点一下要等后端 2~3s 才动）",
        why="用户 2026-09-13：Mac 上「点了一下，过了好一会儿才会关；再点一下，"
        "过了好一会儿才会开」。根因是开关要等 `await api.setChannelEnabled` 回来才改状态，"
        "而蓝牙启停是 2~3s 级的（`ble::start` 等 CoreBluetooth 状态最多 3s、"
        "`ble::stop` 等扫描任务退出最多 2s）。这条退化的形态很隐蔽 —— 功能还在、只是慢，"
        "所以只能靠守卫钉住顺序：先按用户意图改状态 → 再执行 → 失败回退",
        file=ROOT / "src" / "stores" / "useAppStore.ts",
        injections=[(
            "    channels.value = prev.channels.map((c) => (c.channel === channel ? { ...c, enabled } : c));\n",
            "",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="乐观更新",
        tags=["frontend", "channel"],
    ),
    # ---------------- 前端：设置事件必须"带补丁 + 不回发起窗口" ----------------
    Case(
        name="设置事件不得回发给发起窗口（emit_filter vs emit）",
        why="无载荷广播的话，每个窗口（含刚写完的那个）都要全量重拉三份数据，而且发起窗口会被"
        "自己的旧快照回灌（『点了主题又跳回去』）。真实事故：重拉还会走到 pushUiLanguage → "
        "set_ui_language → 再发一次事件，两个窗口形成高频 IPC 环",
        file=TAURI / "src" / "state.rs",
        injections=[(
            "emit_filter(EVENT_SETTINGS_CHANGED, patch, move |target| {",
            "emit(EVENT_SETTINGS_CHANGED, patch); #[allow(unreachable_code)] let _ = move |target: &tauri::EventTarget| {",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="emit_filter",
        tags=["frontend", "ipc"],
    ),
    Case(
        name="改设置的后端命令必须传 origin（否则发起窗口收到自己的事件）",
        why="同上：只要有一个命令把 origin 写成 None，发起窗口就会被自己的事件回灌 —— "
        "而且它只在『改了设置的那个窗口刚好也在监听』时才现形，很难靠手测发现",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "// 只把**变了的键**发给**另一个窗口**（发起窗口自己已经应用过了，不回发）。\n"
            "    state.notify_settings_changed(&changed, Some(window.label()), patch);",
            "// （注入用例：把 origin 写成 None）\n"
            "    state.notify_settings_changed(&changed, None, patch);",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="没传 origin",
        tags=["frontend", "ipc"],
    ),
    Case(
        name="清空数据必须广播（否则主界面毫无反应）",
        why="用户实测（Mac 4.1.10）：在设置里清了缓存、目录和聊天记录，主界面一点变化都没有 —— "
        "清除只发生在设置窗口自己的 store 里，主窗口是另一个 WebView",
        file=TAURI / "src" / "commands.rs",
        injections=[("    s.notify_data_cleared(Some(window.label()));\n", "")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="data-cleared",
        tags=["frontend", "ipc"],
    ),
    # ---------------- 前端：非聊天页不得判已读（④） ----------------
    Case(
        name="非聊天页不得判已读（④）",
        why="用户 2026-09-12 实测：在聊天界面点进设置页（整页浮层），对方发来的消息自己没看到，"
        "却被判成已读并把回执发了回去",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        # 2026-09-28：这三个条件搬进了 `chatViewerLooking()`（INV-P27：已读与「有人@我」共用一份口径），
        # 所以注入点跟着搬过去 —— 从那份家里摘掉「聊天视图可见」这一半，两条规则会同时变松，
        # 正是这条用例要拦的坏法（旧锚点 `activeConv.value !== convId || document.hidden || ...` 已不存在）。
        injections=[(
            "!document.hidden && app.chatVisible;",
            "!document.hidden;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="聊天视图可见",
        tags=["frontend", "mobile"],
    ),
    # ---------------- 前端：链路标签必须由后端判定（⑤） ----------------
    Case(
        name="「蓝牙直连」不得用『没有 IP』反推（⑤）",
        why="用户 2026-09-12 实测：与 Mac 同一 Tailscale 网段的设备也被标成「蓝牙直连」——"
        "因为界面写的是 `p.ip || 蓝牙直连`；链路类型只有后端知道（Link::path_kind 由来路决定）",
        # 判据在 4.2.19 抽到 `utils/peerConnectionInfo.ts`（资料页与添加好友页共用一份），
        # 所以注入点跟着搬过去：这里模拟"按『没有 IP』反推蓝牙"的旧写法。
        file=ROOT / "src" / "utils" / "peerConnectionInfo.ts",
        injections=[(
            '  if (info.link === "bluetooth") return "peer.link.bluetooth";',
            '  if (!info.ip || true) return "peer.link.bluetooth";',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="连接图标名与文案同源",
        tags=["frontend", "friend"],
    ),
    # ---------------- 前端：我的在线状态 = 任一通道在跑 ----------------
    Case(
        name="在线语义：任一通道在跑 = 在线（两个都关才离线）",
        why="用户 2026-09-12 明确规则：手机蓝牙自动开启，此时即使没连 Wi-Fi 也该显示在线；"
        "两个通道都关了才是离线。用 online（只管局域网）会把这种用户标成离线",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "    let present = list.iter().any(|c| c.running);",
            "    let present = online;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="任一通道在跑",
        tags=["frontend", "ipc"],
    ),
    # ---------------- 前端：运行状态必须"一个快照 + 一个事件"（②） ----------------
    Case(
        name="运行状态事件必须带快照、且不回发发起窗口（②）",
        why="以前 runtime-changed 是无载荷广播，每个窗口收到后都要自己重拉一半状态；"
        "而『局域网开没开』这件事在前端有两份表示（channels[lan].enabled 与 online）⇒ "
        "必然出现『外面开了、里面还是关的』。改成带 RuntimeSnapshot 的 emit_filter 之后没有了",
        file=TAURI / "src" / "state.rs",
        injections=[("emit_filter(EVENT_RUNTIME_CHANGED, snapshot, move |target| {",
                     "emit(EVENT_RUNTIME_CHANGED, snapshot); #[allow(unreachable_code)] let _ = move |target: &tauri::EventTarget| {")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="emit_filter",
        tags=["frontend", "ipc"],
    ),
    Case(
        name="前端不得再调『半份状态』的两个旧命令（②）",
        why="get_channel_status / get_network_status 是同一件事的两份来源；只要前端还能调到其中一个，"
        "就又有可能出现『两处不同步』（用户实测过：添加好友里开了局域网、设置里还显示关）",
        file=ROOT / "src" / "api" / "index.ts",
        injections=[(
            '  getRuntimeSnapshot: () => invoke<RuntimeSnapshot>("get_runtime_snapshot"),',
            '  getRuntimeSnapshot: () => invoke<RuntimeSnapshot>("get_runtime_snapshot"),\n'
            '  getChannelStatus: () => invoke<never[]>("get_channel_status"),',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="半份命令",
        tags=["frontend", "ipc"],
    ),
    # ---------------- 前端：IPC 事件契约 ----------------
    Case(
        name="IPC 事件契约（Rust 发的必须有人听）",
        why="真实缺陷：设置窗口改语言/主题后主窗口不刷新 —— 因为根本没有 settings-changed 事件。"
        "同一类还有 group-message-acked 一直没人接",
        file=ROOT / "src" / "api" / "index.ts",
        injections=[(
            'listen<SettingsChanged>("settings-changed"',
            'listen<SettingsChanged>("settings-changed-typo"',
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="settings-changed",
        tags=["frontend", "ipc"],
    ),
    # ---------------- 前端：store 契约 ----------------
    Case(
        name="store 契约（界面用到的成员必须在 store 里导出）",
        why="用户实测：给 store 新增 channels/refreshChannels 后，dev 里旧 store 实例没有这些成员 ⇒ "
        "设置页渲染抛错 ⇒ 整页卡死（点设置卡、过一会儿弹好几个设置、主题延迟切换）",
        file=TAURI / ".." / "src" / "stores" / "useAppStore.ts",
        # ⚠️ 注入的名字必须是**界面真的在用**的那个导出（当前是 `refreshRuntime`，
        #    用在 `NetworkSection.vue` / `AddFriendModal.vue`）。改成没人用的名字护栏会空转。
        injections=[("    refreshRuntime,\n", "    refreshRuntimeRenamed,\n")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="refreshRuntime",
        tags=["frontend", "store"],
    ),
    # ---------------- 好友申请：已是好友的申请必须自动消失（2026-09-12 用户实测） ----------------
    Case(
        name="好友申请（已是好友的申请必须从「新朋友」消失）",
        why="用户实测：双方互发过申请、一方点同意后，另一方点进「新朋友」那条申请还在。"
        "根因是「同意」各条路径行为不一致；前端再用「人已经是好友」这个事实兜一层",
        file=ROOT / "src" / "stores" / "useChatStore.ts",
        injections=[
            (
                "const pendingRequests = computed(() =>\n    actionableRequests(",
                "const pendingRequests = computed(() =>\n    ((x: unknown) => x)(",
            )
        ],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="按好友过滤",
        tags=["frontend", "friend"],
    ),
    Case(
        name="连接信息按链路类型显示（蓝牙不显示 IP）",
        why="用户 2026-09-13：蓝牙链路原来也显示一行『IP 地址：—』，设备类型还直接显示 "
        "desktop/mobile 英文原值。不同链路该说不同的事实：蓝牙说『蓝牙直连（近距离）』且"
        "**不显示 IP**；局域网/跨网段给 ip:port；中继只说跳数（没有直连地址就不许编一个）",
        file=ROOT / "src" / "utils" / "peerConnectionInfo.ts",
        injections=[(
            '  if (info.link === "bluetooth") return false;',
            "  if (false) return false;",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="蓝牙没有 IP 概念",
        tags=["frontend", "peer", "display"],
    ),
    Case(
        name="事件扫描器必须剥掉 Rust 行注释（v4.25.2 那处修复的非空转证明）",
        why="e8b335a 之前，契约扫描器把 Rust **注释里**写的 `emit(\"…\")` 扫成\"后端在发的事件\"，\n"
        "     于是前端「没有消费者」的例外清单被凭空多出来的假事件牵着走。修法是 `stripRustComments`，\n"
        "     但当时只用手改注入证明过一次，没登记进本脚本 ⇒ 违反「每条护栏都要被证明会失败」。\n"
        "     注入方式就是**回到修好之前的形状**：删掉行注释那一段（块注释仍然剥，所以不是全盘失效）。\n"
        "     夹具 `事件扫描器不被注释骗` 必须红：样例里 `// emit(\"phantom-line\")` 会被扫出来。",
        file=ROOT / "scripts" / "rustSrc.ts",

        injections=[(
            "    if (c === \"/\" && d === \"/\") {\n"
            "      let j = src.indexOf(\"\\n\", i);\n"
            "      if (j === -1) j = src.length;\n"
            "      out += \" \".repeat(j - i);\n"
            "      i = j;\n"
            "      continue;\n"
            "    }\n",
            "",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/api/events.test.ts"],
        cwd=ROOT,
        expect_fail_hint="phantom-line",
        tags=["frontend", "new-guards", "ipc-events"],
    ),
    Case(
        name="剥注释时 Rust 的 raw 字符串必须整段跳过（`r#\"…\"#` 里的 `//` 不是注释）",
        why="这条守的是 raw 字符串**整段跳过**这件事。变异点必须在 `skipString` 内部那一句\n"
        "     `const raw = /^r#*\"/`：把它简化成 `/^r\"/`（看着等价，都叫 raw 字符串），\n"
        "     `r#\"…\"#` 就不再被识别 ⇒ 从 `r` 的下一个引号开始配对，串里那个 `//` 落到代码里\n"
        "     ⇒ 整行被当行注释吃掉 ⇒ 真事件**少报**（少报比多报危险：它会放行本该报警的漂移）。\n"
        "     ⚠️ 外层 `/^r#*\"/.test(src.slice(i, i + 6))` 改坏**不会**红：那是与 skipString 冗余的\n"
        "     第二处识别，实测被 skipString 的回落救回来（整组仍 11/11 绿）—— 这条用例本身就是\n"
        "     这个发现的产物：第一版注入选了外层，跑出来是\"护栏空转\"，换成内层才真正咬住。",
        file=ROOT / "scripts" / "rustSrc.ts",

        injections=[(
            "    const raw = /^r#*\"/.exec(s.slice(from));",
            "    const raw = /^r\"/.exec(s.slice(from));",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/api/events.test.ts"],
        cwd=ROOT,
        expect_fail_hint="real-after-raw",
        tags=["frontend", "new-guards", "ipc-events"],
    ),
    Case(
        name="把三态压回两态（只按昵称判 @）—— 具名用例与护栏必须同时红",
        why="这是 #103 那条缺陷的原样复活：`messageMentionsMe` 里删掉 id 那条分支，\n"
        "     剩下的就是改名前的旧文案匹配。名字可变、可重名 ⇒ 两种静默判错都会回来。\n"
        "     非空转要同时抓到两层：messages.test.ts 的具名用例，以及那条\n"
        "     「昵称判定不许出现在调用点」的护栏。",
        file=ROOT / "src" / "utils" / "messages.ts",
        injections=[(
            """  if (Array.isArray(ids)) {
    // 我的 id 为空时直接判不中：不能让"身份尚未就绪"这一格靠空串匹配到名单里的空串
    return !!me.id && ids.includes(me.id);
  }
""",
            "",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="id 名单不含我",
        tags=["frontend", "group", "stability", "new-guards", "mention-identity"],
    ),]
