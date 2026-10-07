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

    /// 分发臂与各册入口的 match 臂必须**一一对齐**（2026-10-07 函数级拆分留下的唯一新口子）。
    ///
    /// 为什么要专门钉：拆之前 `match msg` 是一个整体，编译器保证穷尽；拆成分发器 + 五册之后
    /// 两侧各有一张臂表，编译器**看不见另一侧** ⇒
    ///   · 分册加一臂而分发臂没列 ⇒ 那条臂永远不可达（无警告）；
    ///   · 分发臂列了而分册没有 ⇒ 帧掉进分册的 `_ => {}` 被**静默丢弃**，
    ///     而文件/中继/群那几条路径上"什么都不说"正是 INV-P05 一类事故的形状。
    #[test]
    fn dispatch_arms_and_volume_arms_agree() {
        let view = crate::network::transport_src_for_guards();
        // 台账是现读 `network/transport.rs` 的分发臂逐组数出来的（合计 35 个变体）：
        //   file 6 = FileOffer / FileAccept / FileReject / FileCompleteAck / FileChunk / FileDone
        let families: [(&str, usize); 5] = [
            ("handle_file_messages", 6),
            ("handle_share_and_relay_messages", 6),
            ("handle_group_messages", 11),
            ("handle_identity_and_friend_messages", 8),
            ("handle_messaging_arm", 4),
        ];
        let groups = dispatch_groups(&view);
        assert_eq!(
            groups.len(),
            families.len(),
            "分发器里有 {} 组 `m @ ( … )` 委托臂，台账只有 {} 个入口 ⇒ 有人加/删了一组而这条守卫的名单没跟着改：{:?}",
            groups.len(),
            families.len(),
            groups
                .iter()
                .map(|(h, a)| format!("{h}({})", a.len()))
                .collect::<Vec<_>>()
        );
        let mut dispatched: Vec<String> = Vec::new();
        for (handler, expect) in families {
            let (_, disp) = groups
                .iter()
                .find(|(h, _)| h == handler)
                .unwrap_or_else(|| panic!("分发器里没有委托给 `{handler}` 的那组臂"));
            let vol = arm_variants(&volume_body(&view, &format!("async fn {handler}(")));
            assert_eq!(
                disp.len(),
                expect,
                "{handler}：分发臂列了 {:?}，台账是 {expect} 个（分册侧 {vol:?}）",
                disp
            );
            assert_eq!(
                vol.len(),
                expect,
                "{handler}：分册里有 {:?}，台账是 {expect} 条（分发侧 {disp:?}）",
                vol
            );
            for v in disp {
                assert!(
                    vol.contains(v),
                    "分发把 {v} 交给 {handler}，可 {handler} 里没有那一臂 ⇒ 这一帧会掉进 `_ => {{}}` 被静默丢掉"
                );
            }
            for v in &vol {
                assert!(
                    disp.contains(v),
                    "{handler} 处理 {v}，但分发臂没列出它 ⇒ 那段处理体永远不可达"
                );
            }
            dispatched.extend(disp.iter().cloned());
        }
        let total = dispatched.len();
        dispatched.sort();
        dispatched.dedup();
        assert_eq!(dispatched.len(), total, "同一变体出现在两族的臂表里（{total} → {}）⇒ 两份语义", dispatched.len());
    }

    /// 取一个顶层 `async fn` 的函数体（签名起，到第 0 列的 `}` 为止）。
    ///
    /// 两个锚点找不到都 **panic**，不许退化：第一版写的是 `unwrap_or(view.len())`
    /// （收尾锚点缺失时取到视图末尾）⇒ 会把后面几族的臂一起数进来，
    /// 那是本项目最忌讳的"护栏还在跑、判的却不是那一段"的形状。
    fn volume_body(src: &str, signature: &str) -> String {
        let start = src
            .find(signature)
            .unwrap_or_else(|| panic!("视图里找不到 `{signature}` ⇒ 分发器改了名，这条守卫要同步改"));
        let rest = &src[start..];
        let end = rest
            .find("\n}")
            .unwrap_or_else(|| panic!("`{signature}` 的函数体没有第 0 列的收尾 `}}`（视图拼接错位？）"));
        rest[..end].to_string()
    }

    /// 按行扫分发器：从 `m @ (Message::…` 起、顺着 `| Message::` 续行收集变体，直到看到 `=>`；
    /// 委托目标取 `=>` 之后的第一个 `name(`，`=> {` 那种换行委托再看随后三行。
    ///
    /// 为什么按行而不是按字节切片（两处实测教训）：
    ///   · 第一版用 `view[..d_end].rfind("m @ (")` 定组起点，切出来的是**半行**（8 格缩进被切掉）
    ///     ⇒ 组里第一个变体永远漏数（实测 FileOffer 消失、6 报成 5）；
    ///   · 而 `find("=> handler(")` 接不住 identity 族那种 `=> {\n handler(…)` 的形状。
    fn dispatch_groups(src: &str) -> Vec<(String, Vec<String>)> {
        let lines: Vec<&str> = src.split('\n').collect();
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        let mut i = 0usize;
        while i < lines.len() {
            let Some(first) = lines[i].trim_start().strip_prefix("m @ (Message::") else {
                i += 1;
                continue;
            };
            let mut arms = vec![arm_name(first)];
            let mut handler = String::new();
            let mut j = i;
            while j < lines.len() {
                let line = lines[j];
                if j > i {
                    let t = line.trim_start();
                    if let Some(rest) = t.strip_prefix("| Message::") {
                        arms.push(arm_name(rest));
                    } else if t.is_empty() || t.starts_with("//") {
                        // 组内注释/空行不打断臂表
                    } else {
                        break;
                    }
                }
                if let Some(pos) = line.find("=>") {
                    handler = call_name(&line[pos + 2..]).unwrap_or_default();
                    if handler.is_empty() {
                        for k in (j + 1)..(j + 4).min(lines.len()) {
                            if let Some(n) = call_name(lines[k]) {
                                handler = n;
                                break;
                            }
                        }
                    }
                    break;
                }
                j += 1;
            }
            out.push((handler, arms));
            i = j + 1;
        }
        out
    }

    fn arm_name(rest: &str) -> String {
        rest.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }

    /// 文本里第一个 `name(` 的 name（取 `=>` 之后的委托目标用）；关键字不算调用。
    fn call_name(text: &str) -> Option<String> {
        let cs: Vec<char> = text.chars().collect();
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut i = 0usize;
        while i < cs.len() {
            if !ident(cs[i]) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < cs.len() && ident(cs[j]) {
                j += 1;
            }
            if j < cs.len() && cs[j] == '(' {
                let s: String = cs[i..j].iter().collect();
                if !["match", "if", "while", "for", "return", "async", "fn"].contains(&s.as_str()) {
                    return Some(s);
                }
            }
            i = j;
        }
        None
    }

    /// 取一段文本里**第一层**的 `Message::X` 臂名：只认 8 空格起的臂与其 `|` 续行，
    /// 臂体里（≥12 空格）构造的消息值不算 —— 否则一条臂里再发一帧就会被数成两臂。
    fn arm_variants(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in text.split('\n') {
            let rest = match line
                .strip_prefix("        m @ (Message::")
                .or_else(|| line.strip_prefix("        Message::"))
                .or_else(|| line.strip_prefix("        | Message::"))
            {
                Some(r) => r,
                None => continue,
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
        out
    }
