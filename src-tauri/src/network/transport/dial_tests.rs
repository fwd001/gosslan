// 职责边界：
// - transport 行为测试分册之3 —— 主动建链的拨号判据（大小 ID 对称、退避、同路已连通则不拨）
// 为什么拆：`transport/tests.rs` 原来 3,227 行、5 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    #[test]
    fn should_dial_larger_id_dials_when_not_connected() {
        // 本机是大 ID（my_id > peer_id）：尚无任何连接时，作为对称场景的确定性拨号方，恒拨。
        assert!(should_dial("b", "a", false, false, None, 0));
        assert!(should_dial("b", "a", false, false, Some(0), 0));
    }

    #[test]
    fn should_dial_smaller_id_waits_within_threshold() {
        // 本机是小 ID：对端在线但「首次发现」未超过 10s，不拨（等大 ID 拨）。
        let now = 1_000_000;
        assert!(!should_dial("a", "b", false, false, Some(now - 9_000), now));
        assert!(!should_dial("a", "b", false, false, Some(now), now));
        // 对端尚未在线（first_seen=None）：不拨。
        assert!(!should_dial("a", "b", false, false, None, now));
    }

    #[test]
    fn should_dial_smaller_id_backups_after_threshold() {
        // 本机是小 ID：对端在线却超过 10s 连不上（单侧不可达），兜底拨。
        let now = 1_000_000;
        assert!(should_dial("a", "b", false, false, Some(now - 10_000), now));
        assert!(should_dial("a", "b", false, false, Some(now - 60_000), now));
    }

    /// P1-2 修正的**核心护栏**。
    ///
    /// 被动方（小 ID）已经收到过大 ID 拨来的连接时，绝不能因为「端点表示不对称」
    /// （接受侧 `Link.endpoint` 记的是 TCP 源**临时端口**，而判据比的是 announce 自报的
    /// **监听地址**）而反向再拨一条 —— 那会让同一对节点稳定停留 2 条镜像 TCP，
    /// 连接与读写任务翻倍、心跳双份，并让「断一条仍在线」的判据变成假阳性。
    ///
    /// 注意判据是 `has_lan_link`（**同路径**已连通），与 `has_endpoint` 无关：
    /// 这里刻意传 `has_endpoint=false`（真实场景就是如此）来钉住「只按端点判会误拨」。
    #[test]
    fn should_dial_skips_when_same_path_already_connected() {
        let now = 1_000_000;
        // 小 ID + 已有连接 + 早已超过 10s 阈值 —— 旧实现正是在这里误判为「该兜底拨号」。
        assert!(!should_dial("a", "b", false, true, Some(now - 60_000), now));
        // 大 ID 同理：已有连接不重复拨。
        assert!(!should_dial("b", "a", false, true, Some(now - 60_000), now));
        // 同一端点已连 → 不拨（无论 ID 大小、无论阈值）。
        assert!(!should_dial("b", "a", true, true, None, now));
        assert!(!should_dial("a", "b", true, true, Some(now - 60_000), now));
    }

    /// D5 护栏：判据必须是「**LAN 路径**是否已连通」，不能是「有没有任意连接」。
    ///
    /// 回归场景（复核抓到）：peer 先经 Routed/Tailscale 连上，之后 LAN 的 announce 到达；
    /// 若把任意连接当成「已连通」，LAN 链路**永远不会建立** ⇒ M3 的「LAN > Routed」
    /// 优先级在该拓扑里永不生效，多路径退化成单路径。
    /// 这条测试会在把判据回退成「任意连接」时 FAIL —— 因为它明确区分了两种端点。
    #[test]
    fn only_routed_connection_still_dials_lan_path() {
        let lan: MeshEndpoint = "192.168.1.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();
        let routed: MeshEndpoint = "100.70.10.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();

        // 纯函数内核：只有 Routed 端点 ⇒ 不算 LAN 已连通（⇒ 大 ID 会去补一条 LAN）
        // （`Endpoint` 不是 Copy，测试里 clone 保持可读性）
        assert!(!has_lan_path(&[(routed.clone(), PathKind::Routed)]));
        assert!(has_lan_path(&[(lan.clone(), PathKind::Lan)]));
        assert!(has_lan_path(&[
            (routed.clone(), PathKind::Routed),
            (lan.clone(), PathKind::Lan)
        ]));
        assert!(!has_lan_path(&[]));
        // ⚠️ **关键回归**：端点地址是私有段、但来路是"用户配置的路由端点" ⇒ 仍是 Routed。
        // 此前路径类型是从 IP 段反推的（`path_kind_for`），这条必然被判成 LAN ⇒
        // ① `ensure_link` 以为 LAN 已连通、不再补真正的 LAN 链路（D5 的修复被绕过去）；
        // ② 选路时按最高优先级当成 LAN。把判据改回"按 IP 反推"这条断言立刻 FAIL。
        let private_but_routed: MeshEndpoint = "192.168.1.77:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();
        assert!(
            !has_lan_path(&[(private_but_routed.clone(), PathKind::Routed)]),
            "用户配置的私有段 Routed 端点不得被当成 LAN"
        );

        // 决策层：**只有 Routed 连接**时，大 ID 仍应去补一条 LAN —— 这正是修复点。
        // 若把判据回退成「有任意连接就不拨」，下面这条断言会 FAIL（非空转）。
        let now = 1_000_000;
        let routed_only = [(routed.clone(), PathKind::Routed)];
        let lan_only = [(lan.clone(), PathKind::Lan)];
        let both = [
            (routed.clone(), PathKind::Routed),
            (lan.clone(), PathKind::Lan),
        ];
        assert!(should_dial_for_peer(
            "b",
            "a",
            false,
            &routed_only,
            Some(now - 60_000),
            now
        ));
        // 已有 LAN 连接 ⇒ 不重复拨（避免镜像重复连接，P1-2）。
        assert!(!should_dial_for_peer(
            "b",
            "a",
            false,
            &lan_only,
            Some(now - 60_000),
            now
        ));
        // 已有 LAN（含还有一条 Routed 的多路径场景）⇒ 也不拨。
        assert!(!should_dial_for_peer(
            "b",
            "a",
            false,
            &both,
            Some(now - 60_000),
            now
        ));
        // 完全没有连接 ⇒ 拨（原语义不变）。
        assert!(should_dial_for_peer(
            "b",
            "a",
            false,
            &[],
            Some(now - 60_000),
            now
        ));
        // 小 ID 兜底：只有 Routed 且超过阈值 ⇒ 也去补 LAN。
        assert!(should_dial_for_peer(
            "a",
            "b",
            false,
            &routed_only,
            Some(now - 60_000),
            now
        ));
        // 私有段地址 + Routed 来路 ⇒ 仍应补 LAN（与上面的关键回归同一件事，走决策层）
        assert!(should_dial_for_peer(
            "b",
            "a",
            false,
            &[(private_but_routed.clone(), PathKind::Routed)],
            Some(now - 60_000),
            now
        ));
    }

    // ---- M3-b：发送顺序（按端点对齐两套链路表 + pick_link 排序）----
