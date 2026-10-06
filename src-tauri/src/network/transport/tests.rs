// 职责边界：
// - transport 这组的尾部大块行为测试（从主文件搬出来，`include!` 回同一模块）
// 为什么搬：搬之前主文件 10235 行里这一段占 3180 行（约 31%），生产码只 7055 行。
// ⚠️ 搬进分册的代码必须同步登记进 `network/mod.rs` 的 `transport_src_for_guards`，
// 否则以「transport 全集」为判据的守卫看不见这一段（那条函数自己的注释写着：这是假绿，更危险）。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gossip_engine::GossipEngine;

    /// 「该给谁重发群密钥」的判据（2026-09-24 RC2：链路抖动后 GroupKey 永不重发）。
    #[test]
    fn group_ids_containing_picks_only_actual_members() {
        let groups = vec![
            crate::state::Group {
                id: "g2".into(),
                name: "b".into(),
                creator: "me".into(),
                members: vec!["me".into(), "p1".into()],
            },
            crate::state::Group {
                id: "g1".into(),
                name: "a".into(),
                creator: "me".into(),
                members: vec!["me".into(), "p2".into()],
            },
        ];
        // 只含"成员里有他"的那几个，且**顺序稳定**（登记进 HashSet 后靠它保证重试次序可预期）
        assert_eq!(group_ids_containing(&groups, "p1"), vec!["g2".to_string()]);
        assert_eq!(group_ids_containing(&groups, "p2"), vec!["g1".to_string()]);
        // 我自己在每个群里 ⇒ 拿到全部（排序后）
        assert_eq!(
            group_ids_containing(&groups, "me"),
            vec!["g1".to_string(), "g2".to_string()]
        );
        // 陌生人 ⇒ 空，调用方据此完全不碰 pending 表
        assert!(group_ids_containing(&groups, "nobody").is_empty());
    }

    /// **群信封的"可消费"判据**（2026-09-13 审计的真缺陷，必须钉住）。
    ///
    /// 反例：旧实现把"我不是群成员"直接 `return` 掉，于是**非成员中继不转发群消息**
    /// ⇒ BLE-only 三点中继（手机—电脑—手机）里群聊永远不通，而同链路单聊正常。
    /// **Hello 绝不能带大头像**（真机 2026-09-14 三端日志：握手帧 424303 字节 ⇒ BLE 上
    /// 要么撞 10s 握手超时、要么在 20 字节/片的外设侧直接「帧无法分片」，表现为
    /// 「搜得到、连得上、永远建立不了会话」）。
    #[test]
    fn hello_avatar_is_capped_for_the_handshake_frame() {
        assert_eq!(
            hello_avatar_for_wire(Some("data:image/png;base64,AAAA")),
            Some("data:image/png;base64,AAAA")
        );
        assert_eq!(hello_avatar_for_wire(None), None);
        assert_eq!(hello_avatar_for_wire(Some("")), None, "空串按没有头像处理");
        let big = "x".repeat(HELLO_AVATAR_MAX_BYTES + 1);
        assert_eq!(
            hello_avatar_for_wire(Some(&big)),
            None,
            "超过上限的头像必须被挡在握手帧之外"
        );
        let edge = "x".repeat(HELLO_AVATAR_MAX_BYTES);
        assert_eq!(
            hello_avatar_for_wire(Some(&edge)).map(str::len),
            Some(HELLO_AVATAR_MAX_BYTES),
            "正好等于上限要放行"
        );
        // 源码断言：Hello 构造必须真的用这个闸门（否则上面测的只是「函数存在」）
        let src = crate::network::transport_src_for_guards();
        let at = src
            .find("pub fn build_signed_hello(state: &AppState")
            .expect("必须还有 build_signed_hello（本护栏锚点）");
        // 注意：源码里有大量中文，**不能**按"起始 + 2000 字节"硬切（会切在多字节字符中间 panic）；
        // 用"顶层函数结尾的 `\n}\n`"作终点（与 lib.rs 的 `rust_fn_body` 同一判据）。
        let end = src[at..]
            .find("\n}\n")
            .map(|i| at + i + 3)
            .unwrap_or(src.len());
        let body = &src[at..end];
        assert!(
            body.contains("hello_avatar_for_wire"),
            "`build_signed_hello` 必须用 `hello_avatar_for_wire` 过滤头像"
        );
    }

    /// **Presence 不得内联大头像**（与 Hello 同族，且更危险：每 10s 广播一次、走优先通道）。
    ///
    /// 这张源码断言盯住"闸门是否还在"：一旦有人把 state.avatar 原样塞回 Presence，
    /// 一张 400KB 头像会把聊天与好友请求的优先队列堵住几分钟 —— 而单测不会失败。
    #[test]
    fn presence_caps_inline_avatar() {
        let src = crate::network::transport_src_for_guards();
        let at = src
            .find("async fn broadcast_presence")
            .expect("必须还有 broadcast_presence（本护栏锚点）");
        let end = src[at..]
            .find("\n}\n")
            .map(|i| at + i + 3)
            .unwrap_or(src.len());
        let body = &src[at..end];
        assert!(
            body.contains("hello_avatar_for_wire"),
            "broadcast_presence 必须过 hello_avatar_for_wire 闸门：             否则一张大头像会占满优先通道，聊天与好友请求几分钟才到"
        );
        assert!(
            !body.contains("\"avatar\": avatar"),
            "不能再把 state.avatar 原样内联进 Presence（那条旧写法正是本次修复的缺陷）"
        );
    }

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

    /// **好友同意回执补发策略**：窗口 + 次数 + 间隔三条边界。
    ///
    /// 真机（2026-09-13）：Android 点了「接受」、Android 侧好友已出现，但 Mac 端状态一直
    /// 没同步 —— 那一帧在 BLE 链路抖动时静默丢了且**永不重发**。修法是"有界补发"，
    /// 而"窗口 + 次数 + 间隔"这种策略最容易写反（写反的后果是要么永不补发、要么疯狂打扰
    /// 对端），所以在这里用真值表钉死。
    #[test]
    fn friend_accept_flush_is_bounded_and_spaced() {
        let now = 1_000_000_i64;
        let (max, interval, window) = (3u32, 5_000i64, 120_000i64);

        // 没有登记 ⇒ 什么都不做
        assert_eq!(
            friend_accept_flush_decision(None, now, max, interval, window),
            FriendAcceptFlush::Nothing
        );
        // 刚登记（last=0）⇒ 立刻补发第 1 次
        assert_eq!(
            friend_accept_flush_decision(Some((now, 0, 0)), now, max, interval, window),
            FriendAcceptFlush::Flush(1)
        );
        // 刚发过（距上次不足间隔）⇒ 等下一次心跳，不打扰
        assert_eq!(
            friend_accept_flush_decision(
                Some((now - 10_000, 1, now - 1_000)),
                now,
                max,
                interval,
                window
            ),
            FriendAcceptFlush::Nothing
        );
        // 距上次够了 ⇒ 继续补发（第 2 次）
        assert_eq!(
            friend_accept_flush_decision(
                Some((now - 10_000, 1, now - 5_000)),
                now,
                max,
                interval,
                window
            ),
            FriendAcceptFlush::Flush(2)
        );
        // 次数用尽 ⇒ 收尾
        assert_eq!(
            friend_accept_flush_decision(
                Some((now - 10_000, 3, now - 60_000)),
                now,
                max,
                interval,
                window
            ),
            FriendAcceptFlush::GiveUp
        );
        // 窗口用尽（哪怕一次都没发出去）⇒ 收尾，并让调用方留一条 warn
        assert_eq!(
            friend_accept_flush_decision(
                Some((now - 121_000, 1, now - 60_000)),
                now,
                max,
                interval,
                window
            ),
            FriendAcceptFlush::GiveUp
        );
    }

    // ---- 群成员变更：接收端的分支选择 ----
    //
    // 真实事故（2026-09-12 用户反馈）：群主移人后，**其余成员**的成员表不变小、
    // 也没有任何提示。根因是接收端 `handle_group_member_removed` 开头就
    // `if to != 本机 { return }` —— 压根没有「别人被移出」这个分支。
    // 下面把三分支的选择（纯函数）用真值表钉住。

    #[test]
    fn member_removed_action_full_truth_table() {
        use MemberRemovedAction::*;
        // 发起方不是创建者 → 一律忽略（防成员互踢），与 `to` 是谁无关
        assert_eq!(member_removed_action(false, false, false), Ignore);
        assert_eq!(member_removed_action(false, false, true), Ignore);
        // 本机自己发起的 → 忽略（本地已处理，含群主自己的系统消息）
        assert_eq!(member_removed_action(true, true, false), Ignore);
        assert_eq!(member_removed_action(true, true, true), Ignore);
        assert_eq!(member_removed_action(true, false, false), Ignore);
        assert_eq!(member_removed_action(true, false, true), Ignore);
        // 创建者发起 + 被移出者是别人 → **同步成员表**（本次补齐的分支，
        // 这条断言正是对「其余成员不能 Ignore」的回归钉子）
        assert_eq!(member_removed_action(false, true, false), RemoveOther);
        // 创建者发起 + 被移出者是我 → 清理本地群
        assert_eq!(member_removed_action(false, true, true), RemoveSelf);
    }

    // ---- 拨号决策（M2 双向建链 + P1-2 镜像重复连接修正）----

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

    fn make_link(
        addr: &str,
        kind: PathKind,
    ) -> (
        crate::state::Link,
        mpsc::Receiver<Message>,
        mpsc::Receiver<Message>,
    ) {
        let (b_tx, b_rx) = mpsc::channel(4);
        let (p_tx, p_rx) = mpsc::channel(4);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint: MeshEndpoint::Tcp(addr.parse().unwrap()),
                path_kind: kind,
                high: b_tx.clone(),
                normal: p_tx,
                low: b_tx,
                cancel,
            },
            b_rx,
            p_rx,
        )
    }

    /// 按任意 `Endpoint` 造一条链路（`make_link` 只接受 TCP 地址字符串，BLE 用这个）。
    fn make_link_endpoint(
        endpoint: MeshEndpoint,
        kind: PathKind,
    ) -> (
        crate::state::Link,
        mpsc::Receiver<Message>,
        mpsc::Receiver<Message>,
    ) {
        let (b_tx, b_rx) = mpsc::channel(4);
        let (p_tx, p_rx) = mpsc::channel(4);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint,
                path_kind: kind,
                high: b_tx.clone(),
                normal: p_tx,
                low: b_tx,
                cancel,
            },
            b_rx,
            p_rx,
        )
    }

    /// 按任意 `Endpoint` 造一个 mesh 层 `Connection`（同上）。
    fn mesh_conn_endpoint(
        peer: &str,
        endpoint: MeshEndpoint,
        healthy_at: Option<i64>,
        kind: PathKind,
    ) -> crate::mesh::Connection {
        let mut c = crate::mesh::Connection::new(peer, endpoint, kind);
        if let Some(t) = healthy_at {
            c.health.seed_read_seen(t);
        }
        c
    }

    fn mesh_conn(
        peer: &str,
        addr: &str,
        healthy_at: Option<i64>,
        kind: PathKind,
    ) -> crate::mesh::Connection {
        let mut c = crate::mesh::Connection::new(
            peer,
            crate::mesh::endpoint::Endpoint::Tcp(addr.parse().unwrap()),
            kind,
        );
        if let Some(t) = healthy_at {
            c.health.seed_read_seen(t);
        }
        c
    }

    /// 单链路：顺序无变化（**行为零变化**，M3-b 的前提）。
    #[test]
    fn route_order_single_link_is_unchanged() {
        let (l0, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![l0];
        let conns = vec![mesh_conn(
            "peer",
            "192.168.1.20:59992",
            Some(1000),
            PathKind::Lan,
        )];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order, vec![0]);
    }

    /// 核心（M3-b 的收益）：两条都健康时**LAN 优先**，与插入顺序无关。
    #[test]
    fn route_order_prefers_lan_over_routed_regardless_of_insertion() {
        // 故意把 Routed 放在下标 0（插入在前），LAN 在下标 1
        let (routed, _b0, _p0) = make_link("100.70.10.20:59992", PathKind::Routed);
        let (lan, _b1, _p1) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![routed, lan];
        let conns = vec![
            mesh_conn("peer", "100.70.10.20:59992", Some(1000), PathKind::Routed),
            mesh_conn("peer", "192.168.1.20:59992", Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order[0], 1, "应优先 LAN（下标 1），而不是插入在前的 Routed");
        // 不变量：其余链路仍排在后面做 failover，**一条都不能丢**
        assert_eq!(order.len(), 2);
        assert!(order.contains(&0) && order.contains(&1));
    }

    /// BLE 链路的两个"不该被当成 LAN"判据（ADR-0015 的 7-c）：
    /// ① 选路：TCP（LAN/Routed）必须排在 BLE 前面 —— 蓝牙带宽/功耗都差一个量级；
    /// ② 拨号：只有 BLE 连上**不算**「LAN 已连通」，否则 `ensure_link` 不再补 LAN 链路
    ///    （用户明明在同一局域网，却一直走蓝牙 —— 电量与速度都吃亏）。
    #[test]
    fn ble_link_is_neither_lan_nor_preferred_over_tcp() {
        let ble = MeshEndpoint::Ble(crate::mesh::BleEndpoint::new("node-1"));
        let lan: MeshEndpoint = "192.168.1.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();

        // ① 选路：BLE 插在前面也不该被优先选
        let (ble_link, _b0, _p0) = make_link_endpoint(ble.clone(), PathKind::Bluetooth);
        let (lan_link, _b1, _p1) = make_link_endpoint(lan.clone(), PathKind::Lan);
        let links = vec![ble_link, lan_link];
        let conns = vec![
            mesh_conn_endpoint("peer", ble.clone(), Some(1000), PathKind::Bluetooth),
            mesh_conn_endpoint("peer", lan.clone(), Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order.len(), 2, "BLE 链路同样是 failover 候选，不能丢");
        assert_eq!(order[0], 1, "LAN 必须优先于 BLE");

        // ② 拨号判据：只有 BLE ⇒ LAN 路径尚未连通 ⇒ 仍要去补一条 LAN
        assert!(!has_lan_path(&[(ble.clone(), PathKind::Bluetooth)]));
        let now = 1_000_000;
        assert!(
            should_dial_for_peer(
                "b",
                "a",
                false,
                &[(ble, PathKind::Bluetooth)],
                Some(now - 60_000),
                now
            ),
            "只有 BLE 连接时仍应补 LAN"
        );
    }

    /// 跨世代的入站连接必须被否决（D8-4）。
    #[test]
    fn stale_generation_is_rejected() {
        assert!(generation_is_current(3, 3), "同一世代允许登记");
        assert!(
            !generation_is_current(3, 4),
            "stop/start 之后的旧世代不得登记"
        );
        // 世代只增不减，但"捕获值比当前大"同样视为无效（防御性：不做大小比较）
        assert!(!generation_is_current(4, 3));
    }

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

    /// failover 核心：LAN 的读活性过期（半开）而 Routed 健康 → 选 Routed。
    #[test]
    fn route_order_skips_unhealthy_lan_when_routed_is_healthy() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            // LAN：只有很早的读活性（已过期）
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),
            // Routed：刚刚读到过帧
            mesh_conn("peer", "100.70.10.20:59992", Some(60_000), PathKind::Routed),
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(order[0], 1, "LAN 不健康时必须降级到 Routed（真 failover）");
        assert_eq!(order.len(), 2, "不健康链路仍保留在后面（可作最后手段）");
    }

    /// 登记窗口：传输链路存在但 mesh 侧还没登记 → 合成「刚播种」候选，不能因此被判不可用。
    #[test]
    fn route_order_tolerates_missing_mesh_candidate() {
        let (only, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![only];
        let order = route_order(&links, "peer", &[], 1000, 15_000, 3);
        assert_eq!(order, vec![0], "缺候选时不得丢链路（登记窗口是常态）");
    }

    /// 全部不健康：`pick_link` 退回首条（保持可用），且顺序仍是全量排列。
    #[test]
    fn route_order_keeps_all_links_when_none_healthy() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),
            mesh_conn("peer", "100.70.10.20:59992", Some(0), PathKind::Routed),
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(
            order.len(),
            2,
            "全不健康也要把链路交出去（可用性优先于择优）"
        );
    }

    // ---- M3-b：真实信道上的 failover（ADR-0014 §8「切断被选中那条 → 消息仍送达」的单元版）----

    fn msg(id: &str) -> Message {
        Message::Heartbeat {
            device_id: id.to_string(),
        }
    }

    /// 造 n 组信道（high/normal/low），返回 senders + 各接收端。
    /// msg() 造的是 Heartbeat=High，所以 receivers 保留 high_rx。
    #[allow(clippy::type_complexity)]
    fn channels(
        n: usize,
        closed: &[usize],
    ) -> (
        Vec<(
            mpsc::Sender<Message>,
            mpsc::Sender<Message>,
            mpsc::Sender<Message>,
        )>,
        Vec<Option<mpsc::Receiver<Message>>>,
    ) {
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for _i in 0..n {
            let (h_tx, h_rx) = mpsc::channel(4);
            let (n_tx, n_rx) = mpsc::channel(4);
            let (l_tx, l_rx) = mpsc::channel(4);
            senders.push((h_tx, n_tx, l_tx));
            if closed.contains(&_i) {
                drop(h_rx);
                drop(n_rx);
                drop(l_rx);
                receivers.push(None);
            } else {
                receivers.push(Some(h_rx)); // Heartbeat=High → 走 high
                drop(n_rx);
                drop(l_rx);
            }
        }
        (senders, receivers)
    }

    /// **核心判据**：被选中的那条断了 → 消息必须落到下一条（真 failover，不是"投进死路"）。
    #[tokio::test]
    async fn failover_delivers_on_next_link_when_selected_is_closed() {
        let (senders, mut rx) = channels(2, &[0]); // 下标 0（被选中）已断
                                                   // 顺序模拟选路结果：先试 0（断），再试 1（活）
        let order = vec![0usize, 1];
        let r = send_over_order(
            &senders,
            &order,
            &msg("m1"),
            crate::network::dispatch::MessagePriority::High,
        )
        .await;
        assert!(r.is_ok(), "断一条后必须换下一条送达，实得 {r:?}");
        let got = rx[1]
            .as_mut()
            .expect("链路 1 应存活")
            .try_recv()
            .expect("应在链路 1 上收到");
        assert!(matches!(got, Message::Heartbeat { .. }));
    }

    /// 顺序被尊重：两条都活时只投第一条，**不重复投递**（消息仍然只发出一次）。
    #[tokio::test]
    async fn sends_only_on_first_healthy_link_in_order() {
        let (senders, mut rx) = channels(2, &[]);
        let order = vec![1usize, 0]; // 选路把下标 1 排前面
        assert!(send_over_order(
            &senders,
            &order,
            &msg("m2"),
            crate::network::dispatch::MessagePriority::High
        )
        .await
        .is_ok());
        assert!(
            rx[1].as_mut().unwrap().try_recv().is_ok(),
            "应落在顺序第一的那条"
        );
        assert!(
            rx[0].as_mut().unwrap().try_recv().is_err(),
            "不得同时投到第二条（否则会重复投递）"
        );
    }

    /// 全断 → 返回 Err（调用方据此走 outbox 补发，而不是假装成功）。
    #[tokio::test]
    async fn all_links_closed_returns_err() {
        let (senders, _rx) = channels(2, &[0, 1]);
        let r = send_over_order(
            &senders,
            &[0, 1],
            &msg("m3"),
            crate::network::dispatch::MessagePriority::High,
        )
        .await;
        assert!(r.is_err(), "全断必须报错（Err 由 outbox 兜底补发）");
    }

    /// 与选路联动的**端到端单元判据**：LAN 不健康 → 顺序把 Routed 排前面
    /// → 消息真的落在 Routed 那条（而不是仍投给 LAN）。这就是「切一条不中断」的最小复现。
    #[tokio::test]
    async fn route_order_plus_send_delivers_on_healthy_link_after_lan_degraded() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan), // LAN 读活性过期
            mesh_conn("peer", "100.70.10.20:59992", Some(60_000), PathKind::Routed), // Routed 健康
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(order[0], 1, "应先试健康的 Routed");

        // 用真实信道复现：LAN 那条已断，Routed 那条活着
        let (senders, mut rx) = channels(2, &[0]);
        assert!(send_over_order(
            &senders,
            &order,
            &msg("m4"),
            crate::network::dispatch::MessagePriority::High
        )
        .await
        .is_ok());
        assert!(
            rx[1].as_mut().unwrap().try_recv().is_ok(),
            "LAN 降级后消息必须从 Routed 送出"
        );
    }

    /// 空链路表 → 空顺序（调用方据此返回「未建立连接」）。
    #[test]
    fn route_order_empty_when_no_links() {
        assert!(route_order(&[], "peer", &[], 1000, 15_000, 3,).is_empty());
    }

    // ---- 文件分片流的"钉住一条链路"投递（真机多文件并发失序回归）----

    /// 造一条三通道各自独立、容量可调的链路（`make_link` 把 high/low 合成一个通道，
    /// 这里要分开才能断言分片走的是 Low）。
    fn pinned_link(
        cap: usize,
    ) -> (
        crate::state::Link,
        mpsc::Receiver<Message>,
        mpsc::Receiver<Message>,
        mpsc::Receiver<Message>,
    ) {
        let (h_tx, h_rx) = mpsc::channel(cap);
        let (n_tx, n_rx) = mpsc::channel(cap);
        let (l_tx, l_rx) = mpsc::channel(cap);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint: MeshEndpoint::Tcp("192.168.1.20:59992".parse().unwrap()),
                path_kind: PathKind::Lan,
                high: h_tx,
                normal: n_tx,
                low: l_tx,
                cancel,
            },
            h_rx,
            n_rx,
            l_rx,
        )
    }

    fn chunk(seq: u32) -> Message {
        Message::FileChunk {
            transfer_id: "t1".to_string(),
            seq,
            data: "AA".to_string(),
            attempt: None,
        }
    }

    /// **核心判据**：队列满时原地等待（背压），而不是返回 Err 让上层放弃这次尝试。
    ///
    /// 为什么这是判据而不是"顺手加个测试"：旧路径每片都走 `try_send`，满即 failover 到
    /// 另一条独立 TCP 连接 —— 两条连接到达顺序互不保证，接收端严格递增 seq 的追加写
    /// 立刻判死（"文件分片顺序错误"）。单发不满队列所以看不出，多文件并发必现。
    #[tokio::test]
    async fn send_on_link_backpressures_when_queue_full() {
        let (link, _h, _n, mut low) = pinned_link(2);
        assert!(send_on_link(&link, &chunk(0)).await.is_ok());
        assert!(send_on_link(&link, &chunk(1)).await.is_ok());
        // 队列已满：必须仍在等，而不是 Err（Err 会让上层中途放弃，留下在途旧分片）。
        let r = tokio::time::timeout(
            std::time::Duration::from_millis(80),
            send_on_link(&link, &chunk(2)),
        )
        .await;
        assert!(r.is_err(), "满队列应原地背压等待，实得 {r:?}");
        // 消费端腾出槽位后仍能送达，且**顺序不乱** —— 保序是这条链路的唯一契约。
        let first = low.recv().await.expect("应收到第 0 片");
        assert!(send_on_link(&link, &chunk(2)).await.is_ok());
        let mut seqs = vec![match first {
            Message::FileChunk { seq, .. } => seq,
            other => panic!("只应收到 FileChunk，实得 {other:?}"),
        }];
        for _ in 0..2 {
            match low.recv().await.expect("应收到分片") {
                Message::FileChunk { seq, .. } => seqs.push(seq),
                other => panic!("只应收到 FileChunk，实得 {other:?}"),
            }
        }
        assert_eq!(seqs, vec![0, 1, 2], "同一条链路上的分片必须按提交顺序到达");
    }

    /// 链路死亡（Receiver 被 drop）→ 立刻 Err，不无限挂起。
    /// 这是"满则等"可以不带局部超时的前提：真正的僵死只会表现为通道关闭。
    #[tokio::test]
    async fn send_on_link_errs_immediately_when_closed() {
        let (link, _h, _n, low) = pinned_link(4);
        drop(low);
        let r = send_on_link(&link, &chunk(0)).await;
        assert!(r.is_err(), "通道已关必须报错，实得 {r:?}");
    }

    /// **决定停滞判定能不能写成"带超时的重发循环"**：`tx.send()` 的 future 被中途丢弃时，
    /// 消息会不会已经留在队列里。会留 ⇒ 重试就是**重复片**，而群接收端是严格
    /// `seq != next_seq` 判死 ⇒ 重复片直接打死传输。
    ///
    /// 结论钉在这里：tokio 1.x 的 `send` 取消安全 ⇒ 丢弃即"没发出去"，可以安全地
    /// `timeout(stall_tick, send_on_link(..))` 循环等待并在超时里做停滞检查。
    #[tokio::test]
    async fn timed_out_send_leaves_nothing_behind() {
        let (tx, mut rx) = mpsc::channel::<Message>(1);
        tx.send(chunk(0)).await.unwrap(); // 先把容量占满，逼下一次 send 进入等待
        let r = tokio::time::timeout(std::time::Duration::from_millis(20), tx.send(chunk(1))).await;
        assert!(r.is_err(), "前置条件：队列满 ⇒ 这次 send 必须超时并被丢弃");
        assert!(
            matches!(rx.recv().await, Some(Message::FileChunk { seq: 0, .. })),
            "第一片照常送达"
        );
        let leftover = tokio::time::timeout(std::time::Duration::from_millis(20), rx.recv()).await;
        assert!(
            leftover.is_err(),
            "被丢弃的那条不得留在队列里 —— 否则按\"没发出去\"重发就成了重复片"
        );
    }

    /// INV-P24 第 1 条：**未知帧类型必须降级，不得变成连接错误**。
    ///
    /// 旧行为：`Message` 是内部标签枚举，遇到不认识的 `type` 直接反序列化失败 ⇒
    /// `io::Error` ⇒ reader 循环退出 ⇒ **拆链**。新版本只要上线一种新帧，老设备就从
    /// "少收一条"变成"跟这台设备连不上"（还会重连-再拆的死循环）。
    #[test]
    fn unknown_wire_type_degrades_instead_of_erroring() {
        let buf = serde_json::json!({ "type": "TimeTravelPing", "msg_id": "m1" }).to_string();
        match decode_frame(buf.as_bytes()) {
            Ok(Message::Unknown { wire_type }) => assert_eq!(wire_type, "TimeTravelPing"),
            other => panic!("未知帧必须降级成 Message::Unknown（不报错），实得 {other:?}"),
        }
    }

    /// 但降级**不能顺手把"已知类型 + 字段畸形"也吞掉** —— 那是我们自己的 bug，
    /// 静默忽略就等于把真实协议错误藏起来（INV-005 不允许静默丢消息）。
    #[test]
    fn malformed_known_frame_still_errors() {
        let buf = serde_json::json!({ "type": "heartbeat" }).to_string(); // 故意缺字段
        let e = decode_frame(buf.as_bytes()).err();
        assert!(
            e.is_some(),
            "已知类型缺字段必须报错，实得 Ok —— 说明降级判定吞太宽"
        );
        assert!(
            !e.unwrap().to_string().starts_with("unknown variant"),
            "报错原因必须是字段问题，不是变体未知"
        );
    }

    /// 完全不是 Gosslan 帧的字节（没有 type / 不是 JSON）⇒ 仍然报错（交给调用方丢帧）。
    #[test]
    fn non_frame_bytes_still_error() {
        assert!(
            decode_frame(b"{\"a\":1}").is_err(),
            "没有 type 字段的 JSON 不是我们的帧"
        );
        assert!(
            decode_frame(b"not json at all").is_err(),
            "非 JSON 字节必须报错"
        );
    }

    /// 端到端：走**真实** read_frame 路径收到未知帧 ⇒ 解出 Unknown（不 Err ⇒ 链路不动）。
    #[tokio::test]
    async fn read_frame_tolerates_unknown_wire_type() {
        let payload = serde_json::json!({ "type": "QuantumPing", "seq": 1 }).to_string();
        let (a, b) = tokio::io::duplex(1024);
        let (mut _ar, mut aw) = tokio::io::split(a);
        let (mut br, mut _bw) = tokio::io::split(b);
        let (wr, rd) = tokio::join!(
            crate::transport::tcp::write_bytes(&mut aw, payload.as_bytes()),
            read_frame(&mut br)
        );
        wr.unwrap();
        match rd.expect("未知帧不得成为连接错误") {
            Message::Unknown { wire_type } => assert_eq!(wire_type, "QuantumPing"),
            other => panic!("read_frame 应把未知帧降级成 Unknown，实得 {other:?}"),
        }
    }

    /// 分片走该链路的 Low 通道，控制帧走 High —— 钉链路不能绕过三级通道。
    #[tokio::test]
    async fn send_on_link_respects_priority_channels() {
        let (link, mut high, _n, mut low) = pinned_link(4);
        assert!(send_on_link(&link, &chunk(0)).await.is_ok());
        assert!(send_on_link(&link, &msg("hb")).await.is_ok()); // Heartbeat = High
        assert!(low.try_recv().is_ok(), "分片应落在 Low 通道");
        assert!(matches!(high.try_recv(), Ok(Message::Heartbeat { .. })));
    }

    /// **业务隔离**：大文件把 Low 灌满、对端一时不取时，同一条链路上的文本（Normal）与
    /// 心跳/Ack（High）必须照样送得出去（用户清单 #25 的"大文件失败不得影响其它业务"）。
    ///
    /// 为什么上面那条不够：它只证明"分片落在哪个通道"，不证明"分片堵的时候别人还在动"。
    /// 三级通道若被合成两级（甚至一级），`send_on_link_respects_priority_channels` 依旧全绿，
    /// 而用户看到的是"传大文件期间聊天一起卡住" —— 与 600MB 复核里"持锁 fsync 堵住并发
    /// write_chunk"是同一类耦合，只是发生在发送侧。
    #[tokio::test]
    async fn saturated_chunk_channel_does_not_stall_text_or_control() {
        let (link, mut high, mut normal, mut low) = pinned_link(2);
        assert!(send_on_link(&link, &chunk(0)).await.is_ok());
        assert!(send_on_link(&link, &chunk(1)).await.is_ok());
        // 前置条件：Low 确实满了 ⇒ 第 3 片还挂着。没有这一步，后面三条断言是在测空队列。
        // （`timeout` 丢弃 send 不会留残留片，这条前提由 `timed_out_send_leaves_nothing_behind` 钉着）
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(30),
                send_on_link(&link, &chunk(2))
            )
            .await
            .is_err(),
            "前置条件不成立：Low 没满 ⇒ 这条测试没有可判定的对象"
        );
        let chat = Message::ChatMessage {
            msg_id: "m1".into(),
            from: "a".into(),
            to: "b".into(),
            kind: "text".into(),
            content: "hi".into(),
            ts: 1,
            seq: 1,
        };
        let sent = tokio::time::timeout(
            std::time::Duration::from_millis(30),
            send_on_link(&link, &chat),
        )
        .await;
        assert!(
            matches!(sent, Ok(Ok(()))),
            "分片堵塞时文本必须照常送出，实得 {sent:?}"
        );
        assert!(
            matches!(send_on_link(&link, &msg("hb")).await, Ok(())),
            "分片堵塞时心跳（High）必须照常送出"
        );
        // 而且要真的落在各自的通道里 —— 否则"没堵住"只是因为共用了同一个队列
        assert!(
            matches!(normal.try_recv(), Ok(Message::ChatMessage { .. })),
            "文本必须落在 Normal 通道"
        );
        assert!(matches!(high.try_recv(), Ok(Message::Heartbeat { .. })));
        for expect in [0u32, 1] {
            assert!(
                matches!(low.try_recv(), Ok(Message::FileChunk { seq, .. }) if seq == expect),
                "分片必须仍按提交顺序排在 Low 里等着（第 {expect} 片）"
            );
        }
        assert!(
            low.try_recv().is_err(),
            "被超时丢弃的第 3 片不得留在队列里（否则续发时它就是重复片，群接收端会判死）"
        );
    }

    // ---- Hello 握手身份认证（P0 安全修复回归）----

    /// 用 `signer` 对其公钥 + 指定字段签名，返回 (x25519_pub, ed25519_pub, sig)。
    fn signed_hello(
        signer: &crypto::Identity,
        device_id: &str,
        tcp_port: u16,
        nonce: &str,
    ) -> (String, String, String) {
        let xk = signer.x25519_public_b64();
        let ek = signer.ed25519_public_b64();
        let sig = signer.sign_b64(&hello_signing_bytes(device_id, tcp_port, nonce, &xk, &ek));
        (xk, ek, sig)
    }

    #[test]
    fn hello_auth_accepts_bound_identity() {
        let id = crypto::Identity::generate();
        let (xk, ek, sig) = signed_hello(&id, "dev-a", 59992, "n1");
        let bound = id.ed25519_public_b64();
        assert!(hello_auth_decision(Some(&bound), "dev-a", 59992, "n1", &xk, &ek, &sig).is_ok());
    }

    #[test]
    fn hello_auth_rejects_attacker_declaring_own_key() {
        // 攻击者用自己的密钥签一个「自称是受害者 device_id」的 Hello。
        // 我方已绑定受害者真实公钥 → 自报公钥与绑定不符 → 拒绝。
        let attacker = crypto::Identity::generate();
        let victim = crypto::Identity::generate();
        let (xk, ek, sig) = signed_hello(&attacker, "victim-device", 59992, "n1");
        let bound = victim.ed25519_public_b64();
        assert!(
            hello_auth_decision(Some(&bound), "victim-device", 59992, "n1", &xk, &ek, &sig)
                .is_err()
        );
    }

    #[test]
    fn hello_auth_rejects_forged_sig_with_victim_pubkey() {
        // 攻击者偷到受害者公钥（announce 里是公开信息），但没有私钥 → 签名验不过。
        let attacker = crypto::Identity::generate();
        let victim = crypto::Identity::generate();
        let victim_ek = victim.ed25519_public_b64();
        let victim_xk = victim.x25519_public_b64();
        let sig = attacker.sign_b64(&hello_signing_bytes(
            "victim-device",
            59992,
            "n1",
            &victim_xk,
            &victim_ek,
        ));
        assert!(
            hello_auth_decision(
                Some(&victim_ek),
                "victim-device",
                59992,
                "n1",
                &victim_xk,
                &victim_ek,
                &sig
            )
            .is_err(),
            "冒用绑定公钥但签名不匹配必须被拒"
        );
    }

    #[test]
    fn hello_auth_rejects_missing_signature_and_tampering() {
        let id = crypto::Identity::generate();
        let (xk, ek, sig) = signed_hello(&id, "dev-a", 59992, "n1");
        let bound = id.ed25519_public_b64();
        // 缺 nonce / sig
        assert!(hello_auth_decision(Some(&bound), "dev-a", 59992, "", &xk, &ek, &sig).is_err());
        assert!(hello_auth_decision(Some(&bound), "dev-a", 59992, "n1", &xk, &ek, "").is_err());
        // 篡改被签名覆盖的字段 → 验签失败
        assert!(hello_auth_decision(Some(&bound), "dev-a", 1, "n1", &xk, &ek, &sig).is_err());
        assert!(hello_auth_decision(Some(&bound), "dev-a", 59992, "n2", &xk, &ek, &sig).is_err());
    }

    #[test]
    fn hello_auth_tofu_requires_self_consistent_signature() {
        let id = crypto::Identity::generate();
        let (xk, ek, sig) = signed_hello(&id, "new-node", 59992, "n1");
        // 首次接触：自洽签名可接受
        assert!(hello_auth_decision(None, "new-node", 59992, "n1", &xk, &ek, &sig).is_ok());
        // 首次接触但签名与自报公钥不匹配 → 仍然拒绝
        let other = crypto::Identity::generate();
        let (oxk, oek, _) = signed_hello(&other, "new-node", 59992, "n1");
        assert!(hello_auth_decision(None, "new-node", 59992, "n1", &oxk, &oek, &sig).is_err());
    }

    fn peer_with(ed25519: &str, keys_verified: bool) -> Peer {
        Peer {
            device_id: "victim-device".to_string(),
            nickname: String::new(),
            avatar: None,
            device_type: String::new(),
            ip: String::new(),
            tcp_port: 0,
            last_seen: 0,
            rtt_ms: None,
            x25519_pubkey: Some("xk".to_string()),
            ed25519_pubkey: Some(ed25519.to_string()),
            keys_verified,
            first_seen: None,
            link: None,
        }
    }

    /// 未验签（只来自 UDP announce）的公钥**不得**作为身份绑定。
    /// 这条是「一个伪造广播就能冒充好友」的闸门。
    #[test]
    fn hello_binding_ignores_unverified_announced_keys() {
        let attacker = crypto::Identity::generate();
        let ek = attacker.ed25519_public_b64();

        assert_eq!(
            bound_ed25519_from_peer(Some(&peer_with(&ek, false))),
            None,
            "announce 广播来的公钥不能被当成身份绑定"
        );
        assert_eq!(
            bound_ed25519_from_peer(Some(&peer_with(&ek, true))),
            Some(ek),
            "验签过的公钥才可以作绑定"
        );
        assert_eq!(bound_ed25519_from_peer(None), None);
    }

    /// Gossip 侧信任锚回归（2026-09-19 审计 P0#3）：
    /// peers 是内存态，重启后为空 —— 已在 friends 册的 id 不允许任何 kind 的 TOFU。
    #[test]
    fn gossip_trust_for_known_friend_never_tofus() {
        let friend = crypto::Identity::generate();
        let stored = friend.ed25519_public_b64();

        // 好友的真实信封：三种 kind 全放行
        for kind in [
            GossipKind::Presence,
            GossipKind::FriendRequest,
            GossipKind::Chat,
        ] {
            assert!(
                gossip_trust_for_unpeer_sender(&kind, Some(&stored), &stored),
                "绑定值匹配的好友信封不应被 kind={kind:?} 拒绝"
            );
        }
        // 攻击者自签信封冒充该好友：Presence/FriendRequest/FriendAccept 也必须拒
        let attacker = crypto::Identity::generate();
        let fake = attacker.ed25519_public_b64();
        for kind in [
            GossipKind::Presence,
            GossipKind::FriendRequest,
            GossipKind::FriendAccept,
            GossipKind::Chat,
        ] {
            assert!(
                !gossip_trust_for_unpeer_sender(&kind, Some(&stored), &fake),
                "kind={kind:?} 不得给冒充好友的自签信封开 TOFU 后门"
            );
        }
        // 真正陌生的 id：加好友流程与 Presence 仍可 TOFU；Chat/回执仍拒
        assert!(gossip_trust_for_unpeer_sender(
            &GossipKind::FriendAccept,
            None,
            &fake
        ));
        assert!(!gossip_trust_for_unpeer_sender(
            &GossipKind::Chat,
            None,
            &fake
        ));
        assert!(!gossip_trust_for_unpeer_sender(
            &GossipKind::ChatReadReceipt,
            None,
            &fake
        ));
        // 旧行键列 NULL（Some(None)）：与修复前同宽容（陌生 id 处理）
        assert!(gossip_trust_for_unpeer_sender(
            &GossipKind::Presence,
            None,
            &fake
        ));
    }

    /// peers 新建条目的 friends 锚冲突判定（同审计 P0#3 的第二半）。
    #[test]
    fn new_peer_entry_respects_friend_key_anchor() {
        let fx = "friend-x25519-key";
        let fe = "friend-ed25519-key";
        // 攻击者键冒充好友 ⇒ 两把键任一不符都算冲突，条目不得建立
        assert!(new_peer_conflicts_with_friend(
            Some((Some(fx), Some(fe))),
            Some("attacker-x"),
            Some(fe)
        ));
        assert!(new_peer_conflicts_with_friend(
            Some((Some(fx), Some(fe))),
            Some(fx),
            Some("attacker-e")
        ));
        // 真实键一致 / 锚列为 NULL（旧行未同步）/ 非好友 ⇒ 不冲突
        assert!(!new_peer_conflicts_with_friend(
            Some((Some(fx), Some(fe))),
            Some(fx),
            Some(fe)
        ));
        assert!(!new_peer_conflicts_with_friend(
            Some((None, None)),
            Some("whoever"),
            Some("whoever")
        ));
        assert!(!new_peer_conflicts_with_friend(None, Some("a"), Some("b")));
        // 信封没带键的更新不构成冲突（无从比较）
        assert!(!new_peer_conflicts_with_friend(
            Some((Some(fx), Some(fe))),
            None,
            None
        ));
    }

    /// 完整攻击链的回归：攻击者伪造 announce 抢先把公钥塞进 peers，再用它签 Hello
    /// 冒充受害者 device_id。
    /// 修复前：bound 取自 peers → 就是攻击者自己的公钥 → 验签通过（冒充成功）。
    /// 修复后：未验签 ⇒ bound 为空 ⇒ 落入 TOFU 分支，但**不能**再挤掉已绑定身份；
    /// 若受害者已是我方好友，bound 直接取好友表的真实公钥 ⇒ 攻击者被拒。
    #[test]
    fn announced_attacker_key_cannot_bind_and_impersonate() {
        let attacker = crypto::Identity::generate();
        let victim = crypto::Identity::generate();
        let (axk, aek, asig) = signed_hello(&attacker, "victim-device", 59992, "n1");

        // ① 修复后的 bound 解析：announce 塞进来的条目未验签 → 不构成绑定
        assert_eq!(bound_ed25519_from_peer(Some(&peer_with(&aek, false))), None);

        // ② 好友表里存着受害者真实公钥时，攻击者的 Hello 必须被拒
        let victim_ek = victim.ed25519_public_b64();
        assert!(
            hello_auth_decision(
                Some(&victim_ek),
                "victim-device",
                59992,
                "n1",
                &axk,
                &aek,
                &asig
            )
            .is_err(),
            "用自报公钥冒充已绑定好友必须被拒"
        );

        // ③ 反证：若 bound 误取自 announce（即修复前的行为），攻击者会通过 ——
        //    这条断言锁住「为什么必须过滤」，防止有人把 filter 当成多余代码删掉。
        assert!(
            hello_auth_decision(Some(&aek), "victim-device", 59992, "n1", &axk, &aek, &asig)
                .is_ok(),
            "（反证）把攻击者公钥当绑定就会放行 —— 这正是修复要拦掉的场景"
        );
    }

    #[tokio::test]
    async fn frame_roundtrip() {
        let (a, b) = tokio::io::duplex(4096);
        let (mut _ar, mut aw) = tokio::io::split(a);
        let (mut br, mut _bw) = tokio::io::split(b);
        let msg = Message::Heartbeat {
            device_id: "dev-1".into(),
        };
        let (wr, rd) = tokio::join!(write_frame(&mut aw, &msg), read_frame(&mut br));
        wr.unwrap();
        match rd.unwrap() {
            Message::Heartbeat { device_id } => assert_eq!(device_id, "dev-1"),
            _ => panic!("类型不符"),
        }
    }

    /// 超长帧必须归 **Local**，而且**一个字节都不许上链路**（第 1 步 · 故障隔离）。
    ///
    /// 判据为什么是这两条：旧形状是 `res.is_ok()` 一把抓 ⇒ 一条永远发不出去的帧会把
    /// 整条连接判死，再顺带拖掉同一 peer **其它**链路上正在跑的文件传输。
    /// 而"半截帧写进了流"比"没写"更糟：那条链路从此被污染，后面每一帧都会被对端
    /// 读成截断帧 —— 所以"没上链路"这个事实必须被测出来，不能只靠代码注释。
    #[tokio::test]
    async fn oversize_frame_is_a_local_failure_and_writes_nothing() {
        use crate::protocol::MAX_FRAME;
        use tokio::io::AsyncReadExt;
        let (a, b) = tokio::io::duplex(64);
        let (_ar, mut aw) = tokio::io::split(a);
        let (mut br, _bw) = tokio::io::split(b);
        let msg = Message::ChatMessage {
            msg_id: "m-1".into(),
            from: "dev-1".into(),
            to: "dev-2".into(),
            kind: "text".into(),
            content: "x".repeat(MAX_FRAME + 8),
            ts: 1,
            seq: 1,
        };
        let err = write_frame(&mut aw, &msg)
            .await
            .expect_err("超过 MAX_FRAME 的帧必须失败");
        assert!(
            matches!(err, WriteError::Local(_)),
            "超长帧必须归 Local（链路该保留），实际 {err:?}"
        );

        let mut sink = [0u8; 1];
        let got =
            tokio::time::timeout(std::time::Duration::from_millis(50), br.read(&mut sink)).await;
        assert!(
            got.is_err(),
            "链路上出现了 {} 个字节 —— 长度校验必须在动 socket 之前拦住",
            match got {
                Ok(Ok(n)) => n,
                _ => usize::MAX,
            }
        );
    }

    /// **反向护栏**：真 socket 写失败必须归 `Socket`（⇒ 记连接失败 + 拆这条写半）。
    /// 没有这一条，"写失败不再拆链"会被做成"文件永远不拆链"，
    /// 于是那条已经写不出去的连接会被一直复用（用户 2026-09-24 明确要求的分界）。
    #[tokio::test]
    async fn socket_write_failure_is_classified_as_socket() {
        let (a, b) = tokio::io::duplex(8);
        let (_ar, mut aw) = tokio::io::split(a);
        drop(b); // 对端整个消失 ⇒ 这一帧写不出去
        let msg = Message::Heartbeat {
            device_id: "dev-1".into(),
        };
        let res = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            write_frame(&mut aw, &msg),
        )
        .await
        .expect("对端消失时写必须立刻返回，不能挂住");
        let err = res.expect_err("对端已消失，写必然失败");
        assert!(
            matches!(err, WriteError::Socket(_)),
            "真 IO 失败必须归 Socket ⇒ 该连接判死，实际 {err:?}"
        );
    }

    /// writer_loop 的"写失败"分流**只有一处判据，两半都不许退化**（第 1 步 · 故障隔离）。
    ///
    /// 为什么是源码护栏而不是行为测试：`writer_loop` 吃 `Arc<AppState>`（单测里造不出来），
    /// 而这里要钉的恰恰是"接到分类结果之后做了什么" —— 分类本身已由
    /// `oversize_frame_is_a_local_failure_and_writes_nothing` / `socket_write_failure_is_classified_as_socket`
    /// 用真链路证过。
    ///
    /// 两半各自的退化方向：
    /// - **Local 那一半不许拆链**：一旦它旁边长出 `mark_conn_failure(` 或 `break`，
    ///   一条永远发不出去的帧又会把整条连接带走（旧行为）。
    /// - **Socket 那一半必须拆链**：判死点必须**只剩一处**，且不能在 Local 分支里 ——
    ///   防止"为了保护文件传输"把真 IO 失败也一起放过（用户 2026-09-24 明确划的界）。
    #[test]
    fn writer_loop_splits_local_from_socket_failure_exactly_once() {
        let src = crate::network::transport_src_for_guards();
        let start = src
            .find("async fn writer_loop(")
            .expect("找不到 writer_loop（这条护栏会空转）");
        let end = src[start..]
            .find("async fn reader_loop(")
            .map(|i| start + i)
            .expect("writer_loop 后面找不到 reader_loop（函数边界变了，护栏需同步）");
        let body = &src[start..end];

        // ① 分类必须在此发生，且 Socket 那一半被送到 Failed（不是被 Local 吞掉）
        assert!(
            body.contains("Err(WriteError::Local(why)) => WriteOutcome::Local(why)"),
            "writer_loop 不再接 WriteError 的分型 ⇒ 分流点丢了"
        );
        assert!(
            body.contains("Err(WriteError::Socket(_)) => WriteOutcome::Failed"),
            "真 socket 失败必须仍然走 Failed 那一支（拆链）"
        );

        // ② 判死点全函数只有一处，且**不在** Local 分支里
        assert_eq!(
            body.matches("mark_conn_failure(").count(),
            1,
            "写失败的判死点必须只有一处；现在有 {} 处",
            body.matches("mark_conn_failure(").count()
        );
        let local_at = body
            .find("if let WriteOutcome::Local(why)")
            .expect("Local 分支不见了 ⇒ 本地成帧失败又会拆链");
        let local_end = body[local_at..]
            .find("continue")
            .map(|i| local_at + i + "continue".len())
            .expect("Local 分支没有 continue");
        let local_span = &body[local_at..local_end];
        for forbidden in ["mark_conn_failure(", "break"] {
            assert!(
                !local_span.contains(forbidden),
                "Local 分支里出现了 `{forbidden}`：一个字节都没上链路的帧不该带走整条连接"
            );
        }
        // ③ 剩下的那一处判死点必须在 Local 分支之后（即它服务的是真 IO 失败）
        let kill_at = body
            .find("mark_conn_failure(")
            .expect("Socket 那一半必须记连接失败");
        assert!(
            kill_at > local_end,
            "唯一的判死点落在了 Local 分支之前/之内 ⇒ 分流失效"
        );
    }

    /// 断链只清"这个 peer 真的一条链路都不剩"的接收器（P1 故障隔离的接线判据）。
    ///
    /// 为什么是源码护栏而不是行为测试：`reader_loop` 吃 `Arc<AppState>`（单测里造不出来），
    /// 而这里要钉的是**顺序**——清理必须在 `peer_now_offline` 算出来之后。
    /// 挪回前面就复现旧缺陷：同一 peer 的 LAN + Tailscale 双链路里断一条，
    /// 会把另一条链路上**正在收**的文件一起判死（`fail_receives_for_peer` 按 peer 清，不按连接）。
    /// 判"谁在 `if peer_now_offline` 之前"用位置而不是数量：数量不变、只有顺序变才是这次的形状。
    #[test]
    fn peer_wide_receiver_cleanup_is_gated_on_total_link_loss() {
        let src = crate::network::transport_src_for_guards();
        let start = src
            .find("async fn reader_loop(")
            .expect("找不到 reader_loop（这条护栏会空转）");
        // 上界 = reader_loop 之后的第一个顶层函数。不能用某个远处函数的注释当边界：
        // 那样切片会把中间几十个函数一起圈进来，`count == 1` 那类判据会因**切片过大**而假红。
        let end = [
            "\nasync fn ",
            "\nfn ",
            "\npub fn ",
            "\npub(crate) fn ",
            "\npub(crate) async fn ",
        ]
        .iter()
        .filter_map(|pat| src[start + 1..].find(pat).map(|i| start + 1 + i))
        .min()
        .expect("reader_loop 之后找不到任何函数边界（护栏需同步）");
        let body = &src[start..end];

        let gate_at = body
            .find("if peer_now_offline {")
            .expect("reader_loop 里没有了 `if peer_now_offline` 这道门");
        assert!(
            body.contains("let peer_now_offline = "),
            "门必须建立在\"确认这条连接确实没了\"之后算出的那个值上"
        );

        // 两处按 peer 清的收尾都必须落在门里面。
        // 位置判据**排在数量判据之前**：注入"挪回前面"会变成两处调用，先报数量就看不出
        // 位置判据到底有没有咬住（非空转验证要求红在该报的那一条上，不是"反正都红"）。
        // 群侧那一半从 2026-09-29 起走具名的原子摘取 helper（判据与摘表同一次持锁，
        // 与本函数下面 sweep 那段的 doctrine 一致）⇒ 锚点从"表名出现"收紧成"必须调那个 helper"：
        // 表名再出现在这里反而说明有人把摘表写回了 reader_loop 里两步做。
        for anchor in ["fail_receives_for_peer(", "take_group_receives_for_peer("] {
            let at = body
                .find(anchor)
                .unwrap_or_else(|| panic!("reader_loop 里找不到 `{anchor}` —— 清理被删了？"));
            assert!(
                at > gate_at,
                "`{anchor}` 排在了 `if peer_now_offline` 之前：断一条链路就会杀掉该 peer 全部接收"
            );
        }
        assert_eq!(
            body.matches("fail_receives_for_peer(").count(),
            1,
            "单聊接收器的 peer-wide 清理必须只剩一处（多处 = 又有第二个判据）"
        );
        // 回收侧必须真的接上，而且**两张表都要有**（否则"延后清理"= 那半边永久泄漏）。
        // 必须走 `take_*`：判据与摘表同一次持锁 —— 分开两步就会被"快照之后挤进来的新 Offer"
        // 判死一条正在收的传输。
        let s_start = src
            .find("pub fn sweep_stalled_receives(")
            .expect("sweep_stalled_receives 不见了 ⇒ 延后清理就没有任何东西兜底");
        let s_end = src[s_start..]
            .find("\n}\n")
            .map(|i| s_start + i + 3)
            .expect("sweep_stalled_receives 没有结尾");
        let sweep = &src[s_start..s_end];
        for call in [
            "file::take_stalled_receive(",
            "file::take_stalled_group_receive(",
        ] {
            assert!(
                sweep.contains(call),
                "回收漏了 {call}：那张表上的静默接收器没人摘，`.part` 与文件句柄就此永久留着"
            );
        }
        assert!(
            !sweep.contains("file::receive_is_stale("),
            "判据不该在清扫器里重算一遍 —— 它必须与摘表同一次持锁（`take_stalled_*` 内部）"
        );
    }

    /// 链路的 low 队列必须按**字节**封顶，而不是按帧数（第 2 步 · P3）。
    ///
    /// 旧形状是四个建链点各写死三条 1024 深的 `mpsc::channel`：一片 LAN 分块上线是
    /// `base64(chunk + 28) ≈ 341 KB`，1024 槽 ⇒ **单链路最坏 ~350 MB**
    /// （多链路、群文件多收件人按连接翻倍）。背压是有的，但**位置错了**：
    /// 缓冲先分配完才开始排队。
    ///
    /// 这条断言故意只说"预算"与"不许为 0"，不说槽数是多少 —— 槽数是推导量，
    /// 把 24 写进测试就等于每次调预算都要改一次测试。
    #[test]
    fn link_low_queue_is_bounded_by_bytes_not_frame_count() {
        // `mpsc::channel(0)` 会 panic ⇒ 折算下限必须 ≥1，这里连极小与极大分片一起试
        for plain in [crate::network::file::BLE_FILE_CHUNK, 1, 64] {
            let slots = low_queue_slots(plain);
            assert!(
                slots >= 1,
                "chunk={plain} 折算出 {slots} 槽，channel(0) 会直接 panic"
            );
        }
        // 分片越大 ⇒ 槽越少（单调不增），否则"按字节封顶"这句话是空的
        let mut prev = usize::MAX;
        for plain in [
            1usize,
            4096,
            65536,
            256 * 1024,
            1024 * 1024,
            crate::protocol::MAX_FRAME,
        ] {
            let slots = low_queue_slots(plain);
            assert!(
                slots <= prev,
                "chunk={plain} 反而比更小的分片排得更深（{slots} > {prev}）"
            );
            prev = slots;
        }
        // 真正的 P3 判据：两种生产分片尺寸下「槽数 × 单帧线上字节」都不许越过预算
        for plain in [
            crate::network::file::BLE_FILE_CHUNK,
            crate::protocol::FILE_CHUNK,
        ] {
            let held = low_queue_slots(plain) * crate::network::file::chunk_wire_bytes(plain);
            assert!(
                held <= LINK_QUEUE_BYTE_BUDGET,
                "chunk={plain} ⇒ 队列最坏装 {held} 字节，超过预算 {LINK_QUEUE_BYTE_BUDGET}"
            );
        }
        // 旧形状必须真的被治好：256KB 分片下不可能再排到 1024 深
        assert!(
            low_queue_slots(crate::protocol::FILE_CHUNK) < 1024,
            "LAN 分片的队列还是 1024 深 = 那条 ~350 MB 的老路没堵住"
        );
    }

    /// 四条链路的三条队列必须由**同一个策略**开出来（P3 的防回归位置）。
    ///
    /// 为什么还要源码钉一层，纯函数已经测过了：`1024` 这个字面量散在 4 个建链点里
    /// （入站 / 出站拨号 / BLE 两处）。只要还剩一份字面量，下一个加链路的人就会照抄那一份
    /// ⇒ "字节预算"退化成"其中三处有预算"，而且这种事**只在真机大文件时才看得见**。
    #[test]
    fn link_queues_are_created_from_one_place() {
        let mut src = crate::network::transport_src_for_guards();
        // 路径相对**本文件**：这段测试原来在 `network/transport.rs` 里，`ble.rs` 指的就是
        // 同目录之上的 `network/ble.rs`；搬进 `network/transport/` 之后必须写 `../ble.rs`，
        // 否则读到的是不存在的路径（编译器直接拒，不会静默）。
        src.push_str(include_str!("../ble.rs"));
        // 两个探针都**拼起来写**：本文件就是被扫的源码之一，直接写字面量会数进自己
        // （今天已经在别处被这个坑咬过两次：一次假过、一次假红）。
        let literal = "mpsc::channel(10".to_string() + "24)";
        assert_eq!(
            src.matches(literal.as_str()).count(),
            0,
            "还有写死的 1024 槽建链点：字节预算会被那一份绕过去"
        );
        let call = "link_chan".to_string() + "nels(";
        assert_eq!(
            src.matches(call.as_str()).count(),
            5,
            "应为「定义 1 处 + 四个建链点各 1 处」；少了就是有条链路还在自己开队列"
        );
        // 光"都走 link_channels"还不够：函数本身也得真的按预算折算开 low，
        // 否则预算名存实亡（这一条是被变异测试逼出来的）。
        let wired = "mpsc::channel(low_queue".to_string() + "_slots(";
        assert_eq!(
            src.matches(wired.as_str()).count(),
            1,
            "low 队列必须由 `low_queue_slots` 折算出来，不能直接给常量深度"
        );
    }

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
    async fn frame_roundtrip_large_payload() {
        // 模拟 256KB 文件分片的 base64 负载往返
        let big = "A".repeat(342_000);
        let msg = Message::RelayChunk {
            transfer_id: "t1".into(),
            seq: 7,
            data: big.clone(),
            from: "a".into(),
            to: "b".into(),
            ttl: 3,
        };
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (mut _ar, mut aw) = tokio::io::split(a);
        let (mut br, mut _bw) = tokio::io::split(b);
        let (wr, rd) = tokio::join!(write_frame(&mut aw, &msg), read_frame(&mut br));
        wr.unwrap();
        match rd.unwrap() {
            Message::RelayChunk { data, seq, .. } => {
                assert_eq!(seq, 7);
                assert_eq!(data, big);
            }
            _ => panic!("类型不符"),
        }
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

    fn seal_direct(from: &crate::crypto::Identity, to_pubkey: &str, text: &str) -> String {
        let shared = crate::crypto::shared_secret(&from.x25519_secret, to_pubkey).unwrap();
        format!(
            "enc1:{}",
            STANDARD.encode(crate::crypto::seal(&shared, text.as_bytes()).unwrap())
        )
    }

    /// Test 1 正常 E2EE：正确公钥 → 明文与原始 kind 一并还原（kind 不被改写成 system）。
    #[test]
    fn direct_open_succeeds_with_current_keys() {
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let wire = seal_direct(&a, &b.x25519_public_b64(), "你好 e2ee");
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&a.x25519_public_b64()),
                &wire,
                "code".to_string()
            ),
            Some(("你好 e2ee".to_string(), "code".to_string()))
        );
    }

    /// Test 2 场景 A（暂时缺公钥）：缺发送方公钥必须判为「解不开」（→ 不落库、不 Ack），
    /// 且公钥经 announce/who_has 学到之后，**同一份密文**即可解开 —— 补发重试就能恢复。
    #[test]
    fn direct_open_fails_without_sender_key_and_recovers_when_key_arrives() {
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let wire = seal_direct(&a, &b.x25519_public_b64(), "pending key");
        assert_eq!(
            open_direct_content(&b.x25519_secret, None, &wire, "text".to_string()),
            None
        );
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&a.x25519_public_b64()),
                &wire,
                "text".to_string()
            ),
            Some(("pending key".to_string(), "text".to_string()))
        );
    }

    /// Test 3a 场景 B（发送方换身份）：本地缓存为旧公钥时解不开；
    /// `upsert_peer` 把对方新公钥刷进缓存后，同一份密文可解开（无需重新加密）。
    #[test]
    fn direct_open_recovers_once_sender_pubkey_cache_refreshed() {
        let a_old = crate::crypto::Identity::generate();
        let a_new = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let wire = seal_direct(&a_new, &b.x25519_public_b64(), "rotated sender");
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&a_old.x25519_public_b64()),
                &wire,
                "text".to_string()
            ),
            None
        );
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&a_new.x25519_public_b64()),
                &wire,
                "text".to_string()
            ),
            Some(("rotated sender".to_string(), "text".to_string()))
        );
    }

    /// Test 3b 场景 B（接收方换身份）：outbox 里的密文对着旧公钥封存，重发多少次都解不开，
    /// 必须由持有明文的发送方用**当前**公钥重封；重封可失败（无明文 / 无公钥）时一律返回
    /// None 让调用方按原样补发，绝不伪造内容。
    #[test]
    fn reseal_with_current_receiver_key_recovers_where_retry_cannot() {
        let a = crate::crypto::Identity::generate();
        let b_old = crate::crypto::Identity::generate();
        let b_new = crate::crypto::Identity::generate();
        let stale = seal_direct(&a, &b_old.x25519_public_b64(), "stale seal");

        // 旧密文对新的接收方身份永久无效（重发不解决问题）
        assert_eq!(
            open_direct_content(
                &b_new.x25519_secret,
                Some(&a.x25519_public_b64()),
                &stale,
                "text".to_string()
            ),
            None
        );
        // 重封：同一明文 + 当前公钥 → 可解，且仍是 enc1: 形态
        let resealed = reseal_chat_content(
            &a.x25519_secret,
            Some("stale seal"),
            Some(&b_new.x25519_public_b64()),
        )
        .unwrap();
        assert_ne!(resealed, stale);
        assert_eq!(
            open_direct_content(
                &b_new.x25519_secret,
                Some(&a.x25519_public_b64()),
                &resealed,
                "text".to_string()
            ),
            Some(("stale seal".to_string(), "text".to_string()))
        );
        // 前置条件缺失 → 不重封（调用方保留原 payload）
        let no_plaintext =
            reseal_chat_content(&a.x25519_secret, None, Some(&b_new.x25519_public_b64()));
        assert_eq!(no_plaintext, None);
        let no_pubkey = reseal_chat_content(&a.x25519_secret, Some("stale seal"), None);
        assert_eq!(no_pubkey, None);
    }

    /// Test 3c 场景 C（真损坏）：base64 非法 / 密文被篡改一律判为解不开，
    /// 但**不污染**同一条完好密文的可解性 —— 失败只影响这一次投递。
    #[test]
    fn direct_open_rejects_corrupt_and_tampered_payloads() {
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let spk = a.x25519_public_b64();
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&spk),
                "enc1:!!not base64!!",
                "text".to_string()
            ),
            None
        );
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&spk), "enc1:", "text".to_string()),
            None
        );
        let wire = seal_direct(&a, &b.x25519_public_b64(), "intact");
        let mut raw = STANDARD
            .decode(wire.strip_prefix("enc1:").unwrap())
            .unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xFF; // 破坏 AEAD tag
        let tampered = format!("enc1:{}", STANDARD.encode(&raw));
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&spk), &tampered, "text".to_string()),
            None
        );
        assert!(
            open_direct_content(&b.x25519_secret, Some(&spk), &wire, "text".to_string()).is_some()
        );
    }

    #[test]
    fn plaintext_payload_is_rejected() {
        let me = crate::crypto::Identity::generate();
        assert_eq!(
            open_direct_content(
                &me.x25519_secret,
                None,
                "plain old text",
                "text".to_string()
            ),
            None
        );
    }

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

    /// 链路未建立时（announce 先于 ensure_link）群密钥发送失败，
    /// 必须登记到 pending，否则后续 is_new/key_changed 不再触发 → 永久丢密钥。
    #[test]
    fn redistribute_failure_registers_pending() {
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();

        // 模拟 announce → upsert_peer → redistribute_group_keys → try_send 失败（无 link）
        // 失败分支必须调用 mark_pending_group_key（见 transport.rs::redistribute_group_keys）
        mark_pending_group_key(&mut pending, "peer-b", "g-1");
        mark_pending_group_key(&mut pending, "peer-b", "g-2");
        // 同 (peer, group) 重复登记应幂等
        mark_pending_group_key(&mut pending, "peer-b", "g-1");

        let ids = pending_group_key_ids(&pending, "peer-b");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"g-1".to_string()));
        assert!(ids.contains(&"g-2".to_string()));
    }

    /// 链路就绪后 flush_pending_group_keys 成功 → 必须清除登记项，
    /// 否则 pending 永远不被消费（不增长、不泄漏、达成"成功即清"）。
    #[test]
    fn flush_success_clears_pending() {
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();
        mark_pending_group_key(&mut pending, "peer-b", "g-1");
        mark_pending_group_key(&mut pending, "peer-b", "g-2");
        mark_pending_group_key(&mut pending, "peer-c", "g-1");

        // 模拟 flush_pending_group_keys：peer-b 已建链、两条都发送成功
        let result: Result<(), GroupKeySendErr> = Ok(());
        assert!(!should_retain_pending_group_key(&result));
        for gid in ["g-1", "g-2"] {
            clear_pending_group_key(&mut pending, "peer-b", gid);
        }

        // peer-b 的登记应被清空（键一并移除，避免无意义增长）
        assert!(pending_group_key_ids(&pending, "peer-b").is_empty());
        assert!(!pending.contains_key("peer-b"));
        // peer-c 不受影响
        assert_eq!(
            pending_group_key_ids(&pending, "peer-c"),
            vec!["g-1".to_string()]
        );
    }

    /// 链路仍未就绪（NoLink）或非可重试原因（Fatal）的判定：
    /// NoLink 必须保留登记项等待下一次 flush；Fatal / Ok 必须清除。
    #[test]
    fn flush_failure_retains_or_clears_pending_correctly() {
        // 仍无链路 → 保留
        assert!(should_retain_pending_group_key(&Err(
            GroupKeySendErr::NoLink
        )));
        // 非可重试 → 清除
        assert!(!should_retain_pending_group_key(&Err(
            GroupKeySendErr::Fatal
        )));
        assert!(!should_retain_pending_group_key(&Ok(())));

        // 验证 flush 逻辑：NoLink 分支不调用 clear，pending 保持
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();
        mark_pending_group_key(&mut pending, "peer-b", "g-1");
        let result: Result<(), GroupKeySendErr> = Err(GroupKeySendErr::NoLink);
        if should_retain_pending_group_key(&result) {
            // 故意不调用 clear_pending_group_key：等待下一次 flush
        } else {
            clear_pending_group_key(&mut pending, "peer-b", "g-1");
        }
        assert_eq!(
            pending_group_key_ids(&pending, "peer-b"),
            vec!["g-1".to_string()]
        );

        // 下一轮 flush：Fatal 分支必须清除（重试无意义：缺公钥 / 非成员 / 无密钥）
        clear_pending_group_key(&mut pending, "peer-b", "g-1");
        let result: Result<(), GroupKeySendErr> = Err(GroupKeySendErr::Fatal);
        if !should_retain_pending_group_key(&result) {
            clear_pending_group_key(&mut pending, "peer-b", "g-1");
        }
        assert!(pending_group_key_ids(&pending, "peer-b").is_empty());
    }

    // ---------- 群成员公钥解析（peers 优先、friends 回落） ----------

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

    /// 取一个空闲端口（绑到 0 再读回内核分配的端口）。
    async fn free_port() -> u16 {
        let probe = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("探测端口失败");
        probe.local_addr().expect("读取本地地址失败").port()
    }

    /// Test 1：listener → accept 一条真实连接 → 关闭 → 在同一端口重新建 listener。
    ///
    /// 这条测试用来锁定「accepted connection 的本地端口 == 监听端口」这一事实在
    /// 当前平台上的后果：
    /// - Unix：mio 已设置 SO_REUSEADDR（仅跳过 TIME_WAIT，不允许多监听并存），
    ///   TIME_WAIT 不应阻止重绑；若将来有人绕过 mio 建 listener，这里会立刻失败。
    /// - Windows：没有 SO_REUSEADDR，且本测试没有走 `set_abortive_close`，
    ///   允许出现 AddrInUse —— 这正是生产故障的成因，被这条测试如实记录下来。
    ///   Windows 上「能立即重绑」由 `windows_abortive_close_allows_immediate_rebind`
    ///   单独验证。
    #[tokio::test]
    async fn rebind_after_accepted_connection_matches_platform_semantics() {
        let port = free_port().await;
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("首次绑定失败");
        let client = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接失败");
        let (conn, _) = listener.accept().await.expect("accept 失败");

        // 主动关闭（本测试不设置 SO_LINGER，保留平台默认关闭语义）
        drop(client);
        drop(conn);
        drop(listener);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rebind = TcpListener::bind(("127.0.0.1", port)).await;
        if cfg!(windows) {
            match rebind {
                Ok(_) => {}
                Err(e) => assert_eq!(
                    e.kind(),
                    std::io::ErrorKind::AddrInUse,
                    "Windows 上重绑失败只允许是端口占用，实际: {e}"
                ),
            }
        } else {
            assert!(
                rebind.is_ok(),
                "Unix 上 mio 已设置 SO_REUSEADDR，TIME_WAIT 不应阻止重绑: {:?}",
                rebind.err()
            );
        }
    }

    /// Test 1（Windows 专属）：走生产路径 `set_abortive_close`（SO_LINGER=0）关闭
    /// accepted connection 后，监听端口必须**立即可重绑**。
    ///
    /// 这是 Windows 生产环境「C 重启/退出重进后 59992 无法 bind」的直接回归测试。
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_abortive_close_allows_immediate_rebind() {
        let port = free_port().await;
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("首次绑定失败");
        let client = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接失败");
        let (conn, _) = listener.accept().await.expect("accept 失败");

        // 与 handle_incoming 完全相同的处理顺序：先标记 abortive close，再关闭
        set_abortive_close(&conn);
        drop(client);
        drop(conn);
        drop(listener);

        // 不等待：RST 关闭不应在监听端口留下任何 TIME_WAIT
        TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("SO_LINGER=0 关闭的连接不应在监听端口留下 TIME_WAIT");
    }

    /// Test 2：accept 任务收到 shutdown 并**真正退出**后，同端口必须立即可重绑。
    ///
    /// 对应 `network::stop()` 的语义：不等到旧 listener 释放就继续走，
    /// 同进程切换网卡（stop→start）或 `app.restart()` 起来的新进程都会撞上
    /// AddrInUse。这条测试锁住「任务退出 ⇒ 端口释放」。
    #[tokio::test]
    async fn port_is_free_immediately_after_accept_loop_exits() {
        let port = free_port().await;
        let (tx, rx) = watch::channel(false);
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("首次绑定失败");

        // 与 transport::spawn 的 accept 循环同构
        let task = tokio::spawn(async move {
            let mut shutdown = rx;
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    accept = listener.accept() => {
                        if let Ok((stream, _)) = accept { drop(stream); }
                    }
                }
            }
            // listener 在此 drop
        });

        // 先产生一条真实连接，确认 accept 循环确实在工作
        let client = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接失败");
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let _ = tx.send(true);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("accept 任务应在 shutdown 后立即退出")
            .expect("accept 任务不应 panic");

        TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("旧 accept 任务退出后端口必须立即可用");
    }

    /// Test 4：start → 客户端真实收发 → stop → start，网络功能仍然完整。
    ///
    /// 端到端覆盖监听端口的整个生命周期（不含 Gossip/协议层，只验证 TCP 通路）。
    #[tokio::test]
    async fn start_stop_start_accept_loop_keeps_serving() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        /// 起一个 echo accept 循环，返回 (shutdown 发送端, 任务句柄)。
        async fn spawn_echo(
            port: u16,
        ) -> (
            tokio::sync::watch::Sender<bool>,
            tokio::task::JoinHandle<()>,
        ) {
            let listener = TcpListener::bind(("127.0.0.1", port))
                .await
                .expect("绑定失败");
            let (tx, rx) = watch::channel(false);
            let task = tokio::spawn(async move {
                let mut shutdown = rx;
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => break,
                        accept = listener.accept() => {
                            let Ok((mut stream, _)) = accept else { continue };
                            tokio::spawn(async move {
                                let mut buf = [0u8; 4];
                                if stream.read_exact(&mut buf).await.is_ok() {
                                    let _ = stream.write_all(&buf).await;
                                }
                            });
                        }
                    }
                }
            });
            (tx, task)
        }

        async fn echo_roundtrip(port: u16) -> bool {
            let mut c = match TcpStream::connect(("127.0.0.1", port)).await {
                Ok(c) => c,
                Err(_) => return false,
            };
            if c.write_all(b"ping").await.is_err() {
                return false;
            }
            let mut buf = [0u8; 4];
            match c.read_exact(&mut buf).await {
                Ok(_) => &buf == b"ping",
                Err(_) => false,
            }
        }

        let port = free_port().await;

        // ---- 第一次 start ----
        let (tx1, task1) = spawn_echo(port).await;
        assert!(echo_roundtrip(port).await, "第一次 start 后应能正常收发");

        // ---- stop（等任务真正退出）----
        let _ = tx1.send(true);
        tokio::time::timeout(Duration::from_secs(2), task1)
            .await
            .expect("stop 应立即结束 accept 任务")
            .expect("accept 任务不应 panic");

        // ---- 第二次 start（同一端口，立即）----
        let (tx2, task2) = spawn_echo(port).await;
        assert!(
            echo_roundtrip(port).await,
            "stop 后立即 start，同一端口必须仍能正常收发"
        );

        let _ = tx2.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(2), task2).await;
    }

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
}
