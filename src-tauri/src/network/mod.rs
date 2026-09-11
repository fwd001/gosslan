//! 网络层：UDP 广播发现 + TCP 消息/文件传输。
//!
//! 传输抽象说明：
//! - 当前实现基于 TCP（简单可靠），帧格式见 `protocol.rs`。
//! - 未来可无缝切换/新增 QUIC（如 `quinn`）或 WebSocket 中继：只要实现
//!   “分帧写入/读取 + 建立连接”两个原语，`try_send` 与消息分发逻辑无需改动，
//!   即可支撑“服务端中转连接电脑与移动端”的场景。

pub mod discovery;
/// BLE 传输的运行时接线（feature = "bluetooth"；默认关闭，见 ADR-0015）。
#[cfg(feature = "bluetooth")]
pub mod ble;
pub mod file;
pub mod transport;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::state::{AppState, NetworkHandle};

/// 启动网络（UDP 发现 + TCP 服务）。
/// `bind_ip` 为选定的网卡 IPv4 地址，或 "0.0.0.0" 表示自动（监听所有网卡）。
pub async fn start(state: Arc<AppState>, bind_ip: String) -> Result<(), String> {
    // 先停掉旧实例
    stop(&state).await;

    let ip: std::net::Ipv4Addr = bind_ip
        .parse()
        .map_err(|_| format!("无效的网卡地址: {bind_ip}"))?;
    let tcp_port = state.tcp_port;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (probe_tx, probe_rx) = watch::channel(0u64);

    // 启动顺序：**先 TCP 监听，再 UDP 发现**。
    //
    // 旧顺序（先 discovery 后 transport）在 TCP bind 失败时会留下一个半启动状态：
    // discovery 已经跑起来并广播过一轮，随后 `transport::spawn` 返回 Err，
    // `start()` 提前返回把局部 `shutdown_tx` drop 掉，discovery 任务的
    // `shutdown.changed()` 立刻返回 Err 而退出 —— UDP 被 TCP 的失败「陪葬」。
    // 结果是本机既没有监听也没有 announce，对端连发现都做不到，且日志里看不出
    // 曾经发生过什么。先 transport 可以保证：bind 失败时 discovery 从未启动，
    // 失败状态是干净的。
    let tasks = start_in_order(
        &shutdown_tx,
        Box::pin(transport::spawn(
            state.clone(),
            ip,
            tcp_port,
            shutdown_rx.clone(),
        )),
        Box::pin(discovery::spawn(
            state.clone(),
            ip,
            tcp_port,
            shutdown_rx,
            probe_rx,
        )),
    )
    .await?;

    // Discovery 实际绑定的是真实 LAN IP（auto 模式下），需要让诊断面板展示它。
    let actual_bound_ip = state.diag.lock().unwrap_or_else(|e| e.into_inner()).bound_ip.clone();
    *state.probe.lock().unwrap_or_else(|e| e.into_inner()) = Some(probe_tx);
    // 进入新世代：此后旧世代（上一次 start 的 accept 任务）不得再登记链路。
    state.bump_network_generation();
    *state.network.lock().unwrap_or_else(|e| e.into_inner()) = Some(NetworkHandle {
        shutdown: shutdown_tx,
        bound_ip: bind_ip,
        actual_bound_ip,
        tcp_port,
        tasks,
    });
    Ok(())
}

/// 停止网络：发送关闭信号，**等待后台任务真正退出**，再清理连接与在线表。
pub async fn stop(state: &AppState) {
    // 先进入新世代：让"握手还没完成的上一个世代的任务"在登记前就自我否决
    // （见 `AppState::network_generation` 的注释）。
    state.bump_network_generation();
    // 先取出句柄再 await：`std::sync::MutexGuard` 不能跨 await，否则 stop() 的
    // future 不是 Send，无法放进 `tauri::async_runtime::spawn`。
    let handle = state.network.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handle) = handle {
        let _ = handle.shutdown.send(true);
        // 必须等：旧的 TCP listener 只有 accept 任务退出后才真正释放。
        // 不等就继续走（同进程切换网卡 / 重新开启通道，或 `app.restart()`
        // 起来的新进程）都可能撞上 AddrInUse，Windows 上表现为重启后永久掉线。
        await_tasks(handle.tasks).await;
    }
    *state.probe.lock().unwrap_or_else(|e| e.into_inner()) = None;
    state.links.lock().await.clear();
    state.peers.lock().unwrap_or_else(|e| e.into_inner()).clear();
    state.emit_peers();
}

/// 子系统的启动 future（`transport::spawn` / `discovery::spawn`）。
type SpawnFut =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<tokio::task::JoinHandle<()>>, String>> + Send>>;

/// 启动编排：**只有 TCP 监听成功，才启动 UDP 发现**。
///
/// 单独抽成函数的唯一目的，是让「TCP bind 失败 ⇒ discovery 从未被 poll」这条 P0
/// 不变量可以在单测里验证：`start()` 需要 `AppState`，而它必须由真实 Tauri
/// `AppHandle` 构造，单测中造不出来。
///
/// `discovery` 是一个**尚未被 poll 的** future：`async fn` 在被调用时并不执行
/// 函数体，所以 transport 失败时它会被整个丢弃，UDP 一次都不会启动。
pub(crate) async fn start_in_order(
    shutdown: &watch::Sender<bool>,
    transport: SpawnFut,
    discovery: SpawnFut,
) -> Result<Vec<tokio::task::JoinHandle<()>>, String> {
    let mut tasks = transport.await?;
    match discovery.await {
        Ok(t) => {
            tasks.extend(t);
            Ok(tasks)
        }
        Err(e) => {
            // 收回已经启动的 transport，再向上报错，绝不留下半启动状态
            let _ = shutdown.send(true);
            await_tasks(tasks).await;
            Err(e)
        }
    }
}

/// 后台任务退出等待上限。
///
/// 正常路径上任务收到 shutdown 后是毫秒级退出的；这个上限只是防止个别卡住的
/// 任务把 stop()（进而把 `app.restart()` / 退出流程）永久阻塞。
const STOP_TASK_TIMEOUT: Duration = Duration::from_secs(2);

/// 依次等待后台任务结束，最长 [`STOP_TASK_TIMEOUT`]；超时即打日志放行。
async fn await_tasks(tasks: Vec<tokio::task::JoinHandle<()>>) {
    if tasks.is_empty() {
        return;
    }
    let joined = async move {
        for t in tasks {
            let _ = t.await;
        }
    };
    if tokio::time::timeout(STOP_TASK_TIMEOUT, joined).await.is_err() {
        eprintln!(
            "[lan] 等待网络后台任务退出超时（{}s），强制继续",
            STOP_TASK_TIMEOUT.as_secs()
        );
    }
}

/// 默认绑定地址：自动（监听所有网卡）。
const AUTO_BIND_IP: &str = "0.0.0.0";

/// 按本地偏好开启局域网通道（开机自动开启与设置页开关共用同一条路径）。
///
/// 绑定地址沿用用户已选网卡（`settings.bind_ip`）——自动开启不得改写用户的选择；
/// 只有该网卡已不存在（换了网络）时才回落到自动选择，
/// 否则「默认开启」会比手动开启更脆弱：绑定失败后通道静默不在线。
pub async fn start_from_prefs(state: Arc<AppState>) -> Result<(), String> {
    let bind_ip = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::db::get_setting(&dbc, "bind_ip").unwrap_or_else(|| AUTO_BIND_IP.to_string())
    };
    match start(state.clone(), bind_ip.clone()).await {
        Ok(()) => Ok(()),
        Err(e) if bind_ip != AUTO_BIND_IP => {
            state.logger.warn("lan", format!("绑定 {bind_ip} 失败（{e}），回落到自动选择网卡"));
            start(state, AUTO_BIND_IP.to_string()).await
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc as StdArc;

    /// Test 3：transport（TCP 监听）失败 ⇒ discovery 这个 future 必须从未被 poll，
    /// 也就是 UDP 一次都不会启动。
    ///
    /// 旧实现反过来（先 discovery 后 transport），TCP bind 失败时 UDP 已经广播过
    /// 一轮，随后被 start() 提前返回带崩，结果是「UDP + TCP 同时静默」，
    /// 对端连发现都做不到 —— 生产 1.0「C 重启后彻底掉线」的放大器。
    #[tokio::test]
    async fn transport_bind_failure_never_starts_discovery() {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let discovery_polled = StdArc::new(AtomicBool::new(false));
        let flag = discovery_polled.clone();

        let transport: SpawnFut =
            Box::pin(async { Err("TCP 绑定 0.0.0.0:59992 失败：端口被占用".to_string()) });
        let discovery: SpawnFut = Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(vec![])
        });

        let err = start_in_order(&shutdown_tx, transport, discovery)
            .await
            .expect_err("transport 失败必须向上传播");
        assert!(err.contains("端口被占用"), "错误信息应保留: {err}");
        assert!(
            !discovery_polled.load(Ordering::SeqCst),
            "TCP 绑定失败时 discovery 绝不能被启动（否则 UDP 会被连带陪葬）"
        );
        // 失败路径不应误发 shutdown（没有任务需要停）
        assert!(!*shutdown_rx.borrow_and_update());
    }

    /// 对照：transport 成功时 discovery 必须被启动，且任务句柄合并返回。
    #[tokio::test]
    async fn transport_success_starts_discovery_and_merges_handles() {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let discovery_polled = StdArc::new(AtomicBool::new(false));
        let flag = discovery_polled.clone();

        let transport: SpawnFut = Box::pin(async {
            Ok(vec![tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
            })])
        });
        let discovery: SpawnFut = Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(vec![tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
            })])
        });

        let tasks = start_in_order(&shutdown_tx, transport, discovery)
            .await
            .expect("两步都成功时应返回合并后的句柄");
        assert!(discovery_polled.load(Ordering::SeqCst));
        assert_eq!(tasks.len(), 2, "transport + discovery 的任务都要登记");
    }

    /// discovery 失败时必须收回已启动的 transport：发 shutdown 并等它退出。
    #[tokio::test]
    async fn discovery_failure_rolls_back_started_transport() {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let stopped = StdArc::new(AtomicBool::new(false));
        let stop_flag = stopped.clone();

        let transport: SpawnFut = Box::pin(async move {
            Ok(vec![tokio::spawn(async move {
                // 模拟 accept 任务：等 shutdown 信号
                let _ = shutdown_rx.changed().await;
                stop_flag.store(true, Ordering::SeqCst);
            })])
        });
        let discovery: SpawnFut = Box::pin(async { Err("UDP 绑定失败".to_string()) });

        let err = start_in_order(&shutdown_tx, transport, discovery)
            .await
            .expect_err("discovery 失败必须向上传播");
        assert!(err.contains("UDP 绑定失败"));
        assert!(
            stopped.load(Ordering::SeqCst),
            "discovery 失败时必须等已启动的 transport 任务退出，不留半启动状态"
        );
    }
}
