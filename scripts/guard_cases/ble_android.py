#!/usr/bin/env python3
"""护栏非空转用例分册：蓝牙数据面与安卓侧（ble*.rs / transport/* / .kt / proguard / jni）。

本册 39 条 / 747 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
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
        name="BLE MTU 吞吐估算（净数据必须扣 6 字节分片头）",
        why="日志里的 KB/s 是给用户的量级预期；算错一个量级会误导排障"
            "（旧的 1KB/s 注释就是例子）",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "let net = payload_budget.saturating_sub(crate::transport::ble_framing::BLE_CHUNK_HEADER_LEN);",
            "let net = payload_budget;",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "throughput_estimate_matches_real_mtu_budgets"),
        cwd=TAURI,
        expect_fail_hint="throughput_estimate_matches_real_mtu_budgets",
        tags=["rust", "ble", "new-guards"],
    ),
    # ---------------- Rust：BLE 载荷预算（常量与换算的唯一事实来源） ----------------
    Case(
        name="BLE 分片载荷预算异常值绝不返回 0（否则链路静默假死）",
        why="返回 0 ⇒ `fragment` 拒绝一切、链路静默假死（真机上表现为「连上了但发不出消息」）。"
        "⚠️ 2026-09-16 换了注入点与命令：原先注入 `bluetooth_peripheral.rs` 的 "
        "`central_payload_mtu`，而那份实现已收敛进 `ble_framing::notify_payload_budget`，"
        "旧锚点随之消失 ⇒ 本用例当时退化成「锚点出现 0 次」的报错。"
        "改注入规范位置后**平台限制也一并去掉**：`ble_framing` 不做平台门控，"
        "所以这条现在在 macOS / Windows / Linux 上都有效，且不再需要 `--features bluetooth`"
        "（`ble_framing` 是 `transport/mod.rs` 里无条件编译的模块）。",
        file=TAURI / "src" / "transport" / "ble_framing.rs",
        injections=[(
            '    let min = BLE_CHUNK_HEADER_LEN + 1; // 至少装得下"分片头 + 1 字节"',
            "    let min = 0;",
        )],
        cmd=cargo("test", "--lib", "notify_payload_budget_clamps_and_never_returns_zero"),
        cwd=TAURI,
        expect_fail_hint="应退回默认而不是返回 0",
        tags=["rust", "ble"],
    ),
    Case(
        name="BLE 读循环必须回灌读活性（否则健康链路 45s 自拆）",
        why="ConnectionHealth 的读活性只在建链时播种、此后只由读循环刷新；BLE 读循环漏了这句"
        "⇒ 任何健康蓝牙链路 15s 后被判不健康、45s 被看门狗当死链路拆掉，对端再拨回来再拆，"
        "无限循环（真机体感：蓝牙时好时坏、加好友/消息过一会儿才到）",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "                    mark_conn_seen(&state, &peer_id, &ep);",
            "                    // 回归：不再回灌读活性",
        )],
        cmd=cargo(
            "test",
            "--features",
            "bluetooth",
            "--lib",
            "ble_reader_loop_refreshes_read_activity",
        ),
        cwd=TAURI,
        expect_fail_hint="mark_conn_seen",
        tags=["rust", "ble"],
    ),
    Case(
        name="外设握手失败必须解除『握手中』标记（否则设备再也加入不进 mesh）",
        why="旧实现只在握手成功（RouteCtl::Add）与对端退订（Unlinked）时清理 handshaking，"
        "握手失败时不清理 ⇒ 该 central 之后的真 Hello 被『已在握手』静默丢弃 ⇒ "
        "那台设备再也连不进来（macOS 外设没有断连回调，条目可能永久残留）。"
        "这类退化不会让任何行为测试失败，只能靠结构护栏盯住",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[("if !ok {\n", "if ok {\n")],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "peripheral_handshake_failure_clears_the_handshaking_mark",
        ),
        cwd=TAURI,
        expect_fail_hint="HandshakeFailed",
        tags=["rust", "ble"],
    ),
    Case(
        name="外设每次订阅都清掉该 central 的重组器（否则重连后第一条消息丢）",
        why="对端 msg_id 每条连接从 1 重来，而 macOS 外设没有 didDisconnect 回调、"
        "didUnsubscribe 也不保证到达 ⇒ 上一轮残留的半截消息会和重连后的第一帧撞车、"
        "那条帧被当坏片丢掉（真机体感：重连后第一条消息丢了）。只靠 30s TTL 兜底太慢",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            "                .remove(&id);\n            let _ = self.ivars().signal.send(1); // 订阅数变化 ⇒ 唤醒等订阅的写任务",
            "                .len();\n            let _ = self.ivars().signal.send(1); // 订阅数变化 ⇒ 唤醒等订阅的写任务",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "peripheral_subscribe_resets_that_centrals_reassembler",
        ),
        cwd=TAURI,
        expect_fail_hint="必须**按 central id** remove",
        tags=["rust", "ble"],
    ),
    Case(
        name="蓝牙启动不得阻塞在 CoreBluetooth 状态回执上（否则开关卡 3 秒）",
        why="`start_peripheral` 要等 `peripheral::STATE_WAIT = 3s`（CoreBluetooth 回报状态），"
        "而 `ble::start` 就在 `set_channel_enabled` 的关键路径上 ⇒ 一旦 await 它，"
        "用户点蓝牙开关就要干等 3 秒（用户 2026-09-13 Mac 实测「点了一下，"
        "过了好一会儿才会开」）。外设角色本来就是独立失败的，必须丢后台任务；"
        "同时句柄要先写进 state.ble，否则「刚开就关」时 stop() 拿不到 handle、发不出停机信号。"
        "⚠️ 2026-09-16 更新锚点：该 cfg 后来加入了 `target_os = \"windows\"`（Windows 外设角色"
        "落地），而锚点仍写着旧的两平台列表 ⇒ 本用例此前是「锚点出现 0 次」的报错状态。"
        "这正是「护栏会静默腐烂、只有跑起来才知道」的又一例。",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "#[cfg(any(target_os = \"macos\", target_os = \"windows\", target_os = \"android\"))]\n"
            "    #[allow(clippy::let_underscore_future)] // JoinHandle 丢弃不影响 spawn 的任务\n"
            "    let _ = tokio::spawn(start_peripheral(state.clone(), shutdown_tx.subscribe()));",
            "#[cfg(any(target_os = \"macos\", target_os = \"windows\", target_os = \"android\"))]\n"
            "    #[allow(clippy::let_underscore_future)] // JoinHandle 丢弃不影响 spawn 的任务\n"
            "    start_peripheral(state.clone(), shutdown_tx.subscribe()).await;",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "ble_start_does_not_block_on_the_peripheral_state_wait",
        ),
        cwd=TAURI,
        expect_fail_hint="不许 await 外设启动",
        tags=["rust", "ble", "perf"],
    ),
    Case(
        name="群消息：非成员中继必须继续转发（不能提前 return）",
        why="旧实现把『我不是群成员』直接 return 掉，位置在转发之前 ⇒ 非成员中继不转发群消息 ⇒ "
        "BLE-only 三点中继（手机—电脑—手机）里群聊永远不通，而同链路单聊正常。"
        "这类退化**不会让任何行为测试失败**，只能靠结构护栏盯住",
        file=TAURI / "src" / "network" / "transport" / "gossip.rs",
        injections=[(
            "    let group_consumable = group_envelope_consumable(\n",
            "    if matches!(env.kind, GossipKind::Group) && !env.group_members.is_empty() {\n"
            "        if !env.group_members.iter().any(|m| m == &state.device_id) {\n"
            "            return;\n"
            "        }\n"
            "    }\n"
            "    let group_consumable = group_envelope_consumable(\n",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "handle_gossip_does_not_bail_out_for_non_members",
        ),
        cwd=TAURI,
        expect_fail_hint="非成员",
        tags=["rust", "mesh"],
    ),
    Case(
        name="未知帧类型必须降级而不是拆链",
        why="Message 是 #[serde(tag=\"type\")] 的内部枚举：不认识的 type 直接反序列化失败 ⇒ "
        "io::Error ⇒ reader 退出 ⇒ 拆链。新版本只要上线一种新帧，老设备就不是「少收一条」而是"
        "「跟这台设备连不上」。这条的行为是**什么都没发生**，没有任何测试会因为缺少它而失败",
        file=TAURI / "src" / "network" / "transport" / "outbound.rs",
        injections=[(
            "decode_frame(&buf).map_err(",
            "serde_json::from_slice(&buf).map_err(",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "unknown_wire_frame_is_tolerated_after_auth",
        ),
        cwd=TAURI,
        expect_fail_hint="数据面 read_frame 必须走 decode_frame",
        tags=["rust", "protocol", "compat"],
    ),
    Case(
        name="BLE 离开 PoweredOn 必须摘掉全部订阅",
        why="CoreBluetooth 不会补发「对端断开」⇒ 订阅状态陈旧会让写任务白等 8s 且日志空白",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            "    let mut out: Vec<String> = subscribed.iter().cloned().collect();\n    out.sort(); // 稳定顺序：日志与单测都好读",
            "    let _ = subscribed;\n    let mut out: Vec<String> = Vec::new();",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "bluetooth_peripheral"),
        cwd=TAURI,
        expect_fail_hint="leaving_powered_on_detaches",
        tags=["rust", "ble"],
        platforms=("darwin",),
    ),
    Case(
        name="BLE 写失败必须重试并拆链路（否则留下能收不能发的僵尸链路）",
        why="真机 2026-09-13 安卓：两条 BLE 会话都就绪后，一阵群 gossip 把链路写满 ⇒ 各出现一次"
        "「写失败 ⇒ 结束该链路写循环」⇒ 从此发不出去（界面报连接已关闭），而读还在正常收 ⇒ "
        "看门狗按读活性判健康、45s 也不拆 ⇒ 只能重启应用。根因是只结束写循环、把链路留成僵尸",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "                    {\n"
            "                        let links = state.links.lock().await;\n"
            "                        if let Some(l) = links\n"
            "                            .get(&peer_id)\n"
            "                            .and_then(|v| v.iter().find(|l| l.endpoint == ep))\n"
            "                        {\n"
            "                            let _ = l.cancel.send(true);\n"
            "                        }\n"
            "                    }\n"
            "                    break;",
            "                    break;",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "ble_write_failure_retries_then_tears_the_link_down",
        ),
        cwd=TAURI,
        expect_fail_hint="最终失败必须去链路表里取消",
        tags=["rust", "ble", "perf"],
    ),
    # ---------------- Rust：Android JNI 签名（跨语言一致性） ----------------
    Case(
        name="Android JNI 签名与 Kotlin 对齐（stop 是 ()V 不是 ()Z）",
        why="JNI 不做编译期检查：描述符写错只在真机抛 NoSuchMethodError —— 真实缺陷是"
        "「关掉蓝牙开关后手机仍在广播」，而且日志里什么都没有",
        file=TAURI / "src" / "transport" / "ble_android.rs",
        injections=[('kotlin_method!("stop", "()V")', 'kotlin_method!("stop", "()Z")')],
        cmd=cargo("test", "--lib", "android_jni_signatures_match_kotlin"),
        cwd=TAURI,
        expect_fail_hint="描述符不一致",
        tags=["rust", "ble", "android"],
    ),
    Case(
        name="Android JNI 签名与 Kotlin 对齐（打开文件桥 openWith）",
        why="同一条铁律的第二座桥：Rust 按 (Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String; "
        "调 OpenWith.openWith。Kotlin 侧少写 `: String?`（返回 Unit）时真机只抛 NoSuchMethodError —— "
        "用户看到「点开文件没反应」，日志里什么都没有",
        file=TAURI
        / "gen"
        / "android"
        / "app"
        / "src"
        / "main"
        / "java"
        / "com"
        / "gosslan"
        / "app"
        / "OpenWith.kt",
        injections=[(
            "fun openWith(path: String, mime: String?): String? {",
            "fun openWith(path: String, mime: String?) {",
        )],
        cmd=cargo("test", "--lib", "android_jni_signatures_match_kotlin"),
        cwd=TAURI,
        expect_fail_hint="描述符不一致",
        tags=["rust", "android"],
    ),
    # ---------------- Android JNI：object 成员必须有 @JvmStatic（否则没有静态桥） ----------------
    Case(
        name="Rust 调的 Kotlin 成员必须带 @JvmStatic（否则 Method not found）",
        why="真机实测：BlePeripheral.start() 漏了 @JvmStatic（它旁边 6 个兄弟都有）⇒ object 里不生成"
        "静态桥 ⇒ Rust 的 call_static_method 报 `Method not found: start ()Z`，蓝牙外设起不来，"
        "而 central 扫描照常 ⇒ 从日志上很容易误判成'权限问题'",
        file=TAURI
        / "gen"
        / "android"
        / "app"
        / "src"
        / "main"
        / "java"
        / "com"
        / "gosslan"
        / "app"
        / "BlePeripheral.kt",
        injections=[(
            "    @JvmStatic\n    fun start(): Boolean = startOnMain()",
            "    fun start(): Boolean = startOnMain()",
        )],
        cmd=cargo("test", "--lib", "android_jni_signatures_match_kotlin"),
        cwd=TAURI,
        expect_fail_hint="必须在它上面加 `@JvmStatic`",
        tags=["rust", "jni", "android"],
    ),
    # ---------------- Android JNI：static 形态必须与 Kotlin 一致（真机启动闪退） ----------------
    Case(
        name="JNI static 形态与 Kotlin 顶层/成员一致（少了 static 就闪退）",
        why="真实缺陷：OpenWith.kt 的 nativeAttachOpenWith 是**文件级函数**（=static），而 Rust 侧 "
        "native_method! 少了 static ⇒ 按实例方法注册 ⇒ ART 在第一次调用时直接 abort 整个进程"
        "（'registered as instance but called as static method'）。编译/单测/构建全绿，只在真机启动时现形",
        file=TAURI / "src" / "android_open.rs",
        injections=[(
            "    static extern fn native_attach_open_with() -> (),",
            "    extern fn native_attach_open_with() -> (),",
        )],
        cmd=cargo("test", "--lib", "jni_static_matches_kotlin_toplevel"),
        cwd=TAURI,
        expect_fail_hint="static 形态与 Kotlin 不一致",
        tags=["rust", "jni", "android"],
    ),
    # ---------------- Android release 包：打开文件桥的 R8 keep ----------------
    Case(
        name="R8 keep（打开文件桥漏 keep 必须报出来）",
        why="OpenWith.openWith 只被 Rust 的 JNI 按名字调用，R8 会把它当死代码改名/删掉 ⇒ "
        "release 真机包「点开文件」NoSuchMethodError（debug 不混淆，开发期完全看不见）",
        # 与蓝牙那条同理：事实来源与注入副本必须**同时**改坏，否则先以"两处漂移"失败，
        # 证明不了"漏 keep 也会被抓到"。
        file=ROOT / "scripts" / "android" / "proguard-gosslan.pro",
        injections=[(
            "    public static java.lang.String openWith(java.lang.String, java.lang.String);\n",
            "",
        )],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public static java.lang.String openWith(java.lang.String, java.lang.String);\n",
                "",
            )
        ],
        cmd=cargo("test", "--lib", "release_keeps_every_kotlin_method_called_from_rust"),
        cwd=TAURI,
        expect_fail_hint="缺少 `openWith`",
        tags=["rust", "android", "release"],
    ),
    # ---------------- Rust：BLE 扫描结果不得按未连接的 services() 过滤 ----------------
    Case(
        name="BLE 发现：不平台级过滤、不按未连接的服务过滤",
        why="用户 2026-09-12 实测两台设备永远搜不到彼此：① 平台层用服务 UUID 过滤时，macOS 把 128 位 "
        "UUID 放在扫描响应里、Android 硬件过滤只匹配主广播包 ⇒ 永远收不到 Mac 的广播；"
        "② 拿 Peripheral::services() 复核时，它在 Android 上只有连接并 discover_services() 之后才有值，"
        "未连接恒为空 ⇒ 候选全被丢掉",
        file=TAURI / "src" / "transport" / "bluetooth.rs",
        injections=[(
            "            .start_scan(ScanFilter::default())",
            "            .start_scan(ScanFilter { services: vec![uuid(SERVICE_UUID)] })",
        )],
        cmd=cargo("test", "--lib", "scan_results_are_not_filtered_by_unconnected_services"),
        cwd=TAURI,
        expect_fail_hint="不得在**平台层**", 
        tags=["rust", "ble"],
    ),
    # ---------------- Rust：BLE 端点身份必须忽略大小写 ----------------
    Case(
        name="BLE 端点身份忽略大小写（否则每轮扫描都重拨、反复打断好链路）",
        why="同一台对端在 macOS 外设角色下是大写 UUID、在 btleplug central 下是小写 ⇒ "
        "去重比较永远不命中 ⇒ 每 13s 重拨一次、每次都替换对端 GATT server 的旧连接 ⇒ "
        "把对端拨来的好链路打断（用户真机：加好友报连接已关闭 / 对面没反应）",
        file=TAURI / "src" / "mesh" / "endpoint.rs",
        injections=[(
            "        self.address.eq_ignore_ascii_case(&other.address)",
            "        self.address == other.address",
        )],
        cmd=cargo("test", "--lib", "ble_endpoint_equality_ignores_case"),
        cwd=TAURI,
        expect_fail_hint="大小写不同的同一地址必须相等",
        tags=["rust", "ble"],
    ),
    # ---------------- Rust：BLE 指定拨号方（两端互拨会互相打断） ----------------
    Case(
        name="BLE 指定拨号方：大 id 拨、小 id 只接受（否则镜像链路互扰）",
        why="用户 2026-09-12 真机「点加好友：发送失败，连接已关闭」：两端都跑 central+peripheral ⇒ "
        "互相拨号形成镜像链路，小 id 拨过去的连接会打断对端拨来的好链路 ⇒ 45s 无帧被看门狗拆掉",
        file=TAURI / "src" / "network" / "ble.rs",
        # 2026-09-13：判据多了 `peer_advertises` 前缀（对端不广播 ⇒ 必须我们拨，
        # 否则只做 central 的 Windows 在 id 更小时两侧都不拨）。注入改成把**整条**
        # 判据置为恒真 —— 这时候"小 id 也会去拨"必须被护栏抓到。
        injections=[("!peer_advertises || my_id > peer_id\n}", "true\n}")],
        cmd=cargo("test", "--lib", "ble_link_has_a_designated_dialer"),
        cwd=TAURI,
        expect_fail_hint="大 id 拨、小 id 只接受",
        tags=["rust", "ble"],
    ),
    # ---------------- Android release 包：R8 不得改掉 Rust 按名字调用的 Kotlin 方法 ----------------
    Case(
        name="R8 keep（JNI 方法漏一个就必须报出来）",
        why="release 开 R8 混淆时 `stop/start/send/isConnected/payloadMtu/…` 会被改名成 a/b/c/d/e，"
        "而 JNI 只按「名字 + 签名」查找 ⇒ 真机 release 包的蓝牙外设整条路径 NoSuchMethodError"
        "（debug 不混淆，所以开发期看不见）",
        # ⚠️ 两个文件必须**同时**改坏：只改一个的话护栏会先以"事实来源与注入副本漂移"失败，
        #    那就证明不了"漏掉某个方法也会被抓到"。
        file=ROOT / "scripts" / "android" / "proguard-gosslan.pro",
        injections=[("    public static boolean send(java.lang.String, byte[]);\n", "")],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public static boolean send(java.lang.String, byte[]);\n",
                "",
            )
        ],
        cmd=cargo("test", "--lib", "release_keeps_every_kotlin_method_called_from_rust"),
        cwd=TAURI,
        expect_fail_hint="缺少 `send`",
        tags=["rust", "android", "release"],
    ),
    # ---------------- 安卓实测缺陷（2026-09-12）：触屏定位 / 通道同步 / 新的朋友 / 蓝牙默认开 ----------------
    Case(
        name="触屏定位（tap-safe 不得压掉组件的 absolute）",
        why="真实缺陷：安卓端「回到最新」按钮写的是 `tap-safe absolute bottom-4 right-5`，"
        "而 style.css 在 @tailwind utilities 之后、`.tap-safe{position:relative}` 与 `.absolute` "
        "特异性相同 ⇒ 触屏设备上按钮掉回文档流、不再贴右下角（桌面 pointer:fine 不复现）",
        file=ROOT / "src" / "style.css",
        injections=[(":where(.tap-safe) {", ".tap-safe {")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="pointer: coarse",
        tags=["frontend", "css", "android"],
    ),
    Case(
        name="通道同步（设置页不得用 app.online 当局域网开关值）",
        why="用户实测：「添加好友里打开局域网，设置里还是关的」—— 同一个概念有两份前端状态"
        "（channels[lan].enabled 与 app.online），两处 UI 各读一份就必然不同步",
        file=ROOT / "src" / "components" / "settings" / "NetworkSection.vue",
        injections=[(':model-value="!!lanStatus?.enabled"', ':model-value="app.online"')],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="app.online",
        tags=["frontend", "android", "channel"],
    ),
    Case(
        name="移动端「新的朋友」必须切主面板",
        why="用户实测：安卓端收到好友申请后点「新的朋友」没反应 —— 申请页在右侧主面板里，"
        "而移动端靠 mobileView 平移切换，不切过去就还停在会话列表上",
        file=ROOT / "src" / "layouts" / "ResponsiveLayout.vue",
        injections=[(
            '  if (app.isMobile) app.mobileView = "chat";\n}\n\n/** 收起「新的朋友」页',
            "}\n\n/** 收起「新的朋友」页",
        )],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="mobileView",
        tags=["frontend", "android", "nav"],
    ),
    Case(
        name="JNI keep 必须是 public static（否则静态桥被 R8 删掉 ⇒ Method not found）",
        why="真机实测：Rust 用 call_static_method 调 BlePeripheral.start()，而 Kotlin 的 @JvmStatic "
        "在 object 里生成『实例方法 + 静态桥』两个条目；keep 规则只写 public boolean start(); 时 "
        "R8 把静态桥当死代码删掉 ⇒ `JNI 调用失败：Method not found: start ()Z`（蓝牙外设起不来）",
        # 事实来源与注入副本必须同时改坏（同既有 keep 用例的理由）
        file=ROOT / "scripts" / "android" / "proguard-gosslan.pro",
        injections=[("    public static boolean start();", "    public boolean start();")],
        extra_injections=[
            (
                TAURI / "gen" / "android" / "app" / "proguard-rules.pro",
                "    public static boolean start();",
                "    public boolean start();",
            )
        ],
        cmd=cargo("test", "--lib", "release_keeps_every_kotlin_method_called_from_rust"),
        cwd=TAURI,
        expect_fail_hint="必须写成 `public static",
        tags=["rust", "jni", "android"],
    ),
    Case(
        name="BLE 重连：Hello 必须换路由重新握手（不能投给旧链路）",
        why="真机 2026-09-12：Mac 反复报『对端首帧不是 Hello』/『握手超时：对端未回 Hello』。"
        "BLE 上同一个 central 地址在重连时复用，旧连接的链路任务可能还没清理 —— 新连接的 Hello "
        "一旦被投给旧链路的管道，旧链路写的是旧连接 ⇒ 新连接永远收不到 Hello 回应。"
        "用户侧表现：蓝牙时好时坏、加好友没反应",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[("    if !has_route || frame_is_hello {", "    if false {")],
        # ⚠️ BLE 代码在 `--features bluetooth` 下才编译，测试必须带这个 feature
        cmd=cargo("test", "--lib", "--features", "bluetooth", "reconnect_hello_must_not_go_to_the_stale_route"),
        cwd=TAURI,
        expect_fail_hint="有活路由 + 收到 Hello",
        tags=["rust", "ble", "network"],
        # 该单测与 `peripheral_route_action` 一样只存在于**有外设角色**的平台
        # （`cfg(any(target_os = "macos", target_os = "android"))`）。
        # Windows 这一轮只做 central（ADR-0015 §7.9），所以此处必须跳过而不是假失败。
        platforms=("darwin", "linux"),
    ),
    Case(
        name="BLE 握手失败必须说出『收到的是什么』",
        why="同一轮真机排查里，日志只有一句『对端首帧不是 Hello』，完全无法区分"
        "『对端重连时把旧链路的帧发了过来』『对端状态机没重置』『对面不是 Gosslan』"
        "⇒ 只能靠猜。central 与外设**两侧**的错误都必须带上收到的类型名，"
        "且类型名要走 `Message::wire_kind()`（与 serde tag 同一份事实来源：手写 match "
        "漏一个变体就会打出错的类型名，比没有日志更坏）",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "最后一帧 type={}",
            "最后一帧（这里曾经不带类型名）",
        )],
        cmd=cargo("test", "--lib", "peripheral_reconnect_hello_replaces_the_stale_route"),
        cwd=TAURI,
        expect_fail_hint="最后一帧的类型",
        tags=["rust", "ble", "diagnostics"],
    ),
    Case(
        name="BLE 拨号退避绝不指数增长到分钟级（否则好友申请等几分钟）",
        why="真机 2026-09-13：好友申请等了 5～6 分钟才到。根因之一是退避被锁到分钟级："
        "BLE 上「连过去被拒」是常态，每次失败把一个**稳定地址**推进下一档，而"
        "「小 id 只接受」又让只有一侧会拨 ⇒ 唯一的拨号通道被锁死。"
        "现在的实现是「前 3 次不退避，之后 5s→10s→20s 封顶」。"
        "⚠️ 2026-09-16 两处更新：① 命令指向新测试名（旧测试已随「缓增 + 封顶」重设计改名）；"
        "② 注入改为**去掉封顶**而不是改 `MAX_MS` 的值 —— 实现里 `step.min(2)` 已经把增长压到"
        "3 档，单改 `MAX_MS` 到 600_000 也到不了分钟级，那样的注入是**空转**的"
        "（改坏了测试照样通过）。这条用例的前一版正因为锚点写死在旧值 60_000 而失效。",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "    (BASE_MS << step.min(2)).min(MAX_MS)",
            "    BASE_MS << step",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "dial_backoff_does_not_starve_retries"),
        cwd=TAURI,
        expect_fail_hint="必须封顶",
        tags=["rust", "ble", "backoff"],
    ),
    Case(
        name="BLE 握手必须容忍前导帧（否则残留帧把链路全部打死）",
        why="真机 2026-09-13：Mac 日志反复 `[GATT] 已就绪 → [DISCONNECT] 对端首帧不是 Hello"
        "（收到 chat_message）` ⇒ 链路永久建不起来（双方各自重拨、互相打断）。"
        "Android 的 notify 按 central 地址投递：上一条链路的待发帧会落在新连接上，"
        "而「首帧必须是 Hello」这条旧判据会把本来能建起来的链路全部打死",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            "    } else if dropped >= MAX_HANDSHAKE_PREAMBLE_FRAMES {",
            "    } else if true {",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "handshake_tolerates_leading_non_hello_frames_but_is_bounded"),
        cwd=TAURI,
        expect_fail_hint="额度过小",
        tags=["rust", "ble", "handshake"],
    ),
    Case(
        name="BLE 拨号去重 + 失败断开（否则叠连接把通知投给没人读的那条）",
        why="真机 2026-09-13：Mac 侧反复 `[GATT] 已就绪 → 握手超时：对端未回 Hello`，"
        "而安卓侧 notify 全部成功。根因是同一对端叠了多条连接（扫描 10s 一轮 vs 握手 10s），"
        "失败又从不 disconnect ⇒ 幽灵连接 + 多个通知流订阅，通知被投给没人读的那条。"
        "这条护栏盯：DialGuard 去重、复用前先断开、失败显式断开、分片级统计存在",
        file=TAURI / "src" / "network" / "ble.rs",
        injections=[(
            '    let Some(_dial_guard) = crate::state::DialGuard::try_acquire(&state, format!("ble:{ble_id}"))',
            "    let Some(_dial_guard): Option<crate::state::DialGuard> = None",
        )],
        cmd=cargo("test", "--lib", "ble_dial_is_deduplicated_and_disconnects_on_failure"),
        cwd=TAURI,
        expect_fail_hint="在途去重",
        tags=["rust", "ble", "connection"],
    ),
    Case(
        name="Android 外设通知必须节流（连发会丢片，帧永远拼不完整）",
        why="真机 2026-09-13 的算术证据：Mac 侧 `[FRAG] 收到通知 38 条 / 747 字节`，"
        "而 742 字节的帧在 MTU=23（每片 14 字节载荷）下需要 53 片 ⇒ 丢了 15 片 ⇒ 永远拼不出"
        "完整帧，表现是「安卓收到了好友申请并加上了，Mac 什么都没发生」。"
        "notifyCharacteristicChanged 连发会被 Android 协议栈丢包，必须每片留一个连接间隔",
        file=TAURI / "src" / "transport" / "ble_android.rs",
        injections=[(
            "                tokio::time::sleep(NOTIFY_CHUNK_INTERVAL).await;",
            "                // 非空转验证：把这句去掉",
        )],
        cmd=cargo("test", "--lib", "android_peripheral_paces_its_notifications"),
        cwd=TAURI,
        expect_fail_hint="必须真的 sleep",
        tags=["rust", "ble", "android"],
    ),
    Case(
        name="BLE 文件分块必须能被分片层发出去（否则一帧打死链路）",
        why="真机 2026-09-13：大图两边都显示成功、对方列表里却没有。日志证据 "
        "`[SEND] 写失败 ⇒ 结束该链路写循环 … type=file_chunk` + 接收侧反复 "
        "`接收文件初始化失败: 重复的文件传输` → `file_reject`。根因：一对一文件流每块 "
        "256 KiB，在 MTU=23 上要 18725 片 > 上限 8192 ⇒ fragment() 返回 None ⇒ 拆链路；"
        "重复 offer 又被 reject ⇒ 对端停止重试 ⇒ 文件永远到不了",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            '    if path == crate::mesh::PathKind::Bluetooth.as_str() {',
            "    if false {",
        )],
        cmd=cargo("test", "--lib", "--features", "bluetooth", "ble_file_chunk_actually_fits_the_ble_fragment_layer"),
        cwd=TAURI,
        expect_fail_hint="BLE 分块大小必须能被分片",
        tags=["rust", "ble", "file"],
    ),
    Case(
        name="蓝牙默认开启（三端一致，零配置）",
        why="用户规则：「有蓝牙就默认开，不用手动开关」。缺省一旦退回按平台分支（或改成 false），"
        "就会重新出现「手机有通道、Mac 要手点」以及「偏好=关 vs 运行时=开」互相回灌的启停抖动",
        file=TAURI / "src" / "db.rs",
        injections=[("let default_on = true;", "let default_on = false;")],
        cmd=cargo("test", "--lib", "bt_defaults_on_everywhere"),
        cwd=TAURI,
        expect_fail_hint="缺省必须是",
        tags=["rust", "android", "channel"],
    ),
    # ---------------- 测试清单守卫（挡住「测试静默不跑」） ----------------
    # 这两条守的是 `scripts/check-test-manifest.mjs`。它拦的是一类**没有信号**的故障：
    # 测试明明写着，却根本没被执行，而所有命令都返回 0。
    Case(
        name="测试清单：基线里的用例没跑必须报出来（漏 --features 就靠它）",
        why="`bluetooth` 是**非默认** feature（`src-tauri/Cargo.toml` 的 [features]）。漏掉 "
            "`--features bluetooth` ⇒ BLE 模块根本不编译、那批用例连同被测代码一起消失，"
            "而 `cargo test` **全绿**。本项目真实踩过：BLE 连续四个版本（4.18.7→4.18.10）"
            "边走边修，而这个子系统恰恰是「忘了加 feature 就静默不测」的那个。"
            "清单守卫比对「基线名单 ⋈ 实际 --list」，缺名即红。",
        file=TAURI / "test-baseline.macos.txt",
        injections=[(
            "transport::bluetooth_peripheral::tests::central_mtu_clamps_and_never_returns_zero",
            "transport::bluetooth_peripheral::tests::central_mtu_clamps_and_never_returns_zero\n"
            "transport::bluetooth_peripheral::tests::a_test_that_no_longer_runs",
        )],
        cmd=["node", "scripts/check-test-manifest.mjs", "--only", "rust"],
        cwd=ROOT,
        expect_fail_hint="静默跳过",
        tags=["manifest", "ble"],
        # 基线按平台分文件（macOS 外设 / Windows 外设是互斥的 #[cfg]）。
        # Windows 那条腿的对应用例在下面 `平台基线之间的差额` 那条（2026-09-26 打通）。
        platforms=("darwin",),
    ),
    # ---------------- BLE 常量/换算的单一事实来源 ----------------
    # 守的是 `scripts/check-ble-constants.mjs`。背景：CHANGELOG 4.18.7→4.18.10
    # **连着四个版本**修同一个分片预算问题 —— 根因不是某一行写错，而是同一个概念
    # 在多个地方各算一遍（macOS 外设侧自己留了 `const DEFAULT = 20` / `const MAX = 512`）。
    Case(
        name="BLE 常量只有一个家：重复定义必须报出来",
        why="4.18.7→4.18.10 那四个版本的病根是「同一个概念多处各算一遍」。"
        "2026-09-16 把常量与换算收敛到 `transport/ble_framing.rs` 一处；"
        "本用例把 `BLE_DEFAULT_MTU` 重新定义回 `bluetooth.rs`，必须被报出来 —— "
        "否则下一次漂移会以完全相同的方式发生（数值恰好一致 ⇒ 不报错、只在真机上表现为"
        "「某台设备收不到消息」）。",
        file=TAURI / "src" / "transport" / "bluetooth.rs",
        injections=[(
            "    /// 把协商到的 MTU 换算成**分片有效载荷上限**（central 侧）。",
            "    /// BLE 未协商时的默认 ATT MTU。\n"
            "    pub const BLE_DEFAULT_MTU: u16 = 23;\n\n"
            "    /// 把协商到的 MTU 换算成**分片有效载荷上限**（central 侧）。",
        )],
        cmd=["node", "scripts/check-ble-constants.mjs"],
        cwd=ROOT,
        expect_fail_hint="有 2 处定义",
        tags=["ble", "new-guards"],
    ),
    Case(
        name="BLE 常量只有一个家：匿名常量重述必须报出来（4.18.x 的原始形态）",
        why="这条注入的就是 2026-09-13 真实埋下的那两行：`const DEFAULT: usize = 20` 与 "
        "`const MAX: usize = 512` —— 名字没有信息量、靠注释解释语义。它们让 macOS 外设侧"
        "成了 `ble_framing` 那份换算的**第二份实现**（当时 Windows 走共享函数、macOS 不走，"
        "于是文档里那句「外设侧用的是同一个函数」只对 Windows 成立）。"
        "判据刻意**不扫裸数字**：`const CONNECT_ATTEMPTS = 3` 这类无关常量不许被误伤，"
        "所以规则是「名字按 `_` 分词命中概念词或语义空名」**且**「值恰好是受保护字面量」。",
        file=TAURI / "src" / "transport" / "bluetooth_peripheral.rs",
        injections=[(
            "pub fn central_payload_mtu(max_update_value_length: usize) -> usize {\n"
            "    ble_framing::notify_payload_budget(max_update_value_length)\n"
            "}",
            "pub fn central_payload_mtu(max_update_value_length: usize) -> usize {\n"
            "    const DEFAULT: usize = 20;\n"
            "    const MAX: usize = 512;\n"
            "    let min = ble_framing::BLE_CHUNK_HEADER_LEN + 1;\n"
            "    if max_update_value_length < min {\n"
            "        DEFAULT\n"
            "    } else {\n"
            "        max_update_value_length.min(MAX)\n"
            "    }\n"
            "}",
        )],
        cmd=["node", "scripts/check-ble-constants.mjs"],
        cwd=ROOT,
        expect_fail_hint="第二份事实来源",
        tags=["ble", "new-guards"],
    ),
    Case(
        name="BLE 两侧载荷预算必须能互相推回去（外设侧不再减 ATT 头）",
        why="常量收敛只保证「只有一份」，不保证「这一份是对的」。本用例注入 central 侧的正确"
        "换算被外设侧**又减了一次 ATT 头**（4.18.7 的形态），"
        "`both_sides_agree_on_the_same_link_budget` 必须红。"
        "这条交叉校验此前**只存在于 Windows 专属**的 "
        "`peripheral_and_central_agree_on_payload_budget`，而 macOS 恰恰是当时唯一没走共享"
        "换算的一侧，所以缺口一直没被发现。现在两侧共用同一个测试。",
        file=TAURI / "src" / "transport" / "ble_framing.rs",
        injections=[(
            "        max_update_value_length.min(GATT_MAX_ATTR_LEN)\n    }",
            "        (max_update_value_length - ATT_HEADER_LEN).min(GATT_MAX_ATTR_LEN)\n    }",
        )],
        cmd=cargo("test", "--lib", "both_sides_agree_on_the_same_link_budget"),
        cwd=TAURI,
        expect_fail_hint="必须原样返回",
        tags=["rust", "ble", "new-guards"],
    ),
    Case(
        name="BLE 常量只有一个家：外设平台自己算一遍必须报出来（Android 2026-09-16 的形态）",
        why="判据 A/B 都只盯「定义」，而真实漏掉的那处是**把换算内联进平台实现**："
        "Android 的 `payload_mtu` 自己写 `if (1..=512).contains(&v) { v } else { 20 }` —— "
        "既没重新定义常量（逃过 B），也不是「重新实现具名函数」（逃过 A）。"
        "它还有真 bug：`1..=6` 这类**装不下分片头**的值被放行 ⇒ `fragment` 拒绝一切 ⇒ "
        "整条链路发不出消息，而日志只说「帧无法分片」。这条注入就是把它改回原样。"
        "发现它的正是 Phase 4 引入的 Android `cargo check` —— 它不跑测试，"
        "所以比 `cargo test` 更容易看见「只在某一平台编译的重复」。",
        file=TAURI / "src" / "transport" / "ble_android.rs",
        injections=[(
            "        let raw = call_static_int(\"payloadMtu\", central).unwrap_or(0);\n"
            "        ble_framing::notify_payload_budget(usize::try_from(raw).unwrap_or(0))",
            "        call_static_int(\"payloadMtu\", central)\n"
            "            .map(|v| if (1..=512).contains(&v) { v as usize } else { 20 })\n"
            "            .unwrap_or(20)",
        )],
        cmd=["node", "scripts/check-ble-constants.mjs"],
        cwd=ROOT,
        expect_fail_hint="找不到对规范换算的调用",
        tags=["ble", "android", "new-guards"],
    ),
    Case(
        name="中继电路必须登记成 PathKind::Relay（改回 Routed = 界面把中转标成 VPN）",
        why="四种通道全局统一（用户 2026-09-22）：中转电路此前复用 Routed，被界面标成「跨网段 / VPN」\n"
        "     并与 VPN 直达挤同一选路优先级。只钉枚举的单测（path_rank / best_link_kind /\n"
        "     path_kind_names_are_stable）在拨号改回 Routed 时**照样全绿** —— 接线没人守。\n"
        "     注入方式就是那处唯一拨号构造点：PathKind::Relay 改回 PathKind::Routed（仍可编译，\n"
        "     不然红的是编译器而不是判据），守卫 relay_circuit_is_tagged_relay_not_routed 必须红。",
        file=TAURI / "src" / "network" / "transport" / "relay.rs",
        injections=[(
            "PathKind::Relay,",
            "PathKind::Routed,",
        )],
        cmd=cargo("test", "--lib", "relay_circuit_is_tagged_relay_not_routed"),
        cwd=TAURI,
        expect_fail_hint="中继会合拨号",
        tags=["rust", "relay", "mesh", "new-guards"],
    ),
    Case(
        name="中继邻居投递数必须来自 try_send 的成功判定（丢了返回值就数不出来）",
        why="2026-09-23 审计 A1：`relay_send_to_neighbors` 旧实现是 `let _ = try_send(...)`，\n"
        "     于是「一个邻居都没接住」与「所有邻居都接住」在调用方看来完全一样 —— 发送端\n"
        "     据此宣布成功就是假成功。注入方式 = 把计数退回丢弃返回值（保留 `-> usize`，\n"
        "     让它编译得过、并且 `accepted` 恒为 0 看起来还「更安全」）。\n"
        "     这条打的是守卫里**第二个**断言：只把签名改回 `()` 会先撞上第一个断言，\n"
        "     那样证明不了「计数建立在成功判定上」这一条有独立价值。",
        file=TAURI / "src" / "network" / "transport" / "outbound.rs",
        injections=[(
            """        if try_send(state, &p, msg).await.is_ok() {
            accepted += 1;
        }
""",
            "        let _ = try_send(state, &p, msg).await;\n",
        )],
        cmd=cargo("test", "--lib", "relay_send_does_not_claim_unproven_success"),
        cwd=TAURI,
        expect_fail_hint="计数必须建立在",
        tags=["rust", "relay", "file", "stability", "new-guards", "a1-l1"],
    ),
    Case(
        name="接收端解出 @ 名单却不回送界面（第⑦段接缝断掉）",
        why="这条链跨两种语言七个文件，中段任何一段被改动都没有编译错误，\n"
        "     表现只有一个：「@ 了我不亮红点」。这条用例演的是最容易顺手改坏的那一段 ——\n"
        "     解密拿到了名单，emit 时写成 None。护栏按形状扫七处接缝，缺哪一段点哪一段。",
        file=TAURI / "src" / "network" / "transport" / "gossip.rs",
        injections=[("mention_ids: mentions.as_deref(),", "mention_ids: None,")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="⑦ 接收端解出来了却没回送给界面",
        tags=["frontend", "rust", "group", "stability", "new-guards", "mention-identity"],
    ),]
