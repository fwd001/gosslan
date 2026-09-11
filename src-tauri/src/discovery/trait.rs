//! Discovery 抽象：所有发现机制只产 `PeerCandidate`。
//!
//! 架构原则 P-A04：Discovery 负责回答「谁可能存在」，**不负责**连接。

use async_trait::async_trait;

use crate::mesh::candidate::PeerCandidate;

/// 一种「发现机制」。
///
/// 契约（P-A04）：
/// - 只产出 `PeerCandidate`（身份 + 一条端点）；
/// - **不得**创建 Peer、修改最终状态、或直接建立 / 管理连接——那是 PeerManager 的职责；
/// - 同一节点经不同机制被发现（如 LAN announce 与 Routed 探测）会产出指向同一
///   `device_id` 的多个候选，由 PeerManager 合并成单个 Peer 的多条 Connection。
#[async_trait]
pub trait Discovery: Send + Sync {
    /// 机制名称（诊断 / 日志用）。
    fn name(&self) -> &'static str;

    /// 取出下一个候选；`None` = 当前无新候选。
    ///
    /// 非阻塞语义：调用方会以轮询方式反复调用，因此实现不得在此无限等待，
    /// 否则会饿死同批次的其他发现机制。
    async fn next_candidate(&mut self) -> Option<PeerCandidate>;
}
