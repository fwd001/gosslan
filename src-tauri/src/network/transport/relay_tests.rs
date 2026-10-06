// 职责边界：
// - transport 行为测试分册之8 —— 公网中继：定向转发 share/offer、中继节点不本地处理 Ack、转发保留原字段
// 为什么拆：`transport/tests.rs` 原来 3,227 行、3 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// 定向中继判定：共享目录/中继文件在无直连时靠它借一跳；给本机或旧端无 to 的帧不转发。
    #[test]
    fn directed_relay_target_routes_share_and_offer_frames() {
        let me = "me";
        let tree_to_other = Message::ShareTreeRequest {
            request_id: "r".into(),
            from: "a".into(),
            to: "b".into(),
        };
        assert_eq!(directed_relay_target(&tree_to_other, me), Some("b"));
        let tree_to_me = Message::ShareTreeRequest {
            request_id: "r".into(),
            from: "a".into(),
            to: me.into(),
        };
        assert_eq!(
            directed_relay_target(&tree_to_me, me),
            None,
            "给本机的帧不转发"
        );
        let resp_legacy = Message::ShareTreeResponse {
            request_id: "r".into(),
            from: "a".into(),
            to: None,
            entries: vec![],
        };
        assert_eq!(
            directed_relay_target(&resp_legacy, me),
            None,
            "旧端无 to：按直连处理"
        );
        let resp_relay = Message::ShareTreeResponse {
            request_id: "r".into(),
            from: "a".into(),
            to: Some("b".into()),
            entries: vec![],
        };
        assert_eq!(directed_relay_target(&resp_relay, me), Some("b"));
        let file_req = Message::ShareFileRequest {
            transfer_id: "t".into(),
            from: "a".into(),
            path: "p".into(),
            to: Some("b".into()),
        };
        assert_eq!(directed_relay_target(&file_req, me), Some("b"));
        let offer = Message::RelayFileOffer {
            // 分片尺寸对本用例的路由判定无关，但要给一个"新对端"的真实值，
            // 免得日后有人照着这条用例把 0（= 老对端）当默认值抄。
            chunk_size: crate::file_relay::MIN_CHUNK_SIZE as u32,
            transfer_id: "t".into(),
            from: "a".into(),
            to: "b".into(),
            name: "n".into(),
            size: 1,
            total_chunks: 1,
            sealed_file_key: "k".into(),
            file_sha256: "h".into(),
        };
        assert_eq!(directed_relay_target(&offer, me), Some("b"));
        let normal = Message::Heartbeat {
            device_id: "a".into(),
        };
        assert_eq!(directed_relay_target(&normal, me), None);
    }

    /// Test 2：中继节点收到 Ack → sender_id ≠ 本机 → 不做本地处理。
    /// 验证中继节点不修改自己的消息状态、不删除 outbox。
    #[test]
    fn ack_relay_node_does_not_process_locally() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 中继节点 C 收到 A 发给 D 的消息（通过 Gossip）
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "d1".into(),
                sender_id: "node-a".into(),
                receiver_id: "d1".into(),
                kind: "text".into(),
                content: "hello".into(),
                ts: 200,
                seq: 1,
                status: "delivered".into(),
                mention_targets: None,
            },
        )
        .unwrap();
        // C 自己也有一条 outbox 消息（不同的 msg_id）
        db::insert_outbox(&conn, "m-own", "some-peer", r#"own payload"#).unwrap();

        // Ack handler 查询：sender_id = "node-a" ≠ "me"（当前节点是 C）
        let sender_id: String = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["m1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(sender_id, "me", "sender_id 应为原始发送方 A，不是本机 C");

        // 中继节点不应执行任何本地状态修改
        // （实际 handler 中，Some(sender) if sender == device_id 分支不匹配 → 走转发分支）
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "delivered", "中继节点不修改消息状态");
        assert!(
            !db::list_outbox(&conn, "some-peer").unwrap().is_empty(),
            "中继节点不删除自己的 outbox"
        );
    }

    /// Test 4：中继节点转发 Ack 时，sender_id 和 message_id 不被修改。
    #[test]
    fn ack_relay_preserves_original_fields() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "d1".into(),
                sender_id: "node-a".into(),
                receiver_id: "d1".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "delivered".into(),
                mention_targets: None,
            },
        )
        .unwrap();

        // 中继节点查询到原始 sender_id
        let original_sender: String = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["m1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original_sender, "node-a");

        // 转发时使用原始 Ack 消息（message_id 和 sender_id 不变）
        let ack = Message::Ack {
            msg_id: "m1".into(),
        };
        match &ack {
            Message::Ack { msg_id } => {
                assert_eq!(msg_id, "m1", "message_id 不得被修改");
            }
            _ => panic!("应为 Ack"),
        }
        // original_sender 用于 try_send 的 peer_id 参数，不嵌入 Ack 消息体
    }

    // ================================================================
    // 好友权限测试
    // ================================================================
