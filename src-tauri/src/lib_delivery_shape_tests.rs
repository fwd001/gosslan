// 职责边界：
// - `lib_tests.rs` 测试分册之11 —— 投递形状：未知帧容忍、离线仍列出、群密钥先于群消息、发送成功后才落身份
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
    /// **INV-P24 第 1 条：未知帧类型必须降级，不得成为连接错误**（ADR-0007 落地①）。
    ///
    /// 为什么只能钉源码：这条的行为是"什么都没发生"—— 没有测试会因为**缺少**它而失败，
    /// 而它的缺失后果极重：新版本上线任何一种新帧，老设备不是少收一条消息，
    /// 而是**跟新设备连不上**（反序列化失败 → io::Error → reader 退出 → 重连再失败）。
    /// 同时**握手首帧不许降级**：未认证的连接没有"看不懂就放过"的理由。
    #[test]
    fn unknown_wire_frame_is_tolerated_after_auth() {
        let out = code_flat(include_str!("network/transport/outbound.rs"));
        assert!(
            out.contains("Err(e)ife.to_string().starts_with(\"unknownvariant\")"),
            "未知变体必须由 serde 自己的措辞识别（不维护第二份类型清单，清单会漂移）"
        );
        assert!(
            out.contains("Ok(Message::Unknown{wire_type:tag})"),
            "未知 type 必须降级成 Message::Unknown"
        );
        assert!(
            out.contains("fnread_frame") && out.contains("decode_frame(&buf)"),
            "数据面 read_frame 必须走 decode_frame，否则降级形同不存在"
        );
        let preauth = rust_fn_body(
            include_str!("network/transport/outbound.rs"),
            "async fn read_frame_preauth",
        );
        assert!(
            !preauth.contains("decode_frame"),
            "握手首帧不得降级：非 Hello / 看不懂的帧在认证前就该拒掉"
        );
        let agg = code_flat(&crate::network::transport_src_for_guards());
        assert!(
            agg.contains("Message::Unknown{wire_type}=>{"),
            "handle_message 必须有 Unknown 分支（忽略 + 节流日志），否则会退化成 panic 或误判"
        );
    }

    /// **掉线的节点要留在发现列表里，但不能算"在线"**（两个坑必须同时躲开）。
    ///
    /// 2026-09-12 真机（用户 4.2.11，只开蓝牙）：**手机能看到 Mac（"已发现未建联"），
    /// Mac 里安卓什么都不显示**。根因是链路一断就把节点条目删了 —— BLE 上
    /// "连上 → 被对端按指定拨号方退让 → 断开"是常态，Mac 刚学到身份就被删，
    /// 「添加好友」列表里只闪一下，用户根本点不到。
    ///
    /// 反过来，如果只是"保留条目"而沿用旧的「在 peers 表里就算在线」判据，
    /// 就会把复核抓到过的那个 High 缺陷重新引入（一次"连过又掉线"的节点永久在线）。
    /// 所以两件事必须一起成立：**条目保留**（`mark_peer_offline` 不删 peer）+
    /// **在线看 last_seen 新鲜度**（`friend_is_online`）。
    #[test]
    fn offline_peer_stays_listed_but_is_not_online() {
        let transport = crate::network::transport_src_for_guards();
        let body = rust_fn_body(&transport, "pub(crate) async fn mark_peer_offline(");
        assert!(
            !body.contains("remove(device_id)"),
            "链路断了**不能**立刻删节点条目：BLE 上「连上→退让→断开」是常态，\
             删掉的话「添加好友」列表里对端只闪一下（真机症状）"
        );
        assert!(
            body.contains("clear_conv_link"),
            "链路快照必须立刻清掉 —— 否则聊天头部会一直显示「桥接 N」（用户实测过）"
        );

        let commands = all_commands_src();
        assert!(
            commands.contains("fn friend_is_online("),
            "在线判据必须是独立纯函数（可单测、可护栏）"
        );
        assert!(
            commands
                .contains("friend_is_online(last_seen, now, active_links.contains(&f.device_id))"),
            "get_friends 必须走 friend_is_online：**只看「在不在节点表里」会让刚掉线的节点\
             保持在线的假象**（就是那个 High 缺陷）"
        );
        assert!(
            !commands.contains("f.online = peers.contains_key(&f.device_id)"),
            "旧的 presence 判据不得复活（条目现在会被保留，presence 不再等价于在线）"
        );
    }

    /// **在途文件不能被当成"已被清理"**（用户 2026-09-14：群里收图时好时坏，点几次/
    /// 等一会儿/重发才出来）。
    ///
    /// 接收方在 FileDone 之前写的是 <transfer_id>.part，final 路径尚不存在。若
    /// resolve_media_path 直接报 Gone，前端会把"正在接收"的图片标成「已被清理」并缓存。
    #[test]
    fn in_flight_media_is_not_reported_as_deleted() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "fn resolve_media_path(");
        assert!(
            body.contains("file_receivers")
                && body.contains("group_file_receivers")
                && body.contains("仍在接收"),
            "缺失的 final 文件必须先在途判定（file_receivers / group_file_receivers），在途报 Unknown 而不是 Gone"
        );
    }

    /// **链路徽标与在线状态必须"实时"**（用户 2026-09-14：两边全在局域网，却显示「已桥接」，
    /// 且好友在线状态不实时）。
    ///
    /// 两个必须同时成立的判据：
    /// 1. get_conv_link 有直连时按**此刻实际选路**返回 hop=0，不再回放"上一条消息"的快照；
    /// 2. peers-updated 要带上每个节点的活跃链路（link 字段），前端才能把"有链路但广播没收到"
    ///    的节点也算在线（与后端 friend_is_online 同口径）。
    #[test]
    fn link_badge_and_presence_are_live() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "pub async fn get_conv_link(");
        assert!(
            body.contains("has_link") && body.contains("hop: 0"),
            "有直连时 get_conv_link 必须以实时链路 + hop=0 返回，不能回放消息快照"
        );
        let state = include_str!("state.rs");
        assert!(
            state.contains("p.link = best_link_kind(&kinds)"),
            "peers-updated 必须带上活跃链路（link 字段），否则前端判不出「有链路但广播缺席」的在线"
        );
    }

    /// 任务的两条写命令必须**只在发送成功之后**打一行带 `task=` 的日志（第二阶段 §29）。
    ///
    /// 为什么以前是零：现算过，`src-tauri/src/commands` 里 29 处 `logger.` 调用**无一与任务有关**，
    /// 全仓日志密度都堆在网络层（ble 51 / transport 47 / file 13）⇒ "任务没同步过去"只能靠比对
    /// 两侧的库来定位死在哪一段。判据取**顺序 + 内容 + 不泄漏**三件，不是"那串字面量在不在"：
    /// - 日志早于 `send_group_payload(` ⇒ 会记出一条其实没建成的任务（与"emit 必须由写库成功门控"同族）；
    /// - 少任一行的标记 ⇒ 任务链退回零可观测；
    /// - 格式串里出现 `{title}` 或 `description` ⇒ 用户内容进日志（同"token 值不得入日志"那一族约束）。
    #[test]
    fn todo_commands_log_identity_only_after_the_send_succeeded() {
        let commands = all_commands_src();
        for (head, marker) in [
            ("pub async fn send_group_todo(", "群任务已建 task="),
            ("pub async fn update_group_todo(", "群任务已改 task="),
        ] {
            let body = rust_fn_body(&commands, head);
            let send = body.find("send_group_payload(").unwrap_or_else(|| {
                panic!("{head} 里找不到 send_group_payload ⇒ 形状变了，这条判据要跟着改")
            });
            let log = body.find(marker).unwrap_or_else(|| {
                panic!("{head} 没有打「{marker}」⇒ 任务生命周期又回到零日志（§29）")
            });
            assert!(
                send < log,
                "{head} 的日志早于发送：会记出一条其实没成功的任务"
            );
            let logged = &body[log..(log + 400).min(body.len())];
            // ⚠️ 这里查的是**真实泄漏形状**而不是字面量 `{title}`：非空转实测过，
            // 只查 `{title}` 时"把 payload.title 用 {} 铺进日志"这种坏法完全抓不到（退码仍 0），
            // 那条 clause 等于装饰。故三种写法都堵：字段访问 `.title`、行内捕获 `{title}`、描述字段。
            assert!(
                !logged.contains(".title")
                    && !logged.contains("{title}")
                    && !logged.contains("description"),
                "{head} 的任务日志把用户内容（标题/描述）铺进了日志行"
            );
        }
        // 内核自 emit 之后必须留一行痕迹：双实例 harness 没有任何入口让应用自己执行动作，
        // 所以「A 为什么看得见自己发的那条」只能靠这一行来判（否则永远是循环论证）。
        // 判据仍取顺序：痕迹早于 emit 就成了"还没通知就先记账"，与上面那条同族。
        let kernel = rust_fn_body(&commands, "async fn send_group_payload(");
        let emit = kernel
            .find(r#"emit("message-received""#)
            .expect("群发送内核没有回送本机窗口 ⇒ #82 那格修复被移走了");
        let klog = kernel
            .find("群消息已回送本机窗口 msg=")
            .expect("群发送内核回送后没留痕 ⇒ harness 无法判「本端确实被通知」（§29）");
        assert!(
            emit < klog,
            "自 emit 的痕迹早于 emit 本身：会记出一条其实没通知出去的消息"
        );
    }

    /// 群密钥必须**先于**群消息补发，且每次建链 / Hello / 心跳都重新登记（2026-09-24 RC2）。
    ///
    /// 三个触发点顺序错任何一处，表现都一样：`handle_gossip` 在解密之前就把 msg_id 登进
    /// 去重表 ⇒ 密钥后到时那条群消息已经被"见过"挡掉，之后 group_outbox 重发多少次都
    /// 没有消费者（而且行还会在窗口到期时被删掉）。这条只能钉接线 —— 判据本身没有纯函数，
    /// 是三步调用的先后。
    #[test]
    fn group_keys_always_precede_group_messages() {
        let flat = code_flat(&crate::network::transport_src_for_guards());
        // 只数**调用**（`state,&xxx)` 这种实参形状）：函数定义那行 `state: &AppState` 也含同样的前缀，
        // 按前缀数会多数一次 ⇒ 判据必须是"调用点"的完整形状。
        let calls = flat
            .matches("requeue_group_keys_for_peer(state,&peer_id);")
            .count()
            + flat
                .matches("requeue_group_keys_for_peer(state,&device_id);")
                .count();
        assert_eq!(
            calls, 3,
            "三处触发点（拨号建链 / Hello / 心跳）都要先重新登记密钥；少一处 = 那条路径上\\
             链路抖动丢过的 GroupKey 永远不会再发（GroupKey 没有回执帧）"
        );
        // 每一处 flush_group_outbox 之前必须已经有 flush_pending_group_keys（同一串里）
        let mut seen_ok = 0;
        for (i, _) in flat.match_indices("flush_group_outbox(state,") {
            // 按字节回退窗口会切进多字节字符里直接 panic ⇒ 必须先吸附到字符边界
            let raw = i.saturating_sub(220);
            let start = (raw..i).find(|&k| flat.is_char_boundary(k)).unwrap_or(i);
            let window = &flat[start..i];
            if window.contains("flush_pending_group_keys(state,") {
                seen_ok += 1;
            }
        }
        assert_eq!(
            seen_ok,
            flat.matches("flush_group_outbox(state,").count(),
            "有一处群消息补发排在群密钥之前 ⇒ 密钥没到的那条消息会被去重表永久挡掉"
        );
    }

    /// attempt epoch 的**接线**（2026-09-23 真机 600MB 根治，用户拍板选 B）。
    ///
    /// 判据本身有单测（`frame_is_current`、协议双向兼容），这里钉的是接线是否四处都在：
    /// ① 三帧都带上轮次；② 轮次来自持久化的 outbox.attempts 且必须门控；
    /// ③ 接收侧分片与完成帧都过判据；④ Offer 那两条接受路径都设定轮次。
    /// 为什么用源码守卫：要跑通"上一轮分片还压在链路队列里、新一轮已经开始"需要双链路夹具
    /// 加真实背压，本仓没有这种夹具；而接错任何一处的表现都是真机上那份大文件又死了。
    #[test]
    fn file_attempt_epoch_is_wired_on_both_sides() {
        // 视图读"一个家"`network::file_src_for_guards()`（2026-10-07 file.rs 按角色切成 include! 分册）：
            // 读单个文件只会看见主册 ⇒ 形状守卫对分册失明（假绿形状）。登记对账由 lib_source_view_tests 第四个用例钉两侧。
                    let file = &crate::network::file_src_for_guards();
        let tv = crate::network::transport_src_for_guards();
        // ① 三帧都必须带轮次（少一处 = 那一类帧逃过过滤，病根原样保留）
        let stream = code_flat(&rust_fn_body(file, "async fn stream_file("));
        assert!(
            stream.contains(
                "Message::FileChunk{transfer_id:transfer_id.to_string(),seq,data,attempt,}"
            ),
            "FileChunk 必须带 attempt"
        );
        assert!(
            stream.contains("Message::FileDone{transfer_id:transfer_id.to_string(),attempt,}"),
            "FileDone 必须带 attempt —— 陈旧完成帧会把这一轮刚开头的传输判成\"未完成\"并打死"
        );
        assert!(
            code_flat(&rust_fn_body(file, "pub async fn send_file_from_path_at("))
                .contains("from_bytes:resume_from,attempt,}"),
            "FileOffer 必须带 attempt（它是接收端设定当前轮次的唯一来源）"
        );
        // ② 轮次必须门控，且必须来自**持久化**的 attempts
        let gate = code_flat(&rust_fn_body(file, "fn send_attempt("));
        assert!(
            gate.contains("CONTENT_FEATURE_FILE_EPOCH==0{returnNone;}"),
            "不门控就等于对老端发新语义（违反 ADR-0007 / INV-P24：新帧新语义必须先按能力位门控）"
        );
        assert!(
            gate.contains("get_file_outbox_attempts("),
            "轮次必须取持久化的 outbox.attempts：进程内计数器重启后回到 1，会比接收端已存的轮次还小 ⇒              这份文件的每一轮都被自己判成陈旧（永久饿死）"
        );
        assert!(
            !gate.contains("static") && !gate.contains("AtomicU32"),
            "同上：绝不允许改用进程内计数器当轮次"
        );
        // ③ 接收侧两处过滤
        assert!(
            code_flat(&rust_fn_body(file, "pub fn write_chunk("))
                .contains("!frame_is_current(attempt,r.attempt)"),
            "write_chunk 必须先挡非当前轮次的分片，且必须排在 chunk_seq_decision 之前"
        );
        assert!(
            tv.contains("file::done_is_current(state, &transfer_id, attempt)"),
            "FileDone 必须在 finish_receive 摘走接收器之前判轮次（摘完就分不清\"陈旧\"与\"重复\")"
        );
        // ④ Offer 的两条接受路径都要设定轮次：漏一条 = 那条路径上接收器停在第 0 轮，
        //    于是**所有**新轮分片都被当陈旧丢掉 ⇒ 文件永远差一截且不报错。
        assert_eq!(
            tv.matches("file::note_offer_attempt(state, &transfer_id, attempt)")
                .count(),
            2,
            "幂等 accept 与新建/续建接收器两条路径各一次，少一次就是半边没接"
        );
    }

    /// 「和自己聊天」必须是**纯本地**路径（用户 2026-09-16 的功能）。
    ///
    /// 为什么必须守：自聊一旦走成网络路径，会同时破坏两条不变量 ——
    /// ① 自己的消息进 outbox 后**永远等不到 Ack**（没有对端），那一行会被每次心跳/建链的
    ///    `flush_outbox` 重发，"outbox 必然排空"直接失效；
    /// ② `target = 自己` 的 gossip 信封对本机（`handle_gossip` 里 `sender == 自己` 早退）
    ///    和别人（解不开）都是噪声。
    /// 这两条在界面上**都看不出来**（消息照样显示、列表照样刷新），只有库里悄悄长出一条
    /// 永不消失的 outbox 行 —— 属于只能靠护栏拦的那类退化。
    #[test]
    fn self_chat_stays_local() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "fn insert_self_message(");
        assert!(
            !body.is_empty(),
            "找不到 insert_self_message（这条护栏会变成空转）"
        );
        // 先查"不该有的"：注入成 `insert_message_and_outbox(` 时下面那条 contains 也会失败，
        // 但真正该说的是"你接回了网络路径"——所以把这条放在前面报。
        for forbidden in [
            "insert_message_and_outbox(",
            "broadcast_gossip(",
            "try_send(",
            "crypto::seal(",
            "seal_symmetric(",
        ] {
            assert!(
                !body.contains(forbidden),
                "自聊消息里不得出现 `{forbidden}`：消息不出本机、也不该进 outbox（详见该函数文档）"
            );
        }
        assert!(
            body.contains("db::insert_message("),
            "自聊消息必须只落本地库（`db::insert_message`）"
        );
    }

    /// gossip / 不透明帧的转发候选必须来自**可达链路集**（2026-09-19 审计 P0#7）。
    ///
    /// 为什么必须守：`peers` 是知识集（Presence/announce 跨跳登记，异网段节点在里面
    /// 却没有链路）。历史版本 `choose_fanout` 的三个调用点都拿 `peers.keys()` 当候选，
    /// 跨网段时扇出全部拨向不可达节点、又被 `let _ =` 静默吞掉 —— 「节点互相帮转发」
    /// 在最需要它的场景无声失效。界面上**看不出任何异常**（本地收发一切正常），
    /// 只有跨网段压测才暴露，正是只能靠护栏钉死的那类退化。
    #[test]
    fn gossip_fanout_targets_reachable_links() {
        let transport = crate::network::transport_src_for_guards();
        assert!(
            !transport.contains("choose_fanout(&peers"),
            "转发候选不得再来自 peers（知识集）—— 用 reachable_neighbors（links 中有非空链路的邻居）"
        );
        // 定义 1 处 + 调用 3 处（gossip 广播分支 / 定向洪泛兜底 / OpaqueExternal）
        let uses = transport.matches("reachable_neighbors(").count();
        assert!(
            uses >= 4,
            "reachable_neighbors 应有定义+3 个转发调用点，实际 {uses} 处 —— 有新转发点没走可达集？"
        );
    }

    /// 群发送内核必须**在写库成功之后、且不持 DB 锁**时把这条消息回送给本机窗口（#82）。
    ///
    /// 为什么要这条：群任务看板跑在独立窗口里、有自己的 store 实例，所以 `createTodo`
    /// 的乐观插入只落在任务窗那份 `messages` 上，主聊天窗完全不知情 ⇒ 用户自己发的任务
    /// 在自己的时间线上看不见（重进会话才看见）。同类的群发送（群文件/公告
    /// `commands/group_announcements.rs`）早就在写库后自 emit，缺的正是内核这一句。
    ///
    /// 判据刻意取**顺序 + 作用域**，不是"有没有那串字面量"：
    /// - emit 早于 `tx.commit()` ⇒ 会 emit 一条没落库的消息（本仓不变量明令禁止）；
    /// - emit 落在 `s.db.lock()` 的作用域里 ⇒ 持锁 emit（同样是既有不变量）。
    /// 这两条任一被破坏，下面的相对位置断言都会红。
    #[test]
    fn group_send_kernel_emits_to_own_windows_after_commit_outside_the_lock() {
        let commands = all_commands_src();
        let body = rust_fn_body(&commands, "fn send_group_payload(");
        let lock = body
            .find("s.db.lock()")
            .expect("群发送内核应当有一段持锁写库，找不到 `s.db.lock()` 说明形状变了");
        let commit = body
            .find("tx.commit()")
            .expect("落库与入队必须在同一个事务里（`tx.commit()`）");
        assert!(lock < commit, "持锁段必须包住 commit，否则写入不是原子的");
        let emit = body
            .find(r#"emit("message-received""#)
            .expect("群发送内核没有把这条消息回送给本机窗口：别的窗口（群任务看板）自己发的东西，\
                     主聊天窗收不到 ⇒ 用户看不见自己刚发的任务卡片。\n\
                     对照 `commands/group_announcements.rs` 的同形写法：写库成功后 `let _ = s.app.emit(\"message-received\", &rec);`");
        assert!(
            commit < emit,
            "emit 必须晚于 commit：先通知再落库，前端会渲染出一条其实没存的记录"
        );
        assert!(
            body[commit..emit].contains("\n    }"),
            "emit 不能落在 `s.db.lock()` 的作用域内（持锁 emit 是既有不变量）—— 它必须在锁作用域闭合之后"
        );
    }
