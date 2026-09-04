//! 双通道聚合传输层。
//!
//! 设计目标：
//! - **通道无感知**：上层协议、E2EE（ChaCha20-Poly1305）与交互逻辑只依赖 [`Transport`] 接口，
//!   不关心底层是局域网还是蓝牙。
//! - **独立开关**：局域网 / 蓝牙两条通道可单独开启、关闭或同时开启。
//! - **去重**：接收侧统一按 SHA-256 `message_id` 去重（复用 `gossip_engine` 的 Bloom+LRU），
//!   双通道同时送达同一消息时自动丢弃重复。
//! - **智能分流**：双通道同时开启时按流量特征分流——大负载走局域网高带宽通道，
//!   轻量心跳 / 控制信令优先走蓝牙。

pub mod bluetooth;
pub mod lan;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::state::AppState;

/// 大负载判定阈值（字节）。超过该值视为「大文件 / 长文本」，走局域网高带宽通道。
pub const LARGE_PAYLOAD_THRESHOLD: usize = 64 * 1024;

/// 物理通道标识。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Lan,
    Bluetooth,
}

/// 传输通道统一抽象接口。
///
/// 采用 `async-trait` 使异步方法可装箱、可对象安全，便于统一管理不同物理通道。
#[async_trait]
pub trait Transport: Send + Sync {
    /// 通道显示名。
    fn name(&self) -> &'static str;
    /// 硬件 / 系统是否支持该通道。
    fn available(&self) -> bool;
    /// 通道是否正在运行（收发中）。
    fn running(&self) -> bool;
    /// 当前可达对端数量（用于状态监控）。
    fn peer_count(&self) -> usize;
    /// 启动通道。
    async fn start(&mut self) -> Result<(), String>;
    /// 停止通道。
    async fn stop(&mut self) -> Result<(), String>;
    /// 向指定 peer 发送一条已序列化、已加密的协议帧。
    async fn send(&self, peer_id: &str, payload: &[u8]) -> Result<(), String>;
    /// 广播到所有可达节点。
    async fn broadcast(&self, payload: &[u8]) -> Result<(), String>;
}

/// 单条通道的运行状态（供前端状态栏 / 设置页展示）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChannelStatus {
    pub channel: &'static str,
    pub enabled: bool,
    pub available: bool,
    pub running: bool,
    pub peers: usize,
}

/// 双通道聚合管理器：通道开关、分流决策、状态汇总。
pub struct TransportManager {
    pub lan: lan::LanTransport,
    pub bluetooth: bluetooth::BluetoothTransport,
    /// 蓝牙通道是否被用户开启（局域网通道是否开启由 `state.network` 是否运行决定）
    pub bt_enabled: bool,
}

impl TransportManager {
    pub fn new(state: Arc<AppState>) -> Self {
        // 从本地设置恢复蓝牙通道开关状态
        let bt_enabled = {
            let dbc = state.db.lock().unwrap();
            crate::db::get_setting(&dbc, "bt_enabled").map(|v| v == "1").unwrap_or(false)
        };
        Self {
            lan: lan::LanTransport::new(state),
            bluetooth: bluetooth::BluetoothTransport::default(),
            bt_enabled,
        }
    }

    /// 分流决策：根据负载大小与通道可用性选择传输通道。
    /// 待蓝牙后端接入后，在消息 / 文件发送路径中调用以真正分流。
    #[allow(dead_code)]
    pub fn route(&self, payload_len: usize) -> Channel {
        route_payload(payload_len, self.bluetooth.available(), self.bt_enabled)
    }

    /// 切换蓝牙通道开关。
    pub async fn set_bluetooth_enabled(&mut self, on: bool) -> Result<(), String> {
        if on == self.bt_enabled {
            return Ok(());
        }
        if on {
            self.bluetooth.start().await?;
        } else {
            self.bluetooth.stop().await?;
        }
        self.bt_enabled = on;
        Ok(())
    }

    /// 汇总两条通道的状态（局域网运行状态取自 `state.network`）。
    pub fn status(&self) -> Vec<ChannelStatus> {
        let lan_running = self.lan.running();
        vec![
            ChannelStatus {
                channel: "lan",
                enabled: lan_running,
                available: self.lan.available(),
                running: lan_running,
                peers: self.lan.peer_count(),
            },
            ChannelStatus {
                channel: "bluetooth",
                enabled: self.bt_enabled,
                available: self.bluetooth.available(),
                running: self.bluetooth.running(),
                peers: self.bluetooth.peer_count(),
            },
        ]
    }
}

/// 分流决策纯函数（便于测试）：
/// - 蓝牙不可用或未启用 → 局域网。
/// - 大负载（≥ [`LARGE_PAYLOAD_THRESHOLD`]）→ 局域网（高带宽）。
/// - 轻量负载 → 蓝牙（低功耗、控制 / 心跳优先）。
pub fn route_payload(payload_len: usize, bt_available: bool, bt_enabled: bool) -> Channel {
    if !bt_available || !bt_enabled {
        return Channel::Lan;
    }
    if payload_len >= LARGE_PAYLOAD_THRESHOLD {
        Channel::Lan
    } else {
        Channel::Bluetooth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_prefers_lan_when_bt_unavailable_or_disabled() {
        // 蓝牙不可用 → 局域网
        assert_eq!(route_payload(10, false, true), Channel::Lan);
        // 蓝牙可用但未启用 → 局域网
        assert_eq!(route_payload(10, true, false), Channel::Lan);
    }

    #[test]
    fn route_splits_by_payload_size() {
        // 双通道开启且蓝牙可用：小负载走蓝牙，大负载走局域网
        assert_eq!(route_payload(1, true, true), Channel::Bluetooth);
        assert_eq!(route_payload(64 * 1024 - 1, true, true), Channel::Bluetooth);
        assert_eq!(route_payload(64 * 1024, true, true), Channel::Lan);
        assert_eq!(route_payload(1024 * 1024, true, true), Channel::Lan);
    }
}
