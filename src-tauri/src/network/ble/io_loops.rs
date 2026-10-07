// BLE 的**读写循环**（三级抢占调度 + 帧级活性判据）。
// 
// 为什么单独一册：这一册判的是"链路写满时谁被饿死"和"读活性算不算健康"（真机 2026-09-12 那条
// 僵尸链路：只结束写循环、读还在跑 ⇒ 看门狗 45s 也不拆）。它既不属于 central 也不属于外设，
// 两条角色跑的是同一对循环。
// 
// 恒等判据（与 transport / file 两刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 本模块整体在 `#[cfg(feature = "bluetooth")]` 后面（`network/mod.rs:10`）⇒ 这些册跟着根文件一起门控，
//   不需要各自再写 cfg。搬家同批必须做的三件事：`network/mod.rs::ble_src_for_guards()` 登记本册、
//   `docs/domains.data.mjs` 认领、`scripts/check-ble-constants.mjs` 的 BLE_DOMAIN_FILES 覆盖本册
//   （那份名单是**硬编码文件清单** —— 漏了不会红，会让那条守卫对新册里的匿名常量永远失明）。

#[allow(clippy::too_many_arguments)]
/// BLE writer scheduler：High > Normal > Low 三级 channel（biased select，帧间严格优先）。
///
/// 与 TCP writer_loop 的差别在帧的代价：BLE 的 send_frame 在底层做 MTU 分片循环，
/// 一帧 4KB FileChunk 最坏（小 MTU）要发数秒。本循环的抢占粒度是**帧**：
/// High/Normal 的等待上限 = 正在发送的那一条 Low 帧完成的时间。
/// 片间 yield 需要四个平台的 FrameSink 同步改 fragment 级 API，尚未实现
/// —— 与 `dispatch` 模块头的「抢占粒度」声明保持一致，不要在注释里超前宣称。
async fn ble_writer_loop<S: FrameSink + 'static>(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut writer: S,
    mut high_rx: mpsc::Receiver<Message>,
    mut normal_rx: mpsc::Receiver<Message>,
    mut low_rx: mpsc::Receiver<Message>,
    mut shutdown: watch::Receiver<bool>,
    mut cancel: watch::Receiver<bool>,
) {
    let mut high_open = true;
    let mut normal_open = true;
    let mut low_open = true;
    loop {
        if !high_open && !normal_open && !low_open {
            break;
        }
        // 外层 select：High > Normal > Low，正常调度优先级
        let msg = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = cancel.changed() => break,
            maybe = high_rx.recv(), if high_open => maybe,
            maybe = normal_rx.recv(), if normal_open => maybe,
            maybe = low_rx.recv(), if low_open => maybe,
        };
        match msg {
            Some(msg) => {
                let trace = is_logworthy_frame(&msg).then(|| frame_trace(&msg));
                let Ok(bytes) = serde_json::to_vec(&msg) else {
                    continue;
                };
                // 写也要能被停机/判死打断（与 TCP 的 writer_loop 同一考虑）。
                //
                // ⚠️ **写失败先退避重试，不能一次就判死**（真机 2026-09-13 安卓日志）：
                // 旧实现在第一次写失败就 `break` 结束**写**循环，而**读**循环还活着 ⇒
                // 这条链路变成"能收不能发"的僵尸：上层发送永远报「连接失败/连接已关闭」，
                // 而 45s 看门狗是按**读**活性判健康的（还在收到对端的 gossip）⇒ 永远不拆 ⇒
                // 只能重启应用才恢复。BLE 的写失败大多是瞬态的（对端通知队列满 / 链路忙 /
                // 连发被拒），退避重试即可；真死了也走下面的"拆链路"而不是留个半死链路。
                let mut attempt = 1u32;
                let res = loop {
                    let one = tokio::select! {
                        biased;
                        _ = shutdown.changed() => Err("停机中".to_string()),
                        _ = cancel.changed() => Err("链路已取消".to_string()),
                        res = writer.send_frame(&bytes) => res,
                    };
                    match one {
                        Ok(n) => break Ok(n),
                        Err(e) => {
                            // 「帧无法分片」是**这一帧**的问题（太大/MTU 异常），重试无意义
                            if e.starts_with("帧无法分片") || attempt >= WRITE_RETRY_ATTEMPTS {
                                break Err(e);
                            }
                            // 帧长必须打出来：光看"写失败"无法判断是"分片太大"还是别的原因。
                            // 真机排查时这一行能直接给出「写了多少字节」。
                            state.logger.warn(
                                "ble",
                                format!(
                                    "[SEND] 写失败第 {attempt}/{WRITE_RETRY_ATTEMPTS} 次（{}ms 后重试）peer={peer_id} ep={ep} 帧长={} 原因={e}",
                                    WRITE_RETRY_WAIT.as_millis(),
                                    bytes.len()
                                ),
                            );
                            tokio::select! {
                                biased;
                                _ = shutdown.changed() => break Err("停机中".to_string()),
                                _ = cancel.changed() => break Err("链路已取消".to_string()),
                                _ = tokio::time::sleep(WRITE_RETRY_WAIT) => {}
                            }
                            attempt += 1;
                        }
                    }
                };
                // 分片数要留痕：对端会打 `[FRAG] 收到通知 N 条`，两边的数字一比就知道
                // **是发少了还是收丢了**（真机 2026-09-13：742B 的帧需要 53 片，对端只到 38 片）。
                if let (Ok(n), Some(trace)) = (&res, trace.as_deref()) {
                    state.logger.info(
                        "ble",
                        format!(
                            "[SEND] {trace} → peer={peer_id} ep={ep} bytes={} 分片={n}",
                            bytes.len()
                        ),
                    );
                }
                // 文件分块**真的写出去了**才算进展（发送侧等 FileCompleteAck 的判据，
                // 见 `transport.rs::mark_file_wire_progress` 的注释）。BLE 上这一步尤其关键：
                // 一个 4KiB 文件块要 399 片 × 12ms ≈ 5.5s，判据必须落在"写出去"上。
                if res.is_ok() {
                    mark_file_wire_progress(&state, &msg, &peer_id);
                }
                if let Err(e) = &res {
                    // 「帧无法分片」是**这一帧**太大/MTU 异常，不是链路坏了：拆链路会让
                    // 同一条连接上的其它传输全部失败（真机：一张大图把链路打死，之后的好友
                    // 请求/消息全断）。这里只丢这一帧并留 warn —— 上层 outbox 会按自己的节奏
                    // 重发；真正的写失败（对端走了）仍然拆链路。
                    if e.starts_with("帧无法分片") {
                        state.logger.warn(
                            "ble",
                            format!(
                                "[SEND] 丢弃无法分片的帧（链路保留）peer={peer_id} ep={ep} bytes={} 原因={e}",
                                bytes.len()
                            ),
                        );
                        continue;
                    }
                    state.logger.warn(
                        "ble",
                        format!(
                            "[SEND] 写失败（已重试 {WRITE_RETRY_ATTEMPTS} 次）⇒ 拆掉该链路并等待重拨 peer={peer_id} ep={ep} {} 原因={e}",
                            trace.as_deref().unwrap_or("type=?")
                        ),
                    );
                    // ⚠️ **必须连读循环一起取消**（`teardown_link` 在读循环收尾里）：
                    // 只结束写循环会留下"能收不能发"的僵尸链路，而看门狗按读活性判健康、
                    // 永远不拆它 ⇒ 用户只能重启（真机 2026-09-13）。
                    // 拆掉之后 `teardown_link` 会清该地址的退避并 `wake_scan`，
                    // 下一轮扫描即可重拨。
                    // 写循环手里只有 `cancel` 的**接收端**，所以要按端点去链路表里
                    // 找这一条的取消发送端（与看门狗同一套"按端点定位"的写法）。
                    {
                        let links = state.links.lock().await;
                        if let Some(l) = links
                            .get(&peer_id)
                            .and_then(|v| v.iter().find(|l| l.endpoint == ep))
                        {
                            let _ = l.cancel.send(true);
                        }
                    }
                    break;
                }
            }
            None => {
                // select 的某臂被禁用（对端 Sender 全部 drop ⇒ recv 立即 None 且永久 None）。
                // ⚠️ 三个臂都要判：漏掉 high 会让本循环以 100% CPU 空转且永不退出
                //（链路被摘 ⇒ Link drop ⇒ high sender 归零 ⇒ 该臂每一轮都命中）。
                if high_rx.is_closed() {
                    high_open = false;
                }
                if normal_rx.is_closed() {
                    normal_open = false;
                }
                if low_rx.is_closed() {
                    low_open = false;
                }
            }
        }
    }
}

async fn ble_reader_loop<S: FrameSource + 'static>(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut reader: S,
    mut shutdown: watch::Receiver<bool>,
    mut cancel: watch::Receiver<bool>,
) {
    // 分片统计只在 central 侧（`BleReader`）有意义；外设侧没有这个计数。
    let mut last_frag_n: u64 = 0;
    let mut last_frag_other: u64 = 0;
    let mut last_drop_n: u64 = 0;
    // 退出原因：P0 可观测性 —— 每条链路拆的时候都必须知道是"停机"、"cancel 信号"
    // 还是"读循环错误"，因为 cancel 信号本身可能来自三条不同路径
    // （写失败 / 看门狗 stale / adapter events 被动断链）。
    #[allow(unused_assignments)]
    let mut exit_reason: &'static str = "unknown";
    loop {
        let frame = tokio::select! {
            biased;
            _ = shutdown.changed() => { exit_reason = "shutdown"; break },
            _ = cancel.changed() => { exit_reason = "link_canceled"; break },
            res = reader.next_frame(READ_IDLE) => res,
        };
        match frame {
            Ok(Some(bytes)) => match serde_json::from_slice::<Message>(&bytes) {
                Ok(msg) => {
                    if is_logworthy_frame(&msg) {
                        state.logger.info(
                            "ble",
                            format!(
                                "[RECV] {} ← peer={peer_id} ep={ep} bytes={}",
                                frame_trace(&msg),
                                bytes.len()
                            ),
                        );
                    }
                    // ⚠️ **读到帧 = 这条链路还活着**，必须回灌 mesh 健康度（2026-09-13 审计）。
                    //
                    // 漏掉这一句的后果（真机体感就是"蓝牙时好时坏、延迟很高"）：
                    // `ConnectionHealth` 的读活性只在**建链时播种一次**
                    // （`transport.rs::register_connection` → `seed_connection_read_seen`），
                    // 此后只由 `reader_loop` 刷新 —— 而 BLE 的读循环原来没有调用它。
                    // 于是任何健康的 BLE 链路：15s 后 `is_healthy` 判假（选路/镜像去重都会
                    // 按"不健康"处理），45s 被健康看门狗（`stale_connections`）当作死链路
                    // **拆掉**；对端再拨回来，45s 后再拆一次，无限循环。
                    // TCP 侧的对应调用见 `transport.rs` 的 `reader_loop`。
                    // 本函数同时服务 central（`BleReader`）与外设（`ChannelSource`）两条路径，
                    // 所以一处调用两个方向都覆盖。
                    mark_conn_seen(&state, &peer_id, &ep);
                    handle_message(&state, &peer_id, msg).await
                }
                Err(e) => state
                    .logger
                    .warn("ble", format!("丢弃无法解析的 BLE 帧 peer={peer_id}: {e}")),
            },
            Ok(None) => {
                // 窗口内没有分片：顺手回收半截消息（对端在半途断连时不会永久占内存）
                let _ = reader.gc();
                // **分片级可见性**：真机里"对端说发了、这边什么都没收到"时，
                // 这条日志能一眼区分"没发出来"与"发出来但没到"。
                if let Some((n, bytes, other)) = stats_fn(&reader) {
                    if n != last_frag_n || other != last_frag_other {
                        state.logger.info(
                            "ble",
                            format!(
                                "[FRAG] 收到通知 {n} 条 / {bytes} 字节（非本特征 {other} 条）                                 ← peer={peer_id} ep={ep}"
                            ),
                        );
                        last_frag_n = n;
                        last_frag_other = other;
                    }
                }
                // **被丢弃的分片**：坏片原来在重组器里被静默吞掉 —— 真机上只看到
                // "图片没到"，看不到"到了、被分片层丢了、原因是…"。这里只在计数**增加**时
                // 打一条 warn（不是每片一条），所以不会刷屏。
                if let Some((drops, reason)) = drops_fn(&reader) {
                    if drops != last_drop_n {
                        state.logger.warn(
                            "ble",
                            format!(
                                "[FRAG] 丢弃分片 {drops} 片（新增 {}，最近原因：{reason}）                                 ← peer={peer_id} ep={ep}",
                                drops - last_drop_n
                            ),
                        );
                        last_drop_n = drops;
                    }
                }
            }
            Err(e) => {
                state
                    .logger
                    .info("ble", format!("BLE 读结束 peer={peer_id} ep={ep}: {e}"));
                exit_reason = "reader_error";
                break;
            }
        }
    }
    // 收尾：只拆这一条（同一 peer 可能还有 LAN 链路）
    teardown_link(&state, &peer_id, &ep, exit_reason).await;
}
