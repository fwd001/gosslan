#!/usr/bin/env python3
"""护栏非空转用例分册：TCP/mesh/协议与版本面（network/* 非蓝牙非文件、protocol.rs、discovery）。

本册 27 条 / 538 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
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
        name="Presence 不得内联大头像（否则优先通道被堵）",
        why="Presence 每 10s 广播一次且走优先通道；一张 400KB 头像会让聊天/好友请求"
            "排在几百片分片后面（真机：加好友几分钟才到）",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "let avatar = hello_avatar_for_wire(raw_avatar.as_deref());",
            "let avatar = raw_avatar.as_deref();",
        )],
        cmd=cargo("test", "--lib", "presence_caps_inline_avatar"),
        cwd=TAURI,
        expect_fail_hint="broadcast_presence",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="大头像资料帧必须降到 Low 队列（否则堵住优先通道）",
        why="UserInfo 带大 avatar 时若不降级，会占满优先通道，聊天/好友请求几分钟才到。"
            "分类表已从 is_bulk_message 收进 dispatch::message_priority（单一事实来源），"
            "锚点随之搬到 dispatch.rs",
        file=TAURI / "src" / "network" / "dispatch.rs",
        injections=[(
            "Message::UserInfo { avatar: Some(a), .. } if a.len() > CONTROL_AVATAR_MAX_BYTES => {\n            MessagePriority::Low\n        }",
            "Message::UserInfo { avatar: Some(a), .. } if a.len() > CONTROL_AVATAR_MAX_BYTES => {\n            MessagePriority::Normal\n        }",
        )],
        cmd=cargo("test", "--lib", "bulk_messages_are_only_large_chunks"),
        cwd=TAURI,
        expect_fail_hint="bulk_messages_are_only_large_chunks",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="共享目录/中继文件：无直连时借一跳中继（定向转发判定）",
        why="共享目录原本只支持直连，A 与 B 只能经中继时打不开；这条纯函数决定哪些帧"
            "要借邻居转投（真机 2026-09-14 全 Windows 局域网）",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "Message::RelayFileOffer { to, .. } if to != my_id => Some(to.as_str()),",
            "Message::RelayFileOffer { .. } => None,",
        )],
        cmd=cargo("test", "--lib", "directed_relay_target_routes_share_and_offer_frames"),
        cwd=TAURI,
        expect_fail_hint="directed_relay_target_routes_share_and_offer_frames",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="群文件 completed 的终态保护必须真读库（不是只在测试里模拟）",
        why="favorites_tests.rs 那条 complete_ack_failure_cannot_downgrade_completed 是在**测试体里**"
            "重写了一遍 already_completed 判断再断言结果 —— 它钉的是抄本：把生产 handler 里那段"
            "读库保护删掉，这条测试照样绿（2026-09-29 审计抓到，形状与群分片那条镜像测试相同）。"
            "现在的具名判据读的是 transport.rs 里那句真的去 list_group_file_recipients 查 completed 的表达式，"
            "把它改成常量 false 就必须红",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            '.any(|r| r.recipient_id == peer_id && r.status == "completed")',
            '.any(|_| false)',
        )],
        cmd=cargo("test", "--lib", "completed_guard_reads_recipient_status_in_the_handler"),
        cwd=TAURI,
        expect_fail_hint="completed_guard_reads_recipient_status_in_the_handler",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="群断链收尾必须吃掉原子摘出来的那一份（内容台账的唯一写点）",
        why="`5a0cfcc` 把断链那一路改成「原子摘 → 逐个收尾」时，循环写成 `for (tid, _r) in taken`，\n"
        "     只调了 finalize 那一半（气泡 / 内存 key / recipient 台账）—— 而群收件人**内容台账**的\n"
        "     `record_failure ⇒ Incomplete` 只有 `fail_taken_group_receive` 这一个写点。后果静默：\n"
        "     台账停在 Active ⇒ 重取改走「Active 超 60s」那条兜底，退避口径变了、要多等一轮，\n"
        "     而当时四层门禁 + 200 条护栏整跑**全绿**（没有任何判据看「摘出来的那份有没有被用完」）。\n"
        "     注入方式：删掉那一行调用 ⇒ 形状判据必须红（这是 INV-P28 后半句的非空转证明）",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "            file::fail_taken_group_receive(&state, &r);\n",
            "",
        )],
        cmd=cargo("test", "--lib", "peer_offline_group_cleanup_takes_before_finalizing"),
        cwd=TAURI,
        expect_fail_hint="摘出来的 FileReceiver 没被收尾吃掉",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="Hello 必须声明本机版本，且版本不得进签名材料",
        why="「加两个可选字段」的全部价值就是让「谁版本高」可查：不声明 ⇒ INV-P24 的降级日志"
        "和诊断面板永远只能写「未声明」，跨版本故障重新变回猜。反过来一旦这两个字段进了"
        "Hello 签名材料，老端验签就会失败 ⇒「报版本」这件事自身变成破坏性变更。"
        "两头都是「缺了没人报错」的行为，只能钉源码",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "protocol_version: Some(crate::protocol::PROTOCOL_VERSION),",
            "protocol_version: None,",
        )],
        cmd=cargo("test", "--lib", "peer_version_is_declared_not_signed_and_reclaimed"),
        cwd=TAURI,
        expect_fail_hint="本机 Hello 必须声明 PROTOCOL_VERSION",
        tags=["rust", "protocol", "compat"],
    ),
    Case(
        name="好友同意（两条 FriendAccept 路径走同一个家，且家里要清 pending）",
        why="用户实测根因：直连 `Message::FriendAccept` 只加好友、忘了清 pending，\n"
        "     跨跳 `GossipKind::FriendAccept` 清了 ⇒ 表现成「有时候会清、有时候不清」。\n"
        "     2026-09-28 复审抓到同一条链上的第二半：**去重门控只有跨跳有、直连没有** ⇒\n"
        "     同一个「被『好友申请已通过』刷屏」的症状在直连链路上照旧存在。\n"
        "     两条链路现已收成一个 `apply_friend_accept`，注入点跟着搬家：\n"
        "     ① 拆掉直连那半边的调用（少一条消费者 ⇒ 红）；② 拆掉家里那句 `forget_pending_request`\n"
        "     （家不做自己该做的事 ⇒ 红）。判据形状与理由见 `lib.rs` 里那条同名单测。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[
            (
                '            apply_friend_accept(state, &from, "收到好友同意");',
                "            // （非空转验证：直连这一条不再走那个家）",
            ),
            (
                "    forget_pending_request(state, from);",
                "    // （非空转验证：这个家不再清 pending）",
            ),
        ],
        cmd=cargo("test", "--lib", "every_friend_accept_path_forgets_the_pending_request"),
        cwd=TAURI,
        expect_fail_hint="都必须走同一个家",
        tags=["rust", "friend"],
    ),
    Case(
        name="已是好友的申请必须自动同意（否则双方永远加不上）",
        why="用户实测：B 的好友列表里有 A，而 A 是重置过的账号、列表里没有 B。A 发申请只在 B 侧插"
        "一条 pending，而 UI 又会把『申请人已是好友』的条目过滤掉（那是为了修『申请还挂着』）"
        "⇒ 两边都看不到、谁也加不上，只能先把 B 里的 A 删掉再加回来",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "            if auto_accept_if_already_friend(state, &from).await {",
            "            if false {",
        )],
        cmd=cargo("test", "--lib", "friend_request_from_existing_friend_auto_accepts"),
        cwd=TAURI,
        expect_fail_hint="两条 FriendRequest 路径",
        tags=["rust", "friend"],
    ),
    Case(
        name="发现 socket 必须收得到广播（绑具体 IP 在 macOS 上收不到）",
        why="用户真机：Mac 与手机同一个 Wi‑Fi、都开了局域网，却「互相搜不到」；Mac 列表里安卓只闪一下。"
        "根因是发现 socket 绑定到**具体 LAN IP** —— macOS 上这种 socket 收不到 255.255.255.255 广播、"
        "也收不到组播（本机实测 0 包；绑 0.0.0.0 收得到全部），于是 Mac 发得出去（手机看得到 Mac）、"
        "却一个 announce 都收不到。收发必须是两个 socket：收的绑 0.0.0.0、发的绑具体 LAN IP",
        file=TAURI / "src" / "network" / "discovery.rs",
        injections=[(
            "pub fn discovery_recv_bind_ip() -> Ipv4Addr {\n    Ipv4Addr::UNSPECIFIED\n}",
            "pub fn discovery_recv_bind_ip() -> Ipv4Addr {\n    Ipv4Addr::LOCALHOST\n}",
        )],
        cmd=cargo("test", "--lib", "discovery_recv_socket_actually_receives_broadcast"),
        cwd=TAURI,
        expect_fail_hint="收不到 255.255.255.255 广播",
        tags=["rust", "network", "discovery"],
    ),
    Case(
        name="好友同意回执必须有界补发（否则一次丢帧 = 永久单边好友）",
        why="真机 2026-09-13：Android 点「接受」、Android 侧好友已出现，但 Mac 端状态一直没同步 —— "
        "FriendAccept 只发一次且**没有回执**，链路抖动时静默丢失就永不重发。"
        "修法是窗口 + 次数 + 间隔的有界补发；策略写反了要么永不补发、要么疯狂打扰对端，"
        "所以用真值表钉住",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "    if now - issued > window_ms || attempts >= max_attempts {",
            "    if false {",
        )],
        cmd=cargo("test", "--lib", "friend_accept_flush_is_bounded_and_spaced"),
        cwd=TAURI,
        expect_fail_hint="GiveUp",
        tags=["rust", "friend", "reliability"],
    ),
    Case(
        name="断链清理必须在「确认这个 peer 真的一条链路都不剩」之后（P1 故障隔离）",
        why="`reader_loop` 的收尾顺序不是风格问题。旧形状是在函数开头就按 peer 清接收器，"
        "而同一个 peer 完全可以同时挂 LAN + Tailscale + BLE —— 断其中一条会连带杀掉另外几条链路上"
        "**正在收**的文件（同文件下面那段「只删这一条连接」的注释早就写明了「断一条 ≠ peer 下线」，"
        "文件这两处一直与它自相矛盾）。注入 = 把清理挪回门之前，正是那次回归的形状。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "    // 只移除**这一条**连接（按 channel 身份匹配），不是整条删光：",
            "    file::fail_receives_for_peer(&state, &peer_id);\n"
            "    // 只移除**这一条**连接（按 channel 身份匹配），不是整条删光：",
        )],
        cmd=cargo("test", "--lib", "peer_wide_receiver_cleanup_is_gated_on_total_link_loss"),
        cwd=TAURI,
        expect_fail_hint="排在了 `if peer_now_offline` 之前",
        tags=["rust", "transport", "p1-isolation"],
    ),
    Case(
        name="每小时那一趟必须同时回收 .part / 中继内存表 / 静默接收器",
        why="延后断链清理之后，「对端在线但这一单被发送侧放弃」的接收器只剩这一趟兜底："
        "协议里没有 cancel 帧，发送侧 60s 停滞只是自己退回 outbox，不通知接收端。"
        "摘掉这一行的表现是**永不回收**（表项 + 文件句柄 + `.part`），而 `sweep_stale_parts`"
        "还会因为「还在表里」把它当活跃跳过 —— 不报错、不影响别的功能，只有护栏看得见。",
        file=TAURI / "src" / "lib.rs",
        injections=[(
            "                        let stalled = crate::network::transport::sweep_stalled_receives(&st);\n",
            # 变异必须"照样编译、但这趟不再回收"：
            # 直接删整行会让下面的 `if stalled > 0` 引用未定义 ⇒ 红在编译错误上，什么也没证明；
            # 而保留 `sweep_stalled_receives` 字面量又会让护栏的 contains() 假过。
            "                        let stalled = 0usize;\n",
        )],
        cmd=cargo("test", "--lib", "hourly_sweep_covers_parts_relay_and_stalled_receivers"),
        cwd=TAURI,
        expect_fail_hint="内存态就没人回收了",
        tags=["rust", "transport", "p1-isolation"],
    ),
    Case(
        name="接收器回收必须走「判据与摘表同一次持锁」的 take_*（不许退回快照-再杀）",
        why="先 `iter().filter(stale)` 拿到 id 列表、释放锁、再逐个收尾 —— 这两步之间完全可以"
        "挤进一个新的 FileOffer（同一 transfer_id 重建接收器、`fed_at_ms` 就是现在），"
        "于是按 id 收尾会把一条**正在收**的传输判死。护栏钉的是「清扫器里只准调 take_*，"
        "不许自己算 stale」。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "pub fn sweep_stalled_receives(state: &Arc<AppState>) -> usize {\n"
            "    const REASON: &str = \"接收超时：对端久未继续发送\";\n"
            "    let now = db::now_ms();",
            "pub fn sweep_stalled_receives(state: &Arc<AppState>) -> usize {\n"
            "    const REASON: &str = \"接收超时：对端久未继续发送\";\n"
            "    let now = db::now_ms();\n"
            "    let _snap: Vec<String> = state\n"
            "        .file_receivers\n"
            "        .lock()\n"
            "        .unwrap_or_else(|e| e.into_inner())\n"
            "        .iter()\n"
            "        .filter(|(_, x)| file::receive_is_stale(now - x.fed_at_ms))\n"
            "        .map(|(k, _)| k.clone())\n"
            "        .collect();",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "peer_wide_receiver_cleanup_is_gated_on_total_link_loss",
        ),
        cwd=TAURI,
        expect_fail_hint="重算一遍",
        tags=["rust", "transport", "p1-isolation"],
    ),
    Case(
        name="链路队列必须按字节封顶：折算槽数不许被换成常量深度",
        why="第 2 步 · P3。四个建链点原本各写三条深 1024 的 `mpsc`；一片 LAN 分块上线约 341 KB ⇒ "
        "**单链路最坏 ~350 MB**，多连接按连接翻倍，手机上就是 OOM / 整机变慢、进度假快。\n"
        "     这里只改**容量语义**（优先级模型一字未动）：low 队列按 `LINK_QUEUE_BYTE_BUDGET` 折算，\n"
        "     high/normal 保持按帧数（那里的帧被载荷上限卡着，不是内存问题）。\n"
        "     注入 = 把折算换成直接取上限 —— 预算常量还在、函数还在、调用点也还在，"
        "只是**没人再用它算深度**：这类「名字留着、线断了」的退化不会编译报错，只能这样咬。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[
            (
                "mpsc::channel(low_queue_slots(chunk_plain_bytes)),",
                "mpsc::channel(LINK_QUEUE_MAX_SLOTS),",
            ),
        ],
        cmd=cargo("test", "--lib", "link_queues_are_created_from_one_place"),
        cwd=TAURI,
        expect_fail_hint="low 队列必须由",
        tags=["rust", "transport", "p3-queues"],
    ),
    Case(
        name="链路队列的字节预算必须真的参与折算（不许绕过预算取上限）",
        why="与上一条是一对：那条钉「接没接上」，这条钉「算得对不对」。"
        "把 `BUDGET / wire` 换成常量上限，四条链路的代码形状一模一样、编译也一模一样，"
        "但 1024 槽 × 341 KB 那条老路就回来了 —— 只有「槽数 × 单帧线上字节 ≤ 预算」这条"
        "行为判据抓得住它（所以两条都要留，缺一不可）。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[
            (
                "    (LINK_QUEUE_BYTE_BUDGET / wire).clamp(LINK_QUEUE_MIN_SLOTS, LINK_QUEUE_MAX_SLOTS)",
                "    LINK_QUEUE_MAX_SLOTS",
            ),
        ],
        cmd=cargo("test", "--lib", "link_low_queue_is_bounded_by_bytes_not_frame_count"),
        cwd=TAURI,
        expect_fail_hint="超过预算",
        tags=["rust", "transport", "p3-queues"],
    ),
    Case(
        name="写失败的分流两半都必须各自咬住（本地成帧失败 vs 真 socket 失败）",
        why="第 1 步 · 故障隔离（P2）。判据不是帧类型而是**字节有没有上过链路**：一个字节都没"
        "写出去 ⇒ 那是本机 bug，链路保留；写出去才失败 ⇒ 这条连接不可信，必须判死并拆写半。"
        "两个退化方向各对应一次真实事故，所以两个变异各来一遍：\n"
        "     ① 把 Local 折回 Failed（旧形状 `res.is_ok()` 一把抓）⇒ 一条永远发不出去的超长帧"
        "会带走整条连接，再连带拖死同一 peer 其它链路上的文件接收；\n"
        "     ② 把 Socket 折成 Local（\"保护文件传输\"式修法）⇒ 已经写不出去的连接被一直复用，"
        "消息静默堆在队列里 —— 用户 2026-09-24 明确划的界。\n"
        "     两条都由同一处护栏判出，锚点必须逐字对齐 rustfmt 后的形状（带 `Err(` 那层括号）。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[
            (
                "Err(WriteError::Local(why)) => WriteOutcome::Local(why),",
                "Err(WriteError::Local(_)) => WriteOutcome::Failed,",
            ),
            (
                "Err(WriteError::Socket(_)) => WriteOutcome::Failed,",
                "Err(WriteError::Socket(e)) => WriteOutcome::Local(e.to_string()),",
            ),
        ],
        cmd=cargo(
            "test",
            "--lib",
            "writer_loop_splits_local_from_socket_failure_exactly_once",
        ),
        cwd=TAURI,
        expect_fail_hint="分流",
        tags=["rust", "transport", "write-taxonomy"],
    ),
    # ---------------- 领域依赖方向（docs/domains.data.mjs + check-domain-deps.mjs） ----------------
    # 跟上面三条互补：check-domain-map.mjs 守图的形式（路径/不重叠/enforce），
    # check-domain-deps.mjs 守图的依赖方向（每条 use crate::xxx 是否落在 consumes 里）。
    # 地图与依赖两套都过的领域,才算"自洽";只过一套⇒要么补 consumes 要么删 use。
    # 教训（Phase 5b）：consumes 字段是该领域的**边界协议**,有了它"新增一条 use"不再是
    # 静默演化,而是有闸门的扩展;不写 consumes 等于写"我不关心边界会怎样" —— 守门不让过。
    Case(
        name="领域依赖方向：跨域 use 不在 consumes 中 → FAIL（守住「依赖是声明出来的」）",
        why="messaging 域的 gossip_engine.rs 当前 use 了 crypto::Identity 与 protocol::*,"
        "对 platform 域毫无依赖。本用例临时给它塞一行 `use crate::menu;`（platform 域内"
        "结构体）,守门必须报「不在 consumes 中」并定位到 file:line。否则「新增一条跨域"
        "依赖」就是静默演化 —— 等再有人 PR 又删掉,守门仍全绿,边界已经被改写却没人知道。"
        "修法：① 真有需求就把 platform 加进 messaging 的 consumes;② 删掉这条临时 use。",
        file=ROOT / "src-tauri/src/gossip_engine.rs",
        injections=[(
            'use crate::crypto::Identity;\n'
            'use crate::protocol::{GossipEnvelope, GossipKind};',
            'use crate::crypto::Identity;\n'
            'use crate::protocol::{GossipEnvelope, GossipKind};\n\n'
            '// TEMP-NON-VACUUM-TEST(messaging→platform):必须被 check-domain-deps.mjs 拦下。\n'
            'use crate::menu;',
        )],
        cmd=["node", "scripts/check-domain-deps.mjs"],
        cwd=ROOT,
        expect_fail_hint="想依赖「platform」域",
        tags=["domain-deps", "new-guards"],
    ),
    Case(
        name="身份锚点的打标点必须留在 handle_message 的 Hello 分支（删掉即红）",
        why="v4.25.3 只在三处**握手**后打标，而 `mark_peer_keys_verified` 在 `peers` 条目不存在时\n"
        "     是空操作、`upsert_peer` 新建条目又恒标 `keys_verified: false` ⇒ 出站拨号与 BLE 两条\n"
        "     路径第一次连接的对方整个会话都绑不上锚点（安全码算不出、公网中继永不准入，且无报错）。\n"
        "     修法是把打标补在 `handle_message` 的 Hello 分支（TCP/BLE 通用的那一个写入点）。\n"
        "     注入方式：删掉那一行 —— 计数 5→4，接线断言必须红",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "            mark_peer_keys_verified(state, &device_id, &hello_x, &hello_e);\n",
            "",
        )],
        cmd=cargo("test", "--lib", "friend_identity_anchor_has_one_binding_rule"),
        cwd=TAURI,
        expect_fail_hint="四处打标",
        tags=["rust", "identity", "relay"],
    ),
    Case(
        name="打标必须排在 `upsert_peer` **之后**（换序即红，计数不变所以只有次序断言会响）",
        why="这条守的是次序而不是数量：把那一行挪到 `upsert_peer` 之前，`mark_peer_keys_verified(`\n"
        "     仍然是 5 处 ⇒ 计数断言照样绿，但条目此刻还不存在 ⇒ 空操作 ⇒ 静默回到同一个缺陷。\n"
        "     两条注入合起来正好是「同一行换个位置」，所以这条用例证明的是次序断言本身非空转",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[
            (
                "            let (hello_x, hello_e) = (x25519_pubkey.clone(), ed25519_pubkey.clone());\n"
                "            upsert_peer(",
                "            let (hello_x, hello_e) = (x25519_pubkey.clone(), ed25519_pubkey.clone());\n"
                "            mark_peer_keys_verified(state, &device_id, &hello_x, &hello_e);\n"
                "            upsert_peer(",
            ),
            (
                "            mark_peer_keys_verified(state, &device_id, &hello_x, &hello_e);\n"
                "            // 对齐单聊逻辑时钟",
                "            // 对齐单聊逻辑时钟",
            ),
        ],
        cmd=cargo("test", "--lib", "friend_identity_anchor_has_one_binding_rule"),
        cwd=TAURI,
        expect_fail_hint="打标必须排在 upsert_peer 之后",
        tags=["rust", "identity", "relay"],
    ),
    Case(
        name="公网中转的口令不许被写进日志或诊断事件（INTEGRATION.md 要求 7）",
        why="这条链路上「服务器口令」是唯一准入手段，而调试时最容易手滑写出的就是\n"
        "     `format!(\"token={}\", cfg.token)` 那一行 —— 日志会被导出、会被贴进求助帖。\n"
        "     守卫 `relay_token_never_reaches_logs_or_diagnostics` 按**语句**（`;` 切）扫\n"
        "     transport 与 commands 全集里的 logger/push_diag_event 调用，命中口令值表达式\n"
        "     且不是「只报长度」就红。注入方式就是这个功能真上线时最可能出现的那一行改动：\n"
        "     把探测日志里的 token_len={} 改成 token={}（顺手把 chars().count() 换成裸值，\n"
        "     保持可编译 —— 不然红的是编译器而不是判据）。",
        file=TAURI / "src" / "commands" / "relay.rs",
        injections=[
            (
                '            "探测 kind={} server={} tried={} token_len={}",',
                '            "探测 kind={} server={} tried={} token={}",',
            ),
            (
                "            report.kind, report.server, report.tried, token_norm.chars().count()",
                "            report.kind, report.server, report.tried, token_norm",
            ),
        ],
        cmd=cargo("test", "--lib", "relay_token_never_reaches_logs_or_diagnostics"),
        cwd=TAURI,
        expect_fail_hint="把口令",
        tags=["rust", "relay", "secret"],
    ),
    Case(
        name="中继分册必须留在守卫的 transport 全集视图里（漏登记 = 假绿）",
        why="`transport/relay.rs` 是 `transport.rs` 用 `include!` 并进同一模块的分册，而源码守卫\n"
        "     是拿 `transport_src_for_guards()` 读**文件文本**的。4.25.0 接线时只把它登记进了\n"
        "     `docs/domains.data.mjs`，漏了这份视图 ⇒ 所有以「transport 全集」为判据的守卫\n"
        "     **看不见拨号器本身**。这种漏法不报错，只是永远绿（比假红危险）。\n"
        "     注入方式：把那一行登记去掉，口令守卫必须红（它要扫 relay 分册里的日志调用）。",
        file=TAURI / "src" / "network" / "mod.rs",
        injections=[
            (
                '    src.push_str(include_str!("transport/relay.rs"));\n',
                "",
            ),
        ],
        cmd=cargo("test", "--lib", "relay_token_never_reaches_logs_or_diagnostics"),
        cwd=TAURI,
        expect_fail_hint="假绿",
        tags=["rust", "relay", "new-guards"],
    ),
    Case(
        name="出站清扫器的失败终态必须由写库成功门控（审计 A3：防重复投递）",
        why="旧缺陷：emit(message-failed) 写在 `if let Ok(dbc){ 写库 }` 之外、写库返回值被 `let _ =` 丢掉\n"
        "     ⇒ 锁中毒/写库失败时界面报失败而 outbox 行还在 ⇒ 下次 flush 重发 = 重复投递。\n"
        "     修法是把 emit 收进 `if finalized { … }`（finalized = 两步写库都成功）。\n"
        "     注入方式：把单聊那处的门控 `if finalized {` 改成 `if true {`（emit 变回无条件，仍可编译），\n"
        "     守卫必须红 —— 它数 `if finalized {` 的处数，少一处即判 emit 脱离了写库门控。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "                    finalize_expired_message(&dbc, &msg_id, false)\n"
            "                };\n"
            "                if finalized {",
            "                    finalize_expired_message(&dbc, &msg_id, false)\n"
            "                };\n"
            "                if true {",
        )],
        cmd=cargo("test", "--lib", "outbox_sweeper_emits_only_after_db_write_succeeds"),
        cwd=TAURI,
        expect_fail_hint="门控",
        tags=["rust", "outbox", "stability", "new-guards"],
    ),
    Case(
        name="transport 生产锁必须抗中毒（审计 A8：中毒 panic 会带走 reader_loop）",
        why="全仓主导写法是 lock().unwrap_or_else(|e| e.into_inner())，但 transport.rs 曾留 11 处多行\n"
        "     `.lock().unwrap()`。没有 catch_unwind ⇒ 一次锁中毒 panic 带走 reader_loop 并跳过收尾\n"
        "     （links.remove / mark_peer_offline），对端在界面上永久「在线」。\n"
        "     注入方式：把 pending_file_complete 那处改回 `.lock().unwrap()`（仍可编译），\n"
        "     守卫（去空白后扫 transport 全集，单行与多行一并覆盖）必须红。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "                .pending_file_complete\n"
            "                .lock()\n"
            "                .unwrap_or_else(|e| e.into_inner())",
            "                .pending_file_complete\n"
            "                .lock()\n"
            "                .unwrap()",
        )],
        cmd=cargo("test", "--lib", "transport_locks_tolerate_poison"),
        cwd=TAURI,
        expect_fail_hint="reader_loop",
        tags=["rust", "transport", "stability", "locks", "new-guards"],
    ),
    Case(
        name="中继收文件的哈希必须对组装后的明文算（审计 1.8：乱序/重复分片必错）",
        why="中继链路分片天然重复（多邻居泛洪各送一份）且乱序（多路径时延不同）。旧实现逐片「到达\n"
        "     即喂」增量哈希、喂在 add_chunk 去重/排序之前 ⇒ 分片收齐却必然校验失败：接收端报\n"
        "     「文件完整性校验失败」、发送端却显示成功（无回执），两端状态互相矛盾且无重试路径。\n"
        "     注入方式（2026-09-27 换过一次，见下方锚点注释）：把流式校验换回**旧的退化形状** —— 整份\n"
        "     `fs::read` 进内存再 `Sha256::digest(&…)`；可编译，且一次踩中守卫的两条断言\n"
        "     （不再含 sha256_file_hex(、出现了 Sha256::digest(&）。守卫必须红，实测报的是\n"
        "     「完整性校验必须对…」那一条。",
        file=TAURI / "src" / "network" / "transport.rs",
        # 锚点死因与新坏法（2026-09-27 修，roadmap #96）：旧坏法 `digest(&full)`→`digest(&name)`
        # 指向的形状**已被 P4 有意消灭** —— 中继接收改成流式 `file::sha256_file_hex(&part_path)`，
        # 而 lib.rs:3650 那道守卫现在反过来断言「体内不得出现 `Sha256::digest(&`」「校验必须走
        # `sha256_file_hex(`」「不得有 `.hasher`」⇒ 不是谁弄坏了锚点，是**守卫变强后旧坏法写不出来了**。
        # 新坏法取今天真能编译的退化形状（整份 fs::read 进内存再 digest）：一次踩中两条断言，
        # 且正是 P4 要消灭的「内存峰值 ≈ 2× 文件大小」。
        # ⚠️ 上一版注释里「把 r.hasher 塞回去」那个方案**编译不过**（Reassembly 早已没有该字段，
        #    而那字面量正是断言①要抓的）⇒ 守卫变强时不许凭猜写新锚点，先确认新形状可编译。
        injections=[(
            "            let actual_hex = match file::sha256_file_hex(&part_path) {\n",
            "            let actual_hex = match (|| -> Result<String, String> {\n                use sha2::{Digest, Sha256};\n                let __b = std::fs::read(&part_path).map_err(|e| e.to_string())?;\n                Ok(Sha256::digest(&__b).iter().map(|b| format!(\"{b:02x}\")).collect())\n            })() {\n",
        )],
        cmd=cargo("test", "--lib", "relay_receive_hashes_assembled_plaintext_once"),
        cwd=TAURI,
        expect_fail_hint="完整性校验必须对",
        tags=["rust", "relay", "files", "stability", "new-guards"],
    ),
    # ---------------- 群同步：密钥必须先于群消息（2026-09-24 RC2） ----------------
    Case(
        name="心跳路径退回「群消息先于群密钥」—— 顺序守卫必须红",
        why="三处补发点（拨号建链 / Hello / 心跳）的先后是这条链唯一的保护：\n"
        "     `handle_gossip` 在解密**之前**就把 msg_id 登进去重表，密钥后到时那一条\n"
        "     已经被「见过」挡掉 ⇒ 之后 group_outbox 重发多少次都没有消费者。\n"
        "     注入用带注释锚点的完整块（同一形状在 Hello 与心跳两处都出现，\n"
        "     短锚点会命中 2 次被脚本拒掉 —— 这本身就是「锚点必须唯一」那条纪律）。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            """            // 心跳也是一次"链路确实活着"的重发机会（见 `requeue_group_keys_for_peer` 的注释）。
            requeue_group_keys_for_peer(state, &device_id);
            flush_pending_group_keys(state, &device_id).await;
            flush_group_outbox(state, &device_id).await;""",
            """            // 心跳也是一次"链路确实活着"的重发机会（见 `requeue_group_keys_for_peer` 的注释）。
            flush_group_outbox(state, &device_id).await;
            requeue_group_keys_for_peer(state, &device_id);
            flush_pending_group_keys(state, &device_id).await;""",
        )],
        cmd=cargo("test", "--lib", "group_keys_always_precede_group_messages"),
        cwd=TAURI,
        expect_fail_hint="有一处群消息补发排在群密钥之前",
        tags=["rust", "group", "stability", "new-guards", "rc2-group-sync"],
    ),
    Case(
        name="少一处重新登记群密钥（拨号建链那条被删）",
        why="GroupKey 没有回执帧，旧代码只在「公钥变化 / 新节点」时重发 ⇒ 链路抖动把它带走后\n"
        "     就永远不再发。修法是把「每次链路建立/Hello/心跳」都当成一次重发机会（接收侧幂等）。\n"
        "     注入 = 删掉三处中的一处：另外两处还在，所以「调用次数」这一类弱判据会漏，\n"
        "     必须数得到具体次数。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[("    requeue_group_keys_for_peer(state, &peer_id);\n", "")],
        cmd=cargo("test", "--lib", "group_keys_always_precede_group_messages"),
        cwd=TAURI,
        expect_fail_hint="三处触发点（拨号建链 / Hello / 心跳）都要先重新登记密钥",
        tags=["rust", "group", "stability", "new-guards", "rc2-group-sync"],
    ),
    # ---------------- 「有人@我」从昵称改成绑人（第二阶段 §10／#103） ----------------
    Case(
        name="群明文里摘掉 mentions 键 —— 三态的『明确回答谁都没 @』必须还能发出去",
        why="`gossip_plaintext` 的 None 与 Some(空) 是这条协议字段的全部意义：\n"
        "     摘掉写键这一步，两种都变成 None ⇒ 接收端一律退回按昵称判，\n"
        "     界面上一切照常，只有『两个人同名』与『改过名字』两种场景静默判错 ——\n"
        "     正是 #103 记的那个判不到根的形状。注入 = 算好了名单却不写进明文。",
        file=TAURI / "src" / "protocol.rs",
        injections=[(
            '        v["mentions"] = serde_json::json!(clean);',
            '        let _ = &clean;',
        )],
        cmd=cargo("test", "--lib", "protocol::tests::gossip_plaintext"),
        cwd=TAURI,
        expect_fail_hint="gossip_plaintext_empty_mentions_is_an_explicit_answer_not_absence",
        tags=["rust", "group", "stability", "new-guards", "mention-identity"],
    ),]
