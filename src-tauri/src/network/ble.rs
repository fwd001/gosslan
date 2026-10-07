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

include!("ble/central.rs");
include!("ble/peripheral.rs");
include!("ble/frame_io.rs");
include!("ble/io_loops.rs");

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
        let src = crate::network::ble_src_for_guards();
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
        let src = crate::network::ble_src_for_guards();
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
