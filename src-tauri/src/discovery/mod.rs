//! 发现层：把「谁存在」从「怎么连」里彻底分离（Phase 3）。
//!
//! 目标结构（演进中）：
//! ```text
//! discovery/
//!   trait.rs    — Discovery 抽象：只产 PeerCandidate（P-A04）
//!   manager.rs  — DiscoveryManager：聚合多源、统一产出
//!   routed.rs   — RoutedDiscovery：手动端点（Tailscale / VPN，握手后才产候选）
//!   lan.rs      — LanDiscovery：UDP 广播 / 组播 / who_has（下一步）
//! ```
//!
//! Phase 3 逐步推进，**不动** `network/discovery.rs` 的现有路径；
//! 新发现机制稳定后再由 Adapter 接线、最后才迁移旧实现。

pub mod manager;
pub mod routed;
pub mod r#trait;

pub use manager::DiscoveryManager;
pub use r#trait::Discovery;
pub use routed::RoutedDiscovery;
