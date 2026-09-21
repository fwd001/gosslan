//! BLE 传输的**运行时接线**（`feature = "bluetooth"`，ADR-0015 的 7-e）。
//!
//! ## 与 TCP 路径的关系
//! BLE 是第三种传输，但**复用**已有的一切：握手（`build_signed_hello` + `verify_hello`）、
//! 身份来源（只能由双向 Hello 验签建立 —— BLE 地址不是身份，架构原则 P-A01）、
//! 链路表与选路（`Endpoint::Ble` + `PathKind::Bluetooth`）、入站/出站去重判据
//! （`link_snapshot` + `should_accept_inbound_public`）、消息处理（`handle_message`）。
//! 这一层只补两件 BLE 专属的事：**扫描/连接**与**分片收发**（后者在
//! `transport::bluetooth::driver` + `transport::ble_framing`）。
//!
//! ## 帧格式：与 TCP 完全相同
//! BLE 上跑的还是 `serde_json` 序列化的 `Message` —— `ble_framing` 负责把整帧切成
//! BLE 分片再拼回来。也就是说**线格式零改动**（没碰 `protocol.rs`），
//! 这也是为什么 BLE 不需要 ADR-0017 那套"能力门控"。
//!
//! ## 默认关闭
//! 本模块整体 `#[cfg(feature = "bluetooth")]`，且即使用 feature 构建，
//! 也要用户在设置里打开「蓝牙」（`bt_enabled`，默认关闭）才会启动 ——
//! 局域网路径在任何情况下都不受影响。
// `HashMap`/`HashSet` 只有**外设侧**（`peripheral_accept_loop` 的 routes/handshaking）用到，
// 所以按**外设**的平台集合门控；跟着 central 集合一起加平台会变成 unused import 警告，
// 而本项目是警告零容忍。三个平台现在都有外设角色，所以集合一致。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use btleplug::api::{Central as _, CentralEvent, Peripheral as _};
use btleplug::platform::Adapter;
use futures::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::mesh::{BleEndpoint, Endpoint as MeshEndpoint, PathKind};
use crate::network::transport::{
    build_signed_hello, flush_group_outbox, flush_outbox, flush_pending_group_keys,
    flush_pending_group_reads, flush_pending_reads, handle_message, link_snapshot, mark_conn_seen,
    mark_file_wire_progress, mark_peer_offline, register_connection, should_accept_inbound_public,
    unregister_connection,
};
use crate::protocol::Message;
use crate::state::{AppState, Link};
use crate::transport::bluetooth::driver::{self, BleReader, BleWriter};
// 扫描日志里要显示"命中的是哪个服务 UUID"（区分"对端没广播"与"对端广播的是别的 UUID"）
use crate::transport::bluetooth::SERVICE_UUID;
// 外设角色的驱动按平台切换：macOS 用 CoreBluetooth（objc2）、Android 用 JNI 调 Kotlin、
// Windows 用 WinRT `GattServiceProvider`。三者对外接口**完全同形**
// （`start` / `PeripheralServer` / `PeripheralWriter` / `PeripheralEvent`），
// 因此下面所有外设逻辑（事件循环、握手、路由、读写循环）三个平台共用一份。
//
// 真机 2026-09-13 的教训（`docs/notes/windows-ble-diagnosis-2026-09-13.md`）：
// 「先只做 central、外设下一轮」**行不通** —— 不能广播的一方永远不被对端发现，
// 而镜像护栏又让它（当 id 更小时）不主动拨 ⇒ 两侧都在等对方。外设角色是**必需**的。
#[cfg(target_os = "android")]
use crate::transport::ble_android as peripheral;
#[cfg(target_os = "android")]
use crate::transport::ble_android::{PeripheralEvent, PeripheralWriter};
#[cfg(target_os = "macos")]
use crate::transport::bluetooth_peripheral as peripheral;
#[cfg(target_os = "macos")]
use crate::transport::bluetooth_peripheral::{PeripheralEvent, PeripheralWriter};
#[cfg(target_os = "windows")]
use crate::transport::bluetooth_peripheral_windows as peripheral;
#[cfg(target_os = "windows")]
use crate::transport::bluetooth_peripheral_windows::{PeripheralEvent, PeripheralWriter};

/// 每轮扫描的观察窗口（`btleplug` 的扫描是"持续到显式停止"，给一个窗口再收结果）。
///
/// 2026-09-13 两条要求合流（用户先说「扫描快一点」，后说「按前台/后台分级」）：
/// 窗口从 3s 收到 **2s**（BLE 广播周期通常 20ms~1.28s，2s 已覆盖多轮广播；
/// 窗口越短，等待占比越高、发现越快，而且 `stop()` 等扫描任务退出时也少等 1s），
/// 而两轮之间的**间隔**按前台/后台分级（见下）—— 快在"用户正在用的时候"，
/// 省电在"没人在看的时候"。
const SCAN_WINDOW: Duration = Duration::from_secs(2);
/// **前台/聚焦**时两轮扫描之间的间隔（2s 窗口 + 5s 等待 ≈ 7s 一轮，比旧值快一倍多）。
///
/// 用户 2026-09-13：「APP 在前台（用户正在使用时）可以提高一下信息的刷新率」。
const SCAN_INTERVAL_ACTIVE: Duration = Duration::from_secs(5);
/// **后台/失焦**时两轮扫描之间的间隔。
///
/// 用户 2026-09-13：「APP 在后台可以降低一下扫描率」。30s 保证"别人发来的好友申请
/// 最终仍能到"，但把射频占空比从 2s/5s 降到 2s/30s（耗电与对周围设备的打扰都显著下降）。
/// 「APP 被杀死 ⇒ 直接关掉」不需要额外代码：进程没了，扫描任务自然不存在。
const SCAN_INTERVAL_IDLE: Duration = Duration::from_secs(30);
/// 用户主动要求扫描时，给拨号退避打的折扣（重试更快，但仍留一点间隔避免连打）。
///
/// 为什么需要（真机 2026-09-13）：退避上限 60s 是给"自动重试"用的礼貌间隔，
/// 但用户点了「扫描」却因为退避还在 40s 而**什么都不发生**，体感就是"搜不到"。
/// 于是用户触发的那一轮把退避窗口压到这个值（`ble_scan_now` ⇒ `user_triggered`）。
const USER_TRIGGER_BACKOFF_FACTOR: i64 = 8;
/// 握手（发自己的 Hello → 等对端 Hello）上限。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 建立 GATT 连接（connect + service discovery + 订阅）的**整体**上限。
///
/// 为什么必须有（真机教训，与"已有在途拨号连刷好几分钟"同源）：
/// driver::connect 内部的 peripheral.connect() 在平台上**没有超时**，
/// 一旦系统调用因射频/固件异常挂住，这个拨号任务会一直活着，DialGuard 也就一直不释放
/// ⇒ 该对端在整个进程生命周期内**再也不会被重新拨号**，用户看到的是"怎么等都连不上"。
/// 定时器把这条路径封死：最长 20s 一定结束，守卫一定释放，下一轮扫描可以重试。
const BLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 读循环的单次等待窗口：到点就回到循环顶部，让 shutdown/cancel 有机会被轮询，
/// 同时顺手回收半截消息。
const READ_IDLE: Duration = Duration::from_millis(1_500);
/// 停止时等待后台任务的上限（与 LAN 的 `STOP_TASK_TIMEOUT` 同口径）。
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// 一次写失败后的**退避重试**次数与间隔（见 `ble_writer_loop` 里那段说明）。
///
/// 为什么需要（真机 2026-09-13 安卓日志）：写失败一次就把写循环结束掉，而**读循环还活着**
/// ⇒ 链路变成"能收不能发"的僵尸：上层发送永远报「连接关闭」，
/// 又因为读活性还新鲜、45s 看门狗按"读"判健康 ⇒ 永远拆不掉 ⇒ 只能重启应用。
/// BLE 的写失败大多是瞬态的（对端 GATT 通知队列满、链路忙、连发被拒），退避重试即可。
const WRITE_RETRY_ATTEMPTS: u32 = 4;
const WRITE_RETRY_WAIT: Duration = Duration::from_millis(120);

/// 由「每片有效载荷预算」估算 BLE 吞吐。
///
/// 返回 (净数据字节, KB/s)。净数据 = 预算 - 6 字节分片头（BLE_CHUNK_HEADER_LEN）。
/// KB/s 按外设侧每片 12ms 的通知节流估算（Android / Windows 的真机参数）——
/// central 的写入没有这个节流，所以它是一个**保守下界**，用来给"慢"一个量级参考，
/// 不是精确预测。MTU 23（预算 20）时约 1.2 KB/s，MTU 517（预算 514）时约 41 KB/s。
fn ble_throughput_estimate(payload_budget: usize) -> (usize, f64) {
    const NOTIFY_INTERVAL_SECS: f64 = 0.012;
    let net = payload_budget.saturating_sub(crate::transport::ble_framing::BLE_CHUNK_HEADER_LEN);
    (net, net as f64 / NOTIFY_INTERVAL_SECS / 1024.0)
}

/// 扫描窗口（毫秒）—— 供诊断面板展示当前节奏。
pub fn scan_window_ms() -> u64 {
    SCAN_WINDOW.as_millis() as u64
}

/// 指定节奏下的扫描间隔（毫秒）—— 供诊断面板展示当前节奏。
pub fn scan_interval_ms(active: bool) -> u64 {
    scan_interval(active).as_millis() as u64
}

/// 指定节奏下的扫描间隔（扫描循环与上面的诊断共用一个判据，避免两处各写一份）。
fn scan_interval(active: bool) -> Duration {
    if active {
        SCAN_INTERVAL_ACTIVE
    } else {
        SCAN_INTERVAL_IDLE
    }
}

/// 蓝牙运行时的句柄（放在 `AppState` 里，与 LAN 的 `NetworkHandle` 同思路）。
pub struct BleHandle {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// 蓝牙运行时的**真实**状态：`(是否已启动, 已建立 BLE 链路的对端数)`。
///
/// 为什么需要它：`TransportManager::status()` 里那个 `BluetoothTransport` 是"尚未接线"的
/// 占位实现 —— 它的 `running` 恒为 `false`、`peers` 恒为 `0`。界面直接采信它就会永远显示
/// "蓝牙未运行"，用户点了开关也看不到任何变化（用户 2026-09-12 安卓实测的
/// 「蓝牙通道打不开」里，有一部分就是这个假状态造成的误导）。
pub async fn runtime_state(state: &Arc<AppState>) -> (bool, usize) {
    let running = state
        .ble
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    let peers = {
        let links = state.links.lock().await;
        links
            .values()
            .filter(|ls| ls.iter().any(|l| l.path_kind == PathKind::Bluetooth))
            .count()
    };
    (running, peers)
}

/// 启动蓝牙通道：探测适配器 → 起扫描循环。已在运行时幂等。
pub async fn start(state: Arc<AppState>) -> Result<(), String> {
    {
        let slot = state.ble.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Ok(());
        }
    }
    // 没有适配器 / 用户未授权 ⇒ 明确报错，由上层把通道标为不可用。
    // **绝不能影响局域网**：调用方（`set_channel_enabled`）只在成功时才认为已开启。
    let (adapter, local_addr) = driver::adapter().await?;
    // **本机适配器自己的蓝牙地址**（真机 2026-09-13 第四轮加）。
    //
    // 为什么必须打这一行：自己的广播**也会**出现在扫描结果里
    // （日志里的 `收到 N 个广播，其中 M 个是本应用服务`），而排查中最大的困扰就是
    // "哪个地址是这台机器自己"。前几轮只能靠 RSSI 波动幅度猜（稳定的像自己的网卡），
    // 猜错的代价是**整个判断反向** —— 我们一直把对端当成自己。
    // 有了这一行，扫描结果里"自己 / 对端"就是纯粹的事实比对，不再需要推理。
    match &local_addr {
        Some(addr) => state.logger.info(
            "ble",
            format!("本机蓝牙适配器地址 = {addr}（扫描结果里出现这个地址就是**自己**）"),
        ),
        None => state.logger.warn(
            "ble",
            "平台未暴露本机蓝牙适配器地址 —— 扫描结果里无法直接区分自己与对端",
        ),
    }
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // 「立刻扫一轮」的触发通道（与 LAN 的 `probe` 同范式，见 `AppState::ble_scan_now`）
    let (scan_now_tx, scan_now_rx) = watch::channel(0u64);
    *state.ble_scan_now.lock().unwrap_or_else(|e| e.into_inner()) = Some(scan_now_tx);

    // clone adapter/shutdown —— 两个后台任务（scan_loop + events）都要一份
    let adapter_for_events = adapter.clone();
    let mut shutdown_for_events = shutdown_rx.clone();

    let st = state.clone();
    let task = tokio::spawn(async move { scan_loop(st, adapter, shutdown_rx, scan_now_rx).await });

    // ==== P0：订阅 btleplug 的 CentralEvent 事件流 ====
    // 之前全树零调用 adapter.events() —— btleplug 的被动断链、被动连接、被动状态变化
    // 全被吞掉了。DeviceDisconnected 是区分"我们主动拆（写失败/看门狗）"与"系统掐断"
    // 的唯一锚点。这一层只打日志，不做任何业务动作（现有 teardown_link 已经够统一）。
    let state_for_events = state.clone();
    let _events_task = tokio::spawn(async move {
        match adapter_for_events.events().await {
            Ok(mut stream) => {
                state_for_events
                    .logger
                    .info("ble", "已订阅 btleplug adapter 事件流");
                while let Some(ev) = tokio::select! {
                    biased;
                    _ = shutdown_for_events.changed() => None,
                    ev = stream.next() => ev,
                } {
                    match ev {
                        CentralEvent::DeviceDisconnected(id) => {
                            state_for_events.logger.info(
                                "ble",
                                format!("[EVENT] btleplug DeviceDisconnected id={id}"),
                            );
                        }
                        CentralEvent::DeviceConnected(id) => {
                            state_for_events
                                .logger
                                .info("ble", format!("[EVENT] btleplug DeviceConnected id={id}"));
                        }
                        // 扫描相关事件（DeviceDiscovered / DeviceUpdated / ServicesAdvertisement /
                        // ManufacturerDataAdvertisement / ServiceDataAdvertisement / RssiUpdate）
                        // 已经被 scan_loop 的扫描逻辑覆盖，不重复打日志。
                        CentralEvent::StateUpdate(_) => {}
                        CentralEvent::DeviceServicesModified(_) => {}
                        _ => {}
                    }
                }
            }
            Err(e) => {
                state_for_events.logger.warn(
                    "ble",
                    format!(
                        "无法订阅 btleplug adapter events（{e}）—— 被动断链将无法通过事件流观测"
                    ),
                );
            }
        }
    });

    // ⚠️ **先把句柄放进去，再 spawn 外设**（顺序不能反）：`stop()` 靠这个句柄发停机信号，
    // 句柄晚一步写入就会出现"刚开就关"时 `stop()` 拿不到 handle ⇒ 外设任务永远活着。
    *state.ble.lock().unwrap_or_else(|e| e.into_inner()) = Some(BleHandle {
        shutdown: shutdown_tx.clone(),
        task,
    });

    // 外设角色（GATT server）：三端都实现了（macOS CoreBluetooth / Android Kotlin /
    // Windows WinRT GattServiceProvider），见 ADR-0015 §7.9 与
    // `docs/notes/windows-ble-diagnosis-2026-09-13.md`。
    // 这里独立启动、独立失败：外设起不来只影响"别人连我们"，不该把整个蓝牙开关判死。
    //
    // ⚠️ **不要 await 它**（用户 2026-09-13：「点蓝牙开关很卡，过好一会儿才会开」）：
    // 这条路径里要等 CoreBluetooth 回报状态（`peripheral::STATE_WAIT = 3s`），
    // 而 `start()` 在 `set_channel_enabled` 的关键路径上 ⇒ 命令要等满 3 秒才返回，
    // 开关就跟着卡 3 秒。外设角色既然"独立失败"，就没有任何理由阻塞"通道已启动"这个结论。
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    #[allow(clippy::let_underscore_future)] // JoinHandle 丢弃不影响 spawn 的任务
    let _ = tokio::spawn(start_peripheral(state.clone(), shutdown_tx.subscribe()));

    state.logger.info(
        "ble",
        format!(
            "蓝牙通道已启动（前台 {}s / 后台 {}s 一轮，窗口 {}s；打开「添加好友」会立刻再扫一轮）",
            SCAN_INTERVAL_ACTIVE.as_secs(),
            SCAN_INTERVAL_IDLE.as_secs(),
            SCAN_WINDOW.as_secs()
        ),
    );
    Ok(())
}

/// **让扫描循环立刻扫一轮**（用户打开「添加好友」/点「扫描」时调用）。
///
/// 返回 `true` = 已通知到（通道在跑）；`false` = 蓝牙通道没开，什么也没做。
///
/// 与 LAN 的 `search_nearby_peers` 同一个思路：**周期扫描负责"保持发现"，
/// 用户动作负责"立刻发现"** —— 后者才是用户感知到"快"的地方。
///
/// 与 [`wake_scan`] 的分工（两条链路的语义不同，不要合并）：
///   · 这一个 = **用户主动触发** ⇒ 立刻扫，并给拨号退避打折（`USER_TRIGGER_BACKOFF_FACTOR`）；
///   · `wake_scan` = **内部信号**（断链立刻重拨 / 从后台切回前台）⇒ 立刻扫，但不打折。
pub fn trigger_scan_now(state: &Arc<AppState>) -> bool {
    let slot = state.ble_scan_now.lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_ref() {
        Some(tx) => {
            let next = tx.borrow().wrapping_add(1);
            let _ = tx.send(next);
            state
                .logger
                .info("ble", "[DISCOVERY] 用户触发：立刻再扫一轮 BLE");
            true
        }
        None => false,
    }
}

/// 请求扫描循环**立刻扫一轮**（从后台切回前台 / 断链后立刻重拨时调用）。
///
/// 用 `Notify` 而不是重启循环：不打断正在进行的扫描，只是把"下一轮"的等待清零 ——
/// 否则后台节奏下用户点开「添加好友」最多要干等 30s，体感就是"搜不到人"。
pub fn wake_scan(state: &AppState) {
    state.ble_wake.notify_one();
}

/// 停止蓝牙通道：发停机信号 → 有界等待 → **摘掉所有 BLE 链路**（不动 LAN 链路）。
pub async fn stop(state: &Arc<AppState>) {
    let handle = state.ble.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handle) = handle {
        let _ = handle.shutdown.send(true);
        if tokio::time::timeout(STOP_TIMEOUT, handle.task)
            .await
            .is_err()
        {
            state
                .logger
                .warn("ble", "蓝牙后台任务未在 2s 内退出，继续收尾");
        }
    }
    detach_all_ble_links(state).await;
    // "不要再拨"的名单只对本次运行有效：下次开启允许重新学（对端可能换了角色/设备）
    state
        .ble_no_dial
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    // "谁在广播"同理：广播是每次开机重新发生的事，留着只会让下一轮的拨号判据用过时数据
    state
        .ble_peer_advertises
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    // 触发通道也一起撤掉：通道没开时 trigger_scan_now 必须老实返回 false，
    // 而不是"发进一个没人听的通道、让调用方误以为扫了"。
    *state.ble_scan_now.lock().unwrap_or_else(|e| e.into_inner()) = None;
    state.logger.info("ble", "蓝牙通道已停止");
}

/// 摘掉所有 BLE 链路（只筛 `PathKind::Bluetooth`，LAN/Routed 链路原样保留）。
async fn detach_all_ble_links(state: &Arc<AppState>) {
    // 先收集待处理的 (peer, endpoint)，再逐个拆 —— 避免持有 links 锁时做别的锁操作
    let victims: Vec<(String, MeshEndpoint)> = {
        let links = state.links.lock().await;
        links
            .iter()
            .flat_map(|(peer, list)| {
                list.iter()
                    .filter(|l| l.path_kind == PathKind::Bluetooth)
                    .map(|l| (peer.clone(), l.endpoint.clone()))
            })
            .collect()
    };
    for (peer, ep) in victims {
        teardown_link(state, &peer, &ep, "shutdown").await;
    }
}

/// 拆掉**一条** BLE 链路：取消读写任务 → 从链路表移除 → mesh 侧注销 →
/// 该 peer 若已无任何链路则标记离线。
///
/// 与 TCP 的 `reader_loop` 收尾同口径（"断一条 ≠ peer 下线"）：
/// 同一 peer 可能同时有 LAN 与 BLE 两条链路。
///
/// `reason`：谁/为什么在拆这条链路 —— 枚举值见所有调用点（"write_failure" /
/// "watchdog_stale" / "shutdown" / "peripheral_unlinked" / "new_connection_displace" /
/// "reader_error" / "peer_disconnect" / "passive_bt_stack_disconnect"）。
/// 拆链是 P0 可观测性的核心：这条日志是区分"我们主动拆的"与"系统把链路掐断了"
/// 的唯一锚点。
async fn teardown_link(
    state: &Arc<AppState>,
    peer_id: &str,
    ep: &MeshEndpoint,
    reason: &'static str,
) {
    // **入口必须打**：teardown 是所有 BLE 断链的汇聚点，从 adapter events、
    // watchdog、写失败、peripheral unlinked、拨号侧 cancel 都会走到这里。
    // 没有这条日志，断链原因永远是"不知道"。
    state.logger.info(
        "ble",
        format!("[TEARDOWN] 开始拆链 peer={peer_id} ep={ep} reason={reason}"),
    );
    let cancel = {
        let links = state.links.lock().await;
        links
            .get(peer_id)
            .and_then(|v| v.iter().find(|l| &l.endpoint == ep))
            .map(|l| l.cancel.clone())
    };
    if let Some(cancel) = cancel {
        let _ = cancel.send(true);
    }
    let peer_offline = {
        let mut links = state.links.lock().await;
        let mut offline = false;
        if let Some(v) = links.get_mut(peer_id) {
            v.retain(|l| &l.endpoint != ep);
            if v.is_empty() {
                links.remove(peer_id);
                offline = true;
            }
        }
        offline
    };
    unregister_connection(state, peer_id, ep);
    if peer_offline {
        mark_peer_offline(state, peer_id).await;
    }

    // **断链后立刻重拨**（2026-09-13 审计）：
    //
    // 原来这里只清理链路，不唤醒扫描 —— 于是"链路断掉 → 重新发现对端"要等**下一轮扫描**，
    // 前台最多 5s、**后台最多 30s**（后台节奏见 `SCAN_INTERVAL_IDLE`）。
    // 真机体感就是"断开后几十秒没反应"，而它本该是"立刻重连"。
    //
    // 另外把该 BLE 地址的**失败退避清掉**：退避是给"连不上"用的，
    // 而这条链路本来是**通的**（刚断），不该被它上一次的失败计数拖住重连。
    // 安全性：拆链的频率由"链路建立"决定，不是紧循环；对端真走了的话，
    // 下一轮扫描找不到它就自然回到常规节奏（退避也会重新累计）。
    // 与平台无关：外设角色只在 macOS/Android 有，但 central（拨号）侧各平台都在跑。
    if let MeshEndpoint::Ble(ble) = ep {
        state
            .ble_dial_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&ble.address);
        wake_scan(state);
    }
}

async fn scan_loop(
    state: Arc<AppState>,
    adapter: Adapter,
    mut shutdown: watch::Receiver<bool>,
    mut scan_now: watch::Receiver<u64>,
) {
    // 第一轮**不要等**：用户打开「添加好友」/刚开蓝牙时，最不想等的就是那 2 秒
    let mut skip_initial_wait = true;
    // 本轮是不是用户主动触发的（决定拨号退避要不要打折，见下）
    let mut user_triggered = false;
    loop {
        match driver::scan_peers(&adapter, SCAN_WINDOW).await {
            Ok((peers, total)) => {
                // 每次扫描都要留痕（**包括 0 个**）：真机上这两个数字是排查的关键 ——
                // "收到 0 个广播"= 扫描/权限/硬件问题；"收到 N 个但 0 个是本服务"= 对端没在广播
                // 或广播里没有我们的服务 UUID。用户 2026-09-12 的"互相搜不到"当时日志里
                // 什么都没有，只能靠猜。
                state.logger.info(
                    "ble",
                    format!(
                        "BLE 扫描：收到 {total} 个广播，其中 {} 个是本应用服务{}",
                        peers.len(),
                        if user_triggered {
                            "（用户主动触发）"
                        } else {
                            ""
                        }
                    ),
                );
                // 记进诊断状态（面板要在不重新扫的情况下知道最近一轮看到了什么）
                *state.ble_scan.lock().unwrap_or_else(|e| e.into_inner()) =
                    crate::state::BleScanStats {
                        last_ts: crate::db::now_ms(),
                        total: total as u32,
                        matched: peers.len() as u32,
                    };
                for peripheral in peers {
                    if *shutdown.borrow() {
                        return;
                    }
                    // **扫到 = 对端在广播**（这条扫描结果已经把"服务 UUID 对得上"过滤过了，
                    // 见 `driver::scan_peers`）。记下来供 `should_dial_ble` 判断：
                    // 对端不广播时必须由我们无条件拨（只做 central 的平台唯一能建链的方式）。
                    let adv_id = peripheral.id().to_string();
                    state
                        .ble_peer_advertises
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(adv_id);
                    // 对端是这条链路上的**指定拨号方**（它比我大）⇒ 别去拨它：
                    // 我们拨过去只会被它按镜像规则拒掉，而每次连接都会打断它拨过来的那条
                    // 好链路（真机症状：45s 收不到帧 → 看门狗拆链 → "加好友时连接已关闭"）。
                    if state
                        .ble_no_dial
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .contains(&peripheral.id().to_string())
                    {
                        continue;
                    }
                    // 失败退避：刚连不上的候选先别急着再试（指数退避 5s→60s，见
                    // `ble_dial_backoff_ms` 的注释：旧上限 10 分钟会把暂时性失败变成
                    // 用户可见的"好友申请等了几分钟"）。
                    //
                    // ⚠️ 跳过时**必须留痕**（含剩余毫秒）：否则真机上只能看到
                    // "候选 X 未建立链路"，完全看不出"其实是被退避锁住了"。
                    {
                        let now = crate::db::now_ms();
                        let skip = ble_dial_backoff(&state, &peripheral.id().to_string(), now);
                        if skip {
                            // 用户主动触发 ⇒ 退避窗口打折。理由是实测体感：
                            // 用户点了「扫描」，而退避还剩 40s ⇒ **什么都不发生**，
                            // 只能被理解成"搜不到"。打折后仍留一点间隔，不连打。
                            let raw_left = state
                                .ble_dial_failures
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .get(&peripheral.id().to_string())
                                .map(|(_, next)| (*next - now).max(0))
                                .unwrap_or(0);
                            let left = if user_triggered {
                                raw_left / USER_TRIGGER_BACKOFF_FACTOR
                            } else {
                                raw_left
                            };
                            if left > 0 {
                                state.logger.info(
                                    "ble",
                                    format!(
                                        "[DISCOVERY] 跳过候选 id={} 原因=退避中 剩余={left}ms{}",
                                        peripheral.id(),
                                        if user_triggered {
                                            "（已按用户触发打折）"
                                        } else {
                                            ""
                                        }
                                    ),
                                );
                                continue;
                            }
                            // 打折后已经可以试了：把退避记录清掉，让下面的拨号正常进行
                            clear_ble_dial_failure(&state, &peripheral.id().to_string());
                        }
                    }
                    let st = state.clone();
                    let sd = shutdown.clone();
                    let dial_id = peripheral.id().to_string();
                    // ⚠️ 「已建链」这一判定必须在**打「开始连接」之前**做。
                    // 原先只有 `dial_and_register` 里那条静默跳过，于是日志每轮都写一句
                    // 「候选可拨 ⇒ 开始连接（GATT central）」，后面却什么都没有 ——
                    // 真机日志里连着 18 轮这么写，读起来像"应用在反复拨一个已经连上的对端"，
                    // 而真相是这一轮**什么都没发生**（用户 2026-09-16 排查时被它带偏）。
                    // 放在这里还顺带省掉了下面读广播属性的那次 await。
                    if state
                        .has_endpoint_addr(&MeshEndpoint::Ble(BleEndpoint::new(dial_id.clone())))
                        .await
                    {
                        // ⚠️ 这里必须**补上**原来由 `dial_and_register` 的 `Ok(())` 分支
                        // 顺带做掉的那件事：清掉该地址的拨号退避。否则"已建链"的地址会一直
                        // 留着一条过期退避，等这条链路断掉、下一轮扫描要重拨时被它拖住
                        // （最长 20s）—— 用户看到的是"刚断开却半天连不回来"。
                        clear_ble_dial_failure(&state, &dial_id);
                        state.logger.info(
                            "ble",
                            format!("[DISCOVERY] 跳过候选 id={dial_id} 原因=已建链（不重复拨号）"),
                        );
                        continue;
                    }
                    // 把**广播里能拿到的事实**一起打出来（真机 2026-09-13 第二轮补）：
                    // 之前只打地址，于是"信号多强、是不是随机地址、对端有没有报名字、
                    // 它自报的服务列表是什么"这些一眼能定性的信息全丢了，
                    // 只剩一句 `Not connected` 无从判断。
                    // 这些字段都在 `PeripheralProperties` 里（btleplug 从广播/扫描响应解析）。
                    //
                    // ⚠️ 2026-09-13 第七轮：把**命中的那个服务 UUID**也打出来。
                    // 真机上出现过"Windows 搜得到安卓和 Mac，但安卓的扫描里只有 1 个本应用服务"
                    // —— 到底是对端没广播、还是广播里带的是**另一个** UUID（旧版本 / 另一份构建），
                    // 只有把 UUID 打出来才能区分。这是"三方都能扫到、偏偏有一方扫不到"的决定性证据。
                    let facts = match peripheral.properties().await {
                        Ok(Some(p)) => {
                            // 直接现算 UUID（`driver::uuid` 是私有的，不为了这条日志去放开它）
                            let svc = uuid::Uuid::parse_str(SERVICE_UUID)
                                .expect("BLE 服务 UUID 常量必须合法");
                            let hit = p
                                .services
                                .iter()
                                .find(|u| **u == svc)
                                .map(|u| u.to_string())
                                .unwrap_or_else(|| "(未在本设备广播里看到我们的 UUID)".to_string());
                            format!(
                                "rssi={:?} 地址类型={:?} 名字={:?} 广播服务数={} 命中={hit} 发射功率={:?}",
                                p.rssi,
                                p.address_type,
                                p.local_name.as_deref().or(p.advertisement_name.as_deref()),
                                p.services.len(),
                                p.tx_power_level
                            )
                        }
                        Ok(None) => "广播属性暂不可用".to_string(),
                        Err(e) => format!("读广播属性失败：{e}"),
                    };
                    state.logger.info(
                        "ble",
                        format!(
                            "[DISCOVERY] 候选可拨 id={dial_id} ⇒ 开始连接（GATT central）｜{facts}"
                        ),
                    );
                    // 每个候选一个任务：连接 + 握手最长 10s，串行会把扫描周期拖垮
                    tokio::spawn(async move {
                        let id = peripheral.id().to_string();
                        match dial_and_register(st.clone(), peripheral, sd).await {
                            // 成功 ⇒ 清掉这个候选的失败计数（下次断了还能正常重拨）
                            Ok(()) => clear_ble_dial_failure(&st, &id),
                            // 连接失败是常态（对方正在忙、走远了、不是 Gosslan 端），
                            // 记 info 不记 warn —— 否则日志会被邻居设备刷满
                            Err(e) => {
                                note_ble_dial_failure(&st, &id);
                                st.logger.info(
                                    "ble",
                                    format!("[DISCONNECT] 候选 {id} 未建立链路：{e}（已进入退避）"),
                                );
                            }
                        }
                    });
                }
            }
            Err(e) => state.logger.warn("ble", format!("扫描失败：{e}")),
        }
        // 本轮结束：决定等多久再开下一轮。
        // 节奏由**应用是否在前台/聚焦**决定（用户 2026-09-13 的功耗策略）：前台 5s、后台 30s。
        // 两个"立刻扫"的入口都要认（语义不同，见 helper 的注释）：
        //   · `ble_scan_now` = 用户主动触发（打开「添加好友」/点扫描）⇒ 立刻扫 + 退避打折；
        //   · `ble_wake`     = 内部信号（断链立刻重拨 / 从后台切回前台）⇒ 立刻扫，不打折。
        user_triggered = false;
        if skip_initial_wait {
            skip_initial_wait = false;
            continue;
        }
        let active = state.app_active.load(std::sync::atomic::Ordering::Relaxed);
        let interval = scan_interval(active);
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            // 用户主动触发：收到新值 ⇒ 跳过等待，马上扫一轮（并给退避打折）
            res = scan_now.changed() => {
                if res.is_err() {
                    // 发送端被丢掉（通道停止）：继续按周期跑，不要退出扫描循环
                } else {
                    user_triggered = true;
                }
            }
            // 内部唤醒：断链立刻重拨 / 从后台切回前台 ⇒ 也立刻扫，但**不打折**退避
            _ = state.ble_wake.notified() => {
                state.logger.info("ble", "[SCAN] 收到唤醒信号 ⇒ 立刻扫描一轮");
            }
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

/// BLE 候选失败退避的**纯函数内核**：连续失败 `failures` 次后，要等多久才允许再试。
///
/// ## 为什么几乎不退了（2026-09-13 第三轮真机）
///
/// 真机日志暴露出退避**把重试饿死了**：扫描周期已经是 2s，而退避是 5s→10s→20s→40s，
/// 于是日志里一半的行是 `跳过候选 … 原因=退避中 剩余=7849ms` ——
/// 用户看到的"搜不出来"，很大程度上是**我们自己不去连**。
///
/// 关键认识：**退避的初衷（别打扰对端）已经被 `DialGuard` 在途去重实现了** ——
/// 同一个对端不会叠起多条连接（真机 2026-09-13 第一轮就是那个 bug 的教训）。
/// 既然如此，"每轮扫描都试一次"就是安全的：一轮 4s，对端每 4s 被尝试一次，
/// 而 BLE 上连不上的尝试本身是廉价且无副作用的。
///
/// ## 为什么形状是"前几次不退 + 缓增到 20s"（合并评审 2026-09-13）
///
/// 一开始写成"前 3 次不退、之后**固定 5s**"。评审指出：那是**激进**的那一端 ——
/// 失败很多次（对端长期不在、或根本不是 Gosslan 端）时会一直每轮都敲。
/// 退避**慢**的代价用户可以忍（多等几秒），但射频/功耗被打**没有用户可见的反馈**，
/// 只会在电量上体现。所以改成缓增并把上限放在 20s：
///
/// | 连续失败 | 1–3 | 4 | 5 | 6 | 7+ |
/// |---|---|---|---|---|---|
/// | 冷却 | **0**（不退） | 5s | 10s | 20s | 20s（封顶） |
///
/// 上限 20s 与扫描周期（约 4s）同量级：最多跳过 5 轮就一定会再试一次，
/// 不会重演"被退避锁到分钟级"那次的故障形态。
fn ble_dial_backoff_ms(failures: u32) -> i64 {
    /// 前几次失败不退避 —— 这是"搜不出来"最直接的解药。
    const FREE_ATTEMPTS: u32 = 3;
    /// 缓增的起点与上限。
    const BASE_MS: i64 = 5_000;
    const MAX_MS: i64 = 20_000;
    if failures <= FREE_ATTEMPTS {
        return 0;
    }
    // 第 4 次 ⇒ 5s，第 5 次 ⇒ 10s，第 6 次 ⇒ 20s，之后封顶
    let step = failures - FREE_ATTEMPTS - 1; // 0,1,2,…
    (BASE_MS << step.min(2)).min(MAX_MS)
}

/// 该候选现在是否处于退避期（true = 跳过）。
fn ble_dial_backoff(state: &Arc<AppState>, id: &str, now: i64) -> bool {
    let map = state
        .ble_dial_failures
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.get(id).is_some_and(|(_, next)| now < *next)
}

/// 记一次失败（连续失败次数 +1，并按指数退避设下次允许时间）。
fn note_ble_dial_failure(state: &Arc<AppState>, id: &str) {
    let mut map = state
        .ble_dial_failures
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = map.entry(id.to_string()).or_insert((0, 0));
    entry.0 = entry.0.saturating_add(1);
    entry.1 = crate::db::now_ms() + ble_dial_backoff_ms(entry.0);
}

/// 连上了就清掉失败计数（下次断了还能正常重拨）。
fn clear_ble_dial_failure(state: &Arc<AppState>, id: &str) {
    state
        .ble_dial_failures
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

/// **BLE 链路谁拨号**（与 TCP 的 `should_dial` 同一条规则：**大 id 拨、小 id 只接受**）。
///
/// 抽成纯函数的理由：它是"两端都跑 central+peripheral 时不互相拨号"的**唯一判据**，
/// 而这类缺陷在真机上表现成"链路时好时坏、点加好友说连接已关闭"（镜像链路互相打断），
/// 极难复现；纯函数可以一次钉死，并让护栏在有人把它改成"总是拨"时立刻 FAIL。
///
/// ## `peer_advertises`：为「只做 central 的平台」留的活口（Windows / ADR-0015 §7-f）
///
/// 「大 id 拨」这条规则**只在两端都能广播时才成立** —— 它的前提是"对方也会拨我"。
/// Windows 这一轮只做 central（不能广播、不能被连），如果照搬这条规则：
/// 只要 Windows 的 id 比手机小，就**没有任何一侧会拨号**，链路永远建不起来。
///
/// 所以判据改成：**只要对端不广播（我们扫不到它的外围广播），就必须由我们拨**；
/// 只有确认对端在广播时才回到 id 比较。这样：
///   · Windows（小 id）↔ 手机（广播）⇒ Windows 无条件拨，链路能建；
///   · Mac ↔ 手机（两侧都广播）⇒ 行为与今天**逐字节一致**（id 比较）。
///
/// 注意：这个参数只影响"要不要主动拨"，**不影响身份** —— 身份永远只由双向 Hello 验签建立。
fn should_dial_ble(my_id: &str, peer_id: &str, peer_advertises: bool) -> bool {
    !peer_advertises || my_id > peer_id
}

/// 往已有链路投递，还是当作**新连接**重新握手（外设侧收到一帧时的唯一判据）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeripheralRouteAction {
    /// 投给该 central 已登记的链路（正常数据帧）。
    ToExistingLink,
    /// 走握手路径（没有链路，或这是**重连**发来的新 Hello）。
    ToHandshake,
}

/// 判定外设侧收到的一帧该投给旧链路还是重新握手。
///
/// ## 为什么必须有这条判据（2026-09-12 真机）
///
/// BLE 上同一个 central 的地址在**重连**时会被复用（macOS 侧是 CoreBluetooth 给同一台
/// 手机分配的 UUID，Android 侧是同一个 MAC）。旧连接的链路任务可能还没被清理，
/// 于是新连接发来的 **Hello 会被投给旧链路的管道**：
///   · 旧链路的写句柄指向**旧连接** ⇒ 新连接永远收不到 Hello 回应
///     ⇒ 对端报「握手超时：对端未回 Hello」；
///   · 旧链路把这条 Hello 当普通帧消费掉 ⇒ 对端报「对端首帧不是 Hello」。
/// 两种报错在用户侧都是"蓝牙时好时坏、加好友没反应"。
///
/// 所以：**有活路由 + 收到 Hello ⇒ 一定是重连**，必须换路由并重新握手。
/// 其余情况（普通数据帧、或本来就没有路由）都按原来的投递/握手走。
///
/// 抽成纯函数的理由同 `should_dial_ble`：这类判据写反了在真机上极难复现，
/// 而这里可以把它一次钉死，并让护栏在有人改成"永远投旧链路"时立刻 FAIL。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn peripheral_route_action(has_route: bool, frame_is_hello: bool) -> PeripheralRouteAction {
    // 没有活路由 ⇒ 只能握手；有路由且这帧是 Hello ⇒ 一定是重连 ⇒ 也必须握手。
    // 只有「有路由 + 不是 Hello」才是正常的"在已有链路上收数据"。
    if !has_route || frame_is_hello {
        PeripheralRouteAction::ToHandshake
    } else {
        PeripheralRouteAction::ToExistingLink
    }
}

/// 判断一帧**是不是 Hello** 的成本上限（字节）：Hello 只有设备 id + 两个公钥 + 签名，
/// 几百字节量级；超过这个长度的帧不可能是握手首帧，直接跳过解析。
///
/// 为什么要设上限：外设侧会对**每一个**到达的帧做这个判断（见
/// `peripheral_accept_loop`），而大文件分片是 256 KiB —— 对它们做一次
/// `serde_json::from_slice::<Message>` 就是白烧一倍解析成本。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
const HELLO_PEEK_MAX_BYTES: usize = 1024;

/// 轻量判断：这帧是不是 `Message::Hello`（用于上面的重连判据）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn frame_is_hello(bytes: &[u8]) -> bool {
    bytes.len() <= HELLO_PEEK_MAX_BYTES
        && matches!(
            serde_json::from_slice::<Message>(bytes),
            Ok(Message::Hello { .. })
        )
}

/// 连接一个候选 → 双向 Hello 验签 → 登记链路 → 起收发循环。
async fn dial_and_register(
    state: Arc<AppState>,
    peripheral: btleplug::platform::Peripheral,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ble_id = peripheral.id().to_string();
    // ── 在途去重（与 TCP 同款 `DialGuard`）────────────────────────────────
    // 真机证据（2026-09-13）：扫描每 10s 一轮，而握手最长 10s ⇒ 同一个外设上会**叠起
    // 2~3 个拨号任务**，每个都建一条 CoreBluetooth 连接并各自订阅一次通知流。
    // 而 Android 的 GATT server 对同一地址**只保留最后一条连接**，于是通知很可能被投给
    // "已经没人读的那条" ⇒ Mac 侧一个字节都收不到，而安卓侧 `notify` 全部返回成功。
    let Some(_dial_guard) = crate::state::DialGuard::try_acquire(&state, format!("ble:{ble_id}"))
    else {
        state.logger.info(
            "ble",
            format!("[CONNECT] 跳过 {ble_id}：已有在途拨号（避免在同一对端上叠连接）"),
        );
        return Ok(());
    };
    let ep = MeshEndpoint::Ble(BleEndpoint::new(ble_id.clone()));
    // 这个端点已经连着 ⇒ 跳过（`connect_to_peer` 的同款去重）。
    // 常规路径上扫描侧已经拦掉了（那时的日志是「跳过候选 id=… 原因=已建链」），
    // 这里兜的是"扫描判定完、任务真正跑起来之前"那一小段窗口 —— 概率低但确实会发生，
    // 所以也得留痕，不能像原来那样静默 return（静默正是"日志说开始连接却没了下文"的成因）。
    if state.has_endpoint_addr(&ep).await {
        state.logger.info(
            "ble",
            format!("[CONNECT] 跳过 {ble_id}：判定到拨号之间已建链（不重复拨号）"),
        );
        return Ok(());
    }
    // 上一次失败可能留了一条**已经没人读**的连接：先断开再重连。
    // 不这么做的话，`connect()` 会直接复用那条旧连接，而新订阅的通知流收不到任何东西。
    //
    // 同时把**系统报告的链路状态**记下来（真机 2026-09-13 第二轮需要它）：
    // "Windows 能不能连上手机"这件事有两个完全不同的失败面 ——
    //   · `is_connected=false` 且 connect 一直 Not connected ⇒ 射频层根本没连上
    //     （对端没在监听 / 不在范围 / 系统未授权 / 适配器问题）；
    //   · `is_connected=true` 但服务读不到 ⇒ 连上了、GATT 数据库还没就绪（重试才有意义）。
    // 没有这一行，日志里两者长得一模一样，只能靠猜。
    let before_connected = peripheral.is_connected().await;
    if matches!(before_connected, Ok(true)) {
        state.logger.info(
            "ble",
            format!("[CONNECT] {ble_id} 仍处于已连接状态 ⇒ 先断开，避免复用幽灵连接"),
        );
        let _ = peripheral.disconnect().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let conn = match tokio::time::timeout(BLE_CONNECT_TIMEOUT, driver::connect(&peripheral)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            // 失败时把"系统此刻怎么看这条链路"一并打出来 —— 这是下一轮排查的**唯一**线索
            let after = peripheral
                .is_connected()
                .await
                .map(|v| v.to_string())
                .unwrap_or_else(|err| format!("查询失败({err})"));
            return Err(format!(
                "{e}｜系统链路状态：连接前={:?} 失败后={after}",
                before_connected
            ));
        }
        Err(_) => {
            // 超时（见 BLE_CONNECT_TIMEOUT）：显式断开，DialGuard 随函数返回释放，
            // 该对端随即可被下一轮扫描重新拨号 —— 不再"永远连不上"。
            let _ = peripheral.disconnect().await;
            state.logger.warn(
                "ble",
                format!(
                    "[CONNECT] 连接超时（{}s 内未完成 connect/service discovery）ep={ble_id}",
                    BLE_CONNECT_TIMEOUT.as_secs()
                ),
            );
            return Err(format!(
                "连接超时（{}s 内未完成 connect/service discovery）",
                BLE_CONNECT_TIMEOUT.as_secs()
            ));
        }
    };
    state.logger.info(
        "ble",
        format!("[GATT] 已就绪 ep={ble_id}（连接 + 服务发现 + 通知订阅都成功）"),
    );
    // **每次建链必须打 MTU** —— 排查 BLE 大文件卡死的最关键观测点。
    // WinRT 上 MTU 是异步协商的（connect 返回时可能还是默认 23），
    // 但先记一次 baseline；后续 adapter events / 写循环失败时再交叉验证。
    let mtu = conn.mtu();
    state.logger.info(
        "ble",
        format!(
            "[MTU] ep={ble_id} 协商 MTU={mtu}B （ATT 有效载荷 ≈ {}B）",
            driver::payload_mtu(mtu)
        ),
    );
    // 从这里开始，任何失败都必须**显式断开** —— drop 一个 btleplug `Peripheral`
    // 不会断开 CoreBluetooth 连接，残留会累积成"幽灵连接"（真机症状见上面的注释）。
    match finish_dial(state.clone(), &peripheral, conn, &mut shutdown).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = peripheral.disconnect().await;
            Err(e)
        }
    }
}

/// `dial_and_register` 的握手与登记阶段（拆出来只为让失败路径能统一断开连接）。
async fn finish_dial(
    state: Arc<AppState>,
    peripheral: &btleplug::platform::Peripheral,
    conn: driver::BleConnection,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let ble_id = peripheral.id().to_string();
    let ep = MeshEndpoint::Ble(BleEndpoint::new(ble_id.clone()));
    let (mut writer, mut reader) = conn.into_split();

    // **协商到的 MTU 必须留痕**（2026-09-13 审计）：它是 BLE 吞吐的**唯一**决定因素
    // （每片有效载荷 = MTU-3-6；速率 ≈ 载荷 / 每片间隔）。
    // 此前全仓库没有这一行，于是文档里的"MTU=23 ⇒ 1KB/s"一直是**猜测**，
    // 而代码其实会协商到 182~514 字节载荷（btleplug：macOS `maximumWriteValueLength+3`、
    // Android `requestMtu(517)`）—— 差一个数量级。没有这条日志就没法判断"慢"到底慢在哪。
    // 注意：这里的"每片有效载荷"是**含 6 字节分片头**的 ATT 预算；真正上去的数据是
    // 预算减 6。以前括号里写"MTU=载荷+3+6"是错的（多了 6），会让真机排查算错一个量级。
    let mtu_budget = writer.payload_mtu();
    let (net_bytes, kbps) = ble_throughput_estimate(mtu_budget);
    state.logger.info(
        "ble",
        format!(
            "[GATT] MTU 协商结果 ep={ble_id} 每片有效载荷={mtu_budget} 字节（净数据={net_bytes}，分片头 6；按 12ms/片估算 ≈ {kbps:.1} KB/s）"
        ),
    );

    // ---- 握手：先发自己的 Hello，再读对端的、并**必须验签**（§8 / ADR-0011）----
    // BLE 地址不是身份，所以这里身份一定是"未知"（conv_clock 传 0 即可：
    // 对端 observe_clock 取 max，不会因此倒退）。
    let hello = build_signed_hello(&state, 0);
    let bytes = serde_json::to_vec(&hello).map_err(|e| format!("Hello 序列化失败：{e}"))?;
    writer.send_frame(&bytes).await?;

    // ⚠️ **允许跳过握手前导帧**（2026-09-13 真机抓到的真因）：
    //    日志里反复出现 `对端首帧不是 Hello（收到 chat_message）` ⇒ 链路**永久建不起来**。
    //    原因是 Android 的 notify 按**central 地址**投递：上一条链路的待发帧（outbox flush）
    //    会落在**新连接**上，于是新连接的"第一帧"是先前的业务帧，而不是 Hello。
    //    旧行为直接放弃 ⇒ 双方各自重拨、互相打断，好友申请/消息全部过期。
    //    新行为：窗口内继续读，丢掉非 Hello 的前导帧（**不处理**——身份还没验签），
    //    读到 Hello 就正常握手；窗口耗尽仍只报错（并说明收到了什么）。
    let first = read_hello_frame(
        &mut reader,
        HANDSHAKE_TIMEOUT,
        &mut *shutdown,
        &state,
        &ble_id,
    )
    .await?;
    let Message::Hello {
        device_id,
        nickname,
        device_type,
        tcp_port,
        nonce,
        sig,
        x25519_pubkey,
        ed25519_pubkey,
        ..
    } = &first
    else {
        unreachable!("read_hello_frame 只返回 Hello");
    };
    crate::network::transport::verify_hello_for_ble(
        &state,
        device_id,
        *tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig,
    )?;
    let peer_id = device_id.clone();

    // ---- 指定拨号方判据（与 TCP 的 `should_dial` 同一条规则：**大 id 拨，小 id 只接受**）----
    //
    // 两端都同时跑 central + peripheral ⇒ 会互相拨号。若对端 id 比我大，说明它也会拨我：
    // 我拨过去建成的是一条**镜像链路**，它会把这条拒掉（不回 Hello），而这条连接的建立
    // 过程会打断它拨给我的那条好链路 —— 于是好链路 45s 收不到帧被看门狗拆掉、再重来
    // （用户 2026-09-12 实测：「点加好友：发送失败，连接已关闭」）。
    // 所以：记进"不要再拨"，并主动放弃这一条。
    //
    // `peer_advertises`：我们是在**扫描结果**里看到这个端点的（扫到 = 它在广播），
    // 但只有 `scan_loop` 真的把它记进 `ble_peer_advertises` 才算"确认能广播"。
    // 传 false（Windows 这类只做 central 的平台，或对端不广播）⇒ 无条件拨，见 `should_dial_ble`。
    let peer_advertises = state
        .ble_peer_advertises
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&ble_id);
    if !should_dial_ble(&state.device_id, &peer_id, peer_advertises) {
        state
            .ble_no_dial
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(ble_id.clone());
        state.logger.info(
            "ble",
            format!(
                "对端 {peer_id} 是这条链路的指定拨号方（id 更大）⇒ 记下不再主动拨它，避免镜像链路互扰"
            ),
        );
        // 显式断开：只 return 的话这条连接会挂着，继续占着对端 GATT server 的那个连接槽
        //（对端每次收到我们的新连接都会替换旧连接 ⇒ 正好打断它拨过来的好链路）。
        let _ = peripheral.disconnect().await;
        return Ok(());
    }

    // ---- 去重：与 TCP 入站**同一个判据**（不要在这里复制第二份"有没有同路径连接"）----
    let existing = link_snapshot(&state, &peer_id).await;
    if !should_accept_inbound_public(&state.device_id, &peer_id, PathKind::Bluetooth, &existing) {
        return Err("已有蓝牙链路（或该 peer 链路数已满），不重复建链".to_string());
    }

    // ---- 登记链路（端点 = BLE 标识，路径 = Bluetooth）----
    let (high_tx, high_rx) = mpsc::channel(1024);
    let (normal_tx, normal_rx) = mpsc::channel(1024);
    let (low_tx, low_rx) = mpsc::channel(1024);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: ep.clone(),
            path_kind: PathKind::Bluetooth,
            high: high_tx.clone(),
            normal: normal_tx.clone(),
            low: low_tx.clone(),
            cancel: cancel_tx,
        });
    register_connection(&state, &peer_id, ep.clone(), PathKind::Bluetooth);
    state.logger.info(
        "ble",
        format!(
            "[SESSION] 已就绪 peer={peer_id} 昵称={nickname:?} 类型={device_type} ep={ep}\
             （双向 Hello 已验签，transport 可用；ep 是**本机这一侧的链路标识**，\
              central 侧=对端外设标识 / 外设侧=对端 central 标识，两者不一定同串）"
        ),
    );
    state.logger.info(
        "ble",
        format!("+ble-link peer={peer_id} ep={ep}（双向 Hello 已验签）"),
    );

    // 首帧（对端 Hello）交给统一的处理路径：写身份 + 双公钥、对齐会话时钟、冲刷待发队列
    handle_message(&state, &peer_id, first).await;

    tokio::spawn(ble_writer_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        writer,
        high_rx,
        normal_rx,
        low_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));
    // ↑ 写循环泛型化：central 写 GATT 特征、外设发通知，逻辑同一份（见 `FrameSink`）
    tokio::spawn(ble_reader_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        reader,
        shutdown.clone(),
        cancel_rx,
    ));

    // 建链即冲一次待发队列（与 TCP 拨号成功后的序列一致）
    flush_outbox(&state, &peer_id).await;
    flush_group_outbox(&state, &peer_id).await;
    flush_pending_reads(&state, &peer_id).await;
    flush_pending_group_reads(&state, &peer_id).await;
    flush_pending_group_keys(&state, &peer_id).await;
    crate::commands::flush_pending_files(&state, &peer_id).await;
    crate::commands::flush_pending_group_files(&state, &peer_id).await;
    Ok(())
}

/// 在读循环之外（握手阶段）读一条完整消息：窗口内没有分片就继续等，直到超时。
async fn read_one(
    reader: &mut BleReader,
    overall: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Option<Message>, String> {
    let deadline = tokio::time::Instant::now() + overall;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        let frame = tokio::select! {
            biased;
            _ = shutdown.changed() => return Ok(None),
            res = reader.next_frame(READ_IDLE) => res?,
        };
        let Some(frame) = frame else {
            let _ = reader.gc();
            continue;
        };
        return serde_json::from_slice::<Message>(&frame)
            .map(Some)
            .map_err(|e| format!("握手帧无法解析：{e}"));
    }
}

/// 这一帧值不值得在 BLE 生命周期日志里留一行。
///
/// 为什么过滤：`Heartbeat` / `Presence` 每 5s 一条、`Gossip` 也可能是同一件事的转发；
/// 全打会把真机日志刷满，而排查"好友申请为什么几分钟才到 / 消息为什么一直发送中"
/// 需要的恰恰是**控制帧与业务帧**的收发轨迹（用户 2026-09-13 明确要求）。
fn is_logworthy_frame(msg: &Message) -> bool {
    match msg {
        Message::Heartbeat { .. } | Message::UserInfo { .. } => false,
        // Presence（节点通告）每 5s 一条，经 Gossip 承载 ⇒ 只跳过它；
        // 其它 Gossip（好友申请/同意、单聊、群聊、送达确认）都值得留痕。
        Message::Gossip { envelope } => envelope.kind != crate::protocol::GossipKind::Presence,
        _ => true,
    }
}

/// 一条帧的**可 grep 标识**：类型名 + 消息 id（如果有）。
fn frame_trace(msg: &Message) -> String {
    match msg {
        Message::Ack { msg_id } => format!("type=ack msg_id={msg_id}"),
        Message::ChatMessage { msg_id, kind, .. } => {
            format!("type=chat_message kind={kind} msg_id={msg_id}")
        }
        // 好友申请/同意走的是 `Gossip` 信封（GossipKind::FriendRequest/FriendAccept）——
        // **必须把 kind 打出来**，否则日志里只有 `type=gossip`，根本分不清
        // "好友申请到底发出去没有"（用户 2026-09-13 排查时正是卡在这里）。
        Message::Gossip { envelope } => format!("type=gossip kind={:?}", envelope.kind),
        Message::FriendRequest { from, .. } => format!("type=friend_request from={from}"),
        Message::FriendAccept { from, .. } => format!("type=friend_accept from={from}"),
        Message::FileChunk {
            transfer_id, seq, ..
        } => {
            format!("type=file_chunk transfer={transfer_id} seq={seq}")
        }
        other => format!("type={}", other.wire_kind()),
    }
}

/// 握手期间最多跳过多少个"非 Hello 的前导帧"。
///
/// 为什么要上限：既能让"上一条链路的残留帧"过去，又不能让对端无限灌帧把握手拖住
/// （每一帧都要过一遍 JSON 解析）。
const MAX_HANDSHAKE_PREAMBLE_FRAMES: u32 = 32;

/// 读**首个 Hello**：跳过并丢弃握手前导帧（见调用点的说明）。
///
/// 安全性：被丢掉的帧**绝不进入业务处理**（`handle_message` 那一层）—— 身份来自 Hello
/// 的签名验证，验签之前任何帧都只是字节。
///
/// 注：护栏 `ble_handshake_skips_leading_frames_without_processing_them` 会检查本函数体里
/// **不出现**业务入口的调用，所以这里的说明用文字描述、不写成那句调用本身。
async fn read_hello_frame(
    reader: &mut BleReader,
    overall: Duration,
    shutdown: &mut watch::Receiver<bool>,
    state: &AppState,
    ep_for_log: &str,
) -> Result<Message, String> {
    let deadline = tokio::time::Instant::now() + overall;
    let mut dropped = 0u32;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(format!(
                "握手超时：窗口内没等到 Hello（丢掉了 {dropped} 个前导帧）"
            ));
        }
        let Some(frame) = read_one(reader, left, shutdown).await? else {
            return Err(format!(
                "握手超时：对端未回 Hello（丢掉了 {dropped} 个前导帧）"
            ));
        };
        match preamble_action(dropped, matches!(frame, Message::Hello { .. })) {
            PreambleAction::Hello => {
                if dropped > 0 {
                    state.logger.info(
                        "ble",
                        format!(
                            "[SESSION] 跳过 {dropped} 个握手前导帧后收到 Hello ep={ep_for_log}"
                        ),
                    );
                }
                return Ok(frame);
            }
            PreambleAction::GiveUp => {
                return Err(format!(
                    "对端首帧不是 Hello（连续 {dropped} 帧都不是，最后一帧 type={}）",
                    frame.wire_kind()
                ));
            }
            PreambleAction::Drop => {}
        }
        dropped += 1;
        state.logger.info(
            "ble",
            format!(
                "[SESSION] 丢弃握手前导帧 type={} ep={ep_for_log}（等 Hello，已丢 {dropped}）",
                frame.wire_kind()
            ),
        );
    }
}

/// 握手前导帧的处理决定（纯函数内核，见 `read_hello_frame`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreambleAction {
    /// 收到 Hello ⇒ 进入握手。
    Hello,
    /// 还不是 Hello，但额度没用完 ⇒ 丢掉它继续等。
    Drop,
    /// 额度用尽 ⇒ 明确报错（带上最后一帧的类型，便于真机定位）。
    GiveUp,
}

/// 纯函数：`dropped` = 已经丢掉了多少个前导帧。
fn preamble_action(dropped: u32, is_hello: bool) -> PreambleAction {
    if is_hello {
        PreambleAction::Hello
    } else if dropped >= MAX_HANDSHAKE_PREAMBLE_FRAMES {
        PreambleAction::GiveUp
    } else {
        PreambleAction::Drop
    }
}

/// 写方向的抽象：BLE central 用 [`BleWriter`]（GATT client 写特征），
/// 外设角色用 [`PeripheralSink`]（GATT server 发通知）。
/// 抽出来只为让「取消息 → 序列化 → 发送 → 失败即收尾」这套逻辑**只有一份**，
/// 两个角色的差异全部收在各自的适配器里。
#[async_trait::async_trait]
trait FrameSink: Send {
    async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String>;
}

#[async_trait::async_trait]
impl FrameSink for BleWriter {
    async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String> {
        BleWriter::send_frame(self, payload).await
    }
}

/// 外设侧的一条链路 = 「发通知的句柄 + 对端 central 标识」。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
struct PeripheralSink {
    writer: PeripheralWriter,
    central: String,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
#[async_trait::async_trait]
impl FrameSink for PeripheralSink {
    async fn send_frame(&mut self, payload: &[u8]) -> Result<usize, String> {
        self.writer.send_frame(&self.central, payload).await
    }
}

/// 读方向的抽象：central 从 [`BleReader`] 取帧，外设角色从通道取帧
/// （帧在驱动的 delegate 里就已经重组好了）。
#[async_trait::async_trait]
trait FrameSource: Send {
    /// 等一条**完整帧**；`Ok(None)` = 这个窗口内没有。
    async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String>;
    /// 回收半截消息（对端半途断连时不会永久占内存）。
    fn gc(&mut self) -> usize;
    /// 分片级统计 `(本特征通知数, 字节数, 非本特征通知数)`；没有这层信息就返回 `None`。
    fn frag_stats(&self) -> Option<(u64, usize, u64)> {
        None
    }
    /// 被丢弃的分片 `(累计条数, 最近一次原因)`；没有这层信息就返回 `None`。
    ///
    /// 只有 central 侧（`BleReader`）实现了它 —— 外设侧的分片在各自的驱动里重组，
    /// 拿不到这个计数。`None` 时读循环不会打任何东西。
    fn frag_drops(&self) -> Option<(u64, &'static str)> {
        None
    }
}

/// 泛型薄封装：让读循环不必关心具体实现有没有分片统计。
fn stats_fn<S: FrameSource>(reader: &S) -> Option<(u64, usize, u64)> {
    reader.frag_stats()
}

/// 同上，取"被丢弃的分片"（诊断用）。
fn drops_fn<S: FrameSource>(reader: &S) -> Option<(u64, &'static str)> {
    reader.frag_drops()
}

#[async_trait::async_trait]
impl FrameSource for BleReader {
    async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String> {
        BleReader::next_frame(self, wait).await
    }
    fn gc(&mut self) -> usize {
        BleReader::gc(self)
    }
    fn frag_stats(&self) -> Option<(u64, usize, u64)> {
        Some(BleReader::stats(self))
    }
    fn frag_drops(&self) -> Option<(u64, &'static str)> {
        Some(BleReader::drop_stats(self))
    }
}

/// 外设侧的读方向：帧已经重组好，直接从通道拿。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
struct ChannelSource {
    rx: mpsc::Receiver<Vec<u8>>,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
#[async_trait::async_trait]
impl FrameSource for ChannelSource {
    async fn next_frame(&mut self, _wait: Duration) -> Result<Option<Vec<u8>>, String> {
        // 通道关闭 = 驱动退出（对端断开 / 蓝牙被关）⇒ 当成"链路结束"而不是"暂时没数据"
        match self.rx.recv().await {
            Some(bytes) => Ok(Some(bytes)),
            None => Err("外设链路已关闭".to_string()),
        }
    }
    fn gc(&mut self) -> usize {
        // 半截消息由驱动侧的 `BleReassembler`（带 30s TTL）负责回收
        0
    }
}

#[allow(clippy::too_many_arguments)]
/// BLE writer scheduler：High > Normal > Low 三级 channel（biased select，帧间严格优先）。
///
/// 与 TCP writer_loop 的差别在帧的代价：BLE 的 send_frame 在底层做 MTU 分片循环，
/// 一帧 4KB FileChunk 最坏（小 MTU）要发数秒。本循环的抢占粒度是**帧**：
/// High/Normal 的等待上限 = 正在发送的那一条 Low 帧完成的时间。
/// 片间 yield 需要四个平台的 FrameSink 同步改 fragment 级 API，尚未实现
/// —— 与 `dispatch` 模块头的「抢占粒度」声明保持一致，不要在注释里超前宣称。
async fn ble_writer_loop<S: FrameSink + 'static>(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut writer: S,
    mut high_rx: mpsc::Receiver<Message>,
    mut normal_rx: mpsc::Receiver<Message>,
    mut low_rx: mpsc::Receiver<Message>,
    mut shutdown: watch::Receiver<bool>,
    mut cancel: watch::Receiver<bool>,
) {
    let mut high_open = true;
    let mut normal_open = true;
    let mut low_open = true;
    loop {
        if !high_open && !normal_open && !low_open {
            break;
        }
        // 外层 select：High > Normal > Low，正常调度优先级
        let msg = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = cancel.changed() => break,
            maybe = high_rx.recv(), if high_open => maybe,
            maybe = normal_rx.recv(), if normal_open => maybe,
            maybe = low_rx.recv(), if low_open => maybe,
        };
        match msg {
            Some(msg) => {
                let trace = is_logworthy_frame(&msg).then(|| frame_trace(&msg));
                let Ok(bytes) = serde_json::to_vec(&msg) else {
                    continue;
                };
                // 写也要能被停机/判死打断（与 TCP 的 writer_loop 同一考虑）。
                //
                // ⚠️ **写失败先退避重试，不能一次就判死**（真机 2026-09-13 安卓日志）：
                // 旧实现在第一次写失败就 `break` 结束**写**循环，而**读**循环还活着 ⇒
                // 这条链路变成"能收不能发"的僵尸：上层发送永远报「连接失败/连接已关闭」，
                // 而 45s 看门狗是按**读**活性判健康的（还在收到对端的 gossip）⇒ 永远不拆 ⇒
                // 只能重启应用才恢复。BLE 的写失败大多是瞬态的（对端通知队列满 / 链路忙 /
                // 连发被拒），退避重试即可；真死了也走下面的"拆链路"而不是留个半死链路。
                let mut attempt = 1u32;
                let res = loop {
                    let one = tokio::select! {
                        biased;
                        _ = shutdown.changed() => Err("停机中".to_string()),
                        _ = cancel.changed() => Err("链路已取消".to_string()),
                        res = writer.send_frame(&bytes) => res,
                    };
                    match one {
                        Ok(n) => break Ok(n),
                        Err(e) => {
                            // 「帧无法分片」是**这一帧**的问题（太大/MTU 异常），重试无意义
                            if e.starts_with("帧无法分片") || attempt >= WRITE_RETRY_ATTEMPTS {
                                break Err(e);
                            }
                            // 帧长必须打出来：光看"写失败"无法判断是"分片太大"还是别的原因。
                            // 真机排查时这一行能直接给出「写了多少字节」。
                            state.logger.warn(
                                "ble",
                                format!(
                                    "[SEND] 写失败第 {attempt}/{WRITE_RETRY_ATTEMPTS} 次（{}ms 后重试）peer={peer_id} ep={ep} 帧长={} 原因={e}",
                                    WRITE_RETRY_WAIT.as_millis(),
                                    bytes.len()
                                ),
                            );
                            tokio::select! {
                                biased;
                                _ = shutdown.changed() => break Err("停机中".to_string()),
                                _ = cancel.changed() => break Err("链路已取消".to_string()),
                                _ = tokio::time::sleep(WRITE_RETRY_WAIT) => {}
                            }
                            attempt += 1;
                        }
                    }
                };
                // 分片数要留痕：对端会打 `[FRAG] 收到通知 N 条`，两边的数字一比就知道
                // **是发少了还是收丢了**（真机 2026-09-13：742B 的帧需要 53 片，对端只到 38 片）。
                if let (Ok(n), Some(trace)) = (&res, trace.as_deref()) {
                    state.logger.info(
                        "ble",
                        format!(
                            "[SEND] {trace} → peer={peer_id} ep={ep} bytes={} 分片={n}",
                            bytes.len()
                        ),
                    );
                }
                // 文件分块**真的写出去了**才算进展（发送侧等 FileCompleteAck 的判据，
                // 见 `transport.rs::mark_file_wire_progress` 的注释）。BLE 上这一步尤其关键：
                // 一个 4KiB 文件块要 399 片 × 12ms ≈ 5.5s，判据必须落在"写出去"上。
                if res.is_ok() {
                    mark_file_wire_progress(&state, &msg, &peer_id);
                }
                if let Err(e) = &res {
                    // 「帧无法分片」是**这一帧**太大/MTU 异常，不是链路坏了：拆链路会让
                    // 同一条连接上的其它传输全部失败（真机：一张大图把链路打死，之后的好友
                    // 请求/消息全断）。这里只丢这一帧并留 warn —— 上层 outbox 会按自己的节奏
                    // 重发；真正的写失败（对端走了）仍然拆链路。
                    if e.starts_with("帧无法分片") {
                        state.logger.warn(
                            "ble",
                            format!(
                                "[SEND] 丢弃无法分片的帧（链路保留）peer={peer_id} ep={ep} bytes={} 原因={e}",
                                bytes.len()
                            ),
                        );
                        continue;
                    }
                    state.logger.warn(
                        "ble",
                        format!(
                            "[SEND] 写失败（已重试 {WRITE_RETRY_ATTEMPTS} 次）⇒ 拆掉该链路并等待重拨 peer={peer_id} ep={ep} {} 原因={e}",
                            trace.as_deref().unwrap_or("type=?")
                        ),
                    );
                    // ⚠️ **必须连读循环一起取消**（`teardown_link` 在读循环收尾里）：
                    // 只结束写循环会留下"能收不能发"的僵尸链路，而看门狗按读活性判健康、
                    // 永远不拆它 ⇒ 用户只能重启（真机 2026-09-13）。
                    // 拆掉之后 `teardown_link` 会清该地址的退避并 `wake_scan`，
                    // 下一轮扫描即可重拨。
                    // 写循环手里只有 `cancel` 的**接收端**，所以要按端点去链路表里
                    // 找这一条的取消发送端（与看门狗同一套"按端点定位"的写法）。
                    {
                        let links = state.links.lock().await;
                        if let Some(l) = links
                            .get(&peer_id)
                            .and_then(|v| v.iter().find(|l| l.endpoint == ep))
                        {
                            let _ = l.cancel.send(true);
                        }
                    }
                    break;
                }
            }
            None => {
                // select 的某臂被禁用（对端 Sender 全部 drop ⇒ recv 立即 None 且永久 None）。
                // ⚠️ 三个臂都要判：漏掉 high 会让本循环以 100% CPU 空转且永不退出
                //（链路被摘 ⇒ Link drop ⇒ high sender 归零 ⇒ 该臂每一轮都命中）。
                if high_rx.is_closed() {
                    high_open = false;
                }
                if normal_rx.is_closed() {
                    normal_open = false;
                }
                if low_rx.is_closed() {
                    low_open = false;
                }
            }
        }
    }
}

async fn ble_reader_loop<S: FrameSource + 'static>(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut reader: S,
    mut shutdown: watch::Receiver<bool>,
    mut cancel: watch::Receiver<bool>,
) {
    // 分片统计只在 central 侧（`BleReader`）有意义；外设侧没有这个计数。
    let mut last_frag_n: u64 = 0;
    let mut last_frag_other: u64 = 0;
    let mut last_drop_n: u64 = 0;
    // 退出原因：P0 可观测性 —— 每条链路拆的时候都必须知道是"停机"、"cancel 信号"
    // 还是"读循环错误"，因为 cancel 信号本身可能来自三条不同路径
    // （写失败 / 看门狗 stale / adapter events 被动断链）。
    #[allow(unused_assignments)]
    let mut exit_reason: &'static str = "unknown";
    loop {
        let frame = tokio::select! {
            biased;
            _ = shutdown.changed() => { exit_reason = "shutdown"; break },
            _ = cancel.changed() => { exit_reason = "link_canceled"; break },
            res = reader.next_frame(READ_IDLE) => res,
        };
        match frame {
            Ok(Some(bytes)) => match serde_json::from_slice::<Message>(&bytes) {
                Ok(msg) => {
                    if is_logworthy_frame(&msg) {
                        state.logger.info(
                            "ble",
                            format!(
                                "[RECV] {} ← peer={peer_id} ep={ep} bytes={}",
                                frame_trace(&msg),
                                bytes.len()
                            ),
                        );
                    }
                    // ⚠️ **读到帧 = 这条链路还活着**，必须回灌 mesh 健康度（2026-09-13 审计）。
                    //
                    // 漏掉这一句的后果（真机体感就是"蓝牙时好时坏、延迟很高"）：
                    // `ConnectionHealth` 的读活性只在**建链时播种一次**
                    // （`transport.rs::register_connection` → `seed_connection_read_seen`），
                    // 此后只由 `reader_loop` 刷新 —— 而 BLE 的读循环原来没有调用它。
                    // 于是任何健康的 BLE 链路：15s 后 `is_healthy` 判假（选路/镜像去重都会
                    // 按"不健康"处理），45s 被健康看门狗（`stale_connections`）当作死链路
                    // **拆掉**；对端再拨回来，45s 后再拆一次，无限循环。
                    // TCP 侧的对应调用见 `transport.rs` 的 `reader_loop`。
                    // 本函数同时服务 central（`BleReader`）与外设（`ChannelSource`）两条路径，
                    // 所以一处调用两个方向都覆盖。
                    mark_conn_seen(&state, &peer_id, &ep);
                    handle_message(&state, &peer_id, msg).await
                }
                Err(e) => state
                    .logger
                    .warn("ble", format!("丢弃无法解析的 BLE 帧 peer={peer_id}: {e}")),
            },
            Ok(None) => {
                // 窗口内没有分片：顺手回收半截消息（对端在半途断连时不会永久占内存）
                let _ = reader.gc();
                // **分片级可见性**：真机里"对端说发了、这边什么都没收到"时，
                // 这条日志能一眼区分"没发出来"与"发出来但没到"。
                if let Some((n, bytes, other)) = stats_fn(&reader) {
                    if n != last_frag_n || other != last_frag_other {
                        state.logger.info(
                            "ble",
                            format!(
                                "[FRAG] 收到通知 {n} 条 / {bytes} 字节（非本特征 {other} 条）                                 ← peer={peer_id} ep={ep}"
                            ),
                        );
                        last_frag_n = n;
                        last_frag_other = other;
                    }
                }
                // **被丢弃的分片**：坏片原来在重组器里被静默吞掉 —— 真机上只看到
                // "图片没到"，看不到"到了、被分片层丢了、原因是…"。这里只在计数**增加**时
                // 打一条 warn（不是每片一条），所以不会刷屏。
                if let Some((drops, reason)) = drops_fn(&reader) {
                    if drops != last_drop_n {
                        state.logger.warn(
                            "ble",
                            format!(
                                "[FRAG] 丢弃分片 {drops} 片（新增 {}，最近原因：{reason}）                                 ← peer={peer_id} ep={ep}",
                                drops - last_drop_n
                            ),
                        );
                        last_drop_n = drops;
                    }
                }
            }
            Err(e) => {
                state
                    .logger
                    .info("ble", format!("BLE 读结束 peer={peer_id} ep={ep}: {e}"));
                exit_reason = "reader_error";
                break;
            }
        }
    }
    // 收尾：只拆这一条（同一 peer 可能还有 LAN 链路）
    teardown_link(&state, &peer_id, &ep, exit_reason).await;
}

// ===========================================================================
//                        外设（GATT server）方向
// ===========================================================================
//
// 与上面 central 方向的**唯一**区别是"谁先连谁"：
//   * central（digits 上面那段）：我们扫 → 我们连 → 我们发 Hello → 等对端 Hello；
//   * 外设（本段）：对端连我们 → 对端发 Hello（首帧）→ 我们验签 → **我们回 Hello**。
// 其余全部相同：同一个 `verify_hello_for_ble`、同一个 `should_accept_inbound_public`
// 去重判据、同一份 `Link`/`PathKind::Bluetooth` 登记、同一套冲刷序列。
// 因此下面没有第二套身份/信任判断 —— 身份**只能**由双向 Hello 验签建立。

/// 外设事件循环收到的路由控制消息（握手成功后把"往这个 central 投帧"的管道交给循环）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
enum RouteCtl {
    Add {
        central: String,
        tx: mpsc::Sender<Vec<u8>>,
    },
    /// 握手**失败**收尾 ⇒ 从 `handshaking` 里摘掉这个 central，允许它再次触发握手。
    ///
    /// 为什么必须有（2026-09-13 审计抓到的"加入不了 mesh"缺陷）：旧实现只在
    /// `Add`（握手成功）与 `Unlinked`（对端退订）时清理 `handshaking`，
    /// 而**握手失败**（对端根本不是 Gosslan 端、Hello 验签不过、首帧异常…）时**不清理**
    /// ⇒ 那个 central 之后发来的**真 Hello 会被「已在握手」静默丢弃** ⇒ 设备再也进不来。
    /// macOS 外设没有断连回调（`Unlinked` 不一定到），这个条目可能**永久残留**。
    ///
    /// ⚠️ 只在失败时发：成功路径由 `Add` 清理；若成功也发，会与"刚起来的第二次握手"
    /// 抢同一个标记（把新握手的 `handshaking` 误清 ⇒ 同一 central 叠起多条握手）。
    HandshakeFailed { central: String },
}

/// 把外设事件队列里**已经入队**的 `Notice`/`Warning` 逐条落日志。
///
/// 为什么需要它：外设角色**为什么起不来**（权限缺失 / 蓝牙没开 / 本机不支持广播 /
/// GATT server 打不开……）是 Kotlin / CoreBluetooth 侧经 `Notice`/`Warning` 事件上报的，
/// 它们落在 `server.events` 这条队列里。而这条队列**唯一**的消费点是外设接收循环
/// （`peripheral_accept_loop` 里的 `server.events.recv()`），启动失败时那个循环根本不会起来
/// —— 直接 `stop()` 会把队列连同原因一起丢掉，用户最终只看到一句自指的
/// 「Android BLE 外设未能启动（详见日志中的具体原因）」，而日志里并没有那个原因。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn drain_peripheral_events(server: &mut peripheral::PeripheralServer, state: &AppState) {
    while let Ok(ev) = server.events.try_recv() {
        match ev {
            PeripheralEvent::Warning(text) => state.logger.warn("ble", text),
            PeripheralEvent::Notice(text) => state.logger.info("ble", text),
            // 启动都没成功，不可能有帧/断链事件；真出现也只说明状态机不对，不值得为它编文案
            PeripheralEvent::Frame { .. } | PeripheralEvent::Unlinked { .. } => {}
        }
    }
}

/// 停外设并把队列排空落日志。
///
/// ⚠️ `stop()` **前后各排一次**，缺一不可：原因来自两处 ——
/// 启动期间上报的 `Notice`/`Warning`（先入队），以及 `stop()` 自身失败时塞进来的
/// Warning（见 `PeripheralServer::stop`：停不掉意味着可能还在广播，必须留痕）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn stop_peripheral_and_drain(server: &mut peripheral::PeripheralServer, state: &AppState) {
    drain_peripheral_events(server, state);
    server.stop();
    drain_peripheral_events(server, state);
}

/// 启动外设角色。失败只记日志：能扫别人但别人连不上我们，属于**降级**而不是故障，
/// 不该把整个蓝牙开关判为不可用（LAN 更不受影响）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn start_peripheral(state: Arc<AppState>, shutdown: watch::Receiver<bool>) {
    let mut startup = match peripheral::start() {
        Ok(startup) => startup,
        Err(e) => {
            state.logger.warn(
                "ble",
                format!("蓝牙外设角色未启动（central 角色不受影响，仍可主动连别人）：{e}"),
            );
            return;
        }
    };
    // 等 CoreBluetooth 上报状态：把"未授权 / 蓝牙关着 / 广播失败"变成一条**说得清**的错误。
    // 超时不致命（系统可能只是还没上报），此时按"已启动"继续。
    match tokio::time::timeout(peripheral::STATE_WAIT, startup.state).await {
        Ok(Ok(Ok(()))) => state.logger.info(
            "ble",
            "蓝牙外设角色已启动（广播服务 UUID，等待手机/PC 连入）",
        ),
        Ok(Ok(Err(e))) => {
            state.logger.warn(
                "ble",
                format!("蓝牙外设角色不可用（central 角色不受影响）：{e}"),
            );
            stop_peripheral_and_drain(&mut startup.server, &state);
            return;
        }
        Ok(Err(_)) => {
            state
                .logger
                .warn("ble", "蓝牙外设角色的状态回调通道被关闭，放弃启动");
            stop_peripheral_and_drain(&mut startup.server, &state);
            return;
        }
        Err(_) => state.logger.info(
            "ble",
            "蓝牙外设角色已启动（未在 3s 内收到状态回调，继续广播）",
        ),
    }
    tokio::spawn(peripheral_accept_loop(state, startup.server, shutdown));
}

/// 外设侧的总循环：把每个 central 的帧分派给它的链路任务，首帧走握手。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn peripheral_accept_loop(
    state: Arc<AppState>,
    mut server: peripheral::PeripheralServer,
    mut shutdown: watch::Receiver<bool>,
) {
    // central 标识 → 该链路的帧管道；`handshaking` 防止同一个 central 触发多次握手
    let mut routes: HashMap<String, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut handshaking: HashSet<String> = HashSet::new();
    let (route_tx, mut route_rx) = mpsc::channel::<RouteCtl>(16);

    loop {
        let ev = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            Some(ctl) = route_rx.recv() => {
                match ctl {
                    RouteCtl::Add { central, tx } => {
                        handshaking.remove(&central);
                        routes.insert(central, tx);
                    }
                    // 握手失败 ⇒ 解除"握手中"标记（否则这个 central 的真 Hello 永远被丢）
                    RouteCtl::HandshakeFailed { central } => {
                        handshaking.remove(&central);
                    }
                }
                continue;
            }
            maybe = server.events.recv() => match maybe {
                Some(ev) => ev,
                None => break,
            },
        };

        match ev {
            PeripheralEvent::Frame { central, bytes } => {
                // 先判「投旧链路 还是 重新握手」（判据与理由见 `peripheral_route_action`）：
                // 同一个 central 地址的**重连**发来的 Hello 绝不能被投给旧链路的管道,
                // 否则新连接永远收不到 Hello 回应（对端表现为"握手超时"/"首帧不是 Hello"）。
                let has_route = routes.contains_key(&central);
                let action =
                    peripheral_route_action(has_route, has_route && frame_is_hello(&bytes));
                let mut pending = Some(bytes);
                if action == PeripheralRouteAction::ToHandshake && has_route {
                    routes.remove(&central);
                    state.logger.info(
                        "ble",
                        format!("外设侧收到新连接的 Hello（central={central}）⇒ 换路由并重新握手"),
                    );
                }
                if action == PeripheralRouteAction::ToExistingLink {
                    if let Some(tx) = routes.get(&central).cloned() {
                        match tx.send(pending.take().expect("pending 刚被设置")).await {
                            Ok(()) => continue,
                            Err(e) => {
                                pending = Some(e.0);
                                routes.remove(&central);
                                state.logger.info(
                                    "ble",
                                    format!("外设侧旧链路已失效，按重连处理 central={central}"),
                                );
                            }
                        }
                    }
                }
                let bytes = pending.expect("未投递的帧必须还在");
                // 外设侧的同一类问题：新连接的"第一帧"可能仍是上一条链路的残留业务帧
                // （Android 的 notify 按 central 地址投递）。这里**丢掉**它并等 Hello ——
                // 既不能投给旧路由（那条链路已死），也不能当握手首帧（会立刻失败）。
                if action == PeripheralRouteAction::ToHandshake && !frame_is_hello(&bytes) {
                    let kind = serde_json::from_slice::<Message>(&bytes)
                        .map(|m| m.wire_kind())
                        .unwrap_or_else(|_| "无法解析".to_string());
                    state.logger.info(
                        "ble",
                        format!(
                            "[SESSION] 丢弃外设侧握手前导帧 type={kind} central={central}（等 Hello）"
                        ),
                    );
                    continue;
                }
                if handshaking.insert(central.clone()) {
                    let st = state.clone();
                    let wrt = server.writer.clone();
                    let ctl_tx = route_tx.clone();
                    let failed_central = central.clone();
                    // ⚠️ `shutdown` 必须在**进入 async move 之前**克隆：它是循环外的
                    // `watch::Receiver`，循环顶部的 `select!` 每轮都要用；
                    // 让 `async move` 直接捕获它会把它搬出循环（E0382）。
                    let sd = shutdown.clone();
                    tokio::spawn(async move {
                        let ok =
                            accept_handshake(st, wrt, central, bytes, ctl_tx.clone(), sd).await;
                        // ⚠️ **只在失败时**解除"握手中"标记（成功路径由 `RouteCtl::Add` 解除）。
                        // 失败不解标记 ⇒ 这个 central 的真 Hello 永远被丢 ⇒ 设备再也加入不进来
                        // （macOS 外设没有断连回调，条目可能永久残留）。
                        if !ok {
                            let _ = ctl_tx
                                .send(RouteCtl::HandshakeFailed {
                                    central: failed_central,
                                })
                                .await;
                        }
                    });
                }
            }
            PeripheralEvent::Unlinked { central } => {
                // ⚠️ 真机 2026-09-13 第五轮：**"取消订阅"不能当成"断开"立刻摘链路**。
                //
                // 现象（用户三台设备：手机 + Mac + Windows，Android 侧日志）：
                //   `外设侧对端取消订阅（视为断开）central=34:13:E8:90:51:B3`
                //   每 1~2 秒一条，**14 分钟刷了几百次** —— 而那个地址是 Mac。
                // 旧行为：每一条都立刻 `handshaking.remove` + `routes.remove` +
                // `detach_by_endpoint` ⇒ 事件循环被这条洪水灌满，**同一时刻正在握手的
                // 另一台设备（Windows，17:50:16 已连上、MTU=517 都协商完了）被挤掉**，
                // 永远走不到 `[SESSION] 已就绪（外设侧）`。用户看到的仍是"互相搜不到"。
                //
                // GATT 语义本来就允许"取消订阅"与"断开"是两件事（两者都会走这个回调），
                // 所以这里按**有没有待给的链路**分流：
                //   · 没有已登记链路（还在握手 / 刚连上）⇒ 只清握手标记、**绝不动路由**，
                //     那条链路让握手自己去完成或超时收尾；
                //   · 已有链路 ⇒ 才是真的断开，按原逻辑摘掉。
                let established = {
                    let links = state.links.lock().await;
                    links.values().flatten().any(|l| {
                        l.path_kind == PathKind::Bluetooth
                            && matches!(
                                &l.endpoint,
                                MeshEndpoint::Ble(b)
                                    if b.address.eq_ignore_ascii_case(&central)
                            )
                    })
                };
                handshaking.remove(&central);
                if !established {
                    // 只留痕、**不摘路由**：拒绝把"握手中的链路"误伤掉
                    state.logger.info(
                        "ble",
                        format!(
                            "外设侧取消订阅 central={central}（尚无已登记链路 ⇒ 视为重连前的噪声，保留路由与握手）"
                        ),
                    );
                    continue;
                }
                routes.remove(&central);
                state
                    .logger
                    .info("ble", format!("外设侧已登记链路断开 central={central}"));
                let ep = MeshEndpoint::Ble(BleEndpoint::new(central));
                detach_by_endpoint(&state, &ep, "peripheral_unlinked").await;
            }
            // 驱动侧的诊断/告警：**必须**记进日志 —— 蓝牙在真机上出问题时，
            // 这是用户唯一能贴给我们的线索（"开着蓝牙却没人能发现我们"就是这类）。
            PeripheralEvent::Notice(text) => state.logger.info("ble", text),
            PeripheralEvent::Warning(text) => state.logger.warn("ble", text),
        }
    }

    server.stop();
    state.logger.info("ble", "蓝牙外设角色已停止广播");
}

/// 按 BLE 端点摘链路（外设侧只知道 central 标识，peer_id 要反查）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn detach_by_endpoint(state: &Arc<AppState>, ep: &MeshEndpoint, reason: &'static str) {
    let peer = {
        let links = state.links.lock().await;
        links
            .iter()
            .find(|(_, v)| v.iter().any(|l| &l.endpoint == ep))
            .map(|(p, _)| p.clone())
    };
    if let Some(peer) = peer {
        teardown_link(state, &peer, ep, reason).await;
    }
}

/// 外设侧握手的外壳：失败一律**只记日志**（对端可能只是路过、或者根本不是 Gosslan 端）。
///
/// 返回值 = 是否**真的建链成功**（`try_accept_handshake` 已发出 `RouteCtl::Add`）。
/// 调用方据此决定要不要解除 `handshaking` 标记 —— 见 `RouteCtl::HandshakeFailed` 的注释。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn accept_handshake(
    state: Arc<AppState>,
    writer: PeripheralWriter,
    central: String,
    first_bytes: Vec<u8>,
    route_tx: mpsc::Sender<RouteCtl>,
    shutdown: watch::Receiver<bool>,
) -> bool {
    match try_accept_handshake(&state, writer, &central, first_bytes, route_tx, shutdown).await {
        Ok(()) => true,
        Err(e) => {
            state
                .logger
                .info("ble", format!("外设侧未建链 central={central}：{e}"));
            false
        }
    }
}

/// 真身：验签对端 Hello → 回我们的 Hello → 登记链路 → 起收发。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn try_accept_handshake(
    state: &Arc<AppState>,
    writer: PeripheralWriter,
    central: &str,
    first_bytes: Vec<u8>,
    route_tx: mpsc::Sender<RouteCtl>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ep = MeshEndpoint::Ble(BleEndpoint::new(central.to_string()));

    // **外设侧的 MTU 同样必须留痕**（2026-09-13 审计）：它决定"我们发通知时每片能塞多少字节"，
    // 与 central 侧的写方向是两个独立的值（对端可能协商出不同结果）。
    // 真机"手机→电脑传得慢/传不完"时，第一件事就是比这两条日志。
    let mtu_budget = writer.payload_mtu(central);
    let (net_bytes, kbps) = ble_throughput_estimate(mtu_budget);
    state.logger.info(
        "ble",
        format!(
            "[GATT] 外设侧 MTU 协商结果 central={central} 每片有效载荷={mtu_budget} 字节（净数据={net_bytes}；按 12ms/片估算 ≈ {kbps:.1} KB/s）"
        ),
    );

    // ---- 1. 首帧必须是 Hello，且签名必须验过（BLE 地址不是身份）----
    let first: Message =
        serde_json::from_slice(&first_bytes).map_err(|e| format!("对端首帧无法解析：{e}"))?;
    let Message::Hello {
        device_id,
        tcp_port,
        nonce,
        sig,
        x25519_pubkey,
        ed25519_pubkey,
        ..
    } = &first
    else {
        return Err(format!(
            "外设侧首帧不是 Hello（收到 {}，central={central}）",
            first.wire_kind()
        ));
    };
    crate::network::transport::verify_hello_for_ble(
        state,
        device_id,
        *tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig,
    )?;
    let peer_id = device_id.clone();

    // ---- 2. 同一个 BLE 端点的旧链路让位 ----
    // CoreBluetooth 的外设角色**没有**"central 断开"回调（只有取消订阅），
    // 所以旧链路可能早就死了而我们还留着它；对端重新连上来时必须由新链路取代，
    // 否则这个 central 会永远撞在 `should_accept_inbound_public` 上、彻底连不进来。
    detach_by_endpoint(state, &ep, "new_connection_displace").await;

    // ---- 3. 链路数上限仍然要守（防无界增长），但**同路径的镜像链路要放行** ----
    //
    // 为什么不能像 TCP 那样"按同路径去重直接拒"（4.2.6 的教训，用户真机日志）：
    // BLE 上两端都跑 central+peripheral，小 id 那一侧（Mac）会不停来拨我们；
    // 如果我们**在回 Hello 之前**就拒掉，它就**永远学不到对端 device_id** ⇒
    // 也就永远进不了它自己的"不要再拨"名单 ⇒ 每 13s 重拨一次，
    // 而**每次连接都会打断我们拨过去的那条好链路**（Android GATT server 对同一 central
    // 的新连接会替换旧的）⇒ 好链路 45s 收不到帧被看门狗拆掉 ⇒ 加好友时"连接已关闭"。
    // 所以：**让它握手成功**，它拿到 device_id 后会自己判"我比你小 ⇒ 该你拨我"并把这条
    // 镜像链路收掉（`dial_and_register` 里的 `should_dial_ble`）。一次性打扰，换来永久安静。
    let existing = link_snapshot(state, &peer_id).await;
    if !should_accept_inbound_public(&state.device_id, &peer_id, PathKind::Bluetooth, &existing)
        && existing.len() >= crate::network::transport::MAX_LINKS_PER_PEER
    {
        return Err("该 peer 链路数已满，不重复建链".to_string());
    }
    if existing
        .iter()
        .any(|(_, k, healthy)| *k == PathKind::Bluetooth && *healthy)
    {
        state.logger.info(
            "ble",
            format!("对端 {peer_id} 已有蓝牙链路，这条是镜像入站 —— 仍然完成握手，好让它自己退让"),
        );
    }

    // ---- 4. 路由先就位，再回 Hello ----
    // 对端收到我们的 Hello 后会**立刻**开始冲刷待发队列；路由早一步挂上，
    // 那批帧才不会被"注册还没完成"的缝隙吞掉（通道有缓冲，读者随后就来）。
    let (frame_tx, frame_rx) = mpsc::channel::<Vec<u8>>(1024);
    route_tx
        .send(RouteCtl::Add {
            central: central.to_string(),
            tx: frame_tx,
        })
        .await
        .map_err(|_| "外设事件循环已退出".to_string())?;

    // ---- 5. 回我们的 Hello（对端正卡在 10s 超时里等它）----
    if shutdown.borrow().to_owned() {
        return Err("蓝牙通道正在停止".to_string());
    }
    let hello = build_signed_hello(state, 0);
    let bytes = serde_json::to_vec(&hello).map_err(|e| format!("Hello 序列化失败：{e}"))?;
    writer
        .send_frame(central, &bytes)
        .await
        .map_err(|e| format!("回 Hello 失败：{e}"))?;

    // ---- 6. 登记链路（端点 = BLE central 标识，路径 = Bluetooth）----
    let (high_tx, high_rx) = mpsc::channel(1024);
    let (normal_tx, normal_rx) = mpsc::channel(1024);
    let (low_tx, low_rx) = mpsc::channel(1024);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: ep.clone(),
            path_kind: PathKind::Bluetooth,
            high: high_tx.clone(),
            normal: normal_tx.clone(),
            low: low_tx.clone(),
            cancel: cancel_tx,
        });
    register_connection(state, &peer_id, ep.clone(), PathKind::Bluetooth);
    // 对端能连上我们 ⇒ 之前"我拨不上它"的失败计数已经过期，必须清掉。
    // 不清的话：唯一的拨号方（大 id/不能被拨入的那侧）会被自己的退避锁住，
    // 而它恰恰是断线后唯一会重连的一方（真机表现：好友申请等几分钟）。
    clear_ble_dial_failure(state, central);
    state.logger.info(
        "ble",
        format!("[SESSION] 已就绪（外设侧）peer={peer_id} ep={ep}（双向 Hello 已验签）"),
    );
    state.logger.info(
        "ble",
        format!("+ble-link(外设) peer={peer_id} ep={ep}（双向 Hello 已验签）"),
    );

    // 对端 Hello 交给统一处理路径：写身份 + 双公钥、对齐会话时钟、冲刷待发队列
    handle_message(state, &peer_id, first).await;

    tokio::spawn(ble_writer_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        PeripheralSink {
            writer,
            central: central.to_string(),
        },
        high_rx,
        normal_rx,
        low_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));
    tokio::spawn(ble_reader_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        ChannelSource { rx: frame_rx },
        shutdown,
        cancel_rx,
    ));

    // 建链即冲一次待发队列（与 central / TCP 拨号成功后的序列完全一致）
    flush_outbox(state, &peer_id).await;
    flush_group_outbox(state, &peer_id).await;
    flush_pending_reads(state, &peer_id).await;
    flush_pending_group_reads(state, &peer_id).await;
    flush_pending_group_keys(state, &peer_id).await;
    crate::commands::flush_pending_files(state, &peer_id).await;
    crate::commands::flush_pending_group_files(state, &peer_id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

    /// MTU 日志里的吞吐估算必须与"净数据 / 12ms"一致，并钉住两个真实协商值。
    ///
    /// 这不是"测一个数学函数"：它是给用户的**量级预期**（UI 提示、真机排查日志）
    /// 的唯一计算来源，算错一个量级就会误导排障（旧的 1KB/s 注释就是例子）。
    #[test]
    fn throughput_estimate_matches_real_mtu_budgets() {
        // MTU 23（默认）→ ATT 预算 20 → 净 14 → 约 1.14 KB/s
        let (net, kbps) = ble_throughput_estimate(20);
        assert_eq!(net, 14, "净数据必须扣掉 6 字节分片头");
        assert!(
            (kbps - 1.14).abs() < 0.05,
            "MTU23 应约 1.1 KB/s，实际 {kbps}"
        );
        // 真机日志（Android central）：预算 514 → 净 508 → 约 41 KB/s
        let (net, kbps) = ble_throughput_estimate(514);
        assert_eq!(net, 508);
        assert!(
            (kbps - 41.3).abs() < 0.5,
            "MTU517 应约 41 KB/s，实际 {kbps}"
        );
        // 病态输入：不得 panic、不得出现下溢
        assert_eq!(ble_throughput_estimate(0).0, 0);
        assert_eq!(ble_throughput_estimate(6).0, 0);
    }

    /// **握手失败必须解除"握手中"标记**（2026-09-13 审计的"加入不了 mesh"缺陷）。
    ///
    /// 旧实现只在 `RouteCtl::Add`（成功）与 `Unlinked`（对端退订）时清理 `handshaking`，
    /// 握手**失败**时不清理 ⇒ 该 central 之后的**真 Hello 被「已在握手」静默丢弃** ⇒
    /// 设备再也连不进来；macOS 外设没有断连回调，条目可能永久残留。
    ///
    /// 为什么用源码断言：外设事件循环需要真实 BLE 栈（射频 + GATT server），单测跑不了；
    /// 但"失败路径有没有回传 `HandshakeFailed`"是纯静态事实 —— 而漏了它
    /// **不会让任何测试失败**，只会让真机上表现为"第一次没连上就永远连不上"。
    #[test]
    fn peripheral_handshake_failure_clears_the_handshaking_mark() {
        let src = include_str!("ble.rs");
        let start = src
            .find("async fn peripheral_accept_loop")
            .expect("必须还有 peripheral_accept_loop（本护栏锚点）");
        let body = &src[start..];
        let end = body.find("\n}\n").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains("RouteCtl::HandshakeFailed"),
            "外设事件循环必须在**握手失败**时回传 RouteCtl::HandshakeFailed —— \
             少了它，那台设备之后的真 Hello 会被『已在握手』永久丢弃，再也加入不进 mesh"
        );
        assert!(
            body.contains("if !ok"),
            "必须只在**失败**时回传 HandshakeFailed（成功路径由 RouteCtl::Add 清理，\
             成功也回传会与刚起来的第二次握手抢同一个标记）"
        );
    }

    /// **拨号退避不能把重试饿死**，但也不能无限敲（两轮真机 + 合并评审）。
    ///
    /// 真机日志：`跳过候选 … 原因=退避中 剩余=7849ms` 占了近一半的日志行 ——
    /// 扫描周期已经是 2s，而退避是 5s→10s→20s→40s ⇒ **大部分轮次根本不去连**。
    /// 用户看到的"搜不出来"，很大一部分是我们自己不去试。
    ///
    /// "别打扰对端"这件事已经由 `DialGuard` 在途去重保证（同一对端不会叠连接，
    /// 那是真机第一轮踩出来的 bug），所以前几次**不退避**是安全的。
    ///
    /// 但"之后固定 5s"偏激进（合并评审指出）：对端长期不在时会一直每轮都敲，
    /// 而射频/功耗的代价**没有用户可见反馈**。改成正缓增+封顶。
    #[test]
    fn dial_backoff_does_not_starve_retries() {
        // 前 3 次失败：立刻可再试（不等于"忙等" —— 节奏由 2s 的扫描周期决定）
        for n in 1..=3 {
            assert_eq!(
                ble_dial_backoff_ms(n),
                0,
                "第 {n} 次失败不该有冷却，否则重试被自己的退避饿死"
            );
        }
        // 之后缓增：5s → 10s → 20s
        assert_eq!(ble_dial_backoff_ms(4), 5_000, "第 4 次失败 = 5s");
        assert_eq!(ble_dial_backoff_ms(5), 10_000, "第 5 次失败 = 10s");
        assert_eq!(ble_dial_backoff_ms(6), 20_000, "第 6 次失败 = 20s");
        // 封顶 20s：**绝不允许指数增长到分钟级**（那正是"好友申请等几分钟"的成因）
        for n in 7..200 {
            assert_eq!(
                ble_dial_backoff_ms(n),
                20_000,
                "第 {n} 次失败必须封顶 20s（指数退避会把暂时性失败变成分钟级等待）"
            );
        }
        // 上限必须远小于旧值 60s
        for n in 1..200 {
            assert!(
                ble_dial_backoff_ms(n) <= 20_000,
                "冷却 {0}ms 过大 ⇒ 用户点「扫描」也不会立刻重试",
                ble_dial_backoff_ms(n)
            );
        }
        // 单调不降：冷却不许忽长忽短（否则"退避中"的剩余时间会在日志里来回跳）
        let mut prev = 0;
        for n in 1..40 {
            let cur = ble_dial_backoff_ms(n);
            assert!(cur >= prev, "第 {n} 次的冷却比上一档还小：{prev} → {cur}");
            prev = cur;
        }
    }

    /// **握手必须容忍前导帧**（真机 2026-09-13 的真因之一）。
    ///
    /// 日志证据：`[GATT] 已就绪 → [DISCONNECT] 对端首帧不是 Hello（收到 chat_message）`，
    /// 反复出现 ⇒ 链路永久建不起来（双方各自重拨、互相打断）。
    /// 原因是 Android 的 notify 按 **central 地址**投递：上一条链路的待发帧会落在新连接上。
    #[test]
    fn handshake_tolerates_leading_non_hello_frames_but_is_bounded() {
        // Hello 一到就进握手（无论之前丢过几个）
        assert_eq!(preamble_action(0, true), PreambleAction::Hello);
        assert_eq!(preamble_action(7, true), PreambleAction::Hello);
        // 非 Hello：额度内丢掉继续等
        assert_eq!(
            preamble_action(0, false),
            PreambleAction::Drop,
            "非 Hello 且额度未用尽必须丢弃 —— 额度过小会把残留帧当失败，链路又建不起来"
        );
        assert_eq!(
            preamble_action(MAX_HANDSHAKE_PREAMBLE_FRAMES - 1, false),
            PreambleAction::Drop
        );
        // 额度用尽：明确失败（不能无限被灌帧拖住）
        assert_eq!(
            preamble_action(MAX_HANDSHAKE_PREAMBLE_FRAMES, false),
            PreambleAction::GiveUp
        );
        assert!(
            MAX_HANDSHAKE_PREAMBLE_FRAMES >= 8,
            "额度过小会让残留帧把链路打死"
        );
    }

    /// **重连判据**：同一个 central 地址的**新连接**发来的 Hello，绝不能被投给旧链路。
    ///
    /// 真机（2026-09-12）：旧链路的写句柄指向旧连接 ⇒ 新连接收不到 Hello 回应，
    /// 对端报「握手超时：对端未回 Hello」，或者旧链路把 Hello 当普通帧吃掉 ⇒
    /// 对端报「对端首帧不是 Hello」。用户看到的是"蓝牙时好时坏、加好友没反应"。
    ///
    /// 门控：`peripheral_route_action` / `frame_is_hello` / `HELLO_PEEK_MAX_BYTES`
    /// 只存在于**有外设角色**的平台（macOS / Android）—— Windows 这一轮只做 central，
    /// 这些符号不会编译出来，测试也必须跟着门控，否则 Windows 上 `cargo test` 直接编不过。
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    #[test]
    fn reconnect_hello_must_not_go_to_the_stale_route() {
        assert_eq!(
            peripheral_route_action(true, true),
            PeripheralRouteAction::ToHandshake,
            "有活路由 + 收到 Hello ⇒ 一定是重连，必须换路由重新握手"
        );
        // 三条对照：普通数据帧仍走旧链路；没有路由时一律走握手分支（与旧行为一致）
        assert_eq!(
            peripheral_route_action(true, false),
            PeripheralRouteAction::ToExistingLink
        );
        assert_eq!(
            peripheral_route_action(false, true),
            PeripheralRouteAction::ToHandshake
        );
        assert_eq!(
            peripheral_route_action(false, false),
            PeripheralRouteAction::ToHandshake
        );
    }

    /// `frame_is_hello` 必须**真的认得出 Hello**，且不把大分片当 Hello 去解析。
    /// 门控同 `reconnect_hello_must_not_go_to_the_stale_route`（仅外设角色平台有该函数）。
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
    #[test]
    fn frame_is_hello_peeks_only_small_hello_frames() {
        let hello = Message::Hello {
            device_id: "dev-a".into(),
            nickname: "A".into(),
            avatar: None,
            device_type: "desktop".into(),
            content_features: crate::protocol::content_features(),
            protocol_version: Some(crate::protocol::PROTOCOL_VERSION),
            app_version: Some(crate::protocol::current_app_version().to_string()),
            tcp_port: 59992,
            x25519_pubkey: "xk".into(),
            ed25519_pubkey: "ek".into(),
            conv_clock: 0,
            nonce: "n1".into(),
            sig: "sig".into(),
        };
        let bytes = serde_json::to_vec(&hello).unwrap();
        assert!(
            bytes.len() < HELLO_PEEK_MAX_BYTES,
            "真实 Hello（{} 字节）必须在上限内，否则重连判据会失效",
            bytes.len()
        );
        assert!(frame_is_hello(&bytes), "Hello 必须被认出来");

        // 非 Hello 的小帧：认成 false，而不是 panic
        let hb = serde_json::to_vec(&Message::Heartbeat {
            device_id: "dev-a".into(),
        })
        .unwrap();
        assert!(!frame_is_hello(&hb));

        // 超上限的帧：一律 false（跳过解析，避免给 256KiB 分片白烧一次解析）
        let big = vec![b'{'; HELLO_PEEK_MAX_BYTES + 1];
        assert!(!frame_is_hello(&big));
        // 坏帧也不能 panic
        assert!(!frame_is_hello(b"not json at all"));
    }

    /// **拨号判据**：两端都能广播时保持"大 id 拨"，对端不广播时**必须由我们拨**。
    ///
    /// 为什么值得一条单测（ADR-0015 §7-f，Windows Phase 1）：Windows 只做 central，
    /// 照搬"大 id 拨、小 id 只接受"会让"Windows 的 id 更小"变成**两侧都不拨**的死局，
    /// 而真机上只表现为"搜到了但永远连不上"，没有任何错误信息可循。
    #[test]
    fn dial_rule_keeps_mirror_guard_but_rescues_non_advertising_peers() {
        // 对端在广播（Mac ↔ 手机）：行为必须与今天逐字节一致 —— 大 id 拨、小 id 不动
        assert!(
            should_dial_ble("gosslan-bbb", "gosslan-aaa", true),
            "我 id 更大 ⇒ 由我拨"
        );
        assert!(
            !should_dial_ble("gosslan-aaa", "gosslan-bbb", true),
            "我 id 更小且对端在广播 ⇒ 不能拨（否则每次连接都打断对端拨过来的好链路）"
        );

        // 对端不广播（Windows 只做 central，或对面根本不广播）⇒ 无条件拨，否则永远是死局
        assert!(
            should_dial_ble("gosslan-aaa", "gosslan-bbb", false),
            "对端不广播时，小 id 一侧**必须**拨 —— 否则没有任何一侧会拨号"
        );
        assert!(
            should_dial_ble("gosslan-bbb", "gosslan-aaa", false),
            "对端不广播 + 我 id 更大 ⇒ 当然也拨"
        );
        // 对端不广播时**不许**登记"不要再拨"（登记了就等于自己放弃唯一能建链的方式）
        assert!(should_dial_ble("a", "z", false));
    }

    /// **BLE 读循环必须回灌读活性**（2026-09-13 审计抓到的真缺陷）。
    ///
    /// `ConnectionHealth` 的读活性只在**建链时播种一次**
    /// （`transport.rs::register_connection` → `seed_connection_read_seen`），
    /// 此后只由**读循环**刷新（TCP 侧见 `transport.rs` 的 `reader_loop`）。
    /// BLE 读循环原来漏了这一步 ⇒ 任何**健康**的蓝牙链路：
    /// 15s 后 `is_healthy` 判假（选路与镜像去重都会按"不健康"处理）、
    /// 45s 被健康看门狗 `stale_connections` 当作死链路**拆掉**，对端再拨回来、再拆，
    /// 无限循环 —— 真机体感就是"蓝牙时好时坏、加好友/消息过一会儿才到"。
    ///
    /// 为什么用**源码断言**而不是跑真实链路：射频行为在单测里无法覆盖，
    /// 但"读循环里有没有这一句"是纯静态事实，而且漏了**不会编译失败**、
    /// 只会让链路每 45s 自断一次 —— 正是最该由护栏盯住的那类退化。
    #[test]
    fn ble_reader_loop_refreshes_read_activity() {
        let src = include_str!("ble.rs");
        let start = src
            .find("async fn ble_reader_loop")
            .expect("必须还有 ble_reader_loop（本护栏锚点）");
        let body = &src[start..];
        // 顶层函数的闭合花括号在行首（缩进的都是内部块）
        let end = body.find("\n}\n").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains("mark_conn_seen("),
            "ble_reader_loop 里必须调用 mark_conn_seen —— 少了它，健康的 BLE 链路会在 \
             15s 被判不健康、45s 被看门狗自己拆掉（真机表现为蓝牙时好时坏）"
        );
    }
}
