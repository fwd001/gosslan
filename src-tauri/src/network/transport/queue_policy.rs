// 链路队列的容量策略（字节预算 / 槽数折算 / 写失败分流，第 2 步 · P3）
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第二批，判据与顺序见 docs/large-file-split-plan.md。

/// 一条链路 **low 队列**允许压住多少字节的待发分块。
///
/// 为什么是字节而不是帧数：旧形状是四个建链点各写三条 `mpsc::channel`、深度都是 1024，
/// 而一片 LAN 分块上线约 341 KB ⇒ **单链路最坏 ~350 MB**，多链路/群文件按连接翻倍，
/// 手机上就是 OOM 或整机变慢。背压本来就有（`send_on_link` 满了原地等 + `stall_tick`），
/// 错的只是"缓冲先分配完才开始排队"。8 MB ≈ 24 片 LAN 分块，够 writer 连续排空用。
pub const LINK_QUEUE_BYTE_BUDGET: usize = 8 * 1024 * 1024;

/// 只**收紧**不放宽：今天就是 1024，小分片（BLE 4 KB）时保持原深度。
const LINK_QUEUE_MAX_SLOTS: usize = 1024;

/// 折算下限。必须 ≥1（`mpsc::channel(0)` 直接 panic），留 8 是为了还能流水。
const LINK_QUEUE_MIN_SLOTS: usize = 8;

/// high / normal 两条队列的深度（帧数）。这两条上只有消息与控制帧，被
/// `MAX_MESSAGE_LEN` 与各类载荷上限卡着，不是内存问题 —— 保持今天的 1024 不动。
/// 起名而不写字面量是为了让"只剩一处策略"这件事可被判据检查。
const CONTROL_QUEUE_SLOTS: usize = 1024;

/// 一条队列的收发两端（`mpsc` 的一对）。
type LinkQueue = (mpsc::Sender<Message>, mpsc::Receiver<Message>);

/// 按"这一条链路会装的最大分片"把字节预算折算成槽数。纯函数、单调。
pub fn low_queue_slots(chunk_plain_bytes: usize) -> usize {
    let wire = crate::network::file::chunk_wire_bytes(chunk_plain_bytes).max(1);
    (LINK_QUEUE_BYTE_BUDGET / wire).clamp(LINK_QUEUE_MIN_SLOTS, LINK_QUEUE_MAX_SLOTS)
}

/// 一条链路的三条队列（high / normal / low），四个建链点共用这一份策略。
///
/// high/normal 仍按帧数；只有 low（分块通道）按字节预算 —— 分片是这里唯一能到几百 KB 的东西。
/// 优先级模型一字未动，这次只改容量语义。
pub(crate) fn link_channels(chunk_plain_bytes: usize) -> (LinkQueue, LinkQueue, LinkQueue) {
    (
        mpsc::channel(CONTROL_QUEUE_SLOTS),
        mpsc::channel(CONTROL_QUEUE_SLOTS),
        mpsc::channel(low_queue_slots(chunk_plain_bytes)),
    )
}

/// 单次写出的结果（D8-4）。把"主动放弃"与"写失败"分开：
/// 前者是我们在停机/拆链路，不该记成链路故障（否则选路会把正在关闭的链路算成失败）。
///
/// `Local` 是第三种（第 1 步 · 故障隔离）：**载荷自身不可写**（超长 / 序列化不出来），
/// 一个字节都没上链路 ⇒ 这一帧作废即可，**链路保留**。
/// 旧形状是 `res.is_ok()` 一把抓 ⇒ 一条永远发不出去的超大帧会把整条连接判死，
/// 再顺带拖掉同一 peer 其它链路上正在跑的文件传输。
enum WriteOutcome {
    Ok,
    Local(String),
    Failed,
    Stopped,
}

#[allow(clippy::too_many_arguments)]
async fn writer_loop(
    state: Arc<AppState>,
    peer_id: String,
    // 本连接的端点：健康信号要按**连接**记，必须能唯一定位到是哪一条。
    // 类型是 transport 无关的 `Endpoint`（BLE 也需要它）。
    endpoint: MeshEndpoint,
    mut w: TcpSender,
    mut high_rx: mpsc::Receiver<Message>,
    mut normal_rx: mpsc::Receiver<Message>,
    mut low_rx: mpsc::Receiver<Message>,
    mut shutdown: watch::Receiver<bool>,
    // 本连接的取消信号（M3#6）：由健康 watchdog 在半开链路上触发。
    mut cancel: watch::Receiver<bool>,
) {
    let mut high_open = true;
    let mut normal_open = true;
    let mut low_open = true;
    loop {
        if !high_open && !normal_open && !low_open {
            break;
        }
        let msg = tokio::select! {
            biased;
            // 停止信号优先：立刻放弃待发帧并 drop 写半，让 socket 尽快关闭
            // （Windows 上配合 SO_LINGER=0 发 RST，不留下 TIME_WAIT）。
            _ = shutdown.changed() => break,
            // 本连接被判死（读活性长期过期）→ 与全局停机同样立即收尾。
            _ = cancel.changed() => break,
            maybe = high_rx.recv(), if high_open => maybe,
            maybe = normal_rx.recv(), if normal_open => maybe,
            maybe = low_rx.recv(), if low_open => maybe,
        };
        match msg {
            Some(msg) => {
                // ⚠️ **写必须可被打断**（D8-4）：对端不读时发送缓冲满，`write_all` 会长时间
                // 阻塞；而 shutdown/cancel 只有在回到循环顶部才会被轮询 ⇒ 退出流程与
                // watchdog 的"精确拆链路"在写阻塞场景下都会失效（STOP_TASK_TIMEOUT 兜底
                // 也只能打日志放行）。放进 select 后，停机与判死都能立刻放弃这一帧。
                //
                // 主动放弃时**不记失败**：那不是链路故障，是我们自己在拆。半写出去的分片
                // 会让对端看到截断帧并自行断开 —— 本来就是要断的链路，无妨。
                let outcome = tokio::select! {
                    biased;
                    _ = shutdown.changed() => WriteOutcome::Stopped,
                    _ = cancel.changed() => WriteOutcome::Stopped,
                    res = write_frame(&mut w, &msg) => match res {
                        Ok(()) => WriteOutcome::Ok,
                        Err(WriteError::Local(why)) => WriteOutcome::Local(why),
                        // 真 socket 写失败 ⇒ 这条连接已经不可信，照旧走下面的判死+拆链。
                        Err(WriteError::Socket(_)) => WriteOutcome::Failed,
                    }
                };
                if matches!(outcome, WriteOutcome::Ok) {
                    // 写成功只记**出站**活性（诊断口径）。M3-0b 起它**不**参与 is_healthy：
                    // 半开 TCP 上写会一直"成功"，那是本缺陷要被排除的伪证据。
                    mark_conn_write_seen(&state, &peer_id, &endpoint);
                    mark_file_wire_progress(&state, &msg, &peer_id);
                    continue;
                }
                if matches!(outcome, WriteOutcome::Stopped) {
                    break;
                }
                if let WriteOutcome::Local(why) = outcome {
                    // 载荷自身不可写（超长 / 序列化不出来）：一个字节都没上链路 ⇒
                    // 这一帧作废，**链路保留**、不记连接失败。
                    // ⚠️ 必须响：静默丢帧违反 INV-005「不允许静默丢失」。发不出去的帧
                    //    仍留在 outbox / file_outbox 里，界面上是"未送达/排队中"而不是"已送达"。
                    eprintln!(
                        "[gosslan][WRITE] peer={peer_id} 帧 {} 本机就写不出去，已丢弃这一帧（链路保留）：\
                         {why} —— 这是本机 bug，换链路也会同样失败，别当网络问题查",
                        msg.wire_kind()
                    );
                    continue;
                }
                {
                    // TCP write 失败：普通消息由 outbox 重发；ReadReceipt 需要特殊处理——
                    // 它没有 outbox 行，如果 pending 已被 flush_pending_reads 清除，
                    // 此处不恢复就永久丢失。将 timestamp 重新放回 pending_reads，
                    // 下一次建链 / Hello / Heartbeat 会再次 flush 重发。
                    if let Message::ReadReceipt { last_read_ts, .. } = &msg {
                        let mut pending = state
                            .pending_reads
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let cur = pending.entry(peer_id.clone()).or_insert(*last_read_ts);
                        *cur = (*cur).max(*last_read_ts);
                    }
                    // 这条连接已经写不出去了 —— 记一次失败，供 M3 的选路与收敛使用。
                    mark_conn_failure(&state, &peer_id, &endpoint);
                    break;
                }
            }
            None => {
                // select 无法直接区分是哪个分支关闭，用三个 recv 的 is_closed 兜底。
                if high_rx.is_closed() {
                    high_open = false;
                }
                if normal_rx.is_closed() {
                    normal_open = false;
                }
                if low_rx.is_closed() {
                    low_open = false;
                }
                if !high_open && !normal_open && !low_open {
                    break;
                }
            }
        }
    }
}

async fn reader_loop(
    state: Arc<AppState>,
    mut r: TcpReceiver,
    peer_id: String,
    // 本连接的端点，用于按连接记健康信号。
    endpoint: MeshEndpoint,
    link_tx: mpsc::Sender<Message>,
    mut shutdown: watch::Receiver<bool>,
    // 本连接的取消信号（M3#6）：半开链路上的读会永久阻塞，只有它能打断。
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let res = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = cancel.changed() => break,
            res = read_frame(&mut r) => res,
        };
        match res {
            Ok(msg) => {
                // 入站读到帧是比「写成功」**更强**的活性证据：对端确实活着（不只是内核收下了
                // 我们的字节）。这条信号正是半开 TCP 场景下唯一能区分「真活 / 假活」的东西。
                mark_conn_seen(&state, &peer_id, &endpoint);
                handle_message(&state, &peer_id, msg).await
            }
            Err(_) => break,
        }
    }
    // ⚠️ 这里**不再**清接收器（P1 故障隔离）。旧形状是"这条连接的读半一结束就按 peer 清"，
    // 而同一 peer 完全可以同时挂着 LAN + Tailscale + BLE 多条链路 ⇒
    // 断其中一条会连带杀掉另外几条链路上**正在收**的文件（下面 `:1563` 那段
    // 早就写明了「断一条 ≠ peer 下线」，文件这两处一直与它自相矛盾）。
    // 清理挪到下面算出 `peer_now_offline` 之后，只在"这个 peer 真的没有任何链路了"时做；
    // "对端在线但这一单被放弃"那种情形由 `sweep_stalled_receives` 负责回收。
    // 只移除**这一条**连接（按 channel 身份匹配），不是整条删光：
    // 同一 peer 可能还连着别的端点（LAN + Tailscale），断一条 ≠ peer 下线 ——
    // 这正是 6b 的核心语义。旧实现整条 remove，会让另一条连接一起消失。
    let peer_now_offline = {
        // 先记下被移除的是哪个端点（mesh 层要按端点删对应的 Connection）
        let removed_endpoint = {
            let links = state.links.lock().await;
            links
                .get(&peer_id)
                .and_then(|list| list.iter().find(|l| l.low.same_channel(&link_tx)))
                .map(|l| l.endpoint.clone())
        };
        let mut links = state.links.lock().await;
        let removed = match links.get_mut(&peer_id) {
            Some(list) => {
                let before = list.len();
                list.retain(|l| !l.low.same_channel(&link_tx));
                list.len() != before
            }
            None => false,
        };
        // 移除后该 peer 已无任何连接 → 才算真的离线，并**把空的 Vec 一起删掉**。
        let empty = links.get(&peer_id).map_or(true, |v| v.is_empty());
        if removed && empty {
            // ⚠️ 只 retain 不删 key 会留下一个**空 Vec**，而好几处判定用的是
            // `links.keys()` / `contains_key`（不是 `has_link` 的非空判据）：
            //  - `sweep_peers` 认为「有活跃链路」→ 该 peer 永不被清扫；
            //  - `get_friends` 的 Friend.online 恒 true → 前端**永久显示在线**；
            //  - 定向转发 `contains_key` 命中「直连」分支 → `try_send` 失败后
            //    **不再洪泛兜底**，跨跳的好友申请/回执可能永久丢失。
            // 一次「连过又掉线」的节点就能让上述三条同时成立（复核抓到的 High 缺陷）。
            links.remove(&peer_id);
        }
        let offline = peer_offline_after_tail(removed, empty);
        drop(links);
        // 释放 links 锁后再动 mesh 层（避免持锁嵌套）
        if let Some(ep) = removed_endpoint {
            unregister_connection(&state, &peer_id, &ep);
        }
        offline
    };
    // 所有连接都断了才标记离线；还剩别的连接则保持在线（failover 生效）
    if peer_now_offline {
        mark_peer_offline(&state, &peer_id).await;
        // 只有到这一步才有资格清接收器：这个 peer 确实一条链路都不剩了。
        // 单聊与群文件两张表同规矩（旧代码把它们放在上面，与 `:1563` 的"只删这一条"矛盾）。
        file::fail_receives_for_peer(&state, &peer_id);
        // 摘表与判据同一次持锁（旧写法是锁内 collect id、锁外逐个收尾 ——
        // 那两步之间挤进来的新 FileOffer 会被误判死，见 file::take_group_receives_for_peer）。
        let taken = file::take_group_receives_for_peer(&state, &peer_id);
        // 收尾在锁外：里面要写库、要 emit，都不该持着接收表。
        // ★ 摘出来的那一份必须一起用完：`fail_taken_group_receive` 是群收件人**内容台账**
        //   （`record_failure` ⇒ Incomplete，建链时按退避自动重取）的唯一写点，只调 finalize
        //   那一半会把台账留在 Active。超时那一路（`sweep_stalled_receives`）一直是成对调的。
        for (tid, r) in taken {
            file::fail_taken_group_receive(&state, &r);
            finalize_failed_group_receive(&state, &tid);
        }
    }
}
