// 职责边界：
// - transport 行为测试分册之10 —— 直连 E2EE 载荷：解封成功/失败/自愈、换钥后重密封、明文与篡改一律拒
// 为什么拆：`transport/tests.rs` 原来 3,227 行、7 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
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
