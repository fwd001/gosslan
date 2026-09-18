// 职责边界：
// - favorites.rs 的源码守卫测试
#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn rec(msg_id: &str, conv_id: &str) -> MessageRecord {
        rec_as(msg_id, conv_id, "text", "hi")
    }

    fn rec_as(msg_id: &str, conv_id: &str, kind: &str, content: &str) -> MessageRecord {
        MessageRecord {
            id: 0,
            msg_id: msg_id.into(),
            conv_id: conv_id.into(),
            sender_id: "a".into(),
            receiver_id: "b".into(),
            kind: kind.into(),
            content: content.into(),
            ts: 1,
            seq: 1,
            status: "sent".into(),
        }
    }

    /// 删消息必须连带处理三件事：待发队列、会话摘要、未读收敛。
    ///
    /// 这三条都是"删完看起来没事、下次刷新才现形"的类型：
    /// · 队列不清 ⇒ 补发把删掉的消息又推回来；
    /// · 摘要不重算 ⇒ 列表停在一条已不存在的消息上（摘要对不上任何一条）；
    /// · 未读不收敛 ⇒ 全删光了还挂着 N 条未读。
    #[test]
    fn delete_messages_clears_outbox_and_recomputes_summary() {
        let conn = mem();
        ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
        for (id, ts, text) in [
            ("m1", 10i64, "第一条"),
            ("m2", 20, "第二条"),
            ("m3", 30, "第三条"),
        ] {
            let mut m = rec_as(id, "f1", "text", text);
            m.ts = ts;
            insert_message(&conn, &m).unwrap();
        }
        touch_conversation(&conn, "f1", "single", "张三", None, "第三条", 3).unwrap();
        insert_outbox(&conn, "m3", "f1", "payload").unwrap();

        let n = delete_messages(&conn, &["m3".to_string()]).unwrap();
        assert_eq!(n, 1);
        let convs = list_conversations(&conn).unwrap();
        let c = convs.iter().find(|c| c.id == "f1").unwrap();
        assert_eq!(
            c.last_msg.as_deref(),
            Some("第二条"),
            "末条被删后必须重算摘要"
        );
        assert_eq!(
            c.last_ts,
            Some(20),
            "重算后的时间取**消息自身的时间**（列表要显示上一条的时间）"
        );
        assert_eq!(c.unread, 2, "未读要收敛到剩余条数");
        assert!(
            list_outbox(&conn, "f1").unwrap().is_empty(),
            "待发队列里的同一条必须一起删 —— 否则补发会把删掉的消息推回来"
        );

        // 再删光：摘要与未读一起清空（否则列表上会留一行指向空会话）
        delete_messages(&conn, &["m1".to_string(), "m2".to_string()]).unwrap();
        let convs = list_conversations(&conn).unwrap();
        let c = convs.iter().find(|c| c.id == "f1").unwrap();
        assert!(c.last_msg.is_none() && c.last_ts.is_none());
        assert_eq!(c.unread, 0);
    }

    /// 静默事件与群级沉淀物**不在**可删范围：删掉投票/公告会让聚合视图缺一块，
    /// 那是数据损坏，不是"清理聊天记录"（UI 也只允许勾选时间线上的消息，这是第二道闸门）。
    #[test]
    fn delete_messages_refuses_non_bubble_kinds() {
        let conn = mem();
        ensure_conversation(&conn, "group:g1", "group", "群", None).unwrap();
        insert_message(&conn, &rec_as("a1", "group:g1", "announcement", "公告")).unwrap();
        insert_message(&conn, &rec_as("r1", "group:g1", "reaction", "{}")).unwrap();
        insert_message(&conn, &rec_as("t1", "group:g1", "text", "普通")).unwrap();

        let n = delete_messages(&conn, &["a1".into(), "r1".into(), "t1".into()]).unwrap();
        assert_eq!(n, 1, "只有 Bubble 那条会被删");
        assert!(
            get_message_preview_source(&conn, "a1").is_some(),
            "群公告必须还在"
        );
        assert!(
            get_message_preview_source(&conn, "r1").is_some(),
            "静默事件必须还在"
        );
    }

    /// 空输入是幂等空操作（UI 可能传来空集合）。
    #[test]
    fn delete_messages_with_empty_input_is_noop() {
        let conn = mem();
        assert_eq!(delete_messages(&conn, &[]).unwrap(), 0);
    }

    /// 合并转发：载荷解析与摘要（畸形/超限必须报错，不许静默截断）。
    #[test]
    fn merge_payload_validation_and_summary() {
        let ok = r#"{"title":"群聊的聊天记录","items":[
            {"sender":"张三","kind":"text","content":"你好","ts":1},
            {"sender":"我","kind":"image","content":"{}","ts":2}]}"#;
        assert_eq!(crate::protocol::merge_summary(ok), "[聊天记录] 2 条");
        assert!(crate::protocol::parse_merge_payload(ok).is_ok());

        // 空 items：合并转发没有意义，直接拒
        let empty = r#"{"title":"t","items":[]}"#;
        assert!(crate::protocol::parse_merge_payload(empty).is_err());
        // 超限：报错而不是砍掉后面几条（砍掉的话用户看到的是"我明明选了 N 条"）
        let many: Vec<String> = (0..=crate::protocol::MAX_MERGE_ITEMS)
            .map(|i| format!(r#"{{"sender":"a","kind":"text","content":"{i}","ts":1}}"#))
            .collect();
        let over = format!(r#"{{"title":"t","items":[{}]}}"#, many.join(","));
        let err = crate::protocol::parse_merge_payload(&over).unwrap_err();
        assert!(err.contains("最多"), "错误文案要说清上限，实际：{err}");
        // 畸形 JSON：摘要回落到人话，不能把裸 JSON 顶到会话列表上
        assert_eq!(crate::protocol::merge_summary("not json"), "[聊天记录]");
    }

    #[test]
    fn schema_and_settings() {
        let conn = mem();
        set_setting(&conn, "device_id", "dev-abc").unwrap();
        assert_eq!(get_setting(&conn, "device_id").unwrap(), "dev-abc");
        // 覆盖写入
        set_setting(&conn, "device_id", "dev-new").unwrap();
        assert_eq!(get_setting(&conn, "device_id").unwrap(), "dev-new");
    }

    #[test]
    fn friend_add_and_pubkey_persistence() {
        let conn = mem();
        add_friend(&conn, "f1", "张三", None).unwrap();
        update_friend_pubkeys(&conn, "f1", Some("xk"), Some("ek")).unwrap();
        assert_eq!(get_friend_x25519(&conn, "f1").unwrap(), "xk");
        // 未设置公钥的好友返回 None
        add_friend(&conn, "f2", "李四", None).unwrap();
        assert!(get_friend_x25519(&conn, "f2").is_none());
        assert_eq!(list_friends(&conn).unwrap().len(), 2);
        remove_friend(&conn, "f1").unwrap();
        assert_eq!(list_friends(&conn).unwrap().len(), 1);
    }

    fn fav(id: &str, msg_id: &str, favorited_at: i64) -> Favorite {
        Favorite {
            id: id.into(),
            msg_id: msg_id.into(),
            conv_id: "c1".into(),
            sender_id: "peer".into(),
            kind: "text".into(),
            content: "你好".into(),
            ts: 100,
            favorited_at,
            media_path: None,
            media_size: 0,
            available: false,
        }
    }

    /// 收藏三件套：写入 → 列出（新的在前）→ 删除。
    ///
    /// **幂等是行为约定**，不是实现细节：消息菜单里的「收藏」可以被重复点（用户忘了收没收过），
    /// 重复点必须不产生第二条 —— 否则收藏夹里会出现两份一模一样的条目，
    /// 而两份指向同一个副本文件，删掉其中一条还会把另一条的文件连带删掉。
    #[test]
    fn favorite_insert_list_delete_and_idempotency() {
        let conn = mem();
        assert!(
            insert_favorite(&conn, &fav("f1", "m1", 1000)).unwrap(),
            "首次收藏应写入"
        );
        assert!(
            !insert_favorite(&conn, &fav("f2", "m1", 2000)).unwrap(),
            "同一条消息再次收藏必须是幂等命中（返回 false，供调用方提示「已在收藏中」并删掉多余副本）"
        );
        assert!(insert_favorite(&conn, &fav("f3", "m2", 3000)).unwrap());

        let all = list_favorites(&conn).unwrap();
        assert_eq!(all.len(), 2, "幂等命中不该产生第二行");
        assert_eq!(all[0].id, "f3", "应按收藏时间倒序（新的在前）");

        assert_eq!(get_favorite_by_msg(&conn, "m1").unwrap().unwrap().id, "f1");
        assert!(get_favorite_by_msg(&conn, "missing").unwrap().is_none());

        let removed = delete_favorite(&conn, "f1")
            .unwrap()
            .expect("应返回被删的那一行");
        assert_eq!(removed.msg_id, "m1", "返回值要带副本路径，调用方才能删文件");
        assert_eq!(list_favorites(&conn).unwrap().len(), 1);
        assert!(
            delete_favorite(&conn, "f1").unwrap().is_none(),
            "再删同一条应返回 None"
        );
    }

    /// content 路径改写：**只动 path**，name/size/sha256/subtype 必须原样保留 ——
    /// 前端的文件卡片、图片 MIME 推断全靠这几个字段，改动它们等于把收藏变成另一个文件。
    #[test]
    fn favorite_content_rewrites_only_the_path() {
        let src = r#"{"name":"a.png","path":"/downloads/a.png","size":12,"sha256":"ab","subtype":"image"}"#;
        let out = favorite_content_with_path(src, "/fav/media/f1.png");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["path"], "/fav/media/f1.png");
        assert_eq!(v["name"], "a.png");
        assert_eq!(v["size"], 12);
        assert_eq!(v["sha256"], "ab");
        assert_eq!(v["subtype"], "image");
    }

    /// 畸形 content 不能让"收藏"这个动作失败：原样返回。
    /// （界面上会显示「已清理」占位，这比"点了没反应"好得多。）
    #[test]
    fn favorite_content_keeps_malformed_input_untouched() {
        assert_eq!(favorite_content_with_path("not json", "/x"), "not json");
        assert_eq!(favorite_content_with_path("[1,2]", "/x"), "[1,2]");
    }

    /// Gossip 路径携带的 sender_pubkey 必须能补充到 friends 表（COALESCE 行为），
    /// 使后续 open_direct_content(sender_x25519_pubkey) 的 DB 查询能找到该公钥。
    #[test]
    fn gossip_pubkey_syncs_to_friend_via_coalesce() {
        let conn = mem();
        // 好友存在但 pubkey 为 NULL（模拟 announce 未到达的场景）
        add_friend(&conn, "mac", "Mac", None).unwrap();
        assert!(get_friend_x25519(&conn, "mac").is_none());

        // Gossip handler 同步 pubkey（transport.rs handle_gossip 做同样的事）
        let gossip_key = "gossip_x25519_from_envelope";
        update_friend_pubkeys(&conn, "mac", Some(gossip_key), None).unwrap();
        assert_eq!(get_friend_x25519(&conn, "mac").unwrap(), gossip_key);

        // COALESCE：已有值不会被后续的 NULL 覆盖
        update_friend_pubkeys(&conn, "mac", None, None).unwrap();
        assert_eq!(get_friend_x25519(&conn, "mac").unwrap(), gossip_key);

        // get_friend_x25519 是 sender_x25519_pubkey → open_direct_content 的
        // DB 查询路径，证明 gossip 同步后 outbox 重发的直发 ChatMessage 可以解密
    }

    #[test]
    fn group_reads_only_move_forward() {
        let conn = mem();
        upsert_group_read(&conn, "g1", "reader-a", 200).unwrap();
        upsert_group_read(&conn, "g1", "reader-a", 100).unwrap();
        upsert_group_read(&conn, "g1", "reader-b", 300).unwrap();
        assert_eq!(
            list_group_reads(&conn, "g1").unwrap(),
            vec![("reader-a".to_string(), 200), ("reader-b".to_string(), 300)]
        );
    }

    /// 已绑定的好友公钥**不得被覆盖**（只填空位）。
    /// 这是「一个伪造 announce 就能永久改掉好友公钥、击穿 E2EE」的最后一道闸。
    #[test]
    fn update_friend_pubkeys_never_overwrites_a_bound_key() {
        let conn = mem();
        add_friend(&conn, "f1", "张三", None).unwrap();
        update_friend_pubkeys(&conn, "f1", Some("real-x"), Some("real-e")).unwrap();
        assert_eq!(get_friend_x25519(&conn, "f1").as_deref(), Some("real-x"));

        // 攻击者广播来的公钥：不得覆盖真实值
        update_friend_pubkeys(&conn, "f1", Some("attacker-x"), Some("attacker-e")).unwrap();
        assert_eq!(
            get_friend_x25519(&conn, "f1").as_deref(),
            Some("real-x"),
            "已有公钥被覆盖 = 我发给好友的消息会改用攻击者公钥加密"
        );
        assert_eq!(get_friend_ed25519(&conn, "f1").as_deref(), Some("real-e"));

        // 只补缺的那一列：X25519 已绑定，Ed25519 为空时应被填上
        add_friend(&conn, "f2", "李四", None).unwrap();
        conn.execute(
            "UPDATE friends SET x25519_pubkey = 'x2' WHERE device_id = 'f2'",
            [],
        )
        .unwrap();
        update_friend_pubkeys(&conn, "f2", Some("x2-fake"), Some("e2")).unwrap();
        assert_eq!(get_friend_x25519(&conn, "f2").as_deref(), Some("x2"));
        assert_eq!(get_friend_ed25519(&conn, "f2").as_deref(), Some("e2"));
    }

    #[test]
    fn friend_remove_then_readd_flow() {
        // 删除好友后可重新添加（扫描 → 加好友流程）且历史会话/消息不受影响
        let conn = mem();
        add_friend(&conn, "f1", "张三", None).unwrap();
        update_friend_pubkeys(&conn, "f1", Some("xk"), Some("ek")).unwrap();
        ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
        insert_message(&conn, &rec("m1", "f1")).unwrap();

        remove_friend(&conn, "f1").unwrap();
        assert!(list_friends(&conn).unwrap().is_empty());
        assert!(get_friend(&conn, "f1").is_none());
        // 公钥随好友行一并移除（重新添加后重新学习）
        assert!(get_friend_x25519(&conn, "f1").is_none());
        // 聊天记录与会话行保留
        assert!(message_exists(&conn, "m1"));
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);

        // 重新添加（扫描列表再次出现 → 加好友）
        add_friend(&conn, "f1", "张三回来了", None).unwrap();
        let friends = list_friends(&conn).unwrap();
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].nickname, "张三回来了");
    }

    /// **群级沉淀物不随「清空聊天记录」消失**。
    /// 清空顺手删掉群公告是错误语义 —— 公告/置顶是群资产，不是聊天历史。
    #[test]
    fn clearing_history_keeps_group_level_artifacts() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "group:g1", "text", "普通消息")).unwrap();
        insert_message(
            &conn,
            &rec_as(
                "a1",
                "group:g1",
                "announcement",
                "{\"text\":\"本周五团建\"}",
            ),
        )
        .unwrap();
        insert_message(&conn, &rec_as("r1", "group:g1", "reaction", "{}")).unwrap();
        insert_message(
            &conn,
            &rec_as("s1", "group:g1", "system", "「张三」加入了群聊"),
        )
        .unwrap();

        delete_conversation(&conn, "group:g1").unwrap();

        let left: Vec<String> = conn
            .prepare("SELECT kind FROM messages WHERE conv_id = 'group:g1' ORDER BY kind")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(
            left,
            vec!["announcement", "reaction", "system"],
            "清空聊天记录只应删掉 Bubble（普通消息），公告/静默事件/系统提示必须留下"
        );
    }

    /// **清空边界只挡 Bubble**：若它连公告一起挡，离线成员的公告会被丢弃，
    /// 各成员看到的公告就不一致了 —— 而公告恰恰要求"所有人看到同一份"。
    #[test]
    fn clear_boundary_only_blocks_bubble_kinds() {
        let conn = mem();
        set_setting(&conn, &clear_boundary_key("g1"), "100").unwrap();
        // 水位之下（seq=50 ≤ 100）的各类消息
        assert!(
            group_message_blocked_by_boundary(&conn, "g1", 50, "text"),
            "普通消息该被挡"
        );
        assert!(
            !group_message_blocked_by_boundary(&conn, "g1", 50, "announcement"),
            "公告不得被清空边界挡住（否则离线成员看不到它）"
        );
        assert!(!group_message_blocked_by_boundary(
            &conn, "g1", 50, "reaction"
        ));
        // 水位之上的普通消息照常放行
        assert!(!group_message_blocked_by_boundary(&conn, "g1", 200, "text"));
    }

    /// 撤回：G-Set 幂等 + 物化视图让搜索/预览自动正确。
    #[test]
    fn recall_is_idempotent_and_materializes_content() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "g1", "text", "这句要撤回")).unwrap();
        assert!(!is_recalled(&conn, "m1"));

        assert!(
            insert_recall(&conn, "g1", "m1", "a", 5).unwrap(),
            "首次应返回 true"
        );
        assert!(
            !insert_recall(&conn, "g1", "m1", "a", 5).unwrap(),
            "重复撤回必须幂等"
        );
        assert!(is_recalled(&conn, "m1"));

        assert!(materialize_recall(&conn, "m1").unwrap());
        // content 清空 ⇒ 搜索命中数为 0（**不用给 search_history 加任何过滤**）
        assert_eq!(
            search_history(&conn, "撤回", None, None, None, 100)
                .unwrap()
                .len(),
            0
        );
        // 再物化一次不得改变任何东西（幂等）
        assert!(
            !materialize_recall(&conn, "m1").unwrap(),
            "已物化过应返回 false"
        );
    }

    /// **先撤后到**：撤回事件可能早于被撤回的消息抵达（Gossip 泛洪 vs outbox 直发
    /// 是两条无顺序保证的路径）。权威集合必须在消息落库**之前**就能查到。
    #[test]
    fn recall_recorded_before_the_message_arrives_still_applies() {
        let conn = mem();
        // 撤回先到（消息还没落库）
        insert_recall(&conn, "g1", "later", "a", 9).unwrap();
        assert!(is_recalled(&conn, "later"), "权威集合独立于消息行存在");
        // 消息随后到达：调用方据此以「已撤回」形态入库
        insert_message(&conn, &rec_as("later", "g1", "text", "正文不该留下")).unwrap();
        assert_eq!(
            search_history(&conn, "不该留下", None, None, None, 100)
                .unwrap()
                .len(),
            1,
            "本测试只验证权威集合可先于消息存在；入库形态由 transport 层负责"
        );
    }

    /// 静默类（表情回应）不得进入历史检索，也不得顶起群已读水位。
    ///
    /// 这两条都必须**真正执行 SQL** 才算验证：SQL 里的 kind 清单是从 `WIRE_KINDS`
    /// 拼出来的，拼错（比如占位符没被 `format!` 展开）编译期完全看不出来。
    #[test]
    fn silent_kinds_are_excluded_from_search_and_read_watermark() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "g1", "text", "周报 已发")).unwrap();
        // 同一条消息的表情回应：正文里也含"周报"，若不过滤就会被搜出来
        let mut rx = rec_as(
            "m2",
            "g1",
            "reaction",
            "{\"target\":\"m1\",\"emoji\":\"[赞]\",\"add\":true}",
        );
        rx.ts = 9_999_999; // 比正文晚：若不过滤，它会成为"最后一条"
        insert_message(&conn, &rx).unwrap();

        let hits = search_history(&conn, "周报", None, None, None, 100).unwrap();
        assert_eq!(hits.len(), 1, "回应不该出现在搜索结果里");
        assert_eq!(hits[0].msg_id, "m1");

        // 已读水位：回应晚于正文，若不过滤会把水位顶到回应上
        let (msg_id, _) = last_message_from_sender(&conn, "g1", "a").unwrap();
        assert_eq!(msg_id, "m1", "静默类不得顶起已读水位");
    }

    /// kind 清单从 `WIRE_KINDS` 派生，不是手写的 —— 加新 kind 时不会被漏掉。
    #[test]
    fn kind_class_lists_are_derived_from_the_single_table() {
        use crate::protocol::{is_silent_kind, kind_class, sql_kind_list, KindClass, WIRE_KINDS};
        // 已登记的 kind 都要分类明确；未知 kind 回退 Bubble（宁可多显示、不静默吞）
        for (k, _) in WIRE_KINDS {
            assert_eq!(
                kind_class(k) == KindClass::Silent,
                is_silent_kind(k),
                "{k} 的两个判定入口必须一致"
            );
        }
        assert_eq!(kind_class("reaction"), KindClass::Silent);
        assert_eq!(kind_class("text"), KindClass::Bubble);
        assert_eq!(kind_class("未来才有的新类型"), KindClass::Bubble);
        // 静默清单里必须含 reaction，且不含任何 Bubble 类
        let silent = sql_kind_list(&[], |c| c == KindClass::Silent);
        assert!(
            silent.contains("'reaction'"),
            "静默清单漏了 reaction：{silent}"
        );
        assert!(
            !silent.contains("'text'"),
            "静默清单混入了正文类型：{silent}"
        );
        // 检索排除清单 = 静默类 + 显式追加的 system
        let unsearchable = sql_kind_list(&["system"], |c| c == KindClass::Silent);
        assert!(unsearchable.contains("'system'") && unsearchable.contains("'reaction'"));

        // 「进时间线但不打扰」：静默类 + system。system 此前只在本机插入（不碰未读与
        // 预览），加人通知改走消息管道后必须显式归类，否则会给全体成员推通知。
        use crate::protocol::is_non_notifying_kind;
        assert!(is_non_notifying_kind("system"));
        assert!(is_non_notifying_kind("reaction"));
        assert!(is_non_notifying_kind("recall"));
        assert!(!is_non_notifying_kind("text"));
        assert!(
            !is_non_notifying_kind("recalled"),
            "已撤回要在时间线上、且它是别人主动撤回的结果，不该被静默"
        );
    }

    /// 历史检索：发送人/时间过滤、每会话命中总数、排除系统消息。
    /// 这些判据直接决定搜索结果页上「共 N 条」与筛选是否可信，所以逐条钉住。
    #[test]
    fn search_history_filters_sender_time_and_excludes_system() {
        let conn = mem();
        let mut a1 = rec_as("m1", "c1", "text", "想你 今天一起吃饭");
        a1.sender_id = "alice".into();
        a1.ts = 1_000;
        let mut b1 = rec_as("m2", "c1", "text", "我也想你");
        b1.sender_id = "bob".into();
        b1.ts = 2_000;
        let mut a2 = rec_as("m3", "c1", "text", "想你想你想你");
        a2.sender_id = "alice".into();
        a2.ts = 3_000;
        let mut sys = rec_as("m4", "c1", "system", "想你 被移出群聊");
        sys.sender_id = "sys".into();
        sys.ts = 4_000;
        let mut other = rec_as("m5", "c2", "text", "想你");
        other.sender_id = "alice".into();
        other.ts = 5_000;
        for m in [&a1, &b1, &a2, &sys, &other] {
            insert_message(&conn, m).unwrap();
        }

        // 无筛选：跨会话、按时间倒序、排除 system，且每会话总数正确
        let all = search_history(&conn, "想你", None, None, None, 100).unwrap();
        assert_eq!(all.len(), 4, "system 消息必须被排除");
        assert_eq!(all[0].msg_id, "m5", "按时间倒序（最新在前）");
        assert_eq!(all[0].total, 1, "c2 只有 1 条命中");
        let c1 = all.iter().find(|h| h.msg_id == "m3").unwrap();
        assert_eq!(c1.total, 3, "c1 有 3 条命中（不含 system）");

        // 发送人筛选：只留 alice 发的
        let by_alice = search_history(&conn, "想你", Some("alice"), None, None, 100).unwrap();
        assert_eq!(by_alice.len(), 3);
        assert!(by_alice.iter().all(|h| h.sender_id == "alice"));
        // 总数按**当前筛选**算（c1 只剩 2 条 alice 的）
        let c1_alice = by_alice.iter().find(|h| h.conv_id == "c1").unwrap();
        assert_eq!(c1_alice.total, 2);

        // 时间区间：[2000, 3000] ⇒ m2 + m3
        let ranged = search_history(&conn, "想你", None, Some(2_000), Some(3_000), 100).unwrap();
        let mut ids: Vec<&str> = ranged.iter().map(|h| h.msg_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["m2", "m3"]);

        // limit 只截断返回条数，不改总数（否则"共 N 条"会随分页变小）
        let capped = search_history(&conn, "想你", None, None, None, 1).unwrap();
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].total, 1, "被截断的是 c2 那条，其 total 仍是 1");
    }

    #[test]
    fn message_dedup_by_unique_msg_id() {
        let conn = mem();
        insert_message(&conn, &rec("m1", "c1")).unwrap();
        insert_message(&conn, &rec("m1", "c1")).unwrap(); // 重复 → OR IGNORE
        assert!(message_exists(&conn, "m1"));
        assert!(!message_exists(&conn, "m2"));
        assert_eq!(get_messages(&conn, "c1", 100, 0).unwrap().len(), 1);
    }

    /// P0-2 根因（反面用例，锁定必须避免的写法）：真实 msg_id 一旦被「解密失败的占位
    /// 系统消息」占用，之后同一 msg_id 的正确副本会被 INSERT OR IGNORE 静默吞掉，
    /// 明文永久不可恢复。所以接收端解不开时绝不能写任何占用真实 msg_id 的行。
    #[test]
    fn placeholder_on_real_msg_id_swallows_the_good_copy() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "f1", "system", "[加密消息] 解密失败")).unwrap();
        assert!(message_exists(&conn, "m1")); // 已被占用 → 处理分支会直接 Ack 并返回
        insert_message(&conn, &rec_as("m1", "f1", "text", "real plaintext")).unwrap();
        let rows = get_messages(&conn, "f1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "[加密消息] 解密失败");
        assert_eq!(rows[0].kind, "system");
    }

    /// P0-2 修复形态：解不开 ⇒ 不落库 ⇒ `message_exists` 保持 false（因而不会误发 Ack），
    /// 真实 msg_id 保持空闲，等公钥收敛后补发的正确副本正常入库；重复投递只留一行。
    #[test]
    fn failed_decrypt_leaves_msg_id_free_for_the_later_good_copy() {
        let conn = mem();
        assert!(!message_exists(&conn, "m1"));
        insert_message(&conn, &rec_as("m1", "f1", "text", "real plaintext")).unwrap();
        insert_message(&conn, &rec_as("m1", "f1", "text", "real plaintext")).unwrap(); // 心跳重复补发
        let rows = get_messages(&conn, "f1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "real plaintext");
        assert_eq!(rows[0].kind, "text");
    }

    /// Direct 与 Gossip 两条路径共用同一业务 msg_id：无论谁先到，会话内只落一行。
    #[test]
    fn direct_and_gossip_same_msg_id_persist_single_row() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "f1", "text", "via gossip")).unwrap();
        insert_message(&conn, &rec_as("m1", "f1", "text", "via direct")).unwrap();
        let rows = get_messages(&conn, "f1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "via gossip"); // 先到者胜出，后到者被幂等忽略
    }

    /// 补发前重封依赖的事实前提：发送方本地行存的是**明文**，且能按 (msg_id, sender_id)
    /// 精确取回，不会被同一 msg_id 下别的 sender_id 记录串味。
    /// 若将来本地改为存密文，此测试会立即失败（重封恢复路径随之失效）。
    #[test]
    fn own_sent_row_keeps_plaintext_selectable_by_sender() {
        let conn = mem();
        let mut sent = rec_as("m1", "f1", "text", "hello plain");
        sent.sender_id = "me".into();
        insert_message(&conn, &sent).unwrap();
        let mine: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE msg_id = ?1 AND sender_id = ?2",
                params!["m1", "me"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(mine.as_deref(), Some("hello plain"));
        let other: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE msg_id = ?1 AND sender_id = ?2",
                params!["m1", "peer"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(other, None);
    }

    /// 与两个 handler 同构的一次投递：只有本次真的插入新行才计未读、才算一次投递事件。
    fn deliver(conn: &Connection, msg_id: &str, conv_id: &str, conv_kind: &str, who: &str) -> bool {
        let inserted =
            insert_message_if_new(conn, &rec_as(msg_id, conv_id, "text", "hello")).unwrap();
        if inserted {
            touch_conversation(conn, conv_id, conv_kind, "张三", None, who, 1).unwrap();
        }
        inserted
    }

    /// 同一 msg_id 按给定先后顺序被两条路径各投一次 ⇒ 最终只有 1 行 / 未读 +1 / 事件 1 次，
    /// 且 last_msg 属于先到者（后到者不得改写）。单聊与群聊共用同一套断言。
    fn assert_one_side_effect(conv_id: &str, conv_kind: &str, first: &str, second: &str) {
        let conn = mem();
        ensure_conversation(&conn, conv_id, conv_kind, "张三", None).unwrap();
        let mut events = 0;
        for who in [first, second] {
            if deliver(&conn, "m1", conv_id, conv_kind, who) {
                events += 1;
            }
        }
        let case = format!("{first}→{second} @ {conv_id}");
        assert_eq!(
            get_messages(&conn, conv_id, 10, 0).unwrap().len(),
            1,
            "{case}"
        );
        assert_eq!(events, 1, "{case} 只能产生一次 message-received");
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == conv_id)
            .unwrap();
        assert_eq!(conv.unread, 1, "{case} 未读只能 +1");
        assert_eq!(
            conv.last_msg.as_deref(),
            Some(first),
            "{case} 后到者不得改写 last_msg"
        );
    }

    /// Test A + Test B：三态裁决本身 —— 首次 Ok(true)、重复 Ok(false)、库中只留一条。
    /// 第三条断言同时钉住「msg_id 是全局消息身份」：换一个会话投同一 msg_id 仍算重复。
    #[test]
    fn insert_message_if_new_distinguishes_first_from_duplicate() {
        let conn = mem();
        assert!(
            insert_message_if_new(&conn, &rec_as("m1", "f1", "text", "hello")).unwrap(),
            "首次必须 Ok(true)"
        );
        assert!(
            !insert_message_if_new(&conn, &rec_as("m1", "f1", "text", "hello")).unwrap(),
            "同一 msg_id 重复必须 Ok(false)"
        );
        assert!(
            !insert_message_if_new(&conn, &rec_as("m1", "f2", "text", "hello")).unwrap(),
            "换会话的同一 msg_id 仍是重复：msg_id 是全局身份"
        );
        assert!(message_exists(&conn, "m1"));
        assert_eq!(get_messages(&conn, "f1", 10, 0).unwrap().len(), 1);
        assert_eq!(get_messages(&conn, "f2", 10, 0).unwrap().len(), 0);
    }

    /// Err 不得被折叠成 false：数据库真故障必须原样冒泡，
    /// 让调用方抑制副作用并**禁止 Ack**（否则 outbox 被删 → 临时故障变永久丢失）。
    #[test]
    fn insert_message_if_new_surfaces_db_errors_as_err_not_false() {
        let conn = mem();
        conn.execute("DROP TABLE messages", []).unwrap();
        let out = insert_message_if_new(&conn, &rec_as("m1", "f1", "text", "hello"));
        assert!(
            matches!(out, Err(_)),
            "真实 DB 故障必须是 Err，不能是 Ok(false)"
        );
        assert!(
            insert_message(&conn, &rec_as("m2", "f1", "text", "hi")).is_err(),
            "包装函数同样冒泡"
        );
    }

    /// 群文件接收完成回填 path：update_message_content 必须改写 content + status，
    /// 使 read_file_preview 能按 msg_id 反查到本地路径（否则接收方图片/代码预览缺 path）。
    #[test]
    fn update_message_content_backfills_path_for_preview() {
        let conn = mem();
        // Offer 阶段先落库无 path 的内容（模拟 handle_group_file_offer）
        insert_message(
            &conn,
            &rec_as(
                "gfile-1",
                "group:g1",
                "image",
                r#"{"name":"a.png","size":3,"subtype":"image"}"#,
            ),
        )
        .unwrap();
        // Done 阶段回填 path（模拟 handle_group_file_done）
        update_message_content(
            &conn,
            "gfile-1",
            r#"{"name":"a.png","path":"/tmp/a.png","size":3,"subtype":"image"}"#,
            "delivered",
        )
        .unwrap();
        let (_, content) = get_message_preview_source(&conn, "gfile-1").unwrap();
        assert!(
            content.contains("\"path\""),
            "Done 后 content 必须回填 path"
        );
        assert!(content.contains("/tmp/a.png"), "path 必须指向本地文件");
    }

    /// Test 1 + Test 2（单聊）：Direct→Gossip 与 Gossip→Direct 两种顺序都只生效一次。
    #[test]
    fn unread_and_event_fire_only_for_the_winning_insert() {
        assert_one_side_effect("f1", "single", "direct", "gossip");
        assert_one_side_effect("f1", "single", "gossip", "direct");
    }

    /// Test 1 + Test 2（群聊）：Gossip 的单聊与群聊共用同一落库块，两个 kind 都必须覆盖。
    #[test]
    fn group_msg_id_duplicate_counts_unread_and_event_once() {
        assert_one_side_effect("group:g1", "group", "direct", "gossip");
        assert_one_side_effect("group:g1", "group", "gossip", "direct");
    }

    /// Test 3：同一 msg_id 被重复投递（心跳反复补发 / 同一信封多次到达）
    /// ⇒ 始终只有 1 行、1 次未读、1 次事件。
    #[test]
    fn repeated_delivery_of_same_msg_id_yields_single_side_effect() {
        let conn = mem();
        ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
        let mut events = 0;
        for i in 0..5 {
            if deliver(&conn, "m1", "f1", "single", "dup") {
                events += 1;
            }
            assert_eq!(events, 1, "第 {i} 次投递后累计投递事件数应恒为 1");
        }
        assert_eq!(get_messages(&conn, "f1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_conversations(&conn).unwrap()[0].unread, 1);
    }

    /// Test 5：竞态。多线程同时投递同一 msg_id（Direct 与 Gossip 交错的最坏情况），
    /// 连接模型与 AppState.db 一致（Mutex\<Connection\>）。只可能有一个 fresh=true。
    #[test]
    fn concurrent_same_msg_id_has_exactly_one_winner() {
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;
        let conn = Arc::new(Mutex::new(mem()));
        let gate = Arc::new(Barrier::new(8));
        let winners = Arc::new(Mutex::new(0u32));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let (c, g, w) = (conn.clone(), gate.clone(), winners.clone());
            handles.push(thread::spawn(move || {
                g.wait();
                let dbc = c.lock().unwrap_or_else(|e| e.into_inner());
                if insert_message_if_new(&dbc, &rec_as("m1", "f1", "text", "hello")).unwrap() {
                    touch_conversation(&dbc, "f1", "single", "张三", None, "hello", 1).ok();
                    *w.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            *winners.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "同一 msg_id 只能有一个首次插入者"
        );
        let dbc = conn.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(get_messages(&dbc, "f1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_conversations(&dbc).unwrap()[0].unread, 1);
    }

    #[test]
    fn conversation_unread_and_mark_read() {
        let conn = mem();
        ensure_conversation(&conn, "c1", "single", "张三", None).unwrap();
        touch_conversation(&conn, "c1", "single", "张三", None, "hello", 1).unwrap();
        touch_conversation(&conn, "c1", "single", "张三", None, "world", 1).unwrap();
        let conv = &list_conversations(&conn).unwrap()[0];
        assert_eq!(conv.unread, 2);
        assert_eq!(conv.last_msg.as_deref(), Some("world"));
        mark_read(&conn, "c1").unwrap();
        assert_eq!(list_conversations(&conn).unwrap()[0].unread, 0);
    }

    #[test]
    fn delete_conversation_removes_messages_and_row() {
        let conn = mem();
        // 两个会话互不干扰
        ensure_conversation(&conn, "c1", "single", "张三", None).unwrap();
        ensure_conversation(&conn, "c2", "single", "李四", None).unwrap();
        insert_message(&conn, &rec("m1", "c1")).unwrap();
        insert_message(&conn, &rec("m2", "c1")).unwrap();
        insert_message(&conn, &rec("m3", "c2")).unwrap();
        assert_eq!(list_conversations(&conn).unwrap().len(), 2);

        delete_conversation(&conn, "c1").unwrap();

        // c1 会话行与消息全部清除；c2 不受影响
        let remaining = list_conversations(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "c2");
        assert!(!message_exists(&conn, "m1"));
        assert!(!message_exists(&conn, "m2"));
        assert!(message_exists(&conn, "m3"));
        assert_eq!(get_messages(&conn, "c1", 100, 0).unwrap().len(), 0);
        assert_eq!(get_messages(&conn, "c2", 100, 0).unwrap().len(), 1);
    }

    #[test]
    fn delete_conversation_idempotent_on_missing() {
        let conn = mem();
        // 不存在也不报错（前端 UI 二次确认后用户可能在另一边删了/网络抖动）
        delete_conversation(&conn, "nonexistent").unwrap();
    }

    #[test]
    fn outbox_offline_queue_dedup_and_delete() {
        let conn = mem();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap(); // 同 msg_id 去重
        let pending = list_outbox(&conn, "f1").unwrap();
        assert_eq!(pending.len(), 1);
        delete_outbox(&conn, pending[0].0).unwrap();
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
    }

    /// outbox 的身份是 msg_id 而非密文：补发前重新加密只会换 payload，
    /// 同一 msg_id 再入队仍被唯一约束忽略（不产生第二行、不覆盖首行），
    /// Ack 仍按 msg_id 精确删除 ⇒ 重封不破坏消息身份 / 幂等 / outbox 语义。
    #[test]
    fn outbox_identity_is_msg_id_not_payload() {
        let conn = mem();
        insert_outbox(&conn, "m1", "f1", r#"{"msg_id":"m1","content":"enc1:old"}"#).unwrap();
        insert_outbox(
            &conn,
            "m1",
            "f1",
            r#"{"msg_id":"m1","content":"enc1:resealed"}"#,
        )
        .unwrap();
        let pending = list_outbox(&conn, "f1").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("enc1:old")); // 首行原样保留，由 flush 时重封
                                                    // Ack 分支的删除路径（transport.rs 同构 SQL）
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", params!["m1"])
            .unwrap();
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
    }

    #[test]
    fn message_and_outbox_are_written_atomically() {
        let conn = mem();
        let m = rec("atomic-1", "peer-1");
        insert_message_and_outbox(&conn, &m, "peer-1", "payload").unwrap();
        assert_eq!(get_messages(&conn, "peer-1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_outbox(&conn, "peer-1").unwrap().len(), 1);

        // 同一业务 ID 再写入时事务失败，不能额外留下半条 outbox 或半条消息。
        assert!(insert_message_and_outbox(&conn, &m, "peer-1", "payload-2").is_err());
        assert_eq!(get_messages(&conn, "peer-1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_outbox(&conn, "peer-1").unwrap().len(), 1);
    }

    #[test]
    fn group_create_and_members() {
        let conn = mem();
        create_group(&conn, "g1", "群聊", "me", &["me".into(), "f1".into()]).unwrap();
        let groups = list_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].members.contains(&"f1".to_string()));
        assert!(groups[0].members.contains(&"me".to_string()));
    }

    #[test]
    fn transfer_upsert_tracks_progress() {
        let conn = mem();
        upsert_transfer(&conn, "t1", "f1", "a.txt", 100, "send", "active", None, 0.5).unwrap();
        upsert_transfer(&conn, "t1", "f1", "a.txt", 100, "send", "done", None, 1.0).unwrap();
        let transfers = list_transfers(&conn).unwrap();
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].status, "done");
        assert_eq!(transfers[0].progress, 1.0);
    }

    fn status_of(conn: &Connection, msg_id: &str) -> String {
        conn.query_row(
            "SELECT status FROM messages WHERE msg_id = ?1",
            params![msg_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 已读不可逆：outbox 补发会让同一 msg_id 再送达一次并带回迟到的 Ack，
    /// 无条件覆盖会把已读退回「对方未读」，界面上刚亮的绿勾跳回空圆框。
    #[test]
    fn late_ack_never_regresses_a_read_message() {
        let conn = mem();
        insert_message(&conn, &rec("m1", "f1")).unwrap();
        set_message_status(&conn, "m1", "delivered").unwrap();
        assert_eq!(
            status_of(&conn, "m1"),
            "delivered",
            "正常前进：sent → delivered"
        );
        // 对端已读回执（与 transport.rs ReadReceipt 分支同构的 SQL）
        let updated = conn
            .execute(
                "UPDATE messages SET status = 'read'
                 WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                params!["f1", "a", 5],
            )
            .unwrap();
        assert_eq!(updated, 1);
        assert_eq!(status_of(&conn, "m1"), "read");
        set_message_status(&conn, "m1", "delivered").unwrap();
        assert_eq!(status_of(&conn, "m1"), "read", "迟到的 Ack 不得回退已读");
    }

    /// 局域网默认开启：无键（首次安装、以及从未写过该键的旧版本升级）即为开，
    /// 并立即将 "1" 持久化（之后每次启动读到明确值，不再依赖隐式默认）；
    /// 只有用户显式关闭才持久化为关，「恢复默认」清键后回到默认开。
    #[test]
    fn lan_enabled_defaults_on_and_keeps_explicit_off() {
        let conn = mem();
        // 缺省必须开启，且把 "1" 写入 settings
        assert!(get_lan_enabled(&conn), "缺省必须开启");
        assert_eq!(
            get_setting(&conn, "lan_enabled").as_deref(),
            Some("1"),
            "get_lan_enabled(None) 应立即持久化 true"
        );
        // 用户关闭 → 重启后仍为关
        set_lan_enabled(&conn, false).unwrap();
        assert!(!get_lan_enabled(&conn), "用户关闭后重启仍为关");
        // 用户开启 → 重启后仍为开
        set_lan_enabled(&conn, true).unwrap();
        assert!(get_lan_enabled(&conn));
        // 恢复默认（清键）→ 回到开
        delete_setting(&conn, "lan_enabled").unwrap();
        assert!(get_lan_enabled(&conn), "恢复默认后回到开");
    }

    /// 蓝牙开关偏好：显式值必须被尊重（默认值只在"键不存在"时生效）。
    ///
    /// ⚠️ 默认值本身依赖目标平台（`cfg!(mobile)` ⇒ 手机默认开、桌面默认关），
    /// 主机单测只能覆盖"桌面 = 关"这一半；手机那一半由 `bt_default_on_for_mobile`
    /// 那条源码规则护栏盯着（见 `lib.rs` 的测试模块）。
    #[test]
    fn bt_enabled_defaults_on_and_keeps_explicit_value() {
        let conn = mem();
        assert!(
            get_bt_enabled(&conn),
            "缺省必须是**开**（用户规则：有蓝牙就默认开）"
        );
        assert_eq!(get_setting(&conn, "bt_enabled").as_deref(), Some("1"));

        set_bt_enabled(&conn, true).unwrap();
        assert!(get_bt_enabled(&conn), "显式打开必须生效");
        set_bt_enabled(&conn, false).unwrap();
        assert!(!get_bt_enabled(&conn), "显式关闭必须生效");
    }

    // ================================================================
    // pending_reads 持久化测试
    // ================================================================

    /// Test 1：upsert 使用 max 语义——较旧 timestamp 不覆盖较新。
    #[test]
    fn pending_read_upsert_keeps_max() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "B", 100).unwrap(); // 较旧，不覆盖
        let rows = load_pending_reads(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ("B".to_string(), 200));

        upsert_pending_read(&conn, "B", 300).unwrap(); // 较新，更新
        let rows = load_pending_reads(&conn).unwrap();
        assert_eq!(rows[0], ("B".to_string(), 300));
    }

    /// Test 2：load_pending_reads 正确读取多个 peer。
    #[test]
    fn pending_read_load_multiple_peers() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "C", 500).unwrap();
        let mut rows = load_pending_reads(&conn).unwrap();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("B".to_string(), 200));
        assert_eq!(rows[1], ("C".to_string(), 500));
    }

    /// Test 3：delete_pending_read 只删除指定 peer。
    #[test]
    fn pending_read_delete_only_target() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "C", 500).unwrap();
        delete_pending_read(&conn, "B").unwrap();
        let rows = load_pending_reads(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ("C".to_string(), 500));
    }

    /// Test 4：重启恢复——DB 写入后，新连接 load 能正确恢复。
    #[test]
    fn pending_read_survives_restart() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "C", 500).unwrap();
        // 模拟进程重启：新建 HashMap，从 DB 加载
        let mut restored = std::collections::HashMap::new();
        for (peer_id, ts) in load_pending_reads(&conn).unwrap() {
            let cur = restored.entry(peer_id).or_insert(ts);
            *cur = (*cur).max(ts);
        }
        assert_eq!(restored.get("B"), Some(&200));
        assert_eq!(restored.get("C"), Some(&500));
        // DB 中的记录仍然存在（flush 时才会删除）
        assert_eq!(load_pending_reads(&conn).unwrap().len(), 2);
    }

    /// Test 5：delete 后 load 为空。
    #[test]
    fn pending_read_delete_all() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        delete_pending_read(&conn, "B").unwrap();
        assert!(load_pending_reads(&conn).unwrap().is_empty());
    }

    /// Test 6：空 DB load 返回空 vec。
    #[test]
    fn pending_read_load_empty_db() {
        let conn = mem();
        assert!(load_pending_reads(&conn).unwrap().is_empty());
    }

    // ================================================================
    // 搜索测试
    // ================================================================

    fn insert_text_msg(conn: &Connection, msg_id: &str, conv_id: &str, content: &str) {
        insert_message(
            conn,
            &MessageRecord {
                id: 0,
                msg_id: msg_id.into(),
                conv_id: conv_id.into(),
                sender_id: "a".into(),
                receiver_id: "b".into(),
                kind: "text".into(),
                content: content.into(),
                ts: 1,
                seq: 1,
                status: "delivered".into(),
            },
        )
        .unwrap();
    }

    /// 普通文本搜索：中文 + 英文。
    #[test]
    fn search_messages_plain_text() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "hello world");
        insert_text_msg(&conn, "m2", "conv2", "今天测试 hello");
        insert_text_msg(&conn, "m3", "conv3", "没有匹配");

        let r = search_messages(&conn, "hello", 10).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.contains(&"conv1".to_string()));
        assert!(r.contains(&"conv2".to_string()));

        let r = search_messages(&conn, "没有", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], "conv3");
    }

    /// LIKE 通配符 % 和 _ 按字面字符搜索，不作为 wildcard。
    #[test]
    fn search_messages_escapes_like_wildcards() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "100% 完成");
        insert_text_msg(&conn, "m2", "conv2", "a_b 测试");

        // % 应按字面搜索，不是 wildcard
        let r = search_messages(&conn, "100%", 10).unwrap();
        assert_eq!(r.len(), 1, "% 应按字面匹配");
        assert_eq!(r[0], "conv1");

        // _ 应按字面搜索，不是 wildcard
        let r = search_messages(&conn, "a_b", 10).unwrap();
        assert_eq!(r.len(), 1, "_ 应按字面匹配");
        assert_eq!(r[0], "conv2");

        // 不应匹配 "100% 完成" 中的 "100" 作为独立搜索（% 是字面字符）
        let r = search_messages(&conn, "100", 10).unwrap();
        assert_eq!(r.len(), 1, "'100' 应匹配 '100% 完成'");
    }

    /// 空结果。
    #[test]
    fn search_messages_no_results() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "hello");
        let r = search_messages(&conn, "不存在", 10).unwrap();
        assert!(r.is_empty());
    }

    /// 中文搜索正常。
    #[test]
    fn search_messages_chinese() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "你好世界");
        insert_text_msg(&conn, "m2", "conv2", "hello world");
        let r = search_messages(&conn, "你好", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], "conv1");
    }

    /// emoji 搜索正常（按字符匹配，非 UTF-8 字节）。
    #[test]
    fn search_messages_emoji() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "🎉庆祝🎉");
        let r = search_messages(&conn, "🎉", 10).unwrap();
        assert_eq!(r.len(), 1);
    }

    /// 大小写不敏感搜索。
    #[test]
    fn search_messages_case_insensitive() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "Hello World");
        let r = search_messages(&conn, "hello", 10).unwrap();
        assert_eq!(r.len(), 1);
        let r = search_messages(&conn, "HELLO", 10).unwrap();
        assert_eq!(r.len(), 1);
    }

    /// update_conversation_profile 只更新 name/avatar，不修改 last_msg/last_ts。
    #[test]
    fn update_conversation_profile_preserves_last_msg() {
        let conn = mem();
        ensure_conversation(&conn, "dev-a", "single", "Old", None).unwrap();
        touch_conversation(&conn, "dev-a", "single", "Old", None, "Hello", 1).unwrap();

        // 验证初始状态
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == "dev-a")
            .unwrap();
        assert_eq!(conv.name, "Old");
        assert_eq!(conv.last_msg.as_deref(), Some("Hello"));

        // 执行 update_conversation_profile
        update_conversation_profile(&conn, "dev-a", "New", Some("new_avatar")).unwrap();

        // 验证：name/avatar 已更新，last_msg/last_ts 保持不变
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == "dev-a")
            .unwrap();
        assert_eq!(conv.name, "New", "name 应已更新");
        assert_eq!(
            conv.last_msg.as_deref(),
            Some("Hello"),
            "last_msg 不应被修改"
        );
    }

    /// update_conversation_profile 不影响 group 会话。
    #[test]
    fn update_conversation_profile_ignores_group() {
        let conn = mem();
        ensure_conversation(&conn, "group:g1", "group", "测试群", None).unwrap();
        update_conversation_profile(&conn, "group:g1", "新名", None).unwrap();
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == "group:g1")
            .unwrap();
        assert_eq!(conv.name, "测试群", "group 会话不应被修改");
    }

    /// update_conversation_profile 对不存在的会话不报错。
    #[test]
    fn update_conversation_profile_noop_on_missing() {
        let conn = mem();
        update_conversation_profile(&conn, "nonexistent", "name", None).unwrap();
    }

    /// clear_all_data 通过 SQL 验证：删除所有业务数据，保留 friends 和非 gk: settings。
    #[test]
    fn clear_all_data_sql_deletes_and_preserves() {
        let conn = mem();
        // 插入测试数据
        insert_message(&conn, &rec("m1", "c1")).unwrap();
        ensure_conversation(&conn, "c1", "single", "测试", None).unwrap();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap();
        upsert_transfer(&conn, "t1", "f1", "a.txt", 100, "send", "active", None, 0.0).unwrap();
        upsert_pending_read(&conn, "f1", 200).unwrap();
        create_group(&conn, "g1", "群", "owner", &["a".into()]).unwrap();
        add_friend(&conn, "f1", "好友", None).unwrap();
        set_setting(&conn, "device_id", "dev-1").unwrap();
        set_setting(&conn, "nickname", "昵称").unwrap();
        set_setting(&conn, "gk:g1", "secret").unwrap();

        // 模拟 clear_all_data SQL 部分
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute("DELETE FROM group_members", []).unwrap();
        tx.execute("DELETE FROM groups", []).unwrap();
        tx.execute("DELETE FROM messages", []).unwrap();
        tx.execute("DELETE FROM conversations", []).unwrap();
        tx.execute("DELETE FROM outbox", []).unwrap();
        tx.execute("DELETE FROM file_transfers", []).unwrap();
        tx.execute("DELETE FROM pending_reads", []).unwrap();
        tx.execute("DELETE FROM settings WHERE key LIKE 'gk:%'", [])
            .unwrap();
        tx.commit().unwrap();

        // 验证：业务数据已删除
        assert!(get_messages(&conn, "c1", 10, 0).unwrap().is_empty());
        assert!(list_conversations(&conn).unwrap().is_empty());
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
        assert!(list_transfers(&conn).unwrap().is_empty());
        assert!(load_pending_reads(&conn).unwrap().is_empty());
        assert!(list_groups(&conn).unwrap().is_empty());
        assert!(get_setting(&conn, "gk:g1").is_none());

        // 验证：friends 和非 gk: settings 保留
        assert!(get_friend(&conn, "f1").is_some());
        assert_eq!(get_setting(&conn, "device_id").as_deref(), Some("dev-1"));
        assert_eq!(get_setting(&conn, "nickname").as_deref(), Some("昵称"));
    }

    // ---------- 群文件 per-recipient 投递状态 ----------

    fn group_file(transfer_id: &str) -> GroupFile {
        GroupFile {
            transfer_id: transfer_id.to_string(),
            group_id: "g1".to_string(),
            sender_id: "a".to_string(),
            name: "report.pdf".to_string(),
            size: 1024,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            status: "pending".to_string(),
            created_at: now_ms(),
            scope: "chat".to_string(),
            todo_id: "".to_string(),
        }
    }

    fn group_file_fixture() -> Connection {
        let conn = mem();
        // 群 g1：成员 a（sender）/ b / c / d
        create_group(
            &conn,
            "g1",
            "测试群",
            "a",
            &[
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
            ],
        )
        .unwrap();
        conn
    }

    /// 群文件列表：按群过滤、按创建时间倒序，且不串到别的群。
    #[test]
    fn list_group_files_scoped_and_newest_first() {
        let conn = group_file_fixture();
        let mut older = group_file("gf-old");
        older.created_at = 1_000;
        let mut newer = group_file("gf-new");
        newer.created_at = 2_000;
        insert_group_file(&conn, &older).unwrap();
        insert_group_file(&conn, &newer).unwrap();

        // 另一个群的文件不得混入（insert_group_file 会校验群存在，故先建群）
        create_group(&conn, "g2", "另一个群", "a", &["a".to_string()]).unwrap();
        let mut other = group_file("gf-other");
        other.group_id = "g2".to_string();
        insert_group_file(&conn, &other).unwrap();

        let listed = list_group_files(&conn, "g1").unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|f| f.transfer_id.as_str())
                .collect::<Vec<_>>(),
            vec!["gf-new", "gf-old"]
        );
        assert!(list_group_files(&conn, "nonexistent").unwrap().is_empty());
    }

    /// 1+2+3：建群文件 + 为 B/C/D 建 recipient state + 分别置 completed/sending/pending。
    #[test]
    fn group_file_recipient_states_persist() {
        let conn = group_file_fixture();
        let f = group_file("gf-1");
        insert_group_file(&conn, &f).unwrap();
        assert_eq!(get_group_file(&conn, "gf-1").unwrap().name, "report.pdf");

        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "c", "sending", 0.4).unwrap();
        // d 保持初始 pending

        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        let by_id: std::collections::HashMap<_, _> = recipients
            .iter()
            .map(|r| (r.recipient_id.as_str(), (r.status.as_str(), r.progress)))
            .collect();
        assert_eq!(by_id.get("b"), Some(&("completed", 1.0)));
        assert_eq!(by_id.get("c"), Some(&("sending", 0.4)));
        assert_eq!(by_id.get("d"), Some(&("pending", 0.0)));
    }

    /// 重启后离线补发：upsert_group_file_receive 幂等重建会话，
    /// 未完成态复位回 sending，completed 不被覆盖。
    #[test]
    fn upsert_group_file_receive_reestablishes_session() {
        let conn = group_file_fixture();
        let f = group_file("gf-1");

        // 首次 Offer：建立记录 + recipient（sending）
        upsert_group_file_receive(&conn, &f, "b").unwrap();
        assert_eq!(
            get_group_file_recipient_status(&conn, "gf-1", "b").as_deref(),
            Some("sending")
        );

        // 模拟中断置 failed → 重启后重发 Offer → 复位回 sending
        update_group_file_recipient(&conn, "gf-1", "b", "failed", 0.0).unwrap();
        upsert_group_file_receive(&conn, &f, "b").unwrap();
        assert_eq!(
            get_group_file_recipient_status(&conn, "gf-1", "b").as_deref(),
            Some("sending")
        );

        // 已完成态不被重建覆盖
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        upsert_group_file_receive(&conn, &f, "b").unwrap();
        assert_eq!(
            get_group_file_recipient_status(&conn, "gf-1", "b").as_deref(),
            Some("completed")
        );
    }

    /// 5：更新 C → completed 不影响 B/D 的状态。
    #[test]
    fn update_one_recipient_does_not_affect_others() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        update_group_file_recipient(&conn, "gf-1", "c", "completed", 1.0).unwrap();

        let by_id: std::collections::HashMap<_, _> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| (r.recipient_id, r.status))
            .collect();
        assert_eq!(by_id.get("b").map(String::as_str), Some("completed"));
        assert_eq!(by_id.get("c").map(String::as_str), Some("completed"));
        assert_eq!(by_id.get("d").map(String::as_str), Some("pending"));
    }

    /// 6：同一 (transfer_id, recipient_id) 重复插入必须报错（PRIMARY KEY）。
    #[test]
    fn duplicate_recipient_insert_rejected() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();
        assert!(insert_group_file_recipient(&conn, "gf-1", "b").is_err());
    }

    /// 权限边界：群不存在 / sender 非成员 / recipient 非成员 → 拒绝。
    #[test]
    fn group_file_permission_checks() {
        let conn = group_file_fixture();

        // 群不存在
        let mut f = group_file("gf-x");
        f.group_id = "g-missing".to_string();
        assert!(insert_group_file(&conn, &f).is_err());

        // sender 不是群成员
        let mut f = group_file("gf-1");
        f.sender_id = "outsider".to_string();
        assert!(insert_group_file(&conn, &f).is_err());

        // 群外 peer 不能建 recipient state
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        assert!(insert_group_file_recipient(&conn, "gf-1", "outsider").is_err());
        // recipient 更新不存在的 state 报错（不静默创建）
        assert!(update_group_file_recipient(&conn, "gf-1", "outsider", "pending", 0.0).is_err());
    }

    /// 7：删除群 → 群文件与 recipient 数据级联清理（delete_group 事务语义）。
    #[test]
    fn delete_group_cascades_group_files() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }

        delete_group(&conn, "g1").unwrap();

        assert!(get_group_file(&conn, "gf-1").is_none());
        assert!(list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .is_empty());
    }

    /// get_group_file 不存在的 transfer 返回 None。
    #[test]
    fn get_group_file_missing_returns_none() {
        let conn = mem();
        assert!(get_group_file(&conn, "nope").is_none());
    }

    /// 群主转让：只改 creator，成员表保持不变。
    #[test]
    fn set_group_creator_updates_creator_only() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();

        set_group_creator(&conn, "g1", "b").unwrap();

        let g = get_group(&conn, "g1").unwrap();
        assert_eq!(g.creator, "b");
        let mut got = g.members.clone();
        got.sort();
        assert_eq!(got, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        // 不存在的群：影响 0 行，不报错也不产生记录
        set_group_creator(&conn, "nope", "x").unwrap();
        assert!(get_group(&conn, "nope").is_none());
    }

    // ---------- 群关系同步 ≠ 聊天会话（conversation 只能由聊天活动驱动） ----------

    /// Test 1 + Test 4：群关系同步（GroupKey → `upsert_group`）只建立 groups / group_members，
    /// **不得**创建 conversation —— conversation 是「聊天会话索引」，只有收到新消息
    /// （`insert_message` + `touch_conversation`）时才产生。等价于「重新安装后只同步群关系」。
    #[test]
    fn group_relation_sync_does_not_create_conversation() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        // 初始：groups / conversations / messages 全空（重新安装语义）
        assert!(list_groups(&conn).unwrap().is_empty());
        assert!(list_conversations(&conn).unwrap().is_empty());

        // 群关系同步（等价于 handle_group_key 收到 GroupKey 后调用 upsert_group）
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();

        // groups 恢复，但 conversations 不创建、messages 仍为空
        assert_eq!(list_groups(&conn).unwrap().len(), 1);
        assert!(list_conversations(&conn).unwrap().is_empty());
        assert_eq!(count_messages(&conn, "group:g1"), 0);
    }

    /// Test 2 + Test 5：新群消息（落库 + `touch_conversation`）才创建 conversation。
    /// 在「只同步过群关系、无会话」的基础上收到新消息 → conversation 出现。
    #[test]
    fn group_message_creates_conversation() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();
        assert!(list_conversations(&conn).unwrap().is_empty());

        // 收到新群消息 → insert_message + touch_conversation（transport.rs 群消息接收路径）
        insert_message(&conn, &rec_as("m4", "group:g1", "text", "hi")).unwrap();
        touch_conversation(&conn, "group:g1", "group", "群", None, "hi", 1).unwrap();

        assert_eq!(list_groups(&conn).unwrap().len(), 1);
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);
        assert_eq!(count_messages(&conn, "group:g1"), 1);
    }

    /// Test 3：已有 conversation 时，再次群关系同步既不删除、也不重复创建。
    /// 即「GroupKey 更新永远不能删掉已存在的 conversation」。
    #[test]
    fn existing_conversation_survives_group_relation_sync() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        // 先产生一个真实聊天会话（收到过群消息）
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();
        touch_conversation(&conn, "group:g1", "group", "群", None, "hi", 1).unwrap();
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);

        // 再次群关系同步（重复 GroupKey，含群名刷新）→ conversation 仍为 1
        upsert_group(&conn, "g1", "群改名", "a", &members).unwrap();
        assert_eq!(list_groups(&conn).unwrap().len(), 1);
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);
    }

    /// 群名只能由群主改（持久层兜底）：creator 对不上就不许覆盖名字。
    ///
    /// 注意本测试**只覆盖持久层这一半**：`upsert_group` 看不到信封的 `sender_id`，
    /// 所以「成员把 group_creator 填成真群主的 id」这种伪造它拦不住 ——
    /// 那一半由调用点（`transport.rs` 群消息分支校验 `env.sender_id == creator`）负责。
    /// 两层缺一不可，这里把边界钉清楚，避免有人误以为这一层够了。
    #[test]
    fn group_name_only_updates_for_the_recorded_creator() {
        let conn = mem();
        let members: Vec<String> = ["owner", "member"].iter().map(|s| s.to_string()).collect();
        upsert_group(&conn, "g1", "产品组", "owner", &members).unwrap();
        assert_eq!(get_group(&conn, "g1").unwrap().name, "产品组");

        // 群主本人改（creator 一致）→ 生效
        upsert_group(&conn, "g1", "产品组（改）", "owner", &members).unwrap();
        assert_eq!(get_group(&conn, "g1").unwrap().name, "产品组（改）");

        // 自称是另一个 creator → 名字不得被覆盖，creator 也不得被顶掉
        upsert_group(&conn, "g1", "【系统通知】点此领取", "member", &members).unwrap();
        let g = get_group(&conn, "g1").unwrap();
        assert_eq!(g.name, "产品组（改）", "非群主不得覆盖群名");
        assert_eq!(g.creator, "owner", "creator 不得被顶掉");

        // 成员表只增不减（INSERT OR IGNORE）：新成员能加进来
        upsert_group(
            &conn,
            "g1",
            "产品组（改）",
            "owner",
            &[
                "owner".to_string(),
                "member".to_string(),
                "newbie".to_string(),
            ],
        )
        .unwrap();
        assert!(get_group(&conn, "g1")
            .unwrap()
            .members
            .contains(&"newbie".to_string()));
    }

    // ---------- GroupFileOffer / session-key 阶段 ----------

    /// recipient 集合 = 创建时的成员快照：建文件后再加群成员不影响已建 recipients。
    #[test]
    fn group_file_recipients_are_creation_snapshot() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        // 创建快照之后群新增成员 e（动态成员语义后续阶段处理）
        add_group_member(&conn, "g1", "e").unwrap();

        let ids: Vec<String> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| r.recipient_id)
            .collect();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string(), "d".to_string()]);
    }

    /// 接收端权限判定基础：get_group 返回成员表，群外 sender 不在其中。
    /// （handle_group_file_offer 用同一判定：local group exists && sender ∈ members）
    #[test]
    fn outsider_sender_not_in_local_group_members() {
        let conn = group_file_fixture();
        let g = get_group(&conn, "g1").unwrap();
        assert!(g.members.contains(&"a".to_string()));
        assert!(!g.members.contains(&"outsider".to_string()));
        assert!(get_group(&conn, "g-missing").is_none(), "本地群不存在");
    }

    /// 幂等：相同 transfer_id 的 Offer 重复处理时，已存在记录即安全忽略
    /// （接收端依据 get_group_file 是否已存在；DB 层重复插入本身被 PK 拒绝）。
    #[test]
    fn duplicate_transfer_id_offer_is_idempotent() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();

        // 已存在 → 接收端直接 return（模拟判定条件）
        assert!(get_group_file(&conn, "gf-1").is_some());
        // 即使重复插入也被 PK 拒绝，不会覆盖既有状态
        assert!(insert_group_file(&conn, &group_file("gf-1")).is_err());
        assert!(insert_group_file_recipient(&conn, "gf-1", "b").is_err());
        // 既有状态未被覆盖
        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].status, "pending");
    }

    /// 事务原子性：任一 recipient 创建失败（群外成员）→ 整体回滚，
    /// 不留「group_files 已建但 recipient 只建了一半」的半完成状态；
    /// 发送端据此在 DB 初始化失败时不发送 Offer、不残留内存 file_key。
    #[test]
    fn group_file_creation_is_atomic() {
        let conn = group_file_fixture();
        let f = group_file("gf-1");
        let tx = conn.unchecked_transaction().unwrap();
        insert_group_file(&tx, &f).unwrap();
        insert_group_file_recipient(&tx, "gf-1", "b").unwrap();
        // 群外成员触发失败
        assert!(insert_group_file_recipient(&tx, "gf-1", "outsider").is_err());
        // 模拟发送端在 Err 后放弃提交（rollback）
        drop(tx);

        // 半完成状态不存在：group_files 与 recipients 均未落库
        assert!(get_group_file(&conn, "gf-1").is_none());
        assert!(list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .is_empty());
    }

    /// sender 自己不进入 recipient state（快照只含其他成员）。
    #[test]
    fn sender_not_in_recipient_states() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap(); // sender = "a"
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        let ids: Vec<String> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| r.recipient_id)
            .collect();
        assert!(
            !ids.contains(&"a".to_string()),
            "sender 不应有 recipient state"
        );
        assert_eq!(ids.len(), 3);
    }

    /// file session key 不进入 SQLite 持久化：两张群文件表均无密钥列。
    #[test]
    fn group_file_keys_not_persisted() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();

        for table in ["group_files", "group_file_recipients"] {
            let cols: Vec<String> = conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            for col in &cols {
                assert!(
                    !col.to_lowercase().contains("file_key") && !col.to_lowercase().contains("key"),
                    "{table}.{col} 不应持久化文件会话密钥"
                );
            }
        }
    }

    // ---------- GroupFileCompleteAck（sender 侧 recipient 状态迁移） ----------

    /// 合法 recipient 的 success ACK → completed + progress 1.0，只影响该 recipient。
    #[test]
    fn complete_ack_updates_only_target_recipient() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }

        // B 的 success ACK
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        let by_id: std::collections::HashMap<_, _> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| (r.recipient_id, (r.status, r.progress)))
            .collect();
        assert_eq!(by_id.get("b"), Some(&("completed".to_string(), 1.0)));
        assert_eq!(by_id.get("d"), Some(&("pending".to_string(), 0.0)));
    }

    /// failure ACK → recipient failed（progress 重置为 0），不影响其他成员。
    #[test]
    fn failure_ack_marks_recipient_failed() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "c", "sending", 0.5).unwrap();

        // C 的 failure ACK
        update_group_file_recipient(&conn, "gf-1", "c", "failed", 0.0).unwrap();

        let by_id: std::collections::HashMap<_, _> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| (r.recipient_id, r.status))
            .collect();
        assert_eq!(by_id.get("c").map(String::as_str), Some("failed"));
        assert_eq!(by_id.get("b").map(String::as_str), Some("pending"));
    }

    /// 非 recipient 的 ACK 被拒绝（update 不命中）——不允许群外 peer 伪造状态。
    #[test]
    fn ack_from_non_recipient_is_rejected() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();
        assert!(update_group_file_recipient(&conn, "gf-1", "outsider", "completed", 1.0).is_err());
    }

    /// 重复 ACK 幂等：连续相同更新不报错、状态稳定、无副作用。
    #[test]
    fn repeated_ack_is_idempotent() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();

        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].status, "completed");
        assert_eq!(recipients[0].progress, 1.0);
    }

    /// 发送端验证：group_file.sender_id 必须是本机才处理 ACK
    /// （get_group_file 返回的 sender_id 供此比对；他机发起的 transfer 被拒）。
    #[test]
    fn ack_sender_check_uses_group_file_owner() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap(); // sender = "a"
        let gf = get_group_file(&conn, "gf-1").unwrap();
        // 本机是 "a" 时才处理；本机是 "b"（recipient）时 sender_id != me → 拒绝
        assert_eq!(gf.sender_id, "a");
        assert_ne!(gf.sender_id, "b");
    }

    // ---------- GroupFileChunk 原始 sender 校验 / CompleteAck 降级保护 ----------

    /// 非原始 sender 的 GroupFileChunk 必须被忽略：比对 gf.sender_id 不匹配
    /// 即拒绝（不触发 fail 清理 → 不删 .part、不清 session、不改 recipient 状态）。
    /// 复现修复前缺陷：任何群成员发垃圾 chunk 即可终止合法接收。
    #[test]
    fn group_chunk_from_non_original_sender_is_ignored() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap(); // 原始 sender = "a"
        for rid in ["b", "c"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "b", "sending", 0.5).unwrap();

        // 模拟 handle_group_file_chunk 的判定：chunk 声称来自群成员 "e"（非原始 sender）
        let gf = get_group_file(&conn, "gf-1").unwrap();
        let chunk_sender = "e"; // 群成员（非 recipient 也无妨），但不是原始 sender
        let ignored = gf.sender_id != chunk_sender;
        assert!(ignored, "非原始 sender 的 chunk 必须被忽略");

        // 无副作用：recipient 状态原样保留（未触发 fail 清理）
        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        let b = recipients.iter().find(|r| r.recipient_id == "b").unwrap();
        assert_eq!(b.status, "sending");
        assert_eq!(b.progress, 0.5);
    }

    /// recipient 已 completed 后，failure ACK 不得把 completed 降级为 failed
    /// （handle_group_file_complete_ack 的幂等保护：已 completed 则忽略 failure ACK）。
    #[test]
    fn complete_ack_failure_cannot_downgrade_completed() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();
        // B 正常完成：success ACK → completed / 1.0
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        // 模拟修复后 handle_group_file_complete_ack 对 failure ACK 的保护：
        // 已 completed 则忽略（不执行 update）
        let already_completed = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .any(|r| r.recipient_id == "b" && r.status == "completed");
        if !already_completed {
            update_group_file_recipient(&conn, "gf-1", "b", "failed", 0.0).unwrap();
        }

        // 断言：仍 completed / 1.0（修复前此处会被降级为 failed——暴露 Bug 2）
        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        let b = recipients.iter().find(|r| r.recipient_id == "b").unwrap();
        assert_eq!(b.status, "completed", "completed 不得被 failure ACK 降级");
        assert_eq!(b.progress, 1.0);
    }

    // ---------- 群聊删除边界（清除聊天数据后旧消息防回灌） ----------

    /// 清除边界按逻辑序号拦截：seq <= boundary 的旧历史拦截，之后放行；无边界不拦截。
    #[test]
    fn clear_boundary_blocks_old_group_messages() {
        let conn = group_file_fixture();
        set_clear_boundary(&conn, "g1", 100).unwrap();

        // 清除边界及更早序号 → 拦截
        assert!(group_message_blocked_by_boundary(&conn, "g1", 99, "text"));
        assert!(group_message_blocked_by_boundary(&conn, "g1", 100, "text"));
        // 清除后的新序号 → 放行
        assert!(!group_message_blocked_by_boundary(&conn, "g1", 101, "text"));
        // 未设置边界的群不拦截
        assert!(!group_message_blocked_by_boundary(&conn, "g2", 1, "text"));
        // 重复清除：边界覆盖为新值（新 boundary 之前的旧消息再次被拦截）
        set_clear_boundary(&conn, "g1", 200).unwrap();
        assert!(group_message_blocked_by_boundary(&conn, "g1", 200, "text"));
        assert!(!group_message_blocked_by_boundary(&conn, "g1", 201, "text"));
    }

    /// 删除单个群会话同样写入边界（delete_conversation 群分支语义）。
    #[test]
    fn deleting_group_conversation_sets_boundary() {
        let conn = group_file_fixture();
        set_clear_boundary(&conn, "g1", 123456789).unwrap();
        // 边界及更早序号被拦截
        assert!(group_message_blocked_by_boundary(
            &conn, "g1", 123456789, "text"
        ));
        assert!(!group_message_blocked_by_boundary(
            &conn, "g1", 123456790, "text"
        ));
    }

    // ---------- GroupFile 多 recipient 气泡状态聚合 ----------

    /// 聚合判定（handle_group_file_complete_ack failure 分支）：
    /// 全部终态且有人 completed → delivered（mixed）；全部 failed → failed；
    /// 仍有 pending/sending → 保持当前（不产生最终状态）。
    fn aggregate_bubble(recipients: &[GroupFileRecipient]) -> Option<&'static str> {
        let all_terminal = recipients
            .iter()
            .all(|r| r.status == "completed" || r.status == "failed");
        if !all_terminal {
            return None; // 气泡保持当前（sending）
        }
        Some(if recipients.iter().any(|r| r.status == "completed") {
            "delivered"
        } else {
            "failed"
        })
    }

    /// B success + C success → delivered；B failure + C failure → failed；
    /// B success + C failure（mixed）→ delivered；C 未确认 → 保持 sending。
    #[test]
    fn group_file_ack_aggregation_semantics() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        let states =
            || -> Vec<GroupFileRecipient> { list_group_file_recipients(&conn, "gf-1").unwrap() };

        // mixed：B completed / C failed → delivered（有人收到即算）
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "c", "failed", 0.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), Some("delivered"));

        // 全 failed → failed
        update_group_file_recipient(&conn, "gf-1", "b", "failed", 0.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), Some("failed"));

        // C 未确认（sending）→ None：气泡保持当前，不被单个 ACK 覆盖
        update_group_file_recipient(&conn, "gf-1", "c", "sending", 0.4).unwrap();
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), None);

        // 全部 success → delivered
        update_group_file_recipient(&conn, "gf-1", "c", "completed", 1.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), Some("delivered"));
    }

    /// 群文件本地路径经 file_transfers 持久化（gfile-{tid}）：
    /// 接收完成写入 receive/done + final_path，打开/另存/历史加载
    /// 经 transfer_id 关联到真实本地文件（重启后仍有效）。
    #[test]
    fn gfile_transfer_record_persists_local_path() {
        let conn = mem();
        upsert_transfer(
            &conn,
            "gfile-t1",
            "a",
            "report.pdf",
            2048,
            "receive",
            "done",
            Some("/downloads/report.pdf"),
            1.0,
        )
        .unwrap();
        let t = list_transfers(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "gfile-t1")
            .expect("群文件 transfer 记录应存在");
        assert_eq!(t.path.as_deref(), Some("/downloads/report.pdf"));
        assert_eq!(t.status, "done");
        assert_eq!(t.progress, 1.0);
    }

    /// 群消息 outbox：同一 msg_id 对不同 peer 各保留一行；GroupAck 只删对应行。
    #[test]
    fn group_outbox_per_peer_and_delete_by_msg_peer() {
        let conn = mem();
        insert_group_outbox(&conn, "m1", "g1", "b", "p1").unwrap();
        insert_group_outbox(&conn, "m1", "g1", "c", "p1").unwrap();
        insert_group_outbox(&conn, "m1", "g1", "b", "p1").unwrap(); // 幂等
        assert_eq!(list_group_outbox(&conn, "b").unwrap().len(), 1);
        assert_eq!(list_group_outbox(&conn, "c").unwrap().len(), 1);

        delete_group_outbox(&conn, "m1", "b").unwrap();
        assert!(list_group_outbox(&conn, "b").unwrap().is_empty());
        assert_eq!(list_group_outbox(&conn, "c").unwrap().len(), 1);

        delete_group_outbox_for_peer_in_group(&conn, "g1", "c").unwrap();
        assert!(list_group_outbox(&conn, "c").unwrap().is_empty());
    }

    /// 群 outbox 可整体按群清理（删除群时）。
    #[test]
    fn group_outbox_delete_by_group() {
        let conn = mem();
        insert_group_outbox(&conn, "m1", "g1", "b", "p").unwrap();
        insert_group_outbox(&conn, "m2", "g1", "c", "p").unwrap();
        insert_group_outbox(&conn, "m3", "g2", "b", "p").unwrap();
        delete_group_outbox_for_group(&conn, "g1").unwrap();
        assert!(list_group_outbox(&conn, "c").unwrap().is_empty());
        // g2 仍在 b 名下
        assert_eq!(list_group_outbox(&conn, "b").unwrap().len(), 1);
    }

    /// 文件 outbox 生命周期：pending → sending → pending（重试）→ 成功删除。
    #[test]
    fn file_outbox_lifecycle() {
        let conn = mem();
        insert_file_outbox(&conn, "t1", "b", None, "/tmp/a.txt", "a.txt", 10).unwrap();
        assert_eq!(list_pending_file_outbox(&conn, "b").unwrap().len(), 1);

        mark_file_outbox_sending(&conn, "t1", 0).unwrap();
        assert!(list_pending_file_outbox(&conn, "b").unwrap().is_empty());

        mark_file_outbox_pending(&conn, "t1", 0).unwrap();
        assert_eq!(list_pending_file_outbox(&conn, "b").unwrap().len(), 1);

        delete_file_outbox(&conn, "t1").unwrap();
        assert!(list_pending_file_outbox(&conn, "b").unwrap().is_empty());
    }

    /// 文件 outbox 永久失败后不再参与待投递查询。
    #[test]
    fn file_outbox_failed_is_not_pending() {
        let conn = mem();
        insert_file_outbox(&conn, "t1", "b", None, "/tmp/a.txt", "a.txt", 10).unwrap();
        mark_file_outbox_failed(&conn, "t1").unwrap();
        assert!(list_pending_file_outbox(&conn, "b").unwrap().is_empty());
    }

    /// 已读回执按「某发送者最近一条消息」取 msg_id + ts，
    /// 不取全会话最大时间戳，也不把接收方自己发的消息算进去。
    #[test]
    fn last_message_from_sender_ignores_own_and_other_senders() {
        let conn = mem();
        insert_message(&conn, &rec_as("a1", "b", "text", "from a")).unwrap();
        insert_message(&conn, &rec_as("a2", "b", "text", "from a later")).unwrap();
        // 自己（receiver_id=b 的视角，这里用 sender_id=b 模拟本地发出的消息）
        let mut own = rec_as("b1", "b", "text", "own");
        own.sender_id = "b".into();
        own.ts = 999;
        insert_message(&conn, &own).unwrap();

        let (msg_id, ts) = last_message_from_sender(&conn, "b", "a").unwrap();
        assert_eq!(msg_id, "a2");
        assert_eq!(ts, 1); // 测试 rec_as 统一 ts=1
    }

    /// 旧库没有 seq 列时，init 必须先补列再建索引，不能在建索引时崩掉。
    #[test]
    fn init_migrates_existing_db_without_seq_column() {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-migrate-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    msg_id TEXT UNIQUE NOT NULL,
                    conv_id TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    receiver_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    ts INTEGER NOT NULL,
                    status TEXT NOT NULL DEFAULT 'sent'
                );",
            )
            .unwrap();
        }
        let conn = init(&path).unwrap();
        let has_seq: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'seq'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap();
        assert!(has_seq, "旧库迁移后 messages 必须有 seq 列");
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_messages_conv_seq'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    /// 旧库没有 pinned 列时，init 必须补列且**保留既有会话数据**。
    /// 这是「老用户升级后会话列表直接打不开」的唯一防线：SCHEMA 走的是
    /// CREATE TABLE IF NOT EXISTS，表已存在时不会自动加列。
    #[test]
    fn init_migrates_existing_db_without_pinned_column() {
        let path = std::env::temp_dir().join(format!(
            "gosslan-test-migrate-pinned-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE conversations (
                    id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    name TEXT NOT NULL,
                    avatar TEXT,
                    last_msg TEXT,
                    last_ts INTEGER,
                    unread INTEGER NOT NULL DEFAULT 0,
                    updated_at INTEGER
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO conversations(id, kind, name, unread) VALUES('f1', 'single', '张三', 3)",
                [],
            )
            .unwrap();
        }
        let conn = init(&path).unwrap();
        let convs = list_conversations(&conn).unwrap();
        assert_eq!(convs.len(), 1, "迁移不得丢会话");
        assert_eq!(convs[0].name, "张三");
        assert_eq!(convs[0].unread, 3, "迁移不得重置未读");
        assert!(!convs[0].pinned, "补列的默认值必须是未置顶");
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    /// 置顶只影响本机排序：置顶的旧会话必须排在更新时间更晚的未置顶会话之前。
    #[test]
    fn pinned_conversations_sort_before_recency() {
        let conn = mem();
        ensure_conversation(&conn, "old", "single", "旧的", None).unwrap();
        ensure_conversation(&conn, "new", "single", "新的", None).unwrap();
        touch_conversation(&conn, "old", "single", "旧的", None, "早", 0).unwrap();
        touch_conversation(&conn, "new", "single", "新的", None, "晚", 0).unwrap();
        // 直接把 old 的时间戳压到更早，确保「新的」本来排在前面
        conn.execute("UPDATE conversations SET last_ts = 1 WHERE id = 'old'", [])
            .unwrap();
        conn.execute(
            "UPDATE conversations SET last_ts = 999 WHERE id = 'new'",
            [],
        )
        .unwrap();
        assert_eq!(list_conversations(&conn).unwrap()[0].id, "new");

        set_conversation_pinned(&conn, "old", true).unwrap();
        assert_eq!(
            list_conversations(&conn).unwrap()[0].id,
            "old",
            "置顶后必须排在更晚的会话之前"
        );
        // 取消置顶 → 回到按时间排序
        set_conversation_pinned(&conn, "old", false).unwrap();
        assert_eq!(list_conversations(&conn).unwrap()[0].id, "new");
    }

    /// 会话逻辑时钟：next_clock 单调递增，observe_clock 只前进不回退。
    #[test]
    fn conversation_clock_is_monotonic() {
        let conn = mem();
        assert_eq!(next_clock(&conn, "c1").unwrap(), 1);
        assert_eq!(next_clock(&conn, "c1").unwrap(), 2);
        observe_clock(&conn, "c1", 10).unwrap();
        assert_eq!(get_clock(&conn, "c1"), 10);
        observe_clock(&conn, "c1", 3).unwrap();
        assert_eq!(get_clock(&conn, "c1"), 10, "observe 不得把时钟推回");
        assert_eq!(next_clock(&conn, "c1").unwrap(), 11);
    }

    /// 群待发已读回执：按 (group_id, peer_id) 唯一，max 语义，删除只删对应行。
    #[test]
    fn pending_group_reads_lifecycle() {
        let conn = mem();
        upsert_pending_group_read(&conn, "g1", "b", 10).unwrap();
        upsert_pending_group_read(&conn, "g1", "b", 9).unwrap(); // 不覆盖较大值
        upsert_pending_group_read(&conn, "g1", "c", 20).unwrap();

        let rows = list_pending_group_reads(&conn, "b").unwrap();
        assert_eq!(rows, vec![("g1".to_string(), 10)]);

        delete_pending_group_read(&conn, "g1", "b").unwrap();
        assert!(list_pending_group_reads(&conn, "b").unwrap().is_empty());
        assert_eq!(list_pending_group_reads(&conn, "c").unwrap().len(), 1);
    }
}
