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
use crate::discovery::routed::{parse_endpoints, ROUTED_ENDPOINTS_KEY};
use crate::mesh::router::{ForwardDecision, MeshDestination, MeshFrame, MeshFrameKind};
use crate::mesh::{
    Endpoint as MeshEndpoint, PathKind, PeerCandidate, PeerIdentity, PeerOnlineState,
};
use crate::network::file;
use crate::protocol::{hello_signing_bytes, GossipEnvelope, GossipKind, Message};
use crate::state::{
    AppState, FileDoneInfo, FileFailedInfo, FileProgress, Link, LinkState, MessageRecord, Peer,
    PendingRequest,
};
use crate::transport::tcp::{TcpReceiver, TcpSender};

// ---- 出站投递与链路选路（帧编码 / route_order / try_send / 分片流钉链路 / gossip 扇出）----
// 按 `commands.rs` 的既有先例用 `include!` 分册：同一模块、零 `use` 改动，
// 只是把 9800 行的单文件切到可评审的粒度。
include!("transport/outbound.rs");

// ---- 公网中继（会合循环 / 协商接线 / 撤链，ADR-0020）----
include!("transport/relay.rs");

/// 中继态的内存 TTL：超过它且仍未完成重组的条目一律回收。
///
/// 为什么必须有：`relay_file_keys` 与 `RelayManager::reassemblies` 都以**对端可控**的
/// `transfer_id` 为键，插入点在收到 `RelayFileOffer` 时，而清除点只在「重组完成/失败」。
/// 对端（只需是好友）持续发 `RelayFileOffer{ 每次新 id, total_chunks: 1 }` 却永不发分片，
/// 两张表就只增不减 —— 进程内存单调增长直至 OOM，且没有任何回收路径。
/// 1 小时与 `.part` 的 24h 口径同源（可恢复失败的保留思路），但内存态更敏感故更短。
const RELAY_STATE_TTL_MS: i64 = 60 * 60 * 1000;

/// 清扫过期的中继态（`relay_file_keys` + `reassemblies`），返回清掉的条目数。
/// 与 `sweep_stale_parts` 同一趟定时任务里跑。
///
/// 回收 ≠ 静默消失（2026-09-23 审计 A2）：被回收的传输若在 `file_transfers`
/// 里仍是 active（offer 落过库、前端在显示进度），必须标 failed 并 emit
/// `file-failed` —— 旧行为只 retain，DB 行永远停在某个百分比，接收端永久卡 X%。
pub fn sweep_stale_relay(state: &AppState) -> usize {
    let cutoff = db::now_ms() - RELAY_STATE_TTL_MS;
    let mut removed: Vec<String> = Vec::new();
    state
        .relay_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|k, v| {
            let keep = v.created_at > cutoff;
            if !keep {
                removed.push(k.clone());
            }
            keep
        });
    removed.extend(
        state
            .relay
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sweep_stale_reassemblies(cutoff),
    );
    let n = removed.len();
    if !removed.is_empty() {
        // 锁只圈住写库，emit 一律出锁再做：在 db 锁内 emit 会把「锁内慢活」请回来
        // （前端收到 file-failed 后的下一次 IPC 要抢同一把锁）。
        let marked: Vec<String> = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let mut done = Vec::new();
            for id in &removed {
                match db::mark_transfer_failed_if_active(&dbc, id) {
                    // 只改仍 active 的行：done/failed 的既有终态不许被回收动作改写
                    Ok(true) => done.push(id.clone()),
                    Ok(false) => {}
                    Err(e) => state.logger.warn(
                        "relay",
                        format!(
                            "回收中继传输时标 failed 失败，DB 行可能仍停在 active id={id}: {e}"
                        ),
                    ),
                }
            }
            done
        };
        for id in marked {
            let _ = state.app.emit(
                "file-failed",
                &FileFailedInfo {
                    transfer_id: id,
                    reason: "接收超时：中继传输一小时未完成，已回收".to_string(),
                },
            );
        }
    }
    n
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
    let raw_avatar = state
        .avatar
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    // payload：明文 JSON（昵称/可选头像/设备类型）。身份与双公钥已在 GossipEnvelope 字段里。
    //
    // ⚠️ Presence 每 10s 广播一次，而且走的是**优先通道**：绝不能内联大头像。
    // 一张 400KB 的 base64 头像会让优先队列被上千片分片占住，聊天与好友请求全部排在
    // 它后面（真机症状：开了蓝牙后好友申请几分钟才到、消息一直"发送中"）。
    // 超过内联上限就**整个字段都不带**（接收侧 upsert_peer 只在 Some 时更新头像，
    // 缺失/None 不会清空对端已有头像）；大头像改由建链时的 UserInfo 定向同步一次。
    let avatar = hello_avatar_for_wire(raw_avatar.as_deref());
    let mut payload_json = serde_json::json!({
        "nickname": nickname,
        "device_type": crate::protocol::current_device_type(),
    });
    if let Some(a) = avatar {
        payload_json["avatar"] = serde_json::Value::String(a.to_string());
    }
    let payload = payload_json.to_string();
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
    let state_for_relay = state.clone();
    let shutdown_for_relay = shutdown.clone();
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
                        links.values().flatten().map(|l| l.high.clone()).collect()
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
                    // 把 health_timeout_ms 也带进日志 —— 这样用户能一眼看出
                    // "为什么是 N 秒"（health × 3 = stale），不会在"健康阈值"和"总超时"
                    // 之间来回猜。
                    let health_ms = {
                        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
                        pm.health_timeout_ms()
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
                                .map(|l| (l.low.clone(), l.cancel.clone()))
                        };
                        let Some((bulk, cancel)) = bulk else { continue };
                        // ① 精确取消这一条连接的读写任务（半开的读只有它能打断）。
                        let _ = cancel.send(true);
                        // ② 从传输链路表移除（空 Vec 连 key 一起删），让 `ensure_link` 能重拨。
                        {
                            let mut links = state.links.lock().await;
                            if let Some(v) = links.get_mut(&peer) {
                                v.retain(|l| !l.low.same_channel(&bulk));
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
                                "[WATCHDOG] peer={peer} ep={ep} 读活性超过 {}s 无入站帧（健康阈值={}s ×3 = stale）⇒ 拆除死链路并等待重拨",
                                stale_ms / 1000,
                                health_ms / 1000,
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
                                None,
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

    // 公网中继（ADR-0020）：用户没填服务器时，这个任务每 10s 只做一次
    // "读三个 setting → 什么都没配 → 什么都不做"，不产生任何网络流量。
    let relay_task = tokio::spawn(relay_rendezvous_task(state_for_relay, shutdown_for_relay));

    Ok(vec![
        accept_task,
        heartbeat_task,
        routed_task,
        presence_task,
        relay_task,
    ])
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

// ---------------- Hello 认证与好友接受：判据、密钥绑定、有界补发、签名 Hello 构造 ----------------
include!("transport/handshake.rs");

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
                state
                    .logger
                    .warn("transport", format!("拒绝未通过身份认证的 Hello: {reason}"));
                return;
            }
            // 验签通过 ⇒ 这对公钥**已被证明**由该 device_id 的持有者使用
            // （Hello 的 sig 覆盖 device_id|tcp_port|nonce|x25519|ed25519，且用该 ed25519 验签）。
            // 到此才允许它们参与身份绑定与持久化 —— 这是「已验证」与「只是广播来的」
            // 之间唯一的升级点。
            mark_peer_keys_verified(&state, device_id, x25519_pubkey, ed25519_pubkey);
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
    let ((high_tx, high_rx), (normal_tx, normal_rx), (low_tx, low_rx)) =
        link_channels(crate::protocol::FILE_CHUNK);
    // 本连接独立的取消信号（M3#6）：健康 watchdog 判定僵尸链路时精确断开这一条。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    // 追加到该 peer 的连接列表（而非覆盖）—— 多连接支持的基础。
    // 端点取 TCP 对端的真实地址，使「同一 peer 的不同端点」可被区分。
    // 入站连接的路径类型：**不能一律当 LAN**。真机 2026-09-14（全 Windows 局域网）：
    // 若对端是通过 Clash TUN / VPN / Tailscale 地址拨进来的，把它记成 LAN 会让
    // has_lan_path 永真 ⇒ 我们再也不拨它的真实 LAN 地址，同网段也一直走隧道/中继。
    // 只有非虚拟地址才按 LAN 记；虚拟地址按 Routed（与出站 Routed 同一语义）。
    let inbound_kind = match peer_addr.ip() {
        std::net::IpAddr::V4(v4) if is_virtual_ip(&v4) => PathKind::Routed,
        _ => PathKind::Lan,
    };
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: MeshEndpoint::Tcp(peer_addr),
            path_kind: inbound_kind,
            high: high_tx.clone(),
            normal: normal_tx.clone(),
            low: low_tx.clone(),
            cancel: cancel_tx,
        });
    // 同步到 mesh 层：让 Peer/Connection 模型知道这条连接存在
    register_connection(&state, &peer_id, MeshEndpoint::Tcp(peer_addr), inbound_kind);
    replay_group_frames_to(&state, &peer_id); // #77：入站新链路 ⇒ 补递窗口内的群历史
    tokio::spawn(writer_loop(
        state.clone(),
        peer_id.clone(),
        MeshEndpoint::Tcp(peer_addr),
        w,
        high_rx,
        normal_rx,
        low_rx,
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
    let _ = high_tx.send(build_signed_hello(&state, conv_clock)).await;
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
        low_tx,
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
/// ⚠️ **每一种传输的读循环都必须调用它**（2026-09-13 审计发现：BLE 链路漏了这一句，
/// 于是健康的蓝牙链路 15s 后就被判"不健康"、45s 被看门狗自己拆掉，循环往复）。
/// `pub(crate)` 就是为了让 `network/ble.rs` 也能调。
pub(crate) fn mark_conn_seen(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.mark_connection_seen(peer_id, endpoint, db::now_ms(), None, true);
}

/// 记录一次**写出成功**（M3-0b：只刷出站活性，**不**参与 `is_healthy`）。
///
/// 半开 TCP 上写会持续「成功」，因此它绝不能算成「对端活着」的证据 ——
/// 否则死链路会永久被判健康，选路一直选中它（ADR-0014 §7）。
fn mark_conn_write_seen(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.mark_connection_seen(peer_id, endpoint, db::now_ms(), None, false);
}

/// 记录「某个文件传输**给某个收件人**又有一帧真的离开了链路」：刷新时刻 **并** 累加一片。
///
/// 为什么必须落在"写出"而不是"入队"：发送侧 mpsc 容量 1024，1MB 文件的分块会在
/// **1 秒内**全部入队，而链路上要跑几分钟（BLE 上更久）。两个消费方共用这一个证据点：
///   · `FileCompleteAck` 的等待窗口靠 `at_ms` 从"固定 30s 墙钟"改成"安静 30s 才算失败"
///     （`file.rs::wait_complete_ack`）；
///   · 发送进度靠 `chunks` 换算成字节（v4.22.37，`file.rs::wire_progress_bytes`）——
///     在此之前进度条读的是入队量，于是 LAN 上最多 262MB 还堵在队列里时界面就 100% 了。
/// 放在 writer_loop 里是唯一正确的位置 —— 它是"字节真的走了"的唯一证据点。
///
/// TCP 与 BLE 两条写循环都要调（BLE 见 `network/ble.rs`）。
///
/// `recipient` = **这条链路在 `links` 表里的归属 id**（writer_loop 自己那个 `peer_id`），
/// 不是"下一跳"。群发是 N 个投递任务共用一个 `transfer_id`，只按 id 记的话，甲还在走的
/// 字节会把乙的"最近有写出"一直刷新 ⇒ 真卡死的乙永远判不出停滞。
///
/// ⚠️ 中继转发（`RelayChunk`）**故意不记账**，不是漏：那种帧要送给谁由帧自己的 `to`
/// 决定、与写它的链路无关 —— 按 `to` 记会让每个**转发节点**都为"别人的传输"留一条记录，
/// 而转发侧没有 `WireLedger`，谁也不会去删；按下一跳记又会把共用同一中继的两个成员并成
/// 一个数。中继侧进度要单独设计，不并进这条口径。
pub(crate) fn mark_file_wire_progress(state: &AppState, msg: &Message, recipient: &str) {
    let transfer_id = match msg {
        Message::FileChunk { transfer_id, .. } => transfer_id,
        // 群文件走同一个"等确认"语义（确认是 GroupFileCompleteAck）
        Message::GroupFileChunk { transfer_id, .. } => transfer_id,
        _ => return,
    };
    bump_file_wire_progress_in(
        &state.file_wire_progress,
        &crate::network::file::file_peer_key(transfer_id, recipient),
        db::now_ms(),
    );
}

/// 记账本体（与 `AppState` 解耦，单测才能直接喂一张表验"按收件人分开"）。
pub(crate) fn bump_file_wire_progress_in(
    table: &std::sync::Mutex<HashMap<String, crate::state::FileWireProgress>>,
    wire_key: &str,
    now: i64,
) {
    table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(wire_key.to_string())
        .and_modify(|p| {
            p.at_ms = now;
            // 饱和加：这张表只增不减到收尾，理论上碰不到上界，但进度换算依赖它，
            // 宁可停在 u64::MAX 也不要溢出一个负数把进度算成 0。
            p.chunks = p.chunks.saturating_add(1);
        })
        .or_insert(crate::state::FileWireProgress {
            at_ms: now,
            chunks: 1,
        });
}

/// 读「该 (传输 × 收件人) 最近一次真的写出字节」的时刻（0 = 从未写出过）。
///
/// 参数是 `file_peer_key` 的产物，**不是裸 `transfer_id`** —— 读错键等于永远"没有进展"，
/// 30s 安静窗口会提前把还在正常传输的链路判死。
pub(crate) fn file_wire_progress_at(state: &AppState, wire_key: &str) -> i64 {
    state
        .file_wire_progress
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(wire_key)
        .map(|p| p.at_ms)
        .unwrap_or(0)
}

/// 读「该 (传输 × 收件人) 已经真的写出多少片」（无记录 = 0）。发送进度的唯一真实来源。
///
/// 同 `file_wire_progress_at`：参数是 `file_peer_key` 的产物。
pub(crate) fn file_wire_chunks_at(state: &AppState, wire_key: &str) -> u64 {
    state
        .file_wire_progress
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(wire_key)
        .map(|p| p.chunks)
        .unwrap_or(0)
}

/// 传输收尾时清掉进展记录，避免这张表随历史传输无限增长。
///
/// 收在 `file.rs::WireLedger` 的 `Drop` 里（成功、取消、链路失败、panic 展开都走同一处）；
/// 这里只留"怎么删"，不留第二套"什么时候删"。参数是 `file_peer_key` 的产物。
pub(crate) fn clear_file_wire_progress_in(
    table: &std::sync::Mutex<HashMap<String, crate::state::FileWireProgress>>,
    wire_key: &str,
) {
    table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(wire_key);
}

/// 把「某条连接失败」喂给 mesh 层（写失败）。读循环退出时链路会被 `unregister_connection`
/// 整条摘掉，无需再记失败。
fn mark_conn_failure(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    pm.mark_connection_failure(peer_id, endpoint);
}

// ---------------- 链路队列的容量策略（第 2 步 · P3） ----------------

// ---------------- 链路队列的容量策略（字节预算 / 槽数折算 / 写失败分流，第 2 步 · P3） ----------------
include!("transport/queue_policy.rs");

// ---------------- 传输层 ↔ mesh 层 同步（6b-3） ----------------

// ---------------- 链路状态与 conv_link 快照（mesh 层同步 6b-3、连接登记表、按节流打日志） ----------------
include!("transport/link_state.rs");

// ---------------- 主动建链（小 ID 拨号） ----------------

// ---------------- 主动建链：拨号裁决、链路快照、握手超时与连接建立 ----------------
include!("transport/dial.rs");

// ---------------- 消息分发 ----------------

/// 定向中继判定（纯函数，便于钉住）：这帧是不是「不是给我的、需要我借一跳转投」的定向帧？
///
/// 返回 Some(to) 表示应把**原帧**投给 to（仅当本机有到 to 的直连；没有则由 try_send 失败丢弃）。
/// 覆盖共享目录三件套与中继文件元数据；RelayChunk 有独立的 ttl 转发路径，不在这里。
fn directed_relay_target<'a>(msg: &'a Message, my_id: &str) -> Option<&'a str> {
    match msg {
        Message::ShareTreeRequest { to, .. } if to != my_id => Some(to.as_str()),
        Message::ShareTreeResponse { to: Some(t), .. } if t != my_id => Some(t.as_str()),
        Message::ShareFileRequest { to: Some(t), .. } if t != my_id => Some(t.as_str()),
        Message::RelayFileOffer { to, .. } if to != my_id => Some(to.as_str()),
        _ => None,
    }
}

/// 建链 / Hello 时：把该 peer 名下**未完成（可恢复）的接收**重新拉一遍（ADR-0019 Phase 1）。
///
/// - 只对声明了 CONTENT_FEATURE_PULL 的对端发（旧端不发新帧，保持兼容）；
/// - 退避未到点的跳过（纯策略 should_retry_now）；
/// - 只处理 Receive 方向：Send 方向的重试由既有 file_outbox 负责。
async fn retry_incomplete_content(state: &Arc<AppState>, peer_id: &str) {
    let caps = state
        .peer_content_features
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(peer_id)
        .copied()
        .unwrap_or(0);
    if caps & crate::protocol::CONTENT_FEATURE_PULL == 0 {
        return;
    }
    let rows = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::content::store::list_resumable_for_peer(&dbc, peer_id).unwrap_or_default()
    };
    let now = db::now_ms();
    for rec in rows {
        if rec.direction != crate::content::model::Direction::Receive {
            continue;
        }
        // Incomplete：到点就重试（退避）；Active：长时间没动（丢链/半开）也重试 ——
        // 中途断链不一定有机会写失败记录，不能让"卡住的 Active"永远不重试。
        let due = match rec.status {
            crate::content::model::TransferStatus::Incomplete => {
                crate::content::policy::should_retry_now(rec.status, now, rec.next_attempt_at)
            }
            crate::content::model::TransferStatus::Active => {
                now.saturating_sub(rec.updated_at) > 60_000
            }
            _ => false,
        };
        if !due {
            continue;
        }
        let msg = Message::ContentRequest {
            from: state.device_id.clone(),
            cid: rec.cid.clone(),
            transfer_id: rec.transfer_id.clone().unwrap_or_default(),
            from_seq: 0,
            from_bytes: rec.received,
            name: rec.name.clone(),
            size: rec.size,
        };
        let sent = try_send(state, peer_id, &msg).await.is_ok();
        {
            // 自审必改#1：这一轮到期重发本身就是「上一轮无人应答」的事实 ——
            // 不记一次失败，attempts 永远不涨，MAX_CONTENT_RETRIES 封顶形同虚设
            // （对端重装/文件已删时无应答路径上没有任何 record_failure 调用点）。
            // 记完之后 status 由退避门控制下一次；到封顶收口 Rejected，
            // list_resumable 不再捞它 —— 无限重发链在此闭合。
            let reason = if sent {
                crate::content::model::FailReason::Timeout
            } else {
                crate::content::model::FailReason::LinkDown
            };
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = crate::content::store::record_failure(
                &dbc,
                &rec.cid,
                peer_id,
                rec.direction,
                reason,
                db::now_ms(),
            );
        }
        if sent {
            state.logger.info(
                "content",
                format!("建链自动重试未完成内容 cid={} peer={peer_id}", rec.cid),
            );
        }
    }
}

/// 数据面中继授权闸的**唯一入口**（2026-09-19 P0#5 的三处重复，2026-10-07 收成一个家）。
///
/// 三个转发点都从这里过：定向借道（`handle_message` 的 directed relay）、
/// `OpaqueExternal` 外部帧转投、`RelayChunk` 文件分片转投（`relay_file.rs`）。
/// 判据 `relay_data_plane_respects_policy` 钉的是"闸只有一个家 + 三个消费者各走它"，
/// 不是"数得到三次调用" —— 后者会把"复制三遍"当成正确形状锁死（本轮之前正是如此）。
///
/// 授权主体是**经 Hello 验签的链路对端** `peer_id`（帧内 `from` 可自报伪造，不作依据）；
/// Friends 档要多查一次好友表，所以这里必须碰 db 锁 —— 只在策略真需要时碰（Off/All 不查库）。
///
/// `why` 只在**被拒且节流放行**时求值：中继在文件分片的热路径上，每条分片都 `format!`
/// 一次会白烧一次分配（收家之前它写在三个调用点上、每处也都无条件拼串）。
/// 返回 true 时日志已经打好，调用方只需 `return`。
fn relay_denied(state: &Arc<AppState>, peer_id: &str, why: impl FnOnce() -> String) -> bool {
    let cfg = state.relay_policy_config();
    let allowed = crate::mesh::relay_policy::decide_relay_from_peer(&cfg, peer_id, || {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend(&dbc, peer_id).is_some()
    });
    if allowed {
        return false;
    }
    if log_throttled("relay_deny", 10_000) {
        state
            .logger
            .warn("mesh", format!("按中继策略拒绝对端 {peer_id} 的{}", why()));
    }
    true
}

pub async fn handle_message(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    // ---- 定向中继（一跳）：不是给我的定向帧，借邻居的直连转投给 to ----
    // 共享目录（ShareTree/ShareFile）在无直连时会走这里；RelayFileOffer 同理。
    // 只在「我确实有到 to 的直连」时投递；没有就丢弃（单跳中继限制，见
    // relay_send_to_neighbors 的说明）。
    if let Some(to) = directed_relay_target(&msg, &state.device_id) {
        // 授权闸（2026-09-19 P0#5）：定向借道此前**不经任何策略** —— 设置里关掉
        // 中继也照转，等于「开放文件中继/目录中继」。判据与 gossip 同一张真值表。
        // 2026-10-07：三处重复的判断收进 relay_denied 一个家（这条注释原来在每处各写一遍）。
        if relay_denied(state, peer_id, || format!("定向借道请求（to={to}）")) {
            return;
        }
        if let Err(e) = try_send(state, to, &msg).await {
            // 丢帧必须留痕（INV-005 不得静默丢）：共享目录/中继文件上层有幂等重试，
            // 但「一直失败」以前在本机日志里完全不可见。
            if log_throttled("relay_drop", 10_000) {
                state.logger.warn(
                    "mesh",
                    format!("定向借道转投失败 to={to}：{e}（上层会重试）"),
                );
            }
        }
        return;
    }
    match msg {
        // 跨版本降级（INV-P24 第 1 条）：看不懂的新帧 ⇒ 忽略这一条 + 节流日志，**绝不拆链**。
        // 不这么做的话，新版本一旦上线任何新帧类型，老设备的表现就不是"少收到一条消息"，
        // 而是"跟这台设备彻底连不上"（反序列化失败 → io::Error → reader 退出 → 重连再失败）。
        Message::Unknown { wire_type } => {
            if log_throttled("unknown_frame", 10_000) {
                // 把对端**声明**的版本一起写进日志：以前这句"本机版本低于对端"是从
                // "看不懂这一帧"倒推的，现在真机能直接看到是谁的哪个版本，
                // 老端没声明时如实写"未声明"而不是假装它报了 1。
                let declared = state
                    .peer_versions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(peer_id)
                    .map(|v| {
                        format!(
                            "协议={} 应用={}",
                            v.protocol_version
                                .map(|p| p.to_string())
                                .unwrap_or_else(|| "未声明".into()),
                            v.app_version.as_deref().unwrap_or("未声明"),
                        )
                    })
                    .unwrap_or_else(|| "未声明".into());
                state.logger.warn(
                    "proto",
                    format!(
                        "忽略未知帧类型 type={wire_type} peer={peer_id}：对端声明（{declared}），\
                         本机协议={} —— 本机版本低于对端，升级后即可识别；链路保持",
                        crate::protocol::PROTOCOL_VERSION,
                    ),
                );
            }
        }
        // ADR-0019 Phase 3：按 cid 拉取。**拥有即授权**，无需人工确认 —— 但只服务
        // "确实是我的好友、且 from 就是这条链路的对端（防冒名）"。回发复用既有
        // FileOffer→Chunk→Done→CompleteAck 流程（send_file_from_path）。
        // 「共享目录与单跳中继」6 个变体的处理体在 `transport/handle_share.rs::handle_share_and_relay_messages`
        // （2026-10-07 拆出；分发臂用 `{ .. }` 不绑字段，绑定留在分册里那条同形状的模式上）。
        m @ (Message::ContentRequest { .. }
        | Message::ShareTreeRequest { .. }
        | Message::ShareTreeResponse { .. }
        | Message::ShareFileRequest { .. }
        | Message::RelayFileOffer { .. }
        | Message::RelayChunk { .. }) => handle_share_and_relay_messages(state, peer_id, m).await,
        // 「身份握手与好友关系」8 个变体的处理体在 `transport/handle_identity.rs::handle_identity_and_friend_messages`
        // （2026-10-07 拆出；分发臂用 `{ .. }` 不绑字段，绑定留在分册里那条同形状的模式上）。
        m @ (Message::Hello { .. }
        | Message::Heartbeat { .. }
        | Message::UserInfo { .. }
        | Message::FriendRequest { .. }
        | Message::FriendAccept { .. }
        | Message::FriendReject { .. }
        | Message::FriendRemove { .. }
        | Message::FriendMessageBlocked { .. }) => {
            handle_identity_and_friend_messages(state, peer_id, m).await
        }
        Message::OpaqueExternal { id, ttl, payload } => {
            // 健壮性底线（ADR-0017 决策更新里保留的那条）：畸形/超限帧**只丢这一帧**，
            // 绝不断链 —— 记一条日志就返回，连接的读循环继续跑。
            let bytes = match crate::protocol::validate_opaque_external(&id, ttl, &payload) {
                Ok(b) => b,
                Err(why) => {
                    state
                        .logger
                        .warn("mesh", format!("丢弃不透明外部帧（{why}）from={peer_id}"));
                    return;
                }
            };
            // 去重 + TTL 递减：与业务帧**同一条流水线**（MeshRouter 不解析载荷，P-A03）。
            let decision = {
                let mut router = state.mesh_router.lock().unwrap_or_else(|e| e.into_inner());
                router.on_receive(
                    MeshFrame {
                        frame_id: id.clone(),
                        source_node_id: peer_id.to_string(),
                        destination: MeshDestination::Broadcast,
                        ttl,
                        kind: MeshFrameKind::OpaqueExternal,
                        // 载荷交给路由器只为"同样的流水线"，它不解析、不落库
                        payload: bytes,
                    },
                    &state.device_id,
                )
            };
            let ForwardDecision::Forward { frame, .. } = decision else {
                // 重复帧或 TTL 耗尽 —— 这正是"去得掉重"的落点
                state.logger.info(
                    "mesh",
                    format!("不透明外部帧未转发（重复或 TTL 耗尽）id={id}"),
                );
                return;
            };
            // 转发用路由器给出的 ttl（**已经递减**），fan-out 选邻居并排除来源节点
            let fwd = Message::OpaqueExternal {
                id: frame.frame_id.clone(),
                ttl: frame.ttl,
                payload,
            };
            // 数据面授权闸（P0#5）：外部帧同样必须吃中继策略，默认 All 行为不变
            if relay_denied(state, peer_id, || format!("外部帧转投 id={id}")) {
                return;
            }
            let targets: Vec<String> = {
                let neighbors = reachable_neighbors(state, peer_id).await;
                let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                gossip.choose_fanout(&neighbors, peer_id)
            };
            if targets.is_empty() {
                return;
            }
            // 与业务转发同一纪律：**不阻塞本连接的读循环**，用一个任务串行发完
            let st = state.clone();
            let msg = fwd;
            tokio::spawn(async move {
                for t in targets {
                    if let Err(e) = try_send(&st, &t, &msg).await {
                        if log_throttled("relay_drop", 10_000) {
                            st.logger.warn(
                                "mesh",
                                format!("外部帧转投失败 to={t}：{e}（其余邻居不受影响）"),
                            );
                        }
                    }
                }
            });
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
                // 重复投递也要回 Ack（否则一次丢 ACK = 发送方永远"发送中"）：
                // 留痕区分"没收到"与"收到了但 ACK 丢了"。
                state.logger.info(
                    "ble",
                    format!("[ACK] 重复消息仍回执 msg_id={msg_id} ← peer={peer_id}"),
                );
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
                    mention_targets: None,
                };
                let inserted = db::insert_message_if_new(&dbc, &rec);
                if announced_on(&inserted) {
                    // 与群聊分支同一套口径：时钟照常推进，静默类不计未读/不改预览。
                    db::observe_clock(&dbc, &from, seq).ok();
                    if crate::protocol::is_non_notifying_kind(&kind_str) {
                        db::ensure_conversation(&dbc, &from, "single", &name, None).ok();
                    } else {
                        db::touch_conversation(&dbc, &from, "single", &name, None, &preview, 1)
                            .ok();
                    }
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
            // 留痕（用户要求）：没有这条日志时，"发送中"到底是"没收到"还是"ACK 丢了"分不清。
            state.logger.info(
                "ble",
                format!("[ACK] 已持久化 ⇒ 回执 msg_id={msg_id} → peer={peer_id}"),
            );
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
            // 对方已读：把「我发给对方、ts ≤ effective_ts」的消息标记为 read（幂等）。
            // 判据只有 `db::mark_own_messages_read_upto` 一个家 —— 这里原先内联的那份
            // 只排除 `read`，会把本端**发送失败**的消息一起点亮成「已读」。
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ =
                    db::mark_own_messages_read_upto(&dbc, &from, &state.device_id, effective_ts);
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
        // 「群聊与群文件」11 个变体的处理体在 `transport/handle_group.rs::handle_group_messages`
        // （2026-10-07 拆出；分发臂用 `{ .. }` 不绑字段，绑定留在分册里那条同形状的模式上）。
        m @ (Message::GroupReadReceipt { .. }
        | Message::GroupAck { .. }
        | Message::GroupKey { .. }
        | Message::GroupRename { .. }
        | Message::GroupMemberRemoved { .. }
        | Message::GroupCreatorChanged { .. }
        | Message::GroupMemberLeft { .. }
        | Message::GroupFileOffer { .. }
        | Message::GroupFileChunk { .. }
        | Message::GroupFileDone { .. }
        | Message::GroupFileCompleteAck { .. }) => handle_group_messages(state, peer_id, m).await,
        m @ (Message::FileOffer { .. }
        | Message::FileAccept { .. }
        | Message::FileReject { .. }
        | Message::FileCompleteAck { .. }
        | Message::FileChunk { .. }
        | Message::FileDone { .. }) => handle_file_messages(state, peer_id, m).await,
        Message::Gossip { envelope } => {
            handle_gossip(state, peer_id, envelope).await;
        } // ---- 中继文件传输 ----
    }
}

// ---------------- 直连 E2EE 载荷 ----------------

// ---------------- 直连 E2EE 载荷：解封、重密封与公钥回查 ----------------
include!("transport/e2ee_payload.rs");

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
    !inserted.is_err()
}

// ---------------- Gossip 处理：消费判据、handle_gossip 与载荷还原 ----------------
// 分册登记见 `network::transport_src_for_guards`（源码守卫必须看全集）。
include!("transport/gossip.rs");

// ---------------- 中继文件传输 ----------------

// ---------------- 中继文件传输（offer / chunk 落盘 / 收尾改名） ----------------
include!("transport/relay_file.rs");

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
    if let Some(group) = db::get_group(
        &state.db.lock().unwrap_or_else(|e| e.into_inner()),
        &group_id,
    ) {
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
    state
        .group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(group_id.clone(), k);
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

// ---------------- 群文件传输：offer / chunk / done / 完成回执、失败与停滞回收 ----------------
include!("transport/group_file.rs");

// ---------------- 群治理事件：改名、移除成员、群主转让、成员退群 ----------------
include!("transport/group_membership.rs");

pub async fn get_group_key(state: &AppState, group_id: &str) -> Option<[u8; 32]> {
    if let Some(k) = state
        .group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(group_id)
    {
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
        .unwrap_or_else(|e| e.into_inner())
        .insert(group_id.to_string(), arr);
    Some(arr)
}

// ---------------- 节点与好友辅助 ----------------

// ---------------- 节点表写入、公钥冲突告警、聊天样式补发 ----------------
include!("transport/peer_registry.rs");

/// 群密钥发送失败的原因。仅 `NoLink` 可重试（登记 pending 等建链后 flush）。
enum GroupKeySendErr {
    /// TCP link 不可用（未建立连接 / 已断开）——可重试
    NoLink,
    /// 缺公钥 / 非群成员 / 无本地密钥 / 加密失败——重试无意义
    Fatal,
}

// ---------------- 待发群密钥登记表（纯逻辑，便于单测；不涉及网络与 AppState） ----------------

include!("transport/pending_keys.rs");

// ---------------- 会话链路快照与成员公钥辅助 ----------------
include!("transport/peer_state.rs");

// ---------------- 群成员变更的系统消息与文案 ----------------
include!("transport/member_notices.rs");

// ---------------- 离线队列补发 ----------------
include!("transport/outbox_flush.rs");

// ---------------- Outbox 超时清扫 ----------------

// ---------------- Outbox 超时清扫与过期终态（spawn_outbox_sweeper / finalize_expired_*） ----------------
include!("transport/outbox_sweep.rs");

// ---------------- 已读回执的路由与待发队列（本机读 / 群读） ----------------
include!("transport/read_receipt.rs");

include!("transport/handle_file.rs");
include!("transport/handle_share.rs");
include!("transport/handle_group.rs");
include!("transport/handle_identity.rs");
include!("transport/tests.rs");
