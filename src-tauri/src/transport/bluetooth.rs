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
/// ## 状态：**已实现、尚未接线**（7-e 的一半）
/// 本模块的 API 已按 btleplug 0.13 的**实际源码**写就并**编译通过**
/// （`cargo build --lib --features bluetooth` 在本机 macOS 上验证）。
/// 还没做的是"把它接到 `BluetoothTransport::start/send/broadcast` 与 `state.links`"
/// —— 那一步需要三平台真机（macOS → Windows → Android）才能验证射频行为，
/// 而射频行为**无法在没有设备的机器上验证**，所以刻意分两步：
/// 先让这一层"写对且能编译"，再接线上真机调。
///
/// 接线时要做的（按顺序）：
/// 1. `start()`：`driver::adapter()` 探测适配器；没有（或未授权）就返回 Err ——
///    **绝不能影响局域网**（上层只在 start 成功后把蓝牙标为 running）。
/// 2. 后台任务：`driver::scan_peers()` 周期扫描 → 产出 `PeerCandidate`（发现 ≠ 建连）；
///    对候选调 `driver::connect()`，握手（双向 Hello 验签）通过后把 `Link` 登记进
///    `state.links`（端点 `Endpoint::Ble`、路径 `PathKind::Bluetooth`），
///    并把 `next_frame()` 收到的整帧喂给 `handle_message()`。
/// 3. `send`/`broadcast`：按 `Endpoint::Ble` 找到对应 `BleConnection`，调 `send_frame()`。
///
/// ## 与 `BleFramer` 的分工
/// 这一层只负责"把字节搬过 GATT"：整帧 →（`transport::ble_framing::fragment`）→ 若干
/// 特征写入；notify 收到的分片 → `BleReassembler` → 整帧交回上层。
/// GATT 的 ATT 有效载荷 = MTU - 3（默认 MTU 23 ⇒ 20 字节）。
// 7-e 接线前这些 API 没有生产调用点（与 `transport/tcp.rs` 同一处理的 allow）。
#[allow(dead_code)]
#[cfg(feature = "bluetooth")]
pub mod driver {
    use std::time::Duration;

    use btleplug::api::{
        Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
        ValueNotification, WriteType,
    };
    use btleplug::platform::{Adapter, Manager, Peripheral};
    use futures::{Stream, StreamExt};
    use uuid::Uuid;

    use super::{CHAR_RX_UUID, CHAR_TX_UUID, SERVICE_UUID};
    use crate::transport::ble_framing::{fragment, BleReassembler, PushOutcome};

    /// BLE 未协商时的默认 ATT MTU（蓝牙规范最小值）。
    pub const BLE_DEFAULT_MTU: u16 = 23;
    /// ATT 头长度（1 字节 opcode + 2 字节句柄）：MTU 减去它才是应用可用载荷。
    pub const ATT_HEADER_LEN: usize = 3;

    /// 把协商到的 MTU 换算成**分片有效载荷上限**。
    ///
    /// 异常值（0、或连 ATT 头都装不下）一律退回默认 MTU —— 绝不能返回 0，
    /// 那会让 `fragment` 直接拒绝一切（表现为"蓝牙永远发不出去且没有明显错误"）。
    pub fn payload_mtu(negotiated: u16) -> usize {
        let mtu = if negotiated <= ATT_HEADER_LEN as u16 {
            BLE_DEFAULT_MTU
        } else {
            negotiated
        };
        mtu as usize - ATT_HEADER_LEN
    }

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).expect("UUID 常量必须合法")
    }

    /// 取第一个可用的蓝牙适配器。没有适配器（或系统未授权）时返回 Err，
    /// 上层据此把蓝牙通道标为不可用 —— **绝不能因此影响局域网**。
    pub async fn adapter() -> Result<Adapter, String> {
        let manager = Manager::new()
            .await
            .map_err(|e| format!("蓝牙管理器初始化失败：{e}"))?;
        let adapters = manager
            .adapters()
            .await
            .map_err(|e| format!("枚举蓝牙适配器失败：{e}"))?;
        adapters
            .into_iter()
            .next()
            .ok_or_else(|| "没有可用的蓝牙适配器".to_string())
    }

    /// 扫描支持 Gosslan 服务的对端（**只发现、不建连** —— 与 P-A04 一致）。
    ///
    /// `ScanFilter` 只是让系统少报无关设备：部分平台会忽略过滤条件，
    /// 所以返回值仍要按"服务集合里有没有我们"复核一次（缺失的交由 `connect` 再判）。
    pub async fn scan_peers(adapter: &Adapter, scan_for: Duration) -> Result<Vec<Peripheral>, String> {
        adapter
            .start_scan(ScanFilter {
                services: vec![uuid(SERVICE_UUID)],
            })
            .await
            .map_err(|e| format!("启动扫描失败：{e}"))?;
        // 扫描是"持续到显式停止"的：这里给一个观察窗口再收结果
        tokio::time::sleep(scan_for).await;
        let all = adapter
            .peripherals()
            .await
            .map_err(|e| format!("读取扫描结果失败：{e}"))?;
        // 停止扫描失败不影响结果（下次 start 会覆盖）
        let _ = adapter.stop_scan().await;
        let svc = uuid(SERVICE_UUID);
        // `services()` 返回的是 `Service` 集合（不是 UUID 集合）⇒ 按 uuid 比
        Ok(all
            .into_iter()
            .filter(|p| p.services().iter().any(|s| s.uuid == svc))
            .collect())
    }

    /// 一条已建立的 BLE 链路：对端句柄 + 收发特征 + 通知流 + 重组器。
    pub struct BleConnection {
        peripheral: Peripheral,
        /// 我们写入的特征（对端读）。
        rx: Characteristic,
        /// 对端写入、我们订阅的特征。
        tx: Characteristic,
        /// **连接期只取一次**的通知流：每次 `next_frame` 重新 `notifications()` 会在
        /// 两次调用之间丢消息（流是带缓冲的接收端）。
        notifications: std::pin::Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
        reassembler: BleReassembler,
        /// 连接内递增的消息号（分片头用；回绕即可，同一时刻在途的消息很少）。
        next_msg_id: u16,
    }

    /// 连上并发现特征。对方不是 Gosslan 端时返回 Err（上层静默跳过即可）。
    pub async fn connect(peripheral: &Peripheral) -> Result<BleConnection, String> {
        peripheral
            .connect()
            .await
            .map_err(|e| format!("连接失败：{e}"))?;
        peripheral
            .discover_services()
            .await
            .map_err(|e| format!("发现 GATT 服务失败：{e}"))?;

        let svc = uuid(SERVICE_UUID);
        let rx_uuid = uuid(CHAR_RX_UUID);
        let tx_uuid = uuid(CHAR_TX_UUID);
        // `characteristics()` 返回 BTreeSet，按 UUID 取即可（同一 UUID 不会重复）
        let chars: Vec<Characteristic> = peripheral
            .characteristics()
            .into_iter()
            .filter(|c| c.service_uuid == svc)
            .collect();
        let rx = chars
            .iter()
            .find(|c| c.uuid == rx_uuid)
            .cloned()
            .ok_or_else(|| "对端没有 Gosslan 的接收特征".to_string())?;
        let tx = chars
            .iter()
            .find(|c| c.uuid == tx_uuid)
            .cloned()
            .ok_or_else(|| "对端没有 Gosslan 的通知特征".to_string())?;
        if !rx.properties.contains(CharPropFlags::WRITE)
            && !rx.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
        {
            return Err("接收特征不可写".to_string());
        }
        if !tx.properties.contains(CharPropFlags::NOTIFY) {
            return Err("通知特征不支持 notify".to_string());
        }
        peripheral
            .subscribe(&tx)
            .await
            .map_err(|e| format!("订阅通知失败：{e}"))?;
        let notifications = peripheral
            .notifications()
            .await
            .map_err(|e| format!("获取通知流失败：{e}"))?;

        Ok(BleConnection {
            peripheral: peripheral.clone(),
            rx,
            tx,
            notifications,
            reassembler: BleReassembler::new(),
            next_msg_id: 1,
        })
    }

    impl BleConnection {
        /// 对端标识（日志/诊断用；**不是身份** —— 身份由 Hello 验签建立）。
        pub fn remote_id(&self) -> String {
            format!("{:?}", self.peripheral.id())
        }

        /// 本连接的分片有效载荷上限（从协商到的 MTU 换算）。
        pub fn payload_mtu(&self) -> usize {
            payload_mtu(self.peripheral.mtu())
        }

        pub async fn is_connected(&self) -> bool {
            matches!(self.peripheral.is_connected().await, Ok(true))
        }

        /// 发一条完整帧：按 MTU 分片后逐片写特征，返回写入的分片数（诊断用）。
        pub async fn send_frame(&mut self, payload: &[u8], msg_id: u16) -> Result<usize, String> {
            let mtu = self.payload_mtu();
            let chunks = fragment(payload, mtu, msg_id)
                .ok_or_else(|| format!("帧无法分片（过大或 MTU 非法：len={} mtu={mtu}）", payload.len()))?;
            let n = chunks.len();
            for chunk in chunks {
                // WithoutResponse：蓝牙链路层本身有重传与顺序保证，逐片确认会慢一个量级；
                // 断连时整条连接都会重建，逐片确认并不能救回消息。
                self.peripheral
                    .write(&self.rx, &chunk, WriteType::WithoutResponse)
                    .await
                    .map_err(|e| format!("BLE 写入失败：{e}"))?;
            }
            Ok(n)
        }

        /// 取下一个**完整帧**（内部消费 notify 分片；超时返回 `Ok(None)`）。
        ///
        /// 超时不让调用方阻塞：外层可以据此周期性 `gc()` 回收半截消息。
        pub async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String> {
            loop {
                let next = tokio::time::timeout(wait, self.notifications.next()).await;
                let notification = match next {
                    Err(_) => return Ok(None), // 窗口内没有分片
                    Ok(None) => return Err("通知流已结束（连接可能已断开）".to_string()),
                    Ok(Some(n)) => n,
                };
                // 过滤非本特征的通知（同一连接上可能还有别的订阅）
                if notification.uuid != self.tx.uuid {
                    continue;
                }
                match self.reassembler.push(&notification.value, crate::db::now_ms()) {
                    PushOutcome::Complete(payload) => return Ok(Some(payload)),
                    PushOutcome::Incomplete | PushOutcome::Dropped(_) => continue,
                }
            }
        }

        /// 回收超时的半截消息（断连/对端消失后调用）。
        pub fn gc(&mut self) -> usize {
            self.reassembler.gc(crate::db::now_ms())
        }

        pub async fn disconnect(&self) -> Result<(), String> {
            self.peripheral
                .disconnect()
                .await
                .map_err(|e| format!("断开失败：{e}"))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::transport::ble_framing::BleReassembler;

        /// MTU 换算：正常值直接用；异常值退回默认（**绝不能返回 0**，否则什么都发不出去）。
        #[test]
        fn payload_mtu_handles_normal_and_bogus_values() {
            assert_eq!(payload_mtu(23), 20, "默认 MTU 23 ⇒ 20 字节载荷");
            assert_eq!(payload_mtu(185), 182, "常见协商值");
            assert_eq!(payload_mtu(517), 514, "5.0 常见大 MTU");
            // 异常：0 / 1 / 2 / 3 都装不下 ATT 头 ⇒ 退回默认
            for bogus in [0u16, 1, 2, 3] {
                assert_eq!(payload_mtu(bogus), 20, "MTU={bogus} 应退回默认而不是返回 0");
            }
            assert_eq!(payload_mtu(4), 1, "刚刚装下 1 字节也算合法");
        }

        /// 分片 → 重组在"从真实 MTU 换算出的载荷"下必须逐字节往返
        /// （这是 driver 与 codec 两个模块之间的接口，最容易出现 off-by-one）。
        #[test]
        fn fragment_and_reassemble_agree_at_real_mtu() {
            let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
            for mtu in [BLE_DEFAULT_MTU, 185, 517] {
                let chunks = fragment(&payload, payload_mtu(mtu), 42).expect("应能分片");
                let mut r = BleReassembler::new();
                let mut done = None;
                for c in &chunks {
                    if let PushOutcome::Complete(p) = r.push(c, 0) {
                        done = Some(p);
                    }
                }
                assert_eq!(done.as_deref(), Some(payload.as_slice()), "MTU={mtu}");
            }
        }
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
