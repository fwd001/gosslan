//! UDP 设备发现：广播 + 组播双通道，多网卡选择。
//!
//! 机制：
//! - 每 5 秒向局域网**广播**（255.255.255.255）与**组播**（239.255.42.99）一次 `announce`，
//!   携带设备 ID、昵称、TCP 端口、X25519/Ed25519 公钥。
//! - 启动时额外广播一次 `who_has`，其他节点收到后单播回复自身信息，用于快速互相发现。
//! - 接收到的 `announce` 用于维护在线节点表并触发 TCP 建链（小 ID 主动拨号）。

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::time::{sleep_until, Duration, Instant};

use rand_core::{OsRng, RngCore};

use crate::commands::is_virtual_ip;
use crate::network::transport::{ensure_link, upsert_peer};
use crate::protocol::{
    UdpPacket, ANNOUNCE_INTERVAL_SECS, RELAY_PEER_TIMEOUT_SECS, UDP_PORT,
};
use crate::state::AppState;

/// 组播地址（与广播并行，覆盖被隔离广播域的场景）
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);

/// 检查一个 IPv4 是否属于 RFC1918 私网地址（合法 LAN 常用段）。
fn is_rfc1918(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    (o[0] == 10) || (o[0] == 172 && o[1] >= 16 && o[1] <= 31) || (o[0] == 192 && o[1] == 168)
}

/// 接口名称是否匹配已知虚拟/VPN/容器适配器模式。
/// 覆盖 macOS / Linux / Windows 三端常见名称。
fn is_virtual_interface_name(name: &str) -> bool {
    let n = name.to_lowercase();
    let patterns = [
        "utun",
        "tun",
        "tap",
        "wg", // VPN / WireGuard
        "docker",
        "br-",
        "veth",
        "virbr", // Docker / libvirt
        "vmnet",
        "vboxnet", // VMware / VirtualBox
        "hyper-v",
        "hv_",
        "vethernet", // Hyper-V
        "cf-",
        "clash",
        "wintun", // Clash / Cloudflare WARP / WinTun
        "tailscale",
        "ts-", // Tailscale
        "ham",
        "vpn",
        "vgate", // 通用 VPN / 企业 VPN
    ];
    patterns.iter().any(|p| n.contains(p))
}

/// 为一个 IPv4 候选接口评分（越高越像真实 LAN）。
///
/// 评分维度（可解释、确定性，不依赖 `get_if_addrs()` 返回顺序）：
///   +10  有 broadcast 地址（真实 LAN 的核心标志）
///   +5   RFC1918 私网地址（10.x / 172.16-31.x / 192.168.x）
///   -50  虚拟地址段（198.18/15 / 100.64/10 / 169.254/16）
///   -30  虚拟接口名称（tun / docker / vmnet / wg / hyper-v 等）
fn score_candidate(ip: &Ipv4Addr, name: &str, has_broadcast: bool) -> i32 {
    let mut score = 0;
    if has_broadcast {
        score += 10;
    }
    if is_rfc1918(ip) {
        score += 5;
    }
    if is_virtual_ip(ip) {
        score -= 50;
    }
    if is_virtual_interface_name(name) {
        score -= 30;
    }
    score
}

/// 自动模式下检测最佳 LAN 网卡（IP + broadcast 地址）。
///
/// 使用评分函数而非「第一个匹配就返回」：即使系统上存在多个有 broadcast 的
/// 接口（Docker bridge / VMware / 真实 LAN），评分机制也能稳定选出真实 LAN，
/// 且结果不依赖 `get_if_addrs()` 的返回顺序。
///
/// 返回 `(lan_ip, broadcast_addr)`：
/// - `lan_ip`：用于 `IP_MULTICAST_IF`（强制组播出口）和 `join_multicast_v4`
/// - `broadcast_addr`：用于将 UDP 广播发到精确子网地址（如 `192.168.1.255`），
///   而非 `255.255.255.255`，确保广播不会因默认路由进入 VPN 适配器
fn find_lan_interface() -> Option<(Ipv4Addr, Ipv4Addr)> {
    let ifs = if_addrs::get_if_addrs().ok()?;
    // 平局时按「非虚拟名优先、名称字典序」确定，避免返回顺序影响结果。
    let mut best: Option<(Ipv4Addr, Ipv4Addr, i32, bool, String)> = None;
    for i in &ifs {
        if let if_addrs::IfAddr::V4(v4) = &i.addr {
            let ip = match i.ip() {
                std::net::IpAddr::V4(v) => v,
                _ => continue,
            };
            if i.is_loopback() {
                continue;
            }
            let Some(bc) = v4.broadcast else {
                continue;
            };
            let score = score_candidate(&ip, &i.name, true);
            if score <= 0 {
                continue;
            }
            let non_virtual = !is_virtual_interface_name(&i.name);
            let better = best.as_ref().map_or(true, |(_, _, s, nv, name)| {
                score > *s
                    || (score == *s && non_virtual > *nv)
                    || (score == *s && non_virtual == *nv && i.name < *name)
            });
            if better {
                best = Some((ip, bc, score, non_virtual, i.name.clone()));
            }
        }
    }
    best.map(|(ip, bc, _, _, _)| (ip, bc))
}

/// 决定 Discovery 实际绑定的 IP。
///
/// - Auto（`ip == 0.0.0.0`）：使用 `find_lan_interface` 选出的真实 LAN IP。
/// - Manual：使用用户指定的 IP。
///
/// 抽成纯函数以便单测，不依赖 Tauri AppHandle 或真实网络。
fn resolve_bind_ip(
    ip: Ipv4Addr,
    find_lan: impl FnOnce() -> Option<(Ipv4Addr, Ipv4Addr)>,
) -> Result<(Ipv4Addr, Option<Ipv4Addr>), String> {
    if ip.is_unspecified() {
        let (lan_ip, bc) =
            find_lan().ok_or("auto mode: no eligible LAN interface found".to_string())?;
        Ok((lan_ip, Some(bc)))
    } else {
        Ok((ip, None))
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 绑定一个允许地址复用的 UDP 套接字（SO_REUSEADDR + SO_BROADCAST + unix 下 SO_REUSEPORT），
/// 使同一台机器上的多个 gosslan 实例能同时监听同一发现端口（Windows/macOS/Linux 通用）。
///
/// 组播出口接口 `IP_MULTICAST_IF` 固定设为 `ip`（与 bind 地址一致）：
/// 自动模式下强制走真实 LAN，避免组播被 VPN 默认路由劫持。
/// 设置失败直接让 Discovery 启动失败，不再静默忽略。
fn bind_udp_reusable(ip: Ipv4Addr, port: u16) -> Result<UdpSocket, String> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::SocketAddr;

    let addr: SocketAddr = format!("{ip}:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| format!("create UDP socket: {e}"))?;
    sock.set_reuse_address(true)
        .map_err(|e| format!("SO_REUSEADDR: {e}"))?;
    // macOS/BSD：UDP 同端口多开必须 SO_REUSEPORT（SO_REUSEADDR 仅 Windows 允许重复绑定）。
    // 缺了它，同一台机器的第二个实例 network::start 会报 "Address already in use"，
    // 单机多实例互发现直接失效。
    #[cfg(unix)]
    sock.set_reuse_port(true).map_err(|e| format!("SO_REUSEPORT: {e}"))?;
    sock.set_broadcast(true)
        .map_err(|e| format!("SO_BROADCAST: {e}"))?;
    // 设置组播出口接口：必须与 bind IP 一致，失败直接报错。
    sock.set_multicast_if_v4(&ip)
        .map_err(|e| format!("set_multicast_if_v4({ip}): {e}"))?;
    // tokio 要求注册进 runtime 的 fd 必须非阻塞：socket2 创建的是阻塞 socket，
    // 直接 from_std 在 debug 构建会 panic（tokio blocking check），release 构建虽不 panic
    // 但阻塞 fd 挂在 kqueue/epoll 上会卡死 worker 线程（界面卡顿的帮凶之一）。
    sock.set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    let sock_addr: socket2::SockAddr = addr.into();
    sock.bind(&sock_addr)
        .map_err(|e| format!("UDP bind {ip}:{port} failed: {e}"))?;

    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).map_err(|e| format!("register UDP socket: {e}"))
}

fn announce_packet(state: &AppState, tcp_port: u16) -> UdpPacket {
    // 注意：announce 不携带 avatar——头像可能很大，塞进 UDP 广播会超报文上限
    // （EMSGSIZE "Message too long"）导致发现失效；头像改由 TCP 建链后的 UserInfo 同步。
    UdpPacket {
        kind: "announce".to_string(),
        device_id: state.device_id.clone(),
        nickname: state.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        tcp_port,
        x25519_pubkey: Some(state.identity.x25519_public_b64()),
        ed25519_pubkey: Some(state.identity.ed25519_public_b64()),
    }
}

/// 启动发现任务。
pub async fn spawn(
    state: Arc<AppState>,
    ip: Ipv4Addr,
    tcp_port: u16,
    shutdown: watch::Receiver<bool>,
    mut probe: watch::Receiver<u64>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    // Auto 模式：解析真实 LAN IP；Manual 模式：使用用户指定 IP。
    // 核心修复：UDP 不再绑定 0.0.0.0，而是绑定到真实 LAN IP，避免 Windows Restart/Exit→reopen
    // 后组播/广播出口被错误路由到 VPN/虚拟适配器，导致对端收不到 announce。
    let (udp_bind_ip, lan_broadcast) =
        resolve_bind_ip(ip, find_lan_interface).map_err(|e| e.to_string())?;
    let multicast_iface = udp_bind_ip;

    // 绑定 UDP 端口：失败、multicast_if 设置失败都直接让 Discovery 启动失败。
    let socket = bind_udp_reusable(udp_bind_ip, UDP_PORT)
        .map_err(|e| format!("UDP discovery bind failed: {e}"))?;
    // 加入组播组：必须与 bind IP 使用同一接口；失败直接让启动失败。
    socket
        .join_multicast_v4(MULTICAST_GROUP, multicast_iface)
        .map_err(|e| {
            format!(
                "join multicast {MULTICAST_GROUP} on {multicast_iface} failed: {e}"
            )
        })?;
    let socket = Arc::new(socket);

    // 记录诊断：启动参数（bound_ip 必须是真实 LAN IP，供前端开发者面板展示）
    {
        let mut diag = state.diag.lock().unwrap_or_else(|e| e.into_inner());
        diag.bound_ip = udp_bind_ip.to_string();
        diag.selected_ip = udp_bind_ip.to_string();
        diag.broadcast_target = "255.255.255.255".into();
        diag.multicast_group = format!("{MULTICAST_GROUP}");
        diag.multicast_join_result = "ok".into();
        diag.multicast_if_result = "ok".into();
        diag.udp_port = UDP_PORT;
        diag.selected_interface = if_addrs::get_if_addrs()
            .ok()
            .into_iter()
            .flatten()
            .find_map(|i| {
                if let if_addrs::IfAddr::V4(_) = &i.addr {
                    if i.ip() == std::net::IpAddr::V4(udp_bind_ip) {
                        return Some(i.name.clone());
                    }
                }
                None
            })
            .unwrap_or_default();
    }
    state.push_diag_event(
        "discovery_started",
        &format!("bind={udp_bind_ip}, multicast_iface={multicast_iface}"),
    );

    let my_id = state.device_id.clone();

    // ---- 接收循环 ----
    let recv_task = {
        let socket = socket.clone();
        let state = state.clone();
        let my_id = my_id.clone();
        let mut shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => break,
                    res = socket.recv_from(&mut buf) => {
                        let (len, src) = match res {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let Ok(pkt) = serde_json::from_slice::<UdpPacket>(&buf[..len]) else {
                            continue;
                        };
                        if pkt.device_id == my_id {
                            continue;
                        }
                        match pkt.kind.as_str() {
                            "announce" => {
                                state.push_diag_event("announce_recv", &format!("from={} via={}", pkt.device_id, src.ip()));
                                // 不用对端时间戳推算 RTT：那依赖双方时钟同步，纯本地业务不应
                                // 假设对端时钟。这里只做在线发现与建链，时延指标留待真正的往返测量。
                                upsert_peer(
                                    &state,
                                    &pkt.device_id,
                                    &pkt.nickname,
                                    None,
                                    "",
                                    &src.ip().to_string(),
                                    pkt.tcp_port,
                                    pkt.x25519_pubkey.clone(),
                                    pkt.ed25519_pubkey.clone(),
                                    None,
                                ).await;
                                ensure_link(
                                    &state,
                                    &pkt.device_id,
                                    &src.ip().to_string(),
                                    pkt.tcp_port,
                                    shutdown.clone(),
                                )
                                .await;
                            }
                            "who_has" => {
                                // 惊群治理：who_has 是「打开添加好友」时向全网发的一次探测，
                                // 收到就立刻回包的话，1000 个节点会在同一瞬间把请求方的收包
                                // 与建链路径打满（回包风暴 + 一次性 ensure_link）。
                                // 这里让每个节点各自等一个 0~500ms 随机时长再回，
                                // 把回包摊开；对"打开添加好友"的感知延迟影响可忽略。
                                let socket = socket.clone();
                                let state = state.clone();
                                tokio::spawn(async move {
                                    let jitter = OsRng.next_u64() % 500;
                                    tokio::time::sleep(Duration::from_millis(jitter)).await;
                                    let reply = announce_packet(&state, tcp_port);
                                    if let Ok(data) = serde_json::to_vec(&reply) {
                                        let _ = socket.send_to(&data, src).await;
                                    }
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
        })
    };

    // ---- 广播循环（自适应周期 + 抖动，避免大规模节点广播风暴与同步惊群） ----
    let broadcast_task = {
        let socket = socket.clone();
        let state = state.clone();
        let lan_broadcast = lan_broadcast;
        let mut shutdown = shutdown.clone();
        tokio::spawn(async move {
            // 首次立刻广播
            broadcast(&socket, &state, tcp_port, lan_broadcast).await;
            // 下一轮周期广播的时刻。只在真正广播后重算，探测分支不改变节拍。
            let mut next_at = Instant::now() + Duration::from_secs(next_wait(&state));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => break,
                    _ = sleep_until(next_at) => {
                        broadcast(&socket, &state, tcp_port, lan_broadcast).await;
                        sweep_peers(&state);
                        next_at = Instant::now() + Duration::from_secs(next_wait(&state));
                    }
                    // 按需探测：用户打开「添加好友」时触发一次 who_has 群发
                    _ = probe.changed() => {
                        broadcast_probe(&socket, &state, tcp_port, lan_broadcast).await;
                    }
                }
            }
        })
    };

    Ok(vec![recv_task, broadcast_task])
}

/// 下一轮广播周期（秒）：读一次在线节点数即放锁，绝不跨 await 持锁。
fn next_wait(state: &AppState) -> u64 {
    let node_count = state.peers.lock().unwrap_or_else(|e| e.into_inner()).len();
    adaptive_interval(node_count)
}

/// 依据当前在线节点数自适应调整广播周期（秒），并叠加随机抖动以打散各节点广播相位。
fn adaptive_interval(node_count: usize) -> u64 {
    let base = if node_count >= 500 {
        20
    } else if node_count >= 100 {
        10
    } else {
        ANNOUNCE_INTERVAL_SECS
    };
    // 0..=2s 抖动，避免全网节点在同一瞬间齐发
    let mut rng = OsRng;
    let jitter = rng.next_u64() % 3000;
    base + jitter / 1000
}

/// 根据 `send_to` 结果生成诊断事件（纯函数，便于单测）。
/// Ok(n) → success_kind；Err → broadcast_error。
fn diag_event_from_send_result(
    target: &str,
    success_kind: &'static str,
    res: &std::io::Result<usize>,
) -> (&'static str, String) {
    match res {
        Ok(n) => (success_kind, format!("bytes={n}, target={target}")),
        Err(e) => ("broadcast_error", format!("target={target}, error={e}")),
    }
}

async fn broadcast(
    socket: &UdpSocket,
    state: &AppState,
    tcp_port: u16,
    _lan_broadcast: Option<Ipv4Addr>,
) {
    let pkt = announce_packet(state, tcp_port);
    let Ok(data) = serde_json::to_vec(&pkt) else {
        return;
    };
    // 广播使用 limited broadcast（255.255.255.255）：Windows 默认禁用 directed broadcast
    // （DisableDirectedBroadcasts=1），精确子网地址会被内核静默丢弃。
    // limited broadcast 发送到所有 IFF_BROADCAST 接口，不走默认路由，跨平台可靠。
    let bcast_target = format!("255.255.255.255:{UDP_PORT}");
    let bcast_res = socket.send_to(&data, &bcast_target).await;
    let (kind, detail) = diag_event_from_send_result(&bcast_target, "broadcast_sent", &bcast_res);
    state.push_diag_event(kind, &detail);

    let mcast_target = format!("{MULTICAST_GROUP}:{UDP_PORT}");
    let mcast_res = socket.send_to(&data, &mcast_target).await;
    let (kind, detail) = diag_event_from_send_result(&mcast_target, "multicast_sent", &mcast_res);
    state.push_diag_event(kind, &detail);
}

/// 按需探测：群发 `who_has` 请求周围节点单播回复其 `announce`，并同时广播一次自身 announce。
/// 用于「添加好友」弹窗打开时快速、主动地发现局域网内在线客户端。
async fn broadcast_probe(
    socket: &UdpSocket,
    state: &AppState,
    tcp_port: u16,
    _lan_broadcast: Option<Ipv4Addr>,
) {
    let who = UdpPacket {
        kind: "who_has".to_string(),
        device_id: state.device_id.clone(),
        nickname: String::new(),
        tcp_port,
        x25519_pubkey: None,
        ed25519_pubkey: None,
    };
    if let Ok(data) = serde_json::to_vec(&who) {
        let bcast_target = format!("255.255.255.255:{UDP_PORT}");
        let bcast_res = socket.send_to(&data, &bcast_target).await;
        let (kind, detail) = diag_event_from_send_result(&bcast_target, "who_has_sent", &bcast_res);
        state.push_diag_event(kind, &detail);
    }
    // 同时广播自身，让周围节点也能立刻发现我们
    broadcast(socket, state, tcp_port, _lan_broadcast).await;
}

/// 判定一个节点是否应保留在 peers 表（纯逻辑，便于单测 + 护栏非空转）。
/// - 有活跃 TCP 链接 → 恒保留（豁免超时，避免「能通信却显示离线」）。
/// - 无活跃链接（跨跳节点，靠 Presence 经中继保活）→ last_seen 在
///   `RELAY_PEER_TIMEOUT_SECS` 内才保留。
fn should_keep_peer(last_seen: i64, now: i64, has_active_link: bool) -> bool {
    has_active_link || last_seen >= now - RELAY_PEER_TIMEOUT_SECS * 1000
}

/// 清理超过超时阈值的节点：
/// - **有活跃 TCP 链接**的节点：直接豁免（避免「TCP 能通信但 UI 显示离线」）。
/// - **无活跃链接**的节点（跨跳节点，靠 Presence 经中继保活）：用更长的
///   `RELAY_PEER_TIMEOUT_SECS` 判定，容忍 Tailscale 等高延迟中继的转发抖动——
///   否则 10s Presence 周期 + 15s 超时只留 5s 余量，Presence 一迟到就被误删，
///   在线状态「一会儿绿一会儿灰」。
fn sweep_peers(state: &AppState) {
    let now = now_ms();
    // try_lock 非阻塞：锁被占用时跳过本轮清理（下轮会补上），绝不阻塞广播循环。
    // ⚠️ 只把**非空** Vec 算作「有活跃链路」：`empty()==true` 的 key 曾经会残留
    // （reader_loop 只 retain 不删 key，已在 transport 侧修掉），而 `keys()` 不区分
    // 空与非空 ⇒ 一次「连过又掉线」的节点永不被清扫、UI 永久显示在线。
    // 这里保留非空判据作为第二道防线（与 `AppState::has_link` 口径一致）。
    let active_links: std::collections::HashSet<String> = state
        .links
        .try_lock()
        .map(|l| {
            l.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let changed = {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        let before = peers.len();
        peers.retain(|id, p| should_keep_peer(p.last_seen, now, active_links.contains(id)));
        before != peers.len()
    };
    if changed {
        state.emit_peers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跨跳节点离线判定：45s 超时 + 直连节点豁免（护栏非空转的关键判据）。
    ///
    /// 真机反馈「A 看 C 一会儿绿一会儿灰、C 看 A 一直绿」：跨跳节点靠 10s Presence
    /// 保活，旧 15s 超时只留 5s 余量，Tailscale 中继抖动一迟到就被误删。
    #[test]
    fn relay_peer_kept_within_45s_direct_peer_always_kept() {
        let now = 1_000_000_000_000_i64;
        // 直连节点：last_seen 远超 45s，但有活跃链接 → 恒保留
        assert!(should_keep_peer(now - 100_000, now, true));
        // 跨跳节点：20s 前（旧 15s 阈值早已超，但新 45s 内）→ 保留（修复点）
        assert!(should_keep_peer(now - 20_000, now, false));
        // 跨跳节点：30s 前（45s 内）→ 保留
        assert!(should_keep_peer(now - 30_000, now, false));
        // 跨跳节点：44s 前（45s 内，边界）→ 保留
        assert!(should_keep_peer(now - 44_000, now, false));
        // 跨跳节点：46s 前（超 45s）→ 移除
        assert!(!should_keep_peer(now - 46_000, now, false));
    }

    /// 自适应周期分档与抖动边界：≤99 节点保持 5s，≥100 → ≥10s，≥500 → ≥20s，抖动 ≤2s。
    #[test]
    fn adaptive_interval_thresholds_and_jitter_bounds() {
        for _ in 0..200 {
            let small = adaptive_interval(0);
            let mid = adaptive_interval(99);
            let big = adaptive_interval(100);
            let huge = adaptive_interval(500);
            assert!(
                (5..=7).contains(&small),
                "0 节点应为 5+0..2 秒，得到 {small}"
            );
            assert!((5..=7).contains(&mid), "99 节点仍属小规模，得到 {mid}");
            assert!(
                (10..=12).contains(&big),
                "100 节点应降频到 10+0..2 秒，得到 {big}"
            );
            assert!(
                (20..=22).contains(&huge),
                "500 节点应降频到 20+0..2 秒，得到 {huge}"
            );
            // 抖动幅度必须小于档间间隔，否则扩档形同无效（热区里退化不成阶梯）
            assert!(small < big && big < huge);
        }
    }

    /// 回归 P0-1：广播循环必须按「完整周期」等待。
    ///
    /// tokio 新建的 `Interval` 首个 tick 立即就绪——旧实现在循环里重建 interval，
    /// 等于每轮等待清零，announce 退化为热循环（实测约 1ms/轮）。
    /// 现在循环里没有可重建的计时器，只有指向固定 deadline 的 `sleep_until`；
    /// 这里锁住两种写法的时序差异，防止改回去。
    #[tokio::test]
    async fn recreated_interval_never_waits_but_deadline_sleep_does() {
        let period = Duration::from_millis(200);

        // 旧写法（Bug 形态）：每轮重建 interval → 等待被清零
        let mut tick = tokio::time::interval(period);
        tick.tick().await;
        let start = Instant::now();
        for _ in 0..3 {
            tick = tokio::time::interval(period);
            tick.tick().await;
            assert!(
                start.elapsed() < period * 3,
                "重建 interval 本应不产生完整等待（这正是 P0-1）"
            );
        }

        // 新写法（修复形态）：同一 deadline 上等待 → 必须消耗完整周期
        let deadline = Instant::now() + period;
        sleep_until(deadline).await;
        assert!(Instant::now() >= deadline);
    }

    /// 回归 P0-1 的调度语义：探测（who_has）分支不得推迟周期广播节拍，
    /// deadline 过期后应立刻补发一轮（等待归零而非重新计时）。
    #[tokio::test]
    async fn expired_deadline_is_ready_immediately() {
        let past = Instant::now() - Duration::from_secs(1);
        let start = Instant::now();
        sleep_until(past).await;
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "过期 deadline 应立即就绪"
        );
    }

    // ---- 评分函数与接口识别测试 ----

    /// RFC1918 三段全部命中
    #[test]
    fn is_rfc1918_covers_private_ranges() {
        assert!(is_rfc1918(&"10.0.0.1".parse().unwrap()));
        assert!(is_rfc1918(&"10.255.255.255".parse().unwrap()));
        assert!(is_rfc1918(&"172.16.0.1".parse().unwrap()));
        assert!(is_rfc1918(&"172.31.255.255".parse().unwrap()));
        assert!(is_rfc1918(&"192.168.1.1".parse().unwrap()));
        assert!(is_rfc1918(&"192.168.0.1".parse().unwrap()));
        // 非 RFC1918 不误判
        assert!(!is_rfc1918(&"172.15.0.1".parse().unwrap()));
        assert!(!is_rfc1918(&"172.32.0.1".parse().unwrap()));
        assert!(!is_rfc1918(&"192.169.0.1".parse().unwrap()));
        assert!(!is_rfc1918(&"8.8.8.8".parse().unwrap()));
    }

    /// 非 LAN 地址段全部识别
    #[test]
    fn is_virtual_ip_covers_non_lan_ranges() {
        assert!(is_virtual_ip(&"198.18.0.1".parse().unwrap()));
        assert!(is_virtual_ip(&"198.19.255.254".parse().unwrap()));
        assert!(is_virtual_ip(&"100.64.0.1".parse().unwrap()));
        assert!(is_virtual_ip(&"100.127.255.254".parse().unwrap()));
        assert!(is_virtual_ip(&"169.254.1.1".parse().unwrap()));
        // 合法 LAN 不误判
        assert!(!is_virtual_ip(&"192.168.1.100".parse().unwrap()));
        assert!(!is_virtual_ip(&"10.0.0.1".parse().unwrap()));
        assert!(!is_virtual_ip(&"172.16.0.1".parse().unwrap()));
    }

    /// 虚拟接口名称覆盖 macOS / Linux / Windows 三端
    #[test]
    fn is_virtual_interface_name_covers_common_patterns() {
        // VPN / WireGuard
        assert!(is_virtual_interface_name("utun3"));
        assert!(is_virtual_interface_name("tun0"));
        assert!(is_virtual_interface_name("wg0"));
        assert!(is_virtual_interface_name("tailscale0"));
        // Docker / libvirt
        assert!(is_virtual_interface_name("docker0"));
        assert!(is_virtual_interface_name("br-abcdef"));
        assert!(is_virtual_interface_name("veth1234"));
        assert!(is_virtual_interface_name("virbr0"));
        // VMware / VirtualBox
        assert!(is_virtual_interface_name("vmnet8"));
        assert!(is_virtual_interface_name("vboxnet0"));
        // Hyper-V
        assert!(is_virtual_interface_name("vEthernet (Default Switch)"));
        // Clash / WARP
        assert!(is_virtual_interface_name("ClashMeta"));
        assert!(is_virtual_interface_name("cf-warp"));
        // 真实 LAN 名称不误判
        assert!(!is_virtual_interface_name("en0"));
        assert!(!is_virtual_interface_name("eth0"));
        assert!(!is_virtual_interface_name("wlan0"));
        assert!(!is_virtual_interface_name("Wi-Fi"));
        assert!(!is_virtual_interface_name("以太网"));
        // 实测确认的企业 VPN 适配器
        assert!(is_virtual_interface_name("vgateO"));
    }

    /// 评分确定性：不依赖输入顺序，虚拟接口名始终排在真实 LAN 之后
    #[test]
    fn score_candidate_orders_correctly() {
        // 真实 LAN：broadcast + RFC1918 = +15
        let real_lan = score_candidate(&"192.168.1.100".parse().unwrap(), "en0", true);
        assert_eq!(real_lan, 15);

        // Docker bridge：broadcast + RFC1918 - 虚拟名 = +10+5-30 = -15
        let docker = score_candidate(&"172.17.0.1".parse().unwrap(), "docker0", true);
        assert_eq!(docker, -15);

        // Clash tun：无 broadcast + 虚拟地址 + 虚拟名 = -80
        let clash = score_candidate(&"198.18.0.1".parse().unwrap(), "utun3", false);
        assert_eq!(clash, -80);

        // 真实 LAN（非 RFC1918，如公网 IP）：broadcast = +10
        let public_lan = score_candidate(&"203.0.113.5".parse().unwrap(), "eth0", true);
        assert_eq!(public_lan, 10);

        // 没有 broadcast 的普通接口 = 0
        let no_bcast = score_candidate(&"192.168.1.5".parse().unwrap(), "en0", false);
        assert_eq!(no_bcast, 5);

        // 真实 LAN 永远 > Docker/VMware/Hyper-V（即使后者也有 broadcast）
        assert!(
            real_lan > docker,
            "真实 LAN {real_lan} 应高于 Docker {docker}"
        );
    }

    /// score_candidate 不受 RFC1918 地址范围误判影响
    #[test]
    fn score_candidate_rfc1918_boundary() {
        // 172.15.x.x 不是 RFC1918（紧邻 172.16 但不在范围内）
        let borderline_below = score_candidate(&"172.15.0.1".parse().unwrap(), "en0", true);
        assert_eq!(borderline_below, 10, "172.15 应无 RFC1918 加分");
        // 172.16.x.x 是 RFC1918
        let borderline_above = score_candidate(&"172.16.0.1".parse().unwrap(), "en0", true);
        assert_eq!(borderline_above, 15, "172.16 应有 RFC1918 加分");
    }

    // ---- P0 回归测试：Auto bind / vgate0 / send 诊断 / multicast join 失败 ----

    /// 1. Auto 模式必须绑定真实 LAN IP，而不是 0.0.0.0。
    #[test]
    fn resolve_bind_ip_auto_uses_lan_ip() {
        let auto = Ipv4Addr::UNSPECIFIED;
        let lan_ip: Ipv4Addr = "10.1.19.45".parse().unwrap();
        let bc: Ipv4Addr = "10.1.19.255".parse().unwrap();

        let (bind_ip, lan_bc) = resolve_bind_ip(auto, || Some((lan_ip, bc))).unwrap();
        assert_eq!(bind_ip, lan_ip, "Auto 模式应绑定真实 LAN IP");
        assert_eq!(lan_bc, Some(bc));
    }

    /// Manual 模式保持用户指定 IP，find_lan_interface 不应被调用。
    #[test]
    fn resolve_bind_ip_manual_uses_user_ip() {
        let manual: Ipv4Addr = "192.168.1.50".parse().unwrap();
        let (bind_ip, lan_bc) = resolve_bind_ip(manual, || unreachable!()).unwrap();
        assert_eq!(bind_ip, manual);
        assert!(lan_bc.is_none());
    }

    /// 2. 企业 VPN 虚拟网卡 vgate0 永远不能成为 bind 地址。
    #[test]
    fn vgate0_never_selected_as_bind_address() {
        // vgate0：RFC1918 + broadcast + 虚拟名 = -15，直接出局
        let vgate_score = score_candidate(&"10.20.30.40".parse().unwrap(), "vgate0", true);
        assert!(
            vgate_score <= 0,
            "vgate0 分数必须 ≤0 才能被过滤，实际 {vgate_score}"
        );

        // 只有 vgate0 可选时，决策结果为空
        let only_vgate = vec![("10.20.30.40".parse().unwrap(), "vgate0".to_string(), true)];
        assert!(
            pick_best_for_test(&only_vgate).is_none(),
            "仅有 vgate0 时不应选择任何接口"
        );

        // vgate0 + 真实 LAN：真实 LAN 必须胜出
        let mixed = vec![
            ("10.20.30.40".parse().unwrap(), "vgate0".to_string(), true),
            (
                "192.168.1.100".parse().unwrap(),
                "以太网".to_string(),
                true,
            ),
        ];
        let best = pick_best_for_test(&mixed).unwrap();
        assert_eq!(best.0, "192.168.1.100".parse::<Ipv4Addr>().unwrap());
        assert_eq!(best.1, "以太网");
    }

    /// 测试辅助：模拟 find_lan_interface 的选择逻辑（输入 `(ip, name, has_broadcast)`）。
    fn pick_best_for_test(candidates: &[(Ipv4Addr, String, bool)]) -> Option<(Ipv4Addr, String)> {
        let mut best: Option<(Ipv4Addr, String, i32, bool)> = None;
        for (ip, name, has_bcast) in candidates {
            if !has_bcast {
                continue;
            }
            let score = score_candidate(ip, name, true);
            if score <= 0 {
                continue;
            }
            let non_virtual = !is_virtual_interface_name(name);
            let better = best.as_ref().map_or(true, |(_, _, s, nv)| {
                score > *s || (score == *s && non_virtual > *nv)
            });
            if better {
                best = Some((*ip, name.clone(), score, non_virtual));
            }
        }
        best.map(|(ip, name, _, _)| (ip, name))
    }

    /// 3. broadcast_sent / multicast_sent 事件只在 send_to 返回 Ok 时产生；
    /// Err 时必须产生 broadcast_error / multicast_error。
    #[test]
    fn broadcast_diag_event_reflects_send_result() {
        let target = "255.255.255.255:59991";
        let ok: std::io::Result<usize> = Ok(123);
        let err: std::io::Result<usize> =
            Err(std::io::Error::new(std::io::ErrorKind::Other, "mock fail"));

        let (ok_kind, ok_detail) = diag_event_from_send_result(target, "broadcast_sent", &ok);
        let (err_kind, err_detail) = diag_event_from_send_result(target, "broadcast_sent", &err);

        assert_eq!(ok_kind, "broadcast_sent");
        assert!(ok_detail.contains("bytes=123"), "{ok_detail}");
        assert_eq!(err_kind, "broadcast_error");
        assert!(err_detail.contains("error=mock fail"), "{err_detail}");

        // 组播事件同理，只是 kind 不同
        let (m_ok_kind, _) = diag_event_from_send_result("239.255.42.99:59991", "multicast_sent", &ok);
        assert_eq!(m_ok_kind, "multicast_sent");
    }

    /// 4. 组播加入失败必须向上传播为启动失败，不再静默忽略。
    ///
    /// 用非本地/非法接口地址（240.0.0.1 不是本机任何接口）触发 join_multicast_v4 失败，
    /// 验证代码路径不吞错。
    #[tokio::test]
    async fn multicast_join_failure_propagates() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let invalid_iface: Ipv4Addr = "240.0.0.1".parse().unwrap();
        let res = socket.join_multicast_v4(MULTICAST_GROUP, invalid_iface);
        assert!(
            res.is_err(),
            "在 {invalid_iface} 上加入组播应当失败，但实际成功：{res:?}"
        );
    }
}
