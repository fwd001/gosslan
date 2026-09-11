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

    /// 标记某 peer 的某条 connection 成功收/发（返回是否命中）。
    ///
    /// `inbound`：`true` = 读到了对端的帧（唯一「对端活着」的证据）；
    /// `false` = 写出成功（半开 TCP 上也会发生，不刷新读活性）。
    pub fn mark_connection_seen(
        &mut self,
        device_id: &str,
        endpoint: &Endpoint,
        now_ms: i64,
        rtt_ms: Option<u64>,
        inbound: bool,
    ) -> bool {
        self.peers
            .get_mut(device_id)
            .is_some_and(|p| p.mark_connection_seen(endpoint, now_ms, rtt_ms, inbound))
    }

    /// 建链时播种读活性（唯一允许在 reader_loop 之外写读活性的入口）。
    pub fn seed_connection_read_seen(
        &mut self,
        device_id: &str,
        endpoint: &Endpoint,
        now_ms: i64,
    ) -> bool {
        self.peers
            .get_mut(device_id)
            .is_some_and(|p| p.seed_connection_read_seen(endpoint, now_ms))
    }

    /// 标记某 peer 的某条 connection 失败（返回是否命中）。
    pub fn mark_connection_failure(&mut self, device_id: &str, endpoint: &Endpoint) -> bool {
        self.peers
            .get_mut(device_id)
            .is_some_and(|p| p.mark_connection_failure(endpoint))
    }

    /// 列出**健康判据认为已死**的连接（`(device_id, endpoint)`）。
    ///
    /// 供 M3#6 的「死链路拆除」使用：半开 TCP 上读循环永久阻塞、链路却一直留在表里，
    /// 于是 `ensure_link` 认为已连通不再重拨，而 `try_send` 只把消息投进 mpsc 就返回 Ok
    /// ⇒ 消息静默投进死路。watchdog 用**放大后的超时**（见调用点）调用本方法拿候选，
    /// 再精确取消那一条连接的读写任务并把它移出链路表，让发现层重新建链。
    ///
    /// 注意：这里只做**查询**，不改状态 —— 拆除动作由 `network::transport` 负责
    /// （mesh 层不该反过来操作传输层）。
    pub fn stale_connections(
        &self,
        now_ms: i64,
        timeout_ms: i64,
        max_failures: u32,
    ) -> Vec<(String, Endpoint)> {
        let mut out = Vec::new();
        for (device_id, peer) in &self.peers {
            for c in peer.connections() {
                if !c.health.is_healthy(now_ms, timeout_ms, max_failures) {
                    out.push((device_id.clone(), c.endpoint.clone()));
                }
            }
        }
        out
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
        assert!(m.mark_connection_seen("ABC123", &lan(), 1000, Some(5), true));
        assert_eq!(m.online_state("ABC123", 1000), PeerOnlineState::Online);

        // LAN 超时，Routed 健康 → 仍 Online
        assert!(m.mark_connection_seen("ABC123", &routed(), 2000, Some(30), true));
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
        assert!(m.mark_connection_seen("ABC123", &lan(), 1000, Some(5), true));
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

    /// M3#6 死链路拆除的判据：只有**读活性过期**的连接才会进候选。
    ///
    /// 这是「消息不再静默投进死路」的关键一步：watchdog 每 5s 拿这个候选列表，
    /// 命中就精确取消那条连接的读写任务并移出链路表（`ensure_link` 随后才会重拨）。
    /// 断言刻意覆盖三种形态：
    ///   · 刚建链（播种读活性）→ **不**候选；
    ///   · 只写不读（半开 TCP 的签名）→ **候选**；
    ///   · 持续有入站帧（健康链路，心跳每 5s 一次）→ **永不**候选。
    #[test]
    fn stale_connections_flags_only_links_without_recent_read() {
        let timeout = 15_000i64;
        let max_failures = 3u32;
        let mut m = PeerManager::new(timeout, max_failures);
        m.merge(PeerCandidate::new("ABC123", PeerIdentity::default(), lan(), PathKind::Lan));

        // 建链播种读活性（register_connection 的行为）→ t=0 时健康，不进候选
        m.seed_connection_read_seen("ABC123", &lan(), 0);
        assert!(m.stale_connections(0, timeout, max_failures).is_empty());

        // 半开链路：一直只有写成功（心跳），从未读到帧；跨过 3× 超时后必须进候选
        let mut writes = 5_000i64;
        while writes <= 60_000 {
            m.mark_connection_seen("ABC123", &lan(), writes, None, false);
            writes += 5_000;
        }
        let stale = m.stale_connections(60_000, timeout * 3, max_failures);
        assert_eq!(
            stale.len(),
            1,
            "只写不读的连接必须被判死（否则 ensure_link 不重拨、消息静默投进死路）"
        );
        assert_eq!(stale[0].0, "ABC123");

        // 健康链路：每 5s 有入站帧 ⇒ 任何时刻都不进候选
        let mut reads = 60_000i64;
        while reads <= 120_000 {
            m.mark_connection_seen("ABC123", &lan(), reads, None, true);
            assert!(
                m.stale_connections(reads, timeout * 3, max_failures).is_empty(),
                "持续有入站帧的链路永远不该被拆"
            );
            reads += 5_000;
        }
    }

    /// 不变量：健康超时必须**明显大于**心跳周期 —— 判据是闭区间 `<=`，
    /// 恰好 2 个周期时没有任何余量，丢一拍心跳就足以把正常链路判成不健康。
    ///
    /// 护栏是复核时补的：生产值曾是 10_000ms、心跳 5s ⇒ 容错**恰好一拍**；
    /// 叠加当时「心跳串行发送」（已修）会把网络抖动放大成链路故障。
    /// 上限受 `RELAY_PEER_TIMEOUT_SECS = 45s` 约束（跨跳节点没有直连链路，
    /// 只靠 10s 一轮的 Presence 保活），故落在 3~9 个心跳周期之间。
    #[test]
    fn health_timeout_outlives_three_heartbeats() {
        use crate::network::transport::HEARTBEAT_INTERVAL_SECS;
        let health_ms = 15_000i64; // 生产值，与 state.rs 的 PeerManager::new 保持一致
        let beats = health_ms / (HEARTBEAT_INTERVAL_SECS as i64 * 1000);
        assert!(
            beats >= 3,
            "健康超时 {health_ms}ms 只覆盖 {beats} 个心跳周期（{HEARTBEAT_INTERVAL_SECS}s）—— \
             少于 3 个会让偶发丢拍被判成链路故障"
        );
        assert!(
            health_ms < crate::protocol::RELAY_PEER_TIMEOUT_SECS * 1000,
            "健康超时不应达到跨跳节点超时（{}s）：跨跳节点没有直连链路",
            crate::protocol::RELAY_PEER_TIMEOUT_SECS
        );
    }
}
