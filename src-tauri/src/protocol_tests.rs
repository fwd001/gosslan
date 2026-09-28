// 职责边界：
// - protocol.rs 的行为测试（从主文件搬出来，`include!` 回同一模块 ⇒ 模块路径 protocol_tests::tests::* 一字不变）
// 为什么搬：主文件 2922 行里这一段占 1254 行（约 42%），生产码只 1668 行。
// 搬动机制与 db/favorites_tests.rs 同一条：include! 是文本粘贴 ⇒ 同一模块、同一 use、同一可见性，
// 编译器判等价，测试路径不变 ⇒ 测试清单基线也不该有任何差异。
#[cfg(test)]
mod tests {
    use super::*;

    /// 历史 video/audio 行在**读出口**归一为文件卡片；其余 kind 原样（数据不回写）。
    #[test]
    fn display_kind_maps_legacy_media_to_file() {
        assert_eq!(display_kind("video"), "file");
        assert_eq!(display_kind("audio"), "file");
        assert_eq!(display_kind("image"), "image");
        assert_eq!(display_kind("file"), "file");
        assert_eq!(display_kind("text"), "text");
        // code 有歧义（真代码块同为 kind=code），刻意不映射
        assert_eq!(display_kind("code"), "code");
    }

    // ---------------- 群消息明文里的 @ 名单（mention ids） ----------------
    // 三态是本组用例唯一要钉的东西：None=「这个键不存在」= 旧对端，接收端按昵称兜底；
    // Some(空)=「明确回答谁都没 @」= 不许兜底。合并成两态就会要么漏亮要么误亮。

    #[test]
    fn gossip_plaintext_without_mentions_keeps_the_two_key_shape() {
        let pt = gossip_plaintext("text", "hi", None);
        let v: serde_json::Value = serde_json::from_str(&pt).unwrap();
        assert_eq!(
            v.as_object().map(|m| m.len()),
            Some(2),
            "不该凭空多出 mentions 键"
        );
        let back = parse_gossip_plaintext(pt.as_bytes());
        assert_eq!(back.mentions, None);
        assert_eq!(back.kind, "text");
        assert_eq!(back.content, "hi");
    }

    #[test]
    fn gossip_plaintext_empty_mentions_is_an_explicit_answer_not_absence() {
        let pt = gossip_plaintext("text", "hi", Some(&[]));
        let v: serde_json::Value = serde_json::from_str(&pt).unwrap();
        assert_eq!(v.get("mentions"), Some(&serde_json::json!([])));
        let back = parse_gossip_plaintext(pt.as_bytes());
        assert_eq!(back.mentions, Some(vec![]));
    }

    #[test]
    fn gossip_plaintext_ids_survive_the_round_trip_in_order() {
        let ids = vec!["dev-b".to_string(), "dev-c".to_string()];
        let back = parse_gossip_plaintext(gossip_plaintext("text", "@周工", Some(&ids)).as_bytes());
        assert_eq!(back.mentions, Some(ids));
    }

    /// **载荷外壳可以加字段，正文一个字节都不许动** —— 存储不变量（INV-P24/§8）在
    /// 加密前那一环的落点：这里若做了 trim / 转义改写，对端解出来的 @ 就不是发出去的那个。
    #[test]
    fn gossip_plaintext_never_rewrites_content_bytes() {
        let raw = "  @周工 请看\t中文换行\n以及 emoji 😀  尾部空白  ";
        let back = parse_gossip_plaintext(gossip_plaintext("text", raw, None).as_bytes());
        assert_eq!(back.content, raw);
    }

    #[test]
    fn parse_gossip_plaintext_falls_back_to_lossy_text_for_non_json() {
        // 这是搬过来之前就有的行为（老版本发过纯文本），必须原样保住
        let back = parse_gossip_plaintext("随便一句话".as_bytes());
        assert_eq!(back.kind, "text");
        assert_eq!(back.content, "随便一句话");
        assert_eq!(back.mentions, None);
    }

    #[test]
    fn parse_gossip_plaintext_treats_a_wrong_shaped_mentions_as_absence() {
        // 键在但形状不对（对端实现不同 / 被改坏）⇒ 判成"不知道"，走昵称兜底，
        // 而不是判成"谁都没 @"把红点掐掉。
        let cases = [
            r#"{"kind":"text","content":"@周工","mentions":"dev-b"}"#,
            r#"{"kind":"text","content":"@周工","mentions":3}"#,
            r#"{"kind":"text","content":"@周工","mentions":null}"#,
            r#"{"kind":"text","content":"@周工","mentions":{"a":1}}"#,
        ];
        for c in cases {
            let back = parse_gossip_plaintext(c.as_bytes());
            assert_eq!(back.mentions, None, "错误形状必须判成缺失：{c}");
        }
    }

    #[test]
    fn parse_gossip_plaintext_sanitizes_an_otherwise_valid_list() {
        // 数组 ⇒ 权威。但元素仍要洗：非字符串丢掉、空串丢掉、重复压掉（顺序按首次出现）。
        let c = r#"{"kind":"text","content":"@周工","mentions":["a","",1,"b","a",true,"c"]}"#;
        let back = parse_gossip_plaintext(c.as_bytes());
        assert_eq!(
            back.mentions,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn parse_gossip_plaintext_caps_the_mention_list() {
        // 载荷是群密钥加密的，但"只有成员能发"不等于"成员可以为所欲为"：
        // 一条消息塞十万个 id 会把每一条消息的解析与前端判定都拖垮。
        let ids: Vec<String> = (0..(MAX_GOSSIP_MENTIONS + 50))
            .map(|i| format!("d{i}"))
            .collect();
        let joined = ids
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(",");
        let c = format!(r#"{{"kind":"text","content":"@x","mentions":[{joined}]}}"#);
        let back = parse_gossip_plaintext(c.as_bytes());
        assert_eq!(
            back.mentions.as_ref().map(|v| v.len()),
            Some(MAX_GOSSIP_MENTIONS)
        );
    }

    /// 出网前也要洗：`send_group_message` 是 IPC 公开面，任何调用方都能递进一份名单。
    #[test]
    fn gossip_plaintext_sanitizes_before_it_leaves_the_device() {
        let messy = vec![
            "a".to_string(),
            String::new(),
            "b".to_string(),
            "a".to_string(),
            "c".to_string(),
        ];
        let pt = gossip_plaintext("text", "@x", Some(&messy));
        let v: serde_json::Value = serde_json::from_str(&pt).unwrap();
        assert_eq!(v.get("mentions"), Some(&serde_json::json!(["a", "b", "c"])));
    }

    #[test]
    fn encode_decode_are_inverse_for_the_default_group_text_case() {
        // 环回一条：编码器写出的东西解码器必须原样读回来 —— 这条一旦红，
        // 说明某一侧改了键名或形状，而对端还蒙在鼓里。
        let ids = vec!["dev-x".to_string()];
        let pt = gossip_plaintext("text", "@小王 开会", Some(&ids));
        let back = parse_gossip_plaintext(pt.as_bytes());
        assert_eq!(back.kind, "text");
        assert_eq!(back.content, "@小王 开会");
        assert_eq!(back.mentions, Some(ids));
    }

    /// **未知 kind 的预览必须是占位文案，绝不能是载荷**（INV-P24 第 2 条）。
    ///
    /// 为什么值得单独钉：`preview_text` 的默认分支过去**无条件透传正文**，于是对端版本比
    /// 本机新时（或 4.22.1 之前写坏的 kind 形态），会话列表与系统通知会直接显示一串裸 JSON。
    /// 而它的行为是"少显示点东西"，没有任何测试会因为缺少它而失败。
    /// 三个对照分支同样重要：`text`/`system` 必须照旧透传（否则"全都塞进占位"也能让第一条
    /// 断言通过 = 空转），历史 `video` 行必须归一成 `[文件]`（把真实消息判成"不支持"同样是错）。
    #[test]
    fn preview_text_hides_payload_for_unknown_kind() {
        let payload = r#"{"question":"周五前交","options":["A","B"]}"#;
        assert_eq!(preview_text("sticker", payload), UNSUPPORTED_PREVIEW_LABEL);
        assert!(
            !preview_text("sticker", payload).contains('周'),
            "未知 kind 的预览不得外泄载荷内容"
        );
        // 对照 1：已知但没有专门分支的 kind ⇒ 正文本来就是给人看的，照旧截断
        assert_eq!(preview_text("text", "hello"), "hello");
        assert_eq!(preview_text("system", "X 加入了群聊"), "X 加入了群聊");
        // 对照 2：4.22.1 之前写坏的历史行 ⇒ 归一化成 [文件]，不是"不支持"
        assert_eq!(
            preview_text("video", r#"{"name":"a.mp4","path":"/x/a.mp4","size":1}"#),
            "[文件]"
        );
    }

    /// `is_known_kind` 与 `kind_class` 的区别必须成立（前者能认出"未知"）。
    ///
    /// 只钉一条判据：`kind_class` 对未知值回落到 Bubble（设计上就看不出未知），
    /// 所以渲染层判"未知"必须走 `is_known_kind` —— 这条测试挡住有人把两者混用一个。
    #[test]
    fn only_is_known_kind_can_tell_unrecognized_apart() {
        assert!(!is_known_kind("sticker"), "表里没有的 kind 必须判为未知");
        assert_eq!(kind_class("sticker"), KindClass::Bubble, "回落仍是 Bubble");
        for (k, _) in WIRE_KINDS {
            assert!(is_known_kind(k), "{k} 在表里却判成未知");
        }
    }

    use crate::crypto::Identity;

    // ---------------- announce 自签名 ----------------

    /// 按线上形态构造一条自签名 announce。
    fn signed_announce(id: &Identity, device_id: &str, port: u16, nonce: &str) -> super::UdpPacket {
        let x = id.x25519_public_b64();
        let e = id.ed25519_public_b64();
        let sig = id.sign_b64(&super::announce_signing_bytes(
            device_id, port, nonce, &x, &e,
        ));
        super::UdpPacket {
            kind: "announce".to_string(),
            device_id: device_id.to_string(),
            nickname: "nick".to_string(),
            tcp_port: port,
            x25519_pubkey: Some(x),
            ed25519_pubkey: Some(e),
            nonce: nonce.to_string(),
            sig,
        }
    }

    #[test]
    fn announce_verified_when_self_signed() {
        let id = Identity::generate();
        let pkt = signed_announce(&id, "dev-a", 59992, "n1");
        assert_eq!(super::verify_announce(&pkt), super::AnnounceAuth::Verified);
    }

    /// 篡改任何**被签名覆盖**的字段都必须失败 —— 这是「防篡改」的全部内容。
    #[test]
    fn announce_rejects_tampering_on_every_signed_field() {
        let id = Identity::generate();
        let base = signed_announce(&id, "dev-a", 59992, "n1");

        let mut p = base.clone();
        p.device_id = "victim".to_string();
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "改 device_id"
        );

        let mut p = base.clone();
        p.tcp_port = 1;
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "改 tcp_port"
        );

        let mut p = base.clone();
        p.nonce = "n2".to_string();
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "改 nonce"
        );

        // 换成攻击者自己的公钥（想把绑定指向自己的密钥）
        let attacker = Identity::generate();
        let mut p = base.clone();
        p.ed25519_pubkey = Some(attacker.ed25519_public_b64());
        p.x25519_pubkey = Some(attacker.x25519_public_b64());
        assert!(
            matches!(super::verify_announce(&p), super::AnnounceAuth::Invalid(_)),
            "换公钥"
        );

        // nickname **不在**签名范围内（改名不该让签名失效），故意不测它
        let mut p = base.clone();
        p.nickname = "换个昵称".to_string();
        assert_eq!(
            super::verify_announce(&p),
            super::AnnounceAuth::Verified,
            "nickname 不参与签名"
        );
    }

    /// 用别人的公钥声称自己是对方：签名一定对不上（攻击者没有对方私钥）。
    #[test]
    fn announce_rejects_forged_signature_with_victim_pubkey() {
        let attacker = Identity::generate();
        let victim = Identity::generate();
        let vk = victim.ed25519_public_b64();
        let vx = victim.x25519_public_b64();
        // 攻击者用**自己的**私钥签，却声明受害者的公钥
        let sig = attacker.sign_b64(&super::announce_signing_bytes(
            "victim", 59992, "n1", &vx, &vk,
        ));
        let pkt = super::UdpPacket {
            kind: "announce".to_string(),
            device_id: "victim".to_string(),
            nickname: String::new(),
            tcp_port: 59992,
            x25519_pubkey: Some(vx),
            ed25519_pubkey: Some(vk),
            nonce: "n1".to_string(),
            sig,
        };
        assert!(matches!(
            super::verify_announce(&pkt),
            super::AnnounceAuth::Invalid(_)
        ));
    }

    /// 旧端不签名 → 放行（Legacy）。硬拒会让旧端在局域网内彻底不可见，
    /// 而 announce 本就不能用于身份绑定（公钥恒为 keys_verified=false），放行的风险可控。
    #[test]
    fn announce_without_signature_is_legacy_not_rejected() {
        let id = Identity::generate();
        let mut pkt = signed_announce(&id, "dev-a", 59992, "n1");
        pkt.sig = String::new();
        assert_eq!(super::verify_announce(&pkt), super::AnnounceAuth::Legacy);

        // who_has：不带公钥也不带签名，是正常形态
        let probe = super::UdpPacket {
            kind: "who_has".to_string(),
            device_id: "dev-a".to_string(),
            nickname: String::new(),
            tcp_port: 59992,
            x25519_pubkey: None,
            ed25519_pubkey: None,
            nonce: String::new(),
            sig: String::new(),
        };
        assert_eq!(super::verify_announce(&probe), super::AnnounceAuth::Legacy);
    }

    /// 带签名却缺 nonce / 缺公钥 → 无法防重放或无法验签，必须拒。
    #[test]
    fn announce_rejects_signed_but_incomplete_packets() {
        let id = Identity::generate();

        let mut p = signed_announce(&id, "dev-a", 59992, "n1");
        p.nonce = String::new();
        assert!(matches!(
            super::verify_announce(&p),
            super::AnnounceAuth::Invalid(_)
        ));

        let mut p = signed_announce(&id, "dev-a", 59992, "n1");
        p.x25519_pubkey = None;
        assert!(matches!(
            super::verify_announce(&p),
            super::AnnounceAuth::Invalid(_)
        ));

        let mut p = signed_announce(&id, "dev-a", 59992, "n1");
        p.ed25519_pubkey = Some(String::new());
        assert!(matches!(
            super::verify_announce(&p),
            super::AnnounceAuth::Invalid(_)
        ));
    }

    /// 签名材料对每个字段敏感（防止将来有人漏字段导致"改了也能过"）。
    #[test]
    fn announce_signing_bytes_sensitive_to_every_field() {
        let base = super::announce_signing_bytes("a", 1, "n", "x", "e");
        assert_ne!(base, super::announce_signing_bytes("b", 1, "n", "x", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 2, "n", "x", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 1, "m", "x", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 1, "n", "y", "e"));
        assert_ne!(base, super::announce_signing_bytes("a", 1, "n", "x", "f"));
        // 与 Hello 的材料必须不同域（前缀不同），否则一个协议的签名能拿到另一个用
        assert_ne!(
            base,
            super::hello_signing_bytes("a", 1, "n", "x", "e"),
            "announce 与 Hello 的签名材料必须域分离"
        );
    }

    /// 表情 token 的**形态**校验：挡畸形与超长，但**不判断表情是否存在**
    /// （目录的唯一来源是前端，后端再存一份就是第二个真相源）。
    #[test]
    fn emoji_token_shape_is_validated_but_not_the_catalogue() {
        assert!(super::is_valid_emoji_token("[赞]"));
        assert!(super::is_valid_emoji_token("[微笑]"));
        // 后端不认识的名字也必须放行 —— 前端加了新表情不该需要同时改后端
        assert!(super::is_valid_emoji_token("[后端不认识的表情]"));
        for bad in [
            "",
            "[",
            "]",
            "[]",
            "赞",
            "[赞",
            "赞]",
            "[[赞]]",
            "[赞][踩]",
            "[a\nb]",
        ] {
            assert!(!super::is_valid_emoji_token(bad), "{bad:?} 应被拒");
        }
        assert!(
            !super::is_valid_emoji_token(&format!("[{}]", "很".repeat(20))),
            "超长应被拒"
        );
    }

    /// 回应载荷的线上往返（发送端序列化 → 接收端反序列化）。
    #[test]
    fn reaction_payload_roundtrips() {
        let p = super::ReactionPayload {
            target: "msg-1".to_string(),
            emoji: "[赞]".to_string(),
            add: true,
        };
        let wire = serde_json::to_string(&p).unwrap();
        let back: super::ReactionPayload = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.target, "msg-1");
        assert_eq!(back.emoji, "[赞]");
        assert!(back.add);
        // 与群消息 payload 同形（{"kind","content"} 里的 content 就是它）
        assert!(wire.contains("\"add\":true"));
    }

    /// Phase 8（ADR-0017）：不透明外部帧的边界校验 —— **畸形/超限只丢该帧，不断链**。
    #[test]
    fn opaque_external_validation_bounds() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let ok = STANDARD.encode(b"bitchat-packet");
        assert_eq!(
            super::validate_opaque_external("pkt-1", 3, &ok).unwrap(),
            b"bitchat-packet"
        );
        assert!(super::validate_opaque_external("pkt-1", 0, &ok).is_err());
        assert!(super::validate_opaque_external("pkt-1", super::MAX_OPAQUE_TTL + 1, &ok).is_err());
        assert!(super::validate_opaque_external("", 3, &ok).is_err());
        assert!(
            super::validate_opaque_external(&"x".repeat(super::MAX_OPAQUE_ID + 1), 3, &ok).is_err()
        );
        assert!(super::validate_opaque_external("bad id!", 3, &ok).is_err());
        assert!(super::validate_opaque_external("pkt-1", 3, "not base64!!").is_err());
        assert!(super::validate_opaque_external("pkt-1", 3, "").is_err());
        let huge = STANDARD.encode(vec![0u8; super::MAX_OPAQUE_PAYLOAD + 1]);
        assert!(super::validate_opaque_external("pkt-1", 3, &huge).is_err());
    }

    /// 线格式必须能原样往返（Gosslan 不解码载荷，只透传）。
    #[test]
    fn opaque_external_round_trips_through_wire_format() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let payload = STANDARD.encode(vec![0u8, 1, 2, 250, 255]);
        let msg = super::Message::OpaqueExternal {
            id: "pkt-9".to_string(),
            ttl: 5,
            payload: payload.clone(),
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let back: super::Message = serde_json::from_slice(&json).unwrap();
        match back {
            super::Message::OpaqueExternal {
                id,
                ttl,
                payload: p,
            } => {
                assert_eq!(id, "pkt-9");
                assert_eq!(ttl, 5);
                assert_eq!(p, payload);
            }
            other => panic!("往返后类型变了：{other:?}"),
        }
    }

    fn env() -> GossipEnvelope {
        GossipEnvelope {
            message_id: String::new(),
            sender_id: "dev-a".into(),
            nonce: "nonce-1".into(),
            sender_pubkey: "xk".into(),
            sender_ed25519: "ek".into(),
            sender_sig: "sig".into(),
            ttl: 6,
            kind: GossipKind::Chat,
            group_id: None,
            group_name: None,
            group_creator: None,
            group_members: Vec::new(),
            payload: "ciphertext".into(),
            ts: 123456,
            seq: 1,
            encrypted: true,
            target: None,
        }
    }

    /// 好友申请（定向）信封：加密、签名、验签、解密、target 完整性。
    #[test]
    fn friend_request_envelope_encrypt_sign_decrypt_and_target_integrity() {
        use crate::crypto::Identity;
        use crate::gossip_engine::GossipEngine;
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

        let a = Identity::generate();
        let c = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        // A 构造 FriendRequest（target=C，用 C 的 X25519 公钥加密内容）
        let payload = r#"{"from_nickname":"Alice","from_avatar":null}"#;
        let shared =
            crate::crypto::shared_secret(&a.x25519_secret, &c.x25519_public_b64()).unwrap();
        let sealed = crate::crypto::seal(&shared, payload.as_bytes()).unwrap();
        let payload_b64 = B64.encode(&sealed);
        let mut env = engine.build_envelope(
            &a,
            "dev-a",
            GossipKind::FriendRequest,
            None,
            None,
            &payload_b64,
            123456,
            0,
        );
        env.target = Some("dev-c".into());
        env.sender_sig = a.sign_b64(&env.signing_bytes());

        // 验签通过（target 参与签名）
        assert!(engine.verify_envelope(&env));

        // C 用自己的私钥解开内容
        let shared2 = crate::crypto::shared_secret(&c.x25519_secret, &env.sender_pubkey).unwrap();
        let pt = crate::crypto::open(&shared2, &B64.decode(&env.payload).unwrap()).unwrap();
        assert_eq!(String::from_utf8(pt).unwrap(), payload);

        // 中间节点篡改 target → 验签失败（target 不可篡改）
        let mut tampered = env.clone();
        tampered.target = Some("dev-eve".into());
        assert!(!engine.verify_envelope(&tampered));

        // target 序列化：None 不写键，Some 写入
        let json_none = serde_json::to_string(&GossipEnvelope {
            target: None,
            ..env.clone()
        })
        .unwrap();
        assert!(
            !json_none.contains("target"),
            "None 不应写 target 键: {json_none}"
        );
        let json_some = serde_json::to_string(&env).unwrap();
        assert!(json_some.contains("dev-c"), "Some 应写 target: {json_some}");
    }

    /// ChatAck / ChatReadReceipt（定向、明文）信封：签名、验签、target 完整性、明文往返。
    #[test]
    fn chat_ack_and_read_receipt_plaintext_directed_envelope_integrity() {
        use crate::crypto::Identity;
        use crate::gossip_engine::GossipEngine;
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

        let c = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        // ChatAck：接收方 C 回给原始发送方 A，明文 { msg_id }
        let ack_payload = r#"{"msg_id":"deadbeef"}"#;
        let mut ack = engine.build_envelope(
            &c,
            "dev-c",
            GossipKind::ChatAck,
            None,
            None,
            &B64.encode(ack_payload.as_bytes()),
            123456,
            0,
        );
        ack.encrypted = false;
        ack.target = Some("dev-a".into());
        ack.sender_sig = c.sign_b64(&ack.signing_bytes());

        // 验签通过（target 参与签名）
        assert!(engine.verify_envelope(&ack));
        // 明文：直接 base64 解码即可得到原始 JSON，无需解密
        assert_eq!(B64.decode(&ack.payload).unwrap(), ack_payload.as_bytes());
        // 篡改 target → 验签失败
        let mut tampered = ack.clone();
        tampered.target = Some("dev-eve".into());
        assert!(!engine.verify_envelope(&tampered));

        // ChatReadReceipt：定向明文，payload 含 last_read_ts / last_read_msg_id
        let rr_payload = r#"{"last_read_ts":99,"last_read_msg_id":"m-1"}"#;
        let mut rr = engine.build_envelope(
            &c,
            "dev-c",
            GossipKind::ChatReadReceipt,
            None,
            None,
            &B64.encode(rr_payload.as_bytes()),
            123456,
            0,
        );
        rr.encrypted = false;
        rr.target = Some("dev-a".into());
        rr.sender_sig = c.sign_b64(&rr.signing_bytes());
        assert!(engine.verify_envelope(&rr));
        assert_eq!(B64.decode(&rr.payload).unwrap(), rr_payload.as_bytes());
    }

    #[test]
    fn envelope_encrypted_flag_roundtrip() {
        // 显式 false 往返保持 false
        let mut e = env();
        e.encrypted = false;
        e.compute_message_id();
        let json = serde_json::to_string(&Message::Gossip {
            envelope: e.clone(),
        })
        .unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => assert!(!envelope.encrypted),
            _ => panic!("expect gossip"),
        }

        // 未声明加密标志的旧信封直接拒绝，不再兼容旧协议。
        let legacy = r#"{"type":"gossip","envelope":{"message_id":"m","sender_id":"a","sender_pubkey":"x","sender_ed25519":"e","sender_sig":"s","ttl":6,"kind":"chat","group_id":null,"payload":"p","ts":1}}"#;
        assert!(serde_json::from_str::<Message>(legacy).is_err());
    }

    #[test]
    fn gossip_message_id_deterministic_and_sensitive_to_payload() {
        let mut e1 = env();
        e1.compute_message_id();
        let id1 = e1.message_id.clone();
        assert_eq!(id1.len(), 64); // SHA-256 hex

        let mut e2 = e1.clone();
        e2.compute_message_id();
        assert_eq!(id1, e2.message_id); // 同内容同 id

        e2.payload = "tampered".into();
        e2.compute_message_id();
        assert_ne!(id1, e2.message_id); // 篡改 payload → id 变化

        // 时间戳不参与消息身份：改变 ts 不应改变 message_id。
        let mut e3 = e1.clone();
        e3.ts = 999_999;
        e3.compute_message_id();
        assert_eq!(id1, e3.message_id);

        // nonce 参与消息身份：改变 nonce 必须改变 message_id。
        let mut e4 = e1.clone();
        e4.nonce = "nonce-2".into();
        e4.compute_message_id();
        assert_ne!(id1, e4.message_id);
    }

    #[test]
    fn message_json_roundtrip() {
        let mut e = env();
        e.compute_message_id();
        let msg = Message::Gossip {
            envelope: e.clone(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => {
                assert_eq!(envelope.message_id, e.message_id);
                assert_eq!(envelope.sender_id, "dev-a");
            }
            _ => panic!("应还原为 Gossip 消息"),
        }
    }

    #[test]
    fn msg_kind_mapping() {
        assert_eq!(MsgKind::from_wire_str("code"), MsgKind::Code);
        assert_eq!(MsgKind::from_wire_str("unknown"), MsgKind::Text);
        assert_eq!(MsgKind::Code.as_str(), "code");
    }

    /// 诊断用的类型名必须与**线格式**一致（真机排查只认日志里这个词）。
    ///
    /// 为什么这条测试值得存在：BLE 握手失败时日志现在会写「对端首帧不是 Hello（收到 xxx）」，
    /// `xxx` 就是 `wire_kind()` 的输出。若哪天有人把它改成手写 match 又漏了变体，
    /// 这里会立刻红 —— 而不是等到真机上看着一个错误的类型名猜半天。
    #[test]
    fn wire_kind_matches_the_serde_tag() {
        let hello = Message::Hello {
            device_id: "dev-a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: super::content_features(),
            protocol_version: Some(super::PROTOCOL_VERSION),
            app_version: Some(super::current_app_version().to_string()),
            tcp_port: 59992,
            x25519_pubkey: "xk".into(),
            ed25519_pubkey: "ek".into(),
            conv_clock: 0,
            nonce: "n1".into(),
            sig: "sig".into(),
        };
        assert_eq!(hello.wire_kind(), "hello");
        // 与真实序列化结果的 `type` 字段逐字一致（不是"看起来差不多"）
        let v: serde_json::Value = serde_json::to_value(&hello).unwrap();
        assert_eq!(v["type"], serde_json::json!("hello"));

        assert_eq!(
            Message::Heartbeat {
                device_id: "dev-a".into()
            }
            .wire_kind(),
            "heartbeat"
        );
    }

    /// **serde 事实**：`Message` 是 `#[serde(tag = "type")]` 枚举，未知 `type` 在
    /// `from_slice` 这一层**就是硬错误**。
    ///
    /// ⚠️ 立场已变（ADR-0007 Accepted 2026-09-20 + INV-P24 第 1 条）：这个错误**不允许**
    /// 一路冒到 `reader_loop` —— 旧行为是 `read_frame` 返回 `InvalidData` ⇒ 断开整条连接，
    /// 于是新版本只要上线一种新帧，老设备就不是"少收一条"而是"跟这台设备连不上"，
    /// 还伴随重连-再拆的死循环。现在由 `network::transport::decode_frame` 在帧层降级成
    /// `Message::Unknown`（忽略 + 节流日志，链路保持），并由守卫
    /// `unknown_wire_frame_is_tolerated_after_auth` 钉住。
    ///
    /// 这条测试保留的理由：它钉的是 serde 层的既有事实（降级逻辑正是依赖
    /// "未知变体报 unknown variant"这个措辞来判别），并带一条**防空转对照**
    /// （已知变体必须能解析，否则"全都失败"也会让断言看起来通过）。
    #[test]
    fn unknown_message_type_is_a_hard_parse_error() {
        let unknown = br#"{"type":"some_future_kind","id":"x"}"#;
        assert!(
            serde_json::from_slice::<Message>(unknown).is_err(),
            "serde 层必须仍然报未知 type —— decode_frame 靠这个错误把未知帧降级，而不是当畸形帧"
        );
        // 对照：已知变体必须能解析（否则上面那条断言会因为"全都解析失败"而变成空转）。
        // `Heartbeat` 需要 `device_id`，这里给全字段。
        let known = br#"{"type":"heartbeat","device_id":"dev-a"}"#;
        assert!(
            serde_json::from_slice::<Message>(known).is_ok(),
            "对照用例必须能解析，否则上面的断言是空转（全都失败也算通过）"
        );
    }

    #[test]
    fn hello_signing_bytes_sensitive_to_every_field() {
        let base = hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek");
        // 相同输入必须产出相同字节（签名可复现）
        assert_eq!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek"));
        // 任一字段变化都必须改变签名材料：否则攻击者可平移字段伪造身份
        assert_ne!(base, hello_signing_bytes("dev-b", 59992, "n1", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 1, "n1", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n2", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk2", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek2"));
        // 拼接歧义防护：把不同字段切成另一种组合不应撞车
        assert_ne!(
            hello_signing_bytes("ab", 1, "c", "d", "e"),
            hello_signing_bytes("a", 1, "bc", "d", "e")
        );
    }

    #[test]
    fn hello_carries_nonce_and_sig_roundtrip() {
        let hello = Message::Hello {
            device_id: "dev-a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: super::content_features(),
            protocol_version: None,
            app_version: None,
            tcp_port: 59992,
            x25519_pubkey: "xk".into(),
            ed25519_pubkey: "ek".into(),
            conv_clock: 7,
            nonce: "n1".into(),
            sig: "sig".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello { nonce, sig, .. } => {
                assert_eq!(nonce, "n1");
                assert_eq!(sig, "sig");
            }
            _ => panic!("expect hello"),
        }
        // 不带 nonce/sig 的旧 Hello 仍可解析（serde default），但会在验证层被拒
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello { nonce, sig, .. } => {
                assert!(nonce.is_empty() && sig.is_empty());
            }
            _ => panic!("expect hello"),
        }
    }

    /// device_type：序列化往返保持，旧 Hello 缺省为空串（旧端兼容，不参与签名）。
    #[test]
    fn hello_device_type_roundtrip_and_legacy_default() {
        let hello = Message::Hello {
            device_id: "a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "mobile".into(),
            content_features: super::content_features(),
            protocol_version: None,
            app_version: None,
            tcp_port: 1,
            x25519_pubkey: "x".into(),
            ed25519_pubkey: "e".into(),
            conv_clock: 0,
            nonce: "n".into(),
            sig: "s".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello { device_type, .. } => assert_eq!(device_type, "mobile"),
            _ => panic!("expect hello"),
        }
        // 旧 Hello（无 device_type 字段）→ 缺省空串
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello { device_type, .. } => assert!(device_type.is_empty()),
            _ => panic!("expect hello"),
        }
    }

    /// 版本声明（ADR-0007 决策 1）：新端往返保持，**老 Hello 缺省为 `None`**。
    ///
    /// 这一步的全部价值来自"加这两个字段不断老版本互通"，所以两个方向都要钉：
    /// - 老→新：缺字段的 Hello 必须照样解析（报错就是老设备**连不上**，不是"少个信息"）；
    /// - 新→老：我们多带的字段必须被忽略 —— 判据就是这里**没有** `deny_unknown_fields`，
    ///   一旦有人加上，"给帧加字段"这件事本身会变成一次破坏性变更。
    #[test]
    fn hello_version_fields_roundtrip_and_old_peer_declares_nothing() {
        let hello = Message::Hello {
            device_id: "a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: super::content_features(),
            protocol_version: Some(2),
            app_version: Some("9.9.9".into()),
            tcp_port: 1,
            x25519_pubkey: "x".into(),
            ed25519_pubkey: "e".into(),
            conv_clock: 0,
            nonce: "n".into(),
            sig: "s".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello {
                protocol_version,
                app_version,
                ..
            } => {
                assert_eq!(protocol_version, Some(2));
                assert_eq!(app_version.as_deref(), Some("9.9.9"));
            }
            _ => panic!("expect hello"),
        }
        // 老端（4.22.27 之前）的 Hello：没有这两个字段 ⇒ 必须是 `None`。
        // 不能回落成 Some(1)：诊断面板要能区分"对方是没报版本的老版本"和"对方报了 1"。
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello {
                protocol_version,
                app_version,
                ..
            } => {
                assert_eq!(protocol_version, None);
                assert_eq!(app_version, None);
            }
            _ => panic!("expect hello"),
        }
        // 新→老：未知**字段**必须被忽略（这条断言就是"不许 deny_unknown_fields"的哨兵）。
        let newer = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0,"protocol_version":9,"some_future_field":true}"#;
        assert!(
            serde_json::from_str::<Message>(newer).is_ok(),
            "Hello 遇到未知字段必须照单收下，否则加字段就等于破坏性变更"
        );
    }

    /// **未知 kind 不得把整条 `chat_message` 带崩**（INV-P24 第 2 条）。
    ///
    /// 这条钉的是"`ChatMessage.kind` 为什么是 String 而不是 `MsgKind`"：只要它是枚举，
    /// `kind:"sticker"` 就会让 serde 报 `unknown variant`，而 `decode_frame` 分不清
    /// "未知**帧**类型"与"未知**嵌套枚举值**" ⇒ 整帧被降级成 `Message::Unknown` 丢弃
    /// ⇒ 消息根本进不了库，前端连「不支持的消息类型」都来不及显示，发送方永远等不到 Ack。
    /// 所以正确的判据是：**同一个未知值放在 kind 上必须还能解析，放在 type 上才该降级**。
    #[test]
    fn unknown_message_kind_still_decodes_as_a_chat_message() {
        let frame = br#"{"type":"chat_message","msg_id":"m1","from":"a","to":"b","kind":"sticker","content":"enc1:AAA","ts":1,"seq":1}"#;
        match serde_json::from_slice::<Message>(frame)
            .expect("未知 kind 必须仍能解析成 ChatMessage（否则整条消息会被静默丢弃）")
        {
            Message::ChatMessage { kind, msg_id, .. } => {
                assert_eq!(kind, "sticker", "kind 必须原样保留，不能回落成 text");
                assert_eq!(msg_id, "m1");
            }
            other => panic!("应还原为 ChatMessage，实得 {other:?}"),
        }
        // 对照（防空转）：未知值放在 **type** 上时仍是硬解析错误 —— 那才是
        // `decode_frame` 该降级成 Unknown 的场景（见 unknown_message_type_is_a_hard_parse_error）。
        assert!(serde_json::from_slice::<Message>(br#"{"type":"sticker"}"#).is_err());
    }

    /// 兼容判定的四种输入 —— `Some(PROTOCOL_VERSION)` 那条是**防空转的关键**：
    /// 写成 `>=` 就会在每个同版本好友上刷"对方版本较新"，而真网里同版本才是常态。
    #[test]
    fn peer_protocol_newer_only_when_declared_higher() {
        assert!(!peer_protocol_is_newer(None), "未声明 ≠ 更高（也 ≠ 更低）");
        assert!(
            !peer_protocol_is_newer(Some(PROTOCOL_VERSION)),
            "同版本必须判不高 —— 写成 >= 就会满屏误报"
        );
        assert!(!peer_protocol_is_newer(Some(0)), "0 也不猜成更高");
        assert!(peer_protocol_is_newer(Some(PROTOCOL_VERSION + 1)));
    }

    /// 发送侧门控的四种输入 —— 关键是 `0`（没交换过 Hello / 对端已离线）判**不许发**。
    /// 方向选"不知道就当不支持"是刻意的：宁可少发一条新类型，也不要让老对端整帧丢掉、
    /// 发送方还以为是网络问题。
    #[test]
    fn gated_kind_needs_the_peers_own_declaration() {
        assert!(
            kind_allowed_by_features("text", 0),
            "V1 词表内的 kind 对所有对端都安全，不该被门控挡住"
        );
        assert!(
            !kind_allowed_by_features("merge", 0),
            "对端没声明能力 ⇒ 不许发 merge"
        );
        assert!(kind_allowed_by_features("merge", CONTENT_FEATURE_MERGE));
        // 只认自己那一位：对方有别的 capability 不算数（防"位图非零就放行"这种糊法）
        assert!(
            !kind_allowed_by_features("merge", CONTENT_FEATURE_PULL),
            "按位判定，不是按非零判定"
        );
    }

    /// 门控挡下时的两句话必须分得开：**没条目**只是"不知道"，不该被说成"它版本旧"。
    ///
    /// 为什么单独立一条：`kind_allowed_by_features` 把"不知道"并进 0 是对的方向（宁可少发），
    /// 但那句"对方的 Gosslan 版本较旧"于是会在对方只是不在线时变成一次假指控 —— 用户照着
    /// 去催对方升级，而真正要做的只是等对方上线。**分开的只是解释，不是决策。**
    #[test]
    fn blocked_hint_distinguishes_unknown_from_declared_unsupported() {
        let unknown = kind_blocked_hint("merge", None);
        let declared = kind_blocked_hint("merge", Some(CONTENT_FEATURE_PULL));
        assert!(
            !unknown.contains("版本较旧"),
            "能力未知时不许断言对方版本旧：缺条目多半只是它此刻不在线"
        );
        assert!(
            unknown.contains("不在线") && unknown.contains("重新发送"),
            "未知那句要说清下一步能做什么（失败气泡上有「重新发送」），而不是只报个失败"
        );
        assert_eq!(
            declared,
            kind_unsupported_hint("merge"),
            "对端自己声明过缺位时沿用原句，不许出现第二份文案"
        );
        assert!(
            !kind_allowed_by_features("merge", 0),
            "两种情况都得先挡下来：这里改的是说法，不是门控方向"
        );
    }

    /// 加了门控却忘了在 Hello 里声明 ⇒ 这种消息永远发不出去，而且是静默的。
    /// 这条测试盯的就是"门控表与广播的能力对不上"这个组合。
    #[test]
    fn every_gated_kind_is_advertised_by_us() {
        let ours = content_features();
        let mut gated = Vec::new();
        for (kind, _) in WIRE_KINDS {
            if let Some(bit) = kind_required_feature(kind) {
                assert_ne!(
                    ours & bit,
                    0,
                    "kind `{kind}` 要求能力位 {bit:#b}，但本机 Hello 没声明它 —— 门控会把我们自己的功能锁死"
                );
                gated.push(bit);
            }
        }
        assert!(
            gated.contains(&CONTENT_FEATURE_MERGE),
            "merge 必须仍在被门控之列：v4.20.0 及更早的 MsgKind 里没有这个变体"
        );
    }

    /// 群受众统计必须**三态分开**：确知不支持 / 版本未知 / 支持。
    ///
    /// 为什么单独立一条：把"未知"并进"不支持"在 1:1 发送口是对的方向（宁可少发），
    /// 在群提示上是错的（离线成员是常态 ⇒ 提示永远在响 ⇒ 没人再看）。这条测试钉的就是
    /// 两个函数**不该共用一个默认值**。
    #[test]
    fn group_audience_keeps_unknown_apart_from_unsupported() {
        let members: Vec<String> = ["a", "b", "c", "me"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // a 声明过但没有 merge 位；b 声明了 merge 位；c 从没交换过 Hello（未知）
        let features = |id: &str| match id {
            "a" => Some(CONTENT_FEATURE_PULL),
            "b" => Some(CONTENT_FEATURE_MERGE),
            "c" => None,
            _ => Some(content_features()),
        };
        let (unsupported, unknown, gated) = kind_audience("merge", features, &members, "me");
        assert!(gated, "merge 是受能力位约束的 kind");
        assert_eq!(unsupported, vec!["a".to_string()], "只数确知缺位的");
        assert_eq!(unknown, 1, "没交换过 Hello 的算未知，不算缺位");

        // 不受约束的 kind：一个都不该报，也不该弹提示
        let (u2, k2, g2) = kind_audience("text", features, &members, "me");
        assert!(!g2 && u2.is_empty() && k2 == 0, "text 对所有版本安全");

        // 只有未知、没有确知缺位 ⇒ 句子为空（宁可不说话，也不说一条永远在响的话）
        let only_unknown: Vec<String> = ["c".to_string()].into_iter().collect();
        let (u3, k3, _) = kind_audience("merge", features, &only_unknown, "me");
        assert!(u3.is_empty() && k3 == 1);
        assert_eq!(kind_audience_hint("merge", &[], only_unknown.len()), "");

        let hint = kind_audience_hint("merge", &["小李".to_string(), "阿强".to_string()], 3);
        assert!(
            hint.contains("小李") && hint.contains("阿强"),
            "要列得出是谁"
        );
        assert!(hint.contains("不会丢"), "必须说清只是渲染退化，不是丢消息");
        assert!(
            hint.contains("3 位成员") && hint.contains("版本未知"),
            "未知的另计，不冒充缺位"
        );
    }

    /// 缺位人数很多时不刷屏：列出前几个 + 「等 N 名成员」。
    #[test]
    fn audience_hint_collapses_long_name_lists() {
        let names: Vec<String> = (0..9).map(|i| format!("成员{i}")).collect();
        let hint = kind_audience_hint("merge", &names, 0);
        assert!(hint.contains("等 9 名成员"), "{hint}");
        assert!(
            hint.contains("成员0") && !hint.contains("成员8"),
            "只展示前几个"
        );
    }

    #[test]
    fn group_lifecycle_messages_roundtrip() {
        let changed = Message::GroupCreatorChanged {
            group_id: "g1".into(),
            from: "old".into(),
            to: "new".into(),
        };
        let json = serde_json::to_string(&changed).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupCreatorChanged { group_id, from, to } => {
                assert_eq!(
                    (group_id.as_str(), from.as_str(), to.as_str()),
                    ("g1", "old", "new")
                );
            }
            _ => panic!("expect group_creator_changed"),
        }

        let left = Message::GroupMemberLeft {
            group_id: "g1".into(),
            from: "dev-a".into(),
        };
        let json = serde_json::to_string(&left).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupMemberLeft { group_id, from } => {
                assert_eq!((group_id.as_str(), from.as_str()), ("g1", "dev-a"));
            }
            _ => panic!("expect group_member_left"),
        }
    }

    #[test]
    fn group_ack_and_file_complete_ack_roundtrip() {
        let group_ack = Message::GroupAck {
            group_id: "g1".into(),
            msg_id: "m1".into(),
            from: "dev-a".into(),
        };
        let json = serde_json::to_string(&group_ack).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupAck {
                group_id,
                msg_id,
                from,
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(msg_id, "m1");
                assert_eq!(from, "dev-a");
            }
            _ => panic!("expect group_ack"),
        }

        let file_ack = Message::FileCompleteAck {
            transfer_id: "t1".into(),
            success: true,
        };
        let json = serde_json::to_string(&file_ack).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::FileCompleteAck {
                transfer_id,
                success,
            } => {
                assert_eq!(transfer_id, "t1");
                assert!(success);
            }
            _ => panic!("expect file_complete_ack"),
        }
    }

    /// `attempt` 必须是**双向都不破坏**的可选字段（2026-09-23 attempt epoch）。
    ///
    /// 两个方向分开钉，因为它们的失效模式完全不同：
    /// · 老端 → 新端：JSON 里根本没有这个键 ⇒ 解析必须成功（`serde(default)`）；
    /// · 新端 → 老端：本机没声明能力时**整个字段必须消失**（`skip_serializing_if`）⇒
    ///   字节层面与旧版本一致，不去赌老端的反序列化器容不容得下未知字段。
    #[test]
    fn file_frame_attempt_field_is_wire_compatible_both_ways() {
        let legacy_shape = Message::FileChunk {
            transfer_id: "t1".into(),
            seq: 3,
            data: "AA==".into(),
            attempt: None,
        };
        let json = serde_json::to_string(&legacy_shape).unwrap();
        assert!(
            !json.contains("attempt"),
            "不带轮次时字段必须整个消失，否则新端发出去的帧老端可能整帧解析失败：{json}"
        );
        // 上面这份 JSON 就是"老端发来的形状"，能读回来即证明向后兼容
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::FileChunk { seq, attempt, .. } => {
                assert_eq!(seq, 3);
                assert_eq!(
                    attempt, None,
                    "缺键必须读成 None（= 旧语义），不能读成第 0 轮"
                );
            }
            _ => panic!("expect file_chunk"),
        }
        // 带轮次时新字段必须原样往返
        let with_epoch = Message::FileDone {
            transfer_id: "t1".into(),
            attempt: Some(9),
        };
        let json2 = serde_json::to_string(&with_epoch).unwrap();
        assert!(json2.contains("\"attempt\":9"), "{json2}");
        match serde_json::from_str::<Message>(&json2).unwrap() {
            Message::FileDone { attempt, .. } => assert_eq!(attempt, Some(9)),
            _ => panic!("expect file_done"),
        }
    }

    /// 门控用了哪一位，`content_features()` 就必须声明哪一位。
    ///
    /// `every_gated_kind_is_advertised_by_us` 只管"按 kind 门控"的那批；`attempt` 是
    /// **按帧字段**门控、不落在 `kind_required_feature` 里，所以它必须在这里单独钉 ——
    /// 漏声明的表现是"epoch 静默不生效"，测试与真机都不容易第一时间看出来。
    #[test]
    fn file_epoch_feature_is_advertised() {
        assert_ne!(
            content_features() & CONTENT_FEATURE_FILE_EPOCH,
            0,
            "声明了 FILE_EPOCH 门控却没 advertise ⇒ 对端永远收不到 attempt"
        );
        // 新位必须独占一个 bit（撞车的表现是"两个门控互相误开"，编译器不会报）
        assert_eq!(
            CONTENT_FEATURE_FILE_EPOCH & (CONTENT_FEATURE_PULL | CONTENT_FEATURE_MERGE),
            0,
            "能力位撞车：FILE_EPOCH 复用了已经被占用的 bit"
        );
    }

    // ---------------- #122：呈现层要能区分"同名的两个人" ----------------
    // 判定层早在 31eccbd 就绑了设备 id；剩下的这一半是**哪一段文字算谁**。
    // 用「第几次出现」而不是字符偏移：Rust 按字节、JS 按 UTF-16，偏移跨语言一定错
    // （一个 emoji 就能把两边的编号错开），而"@昵称 第 n 次出现"在两端数出来是同一个东西。
    #[test]
    fn build_mention_targets_numbers_repeated_names_in_text_order() {
        let content = "辛苦 @张三 和 @李四，@张三 记得归档";
        let ids = vec![
            "id-zs".to_string(),
            "id-ls".to_string(),
            "id-zs2".to_string(),
        ];
        let name_of = |id: &str| match id {
            "id-zs" | "id-zs2" => Some("张三".to_string()),
            "id-ls" => Some("李四".to_string()),
            _ => None,
        };
        let t = build_mention_targets(content, &ids, name_of);
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].id.as_str(), t[0].n), ("id-zs", 1));
        assert_eq!((t[1].id.as_str(), t[1].n), ("id-ls", 1));
        // 同名第二个人拿到的是"第二次出现"，不是又指回第一次 ⇒ 这就是 #122 的正身
        assert_eq!((t[2].id.as_str(), t[2].n), ("id-zs2", 2));
        assert_eq!(t[2].name, "张三");
    }

    #[test]
    fn build_mention_targets_skips_unknown_ids_and_absent_names() {
        let content = "只有 @张三 在正文里";
        let ids = vec!["ghost".to_string(), "id-zs".to_string()];
        let t = build_mention_targets(content, &ids, |id| {
            (id == "id-zs").then_some("张三".to_string())
        });
        assert_eq!(t.len(), 1, "查不到昵称的 id 不该造出一个落点");
        assert_eq!(t[0].id, "id-zs");
        // 名字压根不在正文里 ⇒ 不分配 n（否则接收端会把 n=1 对到别的那段文字上）
        let t2 = build_mention_targets("正文里没有 at", &["id-zs".to_string()], |id| {
            (id == "id-zs").then_some("张三".to_string())
        });
        assert!(t2.is_empty());
    }

    #[test]
    fn gossip_plaintext_with_targets_roundtrips_and_old_shape_still_parses() {
        let targets = vec![MentionTarget {
            id: "id-zs".to_string(),
            name: "张三".to_string(),
            n: 1,
        }];
        let s = gossip_plaintext_with_targets(
            "text",
            "@张三 看一下",
            Some(&["id-zs".to_string()][..]),
            Some(&targets[..]),
        );
        let back = parse_gossip_plaintext(s.as_bytes());
        assert_eq!(back.mentions.as_deref(), Some(&["id-zs".to_string()][..]));
        assert_eq!(back.mention_targets.as_deref(), Some(&targets[..]));
        // 老形状（只有 mentions 数组、没有 mention_targets）⇒ 新字段是 None，呈现层退回按昵称
        let old = r#"{"kind":"text","content":"@张三","mentions":["id-zs"]}"#;
        let b2 = parse_gossip_plaintext(old.as_bytes());
        assert_eq!(b2.mentions.as_deref(), Some(&["id-zs".to_string()][..]));
        assert!(b2.mention_targets.is_none(), "缺键 = 不知道，不是空数组");
        // 三态不许压成两态：Some([]) 是"明确没 @ 任何人"，与 None 不同
        let none = r#"{"kind":"text","content":"hi","mentions":[]}"#;
        let b3 = parse_gossip_plaintext(none.as_bytes());
        assert_eq!(b3.mentions.as_deref(), Some(&[][..]));
        // 落点这个键**没带** ⇒ None（不是空数组）：呈现层按昵称兜底，但判定层仍吃上面那个 Some([])。
        // 显式写 `"mention_targets":[]` 才是"发送方权威地说：一个落点都没有"。
        assert!(b3.mention_targets.is_none(), "缺键 = 不知道");
        let b5 = parse_gossip_plaintext(
            r#"{"kind":"text","content":"hi","mentions":[],"mention_targets":[]}"#.as_bytes(),
        );
        assert_eq!(b5.mention_targets.as_deref(), Some(&[][..]));
        let b4 = parse_gossip_plaintext(r#"{"kind":"text","content":"hi"}"#.as_bytes());
        assert!(b4.mentions.is_none() && b4.mention_targets.is_none());
    }

    #[test]
    fn malformed_mention_targets_degrade_to_none_not_partial() {
        for raw in [
            r#"{"kind":"text","content":"@x","mentions":["a"],"mention_targets":"nope"}"#,
            r#"{"kind":"text","content":"@x","mentions":["a"],"mention_targets":[{"id":"a"}]}"#,
            r#"{"kind":"text","content":"@x","mentions":["a"],"mention_targets":[{"id":"","name":"x","n":1}]}"#,
            r#"{"kind":"text","content":"@x","mentions":["a"],"mention_targets":[{"id":"a","name":"x","n":0}]}"#,
        ] {
            let b = parse_gossip_plaintext(raw.as_bytes());
            assert!(
                b.mention_targets.is_none(),
                "半截/畸形落点必须整份判成不知道，而不是收下一半：{raw}"
            );
            assert_eq!(
                b.mentions.as_deref(),
                Some(&["a".to_string()][..]),
                "另一条腿不受牵连"
            );
        }
    }
}
