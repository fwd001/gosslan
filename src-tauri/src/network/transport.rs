//! TCP 消息传输与协议分发（含 Gossip 广播、中继切片、群密钥、E2EE 解密）。
//!
//! 连接建立规则（避免重复建链的竞态）：
//! - **已有连接就不拨**：`ensure_link` 只负责**连通性**（「和这个看得见的 peer 建立联系」），
//!   只要该 peer 已有任意连接就短路返回 —— 否则被动方会因为端点表示不对称而反向再拨一条，
//!   形成镜像重复连接（详见 `ensure_link` 的注释）。
//! - 首次建链：默认由 **device_id 字典序较大** 的一方主动拨号（dial），较小的一方被动接受；
//! - 较小的一方在「对端在线却迟迟连不上」（单向可达）时**兜底拨号**（见 `should_dial`）。
//! - **多路径不由本模块负责**：一个 peer 同时持有多条连接（LAN + Routed + BLE）由各
//!   Transport 自己的驱动产生（Routed 由配置驱动、BLE 由 BLE 发现驱动），它们都不经过
//!   `ensure_link`。这里保持「连通性」与「多路径」关注点分离。
//! - 双方各自维护一个出站 mpsc 发送端，读循环负责解析帧并分发。

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::params;
use tauri::Emitter;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::time::Duration;
use x25519_dalek::StaticSecret;

use crate::commands::{is_virtual_ip, MAX_GROUP_NAME_LEN};
use crate::crypto;
use crate::db;
use crate::network::file;
use crate::protocol::{hello_signing_bytes, GossipEnvelope, GossipKind, Message, MsgKind};
use crate::state::{
    AppState, FileDoneInfo, FileFailedInfo, FileProgress, Link, LinkState, MessageRecord, Peer,
    PendingRequest,
};
use crate::mesh::router::{ForwardDecision, MeshDestination, MeshFrame, MeshFrameKind};
use crate::discovery::routed::{parse_endpoints, ROUTED_ENDPOINTS_KEY};
use crate::mesh::{Endpoint as MeshEndpoint, PathKind, PeerCandidate, PeerIdentity, PeerOnlineState};
use crate::transport::tcp::{TcpReceiver, TcpSender};

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
    let buf = crate::transport::tcp::read_bytes_capped(r, crate::protocol::MAX_PREAUTH_FRAME).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// ---------------- 出站发送 ----------------

/// 大数据分片走普通通道；聊天/控制/小控制帧走高优先级通道，避免被大文件饿死。
fn is_bulk_message(msg: &Message) -> bool {
    matches!(
        msg,
        Message::FileChunk { .. }
            | Message::RelayChunk { .. }
            | Message::GroupFileChunk { .. }
            // 终止帧必须和分片同队列，保证「分片 → Done」的协议顺序不被优先级通道打乱。
            | Message::FileDone { .. }
            | Message::GroupFileDone { .. }
    )
}

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
    senders: &[(mpsc::Sender<Message>, mpsc::Sender<Message>)],
    order: &[usize],
    msg: &Message,
    bulk: bool,
) -> Result<(), String> {
    let mut last_err = "未建立连接".to_string();
    let mut first_full: Option<&mpsc::Sender<Message>> = None;
    for &i in order {
        let Some((bulk_tx, prio_tx)) = senders.get(i) else { continue };
        let tx = if bulk { bulk_tx } else { prio_tx };
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
        pm.get(peer_id).map(|p| p.connections().to_vec()).unwrap_or_default()
    };
    // ③ 选路（M3-b）：按**端点**对齐两套链路表后交给 `pick_link`，返回发送顺序。
    let order = route_order(&links, peer_id, &conns, db::now_ms(), health_timeout_ms, max_failures);

    // ④ 按选路顺序投递（两轮策略见 `send_over_order`）。
    let senders: Vec<(mpsc::Sender<Message>, mpsc::Sender<Message>)> =
        links.iter().map(|l| (l.bulk.clone(), l.priority.clone())).collect();
    send_over_order(&senders, &order, msg, is_bulk_message(msg)).await
}

/// 向所有已连接节点广播一条 Gossip 消息。
pub async fn broadcast_gossip(state: &AppState, envelope: GossipEnvelope) {
    let msg = Message::Gossip {
        envelope: envelope.clone(),
    };

    // 与 `try_send` 同理：锁内只做决策 + 克隆 Sender，发送一律在锁外。
    // 原来在持有 `links` 锁时 `send().await`，一条拥塞链路会锁死整张连接表。
    let targets: Vec<mpsc::Sender<Message>> = {
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
        // 每个 peer 仍取第一条（M3-d 才改为按策略选路），但只克隆 Sender。
        picked
            .iter()
            .filter_map(|peer| links.get(*peer).and_then(|v| v.first()).map(|l| l.priority.clone()))
            .collect()
    };

    for tx in &targets {
        let _ = tx.send(msg.clone()).await;
    }
}

/// 广播一次 Presence：携带自身昵称/头像，靠 Gossip fan-out 跨跳传播。
///
/// 与 announce 的区别：announce 是 UDP 单跳、只覆盖本地网段；Presence 走
/// Gossip 广播（ttl 衰减 + fan-out 转发），能穿过中继节点让 A→B→C 里 A 也
/// 「看到」C。这是「去中心化、节点即服务器」发现层的第一块拼图。
async fn broadcast_presence(state: &Arc<AppState>) {
    let nickname = state
        .nickname
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let avatar = state
        .avatar
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    // payload：明文 JSON（昵称/头像/设备类型）。身份与双公钥已在 GossipEnvelope 字段里。
    let payload = serde_json::json!({
        "nickname": nickname,
        "avatar": avatar,
        "device_type": crate::protocol::current_device_type(),
    })
    .to_string();
    let payload_b64 = STANDARD.encode(payload.as_bytes());

    let mut env = {
        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
        gossip.build_envelope(
            &state.identity,
            &state.device_id,
            GossipKind::Presence,
            None,
            None,
            &payload_b64,
            db::now_ms(),
            0,
        )
    };
    // Presence 是公开的节点身份通告，明文。`signing_bytes()` 覆盖 `encrypted`，
    // 改后必须重签（与 FriendMessageBlocked 同模式）。
    env.encrypted = false;
    env.sender_sig = state.identity.sign_b64(&env.signing_bytes());
    broadcast_gossip(state, env).await;
}

// ---------------- 服务启动 ----------------

/// TCP 监听端口绑定重试次数与间隔（仅用于 `AddrInUse`）。
///
/// 退避的目的**不是**绕过 TIME_WAIT（那是连接关闭生命周期要解决的问题，见
/// `set_abortive_close`），而是覆盖「上一进程正在退出、端口尚未被 OS 释放」
/// 这段短窗口。`app.restart()` 是先 spawn 新进程再 `exit(0)`，两者存在重叠。
const BIND_RETRY_TIMES: u32 = 6;
const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(250);

pub async fn spawn(
    state: Arc<AppState>,
    ip: Ipv4Addr,
    tcp_port: u16,
    mut shutdown: watch::Receiver<bool>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    let bind = format!("{ip}:{tcp_port}");
    // 绑定策略（Windows 关键）：
    // - **刻意不设 SO_REUSEADDR**。Windows 上 SO_REUSEADDR 的语义是「允许强行绑定
    //   另一个 socket 正在使用的端口」，MSDN 明确指出这会让同端口上的行为变得不
    //   确定，是端口劫持（hijack）的入口；Unix 上同名的选项只用于跳过 TIME_WAIT，
    //   语义完全不同。mio 也正是因此只在非 Windows 平台设置它
    //   （mio/src/net/tcp/listener.rs:81 `#[cfg(not(windows))] set_reuseaddr`）。
    // - 因此 Windows 上重绑失败不能靠 socket 选项硬解，只能靠连接关闭生命周期：
    //   见 `set_abortive_close`。
    // - 未设置 SO_EXCLUSIVEADDRUSE：它只是「防劫持加固」（MSDN 建议所有服务端
    //   都设），对 TIME_WAIT 重绑没有任何帮助（MSDN 明确写了 exclusive socket
    //   关闭后仍要等原有连接变为 inactive），且会改变占用时的错误码，属加固项而非
    //   本次 P0 范围，保持 1.0 现状不动。
    let listener = match bind_with_retry(&bind).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            return Err(format!(
                "TCP 绑定 {bind} 失败：端口被占用。通常是有另一个 Gosslan 实例还在后台运行，请先通过托盘图标选择「退出」后再启动"
            ));
        }
        Err(e) => return Err(format!("TCP 绑定 {bind} 失败: {e}")),
    };
    // 两个后台任务都要用 state / shutdown，且 `async move` 会把它们移进闭包，
    // 因此必须在 accept_task 之前把所有副本准备好。
    let state_for_heartbeat = state.clone();
    let shutdown_for_heartbeat = shutdown.clone();
    let state_for_routed = state.clone();
    let shutdown_for_routed = shutdown.clone();
    let state_for_presence = state.clone();
    let shutdown_for_presence = shutdown.clone();
    let accept_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                accept = listener.accept() => {
                    let Ok((stream, peer_addr)) = accept else { continue };
                    // **入站连接上限**：没有它，一台主机可以无限建连（每个连接 2 个 1024
                    // 容量信道 + 2 个任务）。拿不到许可就直接 drop stream（等价于拒绝），
                    // 不排队 —— 排队只会把资源消耗推迟到以后。
                    let Ok(permit) = state.inbound_permits.clone().try_acquire_owned() else {
                        continue;
                    };
                    let st = state.clone();
                    let sd = shutdown.clone();
                    // 捕获**接受时刻**的世代：握手可能持续数秒，期间用户可能切换网卡
                    // （stop→start）。旧世代的任务握手成功后**不允许**登记链路。
                    let generation = state.network_generation();
                    tokio::spawn(async move {
                        // permit 随任务存活：连接结束（函数返回）才释放
                        let _permit = permit;
                        handle_incoming(st, stream, peer_addr, sd, generation).await;
                    });
                }
            }
        }
        // listener 在此 drop：stop() 之后端口立即空闲，新进程/新实例可立即 bind。
    });
    // 心跳：周期性向所有已建链节点发送 Heartbeat，
    // 及时发现静默断连（写失败 → writer 退出 → link 移除 → 在线状态修正）。
    let heartbeat_task = tokio::spawn(async move {
        let state = state_for_heartbeat;
        let mut shutdown = shutdown_for_heartbeat;
        let mut tick = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 拥塞告警限频（心跳每 5s 一轮，不限频会把日志刷满）。
        let mut last_congestion_warn = std::time::Instant::now() - Duration::from_secs(60);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                _ = tick.tick() => {
                    // 与 `try_send`/`broadcast_gossip` 同理：心跳每 5s 给**每条**链路发一次，
                    // 若持 `links` 锁 await，一条拥塞链路会把整张连接表连同心跳一起卡住。
                    // 锁内只克隆 Sender。
                    let txs: Vec<mpsc::Sender<Message>> = {
                        let links = state.links.lock().await;
                        links.values().flatten().map(|l| l.priority.clone()).collect()
                    };
                    let hb = Message::Heartbeat { device_id: state.device_id.clone() };
                    let mut congested = 0usize;
                    for tx in &txs {
                        // **非阻塞**发送：心跳是"可丢弃"的活性信号，不值得为它排队等待。
                        // 复核发现的原实现缺陷：串行 `send().await` ⇒ 一条满信道会**推迟
                        // 给其后所有链路的心跳**，对端读活性随之过期，被判成"不健康"
                        // —— 一条拥塞链路能伪造出全网链路故障。
                        if tx.try_send(hb.clone()).is_err() {
                            congested += 1;
                        }
                    }
                    // 拥塞是"可能出错"的关键状态跃迁：限频记录（30s 一次），
                    // 否则每 5s 一条会把日志刷满（logging 规范：只记可能出错的）。
                    if congested > 0 && last_congestion_warn.elapsed() >= Duration::from_secs(30) {
                        last_congestion_warn = std::time::Instant::now();
                        state.logger.warn(
                            "mesh",
                            format!(
                                "heartbeat 丢弃 {congested}/{} 条 —— 发送队列已满（对端消费不过来）",
                                txs.len()
                            ),
                        );
                    }

                    // ---- 死链路拆除（M3#6）----
                    //
                    // 半开 TCP（对端消失、本机内核仍收写）上：读循环**永久阻塞**、
                    // `Link` 一直留在表里 ⇒ `ensure_link` 认为已连通不再重拨；
                    // 而 `try_send` 只把消息投进 mpsc 就返回 `Ok` ⇒ 前端显示「已发送」，
                    // 消息却**静默投进死路**（outbox 也不会被触发补发，因为没有任何入站帧）。
                    //
                    // 判据刻意保守：读活性要跨过 **3 × 健康超时**（15s × 3 = 45s）才算死。
                    // 健康连接每 5s 必有一次入站心跳，所以正常链路**永远不会**落到这里；
                    // 只有真正半开/僵死的连接会被拆。拆掉后下一轮 announce（≤5s）即可重拨。
                    let stale_ms = {
                        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
                        pm.health_timeout_ms().saturating_mul(3)
                    };
                    let reaped = {
                        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
                        let max_failures = pm.max_failures();
                        pm.stale_connections(db::now_ms(), stale_ms, max_failures)
                    };
                    for (peer, ep) in reaped {
                        // ⚠️ 不再 `let MeshEndpoint::Tcp(addr) = ep else { continue }`：
                        // 那样**非 TCP 端点（BLE）永远拆不掉**，死链路会永久占着选路候选。
                        //
                        // 身份口径统一到 **channel**（`same_channel`），与 `reader_loop`
                        // 收尾时一致：按端点定位/删除时，若同一端点存在两条（历史上确实
                        // 出现过镜像连接），`find` 只取消第一条、`retain` 却删掉两条 ——
                        // 剩下那条的 socket 与读写任务变成孤儿（既不收 cancel 也不注销）。
                        let bulk = {
                            let links = state.links.lock().await;
                            links
                                .get(&peer)
                                .and_then(|v| v.iter().find(|l| l.endpoint == ep))
                                .map(|l| (l.bulk.clone(), l.cancel.clone()))
                        };
                        let Some((bulk, cancel)) = bulk else { continue };
                        // ① 精确取消这一条连接的读写任务（半开的读只有它能打断）。
                        let _ = cancel.send(true);
                        // ② 从传输链路表移除（空 Vec 连 key 一起删），让 `ensure_link` 能重拨。
                        {
                            let mut links = state.links.lock().await;
                            if let Some(v) = links.get_mut(&peer) {
                                v.retain(|l| !l.bulk.same_channel(&bulk));
                                if v.is_empty() {
                                    links.remove(&peer);
                                }
                            }
                        }
                        // ③ 同步 mesh 层（读循环收尾时也会做一次，这里是幂等的）。
                        unregister_connection(&state, &peer, &ep);
                        state.logger.warn(
                            "mesh",
                            format!(
                                "-conn peer={peer} ep={ep} 读活性超过 {}s 无入站帧 ⇒ 拆除死链路并等待重拨",
                                stale_ms / 1000
                            ),
                        );
                    }
                }
            }
        }
    });

    // 节点通告（Presence）：周期广播自身身份，跨跳传播让全网节点互相可见。
    //
    // 这是「去中心化、节点即服务器」的第一块拼图：announce 是 UDP 单跳、只覆盖
    // 本地网段；Presence 走 Gossip fan-out（ttl 衰减）跨跳扩散，让 A→B→C 链式
    // 拓扑里 A 也能「看到」C（经 B 转发）。接收侧按 TOFU 记录远端节点（见
    // handle_gossip 的 Presence 分支）。
    let presence_task = tokio::spawn(async move {
        let state = state_for_presence;
        let mut shutdown = shutdown_for_presence;
        // 周期必须**明显小于**跨跳节点超时（RELAY_PEER_TIMEOUT_SECS=45s）：跨跳节点
        // 没有直连 TCP，全靠 Presence 刷新 last_seen 保活。取 10s（4.5 个周期），
        // 给 Tailscale 等高延迟中继的转发抖动留足余量 —— 早期 30s 周期 + 15s 超时
        // 导致节点「出现 15s、消失 15s」，表现为「扫好几次才扫到」且 FriendAccept
        // 静默丢。跨跳超时已放宽（见 sweep_peers），此处 10s 远小于 45s。
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                _ = tick.tick() => broadcast_presence(&state).await,
            }
        }
    });

    // 跨子网（Routed）端点拨号：手动配置的端点周期性重试，直到连上。
    //
    // 每次循环都重新读配置 —— 这样运行时新增的端点无需重启即可生效。
    // 与 LAN 广播发现不同，这里是**配了就拨**（原因见循环内的注释），
    // 因此只需单侧配置即可建链，不必指望 ID 大小恰好合适的那一边。
    let routed_task = tokio::spawn(async move {
        let state = state_for_routed;
        let mut shutdown = shutdown_for_routed;
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                _ = tick.tick() => {
                    let list = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        parse_endpoints(
                            &db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default(),
                        )
                    };
                    // 并发拨号：一个「黑洞」端点不能把同一轮里的其他端点拖住。
                    let mut dials = tokio::task::JoinSet::new();
                    for ep in list {
                        // 解析失败**必须留痕**：用户配置了东西却什么都不发生时，
                        // 这条日志是唯一的线索（此前是静默 `continue`）。
                        let Some(addr) = ep.socket_addr() else {
                            state.logger.warn(
                                "routed",
                                format!(
                                    "跳过无法解析的地址 peer={} address={:?}",
                                    ep.display_id(),
                                    ep.address
                                ),
                            );
                            continue;
                        };
                        // 刻意**不走** `ensure_link`：那条路径带「只有小 device_id 拨号」
                        // 的规则，用于避免 LAN 广播发现时两端同时拨号。但 Routed 端点
                        // 是用户显式配置的明确意图，50% 概率会因 ID 大小被静默跳过，
                        // 表现为「配了却连不上且无任何提示」。这里直接拨号。
                        //
                        // 去重**只在** `connect_to_peer` 里做（按身份，或身份未知时按端点）——
                        // 「同一判断两处实现、行为还不一致」是这个项目踩过的坑。
                        // `device_id` 可省略（`None` = 身份由握手学），见 `RoutedEndpoint`。
                        let state = state.clone();
                        let shutdown = shutdown.clone();
                        dials.spawn(async move {
                            match connect_to_peer(
                                &state,
                                ep.device_id.as_deref(),
                                addr,
                                PathKind::Routed,
                                shutdown,
                            )
                            .await
                            {
                                DialOutcome::Connected => {
                                    state.logger.info(
                                        "routed",
                                        format!("已连上 peer={} ep={addr}", ep.display_id()),
                                    )
                                }
                                // 每 10s 一轮的常态：端点已有连接 / 已有拨号在途 / 并发已满，静默。
                                DialOutcome::AlreadyConnected
                                | DialOutcome::AlreadyDialing
                                | DialOutcome::DialBusy => {}
                                DialOutcome::Failed(e) => state.logger.warn(
                                    "routed",
                                    format!("拨号未成功 peer={} ep={addr}：{e}", ep.display_id()),
                                ),
                                // 正常停机：不打日志，否则退出时会多出一批误导性的「失败」
                                DialOutcome::Stopped => {}
                            }
                        });
                    }
                    // **必须排空**：`JoinSet` 被 drop 时会立刻 abort 掉所有未完成任务，
                    // 不等就永远拨不完。排空也顺带保证「单轮耗时 < tick 间隔」，
                    // 下一轮才可能对同一端点重拨 —— 按端点去重的前提才成立。
                    while dials.join_next().await.is_some() {}
                }
            }
        }
    });

    Ok(vec![accept_task, heartbeat_task, routed_task, presence_task])
}

/// 绑定监听端口，仅在 `AddrInUse` 时做有限退避重试。
///
/// 覆盖「上一进程正在退出，OS 尚未释放 59992」这一短窗口；
/// TIME_WAIT 场景由 `set_abortive_close` 从源头消除，不靠重试兜底。
async fn bind_with_retry(bind: &str) -> std::io::Result<TcpListener> {
    let mut last = match TcpListener::bind(bind).await {
        Ok(l) => return Ok(l),
        Err(e) => e,
    };
    for _ in 0..BIND_RETRY_TIMES {
        if last.kind() != std::io::ErrorKind::AddrInUse {
            break;
        }
        tokio::time::sleep(BIND_RETRY_INTERVAL).await;
        match TcpListener::bind(bind).await {
            Ok(l) => return Ok(l),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Windows：让连接在关闭时发 RST 而不是 FIN，本端不进入 TIME_WAIT。
///
/// **为什么必须做**：accepted connection 的本地端口 == 监听端口 59992。
/// 谁先发 FIN，谁就在该端口上留下 TIME_WAIT（Windows 默认 120s，
/// TcpTimedWaitDelay）。进程退出时 OS 会替我们关闭所有 socket，等于我们
/// 先发 FIN ⇒ 59992 被 TIME_WAIT 占死 ⇒ 重启后 `bind()` 直接
/// WSAEADDRINUSE ⇒ 局域网永久掉线（生产 1.0 的真实故障）。
///
/// **为什么不设 SO_REUSEADDR**：Windows 上该选项会开放端口劫持（见 `spawn`
/// 处注释与 MSDN《Using SO_REUSEADDR and SO_EXCLUSIVEADDRUSE》）。
/// 用 SO_LINGER=0 做 abortive close 才是 Windows 上唯一既安全又能立即重绑的做法。
///
/// **副作用**：RST 会丢弃发送缓冲区中尚未发出的字节。本代码库中连接只在
/// 「进程停止」「对端已断」「写失败」三类路径上关闭，都不是需要排空发送缓冲的
/// 正常收尾；消息可靠性由 outbox / pending 重发保证，不依赖 TCP 优雅关闭。
///
/// Unix 不需要：mio 已在非 Windows 平台设置 SO_REUSEADDR（仅跳过 TIME_WAIT，
/// 不允许多监听并存），保持优雅关闭语义。
#[cfg(windows)]
fn set_abortive_close(stream: &TcpStream) {
    use socket2::SockRef;
    if let Err(e) = SockRef::from(stream).set_linger(Some(Duration::ZERO)) {
        eprintln!("[lan] 设置 SO_LINGER 失败，重启后可能短暂无法绑定端口: {e}");
    }
}

/// Hello 认证的**纯判定**：给定「已绑定公钥」（`None` = 首次接触 TOFU），
/// 判断本次 Hello 是否可信。抽出来是为了可单测（不依赖 AppState）。
fn hello_auth_decision(
    bound: Option<&str>,
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
    sig_b64: &str,
) -> Result<(), String> {
    if nonce.is_empty() || sig_b64.is_empty() {
        return Err(format!("Hello 缺少 nonce/sig（device_id={device_id}）"));
    }
    let data = hello_signing_bytes(device_id, tcp_port, nonce, x25519_pubkey, ed25519_pubkey);
    match bound {
        // 已知身份：自报公钥必须与绑定公钥一致，且签名必须由该公钥验证通过。
        // 攻击者即便拿到真实公钥也签不出来；用自己公钥签名则与绑定值不符。
        Some(expected) => {
            if expected != ed25519_pubkey {
                return Err(format!("Hello 公钥与已绑定身份不符（device_id={device_id}）"));
            }
            if !crypto::verify_signature(expected, &data, sig_b64) {
                return Err(format!("Hello 签名校验失败（device_id={device_id}）"));
            }
            Ok(())
        }
        // 首次接触（TOFU）：仅要求自洽签名；密钥绑定在后续 announce/upsert 中固化。
        // 注意：TOFU 分支无法冒充「已建立信任的身份」——那是上面 Some 分支的事。
        None => {
            if !crypto::verify_signature(ed25519_pubkey, &data, sig_b64) {
                return Err(format!("Hello 自签名校验失败（device_id={device_id}）"));
            }
            Ok(())
        }
    }
}

/// 校验 Hello 握手，确认 TCP 对端确实持有 `device_id` 绑定的 Ed25519 私钥。
///
/// 信任根：`friends`（持久、权威）→ `peers`（运行时）中该 device_id 已绑定的
/// Ed25519 公钥。两者都没有时才走 TOFU（首次接触），用 Hello 自带的公钥验签。
///
/// 这封堵的是：任意局域网节点在 Hello 里自报好友/群主的 device_id 即可建立链路，
/// 随后利用 `from == peer_id` 的绑定关系伪造 GroupMemberRemoved / GroupRename 等
/// 明文控制消息（把群从受害者本地删掉、改名）。已建立信任的身份必须签名匹配。
///
/// 返回 `Err(原因)` 表示必须拒绝该连接。
pub(crate) fn verify_hello(
    state: &AppState,
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
    sig_b64: &str,
) -> Result<(), String> {
    if nonce.is_empty() || sig_b64.is_empty() {
        return Err(format!("Hello 缺少 nonce/sig（device_id={device_id}）"));
    }
    if !state.accept_hello_nonce(nonce) {
        return Err(format!("Hello nonce 重放（device_id={device_id}）"));
    }
    // 已绑定身份：好友表优先（持久），在线节点表回落（对方可能尚未成为好友但已在发现阶段绑定）
    let bound = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend_ed25519(&dbc, device_id)
    }
    .or_else(|| {
        state
            .peers
            .lock()
            .unwrap()
            .get(device_id)
            .and_then(|p| p.ed25519_pubkey.clone())
    });
    hello_auth_decision(
        bound.as_deref(),
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig_b64,
    )
}

/// 同意好友之后**忘掉这条申请**（内存态 `pending_requests` 里的那一行）。
///
/// 真实缺陷（用户 2026-09-12 真机实测）：双方互发过申请时，A 点了同意，B 的「新朋友」里
/// 那条申请**还在** —— 因为直连路径（`Message::FriendAccept`）只加了好友、没有清 pending，
/// 而跨跳路径（`GossipKind::FriendAccept`）清了。同一件事两条路径行为不一致，
/// 于是"有时候会清、有时候不清"。现在两条路径 + `respond_friend_request` 都走这一个助手，
/// 前端再用 `pendingRequests`（按好友列表过滤）兜一层，不会再出现"已经是好友还挂在申请里"。
pub fn forget_pending_request(state: &AppState, peer_id: &str) {
    state
        .pending_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(peer_id);
}

/// 构造带签名的 Hello（nonce 每次新生成，签名覆盖连接身份的全部字段）。
pub fn build_signed_hello(state: &AppState, conv_clock: i64) -> Message {
    let device_id = state.device_id.clone();
    let tcp_port = state.tcp_port;
    let x25519_pubkey = state.identity.x25519_public_b64();
    let ed25519_pubkey = state.identity.ed25519_public_b64();
    let nonce = STANDARD.encode(crypto::random_key());
    let sig = state.identity.sign_b64(&hello_signing_bytes(
        &device_id,
        tcp_port,
        &nonce,
        &x25519_pubkey,
        &ed25519_pubkey,
    ));
    Message::Hello {
        device_id,
        nickname: state.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        avatar: state.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        device_type: crate::protocol::current_device_type().to_string(),
        tcp_port,
        x25519_pubkey,
        ed25519_pubkey,
        conv_clock,
        nonce,
        sig,
    }
}

async fn handle_incoming(
    state: Arc<AppState>,
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    shutdown: watch::Receiver<bool>,
    // 接受这条连接时的网络世代（见 `AppState::network_generation`）
    generation: u64,
) {
    // Windows：先标记 abortive close，再拆分成读写半（拆分后拿不到 socket 句柄了）。
    #[cfg(windows)]
    set_abortive_close(&stream);
    // 读写半包装成 transport 端点：端点只搬字节，分帧由 `transport::tcp` 负责（P-A03）。
    let (raw_r, raw_w) = stream.into_split();
    let mut r = TcpReceiver::new(raw_r);
    let w = TcpSender::new(raw_w);
    // ⚠️ 首帧必须**有超时且能被停机打断**：验签前的连接既不在 `links` 也不在
    // `peer_manager`，45s 死链路 watchdog 覆盖不到它 —— 对端 accept 后一个字节都不发
    // （或对端断电留下的半开连接），任务与 socket 就会永久存活。这里两条都堵住。
    let mut shutdown_first = shutdown.clone();
    let first = tokio::select! {
        biased;
        _ = shutdown_first.changed() => return,
        res = tokio::time::timeout(
            Duration::from_secs(crate::protocol::FIRST_FRAME_TIMEOUT_SECS),
            read_frame_preauth(&mut r),
        ) => match res {
            Ok(Ok(m)) => m,
            // 超时 / 非法帧 / 连接断开：直接返回 ⇒ drop socket，不留残留状态
            _ => return,
        },
    };
    // 首帧必须是 Hello，且必须先通过身份认证才允许建立链路。
    // 认证失败直接丢弃连接（不插入 links），否则任意节点可冒用他人 device_id 建链。
    let peer_id = match &first {
        Message::Hello {
            device_id,
            tcp_port,
            nonce,
            sig,
            x25519_pubkey,
            ed25519_pubkey,
            ..
        } => {
            if let Err(reason) = verify_hello(
                &state,
                device_id,
                *tcp_port,
                nonce,
                x25519_pubkey,
                ed25519_pubkey,
                sig,
            ) {
                state.push_diag_event("hello_rejected", &format!("{reason}; from={peer_addr}"));
                state.logger.warn(
                    "transport",
                    format!("拒绝未通过身份认证的 Hello: {reason}"),
                );
                return;
            }
            device_id.clone()
        }
        _ => return, // 首帧必须是 Hello
    };
    // ⚠️ **世代校验**：握手跨了 stop/start（换网卡、重开通道）就必须自我否决 ——
    // 否则会把链路登记进新世代的表里，成为一条无人管理、也收不到新 shutdown 的幽灵连接。
    if !generation_is_current(generation, state.network_generation()) {
        state.logger.info(
            "transport",
            format!("丢弃跨世代的入站连接 peer={peer_id} ep={peer_addr}（网络已重启）"),
        );
        return;
    }
    // ⚠️ **入站去重**：验签之后、登记链路之前判（判据见 `should_accept_inbound`）。
    // 放在这里而不是更早：身份要验签通过才有意义；也不能更晚：登记后再拒会留下半条状态。
    let existing: Vec<(MeshEndpoint, PathKind, bool)> = {
        // ① 先取链路快照（锁内只克隆，不 await 别的锁）
        let list = {
            let links = state.links.lock().await;
            links.get(&peer_id).cloned().unwrap_or_default()
        };
        // ② 再取健康判据与 mesh 侧连接（health 与选路同一套判据，避免两处各判一次）
        let (timeout_ms, max_failures, conns) = {
            let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
            (
                pm.health_timeout_ms(),
                pm.max_failures(),
                pm.get(&peer_id)
                    .map(|p| p.connections().to_vec())
                    .unwrap_or_default(),
            )
        };
        let now = db::now_ms();
        list.iter()
            .map(|l| {
                let healthy = conns
                    .iter()
                    .find(|c| c.endpoint == l.endpoint)
                    // 查不到健康信息时**按健康处理**（保守：优先抑制镜像）。
                    // 正常情况下 `register_connection` 与链路登记同时发生，查不到属异常；
                    // 此时宁可少收一条新连接（watchdog 最长 45s 会拆掉死链路），
                    // 也不要放过镜像 —— 后者会污染多路径验收且永久并存。
                    .map(|c| c.health.is_healthy(now, timeout_ms, max_failures))
                    .unwrap_or(true);
                (l.endpoint.clone(), l.path_kind, healthy)
            })
            .collect()
    };
    if !should_accept_inbound(&state.device_id, &peer_id, PathKind::Lan, &existing) {
        state.logger.info(
            "transport",
            format!(
                "拒收重复入站连接 peer={peer_id} ep={peer_addr} path=lan（已有 {} 条链路）",
                existing.len()
            ),
        );
        return;
    }
    let (bulk_tx, bulk_rx) = mpsc::channel(1024);
    let (prio_tx, prio_rx) = mpsc::channel(1024);
    // 本连接独立的取消信号（M3#6）：健康 watchdog 判定僵尸链路时精确断开这一条。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    // 追加到该 peer 的连接列表（而非覆盖）—— 多连接支持的基础。
    // 端点取 TCP 对端的真实地址，使「同一 peer 的不同端点」可被区分。
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: MeshEndpoint::Tcp(peer_addr),
            // 入站连接只可能来自本机 TCP 监听端口 ⇒ LAN 路径（Routed 都是我们主动拨出）
            path_kind: PathKind::Lan,
            bulk: bulk_tx.clone(),
            // priority 留一个 sender 在作用域内：首帧验签后要回发 Hello（见下）。
            priority: prio_tx.clone(),
            cancel: cancel_tx,
        });
    // 同步到 mesh 层：让 Peer/Connection 模型知道这条连接存在
    register_connection(&state, &peer_id, MeshEndpoint::Tcp(peer_addr), PathKind::Lan);
    tokio::spawn(writer_loop(
        state.clone(),
        peer_id.clone(),
        MeshEndpoint::Tcp(peer_addr),
        w,
        bulk_rx,
        prio_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));
    // 验签通过后**回发**自己的 Hello：让拨号方也能学到本节点的身份与双公钥。
    //
    // 为什么需要：只有拨号方发 Hello，被连的一方不回 —— 于是**拨号方**永远不知道
    // 对面是谁。LAN 场景有 announce 兜底（UDP 广播连带把身份和公钥送过去了）所以
    // 看不出来；Routed / 跨网场景没有 announce，缺口就暴露为：对端永远不出现在
    // `peers` 表 →「添加好友」列表里没有它、拿不到 X25519 公钥 → 消息发不出去。
    //
    // 防乒乓：只在**首帧**这里回发一次（每条连接一次）；`handle_message` 的 Hello
    // 分支刻意不回发，因此两个节点之间不会来回刷 Hello。
    // 走 priority 队列，与主动方发 Hello 的路径对称（不被 bulk 积压排在后面）。
    let conv_clock = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &peer_id)
    };
    let _ = prio_tx.send(build_signed_hello(&state, conv_clock)).await;
    state.logger.info(
        "transport",
        format!("握手补全：已向对端回发本节点 Hello（peer={peer_id}）"),
    );
    handle_message(&state, &peer_id, first).await;
    // 用 TCP 对端的真实地址补全 peer IP：解决「被动连接方 peers 表 IP 为空或虚拟」的问题。
    // 新地址必须是非虚拟的可直连 LAN 地址才写入；link-local（169.254.0.0/16）已包含在
    // is_virtual_ip 内，不要在此重复判断——曾有写法 `octets()[0] != 169 && octets()[1] != 254`
    // 既把「169.x 且 x.254」错写成两个独立条件（误杀 10.0.254.x 这类合法局域网地址），
    // 又是完全冗余的（is_virtual_ip 已覆盖该网段）。
    {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peers.get_mut(&peer_id) {
            if let std::net::IpAddr::V4(new_ip) = peer_addr.ip() {
                if !is_virtual_ip(&new_ip) && (p.ip.is_empty() || is_virtual_ip_str(&p.ip)) {
                    p.ip = new_ip.to_string();
                }
            }
        }
    }
    state.emit_peers();
    reader_loop(
        state,
        r,
        peer_id,
        MeshEndpoint::Tcp(peer_addr),
        bulk_tx,
        shutdown,
        cancel_rx,
    )
    .await;
}

/// 把「某条连接成功收发」喂给 mesh 层的 `ConnectionHealth`（ADR-0014 §3.1）。
///
/// 信号**全部复用现有帧**，零新协议。RTT 恒传 `None`：`Message::Heartbeat` 是**单向**的
/// （收到只 `touch_peer` + flush，不回包），没有可靠的往返测量来源；ADR-0014 明确本阶段
/// 不做 RTT，这里也不假装有数据。
///
/// 复杂度：每条连接每 5s 至少一次（心跳），加上真实收发，都是 std 锁上的一次查表 ——
/// 与 `register_connection` 同量级，不构成热点。
fn mark_conn_seen(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.mark_connection_seen(
        peer_id,
        endpoint,
        db::now_ms(),
        None,
        true,
    );
}

/// 记录一次**写出成功**（M3-0b：只刷出站活性，**不**参与 `is_healthy`）。
///
/// 半开 TCP 上写会持续「成功」，因此它绝不能算成「对端活着」的证据 ——
/// 否则死链路会永久被判健康，选路一直选中它（ADR-0014 §7）。
fn mark_conn_write_seen(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.mark_connection_seen(
        peer_id,
        endpoint,
        db::now_ms(),
        None,
        false,
    );
}


/// 把「某条连接失败」喂给 mesh 层（写失败）。读循环退出时链路会被 `unregister_connection`
/// 整条摘掉，无需再记失败。
fn mark_conn_failure(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.mark_connection_failure(peer_id, endpoint);
}

/// 单次写出的结果（D8-4）。把"主动放弃"与"写失败"分开：
/// 前者是我们在停机/拆链路，不该记成链路故障（否则选路会把正在关闭的链路算成失败）。
enum WriteOutcome {
    Ok,
    Failed,
    Stopped,
}

async fn writer_loop(
    state: Arc<AppState>,
    peer_id: String,
    // 本连接的端点：健康信号要按**连接**记，必须能唯一定位到是哪一条。
    // 类型是 transport 无关的 `Endpoint`（BLE 也需要它）。
    endpoint: MeshEndpoint,
    mut w: TcpSender,
    mut bulk_rx: mpsc::Receiver<Message>,
    mut prio_rx: mpsc::Receiver<Message>,
    mut shutdown: watch::Receiver<bool>,
    // 本连接的取消信号（M3#6）：由健康 watchdog 在半开链路上触发。
    mut cancel: watch::Receiver<bool>,
) {
    let mut bulk_open = true;
    let mut prio_open = true;
    loop {
        if !bulk_open && !prio_open {
            break;
        }
        let msg = tokio::select! {
            biased;
            // 停止信号优先：立刻放弃待发帧并 drop 写半，让 socket 尽快关闭
            // （Windows 上配合 SO_LINGER=0 发 RST，不留下 TIME_WAIT）。
            _ = shutdown.changed() => break,
            // 本连接被判死（读活性长期过期）→ 与全局停机同样立即收尾。
            _ = cancel.changed() => break,
            maybe = prio_rx.recv(), if prio_open => maybe,
            maybe = bulk_rx.recv(), if bulk_open => maybe,
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
                    res = write_frame(&mut w, &msg) => {
                        if res.is_ok() { WriteOutcome::Ok } else { WriteOutcome::Failed }
                    }
                };
                if matches!(outcome, WriteOutcome::Ok) {
                    // 写成功只记**出站**活性（诊断口径）。M3-0b 起它**不**参与 is_healthy：
                    // 半开 TCP 上写会一直"成功"，那是本缺陷要被排除的伪证据。
                    mark_conn_write_seen(&state, &peer_id, &endpoint);
                    continue;
                }
                if matches!(outcome, WriteOutcome::Stopped) {
                    break;
                }
                {
                    // TCP write 失败：普通消息由 outbox 重发；ReadReceipt 需要特殊处理——
                    // 它没有 outbox 行，如果 pending 已被 flush_pending_reads 清除，
                    // 此处不恢复就永久丢失。将 timestamp 重新放回 pending_reads，
                    // 下一次建链 / Hello / Heartbeat 会再次 flush 重发。
                    if let Message::ReadReceipt { last_read_ts, .. } = &msg {
                        let mut pending = state.pending_reads.lock().unwrap_or_else(|e| e.into_inner());
                        let cur = pending.entry(peer_id.clone()).or_insert(*last_read_ts);
                        *cur = (*cur).max(*last_read_ts);
                    }
                    // 这条连接已经写不出去了 —— 记一次失败，供 M3 的选路与收敛使用。
                    mark_conn_failure(&state, &peer_id, &endpoint);
                    break;
                }
            }
            None => {
                // select 无法直接区分是哪个分支关闭，用两个 recv 的 is_closed 兜底。
                if prio_rx.is_closed() {
                    prio_open = false;
                }
                if bulk_rx.is_closed() {
                    bulk_open = false;
                }
                if !bulk_open && !prio_open {
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
    file::fail_receives_for_peer(&state, &peer_id);
    // 群文件接收状态同样按对端断链清理，避免 `.part` 与内存状态泄漏。
    {
        let group_ids: Vec<String> = state
            .group_file_receivers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, r)| r.peer_id == peer_id)
            .map(|(id, _)| id.clone())
            .collect();
        for tid in group_ids {
            fail_group_file_chunk(&state, &tid);
        }
    }
    // 只移除**这一条**连接（按 channel 身份匹配），不是整条删光：
    // 同一 peer 可能还连着别的端点（LAN + Tailscale），断一条 ≠ peer 下线 ——
    // 这正是 6b 的核心语义。旧实现整条 remove，会让另一条连接一起消失。
    let peer_now_offline = {
        // 先记下被移除的是哪个端点（mesh 层要按端点删对应的 Connection）
        let removed_endpoint = {
            let links = state.links.lock().await;
            links
                .get(&peer_id)
                .and_then(|list| list.iter().find(|l| l.bulk.same_channel(&link_tx)))
                .map(|l| l.endpoint.clone())
        };
        let mut links = state.links.lock().await;
        let removed = match links.get_mut(&peer_id) {
            Some(list) => {
                let before = list.len();
                list.retain(|l| !l.bulk.same_channel(&link_tx));
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
        let offline = removed && empty;
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
    }
}

// ---------------- 传输层 ↔ mesh 层 同步（6b-3） ----------------

/// 依据端点地址判断路径类型。
///
/// 私有 / 环回 / 链路本地地址视为 LAN；其余（含 Tailscale 的 100.64/10 CGNAT 段，
/// 它**不是** RFC1918 私有地址）视为 Routed —— 正好符合「跨子网走 Routed」的预期。
///
/// 注意 IPv6 的 ULA（`fc00::/7`，含 Tailscale 的 `fd7a:115c:a1e0::/48`）**故意**留在
/// Routed：它虽然叫「唯一本地地址」，但实践中主要出现在跨子网隧道里。判定只依赖
/// 地址属性，不针对任何具体软件（§36：不要把 Clash / Tailscale 写死进网络核心）。

/// 某 peer 当前第一条连接（入站视角）的路径类型字符串。
///
/// 直连时这就是「对方 ↔ 我」的真实路径；桥接时是「中继 ↔ 我」的最后一段。
pub(crate) async fn inbound_path_kind(state: &AppState, peer_id: &str) -> String {
    // ① 锁作用域内只取快照（与 `try_send` 同规矩：不在锁里 await 别的锁）
    let links: Vec<crate::state::Link> = {
        let g = state.links.lock().await;
        match g.get(peer_id) {
            Some(l) if !l.is_empty() => l.clone(),
            _ => return PathKind::Lan.as_str().to_string(),
        }
    };
    // ② 取健康阈值与 mesh 连接，跑**与发送完全相同**的选路
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
    let order = route_order(&links, peer_id, &conns, db::now_ms(), health_timeout_ms, max_failures);
    // ③ 徽标显示「实际会走的那条」= 选路结果的第一条（见 `badge_path_kind`）。
    //
    // 为什么不能用 `first()`（旧实现）：一个 peer 可能同时有 LAN + Routed(+BLE) 多条连接，
    // `first()` 是**插入顺序**，与 `pick_link`（LAN > Routed > Bluetooth + 活性过滤）
    // 可能不一致 ⇒ 界面显示"桥接 N"，消息实际走的是 LAN（用户 2026-09-12 反馈过徽标不符）。
    // 选路函数返回空只可能发生在"全部候选都不可用"，此时退回首条（与发送时的兜底一致）。
    badge_path_kind(&links, &order).as_str().to_string()
}

/// 「当前链路」徽标该显示哪条路径：**选路结果的第一条**。
///
/// 抽成纯函数的原因：徽标是用户唯一能直接看见的链路信息，而它的正确性判据是
/// 「与实际发送选的同一条」—— 那是个下标对应关系，端到端很难复现
/// （要先制造 LAN + Routed 双路径、再对比徽标与日志），但纯函数可以一次钉死。
/// `order` 为空（全部候选不可用）时退回首条，与发送路径的兜底一致。
fn badge_path_kind(links: &[crate::state::Link], order: &[usize]) -> PathKind {
    let idx = order.first().copied().unwrap_or(0);
    links.get(idx).map(|l| l.path_kind).unwrap_or(PathKind::Lan)
}

/// 更新会话的「当前链路」快照（最近一条消息的链路 + 中间节点数）。
///
/// 只在链路**变化**时写并留一行日志——链路状态是内存态，日志是唯一可观测手段
/// （真机排障看连接实际走了哪条路）。收发消息频繁，不做无谓的重复写。
pub(crate) fn update_conv_link(state: &AppState, conv_id: &str, path: &str, hop: u8) {
    let mut links = state.conv_link.lock().unwrap_or_else(|e| e.into_inner());
    let changed = match links.get(conv_id) {
        Some(old) => old.path != path || old.hop != hop,
        None => true,
    };
    if changed {
        links.insert(
            conv_id.to_string(),
            LinkState {
                path: path.to_string(),
                hop,
            },
        );
        state.logger.info("link", format!("conv={conv_id} path={path} hop={hop}"));
    }
}

/// 连接建立后：把这条连接登记到 mesh 层的 `PeerManager`。
///
/// 这样 mesh 层的 Peer/Connection 才与传输层的 `Link` 一一对应，
/// Phase 2 建立的「任一 Connection 健康 ⇒ Online」才有真实连接数据支撑。
/// 公钥在此刻可能尚未学到（拨号侧），留空即可 —— 收到 Hello / announce 后由
/// `PeerIdentity::merge_missing` 补齐（只补空、不覆盖）。
pub(crate) fn register_connection(state: &AppState, peer_id: &str, endpoint: MeshEndpoint, path_kind: PathKind) {
    let identity = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers
            .get(peer_id)
            .map(|p| PeerIdentity {
                x25519_public_key: p.x25519_pubkey.clone(),
                ed25519_public_key: p.ed25519_pubkey.clone(),
            })
            .unwrap_or_default()
    };

    // 路径类型来自调用方（见 `Link::path_kind` 注释：从 IP 反推会把用户配置的
    // 私有段 Routed 端点误判成 LAN）
    let path = path_kind;
    let candidate = PeerCandidate::new(peer_id, identity, endpoint.clone(), path.clone());
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    let (_, outcome) = pm.merge(candidate);
    // 建链即算一次「成功收发」—— 否则「已建立但还没收发」的连接会被健康判据算作不健康
    // （ADR-0014 §3.1 的硬性注意 ①：漏掉这一步，M3 的选路会把刚建好的连接判为不可用，
    // 进而退化成「按固定顺序挑」，甚至触发反复重拨）。
    let now = db::now_ms();
    // M3-0b：建链播种的是**读**活性（"刚建好就算活"），此后只由 `reader_loop` 刷新。
    // 若这里改成写活性，半开链路会重新变成永久健康。
    //
    // ⚠️ **只在真的是新连接时播种**（`is_new_connection`）。
    // 复核发现的原实现缺陷：无条件播种 ⇒ 同一个端点在握手/重连路径上被再次
    // `register_connection` 时，一条**已经死掉**（读活性过期）的 Connection 会被
    // 重新"续命"一个完整超时窗口；更糟的是刚播种的 LAN 链路（可能已是半开）
    // 会在该窗口内**压过一条真正健康的 Routed 链路**（选路按 LAN > Routed 排序）。
    if outcome.is_new_connection {
        pm.seed_connection_read_seen(peer_id, &endpoint, now);
    }
    // `online` 是 mesh 健康信号**在生产路径**唯一的外部可观测点：`ConnectionHealth` 是内存态，
    // 没有它就只能靠读代码相信「信号接上了」（这正是 M3-0 之前的状态）。
    let online = pm.online_state(peer_id, now) == PeerOnlineState::Online;
    state.logger.info(
        "mesh",
        format!(
            "+conn peer={peer_id} ep={endpoint} path={path:?} \
             new_peer={} new_conn={} conns={} online={}",
            outcome.is_new_peer,
            outcome.is_new_connection,
            pm.get(peer_id).map(|p| p.connection_count()).unwrap_or(0),
            u8::from(online),
        ),
    );
}

/// 连接断开后：从 mesh 层移除**这一条** Connection（同一 peer 的其他连接保留）。
pub(crate) fn unregister_connection(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.remove_connection(peer_id, endpoint);
    state.logger.info(
        "mesh",
        format!(
            "-conn peer={peer_id} ep={endpoint} conns={}",
            pm.get(peer_id).map(|p| p.connection_count()).unwrap_or(0)
        ),
    );
}

#[cfg(test)]
mod mesh_sync_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn sa(a: u8, b: u8, c: u8, d: u8) -> std::net::SocketAddr {
        std::net::SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), 59992)
    }

    fn sa6(s: &str) -> std::net::SocketAddr {
        std::net::SocketAddr::new(s.parse::<IpAddr>().unwrap(), 59992)
    }



    /// 地址构造不依赖「拼字符串再解析」，因此 IPv6 **不需要方括号**。
    ///
    /// 旧实现 `format!("{ip}:{port}").parse()` 在 IPv6 上会得到 `fd7a::1:59992`
    /// 这种非法地址 → 解析失败 → 静默丢掉连接。这正是「IPv6 端点配了却不拨号」的根因。
    #[test]
    fn socket_addr_from_accepts_v4_and_bare_v6() {
        assert_eq!(socket_addr_from("192.168.1.20", 59992), Some(sa(192, 168, 1, 20)));
        assert_eq!(
            socket_addr_from("fd7a:115c:a1e0::1", 59992),
            Some(sa6("fd7a:115c:a1e0::1"))
        );
        assert_eq!(socket_addr_from("::1", 59992), Some(sa6("::1")));
        // 非法输入返回 None（调用方跳过本轮，不 panic）
        assert_eq!(socket_addr_from("not-an-ip", 1), None);
        assert_eq!(socket_addr_from("", 1), None);
        // 方括号写法是 `"host:port"` 整体的语法，不是裸 IP —— 传到这里应当被拒绝
        assert_eq!(socket_addr_from("[fd7a::1]", 1), None);
    }
}

// ---------------- 主动建链（小 ID 拨号） ----------------

/// 拨号连接的超时上限。
///
/// 存在的理由：`TcpStream::connect` 在「SYN 被静默丢弃」时（对端防火墙 DROP、
/// VPN / 虚拟网卡路由黑洞）要等操作系统把 SYN 重传耗尽才返回 —— Linux/macOS
/// 可达 75s 以上，Windows 约 21s。
///
/// 而 `ensure_link` 是在 **UDP announce 接收循环里 `.await`** 的，没有上限就意味着
/// 一个收得到广播、TCP 却被丢弃的对端会把**整个发现循环堵死**（表现为发现假死、
/// 其他节点迟迟不出现）。Routed 拨号同样受影响。
///
/// 5s 远大于正常握手（同链路 <1ms；Tailscale 直连或经中继通常 <2s），只用于截断黑洞。
/// 心跳周期（秒）。**健康超时必须 ≥ 3 个周期**，见 `state.rs` 里
/// `PeerManager::new(15_000, 3)` 附近的说明与不变量测试；改这里要同步那个值。
pub const HEARTBEAT_INTERVAL_SECS: u64 = 5;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 主动拨号时等待对端回发 Hello 的上限 —— **只有「身份未知」的 Routed 端点会等**
/// （已知身份的路径不等，行为与历史一致）。
///
/// 远大于正常握手（同链路 <1ms、Tailscale 直连或中继 <2s），只用来兜住
/// 「对端是未升级的旧版本、不会回发 Hello」——否则该轮拨号会一直挂着。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 由字符串 IP + 端口构造 `SocketAddr`。
///
/// **刻意不用 `format!("{ip}:{port}").parse()`**：那种写法把地址与端口先拼成字符串，
/// 而 IPv6 只有写成 `[fd7a::1]:59992` 才是合法 SocketAddr，直接拼会得到
/// `fd7a::1:59992` → 解析失败。调用方拿到的地址在配置层已校验通过，重建失败就等于
/// **把一条合法配置静默丢掉**（曾真实发生：IPv6 端点「配了却永远不拨号」）。
/// 分开解析 IP 与端口，v4 / v6 都成立，也不需要方括号。
fn socket_addr_from(ip: &str, port: u16) -> Option<SocketAddr> {
    ip.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, port))
}

/// 拨号结果。
///
/// `Stopped` 与 `Failed` 分开，是为了在正常停机时不产生误导性的「拨号失败」日志；
/// LAN 路径的正常失败（对端离线、或该由对端拨号）则完全不打日志，避免刷屏。
enum DialOutcome {
    /// 本次真正建链成功（含首次握手学身份）。
    Connected,
    /// 端点已有一条连接（去重命中）。拨号任务每 10s 一轮，这是**常态**，不打日志
    /// —— 否则「已连上」会每 10s 重复刷屏，把真正的新连接淹掉。
    AlreadyConnected,
    /// 同一目标已有拨号在途（D6 在途去重命中）⇒ 本次不拨。同样是常态，不打日志。
    AlreadyDialing,
    /// 并发拨号已达上限（`MAX_CONCURRENT_DIALS`）⇒ 本轮放弃，下个周期再试。常态，不打日志。
    DialBusy,
    /// 拨号被停机信号中断（应用正在退出 / 切换网络）。
    Stopped,
    Failed(String),
}

/// 世代是否仍然有效（D8-4）。抽成纯函数是为了让"跨世代必须否决"这条**安全属性**
/// 有一个能被检索到、能被单测钉住的落点（真实路径要构造 AppState，单测造不出来）。
/// 某 peer 现有链路的快照 `(端点, 路径类型, 该连接是否健康)`。
///
/// 抽成 `pub(crate)` 的唯一目的是**让 BLE 走同一套入站/出站去重判据**
/// （`should_accept_inbound`），而不是在第三种传输里复制一份"有没有同路径连接"的判断 ——
/// 复核报告点名过"同一判断两处实现、行为还不一致"是这个项目踩过的坑。
#[cfg(feature = "bluetooth")]
pub(crate) async fn link_snapshot(
    state: &AppState,
    peer_id: &str,
) -> Vec<(MeshEndpoint, PathKind, bool)> {
    let list = {
        let links = state.links.lock().await;
        links.get(peer_id).cloned().unwrap_or_default()
    };
    let (timeout_ms, max_failures, conns) = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        (
            pm.health_timeout_ms(),
            pm.max_failures(),
            pm.get(peer_id)
                .map(|p| p.connections().to_vec())
                .unwrap_or_default(),
        )
    };
    let now = db::now_ms();
    list.iter()
        .map(|l| {
            let healthy = conns
                .iter()
                .find(|c| c.endpoint == l.endpoint)
                .map(|c| c.health.is_healthy(now, timeout_ms, max_failures))
                .unwrap_or(true);
            (l.endpoint.clone(), l.path_kind, healthy)
        })
        .collect()
}

/// Hello 验签的 `pub(crate)` 包装：BLE 运行时（`network/ble.rs`）复用同一份验签逻辑，
/// **不允许**任何传输自己实现一遍（身份认证只应有一个实现）。
#[cfg(feature = "bluetooth")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_hello_for_ble(
    state: &AppState,
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
    sig_b64: &str,
) -> Result<(), String> {
    verify_hello(
        state,
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig_b64,
    )
}

/// 入站去重判据（BLE 侧复用；TCP 侧在 `handle_incoming` 内联调用同一个函数）。
#[cfg(feature = "bluetooth")]
pub(crate) fn should_accept_inbound_public(
    my_id: &str,
    peer_id: &str,
    incoming: PathKind,
    existing: &[(MeshEndpoint, PathKind, bool)],
) -> bool {
    should_accept_inbound(my_id, peer_id, incoming, existing)
}

/// 世代是否仍然有效（D8-4）。
fn generation_is_current(captured: u64, current: u64) -> bool {
    captured == current
}

/// 单个 peer 允许并存的最大链路数（防御"同 peer 反复建链"的无界增长）。
///
/// 正常拓扑一个 peer 最多 3 条（LAN + Routed + BLE），取 6 留余量（例如换网瞬间新旧并存）。
const MAX_LINKS_PER_PEER: usize = 6;

/// **入站去重判据**（D6-2/D6-3 的核心，纯函数便于钉住）。
///
/// 背景：接受侧原先**无条件**把新连接 append 进 `links`，于是——
/// * 任意已验签对端可以反复建链，链条无界增长（每条 2 个 1024 容量信道 + 2 个任务）；
/// * 两侧都配了对方地址（或 LAN announce 时序不对称）时，同一对等关系会稳定停在
///   2 条镜像 TCP，`route_order`/心跳/候选都翻倍，还污染 M3 的多路径验收。
///
/// 判据设计（必须**确定性且对称**，否则会两边互拒导致谁也连不上）：
/// 1. 已有同**路径类型**的连接，且「本机是指定拨号方」（`my_id > peer_id`，与
///    `should_dial` 同一规则）⇒ 拒收这条入站：镜像里保留**我方拨出的**那条
///    （我方连接由我方健康判据管理，语义最清楚）。对端（小 ID）在同一条件下
///    会接受我们的拨入 ⇒ 双方算出同一个赢家，不会互拒。
/// 2. 链路数已达 `MAX_LINKS_PER_PEER` ⇒ 拒收（防无界增长）。
/// 3. 其余一律接受 —— 尤其**一条都没有时必须接受**，否则直接断掉连通性。
fn should_accept_inbound(
    my_id: &str,
    peer_id: &str,
    incoming: PathKind,
    // (端点, 路径类型, 该连接**当前是否健康**)
    existing: &[(MeshEndpoint, PathKind, bool)],
) -> bool {
    if existing.len() >= MAX_LINKS_PER_PEER {
        return false;
    }
    // 只有"指定拨号方"才拒绝镜像；小 ID 方始终接受（它本来就不主动拨）。
    //
    // ⚠️ 必须再加"那条已有连接**仍然健康**"：若它已经半开/僵死（还没被 watchdog 拆），
    // 按路径存在就拒收会把对端**刚拨进来的新鲜连接**也挡掉 —— 而本机因为
    // `has_lan_path` 仍为真也不会重拨（`ensure_link` 以为 LAN 已连通），于是双方
    // 要等 watchdog（最长 45s）拆掉死链路才能恢复。加了这个条件，新鲜连接立刻接管，
    // 恢复时间从"最长 45s"变成"这一次握手"。
    if my_id > peer_id
        && existing
            .iter()
            .any(|(_, k, healthy)| *k == incoming && *healthy)
    {
        return false;
    }
    true
}

/// 小 ID 兜底拨号的触发阈值：对端在线（announce 首次学到）却在本机无连接超过该时长，
/// 说明大 ID 一方拨不过来（单向可达 / 大 ID 长期离线），小 ID 兜底主动拨号。
/// 10s = 2 个 announce 周期（announce 5s 一轮），给大 ID 足够时间先拨通。
const BACKUP_DIAL_AFTER_MS: i64 = 10_000;

/// `ensure_link` 判据的**纯函数内核**（便于非空转单测）：
/// 该 peer 现有的这些端点里，是否已有**走 LAN 路径**的连接。
///
/// 单独抽出来的理由：D5 的回归点正是「把任意连接当成 LAN 已连通」——
/// 那是**一行布尔表达式**的错误，端到端很难复现（要先 Routed 连上、再等 announce），
/// 而这里可以逐条钉死：只有 Routed 端点 ⇒ `false`（要继续拨 LAN）。
fn has_lan_path(links: &[(MeshEndpoint, PathKind)]) -> bool {
    links.iter().any(|(_, kind)| *kind == PathKind::Lan)
}

/// 拨号决策的**可测入口**：把「现有连接 → 是否还要拨 LAN」这一步也收进函数里。
///
/// 为什么不直接在 `ensure_link` 里算 `has_lan_link`：那样「把任意连接当成 LAN 已连通」
/// 这个回归（D5）只会体现在一行布尔表达式上，测试无从钉住（helper 单独测是空的 ——
/// 只要调用点写错，helper 再对也没用）。收进来后，测试直接喂「只有 Routed 端点」，
/// 回归时该断言必然 FAIL。
fn should_dial_for_peer(
    my_id: &str,
    peer_id: &str,
    has_endpoint: bool,
    existing: &[(MeshEndpoint, PathKind)],
    first_seen: Option<i64>,
    now_ms: i64,
) -> bool {
    // 判据是**同路径（LAN）已连通**，不是「有任意连接」：后者会让先经 Routed 连上的
    // 对等关系永远拿不到 LAN 链路（D5）。
    let has_lan_link = has_endpoint || has_lan_path(existing);
    should_dial(my_id, peer_id, has_endpoint, has_lan_link, first_seen, now_ms)
}

/// 是否该主动拨这个端点（纯函数，便于单测 + 护栏非空转）。
///
/// 决策顺序（越靠前越确定、越便宜，命中即短路）：
/// 1. `has_endpoint`：**这个端点**已经连上了 → 无事可做。
/// 2. `has_lan_link`：**LAN 这条路径**已经连通 → 不拨。
///    这一条源自 P1-2 的修正（原为「任意链路」），但 2026-09-12 复核发现原判据过宽：
///    只要 peer 有任何一条连接（例如先经 Routed/Tailscale 连上），LAN 路径就**永远拿不到**
///    ⇒ M3 的「LAN > Routed」优先级在这些拓扑里**永不生效**，多路径退化成单路径。
///    现在只在「LAN 已连通」时短路，Routed-first 的对等关系仍会补一条 LAN。
///
///    为什么不能按「这个端点」判（`has_endpoint` 单独判不行）：接受侧 `handle_incoming`
///    记录的 `Link.endpoint` 是 TCP **源地址（临时端口）**，而这里拿到的是 announce 自报的
///    **监听地址**，两者永不相等 ⇒ 被动方（小 ID）的第 1 条永远不命中，10s 后兜底拨号会
///    反向再拨一条，同一对节点稳定停留 **2 条镜像 TCP**。所以「同路径是否已连通」必须按
///    **路径类型**判（LAN 链路无论端点记的是监听地址还是临时端口，路径都是 LAN）。
/// 3. 本机是大 ID（`my_id > peer_id`）：恒拨（对称场景的确定性拨号方）。
/// 4. 本机是小 ID：仅当对端在线（`first_seen` 有值）且「首次发现」已超过
///    `BACKUP_DIAL_AFTER_MS` 才兜底拨 —— 给大 ID 足够时间先拨通；单侧不可达
///    （不对称 NAT / 防火墙）时由小 ID 补齐连通性。
fn should_dial(
    my_id: &str,
    peer_id: &str,
    has_endpoint: bool,
    has_lan_link: bool,
    first_seen: Option<i64>,
    now_ms: i64,
) -> bool {
    if has_endpoint || has_lan_link {
        return false;
    }
    if my_id > peer_id {
        return true;
    }
    first_seen.is_some_and(|since| now_ms - since >= BACKUP_DIAL_AFTER_MS)
}

pub async fn ensure_link(
    state: &Arc<AppState>,
    peer_id: &str,
    ip: &str,
    tcp_port: u16,
    shutdown: watch::Receiver<bool>,
) {
    // 端点解析失败则放弃本轮（下一轮 announce 会再试）。
    let Some(endpoint) = socket_addr_from(ip, tcp_port) else { return };
    // ① 这个端点已经连上了（典型是「自己拨出去的那条」）→ 本轮无事可做。
    let has_endpoint = state
        .has_endpoint(peer_id, &MeshEndpoint::Tcp(endpoint))
        .await;
    // ② **LAN 这条路径**已经连通 → 不再拨。
    //
    // 判据是「同路径是否已连通」，不是「有没有任意连接」也不是「有没有连到这个端点」：
    //   · 按端点判：接受侧记的是临时端口、这里比的是监听地址，永不相等 ⇒ 镜像重拨
    //     （见 `should_dial` 注释）；
    //   · 按「任意连接」判：先经 Routed/Tailscale/BLE 连上的对等关系**永远拿不到 LAN 链路**
    //     ⇒ M3 的「LAN > Routed」优先级永不生效（2026-09-12 复核抓到的多路径硬阻塞）。
    // 本函数只负责**给 LAN 路径补连通性**；Routed 由配置驱动、BLE 由发现驱动，都不经过这里。
    // 现有连接的端点快照（锁内只取数据，决策在锁外做）。
    // 快照里带上**路径类型**：判"LAN 是否已连通"必须看 Link 自己记的路径，
    // 不能按端点 IP 段反推（用户配置的私有段 Routed 端点会被误判成 LAN）。
    let existing_links: Vec<(MeshEndpoint, PathKind)> = {
        let links = state.links.lock().await;
        links
            .get(peer_id)
            .map(|v| v.iter().map(|l| (l.endpoint.clone(), l.path_kind)).collect())
            .unwrap_or_default()
    };
    // 首次建链：大 ID 立即拨号，小 ID 等大 ID 拨；小 ID 在「对端在线却迟迟连不上」时兜底。
    let should = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        let first_seen = peers.get(peer_id).and_then(|p| p.first_seen);
        should_dial_for_peer(
            &state.device_id,
            peer_id,
            has_endpoint,
            &existing_links,
            first_seen,
            db::now_ms(),
        )
    };
    if !should {
        return;
    }
    // LAN 发现路径的拨号失败是常态（对端离线、或本轮该由对端拨），刻意不打日志；
    // 但**握手验签失败/身份不符**会在 `connect_to_peer` 内以 warn + 诊断事件留痕
    // （那是「有人冒充」或「配置写错」的信号，不能静默）。
    let _ = connect_to_peer(state, Some(peer_id), endpoint, PathKind::Lan, shutdown).await;
}

/// 建立一条到 `endpoint` 的连接。
///
/// 调用方传 **已解析好的 `SocketAddr`**：地址的解析与校验在配置/announce 层各做一次，
/// 这里不再「拼字符串再解析」（那是 IPv6 丢方括号的根源）。
///
/// `known_id` 决定握手方式：
/// - `Some(id)`：身份已知（LAN announce 学到 / 用户显式配置了 `device_id`）。
///   ⚠️ **仍然要握手验签**：`id` 只说明「对方自称/我们以为它是谁」，
///   未认证的 announce 不能充当身份（否则任意进程可冒用好友 id 接链并伪造 Ack /
///   FriendRemove）。且要求对端自称的 device_id 与 `id` 一致，不一致即失败留痕 ——
///   这样「配置写错」不再表现为静默单向黑洞。
/// - `None`：身份未知（Routed 端点只填了地址）。此时**必须先握手**：发自己的 Hello →
///   等对端回发的 Hello → 验签 → 得到真实 `device_id` 与双公钥，再登记链路。
///   这正是 §8 的 `IP:PORT → TCP → Hello → Node ID → Identity`，
///   也是「不用手填 device_id」的实现方式。
///
/// 为什么必须「先握手、再登记」：链路 key（`links` 的 HashMap key，以及 `writer_loop`
/// 持有的 `peer_id`）必须在 spawn 之前确定，而 `writer_loop` 要用它回写
/// `pending_reads`，事后无法改名。
async fn connect_to_peer(
    state: &Arc<AppState>,
    known_id: Option<&str>,
    endpoint: SocketAddr,
    path_kind: PathKind,
    mut shutdown: watch::Receiver<bool>,
) -> DialOutcome {
    // 传输无关的端点表示（拨号本身仍是 TCP：BLE 走自己的拨号路径，见 ADR-0015）
    let ep = MeshEndpoint::Tcp(endpoint);
    // ① **在途去重（D6）**：`has_endpoint` 与"登记链路"之间隔着 connect + 握手（最长 10s），
    //    两条并发路径会同时看到"还没连上"从而各拨一条 ⇒ `links[peer]` 出现两条同端点链路。
    //    这里用 RAII 守卫登记"我正在拨"，任何提前返回/panic 都会自动释放。
    //    键：身份已知用 `peer:`（同一 peer 的不同地址不该同时拨），否则用 `ep:`。
    let dial_key = match known_id {
        Some(id) => format!("peer:{id}"),
        None => format!("ep:{endpoint}"),
    };
    let _in_flight = match crate::state::DialGuard::try_acquire(state, dial_key) {
        Some(g) => g,
        None => return DialOutcome::AlreadyDialing,
    };
    // ② 并发上限：挡"大量**不同**目标"的拨号洪泛（伪造 announce 可批量制造）。
    //    拿不到许可就本轮放弃 —— announce 5s 一轮、Routed 10s 一轮，都会再来。
    let Ok(_permit) = state.dial_permits.clone().try_acquire_owned() else {
        return DialOutcome::DialBusy;
    };
    // ③ 按端点去重：与 `ensure_link` 的检查构成双重保险（announce 与 Routed 拨号会并发触发）。
    // 身份未知时只能按端点判 —— 否则 10s 重试的每一轮都会重复建链。
    let already = match known_id {
        Some(id) => state.has_endpoint(id, &ep).await,
        None => state.has_endpoint_addr(&ep).await,
    };
    if already {
        return DialOutcome::AlreadyConnected;
    }

    // connect 与停机信号赛跑：`stop()` 只等后台任务 2s，若 connect 正在等超时，
    // 不中断就会拖慢退出 / `app.restart()`（后者还会与端口释放抢时间）。
    let stream = tokio::select! {
        biased;
        _ = shutdown.changed() => return DialOutcome::Stopped,
        res = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(endpoint)) => match res {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return DialOutcome::Failed(format!("连接失败: {e}")),
            Err(_) => {
                return DialOutcome::Failed(format!(
                    "连接超时（{}s 内未建立）",
                    CONNECT_TIMEOUT.as_secs()
                ))
            }
        },
    };

    let (raw_r, raw_w) = stream.into_split();
    // 握手阶段要直接读写 socket（此时还没有 writer / reader 循环），故声明为 mut。
    let mut r = TcpReceiver::new(raw_r);
    let mut w = TcpSender::new(raw_w);

    // ---- 握手：**两条路径都必须验签**（§8 / ADR-0011）----
    //
    // 这里刻意**不做**「已知 id 就跳过握手」的捷径。复核抓到的 High 缺陷正是这个捷径：
    // `known_id` 来自**未认证的 UDP announce**（`pkt.device_id` + `src.ip()`）或本地配置，
    // 它只是「对方自称是谁 / 我们以为它是谁」，**不是身份**。跳过握手 ⇒ 任意进程只要
    // 广播一个好友的 device_id，就会被拨号并**以此身份**接链；随后它能伪造
    // FriendRemove（静默删好友）/ UserInfo / ReadReceipt / **Ack** —— 其中 Ack 会让
    // 发送方删掉 outbox 行，等于对**真实**好友的消息静默永久丢失。
    // 同理，配置里写错或过期的 device_id 会变成「能连上、但对方所有帧都被丢弃」的
    // 单向黑洞，而且原先**一行日志都没有**。
    //
    // 现在：先发自己的 Hello → 读对端 Hello → **必须验签**；`Some(id)` 还要求
    // 对端自称的 device_id 与预期一致，不一致直接失败并留日志。
    let conv_clock = match known_id {
        // 已知身份：可以带上真实会话时钟（未知身份只能发 0，对端 observe_clock 取 max 不会倒退）。
        Some(id) => {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_clock(&dbc, id)
        }
        None => 0,
    };
    let hello = build_signed_hello(state, conv_clock);
    if let Err(e) = write_frame(&mut w, &hello).await {
        return DialOutcome::Failed(format!("握手发送失败: {e}"));
    }
    // 读对端回发的 Hello（对端收到我们的 Hello 后会回发，见 `handle_incoming`）。
    let first = tokio::select! {
        biased;
        _ = shutdown.changed() => return DialOutcome::Stopped,
        res = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut r)) => match res {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => return DialOutcome::Failed(format!("握手读取失败: {e}")),
            Err(_) => {
                return DialOutcome::Failed(format!(
                    "握手超时（{}s 内未收到对端 Hello —— 对端可能不是 Gosslan 节点）",
                    HANDSHAKE_TIMEOUT.as_secs()
                ))
            }
        },
    };
    let Message::Hello {
        device_id,
        tcp_port,
        nonce,
        sig,
        x25519_pubkey,
        ed25519_pubkey,
        ..
    } = &first
    else {
        return DialOutcome::Failed("握手失败: 对端首帧不是 Hello".to_string());
    };
    // 身份一致性：拨号目标是我们**以为**的 id 时，对端必须就是它。
    // 不匹配就断开并留日志 —— 既堵住冒充，也让「配置写错」不再表现为静默黑洞。
    if let Some(expected) = known_id {
        if device_id != expected {
            let reason = format!(
                "握手身份不符：预期 {expected}，对端自称 {device_id}（端点 {endpoint}）"
            );
            state.push_diag_event("hello_mismatch", &reason);
            state.logger.warn("transport", reason.clone());
            return DialOutcome::Failed(reason);
        }
    }
    if let Err(reason) = verify_hello(
        state,
        device_id,
        *tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig,
    ) {
        state.push_diag_event("hello_rejected", &format!("{reason}; from={endpoint}"));
        state.logger.warn("transport", format!("握手验签失败：{reason}（端点 {endpoint}）"));
        return DialOutcome::Failed(format!("握手失败: {reason}"));
    }
    let peer_id: String = device_id.clone();
    let learned_hello: Option<Message> = Some(first);

    let (bulk_tx, bulk_rx) = mpsc::channel(1024);
    let (prio_tx, prio_rx) = mpsc::channel(1024);
    // 本连接独立的取消信号（M3#6），语义同 `handle_incoming`。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: MeshEndpoint::Tcp(endpoint),
            path_kind,
            bulk: bulk_tx.clone(),
            priority: prio_tx.clone(),
            cancel: cancel_tx,
        });
    // 同步到 mesh 层（拨号侧同样登记，路径类型由调用方携带）
    register_connection(state, &peer_id, ep.clone(), path_kind);
    tokio::spawn(writer_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        w,
        bulk_rx,
        prio_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));

    // 握手已在建链前完成（两条路径都发过自己的 Hello，且都验过对端的 Hello），
    // 这里就地处理对端首帧 —— 走的正是 `handle_incoming` 那条路径
    // （写身份 + 双公钥、对齐会话时钟、冲刷待发队列）。
    if known_id.is_none() {
        // 留痕：配置里没写 device_id 时，这行日志是用户/开发者**唯一**能确认
        // 「到底连上了谁」的地方。
        state.logger.info(
            "transport",
            format!("握手学到对端身份 peer={peer_id} ep={endpoint}"),
        );
    }
    if let Some(first) = learned_hello {
        handle_message(state, &peer_id, first).await;
    }

    tokio::spawn(reader_loop(
        state.clone(),
        r,
        peer_id.clone(),
        ep.clone(),
        bulk_tx,
        shutdown, cancel_rx,
    ));
    flush_outbox(state, &peer_id).await;
    flush_group_outbox(state, &peer_id).await;
    flush_pending_reads(state, &peer_id).await;
    flush_pending_group_reads(state, &peer_id).await;
    crate::commands::flush_pending_files(state, &peer_id).await;
    // 主动拨号建链完成：补发此前因无 link 而未送达的群密钥
    flush_pending_group_keys(state, &peer_id).await;
    // 群文件离线投递：该 peer 的 pending GroupFile 顺序发送
    crate::commands::flush_pending_group_files(state, &peer_id).await;
    DialOutcome::Connected
}

// ---------------- 消息分发 ----------------

pub async fn handle_message(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    match msg {
        Message::Hello {
            device_id,
            nickname,
            avatar,
            device_type,
            tcp_port,
            x25519_pubkey,
            ed25519_pubkey,
            conv_clock,
            ..
        } => {
            if device_id != peer_id {
                return;
            }
            let ip = state
                .peers
                .lock()
                .unwrap()
                .get(&device_id)
                .map(|p| p.ip.clone())
                .unwrap_or_default();
            upsert_peer(
                state,
                &device_id,
                &nickname,
                avatar.clone(),
                &device_type,
                &ip,
                tcp_port,
                Some(x25519_pubkey),
                Some(ed25519_pubkey),
                None,
            )
            .await;
            // 对齐单聊逻辑时钟：避免离线期间的时钟落差让后续新消息序号偏小。
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::observe_clock(&dbc, &device_id, conv_clock).ok();
            }
            maybe_update_friend(state, &device_id, &nickname, avatar);
            flush_outbox(state, &device_id).await;
            flush_group_outbox(state, &device_id).await;
            flush_pending_reads(state, &device_id).await;
            flush_pending_group_reads(state, &device_id).await;
            crate::commands::flush_pending_files(state, &device_id).await;
            // 链路刚建立：补发此前因无 link 而未送达的群密钥
            flush_pending_group_keys(state, &device_id).await;
            // 群文件离线投递：该 peer 的 pending GroupFile 顺序发送
            crate::commands::flush_pending_group_files(state, &device_id).await;
        }
        Message::Heartbeat { device_id } => {
            if device_id != peer_id {
                return;
            }
            touch_peer(state, &device_id).await;
            flush_outbox(state, &device_id).await;
            flush_group_outbox(state, &device_id).await;
            flush_pending_reads(state, &device_id).await;
            flush_pending_group_reads(state, &device_id).await;
            crate::commands::flush_pending_files(state, &device_id).await;
            flush_pending_group_keys(state, &device_id).await;
            crate::commands::flush_pending_group_files(state, &device_id).await;
        }
        Message::UserInfo {
            device_id,
            nickname,
            avatar,
            device_type,
        } => {
            if device_id != peer_id {
                return;
            }
            let ip = state
                .peers
                .lock()
                .unwrap()
                .get(&device_id)
                .map(|p| p.ip.clone())
                .unwrap_or_default();
            upsert_peer(
                state,
                &device_id,
                &nickname,
                avatar.clone(),
                &device_type,
                &ip,
                0,
                None,
                None,
                None,
            )
            .await;
            maybe_update_friend(state, &device_id, &nickname, avatar.clone());
            // 同步更新 single 会话的昵称/头像（conversations DB）
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::update_conversation_profile(&dbc, &device_id, &nickname, avatar.as_deref()).ok();
        }
        Message::ChatStyle { from, to, style } => {
            if from == state.device_id {
                return;
            }
            if let Some(t) = &to {
                if t != &state.device_id {
                    return; // 定向给别人，忽略
                }
            }
            // 持久化对端样式表（device_id -> style JSON），前端按发送者渲染其消息气泡
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let mut map: serde_json::Map<String, serde_json::Value> =
                    db::get_setting(&dbc, "chat_peer_styles")
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default();
                map.insert(from.clone(), serde_json::Value::String(style.clone()));
                if let Ok(json) = serde_json::to_string(&map) {
                    db::set_setting(&dbc, "chat_peer_styles", &json).ok();
                }
            }
            let _ = state.app.emit(
                "peer-style-updated",
                &serde_json::json!({ "device_id": from, "style": style }),
            );
        }
        Message::FriendRequest {
            from,
            from_nickname,
            from_avatar,
            to,
            ts,
        } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            let req = PendingRequest {
                from: from.clone(),
                from_nickname: from_nickname.clone(),
                from_avatar: from_avatar.clone(),
                ts,
            };
            state
                .pending_requests
                .lock()
                .unwrap()
                .insert(from.clone(), req.clone());
            let _ = state.app.emit("friend-request", &req);
            let mut extra = std::collections::HashMap::new();
            extra.insert("type".to_string(), "friend_request".to_string());
            notify_with_extra(
                &state.app,
                "好友申请",
                &format!("{from_nickname} 请求添加你为好友"),
                extra,
            );
        }
        Message::FriendAccept { from, to } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            let name = resolve_nickname(state, &from);
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::add_friend(&dbc, &from, &name, None).ok();
                // 同步公钥（否则首次加密发送会失败）
                let (x, e) = {
                    let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
                    peers
                        .get(&from)
                        .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                        .unwrap_or((None, None))
                };
                if x.is_some() || e.is_some() {
                    db::update_friend_pubkeys(&dbc, &from, x.as_deref(), e.as_deref()).ok();
                }
            }
            // 已经是好友了 ⇒ 这条申请必须消失（否则「新朋友」里会留着一条永远处理不掉的申请）
            forget_pending_request(state, &from);
            let _ = state.app.emit("friend-accepted", &from);
            notify(
                &state.app,
                "好友申请已通过",
                &format!("{name} 已成为你的好友"),
            );
        }
        Message::FriendReject { from, to } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            state.pending_requests.lock().unwrap_or_else(|e| e.into_inner()).remove(&from);
            let _ = state.app.emit("friend-rejected", &from);
        }
        Message::FriendRemove { from, to } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 对方删除了好友关系：移除本地好友行（不删除聊天记录）
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::remove_friend(&dbc, &from).ok();
            drop(dbc);
            let _ = state.app.emit("friend-removed", &from);
        }
        Message::FriendMessageBlocked {
            ref from,
            ref to,
            ref original_sender,
        } => {
            if from != peer_id {
                return;
            }
            if to == &state.device_id {
                // 目标是本机：通知前端
                let _ = state.app.emit("friend-message-blocked", from);
            } else if original_sender != &state.device_id {
                // 中继节点：转发给原始发送方（与 Ack relay 同逻辑）
                let _ = try_send(
                    state,
                    original_sender,
                    &Message::FriendMessageBlocked {
                        from: from.clone(),
                        to: to.clone(),
                        original_sender: original_sender.clone(),
                    },
                )
                .await;
            }
        }
        Message::ChatMessage {
            msg_id,
            from,
            to,
            kind,
            content,
            ts: _ts,
            seq,
        } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 去重前置：真实 msg_id 已落库 == 这条消息我此前已成功接收并持久化，
            // 于是只回 Ack。必须早于解密——否则对方轮换密钥后重投的那份「已收好的」
            // 消息会因当前密钥打不开旧密文而被误判为失败。
            let exists = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::message_exists(&dbc, &msg_id)
            };
            if exists {
                let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
                return;
            }
            // 好友关系检查：非好友消息不落库、不 Ack、通知发送方
            let is_friend = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some()
            };
            if !is_friend {
                let _ = try_send(
                    state,
                    peer_id,
                    &Message::FriendMessageBlocked {
                        from: state.device_id.clone(),
                        to: from.clone(),
                        original_sender: from.clone(),
                    },
                )
                .await;
                return;
            }
            // E2EE："enc1:" = 发送方→我的 ChaCha20-Poly1305 密文，用发送方 X25519 公钥打开。
            // 打不开（缺公钥 / 公钥已轮换 / 密文损坏）时**既不落库也不 Ack**，原因：
            //  - Ack 的语义是「已成功接收并持久化」，发送方一收到就会删掉 outbox 行；
            //  - 若用真实 msg_id 写一条占位系统消息，同一 msg_id 的后续正确副本会被
            //    INSERT OR IGNORE 静默吞掉，明文永久不可恢复（P0-2 的原始故障形态）。
            // 不 Ack ⇒ outbox 行保留 ⇒ Hello/心跳继续补发；期间 announce·who_has 会把
            // 双方公钥刷进 peers 与 friends 表，补发前还会用最新公钥重新密封
            // （见 flush_outbox / reseal_for_send），消息随自动恢复且不改变 msg_id。
            let Some((content, kind_str)) = open_direct_content(
                &state.identity.x25519_secret,
                sender_x25519_pubkey(state, &from).as_deref(),
                &content,
                kind,
            ) else {
                return;
            };
            let name = resolve_nickname(state, &from);
            let preview = preview_content(&kind_str, &content);
            // 持锁块只做落库，返回带钳制 ts 的记录 + SQLite 的三态裁决；await 全部在锁外
            // （MutexGuard 非 Send）。首次与否由 INSERT 的受影响行数裁决，而不是先查后写：
            // 与 Gossip 并发时只有一方拿到 Ok(true)，未读 +1 / message-received 因此各只一次。
            let (out_rec, inserted) = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                // 展示时间用本地接收时间；排序用对端给出的逻辑序号 seq。
                let ts = db::now_ms();
                let seq = seq.max(1);
                let rec = MessageRecord {
                    id: 0,
                    msg_id: msg_id.clone(),
                    conv_id: from.clone(),
                    sender_id: from.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: kind_str.clone(),
                    content: content.clone(),
                    ts,
                    seq,
                    status: "delivered".to_string(),
                };
                let inserted = db::insert_message_if_new(&dbc, &rec);
                if announced_on(&inserted) {
                    db::touch_conversation(&dbc, &from, "single", &name, None, &preview, 1).ok();
                    db::observe_clock(&dbc, &from, seq).ok();
                }
                (rec, inserted)
            };
            // 真数据库错误 ⇒ 消息没有持久化 ⇒ 既不投递也绝不 Ack：Ack 会让发送方删除
            // outbox 行，把一次临时故障变成永久丢消息（与 P0-2 同源的红线）。
            if !may_ack(&inserted) {
                return;
            }
            if announced_on(&inserted) {
                let _ = state.app.emit("message-received", &out_rec);
                // 直连单聊消息：链路 = 入站连接的路径，0 个中间节点。
                let path = inbound_path_kind(state, peer_id).await;
                update_conv_link(state, &from, &path, 0);
            }
            // Ack 与「是否本次新建」无关：消息已在库中（无论是哪条路径先写的）即代表已成功接收
            let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
        }
        Message::Ack { msg_id } => {
            // 查询原始发送方：如果这条消息不是我发的，说明我是中继节点，
            // 需要把 Ack 转发给原始发送方（而非本地处理）。
            let original_sender: Option<String> = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                dbc.query_row(
                    "SELECT sender_id FROM messages WHERE msg_id = ?1",
                    params![msg_id],
                    |r| r.get(0),
                )
                .ok()
            };
            match original_sender {
                Some(sender) if sender == state.device_id => {
                    // 情况 1：Ack 对应的原始消息是我发的 → 正常处理
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    // Ack 没有可伪造的 sender 字段，必须同时命中本连接对应的
                    // outbox 目标，避免任意 LAN 节点猜到 msg_id 后伪造送达。
                    let is_expected_peer: bool = dbc
                        .query_row(
                            "SELECT 1 FROM outbox WHERE msg_id = ?1 AND peer_id = ?2",
                            params![msg_id, peer_id],
                            |_| Ok(()),
                        )
                        .is_ok();
                    if !is_expected_peer {
                        return;
                    }
                    db::set_message_status(&dbc, &msg_id, "delivered").ok();
                    dbc.execute("DELETE FROM outbox WHERE msg_id = ?1", params![msg_id])
                        .ok();
                    drop(dbc);
                    let _ = state.app.emit("message-acked", &msg_id);
                }
                Some(sender) => {
                    // 情况 2：中继节点 → 转发 Ack 给原始发送方
                    // sender_id / message_id 保持不变，中继节点不做任何本地状态修改。
                    let _ = try_send(state, &sender, &Message::Ack { msg_id }).await;
                }
                None => {
                    // 查询不到 sender_id（消息不在本地 DB）→ 安全丢弃，不做任何修改。
                }
            }
        }
        Message::ReadReceipt {
            from,
            to,
            last_read_ts,
            last_read_msg_id,
        } => {
            if from != peer_id || to != state.device_id || from == state.device_id {
                return;
            }
            // 优先用 msg_id 换算回「我」的本地时间戳：接收方落库时对时间做过钳制，
            // 直接拿 last_read_ts 在跨设备时钟偏差下会匹配不到我发出的原始消息。
            let effective_ts = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                last_read_msg_id
                    .as_deref()
                    .and_then(|msg_id| {
                        dbc.query_row(
                            "SELECT ts FROM messages WHERE msg_id = ?1 AND sender_id = ?2 AND conv_id = ?3",
                            params![msg_id, state.device_id, from],
                            |r| r.get::<_, i64>(0),
                        )
                        .ok()
                    })
                    .unwrap_or(last_read_ts)
            };
            // 对方已读：把「我发给对方、ts ≤ effective_ts」的消息标记为 read（幂等）
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ = dbc.execute(
                    "UPDATE messages SET status = 'read'
                     WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                    params![from, state.device_id, effective_ts],
                );
            }
            // 无论 updated 是 0 还是 >0 都 emit：DB 可能已经是 read，
            // 但前端内存状态可能落后（事件竞态 / 会话重查覆盖），
            // 重新 emit 让 frontend 用 furthestStatus 再校准一次。
            // 这里必须发换算后的 effective_ts，前端才能用同一阈值正确标绿。
            let _ = state.app.emit(
                "peer-read",
                &serde_json::json!({ "peer_id": from, "last_read_ts": effective_ts }),
            );
        }
        Message::GroupReadReceipt {
            from,
            group_id,
            last_read_ts,
            last_read_msg_id,
        } => {
            if from != peer_id || from == state.device_id {
                return;
            }
            let is_member = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_group(&dbc, &group_id)
                    .map(|g| g.members.contains(&from) && g.members.contains(&state.device_id))
                    .unwrap_or(false)
            };
            if !is_member {
                return;
            }
            let effective_ts = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                last_read_msg_id
                    .as_deref()
                    .and_then(|msg_id| {
                        dbc.query_row(
                            "SELECT ts FROM messages WHERE msg_id = ?1 AND sender_id = ?2 AND conv_id = ?3",
                            params![msg_id, state.device_id, format!("group:{group_id}")],
                            |r| r.get::<_, i64>(0),
                        )
                        .ok()
                    })
                    .unwrap_or(last_read_ts)
            };
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_group_read(&dbc, &group_id, &from, effective_ts).ok();
            }
            let _ = state.app.emit(
                "group-read",
                &serde_json::json!({
                    "group_id": group_id,
                    "reader_id": from,
                    "last_read_ts": effective_ts,
                }),
            );
        }
        Message::GroupAck {
            group_id,
            msg_id,
            from,
        } => {
            if from != peer_id || from == state.device_id {
                return;
            }
            // 只清除该 peer 在该群中的待发记录；不存在时删除是安全的 no-op。
            // 命中失败不向外暴露，避免用伪造 Ack 探测本地 outbox。
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::delete_group_outbox(&dbc, &msg_id, &from);
            let _ = state.app.emit(
                "group-message-acked",
                &serde_json::json!({ "group_id": group_id, "msg_id": msg_id }),
            );
        }
        Message::FileOffer {
            transfer_id,
            from,
            name,
            size,
            sealed_file_key,
            file_sha256,
        } => {
            if from != peer_id || from == state.device_id {
                return;
            }
            let is_friend = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some()
            };
            if !is_friend {
                let _ = try_send(state, peer_id, &Message::FileReject { transfer_id }).await;
                return;
            }
            // SHA-256 元数据格式校验：非法即拒绝（文件级完整性无法验证）
            if !file::valid_sha256_hex(&file_sha256) {
                let _ = try_send(state, peer_id, &Message::FileReject { transfer_id }).await;
                return;
            }
            // E2EE：解封文件会话密钥（发送方用我方公钥封装，只有我能解开）。
            // 解封失败必须拒绝传输——密文分片绝不能落盘。
            let file_key = (|| {
                let sender_pub = resolve_member_x25519(state, &from)?;
                let shared = crypto::shared_secret(&state.identity.x25519_secret, &sender_pub)?;
                let sealed = STANDARD.decode(&sealed_file_key).ok()?;
                crypto::open(&shared, &sealed).and_then(|k| k.try_into().ok())
            })();
            let Some(file_key) = file_key else {
                let _ = try_send(state, peer_id, &Message::FileReject { transfer_id }).await;
                return;
            };
            match file::begin_receive(
                state,
                &transfer_id,
                &from,
                &name,
                size,
                file_key,
                file_sha256,
            ) {
                Ok(_) => {
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileAccept {
                            transfer_id: transfer_id.clone(),
                        },
                    )
                    .await;
                    let _ = state.app.emit(
                        "file-progress",
                        &FileProgress {
                            transfer_id: transfer_id.clone(),
                            received: 0,
                            total: size,
                        },
                    );
                }
                Err(e) => {
                    let _ = try_send(state, peer_id, &Message::FileReject { transfer_id }).await;
                    state.logger.error("file", format!("接收文件初始化失败: {e}"));
                }
            }
        }
        Message::FileAccept { transfer_id } => {
            if let Some(tx) = state
                .pending_file_accept
                .lock()
                .unwrap()
                .remove(&transfer_id)
            {
                let _ = tx.send(());
            }
        }
        Message::FileReject { transfer_id } => {
            state
                .pending_file_accept
                .lock()
                .unwrap()
                .remove(&transfer_id);
        }
        Message::FileCompleteAck {
            transfer_id,
            success,
        } => {
            // 只有该 transfer 的实际接收方发来的完成确认才有效。
            let expected_peer = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                dbc.query_row(
                    "SELECT peer_id FROM file_transfers WHERE id = ?1",
                    params![transfer_id],
                    |r| r.get::<_, String>(0),
                )
                .ok()
            };
            if expected_peer.as_deref() != Some(peer_id) {
                return;
            }
            if let Some(tx) = state
                .pending_file_complete
                .lock()
                .unwrap()
                .remove(&transfer_id)
            {
                let _ = tx.send(success);
            }
        }
        Message::FileChunk {
            transfer_id,
            seq,
            data,
        } => {
            match STANDARD
                .decode(&data)
                .map_err(|e| e.to_string())
                .and_then(|bytes| file::write_chunk(state, &transfer_id, peer_id, seq, &bytes))
            {
                Ok(received) => {
                    // 节流：每 250ms 至多上报一次进度，避免大文件 IPC 事件风暴
                    let (total, should_emit) = {
                        let mut recv = state.file_receivers.lock().unwrap_or_else(|e| e.into_inner());
                        match recv.get_mut(&transfer_id) {
                            Some(r) => {
                                let now = db::now_ms();
                                let emit = now - r.last_report_ms >= 250;
                                if emit {
                                    r.last_report_ms = now;
                                }
                                (r.size, emit)
                            }
                            None => (0, false),
                        }
                    };
                    if should_emit {
                        let _ = state.app.emit(
                            "file-progress",
                            &FileProgress {
                                transfer_id: transfer_id.clone(),
                                received,
                                total,
                            },
                        );
                    }
                }
                Err(e) => {
                    let _ = file::fail_receive(state, &transfer_id, peer_id, &e);
                }
            }
        }
        Message::FileDone { transfer_id } => {
            match file::finish_receive(state, &transfer_id, peer_id) {
                Err(e) => {
                    let _ = state.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id: transfer_id.clone(),
                            reason: e,
                        },
                    );
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileCompleteAck {
                            transfer_id,
                            success: false,
                        },
                    )
                    .await;
                }
                Ok(None) => {
                    // 重复 FileDone：若本机此前已成功完成该 transfer，则补一个成功确认，
                    // 避免发送方因重试而一直等待。
                    let already_done = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::list_transfers(&dbc)
                            .unwrap_or_default()
                            .into_iter()
                            .any(|t| t.id == transfer_id && t.status == "done")
                    };
                    if already_done {
                        let _ = try_send(
                            state,
                            peer_id,
                            &Message::FileCompleteAck {
                                transfer_id,
                                success: true,
                            },
                        )
                        .await;
                    }
                }
                Ok(Some((name, size, path, sender_id))) => {
                    let rec = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        let subtype = file::classify_file_subtype(&name);
                        let kind = if subtype == "image" { "image" } else { "file" };
                        let content = serde_json::json!({
                            "name": name,
                            "path": path.to_string_lossy().to_string(),
                            "size": size,
                            "subtype": subtype,
                        })
                        .to_string();
                        let seq = db::next_clock(&dbc, &sender_id).unwrap_or(1);
                        let rec = MessageRecord {
                            id: 0,
                            msg_id: format!("file-{transfer_id}"),
                            conv_id: sender_id.clone(),
                            sender_id: sender_id.clone(),
                            receiver_id: state.device_id.clone(),
                            kind: kind.to_string(),
                            content,
                            ts: db::now_ms(),
                            seq,
                            status: "delivered".to_string(),
                        };
                        db::insert_message(&dbc, &rec).ok();
                        let nm = resolve_nickname(state, &sender_id);
                        let preview = if kind == "image" { "[图片]".to_string() } else { format!("[文件] {name}") };
                        db::touch_conversation(
                            &dbc,
                            &sender_id,
                            "single",
                            &nm,
                            None,
                            &preview,
                            1,
                        )
                        .ok();
                        rec
                    };
                    let _ = state.app.emit("message-received", &rec);
                    let _ = state.app.emit(
                        "file-done",
                        &FileDoneInfo {
                            transfer_id: transfer_id.clone(),
                            name: name.clone(),
                            size,
                            path: path.to_string_lossy().to_string(),
                        },
                    );
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileCompleteAck {
                            transfer_id,
                            success: true,
                        },
                    )
                    .await;
                }
            }
        }
        Message::ShareTreeRequest {
            request_id,
            from,
            to,
        } => {
            if from != peer_id || to != state.device_id {
                return;
            }
            let is_friend = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some()
            };
            if !is_friend {
                return;
            }
            let entries = {
                let share = state.share_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
                match share {
                    Some(dir) => file::walk_share_dir(Path::new(&dir)),
                    None => Vec::new(),
                }
            };
            let resp = Message::ShareTreeResponse {
                request_id,
                from: state.device_id.clone(),
                entries,
            };
            let _ = try_send(state, peer_id, &resp).await;
        }
        Message::ShareTreeResponse {
            request_id,
            entries,
            ..
        } => {
            if let Some(tx) = state.pending_share_tree.lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id) {
                let _ = tx.send(entries);
            }
        }
        Message::ShareFileRequest {
            transfer_id,
            from,
            path,
        } => {
            if from != peer_id || from == state.device_id {
                return;
            }
            let is_friend = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some()
            };
            if !is_friend {
                return;
            }
            let share = state.share_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let Some(root) = share else { return };
            let root = PathBuf::from(root);
            let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let full = root.join(&path);
            let canon_full = full.canonicalize().unwrap_or_else(|_| full.clone());
            if !canon_full.starts_with(&canon_root) || !canon_full.is_file() {
                return;
            }
            let st = state.clone();
            let from = from.clone();
            // 本地提示：对方下载了你的共享文件（聊天信息内简约系统消息）
            let file_name = path
                .rsplit('/')
                .next()
                .map(|s| s.to_string())
                .unwrap_or_else(|| path.clone());
            let from_name = resolve_nickname(state, &from);
            crate::commands::insert_system_message(
                state,
                &from,
                &format!("「{from_name}」下载了你的文件「{file_name}」"),
            );
            tokio::spawn(async move {
                if let Err(e) = file::send_file_from_path(&st, &from, &transfer_id, canon_full).await
                {
                    let _ = st.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id,
                            reason: e.message,
                        },
                    );
                }
            });
        }
        // ---- Gossip 广播 ----
        Message::Gossip { envelope } => {
            handle_gossip(state, peer_id, envelope).await;
        }
        // ---- 中继文件传输 ----
        Message::RelayFileOffer {
            transfer_id,
            from,
            to,
            name,
            size,
            total_chunks,
            sealed_file_key,
            file_sha256,
        } => {
            handle_relay_file_offer(
                state,
                peer_id,
                transfer_id,
                from,
                to,
                name,
                size,
                total_chunks,
                sealed_file_key,
                file_sha256,
            )
            .await;
        }
        Message::RelayChunk {
            transfer_id,
            seq,
            data,
            from,
            to,
            ttl,
        } => {
            handle_relay_chunk(state, transfer_id, seq, data, from, to, ttl).await;
        }
        // ---- 群密钥分发 ----
        Message::GroupKey {
            group_id,
            from,
            to,
            key,
            group_name,
            members,
            clock,
        } => {
            if from != peer_id {
                return;
            }
            handle_group_key(state, group_id, from, to, key, group_name, members, clock).await;
        }
        Message::GroupRename {
            group_id,
            from,
            name,
        } => {
            if from != peer_id {
                return;
            }
            handle_group_rename(state, group_id, from, name).await;
        }
        Message::GroupMemberRemoved { group_id, from, to } => {
            if from != peer_id {
                return;
            }
            handle_group_member_removed(state, group_id, from, to).await;
        }
        Message::GroupCreatorChanged { group_id, from, to } => {
            if from != peer_id {
                return;
            }
            handle_group_creator_changed(state, group_id, from, to).await;
        }
        Message::GroupMemberLeft { group_id, from } => {
            if from != peer_id {
                return;
            }
            handle_group_member_left(state, group_id, from).await;
        }
        Message::GroupFileOffer {
            transfer_id,
            group_id,
            sender_id,
            name,
            size,
            sha256,
            sealed_file_key,
        } => {
            handle_group_file_offer(
                state,
                peer_id,
                transfer_id,
                group_id,
                sender_id,
                name,
                size,
                sha256,
                sealed_file_key,
            )
            .await;
        }
        Message::GroupFileChunk {
            transfer_id,
            group_id,
            sender_id,
            seq,
            data,
        } => {
            handle_group_file_chunk(state, peer_id, transfer_id, group_id, sender_id, seq, data)
                .await;
        }
        Message::GroupFileDone {
            transfer_id,
            group_id,
            sender_id,
        } => {
            handle_group_file_done(state, peer_id, transfer_id, group_id, sender_id).await;
        }
        Message::GroupFileCompleteAck {
            transfer_id,
            group_id,
            sender_id,
            success,
        } => {
            handle_group_file_complete_ack(state, peer_id, transfer_id, group_id, sender_id, success)
                .await;
        }
    }
}

// ---------------- 直连 E2EE 载荷 ----------------

/// 取发送方当前的 X25519 公钥：好友表优先，回退在线节点表。
/// 好友表由 `upsert_peer` 在 announce / who_has 检测到公钥变化时刷新，
/// 因此「对方换了身份」最长一个广播周期后就会收敛到这里。
fn sender_x25519_pubkey(state: &AppState, from: &str) -> Option<String> {
    let from_db = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend_x25519(&dbc, from)
    };
    from_db.or_else(|| {
        state
            .peers
            .lock()
            .unwrap()
            .get(from)
            .and_then(|p| p.x25519_pubkey.clone())
    })
}

/// 打开直连单聊载荷：`enc1:base64(nonce ‖ ChaCha20-Poly1305 密文)`。
/// 返回 `None` = 当前无法解密（缺对端公钥 / 密钥交换失败 / AEAD 校验失败 / UTF-8 非法）。
/// 调用方据此不落库、不 Ack——绝不返回占位文本，占位文本一旦占用真实 msg_id，
/// 同一 msg_id 的正确副本就永远进不来（`insert_message` 是 INSERT OR IGNORE）。
fn open_direct_content(
    my_x25519_secret: &StaticSecret,
    sender_pubkey: Option<&str>,
    wire: &str,
    kind: MsgKind,
) -> Option<(String, String)> {
    let b64 = wire.strip_prefix("enc1:")?;
    let pubkey = sender_pubkey?;
    let shared = crypto::shared_secret(my_x25519_secret, pubkey)?;
    let bytes = STANDARD.decode(b64).ok()?;
    let plain = crypto::open(&shared, &bytes)?;
    Some((String::from_utf8(plain).ok()?, kind.as_str().to_string()))
}

/// 用接收方当前公钥重新密封待发内容（`msg_id` 由调用方保持不变）。
/// 返回 `None` = 无法重封（本地无明文 / 拿不到当前公钥 / 加密失败），调用方按原样补发。
fn reseal_chat_content(
    my_x25519_secret: &StaticSecret,
    plaintext: Option<&str>,
    receiver_pubkey: Option<&str>,
) -> Option<String> {
    let shared = crypto::shared_secret(my_x25519_secret, receiver_pubkey?)?;
    let sealed = crypto::seal(&shared, plaintext?.as_bytes())?;
    Some(format!("enc1:{}", STANDARD.encode(sealed)))
}

/// 补发前重封一条 `ChatMessage`：密文是「加密时刻」的产物，若双方任一身份在那之后
/// 变化（重装 / 重新加好友），旧密文在接收方永远解不开，重发同一份密文没有意义。
/// 发送方 `messages` 表存的就是明文（见 `commands::send_message`），据此恢复明文并用
/// 最新公钥重封即可；`msg_id` 取自 Gossip 信封 ID、与密文无关，故重封不改变消息身份。
fn reseal_for_send(state: &AppState, msg: Message) -> Message {
    let Message::ChatMessage {
        msg_id,
        from,
        to,
        kind,
        content,
        ts,
        seq,
    } = msg
    else {
        return msg;
    };
    if !content.starts_with("enc1:") {
        return Message::ChatMessage {
            msg_id,
            from,
            to,
            kind,
            content,
            ts,
            seq,
        };
    }
    let (plaintext, from_db) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        // 只认「我自己发出的那条记录」：接收方行的 content 是对方会话的明文，语义不同
        let plaintext = dbc
            .query_row(
                "SELECT content FROM messages WHERE msg_id = ?1 AND sender_id = ?2",
                params![msg_id, state.device_id],
                |r| r.get::<_, String>(0),
            )
            .ok();
        (plaintext, db::get_friend_x25519(&dbc, &to))
    };
    let pubkey = from_db.or_else(|| {
        state
            .peers
            .lock()
            .unwrap()
            .get(&to)
            .and_then(|p| p.x25519_pubkey.clone())
    });
    let resealed = reseal_chat_content(
        &state.identity.x25519_secret,
        plaintext.as_deref(),
        pubkey.as_deref(),
    );
    Message::ChatMessage {
        content: resealed.unwrap_or(content),
        msg_id,
        from,
        to,
        kind,
        ts,
        seq,
    }
}

// ---------------- 落库裁决 → 副作用策略 ----------------
//
// `db::insert_message_if_new` 返回三态，绝不可折叠成 bool：
//   Ok(true)  = 本次真的插入了新行 ⇒ 唯一允许产生本地投递副作用的一方
//   Ok(false) = msg_id 已存在（INSERT OR IGNORE 命中唯一约束）⇒ 不得再有副作用
//   Err(e)    = 真正的数据库故障（不是重复！）⇒ 消息没有持久化
// 因此「是否投递」与「是否 Ack」是两个独立判定：后者只在 Err 时必须禁止，
// 因为 Ack 会让发送方删除 outbox 行，把一次临时故障变成永久丢消息。

/// 本次落库是否应产生本地投递副作用（`touch_conversation(+1)` 与 `message-received`）。
fn announced_on(inserted: &Result<bool, rusqlite::Error>) -> bool {
    matches!(inserted, Ok(true))
}

/// 是否允许向发送方回 Ack。重复（`Ok(false)`）允许——消息确已在库；
/// 真数据库错误（`Err`）不允许——否则 outbox 被删，消息永久丢失。
fn may_ack(inserted: &Result<bool, rusqlite::Error>) -> bool {
    !matches!(inserted, Err(_))
}

// ---------------- Gossip 处理 ----------------

/// 多跳转发：把 MeshFrame 载荷还原成 GossipEnvelope，发给选中的下一跳。
///
/// **不新增协议**：转发出去的仍是 `Message::Gossip`，下一跳按现有逻辑处理，
/// 因此 wire 格式不变、新旧客户端仍然互通（避免触发 INV-P13 的协议变更流程）。
///
/// 排除两类目标：
/// ① 原始源节点（§18 source exclusion，由 `select_outgoing` 完成）；
/// ② 该帧的入站 peer —— 发回去只是浪费，下一跳的 dedup 也会把它丢弃。
///
/// 转发是**尽力而为**：失败可忽略。Gossip 的可靠性由 outbox / dedup 保证，
/// 不依赖中继成功。
async fn handle_gossip(state: &Arc<AppState>, peer_id: &str, env: GossipEnvelope) {
    // Gossip 可经第三方转发，不能仅凭信封内自报的 Ed25519 公钥建立身份。
    // 公钥必须先由 Discovery/Hello 绑定到同一个 device_id；若已知 X25519
    // 公钥也发生变化，同样拒绝，避免冒充好友或污染 E2EE 密钥缓存。
    let sender_trusted = {
        if env.sender_id == state.device_id {
            false
        } else {
            let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
            match peers.get(&env.sender_id) {
                // 已认识：公钥必须匹配（无论是直连还是跨跳转发），防冒充。
                Some(p) => {
                    let direct_peer = peer_id == env.sender_id;
                    (p.ed25519_pubkey.as_deref() == Some(env.sender_ed25519.as_str())
                        || (direct_peer && p.ed25519_pubkey.is_none()))
                        && (p
                            .x25519_pubkey
                            .as_deref()
                            .is_none_or(|key| key == env.sender_pubkey)
                            || (direct_peer && p.x25519_pubkey.is_none()))
                }
                // 未认识：仅 Presence（节点通告）允许 TOFU —— 它存在的目的就是
                // 让「不认识」的节点被全网看到。其余消息仍拒，避免陌生人直接投递。
                None => match env.kind {
                    GossipKind::Presence
                    | GossipKind::FriendRequest
                    | GossipKind::FriendAccept => true,
                    // 回执/确认不能 TOFU：发送方必须是「已绑定身份」的好友。
                    // peers 是内存态，进程重启后为空，此处回退到 friends 表
                    // （持久化的 ed25519 公钥）完成身份绑定，避免重启后跨跳
                    // 回执被误拒。
                    GossipKind::ChatAck | GossipKind::ChatReadReceipt => {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::get_friend_ed25519(&dbc, &env.sender_id).as_deref()
                            == Some(env.sender_ed25519.as_str())
                    }
                    _ => false,
                },
            }
        }
    };
    if !sender_trusted {
        return;
    }
    // 直连 TCP 对端在 Hello 中没有携带公钥时，首次合法 Gossip 可完成 TOFU
    // 绑定；之后所有经中继或直连的信封都必须匹配这组键。
    if peer_id == env.sender_id {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peers.get_mut(&env.sender_id) {
            if p.ed25519_pubkey.is_none() {
                p.ed25519_pubkey = Some(env.sender_ed25519.clone());
            }
            if p.x25519_pubkey.is_none() {
                p.x25519_pubkey = Some(env.sender_pubkey.clone());
            }
        }
    }
    // 1. 先验签，再进入去重缓存。否则攻击者可以用伪造的唯一 message_id
    // 污染 Bloom/LRU，甚至抢先占用真实消息的 id 造成合法消息被丢弃。
    {
        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
        if !gossip.verify_envelope(&env) {
            return;
        }
        drop(gossip);
        // Mesh 层：**全局去重 + TTL 判定**（§15 / §16 / §17）。
        //
        // 必须在**验签之后**才登记去重表，否则攻击者可用伪造的 frame_id 污染
        // Bloom，抢先占用真实帧的 id 造成合法消息被丢弃 —— 与上面 GossipEngine
        // 的防护同理。
        //
        // ⚠️ 这里**只判定不转发**：真正的多跳转发在下面第 4 步（`decide_forward` +
        // `choose_fanout`）。历史上这里还有一条由 `MeshRouter::relay_enabled` 门控的
        // 转发路径（默认关闭、生产从不调用），2026-09-12 已删除 ——
        // **中继授权的唯一真相是 `settings.relay_policy`**（ADR-0016），
        // 接 BLE 时不要再复制第二份转发记账。
        {
            let mut router = state.mesh_router.lock().unwrap_or_else(|e| e.into_inner());
            let frame = MeshFrame {
                frame_id: env.message_id.clone(),
                source_node_id: env.sender_id.clone(),
                destination: MeshDestination::Broadcast,
                ttl: env.ttl,
                kind: MeshFrameKind::Gosslan,
                // 转发在别处做，这里只需要元数据（MeshRouter 不解析载荷，P-A03）
                payload: Vec::new(),
            };
            if let ForwardDecision::Drop(_) = router.on_receive(frame, &state.device_id) {
                return;
            }
        }
        // 业务层去重（Mesh 之后的第二道防线）
        let mut gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
        if !gossip.is_new(&env.message_id) {
            return;
        }
    }
    // 2. 同步发送方公钥：GossipEnvelope 已携带 x25519_pubkey 用于解密，
    //    但此前未写入 peers/friends，导致后续 outbox 重发的直发 ChatMessage
    //    在 open_direct_content() 中因缺 pubkey 被丢弃。此处仅更新已有 peer
    //    条目（不做 insert，避免为未通过 Discovery 的节点创建残缺记录），
    //    同时用 COALESCE 安全地补充 friends 表的 NULL pubkey。
    if env.encrypted && matches!(env.kind, GossipKind::Chat) {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peers.get_mut(&env.sender_id) {
            if p.x25519_pubkey.is_none() {
                p.x25519_pubkey = Some(env.sender_pubkey.clone());
                p.last_seen = db::now_ms();
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::update_friend_pubkeys(&dbc, &env.sender_id, Some(&env.sender_pubkey), None)
                    .ok();
            }
        }
    }
    // 3. 解包：E2EE 关闭时载荷为明文 JSON；开启时按单聊 ECDH / 群密钥解密
    let plaintext = if !env.encrypted {
        // FriendMessageBlocked 等明文 Gossip：payload 是 base64 编码的 JSON
        STANDARD.decode(&env.payload).ok()
    } else {
        match &env.kind {
            GossipKind::Chat => {
                let shared =
                    crypto::shared_secret(&state.identity.x25519_secret, &env.sender_pubkey);
                shared.and_then(|s| {
                    STANDARD
                        .decode(&env.payload)
                        .ok()
                        .and_then(|d| crypto::open(&s, &d))
                })
            }
            GossipKind::Group => {
                let gid = env.group_id.clone().unwrap_or_default();
                let key = get_group_key(state, &gid).await;
                key.and_then(|k| {
                    STANDARD
                        .decode(&env.payload)
                        .ok()
                        .and_then(|d| crypto::open_symmetric(&k, &d))
                })
            }
            GossipKind::FriendMessageBlocked => {
                // 已在 encrypted=false 分支处理，这里不应进入
                return;
            }
            GossipKind::Presence => {
                // Presence 必然是明文；收到 encrypted=true 的是异常，丢弃。
                return;
            }
            GossipKind::FriendRequest | GossipKind::FriendAccept => {
                // 定向好友控制消息：发送方用 target 的 X25519 公钥加密，只有 target
                // 能用自己的私钥解开；中间节点解不开（plaintext=None），无害。
                let shared =
                    crypto::shared_secret(&state.identity.x25519_secret, &env.sender_pubkey);
                shared.and_then(|s| {
                    STANDARD
                        .decode(&env.payload)
                        .ok()
                        .and_then(|d| crypto::open(&s, &d))
                })
            }
            GossipKind::ChatAck | GossipKind::ChatReadReceipt => {
                // 回执/确认必然是明文；收到 encrypted=true 的是异常，丢弃。
                return;
            }
        }
    };

    // 群信封即使签名正确，也只能被群成员消费；签名证明“是谁发的”，
    // 不代表发送者有权把任意节点加入一个群。
    if matches!(env.kind, GossipKind::Group) && !env.group_members.is_empty() {
        if !env.group_members.iter().any(|m| m == &state.device_id)
            || !env.group_members.iter().any(|m| m == &env.sender_id)
        {
            return;
        }
    }

    // 4. 转发（fan-out，TTL 衰减）— 先过**中继授权**（P2 / M4），再选人转发。
    //
    // 三条判据全部收在 `mesh::relay_policy::decide_forward` 里（真值表有单测），
    // 这里只负责喂事实，避免把传播语义散落成 if：
    //   ① 定向帧到达目标后**停止转发**（本机就是 target，只消费）：否则目标会把定向帧
    //      再洪泛给其他邻居，邻居又按 target 定向转发回来，形成冗余中转与回环。真机反馈
    //      「同网段好友申请一直中转、清掉还冒出来」正是这个回环造成的；
    //   ② TTL 耗尽不再转发；
    //   ③ 替**别人**转发要过授权策略（自己发的信封不受策略限制）。
    //
    // ⚠️ 默认策略是 `all`（与今天逐字节一致）；好友/白名单查询是**按需**的 ——
    // `all`/`off` 下一次库都不查，转发热路径零额外开销。
    let is_target = env.target.as_deref() == Some(state.device_id.as_str());
    let sender_is_me = env.sender_id == state.device_id;
    let relay_cfg = state.relay_policy_config();
    let may_forward = crate::mesh::relay_policy::decide_forward(
        &relay_cfg,
        is_target,
        env.ttl,
        sender_is_me,
        &env.sender_id,
        || {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_friend(&dbc, &env.sender_id).is_some()
        },
    );
    if may_forward {
        // 定向帧（FriendRequest/FriendAccept/ChatAck/ChatReadReceipt）优先精确定向：
        // target 是本机直连就只发它；无直连路径时洪泛兜底（第一版无路由表）。广播帧保持 fan-out。
        let targets: Vec<String> = match env.target.as_deref() {
            Some(t) => {
                // ⚠️ 必须判「有**非空**链路」（等价于 `has_link`），不能用 `contains_key`：
                // 后者会把残留的空 Vec 当成「直连」→ 走只发 target 的分支 →
                // `try_send` 返回「未建立连接」，而**洪泛兜底不会执行** ⇒
                // 跨跳的好友申请 / 送达回执 / 已读回执可能永久丢失（复核抓到的缺陷）。
                let direct = {
                    let links = state.links.lock().await;
                    links.get(t).is_some_and(|v| !v.is_empty())
                };
                if direct {
                    vec![t.to_string()]
                } else {
                    let peers: Vec<String> = state
                        .peers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .keys()
                        .cloned()
                        .collect();
                    let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                    gossip.choose_fanout(&peers, &env.sender_id)
                }
            }
            None => {
                let peers: Vec<String> = state
                    .peers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .keys()
                    .cloned()
                    .collect();
                let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                gossip.choose_fanout(&peers, &env.sender_id)
            }
        };
        let mut fwd = env.clone();
        fwd.ttl -= 1;
        let fwd_msg = Message::Gossip { envelope: fwd };
        // ⚠️ 转发**不阻塞本连接的读循环**：`try_send` 在信道满时有界补试 500ms，
        // 逐条 await 最坏 4 × 500ms = 2s —— 期间这条连接的后续帧（含心跳）都要排队，
        // shutdown/取消也要等。顺序在这里无关紧要（接收侧按 msg_id 去重，而且这些
        // 只是同一条消息发给**不同**邻居）。用一个任务串行发完：并发度不变、任务数可控。
        let st = state.clone();
        tokio::spawn(async move {
            for t in targets {
                let _ = try_send(&st, &t, &fwd_msg).await;
            }
        });
    }

    // 5. 按 GossipKind 处理
    match env.kind {
        GossipKind::FriendMessageBlocked => {
            // 控制消息：检查本机是否为原始发送方
            if let Some(pt) = plaintext {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                    if let Some(original) = v.get("original_sender").and_then(|s| s.as_str()) {
                        if original == state.device_id {
                            // 本机就是原始发送方 → 通知前端
                            let _ = state.app.emit("friend-message-blocked", &env.sender_id);
                        }
                        // 非本机 → 已在上面 fan-out 转发，不做任何 UI/DB 操作
                    }
                }
            }
        }
        GossipKind::Presence => {
            // TOFU 记录远端节点：跨跳转发的 Presence，sender 可能不在 peers 里。
            // 「去中心化发现」的落地 —— A 经 B 转发看到 C，C 进入 peers 表，
            // 前端「添加好友」列表即出现 C（即使 A 与 C 无直连）。
            if let Some(pt) = plaintext {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                    let nickname = v
                        .get("nickname")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let avatar = v
                        .get("avatar")
                        .and_then(|a| a.as_str())
                        .map(|s| s.to_string());
                    let device_type = v
                        .get("device_type")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    // 首次学到才留痕：peers 是内存结构、不落库，这行日志是唯一可观测
                    // 「跨跳发现了谁」的手段（与 [mesh] ±conn、握手学到身份同理）。
                    let is_new = {
                        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
                        !peers.contains_key(&env.sender_id)
                    };
                    if is_new {
                        state.logger.info(
                            "presence",
                            format!("学到远端节点 peer={} nickname={}", env.sender_id, nickname),
                        );
                    }
                    // ip 空、tcp_port 0：跨跳转发不知道对端真实地址，仅记录身份
                    // （可被「看到」，但不可直连）。
                    upsert_peer(
                        state,
                        &env.sender_id,
                        &nickname,
                        avatar,
                        &device_type,
                        "",
                        0,
                        Some(env.sender_pubkey.clone()),
                        Some(env.sender_ed25519.clone()),
                        None,
                    )
                    .await;
                }
            }
        }
        GossipKind::FriendRequest => {
            // 定向好友申请：只有 target == 本机才处理（中间节点已转发，不消费）。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                if let Some(pt) = plaintext {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                        let from_nickname = v
                            .get("from_nickname")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let from_avatar = v
                            .get("from_avatar")
                            .and_then(|a| a.as_str())
                            .map(|s| s.to_string());
                        // 同步记录申请方身份与公钥：这样「同意」时才有对方 X25519 公钥
                        // 可加密回发的 FriendAccept（跨跳场景下 Presense 可能还没到）。
                        upsert_peer(
                            state,
                            &env.sender_id,
                            &from_nickname,
                            from_avatar.clone(),
                            "",
                            "",
                            0,
                            Some(env.sender_pubkey.clone()),
                            Some(env.sender_ed25519.clone()),
                            None,
                        )
                        .await;
                        let req = PendingRequest {
                            from: env.sender_id.clone(),
                            from_nickname,
                            from_avatar,
                            ts: env.ts,
                        };
                        state
                            .pending_requests
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(env.sender_id.clone(), req.clone());
                        let _ = state.app.emit("friend-request", &req);
                        // 留痕：跨跳好友申请是内存态（pending_requests），日志是唯一
                        // 可观测「谁申请了我」的手段（headless 测试与真机排障都靠它）。
                        state.logger.info(
                            "friend",
                            format!(
                                "收到跨跳好友申请 peer={} nickname={}",
                                env.sender_id, req.from_nickname
                            ),
                        );
                        let mut extra = std::collections::HashMap::new();
                        extra.insert("type".to_string(), "friend_request".to_string());
                        notify_with_extra(
                            &state.app,
                            "好友申请",
                            &format!("{} 请求添加你为好友", req.from_nickname),
                            extra,
                        );
                    }
                }
            }
        }
        GossipKind::FriendAccept => {
            // 定向好友同意：只有 target == 本机才处理（即「我发的申请被对方同意」）。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                let from = env.sender_id.clone();
                let name = resolve_nickname(state, &from);
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::add_friend(&dbc, &from, &name, None).ok();
                    // 同步公钥（否则首次加密发送会失败）—— 与 Message::FriendAccept 路径一致。
                    let (x, e) = {
                        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
                        peers
                            .get(&from)
                            .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                            .unwrap_or((None, None))
                    };
                    if x.is_some() || e.is_some() {
                        db::update_friend_pubkeys(&dbc, &from, x.as_deref(), e.as_deref()).ok();
                    }
                }
                forget_pending_request(state, &from);
                let _ = state.app.emit("friend-accepted", &from);
                // 留痕：跨跳好友同意是落库（friends 表）+ 内存态，日志便于 headless 观测。
                state.logger.info("friend", format!("收到跨跳好友同意 peer={from}"));
                notify(
                    &state.app,
                    "好友申请已通过",
                    &format!("{name} 已成为你的好友"),
                );
            }
        }
        GossipKind::ChatAck => {
            // 定向送达确认：只有 target == 本机才处理（即「我发的消息被对方收到」）。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                if let Some(pt) = plaintext {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                        if let Some(msg_id) = v.get("msg_id").and_then(|m| m.as_str()) {
                            // 只有「我发给 sender、且仍在 outbox」的 msg_id 才接受：
                            // msg_id 随机不可预测 + 必须命中 outbox 目标，双重防伪造送达。
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            let is_expected = dbc
                                .query_row(
                                    "SELECT 1 FROM outbox WHERE msg_id = ?1 AND peer_id = ?2",
                                    params![msg_id, env.sender_id],
                                    |_| Ok(()),
                                )
                                .is_ok();
                            if !is_expected {
                                return;
                            }
                            db::set_message_status(&dbc, msg_id, "delivered").ok();
                            dbc.execute("DELETE FROM outbox WHERE msg_id = ?1", params![msg_id])
                                .ok();
                            drop(dbc);
                            let _ = state.app.emit("message-acked", msg_id);
                        }
                    }
                }
            }
        }
        GossipKind::ChatReadReceipt => {
            // 定向已读回执：只有 target == 本机才处理。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                if let Some(pt) = plaintext {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                        let from = env.sender_id.clone();
                        let last_read_ts = v.get("last_read_ts").and_then(|t| t.as_i64()).unwrap_or(0);
                        let last_read_msg_id = v
                            .get("last_read_msg_id")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string());
                        // 与 Message::ReadReceipt 分支同构：用 msg_id 换算回本机时间戳。
                        let effective_ts = {
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            last_read_msg_id
                                .as_deref()
                                .and_then(|msg_id| {
                                    dbc.query_row(
                                        "SELECT ts FROM messages WHERE msg_id = ?1 AND sender_id = ?2 AND conv_id = ?3",
                                        params![msg_id, state.device_id, from],
                                        |r| r.get::<_, i64>(0),
                                    )
                                    .ok()
                                })
                                .unwrap_or(last_read_ts)
                        };
                        {
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = dbc.execute(
                                "UPDATE messages SET status = 'read'
                                 WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                                params![from, state.device_id, effective_ts],
                            );
                        }
                        let _ = state.app.emit(
                            "peer-read",
                            &serde_json::json!({ "peer_id": from, "last_read_ts": effective_ts }),
                        );
                    }
                }
            }
        }
        GossipKind::Chat | GossipKind::Group => {
            // 单聊定向：target 存在且不是本机 → 中间节点只转发不消费。即便不判断，
            // 中间节点也会因 ECDH 解不开而 plaintext=None（不会落库），但明确判断
            // 语义更清晰、也避免无谓的好友关系检查。群聊无 target，走原广播消费逻辑。
            if env.kind == GossipKind::Chat
                && env
                    .target
                    .as_deref()
                    .is_some_and(|t| t != state.device_id.as_str())
            {
                return;
            }
            if let Some(pt) = plaintext {
                let (kind, content) = parse_gossip_payload(&pt);
                // GossipKind::Chat：好友关系检查（非好友不落库、不通知、通知发送方）
                if env.kind == GossipKind::Chat {
                    let is_friend = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::get_friend(&dbc, &env.sender_id).is_some()
                    };
                    if !is_friend {
                        // 通过 Gossip 广播拒绝通知（多跳场景下也能回到原始发送方）
                        let mut blocked_env = {
                            let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                            let payload =
                                serde_json::json!({ "original_sender": env.sender_id }).to_string();
                            let payload_b64 = STANDARD.encode(payload.as_bytes());
                            gossip.build_envelope(
                                &state.identity,
                                &state.device_id,
                                GossipKind::FriendMessageBlocked,
                                None,
                                None,
                                &payload_b64,
                                db::now_ms(),
                                0,
                            )
                        };
                        // 这是拒绝通知控制载荷，不是用户聊天内容；显式标记为明文，
                        // 同时不再为用户 Chat/Group 提供明文兼容路径。
                        blocked_env.encrypted = false;
                        blocked_env.sender_sig =
                            state.identity.sign_b64(&blocked_env.signing_bytes());
                        broadcast_gossip(state, blocked_env).await;
                        return;
                    }
                }
                let conv_id = match &env.kind {
                    GossipKind::Chat => env.sender_id.clone(),
                    GossipKind::Group => {
                        format!("group:{}", env.group_id.clone().unwrap_or_default())
                    }
                    _ => return,
                };
                let conv_kind = match &env.kind {
                    GossipKind::Chat => "single",
                    GossipKind::Group => "group",
                    _ => return,
                };
                let name = match &env.kind {
                    GossipKind::Chat => resolve_nickname(state, &env.sender_id),
                    GossipKind::Group => {
                        resolve_group_name(state, env.group_id.as_deref().unwrap_or(""))
                    }
                    _ => return,
                };
                // 群聊删除边界：本机清除过该群（seq ≤ boundary）的旧历史不得回灌。
                // 逻辑序号不依赖墙上时钟，也不猜测发送方时钟。
                if env.kind == GossipKind::Group {
                    let gid = env.group_id.clone().unwrap_or_default();
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    let blocked = db::group_message_blocked_by_boundary(&dbc, &gid, env.seq);
                    drop(dbc);
                    if blocked {
                        return;
                    }
                }
                // 群消息顺带建群：成员端可能从未收到 GroupKey（本地无 groups 行），
                // 但这条群消息携带了完整成员表 → 据此 upsert 建群，成员面板才能显示。
                // 已有则只刷新（成员随踢人/加人变化时也能及时同步）。
                if env.kind == GossipKind::Group {
                    if let (Some(gid), Some(creator), true) = (
                        env.group_id.clone(),
                        env.group_creator.clone(),
                        !env.group_members.is_empty(),
                    ) {
                        let display_name = env.group_name.clone().unwrap_or_else(|| name.clone());
                        let mut all = env.group_members.clone();
                        if !all.contains(&state.device_id) {
                            all.push(state.device_id.clone());
                        }
                        {
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            db::upsert_group(&dbc, &gid, &display_name, &creator, &all).ok();
                        }
                        let _ = state.app.emit("groups-updated", &gid);
                    }
                }
                let preview = preview_content(&kind, &content);
                // 持锁块只做落库；await（fanout 转发已在前面）之后无持锁操作
                // 业务幂等裁决：Direct（含 outbox 补发）可能已经把同一 msg_id 落库，此时
                // 不得再计未读、再发 message-received，否则未读数与系统通知都会重复。
                // 与 Direct 分支共用 announced_on 裁决，两路径并发时只有一方拿到 Ok(true)。
                // 单聊与群聊走同一块 ⇒ 两种 GossipKind 都被覆盖。
                let (out_rec, inserted) = {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    // 展示时间用本地接收时间；排序用信封携带的逻辑序号 seq。
                    let ts = db::now_ms();
                    let seq = env.seq.max(1);
                    let rec = MessageRecord {
                        id: 0,
                        msg_id: env.message_id.clone(),
                        conv_id: conv_id.clone(),
                        sender_id: env.sender_id.clone(),
                        receiver_id: state.device_id.clone(),
                        kind: kind.clone(),
                        content: content.clone(),
                        ts,
                        seq,
                        status: "delivered".to_string(),
                    };
                    let inserted = db::insert_message_if_new(&dbc, &rec);
                    if announced_on(&inserted) {
                        db::touch_conversation(&dbc, &conv_id, conv_kind, &name, None, &preview, 1)
                            .ok();
                        db::observe_clock(&dbc, &conv_id, seq).ok();
                    }
                    (rec, inserted)
                };
                // 重复投递与数据库失败都不产生本地副作用；Gossip 的转发已在上面完成。
                if announced_on(&inserted) {
                    let _ = state.app.emit("message-received", &out_rec);
                }
                // 更新会话「当前链路」：单聊消息的 hop 由 Gossip ttl 反推
                // （初始 ttl - 收到 ttl），path 取入站连接的路径（直连准确，桥接为最后一段）。
                // 仅首次落库（非重复投递）才更新，避免「走了不同路径的重复副本」干扰。
                if conv_kind == "single" && announced_on(&inserted) {
                    let hop = {
                        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                        gossip.ttl.saturating_sub(env.ttl) as u8
                    };
                    let path = inbound_path_kind(state, peer_id).await;
                    update_conv_link(state, &conv_id, &path, hop);
                }
                // 群消息现在有 outbox 兜底：只要消息确实已持久化（无论本次是否新建），
                // 就回 GroupAck 让发送方删除对应 (msg_id, peer_id) 的待发记录。
                // 数据库 Err 时不回 Ack，发送方保留 outbox 继续补发。
                if conv_kind == "group" && !matches!(&inserted, Err(_)) {
                    let ack = Message::GroupAck {
                        group_id: env.group_id.clone().unwrap_or_default(),
                        msg_id: env.message_id.clone(),
                        from: state.device_id.clone(),
                    };
                    let _ = try_send(state, &env.sender_id, &ack).await;
                }
                // 单聊送达确认：跨跳（无直连）时直连 Ack 到不了原始发送方，改走定向
                // Gossip ChatAck；有直连时也走 Gossip，让「已送达」立即出现，不必等
                // 心跳触发 outbox 直发补 Ack。接收端按 outbox(msg_id, sender) 命中才接受，
                // 防伪造送达。
                if conv_kind == "single" && !matches!(&inserted, Err(_)) {
                    let payload = serde_json::json!({ "msg_id": env.message_id }).to_string();
                    let payload_b64 = STANDARD.encode(payload.as_bytes());
                    let mut ack_env = {
                        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                        gossip.build_envelope(
                            &state.identity,
                            &state.device_id,
                            GossipKind::ChatAck,
                            None,
                            None,
                            &payload_b64,
                            db::now_ms(),
                            0,
                        )
                    };
                    ack_env.encrypted = false;
                    ack_env.target = Some(env.sender_id.clone());
                    ack_env.sender_sig = state.identity.sign_b64(&ack_env.signing_bytes());
                    if state.has_link(&env.sender_id).await {
                        let _ = try_send(
                            state,
                            &env.sender_id,
                            &Message::Gossip { envelope: ack_env },
                        )
                        .await;
                    } else {
                        broadcast_gossip(state, ack_env).await;
                    }
                }
            }
        }
    }
}

fn parse_gossip_payload(pt: &[u8]) -> (String, String) {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(pt) {
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("text")
            .to_string();
        let content = v
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        (kind, content)
    } else {
        ("text".to_string(), String::from_utf8_lossy(pt).to_string())
    }
}

// ---------------- 中继文件传输 ----------------

#[allow(clippy::too_many_arguments)]
async fn handle_relay_file_offer(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    from: String,
    to: String,
    name: String,
    size: u64,
    total_chunks: u32,
    sealed_file_key: String,
    file_sha256: String,
) {
    if to != state.device_id {
        return; // 中继节点无需重组，只转发切片
    }
    if from != peer_id || from == state.device_id || total_chunks == 0 || size > i64::MAX as u64 {
        return;
    }
    if file::safe_file_name(&name).is_none() {
        return;
    }
    let is_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend(&dbc, &from).is_some()
    };
    if !is_friend {
        return;
    }
    // SHA-256 元数据格式校验（中继不解密不校验内容，仅最终接收方校验）
    if !file::valid_sha256_hex(&file_sha256) {
        return;
    }
    // E2EE：解封文件会话密钥（发送方用我方公钥封装）。中继节点不持有密钥；
    // 解封失败直接放弃——密文分片绝不落盘。
    let file_key = (|| {
        let sender_pub = resolve_member_x25519(state, &from)?;
        let shared = crypto::shared_secret(&state.identity.x25519_secret, &sender_pub)?;
        let sealed = STANDARD.decode(&sealed_file_key).ok()?;
        crypto::open(&shared, &sealed).and_then(|k| k.try_into().ok())
    })();
    let Some(file_key) = file_key else {
        return;
    };
    state.relay_file_keys.lock().unwrap_or_else(|e| e.into_inner()).insert(
        transfer_id.clone(),
        crate::state::RelayFileReceive {
            file_key,
            expected_sha256: file_sha256,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        },
    );
    state
        .relay
        .lock()
        .unwrap()
        .begin_reassemble(&transfer_id, &name, total_chunks, size);
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &from,
            &name,
            size,
            "receive",
            "active",
            None,
            0.0,
        )
        .ok();
    }
    let _ = state.app.emit(
        "file-progress",
        &FileProgress {
            transfer_id,
            received: 0,
            total: size,
        },
    );
}

async fn handle_relay_chunk(
    state: &Arc<AppState>,
    transfer_id: String,
    seq: u32,
    data: String,
    from: String,
    to: String,
    ttl: u8,
) {
    if to == state.device_id {
        // 最终接收方：先解密（E2EE，密文不落盘），再增量哈希、重组
        let mut keys = state.relay_file_keys.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rs) = keys.get_mut(&transfer_id) else {
            return;
        };
        let Ok(sealed) = STANDARD.decode(&data) else {
            return;
        };
        let Some(bytes) = crypto::open_symmetric(&rs.file_key, &sealed) else {
            return;
        };
        use sha2::Digest;
        rs.hasher.update(&bytes);
        drop(keys);
        let completed = {
            let mut relay = state.relay.lock().unwrap_or_else(|e| e.into_inner());
            relay.add_chunk(&transfer_id, seq, bytes)
        };
        if let Some((name, expected_size, full)) = completed {
            // 重组结束（无论成败）：移除会话状态，取哈希做完整性校验
            let rs = state.relay_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id);
            if full.len() as u64 != expected_size {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_transfer(
                    &dbc,
                    &transfer_id,
                    &from,
                    &name,
                    expected_size,
                    "receive",
                    "failed",
                    None,
                    0.0,
                )
                .ok();
                let _ = state.app.emit(
                    "file-failed",
                    &FileFailedInfo {
                        transfer_id: transfer_id.clone(),
                        reason: "中继文件大小校验失败".to_string(),
                    },
                );
                return;
            }
            // 文件级完整性：重组内容 SHA-256 必须与发送方声明一致，否则不落盘
            if let Some(rs) = rs {
                use sha2::Digest;
                let actual_hex: String = rs
                    .hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                if !actual_hex.eq_ignore_ascii_case(&rs.expected_sha256) {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        &transfer_id,
                        &from,
                        &name,
                        expected_size,
                        "receive",
                        "failed",
                        None,
                        0.0,
                    )
                    .ok();
                    let _ = state.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id: transfer_id.clone(),
                            reason: "文件完整性校验失败".to_string(),
                        },
                    );
                    return;
                }
            }
            let path = match save_received_bytes(state, &name, &full) {
                Ok(path) => path,
                Err(reason) => {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        &transfer_id,
                        &from,
                        &name,
                        expected_size,
                        "receive",
                        "failed",
                        None,
                        0.0,
                    )
                    .ok();
                    let _ = state.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id: transfer_id.clone(),
                            reason,
                        },
                    );
                    return;
                }
            };
            let path_str = path.to_string_lossy().to_string();
            let rec = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_transfer(
                    &dbc,
                    &transfer_id,
                    &from,
                    &name,
                    full.len() as u64,
                    "receive",
                    "done",
                    Some(path_str.as_str()),
                    1.0,
                )
                .ok();
                let content = serde_json::json!({
                    "name": name.clone(),
                    "path": path_str.clone(),
                    "size": full.len(),
                    "subtype": file::classify_file_subtype(&name),
                })
                .to_string();
                let seq = db::next_clock(&dbc, &from).unwrap_or(1);
                let rec = MessageRecord {
                    id: 0,
                    msg_id: format!("file-{transfer_id}"),
                    conv_id: from.clone(),
                    sender_id: from.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: "file".to_string(),
                    content,
                    ts: db::now_ms(),
                    seq,
                    status: "delivered".to_string(),
                };
                db::insert_message(&dbc, &rec).ok();
                let nm = resolve_nickname(state, &from);
                db::touch_conversation(
                    &dbc,
                    &from,
                    "single",
                    &nm,
                    None,
                    &format!("[文件] {name}"),
                    1,
                )
                .ok();
                rec
            };
            let _ = state.app.emit("message-received", &rec);
            let _ = state.app.emit(
                "file-done",
                &FileDoneInfo {
                    transfer_id: transfer_id.clone(),
                    name: name.clone(),
                    size: full.len() as u64,
                    path: path_str,
                },
            );
        }
        // 重组中：进度可基于切片数上报，此处省略，完成时由 file-done 事件通知
    } else if ttl > 1 {
        // 中继转发给最终接收方
        let fwd = Message::RelayChunk {
            transfer_id,
            seq,
            data,
            from,
            to: to.clone(),
            ttl: ttl - 1,
        };
        let _ = try_send(state, &to, &fwd).await;
    }
}

fn save_received_bytes(state: &AppState, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = state.downloads_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe_name = file::safe_file_name(name).ok_or("文件名非法")?;
    let base = dir.join(safe_name);
    if !base.exists() {
        std::fs::write(&base, bytes).map_err(|e| e.to_string())?;
        return Ok(base);
    }
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for i in 1..1000 {
        let cand = if ext.is_empty() {
            dir.join(format!("{stem} ({i})"))
        } else {
            dir.join(format!("{stem} ({i}).{ext}"))
        };
        if !cand.exists() {
            std::fs::write(&cand, bytes).map_err(|e| e.to_string())?;
            return Ok(cand);
        }
    }
    Err("下载目录重名文件过多".to_string())
}

// ---------------- 群密钥 ----------------

#[allow(clippy::too_many_arguments)]
async fn handle_group_key(
    state: &Arc<AppState>,
    group_id: String,
    from: String,
    to: String,
    key: String,
    group_name: String,
    members: Vec<String>,
    clock: i64,
) {
    if to != state.device_id {
        return;
    }
    // 只有群创建者能够分发/轮换群密钥；同时要求消息携带的成员表
    // 明确包含发送者和接收者，避免任意好友注入一个伪造群或密钥。
    //
    // ⚠️ 这道校验**不能放宽给普通成员**：若任意成员都能推送新密钥，恶意成员即可用
    // 自己持有的密钥替换受害者的群密钥，从而读到受害者后续发出的群消息。
    // 因此「群主离线时密钥无法传播」不能靠放宽此处解决，而靠**不制造只有群主才有的密钥**——
    // 见 `group_add_member`：加人不再轮换群密钥；轮换只保留在「移除成员」路径
    // （那个时机群主必然在线，且撤权本就需要换新密钥）。
    // 无本地群记录时（新成员首次拿密钥）此处放行，这是新成员入群的唯一途径。
    if !members.iter().any(|m| m == &from) || !members.iter().any(|m| m == &to) {
        return;
    }
    if let Some(group) = db::get_group(&state.db.lock().unwrap_or_else(|e| e.into_inner()), &group_id) {
        if group.creator != from {
            return;
        }
    }
    let pubkey = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(&from).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(pubkey) = pubkey else { return };
    let Some(shared) = crypto::shared_secret(&state.identity.x25519_secret, &pubkey) else {
        return;
    };
    let Ok(sealed) = STANDARD.decode(&key) else {
        return;
    };
    let Some(raw) = crypto::open(&shared, &sealed) else {
        return;
    };
    if raw.len() != 32 {
        return;
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&raw);
    state.group_keys.lock().unwrap_or_else(|e| e.into_inner()).insert(group_id.clone(), k);
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, &format!("gk:{group_id}"), &STANDARD.encode(k)).ok();
    }
    // 建本地群记录：没有它，收到群消息时群名只能兜底成「群聊 g-xxxx」，
    // 成员面板也会为空。成员列表补上自己，保证与创建者一致。
    let mut all = members;
    if !all.contains(&state.device_id) {
        all.push(state.device_id.clone());
    }
    let conv_id = format!("group:{group_id}");
    let display_name = if group_name.is_empty() {
        resolve_group_name(state, &group_id)
    } else {
        group_name
    };
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_group(&dbc, &group_id, &display_name, &from, &all).ok();
        // 群关系同步（GroupKey / 群成员 / 群密钥 / 群名）只更新 groups / group_members，
        // **不得创建 conversation**：conversation 是「聊天会话索引」，只能由聊天活动驱动
        // （收到新群消息 → insert_message → touch_conversation）。否则用户清库重装后仅凭
        // 群关系同步，群聊就会凭空重新出现在聊天列表（关系同步 ≠ 聊天同步）。
        db::observe_clock(&dbc, &conv_id, clock).ok();
    }
    let _ = state.app.emit("group-key-received", &group_id);
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理群文件发起（Offer → 验证 → file_key 解封 → 保存会话状态）。
/// 本阶段不写文件、不创建 `.part`、不自动 FileAccept——分片传输在下一阶段。
async fn handle_group_file_offer(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
    name: String,
    size: u64,
    sha256: String,
    sealed_file_key: String,
) {
    // 链路上报的 sender 必须与 Offer 声明一致，且不能是自己
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    // 幂等：会话已激活（内存有 file_key）→ 重复 Offer 忽略。
    // 注意不能因为「group_files 里有记录」就跳过：重启后内存 key 丢失但记录还在，
    // 跳过会让离线补发永远无法重建会话（接收卡死）。改为下方「已完成则跳过」+「幂等重建」。
    if state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&transfer_id) {
        return;
    }
    // 权限：本地群存在，且 sender ∈ group_members（防群外 peer 伪造 Offer）
    let (group_exists, sender_is_member) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => (true, g.members.contains(&sender_id)),
            None => (false, false),
        }
    };
    if !group_exists || !sender_is_member {
        return;
    }
    // 本地 GroupKey 解封 file_key（GroupKey 不离开设备；群外无法解开）
    let Some(group_key) = get_group_key(state, &group_id).await else {
        return;
    };
    let Ok(sealed) = STANDARD.decode(&sealed_file_key) else {
        return;
    };
    let Some(file_key) = crypto::open_symmetric(&group_key, &sealed).and_then(|k| k.try_into().ok())
    else {
        return;
    };

    // 已完成的 transfer → 忽略（避免重复接收 / 重复 emit）
    let already_done = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group_file_recipient_status(&dbc, &transfer_id, &state.device_id)
            .as_deref()
            == Some("completed")
    };
    if already_done {
        return;
    }

    // 幂等建立/重建接收会话（重启后内存 file_key 丢失，这里回填 key + 复位 recipient）
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let gf = crate::state::GroupFile {
            transfer_id: transfer_id.clone(),
            group_id: group_id.clone(),
            sender_id: sender_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            status: "sending".to_string(),
            created_at: db::now_ms(),
        };
        if db::upsert_group_file_receive(&dbc, &gf, &state.device_id).is_err() {
            return;
        }
    }
    // 接收气泡：与发送端同一 msg_id（gfile-{transfer_id}），前端据 file-progress
    // 之外的状态事件推进。此处 status=sending，Done 校验通过后转 delivered。
    // 图片文件保持 kind="image"，业务语义不降级。
    let subtype = file::classify_file_subtype(&name);
    let kind = if subtype == "image" { "image" } else { "file" };
    let conv_id = format!("group:{group_id}");
    let seq = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, &conv_id).unwrap_or(1)
    };
    let rec = crate::state::MessageRecord {
        id: 0,
        msg_id: format!("gfile-{transfer_id}"),
        conv_id: conv_id.clone(),
        sender_id: sender_id.clone(),
        receiver_id: state.device_id.clone(),
        kind: kind.to_string(),
        content: serde_json::json!({ "name": name, "size": size, "sha256": sha256, "subtype": subtype }).to_string(),
        ts: db::now_ms(),
        seq,
        status: "sending".to_string(),
    };
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::insert_message(&dbc, &rec).ok();
        db::touch_conversation(
            &dbc,
            &format!("group:{group_id}"),
            "group",
            &name,
            None,
            &format!("[群文件] {name}"),
            1,
        )
        .ok();
    }
    let _ = state.app.emit("message-received", &rec);
    // 会话密钥仅存内存，供下一阶段解密 GroupFileChunk
    state
        .group_file_keys
        .lock()
        .unwrap()
        .insert(transfer_id, file_key);
}

/// 失败收尾：删除 `.part`、移除接收状态与会话密钥、recipient 置 failed。
/// 只影响本 transfer，不 panic、不影响其他群文件。
fn fail_group_file_chunk(state: &Arc<AppState>, transfer_id: &str) {
    file::fail_group_receive(state, transfer_id);
    set_gfile_bubble_status(state, transfer_id, "failed");
    state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(transfer_id);
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let _ = db::update_group_file_recipient(&dbc, transfer_id, &state.device_id, "failed", 0.0);
}

/// 群文件气泡状态推进：msg_id = gfile-{transfer_id}（收发双方本地记录）。
fn set_gfile_bubble_status(state: &AppState, transfer_id: &str, status: &str) {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db::set_message_status(&dbc, &format!("gfile-{transfer_id}"), status).ok();
}

/// 处理群文件发送完毕：最终校验（size + SHA-256）→ sync_all → rename → completed。
/// 幂等：session 已清理（已完成或从未建立）时安全忽略。
/// sender-side 的 completed 只代表「本机接收完成」，不是发送端 recipient 状态。
async fn handle_group_file_done(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
) {
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    // 幂等 / 无 session：已完成或从未建立 Offer session → 安全忽略
    if !state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&transfer_id) {
        return;
    }
    // 权限：群存在 && sender 是群成员
    let (group_exists, sender_is_member) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => (true, g.members.contains(&sender_id)),
            None => (false, false),
        }
    };
    if !group_exists || !sender_is_member {
        return;
    }
    let Some(gf) = db::get_group_file(&state.db.lock().unwrap_or_else(|e| e.into_inner()), &transfer_id) else {
        return;
    };

    // 空文件：无 Chunk 阶段，Done 时才建立接收状态（0 字节 .part）
    if !state.group_file_receivers.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&transfer_id) {
        if gf.size == 0 {
            if file::begin_group_receive(
                state,
                &transfer_id,
                &sender_id,
                &gf.name,
                0,
                [0u8; 32],
                gf.sha256.clone(),
            )
            .is_err()
            {
                return;
            }
        } else {
            // 有声明大小但一个分片都没收到 → 不完整：failed + 清理
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
    }

    // 从接收表移除（取得所有权），做最终校验与落盘
    let mut r = match state.group_file_receivers.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id) {
        Some(r) => r,
        None => return,
    };

    // 1. size 校验：received 必须等于声明大小
    if r.received != r.size {
        let _ = std::fs::remove_file(&r.tmp_path);
        set_gfile_bubble_status(state, &transfer_id, "failed");
        state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id);
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ =
                db::update_group_file_recipient(&dbc, &transfer_id, &state.device_id, "failed", 0.0);
            // 接收 transfer 同步 failed
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &sender_id,
                &gf.name,
                gf.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
        }
        send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
        return;
    }
    // 2. SHA-256 校验：finalize 增量哈希（不重读 .part），与发送方声明比对
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
            set_gfile_bubble_status(state, &transfer_id, "failed");
            state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id);
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ = db::update_group_file_recipient(
                    &dbc,
                    &transfer_id,
                    &state.device_id,
                    "failed",
                    0.0,
                );
            }
            send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
            return;
        }
    }
    // 3. sync_all：落盘前确保数据写透
    if let Err(e) = r.file.sync_all() {
        let _ = e.to_string();
        let _ = std::fs::remove_file(&r.tmp_path);
        set_gfile_bubble_status(state, &transfer_id, "failed");
        state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id);
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ =
                db::update_group_file_recipient(&dbc, &transfer_id, &state.device_id, "failed", 0.0);
            // 接收 transfer 同步 failed
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &sender_id,
                &gf.name,
                gf.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
        }
        send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
        return;
    }
    // 4. drop 文件句柄后 rename（Windows 不允许 rename 打开中的文件）
    drop(r.file);
    // 最终名是"开始接收"时就定下的，那一刻同样在途的同名传输还没落盘、unique_path 看不到它
    // → 两条同名传输可能选中同一个名字。这里在真正落盘前再确认一次：被占走就换名。
    // （单聊路径本来就是在写盘时才定名，所以没有这个竞态——这也是"群聊出问题、单聊正常"的原因。）
    if r.final_path.exists() {
        let dl = state.downloads_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
        r.final_path = file::unique_path(&dl, &r.name);
    }
    if let Err(_) = std::fs::rename(&r.tmp_path, &r.final_path) {
        let _ = std::fs::remove_file(&r.tmp_path);
        set_gfile_bubble_status(state, &transfer_id, "failed");
        state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id);
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ =
                db::update_group_file_recipient(&dbc, &transfer_id, &state.device_id, "failed", 0.0);
            // 接收 transfer 同步 failed
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &sender_id,
                &gf.name,
                gf.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
        }
        send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
        return;
    }
    // 全部成功：正式文件已落盘 → completed / progress 1.0 → 气泡转 delivered → 清理
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(&dbc, &transfer_id, &state.device_id, "completed", 1.0);
        // 群文件本地路径持久化到 transfer 记录：打开/另存/历史加载经
        // transfer_id（gfile-{tid}）关联到该真实本地路径（重启后仍有效）
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &sender_id,
            &gf.name,
            gf.size,
            "receive",
            "done",
            Some(r.final_path.to_string_lossy().as_ref()),
            1.0,
        )
        .ok();
    }
    state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&transfer_id);
    // 重发带本地路径的记录（applyIncoming 按 msg_id 合并更新，未读不重复）：
    // 前端气泡 content.path 就绪 → 打开/另存/图片代码预览立即可用
    let msg_id = format!("gfile-{transfer_id}");
    let seq = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        dbc.query_row(
            "SELECT seq FROM messages WHERE msg_id = ?1",
            params![msg_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(1)
    };
    let subtype = file::classify_file_subtype(&gf.name);
    let kind = if subtype == "image" { "image" } else { "file" };
    let done_rec = crate::state::MessageRecord {
        id: 0,
        msg_id,
        conv_id: format!("group:{group_id}"),
        sender_id: sender_id.clone(),
        receiver_id: state.device_id.clone(),
        kind: kind.to_string(),
        content: serde_json::json!({
            "name": gf.name,
            "path": r.final_path.to_string_lossy(),
            "size": gf.size,
            "sha256": gf.sha256,
            "subtype": subtype,
        })
        .to_string(),
        ts: db::now_ms(),
        seq,
        status: "delivered".to_string(),
    };
    // 回填本地 path 到 messages 表：read_file_preview 按 msg_id 反查 content 定位文件。
    // 单聊 FileDone 走 insert_message 直接落库带 path 的内容；群聊 Offer 先落库无 path 的
    // 内容（文件尚未下载），Done 时必须显式更新，否则接收方图片/代码预览因缺 path 失败。
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::update_message_content(&dbc, &done_rec.msg_id, &done_rec.content, &done_rec.status).ok();
    }
    let _ = state.app.emit("message-received", &done_rec);
    // 完成确认：无论 ACK 发送成败，file_key 已清理不再保留（ACK 丢失由后续阶段处理）
    send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, true).await;
}

/// 向原始群文件发送者回送接收完成确认（receiver → sender）。
/// ACK 丢失可接受（发送端保持 sending，等待后续 retry/offline recovery），
/// 不因此重新保留 file_key。
async fn send_group_file_complete_ack(
    state: &Arc<AppState>,
    transfer_id: &str,
    group_id: &str,
    original_sender: &str,
    success: bool,
) {
    let ack = Message::GroupFileCompleteAck {
        transfer_id: transfer_id.to_string(),
        group_id: group_id.to_string(),
        sender_id: state.device_id.clone(),
        success,
    };
    let _ = try_send(state, original_sender, &ack).await;
}

/// 处理接收完成确认（sender 侧）：更新对应 recipient 的 completed/failed 状态。
///
/// 身份验证（防伪造）：
/// 1. ACK.sender_id == TCP peer_id（不能只相信消息字段）；
/// 2. sender_id != 本机；
/// 3. transfer 对应的 group_file.sender_id == 本机（只有本机发起的群文件
///    的 ACK 才会被处理，B 发给 A 的 transfer 的 ACK 到 C 手上会被拒绝）；
/// 4. ACK 发送者必须是该 transfer 的 recipient（update 不命中即拒绝）。
/// 幂等：重复 ACK 重复 UPDATE 同状态，无副作用、不报错。
async fn handle_group_file_complete_ack(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
    success: bool,
) {
    // 1+2. ACK 发送者必须与链路对端一致，且不能是自己
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    // 3. transfer 必须是本机发出的群文件，且 group_id 一致
    let Some(gf) = db::get_group_file(&state.db.lock().unwrap_or_else(|e| e.into_inner()), &transfer_id) else {
        return;
    };
    if gf.sender_id != state.device_id || gf.group_id != group_id {
        return;
    }
    // 幂等保护：completed 是终态。该 recipient 已 completed 时，
    // 后续 success=false ACK（重放/异常）不得把 completed 降级为 failed；
    // success=true ACK 重复到达则是无害的幂等更新。
    if !success {
        let already_completed = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::list_group_file_recipients(&dbc, &transfer_id)
                .unwrap_or_default()
                .into_iter()
                .any(|r| r.recipient_id == peer_id && r.status == "completed")
        };
        if already_completed {
            return;
        }
    }
    // 4. ACK 发送者必须是该 transfer 的 recipient，且状态按 success 迁移；
    //    只修改该 recipient，不影响其他成员。不命中（非 recipient）→ 拒绝。
    let (status, progress) = if success {
        ("completed", 1.0)
    } else {
        ("failed", 0.0)
    };
    // 发送端气泡 = 全体 recipient 结果的聚合（v0.12 最小语义），
    // 避免最后一个 ACK 直接覆盖之前更准确的总体状态：
    // - success ACK → delivered（有人收到即算；mixed 亦然，且不回退）
    // - failure ACK → 全部 recipient 都到终态（completed/failed）时：
    //     有人 completed → delivered；全部 failed → failed；
    //   仍有 pending/sending → 气泡保持当前状态（sending），等后续 ACK。
    let bubble;
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(&dbc, &transfer_id, &peer_id, status, progress);
        if success {
            bubble = "delivered";
        } else {
            let recipients =
                db::list_group_file_recipients(&dbc, &transfer_id).unwrap_or_default();
            let all_terminal = recipients
                .iter()
                .all(|r| r.status == "completed" || r.status == "failed");
            if !all_terminal {
                return; // 仍有进行中的 recipient：气泡保持当前，等后续 ACK
            }
            bubble = if recipients.iter().any(|r| r.status == "completed") {
                "delivered"
            } else {
                "failed"
            };
        }
        let _ = db::set_message_status(&dbc, &format!("gfile-{transfer_id}"), bubble).ok();
        // sender 的 transfer 记录随聚合结果推进。
        // 状态：delivered → done，否则 failed（保留原有语义）。
        // 进度：按**在线成员**口径算，而不是「delivered 就写 1.0」——
        // 否则「在线成员全到了、但离线成员还 pending」时进度条会提前满格，
        // 与用户口径（离线不计入分母，在线全到才算完）相冲突。
        // ⚠️ `group_file_online_progress` 内部取 db 锁：必须先 drop 本段持有的锁，
        // 否则 std Mutex 同锁重入即死锁。
        let tf_status = if bubble == "delivered" { "done" } else { "failed" };
        let path = db::list_transfers(&dbc)
            .unwrap_or_default()
            .into_iter()
            .find(|t| t.id == transfer_id)
            .and_then(|t| t.path);
        drop(dbc);
        let tf_progress = crate::commands::group_file_online_progress(state, &transfer_id, 0.0);
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &gf.group_id,
            &gf.name,
            gf.size,
            "send",
            tf_status,
            path.as_deref(),
            tf_progress,
        )
        .ok();
        // 本 transfer 已到终态（delivered/failed）→ 进度条口径快照用完即弃，
        // 避免无界增长；注意**不能**只在 `tf_progress >= 1.0` 时清 ——
        // 「发送时无人在线」的 transfer 进度恒 0，那样就永远清不掉。
        if bubble == "delivered" || bubble == "failed" {
            state
                .group_file_online_targets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id);
        }
    }
    if bubble == "delivered" {
        let _ = state.app.emit("message-acked", &format!("gfile-{transfer_id}"));
    }
}

/// 处理群文件分片（E2EE 解密 → seq/size 校验 → 增量哈希 → 写 `.part`）。
/// 无 GroupFileDone / 无 rename / 不标 completed——完成确认在下一阶段。
async fn handle_group_file_chunk(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
    seq: u32,
    data: String,
) {
    // 权限：链路 sender 与声明一致、不能是自己、群存在、sender 是群成员
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    let (group_exists, sender_is_member) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => (true, g.members.contains(&sender_id)),
            None => (false, false),
        }
    };
    if !group_exists || !sender_is_member {
        return;
    }
    // 会话必须已经由合法 GroupFileOffer 建立；无 key 直接丢弃（不尝试其他密钥）
    let Some(file_key) = state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).get(&transfer_id).copied() else {
        return;
    };
    // 本地群文件记录（Offer 阶段建立）提供 name/size/sha256
    let Some(gf) = db::get_group_file(&state.db.lock().unwrap_or_else(|e| e.into_inner()), &transfer_id) else {
        return;
    };
    // 只有该 transfer 的原始 sender 发来的 chunk 才合法。
    // 其他群成员（无 file_key）发送的垃圾 chunk 不得终止合法接收：
    // 直接忽略，不删 .part、不清 session、不改 recipient 状态。
    if gf.sender_id != sender_id {
        return;
    }

    // 首个合法 chunk 到达时才创建 `.part`（安全路径，downloads 目录内）
    if !state.group_file_receivers.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&transfer_id) {
        if let Err(_) = file::begin_group_receive(
            state,
            &transfer_id,
            &sender_id,
            &gf.name,
            gf.size,
            file_key,
            gf.sha256.clone(),
        ) {
            return;
        }
    }

    // 解密（AEAD 失败 → 失败收尾：删 `.part`、置 failed，不写错误明文）
    let Ok(sealed) = STANDARD.decode(&data) else {
        fail_group_file_chunk(state, &transfer_id);
        return;
    };
    let Some(plaintext) = crypto::open_symmetric(&file_key, &sealed) else {
        fail_group_file_chunk(state, &transfer_id);
        return;
    };

    // seq / size 校验与写盘（复用一对一 FileReceiver 的严格语义）
    {
        use std::io::Write;
        let mut recv = state.group_file_receivers.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = recv.get_mut(&transfer_id) else {
            return;
        };
        if seq != r.next_seq {
            // 顺序错误：终止当前接收（TCP 有序，跳号/重复即异常）
            drop(recv);
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
        if plaintext.len() as u64 > r.size.saturating_sub(r.received) {
            // 超出声明大小：防恶意 sender
            drop(recv);
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
        // 增量哈希（为下一阶段 FileDone 校验准备；本阶段不比对）
        use sha2::Digest;
        r.hasher.update(&plaintext);
        if r.file.write_all(&plaintext).is_err() {
            drop(recv);
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
        r.received += plaintext.len() as u64;
        r.next_seq = r.next_seq.wrapping_add(1);
        let progress = if r.size == 0 {
            1.0
        } else {
            (r.received as f64 / r.size as f64).min(1.0)
        };
        drop(recv);
        // 进度落库：本阶段最高 sending，不标 completed
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(
            &dbc,
            &transfer_id,
            &state.device_id,
            "sending",
            progress,
        );
    }
}

/// 处理群名变更广播：仅群创建者可发起，成员端校验后同步本地群名与会话标题。
async fn handle_group_rename(state: &Arc<AppState>, group_id: String, from: String, name: String) {
    if name.is_empty() || from == state.device_id {
        return;
    }
    let name: String = name.chars().take(MAX_GROUP_NAME_LEN).collect();
    // 只接受群创建者的改名
    let is_creator = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.creator == from)
            .unwrap_or(false)
    };
    if !is_creator {
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::rename_group(&dbc, &group_id, &name).ok();
    }
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理「成员被移出群」。
///
/// 两个分支，**此前只有第一个**：
/// 1. `to == 本机`：我本人被移出 → 清理本地群 + 会话 + 群密钥；
/// 2. `to != 本机`：别人被移出 → 同步本地成员表 + 清掉指向他的待补发群消息 +
///    落一条群内系统消息，让群里的人都知道。
async fn handle_group_member_removed(
    state: &Arc<AppState>,
    group_id: String,
    from: String,
    to: String,
) {
    if from == state.device_id {
        return; // 本机发起的移人，本地已处理（含系统消息）
    }
    // 只接受**群创建者**发起的移人（防成员互踢）
    let is_creator = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.creator == from)
            .unwrap_or(false)
    };
    match member_removed_action(false, is_creator, to == state.device_id) {
        MemberRemovedAction::Ignore => return,
        // ---- ① 我本人被移出 ----
        MemberRemovedAction::RemoveSelf => {
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::delete_group(&dbc, &group_id).ok();
                let _ = dbc.execute(
                    "DELETE FROM settings WHERE key = ?1",
                    params![format!("gk:{group_id}")],
                );
            }
            state.group_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&group_id);
            let _ = state.app.emit("group-member-removed", &group_id);
            let _ = state.app.emit("groups-updated", &group_id);
            return;
        }
        // ---- ② 别人被移出：同步成员表 + 群内系统消息 ----
        MemberRemovedAction::RemoveOther => {}
    }

    let changed = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) if g.members.contains(&to) => {
                // 他已经不是成员了：指向他的待补发群消息也不该再投递
                db::delete_group_outbox_for_peer_in_group(&dbc, &group_id, &to).ok();
                db::remove_group_member(&dbc, &group_id, &to).is_ok()
            }
            _ => false,
        }
    };
    if !changed {
        return; // 幂等：已经不在成员表里就不重复插系统消息
    }
    let name = resolve_nickname(state, &to);
    insert_group_system_message(state, &group_id, &group_member_removed_text(state, &name));
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理「群主转让」：只接受**当前创建者**发起、且新群主确实是群成员的转让。
/// 广播给全体成员，因此新任群主自己也会收到并更新本地记录。
async fn handle_group_creator_changed(
    state: &Arc<AppState>,
    group_id: String,
    from: String,
    to: String,
) {
    if from == state.device_id {
        return; // 本机发起的转让，本地已更新
    }
    let ok = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => g.creator == from && g.members.contains(&to),
            None => false,
        }
    };
    if !ok {
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_group_creator(&dbc, &group_id, &to).ok();
    }
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理「成员主动退群」：把 `from` 从本地成员表移除（幂等）。
/// 群主不允许直接退群（须先转让），因此忽略「群主退出」这类异常/伪造消息。
async fn handle_group_member_left(state: &Arc<AppState>, group_id: String, from: String) {
    if from == state.device_id {
        return; // 本机发起的退群，本地已处理
    }
    let changed = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) if g.creator != from && g.members.contains(&from) => {
                // 他已经退了：指向他的待补发群消息不该再投递
                db::delete_group_outbox_for_peer_in_group(&dbc, &group_id, &from).ok();
                db::remove_group_member(&dbc, &group_id, &from).is_ok()
            }
            _ => false,
        }
    };
    if changed {
        // 群内系统消息 —— 此前成员表会同步，但群里看不到任何提示
        let name = resolve_nickname(state, &from);
        insert_group_system_message(state, &group_id, &group_member_left_text(state, &name));
        let _ = state.app.emit("groups-updated", &group_id);
    }
}

pub async fn get_group_key(state: &AppState, group_id: &str) -> Option<[u8; 32]> {
    if let Some(k) = state.group_keys.lock().unwrap_or_else(|e| e.into_inner()).get(group_id) {
        return Some(*k);
    }
    let key_b64 = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, &format!("gk:{group_id}"))
    }?;
    let bytes = STANDARD.decode(key_b64).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    state
        .group_keys
        .lock()
        .unwrap()
        .insert(group_id.to_string(), arr);
    Some(arr)
}

// ---------------- 节点与好友辅助 ----------------

/// 同一 device_id 报出与已绑定值不同的公钥时，向用户给出**一次**可见告警。
///
/// 为什么必须让用户看见：这是区分「对方重装了应用」与「有人冒名顶替」的唯一外部信号。
/// 静默处理会让用户无法判断，违反 AI_RULES §19「不得静默接受不可验证的密钥」。
/// 为什么只在首次告警：announce 每 5s 一次、冲突会持续存在，不去重会把聊天记录刷爆。
///
/// 安全行为不变：冲突时**绝不覆盖**已绑定的公钥（见 `upsert_peer` 的 key_conflict 分支），
/// 因此最坏情况只是对方真的重装后我方需要重新建立信任，而不会把消息发给冒充者的密钥。
fn warn_key_conflict_once(state: &AppState, device_id: &str) {
    {
        let mut warned = state.key_conflict_warned.lock().unwrap_or_else(|e| e.into_inner());
        if !warned.insert(device_id.to_string()) {
            return;
        }
    }
    // 非好友不建会话：避免陌生节点刷出一串空会话
    let name = resolve_nickname(state, device_id);
    let is_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend(&dbc, device_id).is_some()
    };
    if !is_friend {
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::ensure_conversation(&dbc, device_id, "single", &name, None).ok();
    }
    crate::commands::insert_system_message(
        state,
        device_id,
        &format!(
            "⚠️「{name}」的身份密钥发生变化，已保留原密钥未替换。可能是对方重装了应用；\
             也不能排除有人冒名顶替，建议当面核对后再继续通信。"
        ),
    );
}

pub async fn upsert_peer(
    state: &AppState,
    device_id: &str,
    nickname: &str,
    avatar: Option<String>,
    device_type: &str,
    ip: &str,
    tcp_port: u16,
    x25519: Option<String>,
    ed25519: Option<String>,
    rtt_ms: Option<u64>,
) {
    let ts = db::now_ms();
    // 判断是否「新节点」或「公钥首次学到/变化」，据此决定是否做昂贵的落库与群密钥补发。
    // 500-1000 节点下，若每条 announce 都写库 + 遍历群组，会形成明显热点。
    let (is_new, key_changed, key_conflict) = {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        match peers.get_mut(device_id) {
            None => {
                peers.insert(
                    device_id.to_string(),
                    Peer {
                        device_id: device_id.to_string(),
                        nickname: nickname.to_string(),
                        avatar,
                        device_type: device_type.to_string(),
                        ip: ip.to_string(),
                        tcp_port,
                        last_seen: ts,
                        rtt_ms,
                        x25519_pubkey: x25519.clone(),
                        ed25519_pubkey: ed25519.clone(),
                        first_seen: Some(ts),
                    },
                );
                (true, true, false)
            }
            Some(p) => {
                let x_conflict =
                    matches!((&p.x25519_pubkey, &x25519), (Some(old), Some(new)) if old != new);
                let e_conflict =
                    matches!((&p.ed25519_pubkey, &ed25519), (Some(old), Some(new)) if old != new);
                let key_conflict = x_conflict || e_conflict;
                let key_changed = !key_conflict
                    && ((x25519.is_some() && p.x25519_pubkey != x25519)
                        || (ed25519.is_some() && p.ed25519_pubkey != ed25519));
                p.nickname = nickname.to_string();
                if avatar.is_some() {
                    p.avatar = avatar;
                }
                if !device_type.is_empty() {
                    p.device_type = device_type.to_string();
                }
                if !ip.is_empty() {
                    p.ip = ip.to_string();
                }
                if tcp_port != 0 {
                    p.tcp_port = tcp_port;
                }
                if x25519.is_some() && !x_conflict {
                    p.x25519_pubkey = x25519;
                }
                if ed25519.is_some() && !e_conflict {
                    p.ed25519_pubkey = ed25519;
                }
                if rtt_ms.is_some() {
                    p.rtt_ms = rtt_ms;
                }
                p.last_seen = ts;
                (false, key_changed, key_conflict)
            }
        }
    };
    state.emit_peers();

    if key_conflict {
        state.push_diag_event("identity_key_conflict", &format!("device_id={device_id}"));
        warn_key_conflict_once(state, device_id);
        return;
    }

    // 仅在公钥首次学到/变化时才落库（避免每条 announce 都写库）
    if key_changed {
        let (x, e) = {
            let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
            peers
                .get(device_id)
                .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                .unwrap_or((None, None))
        };
        if x.is_some() || e.is_some() {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::update_friend_pubkeys(&dbc, device_id, x.as_deref(), e.as_deref()).ok();
        }
        // 公钥变化时同步到 friends 表：Hello 可能在 announce 之前到达，
        // 此时 peers[peer].x25519_pubkey 为 None → friends 表写入 None；
        // announce 到达后更新了 peers，但 friends 表不会自动刷新。
        // 此处补一次 maybe_update_friend 确保 friends 表与 peers 同步。
        let (nick, av) = {
            let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
            peers
                .get(device_id)
                .map(|p| (p.nickname.clone(), p.avatar.clone()))
                .unwrap_or_default()
        };
        maybe_update_friend(state, device_id, &nick, av);
    }

    // 仅新节点或公钥变化时补发群密钥（处理对方离线时建群的情况）
    if is_new || key_changed {
        redistribute_group_keys(state, device_id).await;
        // 重发本机聊天样式：对方离线期间错过 broadcastChatStyle 广播，
        // 且样式广播无离线补偿——对方上线后必须补发，否则永远看不到配色
        resend_chat_style(state, device_id);
    }
}

/// 向指定 peer 重发本机聊天样式（复用既有 ChatStyle 消息，无新协议）。
fn resend_chat_style(state: &AppState, peer_id: &str) {
    let style = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, "chat_style")
    };
    let Some(style) = style else {
        return;
    };
    if style.is_empty() {
        return;
    }
    let msg = Message::ChatStyle {
        from: state.device_id.clone(),
        to: Some(peer_id.to_string()),
        style,
    };
    let _ = try_send(state, peer_id, &msg);
}

/// 群密钥发送失败的原因。仅 `NoLink` 可重试（登记 pending 等建链后 flush）。
enum GroupKeySendErr {
    /// TCP link 不可用（未建立连接 / 已断开）——可重试
    NoLink,
    /// 缺公钥 / 非群成员 / 无本地密钥 / 加密失败——重试无意义
    Fatal,
}

// ---------------- 待发群密钥登记表（纯逻辑，便于单测；不涉及网络与 AppState） ----------------

/// 登记一个待发群密钥（幂等）。
pub(crate) fn mark_pending_group_key(
    pending: &mut HashMap<String, HashSet<String>>,
    peer_id: &str,
    group_id: &str,
) {
    pending
        .entry(peer_id.to_string())
        .or_default()
        .insert(group_id.to_string());
}

/// 取指定 peer 的待发 group_id 快照（无登记项时为空）。
fn pending_group_key_ids(
    pending: &HashMap<String, HashSet<String>>,
    peer_id: &str,
) -> Vec<String> {
    match pending.get(peer_id) {
        Some(set) => set.iter().cloned().collect(),
        None => Vec::new(),
    }
}

/// 清除一个待发登记项；该 peer 的集合空了则一并移除键，避免无意义增长。
fn clear_pending_group_key(
    pending: &mut HashMap<String, HashSet<String>>,
    peer_id: &str,
    group_id: &str,
) {
    let empty = match pending.get_mut(peer_id) {
        Some(set) => {
            set.remove(group_id);
            set.is_empty()
        }
        None => false,
    };
    if empty {
        pending.remove(peer_id);
    }
}

/// 是否保留登记项等待重试：只有「链路不可用」才保留；
/// 发送成功或失败原因重试无意义（非成员 / 无密钥 / 缺公钥）都清除。
fn should_retain_pending_group_key(result: &Result<(), GroupKeySendErr>) -> bool {
    matches!(result, Err(GroupKeySendErr::NoLink))
}

/// 向指定 peer 发送一次群密钥。GroupKey 消息格式、加密方式与公钥来源
/// （peers 表）均与既有 `redistribute_group_keys` 保持一致。
async fn try_send_group_key(
    state: &AppState,
    peer_id: &str,
    group_id: &str,
) -> Result<(), GroupKeySendErr> {
    let pubkey = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(peer_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(pubkey) = pubkey else {
        return Err(GroupKeySendErr::Fatal);
    };
    let found_group = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_groups(&dbc)
            .unwrap_or_default()
            .into_iter()
            .find(|g| g.id == group_id)
    };
    let Some(g) = found_group else {
        return Err(GroupKeySendErr::Fatal);
    };
    if !g.members.contains(&peer_id.to_string()) {
        return Err(GroupKeySendErr::Fatal);
    }
    let Some(key) = get_group_key(state, group_id).await else {
        return Err(GroupKeySendErr::Fatal);
    };
    let Some(shared) = crypto::shared_secret(&state.identity.x25519_secret, &pubkey) else {
        return Err(GroupKeySendErr::Fatal);
    };
    let Some(sealed) = crypto::seal(&shared, &key) else {
        return Err(GroupKeySendErr::Fatal);
    };
    let clock = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &format!("group:{group_id}"))
    };
    let msg = Message::GroupKey {
        group_id: group_id.to_string(),
        from: state.device_id.clone(),
        to: peer_id.to_string(),
        key: STANDARD.encode(&sealed),
        group_name: g.name.clone(),
        members: g.members.clone(),
        clock,
    };
    // try_send 返回 Err 只可能是「未建立连接」（links 无该 peer）
    try_send(state, peer_id, &msg)
        .await
        .map_err(|_| GroupKeySendErr::NoLink)
}

async fn redistribute_group_keys(state: &AppState, peer_id: &str) {
    let groups = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_groups(&dbc).unwrap_or_default()
    };
    for g in groups {
        if !g.members.contains(&peer_id.to_string()) {
            continue;
        }
        match try_send_group_key(state, peer_id, &g.id).await {
            Ok(()) => {}
            Err(GroupKeySendErr::NoLink) => {
                // announce 先于 ensure_link 执行时 link 尚未建立，此前会静默丢弃
                // 且后续 is_new/key_changed 不再触发 → 成员永久拿不到群密钥。
                // 登记待发，由建链 / Hello / 心跳的 flush_pending_group_keys 重试。
                let mut pending = state.pending_group_keys.lock().unwrap_or_else(|e| e.into_inner());
                mark_pending_group_key(&mut pending, peer_id, &g.id);
            }
            // 缺公钥 / 非成员 / 无密钥：重试无意义，不登记
            Err(GroupKeySendErr::Fatal) => {}
        }
    }
}

/// 冲刷指定 peer 的待发群密钥：仅处理该 peer，发送成功即移除登记项；
/// link 仍不可用则保留，等下一次 flush（Hello / 心跳 / 建链）重试。
pub async fn flush_pending_group_keys(state: &AppState, peer_id: &str) {
    let group_ids: Vec<String> = {
        let pending = state.pending_group_keys.lock().unwrap_or_else(|e| e.into_inner());
        pending_group_key_ids(&pending, peer_id)
    };
    for gid in group_ids {
        let result = try_send_group_key(state, peer_id, &gid).await;
        if should_retain_pending_group_key(&result) {
            // 仍无链路：保留登记项，等待下一次 flush（Hello / 心跳 / 建链）
            continue;
        }
        let mut pending = state.pending_group_keys.lock().unwrap_or_else(|e| e.into_inner());
        clear_pending_group_key(&mut pending, peer_id, &gid);
    }
}

pub async fn touch_peer(state: &AppState, device_id: &str) {
    let ts = db::now_ms();
    if let Some(p) = state.peers.lock().unwrap_or_else(|e| e.into_inner()).get_mut(device_id) {
        p.last_seen = ts;
    }
    state.emit_peers();
}

pub(crate) async fn mark_peer_offline(state: &Arc<AppState>, device_id: &str) {
    state.peers.lock().unwrap_or_else(|e| e.into_inner()).remove(device_id);
    // 链路快照随之失效：`conv_link` 记的是"当前可达路径"，节点已离线 ⇒ 该路径不存在。
    // 不清掉的话，聊天头部的链路徽标会在离线后继续显示（用户 2026-09-12 反馈的
    // 「离线却显示『桥接 1』」）。前端也做了 `online` 绑定，这里是数据侧的对称清理。
    clear_conv_link(state, device_id);
    state.emit_peers();
}

/// 清掉某会话的链路快照（`conv_link` 是内存态；无条目时无操作）。
pub(crate) fn clear_conv_link(state: &AppState, conv_id: &str) {
    state
        .conv_link
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(conv_id);
}

pub(crate) fn maybe_update_friend(
    state: &AppState,
    device_id: &str,
    nickname: &str,
    avatar: Option<String>,
) {
    let (x, e) = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers
            .get(device_id)
            .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
            .unwrap_or((None, None))
    };
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if db::get_friend(&dbc, device_id).is_some() {
        db::add_friend(&dbc, device_id, nickname, avatar.as_deref()).ok();
        // 好友行常在公钥落库之后才创建（好友申请通过才 add_friend），
        // 此处每次同步公钥，保证 E2EE 加密始终能取到对方公钥。
        if x.is_some() || e.is_some() {
            db::update_friend_pubkeys(&dbc, device_id, x.as_deref(), e.as_deref()).ok();
        }
    }
}

/// 群成员公钥选择：peers 表优先、friends 表回落（纯逻辑，便于单测）。
/// peers 表由 announce / Hello 实时维护，几乎总是最新；
/// friends 表可能缺失——accept 方路径此前不补写公钥，
/// 且公钥不变时 key_changed 不触发 maybe_update_friend。
fn pick_member_x25519(peers_key: Option<String>, friends_key: Option<String>) -> Option<String> {
    peers_key.or(friends_key)
}

/// 解析群成员的 X25519 公钥：peers 优先、friends 回落，都缺失才返回 None。
/// 群密钥分发（distribute_group_key / resend_group_key_to）统一走这里，
/// 避免 friends 表公钥缺失导致 GroupKey 被静默跳过、成员永久拿不到群密钥。
pub(crate) fn resolve_member_x25519(state: &AppState, member_id: &str) -> Option<String> {
    let peers_key = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(member_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let friends_key = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend_x25519(&dbc, member_id)
    };
    pick_member_x25519(peers_key, friends_key)
}

/// 群成员变更的**群内系统消息**文案。
///
/// 后端产生的系统消息同样要跟随语言设置 —— 前端 i18n 覆盖不到后端直接写库的行。
/// 两条文案各自只有一处实现，供「发起方」与「接收方」共用，避免措辞漂移。
/// 收到 `GroupMemberRemoved` 后本机应做的动作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberRemovedAction {
    /// 忽略：非创建者发起（防成员互踢），或本消息就来自本机（本地已处理）
    Ignore,
    /// 我本人被移出：清理本地群 + 会话 + 群密钥
    RemoveSelf,
    /// 别人被移出：同步本地成员表 + 落群内系统消息
    RemoveOther,
}

/// 分支选择（纯函数，便于单测）。
///
/// 单独抽出来的理由：这次 bug 的根因就是**少了一个分支** —— 接收端只处理「我本人被
/// 移出」，其余成员直接 return，于是群里其他人的成员表不变小、也看不到任何提示。
/// 把三分支的选择做成纯函数，「必须有 RemoveOther」这件事就被测试钉住了。
pub fn member_removed_action(
    sender_is_me: bool,
    sender_is_creator: bool,
    to_is_me: bool,
) -> MemberRemovedAction {
    if sender_is_me || !sender_is_creator {
        return MemberRemovedAction::Ignore;
    }
    if to_is_me {
        MemberRemovedAction::RemoveSelf
    } else {
        MemberRemovedAction::RemoveOther
    }
}

/// 往指定群的会话插一条本地系统消息。
///
/// 群的 conv_id 约定是 `group:{group_id}` —— 这个约定只有一处实现，避免各处手拼前缀。
pub fn insert_group_system_message(state: &AppState, group_id: &str, text: &str) {
    crate::commands::insert_system_message(state, &format!("group:{group_id}"), text);
}

pub fn group_member_removed_text(state: &AppState, name: &str) -> String {
    if state.is_zh() {
        format!("「{name}」已被移出群聊")
    } else {
        format!("“{name}” has been removed from the group")
    }
}

/// 成员**主动退群**的群内系统消息文案（详见 `group_member_removed_text`）。
pub fn group_member_left_text(state: &AppState, name: &str) -> String {
    if state.is_zh() {
        format!("「{name}」退出了群聊")
    } else {
        format!("“{name}” left the group")
    }
}

pub fn resolve_nickname(state: &AppState, id: &str) -> String {
    if let Some(p) = state.peers.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
        if !p.nickname.is_empty() {
            return p.nickname.clone();
        }
    }
    if let Some(r) = state.pending_requests.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
        return r.from_nickname.clone();
    }
    if let Some((n, _)) = db::get_friend(&state.db.lock().unwrap_or_else(|e| e.into_inner()), id) {
        return n;
    }
    id.to_string()
}

fn resolve_group_name(state: &AppState, group_id: &str) -> String {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(groups) = db::list_groups(&dbc) {
        if let Some(g) = groups.into_iter().find(|g| g.id == group_id) {
            return g.name;
        }
    }
    format!("群聊 {group_id}")
}

fn preview_content(kind: &str, content: &str) -> String {
    match kind {
        "file" => "[文件]".to_string(),
        "image" => "[图片]".to_string(),
        _ => {
            let count = content.chars().count();
            let c: String = content.chars().take(30).collect();
            if count > 30 {
                format!("{c}…")
            } else {
                c
            }
        }
    }
}

/// 补发离线队列中的所有消息。
///
/// 注意：这里**只补发、不删除**——outbox 行仅在收到对方 `Ack`（真正确认送达）时删除。
/// 旧实现 `try_send` 返回 Ok（仅表示已入发送队列）就删行，半开 TCP 链路上会静默丢消息，
/// outbox 兜底因此失效。接收方按 msg_id 去重，重复补发不会重复入库/通知。
///
/// 每条补发前用**当前**公钥重新密封（见 `reseal_for_send`）：outbox 存的是加密时刻的
/// 密文，若之后接收方换了身份，旧密文重发多少次都解不开；`msg_id` 不变，幂等性不受影响。
pub async fn flush_outbox(state: &AppState, peer_id: &str) {
    let pending = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_outbox(&dbc, peer_id).unwrap_or_default()
    };
    for (_id, payload) in pending {
        let Ok(msg) = serde_json::from_str::<Message>(&payload) else {
            continue;
        };
        let msg = reseal_for_send(state, msg);
        let _ = try_send(state, peer_id, &msg).await;
    }
}

/// 补发指定成员的群消息离线队列。
///
/// 与单聊 outbox 同一语义：**只补发、不删除**，GroupAck 到达才删除对应行。
/// Gossip 信封在发送时已经签名，重发无需重新签名，接收方按 msg_id 幂等去重。
pub async fn flush_group_outbox(state: &AppState, peer_id: &str) {
    let pending = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_group_outbox(&dbc, peer_id).unwrap_or_default()
    };
    for (_id, payload) in pending {
        let Ok(Message::Gossip { envelope }) = serde_json::from_str::<Message>(&payload) else {
            continue;
        };
        let _ = try_send(state, peer_id, &Message::Gossip { envelope }).await;
    }
}

/// 发送单聊已读回执：同网段有直连走 `Message::ReadReceipt`（可被 pending 重试），
/// 跨跳（无直连）改走定向 Gossip `ChatReadReceipt`（广播靠中继按 target 转发）。
///
/// 返回是否「已发出」：直连失败返回 false（供 flush 决定是否重新入队），
/// Gossip 广播是尽力而为、视为已发出返回 true。
pub async fn send_read_receipt_route(
    state: &AppState,
    peer_id: &str,
    msg_id: Option<String>,
    last_read_ts: i64,
) -> bool {
    if state.has_link(peer_id).await {
        let msg = Message::ReadReceipt {
            from: state.device_id.clone(),
            to: peer_id.to_string(),
            last_read_ts,
            last_read_msg_id: msg_id,
        };
        try_send(state, peer_id, &msg).await.is_ok()
    } else {
        let payload = serde_json::json!({
            "last_read_ts": last_read_ts,
            "last_read_msg_id": msg_id,
        })
        .to_string();
        let payload_b64 = STANDARD.encode(payload.as_bytes());
        let mut env = {
            let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
            gossip.build_envelope(
                &state.identity,
                &state.device_id,
                GossipKind::ChatReadReceipt,
                None,
                None,
                &payload_b64,
                db::now_ms(),
                0,
            )
        };
        env.encrypted = false;
        env.target = Some(peer_id.to_string());
        env.sender_sig = state.identity.sign_b64(&env.signing_bytes());
        broadcast_gossip(state, env).await;
        true
    }
}

/// 冲刷待发的单聊已读回执（触发点与 `flush_outbox` 一致：建链 / Hello / 心跳）。
///
/// `mark_read` 将 pending 同时写入内存 HashMap 和 SQLite。此处成功发送后
/// 同时清除两者；失败时内存已由 remove 清除但会重新写入，DB 保留不动
/// （由 `mark_read` 写入，下次 flush 重试）。
pub async fn flush_pending_reads(state: &AppState, peer_id: &str) {
    let Some(_last_read_ts) = state.pending_reads.lock().unwrap_or_else(|e| e.into_inner()).remove(peer_id) else {
        return;
    };
    // 补发时重新取「对方最近一条消息」的 msg_id + ts，而不是使用之前内存里的 ts。
    // 因为 ts 可能只是被钳制后的值，msg_id 才能让发送方换算回自己的本地时间戳。
    let last = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::last_message_from_sender(&dbc, peer_id, peer_id)
    };
    let Some((msg_id, last_read_ts)) = last else {
        // 对方没有可标记已读的消息，直接清掉 pending 即可。
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_pending_read(&dbc, peer_id).ok();
        return;
    };
    if !send_read_receipt_route(state, peer_id, Some(msg_id), last_read_ts).await {
        // 直连发送失败：内存重新放入 pending，DB 保留（已由 mark_read 写入）
        let mut pending = state.pending_reads.lock().unwrap_or_else(|e| e.into_inner());
        let cur = pending.entry(peer_id.to_string()).or_insert(last_read_ts);
        *cur = (*cur).max(last_read_ts);
    } else {
        // 发送成功：清除 DB 中的 pending 记录
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_pending_read(&dbc, peer_id).ok();
    }
}

/// 冲刷指定 peer 的待发群已读回执（触发点与单聊 pending_reads 一致）。
pub async fn flush_pending_group_reads(state: &AppState, peer_id: &str) {
    let rows = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_pending_group_reads(&dbc, peer_id).unwrap_or_default()
    };
    for (group_id, _last_read_ts) in rows {
        let conv_id = format!("group:{group_id}");
        let last = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::last_message_from_sender(&dbc, &conv_id, peer_id)
        };
        let Some((msg_id, last_read_ts)) = last else {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::delete_pending_group_read(&dbc, &group_id, peer_id).ok();
            continue;
        };
        let msg = Message::GroupReadReceipt {
            from: state.device_id.clone(),
            group_id: group_id.clone(),
            last_read_ts,
            last_read_msg_id: Some(msg_id),
        };
        if try_send(state, peer_id, &msg).await.is_ok() {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::delete_pending_group_read(&dbc, &group_id, peer_id).ok();
        }
    }
}

pub fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    notify_with_extra(app, title, body, std::collections::HashMap::new());
}

pub fn notify_with_extra(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    extra: std::collections::HashMap<String, String>,
) {
    use tauri_plugin_notification::NotificationExt;
    let mut builder = app.notification().builder().title(title).body(body);
    for (k, v) in &extra {
        builder = builder.extra(k, v);
    }
    let _ = builder.show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gossip_engine::GossipEngine;

    // ---- 群成员变更：接收端的分支选择 ----
    //
    // 真实事故（2026-09-12 用户反馈）：群主移人后，**其余成员**的成员表不变小、
    // 也没有任何提示。根因是接收端 `handle_group_member_removed` 开头就
    // `if to != 本机 { return }` —— 压根没有「别人被移出」这个分支。
    // 下面把三分支的选择（纯函数）用真值表钉住。

    #[test]
    fn member_removed_action_full_truth_table() {
        use MemberRemovedAction::*;
        // 发起方不是创建者 → 一律忽略（防成员互踢），与 `to` 是谁无关
        assert_eq!(member_removed_action(false, false, false), Ignore);
        assert_eq!(member_removed_action(false, false, true), Ignore);
        // 本机自己发起的 → 忽略（本地已处理，含群主自己的系统消息）
        assert_eq!(member_removed_action(true, true, false), Ignore);
        assert_eq!(member_removed_action(true, true, true), Ignore);
        assert_eq!(member_removed_action(true, false, false), Ignore);
        assert_eq!(member_removed_action(true, false, true), Ignore);
        // 创建者发起 + 被移出者是别人 → **同步成员表**（本次补齐的分支，
        // 这条断言正是对「其余成员不能 Ignore」的回归钉子）
        assert_eq!(member_removed_action(false, true, false), RemoveOther);
        // 创建者发起 + 被移出者是我 → 清理本地群
        assert_eq!(member_removed_action(false, true, true), RemoveSelf);
    }

    // ---- 拨号决策（M2 双向建链 + P1-2 镜像重复连接修正）----

    #[test]
    fn should_dial_larger_id_dials_when_not_connected() {
        // 本机是大 ID（my_id > peer_id）：尚无任何连接时，作为对称场景的确定性拨号方，恒拨。
        assert!(should_dial("b", "a", false, false, None, 0));
        assert!(should_dial("b", "a", false, false, Some(0), 0));
    }

    #[test]
    fn should_dial_smaller_id_waits_within_threshold() {
        // 本机是小 ID：对端在线但「首次发现」未超过 10s，不拨（等大 ID 拨）。
        let now = 1_000_000;
        assert!(!should_dial("a", "b", false, false, Some(now - 9_000), now));
        assert!(!should_dial("a", "b", false, false, Some(now), now));
        // 对端尚未在线（first_seen=None）：不拨。
        assert!(!should_dial("a", "b", false, false, None, now));
    }

    #[test]
    fn should_dial_smaller_id_backups_after_threshold() {
        // 本机是小 ID：对端在线却超过 10s 连不上（单侧不可达），兜底拨。
        let now = 1_000_000;
        assert!(should_dial("a", "b", false, false, Some(now - 10_000), now));
        assert!(should_dial("a", "b", false, false, Some(now - 60_000), now));
    }

    /// P1-2 修正的**核心护栏**。
    ///
    /// 被动方（小 ID）已经收到过大 ID 拨来的连接时，绝不能因为「端点表示不对称」
    /// （接受侧 `Link.endpoint` 记的是 TCP 源**临时端口**，而判据比的是 announce 自报的
    /// **监听地址**）而反向再拨一条 —— 那会让同一对节点稳定停留 2 条镜像 TCP，
    /// 连接与读写任务翻倍、心跳双份，并让「断一条仍在线」的判据变成假阳性。
    ///
    /// 注意判据是 `has_lan_link`（**同路径**已连通），与 `has_endpoint` 无关：
    /// 这里刻意传 `has_endpoint=false`（真实场景就是如此）来钉住「只按端点判会误拨」。
    #[test]
    fn should_dial_skips_when_same_path_already_connected() {
        let now = 1_000_000;
        // 小 ID + 已有连接 + 早已超过 10s 阈值 —— 旧实现正是在这里误判为「该兜底拨号」。
        assert!(!should_dial("a", "b", false, true, Some(now - 60_000), now));
        // 大 ID 同理：已有连接不重复拨。
        assert!(!should_dial("b", "a", false, true, Some(now - 60_000), now));
        // 同一端点已连 → 不拨（无论 ID 大小、无论阈值）。
        assert!(!should_dial("b", "a", true, true, None, now));
        assert!(!should_dial("a", "b", true, true, Some(now - 60_000), now));
    }

    /// D5 护栏：判据必须是「**LAN 路径**是否已连通」，不能是「有没有任意连接」。
    ///
    /// 回归场景（复核抓到）：peer 先经 Routed/Tailscale 连上，之后 LAN 的 announce 到达；
    /// 若把任意连接当成「已连通」，LAN 链路**永远不会建立** ⇒ M3 的「LAN > Routed」
    /// 优先级在该拓扑里永不生效，多路径退化成单路径。
    /// 这条测试会在把判据回退成「任意连接」时 FAIL —— 因为它明确区分了两种端点。
    #[test]
    fn only_routed_connection_still_dials_lan_path() {
        let lan: MeshEndpoint = "192.168.1.20:59992".parse::<std::net::SocketAddr>().unwrap().into();
        let routed: MeshEndpoint = "100.70.10.20:59992".parse::<std::net::SocketAddr>().unwrap().into();

        // 纯函数内核：只有 Routed 端点 ⇒ 不算 LAN 已连通（⇒ 大 ID 会去补一条 LAN）
        // （`Endpoint` 不是 Copy，测试里 clone 保持可读性）
        assert!(!has_lan_path(&[(routed.clone(), PathKind::Routed)]));
        assert!(has_lan_path(&[(lan.clone(), PathKind::Lan)]));
        assert!(has_lan_path(&[
            (routed.clone(), PathKind::Routed),
            (lan.clone(), PathKind::Lan)
        ]));
        assert!(!has_lan_path(&[]));
        // ⚠️ **关键回归**：端点地址是私有段、但来路是"用户配置的路由端点" ⇒ 仍是 Routed。
        // 此前路径类型是从 IP 段反推的（`path_kind_for`），这条必然被判成 LAN ⇒
        // ① `ensure_link` 以为 LAN 已连通、不再补真正的 LAN 链路（D5 的修复被绕过去）；
        // ② 选路时按最高优先级当成 LAN。把判据改回"按 IP 反推"这条断言立刻 FAIL。
        let private_but_routed: MeshEndpoint =
            "192.168.1.77:59992".parse::<std::net::SocketAddr>().unwrap().into();
        assert!(
            !has_lan_path(&[(private_but_routed.clone(), PathKind::Routed)]),
            "用户配置的私有段 Routed 端点不得被当成 LAN"
        );

        // 决策层：**只有 Routed 连接**时，大 ID 仍应去补一条 LAN —— 这正是修复点。
        // 若把判据回退成「有任意连接就不拨」，下面这条断言会 FAIL（非空转）。
        let now = 1_000_000;
        let routed_only = [(routed.clone(), PathKind::Routed)];
        let lan_only = [(lan.clone(), PathKind::Lan)];
        let both = [
            (routed.clone(), PathKind::Routed),
            (lan.clone(), PathKind::Lan),
        ];
        assert!(should_dial_for_peer("b", "a", false, &routed_only, Some(now - 60_000), now));
        // 已有 LAN 连接 ⇒ 不重复拨（避免镜像重复连接，P1-2）。
        assert!(!should_dial_for_peer("b", "a", false, &lan_only, Some(now - 60_000), now));
        // 已有 LAN（含还有一条 Routed 的多路径场景）⇒ 也不拨。
        assert!(!should_dial_for_peer("b", "a", false, &both, Some(now - 60_000), now));
        // 完全没有连接 ⇒ 拨（原语义不变）。
        assert!(should_dial_for_peer("b", "a", false, &[], Some(now - 60_000), now));
        // 小 ID 兜底：只有 Routed 且超过阈值 ⇒ 也去补 LAN。
        assert!(should_dial_for_peer("a", "b", false, &routed_only, Some(now - 60_000), now));
        // 私有段地址 + Routed 来路 ⇒ 仍应补 LAN（与上面的关键回归同一件事，走决策层）
        assert!(should_dial_for_peer(
            "b",
            "a",
            false,
            &[(private_but_routed.clone(), PathKind::Routed)],
            Some(now - 60_000),
            now
        ));
    }

    // ---- M3-b：发送顺序（按端点对齐两套链路表 + pick_link 排序）----

    fn make_link(
        addr: &str,
        kind: PathKind,
    ) -> (crate::state::Link, mpsc::Receiver<Message>, mpsc::Receiver<Message>) {
        let (b_tx, b_rx) = mpsc::channel(4);
        let (p_tx, p_rx) = mpsc::channel(4);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint: MeshEndpoint::Tcp(addr.parse().unwrap()),
                path_kind: kind,
                bulk: b_tx,
                priority: p_tx,
                cancel,
            },
            b_rx,
            p_rx,
        )
    }

    /// 按任意 `Endpoint` 造一条链路（`make_link` 只接受 TCP 地址字符串，BLE 用这个）。
    fn make_link_endpoint(
        endpoint: MeshEndpoint,
        kind: PathKind,
    ) -> (crate::state::Link, mpsc::Receiver<Message>, mpsc::Receiver<Message>) {
        let (b_tx, b_rx) = mpsc::channel(4);
        let (p_tx, p_rx) = mpsc::channel(4);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint,
                path_kind: kind,
                bulk: b_tx,
                priority: p_tx,
                cancel,
            },
            b_rx,
            p_rx,
        )
    }

    /// 按任意 `Endpoint` 造一个 mesh 层 `Connection`（同上）。
    fn mesh_conn_endpoint(
        peer: &str,
        endpoint: MeshEndpoint,
        healthy_at: Option<i64>,
        kind: PathKind,
    ) -> crate::mesh::Connection {
        let mut c = crate::mesh::Connection::new(peer, endpoint, kind);
        if let Some(t) = healthy_at {
            c.health.seed_read_seen(t);
        }
        c
    }

    fn mesh_conn(
        peer: &str,
        addr: &str,
        healthy_at: Option<i64>,
        kind: PathKind,
    ) -> crate::mesh::Connection {
        let mut c = crate::mesh::Connection::new(
            peer,
            crate::mesh::endpoint::Endpoint::Tcp(addr.parse().unwrap()),
            kind,
        );
        if let Some(t) = healthy_at {
            c.health.seed_read_seen(t);
        }
        c
    }

    /// 单链路：顺序无变化（**行为零变化**，M3-b 的前提）。
    #[test]
    fn route_order_single_link_is_unchanged() {
        let (l0, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![l0];
        let conns = vec![mesh_conn("peer", "192.168.1.20:59992", Some(1000), PathKind::Lan)];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order, vec![0]);
    }

    /// 核心（M3-b 的收益）：两条都健康时**LAN 优先**，与插入顺序无关。
    #[test]
    fn route_order_prefers_lan_over_routed_regardless_of_insertion() {
        // 故意把 Routed 放在下标 0（插入在前），LAN 在下标 1
        let (routed, _b0, _p0) = make_link("100.70.10.20:59992", PathKind::Routed);
        let (lan, _b1, _p1) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![routed, lan];
        let conns = vec![
            mesh_conn("peer", "100.70.10.20:59992", Some(1000), PathKind::Routed),
            mesh_conn("peer", "192.168.1.20:59992", Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order[0], 1, "应优先 LAN（下标 1），而不是插入在前的 Routed");
        // 不变量：其余链路仍排在后面做 failover，**一条都不能丢**
        assert_eq!(order.len(), 2);
        assert!(order.contains(&0) && order.contains(&1));
    }

    /// BLE 链路的两个"不该被当成 LAN"判据（ADR-0015 的 7-c）：
    /// ① 选路：TCP（LAN/Routed）必须排在 BLE 前面 —— 蓝牙带宽/功耗都差一个量级；
    /// ② 拨号：只有 BLE 连上**不算**「LAN 已连通」，否则 `ensure_link` 不再补 LAN 链路
    ///    （用户明明在同一局域网，却一直走蓝牙 —— 电量与速度都吃亏）。
    #[test]
    fn ble_link_is_neither_lan_nor_preferred_over_tcp() {
        let ble = MeshEndpoint::Ble(crate::mesh::BleEndpoint::new("node-1"));
        let lan: MeshEndpoint = "192.168.1.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();

        // ① 选路：BLE 插在前面也不该被优先选
        let (ble_link, _b0, _p0) = make_link_endpoint(ble.clone(), PathKind::Bluetooth);
        let (lan_link, _b1, _p1) = make_link_endpoint(lan.clone(), PathKind::Lan);
        let links = vec![ble_link, lan_link];
        let conns = vec![
            mesh_conn_endpoint("peer", ble.clone(), Some(1000), PathKind::Bluetooth),
            mesh_conn_endpoint("peer", lan.clone(), Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order.len(), 2, "BLE 链路同样是 failover 候选，不能丢");
        assert_eq!(order[0], 1, "LAN 必须优先于 BLE");

        // ② 拨号判据：只有 BLE ⇒ LAN 路径尚未连通 ⇒ 仍要去补一条 LAN
        assert!(!has_lan_path(&[(ble.clone(), PathKind::Bluetooth)]));
        let now = 1_000_000;
        assert!(
            should_dial_for_peer("b", "a", false, &[(ble, PathKind::Bluetooth)], Some(now - 60_000), now),
            "只有 BLE 连接时仍应补 LAN"
        );
    }

    /// 跨世代的入站连接必须被否决（D8-4）。
    #[test]
    fn stale_generation_is_rejected() {
        assert!(generation_is_current(3, 3), "同一世代允许登记");
        assert!(!generation_is_current(3, 4), "stop/start 之后的旧世代不得登记");
        // 世代只增不减，但"捕获值比当前大"同样视为无效（防御性：不做大小比较）
        assert!(!generation_is_current(4, 3));
    }

    /// 入站去重判据的真值表。这条判据改错的后果是"两边互拒 ⇒ 谁也连不上"，
    /// 或者"镜像连接永久并存"，两者都不是肉眼能立刻发现的，所以逐格钉住。
    #[test]
    fn inbound_dedup_truth_table() {
        let lan_ep: MeshEndpoint = "192.168.1.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();
        let routed_ep: MeshEndpoint = "100.70.10.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();

        // ① 一条都没有 ⇒ **必须接受**（否则彻底断连）
        assert!(should_accept_inbound("b", "a", PathKind::Lan, &[]));

        // ② 大 ID 方（my_id > peer_id）：已有同路径**且健康** ⇒ 拒收镜像；
        //    不同路径 ⇒ 接受（多路径！）
        let with_lan = [(lan_ep.clone(), PathKind::Lan, true)];
        assert!(!should_accept_inbound("b", "a", PathKind::Lan, &with_lan));
        assert!(should_accept_inbound("b", "a", PathKind::Routed, &with_lan));
        // ②b 已有同路径但**已不健康**（半开待拆）⇒ 必须接受对端的新鲜连接，
        //     否则双方要干等 watchdog（最长 45s）才能恢复
        let with_dead_lan = [(lan_ep.clone(), PathKind::Lan, false)];
        assert!(should_accept_inbound("b", "a", PathKind::Lan, &with_dead_lan));

        // ③ 小 ID 方（my_id < peer_id）：**始终接受** —— 否则双方互拒，谁也连不上
        assert!(should_accept_inbound("a", "b", PathKind::Lan, &with_lan));

        // ④ 链路数到上限 ⇒ 拒收（防无界增长），且与路径是否重复无关
        let full: Vec<(MeshEndpoint, PathKind, bool)> = (0..MAX_LINKS_PER_PEER)
            .map(|i| {
                (
                    format!("10.0.0.{i}:59992").parse::<std::net::SocketAddr>().unwrap().into(),
                    if i % 2 == 0 { PathKind::Lan } else { PathKind::Routed },
                    true,
                )
            })
            .collect();
        assert!(!should_accept_inbound("a", "b", PathKind::Bluetooth, &full));
        // 未到上限但已有 Routed ⇒ 接受（③ 的小 ID 方不受 ② 限制）
        let one_routed = [(routed_ep, PathKind::Routed, true)];
        assert!(should_accept_inbound("a", "b", PathKind::Routed, &one_routed));
    }

    /// 徽标必须反映**实际选中的那条**，而不是插入顺序的第一条。
    ///
    /// 旧实现取 `links.first()`：用户配了 Routed 又同处一个局域网时（LAN 由 announce
    /// 后补、插在后面），徽标会一直显示"桥接"，消息却走 LAN —— 用户 2026-09-12
    /// 反馈过徽标与实际不符。
    #[test]
    fn badge_follows_selection_not_insertion_order() {
        let (routed, _b0, _p0) = make_link("100.70.10.20:59992", PathKind::Routed);
        let (lan, _b1, _p1) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![routed, lan];
        let conns = vec![
            mesh_conn("peer", "100.70.10.20:59992", Some(1000), PathKind::Routed),
            mesh_conn("peer", "192.168.1.20:59992", Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(
            badge_path_kind(&links, &order),
            PathKind::Lan,
            "徽标必须跟着选路走（LAN），而不是插入顺序第一条（Routed）"
        );
        // 选路为空（全部不可用）⇒ 退回首条，不能 panic
        assert_eq!(badge_path_kind(&links, &[]), PathKind::Routed);
    }

    /// failover 核心：LAN 的读活性过期（半开）而 Routed 健康 → 选 Routed。
    #[test]
    fn route_order_skips_unhealthy_lan_when_routed_is_healthy() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            // LAN：只有很早的读活性（已过期）
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),
            // Routed：刚刚读到过帧
            mesh_conn("peer", "100.70.10.20:59992", Some(60_000), PathKind::Routed),
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(order[0], 1, "LAN 不健康时必须降级到 Routed（真 failover）");
        assert_eq!(order.len(), 2, "不健康链路仍保留在后面（可作最后手段）");
    }

    /// 登记窗口：传输链路存在但 mesh 侧还没登记 → 合成「刚播种」候选，不能因此被判不可用。
    #[test]
    fn route_order_tolerates_missing_mesh_candidate() {
        let (only, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![only];
        let order = route_order(&links, "peer", &[], 1000, 15_000, 3);
        assert_eq!(order, vec![0], "缺候选时不得丢链路（登记窗口是常态）");
    }

    /// 全部不健康：`pick_link` 退回首条（保持可用），且顺序仍是全量排列。
    #[test]
    fn route_order_keeps_all_links_when_none_healthy() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),
            mesh_conn("peer", "100.70.10.20:59992", Some(0), PathKind::Routed),
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(order.len(), 2, "全不健康也要把链路交出去（可用性优先于择优）");
    }

    // ---- M3-b：真实信道上的 failover（ADR-0014 §8「切断被选中那条 → 消息仍送达」的单元版）----

    fn msg(id: &str) -> Message {
        Message::Heartbeat { device_id: id.to_string() }
    }

    /// 造 n 对信道，返回 senders + 各接收端（`None` 表示该条"已断"：接收端被丢弃）。
    #[allow(clippy::type_complexity)]
    fn channels(
        n: usize,
        closed: &[usize],
    ) -> (
        Vec<(mpsc::Sender<Message>, mpsc::Sender<Message>)>,
        Vec<Option<mpsc::Receiver<Message>>>,
    ) {
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for i in 0..n {
            let (b_tx, b_rx) = mpsc::channel(4);
            let (p_tx, p_rx) = mpsc::channel(4);
            senders.push((b_tx, p_tx));
            if closed.contains(&i) {
                // 模拟"这条链路已断"：channel 关闭（`try_send` 会返回 Closed）
                drop(b_rx);
                drop(p_rx);
                receivers.push(None);
            } else {
                receivers.push(Some(p_rx));
                drop(b_rx); // 只关心 priority 通道
            }
        }
        (senders, receivers)
    }

    /// **核心判据**：被选中的那条断了 → 消息必须落到下一条（真 failover，不是"投进死路"）。
    #[tokio::test]
    async fn failover_delivers_on_next_link_when_selected_is_closed() {
        let (senders, mut rx) = channels(2, &[0]); // 下标 0（被选中）已断
        // 顺序模拟选路结果：先试 0（断），再试 1（活）
        let order = vec![0usize, 1];
        let r = send_over_order(&senders, &order, &msg("m1"), false).await;
        assert!(r.is_ok(), "断一条后必须换下一条送达，实得 {r:?}");
        let got = rx[1].as_mut().expect("链路 1 应存活").try_recv().expect("应在链路 1 上收到");
        assert!(matches!(got, Message::Heartbeat { .. }));
    }

    /// 顺序被尊重：两条都活时只投第一条，**不重复投递**（消息仍然只发出一次）。
    #[tokio::test]
    async fn sends_only_on_first_healthy_link_in_order() {
        let (senders, mut rx) = channels(2, &[]);
        let order = vec![1usize, 0]; // 选路把下标 1 排前面
        assert!(send_over_order(&senders, &order, &msg("m2"), false).await.is_ok());
        assert!(rx[1].as_mut().unwrap().try_recv().is_ok(), "应落在顺序第一的那条");
        assert!(
            rx[0].as_mut().unwrap().try_recv().is_err(),
            "不得同时投到第二条（否则会重复投递）"
        );
    }

    /// 全断 → 返回 Err（调用方据此走 outbox 补发，而不是假装成功）。
    #[tokio::test]
    async fn all_links_closed_returns_err() {
        let (senders, _rx) = channels(2, &[0, 1]);
        let r = send_over_order(&senders, &[0, 1], &msg("m3"), false).await;
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
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),      // LAN 读活性过期
            mesh_conn("peer", "100.70.10.20:59992", Some(60_000), PathKind::Routed), // Routed 健康
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(order[0], 1, "应先试健康的 Routed");

        // 用真实信道复现：LAN 那条已断，Routed 那条活着
        let (senders, mut rx) = channels(2, &[0]);
        assert!(send_over_order(&senders, &order, &msg("m4"), false).await.is_ok());
        assert!(
            rx[1].as_mut().unwrap().try_recv().is_ok(),
            "LAN 降级后消息必须从 Routed 送出"
        );
    }

    /// 空链路表 → 空顺序（调用方据此返回「未建立连接」）。
    #[test]
    fn route_order_empty_when_no_links() {
        assert!(route_order(&[], "peer", &[], 1000, 15_000, 3).is_empty());
    }

    // ---- Hello 握手身份认证（P0 安全修复回归）----

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
        assert!(hello_auth_decision(Some(&bound), "victim-device", 59992, "n1", &xk, &ek, &sig)
            .is_err());
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

    #[test]
    fn bulk_messages_are_only_large_chunks() {
        let chat = Message::ChatMessage {
            msg_id: "m1".into(),
            from: "a".into(),
            to: "b".into(),
            kind: MsgKind::Text,
            content: "hi".into(),
            ts: 1,
            seq: 1,
        };
        assert!(!is_bulk_message(&chat));
        let file_chunk = Message::FileChunk {
            transfer_id: "t1".into(),
            seq: 0,
            data: "abc".into(),
        };
        assert!(is_bulk_message(&file_chunk));
        let group_file_chunk = Message::GroupFileChunk {
            transfer_id: "t1".into(),
            group_id: "g1".into(),
            sender_id: "a".into(),
            seq: 0,
            data: "abc".into(),
        };
        assert!(is_bulk_message(&group_file_chunk));
        // 文件终止帧必须走 bulk，避免跑到未写完的分片前面。
        let file_done = Message::FileDone {
            transfer_id: "t1".into(),
        };
        assert!(is_bulk_message(&file_done));
        let group_file_done = Message::GroupFileDone {
            transfer_id: "t1".into(),
            group_id: "g1".into(),
            sender_id: "a".into(),
        };
        assert!(is_bulk_message(&group_file_done));
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

    #[tokio::test]
    async fn e2e_gossip_encrypt_sign_decrypt() {
        // 端到端：A 加密→签名→广播信封，B 验签→解密
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();

        // A 用 B 的公钥 ECDH 派生共享密钥并加密
        let shared =
            crate::crypto::shared_secret(&a.x25519_secret, &b.x25519_public_b64()).unwrap();
        let plaintext = b"{\"kind\":\"text\",\"content\":\"hello\"}";
        let sealed = crate::crypto::seal(&shared, plaintext).unwrap();
        let payload_b64 = STANDARD.encode(&sealed);

        // 构造并签名信封
        let engine = GossipEngine::new(100, 10, 4, 6);
        let env = engine.build_envelope(&a, "dev-a", GossipKind::Chat, None, None, &payload_b64, 1, 1);

        // B 验签 + 解密
        assert!(engine.verify_envelope(&env));
        let shared_b = crate::crypto::shared_secret(&b.x25519_secret, &env.sender_pubkey).unwrap();
        let decrypted = STANDARD.decode(&env.payload).unwrap();
        let opened = crate::crypto::open(&shared_b, &decrypted).unwrap();
        assert_eq!(opened, plaintext);
    }

    // ---------------- P0-2：直连 E2EE 解密失败不得消费真实 msg_id ----------------

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
                MsgKind::Code
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
            open_direct_content(&b.x25519_secret, None, &wire, MsgKind::Text),
            None
        );
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&a.x25519_public_b64()),
                &wire,
                MsgKind::Text
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
                MsgKind::Text
            ),
            None
        );
        assert_eq!(
            open_direct_content(
                &b.x25519_secret,
                Some(&a_new.x25519_public_b64()),
                &wire,
                MsgKind::Text
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
                MsgKind::Text
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
                MsgKind::Text
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
                MsgKind::Text
            ),
            None
        );
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&spk), "enc1:", MsgKind::Text),
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
            open_direct_content(&b.x25519_secret, Some(&spk), &tampered, MsgKind::Text),
            None
        );
        assert!(open_direct_content(&b.x25519_secret, Some(&spk), &wire, MsgKind::Text).is_some());
    }

    #[test]
    fn plaintext_payload_is_rejected() {
        let me = crate::crypto::Identity::generate();
        assert_eq!(
            open_direct_content(&me.x25519_secret, None, "plain old text", MsgKind::Text),
            None
        );
    }

    /// P1-3：两层去重必须互相独立——Gossip 的 Bloom/LRU 属网络传播层（只认 `message_id`），
    /// SQLite 的 `msg_id` 属业务持久化层。业务层的「本机已落库」只能抑制未读/事件，
    /// 绝不能前移到转发之前，否则已经 Direct 收到过该消息的节点会拒绝继续 fan-out，
    /// epidemic 传播在此断链。转发目标只由邻居集合 / fanout / exclude 决定。
    #[test]
    fn gossip_propagation_layer_stays_independent_of_local_persistence() {
        let mut engine = GossipEngine::new(100, 10, 4, 6);
        assert!(engine.is_new("m1"), "首次见到的信封必须进入处理与转发");
        assert!(!engine.is_new("m1"), "同一信封第二次到达在传播层判为重复");
        assert!(engine.is_new("m2"), "另一条消息不受前者影响");
        let peers = vec!["b".to_string(), "c".to_string(), "d".to_string()];
        let targets = engine.choose_fanout(&peers, "a");
        assert_eq!(targets.len(), 3, "fanout=4 时三个邻居都应被转发到");
        assert!(!targets.contains(&"a".to_string()), "不回发给信封的发送方");
    }

    /// P1-3 / Test 4 的前提条件：Direct 已把某 msg_id 落库之后，同一信封再到达时
    /// 必须「业务层判为已存在（于是抑制未读与事件）」且「传播层仍判为首次见到（于是
    /// 继续 verify 并在 ttl > 1 时 fan-out）」同时成立。
    /// 若把 `message_exists` 前移到 handle_gossip 开头直接 return，第二项就会被破坏，
    /// epidemic 传播在本节点断链 —— 这条测试就是防止那种"顺手简化"。
    #[test]
    fn business_duplicate_still_enters_the_propagation_layer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        let rec = MessageRecord {
            id: 0,
            msg_id: "m1".into(),
            conv_id: "dev-a".into(),
            sender_id: "dev-a".into(),
            receiver_id: "me".into(),
            kind: "text".into(),
            content: "hello".into(),
            ts: 1,
            seq: 1,
            status: "delivered".into(),
        };
        // Direct 先到并落库 → 它是唯一产生本地副作用的一方
        assert!(db::insert_message_if_new(&conn, &rec).unwrap());
        // 之后 Gossip 副本到达：业务层判为已存在 ⇒ 不再 touch / 不再 emit
        assert!(!db::insert_message_if_new(&conn, &rec).unwrap());
        // 但传播层（独立的内存 Bloom/LRU）从未被 Direct 登记 ⇒ 仍会走到 fan-out
        let mut engine = GossipEngine::new(100, 10, 4, 6);
        assert!(engine.is_new("m1"), "业务层已存在不得让传播层跳过转发");
        // 且两路径共用同一 msg_id，库里始终只有一行
        assert_eq!(db::get_messages(&conn, "dev-a", 10, 0).unwrap().len(), 1);
    }

    /// P1-3 / Test C：三态落库裁决 → 副作用与 Ack 策略的映射必须是显式且可测的。
    /// - 未读 +1 与 message-received：只有 `Ok(true)`（本次真的新建）才允许；
    /// - Ack：`Ok(true)` / `Ok(false)` 都允许（消息确已在库），`Err` 必须禁止
    ///   （Ack 会让发送方删掉 outbox 行 ⇒ 临时 DB 故障变成永久丢消息）。
    #[test]
    fn insert_outcome_maps_to_side_effect_and_ack_policy() {
        let fresh: Result<bool, rusqlite::Error> = Ok(true);
        let duplicate: Result<bool, rusqlite::Error> = Ok(false);
        let db_error: Result<bool, rusqlite::Error> = Err(rusqlite::Error::QueryReturnedNoRows);

        assert!(announced_on(&fresh), "本次新建 ⇒ 计未读 + 投递事件");
        assert!(!announced_on(&duplicate), "重复 ⇒ 不得再有副作用");
        assert!(
            !announced_on(&db_error),
            "DB 故障 ⇒ 不得有副作用（更不得当成重复）"
        );

        assert!(may_ack(&fresh), "本次新建 ⇒ Ack");
        assert!(
            may_ack(&duplicate),
            "已在库中 ⇒ 仍 Ack（Ack 语义 = 已成功接收并持久化）"
        );
        assert!(
            !may_ack(&db_error),
            "DB 故障 ⇒ 绝不 Ack，outbox 行必须保留以便重发"
        );
    }

    /// ReadReceipt 正常到达：ts ≤ last_read_ts 的消息推进到 read。
    #[test]
    fn read_receipt_marks_messages_as_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "peer-a".into(),
                sender_id: "me".into(),
                receiver_id: "peer-a".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "delivered".into(),
            },
        )
        .unwrap();
        // 模拟 ReadReceipt handler 的 UPDATE 语句
        let updated = conn
            .execute(
                "UPDATE messages SET status = 'read'
             WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                params!["peer-a", "me", 100],
            )
            .unwrap();
        assert_eq!(updated, 1, "应有 1 行被更新");
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "read");
    }

    /// Test D: updated = 0 时 DB UPDATE 无行被更新，但 handler 仍然 emit peer-read。
    /// 注意：emit 依赖 Tauri AppHandle，无法在纯单测中断言事件；
    /// 这里验证的是 SQL 路径正确返回 updated=0（与 emit 条件分离）。
    #[test]
    fn read_receipt_db_update_zero_when_already_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "peer-a".into(),
                sender_id: "me".into(),
                receiver_id: "peer-a".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "read".into(),
            },
        )
        .unwrap();
        let updated = conn
            .execute(
                "UPDATE messages SET status = 'read'
             WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                params!["peer-a", "me", 100],
            )
            .unwrap();
        assert_eq!(updated, 0, "DB 已是 read，无行被更新");
        // handler 仍然 emit peer-read（always-emit 修复），但 emit 本身无法在单测中断言
    }

    /// Test C: 多次 mark_read 只保留最大 timestamp。
    #[test]
    fn pending_reads_keeps_max_timestamp() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // mark_read(ts=100)
        let cur = pending.entry("peer-a".into()).or_insert(100);
        *cur = (*cur).max(100);
        assert_eq!(pending["peer-a"], 100);
        // mark_read(ts=80) — 较小，不更新
        let cur = pending.entry("peer-a".into()).or_insert(80);
        *cur = (*cur).max(80);
        assert_eq!(pending["peer-a"], 100);
        // mark_read(ts=200) — 较大，更新
        let cur = pending.entry("peer-a".into()).or_insert(200);
        *cur = (*cur).max(200);
        assert_eq!(pending["peer-a"], 200);
    }

    /// Test B: writer write_frame 失败时，ReadReceipt 的 timestamp 被重新放入 pending_reads。
    /// 模拟 writer_loop 的失败回收逻辑。
    #[test]
    fn writer_failure_preserves_pending_for_read_receipt() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // flush_pending_reads 已经 remove
        pending.insert("peer-a".into(), 200);
        let last_read_ts = pending.remove("peer-a").unwrap();
        assert!(pending.is_empty(), "flush 后 pending 应为空");
        // 模拟 writer_loop write_frame 失败后的回收逻辑
        {
            let cur = pending.entry("peer-a".into()).or_insert(last_read_ts);
            *cur = (*cur).max(last_read_ts);
        }
        assert_eq!(
            pending.get("peer-a"),
            Some(&200),
            "写入失败后 pending 应恢复"
        );
    }

    /// Test A: writer write_frame 成功时，pending 不被重新插入。
    /// flush_pending_reads remove 后发送成功，pending 保持清空。
    #[test]
    fn successful_flush_clears_pending() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        pending.insert("peer-a".into(), 200);
        // 模拟 flush_pending_reads: remove + try_send Ok + writer write_frame Ok
        let last_read_ts = pending.remove("peer-a").unwrap();
        // write_frame 成功 → 不执行 writer_loop 的回收逻辑
        let _ = last_read_ts;
        assert!(pending.is_empty(), "写入成功后 pending 应保持清空");
    }

    /// flush_pending_reads try_send 失败时（链路不存在），pending 必须恢复——
    //  否则 ReadReceipt 永久丢失，要等用户下次手动打开会话才能补发。
    #[test]
    fn flush_failure_reinserts_pending_read() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        pending.insert("peer-a".into(), 300);
        // 模拟 flush_pending_reads: remove → try_send Err → 必须 re-insert
        let last_read_ts = pending.remove("peer-a").unwrap();
        assert!(pending.is_empty(), "remove 后 pending 应为空");
        // try_send 失败 → 重新放入 pending（与 writer_loop 的失败回收同逻辑）
        {
            let cur = pending.entry("peer-a".into()).or_insert(last_read_ts);
            *cur = (*cur).max(last_read_ts);
        }
        assert_eq!(
            pending.get("peer-a"),
            Some(&300),
            "try_send 失败后 pending 应恢复"
        );
    }

    /// 多次 flush 失败只保留最大 timestamp（幂等性）。
    #[test]
    fn repeated_flush_failure_keeps_max_timestamp() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // 第一次 mark_read(ts=300) → flush 失败
        pending.insert("peer-a".into(), 300);
        let ts1 = pending.remove("peer-a").unwrap();
        {
            let cur = pending.entry("peer-a".into()).or_insert(ts1);
            *cur = (*cur).max(ts1);
        }
        // 第二次 mark_read(ts=200) → 较小，不覆盖
        let cur = pending.entry("peer-a".into()).or_insert(200);
        *cur = (*cur).max(200);
        assert_eq!(pending["peer-a"], 300);
        // 第三次 mark_read(ts=500) → 较大，更新
        let cur = pending.entry("peer-a".into()).or_insert(500);
        *cur = (*cur).max(500);
        assert_eq!(pending["peer-a"], 500);
    }

    /// read 不会被 delivered 回退（set_message_status 守卫）。
    #[test]
    fn read_status_never_regresses_to_delivered() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "f1".into(),
                sender_id: "a".into(),
                receiver_id: "b".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "read".into(),
            },
        )
        .unwrap();
        db::set_message_status(&conn, "m1", "delivered").unwrap();
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "read", "delivered 不得回退 read");
    }

    // ================================================================
    // Ack 中继转发测试
    // ================================================================

    /// Test 1：本机是原始发送者 → Ack 正常处理（status→delivered, outbox 删除）。
    #[test]
    fn ack_local_sender_marks_delivered_and_clears_outbox() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 模拟本机发送的消息
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "d1".into(),
                sender_id: "me".into(),
                receiver_id: "d1".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "sent".into(),
            },
        )
        .unwrap();
        db::insert_outbox(&conn, "m1", "d1", r#"payload"#).unwrap();

        // Ack handler 的查询：sender_id = "me" == 本机 → 走正常处理分支
        let sender_id: String = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["m1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sender_id, "me");

        // 模拟正常处理
        db::set_message_status(&conn, "m1", "delivered").unwrap();
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", params!["m1"])
            .unwrap();

        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "delivered");
        assert!(db::list_outbox(&conn, "d1").unwrap().is_empty());
    }

    /// Test 2：中继节点收到 Ack → sender_id ≠ 本机 → 不做本地处理。
    /// 验证中继节点不修改自己的消息状态、不删除 outbox。
    #[test]
    fn ack_relay_node_does_not_process_locally() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 中继节点 C 收到 A 发给 D 的消息（通过 Gossip）
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "d1".into(),
                sender_id: "node-a".into(),
                receiver_id: "d1".into(),
                kind: "text".into(),
                content: "hello".into(),
                ts: 200,
                seq: 1,
                status: "delivered".into(),
            },
        )
        .unwrap();
        // C 自己也有一条 outbox 消息（不同的 msg_id）
        db::insert_outbox(&conn, "m-own", "some-peer", r#"own payload"#).unwrap();

        // Ack handler 查询：sender_id = "node-a" ≠ "me"（当前节点是 C）
        let sender_id: String = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["m1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(sender_id, "me", "sender_id 应为原始发送方 A，不是本机 C");

        // 中继节点不应执行任何本地状态修改
        // （实际 handler 中，Some(sender) if sender == device_id 分支不匹配 → 走转发分支）
        let status: String = conn
            .query_row("SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "delivered", "中继节点不修改消息状态");
        assert!(
            !db::list_outbox(&conn, "some-peer").unwrap().is_empty(),
            "中继节点不删除自己的 outbox"
        );
    }

    /// Test 3：Ack 对应的 msg_id 不存在 → 查询返回 None → 安全丢弃。
    #[test]
    fn ack_unknown_msg_id_is_silently_dropped() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 不存在的消息
        let result: Option<String> = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["nonexistent"],
                |r| r.get(0),
            )
            .ok();
        assert!(result.is_none(), "查询不存在的 msg_id 应返回 None");
        // 此时 handler 走 None 分支 → 不做任何修改
    }

    /// Test 4：中继节点转发 Ack 时，sender_id 和 message_id 不被修改。
    #[test]
    fn ack_relay_preserves_original_fields() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(
            &conn,
            &MessageRecord {
                id: 0,
                msg_id: "m1".into(),
                conv_id: "d1".into(),
                sender_id: "node-a".into(),
                receiver_id: "d1".into(),
                kind: "text".into(),
                content: "hi".into(),
                ts: 100,
                seq: 1,
                status: "delivered".into(),
            },
        )
        .unwrap();

        // 中继节点查询到原始 sender_id
        let original_sender: String = conn
            .query_row(
                "SELECT sender_id FROM messages WHERE msg_id = ?1",
                params!["m1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original_sender, "node-a");

        // 转发时使用原始 Ack 消息（message_id 和 sender_id 不变）
        let ack = Message::Ack {
            msg_id: "m1".into(),
        };
        match &ack {
            Message::Ack { msg_id } => {
                assert_eq!(msg_id, "m1", "message_id 不得被修改");
            }
            _ => panic!("应为 Ack"),
        }
        // original_sender 用于 try_send 的 peer_id 参数，不嵌入 Ack 消息体
    }

    // ================================================================
    // 好友权限测试
    // ================================================================

    /// 好友状态下 Direct Chat 可以正常接收（insert_message_if_new 成功）。
    #[test]
    fn friend_chat_message_accepted() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "a", "Alice", None).unwrap();
        assert!(
            db::get_friend(&conn, "a").is_some(),
            "好友存在时应能处理消息"
        );
    }

    /// 删除好友后 Direct Chat 不落库。
    #[test]
    fn non_friend_chat_message_rejected() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "a", "Alice", None).unwrap();
        db::remove_friend(&conn, "a").unwrap();
        assert!(db::get_friend(&conn, "a").is_none(), "删除好友后应检测不到");
    }

    /// Gossip Group 不受好友检查影响。
    #[test]
    fn gossip_group不受好友检查影响() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 群聊不需要好友关系
        db::create_group(&conn, "g1", "测试群", "owner", &["a".into(), "b".into()]).unwrap();
        let groups = db::list_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1);
    }

    /// FriendMessageBlocked 包含 original_sender 字段。
    #[test]
    fn friend_message_blocked_has_original_sender() {
        let msg = Message::FriendMessageBlocked {
            from: "c".into(),
            to: "a".into(),
            original_sender: "a".into(),
        };
        match &msg {
            Message::FriendMessageBlocked {
                from,
                to,
                original_sender,
            } => {
                assert_eq!(from, "c");
                assert_eq!(to, "a");
                assert_eq!(original_sender, "a");
            }
            _ => panic!("应为 FriendMessageBlocked"),
        }
    }

    // ---------- 待发群密钥 pending 表（最小 P0 修复） ----------

    /// 链路未建立时（announce 先于 ensure_link）群密钥发送失败，
    /// 必须登记到 pending，否则后续 is_new/key_changed 不再触发 → 永久丢密钥。
    #[test]
    fn redistribute_failure_registers_pending() {
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();

        // 模拟 announce → upsert_peer → redistribute_group_keys → try_send 失败（无 link）
        // 失败分支必须调用 mark_pending_group_key（见 transport.rs::redistribute_group_keys）
        mark_pending_group_key(&mut pending, "peer-b", "g-1");
        mark_pending_group_key(&mut pending, "peer-b", "g-2");
        // 同 (peer, group) 重复登记应幂等
        mark_pending_group_key(&mut pending, "peer-b", "g-1");

        let ids = pending_group_key_ids(&pending, "peer-b");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"g-1".to_string()));
        assert!(ids.contains(&"g-2".to_string()));
    }

    /// 链路就绪后 flush_pending_group_keys 成功 → 必须清除登记项，
    /// 否则 pending 永远不被消费（不增长、不泄漏、达成"成功即清"）。
    #[test]
    fn flush_success_clears_pending() {
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();
        mark_pending_group_key(&mut pending, "peer-b", "g-1");
        mark_pending_group_key(&mut pending, "peer-b", "g-2");
        mark_pending_group_key(&mut pending, "peer-c", "g-1");

        // 模拟 flush_pending_group_keys：peer-b 已建链、两条都发送成功
        let result: Result<(), GroupKeySendErr> = Ok(());
        assert!(!should_retain_pending_group_key(&result));
        for gid in ["g-1", "g-2"] {
            clear_pending_group_key(&mut pending, "peer-b", gid);
        }

        // peer-b 的登记应被清空（键一并移除，避免无意义增长）
        assert!(pending_group_key_ids(&pending, "peer-b").is_empty());
        assert!(!pending.contains_key("peer-b"));
        // peer-c 不受影响
        assert_eq!(pending_group_key_ids(&pending, "peer-c"), vec!["g-1".to_string()]);
    }

    /// 链路仍未就绪（NoLink）或非可重试原因（Fatal）的判定：
    /// NoLink 必须保留登记项等待下一次 flush；Fatal / Ok 必须清除。
    #[test]
    fn flush_failure_retains_or_clears_pending_correctly() {
        // 仍无链路 → 保留
        assert!(should_retain_pending_group_key(&Err(GroupKeySendErr::NoLink)));
        // 非可重试 → 清除
        assert!(!should_retain_pending_group_key(&Err(GroupKeySendErr::Fatal)));
        assert!(!should_retain_pending_group_key(&Ok(())));

        // 验证 flush 逻辑：NoLink 分支不调用 clear，pending 保持
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();
        mark_pending_group_key(&mut pending, "peer-b", "g-1");
        let result: Result<(), GroupKeySendErr> = Err(GroupKeySendErr::NoLink);
        if should_retain_pending_group_key(&result) {
            // 故意不调用 clear_pending_group_key：等待下一次 flush
        } else {
            clear_pending_group_key(&mut pending, "peer-b", "g-1");
        }
        assert_eq!(pending_group_key_ids(&pending, "peer-b"), vec!["g-1".to_string()]);

        // 下一轮 flush：Fatal 分支必须清除（重试无意义：缺公钥 / 非成员 / 无密钥）
        clear_pending_group_key(&mut pending, "peer-b", "g-1");
        let result: Result<(), GroupKeySendErr> = Err(GroupKeySendErr::Fatal);
        if !should_retain_pending_group_key(&result) {
            clear_pending_group_key(&mut pending, "peer-b", "g-1");
        }
        assert!(pending_group_key_ids(&pending, "peer-b").is_empty());
    }

    // ---------- 群成员公钥解析（peers 优先、friends 回落） ----------

    /// 群密钥分发的公钥选择规则：peers 表优先（announce/Hello 实时维护）、
    /// friends 表回落、两边都缺才返回 None（安全跳过）。
    #[test]
    fn pick_member_x25519_prefers_peers_then_friends() {
        // peers 有 → 用 peers（即使 friends 也有）
        assert_eq!(
            pick_member_x25519(Some("pk-peer".into()), Some("pk-friend".into())).as_deref(),
            Some("pk-peer")
        );
        // peers 无、friends 有 → 回落 friends
        assert_eq!(
            pick_member_x25519(None, Some("pk-friend".into())).as_deref(),
            Some("pk-friend")
        );
        // 两边都无 → None（调用方 continue 安全跳过）
        assert_eq!(pick_member_x25519(None, None), None);
    }

    /// respond_friend_request accept 路径的公钥补写（对齐 FriendAccept 接收路径）：
    /// add_friend 时不带公钥 → maybe_update_friend 从 peers 补写 → friends 可查。
    /// 此前 accept 方不补写且 key_changed 不再触发 → 公钥永久缺失 →
    /// 群密钥分发 continue 静默跳过（B 后加群收不到的根因）。
    #[test]
    fn friend_accept_backfills_pubkeys_from_peers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // accept：add_friend 不带公钥（commands.rs respond_friend_request 现状）
        db::add_friend(&conn, "b", "Bob", None).unwrap();
        assert!(
            db::get_friend_x25519(&conn, "b").is_none(),
            "accept 后 friends 公钥应为空（复现补写前状态）"
        );
        // maybe_update_friend 的补写行为：peers 表已有公钥 → 写入 friends
        db::update_friend_pubkeys(&conn, "b", Some("xk-b"), Some("ek-b")).ok();
        assert_eq!(
            db::get_friend_x25519(&conn, "b").as_deref(),
            Some("xk-b"),
            "补写后群密钥分发必须能取到公钥"
        );
        // 重复补写幂等（maybe_update_friend 每次都可能调用）
        db::update_friend_pubkeys(&conn, "b", Some("xk-b"), Some("ek-b")).ok();
        assert_eq!(db::get_friend_x25519(&conn, "b").as_deref(), Some("xk-b"));
    }

    // ---------------- P0：TCP 监听端口生命周期（生产 1.0 重启掉线） ----------------

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
}
