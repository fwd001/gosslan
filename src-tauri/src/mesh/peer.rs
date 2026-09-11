//! Peer：稳定节点身份 + 到达它的多条 Connection。
//!
//! 核心不变量（P-A01 / P-A02）：
//! - `device_id` 是身份，与网络路径无关（IP / BLE 句柄只是端点）；
//! - 一个 Peer 拥有 N 条 Connection。

use super::connection::Connection;
use super::endpoint::Endpoint;

/// Peer 的公开身份（只含公钥，绝不含私钥）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerIdentity {
    /// X25519 公钥（base64，ECDH 用）。
    pub x25519_public_key: Option<String>,
    /// Ed25519 公钥（base64，验签用）。
    pub ed25519_public_key: Option<String>,
}

impl PeerIdentity {
    /// 只补空字段、不覆盖已有值（公钥冲突不静默覆盖，对齐 INV-P11 语义）。
    pub fn merge_missing(&mut self, other: &PeerIdentity) {
        if self.x25519_public_key.is_none() {
            self.x25519_public_key = other.x25519_public_key.clone();
        }
        if self.ed25519_public_key.is_none() {
            self.ed25519_public_key = other.ed25519_public_key.clone();
        }
    }
}

/// Peer 的整体在线状态：由 Connection 集合聚合而来。
///
/// 规则（设计 §34）：**任何** Connection 健康 ⇒ Online；
/// 绝不因为「一条连接断开」就判 Offline。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeerOnlineState {
    Online,
    Offline,
}

/// 一个稳定节点。
#[derive(Clone, Debug)]
pub struct Peer {
    pub device_id: String,
    pub identity: PeerIdentity,
    /// 到达本 Peer 的连接集合（端点互异）。
    connections: Vec<Connection>,
}

impl Peer {
    pub fn new(device_id: impl Into<String>, identity: PeerIdentity) -> Self {
        Self {
            device_id: device_id.into(),
            identity,
            connections: Vec::new(),
        }
    }

    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// 合并一条 Connection：
    /// - 同 `endpoint` 已存在 → 更新 health 与 path_kind（幂等，返回 `false`）；
    /// - 新 `endpoint` → 追加（返回 `true`）。
    ///
    /// 这是「LAN + Tailscale + BLE 三条端点汇成同一 Peer」的关键入口：
    /// 调用方保证 `conn.peer_id == self.device_id`（debug 构建下断言兜底）。
    pub fn upsert_connection(&mut self, conn: Connection) -> bool {
        debug_assert_eq!(
            conn.peer_id, self.device_id,
            "connection 必须属于本 peer"
        );
        if let Some(existing) = self
            .connections
            .iter_mut()
            .find(|c| c.endpoint == conn.endpoint)
        {
            existing.health = conn.health;
            existing.path_kind = conn.path_kind;
            false
        } else {
            self.connections.push(conn);
            true
        }
    }

    /// 按端点移除一条 Connection；返回是否真的移除。
    pub fn remove_connection(&mut self, endpoint: &Endpoint) -> bool {
        let before = self.connections.len();
        self.connections.retain(|c| c.endpoint != *endpoint);
        self.connections.len() != before
    }

    /// 按连接 id 移除一条 Connection；返回是否真的移除。
    pub fn remove_connection_by_id(&mut self, id: &str) -> bool {
        let before = self.connections.len();
        self.connections.retain(|c| c.id != id);
        self.connections.len() != before
    }

    /// 聚合在线状态：任何 Connection 健康 ⇒ Online。
    ///
    /// `health_timeout_ms` / `max_failures` 是健康判定阈值（由 PeerManager 提供）。
    pub fn online_state(
        &self,
        now_ms: i64,
        health_timeout_ms: i64,
        max_failures: u32,
    ) -> PeerOnlineState {
        let online = self
            .connections
            .iter()
            .any(|c| c.health.is_healthy(now_ms, health_timeout_ms, max_failures));
        if online {
            PeerOnlineState::Online
        } else {
            PeerOnlineState::Offline
        }
    }

    /// 标记某条 Connection 成功收发（返回是否命中）。
    pub fn mark_connection_seen(
        &mut self,
        endpoint: &Endpoint,
        now_ms: i64,
        rtt_ms: Option<u64>,
    ) -> bool {
        match self.connections.iter_mut().find(|c| c.endpoint == *endpoint) {
            Some(c) => {
                c.health.mark_seen(now_ms, rtt_ms);
                true
            }
            None => false,
        }
    }

    /// 标记某条 Connection 失败（返回是否命中）。
    pub fn mark_connection_failure(&mut self, endpoint: &Endpoint) -> bool {
        match self.connections.iter_mut().find(|c| c.endpoint == *endpoint) {
            Some(c) => {
                c.health.mark_failure();
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::connection::Connection;
    use crate::mesh::endpoint::{BleEndpoint, Endpoint};
    use crate::mesh::path::PathKind;
    use std::net::SocketAddr;

    fn lan_endpoint() -> Endpoint {
        Endpoint::Tcp(SocketAddr::from(([192, 168, 1, 20], 59992)))
    }

    fn routed_endpoint() -> Endpoint {
        Endpoint::Tcp(SocketAddr::from(([100, 80, 20, 30], 59992)))
    }

    fn ble_endpoint() -> Endpoint {
        Endpoint::Ble(BleEndpoint::new("bit-chat-node-xyz"))
    }

    /// 验收：同一 device_id 的 LAN + Tailscale + BLE 三条端点 → 1 Peer + 3 Connections。
    #[test]
    fn one_device_three_connections() {
        let mut peer = Peer::new("ABC123", PeerIdentity::default());

        assert!(peer.upsert_connection(Connection::new(
            "ABC123",
            lan_endpoint(),
            PathKind::Lan
        )));
        assert!(peer.upsert_connection(Connection::new(
            "ABC123",
            routed_endpoint(),
            PathKind::Routed
        )));
        assert!(peer.upsert_connection(Connection::new(
            "ABC123",
            ble_endpoint(),
            PathKind::Bluetooth
        )));

        assert_eq!(peer.device_id, "ABC123");
        assert_eq!(peer.connection_count(), 3);
        // 三种路径各一条
        assert!(peer
            .connections()
            .iter()
            .any(|c| c.path_kind == PathKind::Lan));
        assert!(peer
            .connections()
            .iter()
            .any(|c| c.path_kind == PathKind::Routed));
        assert!(peer
            .connections()
            .iter()
            .any(|c| c.path_kind == PathKind::Bluetooth));
    }

    #[test]
    fn same_endpoint_upsert_is_idempotent() {
        let mut peer = Peer::new("ABC123", PeerIdentity::default());
        assert!(peer.upsert_connection(Connection::new(
            "ABC123",
            lan_endpoint(),
            PathKind::Lan
        )));
        // 同 endpoint 再次 upsert：不新增，只更新
        assert!(!peer.upsert_connection(Connection::new(
            "ABC123",
            lan_endpoint(),
            PathKind::Lan
        )));
        assert_eq!(peer.connection_count(), 1);
    }

    #[test]
    fn remove_connection_by_endpoint() {
        let mut peer = Peer::new("ABC123", PeerIdentity::default());
        peer.upsert_connection(Connection::new("ABC123", lan_endpoint(), PathKind::Lan));
        peer.upsert_connection(Connection::new("ABC123", ble_endpoint(), PathKind::Bluetooth));
        assert_eq!(peer.connection_count(), 2);

        assert!(peer.remove_connection(&lan_endpoint()));
        assert_eq!(peer.connection_count(), 1);
        // 已移除，再删返回 false
        assert!(!peer.remove_connection(&lan_endpoint()));
    }

    #[test]
    fn remove_connection_by_id() {
        let mut peer = Peer::new("ABC123", PeerIdentity::default());
        peer.upsert_connection(Connection::new("ABC123", lan_endpoint(), PathKind::Lan));
        let id = peer.connections()[0].id.clone();

        assert!(peer.remove_connection_by_id(&id));
        assert_eq!(peer.connection_count(), 0);
        assert!(!peer.remove_connection_by_id(&id));
    }

    /// 关键不变量（设计 §34）：一条连接断开 ≠ Peer 离线。
    #[test]
    fn online_state_is_any_connection_healthy() {
        let mut peer = Peer::new("ABC123", PeerIdentity::default());
        peer.upsert_connection(Connection::new("ABC123", lan_endpoint(), PathKind::Lan));
        peer.upsert_connection(Connection::new("ABC123", routed_endpoint(), PathKind::Routed));

        // 初始：无健康记录 → Offline
        assert_eq!(peer.online_state(0, 10_000, 3), PeerOnlineState::Offline);

        // LAN 健康 → Online
        assert!(peer.mark_connection_seen(&lan_endpoint(), 1000, Some(5)));
        assert_eq!(peer.online_state(1000, 10_000, 3), PeerOnlineState::Online);

        // LAN 超时但 Routed 健康 → 仍 Online（一条断开不回退）
        assert!(peer.mark_connection_seen(&routed_endpoint(), 2000, Some(30)));
        assert_eq!(peer.online_state(12_000, 10_000, 3), PeerOnlineState::Online);

        // 两条都超时 → Offline
        assert_eq!(peer.online_state(13_000, 10_000, 3), PeerOnlineState::Offline);
    }

    #[test]
    fn empty_connections_are_offline() {
        let peer = Peer::new("ABC123", PeerIdentity::default());
        assert_eq!(peer.online_state(0, 10_000, 3), PeerOnlineState::Offline);
    }

    /// 连续失败超过阈值 → 该条 connection 不健康，但另一条仍可撑住 Online。
    #[test]
    fn consecutive_failures_break_only_that_connection() {
        let mut peer = Peer::new("ABC123", PeerIdentity::default());
        peer.upsert_connection(Connection::new("ABC123", lan_endpoint(), PathKind::Lan));
        peer.upsert_connection(Connection::new("ABC123", routed_endpoint(), PathKind::Routed));

        peer.mark_connection_seen(&lan_endpoint(), 1000, Some(5));
        peer.mark_connection_seen(&routed_endpoint(), 1000, Some(5));
        assert_eq!(peer.online_state(1000, 10_000, 3), PeerOnlineState::Online);

        // LAN 连续失败 4 次（> max_failures=3）
        for _ in 0..4 {
            peer.mark_connection_failure(&lan_endpoint());
        }
        // LAN 已不健康，但 Routed 仍健康 → Online
        assert_eq!(peer.online_state(1000, 10_000, 3), PeerOnlineState::Online);
    }
}
