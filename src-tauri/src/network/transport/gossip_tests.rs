// 职责边界：
// - transport 行为测试分册之9 —— Gossip 消费判据与传播层：非成员也转发、大块分级、传播层不依赖本地落库
// 为什么拆：`transport/tests.rs` 原来 3,227 行、7 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// 这条测试只钉"能不能消费"；"非成员仍要转发"由下面那条 + `handle_gossip` 的结构保证。
    #[test]
    fn group_envelope_consumption_rule() {
        let members = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let group = members(&["me", "other"]);
        let me = "me";
        let other = "other";

        // 是群成员、且发送者也在成员表里 ⇒ 可以消费
        assert!(group_envelope_consumable(
            &GossipKind::Group,
            &group,
            me,
            other
        ));
        // 我不是成员 ⇒ 不消费（但**不影响转发** —— 见 group_envelope_consumable 的注释）
        assert!(!group_envelope_consumable(
            &GossipKind::Group,
            &group,
            "stranger",
            other
        ));
        // 发送者自称不在成员表里（伪造者想把群消息广播给别人）⇒ 不消费
        assert!(!group_envelope_consumable(
            &GossipKind::Group,
            &group,
            me,
            "outsider"
        ));
        // 成员表为空 = 旧端发的群信封 ⇒ 保持兼容，允许消费
        assert!(group_envelope_consumable(
            &GossipKind::Group,
            &[],
            me,
            other
        ));
        // 非群种类一律不受这条判据影响
        assert!(group_envelope_consumable(
            &GossipKind::Presence,
            &group,
            "stranger",
            other
        ));
        assert!(group_envelope_consumable(
            &GossipKind::ChatAck,
            &group,
            "stranger",
            other
        ));
    }

    /// **非成员中继必须转发群消息**（结构护栏）：`handle_gossip` 里那句早期 `return` 一旦
    /// 被加回来，BLE-only 多跳的群聊就又断了 —— 而它**不会让任何测试失败**，
    /// 只会让真机上的群聊静默不通。所以用源码断言把"只记判据、不 return"钉住。
    #[test]
    fn handle_gossip_does_not_bail_out_for_non_members() {
        let src = crate::network::transport_src_for_guards();
        // 锚点必须"行首 + 带左括号"：聚合文本里本测试自己那句字面量 `"async fn handle_gossip("`
        // 也算一次命中，不带行首锚点会先匹配到它，然后 body 里含下面那条 forbidden 字面量
        // —— 守卫自证其罪（分册之后主文件排在前面，这个陷阱才暴露出来）。
        let start = src
            .find("\nasync fn handle_gossip(")
            .map(|i| i + 1)
            .expect("必须还有 handle_gossip");
        let body = &src[start..];
        let end = body.find("\n}\n").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains("group_envelope_consumable("),
            "handle_gossip 必须用 group_envelope_consumable 判据"
        );
        assert!(
            !body.contains(
                "if matches!(env.kind, GossipKind::Group) && !env.group_members.is_empty()"
            ),
            "不能恢复『非成员直接 return』的旧写法 —— 那会让非成员中继不再转发群消息，\
             多跳（BLE-only 手机↔电脑↔手机）群聊永远不通"
        );
    }

    /// 造一个只关心 payload 长度/类型的 Gossip 信封（其余字段对优先级分类无意义）。
    fn test_envelope(payload_len: usize, kind: GossipKind) -> GossipEnvelope {
        GossipEnvelope {
            message_id: "m".into(),
            sender_id: "a".into(),
            nonce: "n".into(),
            sender_pubkey: "pk".into(),
            sender_ed25519: "ek".into(),
            sender_sig: "sig".into(),
            ttl: 4,
            kind,
            group_id: None,
            group_name: None,
            group_creator: None,
            group_members: vec![],
            payload: "p".repeat(payload_len),
            ts: 1,
            seq: 0,
            encrypted: false,
            target: None,
        }
    }

    #[test]
    fn bulk_messages_are_only_large_chunks() {
        // 旧 is_bulk_message 的判据已迁移到 `dispatch::message_priority`（单一事实来源）。
        // 本测试保住的是**同一张真值表**：bulk ⇔ Low，且边界值不变。
        use crate::network::dispatch::{message_priority, MessagePriority};
        let is_bulk = |m: &Message| message_priority(m) == MessagePriority::Low;
        let chat = Message::ChatMessage {
            msg_id: "m1".into(),
            from: "a".into(),
            to: "b".into(),
            kind: "text".into(),
            content: "hi".into(),
            ts: 1,
            seq: 1,
        };
        assert!(!is_bulk(&chat));
        let file_chunk = Message::FileChunk {
            transfer_id: "t1".into(),
            seq: 0,
            data: "abc".into(),
            attempt: None,
        };
        assert!(is_bulk(&file_chunk));
        let group_file_chunk = Message::GroupFileChunk {
            transfer_id: "t1".into(),
            group_id: "g1".into(),
            sender_id: "a".into(),
            seq: 0,
            data: "abc".into(),
        };
        assert!(is_bulk(&group_file_chunk));
        // 文件终止帧必须走 bulk，避免跑到未写完的分片前面。
        let file_done = Message::FileDone {
            transfer_id: "t1".into(),
            attempt: None,
        };
        assert!(is_bulk(&file_done));
        let group_file_done = Message::GroupFileDone {
            transfer_id: "t1".into(),
            group_id: "g1".into(),
            sender_id: "a".into(),
        };
        assert!(is_bulk(&group_file_done));

        // 小头像资料帧：资料变更要立刻可见 ⇒ 仍走优先道。
        let small_user_info = Message::UserInfo {
            device_id: "a".into(),
            nickname: "A".into(),
            avatar: Some("x".repeat(CONTROL_AVATAR_MAX_BYTES)),
            device_type: "desktop".into(),
        };
        assert!(!is_bulk(&small_user_info), "恰好等于上限的头像仍应走优先道");

        // 大头像资料帧：内容大、可晚到 ⇒ 必须降级到 bulk，绝不占聊天/好友的优先道。
        let big_user_info = Message::UserInfo {
            device_id: "a".into(),
            nickname: "A".into(),
            avatar: Some("x".repeat(CONTROL_AVATAR_MAX_BYTES + 1)),
            device_type: "desktop".into(),
        };
        assert!(
            is_bulk(&big_user_info),
            "超过上限的头像资料帧必须走 bulk（否则会堵住聊天与好友请求）"
        );

        // 小载荷 Gossip 走优先道；内联大载荷（例如带大图的 Presence）降级到 bulk。
        let small_gossip = Message::Gossip {
            envelope: test_envelope(BULK_GOSSIP_PAYLOAD_MAX_BYTES, GossipKind::Presence),
        };
        assert!(!is_bulk(&small_gossip));
        let big_gossip = Message::Gossip {
            envelope: test_envelope(BULK_GOSSIP_PAYLOAD_MAX_BYTES + 1, GossipKind::Presence),
        };
        assert!(is_bulk(&big_gossip));
    }

    #[tokio::test]
    async fn e2e_gossip_encrypt_sign_decrypt() {
        // 端到端：A 加密→签名→广播信封，B 验签→解密
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();

        // A 用 B 的公钥 ECDH 派生共享密钥并加密
        let shared =
            crate::crypto::shared_secret(&a.x25519_secret, &b.x25519_public_b64()).unwrap();
        let plaintext = b"{\"kind\":\"text\",\"content\":\"hello\"}";
        let sealed = crate::crypto::seal(&shared, plaintext).unwrap();
        let payload_b64 = STANDARD.encode(&sealed);

        // 构造并签名信封
        let engine = GossipEngine::new(100, 10, 4, 6);
        let env = engine.build_envelope(
            &a,
            "dev-a",
            GossipKind::Chat,
            None,
            None,
            &payload_b64,
            1,
            1,
        );

        // B 验签 + 解密
        assert!(engine.verify_envelope(&env));
        let shared_b = crate::crypto::shared_secret(&b.x25519_secret, &env.sender_pubkey).unwrap();
        let decrypted = STANDARD.decode(&env.payload).unwrap();
        let opened = crate::crypto::open(&shared_b, &decrypted).unwrap();
        assert_eq!(opened, plaintext);
    }

    // ---------------- P0-2：直连 E2EE 解密失败不得消费真实 msg_id ----------------

    /// P1-3：两层去重必须互相独立——Gossip 的 Bloom/LRU 属网络传播层（只认 `message_id`），
    /// SQLite 的 `msg_id` 属业务持久化层。业务层的「本机已落库」只能抑制未读/事件，
    /// 绝不能前移到转发之前，否则已经 Direct 收到过该消息的节点会拒绝继续 fan-out，
    /// epidemic 传播在此断链。转发目标只由邻居集合 / fanout / exclude 决定。
    #[test]
    fn gossip_propagation_layer_stays_independent_of_local_persistence() {
        let mut engine = GossipEngine::new(100, 10, 4, 6);
        assert!(engine.is_new("m1"), "首次见到的信封必须进入处理与转发");
        assert!(!engine.is_new("m1"), "同一信封第二次到达在传播层判为重复");
        assert!(engine.is_new("m2"), "另一条消息不受前者影响");
        let neighbors = vec!["b".to_string(), "c".to_string(), "d".to_string()];
        let targets = engine.choose_fanout(&neighbors, "a");
        assert_eq!(targets.len(), 3, "fanout=4 时三个邻居都应被转发到");
        assert!(!targets.contains(&"a".to_string()), "不回发给信封的发送方");
    }

    /// P1-3 / Test 4 的前提条件：Direct 已把某 msg_id 落库之后，同一信封再到达时
    /// 必须「业务层判为已存在（于是抑制未读与事件）」且「传播层仍判为首次见到（于是
    /// 继续 verify 并在 ttl > 1 时 fan-out）」同时成立。
    /// 若把 `message_exists` 前移到 handle_gossip 开头直接 return，第二项就会被破坏，
    /// epidemic 传播在本节点断链 —— 这条测试就是防止那种"顺手简化"。
    #[test]
    fn business_duplicate_still_enters_the_propagation_layer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        let rec = MessageRecord {
            id: 0,
            msg_id: "m1".into(),
            conv_id: "dev-a".into(),
            sender_id: "dev-a".into(),
            receiver_id: "me".into(),
            kind: "text".into(),
            content: "hello".into(),
            ts: 1,
            seq: 1,
            status: "delivered".into(),
            mention_targets: None,
        };
        // Direct 先到并落库 → 它是唯一产生本地副作用的一方
        assert!(db::insert_message_if_new(&conn, &rec).unwrap());
        // 之后 Gossip 副本到达：业务层判为已存在 ⇒ 不再 touch / 不再 emit
        assert!(!db::insert_message_if_new(&conn, &rec).unwrap());
        // 但传播层（独立的内存 Bloom/LRU）从未被 Direct 登记 ⇒ 仍会走到 fan-out
        let mut engine = GossipEngine::new(100, 10, 4, 6);
        assert!(engine.is_new("m1"), "业务层已存在不得让传播层跳过转发");
        // 且两路径共用同一 msg_id，库里始终只有一行
        assert_eq!(db::get_messages(&conn, "dev-a", 10, 0).unwrap().len(), 1);
    }
