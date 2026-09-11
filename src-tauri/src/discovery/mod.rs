//! 发现层：把「谁存在」从「怎么连」里彻底分离（Phase 3）。
//!
//! 目标结构（演进中）：
//! ```text
//! discovery/
//!   trait.rs    — Discovery 抽象：只产 PeerCandidate（P-A04）
//!   manager.rs  — DiscoveryManager：聚合多源、统一产出
//!   lan.rs      — LanDiscovery：UDP 广播 / 组播 / who_has（下一步）
//!   routed.rs   — RoutedDiscovery：手动 / 已学习端点（下一步）
//! ```
//!
//! Phase 3 第一步只新增骨架与单测，**不动** `network/discovery.rs` 的现有路径；
//! 新发现机制稳定后再由 Adapter 接线、最后才迁移旧实现。

pub mod manager;
pub mod r#trait;

pub use manager::DiscoveryManager;
pub use r#trait::Discovery;
