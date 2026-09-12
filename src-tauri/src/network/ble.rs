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
#[cfg(any(target_os = "macos", target_os = "android"))]
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use btleplug::api::Peripheral as _; // `Peripheral::id()` 来自 trait，必须引入
use btleplug::platform::Adapter;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::mesh::{BleEndpoint, Endpoint as MeshEndpoint, PathKind};
use crate::network::transport::{
    build_signed_hello, flush_group_outbox, flush_outbox, flush_pending_group_keys,
    flush_pending_group_reads, flush_pending_reads, handle_message, link_snapshot,
    mark_peer_offline, register_connection, should_accept_inbound_public, unregister_connection,
};
use crate::protocol::Message;
use crate::state::{AppState, Link};
use crate::transport::bluetooth::driver::{self, BleReader, BleWriter};
// 外设角色的驱动按平台切换：macOS 用 CoreBluetooth（objc2），Android 用 JNI 调 Kotlin。
// 两者对外接口**完全同形**（`start` / `PeripheralServer` / `PeripheralWriter` / `PeripheralEvent`），
// 因此下面所有外设逻辑（事件循环、握手、路由、读写循环）两个平台共用一份。
#[cfg(target_os = "macos")]
use crate::transport::bluetooth_peripheral as peripheral;
#[cfg(target_os = "android")]
use crate::transport::ble_android as peripheral;
#[cfg(target_os = "macos")]
use crate::transport::bluetooth_peripheral::{PeripheralEvent, PeripheralWriter};
#[cfg(target_os = "android")]
use crate::transport::ble_android::{PeripheralEvent, PeripheralWriter};

/// 每轮扫描的观察窗口（`btleplug` 的扫描是"持续到显式停止"，给一个窗口再收结果）。
const SCAN_WINDOW: Duration = Duration::from_secs(3);
/// 两轮扫描之间的间隔。10s 与 LAN 的 announce 周期同量级：足够快发现，也不至于耗电。
const SCAN_INTERVAL: Duration = Duration::from_secs(10);
/// 握手（发自己的 Hello → 等对端 Hello）上限。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 读循环的单次等待窗口：到点就回到循环顶部，让 shutdown/cancel 有机会被轮询，
/// 同时顺手回收半截消息。
const READ_IDLE: Duration = Duration::from_millis(1_500);
/// 停止时等待后台任务的上限（与 LAN 的 `STOP_TASK_TIMEOUT` 同口径）。
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

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
    let running = state.ble.lock().unwrap_or_else(|e| e.into_inner()).is_some();
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
    let adapter = driver::adapter().await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let st = state.clone();
    let task = tokio::spawn(async move { scan_loop(st, adapter, shutdown_rx).await });

    // 外设角色（GATT server）：只做 central 的话，手机**永远连不上** Mac
    // （btleplug 只能主动连，不能被连 —— ADR-0015 §3.1）。这里独立启动、
    // 独立失败：外设起不来只影响"别人连我们"，不该把整个蓝牙开关判死。
    #[cfg(any(target_os = "macos", target_os = "android"))]
    start_peripheral(state.clone(), shutdown_tx.subscribe()).await;

    *state.ble.lock().unwrap_or_else(|e| e.into_inner()) = Some(BleHandle {
        shutdown: shutdown_tx,
        task,
    });
    state.logger.info("ble", "蓝牙通道已启动（每 10s 扫描一次）");
    Ok(())
}

/// 停止蓝牙通道：发停机信号 → 有界等待 → **摘掉所有 BLE 链路**（不动 LAN 链路）。
pub async fn stop(state: &Arc<AppState>) {
    let handle = state.ble.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handle) = handle {
        let _ = handle.shutdown.send(true);
        if tokio::time::timeout(STOP_TIMEOUT, handle.task).await.is_err() {
            state.logger.warn("ble", "蓝牙后台任务未在 2s 内退出，继续收尾");
        }
    }
    detach_all_ble_links(state).await;
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
        teardown_link(state, &peer, &ep).await;
    }
}

/// 拆掉**一条** BLE 链路：取消读写任务 → 从链路表移除 → mesh 侧注销 →
/// 该 peer 若已无任何链路则标记离线。
///
/// 与 TCP 的 `reader_loop` 收尾同口径（"断一条 ≠ peer 下线"）：
/// 同一 peer 可能同时有 LAN 与 BLE 两条链路。
async fn teardown_link(state: &Arc<AppState>, peer_id: &str, ep: &MeshEndpoint) {
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
}

async fn scan_loop(state: Arc<AppState>, adapter: Adapter, mut shutdown: watch::Receiver<bool>) {
    loop {
        match driver::scan_peers(&adapter, SCAN_WINDOW).await {
            Ok(peers) => {
                for peripheral in peers {
                    if *shutdown.borrow() {
                        return;
                    }
                    let st = state.clone();
                    let sd = shutdown.clone();
                    // 每个候选一个任务：连接 + 握手最长 10s，串行会把扫描周期拖垮
                    tokio::spawn(async move {
                        let id = peripheral.id().to_string();
                        match dial_and_register(st.clone(), peripheral, sd).await {
                            Ok(()) => {}
                            // 连接失败是常态（对方正在忙、走远了、不是 Gosslan 端），
                            // 记 info 不记 warn —— 否则日志会被邻居设备刷满
                            Err(e) => st.logger.info("ble", format!("候选 {id} 未建立链路：{e}")),
                        }
                    });
                }
            }
            Err(e) => state.logger.warn("ble", format!("扫描失败：{e}")),
        }
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = tokio::time::sleep(SCAN_INTERVAL) => {}
        }
    }
}

/// 连接一个候选 → 双向 Hello 验签 → 登记链路 → 起收发循环。
async fn dial_and_register(
    state: Arc<AppState>,
    peripheral: btleplug::platform::Peripheral,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ble_id = peripheral.id().to_string();
    let ep = MeshEndpoint::Ble(BleEndpoint::new(ble_id.clone()));
    // 这个端点已经连着 ⇒ 跳过（`connect_to_peer` 的同款去重）
    if state.has_endpoint_addr(&ep).await {
        return Ok(());
    }
    let conn = driver::connect(&peripheral).await?;
    let (mut writer, mut reader) = conn.into_split();

    // ---- 握手：先发自己的 Hello，再读对端的、并**必须验签**（§8 / ADR-0011）----
    // BLE 地址不是身份，所以这里身份一定是"未知"（conv_clock 传 0 即可：
    // 对端 observe_clock 取 max，不会因此倒退）。
    let hello = build_signed_hello(&state, 0);
    let bytes = serde_json::to_vec(&hello).map_err(|e| format!("Hello 序列化失败：{e}"))?;
    writer.send_frame(&bytes).await?;

    let first = read_one(&mut reader, HANDSHAKE_TIMEOUT, &mut shutdown).await?;
    let Some(first) = first else {
        return Err("握手超时：对端未回 Hello".to_string());
    };
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
        return Err("对端首帧不是 Hello".to_string());
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

    // ---- 去重：与 TCP 入站**同一个判据**（不要在这里复制第二份"有没有同路径连接"）----
    let existing = link_snapshot(&state, &peer_id).await;
    if !should_accept_inbound_public(
        &state.device_id,
        &peer_id,
        PathKind::Bluetooth,
        &existing,
    ) {
        return Err("已有蓝牙链路（或该 peer 链路数已满），不重复建链".to_string());
    }

    // ---- 登记链路（端点 = BLE 标识，路径 = Bluetooth）----
    let (bulk_tx, bulk_rx) = mpsc::channel(1024);
    let (prio_tx, prio_rx) = mpsc::channel(1024);
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
            bulk: bulk_tx.clone(),
            priority: prio_tx.clone(),
            cancel: cancel_tx,
        });
    register_connection(&state, &peer_id, ep.clone(), PathKind::Bluetooth);
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
        bulk_rx,
        prio_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));
    // ↑ 写循环泛型化：central 写 GATT 特征、外设发通知，逻辑同一份（见 `FrameSink`）
    tokio::spawn(ble_reader_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        reader,
        shutdown,
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
#[cfg(any(target_os = "macos", target_os = "android"))]
struct PeripheralSink {
    writer: PeripheralWriter,
    central: String,
}

#[cfg(any(target_os = "macos", target_os = "android"))]
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
}

#[async_trait::async_trait]
impl FrameSource for BleReader {
    async fn next_frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String> {
        BleReader::next_frame(self, wait).await
    }
    fn gc(&mut self) -> usize {
        BleReader::gc(self)
    }
}

/// 外设侧的读方向：帧已经重组好，直接从通道拿。
#[cfg(any(target_os = "macos", target_os = "android"))]
struct ChannelSource {
    rx: mpsc::Receiver<Vec<u8>>,
}

#[cfg(any(target_os = "macos", target_os = "android"))]
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

async fn ble_writer_loop<S: FrameSink + 'static>(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut writer: S,
    mut bulk_rx: mpsc::Receiver<Message>,
    mut prio_rx: mpsc::Receiver<Message>,
    mut shutdown: watch::Receiver<bool>,
    mut cancel: watch::Receiver<bool>,
) {
    let mut bulk_open = true;
    let mut prio_open = true;
    loop {
        if !bulk_open && !prio_open {
            break;
        }
        let msg = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = cancel.changed() => break,
            maybe = prio_rx.recv(), if prio_open => maybe,
            maybe = bulk_rx.recv(), if bulk_open => maybe,
        };
        match msg {
            Some(msg) => {
                let Ok(bytes) = serde_json::to_vec(&msg) else {
                    continue;
                };
                // 写也要能被停机/判死打断（与 TCP 的 writer_loop 同一考虑）
                let ok = tokio::select! {
                    biased;
                    _ = shutdown.changed() => false,
                    _ = cancel.changed() => false,
                    res = writer.send_frame(&bytes) => res.is_ok(),
                };
                if !ok {
                    state.logger.info("ble", format!("写失败，结束该 BLE 链路的写循环 peer={peer_id} ep={ep}"));
                    break;
                }
            }
            None => {
                if prio_rx.is_closed() {
                    prio_open = false;
                }
                if bulk_rx.is_closed() {
                    bulk_open = false;
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
    loop {
        let frame = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = cancel.changed() => break,
            res = reader.next_frame(READ_IDLE) => res,
        };
        match frame {
            Ok(Some(bytes)) => match serde_json::from_slice::<Message>(&bytes) {
                Ok(msg) => handle_message(&state, &peer_id, msg).await,
                Err(e) => state
                    .logger
                    .warn("ble", format!("丢弃无法解析的 BLE 帧 peer={peer_id}: {e}")),
            },
            Ok(None) => {
                // 窗口内没有分片：顺手回收半截消息（对端在半途断连时不会永久占内存）
                let _ = reader.gc();
            }
            Err(e) => {
                state
                    .logger
                    .info("ble", format!("BLE 读结束 peer={peer_id} ep={ep}: {e}"));
                break;
            }
        }
    }
    // 收尾：只拆这一条（同一 peer 可能还有 LAN 链路）
    teardown_link(&state, &peer_id, &ep).await;
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
#[cfg(any(target_os = "macos", target_os = "android"))]
enum RouteCtl {
    Add {
        central: String,
        tx: mpsc::Sender<Vec<u8>>,
    },
}

/// 启动外设角色。失败只记日志：能扫别人但别人连不上我们，属于**降级**而不是故障，
/// 不该把整个蓝牙开关判为不可用（LAN 更不受影响）。
#[cfg(any(target_os = "macos", target_os = "android"))]
async fn start_peripheral(state: Arc<AppState>, shutdown: watch::Receiver<bool>) {
    let startup = match peripheral::start() {
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
        Ok(Ok(Ok(()))) => state
            .logger
            .info("ble", "蓝牙外设角色已启动（广播服务 UUID，等待手机/PC 连入）"),
        Ok(Ok(Err(e))) => {
            state
                .logger
                .warn("ble", format!("蓝牙外设角色不可用（central 角色不受影响）：{e}"));
            startup.server.stop();
            return;
        }
        Ok(Err(_)) => {
            state
                .logger
                .warn("ble", "蓝牙外设角色的状态回调通道被关闭，放弃启动");
            startup.server.stop();
            return;
        }
        Err(_) => state
            .logger
            .info("ble", "蓝牙外设角色已启动（未在 3s 内收到状态回调，继续广播）"),
    }
    tokio::spawn(peripheral_accept_loop(state, startup.server, shutdown));
}

/// 外设侧的总循环：把每个 central 的帧分派给它的链路任务，首帧走握手。
#[cfg(any(target_os = "macos", target_os = "android"))]
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
                let RouteCtl::Add { central, tx } = ctl;
                handshaking.remove(&central);
                routes.insert(central, tx);
                continue;
            }
            maybe = server.events.recv() => match maybe {
                Some(ev) => ev,
                None => break,
            },
        };

        match ev {
            PeripheralEvent::Frame { central, bytes } => {
                // 有活路由就投递。投递失败（接收端已 drop）= 旧链路已死，
                // 这一帧很可能正是对端**重连**后的 Hello ⇒ 落到下面的握手分支。
                let mut pending = Some(bytes);
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
                let bytes = pending.expect("未投递的帧必须还在");
                if handshaking.insert(central.clone()) {
                    tokio::spawn(accept_handshake(
                        state.clone(),
                        server.writer.clone(),
                        central,
                        bytes,
                        route_tx.clone(),
                        shutdown.clone(),
                    ));
                }
            }
            PeripheralEvent::Unlinked { central } => {
                handshaking.remove(&central);
                routes.remove(&central);
                state
                    .logger
                    .info("ble", format!("外设侧对端取消订阅（视为断开）central={central}"));
                let ep = MeshEndpoint::Ble(BleEndpoint::new(central));
                detach_by_endpoint(&state, &ep).await;
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
#[cfg(any(target_os = "macos", target_os = "android"))]
async fn detach_by_endpoint(state: &Arc<AppState>, ep: &MeshEndpoint) {
    let peer = {
        let links = state.links.lock().await;
        links
            .iter()
            .find(|(_, v)| v.iter().any(|l| &l.endpoint == ep))
            .map(|(p, _)| p.clone())
    };
    if let Some(peer) = peer {
        teardown_link(state, &peer, ep).await;
    }
}

/// 外设侧握手的外壳：失败一律**只记日志**（对端可能只是路过、或者根本不是 Gosslan 端）。
#[cfg(any(target_os = "macos", target_os = "android"))]
async fn accept_handshake(
    state: Arc<AppState>,
    writer: PeripheralWriter,
    central: String,
    first_bytes: Vec<u8>,
    route_tx: mpsc::Sender<RouteCtl>,
    shutdown: watch::Receiver<bool>,
) {
    if let Err(e) =
        try_accept_handshake(&state, writer, &central, first_bytes, route_tx, shutdown).await
    {
        state
            .logger
            .info("ble", format!("外设侧未建链 central={central}：{e}"));
    }
}

/// 真身：验签对端 Hello → 回我们的 Hello → 登记链路 → 起收发。
#[cfg(any(target_os = "macos", target_os = "android"))]
async fn try_accept_handshake(
    state: &Arc<AppState>,
    writer: PeripheralWriter,
    central: &str,
    first_bytes: Vec<u8>,
    route_tx: mpsc::Sender<RouteCtl>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ep = MeshEndpoint::Ble(BleEndpoint::new(central.to_string()));

    // ---- 1. 首帧必须是 Hello，且签名必须验过（BLE 地址不是身份）----
    let first: Message = serde_json::from_slice(&first_bytes)
        .map_err(|e| format!("对端首帧无法解析：{e}"))?;
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
        return Err("对端首帧不是 Hello".to_string());
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
    detach_by_endpoint(state, &ep).await;

    // ---- 3. 与 TCP 入站**同一个**去重判据（不要在这里复制第二份规则）----
    let existing = link_snapshot(state, &peer_id).await;
    if !should_accept_inbound_public(
        &state.device_id,
        &peer_id,
        PathKind::Bluetooth,
        &existing,
    ) {
        return Err("已有蓝牙链路（或该 peer 链路数已满），不重复建链".to_string());
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
    let (bulk_tx, bulk_rx) = mpsc::channel(1024);
    let (prio_tx, prio_rx) = mpsc::channel(1024);
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
            bulk: bulk_tx.clone(),
            priority: prio_tx.clone(),
            cancel: cancel_tx,
        });
    register_connection(state, &peer_id, ep.clone(), PathKind::Bluetooth);
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
        bulk_rx,
        prio_rx,
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
