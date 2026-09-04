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
use tokio::time::{interval, Duration};

use rand_core::{OsRng, RngCore};

use crate::network::transport::{ensure_link, upsert_peer};
use crate::protocol::{UdpPacket, ANNOUNCE_INTERVAL_SECS, PEER_TIMEOUT_SECS, UDP_PORT};
use crate::state::AppState;

/// 组播地址（与广播并行，覆盖被隔离广播域的场景）
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 绑定一个允许地址复用的 UDP 套接字（SO_REUSEADDR + SO_BROADCAST + unix 下 SO_REUSEPORT），
/// 使同一台机器上的多个 gosslan 实例能同时监听同一发现端口（Windows/macOS/Linux 通用）。
fn bind_udp_reusable(ip: Ipv4Addr, port: u16) -> Result<UdpSocket, String> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::SocketAddr;

    let addr: SocketAddr = format!("{ip}:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| e.to_string())?;
    sock.set_reuse_address(true).map_err(|e| e.to_string())?;
    // macOS/BSD：UDP 同端口多开必须 SO_REUSEPORT（SO_REUSEADDR 仅 Windows 允许重复绑定）。
    // 缺了它，同一台机器的第二个实例 network::start 会报 "Address already in use"，
    // 单机多实例互发现直接失效。
    #[cfg(unix)]
    sock.set_reuse_port(true).map_err(|e| e.to_string())?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;
    // tokio 要求注册进 runtime 的 fd 必须非阻塞：socket2 创建的是阻塞 socket，
    // 直接 from_std 在 debug 构建会 panic（tokio blocking check），release 构建虽不 panic
    // 但阻塞 fd 挂在 kqueue/epoll 上会卡死 worker 线程（界面卡顿的帮凶之一）。
    sock.set_nonblocking(true).map_err(|e| e.to_string())?;
    let sock_addr: socket2::SockAddr = addr.into();
    sock.bind(&sock_addr).map_err(|e| e.to_string())?;

    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).map_err(|e| e.to_string())
}

fn announce_packet(state: &AppState, tcp_port: u16) -> UdpPacket {
    UdpPacket {
        kind: "announce".to_string(),
        device_id: state.device_id.clone(),
        nickname: state.nickname.lock().unwrap().clone(),
        avatar: state.avatar.lock().unwrap().clone(),
        tcp_port,
        x25519_pubkey: Some(state.identity.x25519_public_b64()),
        ed25519_pubkey: Some(state.identity.ed25519_public_b64()),
        ts: now_ms(),
    }
}

/// 启动发现任务。
pub async fn spawn(
    state: Arc<AppState>,
    ip: Ipv4Addr,
    tcp_port: u16,
    shutdown: watch::Receiver<bool>,
    mut probe: watch::Receiver<u64>,
) -> Result<(), String> {
    // 绑定 UDP 端口（SO_REUSEADDR 允许同一台机器上多个 gosslan 实例共存，用于多开测试）
    let socket = bind_udp_reusable(ip, UDP_PORT)
        .map_err(|e| format!("UDP 绑定 {ip}:{UDP_PORT} 失败: {e}"))?;
    // 加入组播组（0.0.0.0 绑定则用 UNSPECIFIED 接口）
    let iface = if ip.is_unspecified() { Ipv4Addr::UNSPECIFIED } else { ip };
    socket.join_multicast_v4(MULTICAST_GROUP, iface).ok();
    let socket = Arc::new(socket);

    let my_id = state.device_id.clone();

    // ---- 接收循环 ----
    {
        let socket = socket.clone();
        let state = state.clone();
        let my_id = my_id.clone();
        let mut shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                tokio::select! {
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
                                // 粗略 RTT：基于双方 NTP 同步时钟的时间差（局域网内近似）
                                let delta = now_ms().saturating_sub(pkt.ts);
                                let rtt = if delta > 0 && delta < 5000 { Some(delta as u64) } else { None };
                                upsert_peer(
                                    &state,
                                    &pkt.device_id,
                                    &pkt.nickname,
                                    pkt.avatar.clone(),
                                    &src.ip().to_string(),
                                    pkt.tcp_port,
                                    pkt.x25519_pubkey.clone(),
                                    pkt.ed25519_pubkey.clone(),
                                    rtt,
                                ).await;
                                ensure_link(&state, &pkt.device_id, &src.ip().to_string(), pkt.tcp_port).await;
                            }
                            "who_has" => {
                                let reply = announce_packet(&state, tcp_port);
                                if let Ok(data) = serde_json::to_vec(&reply) {
                                    let _ = socket.send_to(&data, src).await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });
    }

    // ---- 广播循环（自适应周期 + 抖动，避免大规模节点广播风暴与同步惊群） ----
    {
        let socket = socket.clone();
        let state = state.clone();
        let mut shutdown = shutdown.clone();
        tokio::spawn(async move {
            // 首次立刻广播
            broadcast(&socket, &state, tcp_port).await;
            let mut tick = interval(Duration::from_secs(ANNOUNCE_INTERVAL_SECS));
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = tick.tick() => {
                        broadcast(&socket, &state, tcp_port).await;
                        sweep_peers(&state);
                        // 节点越多，广播越稀疏：缓解 N=500-1000 时的 UDP 风暴
                        let secs = adaptive_interval(&state);
                        tick = interval(Duration::from_secs(secs));
                    }
                    // 按需探测：用户打开「添加好友」时触发一次 who_has 群发
                    _ = probe.changed() => {
                        broadcast_probe(&socket, &state, tcp_port).await;
                    }
                }
            }
        });
    }

    Ok(())
}

/// 依据当前在线节点数自适应调整广播周期（秒），并叠加随机抖动以打散各节点广播相位。
fn adaptive_interval(state: &AppState) -> u64 {
    let n = state.peers.lock().unwrap().len();
    let base = if n >= 500 {
        20
    } else if n >= 100 {
        10
    } else {
        ANNOUNCE_INTERVAL_SECS
    };
    // 0..=2s 抖动，避免全网节点在同一瞬间齐发
    let mut rng = OsRng;
    let jitter = rng.next_u64() % 3000;
    base + jitter / 1000
}

async fn broadcast(socket: &UdpSocket, state: &AppState, tcp_port: u16) {
    let pkt = announce_packet(state, tcp_port);
    let Ok(data) = serde_json::to_vec(&pkt) else {
        return;
    };
    // 广播 + 组播双通道
    let _ = socket.send_to(&data, format!("255.255.255.255:{UDP_PORT}")).await;
    let _ = socket.send_to(&data, format!("{MULTICAST_GROUP}:{UDP_PORT}")).await;
}

/// 按需探测：群发 `who_has` 请求周围节点单播回复其 `announce`，并同时广播一次自身 announce。
/// 用于「添加好友」弹窗打开时快速、主动地发现局域网内在线客户端。
async fn broadcast_probe(socket: &UdpSocket, state: &AppState, tcp_port: u16) {
    let who = UdpPacket {
        kind: "who_has".to_string(),
        device_id: state.device_id.clone(),
        nickname: String::new(),
        avatar: None,
        tcp_port,
        x25519_pubkey: None,
        ed25519_pubkey: None,
        ts: now_ms(),
    };
    if let Ok(data) = serde_json::to_vec(&who) {
        let _ = socket.send_to(&data, format!("255.255.255.255:{UDP_PORT}")).await;
        let _ = socket.send_to(&data, format!("{MULTICAST_GROUP}:{UDP_PORT}")).await;
    }
    // 同时广播自身，让周围节点也能立刻发现我们
    broadcast(socket, state, tcp_port).await;
}

/// 清理超过 `PEER_TIMEOUT_SECS` 未活跃的节点。
fn sweep_peers(state: &AppState) {
    let cutoff = now_ms() - PEER_TIMEOUT_SECS * 1000;
    let changed = {
        let mut peers = state.peers.lock().unwrap();
        let before = peers.len();
        peers.retain(|_, p| p.last_seen >= cutoff);
        before != peers.len()
    };
    if changed {
        state.emit_peers();
    }
}
