// 职责边界：
// - transport 行为测试分册之5 —— 出站投递：按链路顺序发送、failover、背压、超时不留残、优先级通道互不阻塞
// 为什么拆：`transport/tests.rs` 原来 3,227 行、14 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
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
