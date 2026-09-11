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

async fn ble_writer_loop(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut writer: BleWriter,
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

async fn ble_reader_loop(
    state: Arc<AppState>,
    peer_id: String,
    ep: MeshEndpoint,
    mut reader: BleReader,
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
