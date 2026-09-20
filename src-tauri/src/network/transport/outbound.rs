/// 字符串 IP 是否为虚拟地址（用于 peers 表中已存储的 IP 字符串判断）。
fn is_virtual_ip_str(ip_str: &str) -> bool {
    ip_str
        .parse::<Ipv4Addr>()
        .map(|ip| is_virtual_ip(&ip))
        .unwrap_or(false)
}

// ---------------- 分帧 ----------------

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, msg: &Message) -> std::io::Result<()> {
    let json = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // 分帧（4 字节大端长度 + payload）与长度校验统一交给 bytes 层，
    // 业务侧只负责序列化 —— 单一真相源见 `transport::tcp`（P-A03）。
    crate::transport::tcp::write_bytes(w, &json).await
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Message> {
    // 同上：解帧与长度校验由 bytes 层负责，这里只做业务反序列化。
    let buf = crate::transport::tcp::read_bytes(r).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// 预认证阶段读首帧：上限收紧到 `MAX_PREAUTH_FRAME`（未验签的连接不得要求大缓冲）。
async fn read_frame_preauth<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Message> {
    let buf =
        crate::transport::tcp::read_bytes_capped(r, crate::protocol::MAX_PREAUTH_FRAME).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// ---------------- 出站发送 ----------------

/// 资料/控制帧内联头像的上限（与 Hello 同一判据）。
///
/// 超过它就**不再占优先通道**：聊天与好友请求必须永远排在大头像前面。
/// 与 HELLO_AVATAR_MAX_BYTES 恒等，避免两个数字漂移出两套行为。
pub const CONTROL_AVATAR_MAX_BYTES: usize = HELLO_AVATAR_MAX_BYTES;

/// 内联载荷超过这个大小的 Gossip 也降级到 bulk 通道。
///
/// 正常聊天/好友帧远小于它；只有「内联大图的通告」才会触到。16KiB 是保守值：
/// 单帧在 BLE 上按 508B/片算也只有约 32 片（约 0.4s），不会把优先队列拖住。
pub const BULK_GOSSIP_PAYLOAD_MAX_BYTES: usize = 16 * 1024;

// 通道分类的单一事实来源已收进 `dispatch::message_priority`（旧 `is_bulk_message` 删除，
// 阈值常量由它从这里复用 —— 同名概念不再有两份）。

/// 计算一次发送要按什么顺序尝试各条链路（纯函数，便于单测 + 护栏非空转）。
///
/// ## 为什么需要「按端点对齐」这一层
/// mesh 层（`Connection`）与传输层（`Link`）是**两套**链路表，且**不保证 1:1 同序**：
/// `handle_incoming` 追加 `Link` 时**不做端点去重**，而 `upsert_connection` 按端点去重；
/// 两者还有各自的登记/清理窗口。所以 `pick_link` 返回的**下标绝不能直接拿去索引 `Link`**
/// —— 必须用端点把 mesh 连接映射回传输链路。这正是复核里点名的坑。
///
/// ## 缺候选时怎么办（登记窗口）
/// 传输链路存在、mesh 侧还没登记（或刚被清理）时**合成一条「刚播种」的候选**，
/// 当作健康处理：它是一条**我们刚接受/建立的真实 TCP 连接**，不能因为登记窗口而
/// 被判不可用。反过来 mesh 侧多出来的连接（传输已清理）不参与排序。
///
/// 返回：`links` 的下标序列，按「优先尝试」排序。全部不健康时 `pick_link` 会退回
/// 首条（保持可用），其余链路仍然排在后面做 failover。
fn route_order(
    links: &[crate::state::Link],
    peer_id: &str,
    conns: &[crate::mesh::Connection],
    now_ms: i64,
    health_timeout_ms: i64,
    max_failures: u32,
) -> Vec<usize> {
    // 与 `links` 同序的候选：能按端点命中就用真实健康信息，否则合成「刚播种」候选。
    let candidates: Vec<crate::mesh::Connection> = links
        .iter()
        .map(|l| {
            let ep = l.endpoint.clone();
            if let Some(c) = conns.iter().find(|c| c.endpoint == ep) {
                c.clone()
            } else {
                let mut fresh = crate::mesh::Connection::new(peer_id, ep, l.path_kind);
                fresh.health.seed_read_seen(now_ms);
                fresh
            }
        })
        .collect();

    let Some(best) = crate::mesh::pick_link(&candidates, now_ms, health_timeout_ms, max_failures)
    else {
        return Vec::new();
    };
    // 选中的排最前，其余保持插入序做 failover。
    let mut order: Vec<usize> = Vec::with_capacity(candidates.len());
    order.push(best);
    for i in 0..candidates.len() {
        if i != best {
            order.push(i);
        }
    }
    order
}

/// `try_send` 第一轮全部遇到「信道满」时的**有界**补试时长。
///
/// 之所以不是直接 `Err`：信道满只说明对端这一拍消费不过来（writer 正在写 TCP），
/// 短暂等待通常能成功，直接失败会让上层误判「发送失败」。
/// 之所以有界：无界等待会在对端僵死时**永久挂起**调用方（复核确认的真实缺陷）。
const SEND_QUEUE_FULL_TIMEOUT: Duration = Duration::from_millis(500);

/// 尝试通过已建立连接发送消息；无连接则返回 Err。
///
/// 一个 peer 可能有多条连接（LAN + Tailscale + BLE）：**依次尝试**。
/// 某条连接已断（channel 关闭 → send 失败）就自动换下一条 —— 这是连接级 failover。
/// 任一连接成功即返回，所以消息仍然只发出一次（单连接场景下与改造前等价）。
///
/// ## 顺序由选路决定（M3-b）
/// 自 M3-b 起，尝试顺序不再等于插入顺序，而是 `route_order` 给出的顺序：
/// **活性过滤 + 路径优先级 LAN > Routed > Bluetooth + 稳定序打破平局**（ADR-0014 §3.2，
/// `mesh::selection::pick_link`）。单链路时顺序无变化（行为零变化）。
///
/// ## 为什么先快照 Sender 再发送（而不是持锁发送）
///
/// `state.links` 是**全局**连接表：建链登记、读循环清理、`has_link`/`ensure_link`、
/// 心跳、所有 peer 的发送都要拿它。原先这里在**持锁**状态下 `tx.send(..).await` ——
/// mpsc 容量有限（1024），一条拥塞/僵死的链路会让 `send` **挂起**（而不是返回 Err），
/// 于是：① 整张连接表被锁住，别人的建链/清理/发送全部阻塞；② 本函数的「换下一条」
/// 永远走不到（只有 channel **关闭**才返回 Err）。
/// 快照只克隆 `mpsc::Sender`（廉价、可 clone），锁在 await 之前就释放。
///
/// ## 两轮发送（复核确认的 High 缺陷的修法）
///
/// 第一轮**全部用非阻塞 `try_send`**：`Closed` / `Full` 都只意味着「这一条现在不行」，
/// 立刻换下一条。这样「信道满」也能触发 failover —— 原实现只有 `Closed` 才换。
/// 若所有链路都满（对端普遍消费不过来），才对**第一条满的**做一次有界补试
/// （`SEND_QUEUE_FULL_TIMEOUT`），超时即返回 Err，**绝不无限挂起**。
/// 注意：`Err` 不代表消息丢了 —— 单聊消息在 `send_message` 里已先入 outbox，
/// 由 Hello/心跳触发 `flush_outbox` 补发（这是既有契约）。
/// 按给定顺序尝试把消息投进各连接的 mpsc；任一成功即返回。
///
/// 抽成独立函数的唯一目的是**可测**：failover（「被选中那条断了 → 下一条仍送达」）
/// 是 M3-b 的核心承诺，但它埋在 `try_send` 里、要先构造 `AppState` 才能验证。
/// 这里只依赖「若干对 Sender + 一个顺序」，于是可以用真实 mpsc 信道直接钉死：
/// 关掉被选中那条的接收端、断言消息落到了下一条。
///
/// 两轮策略（复核确认的 High 缺陷的修法）：
/// ① 第一轮全用**非阻塞** `try_send`：`Closed` / `Full` 都只说明「这一条现在不行」，
///    立刻换下一条 —— 原实现只有 `Closed` 才换，信道满会**挂起**（并锁死调用方）；
/// ② 全部为 `Full` 时才对该条做**有界**补试（`SEND_QUEUE_FULL_TIMEOUT`），超时即 `Err`。
async fn send_over_order(
    senders: &[(
        mpsc::Sender<Message>,
        mpsc::Sender<Message>,
        mpsc::Sender<Message>,
    )],
    order: &[usize],
    msg: &Message,
    priority: crate::network::dispatch::MessagePriority,
) -> Result<(), String> {
    use crate::network::dispatch::MessagePriority;
    let mut last_err = "未建立连接".to_string();
    let mut first_full: Option<&mpsc::Sender<Message>> = None;
    for &i in order {
        let Some((high_tx, normal_tx, low_tx)) = senders.get(i) else {
            continue;
        };
        let tx = match priority {
            MessagePriority::High => high_tx,
            MessagePriority::Normal => normal_tx,
            MessagePriority::Low => low_tx,
        };
        match tx.try_send(msg.clone()) {
            Ok(()) => return Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                last_err = "连接已关闭".to_string();
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                if first_full.is_none() {
                    first_full = Some(tx);
                }
            }
        }
    }
    if let Some(tx) = first_full {
        return match tokio::time::timeout(SEND_QUEUE_FULL_TIMEOUT, tx.send(msg.clone())).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("发送队列已满（对端消费不过来）".to_string()),
        };
    }
    Err(last_err)
}

/// 尝试通过已建立连接发送消息；无连接则返回 Err。
pub async fn try_send(state: &AppState, peer_id: &str, msg: &Message) -> Result<(), String> {
    // ① 锁作用域内只做「取 + 克隆」，不 await（锁跨 await 会让一条拥塞链路锁死全表）。
    let links: Vec<crate::state::Link> = {
        let g = state.links.lock().await;
        match g.get(peer_id) {
            Some(l) if !l.is_empty() => l.clone(),
            _ => return Err("未建立连接".to_string()),
        }
    };

    // ② 取健康阈值与 mesh 连接（两把锁分别取，不嵌套）。
    let (health_timeout_ms, max_failures) = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        (pm.health_timeout_ms(), pm.max_failures())
    };
    let conns: Vec<crate::mesh::Connection> = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        pm.get(peer_id)
            .map(|p| p.connections().to_vec())
            .unwrap_or_default()
    };
    // ③ 选路（M3-b）：按**端点**对齐两套链路表后交给 `pick_link`，返回发送顺序。
    let order = route_order(
        &links,
        peer_id,
        &conns,
        db::now_ms(),
        health_timeout_ms,
        max_failures,
    );

    // ④ 按选路顺序投递（两轮策略见 `send_over_order`）。
    let senders: Vec<(
        mpsc::Sender<Message>,
        mpsc::Sender<Message>,
        mpsc::Sender<Message>,
    )> = links
        .iter()
        .map(|l| (l.high.clone(), l.normal.clone(), l.low.clone()))
        .collect();
    let priority = crate::network::dispatch::message_priority(msg);
    send_over_order(&senders, &order, msg, priority).await
}

/// 为一条**有序字节流**（文件分片）解析它应当钉住的链路：选路结果里那条"实际会走的"连接。
///
/// 为什么文件分片不能像其它消息一样用 `try_send`（真机 2026-09-19：一次多选里
/// 500-600MB 的文件发到 100% 报"文件分片顺序错误"，群聊连发 9-10 张图总有 1-2 张收不全，
/// 单独发同一个文件/图片则必定成功）：
///
/// `try_send` 是**逐消息**重算选路的，且第一轮用非阻塞 `try_send` —— 队列满就算这条链路
/// "现在不行"，立刻换下一条（这是 M3-b 给普通消息设计的 failover）。一个 peer 有多条独立
/// TCP 连接，同一个 `FileChunk{seq}` 流因此可能被拆到两条连接上发：两条连接的到达顺序
/// 互不保证，接收端 `file::write_chunk` 要求 seq 严格递增、追加写且**不 seek**，一片失序
/// 就 `ChunkSeq::Gap` ⇒ 整条传输判死。
///
/// 空闲时队列不满，永远走第一条链路，所以单发看不出问题；两个流并发共用一条 1024 槽的
/// Low 队列（每连接一套，见 `dispatch.rs`），满了才第一次真正触发 failover —— 症状由此
/// 只在多文件并发时出现。
///
/// 返回 `None` 表示当前没有可用链路（调用方按可重试失败处理，交给 outbox 续投）。
pub(crate) async fn resolve_stream_link(
    state: &AppState,
    peer_id: &str,
) -> Option<crate::state::Link> {
    let links: Vec<crate::state::Link> = {
        let g = state.links.lock().await;
        match g.get(peer_id) {
            Some(l) if !l.is_empty() => l.clone(),
            _ => return None,
        }
    };
    let (health_timeout_ms, max_failures) = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        (pm.health_timeout_ms(), pm.max_failures())
    };
    let conns: Vec<crate::mesh::Connection> = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        pm.get(peer_id)
            .map(|p| p.connections().to_vec())
            .unwrap_or_default()
    };
    let order = route_order(
        &links,
        peer_id,
        &conns,
        db::now_ms(),
        health_timeout_ms,
        max_failures,
    );
    // 只在"选路优先 + 通道仍开着"里取第一条；全关了才返回 None。
    order
        .iter()
        .filter_map(|&i| links.get(i))
        .find(|l| !l.low.is_closed())
        .cloned()
}

/// 在**指定的那一条**链路上投递一帧。队列满时原地等待，绝不换链路。
///
/// 与 `send_over_order` 的两点关键差别，都是为了保序：
/// - 满 ⇒ 等（背压），而不是 failover 到另一条连接；
/// - 等待不设局部超时 —— 链路死掉时 writer 循环退出会 drop 掉 Receiver，`send` 立刻返回
///   `Err`，所以不会真的挂住；整体上限由调用方的 `send_deadline_for(size)` 兜住。
///   在这里加短超时反而更糟：超时放弃后，队列里那批在途旧分片仍会被送达，而续传尝试的
///   seq 从 0 重新数，两者混在同一条追加写的流里 ⇒ 更难恢复的失序。
pub(crate) async fn send_on_link(link: &crate::state::Link, msg: &Message) -> Result<(), String> {
    use crate::network::dispatch::MessagePriority;
    let tx = match crate::network::dispatch::message_priority(msg) {
        MessagePriority::High => &link.high,
        MessagePriority::Normal => &link.normal,
        MessagePriority::Low => &link.low,
    };
    match tx.try_send(msg.clone()) {
        Ok(()) => return Ok(()),
        Err(mpsc::error::TrySendError::Closed(_)) => return Err("链路已关闭".to_string()),
        Err(mpsc::error::TrySendError::Full(_)) => {}
    }
    tx.send(msg.clone())
        .await
        .map_err(|_| "链路已关闭".to_string())
}

/// 一次等待 tick 之后，调用方决定继续等还是放弃（`send_on_link_with_tick` 用）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Tick {
    Wait,
    Abort,
}

/// 与 `send_on_link` 等价，但在**等待期间**每隔 `tick` 醒来调用一次 `on_tick`。
///
/// 为什么必须存在：对端不收时 `send_on_link` 就一直挂在背压上，任何写在它**之后**的
/// 停滞检查都得不到执行机会 —— 检查必须活在这个等待里面。
///
/// 安全性前提（已实测钉死，见 `timed_out_send_leaves_nothing_behind`）：`tx.send()` 的
/// future 被丢弃不会把消息留在队列里 ⇒ "超时后重发同一条"不会产生重复分片；重复分片在
/// 群接收端是致命的（严格 `seq != next_seq` 判死），所以这条不是"顺手加个轮询"。
pub(crate) async fn send_on_link_with_tick(
    link: &crate::state::Link,
    msg: &Message,
    tick: Duration,
    mut on_tick: impl FnMut() -> Tick,
) -> Result<(), String> {
    loop {
        match tokio::time::timeout(tick, send_on_link(link, msg)).await {
            Ok(r) => return r,
            Err(_) => {
                if on_tick() == Tick::Abort {
                    return Err("链路停滞（对端长时间未再接收）".to_string());
                }
            }
        }
    }
}

/// 无直连时，把一条**定向**帧借一跳中继发给 to（共享目录 / 中继文件在无直连时用）。
///
/// 只做「借邻居的直连」这一跳：给所有有直连的邻居各发一份（帧自带 to），邻居收到后
/// 按 to 直接投递（见 handle_message 顶部的定向中继分支）。邻居若与 to 没有直连就丢弃
/// —— 与既有 RelayChunk 的单跳限制一致；不泛洪，因此不存在环路。
pub(crate) async fn relay_send_to_neighbors(state: &AppState, to: &str, msg: &Message) {
    let peers: Vec<String> = { state.links.lock().await.keys().cloned().collect() };
    for p in peers {
        if p == to {
            continue;
        }
        let _ = try_send(state, &p, msg).await;
    }
}

/// 向所有已连接节点广播一条 Gossip 消息。
pub async fn broadcast_gossip(state: &AppState, envelope: GossipEnvelope) {
    let msg = Message::Gossip {
        envelope: envelope.clone(),
    };

    // 与 `try_send` 同理：锁内只做决策 + 克隆 Sender，发送一律在锁外。
    // 原来在持有 `links` 锁时 `send().await`，一条拥塞链路会锁死整张连接表。
    let targets: Vec<(mpsc::Sender<Message>, String, MeshEndpoint)> = {
        let links = state.links.lock().await;
        // 出站目标经 MeshRouter 裁决（§18 source exclusion）。
        //
        // 这里刻意用 `exclude_source` 而**不是** `select_outgoing`：后者带 fanout 截断，
        // 只适用于**转发**（§20 控制风暴）。源发必须覆盖所有直连节点，一旦截断，
        // 连接数超过 fanout 的节点就会收不到 —— 群消息静默漏发。
        let candidates: Vec<String> = links.keys().cloned().collect();
        let picked = {
            let router = state.mesh_router.lock().unwrap_or_else(|e| e.into_inner());
            router.exclude_source(&candidates, &envelope.sender_id)
        };
        // M3-d（2026-09-14 全 Windows 局域网真机）：按**路径优先级**选一条发送链路，
        // 而不是 v.first()（插入顺序）。旧写法在同一 peer 同时有 LAN 与 BLE 链路时，
        // Gossip/控制帧可能走 BLE——表现为「同局域网却走了蓝牙/中继」。
        // best_link_kind 给出 LAN > Routed > Bluetooth，再取该链路；同时保留 peer_id + endpoint
        // 以便 timeout 时能定位到具体 Connection 并标记 congestion。
        picked
            .iter()
            .filter_map(|peer| {
                let peer_id: &str = peer;
                let ls = links.get(peer_id)?;
                let kinds: Vec<PathKind> = ls.iter().map(|l| l.path_kind).collect();
                let best = crate::state::best_link_kind(&kinds)?;
                let link = ls.iter().find(|l| l.path_kind == best)?;
                // 队列由分类表决定：内联大载荷的 Gossip（带大图的 Presence 等）
                // 自动降级 Low，不占聊天道（P0#5 批次：消除恒走 Normal 的旁路）
                let tx = match crate::network::dispatch::message_priority(&msg) {
                    crate::network::dispatch::MessagePriority::High => link.high.clone(),
                    crate::network::dispatch::MessagePriority::Normal => link.normal.clone(),
                    crate::network::dispatch::MessagePriority::Low => link.low.clone(),
                };
                Some((tx, peer_id.to_owned(), link.endpoint.clone()))
            })
            .collect()
    };

    for (tx, _peer_id, _endpoint) in &targets {
        // ⚠️ **必须有界等待**：这是有界队列（1024），对端僵死（BLE 低带宽 / 半开 TCP）
        // 时无超时的 `send().await` 会让本函数永久挂起 —— 而它被 `handle_gossip` 内联
        // await，`handle_gossip` 又由 reader_loop 调用 ⇒ **另一个对端的读循环被卡住**，
        // 它后续的帧（含心跳）全部排队，最终被判不健康而拆链。
        // 即"一条拥塞链路伪造出全网链路故障"。口径与 `send_over_order` 一致。
        // 超时即丢弃该 peer 的这条 gossip（其 outbox 会在下次心跳/Hello 时补发）。
        // **必须留痕**：这条路径原先完全静默，真机排查「发出去但对方收不到」时不可观测。
        // 限频（每 30s 一条）避免拥塞时刷屏。
        if tokio::time::timeout(SEND_QUEUE_FULL_TIMEOUT, tx.send(msg.clone()))
            .await
            .is_err()
        {
            // 这条路径原先完全静默，真机排查「发出去但对方收不到」时不可观测。
            // 限频（每 30s 一条）避免拥塞时刷屏。
            if log_throttled("gossip_drop", 30_000) {
                state.logger.warn(
                    "transport",
                    "gossip 扇出队列满，丢弃本条（对方 outbox 会补发；持续出现说明该链路拥塞）"
                        .to_string(),
                );
            }
        }
    }
}

/// 转发候选集合 = **可达**邻居（有非空链路），不是 peers 的「已知/已发现」集合。
///
/// 为什么必须有（2026-09-19 审计 P0#7）：`peers` 是知识集——Presence/announce 会跨跳
/// 登记，异网段节点在 peers 里却没有 TCP 链路可发。拿它做 fan-out 候选，跨网段时
/// 「看得见几十个节点、零个可达」，`try_send` 每一个都失败且被 `let _ =` 静默吞掉
/// ——「节点互相帮转发」恰好在最需要它的场景失效。源发侧 `broadcast_gossip` 一直用的是
/// `links.keys()`，转发与此同口径（护栏：`gossip_fanout_targets_reachable_links`）。
async fn reachable_neighbors(state: &AppState, exclude: &str) -> Vec<String> {
    let links = state.links.lock().await;
    links
        .iter()
        .filter(|(id, v)| !v.is_empty() && id.as_str() != exclude)
        .map(|(id, _)| id.clone())
        .collect()
}
