//! 蓝牙传输通道实现（BLE / RFCOMM）。
//!
//! 说明：跨平台蓝牙 I/O 依赖各操作系统栈（Windows 的 WinRT BLE、macOS 的 CoreBluetooth、
//! Linux 的 BlueZ），需要引入平台专用后端。当前实现提供了完整的 [`Transport`] 接口契约与
//! 生命周期管理，`available()` 默认返回 `false`（未编译蓝牙后端），上层会优雅降级为纯局域网。
//!
//! ## 接入真实蓝牙后端的步骤
//! 1. 在 `Cargo.toml` 增加 `[features] bluetooth = ["dep:btleplug"]`，引入 `btleplug`（BLE）或
//!    平台 RFCOMM 实现（Windows 可用 `btleplug` 的 GATT 传输，RFCOMM 可用 Windows 蓝牙套接字）。
//! 2. 在本模块 `#[cfg(feature = "bluetooth")]` 分支中实现扫描、配对、建立虚拟连接与收发。
//! 3. 将 `available()` 改为探测系统蓝牙适配器是否存在，`start()` 执行扫描 / 监听，
//!    `send` / `broadcast` 走蓝牙连接。上层协议无需任何改动。

use async_trait::async_trait;

use super::Transport;

/// 蓝牙通道。当前为接口占位（`available = false`），接入后端后即可透明启用。
pub struct BluetoothTransport {
    running: bool,
}

impl Default for BluetoothTransport {
    fn default() -> Self {
        Self { running: false }
    }
}

#[async_trait]
impl Transport for BluetoothTransport {
    fn name(&self) -> &'static str {
        "蓝牙"
    }

    fn available(&self) -> bool {
        // 未编译蓝牙后端：始终不可用。接入 btleplug/RFCOMM 后改为探测适配器。
        false
    }

    fn running(&self) -> bool {
        self.running
    }

    fn peer_count(&self) -> usize {
        0
    }

    async fn start(&mut self) -> Result<(), String> {
        Err("蓝牙后端未编译（需接入 BLE/RFCOMM 平台实现）".to_string())
    }

    async fn stop(&mut self) -> Result<(), String> {
        self.running = false;
        Ok(())
    }

    async fn send(&self, _peer_id: &str, _payload: &[u8]) -> Result<(), String> {
        Err("蓝牙通道不可用".to_string())
    }

    async fn broadcast(&self, _payload: &[u8]) -> Result<(), String> {
        Err("蓝牙通道不可用".to_string())
    }
}
