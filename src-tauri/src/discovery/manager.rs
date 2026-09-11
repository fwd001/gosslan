//! DiscoveryManager：聚合多种发现机制，统一产出 PeerCandidate。

use crate::discovery::Discovery;
use crate::mesh::candidate::PeerCandidate;

/// 聚合多个 Discovery 源的调度器。
///
/// 本类型只做「收集」，不做「决策」：候选是否合并成 Peer、是否新增 Connection，
/// 一律交给 `PeerManager::merge`（P-A04 / P-A05 的分层边界）。
pub struct DiscoveryManager {
    sources: Vec<Box<dyn Discovery>>,
}

impl DiscoveryManager {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// 注册一个发现机制（注册顺序 = 轮询顺序）。
    pub fn register(&mut self, source: Box<dyn Discovery>) {
        self.sources.push(source);
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// 轮询所有源一次，收集候选。
    ///
    /// 每个源会被问到返回 `None` 为止（因此 `next_candidate` 必须是非阻塞的，
    /// 见 trait 文档）。本函数不修改任何 Peer 状态。
    pub async fn poll_candidates(&mut self) -> Vec<PeerCandidate> {
        let mut out = Vec::new();
        for s in self.sources.iter_mut() {
            while let Some(c) = s.next_candidate().await {
                out.push(c);
            }
        }
        out
    }
}

impl Default for DiscoveryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::endpoint::{BleEndpoint, Endpoint};
    use crate::mesh::path::PathKind;
    use crate::mesh::peer::PeerIdentity;
    use async_trait::async_trait;
    use std::net::SocketAddr;

    /// 按给定列表吐候选，吐完返回 None（模拟非阻塞的一次 poll）。
    struct MockDiscovery {
        name: &'static str,
        items: Vec<PeerCandidate>,
    }

    #[async_trait]
    impl Discovery for MockDiscovery {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn next_candidate(&mut self) -> Option<PeerCandidate> {
            if self.items.is_empty() {
                None
            } else {
                Some(self.items.remove(0))
            }
        }
    }

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

    #[tokio::test]
    async fn no_sources_yields_nothing() {
        let mut m = DiscoveryManager::new();
        assert_eq!(m.source_count(), 0);
        assert!(m.poll_candidates().await.is_empty());
    }

    /// 关键：同一 device_id 经 LAN + Routed + BLE 三种机制产出三条不同端点的候选。
    /// 这些候选交给 PeerManager 后应合并成 1 Peer + 3 Connections（Phase 2 已验证）。
    #[tokio::test]
    async fn multiple_sources_yield_same_device_different_endpoints() {
        let mut m = DiscoveryManager::new();
        m.register(Box::new(MockDiscovery {
            name: "lan",
            items: vec![cand("ABC123", lan(), PathKind::Lan)],
        }));
        m.register(Box::new(MockDiscovery {
            name: "routed",
            items: vec![cand("ABC123", routed(), PathKind::Routed)],
        }));
        m.register(Box::new(MockDiscovery {
            name: "ble",
            items: vec![cand("ABC123", ble(), PathKind::Bluetooth)],
        }));

        assert_eq!(m.source_count(), 3);
        let got = m.poll_candidates().await;

        assert_eq!(got.len(), 3);
        // 全部指向同一身份
        assert!(got.iter().all(|c| c.device_id == "ABC123"));
        // 端点互异 —— 合并后应为 3 条 Connection
        let mut endpoints: Vec<_> = got.iter().map(|c| c.endpoint.clone()).collect();
        endpoints.sort_by_key(|e| format!("{e:?}"));
        endpoints.dedup();
        assert_eq!(endpoints.len(), 3);
    }

    /// 全链路串联：Discovery（LAN + Routed 两源）→ PeerManager，
    /// 同一 device_id 的两条端点必须合并成 **1 Peer + 2 Connections**。
    ///
    /// 这是 Phase 2 与 Phase 3 的协同验收：发现层只产候选，合并语义由 PeerManager 持有。
    #[tokio::test]
    async fn discovery_to_peer_manager_merges_multipath() {
        use crate::mesh::manager::PeerManager;

        let mut dm = DiscoveryManager::new();
        dm.register(Box::new(MockDiscovery {
            name: "lan",
            items: vec![cand("ABC123", lan(), PathKind::Lan)],
        }));
        dm.register(Box::new(MockDiscovery {
            name: "routed",
            items: vec![cand("ABC123", routed(), PathKind::Routed)],
        }));

        let mut pm = PeerManager::new(10_000, 3);
        for c in dm.poll_candidates().await {
            pm.merge(c);
        }

        assert_eq!(pm.peer_count(), 1, "同一 device_id 必须只有一个 Peer");
        assert_eq!(
            pm.get("ABC123").unwrap().connection_count(),
            2,
            "LAN 与 Routed 各一条 Connection"
        );

        // 未握手 ⇒ 无健康记录 ⇒ 仍 Offline（发现 ≠ 连通，Phase 5 建链后才 mark_seen）
        assert_eq!(
            pm.online_state("ABC123", 0),
            crate::mesh::peer::PeerOnlineState::Offline
        );
    }

    /// 已耗尽的源再次 poll 不应重复产出（幂等，配合 PeerManager 的幂等 merge）。
    #[tokio::test]
    async fn drained_source_yields_nothing_on_second_poll() {
        let mut m = DiscoveryManager::new();
        m.register(Box::new(MockDiscovery {
            name: "lan",
            items: vec![cand("ABC123", lan(), PathKind::Lan)],
        }));

        assert_eq!(m.poll_candidates().await.len(), 1);
        assert!(m.poll_candidates().await.is_empty());
    }
}
