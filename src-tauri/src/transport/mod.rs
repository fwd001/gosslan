//! 双通道（局域网 / 蓝牙）的**状态聚合**：给运行时快照与设置页一份 `ChannelStatus[]`。
//!
//! ⚠️ 本模块**不是数据面，也不是分流决策点**。所有帧的收发与选路都在
//! `network/transport.rs`（+ `outbound.rs` 的 `route_order` / `pick_link`）。
//! 这里曾有第三套"通道抽象"：`Transport` trait + `TransportManager::route` +
//! `route_payload`（按 `LARGE_PAYLOAD_THRESHOLD` = 64 KiB 决定"大负载走 LAN、小负载走 BLE"）。
//! 它 `#[allow(dead_code)]` 且零调用点，而**真正的分流早已按语义分类实现在
//! `network/dispatch.rs::message_priority` + `mesh/selection.rs::pick_link`**（并按链路
//! 能力适配分片尺寸，见 `file.rs::chunk_size_for_path`）。
//! 2026-09-24 架构复审 0-A2 删除：留着它的代价不是几十行代码，是"下一个人以为分流在这里改"。

pub mod ble_framing;
pub mod bluetooth;
// BLE 外设（GATT server）角色：只有 macOS + `--features bluetooth` 才编译。
#[cfg(all(feature = "bluetooth", target_os = "macos"))]
pub mod bluetooth_peripheral;
// Android 外设角色的 Rust 侧（JNI 桥，见该文件注释与 ADR-0015 §7.7）。
#[cfg(all(feature = "bluetooth", target_os = "android"))]
pub mod ble_android;
// Windows 外设角色的 Rust 侧（WinRT `GattServiceProvider`，见该文件注释与 ADR-0015 §7.9）。
// 与 macOS/Android 是**第三套平台实现**，但对外接口逐字同形 ⇒ `network/ble.rs` 三边共用一份。
// 没有它，Windows 从不广播 ⇒ 手机永远发现不了 Windows（真机 2026-09-13，
// 见 docs/notes/windows-ble-diagnosis-2026-09-13.md）。
#[cfg(all(feature = "bluetooth", target_os = "windows"))]
pub mod bluetooth_peripheral_windows;
pub mod lan;
// 公网中继的记录层封装与字节流管道（被 `transport/tcp.rs` 的可选密封态使用）。
pub mod relay_seal;
pub mod tcp;

use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;

/// 单条通道的运行状态（供前端状态栏 / 设置页展示）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChannelStatus {
    pub channel: &'static str,
    pub enabled: bool,
    pub available: bool,
    pub running: bool,
    pub peers: usize,
    /// **持久化的用户偏好**（“用户选过什么”），与 running（“此刻是否在跑”）分开。
    ///
    /// 为什么必须分成两个字段：快照里 enabled 表示“运行时是否在跑”，应用刚启动时必然是
    /// false ⇒ 前端无法区分“用户明确关掉了”与“还没启动”，自动拉起（ensureBluetoothOn）
    /// 就会把用户的关闭选择覆盖掉。真机 2026-09-14：电脑端关掉蓝牙，退出重进又被打开。
    pub preferred: bool,
}

/// 双通道聚合：只负责**状态汇总**与"未编译 BLE 后端"时的开关兜底。
pub struct TransportManager {
    pub lan: lan::LanTransport,
    pub bluetooth: bluetooth::BluetoothTransport,
    /// 蓝牙通道是否被用户开启（局域网通道是否开启由 `state.network` 是否运行决定）
    pub bt_enabled: bool,
}

impl TransportManager {
    pub fn new(state: Arc<AppState>) -> Self {
        // 从本地设置恢复蓝牙通道开关状态（缺省值由平台决定：手机默认开，见 `db::get_bt_enabled`）
        let bt_enabled = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            crate::db::get_bt_enabled(&dbc)
        };
        Self {
            lan: lan::LanTransport::new(state),
            bluetooth: bluetooth::BluetoothTransport::default(),
            bt_enabled,
        }
    }

    /// 切换蓝牙通道开关。
    ///
    /// ⚠️ 开了 `bluetooth` feature 时**不用它**：那时真正的运行时是 `network::ble`
    /// （扫描/连接/握手/链路登记），命令层直接调它的 `start/stop`；
    /// 本方法只服务"未编译 BLE 后端"的默认构建（保留它才能给出明确错误）。
    #[cfg_attr(feature = "bluetooth", allow(dead_code))]
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
                // 局域网偏好由 build_runtime_snapshot 从 lan_enabled 覆盖（这里没有 db 句柄）。
                preferred: lan_running,
            },
            ChannelStatus {
                channel: "bluetooth",
                enabled: self.bt_enabled,
                available: self.bluetooth.available(),
                running: self.bluetooth.running(),
                peers: self.bluetooth.peer_count(),
                preferred: self.bt_enabled,
            },
        ]
    }
}
