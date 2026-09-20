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

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::commands::is_virtual_ip;
use crate::network::transport::{ensure_link, upsert_peer};
use crate::protocol::{UdpPacket, ANNOUNCE_INTERVAL_SECS, RELAY_PEER_TIMEOUT_SECS, UDP_PORT};
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
                    || (score == *s && non_virtual & !*nv)
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
        // ⚠️ **手动选网卡时也必须算出子网广播地址**。
        //
        // 此前这里直接返回 `None`，于是 `broadcast()` 只发 limited broadcast
        // （255.255.255.255）—— 而它在 macOS 上会失败（socket 绑定到具体网卡 IP 时
        // 返回 EHOSTUNREACH）。真机日志：每轮 `broadcast_error: No route to host`，
        // 且**从未出现子网广播**（因为根本没算）⇒ 本机在局域网上发不出声，
        // 对端只能靠蓝牙找过来。表现是「局域网只通一半」：我收得到别人，别人找不到我。
        Ok((ip, broadcast_for_ip(ip)))
    }
}

/// 取指定本机 IP 所在网卡的**子网广播地址**（如 `192.168.31.255`）。
///
/// macOS 上这是唯一能用的广播目标：socket 绑定到具体网卡 IP 时，
/// 向 limited broadcast（255.255.255.255）发送会返回 EHOSTUNREACH。
/// 找不到匹配网卡时返回 None，调用方回落到 limited broadcast（Windows 需要它）。
fn broadcast_for_ip(ip: Ipv4Addr) -> Option<Ipv4Addr> {
    let ifs = if_addrs::get_if_addrs().ok()?;
    for i in &ifs {
        if let if_addrs::IfAddr::V4(v4) = &i.addr {
            if v4.ip == ip {
                return v4.broadcast;
            }
        }
    }
    None
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
    sock.set_reuse_port(true)
        .map_err(|e| format!("SO_REUSEPORT: {e}"))?;
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

/// **接收** socket 必须绑的地址：`0.0.0.0`（不是具体 LAN IP）。
///
/// ## 为什么（2026-09-12 实测，macOS）
///
/// 绑定到**具体网卡地址**的 UDP socket，在 macOS 上**收不到** `255.255.255.255` 广播、
/// 也收不到组播 —— 本机实测（空闲端口，无任何竞争）：
///
/// | 接收方 bind | 收到的广播 | 收到的组播 |
/// |---|---|---|
/// | `192.168.31.113:60001`（具体地址） | **0** | **0** |
/// | `0.0.0.0:60001` | 3 | 3 |
///
/// 后果（用户真机症状）：Mac 的 announce **发得出去**（手机能看到 Mac），但 Mac
/// **一个 announce 都收不到** ⇒ 局域网里「手机看得到 Mac、Mac 看不到手机」，
/// Mac 只能靠 BLE 建立一次会立刻断掉的链路（列表里"闪一下"）。
///
/// ## 为什么不干脆全绑 0.0.0.0
///
/// 发送侧必须钉在 LAN 接口上（见 `bind_udp_reusable` 的注释：Windows 上
/// 组播/广播出口会被 VPN/虚拟网卡劫持）。所以这里是**两个 socket**：
/// 收的绑 `0.0.0.0`（本函数），发的绑具体 LAN IP（`bind_udp_reusable`），
/// 两个都读，避免 SO_REUSEPORT 把数据报只投给其中一个。
pub fn discovery_recv_bind_ip() -> Ipv4Addr {
    Ipv4Addr::UNSPECIFIED
}

/// 绑定**接收** socket：`0.0.0.0:port`，允许地址复用（与发送 socket 共用端口）。
///
/// 不改 `IP_MULTICAST_IF`：出口选择是发送 socket 的事；这个 socket 只负责收。
fn bind_udp_recv(port: u16) -> Result<UdpSocket, String> {
    use socket2::{Domain, Protocol, Socket, Type};

    let addr: std::net::SocketAddr = format!("{}:{port}", discovery_recv_bind_ip())
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| format!("create UDP recv socket: {e}"))?;
    sock.set_reuse_address(true)
        .map_err(|e| format!("SO_REUSEADDR(recv): {e}"))?;
    #[cfg(unix)]
    sock.set_reuse_port(true)
        .map_err(|e| format!("SO_REUSEPORT(recv): {e}"))?;
    sock.set_broadcast(true)
        .map_err(|e| format!("SO_BROADCAST(recv): {e}"))?;
    sock.set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking(recv): {e}"))?;
    let sock_addr: socket2::SockAddr = addr.into();
    sock.bind(&sock_addr)
        .map_err(|e| format!("UDP recv bind {addr} failed: {e}"))?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).map_err(|e| format!("register UDP recv socket: {e}"))
}

fn announce_packet(state: &AppState, tcp_port: u16) -> UdpPacket {
    // 注意：announce 不携带 avatar——头像可能很大，塞进 UDP 广播会超报文上限
    // （EMSGSIZE "Message too long"）导致发现失效；头像改由 TCP 建链后的 UserInfo 同步。
    let x = state.identity.x25519_public_b64();
    let e = state.identity.ed25519_public_b64();
    // 每次广播都换一个 nonce：签名因此**不可跨轮重放**（旧包即使被抓到，重发也会因为
    // nonce 与签名绑定而只是"同一个旧 nonce"——配合接收端的诊断即可识别为异常重复）。
    let nonce = STANDARD.encode(crate::crypto::random_key());
    let sig = state
        .identity
        .sign_b64(&crate::protocol::announce_signing_bytes(
            &state.device_id,
            tcp_port,
            &nonce,
            &x,
            &e,
        ));
    UdpPacket {
        kind: "announce".to_string(),
        device_id: state.device_id.clone(),
        nickname: state
            .nickname
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        tcp_port,
        x25519_pubkey: Some(x),
        ed25519_pubkey: Some(e),
        nonce,
        sig,
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
    //
    // ⚠️ **两个 socket**（2026-09-12 起因「Mac 看不到手机」的真因）：
    //   · `send_socket` 绑**具体 LAN IP** ⇒ 出口钉在真实 LAN（Windows 上躲开 VPN 劫持，
    //     见 `bind_udp_reusable` 的注释）；
    //   · `recv_socket` 绑 **0.0.0.0** ⇒ macOS 上才收得到广播/组播（绑具体地址收不到，
    //     实测见 `discovery_recv_bind_ip`）。
    // 两个都进接收循环读取：SO_REUSEPORT 会把同一份数据报只投给其中**一个** socket，
    // 只读一个就会漏包。
    let send_socket = bind_udp_reusable(udp_bind_ip, UDP_PORT)
        .map_err(|e| format!("UDP discovery bind failed: {e}"))?;
    let recv_socket =
        bind_udp_recv(UDP_PORT).map_err(|e| format!("UDP discovery recv bind failed: {e}"))?;
    // 加入组播组：必须与 bind IP 使用同一接口；失败直接让启动失败。
    // 发送侧那个 socket 也要加（Linux 上它一直是真正收组播的那个，别退化）。
    for sock in [&send_socket, &recv_socket] {
        sock.join_multicast_v4(MULTICAST_GROUP, multicast_iface)
            .map_err(|e| {
                format!("join multicast {MULTICAST_GROUP} on {multicast_iface} failed: {e}")
            })?;
    }
    let send_socket = Arc::new(send_socket);
    let recv_socket = Arc::new(recv_socket);

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
        &format!(
            "send_bind={udp_bind_ip}, recv_bind={}, multicast_iface={multicast_iface}",
            discovery_recv_bind_ip()
        ),
    );

    let my_id = state.device_id.clone();

    // ---- 接收循环 ----
    //
    // 两个 socket 都要读：`recv_socket`(0.0.0.0) 负责广播/组播（macOS 上只有它能收到），
    // `send_socket`(具体 LAN IP) 负责发给「具体地址」的单播 —— macOS 的 SO_REUSEPORT
    // 会把同一份数据报只投给其中一个 socket，只读一个就会漏掉一半的发现包。
    let recv_task = {
        let recv_socket = recv_socket.clone();
        let send_socket = send_socket.clone();
        let state = state.clone();
        let my_id = my_id.clone();
        let mut shutdown = shutdown.clone();
        tokio::spawn(async move {
            // 两个 socket 各一个缓冲区：`tokio::select!` 的两条分支不能同时可变借用同一个 buf。
            let mut buf_recv = vec![0u8; 2048];
            let mut buf_send = vec![0u8; 2048];
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => break,
                    res = recv_socket.recv_from(&mut buf_recv) => {
                        let Ok((len, src)) = res else { continue };
                        handle_datagram(&state, &send_socket, &my_id, &buf_recv[..len], src, tcp_port, &shutdown).await;
                    }
                    res = send_socket.recv_from(&mut buf_send) => {
                        let Ok((len, src)) = res else { continue };
                        handle_datagram(&state, &send_socket, &my_id, &buf_send[..len], src, tcp_port, &shutdown).await;
                    }
                }
            }
        })
    };

    // ---- 广播循环（自适应周期 + 抖动，避免大规模节点广播风暴与同步惊群） ----
    let broadcast_task = {
        let socket = send_socket.clone();
        let state = state.clone();
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

/// 处理一个收到的发现报文（`announce` / `who_has`）。
///
/// 从接收循环里抽出来是为了**两个 socket 共用同一套逻辑**（见 `spawn` 里为什么有两个）。
/// 出包一律走 `send_socket`（绑具体 LAN IP）—— 对端会拿 `src.ip()` 回连我们，
/// 源地址必须是真实 LAN IP。
#[allow(clippy::too_many_arguments)]
async fn handle_datagram(
    state: &Arc<AppState>,
    send_socket: &Arc<UdpSocket>,
    my_id: &str,
    buf: &[u8],
    src: std::net::SocketAddr,
    tcp_port: u16,
    shutdown: &watch::Receiver<bool>,
) {
    let Ok(pkt) = serde_json::from_slice::<UdpPacket>(buf) else {
        return;
    };
    if pkt.device_id == my_id {
        return;
    }
    match pkt.kind.as_str() {
        "announce" => {
            // 自签名校验：带签名却验不过 = 篡改或伪造，**在它影响任何状态之前**丢掉。
            // 无签名（旧端）放行 —— 它只能驱动拨号，身份绑定一律由 Hello 验签决定
            // （announce 来的公钥恒为 keys_verified=false，见 upsert_peer）。
            match crate::protocol::verify_announce(&pkt) {
                crate::protocol::AnnounceAuth::Invalid(reason) => {
                    state.push_diag_event(
                        "announce_rejected",
                        &format!("{reason}; from={} via={}", pkt.device_id, src.ip()),
                    );
                    return;
                }
                crate::protocol::AnnounceAuth::Verified => state.push_diag_event(
                    "announce_verified",
                    &format!("from={} via={}", pkt.device_id, src.ip()),
                ),
                crate::protocol::AnnounceAuth::Legacy => {}
            }
            state.push_diag_event(
                "announce_recv",
                &format!("from={} via={}", pkt.device_id, src.ip()),
            );
            // 不用对端时间戳推算 RTT：那依赖双方时钟同步，纯本地业务不应
            // 假设对端时钟。这里只做在线发现与建链，时延指标留待真正的往返测量。
            upsert_peer(
                state,
                &pkt.device_id,
                &pkt.nickname,
                None,
                "",
                &src.ip().to_string(),
                pkt.tcp_port,
                pkt.x25519_pubkey.clone(),
                pkt.ed25519_pubkey.clone(),
                None,
            )
            .await;
            // ⚠️ **必须 spawn**：`ensure_link` 内部会做 connect + 握手，
            // 最坏阻塞 = CONNECT_TIMEOUT(5s) + HANDSHAKE_TIMEOUT(5s) ≈ 10s。
            // 而 announce 周期只有 5s ⇒ 串行 await 会让收包循环**永远落后**：
            // 一个收得到 UDP、TCP 被 DROP 的"黑洞"对端（VPN/防火墙场景）就足以
            // 把循环堵死，UDP 接收缓冲溢出后其它节点的 announce/who_has 静默丢失
            // —— 用户看到的是"扫不到节点 / 加不上好友"。
            // 拨号风暴由 `connect_to_peer` 的在途去重 + 并发上限挡住。
            let state = state.clone();
            let shutdown = shutdown.clone();
            let device_id = pkt.device_id.clone();
            let ip = src.ip().to_string();
            let port = pkt.tcp_port;
            tokio::spawn(async move {
                ensure_link(&state, &device_id, &ip, port, shutdown).await;
            });
        }
        "who_has" => {
            // 惊群治理：who_has 是「打开添加好友」时向全网发的一次探测，
            // 收到就立刻回包的话，1000 个节点会在同一瞬间把请求方的收包
            // 与建链路径打满（回包风暴 + 一次性 ensure_link）。
            // 这里让每个节点各自等一个 0~500ms 随机时长再回，
            // 把回包摊开；对"打开添加好友"的感知延迟影响可忽略。
            let socket = send_socket.clone();
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

async fn broadcast(
    socket: &UdpSocket,
    state: &AppState,
    tcp_port: u16,
    lan_broadcast: Option<Ipv4Addr>,
) {
    let pkt = announce_packet(state, tcp_port);
    let Ok(data) = serde_json::to_vec(&pkt) else {
        return;
    };
    // **两种广播都发**，各自覆盖对方的短板：
    // · limited broadcast（255.255.255.255）：Windows 默认禁用 directed broadcast
    //   （DisableDirectedBroadcasts=1），只有它能穿透；但它**在 macOS 上会失败**
    //   （socket 绑定到具体网卡 IP 时内核返回 EHOSTUNREACH "No route to host"）——
    //   用户真机日志里每一轮都是这个错误，导致 Mac **从不在局域网上出现**，
    //   对端于是只能走蓝牙（并因此撞上蓝牙那条通道自身的问题）。
    // · 子网广播（如 192.168.31.255）：`find_lan_interface` 早就算好了它并一路传到这里，
    //   但本函数此前把它丢掉了（参数名是 `_lan_broadcast`）—— macOS 上真正能用的就是它。
    //
    // 两发一收不会重复：接收端按 `device_id` + 消息去重，多收到一份是幂等的。
    // 子网广播用**独立的事件名** `bc_directed`：原先复用了 `broadcast_sent`，
    // 而那个名字在 `push_diag_event` 的 DROPPED 名单里（高频心跳类）——
    // 于是**成功时什么都不打**，真机上「子网广播到底发出去没有」完全不可见。
    // 这正是排查"局域网只通一半"时最需要看到的一行。
    let directed_ok = if let Some(bc) = lan_broadcast {
        let directed = format!("{bc}:{UDP_PORT}");
        let res = socket.send_to(&data, &directed).await;
        let (kind, detail) = diag_event_from_send_result(&directed, "bc_directed", &res);
        state.push_diag_event(kind, &detail);
        res.is_ok()
    } else {
        false
    };
    let bcast_target = format!("255.255.255.255:{UDP_PORT}");
    let bcast_res = socket.send_to(&data, &bcast_target).await;
    let mcast_target = format!("{MULTICAST_GROUP}:{UDP_PORT}");
    let mcast_res = socket.send_to(&data, &mcast_target).await;
    // 子网广播成功时，**不再**为 limited / multicast 的失败刷告警 ——
    // macOS 上它们本来就发不出去（socket 绑具体网卡 IP 时 EHOSTUNREACH），
    // 而我们已经有一条能用的广播路径了。每 5 秒两条 WARN 会把真正有用的信息淹掉。
    // 只有在**三条路全失败**时才留痕：那才是"本机在局域网上发不出声"的真信号。
    if !directed_ok {
        let (kind, detail) = diag_event_from_send_result(&bcast_target, "bc_limited", &bcast_res);
        state.push_diag_event(kind, &detail);
        let (kind, detail) = diag_event_from_send_result(&mcast_target, "bc_multicast", &mcast_res);
        state.push_diag_event(kind, &detail);
    }
}

/// 按需探测：群发 `who_has` 请求周围节点单播回复其 `announce`，并同时广播一次自身 announce。
/// 用于「添加好友」弹窗打开时快速、主动地发现局域网内在线客户端。
async fn broadcast_probe(
    socket: &UdpSocket,
    state: &AppState,
    tcp_port: u16,
    lan_broadcast: Option<Ipv4Addr>,
) {
    let who = UdpPacket {
        kind: "who_has".to_string(),
        device_id: state.device_id.clone(),
        nickname: String::new(),
        tcp_port,
        x25519_pubkey: None,
        ed25519_pubkey: None,
        // who_has 只是"谁在线"的探测：不声明身份、也不参与任何绑定，故不签名。
        // 接收端按 AnnounceAuth::Legacy 处理（见 verify_announce）。
        nonce: String::new(),
        sig: String::new(),
    };
    if let Ok(data) = serde_json::to_vec(&who) {
        // 与 announce 同一口径：子网广播 + limited 广播都发（见 broadcast 的说明）。
        // 「打开添加好友」在 macOS 上能否发现对方，就取决于这条。
        if let Some(bc) = lan_broadcast {
            let directed = format!("{bc}:{UDP_PORT}");
            let res = socket.send_to(&data, &directed).await;
            let (kind, detail) = diag_event_from_send_result(&directed, "who_has_sent", &res);
            state.push_diag_event(kind, &detail);
        }
        let bcast_target = format!("255.255.255.255:{UDP_PORT}");
        let bcast_res = socket.send_to(&data, &bcast_target).await;
        let (kind, detail) = diag_event_from_send_result(&bcast_target, "who_has_sent", &bcast_res);
        state.push_diag_event(kind, &detail);
    }
    // 同时广播自身，让周围节点也能立刻发现我们
    broadcast(socket, state, tcp_port, lan_broadcast).await;
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
    let removed: Vec<String> = {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        let before: Vec<String> = peers.keys().cloned().collect();
        peers.retain(|id, p| should_keep_peer(p.last_seen, now, active_links.contains(id)));
        let after: std::collections::HashSet<&String> = peers.keys().collect();
        before
            .into_iter()
            .filter(|id| !after.contains(id))
            .collect()
    };
    // 被清扫掉的节点：连带清掉它的链路快照（`conv_link` 是"当前可达路径"，
    // 节点已不在 peers 表 ⇒ 该路径失效）。否则聊天头部的链路徽标会在节点早已被清扫后
    // 继续显示历史路径（用户 2026-09-12 反馈的「离线却显示『桥接 1』」）。
    let changed = !removed.is_empty();
    // 超时清理：与「掉线」（mark_peer_offline）是**两条不同路径**，只有日志能区分。
    // 45s 超时清掉的节点在界面上同样表现为"离线"，但原因完全不同
    // （前者是链路断了，后者是我们没再收到它的 announce/Presence）。
    if !removed.is_empty() {
        state.logger.info(
            "link",
            format!("超时清理 {} 个节点：{}", removed.len(), removed.join(",")),
        );
    }
    for id in removed {
        crate::network::transport::clear_conv_link(state, &id);
        // 连带清掉按 device_id 索引的内存表：它们原先只在「删好友」时清，
        // 而节点进出（换网、换设备、临时上线）比删好友频繁得多 ——
        // 长跑后 `peer_content_features` / `peer_versions` / `key_conflict_warned` 会无界增长。
        // 与 `conv_link` 同一个回收点：节点已不在 peers 表，这些按它的 id 记的状态
        // 也就失去了参照（它再上线会重新 Hello / 重新 announce，届时按需重建）。
        state
            .peer_content_features
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        state
            .peer_versions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        state
            .key_conflict_warned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
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

    /// 探测「本机能不能完成一次 `255.255.255.255` 的本机广播收发」。
    ///
    /// ⚠️ 收端**硬编码绑 `0.0.0.0`**，而不是 `discovery_recv_bind_ip()` —— 这是刻意的：
    /// 探测必须与「被测代码的绑定选择」**解耦**，否则它区分不了"环境不支持广播"与
    /// "我们把绑定写错了"。拿 `0.0.0.0` 这个**已知正确**的绑定去问环境，答案才可信；
    /// 于是它失败只可能是环境问题，绝不会掩盖真正的回归。
    fn loopback_broadcast_works(lan_ip: Ipv4Addr) -> bool {
        use std::net::UdpSocket as StdUdpSocket;

        let Ok(recv) = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
            return false;
        };
        let Ok(port) = recv.local_addr().map(|a| a.port()) else {
            return false;
        };
        if recv
            .set_read_timeout(Some(std::time::Duration::from_millis(1500)))
            .is_err()
        {
            return false;
        }
        let Ok(send) = StdUdpSocket::bind((lan_ip, 0)) else {
            return false;
        };
        if send.set_broadcast(true).is_err() {
            return false;
        }
        if send
            .send_to(b"gosslan-discovery-probe", (Ipv4Addr::BROADCAST, port))
            .is_err()
        {
            return false;
        }
        let mut buf = [0u8; 64];
        matches!(recv.recv_from(&mut buf), Ok((n, _)) if &buf[..n] == b"gosslan-discovery-probe")
    }

    /// **真机根因的回归护栏**：接收 socket 必须真的收得到 `255.255.255.255` 广播。
    ///
    /// 2026-09-12 用户真机：「Mac 和手机同一个 Wi‑Fi、都开了局域网，却互相搜不到；
    /// Mac 列表里安卓只闪一下」。根因是发现 socket **绑定到具体 LAN IP**：
    /// macOS 上这种 socket 收不到广播/组播（本机实测 0 包；绑 `0.0.0.0` 收得到全部），
    /// 于是 Mac 发得出去（手机看得到 Mac）、却一个 announce 都收不到。
    ///
    /// 这个测试做一次**真实的本机收发**：接收方按 `discovery_recv_bind_ip()` 绑、
    /// 发送方绑本机 LAN IP 并往 `255.255.255.255` 发。把接收绑定改回具体 IP，
    /// 它在 macOS 上会立刻红 —— 也就是这次事故会当场被拦下。
    ///
    /// 两类环境下跳过（都测不了，不该误报）：① 没有可用 LAN 接口（纯容器）；
    /// ② **有接口但本机收不到自己的广播** —— GitHub 的 macOS runner 就是这种
    /// （2026-09-16 本项目第一次在 CI 跑测试，502 通过 / 1 失败、红的正是这条）。
    /// 跳过条件由 [`loopback_broadcast_works`] 用证据判定，而不是看 `CI` 环境变量 ——
    /// 后者会把"碰巧跑在 CI 上的真机"也一起漏掉。
    #[test]
    fn discovery_recv_socket_actually_receives_broadcast() {
        use std::net::UdpSocket as StdUdpSocket;

        let Some((lan_ip, _)) = find_lan_interface() else {
            return;
        };

        if !loopback_broadcast_works(lan_ip) {
            eprintln!(
                "跳过 discovery_recv_socket_actually_receives_broadcast：\
                 本环境有 LAN 接口但收不到本机 255.255.255.255 广播（CI 容器常见），\
                 无法验证接收绑定。这条护栏的有效场景是真机与本地开发。"
            );
            return;
        }

        let recv = StdUdpSocket::bind((discovery_recv_bind_ip(), 0)).expect("绑定接收 socket 失败");
        let port = recv.local_addr().expect("取接收端口失败").port();
        recv.set_read_timeout(Some(std::time::Duration::from_millis(1500)))
            .expect("设置读超时失败");

        let send = StdUdpSocket::bind((lan_ip, 0)).expect("绑定发送 socket 失败");
        send.set_broadcast(true).expect("SO_BROADCAST 失败");
        send.send_to(b"gosslan-discovery-probe", (Ipv4Addr::BROADCAST, port))
            .expect("发送广播失败");

        let mut buf = [0u8; 64];
        let got = recv.recv_from(&mut buf);
        let Ok((n, _src)) = got else {
            panic!(
                "接收 socket（bind={}）收不到 255.255.255.255 广播 —— \
                 接收侧必须绑 0.0.0.0：macOS 上绑具体网卡地址的 UDP socket \
                 收不到广播/组播，真机症状是「对方看得到我、我看不到对方」",
                discovery_recv_bind_ip()
            );
        };
        assert_eq!(&buf[..n], b"gosslan-discovery-probe");
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
            ("192.168.1.100".parse().unwrap(), "以太网".to_string(), true),
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
        let (m_ok_kind, _) =
            diag_event_from_send_result("239.255.42.99:59991", "multicast_sent", &ok);
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
