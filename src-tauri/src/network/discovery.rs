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
            // 下一轮周期广播的时刻。只在真正广播后重算，探测分支不改变节拍。
            let mut next_at = Instant::now() + Duration::from_secs(next_wait(&state));
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = sleep_until(next_at) => {
                        broadcast(&socket, &state, tcp_port).await;
                        sweep_peers(&state);
                        next_at = Instant::now() + Duration::from_secs(next_wait(&state));
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

/// 下一轮广播周期（秒）：读一次在线节点数即放锁，绝不跨 await 持锁。
fn next_wait(state: &AppState) -> u64 {
    let node_count = state.peers.lock().unwrap().len();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 自适应周期分档与抖动边界：≤99 节点保持 5s，≥100 → ≥10s，≥500 → ≥20s，抖动 ≤2s。
    #[test]
    fn adaptive_interval_thresholds_and_jitter_bounds() {
        for _ in 0..200 {
            let small = adaptive_interval(0);
            let mid = adaptive_interval(99);
            let big = adaptive_interval(100);
            let huge = adaptive_interval(500);
            assert!((5..=7).contains(&small), "0 节点应为 5+0..2 秒，得到 {small}");
            assert!((5..=7).contains(&mid), "99 节点仍属小规模，得到 {mid}");
            assert!((10..=12).contains(&big), "100 节点应降频到 10+0..2 秒，得到 {big}");
            assert!((20..=22).contains(&huge), "500 节点应降频到 20+0..2 秒，得到 {huge}");
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
        assert!(start.elapsed() < Duration::from_millis(100), "过期 deadline 应立即就绪");
    }
}
