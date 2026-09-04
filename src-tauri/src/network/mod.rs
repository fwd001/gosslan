//! 网络层：UDP 广播发现 + TCP 消息/文件传输。
//!
//! 传输抽象说明：
//! - 当前实现基于 TCP（简单可靠），帧格式见 `protocol.rs`。
//! - 未来可无缝切换/新增 QUIC（如 `quinn`）或 WebSocket 中继：只要实现
//!   “分帧写入/读取 + 建立连接”两个原语，`try_send` 与消息分发逻辑无需改动，
//!   即可支撑“服务端中转连接电脑与移动端”的场景。

pub mod discovery;
pub mod file;
pub mod transport;

use std::sync::Arc;

use tokio::sync::watch;

use crate::state::{AppState, NetworkHandle};

/// 启动网络（UDP 发现 + TCP 服务）。
/// `bind_ip` 为选定的网卡 IPv4 地址，或 "0.0.0.0" 表示自动（监听所有网卡）。
pub async fn start(state: Arc<AppState>, bind_ip: String) -> Result<(), String> {
    // 先停掉旧实例
    stop(&state).await;

    let ip: std::net::Ipv4Addr = bind_ip
        .parse()
        .map_err(|_| format!("无效的网卡地址: {bind_ip}"))?;
    let tcp_port = state.tcp_port;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (probe_tx, probe_rx) = watch::channel(0u64);

    discovery::spawn(state.clone(), ip, tcp_port, shutdown_rx.clone(), probe_rx).await?;
    transport::spawn(state.clone(), ip, tcp_port, shutdown_rx).await?;

    *state.probe.lock().unwrap() = Some(probe_tx);
    *state.network.lock().unwrap() = Some(NetworkHandle {
        shutdown: shutdown_tx,
        bound_ip: bind_ip,
        tcp_port,
    });
    Ok(())
}

/// 停止网络：发送关闭信号并清理连接与在线表。
pub async fn stop(state: &AppState) {
    if let Some(handle) = state.network.lock().unwrap().take() {
        let _ = handle.shutdown.send(true);
    }
    *state.probe.lock().unwrap() = None;
    state.links.lock().await.clear();
    state.peers.lock().unwrap().clear();
    state.emit_peers();
}
