//! PeerCandidate：Discovery 层产出、PeerManager 消费的候选节点。
//!
//! 架构原则 P-A04：Discovery 只产 `PeerCandidate`，**不能**创建 Peer、
//! 修改最终状态、或直接管理连接。候选是否合并成 Peer、是否新增 Connection，
//! 一律由 `PeerManager` 决定。

use super::connection::Connection;
use super::endpoint::Endpoint;
use super::path::PathKind;
use super::peer::PeerIdentity;

/// 一个被「发现」到的候选节点：身份 + 一条可达端点。
///
/// 注意：候选里的 endpoint 是临时信息（P-A01），身份是 `device_id`。
#[derive(Clone, Debug, PartialEq)]
pub struct PeerCandidate {
    pub device_id: String,
    pub identity: PeerIdentity,
    pub endpoint: Endpoint,
    pub path_kind: PathKind,
}

impl PeerCandidate {
    pub fn new(
        device_id: impl Into<String>,
        identity: PeerIdentity,
        endpoint: Endpoint,
        path_kind: PathKind,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            identity,
            endpoint,
            path_kind,
        }
    }

    /// 转成一条 Connection（由 PeerManager 在 merge 时调用）。
    pub fn into_connection(self) -> Connection {
        Connection::new(self.device_id.clone(), self.endpoint, self.path_kind)
    }
}
