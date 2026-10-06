// 职责边界：
// - file.rs 的行为测试（从主文件搬出来，`include!` 回同一模块 ⇒ 模块路径 file_tests::tests::* 一字不变）
// 为什么搬：主文件 4396 行里这一段占 1942 行（约 44%），生产码只 2454 行。
// 搬动机制与 db/favorites_tests.rs 同一条：include! 是文本粘贴 ⇒ 同一模块、同一 use、同一可见性，
// 编译器判等价，测试路径不变 ⇒ 测试清单基线也不该有任何差异。
#[cfg(test)]
mod tests {
    use super::{
        chunk_exceeds_declared, chunk_seq_decision, classify_file_subtype, clear_file_wire_progress_in, derive_file_name,
        file_peer_key, receive_is_stale, safe_file_name, safe_transfer_id, unique_path,
        wire_progress_bytes, ChunkSeq, WireLedger, FILE_RECEIVE_IDLE_ABORT_MS,
        MAX_FILE_OUTBOX_RETRIES,
    };

    /// 写出记账必须**随发送尝试一起回收**（v4.22.38）。
    ///
    /// 重点是"提前 return 那条路"：`stream_file` 有十来处早退（取消/链路关闭/加密失败/`?`），
    /// 旧写法只在成功路径清一次 ⇒ 失败后重试会读到上一次的 `chunks`，进度悄悄退回
    /// "按入队算"，也就是把 v4.22.37 那条修复抹掉。这里用本地表复现同一个 Drop 语义
    /// （造不出也不需要造 AppState —— 要验的就是"离开作用域就没残留"）。
    ///
    /// 键用的是 `file_peer_key(transfer, 收件人)`：回收的单位是"这一次给这个人的尝试"。
    #[test]
    fn wire_ledger_is_reclaimed_on_every_exit_path() {
        use crate::state::FileWireProgress;
        use std::collections::HashMap;
        use std::sync::Mutex;

        let table: Mutex<HashMap<String, FileWireProgress>> = Mutex::new(HashMap::new());
        let key = file_peer_key("t1", "peer-a");
        let attempt = |early: bool| -> Result<(), ()> {
            let _ledger = WireLedger {
                table: &table,
                wire_key: key.clone(),
            };
            // 模拟 writer 记账：这一片真的写出去了
            crate::network::transport::bump_file_wire_progress_in(&table, &key, 1);
            if early {
                return Err(());
            }
            Ok(())
        };

        assert!(attempt(false).is_ok());
        assert!(table.lock().unwrap().is_empty(), "正常结束必须回收这条记账");
        assert!(attempt(true).is_err());
        assert!(
            table.lock().unwrap().is_empty(),
            "提前 return 也必须回收 —— 旧写法漏的就是这一条"
        );
    }

    /// 同一次群投递里，每个收件人必须有**自己那份**写出记账（#35）。
    ///
    /// 为什么单独立一条：群发是 N 个任务共用一个 `transfer_id`（`group_file_dispatch.rs`），
    /// 键只按 id 记时两种坏行为都是静默的 ——
    ///   · `at_ms` 被任何一个人的写出刷新 ⇒ 真卡死的那个人永远判不出停滞；
    ///   · 进度按 `chunks` 折算 ⇒ 别人走过的量算进这一条链路，界面比实际快。
    /// 回收同理：谁先收尾就把整条传输的记录删掉，剩下还在写的人从此没有记账。
    #[test]
    fn wire_progress_is_counted_per_recipient_within_one_group_transfer() {
        use crate::state::FileWireProgress;
        use std::collections::HashMap;
        use std::sync::Mutex;

        let table: Mutex<HashMap<String, FileWireProgress>> = Mutex::new(HashMap::new());
        let a = file_peer_key("t1", "peer-a");
        let b = file_peer_key("t1", "peer-b");
        assert_ne!(a, b, "同一个 transfer 的两个收件人必须各自成键");
        assert_ne!(
            a,
            "t1".to_string(),
            "裸 transfer_id 不能是合法键 —— 否则新旧口径会互相读到"
        );

        for i in 0..3 {
            crate::network::transport::bump_file_wire_progress_in(&table, &a, i + 1);
        }
        crate::network::transport::bump_file_wire_progress_in(&table, &b, 9);

        let snap = table.lock().unwrap();
        assert_eq!(snap.get(&a).unwrap().chunks, 3);
        assert_eq!(
            snap.get(&b).unwrap().chunks,
            1,
            "甲走过的片数不许算到乙头上（进度与停滞判定都会偏）"
        );
        assert_eq!(snap.get(&b).unwrap().at_ms, 9, "各自的时刻也必须各自记");
        drop(snap);

        // 甲先收尾：只许删甲自己那一条。
        clear_file_wire_progress_in(&table, &a);
        let after = table.lock().unwrap();
        assert!(
            after.contains_key(&b),
            "回收必须按收件人，否则先结束的成员会把还在写的成员的记账删掉"
        );
        assert_eq!(after.get(&b).unwrap().chunks, 1);
    }

    /// 发送进度必须按"**已写出链路**"算，不按入队算（v4.22.37）。
    ///
    /// 症状（真机 600MB）：`send_on_link` 成功只代表进了那条链路的 1024 槽队列，
    /// LAN 一片 256KB ⇒ 最多 262MB 还在排队时界面已经 100%。①⑤ 就是这条主症状；
    /// ②③④ 各钉一个换算边界（续传前缀、短片、计数器残留）。
    #[test]
    fn progress_counts_written_chunks_not_enqueued_bytes() {
        let chunk = 256 * 1024usize;
        let enqueued = 1024 * chunk as u64; // 一整条队列都灌满了
                                            // ① 主症状：1024 片全入队、链路只走了 1 片 ⇒ 进度就是 1 片
        assert_eq!(
            wire_progress_bytes(0, chunk, enqueued, 1, enqueued * 2),
            chunk as u64
        );
        // ⑤ 一片都没写出 ⇒ 0（旧口径这里已经是 262MB）
        assert_eq!(wire_progress_bytes(0, chunk, enqueued, 0, enqueued * 2), 0);
        // ② 断点续传：from_bytes 是对端已持有的前缀，天然算"已上路"，必须计入
        assert_eq!(
            wire_progress_bytes(
                1_000_000,
                chunk,
                1_000_000 + 5 * chunk as u64,
                3,
                10_000_000
            ),
            1_000_000 + 3 * chunk as u64
        );
        // ③ 最后一片是短片：按整片折算会越过文件总大小 ⇒ 钳到 size
        let size = 3 * chunk as u64 + 10;
        assert_eq!(wire_progress_bytes(0, chunk, size, 4, size), size);
        // ④ 计数器残留得比本机读出来的还多 ⇒ 绝不能超过已入队量
        assert_eq!(wire_progress_bytes(0, chunk, 7, 999, 10_000), 7);
    }

    /// **收到分片的判定规则**（2026-09-13 审计的真缺陷，必须钉住）。
    ///
    /// 反例就是用户报的那条「文件分片顺序错误」：发送方重试时 `seq` 从 0 重来，
    /// 而旧实现把"重复/迟到"也当成致命错误 ⇒ 接收方整单失败 ⇒ 新 attempt 永远拼不齐。
    #[test]
    fn chunk_seq_rule_only_rejects_real_gaps() {
        // 正好下一片 ⇒ 写入
        assert_eq!(chunk_seq_decision(0, 0), ChunkSeq::Accept);
        assert_eq!(chunk_seq_decision(7, 7), ChunkSeq::Accept);
        // 重复 / 迟到（重传时上一轮的残片）⇒ **忽略**，不许失败
        assert_eq!(chunk_seq_decision(0, 3), ChunkSeq::Duplicate);
        assert_eq!(chunk_seq_decision(2, 3), ChunkSeq::Duplicate);
        // 跳号（中间真缺片）⇒ 报错，靠重传补齐
        assert_eq!(chunk_seq_decision(4, 3), ChunkSeq::Gap);
        // 边界：u32 极值也不 panic
        assert_eq!(chunk_seq_decision(u32::MAX, u32::MAX - 1), ChunkSeq::Gap);
    }

    #[test]
    fn image_extensions() {
        for n in ["a.png", "a.jpg", "a.jpeg", "a.gif", "a.webp"] {
            assert_eq!(classify_file_subtype(n), "image", "{n}");
        }
    }

    #[test]
    fn code_extensions() {
        for n in [
            "a.rs", "a.ts", "a.tsx", "a.js", "a.jsx", "a.vue", "a.py", "a.go", "a.java", "a.c",
            "a.cpp", "a.h", "a.hpp", "a.json", "a.yaml", "a.yml", "a.html", "a.css", "a.sql",
            "a.sh",
        ] {
            assert_eq!(classify_file_subtype(n), "code", "{n}");
        }
    }

    #[test]
    fn markdown_is_a_document_not_code() {
        // Markdown 是文档不是代码：发送 .md 文件应按文件卡片渲染，而不是代码预览块。
        assert_eq!(classify_file_subtype("a.md"), "file");
        assert_eq!(classify_file_subtype("README.MD"), "file");
    }

    #[test]
    fn other_files() {
        for n in [
            "a.exe", "a.zip", "a.pdf", "a.docx", "a.txt", "Makefile", "LICENSE",
        ] {
            assert_eq!(classify_file_subtype(n), "file", "{n}");
        }
    }

    #[test]
    fn case_insensitive_and_multidot_and_chinese() {
        assert_eq!(classify_file_subtype("PHOTO.PNG"), "image");
        assert_eq!(classify_file_subtype("App.Vue"), "code");
        assert_eq!(classify_file_subtype("archive.tar.gz"), "file"); // 末段 gz 不在清单
        assert_eq!(classify_file_subtype("min.bundle.js"), "code"); // 末段 js
        assert_eq!(classify_file_subtype("报告 截图.JPG"), "image"); // 中文名 + 空格
        assert_eq!(classify_file_subtype("代码.rs"), "code");
        assert_eq!(classify_file_subtype(".gitignore"), "file"); // 隐藏文件无有效扩展
        assert_eq!(classify_file_subtype(""), "file");
    }

    #[test]
    fn rejects_path_traversal_file_names() {
        for name in ["../secret.txt", "..\\secret.txt", "/tmp/secret", "..", ""] {
            assert!(safe_file_name(name).is_none(), "{name} must be rejected");
        }
        assert_eq!(safe_file_name("report.txt").as_deref(), Some("report.txt"));
    }

    #[test]
    fn derive_file_name_normal() {
        // 正常路径有扩展名 → 直接取
        assert_eq!(
            derive_file_name("/storage/emulated/0/DCIM/Camera/VID_001.mp4"),
            "VID_001.mp4"
        );
        assert_eq!(
            derive_file_name("/home/user/Downloads/report.pdf"),
            "report.pdf"
        );
    }

    #[test]
    fn derive_file_name_temporal_file_missing_ext() {
        // Tauri Android 临时文件：file_name() 没扩展名，但完整路径末尾有 .mp4
        assert_eq!(
            derive_file_name("content://media/external/video/media/123456/VID_20250918.mp4"),
            "VID_20250918.mp4"
        );
        assert_eq!(
            derive_file_name("/data/data/com.gosslan.app/cache/478812312.jpg"),
            "478812312.jpg"
        );
    }

    #[test]
    fn derive_file_name_no_ext_anywhere() {
        // 真的没有扩展名 → 原样返回
        assert_eq!(derive_file_name("/tmp/README"), "README");
        assert_eq!(derive_file_name("/tmp/478812312"), "478812312");
    }

    /// `transfer_id` 会被拼成 `{id}.part` 落盘，必须与文件名同级消毒。
    /// 未校验时一个 `../../../../Users/me/Documents/x` 就能让 `File::create`
    /// 在下载目录之外创建/截断文件（内容由对端控制）。
    #[test]
    fn rejects_path_traversal_transfer_ids() {
        for id in [
            "../../../../Users/me/Documents/report",
            "..\\..\\windows\\system32\\x",
            "/etc/passwd",
            "a/b",
            "a\\b",
            "..",
            ".",
            "",
            "with space",
            "null\0byte",
            "中文 id",
        ] {
            assert!(safe_transfer_id(id).is_none(), "{id:?} 必须被拒");
        }
        // 长度上限：超长 id 会成为超长文件名
        assert!(safe_transfer_id(&"a".repeat(65)).is_none());
        assert!(safe_transfer_id(&"a".repeat(64)).is_some());
    }

    /// 生产端用 UUID、E2E 用连字符短 id —— 合法形态一个都不能被误杀。
    #[test]
    fn accepts_real_world_transfer_ids() {
        for id in [
            "3f2504e0-4f89-11d3-9a0c-0305e82c3301", // Uuid::new_v4()
            "e2e-file-001",
            "e2e-group-image-001",
            "e2e-dl-001",
            "ABCdef123_-",
        ] {
            assert_eq!(safe_transfer_id(id).as_deref(), Some(id), "{id} 不应被拒");
        }
    }

    /// `unique_path` 在任何分支下都不得返回已存在的路径。
    /// 旧实现在「同名文件已达 999 个」时直接 `return base`（base 必定已存在），
    /// 会静默覆盖用户已有文件——这条测试锁定该兜底分支。
    #[test]
    fn unique_path_never_returns_existing_path() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("gosslan-uniquepath-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 无冲突：原样返回
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a.txt"));

        // 占满基准名与 1..999 全部候选名，逼出随机后缀分支
        fs::write(dir.join("a.txt"), b"x").unwrap();
        for i in 1..1000 {
            fs::write(dir.join(format!("a ({i}).txt")), b"x").unwrap();
        }
        let got = unique_path(&dir, "a.txt");
        assert!(!got.exists(), "返回了已存在的路径，会覆盖用户文件：{got:?}");
        assert_ne!(got, dir.join("a.txt"));

        // 无扩展名走同一分支
        fs::write(dir.join("README"), b"x").unwrap();
        for i in 1..1000 {
            fs::write(dir.join(format!("README ({i})")), b"x").unwrap();
        }
        let got2 = unique_path(&dir, "README");
        assert!(!got2.exists(), "返回了已存在的路径：{got2:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    // ---------- 文件传输 E2EE（协议层模拟，不依赖 AppState） ----------

    use super::super::super::crypto;
    use super::chunk_size_for_path;
    use super::{
        frame_is_current, refuse_reason_for_best_link, send_deadline_for, sha256_file_hex,
        stall_verdict, valid_sha256_hex, StallVerdict, BLE_FILE_SIZE_LIMIT, FILE_SEND_DEADLINE,
        FILE_STALL_ABORT_MS, FILE_STALL_WARN_MS,
    };
    use crate::protocol::FILE_CHUNK;
    use std::time::Duration;

    /// **分块大小必须能真的被 BLE 分片层发出去**（真机 2026-09-13：大图两边都显示成功、
    /// 对方列表里却没有）。这条测试是**行为级**的：直接把两种分块大小喂给真正的
    /// `fragment()`，用默认 MTU（23 ⇒ 20 字节载荷 ⇒ 每片 14 字节）。
    ///
    /// 旧行为（256 KiB）在这一步会返回 `None` ⇒ 写循环把它当写失败并**拆掉整条链路**
    /// ⇒ 传输永远完不成，而发送方界面照样显示"已发送/已读"。
    #[test]
    fn ble_file_chunk_actually_fits_the_ble_fragment_layer() {
        use crate::transport::ble_framing::{fragment, MAX_BLE_CHUNKS_PER_MESSAGE};
        let mtu = 20; // MTU 23 - 3 字节 ATT 头

        // ① BLE 的分块大小必须能分片成功，且离上限有充足余量
        let ble_chunk = vec![0u8; chunk_size_for_path("bluetooth")];
        let chunks = fragment(&ble_chunk, mtu, 1)
            .expect("BLE 分块大小必须能被分片 —— 否则写循环会拆掉整条链路");
        assert!(
            chunks.len() * 4 <= MAX_BLE_CHUNKS_PER_MESSAGE,
            "分片数 {} 必须离上限 {} 有 ≥4× 余量",
            chunks.len(),
            MAX_BLE_CHUNKS_PER_MESSAGE
        );

        // ② 对照：桌面默认的 256 KiB 在小 MTU 上**分不出片**（这正是那个 bug 的形态）
        assert!(
            fragment(&vec![0u8; FILE_CHUNK], mtu, 1).is_none(),
            "256 KiB 在 MTU=23 上必然超过分片上限 —— 这条断言把 bug 的成因钉在测试里"
        );

        // ③ 非蓝牙链路仍用大块（局域网带宽高，小块会拖慢吞吐）
        assert_eq!(chunk_size_for_path("lan"), FILE_CHUNK);
        assert_eq!(chunk_size_for_path("routed"), FILE_CHUNK);
    }

    use base64::Engine as _;

    /// 在系统临时目录创建唯一的 .part 文件（测试接收端用），返回句柄与路径。
    fn temp_part(tag: &str) -> (std::fs::File, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-{tag}-{}.part", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let f = std::fs::File::create(&path).unwrap();
        (f, path)
    }

    /// 模拟接收端 write_chunk 的核心序列：AEAD 解密 → 增量哈希 → 写 .part。
    /// （write_chunk 本体需要 AppState，此处按相同操作序列驱动 FileReceiver。）
    fn receive_one_chunk(r: &mut crate::state::FileReceiver, seq: u32, sealed: &[u8]) {
        assert_eq!(seq, r.next_seq, "write_chunk 语义：seq 必须严格递增");
        use sha2::Digest;
        let plaintext = crypto::open_symmetric(&r.file_key, sealed).expect("解密失败");
        r.hasher.update(&plaintext);
        std::io::Write::write_all(&mut r.file, &plaintext).unwrap();
        r.received += plaintext.len() as u64;
        r.next_seq += 1;
    }

    /// 模拟 finish_receive 的最终裁决：size 一致 + SHA-256 一致才算完成。
    fn finish_verdict(r: &mut crate::state::FileReceiver) -> Result<(), String> {
        use sha2::Digest;
        if r.received != r.size {
            return Err("文件传输未完成".to_string());
        }
        let actual_hex: String = r
            .hasher
            .clone()
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
            return Err("文件完整性校验失败".to_string());
        }
        Ok(())
    }

    fn hex_of(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 1. FileOffer.sealed_file_key：发送方以接收方公钥封装、接收方解封，
    ///    必须还原出同一个文件会话密钥（ECDH 对称性）。
    #[test]
    fn file_offer_sealed_key_roundtrip() {
        let sender = crypto::Identity::generate();
        let receiver = crypto::Identity::generate();
        let file_key = crypto::random_key();

        // 发送端（send_file_from_path 同逻辑）：receiver 公钥封装
        let shared = crypto::shared_secret(&sender.x25519_secret, &receiver.x25519_public_b64())
            .expect("ECDH 失败");
        let sealed_key_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal(&shared, &file_key).expect("封装失败"));

        // 接收端（handle_message FileOffer 同逻辑）：sender 公钥解封
        let shared_rx = crypto::shared_secret(&receiver.x25519_secret, &sender.x25519_public_b64())
            .expect("ECDH 失败");
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&sealed_key_b64)
            .expect("base64 非法");
        let opened = crypto::open(&shared_rx, &sealed).expect("解封失败");
        assert_eq!(opened.len(), 32);
        assert_eq!(opened, file_key, "解封出的文件会话密钥必须与原密钥一致");
    }

    /// 2. FileChunk 加密→解密 roundtrip：原始 bytes 完整还原。
    #[test]
    fn file_chunk_encrypt_roundtrip() {
        let file_key = crypto::random_key();
        let plaintext: Vec<u8> = (0u8..=255).cycle().take(FILE_CHUNK).collect();
        let sealed = crypto::seal_symmetric(&file_key, &plaintext).expect("加密失败");
        let opened = crypto::open_symmetric(&file_key, &sealed).expect("解密失败");
        assert_eq!(opened, plaintext);
    }

    /// 3. 密文被篡改后解密必须失败（AEAD 完整性），不能产出可用明文。
    #[test]
    fn tampered_chunk_fails_to_decrypt() {
        let file_key = crypto::random_key();
        let plaintext = b"gosslan file chunk";
        let mut sealed = crypto::seal_symmetric(&file_key, plaintext).expect("加密失败");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF; // 翻转密文最后一比特
        assert!(
            crypto::open_symmetric(&file_key, &sealed).is_none(),
            "篡改后的密文必须解密失败"
        );
    }

    /// 4. 每个 transfer 生成独立的随机文件会话密钥，不得共用。
    #[test]
    fn distinct_transfers_have_distinct_keys() {
        let a = crypto::random_key();
        let b = crypto::random_key();
        assert_ne!(a, b, "两次 random_key() 必须产生不同密钥（CSPRNG）");
        // 密文互换后必须解不开：证明密钥确实互不通用
        let msg = b"content of transfer";
        let sealed_with_a = crypto::seal_symmetric(&a, msg).unwrap();
        assert!(crypto::open_symmetric(&b, &sealed_with_a).is_none());
    }

    /// 5. 中继节点原样转发密文（RelayChunk 只透传 data），
    ///    接收端用自己解封的会话密钥仍可解密——中继无需也无法解密。
    #[test]
    fn relay_forwarded_ciphertext_still_decryptable() {
        let sender = crypto::Identity::generate();
        let receiver = crypto::Identity::generate();
        let file_key = crypto::random_key();

        // 发送端：封装会话密钥 + 加密 chunk
        let shared =
            crypto::shared_secret(&sender.x25519_secret, &receiver.x25519_public_b64()).unwrap();
        let sealed_key_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal(&shared, &file_key).unwrap());
        let plaintext = b"chunk travels through relay nodes";
        let ciphertext_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal_symmetric(&file_key, plaintext).unwrap());

        // 模拟中继：data 原样透传（无密钥、无修改）——中继不可见明文
        let forwarded = ciphertext_b64.clone();

        // 接收端：解封密钥 → 解密转发的密文
        let shared_rx =
            crypto::shared_secret(&receiver.x25519_secret, &sender.x25519_public_b64()).unwrap();
        let opened_key: [u8; 32] = crypto::open(
            &shared_rx,
            &base64::engine::general_purpose::STANDARD
                .decode(&sealed_key_b64)
                .unwrap(),
        )
        .unwrap()
        .try_into()
        .unwrap();
        let decrypted = crypto::open_symmetric(
            &opened_key,
            &base64::engine::general_purpose::STANDARD
                .decode(&forwarded)
                .unwrap(),
        )
        .expect("中继转发后的密文必须仍可解密");
        assert_eq!(decrypted, plaintext);
    }

    /// 6. 完整传输生命周期（协议层）：解封密钥 → 多分片逐片加解密 →
    ///    拼接还原 + 大小校验通过——与 finish_receive 的裁决一致。
    #[test]
    fn full_transfer_lifecycle_still_completes() {
        let sender = crypto::Identity::generate();
        let receiver = crypto::Identity::generate();

        // 原始文件：3 片（末片不满 256KB，覆盖边界）
        let mut original: Vec<u8> = Vec::new();
        for i in 0..(FILE_CHUNK * 3 - 1234) {
            original.push((i % 251) as u8);
        }
        let chunks: Vec<&[u8]> = original.chunks(FILE_CHUNK).collect();

        // 发送端生命周期：random_key → 封装 → 逐片加密
        let file_key = crypto::random_key();
        let shared =
            crypto::shared_secret(&sender.x25519_secret, &receiver.x25519_public_b64()).unwrap();
        let sealed_key_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal(&shared, &file_key).unwrap());
        let wire_chunks: Vec<Vec<u8>> = chunks
            .iter()
            .map(|c| crypto::seal_symmetric(&file_key, c).unwrap())
            .collect();

        // 接收端生命周期：解封密钥 → 逐片解密重组 → 大小校验
        let shared_rx =
            crypto::shared_secret(&receiver.x25519_secret, &sender.x25519_public_b64()).unwrap();
        let restored_key: [u8; 32] = crypto::open(
            &shared_rx,
            &base64::engine::general_purpose::STANDARD
                .decode(&sealed_key_b64)
                .unwrap(),
        )
        .unwrap()
        .try_into()
        .unwrap();
        let mut assembled: Vec<u8> = Vec::new();
        for (seq, wire) in wire_chunks.iter().enumerate() {
            // seq 严格递增校验（write_chunk 语义）：乱序片在这里被拒绝
            assert_eq!(seq as usize, assembled.chunks(FILE_CHUNK).count());
            let plain = crypto::open_symmetric(&restored_key, wire)
                .unwrap_or_else(|| panic!("分片 {seq} 解密失败"));
            assembled.extend_from_slice(&plain);
        }
        assert_eq!(assembled.len(), original.len(), "重组大小必须一致");
        assert_eq!(assembled, original, "重组内容必须与原文件一致");
    }

    // ---------- 文件级 SHA-256 完整性校验 ----------

    /// attempt epoch 的过滤判据（2026-09-23 真机 600MB 的根治点，用户拍板选 B）。
    ///
    /// 病根形状：一轮超时后 outbox 重投，而**上一轮已经塞进链路队列的分片不会被撤回**
    /// （Low 队列 1024 槽 ≈ 262MB）。它们带着上一轮的序号落到新一轮上，旧代码会判成
    /// "跳号"⇒ 整单打死。三条设计各自都要钉住：老端不带字段 ⇒ 恒真（升级不许改行为）、
    /// 只有相等才算当前轮、**比本机新的轮次也不算当前**。
    #[test]
    fn stale_attempt_frames_are_filtered_but_legacy_frames_never_are() {
        // 老端 / 对端没声明能力 ⇒ 字段缺席 ⇒ 完全旧语义
        assert!(frame_is_current(None, 0), "legacy 帧永远是当前轮");
        assert!(
            frame_is_current(None, 7),
            "本机已经在第 7 轮时，legacy 帧也不许被丢掉 —— 那是「升级就把对端发死」"
        );
        assert!(frame_is_current(Some(7), 7), "同轮 = 当前");
        assert!(
            !frame_is_current(Some(6), 7),
            "上一轮的残留必须丢：不丢就被下面的序号判据打成跳号、整单判死"
        );
        assert!(
            !frame_is_current(Some(8), 7),
            "未来轮次的帧也不能写：它的 Offer 还在另一条链路上排队。丢掉由「Offer + 续传」自愈，\
             写进文件里才是真事故（末尾 SHA 只能判死，救不回内容）"
        );
        assert!(
            frame_is_current(Some(0), 0),
            "0 不是哨兵，就是第一轮/未知轮的同值"
        );
    }

    /// 大文件在只剩蓝牙时不启动（详见 `BLE_FILE_SIZE_LIMIT` 的推导）。
    ///
    /// 钉的是**判据**而不是接线：BLE 上分片被压到 4KiB、带宽 ≈14KB/s，600MB 要十几小时，
    /// 而单轮 deadline 封顶 1h ⇒ 必然反复超窗重投 ⇒ 重新选路 + 重新编号 ⇒ 接收端 Gap 判死，
    /// 并且 `file_sending` 按 peer 去重，会把同 peer 的其它文件一起堵死。
    #[test]
    fn ble_only_link_must_not_start_a_hopeless_large_file() {
        use crate::mesh::PathKind::*;
        let mb = 1024 * 1024;
        // 阈值内照发：这道闸不是"BLE 上一律不发文件"，那会牺牲既有的小图能力。
        assert_eq!(refuse_reason_for_best_link(Some(Bluetooth), 4 * mb), None);
        assert_eq!(
            refuse_reason_for_best_link(Some(Bluetooth), BLE_FILE_SIZE_LIMIT),
            None,
            "边界取「不超过就发」，别把阈值当成开区间悄悄改语义"
        );
        // 超阈值 ⇒ 不启动，且原因是给用户看的句子（要能看出是"等更好的链路"不是故障）
        let reason = refuse_reason_for_best_link(Some(Bluetooth), 600 * mb);
        assert!(reason.is_some(), "600MB 在只有蓝牙时必须拒绝启动");
        assert!(
            reason.unwrap().contains("蓝牙"),
            "原因必须点名是哪条链路不适合，不能只说“发送失败”"
        );
        // 只要还有别的链路可选就与蓝牙无关（判据只看**最佳**链路，不看是否存在蓝牙）
        assert_eq!(refuse_reason_for_best_link(Some(Lan), 600 * mb), None);
        assert_eq!(refuse_reason_for_best_link(Some(Routed), 2048 * mb), None);
        assert_eq!(refuse_reason_for_best_link(Some(Relay), 2048 * mb), None);
        // 完全没有链路时这里不表态（调用方另有 has_link 分支，两处不许互相抢判据）
        assert_eq!(refuse_reason_for_best_link(None, 600 * mb), None);
    }

    /// 大文件的发送期限必须随体积伸缩，**且按线上字节估**（2026-09-23 真机 600MB 复核）。
    /// 两个真机根因都钉在这里：固定 10min 窗口 = 必失败；按明文估 = 少给 25% 窗口 ⇒
    /// 慢链路上单轮注定超窗 ⇒ 只能靠重投 + `.part` 接力 ⇒ 撞上接收端的 Gap 判死。
    #[test]
    fn send_deadline_scales_with_size() {
        assert_eq!(
            send_deadline_for(1024),
            FILE_SEND_DEADLINE,
            "小文件保持 10min 下限"
        );
        // 600MiB：线上 = ×4/3 = 800MiB ⇒ 800MiB ÷ 512KiB/s = 1600s，+60s 余量 = 1660s。
        // 明文口径只会给 1260s（21min），所以这个精确值同时钉住了"不许退回明文估算"。
        assert_eq!(
            send_deadline_for(600 * 1024 * 1024),
            Duration::from_secs(1660),
            "600MiB 的窗口必须按线上字节算（明文口径是 1260s）"
        );
        assert!(
            send_deadline_for(600 * 1024 * 1024) > Duration::from_secs(25 * 60),
            "512KiB/s 下 600MiB 实需 26.7min，窗口必须容得下"
        );
        assert_eq!(
            send_deadline_for(u64::MAX),
            Duration::from_secs(60 * 60),
            "再大也封顶 1h，超出交给断点续传重试而不是吊死任务"
        );
    }

    /// 停滞判定的三档边界。钉的是"什么时候该提醒、什么时候该放弃"，
    /// 阈值本身写死在常量里，改常量必须同时改这里（防止有人顺手把 abort 调成 warn）。
    /// 接收侧静默回收的边界（P1 的另一半：延后"断链就清"之后，必须有东西来清）。
    ///
    /// 为什么这条判据必须存在（实测出来的，不是设想）：协议里**没有 cancel 帧**
    /// （`protocol.rs` 里 `Cancel` 零命中），发送侧 60s 停滞就自己 abort 并把这一单退回
    /// `file_outbox` —— 但它**从不告诉接收端**。而 `sweep_stale_parts` 的规矩是
    /// "只删不在这两张表里的 `.part`" ⇒ 一个被放弃的接收器会把**文件句柄 + `.part`
    /// 一起永久钉住**（24h 的清扫反而永远跳过它）。
    /// 所以"断一条链不再立刻清"必须配这条回收，否则只是把「杀错人」换成「泄漏」。
    #[test]
    fn stalled_receiver_is_reclaimed_only_after_the_idle_window() {
        use super::FILE_STALL_ABORT_MS;
        // 窗口内一律不回收：对端可能正在重连后重新 Offer（同一 transfer_id 会覆盖表项）
        assert!(!receive_is_stale(FILE_RECEIVE_IDLE_ABORT_MS - 1));
        assert!(!receive_is_stale(0));
        // 到点即回收（含恰好等于边界）
        assert!(receive_is_stale(FILE_RECEIVE_IDLE_ABORT_MS));
        assert!(receive_is_stale(i64::MAX));
        // 时钟倒挂（未来时间戳）不得被当成"已经静默了 2^63 毫秒"
        assert!(!receive_is_stale(-1));
        // 回收窗口必须**宽于**发送侧的 abort：否则接收端会在发送端还在重试的间隙里
        // 把自己那半截清掉，续传点位与对端的记录就此分裂。
        assert!(
            FILE_RECEIVE_IDLE_ABORT_MS > FILE_STALL_ABORT_MS,
            "接收侧窗口必须大于发送侧 abort（{} vs {}）",
            FILE_RECEIVE_IDLE_ABORT_MS,
            FILE_STALL_ABORT_MS
        );
    }

    #[test]
    fn stall_verdict_boundaries() {
        assert_eq!(stall_verdict(0), StallVerdict::Healthy);
        assert_eq!(
            stall_verdict(FILE_STALL_WARN_MS - 1),
            StallVerdict::Healthy,
            "还没到提醒线不得提前吓用户"
        );
        assert_eq!(stall_verdict(FILE_STALL_WARN_MS), StallVerdict::Warn);
        assert_eq!(
            stall_verdict(FILE_STALL_ABORT_MS - 1),
            StallVerdict::Warn,
            "提醒与放弃之间只有 Warn"
        );
        assert_eq!(stall_verdict(FILE_STALL_ABORT_MS), StallVerdict::Abort);
        assert_eq!(stall_verdict(i64::MAX), StallVerdict::Abort);
        assert!(
            FILE_STALL_WARN_MS < FILE_STALL_ABORT_MS,
            "提醒必须早于放弃，否则用户只看到突然失败"
        );
        assert!(
            FILE_STALL_ABORT_MS < FILE_SEND_DEADLINE.as_millis() as i64,
            "停滞放弃要早于最短 deadline，否则 deadline 才是唯一出口（界面会冻住十分钟）"
        );
    }

    /// sha256_file_hex：流式分块结果必须与一次性内存计算一致（发送端正确性）。
    #[test]
    fn sha256_file_hex_matches_in_memory_hash() {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-sha-{}.bin", std::process::id()));
        std::fs::write(&path, b"gosslan sha-256 streaming test body").unwrap();
        let got = sha256_file_hex(&path).unwrap();
        let want = hex_of(b"gosslan sha-256 streaming test body");
        let _ = std::fs::remove_file(&path);
        assert_eq!(got, want);
        assert_eq!(got.len(), 64, "hex 表示必须为 64 字符");
    }

    /// SHA-256 hex 字段格式校验（FileOffer 元数据），非法即拒绝。
    #[test]
    fn invalid_sha256_format_is_rejected() {
        assert!(valid_sha256_hex(&hex_of(b"ok")));
        assert!(
            valid_sha256_hex(&hex_of(b"ok").to_uppercase()),
            "大写 hex 也合法"
        );
        assert!(!valid_sha256_hex(""), "空串");
        assert!(!valid_sha256_hex("abc"), "长度不足");
        assert!(!valid_sha256_hex(&"a".repeat(63)), "63 位");
        assert!(!valid_sha256_hex(&"a".repeat(65)), "65 位");
        assert!(
            !valid_sha256_hex(&format!("{}g", "a".repeat(63))),
            "非 hex 字符"
        );
    }

    /// 空文件边界：SHA-256 已知值 + sha256_file_hex 对 0 字节文件正确。
    #[test]
    fn empty_file_sha256_matches_known_value() {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-empty-{}.bin", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let got = sha256_file_hex(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            got, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "空文件 SHA-256 必须是标准已知值"
        );
    }

    /// 取消登记必须**按收件人分键**（真机：三成员以上群文件只有一人收得到）。
    ///
    /// 钉的判据：单键时后注册的 `insert` 挤掉前一个任务的 `Sender`，对方的 oneshot
    /// 立刻 `Err(RecvError)` 完成，而投递循环的取消分支分不清"被取消"与"登记被顶替"
    /// ⇒ N-1 个成员以「用户取消发送」这个假原因当场中断。
    #[test]
    fn file_cancel_keys_are_scoped_per_recipient() {
        let a = super::file_cancel_key("t1", "peer-a");
        let b = super::file_cancel_key("t1", "peer-b");
        assert_ne!(a, b, "同一 transfer_id 的不同收件人必须各自成键");
        let prefix = super::file_cancel_prefix("t1");
        assert!(
            a.starts_with(prefix.as_str()) && b.starts_with(prefix.as_str()),
            "取消入口要能按前缀一次命中该 transfer 的全部在途流"
        );
        assert!(
            !super::file_cancel_key("t11", "peer-a").starts_with(prefix.as_str()),
            "前缀匹配不得误伤 id 恰好同前缀的兄弟传输"
        );
        assert_eq!(
            a.split('\u{0}').count(),
            2,
            "键必须恰好 transfer_id + recipient 两段"
        );
    }

    /// 正常文件：多分片经「解密 → 增量哈希 → 写盘」后，最终 SHA-256 一致 → 完成。
    #[test]
    fn receiver_hash_lifecycle_success() {
        let original: Vec<u8> = (0..FILE_CHUNK * 2 + 777u32 as usize)
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = hex_of(&original);
        let file_key = crypto::random_key();

        let (f, part_path) = temp_part("ok");
        let mut r = crate::state::FileReceiver {
            file: f,
            name: "ok.bin".into(),
            size: original.len() as u64,
            received: 0,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: part_path.clone(),
            final_path: part_path.clone(),
            peer_id: "a".into(),
            last_report_ms: 0,
            fed_at_ms: 0,
            file_key,
            expected_sha256: expected.clone(),
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        };

        for (seq, chunk) in original.chunks(FILE_CHUNK).enumerate() {
            let sealed = crypto::seal_symmetric(&file_key, chunk).unwrap();
            receive_one_chunk(&mut r, seq as u32, &sealed);
        }
        assert!(finish_verdict(&mut r).is_ok(), "内容一致时校验必须通过");
        let _ = std::fs::remove_file(&part_path);
    }

    /// 篡改某个明文分片：最终 SHA-256 不一致 → failed（不得视为完成）。
    #[test]
    fn receiver_hash_mismatch_fails() {
        let original: Vec<u8> = (0..FILE_CHUNK + 100u32 as usize)
            .map(|i| (i % 199) as u8)
            .collect();
        let expected = hex_of(&original);
        let file_key = crypto::random_key();

        let (f, part_path) = temp_part("bad");
        let mut r = crate::state::FileReceiver {
            file: f,
            name: "bad.bin".into(),
            size: original.len() as u64,
            received: 0,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: part_path.clone(),
            final_path: part_path.clone(),
            peer_id: "a".into(),
            last_report_ms: 0,
            fed_at_ms: 0,
            file_key,
            expected_sha256: expected,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        };

        for (seq, chunk) in original.chunks(FILE_CHUNK).enumerate() {
            let mut plain = chunk.to_vec();
            if seq == 0 {
                plain[0] ^= 0x01; // 篡改首片一个比特
            }
            let sealed = crypto::seal_symmetric(&file_key, &plain).unwrap();
            receive_one_chunk(&mut r, seq as u32, &sealed);
        }
        let verdict = finish_verdict(&mut r);
        assert_eq!(verdict.unwrap_err(), "文件完整性校验失败");
        let _ = std::fs::remove_file(&part_path);
    }

    /// relay 场景（2026-09-23 审计 1.8 修复后）：最终接收方逐片解密，重组完成后
    /// 对按 seq 组装的明文**一次性**算 SHA-256；中继只透传密文，不参与哈希。
    #[test]
    fn relay_receiver_hash_lifecycle_success() {
        use crate::state::RelayFileReceive;
        let original: Vec<u8> = (0..FILE_CHUNK + 500u32 as usize)
            .map(|i| (i % 241) as u8)
            .collect();
        let expected = hex_of(&original);
        let file_key = crypto::random_key();

        let rs = RelayFileReceive {
            file_key,
            expected_sha256: expected,
            created_at: crate::db::now_ms(),
            last_progress_at: 0,
        };

        // 模拟 handle_relay_chunk 的接收路径：解密 → add_chunk（去重 + 按 seq 组装）。
        // 分片**故意乱序到达且 seq=0 重复投递一次**（多邻居泛洪 + 多路径时延不同
        // 是该链路的常态）—— 修复前的增量哈希在这两种情况下都会算错，导致
        // 「分片齐了却报文件完整性校验失败」，发送端却显示成功。
        // 乱序 + 重复投递的语义没变，变的只是"组出来的东西在哪"：
        // 现在每片直接按 `seq × chunk_size` 落进 `.part`，完成时给的是**文件路径**。
        let dir = std::env::temp_dir().join(format!("gosslan-relay-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut relay = crate::file_relay::RelayManager::new();
        relay
            .begin_reassemble(
                "t",
                "f.bin",
                2,
                original.len() as u64,
                FILE_CHUNK as u32,
                &dir,
            )
            .unwrap();
        let sealed: Vec<Vec<u8>> = original
            .chunks(FILE_CHUNK)
            .map(|c| crypto::seal_symmetric(&rs.file_key, c).unwrap())
            .collect();
        // 乱序：先到 seq=1，再到 seq=0（此刻重组完成），然后 seq=0 再来一份（重复）
        let mut done: Option<(String, u64, std::path::PathBuf)> = None;
        for (seq, s) in [(1u32, &sealed[1]), (0, &sealed[0]), (0, &sealed[0])] {
            let plain = crypto::open_symmetric(&rs.file_key, s).unwrap();
            if let crate::file_relay::ChunkOutcome::Complete { name, size, path } =
                relay.add_chunk("t", seq, &plain)
            {
                done = Some((name, size, path));
            }
        }
        let Some((_, _, part)) = done else {
            panic!("三条分片后必须完成重组");
        };
        let full = std::fs::read(&part).unwrap();
        assert_eq!(full, original, "乱序+重复到达也要落出原始明文");

        // 修复后的校验点：对**已归位的字节**整体算一次哈希（生产用同一份流式实现）
        assert!(
            sha256_file_hex(&part)
                .unwrap()
                .eq_ignore_ascii_case(&rs.expected_sha256),
            "乱序+重复分片场景最终校验必须通过"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 群文件 session key（GroupFileOffer 阶段） ----------

    /// 5. file_key 用 GroupKey seal/open round-trip：群内成员可解封。
    #[test]
    fn group_file_key_roundtrip_with_group_key() {
        let group_key = crypto::random_key();
        let file_key = crypto::random_key();
        let sealed = crypto::seal_symmetric(&group_key, &file_key).unwrap();
        let opened: [u8; 32] = crypto::open_symmetric(&group_key, &sealed)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(opened, file_key);
    }

    /// 6. 错误 GroupKey / 篡改密文 → 解封失败（群外与篡改者无法获得 file_key）。
    #[test]
    fn group_file_key_rejects_wrong_key_or_tampered_ciphertext() {
        let group_key = crypto::random_key();
        let wrong_key = crypto::random_key();
        let file_key = crypto::random_key();
        let sealed = crypto::seal_symmetric(&group_key, &file_key).unwrap();

        // 错误群密钥（群外 peer 用自己的“群密钥”）
        assert!(crypto::open_symmetric(&wrong_key, &sealed).is_none());
        // 篡改密文
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        assert!(crypto::open_symmetric(&group_key, &tampered).is_none());
    }

    /// 9. 一个 transfer 的多个 recipient 使用同一个 file session key：
    ///    同一份 sealed 密文被每个成员解封，得到同一个 file_key。
    #[test]
    fn all_recipients_share_one_session_key() {
        let group_key = crypto::random_key(); // 全体成员相同的群密钥
        let file_key = crypto::random_key();
        let sealed = crypto::seal_symmetric(&group_key, &file_key).unwrap();

        let mut opened_keys = Vec::new();
        for _recipient in ["b", "c", "d"] {
            let opened: [u8; 32] = crypto::open_symmetric(&group_key, &sealed)
                .unwrap()
                .try_into()
                .unwrap();
            opened_keys.push(opened);
        }
        assert!(opened_keys.iter().all(|k| *k == file_key));
    }

    /// 4. 每个 transfer 生成独立的随机 file session key（群文件版断言）。
    #[test]
    fn group_file_keys_distinct_across_transfers() {
        let k1 = crypto::random_key();
        let k2 = crypto::random_key();
        assert_ne!(k1, k2);
        let msg = b"group file content";
        let sealed = crypto::seal_symmetric(&k1, msg).unwrap();
        assert!(crypto::open_symmetric(&k2, &sealed).is_none());
    }

    // ---------- GroupFileChunk（协议 + 接收语义） ----------

    use crate::protocol::Message;

    /// 1+2. GroupFileChunk JSON round-trip：字段完整保留（serde tag + 字段名）。
    #[test]
    fn group_file_chunk_roundtrip_preserves_fields() {
        let msg = Message::GroupFileChunk {
            transfer_id: "gf-1".into(),
            group_id: "g-1".into(),
            sender_id: "dev-a".into(),
            seq: 7,
            data: "bm9uY2UrY2lwaGVydGV4dA==".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"group_file_chunk\""),
            "serde tag 必须是 group_file_chunk"
        );
        assert!(json.contains("\"transfer_id\":\"gf-1\""));
        assert!(json.contains("\"group_id\":\"g-1\""));
        assert!(json.contains("\"sender_id\":\"dev-a\""));
        assert!(json.contains("\"seq\":7"));
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::GroupFileChunk {
                transfer_id,
                group_id,
                sender_id,
                seq,
                data,
            } => {
                assert_eq!(transfer_id, "gf-1");
                assert_eq!(group_id, "g-1");
                assert_eq!(sender_id, "dev-a");
                assert_eq!(seq, 7);
                assert_eq!(data, "bm9uY2UrY2lwaGVydGV4dA==");
            }
            _ => panic!("应为 GroupFileChunk"),
        }
    }

    /// 6. 同一 file_key 加密同一明文两次 → 密文不同（每片独立随机 nonce，无重用）。
    #[test]
    fn group_chunks_use_distinct_nonces() {
        let file_key = crypto::random_key();
        let plaintext = vec![42u8; 1024];
        let c1 = crypto::seal_symmetric(&file_key, &plaintext).unwrap();
        let c2 = crypto::seal_symmetric(&file_key, &plaintext).unwrap();
        assert_ne!(c1, c2, "随机 nonce 下相同明文的两次密文必须不同");
        // 但都能解回同一明文
        assert_eq!(crypto::open_symmetric(&file_key, &c1).unwrap(), plaintext);
        assert_eq!(crypto::open_symmetric(&file_key, &c2).unwrap(), plaintext);
    }

    /// 构造群文件接收状态的测试 helper（与 handle_group_file_chunk 写入序列一致）。
    fn group_receiver(
        tag: &str,
        size: u64,
        expected: String,
        file_key: [u8; 32],
    ) -> crate::state::FileReceiver {
        let (f, part_path) = temp_part(tag);
        crate::state::FileReceiver {
            file: f,
            name: format!("{tag}.bin"),
            size,
            received: 0,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: part_path.clone(),
            final_path: part_path,
            peer_id: "dev-a".into(),
            last_report_ms: 0,
            fed_at_ms: 0,
            file_key,
            expected_sha256: expected,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        }
    }

    /// 群分片处理的夹具：**两道判定都调生产侧的纯函数**，不再在这里抄一份规则。
    ///
    /// 为什么必须这样写（这条测试自己就是反面教材）：原来这个夹具里写的是
    /// `if seq != r.next_seq { return Err(..) }` —— 那是对生产码那段内联判断的**抄本**，
    /// 于是它钉的是抄本：把生产侧改成"重复忽略"，这条用例照样全绿，
    /// 而矩阵与 protocol-invariants 却拿它的名字当"群侧已覆盖"的证据（假覆盖）。
    /// 现在 seq 走 [`chunk_seq_decision`]、越界走 [`chunk_exceeds_declared`]，
    /// 生产侧改规则 ⇒ 这里当场跟着红。
    ///
    /// 第三种结局用 `Ignored` 表达（重复片：不写盘、不报错、不终止），
    /// 与生产侧 `ChunkSeq::Duplicate` 那一支一一对应。
    #[derive(Debug, PartialEq)]
    enum ChunkOutcome {
        Written(f64),
        Ignored,
        Fatal(String),
    }

    /// 断言口：这一片**必须被写入**，并把进度交回去；其它结局直接 panic。
    fn must_write(o: ChunkOutcome) -> f64 {
        match o {
            ChunkOutcome::Written(p) => p,
            other => panic!("期望写入，实得 {other:?}"),
        }
    }

    /// 断言口：这一片**必须致命**（真跳号 / 越界 / 解不开）。
    fn must_fatal(o: ChunkOutcome) {
        assert!(
            matches!(o, ChunkOutcome::Fatal(_)),
            "期望致命结局，实得 {o:?}"
        );
    }

    fn receive_group_chunk(
        r: &mut crate::state::FileReceiver,
        seq: u32,
        sealed: &[u8],
    ) -> ChunkOutcome {
        use std::io::Write;
        match chunk_seq_decision(seq, r.next_seq) {
            ChunkSeq::Duplicate => return ChunkOutcome::Ignored,
            ChunkSeq::Gap => {
                return ChunkOutcome::Fatal("文件分片顺序错误".to_string());
            }
            ChunkSeq::Accept => {}
        }
        let plaintext = match crypto::open_symmetric(&r.file_key, sealed) {
            Some(p) => p,
            None => return ChunkOutcome::Fatal("分片解密失败".to_string()),
        };
        if chunk_exceeds_declared(r.size, r.received, plaintext.len() as u64) {
            return ChunkOutcome::Fatal("超出声明大小".to_string());
        }
        use sha2::Digest;
        r.hasher.update(&plaintext);
        if let Err(e) = r.file.write_all(&plaintext) {
            return ChunkOutcome::Fatal(format!("写盘失败: {e}"));
        }
        r.received += plaintext.len() as u64;
        r.next_seq += 1;
        ChunkOutcome::Written(if r.size == 0 {
            1.0
        } else {
            (r.received as f64 / r.size as f64).min(1.0)
        })
    }

    /// 8+11+17+20. seq 0→1→2 正常、明文写入 `.part`、进度递增且不超过 1.0。
    #[test]
    fn group_receive_seq_progress_and_part_writes() {
        let original: Vec<u8> = (0..1024).map(|i| (i % 97) as u8).collect();
        let file_key = crypto::random_key();
        let mut r = group_receiver("seq-ok", original.len() as u64, hex_of(&original), file_key);

        let mut last_progress = 0.0;
        for (seq, chunk) in original.chunks(400).enumerate() {
            let sealed = crypto::seal_symmetric(&file_key, chunk).unwrap();
            let progress = must_write(receive_group_chunk(&mut r, seq as u32, &sealed));
            assert!(progress > last_progress && progress <= 1.0);
            last_progress = progress;
        }
        assert_eq!(r.received, original.len() as u64);
        assert_eq!(r.next_seq, 3);
        assert_eq!(last_progress, 1.0);
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    /// 9+10. **跳号致命、重复忽略**（群侧与单聊同一份规则），而且重复之后下一片照常能进来。
    ///
    /// 这条改的是规格而不只是实现：群里同一片经中继会被送两次，把它当致命就等于
    /// "群里收到的文件注定失败"。最后一行是这条判据的重点 —— 忽略必须是**可恢复的忽略**。
    #[test]
    fn group_receive_rejects_gap_but_ignores_duplicate_seq() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("seq-mixed", 4096, hex_of(b"whatever"), file_key);
        let c0 = crypto::seal_symmetric(&file_key, b"chunk0").unwrap();
        let c1 = crypto::seal_symmetric(&file_key, b"chunk1").unwrap();

        assert!(matches!(
            receive_group_chunk(&mut r, 0, &c0),
            ChunkOutcome::Written(_)
        ));
        // 重复 0 ⇒ 忽略：不写盘、不报错、next_seq 不动
        assert_eq!(receive_group_chunk(&mut r, 0, &c0), ChunkOutcome::Ignored);
        assert_eq!(r.next_seq, 1, "重复片不许推进序号（否则下一片会被判成跳号）");
        // 紧接着的 1 必须照常收下 —— 只忽略不可恢复的话，这一行会红
        assert!(matches!(
            receive_group_chunk(&mut r, 1, &c1),
            ChunkOutcome::Written(_)
        ));
        // 真跳号（要 3 却给 5）⇒ 仍然致命
        assert!(matches!(
            receive_group_chunk(&mut r, 5, &c1),
            ChunkOutcome::Fatal(_)
        ));
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    /// 群侧**必须**调那份共享规则，不许在自己文件里另写一遍 `seq != next_seq`。
    /// （形状判据：本仓撞过太多次"点名式判据 = 半个守卫"，这条钉的是"只有一个家"。）
    #[test]
    fn group_chunk_seq_rule_has_exactly_one_home() {
        let transport = crate::network::transport_src_for_guards();
        let at = transport
            .find("fn handle_group_file_chunk")
            .expect("群分片入口改名了 ⇒ 同步改这条判据");
        // 窗口取"到下一个函数头为止"，而不是拍一个固定字符数：
        // 固定窗口会盖不住真正的分片判断（那条函数有 400+ 行），也会越界读到邻居函数。
        let rest = &transport[at..];
        let next_fn = rest[10..]
            .find("\nasync fn ")
            .map(|i| i + 10)
            .unwrap_or_else(|| rest.len());
        let body = &rest[..next_fn];
        assert!(
            body.contains("chunk_seq_decision"),
            "群分片入口没有走那份共享规则 ⇒ 规则又分成两个家"
        );
        // 比"抄本回来了"之前先剥掉注释：这一族判据第一次跑就红在我自己写的那句
        // "以前这里是 if seq != r.next_seq"上 —— 拿原文子串比代码，等于把解释当代码。
        let code_only: String = body
            .lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code_only.contains("seq != r.next_seq"),
            "群分片入口里还留着自写的 seq 比较 ⇒ 抄本回来了"
        );
    }

    /// 12. 分片总明文超过声明大小 → 拒绝（防恶意 sender 溢出写）。
    #[test]
    fn group_receive_rejects_oversize() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("oversize", 10, hex_of(b"0123456789"), file_key);
        // 第一片 6 字节 OK
        let s0 = crypto::seal_symmetric(&file_key, b"012345").unwrap();
        must_write(receive_group_chunk(&mut r, 0, &s0));
        // 第二片 6 字节：6+6 > 10 → 拒绝
        let s1 = crypto::seal_symmetric(&file_key, b"abcdef").unwrap();
        must_fatal(receive_group_chunk(&mut r, 1, &s1));
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    /// 14（AEAD 面）. 错误 file_key 解密失败 → 调用方终止接收（返回 Err）。
    #[test]
    fn group_receive_rejects_wrong_file_key() {
        let right_key = crypto::random_key();
        let wrong_key = crypto::random_key();
        // 接收端持有 wrong_key（模拟 session key 不匹配）
        let mut r = group_receiver("wrong-key", 1024, hex_of(b"x"), wrong_key);
        let sealed = crypto::seal_symmetric(&right_key, b"secret chunk").unwrap();
        // 接收端持有 wrong_key：解密失败 → handle_group_file_chunk 走 fail 收尾
        must_fatal(receive_group_chunk(&mut r, 0, &sealed));
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    // ---------- GroupFileDone（最终校验 + 正式文件落盘） ----------

    use crate::protocol::Message as ProtocolMessage;

    /// 1. GroupFileDone JSON round-trip：字段完整保留。
    #[test]
    fn group_file_done_roundtrip_preserves_fields() {
        let msg = ProtocolMessage::GroupFileDone {
            transfer_id: "gf-1".into(),
            group_id: "g-1".into(),
            sender_id: "dev-a".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"group_file_done\""),
            "serde tag 必须是 group_file_done"
        );
        assert!(json.contains("\"transfer_id\":\"gf-1\""));
        assert!(json.contains("\"group_id\":\"g-1\""));
        assert!(json.contains("\"sender_id\":\"dev-a\""));
        let back: ProtocolMessage = serde_json::from_str(&json).unwrap();
        match back {
            ProtocolMessage::GroupFileDone {
                transfer_id,
                group_id,
                sender_id,
            } => {
                assert_eq!(transfer_id, "gf-1");
                assert_eq!(group_id, "g-1");
                assert_eq!(sender_id, "dev-a");
            }
            _ => panic!("应为 GroupFileDone"),
        }
    }

    /// 模拟 handle_group_file_done 的最终校验与落盘序列：
    /// size → SHA-256 → sync_all → drop(file) → rename（与生产代码同序）。
    fn finalize_group_receive(r: crate::state::FileReceiver) -> Result<std::path::PathBuf, String> {
        if r.received != r.size {
            let _ = std::fs::remove_file(&r.tmp_path);
            return Err("文件传输未完成".to_string());
        }
        {
            use sha2::Digest;
            let actual_hex: String = r
                .hasher
                .clone()
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
                let _ = std::fs::remove_file(&r.tmp_path);
                return Err("文件完整性校验失败".to_string());
            }
        }
        r.file.sync_all().map_err(|e| {
            let _ = std::fs::remove_file(&r.tmp_path);
            e.to_string()
        })?;
        drop(r.file);
        std::fs::rename(&r.tmp_path, &r.final_path).map_err(|e| {
            let _ = std::fs::remove_file(&r.tmp_path);
            e.to_string()
        })?;
        Ok(r.final_path)
    }

    /// 2+3+4. 正常多 chunk + Done：SHA-256 正确 → rename 成功 → 正式文件内容一致，
    /// progress 对应 1.0 / completed 语义。
    #[test]
    fn group_done_success_renames_part() {
        let original: Vec<u8> = (0..2048).map(|i| (i % 173) as u8).collect();
        let file_key = crypto::random_key();
        let mut r = group_receiver(
            "done-ok",
            original.len() as u64,
            hex_of(&original),
            file_key,
        );
        for (seq, chunk) in original.chunks(700).enumerate() {
            let sealed = crypto::seal_symmetric(&file_key, chunk).unwrap();
            must_write(receive_group_chunk(&mut r, seq as u32, &sealed));
        }
        assert_eq!(
            r.received as f64 / r.size as f64,
            1.0,
            "progress 必须为 1.0"
        );

        let final_path = finalize_group_receive(r).expect("最终校验应通过");
        assert!(!final_path.as_os_str().is_empty());
        let saved = std::fs::read(&final_path).unwrap();
        assert_eq!(saved, original, "正式文件内容必须与原文件一致");
        let _ = std::fs::remove_file(&final_path);
    }

    /// 5. received < size → failed（不 rename、删 .part）。
    #[test]
    fn group_done_short_receive_fails() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("done-short", 1024, hex_of(b"0123456789"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"012345").unwrap();
        must_write(receive_group_chunk(&mut r, 0, &sealed)); // 只收 6 字节 < 1024

        let part = r.tmp_path.clone();
        let err = finalize_group_receive(r).unwrap_err();
        assert_eq!(err, "文件传输未完成");
        assert!(!part.exists(), "失败后 .part 必须被删除");
    }

    /// 7. SHA-256 mismatch → failed + .part 删除（绝不 rename）。
    #[test]
    fn group_done_sha_mismatch_fails_and_cleans_part() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("done-mismatch", 8, hex_of(b"deadbeef"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"content8").unwrap();
        must_write(receive_group_chunk(&mut r, 0, &sealed)); // size 对但内容不同

        let part = r.tmp_path.clone();
        let err = finalize_group_receive(r).unwrap_err();
        assert_eq!(err, "文件完整性校验失败");
        assert!(!part.exists(), "SHA 不匹配后 .part 必须被删除");
    }

    /// 9. rename 失败 → failed（final_path 非法/被占用），不报告完成。
    #[test]
    fn group_done_rename_failure_fails() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("done-rename", 4, hex_of(b"data"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"data").unwrap();
        must_write(receive_group_chunk(&mut r, 0, &sealed));

        // final_path 指向一个已存在的目录 → rename 必然失败
        let dir = std::env::temp_dir().join(format!("gosslan-test-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        r.final_path = dir.clone();

        let part = r.tmp_path.clone();
        assert!(finalize_group_receive(r).is_err());
        assert!(!part.exists(), "rename 失败后 .part 必须被清理");
        let _ = std::fs::remove_dir(&dir);
    }

    /// 19. 空文件：无 Chunk，Done 阶段 size==0 → 空文件 SHA-256 → 正式文件创建，
    /// completed / progress 1.0 语义成立。
    #[test]
    fn group_done_empty_file_creates_zero_byte_file() {
        // 空文件 SHA-256（发送端对 0 字节文件计算的结果）
        let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let file_key = crypto::random_key();
        let r = group_receiver("done-empty", 0, expected.to_string(), file_key);
        assert_eq!(r.received, 0, "空文件无任何 Chunk");

        let final_path = finalize_group_receive(r).expect("空文件必须能正常完成");
        let meta = std::fs::metadata(&final_path).unwrap();
        assert_eq!(meta.len(), 0, "正式文件必须是 0 字节");
        let _ = std::fs::remove_file(&final_path);
    }

    // ---------- GroupFileCompleteAck（协议 round-trip） ----------

    /// 1+2+3. GroupFileCompleteAck JSON round-trip：success=true / false 均完整保留。
    #[test]
    fn group_file_complete_ack_roundtrip() {
        for success in [true, false] {
            let msg = ProtocolMessage::GroupFileCompleteAck {
                transfer_id: "gf-1".into(),
                group_id: "g-1".into(),
                sender_id: "dev-b".into(),
                success,
            };
            let json = serde_json::to_string(&msg).unwrap();
            assert!(json.contains("\"group_file_complete_ack\""));
            assert!(json.contains(&format!("\"success\":{}", success)));
            let back: ProtocolMessage = serde_json::from_str(&json).unwrap();
            match back {
                ProtocolMessage::GroupFileCompleteAck {
                    transfer_id,
                    group_id,
                    sender_id,
                    success: s,
                } => {
                    assert_eq!(transfer_id, "gf-1");
                    assert_eq!(group_id, "g-1");
                    assert_eq!(sender_id, "dev-b");
                    assert_eq!(s, success);
                }
                _ => panic!("应为 GroupFileCompleteAck"),
            }
        }
    }

    // ---------------- 续传：offer 位置判据（`decide_offer`）----------------

    /// 真机事故形状（2026-09-22 跨网首测，160MB 永远传不完）：接收器**还活着**、已收 40MB，
    /// 而发送端每一轮重试都从 `from_bytes = 0` 重发（`flush_pending_files` 就是调
    /// `send_file_from_path`，不携带位置）。这时接收端必须把真实位置回给它，
    /// 而不是回 `Accept` 再把前 40MB 当"迟到的重复片"静默丢掉 ——
    /// 后者等于"每一轮都要重新传一遍已收前缀"，慢链路上永远跑不完。
    #[test]
    fn live_receiver_must_tell_the_sender_where_it_actually_is() {
        use super::{decide_offer, OfferDecision};
        let held = 40 * 1024 * 1024;
        assert_eq!(
            decide_offer(true, held, held, 100 * 1024 * 1024, 0, false),
            OfferDecision::ResumeFrom(held),
            "活跃接收器已收 40MB、对方却从 0 重发 ⇒ 必须回真实位置，不能裸 Accept"
        );
    }

    /// 位置本来就对得上 ⇒ 仍然要 `Accept`。这条不许被上一条"顺手改坏"：
    /// 幂等 Accept 修的是真机缺陷（"两边都显示成功、接收侧列表里没有"），
    /// 对端没收到我们的 accept 时会**重发同一个 offer**，那时 from_bytes 是一致的。
    #[test]
    fn matching_position_still_accepts_idempotently() {
        use super::{decide_offer, OfferDecision};
        let held = 40 * 1024 * 1024;
        // 位置对得上 ⇒ 答复仍然属于"接受"这一族，绝不退回 reject（那是被真机教育过的旧行为：
        // 两边都显示成功、接收侧列表里没有）。但**续传段**必须连带把段号归零 ⇒ 判据要能区分。
        assert_eq!(
            decide_offer(true, held, held, 100 * 1024 * 1024, held, false),
            OfferDecision::AcceptResumeSegment,
            "位置一致的续传段：接受 + 段号归零"
        );
        // 同一起点的重复 offer ⇒ **不许**归零：上一轮 attempt 已入队的分片还在排空，
        // 归零会把它们判成「跳号」⇒ `Err(文件分片顺序错误)` ⇒ 整单死。
        assert_eq!(
            decide_offer(true, 0, 0, 1024, 0, false),
            OfferDecision::Accept
        );
        // 全新传输：什么都没有，对方也从 0 开始
        assert_eq!(
            decide_offer(false, 0, 0, 1024, 0, false),
            OfferDecision::Accept
        );
    }

    /// 回归（2026-09-23 审计 A6）：「本机已收完」必须优先于一切位置判据。
    ///
    /// 收完之后 `.part` 已改名、活跃接收器已清空 ⇒ 三输入全归零（与全新传输同形），
    /// 旧判据把重复 Offer 判成 Accept ⇒ 整份重推落「名字(1)」副本
    /// （重复 Offer 的常见来源：终态回执丢失 → 发送端 outbox 重试）。
    #[test]
    fn completed_transfer_rejects_duplicate_offer_without_resend() {
        use super::{decide_offer, OfferDecision};
        // 三输入全零但已收完：必须 AlreadyHave，绝不能 Accept
        assert_eq!(
            decide_offer(false, 0, 0, 1024, 0, true),
            OfferDecision::AlreadyHave,
            "已收完的传输收到重复 Offer：拒绝重推（审计 A6）"
        );
        // 即使残留了活跃接收器/磁盘前缀的形态，已收完也一票否决
        assert_eq!(
            decide_offer(true, 1024, 1024, 1024, 1024, true),
            OfferDecision::AlreadyHave
        );
    }

    /// 权威是"**我有什么**"，而活跃接收器的内存计数比磁盘 `.part` 更靠前
    /// （`write_chunk` 每片都 `write_all`，但进度落库是 500ms 节流）。
    /// 回一个偏小的位置会让发送端重灌已写进文件的字节 ⇒ 文件超长、校验必失败。
    #[test]
    fn the_active_receiver_is_the_authority_not_the_disk_prefix() {
        use super::{decide_offer, OfferDecision};
        let (live, disk) = (30 * 1024 * 1024, 20 * 1024 * 1024);
        assert_eq!(
            decide_offer(true, live, disk, 100 * 1024 * 1024, 0, false),
            OfferDecision::ResumeFrom(live),
            "有活跃接收器时必须报内存里的真实值，不是 .part 大小"
        );
        // 没有活跃接收器（断链后进程没重启）⇒ 磁盘前缀才是唯一事实
        assert_eq!(
            decide_offer(false, 0, disk, 100 * 1024 * 1024, 0, false),
            OfferDecision::ResumeFrom(disk),
            "无活跃接收器时仍以 .part 前缀为准（这条是既有行为，锁住别退回去）"
        );
    }

    /// ★ A-12（2026-09-26 由注入⑧在真实双实例上照出）：**`.part` 字节数够了 ≠ 本机已收完**。
    ///
    /// 「rename 才算完成」的反面形状：上一次收尾那刀没落地（接收目录只读 / 磁盘满 /
    /// 进程被杀在半步）⇒ 字节是整份的，但**磁盘上没有成品文件**。此时把 `size` 当"已收位置"
    /// 回出去是致命的：发送端读的是 `received >= size ⇒ 对方已完整收下`，于是它记 `done`、
    /// **删掉队列行**（此后再没有人重试这一单），而接收端只有一个改不了名的 `.part`。
    /// 一次 `FileReject.received` 承载两个含义，而字节数恰好落在"整份"这个值上。
    #[test]
    fn a_full_length_part_without_rename_is_not_progress() {
        use super::{decide_offer, OfferDecision};
        let size = 1024 * 1024;
        assert_eq!(
            decide_offer(false, 0, size, size, 0, false),
            OfferDecision::Accept,
            "字节数够了却没有成品文件 ⇒ 既不认已收完，也不许把这个数当续传位置报出去"
        );
        // 超长（上一轮被改坏的前缀）同理：越界的"进度"一律不算进度
        assert_eq!(
            decide_offer(false, 0, size + 4096, size, 0, false),
            OfferDecision::Accept
        );
        // ⚠️ 反向半边：**没到 size 的真前缀仍然必须照常续传**。
        // 把它退化成"一律重收"会打掉跨 attempt 的进度累积 —— 那正是 160MB 传不完的老病根，
        // 也是注入②（真前缀续传）/ 注入③（SIGKILL 后按真实字节数续完）两条活实例证明。
        assert_eq!(
            decide_offer(false, 0, size - 1, size, 0, false),
            OfferDecision::ResumeFrom(size - 1),
            "小于 size 的磁盘前缀仍是合法续传位置，不许被这条新判据顺手废掉"
        );
        // 真的收完过（台账 done）⇒ `AlreadyHave` 的一票否决不受影响（审计 A6）
        assert_eq!(
            decide_offer(false, 0, size, size, 0, true),
            OfferDecision::AlreadyHave
        );
    }

    /// 造一个"分片已全部收完、只差收尾裁决"的接收器：真的写进一个临时文件，并按写入顺序
    /// 喂增量哈希（与 `write_chunk` 的操作序列一致）。`declared_size` / `expected` 允许与
    /// 实收不一致 —— 那正是要验的两种"假成功"。
    fn receiver_holding(
        tag: &str,
        got: &[u8],
        declared_size: u64,
        expected: &str,
    ) -> crate::state::FileReceiver {
        use std::io::Write;
        let (mut f, tmp) = temp_part(tag);
        f.write_all(got).unwrap();
        use sha2::Digest as _;
        let mut h = sha2::Sha256::new();
        h.update(got);
        let final_path = tmp.with_extension("done");
        let _ = std::fs::remove_file(&final_path);
        crate::state::FileReceiver {
            file: f,
            name: format!("{tag}.bin"),
            size: declared_size,
            received: got.len() as u64,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: tmp,
            final_path,
            peer_id: "dev-a".into(),
            last_report_ms: 0,
            fed_at_ms: 0,
            file_key: [7u8; 32],
            expected_sha256: expected.to_string(),
            hasher: h,
        }
    }

    /// §七「两个 offer 都在任一次 rename 之前到达」＝ 两份**不同内容**的同名文件同时在途。
    ///
    /// 机制（不是猜测，逐处可查）：单聊的 `final_path` 在 `begin` 就用 `unique_path` 定死
    /// （`make_receiver` 里那行注释自己写着"两张同名图同时在途时 begin 一刻磁盘上还没有同名文件"），
    /// 而那一刻谁都没落地 ⇒ 两份拿到**同一个** `final_path` ⇒ 收尾那次 `rename` 在 POSIX 上
    /// **直接覆盖前一份** ⇒ 两行台账都是 `done`、两个气泡都在，其中一份字节已经不在了。
    /// 群聊收尾（`transport.rs`）已有"落盘前再确认名字、被占走就换名"的兜底，其注释断言
    /// "单聊本来就是在写盘时才定名 ⇒ 没有这个竞态" —— 这句与 `make_receiver` 不符 ⇒ 钉这条。
    /// 用户已拍板：改名让两份共存（不拒第二份）。
    #[test]
    fn two_same_named_transfers_do_not_overwrite_each_other() {
        use super::finish_receiver_into;
        use sha2::Digest as _;
        let sha = |b: &[u8]| {
            let mut h = sha2::Sha256::new();
            h.update(b);
            h.finalize()
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<String>()
        };
        let first: Vec<u8> = vec![0xA1u8; 4096];
        let second: Vec<u8> = vec![0xB2u8; 4096];
        let db = std::sync::Mutex::new(rusqlite::Connection::open_in_memory().unwrap());
        db.lock().unwrap().execute_batch(crate::db::SCHEMA).unwrap();
        let path_of = |id: &str| -> Option<String> {
            db.lock()
                .unwrap()
                .query_row(
                    "SELECT status, path FROM file_transfers WHERE id = ?1",
                    rusqlite::params![id],
                    |r| {
                        let st: String = r.get(0)?;
                        assert_eq!(st, "done", "{id} 的终态必须是 done，实得 {st}");
                        r.get::<_, Option<String>>(1)
                    },
                )
                .unwrap_or_else(|e| panic!("{id} 必须留下一行终态，实得 {e}"))
        };

        let a = receiver_holding("same-name-a", &first, first.len() as u64, &sha(&first));
        let mut b = receiver_holding("same-name-b", &second, second.len() as u64, &sha(&second));
        // 忠实复刻 begin 的结果：两次 unique_path 给出同一个名字（谁都没落地 ⇒ 都以为名字是空的）。
        // 只改夹具读的输入，不改断言。
        b.final_path = a.final_path.clone();
        b.name = a.name.clone();

        assert!(
            finish_receiver_into(&db, "xfer-a", a).is_ok(),
            "第一份收尾应当成功"
        );
        assert!(
            finish_receiver_into(&db, "xfer-b", b).is_ok(),
            "第二份也该收下（拍板＝改名共存，不是拒收）"
        );

        let pa = path_of("xfer-a").expect("xfer-a 要留下真实路径");
        let pb = path_of("xfer-b").expect("xfer-b 要留下真实路径");
        assert_ne!(
            pa, pb,
            "两份同名文件落在同一个路径 ⇒ 其中一份必然不存在（气泡指着空路径）"
        );
        assert_eq!(
            std::fs::read(&pa).ok().as_deref(),
            Some(first.as_slice()),
            "先到的那份必须还在、内容还是它自己的字节（被后到的覆盖＝静默丢用户文件）"
        );
        assert_eq!(
            std::fs::read(&pb).ok().as_deref(),
            Some(second.as_slice()),
            "后到的那份也必须自己完整地落在另一个名字下"
        );
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
    }

    /// 「界面上那句『已收到』到底是不是真话」—— 直接驱动生产收尾 `finish_receiver_into`（#25）。
    ///
    /// 为什么值得单独一条：这条链上真机事故的共同形状就是"显示成功而文件其实不在"
    /// （R4 的假成功 Ack、A6 的捷径不落终态）。而在此之前的接收端测试全是**影子副本**
    /// （`receive_one_chunk` / `receive_group_chunk` 各自重写一遍操作序列 —— 生产码被改坏
    /// 它们照样绿）。`finish_receiver_into` 不要 `AppState`，所以这是第一次能拿真码验。
    #[test]
    fn receive_is_done_only_when_size_and_sha_both_prove_it() {
        use super::finish_receiver_into;
        use sha2::Digest as _;
        let payload: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let real_sha: String = {
            let mut h = sha2::Sha256::new();
            h.update(&payload);
            h.finalize().iter().map(|b| format!("{b:02x}")).collect()
        };
        let db = std::sync::Mutex::new(rusqlite::Connection::open_in_memory().unwrap());
        db.lock().unwrap().execute_batch(crate::db::SCHEMA).unwrap();
        let row = |id: &str| -> (String, Option<String>) {
            db.lock()
                .unwrap()
                .query_row(
                    "SELECT status, path FROM file_transfers WHERE id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .unwrap_or_else(|e| panic!("{id} 必须留下一行终态，实得 {e}"))
        };

        // ① 绿路：字节齐 + 哈希对 ⇒ 改名落盘、行是 done 且带真实路径
        let ok = receiver_holding("rcv-ok", &payload, payload.len() as u64, &real_sha);
        let (tmp, final_path) = (ok.tmp_path.clone(), ok.final_path.clone());
        assert!(finish_receiver_into(&db, "t-ok", ok).is_ok());
        assert!(!tmp.exists(), "临时文件必须被改名带走（留下就是双份）");
        assert_eq!(
            std::fs::read(&final_path).unwrap(),
            payload,
            "落盘内容必须就是收到的那些字节"
        );
        assert_eq!(
            row("t-ok"),
            (
                "done".to_string(),
                Some(final_path.to_string_lossy().to_string())
            ),
            "done 必须同时留下可打开的路径，否则前端只会显示一个不存在的气泡"
        );

        // ② 少收一片（size 与实收不符）⇒ 不得算成功，且不得把半截文件改名"冒充成品"
        let short = receiver_holding(
            "rcv-short",
            &payload[..payload.len() - 1],
            payload.len() as u64,
            &real_sha,
        );
        let (tmp, final_path) = (short.tmp_path.clone(), short.final_path.clone());
        assert_eq!(
            finish_receiver_into(&db, "t-short", short).err(),
            Some("文件传输未完成".to_string())
        );
        assert!(
            !tmp.exists(),
            "半截临时文件必须删掉（它不在任何索引里，留着就是垃圾）"
        );
        assert!(!final_path.exists(), "没收完绝不许出现成品文件");
        assert_eq!(row("t-short"), ("failed".to_string(), None));

        // ③ 分片被改过：长度对、内容错 ⇒ 只有 SHA 抓得住（这就是"分片损坏"的应用层形状）
        let mut corrupted = payload.clone();
        corrupted[7] ^= 0xff;
        let bad = receiver_holding("rcv-bad", &corrupted, payload.len() as u64, &real_sha);
        let (tmp, final_path) = (bad.tmp_path.clone(), bad.final_path.clone());
        assert_eq!(
            finish_receiver_into(&db, "t-bad", bad).err(),
            Some("文件完整性校验失败".to_string())
        );
        assert!(
            !tmp.exists() && !final_path.exists(),
            "哈希不过同样不许落盘"
        );
        assert_eq!(row("t-bad"), ("failed".to_string(), None));

        // ④ 发送方没声明哈希（空期望）⇒ **fail-closed**：没有期望值不等于通过
        let nodecl = receiver_holding("rcv-nodecl", &payload, payload.len() as u64, "");
        assert_eq!(
            finish_receiver_into(&db, "t-nodecl", nodecl).err(),
            Some("文件完整性校验失败".to_string()),
            "空 expected_sha256 必须判失败 —— 否则一条不带哈希的 FileDone 就能宣布成功"
        );
        assert_eq!(row("t-nodecl"), ("failed".to_string(), None));

        // ⑤ 大小写不同的 hex 仍是同一个哈希（对端实现差异不得变成"传不过去"）
        let upper = receiver_holding(
            "rcv-upper",
            &payload,
            payload.len() as u64,
            &real_sha.to_uppercase(),
        );
        assert!(finish_receiver_into(&db, "t-upper", upper).is_ok());
        assert_eq!(row("t-upper").0, "done");
    }

    /// 「目标目录变化」（§七 列的那一格）在生产收尾路径上的覆盖。
    ///
    /// 为什么这条不是 ①–⑤ 的重复：那五条没有一条走进 rename 出口（全部在 size / SHA 两处
    /// 就被判掉了），而唯一碰过 rename 失败的历史用例走的是 `finalize_group_receive` ——
    /// 群侧另一份实现，生产单聊收尾改坏它照样绿。
    ///
    /// 真实形状：接收中途用户把下载目录删掉/移走。`FileReceiver.file` 是**已打开的句柄**，
    /// POSIX 下目录被 unlink 后写入与 fsync 照旧成功（inode 还在），于是整条收尾只剩最后
    /// 一次 rename 能发现"成品无处安放"。这条就是把 §三「rename 才算完成」钉在文件系统
    /// 故障边界上：字节齐 + 哈希对都**不许**顶替它。
    #[test]
    fn rotted_receive_directory_finishes_failed_never_done() {
        use super::finish_receiver_into;
        use sha2::Digest as _;
        use std::io::Write as _;

        let payload: Vec<u8> = (0..257u32).map(|i| (i % 251) as u8).collect();
        let real_sha: String = {
            let mut h = sha2::Sha256::new();
            h.update(&payload);
            h.finalize().iter().map(|b| format!("{b:02x}")).collect()
        };

        // 不复用 temp_part：它把 .part 直接放在 temp_dir() 根上，要模拟"整个下载目录消失"
        // 就得连别人的临时文件一起删 —— 不碰。
        let root = std::env::temp_dir().join(format!("gosslan-test-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();

        let db = std::sync::Mutex::new(rusqlite::Connection::open_in_memory().unwrap());
        db.lock().unwrap().execute_batch(crate::db::SCHEMA).unwrap();
        let row = |id: &str| -> (String, Option<String>) {
            db.lock()
                .unwrap()
                .query_row(
                    "SELECT status, path FROM file_transfers WHERE id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .unwrap_or_else(|e| panic!("{id} 必须留下一行终态，实得 {e}"))
        };

        let tmp = dl.join("t-rot.part");
        let final_path = dl.join("photo.jpg");
        let mut f = std::fs::File::create(&tmp).unwrap();
        f.write_all(&payload).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&payload);
        let r = crate::state::FileReceiver {
            file: f,
            name: "photo.jpg".into(),
            size: payload.len() as u64,
            received: payload.len() as u64,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: tmp,
            final_path: final_path.clone(),
            peer_id: "dev-a".into(),
            last_report_ms: 0,
            fed_at_ms: 0,
            file_key: [7u8; 32],
            expected_sha256: real_sha,
            hasher,
        };

        std::fs::remove_dir_all(&root).unwrap();

        let err = finish_receiver_into(&db, "t-rot", r).err();
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            err.is_some(),
            "目录已消失 ⇒ 收尾必须失败（实得 Ok 意味着报成功而磁盘上根本没有成品）"
        );
        assert_eq!(
            row("t-rot"),
            ("failed".to_string(), None),
            "只能记 failed 且不带路径 —— 带路径的 done 会让前端渲染一个点开就是「文件不存在」的气泡"
        );
        assert!(
            !final_path.exists(),
            "放不下的东西不能被凭空造出来：不许出现成品文件"
        );
    }

    /// 零字节文件走**单聊**收尾（§七里最后那半格）：群路径早就有 `group_done_empty_file_creates_zero_byte_file`，
    /// 1:1 的 `finish_receiver_into` 却没有 —— 而 `size = 0` 恰好让两处判据都落到特殊分支上
    /// （`decide_offer` 的磁盘前缀钳制带着 `size > 0` 条件、`chunk_exceeds_declared` 在 0 上恒拒），
    /// 所以"空文件也必须能正常完成、且成品就是 0 字节"这条只能在这里钉住。
    #[test]
    fn empty_file_finishes_done_on_the_one_to_one_path_too() {
        use super::finish_receiver_into;
        let empty_sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let recv = receiver_holding("rcv-empty", b"", 0, empty_sha);
        let (tmp, final_path) = (recv.tmp_path.clone(), recv.final_path.clone());
        let db = std::sync::Mutex::new(rusqlite::Connection::open_in_memory().unwrap());
        db.lock().unwrap().execute_batch(crate::db::SCHEMA).unwrap();

        finish_receiver_into(&db, "t-empty", recv)
            .expect("0 字节的空文件必须能正常完成，不许因 size=0 被判成「没收完」");
        assert_eq!(
            std::fs::metadata(&final_path).unwrap().len(),
            0,
            "成品必须是真实存在的 0 字节文件（用户点开应当看到空文件，而不是「文件不存在」）"
        );
        assert!(!tmp.exists(), "收尾之后不许留下 .part");
        let row: (String, Option<String>) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT status, path FROM file_transfers WHERE id = ?1",
                rusqlite::params!["t-empty"],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "done", "空文件的终态只能是 done");
        assert_eq!(
            row.1.as_deref(),
            Some(final_path.to_string_lossy().as_ref()),
            "done 必须带着能打开的路径，否则前端显示一个不存在的气泡"
        );
        let _ = std::fs::remove_file(&final_path);

        // ★ 反面（这条才让上面的用例真的能咬人）：`size = 0` **不是免检通行证**。
        // 最像"合理优化"的回归就是把空文件写成"0 字节哪来的哈希可比，跳过校验"，
        // 那等于让对端用一个空 payload 顶掉任何声明为空的文件。所以空文件也必须真比对。
        let bad = receiver_holding("rcv-empty-bad", b"", 0, "不是真的哈希");
        let bad_final = bad.final_path.clone();
        assert!(
            finish_receiver_into(&db, "t-empty-bad", bad).is_err(),
            "空文件摘要不符 ⇒ 必须失败，不许因 size=0 免检"
        );
        assert!(!bad_final.exists(), "摘要不符的空文件不许被改名成交付成品");
        let bad_status: String = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT status FROM file_transfers WHERE id = ?1",
                rusqlite::params!["t-empty-bad"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bad_status, "failed", "failed 只能由这里裁决一次");
    }

    // ---------------- 「错误 size」：分片长度越过声明长度（§七那一格） ----------------

    /// 接收端对"声明的 size"的裁决必须是**边界正确**的，两个方向都不能错：
    /// - 把它写成 `>=` ⇒ **每一单**都在最后一片上失败（`stream_file` 的最后一片通常正好补到 size）；
    /// - 写成 `>` 的反面（不设上限）⇒ 对端可以多灌任意字节，而 `.part` 会越写越大，
    ///   最后 `finish_receiver_into` 才因"字节数与声明不符"判死 —— 用户看到的是"传到 100% 然后失败"，
    ///   而且这段时间里磁盘被写掉了超出声明的量。
    #[test]
    fn chunk_length_may_fill_declared_size_but_must_not_exceed_it() {
        use super::chunk_exceeds_declared;

        // ① 恰好填满 ⇒ 允许（这条是防"过度加固"的那一半）
        assert!(
            !chunk_exceeds_declared(1024, 1000, 24),
            "最后一片正好补到 size 必须放行"
        );
        assert!(
            !chunk_exceeds_declared(1024, 0, 1024),
            "单片就是整份（小文件只有一片）必须放行"
        );
        // ② 超一个字节 ⇒ 拒
        assert!(
            chunk_exceeds_declared(1024, 1000, 25),
            "多一个字节就是越界，不能等到收尾才发现"
        );
        // ③ 已经收满还来一片 ⇒ 拒（`saturating_sub` 在这里是承重的：不许下溢成 u64 的天文数字）
        assert!(
            chunk_exceeds_declared(1024, 1024, 1),
            "收完之后任何一片都是多余的，不许因减法下溢被判成合法"
        );
        // ④ 声明 0 字节的空文件 ⇒ 任何内容都算越界（空文件应当只有 Offer + Done，没有分片）
        assert!(chunk_exceeds_declared(0, 0, 1), "size=0 时不许写进任何字节");
    }

    // ---------------- 投递失败之后的重试裁决（#25 第 2 段） ----------------
    //
    // 这一格判据原先inline 在 `flush_pending_files` 的循环里 ⇒ 要吃 `AppState` 才能触发，
    // 而它恰恰是"用户看到永久转圈"还是"看到失败"的唯一分岔口（真机 160MB 那次就是
    // 连续 5 次从头重灌，每次都"可重试"）。所以先把它抽成纯函数再测。

    /// 永久错误必须当场放弃，而且**理由就是原始错误文案** —— 换成一句笼统的"发送失败"
    /// 等于把"文件不存在"这种用户能自己解决的事藏起来。
    #[test]
    fn permanent_send_failure_gives_up_with_the_real_reason() {
        use super::{send_retry_verdict, RetryVerdict, SendFileError};
        let e = SendFileError::permanent("文件不存在或不可读：No such file");
        assert_eq!(
            send_retry_verdict(&e, Some(1)),
            RetryVerdict::GiveUp("文件不存在或不可读：No such file".into()),
            "永久错误不得进入重试队列"
        );
    }

    /// 预算内的可恢复错误 ⇒ 回到 pending 等下一次（链路回来了 / 心跳触发）。
    #[test]
    fn retryable_failure_within_budget_is_rescheduled() {
        use super::{send_retry_verdict, RetryVerdict, SendFileError};
        let e = SendFileError::retryable("未建立连接");
        assert_eq!(
            send_retry_verdict(&e, Some(1)),
            RetryVerdict::Retry { backoff_ms: 5_000 }
        );
        // 边界：预算是 5 次，第 4 次失败之后必须还留着一次机会
        assert_eq!(
            send_retry_verdict(&e, Some(MAX_FILE_OUTBOX_RETRIES - 1)),
            RetryVerdict::Retry { backoff_ms: 5_000 },
            "差一次就放弃 = 少给用户一次机会"
        );
    }

    /// **retryable 也要查预算**（这条是历史缺陷的形状）：只按 `retryable` 分岔的话，
    /// 一个永远跑不完的 160MB 会每 5 秒从头重灌、无限次，界面永远显示"发送中"。
    #[test]
    fn retryable_failure_at_the_budget_limit_gives_up() {
        use super::{send_retry_verdict, RetryVerdict, SendFileError};
        let e = SendFileError::retryable("接收方未确认文件完成");
        assert!(
            matches!(
                send_retry_verdict(&e, Some(MAX_FILE_OUTBOX_RETRIES)),
                RetryVerdict::GiveUp(_)
            ),
            "到预算上限必须停 —— 否则\"可重试\"就是\"永远转圈\""
        );
        // 超上限之后继续加也不能被"再试一次"救回来（比较是 >=，不是 ==）
        assert!(matches!(
            send_retry_verdict(&e, Some(MAX_FILE_OUTBOX_RETRIES + 3)),
            RetryVerdict::GiveUp(_)
        ));
    }

    /// 读不到次数（prepare 失败 / 行已被别的出口删掉）时**不得判死**：
    /// 那是本机自己的一时故障，把它当成"这个文件已经试了 5 次"会直接毁掉一次可恢复的投递。
    #[test]
    fn unknown_attempt_count_is_not_a_death_sentence() {
        use super::{send_retry_verdict, RetryVerdict, SendFileError};
        let e = SendFileError::retryable("链路已关闭");
        assert_eq!(
            send_retry_verdict(&e, None),
            RetryVerdict::Retry { backoff_ms: 5_000 },
            "计数读不到时宁可多试一次"
        );
        // 但永久错误与计数无关，仍然当场放弃
        assert!(matches!(
            send_retry_verdict(&SendFileError::permanent("用户取消发送"), None),
            RetryVerdict::GiveUp(_)
        ));
    }
}

#[cfg(test)]
mod group_receive_atomicity_tests {
    /// 对端下线的群接收收尾**必须先原子摘取再逐个收尾**。
    ///
    /// 为什么用读源码文本的形状判据而不是行为测试：这段吃 `AppState`（接收表 + db + emit），
    /// 本仓没有能驱动它的夹具（`file_tests.rs` 顶上那句注释就写着"造不出也不需要造 AppState"），
    /// 硬凑一个 mock 只会得到假绿。而它要防的回归非常具体 —— 退回"锁内 collect id、锁外逐个收尾"，
    /// 那个形状被同文件 `take_stalled_receive` 的注释明确判死过（中间挤进来的新 FileOffer 会被误判死）。
    #[test]
    fn peer_offline_group_cleanup_takes_before_finalizing() {
        let src = crate::network::transport_src_for_guards();
        let at = src
            .find("if peer_now_offline {")
            .expect("对端下线那一段改名/搬走了 ⇒ 同步改这条判据");
        // 窗口取"到下一个函数头为止"，不拍固定字符数：固定长度会越界读进邻居函数（这条判据
        // 第一次假红就是这么来的），而新加几行注释又会让它在收尾那行之前悄悄截短。
        // ⚠️ 找不到函数头必须炸，不许回退成"整份文件" —— 那会让下面两条否定断言扫到
        //    邻居函数里的同名形状，判据当场变成摆设。
        let rest = &src[at..];
        let end = [
            "\nfn ",
            "\npub fn ",
            "\nasync fn ",
            "\npub(crate) fn ",
            "\npub(crate) async fn ",
        ]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()
        .expect("这段之后的下一个函数头改名/搬走了 ⇒ 同步改这条判据（不许放宽成整文件）");
        let body = &rest[..end];
        // 反空转闸：窗口必须真的盖到"收尾那一行"，否则下面所有断言都在判空气。
        assert!(
            body.contains("finalize_failed_group_receive"),
            "窗口没覆盖到收尾那一行 ⇒ 这条判据已失去落点，改它而不是放行"
        );
        assert!(
            body.contains("take_group_receives_for_peer("),
            "对端下线没有走原子摘取 ⇒ 摘表与判据又分成了两次持锁"
        );
        // ★ 摘出来的那一份必须被用完（INV-P28 的后半句）：群收件人的**内容台账**
        // `record_failure ⇒ Incomplete` 只有 `fail_taken_group_receive` 这一个写点，
        // 只调 finalize 那一半 ⇒ 台账停在 Active，重取走的是另一条兜底口径。
        // 这是 `5a0cfcc` 自己引入过的漏（`for (tid, _r)` 把那份丢了，四层门禁全绿）。
        assert!(
            body.contains("fail_taken_group_receive(&state, &r)"),
            "摘出来的 FileReceiver 没被收尾吃掉 ⇒ 群内容台账这次没人写 Incomplete"
        );
        assert!(
            !body.contains("(tid, _r) in taken"),
            "又把摘出来的那份丢了 ⇒ 正是 5a0cfcc 那半收尾的形状，内容台账会停在 Active"
        );
        assert!(
            !body.contains("let group_ids: Vec<String>"),
            "快照 id 那份旧写法又回来了 ⇒ 两步之间可以挤进一个新 FileOffer"
        );
        assert!(
            !body.contains("fail_group_file_chunk(&state, &tid)"),
            "收尾又改成按 id 逐个走完整函数 ⇒ 会把正在收的那一单判死"
        );
    }

    /// 同一刀必须也落在**单聊**那半边（"同一开关的另一个面"最容易在"已改"那行底下继续漏）。
    /// 判据与群侧那条同形：摘表一次持锁做完，收尾只吃已经摘出来的接收器。
    #[test]
    fn single_peer_offline_cleanup_also_takes_before_finalizing() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/network/file.rs"
        ))
        .expect("读不到 file.rs ⇒ 这条判据失去落点");
        let at = src
            .find("pub fn fail_receives_for_peer(")
            .expect("单聊 peer-wide 收尾改名了 ⇒ 同步改这条判据");
        // 窗口取"到下一个函数头为止"。拍固定字符数会越界读进邻居函数 ——
        // 这条判据第一次跑就是这么假红的（`.collect(); for id in ids` 在下一个函数里）。
        let rest = &src[at..];
        let end = rest[10..]
            .find("\npub fn ")
            .map(|i| i + 10)
            .unwrap_or_else(|| rest.len());
        let body = &rest[..end];
        assert!(
            body.contains("take_receives_for_peer("),
            "单聊侧没有走原子摘取 ⇒ 与群侧又不是同一族规则"
        );
        assert!(
            !body.contains("fail_receive(state,"),
            "单聊侧又退回「逐个调完整函数」⇒ 摘表与判据重新分成两次持锁"
        );
    }

    /// `completed` 的终态保护必须**在生产 handler 里真的读收件人状态**。
    ///
    /// 这条是被一条假测试逼出来的：`favorites_tests.rs::complete_ack_failure_cannot_downgrade_completed`
    /// 在测试体里自己重写了一遍 `already_completed` 判断，所以把生产码那段删掉它照样绿。
    /// 配套护栏用例（`verify-guards.py` 里同名 Case）会把那句 `.any(...)` 注入成常量 false，
    /// 届时这条必须红 —— 两条一起才是完整的非空转证明。
    #[test]
    fn completed_guard_reads_recipient_status_in_the_handler() {
        let src = crate::network::transport_src_for_guards();
        let at = src
            .find("async fn handle_group_file_complete_ack")
            .expect("群文件 complete ACK 处理改名了 ⇒ 同步改这条判据");
        let body = &src[at..at + 2600];
        assert!(
            body.contains(r#".any(|r| r.recipient_id == peer_id && r.status == "completed")"#),
            "failure ACK 不再读该收件人的 completed 状态 ⇒ 终态保护只剩测试里那份抄本"
        );
    }
}
