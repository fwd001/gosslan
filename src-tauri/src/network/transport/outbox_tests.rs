// 职责边界：
// - transport 行为测试分册之13 —— 离线队列与过期终态：过期文件不许改写已完成传输、Ack 点亮送达并按 msg_id 删行
// 为什么拆：`transport/tests.rs` 原来 3,227 行、10 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// 审计 A3：过期消息落终态必须**两步写库都成功**才返回 true（emit 据此门控），且真的把
    /// status 置 failed、把 outbox 行删掉；任一步失败必须返回 false（绝不让 emit 谎报"失败"
    /// 而 outbox 行还在 ⇒ 下次 flush 重发 = 重复投递）。
    #[test]
    fn finalize_expired_message_writes_both_and_gates_on_failure() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        let rec = MessageRecord {
            id: 0,
            msg_id: "m1".into(),
            conv_id: "peer".into(),
            sender_id: "me".into(),
            receiver_id: "peer".into(),
            kind: "text".into(),
            content: "hi".into(),
            ts: 1,
            seq: 1,
            status: "sending".into(),
            mention_targets: None,
        };
        assert!(db::insert_message_if_new(&conn, &rec).unwrap());
        conn.execute(
            "INSERT INTO outbox(msg_id, peer_id, payload, created_at) VALUES('m1','peer','{}',0)",
            [],
        )
        .unwrap();

        // 绿路：两步都成功 ⇒ true，且 status=failed、outbox 行已删
        assert!(finalize_expired_message(&conn, "m1", false));
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id='m1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "failed");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM outbox WHERE msg_id='m1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            n, 0,
            "outbox 行必须被删掉（否则下次 flush 重发 = 重复投递）"
        );

        // 失败路：删掉 outbox 表 ⇒ 第二步写库失败 ⇒ 必须返回 false（emit 因此被门控住，不谎报失败）
        conn.execute_batch("DROP TABLE outbox").unwrap();
        assert!(
            !finalize_expired_message(&conn, "m2", false),
            "任一步写库失败必须返回 false —— 否则 emit 谎报失败而 outbox 行还在 ⇒ 重复投递"
        );
    }

    /// 审计 A3 的**自身缺陷**（2026-09-23 review 发现，非清单条目）：
    /// `finalize_expired_file` 四步里只有 `mark_file_outbox_failed` 会把行踢出重试集合
    /// （`list_expired_file_outbox` 只选 pending/sending）。它原先排**第一** ⇒ 后面任一步失败时
    /// "返回 false、下一 tick 重试"是假承诺：行已是 failed，再也扫不到，界面永久停在"发送中"。
    /// 判据 = 破坏性写必须排最后，且失败时行仍要能被扫到。
    #[test]
    fn finalize_expired_file_failure_leaves_the_row_retryable() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        let deadline = db::now_ms() + 10_000;

        // 绿路：四步全成功 ⇒ true，outbox 行落 failed（此后不再进重试集合，这是**成功**的表现）
        db::insert_file_outbox(&conn, "t1", "peer", None, "/tmp/x", "x.bin", 10).unwrap();
        db::mark_file_outbox_sending(&conn, "t1", 0).unwrap();
        assert!(finalize_expired_file(&conn, "t1"));
        assert_eq!(
            file_outbox_status(&conn, "t1").as_deref(),
            Some("failed"),
            "成功落终态后行必须标 failed"
        );
        assert!(
            !expired_ids(&conn, deadline).contains(&"t1".to_string()),
            "已落终态的传输不该再被扫到"
        );

        // 失败路：messages 表不可用 ⇒ 第一步就失败 ⇒ 返回 false（emit 被门控），
        // 且这条行必须**仍在**重试集合里，否则"下一 tick 重试"就是假话。
        conn.execute_batch("DROP TABLE messages").unwrap();
        db::insert_file_outbox(&conn, "t2", "peer", None, "/tmp/y", "y.bin", 10).unwrap();
        db::mark_file_outbox_sending(&conn, "t2", 0).unwrap();
        assert!(
            !finalize_expired_file(&conn, "t2"),
            "任一步写库失败必须返回 false"
        );
        assert_eq!(
            file_outbox_status(&conn, "t2").as_deref(),
            Some("sending"),
            "写库没成功时不得提前把行标 failed —— 否则它永远退不出重试集合"
        );
        assert!(
            expired_ids(&conn, deadline).contains(&"t2".to_string()),
            "失败的终态必须还能被下一轮清扫扫到（破坏性写要排最后）"
        );
    }

    fn msg_status(conn: &rusqlite::Connection, msg_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT status FROM messages WHERE msg_id = ?1",
            rusqlite::params![msg_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    fn ins_bubble(conn: &rusqlite::Connection, msg_id: &str, status: &str) {
        conn.execute(
            "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status)
             VALUES(?1, 'c1', 'me', 'peer', 'file', 'x', 1000, 1, ?2)",
            rusqlite::params![msg_id, status],
        )
        .unwrap();
    }

    /// INV-P26 的另一半：**已 `done` 的传输不是失败**，超时清扫也不例外。
    ///
    /// `fail_file_job` 上有这道闸门（台账 done ⇒ 面向用户的写与 emit 全部跳过），
    /// 但同一件事的**另一份实现**（清扫器这条）没有 —— 三份收尾各写一遍的后果就是
    /// 闸门只装在其中两扇门上。这条测试钉住第三个门：气泡已经是 `delivered`、
    /// 台账已经是 `done`（文件真在盘上）时，清扫器不得把它改写成失败，
    /// 而且必须回报 `false`（调用方据此**不 emit**，否则用户凭空收到一条失败提示）。
    /// 队列行仍然要关掉：它不留活口，否则每 tick 重扫一遍又什么都不做。
    #[test]
    fn expired_file_never_rewrites_a_completed_transfer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();

        ins_bubble(&conn, "file-t3", "delivered");
        db::upsert_transfer(
            &conn,
            "t3",
            "peer",
            "x.bin",
            10,
            "send",
            "done",
            Some("/d/x.bin"),
            1.0,
        )
        .unwrap();
        db::insert_file_outbox(&conn, "t3", "peer", None, "/d/x.bin", "x.bin", 10).unwrap();
        db::mark_file_outbox_sending(&conn, "t3", 0).unwrap();

        assert!(
            !finalize_expired_file(&conn, "t3"),
            "台账已 done ⇒ 这一单不是失败，必须回报 false（调用方据此不 emit file-failed）"
        );
        assert_eq!(
            msg_status(&conn, "file-t3").as_deref(),
            Some("delivered"),
            "已送达的气泡不许被清扫器改写成失败"
        );
        assert_eq!(
            conn.query_row(
                "SELECT status FROM file_transfers WHERE id = 't3'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "done",
            "INV-P26：done 不可降级"
        );
        assert_eq!(
            file_outbox_status(&conn, "t3").as_deref(),
            Some("failed"),
            "队列行仍要关掉 —— 它是队列自己的状态机，不留活口"
        );
    }

    /// 群气泡**不归这条路径管**：`file_outbox` 今天只服务单聊（群文件的逐人投递台账在
    /// `group_file_recipients`，那里每人一行、状态各自推进）。而 `finalize_expired_file`
    /// 里那句 `gfile-{transfer_id}` 写的是**共享气泡** —— 一旦哪天群文件也进了这个队列，
    /// "一个收件人超时"就会把整条群消息标成失败，别人其实收到了。
    ///
    /// ⚠️ 诚实交代（本条写出来之前我先错过一次）：这句今天**撞不到** —— 生产代码里唯一写
    /// `file_outbox` 的地方 `group_id` 恒为 `None`，所以它永远命中 0 行；而就算有行，
    /// `set_message_status` 也拒绝把 `delivered`/`read` 改回失败 ⇒ 只有还在途中的气泡会被改写。
    /// 那为什么还要删："**靠暂时没人这么写才不出事**"的代码，正是这个仓一直在出事的那类
    /// （`file_outbox` 的 schema 里就有 `group_id` 这一列，哪天群文件进队列，它立刻变成
    /// "一个收件人超时 ⇒ 整条群消息显示失败，而别人其实收到了"）。判据把边界钉住，
    /// 合并三份收尾时那句一并删。
    #[test]
    fn expired_file_does_not_touch_the_group_bubble() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();

        // 群文件气泡在投递途中就是 `sending`（`delivered` 是全员收齐之后）。
        // 选这个状态是刻意的：`set_message_status` 本身拒绝把 delivered/read 改回失败，
        // 只有"还在途中"的气泡会被这句隔空改写 —— 所以它是**潜伏**而不是活跃缺陷。
        ins_bubble(&conn, "gfile-t4", "sending");
        db::insert_file_outbox(&conn, "t4", "peer", None, "/d/y.bin", "y.bin", 10).unwrap();
        db::mark_file_outbox_sending(&conn, "t4", 0).unwrap();

        assert!(
            finalize_expired_file(&conn, "t4"),
            "单聊这一路的四步写应当照常成功"
        );
        assert_eq!(
            msg_status(&conn, "gfile-t4").as_deref(),
            Some("sending"),
            "群文件的气泡状态由 `group_file_recipients` 那条路决定，清扫器不许隔空改写"
        );
    }

    fn file_outbox_status(conn: &rusqlite::Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT status FROM file_outbox WHERE transfer_id = ?1",
            rusqlite::params![id],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    fn expired_ids(conn: &rusqlite::Connection, deadline: i64) -> Vec<String> {
        db::list_expired_file_outbox(conn, deadline)
            .unwrap()
            .into_iter()
            .map(|(id, _, _, _)| id)
            .collect()
    }

    /// Test 1：本机是原始发送者 → Ack 正常处理（status→delivered, outbox 删除）。
    #[test]
    fn ack_local_sender_marks_delivered_and_clears_outbox() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 模拟本机发送的消息
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "d1".into(),
                sender_id: "me".into(),
                receiver_id: "d1".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "sent".into(),
                mention_targets: None,
            },
        )
        .unwrap();
        db::insert_outbox(&conn, "m1", "d1", r#"payload"#).unwrap();

        // Ack handler 的查询：sender_id = "me" == 本机 → 走正常处理分支
        let sender_id: String = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["m1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sender_id, "me");

        // 模拟正常处理
        db::set_message_status(&conn, "m1", "delivered").unwrap();
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", params!["m1"])
            .unwrap();

        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "delivered");
        assert!(db::list_outbox(&conn, "d1").unwrap().is_empty());
    }

    /// Test 3：Ack 对应的 msg_id 不存在 → 查询返回 None → 安全丢弃。
    #[test]
    fn ack_unknown_msg_id_is_silently_dropped() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 不存在的消息
        let result: Option<String> = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["nonexistent"],
                |r| r.get(0),
            )
            .ok();
        assert!(result.is_none(), "查询不存在的 msg_id 应返回 None");
        // 此时 handler 走 None 分支 → 不做任何修改
    }
