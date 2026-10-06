// 职责边界：
// - `lib_tests.rs` 测试分册之5 —— 文件传输形状：泡前不整档扫、进度按已写出字节、收尾在锁外、取消登记按收件人
// 为什么拆：`src-tauri/src/lib_tests.rs` 原来 4,009 行、96 个顶层项挤在同一个 `mod tests` 里。
// 机制与 `network/transport/tests.rs` 那一刀同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::lib_tests` 里那同一个 `mod tests` ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ **本册必须与 `lib_tests.rs` 同级平铺**（不许挪进 `lib_tests/` 子目录）：册里到处是
// `include_str!("commands.rs")`、`include_str!("../gen/android/…")` 这类**相对本文件**的路径，
// 换目录会让它们整体偏移一位 —— 编译器会拒（不会静默），但那就不再是"逐字未变"的搬家，
// 恒等判据当场降级成"我读过觉得没变"。同目录先例：`protocol_tests.rs`。
// ⚠️ 册名以 `_tests.rs` 结尾也是被判据选定的：`transport_src_for_guards()` 那份"生产码全集"视图
// 只许装生产码，而登记对账守卫按 `_tests.rs` 后缀豁免测试分册（并进视图是最顺手却假绿的消红办法）。
    /// **气泡前不得做 O(体积) 的整文件扫描**（真机 Mac 发 600MB：点完"卡一会儿"才出现
    /// 发送中气泡 —— 建发送记录时先整读一遍文件算 sha256，而投递任务待会儿还要再读一遍）。
    ///
    /// 这条不变量很容易"顺手写回去"：它看起来只是"提前把 cid 准备好"，代价却挂在用户
    /// 点下发送按钮之后。cid 的正确来源是投递任务（它本来就要算，用于 FileOffer 校验值），
    /// 算完回填发送行。
    #[test]
    fn no_whole_file_scan_before_the_file_bubble() {
        let files = include_str!("commands/files.rs");
        let build = rust_fn_body(files, "fn build_file_message(");
        assert!(
            !build.contains("sha256_file_hex("),
            "建发送记录前不得整读文件算哈希：那会把气泡挡在一次 O(体积) 扫描之后"
        );
        assert!(
            build.contains("\"sha256\": \"\""),
            "cid 必须以空串占位（载荷形状不能变，前端按 sha256 键取值）"
        );

        let file = include_str!("network/file.rs");
        let body = rust_fn_body(file, "pub async fn send_file_from_path_at(");
        assert!(
            body.contains("spawn_blocking"),
            "整文件哈希必须在阻塞线程池：占住 async worker 会连带拖慢其它传输"
        );
        assert!(
            body.contains("fill_message_sha256"),
            "投递任务算出的 cid 必须回填发送行，否则本地合并卡片按 cid 找不回字节"
        );
        assert!(
            body.contains("backfilled") && body.contains("emit(\"message-received\", &rec)"),
            "回填改了库就必须通知前端：只写库时内存里那条记录仍是 cid 空的版本，\
             同一会话把刚发的文件转成合并卡片会丢掉对端的拉取钥匙（v4.22.16 的回归）"
        );
    }

    /// **接收端判死必须把否定确认送回，而且发送端要能在分片循环里立刻看到**。
    ///
    /// 旧行为是"判死但谁也不告诉"：发送端把剩下的整份文件继续灌进一条已经死掉的传输，
    /// FileDone 无人应答 → 干等一个 `FILE_ACK_IDLE` → 判"可重试" → 再整发 5 次。
    /// 真机形状：600MB 跑到 100% 两边都显示失败，中间几十分钟界面一直"发送中"。
    /// 这条链有**三个环节**（回帧、早注册、循环里盯），断掉任意一个都看不出差别，
    /// 所以三个点一起钉。
    #[test]
    fn receiver_abort_notifies_the_sender_inside_the_loop() {
        let tr = code_flat(&crate::network::transport_src_for_guards());
        assert!(
            tr.contains("iffile::fail_receive(state,&transfer_id,peer_id,&e){")
                && tr.contains(
                    "Message::FileCompleteAck{transfer_id:transfer_id.clone(),success:false,}"
                ),
            "分片判死必须回 FileCompleteAck{{success:false}}（fail_receive 返回 false 表示没有活的\
             接收器，不重复回）"
        );
        assert!(
            !tr.contains("let _=file::fail_receive(state,&transfer_id,peer_id,&e);"),
            "不得退回「abort 了但不告诉发送端」的旧写法"
        );
        let f = code_flat(include_str!("network/file.rs"));
        assert!(
            f.contains("_=&mutack_rx=>"),
            "分片循环必须盯着否定确认，才能当场停手而不是把剩余字节灌完"
        );
        assert_eq!(
            f.matches(".insert(transfer_id.to_string(),tx)").count(),
            2,
            "pending_file_accept 与 pending_file_complete 各登记一次；出现第三次说明有人\
             又把 complete 注册挪回了循环之后（那样循环里的 ack_rx 就成了死代码）"
        );
    }

    /// **群文件投递的取消登记必须按 (transfer_id, recipient) 分键**（真机：三成员以上
    /// 的群文件只有一个人收得到）。
    ///
    /// 群发是「每个可达成员各 `tokio::spawn` 一个任务、共用同一个 `transfer_id`」。
    /// 单键时后注册的 `HashMap::insert` 会挤掉前一个任务的 `Sender`，对方的 oneshot 立刻
    /// 以 `Err(RecvError)` 完成，而投递循环的取消分支分不清「用户真点了取消」和
    /// 「登记被顶替」⇒ N-1 个成员以「用户取消发送」这个假原因当场中断。
    /// 只能钉源码：这是语言级细节，行为测试要先构造出"顶替"才看得到。
    #[test]
    fn group_file_cancel_registry_is_scoped_per_recipient() {
        let dispatch = code_flat(include_str!("commands/group_file_dispatch.rs"));
        let fanout = code_flat(include_str!("commands/group_announcements.rs"));
        assert!(
            dispatch.contains("file_cancel_key(transfer_id,recipient)"),
            "群投递的取消登记必须带 recipient（否则同 transfer_id 的任务互相挤掉登记）"
        );
        assert!(
            !dispatch.contains(".insert(transfer_id.to_string(),cancel_tx)"),
            "不得回退成 transfer_id 单键"
        );
        // ★ 出错必须落终态，而且**两条投递路径共用同一个裁决**（#154-4）。
        // 原先只有群发那条写状态、且按错误**文案**猜 cancelled/failed；离线补发那条
        // 什么都不写 ⇒ 补发失败一次就把该成员永久卡在 sending（重试查询只捞 pending）。
        let requeue = code_flat(include_str!("commands/group_file_keys.rs"));
        for (name, src) in [
            ("group_announcements.rs（群发 fan-out）", fanout.clone()),
            ("group_file_keys.rs（离线补发）", requeue.clone()),
        ] {
            assert!(
                src.contains("group_file_status_after_fail(&e)"),
                "{name} 必须走那一个终态裁决，不许自己判状态"
            );
            assert!(
                src.contains("update_group_file_recipient("),
                "{name} 必须真的把终态写回库：离线补发只捞 status='pending'，\
                 留在 sending 就是「永远在发、重启也不重试」"
            );
        }
        assert!(
            !fanout.contains("\"用户取消发送\""),
            "不许再按错误**文案**猜状态（文案一改就判错；状态由 GroupFileSendErr 的种类决定）"
        );
    }

    /// 发送进度必须落在"已写出链路"上，不许回到"已入队"（v4.22.37 的用户可见症状）。
    ///
    /// 六处同时成立才有意义，所以一起钉：
    ///   ① writer 那唯一的证据点必须**累加片数**（只记时刻的话进度算不出来）；
    ///   ② 状态表的值必须是带 `chunks` 的结构，而不是裸时间戳；
    ///   ③ `stream_file` 的进度与 `file-progress` 事件必须用换算后的 `on_wire` ——
    ///      旧写法 `received: sent` 就是那个"262MB 还在队列里就 100%"的假象。
    ///   ④ 生命周期：装了回收守卫，且清理只此一处；
    ///   ⑤ 键按 (传输 × 收件人)：写侧与读侧必须同一个 `file_peer_key`（#35 —— 群发是 N 个
    ///      任务共用一个 `transfer_id`，退回裸 id 就是"甲的写出替乙续命"）；
    ///   ⑥ 群投递侧同样装守卫、同样按收件人判停滞，且**中继帧不记账**这条决定被钉住。
    /// 判据都取**函数体**而不是全文：`received: sent` 在中继发文件那条路径里是合法的
    /// （另一套语义），全文一扫会误伤。
    #[test]
    fn file_send_progress_counts_wire_not_queue() {
        let file = include_str!("network/file.rs");
        let body = rust_fn_body(file, "async fn stream_file(");
        assert!(
            body.contains("wire_progress_bytes("),
            "stream_file 的进度必须经 wire_progress_bytes 换算"
        );
        assert!(
            body.contains("file_wire_chunks_at("),
            "换算必须读 writer 记的已写出片数，否则等于没换"
        );
        assert!(
            !body.contains("received: sent"),
            "file-progress 不许再直接发入队量（那就是 100% 假象）"
        );
        // ④ 记账的生命周期：装了回收守卫，且**没有第二处手写清理**。
        //    只测 `WireLedger` 的 Drop 语义不够 —— 把 stream_file 里那行装守卫的代码删掉，
        //    Drop 测试照样全绿（它测的是辅助类型，不是接线）。
        assert!(
            body.contains("WireLedger::install("),
            "stream_file 必须装写出记账的回收守卫，否则失败路径的残留会让重试退回入队口径"
        );
        assert!(
            !body.contains("clear_file_wire_progress"),
            "清理只许有 WireLedger::drop 一处，第二处迟早与它口径不同"
        );
        // ⑤ 键必须按收件人（#35）。读侧只要有一处退回裸 `transfer_id`，读到的就是
        //    "别人的写出"或"永远没有写出" —— 两者都静默。
        assert!(
            body.contains("file_wire_chunks_at(state, &wire_key)"),
            "进度换算必须读 (传输 × 收件人) 那一条记账，裸 transfer_id 是旧口径"
        );
        assert!(
            !body.contains("file_wire_chunks_at(state, transfer_id)"),
            "stream_file 里出现裸 transfer_id 读记账 = 与 writer 的键不一致，进度会永远 0"
        );

        // ⑥ 群发投递侧同样要装回收守卫并按收件人查停滞（N 个任务共用一个 transfer_id）。
        let dispatch = include_str!("commands/group_file_dispatch.rs");
        assert!(
            dispatch.contains("file::WireLedger::install("),
            "群文件投递必须回收自己的写出记账，否则每个成员一条记录永久留在表里"
        );
        assert!(
            dispatch.contains("file::stall_tick(") && dispatch.contains("recipient,"),
            "群侧停滞判定必须带上收件人，不然甲的写出会把乙的卡死盖住"
        );

        let transport = crate::network::transport_src_for_guards();
        let mark = rust_fn_body(&transport, "pub(crate) fn mark_file_wire_progress(");
        assert!(
            code_flat(&mark).contains("bump_file_wire_progress_in(&state.file_wire_progress,&"),
            "writer 的写出证据点必须落进记账表（拆成 bump_* 是为了单测能直接喂一张表）"
        );
        assert!(
            code_flat(&mark).contains("file_peer_key(transfer_id,recipient)"),
            "证据点必须按 (传输 × 收件人) 成键 —— 群发 N 个任务共用一个 transfer_id"
        );
        // 中继帧**故意**不记账（见 `mark_file_wire_progress` 的注释）。这条断言不是装饰：
        // 没有它，下一个"顺手补上 RelayChunk"的改动会让每个转发节点都为别人的传输留一条
        // 没人回收的记录 —— 转发侧没有 WireLedger，那是比"进度不准"更糟的形状。
        assert!(
            !mark.contains("RelayChunk"),
            "RelayChunk 不进入写出记账是**已登记的决定**；要改必须先解决\"谁回收转发侧的记录\""
        );
        let bump = rust_fn_body(&transport, "pub(crate) fn bump_file_wire_progress_in(");
        assert!(
            code_flat(&bump).contains("p.chunks=p.chunks.saturating_add(1)"),
            "writer 的写出证据点必须累加片数"
        );
        let state = include_str!("state.rs");
        assert!(
            state.contains("Mutex<HashMap<String, FileWireProgress>>"),
            "进展表的值必须是 {{at_ms, chunks}} 结构，时间戳单独一个字段撑不起进度口径"
        );
    }

    /// 重复 FileDone 不许静默（2026-09-23 审计 A5）：接收器已被清时，两个出口都得回 Ack。
    ///
    /// 旧代码只有 `if already_done { 回成功 Ack }` 而**没有 else** ⇒ 本机从没收下这份文件时
    /// 一帧都不回，发送端只能干等整个静默窗口（`FILE_ACK_IDLE`）才判失败，然后整套重发。
    /// 为什么是源码守卫：这一条是 `handle_message` 的一个 match 臂，本仓没有能驱动它并捕获
    /// 出站帧的异步夹具（与 A3 那条同理）。判据取「锚点注释 → 下一个 `Ok(Some(` 之间」这段
    /// 区域，避开同函数里其他 FileCompleteAck 的计数干扰。
    #[test]
    fn duplicate_file_done_still_answers_with_an_ack() {
        let src = crate::network::transport_src_for_guards();
        let body = rust_fn_body(&src, "pub async fn handle_message(");
        let at = body.find("重复 FileDone").expect(
            "handle_message 里那段「重复 FileDone」判据注释不见了 ⇒ 这条守卫的区域锚点失效",
        );
        let arm = &body[at..];
        let region = &arm[..arm
            .find("Ok(Some(")
            .expect("接收器在位的那条分支锚点不见了")];
        let flat = code_flat(region);
        assert_eq!(
            flat.matches("Message::FileCompleteAck{").count(),
            2,
            "两个出口各回一帧 Ack：已收完回成功、没收下也必须回失败（静默 = 发送端白等一整个静默窗口）"
        );
        assert!(
            flat.contains("Message::FileCompleteAck{transfer_id,success:false,}"),
            "「本机没这份文件」的出口必须回**失败** Ack；写成功会让发送端把没收下的文件标成已送达"
        );
    }

    /// 「对方已收完」这条捷径必须和成功回执走**同一份**收尾（2026-09-23 审计 A6 的自审发现）。
    ///
    /// Offer 阶段对方直接回 `received = size` 时，发送端 `return Ok(())` 是不再发一个分片的
    /// 正确决定 —— 但收尾（`file_transfers` 落 done、`file-*` 推进 delivered、
    /// `message-acked` / `file-done` / `file-progress` 三个事件）原本整段写在 `stream_file`
    /// 里，捷径一绕过去就是**对方已经收到、我方气泡永久转圈**。判据用定义+调用的总数，
    /// 因为"少了哪一处"正是这类缺陷的形状；两个调用点各自对应一条成功证据。
    #[test]
    fn already_have_shortcut_shares_the_send_finalization() {
        let file = include_str!("network/file.rs");
        assert_eq!(
            file.matches("finalize_send_accepted(").count(),
            3,
            "1 处定义 + 2 处调用（stream_file 成功回执 / Offer 的 received≥size 捷径）。\
             少于 3 = 又出现一条「发送成功却没落终态」的出口 —— 用户看到的是文件气泡一直转圈"
        );
        let arm = rust_fn_body(file, "pub async fn send_file_from_path_at(");
        let at = arm
            .find("if n >= size && size > 0")
            .expect("Offer 阶段「对方已收完」那条分支的判据锚点不见了");
        assert!(
            arm[at..]
                .find("finalize_send_accepted(")
                .is_some_and(|i| i < arm[at..].find("return Ok(())").unwrap_or(usize::MAX)),
            "「对方已收完」必须先落收尾再 return，否则本机状态永远停在 pending"
        );
    }

    /// 接收端 `finish_receive`：慢活必须在**放锁之后**（2026-09-23 真机 600MB + 多文件复核）。
    ///
    /// 为什么钉形状而不是钉行为：要复现"持锁 fsync 堵住别的并发传输"需要两条在途传输 +
    /// 真实磁盘 + 计时，单元层拿不住；而形状一旦退化（锁挪回函数作用域），表现是
    /// "多文件里有一个大文件时其它文件莫名停滞判死"，回查成本极高。
    /// 判据用**位置比较**而不是"调用了几次"：退化前后 `sync_all()` / `finalize()` 的次数一模一样。
    ///
    /// 2026-09-24（#25）慢活整体搬进了 `finish_receiver_into`（那个函数不要 `AppState`，
    /// 所以"显示成功是不是真成功"终于能被单测驱动）⇒ 判据跟着升级成三件事：
    /// 摘锁形状留在包装函数里、慢活留在核心里、而**核心拿不到 `file_receivers` 那张表**
    /// （拿不到就不可能把锁再写回来，这比"位置在后面"强：它是结构性的）。
    #[test]
    fn receive_finalize_slow_work_happens_outside_the_receiver_lock() {
        let file = include_str!("network/file.rs");
        let body = rust_fn_body(file, "pub fn finish_receive(");
        let flat = code_flat(&body);
        assert!(
            flat.contains("letr={letmutrecv=state.file_receivers.lock()"),
            "接收器必须在块内摘出（块尾即放锁）。函数作用域的锁会把 SHA+fsync+rename \
             全包进锁里 —— 600MB 的 fsync 期间，其它并发文件的 write_chunk 全堵在同一把锁上，\
             它们不再写出 ⇒ 发送端 60s 停滞判据把它们判死"
        );
        assert!(
            !flat.contains("letmutrecv=state.file_receivers.lock().unwrap_or_else(|e|e.into_inner());letr=matchrecv.remove"),
            "旧的函数作用域形状不许回来（那份守卫横跨全部慢活）"
        );
        // 来源不符时的"塞回去"必须留在锁内
        assert!(
            flat.contains("returnErr(\"文件传输来源不匹配\".to_string());}"),
            "找不到来源不符分支 —— 护栏需要同步更新"
        );
        // 摘出来的那份接收器必须**交给核心**，不是就地收尾（就地收尾 = 慢活回到锁的同一作用域）
        assert!(
            flat.contains("finish_receiver_into(&state.db,transfer_id,r)"),
            "包装函数必须把摘出的接收器整体交给 finish_receiver_into —— 自己留着用就是持锁慢活"
        );
        let core = rust_fn_body(file, "fn finish_receiver_into(");
        let core_flat = code_flat(&core);
        for slow in [".sync_all()", "hasher.clone().finalize()"] {
            assert!(
                core_flat.contains(slow),
                "慢活 {slow} 必须在 finish_receiver_into 里（挪走≠删掉，掉了它这份文件永远不会落盘）"
            );
        }
        assert!(
            !core_flat.contains("file_receivers"),
            "核心函数不得再碰 file_receivers 表：一碰就能把锁写回慢活的作用域（这条是结构性的）"
        );
    }
