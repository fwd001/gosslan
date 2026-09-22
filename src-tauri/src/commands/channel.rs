// 职责边界：
// - 双通道开关（蓝牙/LAN）、运行态快照
// - 缓存读、权限检查（蓝牙权限）
// ---------------- 双通道与缓存 ----------------

/// 局域网 / 蓝牙通道状态（设置页开关 + 「添加好友」页的就地开关共用这一份）。
///
/// ⚠️ 蓝牙的 `running` / `peers` 必须取**真实运行时**（`network::ble`），不能采信
/// `TransportManager` 里那个占位 `BluetoothTransport`（它的 running 恒 false）——
/// 否则界面永远显示"未运行"，用户点了开关也看不出变化。
/// 「enabled」也一律以真实运行为准：起不来就是关（这样界面与用户预期一致，
/// 也能让他再点一次重试，而不是假装已经打开）。
///
/// 返回 `Result` 是 Tauri 的硬性要求（async 命令带 `State<'_>` 引用参数时必须返回 Result），
/// 前端侧不受影响（`invoke` 拿到的是 `Ok` 里的数组，永远不返回 `Err`）。
/// 默认昵称（按 `nickname.rs` 的规则由 device_id 派生）。
///
/// 给"恢复默认"用：前端不该再写死一份默认名文案（那是旧规则 `hostname` 的遗留），
/// 否则"恢复默认"得到的名字与首次安装得到的名字会不一致。
#[tauri::command(async)]
pub fn default_nickname(state: State<'_, Arc<AppState>>) -> String {
    crate::nickname::default_nickname(&state.inner().device_id)
}

/// **运行状态的唯一采集点**（用户要求的第 ② 项）。
///
/// 把所有"运行状态"一次读全：通道（lan/bluetooth 的 enabled/available/running/peers）、
/// 局域网是否在跑 + 绑定地址、蓝牙事实、在线节点数。任何"运行状态变了"的地方
/// （通道开关 / 起停网络）都调它一次，然后：
///   · 命令把它**作为返回值**给发起窗口（发起窗口零额外 IPC）；
///   · `notify_runtime_changed` 把它**作为事件载荷**发给其它窗口。
/// 于是一件事只有一份前端状态（`RuntimeSnapshot`），不可能再各说各话。
pub async fn build_runtime_snapshot(s: &Arc<AppState>) -> RuntimeSnapshot {
    let mut list = TransportManager::new(s.clone()).status();
    #[cfg(feature = "bluetooth")]
    let (bt_running, bt_peers) = crate::network::ble::runtime_state(s).await;
    #[cfg(not(feature = "bluetooth"))]
    let (bt_running, bt_peers) = (false, 0usize);
    if let Some(bt) = list.iter_mut().find(|c| c.channel == "bluetooth") {
        bt.running = bt_running;
        bt.peers = bt_peers;
        bt.enabled = bt_running;
    }
    // 持久化偏好必须与“此刻是否在跑”分开表达：应用刚启动时 BLE 还没拉起，enabled/running
    // 都是 false，前端无法据此区分“用户明确关掉”与“尚未启动”⇒ 自动拉起会覆盖用户的关闭选择
    // （真机 2026-09-14：关掉蓝牙、退出重进又被打开）。get_*_enabled 在键缺失时会顺手落默认值
    // （首次安装 ⇒ 默认开）。
    {
        let (lan_pref, bt_pref) = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            (
                crate::db::get_lan_enabled(&dbc),
                crate::db::get_bt_enabled(&dbc),
            )
        };
        for c in list.iter_mut() {
            match c.channel {
                "lan" => c.preferred = lan_pref,
                "bluetooth" => c.preferred = bt_pref,
                _ => {}
            }
        }
    }
    let (online, bound_ip) = {
        let net = s.network.lock().unwrap_or_else(|e| e.into_inner());
        (net.is_some(), net.as_ref().map(|n| n.bound_ip.clone()))
    };
    let peer_count = s.peers.lock().unwrap_or_else(|e| e.into_inner()).len();
    // 我的在线状态 = **任一通道在跑**（用户规则：两个都关才是离线）。
    // 注意用 `running` 而不是 `enabled`：开关打开但起不来（如权限被拒）不该算在线。
    let present = list.iter().any(|c| c.running);
    // 跨网可达性（**不是发现通道**，配置在设置页）：只取"有没有 / 通没通"，绝不取口令与服务器地址。
    // 「添加好友」页据此如实告知"局域网/蓝牙之外还开着什么"（用户 2026-09-22）。
    let (routed_endpoints, relay_enabled) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        (
            crate::discovery::routed::parse_endpoints(
                &db::get_setting(&dbc, crate::discovery::routed::ROUTED_ENDPOINTS_KEY)
                    .unwrap_or_default(),
            )
            .len(),
            db::get_setting(&dbc, RELAY_ENABLED_KEY).is_some_and(|v| v == "1"),
        )
    };
    // 中继是否真的连通：有任意一条 `path_kind==Relay` 的活跃链路即算（用枚举判，不按服务器地址）。
    let relay_connected = {
        let links = s.links.lock().await;
        links
            .values()
            .any(|ls| ls.iter().any(|l| l.path_kind == crate::mesh::PathKind::Relay))
    };
    RuntimeSnapshot {
        present,
        channels: list,
        online,
        bound_ip,
        ble: BleRuntimeFacts {
            feature_compiled: cfg!(feature = "bluetooth"),
        },
        peer_count,
        routed_endpoints,
        relay: crate::state::RelayRuntimeStatus {
            enabled: relay_enabled,
            connected: relay_connected,
        },
    }
}

/// 取当前运行状态快照（窗口初始化 / 手动刷新用）。
#[tauri::command(async)]
pub async fn get_runtime_snapshot(
    state: State<'_, Arc<AppState>>,
) -> Result<RuntimeSnapshot, String> {
    Ok(build_runtime_snapshot(state.inner()).await)
}

/// 蓝牙通道「开关意图」的最后一次值：`1` = 开，`0` = 关，`-1` = 还没有请求。
///
/// 为什么需要：冷却期内到达的新意图**不能丢**（见 [`apply_bluetooth_switch`]）。
#[cfg(feature = "bluetooth")]
static BT_DESIRED: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);

/// 上一次**真的**启停过蓝牙的时刻（毫秒；0 = 从未启停过）。
#[cfg(feature = "bluetooth")]
static BT_LAST_TRANSITION_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 蓝牙启停的串行锁：同一时刻只有一次真实启停，后到的请求**排队**（而不是被丢掉）。
#[cfg(feature = "bluetooth")]
static BT_SWITCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 两次**真实**启停之间的最小间隔（防抖；见 [`bt_switch_plan`]）。
#[cfg(feature = "bluetooth")]
const BT_SWITCH_COOLDOWN_MS: u64 = 3_000;

/// 一次蓝牙启停请求的决策结果（纯数据，便于单测）。
#[cfg(feature = "bluetooth")]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BtSwitchPlan {
    /// 动手之前要等的毫秒数（0 = 立刻动手）
    pub wait_ms: u64,
    /// 是否需要**真的**启停蓝牙栈（false = 幂等命中，或已被更新的意图取代）
    pub apply: bool,
    /// 不做 / 等待的原因（写日志用）
    pub reason: &'static str,
}

/// 决定"这次蓝牙开关请求该不该动蓝牙栈"。
///
/// 顺序即优先级（三条都是真机事故换来的）：
/// 1. **已被更新的意图取代** ⇒ 什么都不做 —— 那次更新的请求会执行。
///    这是"意图合并"的关键：用户"关了立刻又开"时，最后一次意图一定会被执行到。
/// 2. **运行状态已经是目标状态** ⇒ 幂等，不碰蓝牙栈（用户 2026-09-12 的抖动事故：
///    每秒十几次 `启动→停止→启动` 会把 CoreBluetooth 的 GATT server + 广播反复拆建，
///    CPU 与蓝牙栈被打满 ⇒ 整个应用顿卡、连局域网消息都变慢）。
/// 3. **距上次真实启停不足冷却** ⇒ **等够了再做**（`wait_ms`），不是丢弃。
///    旧实现是"丢弃"（`忽略高频蓝牙通道切换请求`）：用户"关一下马上又开"会静默少执行一次，
///    表现就是"点了没反应"（用户 2026-09-13）。
#[cfg(feature = "bluetooth")]
pub(crate) fn bt_switch_plan(
    running: bool,
    enabled: bool,
    desired: Option<bool>,
    last_ms: u64,
    now_ms: u64,
    cooldown_ms: u64,
) -> BtSwitchPlan {
    if desired != Some(enabled) {
        return BtSwitchPlan {
            wait_ms: 0,
            apply: false,
            reason: "已被更新的开关意图取代",
        };
    }
    if running == enabled {
        return BtSwitchPlan {
            wait_ms: 0,
            apply: false,
            reason: "运行状态已经是目标状态（幂等）",
        };
    }
    let wait_ms = if last_ms == 0 {
        0
    } else {
        cooldown_ms.saturating_sub(now_ms.saturating_sub(last_ms))
    };
    BtSwitchPlan {
        wait_ms,
        apply: true,
        reason: if wait_ms > 0 {
            "冷却期内，排队等待"
        } else {
            "立刻启停"
        },
    }
}

/// 执行一次蓝牙开关（**意图合并 + 冷却排队 + 幂等**）。
///
/// ⚠️ 调用方 `await` 它，但**前端不该等它**（用户 2026-09-13 的规则：乐观更新优先）：
/// 这里可能为了合并抖动等满一个冷却周期（最多 3s）。
#[cfg(feature = "bluetooth")]
async fn apply_bluetooth_switch(s: &Arc<AppState>, enabled: bool) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    // 先记意图（不需要锁）：排队醒来后要靠它判断"自己还是不是最新意图"
    BT_DESIRED.store(if enabled { 1 } else { 0 }, Ordering::Relaxed);
    // 串行化：同一时刻只有一次真实启停，后来者在这里排队
    let _guard = BT_SWITCH_LOCK.lock().await;
    for _ in 0..3 {
        let running = s.ble.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        let now = db::now_ms().max(0) as u64;
        let desired = match BT_DESIRED.load(Ordering::Relaxed) {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        };
        let plan = bt_switch_plan(
            running,
            enabled,
            desired,
            BT_LAST_TRANSITION_MS.load(Ordering::Relaxed),
            now,
            BT_SWITCH_COOLDOWN_MS,
        );
        if !plan.apply {
            s.logger.info(
                "ble",
                format!(
                    "蓝牙通道开关：{}（运行中={running}，请求={enabled}）",
                    plan.reason
                ),
            );
            break;
        }
        if plan.wait_ms > 0 {
            // **排队而不是丢弃**：用户"关了又马上开"时，最后那次意图一定会被执行到
            s.logger.info(
                "ble",
                format!(
                    "蓝牙通道开关进入冷却：等待 {}ms 后执行（合并抖动）",
                    plan.wait_ms
                ),
            );
            tokio::time::sleep(Duration::from_millis(plan.wait_ms)).await;
            continue; // 醒来重判：意图可能又被改过、运行状态也可能变过
        }
        BT_LAST_TRANSITION_MS.store(now, Ordering::Relaxed);
        s.logger.info(
            "ble",
            format!(
                "蓝牙通道切换：{} → {}",
                if running { "运行中" } else { "已停止" },
                if enabled { "开启" } else { "关闭" }
            ),
        );
        let result = if enabled {
            crate::network::ble::start(s.clone()).await
        } else {
            crate::network::ble::stop(s).await;
            Ok(())
        };
        if result.is_err() {
            // ⚠️ **失败要放行重试**：冷却的用途是挡住"成功之后又被反复切换"的抖动，
            // 不是挡住用户/前端的重试。真实缺陷（用户 4.1.9 实测）：
            // 第一次 `已停止 → 开启` 失败（当时安卓还缺 btleplug 的 Java 类），
            // 前端的自动重试落进 3s 冷却被丢掉 ⇒ 表现为"蓝牙没有默认开启"。
            BT_LAST_TRANSITION_MS.store(0, Ordering::Relaxed);
        }
        result?;
        break;
    }
    Ok(())
}

/// 切换通道开关。局域网复用 `network`；蓝牙后端未编译，开启时返回明确错误。
#[tauri::command(async)]
pub async fn set_channel_enabled(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    channel: String,
    enabled: bool,
) -> Result<RuntimeSnapshot, String> {
    let s = state.inner();
    match channel.as_str() {
        "lan" => {
            if enabled {
                // 绑定地址沿用用户已选网卡（settings.bind_ip），不再硬编码 0.0.0.0
                network::start_from_prefs(s.clone()).await?;
            } else {
                network::stop(s).await;
            }
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::set_lan_enabled(&dbc, enabled).ok();
        }
        "bluetooth" => {
            // 开了 feature 才真正启动 BLE 运行时；没开 feature 时与今天一致：
            // 只写偏好（并返回"后端未编译"的明确错误）。
            #[cfg(feature = "bluetooth")]
            {
                apply_bluetooth_switch(s, enabled).await?;
            }
            #[cfg(not(feature = "bluetooth"))]
            {
                if enabled {
                    let mut mgr = TransportManager::new(s.clone());
                    mgr.set_bluetooth_enabled(true).await?;
                }
            }
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::set_bt_enabled(&dbc, enabled).ok();
        }
        _ => return Err(format!("未知通道: {channel}")),
    }
    // 运行状态只在这里"变"：采一次快照 —— 发起窗口拿返回值（零额外 IPC），
    // 其余窗口拿事件载荷（`runtime-changed` 带快照）。两边拿到的是**同一份结构**。
    let snap = build_runtime_snapshot(s).await;
    s.notify_runtime_changed(snap.clone(), Some(window.label()));
    Ok(snap)
}

const RETENTION_KEY: &str = "cache_retention_days";
const MAX_BYTES_KEY: &str = "cache_max_bytes";

fn load_policy(s: &AppState) -> CachePolicy {
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let retention = db::get_setting(&dbc, RETENTION_KEY)
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&d| d > 0);
    let max = db::get_setting(&dbc, MAX_BYTES_KEY)
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&m| m > 0);
    CachePolicy {
        retention_days: retention,
        max_bytes: max,
    }
}

/// 纳入统计与清理的目录：接收的图片/文件目录 + 历史遗留的 cache 目录。
/// 旧的 `cache/` 可能残留早期版本抽取的图片，一并纳入，避免"看不见也清不掉"。
fn media_dirs(s: &AppState) -> Vec<PathBuf> {
    let downloads = s
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    // favorites（收藏的独立媒体副本）必须在内 —— 此前它既不占配额也永不回收，
    // 收藏越多越接近无限增长（2026-09-19 审计）。
    vec![downloads, s.cache_dir.clone(), s.favorites_dir.clone()]
}

/// 自动缓存清理调度（README 承诺的「3/7/30 天 + 配额自动清理」此前只有手动按钮）。
///
/// 启动 60s 后跑第一轮，之后每 6h；清理走 `clean_files`（不无脑 VACUUM），
/// 只有真删了东西才补一次 VACUUM —— 整段在 spawn_blocking 里做：
/// 文件遍历/删除与偶发 VACUUM 都不该占用 tokio worker，更不跨 await 持 db 锁。
pub fn spawn_cache_auto_clean(s: &std::sync::Arc<crate::state::AppState>) {
    let st = s.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
            let st2 = st.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let policy = load_policy(&st2);
                let dirs = media_dirs(&st2);
                let report = cache_cleaner::clean_files(&dirs, policy);
                if report.removed > 0 {
                    let dbc = st2.db.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = dbc.execute_batch("VACUUM");
                    drop(dbc);
                }
                if report.removed > 0 {
                    st2.logger.info(
                        "storage",
                        format!(
                            "自动缓存清理：删 {} 个文件、释放 {} 字节",
                            report.removed, report.freed_bytes
                        ),
                    );
                }
            })
            .await;
        }
    });
}

/// SQLite 数据库文件占用（含 -wal / -shm 两个伴随文件）。
fn db_file_bytes(s: &AppState) -> u64 {
    let base = s.db_path.to_string_lossy().to_string();
    let mut total = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        if let Ok(m) = std::fs::metadata(format!("{base}{suffix}")) {
            total += m.len();
        }
    }
    total
}

/// 存储占用与当前清理策略。
#[tauri::command(async)]
pub fn get_cache_info(state: State<'_, Arc<AppState>>) -> CacheInfo {
    let s = state.inner();
    let policy = load_policy(s);
    let (media_count, media_bytes) = cache_cleaner::usage(&media_dirs(s));
    CacheInfo {
        media_count,
        media_bytes,
        db_bytes: db_file_bytes(s),
        retention_days: policy.retention_days,
        max_bytes: policy.max_bytes,
    }
}

/// 设置缓存清理策略（保留时长 / 磁盘配额；`None` 或 `0` 表示不限制）。
#[tauri::command(async)]
pub fn set_cache_policy(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    retention_days: Option<u32>,
    max_bytes: Option<u64>,
) -> Result<(), String> {
    let s = state.inner();
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let d = retention_days.unwrap_or(0);
    db::set_setting(&dbc, RETENTION_KEY, &d.to_string()).map_err(|e| e.to_string())?;
    let m = max_bytes.unwrap_or(0);
    db::set_setting(&dbc, MAX_BYTES_KEY, &m.to_string()).map_err(|e| e.to_string())?;
    // patch 在**持锁时**读好（见 settings_patch_values 的调用约定），再放锁、再广播
    let changed = ["retentionDays", "maxBytes"];
    let patch = settings_patch_values(&dbc, &changed);
    drop(dbc);
    state.notify_settings_changed(&changed, Some(window.label()), patch);
    Ok(())
}

/// 立即执行一次清理：按保留时长 / 配额删除过期的图片与文件（含历史遗留 cache 目录），
/// 并对数据库执行 VACUUM。**不删除聊天文字**；被清理的图片/文件在历史消息里将无法再打开。
#[tauri::command(async)]
pub fn clean_cache_now(state: State<'_, Arc<AppState>>) -> CleanupReport {
    let s = state.inner();
    let policy = load_policy(s);
    let dirs = media_dirs(s);
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    cache_cleaner::clean(&dirs, policy, &dbc)
}
