//! PeerManager：统一 Peer 生命周期（Phase 2）。
//!
//! 负责把 Discovery 产出的 `PeerCandidate` 合并成 `Peer`：
//! - **同一 device_id = 同一 Peer**；
//! - 同一 endpoint = 幂等更新，不同 endpoint = 新增 Connection。
//!
//! 同时提供 `online_state()`：**任何 Connection 健康 ⇒ Online**，
//! 绝不因为「一条连接断开」就把整个 Peer 判为离线（设计 §34）。
//!
//! Phase 2 仍是旁路：不接管 `state::peers` / `state::links`。

use std::collections::HashMap;

use super::candidate::PeerCandidate;
use super::endpoint::Endpoint;
use super::peer::{Peer, PeerOnlineState};

/// `merge` 的结果，供调用方（Discovery）区分「新节点」与「已有节点的连接更新」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeOutcome {
    /// 本次 merge 是否新建了 Peer。
    pub is_new_peer: bool,
    /// 本次 merge 是否新增了一条 Connection（false = 端点已存在，仅更新 health）。
    pub is_new_connection: bool,
}

/// Peer 生命周期管理器。
pub struct PeerManager {
    peers: HashMap<String, Peer>,
    health_timeout_ms: i64,
    max_failures: u32,
}

impl PeerManager {
    pub fn new(health_timeout_ms: i64, max_failures: u32) -> Self {
        Self {
            peers: HashMap::new(),
            health_timeout_ms,
            max_failures,
        }
    }

    /// 合并一个 PeerCandidate，返回 `(peer_id, outcome)`。
    ///
    /// - `is_new_peer`：该 device_id 首次出现；
    /// - `is_new_connection`：该 endpoint 首次出现（同端点重复 merge 只更新 health）。
    ///
    /// identity 只补空、不覆盖（对齐 INV-P11：公钥冲突不静默覆盖）。
    pub fn merge(&mut self, candidate: PeerCandidate) -> (String, MergeOutcome) {
        let device_id = candidate.device_id.clone();
        let is_new_peer = !self.peers.contains_key(&device_id);

        let peer = self
            .peers
            .entry(device_id.clone())
            .or_insert_with(|| Peer::new(device_id.clone(), candidate.identity.clone()));
        if !is_new_peer {
            peer.identity.merge_missing(&candidate.identity);
        }

        let is_new_connection = peer.upsert_connection(candidate.into_connection());

        (
            device_id,
            MergeOutcome {
                is_new_peer,
                is_new_connection,
            },
        )
    }

    pub fn get(&self, device_id: &str) -> Option<&Peer> {
        self.peers.get(device_id)
    }

    pub fn get_mut(&mut self, device_id: &str) -> Option<&mut Peer> {
        self.peers.get_mut(device_id)
    }

    pub fn peers(&self) -> impl Iterator<Item = &Peer> {
        self.peers.values()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// 任何 Connection 健康 ⇒ Online；否则 Offline（不存在 = Offline）。
    pub fn online_state(&self, device_id: &str, now_ms: i64) -> PeerOnlineState {
        self.peers
            .get(device_id)
            .map(|p| p.online_state(now_ms, self.health_timeout_ms, self.max_failures))
            .unwrap_or(PeerOnlineState::Offline)
    }

    /// 健康判定阈值：最近多久内有过成功才算「活」。
    ///
    /// 暴露出来是为了让选路（`mesh::selection::pick_link`）复用**同一个**阈值 ——
    /// 阈值散落两处是「同一判断两处实现、行为还不一致」的老坑（本项目已踩过一次）。
    pub fn health_timeout_ms(&self) -> i64 {
        self.health_timeout_ms
    }

    /// 健康判定阈值：连续失败超过该值即视为不健康。
    pub fn max_failures(&self) -> u32 {
        self.max_failures
    }

    /// 标记某 peer 的某条 connection 成功收发（返回是否命中）。
    pub fn mark_connection_seen(
        &mut self,
        device_id: &str,
        endpoint: &Endpoint,
        now_ms: i64,
        rtt_ms: Option<u64>,
    ) -> bool {
        self.peers
            .get_mut(device_id)
            .is_some_and(|p| p.mark_connection_seen(endpoint, now_ms, rtt_ms))
    }

    /// 标记某 peer 的某条 connection 失败（返回是否命中）。
    pub fn mark_connection_failure(&mut self, device_id: &str, endpoint: &Endpoint) -> bool {
        self.peers
            .get_mut(device_id)
            .is_some_and(|p| p.mark_connection_failure(endpoint))
    }

    /// 移除某 peer 的某条 connection（返回是否真的移除）。
    pub fn remove_connection(&mut self, device_id: &str, endpoint: &Endpoint) -> bool {
        self.peers
            .get_mut(device_id)
            .is_some_and(|p| p.remove_connection(endpoint))
    }

    /// 移除整个 peer（返回被移除的 peer）。
    pub fn remove_peer(&mut self, device_id: &str) -> Option<Peer> {
        self.peers.remove(device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::candidate::PeerCandidate;
    use crate::mesh::endpoint::{BleEndpoint, Endpoint};
    use crate::mesh::path::PathKind;
    use crate::mesh::peer::{PeerIdentity, PeerOnlineState};
    use std::net::SocketAddr;

    fn lan() -> Endpoint {
        Endpoint::Tcp(SocketAddr::from(([192, 168, 1, 20], 59992)))
    }

    fn routed() -> Endpoint {
        Endpoint::Tcp(SocketAddr::from(([100, 80, 20, 30], 59992)))
    }

    fn ble() -> Endpoint {
        Endpoint::Ble(BleEndpoint::new("node-xyz"))
    }

    fn cand(id: &str, endpoint: Endpoint, kind: PathKind) -> PeerCandidate {
        PeerCandidate::new(id, PeerIdentity::default(), endpoint, kind)
    }

    /// 验收：同一 device_id 的 LAN + Tailscale + BLE → 1 Peer + 3 Connections。
    #[test]
    fn same_device_merges_into_one_peer() {
        let mut m = PeerManager::new(10_000, 3);
        let (id1, o1) = m.merge(cand("ABC123", lan(), PathKind::Lan));
        let (id2, o2) = m.merge(cand("ABC123", routed(), PathKind::Routed));
        let (id3, o3) = m.merge(cand("ABC123", ble(), PathKind::Bluetooth));

        assert_eq!(id1, "ABC123");
        assert_eq!(id2, "ABC123");
        assert_eq!(id3, "ABC123");
        assert!(o1.is_new_peer && o1.is_new_connection);
        assert!(!o2.is_new_peer && o2.is_new_connection);
        assert!(!o3.is_new_peer && o3.is_new_connection);

        assert_eq!(m.peer_count(), 1);
        assert_eq!(m.get("ABC123").unwrap().connection_count(), 3);
    }

    #[test]
    fn different_devices_are_different_peers() {
        let mut m = PeerManager::new(10_000, 3);
        m.merge(cand("A", lan(), PathKind::Lan));
        m.merge(cand("B", lan(), PathKind::Lan));
        assert_eq!(m.peer_count(), 2);
    }

    /// 同 endpoint 重复 merge：不新增 peer、不新增 connection。
    #[test]
    fn same_endpoint_merge_is_idempotent() {
        let mut m = PeerManager::new(10_000, 3);
        let (_, o1) = m.merge(cand("ABC123", lan(), PathKind::Lan));
        let (_, o2) = m.merge(cand("ABC123", lan(), PathKind::Lan));
        assert!(o1.is_new_connection);
        assert!(!o2.is_new_connection);
        assert!(!o2.is_new_peer);
        assert_eq!(m.get("ABC123").unwrap().connection_count(), 1);
    }

    /// 关键不变量：一条连接断开 ≠ Peer 离线。
    #[test]
    fn online_state_any_connection_healthy() {
        let mut m = PeerManager::new(10_000, 3);
        m.merge(cand("ABC123", lan(), PathKind::Lan));
        m.merge(cand("ABC123", routed(), PathKind::Routed));

        assert_eq!(m.online_state("ABC123", 0), PeerOnlineState::Offline);

        // LAN 健康 → Online
        assert!(m.mark_connection_seen("ABC123", &lan(), 1000, Some(5)));
        assert_eq!(m.online_state("ABC123", 1000), PeerOnlineState::Online);

        // LAN 超时，Routed 健康 → 仍 Online
        assert!(m.mark_connection_seen("ABC123", &routed(), 2000, Some(30)));
        assert_eq!(m.online_state("ABC123", 12_000), PeerOnlineState::Online);

        // 全部超时 → Offline
        assert_eq!(m.online_state("ABC123", 13_000), PeerOnlineState::Offline);
    }

    #[test]
    fn online_state_unknown_peer_is_offline() {
        let m = PeerManager::new(10_000, 3);
        assert_eq!(m.online_state("nobody", 0), PeerOnlineState::Offline);
    }

    #[test]
    fn remove_connection_and_peer() {
        let mut m = PeerManager::new(10_000, 3);
        m.merge(cand("ABC123", lan(), PathKind::Lan));
        m.merge(cand("ABC123", routed(), PathKind::Routed));

        assert!(m.remove_connection("ABC123", &lan()));
        assert_eq!(m.get("ABC123").unwrap().connection_count(), 1);

        let removed = m.remove_peer("ABC123").unwrap();
        assert_eq!(removed.device_id, "ABC123");
        assert_eq!(m.peer_count(), 0);
    }

    /// 回归：周期性 announce（同 endpoint 重复 merge）不得重置 health。
    /// 否则跨过 Peer 层直接调用 merge 的调用方也会踩到「在线恒 Offline」。
    #[test]
    fn repeated_merge_preserves_connection_health() {
        let mut m = PeerManager::new(10_000, 3);
        m.merge(cand("ABC123", lan(), PathKind::Lan));
        assert!(m.mark_connection_seen("ABC123", &lan(), 1000, Some(5)));
        assert_eq!(m.online_state("ABC123", 1000), PeerOnlineState::Online);

        // 下一轮 announce：同 endpoint 再 merge 一次
        m.merge(cand("ABC123", lan(), PathKind::Lan));

        assert_eq!(m.online_state("ABC123", 1000), PeerOnlineState::Online);
    }

    /// identity 只补空、不覆盖：公钥冲突不静默覆盖（INV-P11）。
    #[test]
    fn identity_merge_never_overwrites_existing_keys() {
        let mut m = PeerManager::new(10_000, 3);

        let mut id1 = PeerIdentity::default();
        id1.x25519_public_key = Some("x1".into());
        m.merge(PeerCandidate::new("ABC123", id1, lan(), PathKind::Lan));

        // 第二次 merge 带一个「不同」的 x25519 公钥 → 不得覆盖
        let mut id2 = PeerIdentity::default();
        id2.x25519_public_key = Some("x1-different".into());
        id2.ed25519_public_key = Some("e2".into());
        m.merge(PeerCandidate::new("ABC123", id2, routed(), PathKind::Routed));

        let peer = m.get("ABC123").unwrap();
        assert_eq!(peer.identity.x25519_public_key.as_deref(), Some("x1"));
        // 空字段被补齐
        assert_eq!(peer.identity.ed25519_public_key.as_deref(), Some("e2"));
    }
}
