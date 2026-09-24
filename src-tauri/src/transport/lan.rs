//! 局域网通道的**状态视图**：把 `state.network` / `state.peers` 翻成
//! `ChannelStatus{available, running, peers}`，供运行时快照与设置页展示。
//!
//! ⚠️ 这里**不是**数据面。真正的收发在 `network/transport.rs`（连接表 `state.links` +
//! 每连接 reader/writer）与 `network/discovery.rs`（UDP 发现）。
//! 本文件曾有 `send` / `broadcast` 两个方法（把 payload 反序列化成 `Message` 再走
//! `try_send` / 逐链路 `low` 队列），它们是 `outbound.rs` 里同名逻辑的**第二份实现**、
//! 且零调用点 ⇒ 2026-09-24 架构复审 0-A2 删除。留着不会出错，但会让人以为
//! "要改发送行为就改这里"。

use std::sync::Arc;

use crate::state::AppState;

/// 局域网通道（UDP 广播/组播发现 + TCP 分帧传输，实现见 `network/`）。
pub struct LanTransport {
    state: Arc<AppState>,
}

impl LanTransport {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    pub fn available(&self) -> bool {
        true
    }

    pub fn running(&self) -> bool {
        self.state
            .network
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn peer_count(&self) -> usize {
        self.state
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}
