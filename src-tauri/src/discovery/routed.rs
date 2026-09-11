//! Routed 发现：跨子网 / VPN / Tailscale 等「已路由 IP」上的节点。
//!
//! 设计 §8：不要为 Tailscale 单做一个机制——它只是 Routed IP 的一种实现，
//! WireGuard / ZeroTier / 企业 VPN / 普通跨子网路由都复用这一套。
//!
//! 流程（§8）：
//! ```text
//! IP:PORT → TCP → Hello → Node ID → Identity → 产出 PeerCandidate
//! ```
//!
//! 关键约束：**身份只能来自握手，不能由端点推测**（P-A01）。
//! 因此握手完成前本机制不产出任何候选——它只维护「待探测端点」列表，
//! 由传输层（Phase 4）拨号、验签 Hello 后回调 [`RoutedDiscovery::on_hello_verified`]
//! 注入真正的候选。
//!
//! 第一版只支持手动端点：不扫描整个网络（§52）。

use std::collections::VecDeque;
use std::net::SocketAddr;

use async_trait::async_trait;

use crate::discovery::Discovery;
use crate::mesh::candidate::PeerCandidate;
use crate::mesh::endpoint::Endpoint;
use crate::mesh::path::PathKind;
use crate::mesh::peer::PeerIdentity;

/// Routed 发现机制。
pub struct RoutedDiscovery {
    /// 手动配置的待探测端点（去重、保持插入顺序）。
    endpoints: Vec<SocketAddr>,
    /// 已完成握手、待吐出的候选。
    pending: VecDeque<PeerCandidate>,
}

impl RoutedDiscovery {
    pub fn new() -> Self {
        Self {
            endpoints: Vec::new(),
            pending: VecDeque::new(),
        }
    }

    /// 登记一个待探测端点；已存在则返回 `false`（幂等）。
    pub fn add_endpoint(&mut self, addr: SocketAddr) -> bool {
        if self.endpoints.contains(&addr) {
            return false;
        }
        self.endpoints.push(addr);
        true
    }

    pub fn remove_endpoint(&mut self, addr: &SocketAddr) -> bool {
        let before = self.endpoints.len();
        self.endpoints.retain(|a| a != addr);
        self.endpoints.len() != before
    }

    /// 待探测端点列表（供传输层拿去拨号）。
    pub fn endpoints(&self) -> &[SocketAddr] {
        &self.endpoints
    }

    /// 传输层在 **Hello 验签通过后**回调：此刻才拿到真实 `device_id`，
    /// 才能产出合法候选。
    ///
    /// 端点必须是已登记的，否则拒绝——防止任意地址被注入成候选。
    pub fn on_hello_verified(
        &mut self,
        device_id: impl Into<String>,
        identity: PeerIdentity,
        addr: SocketAddr,
    ) -> bool {
        if !self.endpoints.contains(&addr) {
            return false;
        }
        self.pending.push_back(PeerCandidate::new(
            device_id,
            identity,
            Endpoint::Tcp(addr),
            PathKind::Routed,
        ));
        true
    }
}

impl Default for RoutedDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Discovery for RoutedDiscovery {
    fn name(&self) -> &'static str {
        "routed"
    }

    async fn next_candidate(&mut self) -> Option<PeerCandidate> {
        self.pending.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), 59992)
    }

    /// 身份不可由端点推测：握手完成前不产出任何候选。
    #[tokio::test]
    async fn no_candidate_before_handshake() {
        let mut d = RoutedDiscovery::new();
        assert!(d.add_endpoint(addr(100, 80, 20, 30)));
        assert_eq!(d.endpoints().len(), 1);
        assert!(d.next_candidate().await.is_none());
    }

    #[tokio::test]
    async fn hello_verified_yields_routed_candidate() {
        let mut d = RoutedDiscovery::new();
        let a = addr(100, 80, 20, 30);
        d.add_endpoint(a);

        assert!(d.on_hello_verified("ABC123", PeerIdentity::default(), a));

        let c = d.next_candidate().await.expect("握手后应产出候选");
        assert_eq!(c.device_id, "ABC123");
        assert_eq!(c.endpoint, Endpoint::Tcp(a));
        assert_eq!(c.path_kind, PathKind::Routed);
        // 队列已排空
        assert!(d.next_candidate().await.is_none());
    }

    /// 未登记的地址不得注入候选（防止任意地址伪造身份）。
    #[tokio::test]
    async fn unregistered_endpoint_is_rejected() {
        let mut d = RoutedDiscovery::new();
        d.add_endpoint(addr(100, 80, 20, 30));

        assert!(!d.on_hello_verified("EVIL", PeerIdentity::default(), addr(1, 2, 3, 4)));
        assert!(d.next_candidate().await.is_none());
    }

    #[test]
    fn add_endpoint_is_deduplicated_and_removable() {
        let mut d = RoutedDiscovery::new();
        let a = addr(100, 80, 20, 30);

        assert!(d.add_endpoint(a));
        assert!(!d.add_endpoint(a), "重复端点应被忽略");
        assert_eq!(d.endpoints().len(), 1);

        assert!(d.remove_endpoint(&a));
        assert!(!d.remove_endpoint(&a));
        assert!(d.endpoints().is_empty());
    }
}
