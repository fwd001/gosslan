// 职责边界：
// - transport 行为测试分册之7 —— 帧编解码与读写循环：未知 wire type 降级、超长帧、本地错与 socket 错分型
// 为什么拆：`transport/tests.rs` 原来 3,227 行、10 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
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
