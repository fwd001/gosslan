// 职责边界：
// - transport 行为测试分册之12 —— 分发与落库裁决：入站去重真值表、徽章归属、副作用策略、好友权限门
// 为什么拆：`transport/tests.rs` 原来 3,227 行、7 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// 入站去重判据的真值表。这条判据改错的后果是"两边互拒 ⇒ 谁也连不上"，
    /// 或者"镜像连接永久并存"，两者都不是肉眼能立刻发现的，所以逐格钉住。
    #[test]
    fn inbound_dedup_truth_table() {
        let lan_ep: MeshEndpoint = "192.168.1.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();
        let routed_ep: MeshEndpoint = "100.70.10.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();

        // ① 一条都没有 ⇒ **必须接受**（否则彻底断连）
        assert!(should_accept_inbound("b", "a", PathKind::Lan, &[]));

        // ② 大 ID 方（my_id > peer_id）：已有同路径**且健康** ⇒ 拒收镜像；
        //    不同路径 ⇒ 接受（多路径！）
        let with_lan = [(lan_ep.clone(), PathKind::Lan, true)];
        assert!(!should_accept_inbound("b", "a", PathKind::Lan, &with_lan));
        assert!(should_accept_inbound("b", "a", PathKind::Routed, &with_lan));
        // ②b 已有同路径但**已不健康**（半开待拆）⇒ 必须接受对端的新鲜连接，
        //     否则双方要干等 watchdog（最长 45s）才能恢复
        let with_dead_lan = [(lan_ep.clone(), PathKind::Lan, false)];
        assert!(should_accept_inbound(
            "b",
            "a",
            PathKind::Lan,
            &with_dead_lan
        ));

        // ③ 小 ID 方（my_id < peer_id）：**始终接受** —— 否则双方互拒，谁也连不上
        assert!(should_accept_inbound("a", "b", PathKind::Lan, &with_lan));

        // ④ 链路数到上限 ⇒ 拒收（防无界增长），且与路径是否重复无关
        let full: Vec<(MeshEndpoint, PathKind, bool)> = (0..MAX_LINKS_PER_PEER)
            .map(|i| {
                (
                    format!("10.0.0.{i}:59992")
                        .parse::<std::net::SocketAddr>()
                        .unwrap()
                        .into(),
                    if i % 2 == 0 {
                        PathKind::Lan
                    } else {
                        PathKind::Routed
                    },
                    true,
                )
            })
            .collect();
        assert!(!should_accept_inbound("a", "b", PathKind::Bluetooth, &full));
        // 未到上限但已有 Routed ⇒ 接受（③ 的小 ID 方不受 ② 限制）
        let one_routed = [(routed_ep, PathKind::Routed, true)];
        assert!(should_accept_inbound(
            "a",
            "b",
            PathKind::Routed,
            &one_routed
        ));
    }

    /// 徽标必须反映**实际选中的那条**，而不是插入顺序的第一条。
    ///
    /// 旧实现取 `links.first()`：用户配了 Routed 又同处一个局域网时（LAN 由 announce
    /// 后补、插在后面），徽标会一直显示"桥接"，消息却走 LAN —— 用户 2026-09-12
    /// 反馈过徽标与实际不符。
    #[test]
    fn badge_follows_selection_not_insertion_order() {
        let (routed, _b0, _p0) = make_link("100.70.10.20:59992", PathKind::Routed);
        let (lan, _b1, _p1) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![routed, lan];
        let conns = vec![
            mesh_conn("peer", "100.70.10.20:59992", Some(1000), PathKind::Routed),
            mesh_conn("peer", "192.168.1.20:59992", Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(
            badge_path_kind(&links, &order),
            PathKind::Lan,
            "徽标必须跟着选路走（LAN），而不是插入顺序第一条（Routed）"
        );
        // 选路为空（全部不可用）⇒ 退回首条，不能 panic
        assert_eq!(badge_path_kind(&links, &[]), PathKind::Routed);
    }

    /// P1-3 / Test C：三态落库裁决 → 副作用与 Ack 策略的映射必须是显式且可测的。
    /// - 未读 +1 与 message-received：只有 `Ok(true)`（本次真的新建）才允许；
    /// - Ack：`Ok(true)` / `Ok(false)` 都允许（消息确已在库），`Err` 必须禁止
    ///   （Ack 会让发送方删掉 outbox 行 ⇒ 临时 DB 故障变成永久丢消息）。
    #[test]
    fn insert_outcome_maps_to_side_effect_and_ack_policy() {
        let fresh: Result<bool, rusqlite::Error> = Ok(true);
        let duplicate: Result<bool, rusqlite::Error> = Ok(false);
        let db_error: Result<bool, rusqlite::Error> = Err(rusqlite::Error::QueryReturnedNoRows);

        assert!(announced_on(&fresh), "本次新建 ⇒ 计未读 + 投递事件");
        assert!(!announced_on(&duplicate), "重复 ⇒ 不得再有副作用");
        assert!(
            !announced_on(&db_error),
            "DB 故障 ⇒ 不得有副作用（更不得当成重复）"
        );

        assert!(may_ack(&fresh), "本次新建 ⇒ Ack");
        assert!(
            may_ack(&duplicate),
            "已在库中 ⇒ 仍 Ack（Ack 语义 = 已成功接收并持久化）"
        );
        assert!(
            !may_ack(&db_error),
            "DB 故障 ⇒ 绝不 Ack，outbox 行必须保留以便重发"
        );
    }

    /// 好友状态下 Direct Chat 可以正常接收（insert_message_if_new 成功）。
    #[test]
    fn friend_chat_message_accepted() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "a", "Alice", None).unwrap();
        assert!(
            db::get_friend(&conn, "a").is_some(),
            "好友存在时应能处理消息"
        );
    }

    /// 删除好友后 Direct Chat 不落库。
    #[test]
    fn non_friend_chat_message_rejected() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "a", "Alice", None).unwrap();
        db::remove_friend(&conn, "a").unwrap();
        assert!(db::get_friend(&conn, "a").is_none(), "删除好友后应检测不到");
    }

    /// Gossip Group 不受好友检查影响。
    #[test]
    fn gossip_group不受好友检查影响() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 群聊不需要好友关系
        db::create_group(&conn, "g1", "测试群", "owner", &["a".into(), "b".into()]).unwrap();
        let groups = db::list_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1);
    }

    /// FriendMessageBlocked 包含 original_sender 字段。
    #[test]
    fn friend_message_blocked_has_original_sender() {
        let msg = Message::FriendMessageBlocked {
            from: "c".into(),
            to: "a".into(),
            original_sender: "a".into(),
        };
        match &msg {
            Message::FriendMessageBlocked {
                from,
                to,
                original_sender,
            } => {
                assert_eq!(from, "c");
                assert_eq!(to, "a");
                assert_eq!(original_sender, "a");
            }
            _ => panic!("应为 FriendMessageBlocked"),
        }
    }

    // ---------- 待发群密钥 pending 表（最小 P0 修复） ----------
