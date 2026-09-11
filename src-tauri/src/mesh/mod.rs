//! 统一 Mesh 网络模型（Phase 1 旁路）。
//!
//! 这是 Gosslan 从「Peer = Device = TCP Connection」演进到
//! 「Node Identity → Peer → N × Connection → MeshRouter」的第一块积木。
//!
//! **Phase 1 只新增数据结构与单测，不接管现有网络路径**：
//! `state::Peer` / `state::links` 保持不变，本模块作为旁路存在，
//! 由后续 Phase 2（PeerManager）开始接线。
//!
//! 关键不变量（永久有效）：
//! - P-A01：Identity 永远独立于网络（`device_id` 是唯一 Node Identity）。
//! - P-A02：Peer 与 Connection 分离（一个 Peer 拥有多条 Connection）。

pub mod candidate;
pub mod connection;
pub mod endpoint;
pub mod manager;
pub mod path;
pub mod peer;
pub mod relay_policy;
pub mod router;
pub mod selection;

pub use candidate::PeerCandidate;
pub use connection::{Connection, ConnectionHealth};
pub use endpoint::{BleEndpoint, Endpoint};
pub use manager::{MergeOutcome, PeerManager};
pub use path::PathKind;
pub use relay_policy::{should_forward, RelayInput, RelayPolicy};
pub use peer::{Peer, PeerIdentity, PeerOnlineState};
pub use router::{
    ForwardDecision, MeshDestination, MeshFrame, MeshFrameKind, MeshRouter, DropReason,
};
pub use selection::pick_link;
