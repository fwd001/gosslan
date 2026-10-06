// 职责边界：
// - transport 行为测试分册之2 —— peers 表在线判定 + 成员 X25519 公钥解析与好友接受时的补绑
// 为什么拆：`transport/tests.rs` 原来 3,227 行、5 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// 「这个 peer 到底离线了没有」的真值表（#162）。
    ///
    /// 旧写法是 `removed && empty`，而**半开看门狗**会先把整条 key 摘掉
    /// （`transport.rs` 的看门狗那段：`if v.is_empty() { links.remove(&peer); }`），
    /// 于是读循环收尾时 `removed=false`、`empty=true` ⇒ 判成"没离线"：
    /// 徽标继续显示在线、在途的接收要等 5 分钟空闲超时才失败、也不打掉线日志。
    /// 正确的定义只有一件事可判：**这个 peer 一条链路都不剩了没有**。
    #[test]
    fn peer_offline_after_tail_truth_table() {
        // 我摘掉了它最后一条链路 ⇒ 离线
        assert!(super::peer_offline_after_tail(true, true));
        // 别人（半开看门狗）先摘了 key，我这边只是没摘到东西 ⇒ 仍然是离线（本轮修的就是这一格）
        assert!(
            super::peer_offline_after_tail(false, true),
            "链路已空却不点亮离线 ⇒ 徽标停在在线、在途接收卡在 5 分钟超时"
        );
        // 还剩别的链路（LAN + Tailscale + BLE 并存）⇒ 不许判离线，那是 failover
        assert!(!super::peer_offline_after_tail(true, false));
        assert!(!super::peer_offline_after_tail(false, false));
    }

    /// 那条判据只许有一个家：读循环收尾必须调用它，不许再内联一份 `removed && empty`。
    #[test]
    fn peer_offline_rule_has_one_home() {
        let src = crate::network::transport_src_for_guards();
        assert!(
            src.contains("peer_offline_after_tail(removed, empty)"),
            "读循环收尾不再调用那个判据 ⇒ 真值表与现场分家"
        );
        assert!(
            !src.contains("let offline = removed && empty;"),
            "transport.rs 里又出现内联的第二份判据"
        );
    }
    /// 群密钥分发的公钥选择规则：peers 表优先（announce/Hello 实时维护）、
    /// friends 表回落、两边都缺才返回 None（安全跳过）。
    #[test]
    fn pick_member_x25519_prefers_peers_then_friends() {
        // peers 有 → 用 peers（即使 friends 也有）
        assert_eq!(
            pick_member_x25519(Some("pk-peer".into()), Some("pk-friend".into())).as_deref(),
            Some("pk-peer")
        );
        // peers 无、friends 有 → 回落 friends
        assert_eq!(
            pick_member_x25519(None, Some("pk-friend".into())).as_deref(),
            Some("pk-friend")
        );
        // 两边都无 → None（调用方 continue 安全跳过）
        assert_eq!(pick_member_x25519(None, None), None);
    }

    /// respond_friend_request accept 路径的公钥补写（对齐 FriendAccept 接收路径）：
    /// add_friend 时不带公钥 → maybe_update_friend 从 peers 补写 → friends 可查。
    /// 此前 accept 方不补写且 key_changed 不再触发 → 公钥永久缺失 →
    /// 群密钥分发 continue 静默跳过（B 后加群收不到的根因）。
    #[test]
    fn friend_accept_backfills_pubkeys_from_peers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // accept：add_friend 不带公钥（commands.rs respond_friend_request 现状）
        db::add_friend(&conn, "b", "Bob", None).unwrap();
        assert!(
            db::get_friend_x25519(&conn, "b").is_none(),
            "accept 后 friends 公钥应为空（复现补写前状态）"
        );
        // maybe_update_friend 的补写行为：peers 表已有公钥 → 写入 friends
        db::update_friend_pubkeys(&conn, "b", Some("xk-b"), Some("ek-b")).ok();
        assert_eq!(
            db::get_friend_x25519(&conn, "b").as_deref(),
            Some("xk-b"),
            "补写后群密钥分发必须能取到公钥"
        );
        // 重复补写幂等（maybe_update_friend 每次都可能调用）
        db::update_friend_pubkeys(&conn, "b", Some("xk-b"), Some("ek-b")).ok();
        assert_eq!(db::get_friend_x25519(&conn, "b").as_deref(), Some("xk-b"));
    }

    /// 成为好友那一刻：**加密钥匙照绑，身份锚点只认被证明过的来源**（#32 这一片的核心判据）。
    ///
    /// 为什么值得单独钉：`friends.ed25519_pubkey` 此后既是 Hello 的验签锚点（INV-P21）、
    /// 安全码的输入，又是**公网中继电路的准入判据**（`list_bound_friend_identities` 只看它
    /// 非空），而 `update_friend_pubkeys` 只填空、首写者永久胜出 —— 一次伪造的 UDP announce
    /// 抢先写进来就是永久的。反过来留 NULL 不是死路，见下面第三段。
    #[test]
    fn accept_binds_encryption_key_but_defers_unverified_anchor() {
        let (x, e) = acceptable_friend_keys(false, Some("xk-b".into()), Some("ek-evil".into()));
        assert_eq!(
            x.as_deref(),
            Some("xk-b"),
            "加密钥匙必须照旧早绑，否则首次加密发送失败（那三条路径原本各写一遍的理由）"
        );
        assert_eq!(
            e, None,
            "未 verified 的来源不得成为身份锚点：写进去就再也改不掉"
        );

        let (x2, e2) = acceptable_friend_keys(true, Some("xk-b".into()), Some("ek-b".into()));
        assert_eq!(
            (x2, e2),
            (Some("xk-b".into()), Some("ek-b".into())),
            "验签通过的来源两列都照绑"
        );

        // 留 NULL 的自愈路径：这一轮只绑到加密钥匙，下一轮一次 verified 的写入补上锚点，
        // 且不许把先绑上的 x25519 换掉（两列各自 COALESCE，互不牵连）。
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "b", "Bob", None).unwrap();
        db::update_friend_pubkeys(&conn, "b", Some("xk-b"), None).ok();
        assert!(
            db::get_friend_ed25519(&conn, "b").is_none(),
            "未 verified 的这一轮就该没有锚点"
        );
        db::update_friend_pubkeys(&conn, "b", Some("xk-other"), Some("ek-b")).ok();
        assert_eq!(db::get_friend_ed25519(&conn, "b").as_deref(), Some("ek-b"));
        assert_eq!(
            db::get_friend_x25519(&conn, "b").as_deref(),
            Some("xk-b"),
            "先绑上的加密钥匙不许被后来的值改掉（fill-only 是最后一道闸）"
        );
    }

    // ---------------- P0：TCP 监听端口生命周期（生产 1.0 重启掉线） ----------------
