//! Peer：稳定节点身份 + 到达它的多条 Connection。
//!
//! 核心不变量（P-A01 / P-A02）：
//! - `device_id` 是身份，与网络路径无关（IP / BLE 句柄只是端点）；
//! - 一个 Peer 拥有 N 条 Connection。

use super::connection::Connection;
use super::endpoint::Endpoint;

/// Peer 的公开身份（只含公钥，绝不含私钥）。
#[derive(Clone, Debug, Default)]
pub struct PeerIdentity {
    /// X25519 公钥（base64，ECDH 用）。
    pub x25519_public_key: Option<String>,
    /// Ed25519 公钥（base64，验签用）。
    pub ed25519_public_key: Option<String>,
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
}
