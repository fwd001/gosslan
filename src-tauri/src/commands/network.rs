// 职责边界：
// - 网络启停（start/stop_network）、网卡枚举
// - 拓扑查询、心跳、在线状态
// ---------------- 网络控制 ----------------

#[tauri::command(async)]
pub async fn start_network(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    bind_ip: String,
) -> Result<RuntimeSnapshot, String> {
    let arc = state.inner().clone();
    network::start(arc.clone(), bind_ip).await?;
    {
        // 作用域块：MutexGuard 在 await 之前就结束（否则 async 命令的 future 不是 Send）
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_lan_enabled(&dbc, true).map_err(|e| e.to_string())?;
    }
    // 起网络也是一次"运行状态变了"：返回快照给发起窗口，同时广播给其它窗口
    let snap = build_runtime_snapshot(&arc).await;
    arc.notify_runtime_changed(snap.clone(), Some(window.label()));
    Ok(snap)
}

#[tauri::command(async)]
pub async fn stop_network(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
) -> Result<RuntimeSnapshot, String> {
    let arc = state.inner().clone();
    network::stop(&arc).await;
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_lan_enabled(&dbc, false).map_err(|e| e.to_string())?;
    }
    let snap = build_runtime_snapshot(&arc).await;
    arc.notify_runtime_changed(snap.clone(), Some(window.label()));
    Ok(snap)
}

#[tauri::command(async)]
pub async fn get_peers(state: State<'_, Arc<AppState>>) -> Result<Vec<Peer>, String> {
    let mut peers: Vec<Peer> = state
        .inner()
        .peers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    peers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    fill_peer_links(state.inner(), &mut peers).await;
    Ok(peers)
}

/// 给 peer 列表补上**实际链路类型**（`Peer::link`）。
///
/// 为什么在命令里补、而不是让事件也带：`links` 是**异步锁**（tokio::Mutex），
/// 而节点表推送（`peers-updated`）是同步上下文 —— 那里的 `Peer::link` 恒为 None。
/// 界面只把"字段存在且是 bluetooth"当作**真的蓝牙直连**；
/// 以前用 `ip || 蓝牙直连` 反推，会把同一 Tailscale 网段（Routed）的设备也标成蓝牙直连
/// （用户 2026-09-12 实测）。
async fn fill_peer_links(s: &Arc<AppState>, peers: &mut [Peer]) {
    let kinds: std::collections::HashMap<String, Vec<crate::mesh::PathKind>> = {
        let links = s.links.lock().await;
        links
            .iter()
            .map(|(id, ls)| (id.clone(), ls.iter().map(|l| l.path_kind).collect()))
            .collect()
    };
    for p in peers.iter_mut() {
        p.link = kinds
            .get(&p.device_id)
            .and_then(|k| crate::state::best_link_kind(k))
            .map(|k| k.as_str().to_string());
    }
}

/// 按需探测周围在线节点：群发一次 `who_has`，等待约 1.5s 收集单播回复后返回当前节点表。
/// 仅在用户打开「添加好友」时调用，避免启动时持续全网扫描。
///
/// ⚠️ 2026-09-13：**同时触发一次 BLE 立刻扫描**。用户实测的体感是"蓝牙搜不到"，
/// 而真因是 BLE 的周期扫描（当时 10s 一轮）与用户动作**完全错开** ——
/// 点开「添加好友」后最多要等一整个周期才可能看到对端。
/// 蓝牙那条路没有 `who_has` 这种"喊一声"的机制，能做的最接近的事就是**立刻扫一轮**。
#[tauri::command]
pub async fn search_nearby_peers(state: State<'_, Arc<AppState>>) -> Result<Vec<Peer>, String> {
    let s = state.inner();
    // 触发一次探测（若网络已启动）
    let triggered = if let Some(tx) = s.probe.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let next = tx.borrow().saturating_add(1);
        let _ = tx.send(next);
        true
    } else {
        false
    };
    // 让 BLE 也立刻扫一轮（通道没开时返回 false，不影响 LAN 那条路）
    #[cfg(feature = "bluetooth")]
    let ble_triggered = crate::network::ble::trigger_scan_now(s);
    #[cfg(not(feature = "bluetooth"))]
    let ble_triggered = false;
    // 等待节点单播回复（BLE 的扫描窗口是 2s，所以这里取 2s：两边都覆盖得到）
    if triggered || ble_triggered {
        tokio::time::sleep(Duration::from_millis(2000)).await;
    }
    let mut peers: Vec<Peer> = s
        .peers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    peers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    fill_peer_links(s, &mut peers).await;
    Ok(peers)
}

/// 更新「应用是否在前台且窗口聚焦」（用户 2026-09-13 的功耗策略）。
///
/// 前端在 `visibilitychange`（页面是否可见）与 `focus` / `blur`（PC 窗口是否聚焦）时调用。
/// 蓝牙扫描循环据此在快/慢节奏间切换：
///   · 前台 / 聚焦 ⇒ 5s 一轮（发现更快，加好友不用干等）；
///   · 后台 / 失焦 ⇒ 30s 一轮（省电；好友申请仍能最终到达）；
///   · 进程退出 / 被系统杀死 ⇒ 扫描任务随进程消失，无需额外代码。
///
/// 从后台变回前台时额外 **wake** 一次扫描，立刻补一轮，而不是等完当前慢周期。
#[tauri::command(async)]
pub fn set_app_active(state: State<'_, Arc<AppState>>, active: bool) {
    use std::sync::atomic::Ordering;
    let s = state.inner();
    if s.app_active.swap(active, Ordering::Relaxed) == active {
        return;
    }
    #[cfg(feature = "bluetooth")]
    {
        crate::network::ble::wake_scan(s);
        s.logger.info(
            "ble",
            format!(
                "[SCAN] 应用{} ⇒ 扫描节奏切换为 {}",
                if active {
                    "回到前台/聚焦"
                } else {
                    "进入后台/失焦"
                },
                if active { "5s" } else { "30s" }
            ),
        );
    }
}

/// 从后台唤起并聚焦主窗口（冷启动首显 / 点击系统通知 / 消息点击唤起）。
#[cfg(desktop)]
#[tauri::command(async)]
pub fn focus_window(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Err("主窗口不存在".to_string());
    };
    // 冷启动白闪修复：窗口 show 的第一帧会露出 WebView2 的默认背景色（tauri.conf.json
    // 写死浅色 #edf1f6）。暗色主题用户在骨架合成前会看到"闪一下白"。show 之前把窗口
    // 底色改成跟随主题（浅 #edf1f6 / 深 #0b1220，与 body 的 --gosslan-app-bg 一致），
    // 第一帧即正确底色而非浅色。dark_mode 是"解析后的结果"（跟随系统时已按系统偏好算好），
    // 冷启动直接可用。
    let dark = {
        let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, "dark_mode")
            .map(|v| v == "1")
            .unwrap_or(false)
    };
    let color = if dark {
        tauri::window::Color(11, 18, 32, 255) // #0b1220
    } else {
        tauri::window::Color(237, 241, 246, 255) // #edf1f6
    };
    let _ = win.set_background_color(Some(color));
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

/// 移动端无独立窗口概念，系统通知自带唤起行为，无需额外处理。
#[cfg(mobile)]
#[tauri::command]
pub fn focus_window(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

/// 新消息到达时提请用户注意：Windows 闪任务栏按钮、macOS 弹跳 Dock 图标，**直到应用获得焦点**。
///
/// 为什么是 `Critical` 而不是 `Informational`（tao 0.35 的实现差异）：
///   · Windows：`FLASHW_ALL | FLASHW_TIMERNOFG`，闪窗口边框 + 任务栏按钮，`uCount = u32::MAX`
///     ⇒ 一直闪到窗口回到前台；`Informational` 是 `FLASHW_TRAY` + 4 次，一闪而过。
///   · macOS：`NSApp.requestUserAttention(CriticalRequest)` ⇒ Dock 图标**持续弹跳**；
///     `Informational` 只弹一下。
/// 微信就是"闪到你看为止"，所以这里用 `Critical`。
///
/// **不提供"停止闪烁"接口**：撤销由系统负责（窗口获得焦点即停），前端多此一举反而会出现
/// "通知已关但还在闪"的状态不一致。
///
/// **为什么值得存在**：系统通知（toast）会被用户的「通知总开关 / 专注助手」静默丢弃
/// ——2026-09-17 本机实测就是 `ToastEnabled = 0`，应用侧发送成功但用户什么都看不到。
/// 闪烁与 Dock 弹跳不经过通知中心，不受该开关影响，是目前唯一"关不掉"的提醒途径。
///
/// **必须是同步命令**：macOS 分支在 tao 内部直接 `NSApp(mtm).requestUserAttention(..)`，
/// 只能从主线程调用；同步命令由 wry 的 IPC 回调在主线程内联执行（Windows 分支自己会
/// 把 `FlashWindowEx` 投递到窗口线程，两边都安全）。函数体不含任何重资源访问，
/// 不会触发 `blocking_commands_run_off_the_main_thread` 守卫。
#[cfg(desktop)]
#[tauri::command]
pub fn request_attention(app: tauri::AppHandle) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Err("主窗口不存在".to_string());
    };
    // 失败不致命：个别 Linux 桌面环境 / 远程会话不支持，忽略即可（前端也不关心结果）。
    let _ = win.request_user_attention(Some(tauri::UserAttentionType::Critical));
    Ok(())
}

/// 移动端没有任务栏可闪；系统通知本身就是强提醒，静默降级。
#[cfg(mobile)]
#[tauri::command]
pub fn request_attention(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

/// 更新未读提醒：托盘红点 + tooltip 条数 + Windows 任务栏按钮角标 / macOS Dock 数字。
///
/// 为什么整件事放后端：前端要改的是**托盘图标、任务栏覆盖图标、Dock 标签**三种平台原生物，
/// 在 WebView 里做等于把平台判断搬进前端（还要判 isMobile）。前端只说"未读是 N 条"，
/// 后端一处决定怎么表达。
///
/// 幂等：前端按未读总数去抖后推送，重复用同一个值调用没有副作用。失败静默 ——
/// 角标是锦上添花，不该因为某个桌面环境不支持就影响聊天。
#[cfg(desktop)]
#[tauri::command]
pub fn set_unread_badge(app: tauri::AppHandle, count: u32) -> Result<(), String> {
    crate::tray::set_unread_badge(&app, count);
    Ok(())
}

/// 移动端桩：启动器角标（Android/iOS）由系统通知通道负责，没有托盘图标可改。
///
/// ⚠️ 参数必须叫 `count`（不能写成 `_count`）：Tauri 按**参数名**匹配前端传来的 JSON key
/// （宏里是 `ident.unraw().to_string()` 后转 camelCase），改名就等于换了 key。
#[cfg(mobile)]
#[tauri::command]
pub fn set_unread_badge(_app: tauri::AppHandle, count: u32) -> Result<(), String> {
    let _ = count;
    Ok(())
}

/// 桌面消息通知：前端在"应用在后台 / 正在看别的会话"时调用。
///
/// 走 crate::notifications（能返回真实错误），并**再判一次总开关**（前端已判，这里是
/// 第二道闸门：后端也能独立触发通知，不能只依赖前端状态）。返回 false 表示用户关了通知。
///
/// `conv_id` = 这条通知属于哪个会话：用户**点通知**时（Windows 上由 notify-rust 的
/// handle 捕获）后端唤起主窗口并把这个 id 发给前端，前端据此直接定位过去。
/// 没有它就只能"唤起窗口但停在原来的会话上"——用户报的正是这个。
#[tauri::command(async)]
pub fn notify_desktop(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    title: String,
    body: String,
    conv_id: String,
) -> Result<bool, String> {
    let s = state.inner().clone();
    let click_app = app.clone();
    crate::notifications::show_click_if_enabled(
        &s,
        &title,
        &body,
        std::collections::HashMap::new(),
        move || crate::notifications::on_notification_clicked(&click_app, "chat", Some(conv_id)),
    )
}

/// 设置页「发送测试通知」：忽略总开关（用户显式要试），但**如实返回失败原因**。
///
/// 为什么需要：Windows 上通知失败可能完全静默（未安装的 exe 没注册 AUMID、专注助手/勿扰、
/// 系统里把 Gosslan 的通知关了）。没有这个入口，用户只能描述"收不到"，我们无法判断是
/// 应用链路问题还是系统设置问题。
#[tauri::command(async)]
pub fn send_test_notification(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let s = state.inner();
    match crate::notifications::show(
        &s.app,
        "Gosslan 测试通知",
        "如果你看到这条系统通知，说明通知链路正常。",
    ) {
        Ok(()) => {
            s.logger.info("notify", "测试通知已发送");
            Ok(crate::notifications::platform_hint().to_string())
        }
        Err(e) => {
            s.logger.warn("notify", format!("测试通知发送失败：{e}"));
            Err(format!("{e}。{}", crate::notifications::platform_hint()))
        }
    }
}

/// 网络拓扑摘要：节点数、中继数、平均时延。
#[tauri::command(async)]
pub fn get_topology(state: State<'_, Arc<AppState>>) -> TopologyInfo {
    let s = state.inner();
    let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
    let node_count = peers.len();
    let rtts: Vec<u64> = peers.values().filter_map(|p| p.rtt_ms).collect();
    let avg_rtt_ms = if rtts.is_empty() {
        None
    } else {
        Some(rtts.iter().sum::<u64>() / rtts.len() as u64)
    };
    let relay_count = s
        .relay
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active_sends();
    let online = s
        .network
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    TopologyInfo {
        node_count,
        relay_count,
        avg_rtt_ms,
        online,
    }
}
