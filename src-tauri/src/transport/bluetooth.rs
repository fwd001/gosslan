//! 蓝牙传输通道实现（BLE GATT）。
//!
//! ## 本模块的三块，接线状态**各不相同**（2026-09-16 逐项核对调用点）
//!
//! | 块 | 内容 | 状态 |
//! |---|---|---|
//! | `driver`（`pub mod`，仅 `feature = "bluetooth"`） | btleplug 封装：扫描 / 连接 / 分片收发 | ✅ **已接线** —— `network/ble.rs` 有 7 处调用（`adapter` / `scan_peers` / `connect` / `BleConnection` / `BleWriter::send_frame`） |
//! | `BluetoothTransport`（本文件底部） | `Transport` trait 实现 | ⚠️ **占位**：只服务 `TransportManager::status()` 的展示（`running` 恒 `false`、`peer_count` 恒 0），未接管真实收发 |
//! | `TransportManager::route()`（在 `transport/mod.rs`） | 按负载大小分流 LAN / BLE | ⚠️ **未接线**（带 `#[allow(dead_code)]` 与"待蓝牙后端接入后…调用以真正分流"） |
//!
//! 一句话：**BLE 的字节级收发早已在跑** —— `network/ble.rs` 是策略侧（扫描循环、握手验签、
//! 链路登记、去重、退避），本模块 `driver` 是字节侧。没接的是"把 BLE 也挂到
//! `Transport` 抽象与分流决策上"，**不是**"BLE 还没实现"。
//!
//! ⚠️ **历史（2026-09-16 修正）**：本文件此前写着「当前实现提供了完整的 `Transport` 接口契约……
//! **需要引入平台专用后端**」外加一节「接入真实蓝牙后端的步骤」——那套步骤**早已做完**
//! （`Cargo.toml` 已有 `bluetooth` feature、btleplug 已集成、`driver` 已实现并接线）。
//! 那节"接线时要做的（按顺序）"清单同样过期：它列的扫描 → 候选 → 连接 → Hello → 登记
//! `state.links` **已经在 `network/ble.rs` 里做完了**，只是没做在本模块内。
//!
//! **通病提醒**：`"待接线"的注释在接线之后没人回头改`。同类问题已在
//! `ble_framing.rs`（Phase 3）、`transport/tcp.rs`（Phase 5 上报）各发现一次。
//! 这些注释大多还挂着 `#[allow(dead_code)]`，把编译器本会给出的提示一起静音了 ——
//! 所以它们能存活很久。**动这一带代码前请先核对调用点，别信注释。**

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
/// ## 状态：**已接线**（2026-09-16 核对调用点后修正此前的"尚未接线"）
///
/// 调用点全在 `network/ble.rs` —— 那是本模块的**策略侧**：
///
/// ```text
/// :179  driver::adapter()        探测适配器（没有 / 未授权 ⇒ Err，绝不影响局域网）
/// :386  driver::scan_peers()     周期扫描 ⇒ PeerCandidate（发现 ≠ 建连）
/// :785  driver::connect()        连上并发现特征
/// :835  driver::BleConnection    连接对象（next_frame 收到的整帧喂给 handle_message）
/// :863  writer.send_frame()      整帧 ⇒ 分片 ⇒ 写特征
/// ```
///
/// 握手验签、链路登记进 `state.links`（`Endpoint::Ble` / `PathKind::Bluetooth`）、
/// 去重与退避**都在 `network/ble.rs`**，不在本模块。此前本节列的「接线时要做的（按顺序）」
/// 清单确实已经做完 —— 只是做在那边，所以本节已经过期。
///
/// ## 与 `ble_framing` 的分工
/// 这一层只负责"把字节搬过 GATT"：整帧 →（`transport::ble_framing::fragment`）→ 若干
/// 特征写入；notify 收到的分片 → `BleReassembler` → 整帧交回上层。
/// ATT 有效载荷的换算**只有一处**（`ble_framing::att_payload_budget`，见 INV-P23）——
/// 本模块的 `payload_mtu()` 只做转发，**不要再在这里写 `MTU - 3`**。
///
/// ## 仍未接线的 4 项（逐个标注，而不是给整个模块挂 allow）
/// 下面这些是为「把 BLE 挂上 `Transport` 抽象 / 断连」预备的 API，当前无生产调用点。
/// 2026-09-16 把**模块级** `#[allow(dead_code)]` 收成逐个标注：模块级 allow 会把
/// "本层是否还在被调用"这个信息一起静音。实测收掉后只剩这 4 项 ⇒ 其余全在跑。
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

    /// BLE 链路建立后**等待服务可读**的重试参数（真机 2026-09-13，Windows P0）。
    ///
    /// ## 为什么必须重试（不是"更稳一点"，而是 Windows 上根本连不上）
    ///
    /// `driver::connect` 原本是「`connect()` 然后立刻 `discover_services()`」，一次重试都没有。
    /// 这在 macOS/Android 上恰好能过，但在 **Windows** 上必失败，因为 btleplug 的 WinRT 后端里
    /// "连接"**本身就是一次 `GetGattServicesAsync(Uncached)`**：
    ///
    /// ```text
    /// // btleplug-0.13.0/src/winrtble/peripheral.rs:488  connect()
    /// let device = BLEDevice::new(...).await?;
    /// device.connect().await?;                       // ← 这里
    /// // btleplug-0.13.0/src/winrtble/ble/device.rs:112  BLEDevice::connect()
    /// let service_result = self.get_gatt_services(BluetoothCacheMode::Uncached).await?;
    /// utils::to_error(service_result.Status()?)   // Unreachable → Error::NotConnected
    /// ```
    ///
    /// BLE 从"发起连接"到"对端 GATT 数据库可读"之间有几百 ms~数秒的窗口，这个窗口里
    /// WinRT 返回 `GattCommunicationStatus::Unreachable`，btleplug 原样翻成 `NotConnected`，
    /// 于是日志里就是「连接失败：Not connected」——**看着像被拒，其实是"还没准备好"**。
    ///
    /// 真机证据（`%APPDATA%\com.gosslan.app\logs\gosslan.log`）：
    /// `[DISCOVERY] 候选可拨 … ⇒ 开始连接` → **2~3 秒后** `连接失败：Not connected`，
    /// 13 秒一轮反复出现，从未走到握手阶段。
    /// 详见 `docs/notes/windows-ble-diagnosis-2026-09-13.md`。
    pub const LINK_READY_ATTEMPTS: u32 = 12;
    /// 两次尝试之间的等待。12 × 250ms = 3s 上限：够覆盖 WinRT 的准备窗口，
    /// 又不会把 10s 的扫描周期拖垮（`network/ble.rs` 给每个候选一个独立任务）。
    pub const LINK_READY_WAIT: Duration = Duration::from_millis(250);
    /// 整条连接（`connect()`）失败后的退避：给协议栈一点时间再重来，而不是立刻猛敲。
    pub const CONNECT_RETRY_WAIT: Duration = Duration::from_millis(400);
    /// 整条 `connect()` 的尝试次数（含第一次）。
    pub const CONNECT_ATTEMPTS: u32 = 3;

    /// 把协商到的 MTU 换算成**分片有效载荷上限**（central 侧）。
    ///
    /// 常量与换算本体都在 [`crate::transport::ble_framing`] —— **central 与外设两侧
    /// 用的是同一组常量、两个换算入口**：
    ///
    /// | 侧 | 入口 | 输入语义 |
    /// |---|---|---|
    /// | central（本函数） | `att_payload_budget` | 协商出的 **ATT MTU**（要减 ATT 头） |
    /// | peripheral | `notify_payload_budget` | 对端声明的**通知载荷上限**（本身已是载荷，不减） |
    ///
    /// ⚠️ 2026-09-16 修正：本模块此前自己定义了一份 `BLE_DEFAULT_MTU = 23` /
    /// `ATT_HEADER_LEN = 3`，是 `ble_framing` 那份的**重复**；而旧文档还写着
    /// 「**外设侧用的是同一个函数**」—— **那句只对 Windows 成立**（macOS 当时另有一份
    /// `central_payload_mtu` 实现）。两处都已收敛，由 `scripts/check-ble-constants.mjs` 守门。
    pub fn payload_mtu(negotiated: u16) -> usize {
        crate::transport::ble_framing::att_payload_budget(negotiated)
    }

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).expect("UUID 常量必须合法")
    }

    /// 取第一个可用的蓝牙适配器。没有适配器（或系统未授权）时返回 Err，
    /// 上层据此把蓝牙通道标为不可用 —— **绝不能因此影响局域网**。
    ///
    /// 返回值第二个元素是**本机适配器自己的蓝牙地址**（平台不暴露时为 `None`）。
    ///
    /// 为什么要把它带出来（真机 2026-09-13 第四轮）：排查中最大的困扰是
    /// "扫描结果里哪个地址是这台机器自己"—— 自己的广播**也会**出现在扫描结果里
    /// （日志里的 `收到 N 个广播，其中 M 个是本应用服务`）。
    /// 之前只能靠 RSSI 波动幅度去猜（稳定的像自己的网卡），而**猜错会让整个判断反向**
    /// （前几轮就一直把对端当成自己）。带出这一行之后，"哪个是对端"就是纯粹的事实比对。
    pub async fn adapter() -> Result<(Adapter, Option<String>), String> {
        // Android：btleplug 需要先 `platform::init()`（由 `BlePeripheral.bootstrap` 经 JNI 触发）。
        // 未就绪时**绝不能**往下走 —— `Manager::new()` 会在 crate 内 panic，而安卓 release 是
        // `panic = "abort"`（整进程消失，用户实测的闪退）。这里提前返回 Err，UI 顶多显示"蓝牙不可用"。
        #[cfg(target_os = "android")]
        if !crate::transport::ble_android::droidplug_ready() {
            return Err("Android 蓝牙后端尚未就绪（btleplug droidplug 未初始化）—— \
                 若是 release 包，检查 proguard 是否保留了 com.nonpolynomial.btleplug.**"
                .to_string());
        }
        let manager = Manager::new()
            .await
            .map_err(|e| format!("蓝牙管理器初始化失败：{e}"))?;
        let adapters = manager
            .adapters()
            .await
            .map_err(|e| format!("枚举蓝牙适配器失败：{e}"))?;
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| "没有可用的蓝牙适配器".to_string())?;
        // 本机适配器自己的地址（平台不暴露时为 None）—— 由调用方写进日志，
        // 因为 driver 这一层没有 logger（见本函数文档注释里的理由）。
        let local_addr = match adapter.adapter_address().await {
            Ok(Some(addr)) => Some(addr.to_string()),
            Ok(None) => None,
            Err(_) => None,
        };
        Ok((adapter, local_addr))
    }

    /// 扫描支持 Gosslan 服务的对端（**只发现、不建连** —— 与 P-A04 一致）。
    ///
    /// 返回 `(命中本服务的候选, 本次一共收到多少个广播)` —— 后者只用于诊断日志
    /// （区分"扫描根本收不到广播"和"收到了但都不是本服务"，真机排查时这两件事完全不同）。
    ///
    /// ## ⚠️ 两条真机踩出来的铁律，都别再改回去
    /// 1. **不在平台层用服务 UUID 过滤**：macOS 的 `CBAdvertisementDataServiceUUIDsKey` 会把
    ///    128 位 UUID 放进**扫描响应（scan response）**，而 Android 的硬件过滤只匹配
    ///    **主广播包** ⇒ 用 `ScanFilter{services}` 会**永远收不到 Mac 的广播**。
    ///    真机症状（用户 2026-09-12）：Mac（central）能找到手机，手机（central）却
    ///    "扫描到 0 个候选"，两台设备永远发现不了彼此。
    ///    所以这里扫**全部**设备，再在 Rust 侧按广播内容判定 —— `properties().services`
    ///    正是 btleplug 从**广播/扫描响应**里解析出来的，两端都可靠。
    /// 2. **不要用 `Peripheral::services()` 判定**：它在 Android 上只有 `discover_services()`
    ///    （= 连接）之后才有值，未连接时恒为空集合 ⇒ 会把所有候选丢掉。
    ///    "对方不是 Gosslan 端"由 `connect()` 里的**特征校验**兜住 —— 那一步本来就要连上。
    pub async fn scan_peers(
        adapter: &Adapter,
        scan_for: Duration,
    ) -> Result<(Vec<Peripheral>, usize), String> {
        adapter
            .start_scan(ScanFilter::default())
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
        let total = all.len();
        // `properties()` 是 **async**（读的是内存里的广播属性），所以这里用 async 过滤
        let mut hits = Vec::new();
        for p in all {
            if let Ok(Some(props)) = p.properties().await {
                if props.services.contains(&svc) {
                    hits.push(p);
                }
            }
        }
        Ok((hits, total))
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
    }

    /// 连上并发现特征。对方不是 Gosslan 端时返回 Err（上层静默跳过即可）。
    ///
    /// ## 为什么要重试（Windows P0，见 `LINK_READY_ATTEMPTS` 的文档注释）
    ///
    /// `connect()` 与 `discover_services()` 都可能**暂时性**失败：前者在 Windows 上
    /// 直接就是一次 uncached 的服务查询，链路刚建立时它会返回 `Unreachable`/`NotConnected`。
    /// 所以这里做两层有界重试，**失败原因原样带出去**（真机排障只认这条日志）。
    ///
    /// 重试是**无副作用**的：`connect()` 幂等（btleplug 内部 `is_connected` 先判一次），
    /// `discover_services()` 只是重读 GATT 数据库。macOS/Android 上第一次就成功，
    /// 因此不会多等一次 —— 这条改动不会降低它们的可用性。
    pub async fn connect(peripheral: &Peripheral) -> Result<BleConnection, String> {
        // ⚠️ 重试的形状很讲究（真机 2026-09-13 第二轮）：**每次重试都重建
        // `BluetoothLEDevice` 是错的**。WinRT 的 `connect()` 内部要
        // `FromBluetoothAddressAsync` + `GattSession::FromDeviceIdAsync`，既慢又可能
        // 因为"上一个对象还没释放"而互相干扰。所以顺序反过来：
        //   ① 先用**便宜**的 `discover_services()` 试 —— 链路如果已经好了，一次就成；
        //   ② 只有它也不成，才走完整的 `connect()`（内部会重建设备对象）。
        // 这样既覆盖"设备对象建好了但服务还读不到"，也避免把重试变成自我干扰。
        let mut last_err = String::new();

        // ---- ① 便宜路径：**已经连着**的时候先直接试读服务 ----
        //
        // ⚠️ **必须先用 `is_connected()` 判一下**（2026-09-13 合并评审发现）：
        // `discover_services()` 在**未连接**时必然失败，而这条循环是
        // 12 次 × 250ms = **3 秒**。原实现无条件先跑它 ⇒ 在 macOS/Android 上每次拨号都会
        // 凭空多等 3 秒才去真正 `connect()`（这两端本来第一次 connect 就成功），
        // 白白拖慢握手、还可能撞上 10s 的握手超时。
        // WinRT 的语义不同（`connect()` 内部就是一次 uncached 服务查询），所以
        // "先试着读服务"在 Windows 上是有意义的 —— 用 `is_connected()` 精确区分这两种情形。
        if matches!(peripheral.is_connected().await, Ok(true)) {
            for attempt in 1..=LINK_READY_ATTEMPTS {
                match peripheral.discover_services().await {
                    Ok(()) => {
                        return finish_connect(peripheral).await;
                    }
                    Err(e) => {
                        last_err = format!(
                            "发现 GATT 服务失败（第 {attempt}/{LINK_READY_ATTEMPTS} 次）：{e}"
                        );
                        if attempt < LINK_READY_ATTEMPTS {
                            tokio::time::sleep(LINK_READY_WAIT).await;
                        }
                    }
                }
            }
        }

        // ---- ② 完整路径：重建设备对象 + 等 GATT 数据库可读 ----
        for attempt in 1..=CONNECT_ATTEMPTS {
            match peripheral.connect().await {
                Ok(()) => {
                    last_err.clear();
                    break;
                }
                Err(e) => {
                    last_err = format!("连接失败（第 {attempt}/{CONNECT_ATTEMPTS} 次）：{e}");
                    if attempt < CONNECT_ATTEMPTS {
                        tokio::time::sleep(CONNECT_RETRY_WAIT).await;
                    }
                }
            }
        }
        if !last_err.is_empty() {
            return Err(last_err);
        }

        // connect() 成功之后再读一次服务（WinRT 上"连上"不等于"服务可读"，见上）
        let mut discovered = false;
        for attempt in 1..=LINK_READY_ATTEMPTS {
            match peripheral.discover_services().await {
                Ok(()) => {
                    discovered = true;
                    break;
                }
                Err(e) => {
                    last_err =
                        format!("发现 GATT 服务失败（第 {attempt}/{LINK_READY_ATTEMPTS} 次）：{e}");
                    if attempt < LINK_READY_ATTEMPTS {
                        tokio::time::sleep(LINK_READY_WAIT).await;
                    }
                }
            }
        }
        if !discovered {
            return Err(last_err);
        }

        finish_connect(peripheral).await
    }

    /// `connect()` + 服务发现都成功之后：取特征、订阅通知、组装连接。
    ///
    /// 拆出来是因为它**没有任何重试语义**，而上面的重试路径要在两个不同的入口
    /// （便宜的 discover_services 命中 / 完整 connect 命中）都走到这里 ——
    /// 复制两份的话，"取特征/订阅"这一步迟早会漂移。
    async fn finish_connect(peripheral: &Peripheral) -> Result<BleConnection, String> {
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
            && !rx
                .properties
                .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
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
        })
    }

    /// 只负责**写**的一半：整帧分片 → 逐片写特征。
    ///
    /// 与读半分开的原因：读要长时间 await 通知流（最长一个扫描/读窗口），
    /// 若读写共用一把 `Mutex<BleConnection>`，一个正在等待通知的读会把发送也卡住。
    /// GATT 本身允许写与通知并发，拆开即天然无锁。
    pub struct BleWriter {
        peripheral: Peripheral,
        rx: Characteristic,
        /// 连接内递增的消息号（分片头用；回绕即可）。
        ///
        /// ⚠️ **这是当前唯一一份**"连接内消息号"状态（2026-09-17 收敛）：
        /// `BleConnection` 上曾经并列另一份 `next_msg_id` 字段,但没有任何读者,
        /// 而且 `BleConnection::into_split` 拆分时把它**丢了**(新 BleWriter 永远从 1 重启)——
        /// 也就是"两份状态、一份死"的同型风险,与 `INV-P23` 同一类。
        /// 现在只剩这里,新增/拆分/接线时**不要再让对端也存一份**。
        next_msg_id: u16,
    }

    /// 只负责**读**的一半：消费通知流 → 分片重组 → 整帧。
    pub struct BleReader {
        notifications: std::pin::Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
        tx_uuid: Uuid,
        reassembler: BleReassembler,
        /// 已收到的通知条数 / 字节数（**诊断用**）。
        ///
        /// 为什么必须有：真机现象是"安卓侧 notify 全是成功、Mac 侧一个字节都没收到"。
        /// 只有把"到底有没有分片到过"记下来，才能区分
        /// **对端没发出去** 与 **发出来了但这边没收到** —— 这两者的修法完全不同。
        seen_notifications: u64,
        seen_bytes: usize,
        /// 不属于本特征的通知（同连接上可能订阅了别的东西）。
        seen_other_uuid: u64,
        /// **被丢弃的分片数 + 最近一次原因**（诊断用）。
        ///
        /// 为什么需要（用户 2026-09-13 真机：「发图片时报了分片/顺序相关的错」）：
        /// `BleReassembler::push` 对坏片（重复 / 越界 / 分片数不一致 / 超上限）只返回
        /// `Dropped(&str)`，**原来这一路是静默吞掉的** —— 真机上只看到"图片没到"，
        /// 看不到"到了、但被分片层丢了、原因是…"。把原因留给上层读循环按需打日志。
        dropped: u64,
        last_drop_reason: &'static str,
    }

    impl BleConnection {
        /// 拆成写半与读半（见 `BleWriter`/`BleReader` 的注释）。
        pub fn into_split(self) -> (BleWriter, BleReader) {
            (
                BleWriter {
                    peripheral: self.peripheral,
                    rx: self.rx,
                    next_msg_id: 1,
                },
                BleReader {
                    notifications: self.notifications,
                    tx_uuid: self.tx.uuid,
                    reassembler: self.reassembler,
                    seen_notifications: 0,
                    seen_bytes: 0,
                    seen_other_uuid: 0,
                    dropped: 0,
                    last_drop_reason: "",
                },
            )
        }

        /// 对端标识（日志/诊断用；**不是身份** —— 身份由 Hello 验签建立）。
        ///
        /// ⚠️ 当前无生产调用点（日志里用的是 `Endpoint::Ble` 的字符串形式）。
        /// 保留是因为排查"哪条链路在动"时它是第一手信息，接线日志时应当用上。
        #[allow(dead_code)]
        pub fn remote_id(&self) -> String {
            self.peripheral.id().to_string()
        }

        /// 这条连接**当前**的协商 MTU（字节）。每次调用都从 btleplug 内部 AtomicU16
        /// 读一次，所以能反映 WinRT `MaxPduSizeChanged` 等异步事件带来的更新。
        pub fn mtu(&self) -> u16 {
            self.peripheral.mtu()
        }
    }

    impl BleWriter {
        /// 本连接的分片有效载荷上限（从协商到的 MTU 换算）。
        pub fn payload_mtu(&self) -> usize {
            payload_mtu(self.peripheral.mtu())
        }

        /// 这条连接**当前**的协商 MTU（字节）。
        #[allow(dead_code)]
        pub fn mtu(&self) -> u16 {
            self.peripheral.mtu()
        }

        /// ⚠️ 当前无生产调用点：重连/健康检查目前由 `network/ble.rs` 的链路状态
        /// 与 `mesh::connection` 的健康判据承担；接线 `Transport::running` 时应当用上。
        #[allow(dead_code)]
        pub async fn is_connected(&self) -> bool {
            matches!(self.peripheral.is_connected().await, Ok(true))
        }

        /// 发一条完整帧：按 MTU 分片后逐片写特征，返回写入的分片数（诊断用）。
        pub async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String> {
            // 消息号在**本半**自增（分片头用；回绕即可，同一时刻在途的消息很少）
            let msg_id = self.next_msg_id;
            self.next_msg_id = self.next_msg_id.wrapping_add(1).max(1);
            let mtu = self.payload_mtu();
            let chunks = fragment(payload, mtu, msg_id).ok_or_else(|| {
                format!(
                    "帧无法分片（过大或 MTU 非法：len={} mtu={mtu}）",
                    payload.len()
                )
            })?;
            let n = chunks.len();
            for chunk in chunks {
                // WithoutResponse：蓝牙链路层本身有重传与顺序保证，逐片确认会慢一个量级；
                // 断连时整条连接都会重建，逐片确认并不能救回消息。
                if let Err(e) = self
                    .peripheral
                    .write(&self.rx, &chunk, WriteType::WithoutResponse)
                    .await
                {
                    // 兜底（真机 2026-09-14）：Android 在多片帧上会把第 2 片的 no-response 写
                    // 直接拒掉。若这条特征也支持带响应写，就用它重试**同一片** —— 慢一档，
                    // 但比整帧失败→拆链路→重连循环好得多。只在失败路径发生。
                    if !self.rx.properties.contains(CharPropFlags::WRITE) {
                        return Err(format!("BLE 写入失败：{e}"));
                    }
                    self.peripheral
                        .write(&self.rx, &chunk, WriteType::WithResponse)
                        .await
                        .map_err(|e2| {
                            format!("BLE 写入失败：{e}｜WithResponse 兜底也失败：{e2}")
                        })?;
                }
            }
            Ok(n)
        }

        /// ⚠️ 当前无生产调用点：主动断连目前由上层 drop 连接对象完成。
        /// 接线 `Transport::stop` / 换路重连时应当用上（显式断连比等 drop 更可控）。
        #[allow(dead_code)]
        pub async fn disconnect(&self) -> Result<(), String> {
            self.peripheral
                .disconnect()
                .await
                .map_err(|e| format!("断开失败：{e}"))
        }
    }

    impl BleReader {
        /// 取下一个**完整帧**（内部消费 notify 分片；窗口内没有分片返回 `Ok(None)`）。
        ///
        /// 超时不让调用方永久阻塞：外层据此周期性 `gc()` 回收半截消息。
        pub async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String> {
            loop {
                let next = tokio::time::timeout(wait, self.notifications.next()).await;
                let notification = match next {
                    Err(_) => return Ok(None), // 窗口内没有分片
                    Ok(None) => return Err("通知流已结束（连接可能已断开）".to_string()),
                    Ok(Some(n)) => n,
                };
                // 过滤非本特征的通知（同一连接上可能还有别的订阅）
                if notification.uuid != self.tx_uuid {
                    self.seen_other_uuid += 1;
                    continue;
                }
                self.seen_notifications += 1;
                self.seen_bytes += notification.value.len();
                match self
                    .reassembler
                    .push(&notification.value, crate::db::now_ms())
                {
                    PushOutcome::Complete(payload) => return Ok(Some(payload)),
                    PushOutcome::Incomplete => continue,
                    // 坏片：记下来（原因留给上层读循环打日志），继续等下一片 ——
                    // 绝不因为一个畸形分片拆掉整条链路。
                    PushOutcome::Dropped(reason) => {
                        self.dropped += 1;
                        self.last_drop_reason = reason;
                        continue;
                    }
                }
            }
        }

        /// 回收超时的半截消息（断连/对端消失后调用）。
        pub fn gc(&mut self) -> usize {
            self.reassembler.gc(crate::db::now_ms())
        }

        /// 读到现在的分片统计：`(本特征通知数, 字节数, 非本特征通知数)`。
        pub fn stats(&self) -> (u64, usize, u64) {
            (
                self.seen_notifications,
                self.seen_bytes,
                self.seen_other_uuid,
            )
        }

        /// 被丢弃的分片：`(累计条数, 最近一次原因)`（诊断用，见 `dropped` 字段的注释）。
        pub fn drop_stats(&self) -> (u64, &'static str) {
            (self.dropped, self.last_drop_reason)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::transport::ble_framing::{BleReassembler, BLE_DEFAULT_MTU};

        /// MTU 换算：正常值直接用；异常值退回默认（**绝不能返回 0**，否则什么都发不出去）。
        #[test]
        fn payload_mtu_handles_normal_and_bogus_values() {
            assert_eq!(payload_mtu(23), 20, "默认 MTU 23 ⇒ 20 字节载荷");
            assert_eq!(payload_mtu(185), 182, "常见协商值");
            // ⚠️ 大 MTU 必须封顶到 AOSP 的 `GATT_MAX_ATTR_LEN = 512`（**与协商 MTU 无关**）。
            // 安卓上 btleplug 的 `mtu()` 返回的是**请求值 517**，517-3=514 > 512 会让
            // `writeCharacteristic` 直接抛 IllegalArgumentException ⇒
            // 单分片帧正常、**多分片帧永远发不出去**（4.18.8 的真机缺陷，4.18.9 才修）。
            // 这条断言把边界钉死，避免有人"顺手"把封顶去掉。
            assert_eq!(payload_mtu(517), 512, "5.0 大 MTU 封顶到 AOSP 上限");
            assert_eq!(payload_mtu(515), 512, "恰好落在上限上");
            assert_eq!(payload_mtu(514), 511, "上限之下一字节不封顶");
            assert_eq!(payload_mtu(513), 510, "上限之下不封顶");
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

/// 蓝牙通道的**状态视图**。接口占位：`available` 取决于是否编译了 BLE 后端
/// （feature `bluetooth`），驱动实现前 `start()` 一律返回明确错误，上层据此继续走局域网。
///
/// ⚠️ 真正的 BLE 运行时在 `network/ble.rs`（扫描/连接/握手/登记链路）+ 本目录下的
/// `bluetooth_peripheral*` / `ble_android` 平台实现；这个占位只服务"未编译 BLE 后端"的
/// 默认构建，让状态页能给出明确答案而不是含糊的"已关闭"。
#[derive(Default)]
pub struct BluetoothTransport {
    running: bool,
}

impl BluetoothTransport {
    pub fn available(&self) -> bool {
        // 编译了 BLE 后端就"可能可用"；真正的探测（有没有适配器 / 用户是否授权）
        // 只能在异步的 `start()` 里做 —— 这个接口是同步的，不能在这里 await。
        // 未编译 feature 时恒 false（默认构建即此分支）。
        cfg!(feature = "bluetooth")
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn peer_count(&self) -> usize {
        0
    }

    pub async fn start(&mut self) -> Result<(), String> {
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

    pub async fn stop(&mut self) -> Result<(), String> {
        self.running = false;
        Ok(())
    }
}
