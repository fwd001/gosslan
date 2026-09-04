//! 局域网传输通道实现：UDP 发现 + TCP 传输（复用 `network` 模块）。
//!
//! 作为 [`Transport`] 的具体实现，把「已序列化 + 已加密」的协议帧交给底层 `network` 层
//! 做实际收发，使上层逻辑与物理通道解耦。

use std::sync::Arc;

use async_trait::async_trait;

use crate::network;
use crate::protocol::Message;
use crate::state::AppState;

use super::Transport;

/// 局域网通道（UDP 广播/组播发现 + TCP 分帧传输，未来可替换为 QUIC）。
pub struct LanTransport {
    state: Arc<AppState>,
    bind_ip: String,
}

impl LanTransport {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            bind_ip: "0.0.0.0".to_string(),
        }
    }
}

#[async_trait]
impl Transport for LanTransport {
    fn name(&self) -> &'static str {
        "局域网"
    }

    fn available(&self) -> bool {
        true
    }

    fn running(&self) -> bool {
        self.state.network.lock().unwrap().is_some()
    }

    fn peer_count(&self) -> usize {
        self.state.peers.lock().unwrap().len()
    }

    async fn start(&mut self) -> Result<(), String> {
        network::start(self.state.clone(), self.bind_ip.clone()).await
    }

    async fn stop(&mut self) -> Result<(), String> {
        network::stop(&self.state).await;
        Ok(())
    }

    async fn send(&self, peer_id: &str, payload: &[u8]) -> Result<(), String> {
        let msg: Message = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
        network::transport::try_send(&self.state, peer_id, &msg).await
    }

    async fn broadcast(&self, payload: &[u8]) -> Result<(), String> {
        let msg: Message = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
        let links = self.state.links.lock().await;
        for tx in links.values() {
            let _ = tx.send(msg.clone()).await;
        }
        Ok(())
    }
}
