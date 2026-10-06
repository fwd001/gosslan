// 职责边界：
// - transport 行为测试分册之15 —— 监听口平台语义：accept 后重绑、Windows 断离式关闭、启停循环仍服务
// 为什么拆：`transport/tests.rs` 原来 3,227 行、5 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
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
