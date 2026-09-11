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

/// Gosslan 的 BLE GATT 服务与特征 UUID（128 位，随机生成后固定 —— 不能改，
/// 改了对端就发现不了彼此；也不要用标准 UUID，免得与其它 BLE 设备混淆）。
///
/// 只在 `bluetooth` feature 下被驱动使用（默认构建里它们是文档与常量占位）。
#[cfg_attr(not(feature = "bluetooth"), allow(dead_code))]
pub const SERVICE_UUID: &str = "6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c31";
/// 中心设备 → 外围设备（我们写、对端读/通知）
#[cfg_attr(not(feature = "bluetooth"), allow(dead_code))]
pub const CHAR_RX_UUID: &str = "6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c32";
/// 外围设备 → 中心设备（对端通知我们）
#[cfg_attr(not(feature = "bluetooth"), allow(dead_code))]
pub const CHAR_TX_UUID: &str = "6b1a7e60-3f4c-4d8a-9c2b-1e5f7a9d0c33";

/// BLE GATT 后端（`feature = "bluetooth"` 时才编译）。
///
/// ## 职责边界（ADR-0015 §2）
/// 只做 **packet transport**：扫描 / 连接 / 分片收发 / 断连。不解密、不写库、
/// 不建用户、不做 BitChat 的 channel；业务层（Gossip / E2EE / Outbox）完全复用。
///
/// ## 当前状态（7-a：feature 门骨架）
/// 依赖与 feature 已就位（`bluetooth = ["dep:btleplug"]`，**默认关闭**）；驱动实现是
/// 下一步（7-e）。**刻意不在这里写"看起来能用"的 btleplug 调用**：
/// 本机沙箱无法下载依赖 ⇒ 那些调用无法编译验证，留着就是给后来者埋雷。
///
/// ## 实现时要做的事（按此顺序，每步都能单独验证）
/// 1. `Manager::new()` → `adapters()` 取适配器；没有（或未授权）就返回 Err，
///    **绝不能影响局域网通道**（上层只在 `start()` 成功后才把蓝牙标为 running）。
/// 2. 扫描时用本模块的 `SERVICE_UUID` 过滤（`ScanFilter`），只认自家设备。
/// 3. 连接后 `discover_services()`，按 `CHAR_RX_UUID`（我们写）与 `CHAR_TX_UUID`
///    （订阅 notify）取特征；缺失即视为"不是 Gosslan 端"。
/// 4. 发送：整帧 → `transport::ble_framing::fragment(payload, mtu, msg_id)` → 逐片写
///    `CHAR_RX`（`WriteType::WithoutResponse`：链路层本身有重传，逐片确认慢一个量级）。
/// 5. 接收：notify 分片 → `BleReassembler::push` → 收齐后把整帧交回上层；
///    断连时 `gc()` 回收半截消息。
/// 6. `mtu` 从协商结果取（ATT 有效载荷 = MTU - 3；默认 23 ⇒ 20 字节）。
///
/// ⚠️ 上述 API 名称以 btleplug 0.13 的文档为准，实现时**先核对一遍再写**
/// （本机无法 `cargo build --features bluetooth`：沙箱禁止写 ~/.cargo 缓存，
/// 所以这一步必须在能联网编译的环境里做）。
#[cfg(feature = "bluetooth")]
pub mod driver {
    use super::{CHAR_RX_UUID, CHAR_TX_UUID, SERVICE_UUID};

    /// 供上层日志/诊断读取的三个 UUID（驱动实现后用于扫描过滤与特征查找）。
    pub fn uuids() -> (&'static str, &'static str, &'static str) {
        (SERVICE_UUID, CHAR_RX_UUID, CHAR_TX_UUID)
    }
}

/// 蓝牙通道。接口占位：`available` 取决于是否编译了 BLE 后端（feature `bluetooth`），
/// 驱动实现前 `start()` 一律返回明确错误，上层据此继续走局域网。
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
        // 编译了 BLE 后端就"可能可用"；真正的探测（有没有适配器 / 用户是否授权）
        // 只能在异步的 `start()` 里做 —— 这个接口是同步的，不能在这里 await。
        // 未编译 feature 时恒 false（默认构建即此分支）。
        cfg!(feature = "bluetooth")
    }

    fn running(&self) -> bool {
        self.running
    }

    fn peer_count(&self) -> usize {
        0
    }

    async fn start(&mut self) -> Result<(), String> {
        // 两种情况下都不能声称"已启动"：
        // · 未编译 feature（默认构建）：明确提示怎么开；
        // · 编译了 feature 但驱动还没实现（7-e 待做）：同样明确报错。
        // 上层据此把蓝牙通道标为不可用并继续走局域网 —— 这是"0 配置无感"的前提：
        // 蓝牙没起来绝不能让用户看到失败，更不能影响 LAN。
        if cfg!(feature = "bluetooth") {
            Err("蓝牙驱动尚未实现（feature 门已就位，见 ADR-0015 的 7-e）".to_string())
        } else {
            Err("蓝牙后端未编译（用 --features bluetooth 构建，见 ADR-0015）".to_string())
        }
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
