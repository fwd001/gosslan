//! Windows BLE **外设角色**（GATT server + 广播）—— WinRT `GattServiceProvider`。
//!
//! ## 为什么需要它（真机 2026-09-13）
//!
//! `btleplug` **只做 central**（ADR-0015 §3.1），所以一个只做 central 的 Windows
//! **从不广播、也不能被连**。真机后果（`docs/notes/windows-ble-diagnosis-2026-09-13.md`）：
//! 手机扫描里永远没有 Windows（「你搜不到我」），而按「大 id 拨、小 id 只接受」的镜像规则，
//! Windows 又因为 `device_id` 更小而不主动拨 ⇒ **两侧都在等对方**。
//!
//! ## 与 macOS / Android 的关系
//!
//! **接口逐字同形**（`start` / `PeripheralServer` / `PeripheralWriter` / `PeripheralEvent`
//! / `STATE_WAIT`），所以 `network/ble.rs` 里的事件循环、握手、路由、读写循环
//! **三个平台共用一份**，只有 `use ... as peripheral` 那一行按平台切换。
//!
//! 与另两个平台的两处**有意不同**：
//! 1. **不需要 bootstrap**：Android 的 Kotlin 类要靠 JNI 的类加载器缓存（JNI 的坑），
//!    macOS 的 CoreBluetooth 对象不是 `Send`。WinRT 的 `GattServiceProvider` 在 Rust 里
//!    直接可用，于是这里没有全局 `OnceLock`/JNI 桥 —— 状态放在 `Arc<Shared>` 里，
//!    由 `PeripheralServer` 持有。
//! 2. **CCCD 由系统处理**：与 macOS 相同，不需要像 Android 那样显式加
//!    `00002902-…` 描述符；订阅情况通过 `GattLocalCharacteristic::SubscribedClients()` 读。
//!
//! ## 三个必须在实现里守住的行为契约（否则真机上"看着成功、其实不通"）
//!
//! - **写请求必须 `Respond()`**：GATT 语义要求每个 write-with-response 都被应答，
//!   不应答的话 central 每次写都要等到超时（ADR-0015 §7.3 第 5 条同款）。
//! - **`NotifyValueForSubscribedClientAsync` 的返回值必须看**：返回非 Success 说明这一片
//!   **没送到**，必须重发/等待 —— 否则表现为"发成功但对面永远拼不出帧"。
//! - **分片之间要节流**：与 Android 侧同一条真机教训（`NOTIFY_CHUNK_INTERVAL`）——
//!   通知连发会被协议栈丢中间片，重组器永远拼不出完整帧。
#![cfg(all(feature = "bluetooth", target_os = "windows"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use windows::core::GUID;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattCommunicationStatus, GattLocalCharacteristic,
    GattLocalCharacteristicParameters, GattServiceProvider,
    GattServiceProviderAdvertisingParameters, GattSubscribedClient, GattWriteOption,
    GattWriteRequest,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter};

use crate::transport::ble_framing::{self, BleReassembler, PushOutcome};
use crate::transport::bluetooth::{CHAR_RX_UUID, CHAR_TX_UUID, SERVICE_UUID};

/// 等广播真正可用的窗口（与 macOS/Android 侧同口径，供 `network/ble.rs` 复用）。
pub const STATE_WAIT: Duration = Duration::from_secs(3);

/// 一片通知发失败后的重试间隔。
const NOTIFY_RETRY_WAIT: Duration = Duration::from_millis(20);
/// 一片通知的重试上限（对端还没订阅 / 通知队列满时等一等，不能忙等也不能无限等）。
const NOTIFY_DEADLINE: Duration = Duration::from_secs(8);
/// **相邻通知之间的最小间隔**（理由与 Android 侧逐字相同：连发会丢中间分片）。
/// Windows 这一侧是"往一个订阅者连续 NotifyValue"，同样会撞上协议栈发送缓冲。
const NOTIFY_CHUNK_INTERVAL: Duration = Duration::from_millis(12);

/// 把一个对端标识成 `GUID`（服务/特征 UUID 常量）。
fn uuid(s: &str) -> GUID {
    GUID::from_u128(
        uuid::Uuid::parse_str(s)
            .expect("BLE UUID 常量必须合法")
            .as_u128(),
    )
}

/// 与 macOS / Android 侧**同构**的事件（`network/ble.rs` 三边共用同一套处理代码）。
#[derive(Debug)]
pub enum PeripheralEvent {
    /// 一条**完整帧**（分片重组在 Rust 侧完成，复用 `ble_framing`）。
    Frame { central: String, bytes: Vec<u8> },
    /// 对端断开。
    ///
    /// 诚实说明：WinRT 的 `GattLocalCharacteristic` **没有"订阅者断开"事件**
    /// （只有 `SubscribedClientsChanged`，它也可能因为"还在连但取消了订阅"而触发），
    /// 所以这里只在**订阅列表里少了一个人**时发 `Unlinked` —— 与 macOS 侧同一条已知限制：
    /// "对端走远/掉电"只能靠写失败或心跳超时收尾。
    Unlinked { central: String },
    /// 诊断信息（上层记 info）。
    Notice(String),
    /// 需要用户知道的问题（上层记 warn）。
    Warning(String),
}

/// 外设侧的共享状态：WinRT 的事件回调在**任意线程**上跑，所以全部状态过 `Arc<Mutex>`。
struct Shared {
    /// 每个对端的分片重组器（带 30s TTL，与 macOS/Android 同一个实现）。
    reassemblers: Mutex<HashMap<String, BleReassembler>>,
    /// 事件通道（回调线程 → 网络层）。
    events: mpsc::UnboundedSender<PeripheralEvent>,
}

impl Shared {
    fn emit(&self, ev: PeripheralEvent) {
        // 接收端还在 ⇒ 非阻塞投递即可（回调线程上绝不能 await）
        if self.events.send(ev).is_err() {
            // 通道关闭 = 网络层已经不听了（通道被关掉）；此时静默是**错**的，
            // 但也没别的地方可报，所以在下一层（`PeripheralServer` 被 drop）体现。
        }
    }

    fn notice(&self, msg: impl Into<String>) {
        self.emit(PeripheralEvent::Notice(msg.into()));
    }

    fn warn(&self, msg: impl Into<String>) {
        self.emit(PeripheralEvent::Warning(msg.into()));
    }
}

/// 往某个对端发帧的句柄。
#[derive(Clone)]
pub struct PeripheralWriter {
    tx_char: GattLocalCharacteristic,
    shared: Arc<Shared>,
}

/// 已启动的外设角色。**必须持有 `server`** —— 它被 drop 就停止广播/撤掉服务。
pub struct PeripheralServer {
    pub events: mpsc::UnboundedReceiver<PeripheralEvent>,
    pub writer: PeripheralWriter,
    /// 持有 GATT server 的生命周期（drop = 停止广播并撤服务）。
    server: GattServiceProvider,
}

impl PeripheralServer {
    /// 停止广播并撤掉 GATT server（幂等）。
    pub fn stop(&self) {
        // 先停广播再撤服务：顺序反了会让还在连的 central 收到一个不完整的数据库
        if let Err(e) = self.server.StopAdvertising() {
            // 这里失败意味着**可能仍在广播** —— 必须留痕，别静默
            self.writer
                .shared
                .warn(format!("停止 BLE 广播失败（可能仍在广播）：{e}"));
        }
        self.writer
            .shared
            .notice("Windows 蓝牙外设已停止（广播已撤）");
    }
}

/// 与 macOS / Android 侧同形的启动结果。
pub struct PeripheralStart {
    pub server: PeripheralServer,
    /// WinRT 的建服务/开广播都在 `start()` 里 await 完成，所以这个 channel 立刻就有结果
    /// （保留这个形状只为让 `network/ble.rs` 三平台同码）。
    pub state: oneshot::Receiver<Result<(), String>>,
}

/// 启动外设角色：建服务 → 建两个特征 → 挂事件 → 开广播。
///
/// **与 macOS / Android 一样是同步函数**：WinRT 的 `CreateAsync` /
/// `CreateCharacteristicAsync` 虽然只有 async 形态，但 `IAsyncOperation` 自带的
/// 阻塞 `join()`（`windows-future` 的 inherent 方法）可以直接等它 —— 建服务只有一次，
/// 阻塞几十毫秒无妨，换来的是三个平台 `start()` 同形（`network/ble.rs` 零差异）。
///
/// 失败就返回 `Err`：上层（`network/ble/peripheral.rs::start_peripheral`）只记 warn 并继续做 central，
/// **绝不影响局域网**。
pub fn start() -> Result<PeripheralStart, String> {
    let (tx, rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        reassemblers: Mutex::new(HashMap::new()),
        events: tx,
    });

    // ---- 1. 建服务（WinRT: GattServiceProvider.CreateAsync）----
    let provider = GattServiceProvider::CreateAsync(uuid(SERVICE_UUID))
        .map_err(|e| format!("创建 GATT 服务失败：{e}"))?
        .join()
        .map_err(|e| format!("创建 GATT 服务失败：{e}"))?
        .ServiceProvider()
        .map_err(|e| format!("创建 GATT 服务失败（无 provider）：{e}"))?;
    let service = provider
        .Service()
        .map_err(|e| format!("取 GATT 服务失败：{e}"))?;

    // ---- 2. 两个特征：RX（central 写我们读）/ TX（我们通知 central）----
    let rx_char = create_characteristic(
        &service,
        CHAR_RX_UUID,
        GattCharacteristicProperties::Write | GattCharacteristicProperties::WriteWithoutResponse,
        "接收（RX）",
    )?;
    let tx_char = create_characteristic(
        &service,
        CHAR_TX_UUID,
        GattCharacteristicProperties::Notify,
        "通知（TX）",
    )?;

    // ---- 3. 挂事件：写请求（收数据）+ 订阅变化（跟踪 central / 断连）----
    install_write_handler(&rx_char, shared.clone())?;
    install_subscription_handler(&tx_char, shared.clone())?;

    // 订阅一次已有的（正常情况下是空的，但别假设）
    sync_subscribers(&tx_char, &shared);

    // ---- 4. 开广播 ----
    // `IsConnectable(true)` 是**必须**的：不可连接的广播会让对端"搜得到、连不上"
    //（Windows 上那正是我们自己在 central 侧踩过的形态）。
    //
    // ⚠️ 真机 2026-09-13 第六轮：**必须显式 `SetIsDiscoverable(true)`**。
    // 上游注释假设"默认即可发现"，但真机结果是 **安卓和 Mac 都收不到 Windows 的广播**
    //（两端日志里从头到尾没有 Windows 的地址），而 Windows 自己以为一切正常。
    // 这类"广播了但没人看得见"在 WinRT 上不会报错，只能靠显式打开 + 查状态发现。
    let adv = GattServiceProviderAdvertisingParameters::new()
        .map_err(|e| format!("构造广播参数失败：{e}"))?;
    adv.SetIsConnectable(true)
        .map_err(|e| format!("设置可连接广播失败：{e}"))?;
    adv.SetIsDiscoverable(true)
        .map_err(|e| format!("设置可被发现广播失败：{e}"))?;
    provider
        .StartAdvertisingWithParameters(&adv)
        .map_err(|e| format!("启动 BLE 广播失败：{e}"))?;

    // **把广播的真实状态查出来**（本轮的教训：`StartAdvertising` 返回 Ok 不等于广播生效）。
    // WinRT 的状态含义（见 `GattServiceProviderAdvertisementStatus`）：
    //   Started(2)                        = 广播正常
    //   StartedWithoutAllAdvertisementData(4) = **广播了，但数据不全**（对端可能认不出服务 UUID）
    //   Aborted(3) / Stopped(1)           = 没在广播
    // 这三个在日志里长得完全不一样，而"对端搜不到"的下一步动作取决于到底是哪个。
    match provider.AdvertisementStatus() {
        Ok(st) => {
            let code = st.0;
            if code == 2 {
                shared.notice(
                    "Windows 蓝牙广播已生效（状态=Started，服务 UUID 已随广播发出）".to_string(),
                );
            } else {
                shared.warn(format!(
                    "Windows 蓝牙广播状态异常：code={code}（2=Started / 3=Aborted / \
                     4=StartedWithoutAllAdvertisementData）—— 对端很可能搜不到本机"
                ));
            }
        }
        Err(e) => shared.warn(format!("读取广播状态失败：{e}")),
    }

    // **持续盯着广播状态**：Windows 会在若干情形下（系统省电、无线电被别的应用抢占、
    // 蓝牙被关闭再打开）**静默 Aborted** —— 而那时我们的日志里只有一条"已启动"，
    // 用户看到的就是"刚才还能搜到、现在搜不到了"。状态一变就留痕。
    {
        let adv_shared = shared.clone();
        let token = provider
            .AdvertisementStatusChanged(&windows::Foundation::TypedEventHandler::<
                GattServiceProvider,
                windows::Devices::Bluetooth::GenericAttributeProfile::GattServiceProviderAdvertisementStatusChangedEventArgs,
            >::new(move |_sender, args| {
                let Ok(args) = args.ok() else { return Ok(()) };
                let code = args.Status().map(|s| s.0).unwrap_or(-1);
                match code {
                    2 => adv_shared.notice("Windows 蓝牙广播状态 → Started（已生效）"),
                    4 => adv_shared.warn(
                        "Windows 蓝牙广播状态 → StartedWithoutAllAdvertisementData\
                         （广播数据不全，对端可能认不出服务 UUID）",
                    ),
                    3 => adv_shared.warn("Windows 蓝牙广播状态 → Aborted（**广播已停，对端搜不到本机**）"),
                    1 => adv_shared.warn("Windows 蓝牙广播状态 → Stopped"),
                    other => adv_shared.notice(format!("Windows 蓝牙广播状态 → code={other}")),
                }
                Ok(())
            }))
            .map_err(|e| format!("注册广播状态回调失败：{e}"))?;
        // token 随 provider 一起存活；显式 drop 会让回调失效，所以这里只记录不释放
        let _ = token;
    }

    shared.notice("Windows 蓝牙外设角色已启动（GattServiceProvider 广播中，等待对端连入）");

    let (state_tx, state_rx) = oneshot::channel();
    let _ = state_tx.send(Ok(()));

    Ok(PeripheralStart {
        server: PeripheralServer {
            events: rx,
            writer: PeripheralWriter { tx_char, shared },
            server: provider,
        },
        state: state_rx,
    })
}

/// 建一个本地特征（属性按角色给），失败时**带上是哪个特征**。
fn create_characteristic(
    service: &windows::Devices::Bluetooth::GenericAttributeProfile::GattLocalService,
    char_uuid: &str,
    props: GattCharacteristicProperties,
    label: &str,
) -> Result<GattLocalCharacteristic, String> {
    let params = GattLocalCharacteristicParameters::new()
        .map_err(|e| format!("构造特征参数失败（{label}）：{e}"))?;
    params
        .SetCharacteristicProperties(props)
        .map_err(|e| format!("设置特征属性失败（{label}）：{e}"))?;

    service
        .CreateCharacteristicAsync(uuid(char_uuid), &params)
        .map_err(|e| format!("创建特征失败（{label}）：{e}"))?
        .join()
        .map_err(|e| format!("创建特征失败（{label}）：{e}"))?
        .Characteristic()
        .map_err(|e| format!("创建特征失败（{label}）：{e}"))
}

/// 把 WinRT 的 `IBuffer` 读成 `Vec<u8>`（与 btleplug 的 `utils::to_vec` 同款）。
fn buffer_to_vec(buf: &windows::Storage::Streams::IBuffer) -> Result<Vec<u8>, String> {
    let reader = DataReader::FromBuffer(buf).map_err(|e| format!("读取写入值失败：{e}"))?;
    let len = reader
        .UnconsumedBufferLength()
        .map_err(|e| format!("读取写入长度失败：{e}"))? as usize;
    let mut data = vec![0u8; len];
    if len > 0 {
        reader
            .ReadBytes(&mut data)
            .map_err(|e| format!("读取写入字节失败：{e}"))?;
    }
    Ok(data)
}

/// 把 `Vec<u8>` 包成 WinRT `IBuffer`。
fn vec_to_buffer(data: &[u8]) -> Result<windows::Storage::Streams::IBuffer, String> {
    let writer = DataWriter::new().map_err(|e| format!("构造通知缓冲失败：{e}"))?;
    writer
        .WriteBytes(data)
        .map_err(|e| format!("写入通知缓冲失败：{e}"))?;
    writer
        .DetachBuffer()
        .map_err(|e| format!("取出通知缓冲失败：{e}"))
}

/// 对端标识：优先用 GATT session 的设备 id（稳定、可跨重连识别），
/// 取不到时退回"未知对端"（**绝不 panic**：回调线程里 panic 会带走整个进程，
/// 而 Windows release 也是 `panic = "abort"`）。
///
/// ⚠️ **写路径与订阅路径必须用同一个函数**：前者给重组器当 key，后者给订阅表当 key；
/// 两处各写一份的话，键一旦不一致就会表现为"收到了分片却永远找不到订阅者"（发不回去）。
fn session_id(client: &GattSubscribedClient) -> String {
    device_id_of(client.Session().ok().as_ref())
}

/// 从 `GattSession` 取对端标识。
///
/// `BluetoothDeviceId` **没有 `Display`**（只有 `Id() -> HSTRING`），
/// 所以不能直接 `to_string()` —— 这是一个只看类型名很容易写错的地方。
fn device_id_of(
    session: Option<&windows::Devices::Bluetooth::GenericAttributeProfile::GattSession>,
) -> String {
    session
        .and_then(|s| s.DeviceId().ok())
        .and_then(|id| id.Id().ok())
        .map(|h| h.to_string())
        .unwrap_or_else(|| UNKNOWN_CENTRAL.to_string())
}

/// 取不到 session 时的兜底对端名。**必须是一个固定的常量**：
/// 写路径与订阅路径都用它，键才对得上（各自造一个字符串就永远匹配不上）。
const UNKNOWN_CENTRAL: &str = "unknown-central";

/// 当前订阅者（central 标识 → 订阅句柄）。**进程级**一份：一台机器只有一个 GATT server。
///
/// 用 `OnceLock` 而不是 `Mutex::new(HashMap::new())`：后者不是 const fn，
/// 静态初始化里用不了（E0015）。`LazyLock` 需要 Rust 1.80，而 MSRV 是 1.77。
static SUBSCRIBERS: OnceLock<Mutex<HashMap<String, GattSubscribedClient>>> = OnceLock::new();

fn subscribers() -> &'static Mutex<HashMap<String, GattSubscribedClient>> {
    SUBSCRIBERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 当前时间（毫秒）—— 重组器的分片 TTL 要用（与 macOS/Android 侧同一套回收语义）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 读一次订阅列表，把它同步成本地表；对**消失的人**发 `Unlinked`。
///
/// WinRT 没有"订阅者断开"事件，只有 `SubscribedClientsChanged`（取消订阅、断开都会触发），
/// 所以"谁不在了"只能靠差集算出来 —— 与 macOS 侧同一条平台限制。
fn sync_subscribers(tx_char: &GattLocalCharacteristic, shared: &Arc<Shared>) {
    let current: Vec<GattSubscribedClient> = match tx_char.SubscribedClients() {
        Ok(v) => v.into_iter().collect(),
        Err(e) => {
            shared.warn(format!("读取订阅列表失败：{e}"));
            return;
        }
    };
    let ids: Vec<String> = current.iter().map(session_id).collect();
    let mut known = subscribers().lock().unwrap_or_else(|e| e.into_inner());

    // 1) 消失的人 ⇒ Unlinked，并作废他那份半截消息（否则残留分片一直占内存）
    let before: Vec<String> = known.keys().cloned().collect();
    for gone in before.iter().filter(|id| !ids.contains(id)) {
        known.remove(gone);
        if let Ok(mut rs) = shared.reassemblers.lock() {
            rs.remove(gone);
        }
        shared.emit(PeripheralEvent::Unlinked {
            central: gone.clone(),
        });
    }

    // 2) 新增的人 ⇒ 记下来（发通知要用它），并留一条可 grep 的日志
    for (id, client) in ids.iter().zip(current.iter()) {
        if known.insert(id.clone(), client.clone()).is_none() {
            shared.notice(format!("对端已订阅通知（{id}）"));
        }
    }
}

/// 挂上"收到写请求"的处理：读值 → 喂重组器 → 完整帧投给网络层；**每个请求都要应答**。
fn install_write_handler(
    rx_char: &GattLocalCharacteristic,
    shared: Arc<Shared>,
) -> Result<(), String> {
    let token = rx_char
        .WriteRequested(&TypedEventHandler::<
            GattLocalCharacteristic,
            windows::Devices::Bluetooth::GenericAttributeProfile::GattWriteRequestedEventArgs,
        >::new(move |_sender, args| {
            let Ok(args) = args.ok() else { return Ok(()) };
            // 对端标识取自 **args 的 session**，与订阅表用的是同一套取法 ——
            // 两处键必须一致，否则"收到了分片却找不到订阅者回不了消息"。
            let central = device_id_of(args.Session().ok().as_ref());

            // 请求对象要 await 才能拿到（WinRT 的 deferral 语义）。这里用
            // `IAsyncOperation` 自带的**阻塞** `join()`（`GetRequestAsync` 很快返回），
            // 在**回调线程**上完成 —— 不跨 await，也就不会把非 `Send` 的 WinRT
            // 对象带进 tokio 任务。回调线程是一次性的（不做渲染），阻塞安全。
            match args.GetRequestAsync() {
                Ok(op) => match op.join() {
                    Ok(req) => handle_write_request(req, &central, &shared),
                    Err(e) => shared.warn(format!("读取写请求失败：{e}")),
                },
                Err(e) => shared.warn(format!("取写请求失败：{e}")),
            }
            Ok(())
        }))
        .map_err(|e| format!("注册写请求回调失败：{e}"))?;
    let _ = token; // 回调随特征一起存活；显式保留便于将来解除
    Ok(())
}

/// 处理一条写请求：先应答（GATT 语义），再把值喂给重组器。
fn handle_write_request(req: GattWriteRequest, central: &str, shared: &Arc<Shared>) {
    // 先取值，再应答：应答之后请求对象就失效了
    let data = match req
        .Value()
        .map_err(|e| format!("读取写入值失败：{e}"))
        .and_then(|b| buffer_to_vec(&b))
    {
        Ok(d) => Some(d),
        Err(e) => {
            shared.warn(e);
            None
        }
    };

    // **必须应答**：write-with-response 不应答的话 central 每次写都等到超时
    //（ADR-0015 §7.3 第 5 条同款）。WriteWithoutResponse 不需要（WinRT 也不要求）。
    let needs_response = req
        .Option()
        .map(|o| o == GattWriteOption::WriteWithResponse)
        .unwrap_or(true);
    if needs_response {
        if let Err(e) = req.Respond() {
            shared.warn(format!("应答写请求失败（对端会等到超时）：{e}"));
        }
    }

    let Some(data) = data else { return };

    let now = now_ms();
    let outcome = {
        let mut rs = match shared.reassemblers.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        match rs.get_mut(central) {
            Some(r) => r.push(&data, now),
            None => {
                let mut r = BleReassembler::new();
                let o = r.push(&data, now);
                rs.insert(central.to_string(), r);
                o
            }
        }
    };

    match outcome {
        PushOutcome::Complete(bytes) => {
            shared.emit(PeripheralEvent::Frame {
                central: central.to_string(),
                bytes,
            });
        }
        // 正常：这一片收下了，消息还没齐
        PushOutcome::Incomplete => {}
        PushOutcome::Dropped(reason) => {
            // 对端发来的东西不合法或重复（超长/超分片/在途过多/重复片）——必须留痕，
            // 否则表现为"对端一直在发、我们永远拼不出帧"，日志里什么都没有
            shared.notice(format!("丢弃来自 {central} 的分片：{reason}"));
        }
    }
}

/// 挂上"订阅变化"的处理：跟踪谁订阅了我们（发通知要用），并对离开的人发 `Unlinked`。
fn install_subscription_handler(
    tx_char: &GattLocalCharacteristic,
    shared: Arc<Shared>,
) -> Result<(), String> {
    let handler_shared = shared.clone();
    let char_for_sync = tx_char.clone();
    let token = tx_char
        .SubscribedClientsChanged(&TypedEventHandler::<
            GattLocalCharacteristic,
            windows::core::IInspectable,
        >::new(move |_sender, _args| {
            sync_subscribers(&char_for_sync, &handler_shared);
            Ok(())
        }))
        .map_err(|e| format!("注册订阅变化回调失败：{e}"))?;
    let _ = token;
    Ok(())
}

impl PeripheralWriter {
    /// 该对端一次通知能收多少**应用层字节**（分片有效载荷）。
    ///
    /// 取 `GattSubscribedClient::MaxNotificationSize()`（一次通知能装的字节数，
    /// **已含 ATT 头**）并交给与 central 侧共用的换算 —— 两边不会各说各话。
    ///
    /// ⚠️ **三个平台的输入语义并不相同**，不要以为可以互换：
    ///
    /// | 平台 | 来源 | 含 ATT 头？ | 用哪个换算 |
    /// |---|---|---|---|
    /// | macOS | `maximumUpdateValueLength` | **不含**（Apple 文档明确：就是载荷） | `notify_payload_budget` |
    /// | Windows | `MaxNotificationSize` | **含**（本文件所据） | `att_payload_budget` |
    /// | Android | Kotlin `payloadMtu` | **不含**（与 macOS 同口径） | `notify_payload_budget` |
    ///
    /// ⚠️ Windows 那一格（"含 ATT 头"）**尚未在真机验证**：若实际不含，我们每片会少发
    /// 3 字节 —— 那是**偏保守**的方向（吞吐略降），不会像 Android 2026-09-16 修掉的那个
    /// 缺陷那样"直接发不出去"。要动它请先真机确认 `MaxNotificationSize` 的语义。
    pub fn payload_mtu(&self, central: &str) -> usize {
        let max = {
            let subs = subscribers().lock().unwrap_or_else(|e| e.into_inner());
            subs.get(central)
                .and_then(|c| c.MaxNotificationSize().ok())
                .unwrap_or(0)
        };
        ble_framing::att_payload_budget(max)
    }

    /// 对端是否还订阅着（断开/取消订阅都为 false）。
    pub fn is_subscribed(&self, central: &str) -> bool {
        subscribers()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(central)
    }

    /// 发一条完整帧：按 MTU 分片，逐片 `NotifyValueForSubscribedClientAsync`。
    pub async fn send_frame(&self, central: &str, payload: &[u8]) -> Result<usize, String> {
        let mtu = self.payload_mtu(central);
        let chunks = ble_framing::fragment(payload, mtu, next_msg_id()).ok_or_else(|| {
            format!(
                "帧无法分片（过大或 MTU 非法：len={} mtu={mtu}）",
                payload.len()
            )
        })?;
        let total = chunks.len();
        let central_owned = central.to_string();

        // ⚠️ 真正的发送放进 `spawn_blocking`：WinRT 的 `NotifyValueForSubscribedClientAsync`
        // 只能**阻塞**等待结果（`join()`），而 `IBuffer` / `GattSubscribedClient` **既不是
        // `Send` 也不是 `Sync`** ⇒ 绝不能让它们跨 `.await`（否则整个 future 非 `Send`，
        // `network/ble.rs` 里把它放进 `tokio::spawn` 会直接编译不过）。
        // 放进阻塞任务后：WinRT 对象全部留在那个线程里，异步侧只传 `Vec<u8>`。
        let writer = self.clone();
        tokio::task::spawn_blocking(move || writer.send_chunks_blocking(&central_owned, &chunks))
            .await
            .map_err(|e| format!("通知发送任务失败：{e}"))??;
        Ok(total)
    }

    /// **阻塞**发送全部分片：WinRT 侧一律用 `join()` 等结果，不引入 async。
    ///
    /// 分片之间必须节流（见 `NOTIFY_CHUNK_INTERVAL`）：连发会被协议栈丢掉中间分片，
    /// 对端重组器就永远拼不出完整帧。
    fn send_chunks_blocking(&self, central: &str, chunks: &[Vec<u8>]) -> Result<(), String> {
        let total = chunks.len();
        for (idx, chunk) in chunks.iter().enumerate() {
            let deadline = std::time::Instant::now() + NOTIFY_DEADLINE;
            loop {
                // 与 macOS/Android 同一条判据：**对端还订阅着**才发得出去。
                // 用它给出比"通知返回非 Success"更清楚的错误（后者可能是队列满，可重试）。
                if !self.is_subscribed(central) {
                    return Err(format!("对端未订阅通知（central={central}）"));
                }
                let client = {
                    let subs = subscribers().lock().unwrap_or_else(|e| e.into_inner());
                    subs.get(central).cloned()
                };
                let Some(client) = client else {
                    return Err(format!("对端已取消订阅（central={central}）"));
                };

                let buffer = vec_to_buffer(chunk)?;
                match client_result(&self.tx_char, &buffer, &client) {
                    Ok(GattCommunicationStatus::Success) => break,
                    // 剩下的都算"这一片没送到"：等一等再试（可能是通知队列满）
                    Ok(other) => {
                        if std::time::Instant::now() >= deadline {
                            return Err(format!(
                                "通知发送未成功（central={central} status={other:?}）"
                            ));
                        }
                    }
                    Err(e) => {
                        if std::time::Instant::now() >= deadline {
                            return Err(format!("通知发送失败（central={central}）：{e}"));
                        }
                    }
                }
                std::thread::sleep(NOTIFY_RETRY_WAIT);
            }
            if idx + 1 < total {
                std::thread::sleep(NOTIFY_CHUNK_INTERVAL);
            }
        }
        Ok(())
    }
}

/// 对单个订阅者发一条通知，并把 WinRT 的结果翻成 `GattCommunicationStatus`。
///
/// 单独抽出来是为了让"看返回值"这件事显式可读：**`Ok(())` 不够，必须看 status**。
fn client_result(
    tx_char: &GattLocalCharacteristic,
    buffer: &windows::Storage::Streams::IBuffer,
    client: &GattSubscribedClient,
) -> Result<GattCommunicationStatus, String> {
    tx_char
        .NotifyValueForSubscribedClientAsync(buffer, client)
        .map_err(|e| format!("发起通知失败：{e}"))?
        .join()
        .map_err(|e| format!("通知未完成：{e}"))?
        .Status()
        .map_err(|e| format!("读取通知结果失败：{e}"))
}

fn next_msg_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 外设与 central **必须用同一份换算**：一条链路上能发多大一片只有一个答案。
    ///
    /// 为什么值得一条护栏：`MaxNotificationSize`（本模块）与协商 MTU（`driver::payload_mtu`）
    /// 是两个不同来源的同一个概念，一旦有人各写一份减法，症状就是"某台设备就是收不到消息"。
    #[test]
    fn peripheral_and_central_agree_on_payload_budget() {
        // 默认 MTU 23 ⇒ 载荷 20（两侧同值）
        assert_eq!(ble_framing::att_payload_budget(23), 20);
        assert_eq!(
            crate::transport::bluetooth::driver::payload_mtu(23),
            ble_framing::att_payload_budget(23)
        );
        // 协商到 185 ⇒ 载荷 182
        assert_eq!(ble_framing::att_payload_budget(185), 182);
        // 异常值：0 / 1 / 2 / 3 都不能返回 0（否则 fragment 拒绝一切 ⇒ 链路静默假死）
        for bad in [0u16, 1, 2, 3] {
            let budget = ble_framing::att_payload_budget(bad);
            assert!(
                budget >= 20,
                "att_payload_budget({bad}) = {budget}，异常 MTU 必须退回默认 20 字节"
            );
        }
        // 边界：刚刚装得下 ATT 头（4-3=1）是**合法换算**，fragment 自己会拒绝这种迷你值
        assert_eq!(ble_framing::att_payload_budget(4), 1);

        // **AOSP 硬上限 512**（`BluetoothGatt.GATT_MAX_ATTR_LEN`，见 ble_framing 的注释）：
        // `writeCharacteristic` 对 value 长度是 `> 512` 直接抛 IllegalArgumentException，
        // 与协商 MTU 无关。btleplug 在 Android 上 `Peripheral::mtu()` 返回**请求值 517**
        // ⇒ 517-3=514 > 512 ⇒ 每片 514 字节必被拒。
        // 真机症状：单分片帧（272B 聊天）正常、多分片帧（738B 好友申请）永远发不出去。
        assert_eq!(
            ble_framing::att_payload_budget(517),
            512,
            "协商值超过 AOSP 上限时必须封顶到 512，否则多分片帧永远写不出去"
        );
        assert_eq!(ble_framing::att_payload_budget(1024), 512);
        assert_eq!(ble_framing::att_payload_budget(515), 512);
    }

    /// 分片 → 重组必须往返一致（Windows 外设侧与另两端共用同一份 `ble_framing`）。
    #[test]
    fn fragmentation_round_trips_at_windows_notification_size() {
        let payload: Vec<u8> = (0..500u32).map(|i| (i % 251) as u8).collect();
        let mtu = ble_framing::att_payload_budget(23); // 最坏情况：没协商成功
        let chunks = ble_framing::fragment(&payload, mtu, 7).expect("必须能分片");
        let mut r = BleReassembler::new();
        let mut out = None;
        for c in &chunks {
            if let PushOutcome::Complete(bytes) = r.push(c, 0) {
                out = Some(bytes);
            }
        }
        assert_eq!(out.as_deref(), Some(payload.as_slice()));
    }
}
