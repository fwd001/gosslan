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
use serde::{Deserialize, Serialize};

use crate::discovery::Discovery;
use crate::mesh::candidate::PeerCandidate;
use crate::mesh::endpoint::Endpoint;
use crate::mesh::path::PathKind;
use crate::mesh::peer::PeerIdentity;

/// 手动配置的 Routed 端点在 `settings` 表中的键。
pub const ROUTED_ENDPOINTS_KEY: &str = "routed_endpoints";

/// 一个手动配置的 Routed 端点。
///
/// **必须携带 `device_id`**：跨子网拨号时，主动方在收到 Hello 之前无从得知对端身份，
/// 而 `handle_message` 的 Hello 分支要求 `device_id == peer_id`（身份绑定校验，
/// 见 INV-P21），用占位值会让连接被直接丢弃。因此 Routed 的语义是
/// 「连接**已知**节点的跨子网 / VPN 路径」，而不是「扫描未知节点」。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutedEndpoint {
    pub device_id: String,
    /// `"ip:port"`，如 `100.64.0.1:59992`
    pub address: String,
}

impl RoutedEndpoint {
    pub fn new(device_id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            address: address.into(),
        }
    }

    /// 解析出可拨号的地址；格式非法返回 `None`（调用方应跳过而非报错）。
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.address.parse().ok()
    }
}

/// 解析存储的 JSON 数组。**逐条跳过非法条目**，绝不因一条坏数据导致整体失败。
pub fn parse_endpoints(json: &str) -> Vec<RoutedEndpoint> {
    serde_json::from_str::<Vec<RoutedEndpoint>>(json).unwrap_or_default()
}

/// 序列化待存储。
pub fn encode_endpoints(list: &[RoutedEndpoint]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".to_string())
}

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

    /// 配置序列化往返
    #[test]
    fn endpoints_serialize_roundtrip() {
        let list = vec![
            RoutedEndpoint::new("dev-a", "100.64.0.1:59992"),
            RoutedEndpoint::new("dev-b", "10.0.0.5:60002"),
        ];
        let json = encode_endpoints(&list);
        assert_eq!(parse_endpoints(&json), list);
    }

    /// 坏数据 / 空输入不 panic，且解析结果为空
    #[test]
    fn malformed_endpoints_json_yields_empty() {
        assert!(parse_endpoints("").is_empty());
        assert!(parse_endpoints("not json").is_empty());
        assert!(parse_endpoints("{}").is_empty());
    }

    #[test]
    fn socket_addr_parses_or_none() {
        assert!(RoutedEndpoint::new("a", "100.64.0.1:59992")
            .socket_addr()
            .is_some());
        assert!(RoutedEndpoint::new("a", "garbage").socket_addr().is_none());
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
