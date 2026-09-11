//! BLE **peripheral（GATT server / 外设角色）** —— 目前只在 macOS 上实现（ADR-0015 §7）。
//!
//! ## 为什么必须有这一半
//! `btleplug` 只能做 **central**（见 ADR-0015 §3.1）：它能主动扫、主动连，
//! 但**不能**被别人连上。于是只做 central 的 Mac 无法被手机发现，
//! 「手机与电脑不在同一个 Wi-Fi」这个需求就落不了地。BLE 链路天然要求一侧当外设，
//! 本项目先让 **Mac 当外设**，于是：
//!
//! ```text
//!   手机（Android/iOS，central）──BLE──▶ Mac（peripheral）──LAN──▶ Windows
//! ```
//!
//! 手机只要装上 App 打开蓝牙开关就能连上 Mac（手机侧零新增原生代码，
//! 用的还是 `btleplug` 的 central 路径），Mac 再把消息中继给同局域网的 Windows。
//!
//! ## 线格式与 central 侧**逐字节相同**
//! 广播里只放服务 UUID（见 [`advertisement_payload_bytes`]），
//! 特征、分片（[`crate::transport::ble_framing`]）、Hello 握手、验签、去重判据
//! 全部复用 central 侧那一套 —— 也就是说**没有任何新的线上协议**，
//! 手机当 central、Mac 当 peripheral 时才可能互通。
//!
//! ## 线程模型（重要）
//! CoreBluetooth 的回调派发到 `initWithDelegate:queue:` 给的队列（我们给 nil ⇒ 主队列），
//! 而 `updateValue:...`（发通知）由 tokio 的写任务调用。objc2 **不会**自动为框架类实现
//! `Send`/`Sync`（它保守地不假设框架线程安全），我们因此显式断言 [`SendObj`]：
//! Apple 明确 peripheral manager 的方法可从任意线程调用、回调只在给定队列上串行派发，
//! 且本模块内不会并发调用同一对象的同一方法。
//! 回调里只做「拷字节 + 查表 + 发通道」，**绝不做重活**，因此不会卡主线程。
#![cfg(all(feature = "bluetooth", target_os = "macos"))]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBAdvertisementDataServiceUUIDsKey, CBATTError, CBATTRequest, CBAttributePermissions,
    CBCharacteristic, CBCharacteristicProperties, CBCentral, CBMutableCharacteristic,
    CBMutableService, CBManagerState, CBPeripheralManager, CBPeripheralManagerDelegate, CBUUID,
};
use objc2_foundation::{NSArray, NSDictionary, NSObject, NSObjectProtocol, NSString};
use tokio::sync::{mpsc, oneshot, watch};

use crate::transport::ble_framing::{self, BleReassembler, PushOutcome};
use crate::transport::bluetooth::{CHAR_RX_UUID, CHAR_TX_UUID, SERVICE_UUID};

/// 传统（legacy）广播包的净荷上限：31 字节 —— 蓝牙规范写死的，超了整个广播就发不出去
/// （macOS 会在 `peripheralManagerDidStartAdvertising:error:` 里报错，现象是"没人能发现我们"）。
pub const LEGACY_ADV_PAYLOAD_LIMIT: usize = 31;

/// 128 位服务 UUID 在广播里占的字节数：2 字节（长度+类型）+ 16 字节。
pub const ADV_UUID_BYTES: usize = 18;

/// 一轮通知最多重试多久（对端订阅队列满 ⇒ `updateValue` 返回 false，要等系统通知我们"有空位了"）。
const WRITE_DEADLINE: Duration = Duration::from_secs(8);
/// 等"系统说队列有空位"的单次时长。
const WRITE_READY_WAIT: Duration = Duration::from_millis(500);
/// 等对端订阅（central 可能还没订阅完，我们就开始发 Hello）的上限。
const SUBSCRIBE_WAIT: Duration = Duration::from_secs(5);
/// 等 CoreBluetooth 上报状态的上限（超过就认为"至少不明确失败"，仅记日志）。
pub const STATE_WAIT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// 纯函数（可单测，不碰无线电）
// ---------------------------------------------------------------------------

/// 广播净荷字节数：`2 + 16` / 个服务 UUID，本地名 `2 + len`。
///
/// 单独抽出来是因为它是**硬约束**（31 字节）而不是风格问题：
/// 我们**故意不放本地名** —— 设备名在广播里没有任何用（身份一律由 Hello 验签确定，
/// 架构原则 P-A01「BLE 地址不是身份」），却要吃掉宝贵的广播预算，
/// 128 位 UUID + 名字一旦超标，系统会直接拒绝开始广播。
pub fn advertisement_payload_bytes(service_uuids: usize, local_name: Option<&str>) -> usize {
    let uuids = service_uuids * ADV_UUID_BYTES;
    let name = local_name.map(|n| 2 + n.len()).unwrap_or(0);
    uuids + name
}

/// 对端 central 的 `maximumUpdateValueLength` ⇒ 我们一次通知能塞多少字节。
///
/// Apple 文档：该值就是"一次通知/指示里 central 能收的最大字节数"（即 ATT 有效载荷），
/// 所以**不再减 3**（减 3 的是 MTU 换算，见 `transport::bluetooth::driver::payload_mtu`）。
/// 异常值（0 / 装不下分片头 / 超出我们对端的接收上限）一律退回默认 20 字节 ——
/// **绝不能返回 0**，否则什么都发不出去，链路会静默假死。
pub fn central_payload_mtu(max_update_value_length: usize) -> usize {
    const DEFAULT: usize = 20; // ATT 默认 MTU 23 - 3
    const MAX: usize = 512; // 与 central 侧的大 MTU 同量级
    let min = ble_framing::BLE_CHUNK_HEADER_LEN + 1; // 至少装得下"分片头 + 1 字节"
    if max_update_value_length < min {
        DEFAULT
    } else {
        max_update_value_length.min(MAX)
    }
}

/// 蓝牙状态的中文说明。
///
/// 为什么要有它：`CBManagerState` 只有数字（4/5…），日志里出现"状态码 4"对用户毫无意义；
/// 而"蓝牙关着"与"没授权"要给的**处理建议完全不同**（前者去开蓝牙，后者去隐私设置）。
pub fn state_label(state: CBManagerState) -> &'static str {
    if state == CBManagerState::PoweredOn {
        "已开启"
    } else if state == CBManagerState::PoweredOff {
        "蓝牙已关闭"
    } else if state == CBManagerState::Unauthorized {
        "未授权（去「系统设置 → 隐私与安全性 → 蓝牙」允许本应用）"
    } else if state == CBManagerState::Unsupported {
        "本机不支持低功耗蓝牙"
    } else if state == CBManagerState::Resetting {
        "系统蓝牙服务正在重置"
    } else {
        "状态未知"
    }
}

/// 离开"已开启"之后，需要**主动摘掉**的 central 列表（纯函数，便于单测）。
///
/// 为什么必须主动摘：系统蓝牙关闭 / 权限被撤 / 服务重置时，CoreBluetooth 会清空本地
/// GATT 数据库并断开所有 central，但它**不会**回调 `didUnsubscribeFromCharacteristic:`
/// ⇒ 我们手里那份"谁订阅了我"的集合会变成**陈旧状态**：`is_subscribed` 仍返回 true，
/// 于是写任务会一直等到 `updateValue` 失败（最长 8s）才收尾，而用户看不到任何日志。
/// 因此：只要状态不是 PoweredOn，就把之前记住的订阅**全部**当成失效。
pub fn detach_targets(state: CBManagerState, subscribed: &HashSet<String>) -> Vec<String> {
    if state == CBManagerState::PoweredOn {
        return Vec::new();
    }
    let mut out: Vec<String> = subscribed.iter().cloned().collect();
    out.sort(); // 稳定顺序：日志与单测都好读
    out
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 跨线程句柄
// ---------------------------------------------------------------------------

/// 显式的 `Send + Sync` 断言（理由见模块头「线程模型」）。
struct SendObj<T: Message>(Retained<T>);

// SAFETY: CoreBluetooth 的 CBPeripheralManager / CBMutableCharacteristic 都是线程安全的
// 框架对象：Apple 文档说明 manager 的方法可从任意线程调用，回调串行派发到构造时给的队列；
// 我们只调用 `updateValue:forCharacteristic:onSubscribedCentrals:` 这一个方法，
// 且从不在多个任务里并发调用它（写循环每条链路一个，`Retained` 的引用计数本身是原子的）。
unsafe impl<T: Message> Send for SendObj<T> {}
unsafe impl<T: Message> Sync for SendObj<T> {}

impl<T: Message> Clone for SendObj<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// ---------------------------------------------------------------------------
// 事件与写句柄
// ---------------------------------------------------------------------------

/// peripheral 侧从对端收到的**完整帧**（已按 `ble_framing` 重组）。
#[derive(Debug)]
pub enum PeripheralEvent {
    /// 收到一条完整帧。`central` 是 CoreBluetooth 的 central 标识（≈ 我们的"端点地址"）。
    Frame { central: String, bytes: Vec<u8> },
    /// 对端取消订阅（CoreBluetooth 外设角色**没有**"central 断开"回调，这是唯一可靠的信号）。
    ///
    /// 另外：**系统蓝牙被关掉 / 权限被撤**时 CoreBluetooth 也不会回调取消订阅，
    /// 我们会主动为每个已订阅的 central 各发一条 —— 否则链路要等到写入失败（最长 8s）才被拆，
    /// 而且用户完全看不出发生了什么。
    Unlinked { central: String },
    /// 诊断信息（走 `logger.info`）：状态变化、恢复广播等。
    Notice(String),
    /// 需要用户知道的问题（走 `logger.warn`）：广播启动失败、蓝牙不可用等。
    Warning(String),
}

/// 共享的 per-central 状态（订阅集合 + 协商到的通知载荷上限）。
#[derive(Default)]
struct CentralState {
    subscribed: HashSet<String>,
    mtu: HashMap<String, usize>,
}

/// 往某个 central 发帧的句柄（`Clone`，可给每条链路的写任务各持一份）。
#[derive(Clone)]
pub struct PeripheralWriter {
    manager: SendObj<CBPeripheralManager>,
    tx: SendObj<CBMutableCharacteristic>,
    state: Arc<Mutex<CentralState>>,
    /// 订阅/就绪信号：订阅数变化或"队列又有空位"时 +1，避免忙等。
    signal: watch::Receiver<u64>,
    msg_id: Arc<AtomicU16>,
}

impl PeripheralWriter {
    /// 该 central 一次通知能收多少字节（未知 ⇒ 20）。
    pub fn payload_mtu(&self, central: &str) -> usize {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        central_payload_mtu(st.mtu.get(central).copied().unwrap_or(20))
    }

    /// 对端是否仍订阅着我们的 TX 特征。
    pub fn is_subscribed(&self, central: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribed
            .contains(central)
    }

    /// 发一条完整帧：按 MTU 分片后逐片通知；队列满就等系统的"ready"信号再重试。
    pub async fn send_frame(&self, central: &str, payload: &[u8]) -> Result<usize, String> {
        let mut signal = self.signal.clone();
        let mtu = self.payload_mtu(central);
        let msg_id = self.msg_id.fetch_add(1, Ordering::Relaxed);
        let chunks = ble_framing::fragment(payload, mtu, msg_id)
            .ok_or_else(|| format!("帧无法分片（过大或 MTU 非法：len={} mtu={mtu}）", payload.len()))?;

        let deadline = tokio::time::Instant::now() + WRITE_DEADLINE;
        for chunk in &chunks {
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("等待对端接收窗口超时（central={central}）"));
                }
                // 还没订阅（Hello 回程常常赶在订阅完成之前）⇒ 等订阅信号，**别直接失败**
                if !self.is_subscribed(central) {
                    wait_or_nap(&mut signal, SUBSCRIBE_WAIT).await;
                    continue;
                }
                // 注意作用域：`Retained<NSData>` **不是** Send，必须在任何 await 之前析构，
                // 否则会把整个写循环的 future 染成 !Send（tokio::spawn 直接编不过）。
                let ok = {
                    let data = objc2_foundation::NSData::with_bytes(chunk);
                    // SAFETY: 特征对象在 `PeripheralServer` 生命周期内一直有效；
                    // `centrals=None` = 通知所有已订阅该特征的 central。
                    unsafe {
                        self.manager.0.updateValue_forCharacteristic_onSubscribedCentrals(
                            &data, &self.tx.0, None,
                        )
                    }
                };
                if ok {
                    break;
                }
                // 发送队列满：等 peripheralManagerIsReadyToUpdateSubscribers: 唤醒（或超时重试）
                wait_or_nap(&mut signal, WRITE_READY_WAIT).await;
            }
        }
        Ok(chunks.len())
    }
}

/// 等一个 watch 信号；**通道已经关掉时绝不能忙等**（`changed()` 会立刻返回 Err，
/// 不睡一下就成了死循环，把核跑满还发不出去任何东西）。
async fn wait_or_nap(signal: &mut watch::Receiver<u64>, max: Duration) {
    match tokio::time::timeout(max, signal.changed()).await {
        Ok(Ok(())) => {}
        _ => tokio::time::sleep(Duration::from_millis(50)).await,
    }
}

// ---------------------------------------------------------------------------
// 服务端
// ---------------------------------------------------------------------------

/// 已启动的外设角色。持有它 = 保持广播；`stop()` 或 drop 都会停止广播。
pub struct PeripheralServer {
    /// 对端发来的帧（`recv()` 返回 `None` 表示驱动已退出）。
    pub events: mpsc::UnboundedReceiver<PeripheralEvent>,
    /// 往对端发帧的句柄。
    pub writer: PeripheralWriter,
    manager: SendObj<CBPeripheralManager>,
    /// CoreBluetooth 的 delegate 是**弱引用**，我们必须自己持有，否则回调直接没了。
    _delegate: Retained<Delegate>,
}

impl PeripheralServer {
    /// 停止广播并撤掉 GATT 数据库（幂等；之后不可再 send）。
    pub fn stop(&self) {
        unsafe {
            self.manager.0.stopAdvertising();
            self.manager.0.removeAllServices();
        }
    }
}

/// 走一次 `CBUUID::UUIDWithString`（大写小写都收，返回 nil 时我们当成编程错误 panic 掉，
/// 因为这些 UUID 是本项目源码里的常量，解析失败只可能是被改坏了）。
fn uuid(s: &str) -> Retained<CBUUID> {
    let ns = NSString::from_str(s);
    // SAFETY: 纯构造函数，参数是本文件里的常量字符串（非 nil、格式合法）。
    unsafe { CBUUID::UUIDWithString(&ns) }
}

/// 外设角色的启动结果：服务端 + "状态回调结论"的接收端。
///
/// 状态结论**故意**不在 [`start`] 里等：`Retained<CBUUID>` 之类的 CoreBluetooth 对象
/// 不是 `Send`，只要它们在 await 期间还活着，整个 future 就不是 `Send`，
/// 会一路把 `#[tauri::command]` 的返回值也染成非 `Send`（编译期就会炸）。
/// 于是 [`start`] 全程同步、一个 await 都没有，等待交给调用方，
/// 而等待时它手里只有 `PeripheralServer`（`Send`）和一个 oneshot。
pub struct PeripheralStart {
    pub server: PeripheralServer,
    /// `Ok(())` = 蓝牙已就绪（服务已加、广播已开）；`Err` = 明确的原因（未授权 / 蓝牙关着 / 广播失败）。
    pub state: oneshot::Receiver<Result<(), String>>,
}

/// 启动外设角色（同步，不阻塞）。成功返回的 `PeripheralServer` 必须被持有（drop 即停播）。
///
/// 失败原因会**明确**带出来（蓝牙未开启 / 未授权 / 不支持），
/// 让上层能把它记成日志而不是静默失效 —— 手机扫不到我们时这是唯一的线索。
pub fn start() -> Result<PeripheralStart, String> {
    let svc_uuid = uuid(SERVICE_UUID);
    let rx_uuid = uuid(CHAR_RX_UUID);
    let tx_uuid = uuid(CHAR_TX_UUID);

    // ---- 特征：RX = 对端写给我们；TX = 我们通知对端 ----
    let rx_char = unsafe {
        CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            &rx_uuid,
            CBCharacteristicProperties::Write | CBCharacteristicProperties::WriteWithoutResponse,
            None,
            CBAttributePermissions::Writeable,
        )
    };
    let tx_char = unsafe {
        CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            &tx_uuid,
            CBCharacteristicProperties::Notify,
            None,
            CBAttributePermissions::Readable,
        )
    };
    let service = unsafe {
        CBMutableService::initWithType_primary(CBMutableService::alloc(), &svc_uuid, true)
    };
    {
        // 特征列表要的是 `NSArray<CBCharacteristic>`，我们是其子类 ⇒ 安全的向上转型
        let rx_super: Retained<CBCharacteristic> =
            unsafe { Retained::cast_unchecked(rx_char.clone()) };
        let tx_super: Retained<CBCharacteristic> =
            unsafe { Retained::cast_unchecked(tx_char.clone()) };
        let chars = NSArray::from_retained_slice(&[rx_super, tx_super]);
        unsafe { service.setCharacteristics(Some(&chars)) };
    }

    let (event_tx, event_rx) = mpsc::unbounded_channel::<PeripheralEvent>();
    let (signal_tx, signal_rx) = watch::channel(0u64);
    let (state_tx, state_rx) = oneshot::channel::<Result<(), String>>();
    let state = Arc::new(Mutex::new(CentralState::default()));

    let delegate = Delegate::new(
        event_tx,
        signal_tx,
        state.clone(),
        SendObj(service),
        Mutex::new(Some(state_tx)),
    );

    // queue = nil ⇒ 回调走主队列（AppKit 的 runloop 一直在跑）。回调里只做轻量工作。
    let manager = unsafe {
        CBPeripheralManager::initWithDelegate_queue(
            CBPeripheralManager::alloc(),
            Some(ProtocolObject::from_ref(&*delegate)),
            None,
        )
    };

    Ok(PeripheralStart {
        server: PeripheralServer {
            events: event_rx,
            writer: PeripheralWriter {
                manager: SendObj(manager.clone()),
                tx: SendObj(tx_char),
                state,
                signal: signal_rx,
                msg_id: Arc::new(AtomicU16::new(1)),
            },
            manager: SendObj(manager),
            _delegate: delegate,
        },
        state: state_rx,
    })
}

// ---------------------------------------------------------------------------
// delegate
// ---------------------------------------------------------------------------

/// delegate 的实例变量。
///
/// 全部是 `Send + Sync` 的普通数据（通道 + 锁），**唯独** CoreBluetooth 对象走 [`SendObj`]；
/// 回调只做"拷字节 + 查表 + 发通道"，重活（验签、写库、加解密）都在 tokio 侧。
struct DelegateIvars {
    events: mpsc::UnboundedSender<PeripheralEvent>,
    signal: watch::Sender<u64>,
    state: Arc<Mutex<CentralState>>,
    service: SendObj<CBMutableService>,
    /// 第一次状态回调把结论送回 `start()`（`take()` ⇒ 只回报一次）。
    init_state: Mutex<Option<oneshot::Sender<Result<(), String>>>>,
    reassemblers: Mutex<HashMap<String, BleReassembler>>,
}

define_class!(
    // SAFETY: NSObject 没有子类化要求；本类不实现 Drop。
    #[unsafe(super(NSObject))]
    #[name = "GosslanBlePeripheralDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl CBPeripheralManagerDelegate for Delegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, peripheral: &CBPeripheralManager) {
            let state = unsafe { peripheral.state() };
            if state == CBManagerState::PoweredOn {
                // 每次回到 PoweredOn 都要**重新** addService + startAdvertising：
                // CoreBluetooth 在离开 PoweredOn 时会清空本地 GATT 数据库，
                // 重新打开蓝牙后不重新发布就永远不会再有人能连上我们。
                self.add_service_and_advertise(peripheral);
                self.notice(format!("蓝牙{}，本机已在广播（等待对端连入）", state_label(state)));
                if let Some(tx) = self.ivars().init_state.lock().unwrap_or_else(|e| e.into_inner()).take()
                {
                    let _ = tx.send(Ok(()));
                }
                return;
            }

            // ---- 离开"已开启"：把订阅状态与半截消息全部作废，并**主动**通知上层拆链路 ----
            // （CoreBluetooth 不会补发 didUnsubscribe，见 `detach_targets` 的注释）
            let victims = {
                let mut st = self.ivars().state.lock().unwrap_or_else(|e| e.into_inner());
                let targets = detach_targets(state, &st.subscribed);
                st.subscribed.clear();
                st.mtu.clear();
                targets
            };
            self.ivars()
                .reassemblers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            for central in &victims {
                let _ = self.ivars().events.send(PeripheralEvent::Unlinked {
                    central: central.clone(),
                });
            }
            let _ = self.ivars().signal.send(1); // 唤醒可能在等订阅的写任务，让它们立刻失败收尾
            self.warn(format!(
                "蓝牙不可用（{}）：已断开 {} 条 BLE 链路，等待蓝牙恢复",
                state_label(state),
                victims.len()
            ));
            if let Some(tx) = self.ivars().init_state.lock().unwrap_or_else(|e| e.into_inner()).take()
            {
                let _ = tx.send(Err(format!(
                    "蓝牙不可用（{}）：请确认已开启蓝牙并在系统设置里允许本应用使用蓝牙",
                    state_label(state)
                )));
            }
        }

        /// 对端订阅了 TX 特征：记下它、记下它能收多大，并唤醒可能在等订阅的写任务。
        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn did_subscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            let id = unsafe { central.identifier().UUIDString().to_string() };
            let mtu = unsafe { central.maximumUpdateValueLength() };
            {
                let mut st = self.ivars().state.lock().unwrap_or_else(|e| e.into_inner());
                st.subscribed.insert(id.clone());
                st.mtu.insert(id, mtu);
            }
            let _ = self.ivars().signal.send(1); // 订阅数变化 ⇒ 唤醒等订阅的写任务
        }

        /// 对端取消订阅 —— 外设角色能拿到的**唯一**"断开"信号。
        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            let id = unsafe { central.identifier().UUIDString().to_string() };
            {
                let mut st = self.ivars().state.lock().unwrap_or_else(|e| e.into_inner());
                st.subscribed.remove(&id);
                st.mtu.remove(&id);
            }
            // 两次取锁分开，不在持有 `state` 时去拿 `reassemblers`（锁序越简单越不会出事）
            self.ivars()
                .reassemblers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            let _ = self.ivars().events.send(PeripheralEvent::Unlinked {
                central: id.clone(),
            });
            let _ = self.ivars().signal.send(1);
        }

        /// 对端写 RX 特征：**必须逐个 respond**（否则对端会一直等），再把分片拼成帧。
        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            for req in requests.iter() {
                let id = unsafe { req.central().identifier().UUIDString().to_string() };
                match unsafe { req.value() } {
                    Some(data) => {
                        let chunk = data.to_vec();
                        unsafe {
                            peripheral.respondToRequest_withResult(&req, CBATTError::Success)
                        };
                        self.push_chunk(&id, &chunk);
                    }
                    // 读请求（我们没开 Read 权限）：明确拒绝，别让对端挂在那儿
                    None => unsafe {
                        peripheral
                            .respondToRequest_withResult(&req, CBATTError::ReadNotPermitted)
                    },
                }
            }
        }

        /// 发送队列又有空位了：唤醒写任务重试。
        #[unsafe(method(peripheralManagerIsReadyToUpdateSubscribers:))]
        fn ready_to_update(&self, _peripheral: &CBPeripheralManager) {
            let _ = self.ivars().signal.send(1);
        }

        /// 广播起没起来（起不来时最常见的原因就是净荷超 31 字节）。
        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn did_start_advertising(
            &self,
            _peripheral: &CBPeripheralManager,
            error: Option<&objc2_foundation::NSError>,
        ) {
            if let Some(err) = error {
                // ⚠️ 必须走 `events` 报给上层日志，**不能**只在 `init_state` 通道上尽力报一次：
                // 那个通道只在首次状态回调时还开着，之后（例如用户关掉蓝牙再打开、
                // 我们重新广播时失败）错误会**被静默丢弃** —— 现象就是"蓝牙开着却没人能发现我们"，
                // 而日志里一个字都没有。
                self.warn(format!(
                    "蓝牙广播启动失败：{err}（对端将无法发现本机；可尝试关闭再打开蓝牙开关）"
                ));
                if let Some(tx) = self
                    .ivars()
                    .init_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                {
                    let _ = tx.send(Err(format!("蓝牙广播启动失败：{err}")));
                }
            }
        }
    }
);

impl Delegate {
    /// 只有拿到 `peripheralManagerDidUpdateState:`（PoweredOn）之后，
    /// `addService:` / `startAdvertising:` 才是合法调用 —— 否则系统直接忽略。
    fn add_service_and_advertise(&self, manager: &CBPeripheralManager) {
        // 广播里只放一个 128 位服务 UUID：这是**平台硬约束**（31 字节），
        // 一旦有人"顺手"往广播里塞本地名/厂商数据，广播会直接失败（现象是"谁都发现不了我们"）。
        debug_assert!(
            advertisement_payload_bytes(1, None) <= LEGACY_ADV_PAYLOAD_LIMIT,
            "广播净荷超出蓝牙规范上限，广播会启动失败"
        );
        let adv = {
            // 广播数据里**只**放服务 UUID（本地名不放，见 `advertisement_payload_bytes`）
            let uuids = NSArray::from_retained_slice(&[uuid(SERVICE_UUID)]);
            let value: Retained<AnyObject> = unsafe { Retained::cast_unchecked(uuids) };
            // SAFETY: `CBAdvertisementDataServiceUUIDsKey` 是框架提供的常量（非空、生命周期 'static）；
            // key/value 两个切片长度相同（各 1 个）。
            unsafe {
                NSDictionary::from_slices(&[CBAdvertisementDataServiceUUIDsKey], &[&*value])
            }
        };
        // SAFETY: 已确认状态为 PoweredOn（调用点保证）；`service`/`adv` 都是本对象持有的有效对象。
        unsafe {
            manager.addService(&self.ivars().service.0);
            manager.startAdvertising(Some(&adv));
        }
    }

    fn new(
        events: mpsc::UnboundedSender<PeripheralEvent>,
        signal: watch::Sender<u64>,
        state: Arc<Mutex<CentralState>>,
        service: SendObj<CBMutableService>,
        init_state: Mutex<Option<oneshot::Sender<Result<(), String>>>>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars {
            events,
            signal,
            state,
            service,
            init_state,
            reassemblers: Mutex::new(HashMap::new()),
        });
        unsafe { objc2::msg_send![super(this), init] }
    }

    /// 诊断信息（上层记 info）。
    fn notice(&self, text: String) {
        let _ = self.ivars().events.send(PeripheralEvent::Notice(text));
    }

    /// 需要用户知道的问题（上层记 warn）。
    fn warn(&self, text: String) {
        let _ = self.ivars().events.send(PeripheralEvent::Warning(text));
    }

    /// 分片 ⇒ 完整帧 ⇒ 事件。半截消息由 `BleReassembler` 自己带 TTL 回收。
    ///
    /// 坏片（重复 / 越界 / 超限）只会**丢这一片**，不会断链路 —— 与 central 侧同口径：
    /// 宁可丢一条消息，也不要因为一个畸形分片把整条链路拆掉。
    fn push_chunk(&self, central: &str, chunk: &[u8]) {
        let now = now_ms();
        // 一次性拿锁：push + 顺手 gc（gc 只扫本表，量级是"同时在途的消息数"，很便宜）
        let outcome = {
            let mut map = self
                .ivars()
                .reassemblers
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let out = map.entry(central.to_string()).or_default().push(chunk, now);
            for re in map.values_mut() {
                let _ = re.gc(now);
            }
            out
        };
        if let PushOutcome::Complete(frame) = outcome {
            let _ = self.ivars().events.send(PeripheralEvent::Frame {
                central: central.to_string(),
                bytes: frame,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 广播净荷是**硬约束**：128 位服务 UUID 18 字节，放得下；
    /// 但绝不能再塞本地名之类的东西（设备名要 2+N 字节，很容易就超 31）。
    #[test]
    fn advertisement_fits_legacy_budget_and_deliberately_omits_local_name() {
        assert_eq!(advertisement_payload_bytes(1, None), ADV_UUID_BYTES);
        assert_eq!(advertisement_payload_bytes(2, None), 2 * ADV_UUID_BYTES);
        assert!(
            advertisement_payload_bytes(1, None) <= LEGACY_ADV_PAYLOAD_LIMIT,
            "一个 128 位服务 UUID（{ADV_UUID_BYTES} 字节）必须放得下 {LEGACY_ADV_PAYLOAD_LIMIT} 字节的广播包"
        );
        // 说明为什么"顺手把设备名加上"是不行的：名字长一点就爆预算
        assert_eq!(advertisement_payload_bytes(1, Some("Gosslan")), 18 + 2 + 7);
        assert!(
            advertisement_payload_bytes(2, Some("Gosslan")) > LEGACY_ADV_PAYLOAD_LIMIT,
            "两个 UUID + 名字已经超标 —— 这正是我们不放名字的原因"
        );
    }

    /// 对端给的 `maximumUpdateValueLength` 是"一次通知能收多少字节"，
    /// 异常值必须退回默认（返回 0 会让链路静默假死）。
    #[test]
    fn central_mtu_clamps_and_never_returns_zero() {
        assert_eq!(central_payload_mtu(20), 20, "默认 ATT 载荷");
        assert_eq!(central_payload_mtu(182), 182, "常见协商值");
        assert_eq!(central_payload_mtu(512), 512, "上限");
        assert_eq!(central_payload_mtu(4096), 512, "超上限要收敛");
        for bogus in [0usize, 1, 2, 3, 4, 5, 6] {
            assert_eq!(
                central_payload_mtu(bogus),
                20,
                "max_update_value_length={bogus} 应退回默认而不是返回 0"
            );
            assert!(
                central_payload_mtu(bogus) > 0,
                "绝不能返回 0：分片会全部失败"
            );
        }
    }

    /// 状态文案必须把"关着"和"没授权"分开 —— 两者的处理建议完全不同
    /// （一个去开蓝牙，一个去隐私设置），日志里只说"状态码 4"等于什么都没说。
    #[test]
    fn state_label_tells_the_user_what_to_do() {
        assert_eq!(state_label(CBManagerState::PoweredOn), "已开启");
        assert_eq!(state_label(CBManagerState::PoweredOff), "蓝牙已关闭");
        let unauthorized = state_label(CBManagerState::Unauthorized);
        assert!(
            unauthorized.contains("系统设置"),
            "未授权必须给出处理建议（去系统设置），实际：{unauthorized}"
        );
        assert_eq!(state_label(CBManagerState::Unsupported), "本机不支持低功耗蓝牙");
        assert_eq!(state_label(CBManagerState::Resetting), "系统蓝牙服务正在重置");
        assert_eq!(state_label(CBManagerState::Unknown), "状态未知");
    }

    /// 只要离开 PoweredOn，**所有**已订阅的 central 都必须被摘掉。
    ///
    /// 这条守着一个真实的静默故障：CoreBluetooth 在蓝牙被关/权限被撤时不会回调
    /// `didUnsubscribeFromCharacteristic:`，于是"谁订阅了我"会一直是旧数据 ——
    /// 写任务要等 `updateValue` 失败（最长 8s）才收尾，用户日志里也一片空白。
    #[test]
    fn leaving_powered_on_detaches_every_subscribed_central() {
        let mut subscribed = HashSet::new();
        subscribed.insert("central-a".to_string());
        subscribed.insert("central-b".to_string());

        assert!(
            detach_targets(CBManagerState::PoweredOn, &subscribed).is_empty(),
            "已经开启时不该摘任何链路"
        );
        for state in [
            CBManagerState::PoweredOff,
            CBManagerState::Unauthorized,
            CBManagerState::Unsupported,
            CBManagerState::Resetting,
            CBManagerState::Unknown,
        ] {
            let mut got = detach_targets(state, &subscribed);
            got.sort();
            assert_eq!(
                got,
                vec!["central-a".to_string(), "central-b".to_string()],
                "状态 {:?} 时必须把订阅全部当成失效",
                state.0
            );
        }
        assert!(
            detach_targets(CBManagerState::PoweredOff, &HashSet::new()).is_empty(),
            "从来没订阅过就没什么可摘的"
        );
    }

    /// 用 clamp 出来的 MTU 分片，必须能被**对端同一套**重组器逐字节还原
    /// （这就是"peripheral 与 central 线格式一致"的可测部分）。
    #[test]
    fn fragments_made_with_this_mtu_round_trip_through_the_shared_reassembler() {
        let payload: Vec<u8> = (0..600u32).map(|i| (i % 251) as u8).collect();
        for raw in [10usize, 20, 182, 4096] {
            let mtu = central_payload_mtu(raw);
            let chunks = ble_framing::fragment(&payload, mtu, 7).expect("应能分片");
            let mut re = BleReassembler::new();
            let mut done = None;
            for c in &chunks {
                if let PushOutcome::Complete(frame) = re.push(c, 0) {
                    done = Some(frame);
                }
            }
            assert_eq!(
                done.as_deref(),
                Some(payload.as_slice()),
                "raw={raw} mtu={mtu} 时应逐字节还原"
            );
        }
    }
}
