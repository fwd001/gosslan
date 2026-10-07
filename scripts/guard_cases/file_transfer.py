#!/usr/bin/env python3
"""护栏非空转用例分册：文件传输链（file.rs / group_file* / file_relay / file_offline / 预览）。

本册 32 条 / 653 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
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
        name="发送进度不得退回按入队计数（262MB 还在队列里就 100%）",
        why="send_on_link 返回成功只代表帧进了那条链路的 1024 槽 mpsc 队列，LAN 一片 256KB ⇒ "
        "最多 262MB 还在排队时界面已经显示 100%（真机 600MB 就是这条）。v4.22.37 起进度按 "
        "writer 记账的已写出片数换算；这条用例证明「把 file-progress 改回直接发入队量」一定被抓"
        ,
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            "                    received: on_wire,",
            "                    received: sent,",
        )],
        cmd=cargo("test", "--lib", "file_send_progress_counts_wire_not_queue"),
        cwd=TAURI,
        expect_fail_hint="file-progress",
        tags=["rust", "file", "progress"],
    ),
    Case(
        name="发送尝试必须装写出记账的回收守卫（否则失败重试退回入队口径）",
        why="v4.22.37 的进度按 writer 记的 chunks 换算，而 stream_file 有十来处提前 return，"
        "旧写法只在成功路径清一次 ⇒ 失败重试时读到上一轮残留的 chunks，进度又变回按入队算，"
        "而且只在「发失败再重试」时才出现。v4.22.38 改成 WireLedger 的 Drop 统一回收。"
        "单测只能证明 Drop 本身对，删掉这行装守卫的代码它照样绿 —— 所以必须机器钉接线",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            "    let _wire_ledger = WireLedger::install(state, transfer_id, peer_id);",
            "    let _ = (&state, transfer_id, peer_id);",
        )],
        cmd=cargo("test", "--lib", "file_send_progress_counts_wire_not_queue"),
        cwd=TAURI,
        expect_fail_hint="回收守卫",
        tags=["rust", "file", "lifecycle"],
    ),
    Case(
        name="写出记账必须按 (传输 × 收件人) 成键（群发 N 个任务共用一个 transfer_id）",
        why="#35：键只按 `transfer_id` 记时，一次群文件投递里 N 个成员任务共用同一条记录 ——\n"
        "     甲还在链路上走的字节会把乙的「最近有写出」一直刷新，真卡死的乙永远判不出停滞；\n"
        "     `chunks` 也会把别人走过的量算进这一条链路。两种坏行为都是静默的。\n"
        "     注入方式：把 writer 证据点的键退回裸 transfer_id —— 能编译、601 条用例全跑，\n"
        "     只有「写侧与读侧必须同一个键」这条接线断言会红",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "        &crate::network::file::file_peer_key(transfer_id, recipient),",
            "        transfer_id.as_str(),",
        )],
        cmd=cargo("test", "--lib", "file_send_progress_counts_wire_not_queue"),
        cwd=TAURI,
        expect_fail_hint="file_send_progress_counts_wire_not_queue",
        tags=["rust", "file", "lifecycle"],
    ),
    Case(
        name="群投递侧必须自己装写出记账的回收守卫（不得只修单聊那一半）",
        why="v4.22.38 给单聊装了 `WireLedger`，群侧一直没装 ⇒ 每发一次群文件，每个成员各留下\n"
        "     一条永不回收的记录（清理按守卫只许有 Drop 一处）。两笔账：表随历史传输无界增长；\n"
        "     同一 transfer_id 的补发/重试进门就不是 0，第一次停滞判定被上一轮残留的时刻推迟\n"
        "     —— 正是 v4.22.37 在单聊侧修掉的那个形状，只是换了条路。\n"
        "     注入方式：删掉群投递里那行装守卫的调用（发送照跑、编译照过）",
        file=TAURI / "src" / "commands" / "group_file_dispatch.rs",
        injections=[(
            "    let _wire_ledger = file::WireLedger::install(state, transfer_id, recipient);",
            "    let _ = (&state, transfer_id, recipient);",
        )],
        cmd=cargo("test", "--lib", "file_send_progress_counts_wire_not_queue"),
        cwd=TAURI,
        expect_fail_hint="群文件投递必须回收",
        tags=["rust", "file", "lifecycle"],
    ),
    Case(
        name="中继文件接收幂等（重复 offer 不清空已收切片）",
        why="多邻居泛洪会送来重复的 RelayFileOffer；覆盖式 insert 会清空已收到的切片 ⇒ "
            "文件永远缺片（完整性校验也必然失败）",
        file=TAURI / "src" / "file_relay.rs",
        # 锚点 2026-09-27 更新：幂等修复把旧的 `entry().or_insert_with()` 换成了
        # 「已存在就原样保留 + 早退」，旧锚点因此数到 0 次 ⇒ 这一格**注入不上**（护栏失效但不会假装绿）。
        # 现在的坏法就一句：把那条早退摘掉 ⇒ 重复 offer 会重新开档 truncate 已写分片。
        injections=[(
            "        if self.reassemblies.contains_key(&id) {\n"
            "            return Ok(());\n"
            "        }",
            "        // 注入：摘掉幂等早退（重复 offer 会覆盖式重开档）",
        )],
        cmd=cargo("test", "--lib", "begin_reassemble_is_idempotent"),
        cwd=TAURI,
        expect_fail_hint="begin_reassemble_is_idempotent",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="图片预览：在途失败不能永久缓存（否则字节落盘也不重读）",
        why="用户 2026-09-14：群里收图时好时坏，点几次/等一会儿/重发才出来。收到图片时可能"
            "\"消息先到、字节后到\"，在途读预览得到的失败若被永久缓存，文件落盘后也不会重读。",
        file=ROOT / "src" / "utils" / "filePreview.ts",
        injections=[(
            '      if (r.missing || r.note === "文件过大，无法预览") cache.set(msgId, r);',
            "      cache.set(msgId, r);",
        )],
        cmd=["node", "--test", "--experimental-strip-types", "--disable-warning=ExperimentalWarning",
             "src/utils/channelState.test.ts"],
        cwd=ROOT,
        expect_fail_hint="确定性失败",
        tags=["frontend", "new-guards"],
    ),
    Case(
        name="文件分片：重复/迟到必须忽略（只有真跳号才报错）",
        why="发送方重试时 seq 从 0 重来，而旧实现把『重复/迟到』也当致命错误 ⇒ 接收方整单失败、"
        "清掉状态 ⇒ 新 attempt 永远拼不齐（用户实测的那条「文件分片顺序错误」）⇒ "
        "BLE 上 >20KB 的文件事实上永远传不完",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            "Ordering::Less => ChunkSeq::Duplicate",
            "Ordering::Less => ChunkSeq::Gap",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "chunk_seq_rule_only_rejects_real_gaps",
        ),
        cwd=TAURI,
        expect_fail_hint="chunk_seq_rule",
        tags=["rust", "file"],
    ),
    Case(
        name="文件分片流必须钉在单条链路上投递",
        why="真机多文件并发：600MB 跑到 100% 报「文件分片顺序错误」。逐条 try_send 在队列满时"
        "会 failover 到另一条独立 TCP 连接 ⇒ 同一串 seq 跨连接乱序，接收端追加写不 seek ⇒ 判死。"
        "把分片改回 try_send 是「看起来更智能（会换路）」的退化，只有源码守卫拦得住",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            "r = send_on_link(&link, &done) => {",
            "r = try_send(state, peer_id, &done) => {",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "ble_file_transfer_respects_link_limits",
        ),
        cwd=TAURI,
        expect_fail_hint="FileDone 必须排在",
        tags=["rust", "file", "transport"],
    ),
    Case(
        name="分片等待期间必须做停滞检查",
        why="对端不收时 send_on_link 一直挂在背压上，写在它之后的任何检查都得不到执行 ⇒ "
        "界面冻在同一个百分比最长到 deadline（1h），用户只能猜是不是软件死了。"
        "摘掉检查是**无声**的：功能测试全绿、进度条照走",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            # ⚠️ 这段字面量跟着 `stall_tick` 的调用形状走：v4.24.5 给它加了 `peer_id`
            # （记账键按收件人拆），rustfmt 把它拆成多行 —— 签名再变时这里必须同步，
            # 否则就是"锚点已经不匹配、用例却静默不注入"的那种假绿。
            "|| stall_tick(\n"
            "                    state,\n"
            "                    transfer_id,\n"
            "                    peer_id,\n"
            "                    stream_started_ms,\n"
            "                    &mut stalled_shown,\n"
            "                ),",
            "|| crate::network::transport::Tick::Wait,",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "ble_file_transfer_respects_link_limits",
        ),
        cwd=TAURI,
        expect_fail_hint="分片必须投到钉住的那条链路",
        tags=["rust", "file", "reliability"],
    ),
    Case(
        name="群文件 Offer 必须与分片同链路",
        why="Offer=Normal、Chunk=Low，各自逐条选路。Normal 满时 failover 到另一条连接 ⇒ "
        "分片先到（接收端还没密钥，静默丢弃）、Offer 后到 ⇒ 首个 seq 对不上 ⇒ 整条判死。"
        "只钉分片不钉 Offer 比不钉更危险：分裂点是新造出来的",
        file=TAURI / "src" / "commands" / "group_file_dispatch.rs",
        injections=[(
            "crate::network::transport::send_on_link(&link, &offer)",
            "crate::network::transport::try_send(state, recipient, &offer)",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "ble_file_transfer_respects_link_limits",
        ),
        cwd=TAURI,
        expect_fail_hint="群文件的 Offer 与分片必须走同一条钉住的链路",
        tags=["rust", "file", "transport"],
    ),
    Case(
        name="文件气泡前不得做整文件扫描",
        why="真机（Mac 发送端）点大文件后要「卡一会儿」才出现发送中气泡：建发送记录时整读文件"
        "算 sha256。这类代码是「顺手把 cid 提前准备好」写回去的，代价挂在用户点击之后 ⇒ 必须钉死",
        file=TAURI / "src" / "commands" / "files.rs",
        injections=[(
            '"sha256": "",',
            '"sha256": file::sha256_file_hex(std::path::Path::new(path)).unwrap_or_default(),',
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "no_whole_file_scan_before_the_file_bubble",
        ),
        cwd=TAURI,
        expect_fail_hint="建发送记录前不得整读文件",
        tags=["rust", "file", "perf"],
    ),
    Case(
        name="群文件取消登记必须按收件人分键",
        why="群发是「每个成员一个投递任务、共用同一个 transfer_id」。单键时后注册的 insert "
        "挤掉前一个任务的 Sender ⇒ 对方 oneshot 立刻 Err(RecvError)，被取消分支当成"
        "「用户取消发送」⇒ N 个成员里只有最后一个发得完（真机三成员群必现，且报错原因是假的）",
        file=TAURI / "src" / "commands" / "group_file_dispatch.rs",
        injections=[(
            "let cancel_key = file::file_cancel_key(transfer_id, recipient);",
            "let cancel_key = transfer_id.to_string();",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "group_file_cancel_registry_is_scoped_per_recipient",
        ),
        cwd=TAURI,
        expect_fail_hint="群投递的取消登记必须带 recipient",
        tags=["rust", "file", "group"],
    ),
    Case(
        name="接收端分片判死必须回否定确认",
        why="旧行为是「abort 了但谁也不告诉」⇒ 发送端把剩下的整份文件继续灌进一条已死的传输，"
        "FileDone 无人应答 → 干等一个 FILE_ACK_IDLE → 判可重试 → 再整发 5 次。"
        "真机形状：600MB 跑到 100% 两边都失败，中间几十分钟界面一直「发送中」",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "if file::fail_receive(state, &transfer_id, peer_id, &e) {",
            "if false {\n                        let _ = file::fail_receive(state, &transfer_id, peer_id, &e);",
        )],
        cmd=cargo(
            "test",
            "--lib",
            "--features",
            "bluetooth",
            "receiver_abort_notifies_the_sender_inside_the_loop",
        ),
        cwd=TAURI,
        expect_fail_hint="分片判死必须回 FileCompleteAck",
        tags=["rust", "file", "reliability"],
    ),
    Case(
        name="db 锁作用域守卫必须抓得住「emit 写回锁内」",
        why="只有一条 SQLite 连接，前端收到事件后的第一次 IPC 要抢同一把锁 ⇒ 锁内 emit = 那一刻界面冻一下。\n"
        "     这条纪律原先只有一句注释（transport.rs:90）加一个点的守卫，2026-09-25 补接收侧回收时\n"
        "     `fail_taken_receive` 就在锁内 emit —— 逐行看谁都觉得没错，所以判据升级成 292 个取锁点全扫。\n"
        "     注入方式：把那条 emit 挪回 `{ }` 里面（大括号平衡、照样能编译，就是「锁还活着时发事件」），\n"
        "     check-lock-scope.mjs 必须报「存活期内在 emit」。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            "    }\n"
            "    emit_failed(state, transfer_id, reason);\n"
            "}",
            "        emit_failed(state, transfer_id, reason);\n"
            "    }\n"
            "}",
        )],
        cmd=["node", "scripts/check-lock-scope.mjs"],
        cwd=ROOT,
        expect_fail_hint="存活期",
        tags=["rust", "file", "stability", "locks", "new-guards"],
    ),
    Case(
        name="upsert_transfer 的「done 不可降级」闸门不得被摘掉（第 4 步 P7 终态契约）",
        why="`file_transfers` 有 39 个写入点（12 处写 failed、12 处写 active）。\n"
        "     任何一处晚到一步 —— 清扫器、重复帧、上一轮 attempt 还堵在链路队列里的残留 ——\n"
        "     都会把「已收到」改成「失败」并把进度从 100% 打回 0%，而那个文件此刻正躺在下载目录里能打开。\n"
        "     注入方式：把 DO UPDATE 尾巴上那句 `WHERE file_transfers.status <> 'done'` 删掉\n"
        "     （最自然的「简化」：看着像多余的 WHERE），正向判据必须红。",
        file=TAURI / "src" / "db" / "file_transfer.rs",
        injections=[(
            "             progress = excluded.progress\n"
            "         WHERE file_transfers.status <> 'done'\",",
            "             progress = excluded.progress\",",
        )],
        cmd=cargo("test", "--lib", "a_completed_transfer_row_is_never_downgraded"),
        cwd=TAURI,
        expect_fail_hint="不许被晚到的",
        tags=["rust", "db", "files", "terminal-state", "new-guards"],
    ),
    Case(
        name="反向：failed/cancelled/pending 必须还能改回 active（续传复用同一个 transfer_id）",
        why="这条规则最容易被「顺手扩大集合」破坏：复审原本建议的写法是\n"
        "     `WHERE status NOT IN ('done','failed','cancelled')`，而 `retry_incomplete_content`\n"
        "     复用同一个 `transfer_id` 发 `ContentRequest` ⇒ 把 failed 一起钉死 = 一判死就永远停在\n"
        "     失败，而字节其实还在流（症状恰好是本次要修的那个的反面）。\n"
        "     注入方式：照那份建议把集合写成三个状态（编译照过、正向判据照绿），反向判据必须红。",
        file=TAURI / "src" / "db" / "file_transfer.rs",
        injections=[(
            "         WHERE file_transfers.status <> 'done'\",",
            "         WHERE file_transfers.status NOT IN ('done', 'failed', 'cancelled')\",",
        )],
        cmd=cargo("test", "--lib", "a_failed_row_can_be_reactivated_by_the_next_attempt"),
        cwd=TAURI,
        expect_fail_hint="改回 active",
        tags=["rust", "db", "files", "terminal-state", "new-guards"],
    ),
    Case(
        name="用户取消不得记成失败（file_outbox 的 cancelled 与 failed 是两个口径）",
        why="`cancel_file_transfer` 自己的注释写着「用户主动停止用 cancelled，自动失败用 failed」，\n"
        "     而它调的是 `mark_file_outbox_failed` ⇒ 台账里落 failed。三条队列查询只认 pending/sending，\n"
        "     所以**功能等价、台账不等价**：下一个排查「这单为什么失败」的人（或照文档改代码的 AI）\n"
        "     读到的是用户自己按下的取消。\n"
        "     注入方式（P7 合并后跟着搬家）：把取消那一路的口径从 `FileJobEnd::Cancelled` 改成\n"
        "     `Expired` —— 编译照过、队列照样关掉，只有取消的台账口径错了，守卫必须红。\n"
        "     ⚠️ 反向也要成立：`mark_queued_transfer_failed`（自动判死）那边仍写 failed —— 两个口径不许合并。",
        file=TAURI / "src" / "commands" / "files.rs",
        injections=[(
            "db::finalize_file_failure(&dbc, &transfer_id, db::FileJobEnd::Cancelled);",
            "db::finalize_file_failure(&dbc, &transfer_id, db::FileJobEnd::Expired);",
        )],
        cmd=cargo("test", "--lib", "a_user_cancel_is_not_recorded_as_a_failure"),
        cwd=TAURI,
        expect_fail_hint="取消必须走",
        tags=["rust", "db", "files", "terminal-state", "new-guards"],
    ),
    Case(
        name="群文件终态：取消要写 gfile 气泡、超时/放弃不许写（两个方向都钉）",
        why="合并成一份出口之后，「哪些口径允许碰群气泡」变成这个函数里的一行 `if cancelled`。\n"
        "     写窄了：用户取消群文件，气泡原地不动 —— 按了没反应。\n"
        "     写宽了（当年清扫器就是宽的那一侧）：某一个收件人超时/放弃 ⇒ 整条群消息显示失败，\n"
        "     而其余收件人其实收到了 ⇒ 用户重发，群里多出一份重复文件。\n"
        "     注入方式：把 `if cancelled` 写成 `if true`（放宽到所有口径），反向判据必须红。",
        file=TAURI / "src" / "db" / "file_transfer.rs",
        injections=[(
            "        if cancelled {\n            bubble &= set_message_status(conn, &format!(\"gfile-{transfer_id}\")",
            "        if true {\n            bubble &= set_message_status(conn, &format!(\"gfile-{transfer_id}\")",
        )],
        cmd=cargo("test", "--lib", "expired_file_does_not_touch_the_group_bubble"),
        cwd=TAURI,
        expect_fail_hint="清扫器不许隔空改写",
        tags=["rust", "db", "files", "terminal-state", "new-guards"],
    ),
    Case(
        name="收尾路径的写序守卫必须扫到没被点名的函数（踢出重试集合的那一步排最后）",
        why="审计 A3 当时只给 `finalize_expired_file` 写了逐函数守卫，而 `fail_file_job` 与\n"
        "     `cancel_file_transfer` 各自又抄了一份同形状的收尾 —— 点名式守卫看不见没被点名的那些，\n"
        "     于是同一条被明令禁止的顺序在另外两处一直成立：outbox 行先变 failed/cancelled，\n"
        "     后面任何一步失败就没人补了（`list_expired_file_outbox` 只选 pending/sending）⇒\n"
        "     症状是那条气泡永久停在「发送中」，而且行已不在任何重试集合里，永远无人再修。\n"
        "     判据已升级成自动扫（结构式，不是清单）。\n"
        "     注入方式刻意选在**调用方**：给 `fail_file_job` 加两行「自己先关行、再补一笔气泡状态」\n"
        "     （= 有人嫌唯一出口麻烦、就地内联回旧写法）。逐函数守卫只盯着\n"
        "     `db::finalize_file_failure`，看不见调用方里的这一笔；只有自动扫会红 ——\n"
        "     这才是本用例要证明的那件事（判据要两头都命中才算命中：先关行、后有面向用户的写）。",
        file=TAURI / "src" / "commands" / "files.rs",
        injections=[(
            "        db::finalize_file_failure(&dbc, transfer_id, db::FileJobEnd::GiveUp).unwrap_or(false)",
            "        let _ = db::mark_file_outbox_failed(&dbc, transfer_id);\n"
            "        let _ = db::set_message_status(&dbc, &format!(\"file-{transfer_id}\"), \"failed\");\n"
            "        db::finalize_file_failure(&dbc, transfer_id, db::FileJobEnd::GiveUp).unwrap_or(false)",
        )],
        cmd=cargo("test", "--lib", "every_finalize_path_defers_the_destructive_write"),
        cwd=TAURI,
        expect_fail_hint="写序反了",
        tags=["rust", "db", "files", "terminal-state", "new-guards"],
    ),
    Case(
        name="幂等 accept 时必须重置段号（少这一句，续传段会被当成迟到重复片整段丢掉）",
        why="2026-09-22 跨网首测：160MB 永远停在 0%，最后报分片失败。\n"
        "     发送端续传时分片**按段从 seq 0 重编**，而活跃接收器的 next_seq 已推进到上一段末尾\n"
        "     ⇒ 新数据被 write_chunk 的 Duplicate 分支静默吞掉，文件永远差一截，且**不报任何错**。\n"
        "     注入方式就是那个看起来无害的改动：删掉 restart_segment 那一句\n"
        "     （有人会觉得「幂等嘛，回个 Accept 就够了」—— 差的就是这一句）。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "                            file::restart_segment(state, &transfer_id);\n",
            "",
        )],
        cmd=cargo("test", "--lib", "ble_file_transfer_respects_link_limits"),
        cwd=TAURI,
        expect_fail_hint="那一份判据",
        tags=["rust", "file", "resume"],
    ),
    Case(
        name="offer 位置判据不许退回「有活跃接收器就一律 Accept」",
        why="那半套规矩就是本次事故的根因：接收端不回真实位置 ⇒ 发送端每轮从 0 重发 ⇒\n"
        "     每轮重传一遍已收前缀 ⇒ 慢链路上永不收敛。三条纯函数用例（decide_offer）是第一道闸，\n"
        "     本用例证明它们不是装饰：把判据退回旧形状（让 held 直接等于 from_bytes，\n"
        "     等价于「永远认为位置对得上」），它们必须红。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """    let held = if has_active {
        active_received
    } else if size > 0 && disk_retained >= size {
        0
    } else {
        disk_retained
    };""",
            "    let _ = (active_received, disk_retained);\n    let held = from_bytes;",
        )],
        cmd=cargo("test", "--lib", "network::file::tests"),
        cwd=TAURI,
        expect_fail_hint="必须回真实位置",
        tags=["rust", "file", "resume"],
    ),
    Case(
        name="中继态回收必须真的写库（不许清完内存就宣布「已落终态」）",
        why="2026-09-23 审计 A2：旧行为只把两张内存表 `retain` 一遍 ⇒ `file_transfers` 里的行\n"
        "     永远停在 active，接收端界面**永久卡在某个百分比**，且一行日志都没有。\n"
        "     注入方式是这条链路上最「看起来无害」的那种优化：写库结果换成常量 `true` ——\n"
        "     内存清了、事件照发、前端也不报错，只有查库才发现行还是 active（本用例证明\n"
        "     守卫盯的是「写库发生了没有」，而不是「emit 发生了没有」）。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "            match db::mark_transfer_failed_if_active(&dbc, id) {",
            "            match Ok::<bool, rusqlite::Error>(true) {",
        )],
        cmd=cargo("test", "--lib", "relay_reclaim_finalizes_in_db_and_emits_outside_the_lock"),
        cwd=TAURI,
        expect_fail_hint="回收中继态必须把仍 active 的传输行落 failed",
        tags=["rust", "relay", "file", "stability", "new-guards", "st3-terminal"],
    ),
    Case(
        name="文件终态：把行踢出重试集合的那一次写必须排最后",
        why="这是审计 A3 修法**自身**的缺陷（2026-09-23 review 查出；P7 之后实现在\n"
        "     `db::finalize_file_failure`，本用例的注入点跟着搬过去）。四步里只有\n"
        "     `mark_file_outbox_failed`\n"
        "     四步里只有 `mark_file_outbox_failed` 会让 `list_expired_file_outbox` 再也扫不到这行\n"
        "     （它只选 pending/sending），所以它一旦排到最前面，「返回 false、下一 tick 重试」就变成\n"
        "     假话：后面任何一步失败，行已经是 failed，再没人重试 ⇒ 界面永久停在「发送中」。\n"
        "     注入方式 = 把它挪回第一位（就是修好之前的次序），顺序判据必须红；\n"
        "     只盯「调用了几次」是抓不到的 —— 次数一模一样。",
        file=TAURI / "src" / "db" / "file_transfer.rs",
        injections=[(
            """    let mut changed = false;
    if !already_done || cancelled {""",
            """    let mut changed = false;
    // 注入：把关行挪到最前面（= 修好之前的次序）
    let _ = mark_file_outbox_failed(conn, transfer_id);
    if !already_done || cancelled {""",
        )],
        cmd=cargo("test", "--lib", "terminal_finalize_defers_the_destructive_write"),
        cwd=TAURI,
        expect_fail_hint="破坏性写（把行踢出重试集合）必须排最后",
        tags=["rust", "file", "stability", "new-guards", "st3-terminal"],
    ),
    Case(
        name="重复 FileDone 的「本机没这份文件」出口不许退回静默",
        why="2026-09-23 审计 A5：`Ok(None)` 分支原先只有 `if already_done { 回成功 Ack }`、**没有 else**\n"
        "     ⇒ 本机从没收下这份文件时一帧都不回，发送端只能干等整个静默窗口（`FILE_ACK_IDLE`）\n"
        "     才判失败，然后整套重发。注入方式就是回到修好之前的形状：整块删掉 else。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            """                    } else {
                        // 接收器已清且库非 done（接收态被 TTL 回收 / 本机从未接受该
                        // 传输）：也必须回一帧失败确认（2026-09-23 审计 A5）—— 旧实现
                        // 这里静默，发送端收不到任何帧只能干等静默窗口超时。立刻拿到
                        // 失败终态才不会白等；发送端只认 file_transfers 里登记的
                        // peer_id，伪造面不变。
                        let _ = try_send(
                            state,
                            peer_id,
                            &Message::FileCompleteAck {
                                transfer_id,
                                success: false,
                            },
                        )
                        .await;
                    }
""",
            # 只留 `if already_done { … }` 的收尾花括号 —— 就是修好之前的形状。
            # ⚠️ 替换串不能是 ""：那会把 if 的收尾括号一起删掉，红的是编译器而不是判据
            #（第一版就这么错过了一次真确认，2026-09-23）。
            "                    }\n",
        )],
        cmd=cargo("test", "--lib", "duplicate_file_done_still_answers_with_an_ack"),
        cwd=TAURI,
        expect_fail_hint="两个出口各回一帧 Ack",
        tags=["rust", "file", "stability", "new-guards", "st3-terminal"],
    ),
    Case(
        name="「对方已收完」的捷径不许绕过发送端收尾（少一份 = 气泡永久转圈）",
        why="2026-09-23 审计 A6 的**自审发现**：接收端回 `received = size` 后发送端直接\n"
        "     `return Ok(())` 是对的，但收尾整段写在 `stream_file` 里，捷径一绕过去，\n"
        "     `file_transfers` 停在 pending、`file-*` 不推进 delivered、三个事件一个都不发\n"
        "     ⇒ 对方已经收到文件，我方气泡却一直转圈。注入方式 = 把它「退化回只 return」，\n"
        "     这正是当初会写出的形状（少写一处而不是写错一处）。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """                    // 收尾与 stream_file 拿到成功回执时**共用同一份**：少了它，对方明明
                    // 已经有了，本机气泡却永久转圈、transfer 行停在 pending。
                    finalize_send_accepted(state, transfer_id, peer_id, &name, size, &path);
""",
            "",
        )],
        cmd=cargo("test", "--lib", "already_have_shortcut_shares_the_send_finalization"),
        cwd=TAURI,
        expect_fail_hint="少于 3 = 又出现一条",
        tags=["rust", "file", "stability", "new-guards", "st3-terminal"],
    ),
    Case(
        name="中继 Offer 不许无条件发出（0 个邻居接住必须当场判失败）",
        why="A1 的「下限判据」：Offer 没有任何邻居接住 ⇒ 这一帧从未离开本机，正确做法是在**读盘之前**\n"
        "     就失败。注入方式 = 退回修好之前的那一行（发完就走、不看结果）。\n"
        "     ⚠️ 反向不成立：`≥1` 不代表送达（邻居未必与目标有直连），所以守卫钉的是\n"
        "     「两处判据都在」，而不是「它保证成功」—— A1-L2 的接收端回执才是真证明。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """        if crate::network::transport::relay_send_to_neighbors(state, peer_id, &offer).await == 0 {
            return Err("没有可达的中继邻居：文件未发出".to_string());
        }
""",
            "        let _ = crate::network::transport::relay_send_to_neighbors(state, peer_id, &offer).await;\n",
        )],
        cmd=cargo("test", "--lib", "relay_send_does_not_claim_unproven_success"),
        cwd=TAURI,
        expect_fail_hint="Offer 与每一片各一处",
        tags=["rust", "relay", "file", "stability", "new-guards", "a1-l1"],
    ),
    Case(
        name="中继发送失败必须落 DB 终态（发送方向没有任何清扫器兜底）",
        why="接收侧有 `sweep_stale_relay`，**发送侧一行 active 没人管**。旧实现里取消 / 读盘失败 /\n"
        "     整体超时 / 分片失败每一条 Err 路径都把行留在 active —— 界面当场报失败，\n"
        "     重启后它又回到「进行中 X%」并永久挂着。修法是把包装层做成唯一出口：失败统一标 failed。\n"
        "     注入方式 = 删掉这段标记（回到「只有内层推流、外层不管终态」的形状）。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """    if let Err(reason) = &outcome {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = db::mark_transfer_failed_if_active(&dbc, transfer_id) {
            state.logger.warn(
                "file",
                format!(
                    "中继发送失败后落终态也失败 transfer={transfer_id}: {e}（发送失败原因：{reason}）"
                ),
            );
        }
    }
""",
            "",
        )],
        cmd=cargo("test", "--lib", "relay_send_does_not_claim_unproven_success"),
        cwd=TAURI,
        expect_fail_hint="失败必须落 DB 终态",
        tags=["rust", "relay", "file", "stability", "new-guards", "a1-l1"],
    ),

    Case(
        name="中继推送写完分片只能落 sent，不许直接写 done",
        why="审计 A1 的 L2 那一半（L1 只修了「0 个邻居接住」这条 Err 出口）。\n"
        "     循环跑完仅代表每一片都被**至少一个邻居**接住，邻居不保证与对端有直连，\n"
        "     更没有「对端收全 + SHA 校验通过 + 落盘」的证据 —— 写 done 就是无证据的成功，\n"
        "     直接命中验收红线「不要因为 TCP write 成功就认为消息已送达」。\n"
        "     注入方式 = 把终态改回修好之前的 done（一行，形状与修好前完全一致）。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """                "send",
                "sent",
                Some(path.to_string_lossy().as_ref()),""",
            """                "send",
                "done",
                Some(path.to_string_lossy().as_ref()),""",
        )],
        cmd=cargo("test", "--lib", "relay_send_does_not_claim_unproven_success"),
        cwd=TAURI,
        expect_fail_hint="中继推送写完分片必须落 sent",
        tags=["rust", "relay", "file", "stability", "new-guards", "a1-l2"],
    ),
    Case(
        name="没有回执的推送不许广播完成事件",
        why="事件名从 file-progress 改成 file-done：DB 那行仍是 sent，但前端 `onFileDone`\n"
        "     会把内存里那行写成 done ⇒ 「库里 sent、界面上 ✓」，正是要防的那条不一致。\n"
        "     注入刻意**只换事件名**、载荷结构不动（emit 对载荷是泛型，编译照过），\n"
        "     这样红的一定是判据而不是编译器。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """        let _ = state.app.emit(
            "file-progress",
            &crate::state::FileProgress {
                transfer_id: transfer_id.to_string(),
                received: size,""",
            """        let _ = state.app.emit(
            "file-done",
            &crate::state::FileProgress {
                transfer_id: transfer_id.to_string(),
                received: size,""",
        )],
        cmd=cargo("test", "--lib", "relay_send_does_not_claim_unproven_success"),
        cwd=TAURI,
        expect_fail_hint="没有回执的推送不许广播完成事件",
        tags=["rust", "relay", "file", "stability", "new-guards", "a1-l2"],
    ),
    # ---------------- attempt epoch 接线（2026-09-23 真机 600MB，用户选 B） ----------------
    Case(
        name="分片帧写死 attempt:None —— 编译得过、判据必须红",
        why="上一轮还压在链路队列里的分片（Low 队列 1024 槽 ≈ 262MB）落到新一轮上时，\n"
        "     旧代码把它判成「跳号」⇒ 整单打死，这就是 600MB 反复失败的直接机制。\n"
        "     注入刻意用 `attempt: None`（字段仍在、类型仍对）而不是删掉：删字段红的是编译器，\n"
        "     那种「确认」证明不了接线（A5 那轮踩过一次，教训写在 a1-l2 的用例注释里）。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """            seq,
            data,
            attempt,
        };""",
            """            seq,
            data,
            attempt: None,
        };""",
        )],
        cmd=cargo("test", "--lib", "file_attempt_epoch_is_wired_on_both_sides"),
        cwd=TAURI,
        expect_fail_hint="FileChunk 必须带 attempt",
        tags=["rust", "file", "stability", "new-guards", "file-epoch"],
    ),
    Case(
        name="Offer 只有一条接受路径设定轮次（半边没接）",
        why="FileOffer 有两条接受出口：幂等 accept（含续传段归零）与新建/续建接收器。\n"
        "     漏一条 = 那条路径上接收器停在第 0 轮 ⇒ 此后**所有**新轮分片都被当陈旧丢掉，\n"
        "     表现是「进度条走到一半再也不动、也不报错」——比原来的跳号判死更难查。",
        file=TAURI / "src" / "network" / "transport.rs",
        injections=[(
            "                        file::note_offer_attempt(state, &transfer_id, attempt);\n",
            "",
        )],
        cmd=cargo("test", "--lib", "file_attempt_epoch_is_wired_on_both_sides"),
        cwd=TAURI,
        expect_fail_hint="幂等 accept 与新建/续建接收器两条路径各一次",
        tags=["rust", "file", "stability", "new-guards", "file-epoch"],
    ),
    Case(
        name="attempt 不门控能力位就对老端发新语义",
        why="ADR-0007 / INV-P24 的硬要求：新帧新语义必须先按对端能力门控。\n"
        "     老端虽然会忽略未知字段，但「忽略」是**依赖对方 serde 配置**的赌注；\n"
        "     门控之后新端不发、老端不收，兼容面回到字节层面一致。注入 = 删掉早退分支。",
        file=TAURI / "src" / "network" / "file.rs",
        injections=[(
            """    if caps & crate::protocol::CONTENT_FEATURE_FILE_EPOCH == 0 {
        return None;
    }
""",
            "",
        )],
        cmd=cargo("test", "--lib", "file_attempt_epoch_is_wired_on_both_sides"),
        cwd=TAURI,
        expect_fail_hint="不门控就等于对老端发新语义",
        tags=["rust", "file", "stability", "new-guards", "file-epoch"],
    ),]
