// 职责边界：
// - transport 行为测试分册之1 —— 握手帧的头像预算 + Hello 验签决策表 + 公钥锚点（TOFU / 好友锚）
// 为什么拆：`transport/tests.rs` 原来 3,227 行、13 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
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
