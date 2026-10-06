// 职责边界：
// - transport 行为测试分册之14 —— 已读回执：只前进不后退、写失败时待发队列不丢最大时间戳
// 为什么拆：`transport/tests.rs` 原来 3,227 行、8 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// ReadReceipt 正常到达：ts ≤ last_read_ts 的消息推进到 read。
    #[test]
    fn read_receipt_marks_messages_as_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "peer-a".into(),
                sender_id: "me".into(),
                receiver_id: "peer-a".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "delivered".into(),
                mention_targets: None,
            },
        )
        .unwrap();
        // 模拟 ReadReceipt handler 的 UPDATE 语句
        let updated = conn
            .execute(
                "UPDATE messages SET status = 'read'
             WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                params!["peer-a", "me", 100],
            )
            .unwrap();
        assert_eq!(updated, 1, "应有 1 行被更新");
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "read");
    }

    /// Test D: updated = 0 时 DB UPDATE 无行被更新，但 handler 仍然 emit peer-read。
    /// 注意：emit 依赖 Tauri AppHandle，无法在纯单测中断言事件；
    /// 这里验证的是 SQL 路径正确返回 updated=0（与 emit 条件分离）。
    #[test]
    fn read_receipt_db_update_zero_when_already_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "peer-a".into(),
                sender_id: "me".into(),
                receiver_id: "peer-a".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "read".into(),
                mention_targets: None,
            },
        )
        .unwrap();
        let updated = conn
            .execute(
                "UPDATE messages SET status = 'read'
             WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                params!["peer-a", "me", 100],
            )
            .unwrap();
        assert_eq!(updated, 0, "DB 已是 read，无行被更新");
        // handler 仍然 emit peer-read（always-emit 修复），但 emit 本身无法在单测中断言
    }

    /// Test C: 多次 mark_read 只保留最大 timestamp。
    #[test]
    fn pending_reads_keeps_max_timestamp() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // mark_read(ts=100)
        let cur = pending.entry("peer-a".into()).or_insert(100);
        *cur = (*cur).max(100);
        assert_eq!(pending["peer-a"], 100);
        // mark_read(ts=80) — 较小，不更新
        let cur = pending.entry("peer-a".into()).or_insert(80);
        *cur = (*cur).max(80);
        assert_eq!(pending["peer-a"], 100);
        // mark_read(ts=200) — 较大，更新
        let cur = pending.entry("peer-a".into()).or_insert(200);
        *cur = (*cur).max(200);
        assert_eq!(pending["peer-a"], 200);
    }

    /// Test B: writer write_frame 失败时，ReadReceipt 的 timestamp 被重新放入 pending_reads。
    /// 模拟 writer_loop 的失败回收逻辑。
    #[test]
    fn writer_failure_preserves_pending_for_read_receipt() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // flush_pending_reads 已经 remove
        pending.insert("peer-a".into(), 200);
        let last_read_ts = pending.remove("peer-a").unwrap();
        assert!(pending.is_empty(), "flush 后 pending 应为空");
        // 模拟 writer_loop write_frame 失败后的回收逻辑
        {
            let cur = pending.entry("peer-a".into()).or_insert(last_read_ts);
            *cur = (*cur).max(last_read_ts);
        }
        assert_eq!(
            pending.get("peer-a"),
            Some(&200),
            "写入失败后 pending 应恢复"
        );
    }

    /// Test A: writer write_frame 成功时，pending 不被重新插入。
    /// flush_pending_reads remove 后发送成功，pending 保持清空。
    #[test]
    fn successful_flush_clears_pending() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        pending.insert("peer-a".into(), 200);
        // 模拟 flush_pending_reads: remove + try_send Ok + writer write_frame Ok
        let last_read_ts = pending.remove("peer-a").unwrap();
        // write_frame 成功 → 不执行 writer_loop 的回收逻辑
        let _ = last_read_ts;
        assert!(pending.is_empty(), "写入成功后 pending 应保持清空");
    }

    /// flush_pending_reads try_send 失败时（链路不存在），pending 必须恢复——
    //  否则 ReadReceipt 永久丢失，要等用户下次手动打开会话才能补发。
    #[test]
    fn flush_failure_reinserts_pending_read() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        pending.insert("peer-a".into(), 300);
        // 模拟 flush_pending_reads: remove → try_send Err → 必须 re-insert
        let last_read_ts = pending.remove("peer-a").unwrap();
        assert!(pending.is_empty(), "remove 后 pending 应为空");
        // try_send 失败 → 重新放入 pending（与 writer_loop 的失败回收同逻辑）
        {
            let cur = pending.entry("peer-a".into()).or_insert(last_read_ts);
            *cur = (*cur).max(last_read_ts);
        }
        assert_eq!(
            pending.get("peer-a"),
            Some(&300),
            "try_send 失败后 pending 应恢复"
        );
    }

    /// 多次 flush 失败只保留最大 timestamp（幂等性）。
    #[test]
    fn repeated_flush_failure_keeps_max_timestamp() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // 第一次 mark_read(ts=300) → flush 失败
        pending.insert("peer-a".into(), 300);
        let ts1 = pending.remove("peer-a").unwrap();
        {
            let cur = pending.entry("peer-a".into()).or_insert(ts1);
            *cur = (*cur).max(ts1);
        }
        // 第二次 mark_read(ts=200) → 较小，不覆盖
        let cur = pending.entry("peer-a".into()).or_insert(200);
        *cur = (*cur).max(200);
        assert_eq!(pending["peer-a"], 300);
        // 第三次 mark_read(ts=500) → 较大，更新
        let cur = pending.entry("peer-a".into()).or_insert(500);
        *cur = (*cur).max(500);
        assert_eq!(pending["peer-a"], 500);
    }

    /// read 不会被 delivered 回退（set_message_status 守卫）。
    #[test]
    fn read_status_never_regresses_to_delivered() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "f1".into(),
                sender_id: "a".into(),
                receiver_id: "b".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "read".into(),
                mention_targets: None,
            },
        )
        .unwrap();
        db::set_message_status(&conn, "m1", "delivered").unwrap();
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "read", "delivered 不得回退 read");
    }

    // ================================================================
    // Ack 中继转发测试
    // ================================================================
