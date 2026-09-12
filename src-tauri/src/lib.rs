//! Gosslan 应用入口（库目标，供 Tauri 加载）。

mod commands;
/// 公开给 `examples/e2e_peer.rs` 协议级 E2E 测试对端复用（线格式与密码学原语）。
pub mod crypto;
mod db;
mod device;
pub mod discovery;
/// 聊天记录导出（纯文字单文件）：磁盘满 / 换机时的自救手段。
pub mod export;
mod gossip_engine;
mod logging;
pub mod mesh;
mod network;
pub mod protocol;
mod relay_manager; // 文件切片中继（BitTorrent 式分发），与 `mesh::router` 无关
mod user_dirs;
mod state;
mod storage;
mod transport;
#[cfg(desktop)]
mod tray;
/// macOS 原生菜单栏（自绘标题栏 + `decorations: false` 导致系统菜单栏缺失，需补回）。
/// 只在 macOS 建：Windows / Linux 用自绘标题栏，加系统菜单条会顶在标题栏之上破坏布局。
#[cfg(target_os = "macos")]
mod menu;
/// 打开本地文件的平台实现：macOS 用 NSWorkspace（沙盒下 /usr/bin/open 被拦）、
/// Android 用 FileProvider + ACTION_VIEW（opener 只发 file:// 会被系统拒绝）、
/// Windows/Linux 走 opener。
mod open_path;
/// Android 的「打开文件」JNI 桥（FileProvider：私有目录文件必须以 content:// 交出去）。
#[cfg(target_os = "android")]
mod android_open;
/// JNI 方法登记宏（Android 的两条桥共用，见该文件注释）。
#[cfg(target_os = "android")]
mod jni_method;
/// macOS 窗口外观：运行时加 squircle 圆角 + 关掉与圆角不兼容的系统阴影。
/// 见 macos_window.rs 注释（与自绘标题栏的取舍）。
#[cfg(target_os = "macos")]
mod macos_window;
/// macOS App Sandbox 的安全作用域书签（共享目录重启后不失访，见该文件注释）。
#[cfg(target_os = "macos")]
mod macos_bookmark;
mod nickname;

use tauri::Manager;

/// 运行时会创建的全部窗口标签。
///
/// **为什么要有这份清单**：Tauri 的 capability 是按**窗口标签**匹配的（见 tauri 源码
/// `webview::mod.rs` 里 `resolve_access(cmd, window.label(), webview.label(), origin)`）。
/// 运行期新建的窗口如果没有被任何 capability 覆盖，它的 `plugin:*` 与 `core:*` 调用会被
/// ACL 直接拒绝 —— 而本项目没有 app ACL manifest（`src-tauri/permissions/` 不存在），
/// 本地来源的**自定义命令不受 ACL 校验**，所以窗口看起来"基本能用"，
/// 只有选目录（dialog）、开链接（opener）、订阅事件（core:event）这些静默失败，很难发现。
/// 曾经的真实缺陷：capability 只写了 `["main"]`，于是设置窗口里"选择共享目录"必然失败。
///
/// 因此：**新建窗口时必须把标签加进这里**，`capability_covers_every_window_label` 测试
/// 会拿它去核对 `capabilities/default.json`（改错就红）。
pub const WINDOW_MAIN: &str = "main";
/// 独立的「设置」窗口（`open_settings_window`）。
pub const WINDOW_SETTINGS: &str = "settings";
/// 独立的「运行日志」窗口（`open_log_window`）。
pub const WINDOW_LOGS: &str = "logs";
/// 主窗口在 `tray::MAIN_WINDOW_LABEL` 也有一份（那里是 `#[cfg(desktop)]`），
/// 测试里断言两者一致，避免漂移。
pub const WINDOW_LABELS: &[&str] = &[WINDOW_MAIN, WINDOW_SETTINGS, WINDOW_LOGS];

/// 安装 panic hook：把 panic（位置 + 消息）写进应用日志文件，并打到 stderr。
///
/// 为什么必须装（用户 2026-09-12 安卓实测「点进去 3 秒闪退，拿不到任何日志」）：
/// 默认的 panic 输出在安卓上**看不到**（stdout/stderr 不进文件、Release 也没人读），
/// 而 `[profile.release] panic = "abort"` 时进程直接消失 ⇒ 用户和我都无从下手。
/// 现在 panic 会以 `channel="panic"` 落进「运行日志」页，adbd 下也能从 logcat 捞到。
/// ⚠️ 这个 hook 必须在**任何可能 panic 的代码之前**装好（`run()` 的第一行）。
/// 启动路标：此刻 logger 可能还没就绪，所以只打 logcat（`log -t gosslan`）——
/// 用户「打开就闪退」时，这一步能告诉我们崩在 init 之前还是之后。
fn state_mark(target: &str, message: &str) {
    #[cfg(target_os = "android")]
    {
        use std::process::{Command, Stdio};
        let line = format!("[info] [{target}] {message}");
        let _ = Command::new("log")
            .args(["-t", "gosslan", &line])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (target, message);
    }
}

fn install_panic_hook(app: Option<tauri::AppHandle>) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "(未知位置)".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(无消息)".to_string());
        let text = format!("panic @ {location}：{payload}");
        // ① 文件日志（应用内「运行日志」页能看到）——
        //    通过 AppHandle 取 `Arc<AppState>` 的 logger，避免给 Logger 加 Clone
        if let Some(handle) = &app {
            use tauri::Manager as _;
            if let Some(st) = handle.try_state::<std::sync::Arc<crate::state::AppState>>() {
                st.logger.error("panic", text.clone());
            }
        }
        // ② stderr：android logcat / 终端都能捞到（`adb logcat | grep -i panic`）
        eprintln!("[gosslan]{text}");
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 先装 hook（此刻还没有 state，先只打 stderr；setup 里拿到 logger 后再装一次带上文件日志）
    install_panic_hook(None);
    // 移动端没有那段"桌面才加插件"的 `builder = builder.plugin(...)`（见下面的 #[cfg(desktop)]），
    // 于是 `mut` 在移动端是多余的 —— 显式标注而不是去掉 `mut`（桌面端确实要改）。
    #[cfg_attr(mobile, allow(unused_mut))]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init());

    // 桌面端记住窗口尺寸/位置（HIG：重开应用恢复窗口状态，macOS/Windows 一致）。
    // 只保存 SIZE/POSITION/MAXIMIZED/FULLSCREEN，**刻意排除 VISIBLE**：
    // 本应用「关闭=隐藏到托盘」，若把 visible 也持久化，会记成"关闭后是隐藏态"，
    // 重启就可能不再显示窗口。DECORATIONS 也排除——自绘标题栏由本项目自己管理。
    #[cfg(desktop)]
    {
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED
                        | tauri_plugin_window_state::StateFlags::FULLSCREEN,
                )
                .build(),
        );
    }

    let app = builder
        .setup(|app| {
            state_mark("boot", "AppState::init 之前");
            let state = state::AppState::init(app.handle().clone())?;
            state.logger.info("boot", "AppState::init 完成（设置/目录/局域网就绪）");
            // 拿到 AppHandle 之后**重装** panic hook：这次的 hook 会把 panic 同时写进
            // 「运行日志」页（安卓上这是唯一能拿到的诊断路径，见 `install_panic_hook`）。
            install_panic_hook(Some(app.handle().clone()));
            state::AppState::spawn_peer_emitter(&state);
            app.manage(state.clone());
            // 系统托盘：关闭主窗口仅隐藏到托盘，退出需走托盘菜单
            // ⚠️ 三条语句必须一起包进 `#[cfg(desktop)]` 块：属性只作用于**紧跟其后的那一条**，
            //    拆开写会让 `tray::setup(...)` 掉出 cfg ⇒ 移动端编不过（E0433，真踩过）。
            #[cfg(desktop)]
            {
                state.logger.info("boot", "开始 tray::setup");
                tray::setup(app.handle())?;
                state.logger.info("boot", "tray::setup 完成");
            }
            // macOS 菜单栏：⌘Q / ⌘, / ⌘W / ⌘M 与标准「编辑」项。
            // 属"锦上添花"——初始化失败**不阻断启动**（与托盘不同：托盘失败会改行为，
            // 菜单失败只是没有菜单，快捷键还有前端兜底）。
            #[cfg(target_os = "macos")]
            {
                // 初始语言取持久化的**显式**偏好（"跟随系统"交给前端推到 `set_ui_language`，
                // 避免后端再实现一份系统语言检测 —— 见 menu.rs 顶部注释）。
                let persisted = {
                    let conn = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::get_setting(&conn, "language")
                };
                let lang = menu::initial_lang(persisted.as_deref());
                if let Err(e) = menu::setup(app.handle(), lang) {
                    state.logger.warn("menu", format!("菜单栏初始化失败（不影响启动）：{e}"));
                }
            }
            // macOS：`decorations: false` 使 tao 以 `Borderless`（不含 `Closable` 位）样式
            // 掩码创建 NSWindow，AppKit 据此把「关闭窗口」菜单项（Cmd+W / performClose:）
            // 判为不可用，导致 Cmd+W 无效。窗口创建后补回 `Closable` 位，恢复系统原生
            // Cmd+W（仍无标题栏/关闭按钮，因未加 `Titled` 位）；关闭动作照旧走
            // CloseRequested → 托盘隐藏路径，与点击自定义标题栏「×」行为一致。
            #[cfg(all(desktop, target_os = "macos"))]
            if let Some(win) = app.handle().get_webview_window(tray::MAIN_WINDOW_LABEL) {
                if let Err(e) = win.set_closable(true) {
                    state.logger.warn("window", format!("恢复 macOS Cmd+W 关闭能力失败: {e}"));
                }
                // 关系统阴影（与圆角冲突；NSWindow 级、不被 wry 替换 contentView 影响）。
                // 圆角本身在 WebView 加载完成后由前端调 `apply_macos_window_shape` 命令设置
                // （wry 在窗口显示时才用 parent_view 替换 contentView，setup 阶段设圆角会丢）。
                macos_window::disable_shadow(&win);
            }
            // 窗口以 `visible: false` 创建（见 tauri.conf.json），由前端在挂载完成后调用
            // `focus_window` 显示——目的是让窗口露出来的第一帧就是 index.html 的内联骨架，
            // 消除暗色主题下"整屏浅色一闪"（窗口静态 backgroundColor 只能是浅色或深色之一）。
            //
            // 兜底：前端若因初始化异常没能调用，必须仍然把窗口显示出来。否则用户面对的是
            // 「应用启动了、却没有窗口」——比闪一下白严重得多，且无法自行恢复。
            // 只在"确定处于隐藏态"时才强制显示，避免 4 秒后抢走用户当前焦点。
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();
                let st_timeout = state.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                    if let Some(win) = handle.get_webview_window(tray::MAIN_WINDOW_LABEL) {
                        if matches!(win.is_visible(), Ok(false)) {
                            st_timeout
                                .logger
                                .warn("window", "前端未在超时内显示窗口，兜底显示");
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                });
            }
            // 局域网默认开启：首次安装（以及尚未写入该键的旧版本升级）启动即自动联网；
            // 用户在设置页关闭后持久化为关闭，重启不再联网。「恢复默认」清除该键 ⇒ 回到默认开启。
            // GOSSLAN_AUTOSTART=1 强制以 0.0.0.0 开启（headless 多实例互测，
            // examples/e2e_peer.rs 依赖此行为），且不改动已持久化的偏好。
            #[cfg(desktop)]
            {
                let st = state.clone();
                tauri::async_runtime::spawn(async move {
                    let forced = std::env::var("GOSSLAN_AUTOSTART").ok().as_deref() == Some("1");
                    // 键缺失（首次安装 / 旧版本升级）→ 持久化为 true，之后每次启动
                    // 读到明确的 "1" 而非依赖 unwrap_or(true) 的隐式默认。
                    let enabled = {
                        let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                        crate::db::get_lan_enabled(&dbc)
                    };
                    if forced || enabled {
                        // st 即将 move 进 start，先 clone 一份用于失败日志。
                        let st_log = st.clone();
                        let started = if forced {
                            network::start(st, "0.0.0.0".to_string()).await
                        } else {
                            network::start_from_prefs(st).await
                        };
                        if let Err(e) = started {
                            st_log.logger.error("lan", format!("自动开启局域网通道失败: {e}"));
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_device_info,
            commands::update_profile,
            commands::list_interfaces,
            commands::get_runtime_snapshot,
            commands::start_network,
            commands::stop_network,
            commands::get_peers,
            commands::search_nearby_peers,
            commands::focus_window,
            commands::get_topology,
            commands::set_channel_enabled,
            commands::get_cache_info,
            commands::set_cache_policy,
            commands::clean_cache_now,
            commands::get_settings,
            commands::log_frontend_error,
            commands::import_picked_file,
            commands::default_nickname,
            commands::request_ble_permissions,
            commands::set_ui_language,
            commands::search_chat_history,
            commands::save_settings,
            commands::reset_settings,
            commands::broadcast_chat_style,
            commands::get_friends,
            commands::remove_friend,
            commands::get_pending_requests,
            commands::send_friend_request,
            commands::respond_friend_request,
            commands::send_message,
            commands::get_messages,
            commands::get_conv_link,
            commands::get_message_count,
            commands::get_conversations,
            commands::ensure_conversation,
            commands::mark_read,
            commands::delete_conversation,
            commands::create_group,
            commands::distribute_group_key,
            commands::rename_group,
            commands::group_add_member,
            commands::group_remove_member,
            commands::transfer_group_creator,
            commands::leave_group,
            commands::get_groups,
            commands::get_group_reads,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_is_maximized,
            commands::window_toggle_fullscreen,
            commands::window_close,
            commands::send_group_message,
            commands::send_group_file,
            commands::get_group_file_delivery_summary,
            commands::send_file,
            commands::send_file_auto,
            commands::send_file_relay,
            commands::get_transfers,
            commands::set_share_dir,
            commands::get_share_dir,
            commands::get_downloads_dir,
            commands::set_downloads_dir,
            commands::open_downloads_dir,
            commands::request_share_tree,
            commands::download_shared_file,
            commands::copy_file,
            commands::copy_file_to_clipboard,
            commands::read_clipboard_file_paths,
            commands::save_data_file,
            commands::delete_file,
            commands::save_outgoing_image,
            commands::open_file_native,
            commands::apply_macos_window_shape,
            commands::read_file_preview,
            commands::media_present,
            commands::export_chat_text,
            commands::search_messages,
            commands::clear_all_data,
            commands::get_discovery_diag,
            commands::get_interface_candidates,
            commands::list_routed_endpoints,
            commands::add_routed_endpoint,
            commands::remove_routed_endpoint,
            commands::get_logs,
            commands::clear_logs,
            commands::open_log_window,
            commands::close_log_window,
            commands::open_settings_window,
            commands::close_settings_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app_handle, event| {
        // macOS：点击 Dock 图标且无可见窗口时恢复主窗口（避免「程序没反应」的错觉）
        #[cfg(all(desktop, target_os = "macos"))]
        if let tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } = event
        {
            if !has_visible_windows {
                tray::show_main_window(app_handle);
            }
        }
        #[cfg(not(all(desktop, target_os = "macos")))]
        let _ = (app_handle, event);
    });
}


#[cfg(test)]
mod tests {
    use super::*;

    /// capability 的 `windows` 模式匹配。Tauri 内部用 glob；本项目只需要支持
    /// 全匹配 / `前缀*` / `*后缀` 三种写法（够用且不引入新依赖）。
    fn window_pattern_matches(pattern: &str, label: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return label.starts_with(prefix);
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            return label.ends_with(suffix);
        }
        pattern == label
    }

    /// 匹配器本身不许"永远为真"（否则下面的守卫会变成空转）。
    #[test]
    fn window_pattern_matcher_is_not_vacuous() {
        assert!(window_pattern_matches("main", "main"));
        assert!(!window_pattern_matches("main", "settings"));
        assert!(window_pattern_matches("*", "settings"));
        assert!(window_pattern_matches("set*", "settings"));
        assert!(window_pattern_matches("*ings", "settings"));
        assert!(!window_pattern_matches("settings", "settings-extra"));
    }

    /// 每一个会创建窗口的标签都必须被 capability 覆盖 —— 否则那个窗口的
    /// `plugin:*` / `core:*` 调用会被 ACL 拒绝（本项目没有 app ACL manifest，
    /// 自定义命令不校验，所以症状是"设��窗口里选目录/开链接/订阅事件静默失败"）。
    #[test]
    fn capability_covers_every_window_label() {
        let raw = include_str!("../capabilities/default.json");
        let caps: serde_json::Value =
            serde_json::from_str(raw).expect("capabilities/default.json 必须是合法 JSON");
        let patterns: Vec<String> = caps
            .get("windows")
            .and_then(|v| v.as_array())
            .expect("capabilities/default.json 缺少 windows 数组")
            .iter()
            .map(|v| {
                v.as_str()
                    .expect("windows 数组项必须是字符串")
                    .to_string()
            })
            .collect();
        for label in WINDOW_LABELS {
            assert!(
                patterns.iter().any(|p| window_pattern_matches(p, label)),
                "窗口 `{label}` 没有被任何 capability 覆盖（windows = {patterns:?}）"
            );
        }
    }

    /// **规则式**守卫：任何会碰重资源（数据库 / 文件系统 / 日志 / 剪贴板 / 网卡枚举 / 阻塞睡眠）
    /// 的命令，都必须声明 `#[tauri::command(async)]`。
    ///
    /// 依据（读过上游源码，不是猜的）：
    /// - `tauri-macros` 的 `body_blocking` 把命令函数**内联调用**在 IPC 处理器里；
    ///   只有 `ExecutionContext::Async` 才走 `respond_async_serialized`
    ///   → `crate::async_runtime::spawn(...)`（异步运行时线程）。
    /// - wry 的 `WKScriptMessageHandler::did_receive` 在 AppKit 消息循环里同步回调
    ///   （`wry-*/src/wkwebview/class/wry_web_view_delegate.rs`），即 **macOS 主线程**。
    ///
    /// 于是同步命令 = 在 UI 主线程上跑：**整个进程**（所有窗口）都会卡住，不只是发起调用的那个窗口。
    ///
    /// 为什么从"名字清单"改成"规则"：清单只能盯住写清单时想到的那几个。
    /// 真实事故（2026-09-12 用户反馈"点清除数据/恢复，设置窗口直接卡死；点添加好友主窗口卡死"）：
    /// 清单里 12 个命令是 async，但**另外 42 个**（`get_settings`/`get_friends`/`get_transfers`/
    /// `get_logs`/`list_interfaces`/`reset_settings`/`open_settings_window` …）仍是同步命令。
    /// 它们本身很快，但 `clear_all_data` 那种长事务会把 `db` 互斥锁握住数秒，
    /// 于是这些同步读**在主线程上等锁** ⇒ 两个窗口一起冻住。
    /// 规则式守卫能覆盖"以后新加的命令"，名字清单不能。
    #[test]
    fn blocking_commands_run_off_the_main_thread() {
        let src = include_str!("commands.rs");
        // 例外必须写在这里并交代理由（当前为空：纯窗口操作天然不含下列标记）
        const ALLOWED: [&str; 0] = [];

        // 重资源标记 → 人类可读的原因
        let markers: [(&str, &str); 9] = [
            (".db", "访问 SQLite（可能等锁数秒）"),
            ("db::", "访问 SQLite"),
            ("std::fs", "文件系统 IO"),
            ("logger.", "日志（含整份快照）"),
            ("Clipboard", "系统剪贴板（可能被别的程序占着）"),
            ("list_interfaces", "枚举网卡"),
            ("if_addrs", "枚举网卡"),
            ("thread::sleep", "阻塞睡眠"),
            ("block_on", "阻塞等待异步任务"),
        ];

        let mut offenders: Vec<String> = Vec::new();
        for (name, is_async, body) in command_bodies(src) {
            if is_async || ALLOWED.contains(&name.as_str()) {
                continue;
            }
            let hit: Vec<&str> = markers
                .iter()
                .filter(|(m, _)| body.contains(m))
                .map(|(_, why)| *why)
                .collect();
            if !hit.is_empty() {
                offenders.push(format!("  {name}: {}", hit.join("、")));
            }
        }
        assert!(
            offenders.is_empty(),
            "以下命令会阻塞 macOS 主线程（同步命令在 wry 的 IPC 回调里内联执行），\
             必须加 `#[tauri::command(async)]` —— 否则长事务/读盘期间**所有窗口**一起卡死：\n{}",
            offenders.join("\n")
        );
    }

    /// 拆出 `commands.rs` 里每个 `#[tauri::command…]` 的 `(名字, 是否 async, 函数体)`。
    ///
    /// 手写扫描而不是上 syn：本测试只做文本判据，不引入新依赖；字符串/注释/生命周期都跳过，
    /// 否则函数体里的花括号（`format!("{}")` 之类）会让配对错位。
    fn command_bodies(src: &str) -> Vec<(String, bool, String)> {
        let bytes = src.as_bytes();
        let mut out = Vec::new();
        let mut i = 0usize;
        while let Some(pos) = src[i..].find("#[tauri::command") {
            let attr_start = i + pos;
            let attr_end = match src[attr_start..].find(']') {
                Some(e) => attr_start + e,
                None => break,
            };
            // 两条路都算"off main thread"：属性 `#[tauri::command(async)]`（同步函数被 spawn），
            // 或函数本身是 `async fn`（宏直接走 respond_async_serialized）。
            let attr_async = src[attr_start..attr_end].contains("async");
            // 从属性之后找 `fn 名字(`
            let after = &src[attr_end..];
            let fn_pos = match after.find("fn ") {
                Some(p) => attr_end + p,
                None => {
                    i = attr_end;
                    continue;
                }
            };
            let name: String = src[fn_pos + 3..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let is_async =
                attr_async || src[attr_end..fn_pos].contains("async");
            // 花括号配对取函数体（跳过字符串/字符/注释）
            let mut depth = 0i32;
            let mut j = fn_pos;
            let mut in_str = false;
            let mut in_line_comment = false;
            let mut in_block_comment = false;
            let mut body = String::new();
            while j < bytes.len() {
                let c = bytes[j] as char;
                let next = bytes.get(j + 1).map(|b| *b as char);
                if in_line_comment {
                    if c == '\n' {
                        in_line_comment = false;
                    }
                } else if in_block_comment {
                    if c == '*' && next == Some('/') {
                        in_block_comment = false;
                        j += 1;
                    }
                } else if in_str {
                    if c == '\\' {
                        j += 1;
                    } else if c == '"' {
                        in_str = false;
                    }
                } else if c == '/' && next == Some('/') {
                    in_line_comment = true;
                    j += 1;
                } else if c == '/' && next == Some('*') {
                    in_block_comment = true;
                    j += 1;
                } else if c == '"' {
                    in_str = true;
                } else if c == '\'' {
                    // 生命周期（`'_` / `'a`）不是字符字面量：只有 `'x'` 形式才算
                    if next == Some('\\') || bytes.get(j + 2).map(|b| *b as char) == Some('\'') {
                        j += 2;
                    }
                } else if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if depth > 0 {
                    body.push(c);
                }
                j += 1;
            }
            out.push((name, is_async, body));
            i = j.max(attr_end + 1);
        }
        out
    }

    /// **Kotlin ↔ Rust 的 JNI 方法签名必须逐字对齐**。
    ///
    /// JNI 调用**不做任何编译期检查**：描述符写错只会在运行期抛 `NoSuchMethodError`，
    /// 而且只有真机上才现形。真实缺陷（本轮 code review 抓到）：
    /// Kotlin 的 `fun stop()` 是 Unit 方法（JNI `()V`），Rust 侧却用 `()Z` 调用 ⇒
    /// 用户关掉「蓝牙通道」后手机**仍在广播**（耗电 + 隐私），日志里一个字都没有。
    ///
    /// 这条护栏在**主机上**就能跑：解析 Kotlin 源码里 `fun` 的形参/返回类型推出 JNI 描述符，
    /// 与 Rust 侧 `kotlin_method!("名字", "描述符")` 的登记逐条比对；
    /// 并检查每个 `extern fn`（native 回调）在 Kotlin 里确有同名 `external fun`。
    #[test]
    fn android_jni_signatures_match_kotlin() {
        let kotlin = include_str!("../gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt");
        let rust = include_str!("transport/ble_android.rs");
        // 第二条 JNI 桥：「用系统里的其它应用打开文件」（FileProvider）。同一个坑，
        // 所以必须同一条护栏盯着 —— 新桥单独立一份检查只会漂移。
        let kotlin_open = include_str!("../gen/android/app/src/main/java/com/gosslan/app/OpenWith.kt");
        let rust_open = include_str!("android_open.rs");

        let kotlin_fns = parse_kotlin_funs(kotlin);
        let open_fns = parse_kotlin_funs(kotlin_open);
        for expected in [
            "start",
            "stop",
            "send",
            "isConnected",
            "payloadMtu",
            "nativeBootstrap",
            "nativeOnFrame",
            "nativeOnUnlinked",
            "nativeOnNotice",
            "nativeOnWarning",
        ] {
            assert!(
                kotlin_fns.iter().any(|(n, _, _)| n == expected),
                "没在 BlePeripheral.kt 里解析到 `{expected}` —— Kotlin 写法变了就要同步更新本护栏"
            );
        }
        // 打开文件的桥：Rust 调 `openWith`，Kotlin 调 `nativeAttachOpenWith`
        for (file, fns, expected) in [
            ("OpenWith.kt", &open_fns, "openWith"),
            ("OpenWith.kt", &open_fns, "nativeAttachOpenWith"),
        ] {
            assert!(
                fns.iter().any(|(n, _, _)| n == expected),
                "没在 {file} 里解析到 `{expected}` —— Kotlin 写法变了就要同步更新本护栏"
            );
        }

        // ① Rust 登记的每个 Kotlin 方法，描述符必须与 Kotlin 源码推出的**逐字相同**
        let registered = parse_kotlin_method_registrations(rust);
        assert!(
            registered.len() >= 5,
            "Rust 侧至少应登记 5 个 Kotlin 方法，实际 {}",
            registered.len()
        );
        let open_registered = parse_kotlin_method_registrations(rust_open);
        assert_eq!(
            open_registered.len(),
            1,
            "android_open.rs 应恰好登记 1 个 Kotlin 方法（openWith），实际 {} —— \
             解析器失效或有人漏登记",
            open_registered.len()
        );
        let mut static_checked = 0usize;
        for (name, desc) in registered.iter().chain(open_registered.iter()) {
            let (_, kotlin_desc, _) = kotlin_fns
                .iter()
                .chain(open_fns.iter())
                .find(|(n, _, _)| n == name)
                .unwrap_or_else(|| panic!("Rust 登记了 Kotlin 里不存在的 `{name}`"));
            assert_eq!(
                kotlin_desc, desc,
                "`{name}` 的 JNI 描述符不一致：Kotlin 是 `{kotlin_desc}`，Rust 却按 `{desc}` 调用 \
                 —— JNI 不做任何编译期检查，这只会在真机上抛 NoSuchMethodError"
            );

            // ③ Rust 用 `call_static_method` 调它 ⇒ Kotlin 侧必须有**静态桥**。
            //    顶层函数天然是 static；`object`/`class` 的成员**必须带 `@JvmStatic`** ——
            //    少了它真机日志是 `JNI 调用失败：Method not found: start ()Z`
            //    （蓝牙外设整条路径失效，而 central 扫描不受影响 ⇒ 症状极其隐蔽）。
            let (file, kt) = if kotlin_fns.iter().any(|(n, _, _)| n == name) {
                ("BlePeripheral.kt", kotlin)
            } else {
                ("OpenWith.kt", kotlin_open)
            };
            let (idx, decl) = kt
                .lines()
                .enumerate()
                .find(|(_, l)| l.contains(&format!("fun {name}(")))
                .unwrap_or_else(|| panic!("在 {file} 里找不到 `fun {name}(` 的声明行"));
            let indented = decl.starts_with(' ') || decl.starts_with('\t');
            if indented {
                let lines: Vec<&str> = kt.lines().collect();
                let from = idx.saturating_sub(12);
                let annotated = lines[from..idx].iter().any(|l| l.trim() == "@JvmStatic");
                assert!(
                    annotated,
                    "`{name}` 是 {file} 里 object/class 的成员，而 Rust 用 call_static_method 调它 \
                     ⇒ 必须在它上面加 `@JvmStatic`（否则没有静态桥：真机 `Method not found: {name}`）"
                );
            }
            static_checked += 1;
        }
        assert!(
            static_checked >= 7,
            "应检查 ≥7 个 Kotlin 方法（openWith + BlePeripheral 6 个），实际 {static_checked} —— 护栏失效了"
        );

        // ② Rust 导出的 native 回调（snake_case → lowerCamelCase）必须在 Kotlin 里是 external fun
        for snake in parse_extern_fn_names(rust)
            .into_iter()
            .chain(parse_extern_fn_names(rust_open))
        {
            let camel = snake_to_lower_camel(&snake);
            let found = kotlin_fns
                .iter()
                .chain(open_fns.iter())
                .find(|(n, _, _)| n == &camel)
                .unwrap_or_else(|| {
                    panic!(
                        "Rust 导出了 native 方法 `{snake}`（Java 名 `{camel}`），但 Kotlin 里没有这个 \
                         `external fun` —— JVM 会 UnsatisfiedLinkError"
                    )
                });
            assert!(
                found.2,
                "Kotlin 的 `{camel}` 不是 `external` 声明，JVM 不会去查 native 实现"
            );
        }
    }

    /// **JNI 的 static / 实例形态必须与 Kotlin 声明的形态一致**。
    ///
    /// 真实缺陷（2026-09-12 真机实测的**启动闪退**，而且编译、单测、构建全绿）：
    /// `OpenWith.kt` 里 `nativeAttachOpenWith()` 是**文件级（顶层）函数** ⇒ 编译成
    /// `OpenWithKt` 的 **static** 方法；而 Rust 侧的 `native_method!` 少了 `static` 关键字
    /// ⇒ 宏把它按**实例方法**注册。ART 在第一次调用时判定不一致并**直接 abort 整个进程**：
    ///   `Native method '"nativeAttachOpenWith"' was registered as instance but called as static method`
    /// （崩溃栈落在 `MainActivity.onCreate` → `OpenWith.bootstrap`）。
    ///
    /// 判据（Kotlin 的一行声明就足够）：**顶格声明的 `external fun` = 顶层 = static**；
    /// 缩进在 `object`/`class` 里的 = 成员 = 实例。两者与 Rust 的 `static` 关键字必须一一对应。
    /// 对照：`BlePeripheral` 的 `nativeBootstrap()` 在 object 内 ⇒ 实例 ⇒ 宏不加 `static`。
    #[test]
    fn jni_static_matches_kotlin_toplevel() {
        let ble_kt = include_str!("../gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt");
        let open_kt = include_str!("../gen/android/app/src/main/java/com/gosslan/app/OpenWith.kt");
        let cases = [
            (include_str!("transport/ble_android.rs"), ble_kt),
            (include_str!("android_open.rs"), open_kt),
        ];
        let mut checked = 0usize;
        for (rust, kotlin) in cases {
            for (snake, is_static) in rust_native_methods(rust) {
                let camel = snake_to_lower_camel(&snake);
                let toplevel = kotlin_fun_is_toplevel(kotlin, &camel).unwrap_or_else(|| {
                    panic!("Kotlin 里找不到 native 方法 `{camel}` —— 护栏需要同步更新")
                });
                assert_eq!(
                    is_static, toplevel,
                    "`{camel}` 的 static 形态与 Kotlin 不一致：Rust 注册为{}，Kotlin 却是{}函数 ——                      真机第一次调用时 ART 会直接 abort（进程消失、连日志都来不及写全）",
                    if is_static { "static" } else { "实例" },
                    if toplevel { "顶层（=static）" } else { "成员（=实例）" },
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 5,
            "应至少检查 5 个 native 方法（BlePeripheral 5 个 + OpenWith 1 个），实际 {checked} —— 解析器失效了"
        );
    }

    /// 解析 Rust 侧 `native_method! { … extern fn <名字> … }` → (snake 名, 是否 static)。
    fn rust_native_methods(src: &str) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("native_method! {") {
            rest = &rest[i + "native_method! {".len()..];
            let head = match rest.find("fn =") {
                Some(e) => &rest[..e],
                None => continue,
            };
            if let Some(p) = head.find("extern fn ") {
                let after = &head[p + "extern fn ".len()..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push((name, head[..p].contains("static")));
                }
            }
        }
        out
    }

    /// Kotlin 里 `fun <camel>(` 这一行是否**顶格**（顶格 = 顶层函数 = JNI static）。
    fn kotlin_fun_is_toplevel(src: &str, camel: &str) -> Option<bool> {
        let needle = format!("fun {camel}(");
        src.lines().find(|l| l.contains(&needle)).map(|l| {
            let mut chars = l.chars();
            !matches!(chars.next(), Some(' ') | Some('\t'))
        })
    }

    /// **扫描结果不得再用 `Peripheral::services()` 二次过滤**（真机踩过，症状极隐蔽）。
    ///
    /// `start_scan(ScanFilter{services})` 已在平台层过滤；而 `services()` 在 **Android 上
    /// 只有连接并 `discover_services()` 之后才有值**，未连接时恒为空 ⇒ 拿它过滤会把所有候选
    /// 丢掉。真机症状（用户 2026-09-12）：两台设备蓝牙都开着、都在广播、系统层扫描也命中，
    /// 但**一个候选都不去连**，双方永远发现不了彼此，而日志里一个字都没有。
    #[test]
    fn scan_results_are_not_filtered_by_unconnected_services() {
        let bt = include_str!("transport/bluetooth.rs");
        let body = rust_fn_body(bt, "pub async fn scan_peers(");
        assert!(
            !body.contains("p.services()") && !body.contains(".services().iter()"),
            "扫描结果必须**原样返回**：按 `services()` 过滤在 Android 上会把候选全丢掉 \
             （未连接时它恒为空集合）"
        );
        assert!(
            body.contains("Ok(all)"),
            "应当直接把平台层已经过滤好的结果返回"
        );
        assert!(
            body.contains("discover_services()") || body.contains("连接"),
            "必须在注释里写清为什么不能过滤 —— 否则下一个人很容易『顺手补一个校验』"
        );
        // 扫到候选必须留痕（否则这类缺陷在日志里完全不可见）
        let ble = include_str!("network/ble.rs");
        assert!(
            ble.contains("BLE 扫描到"),
            "扫到候选要打一条日志：真机排查时这是『到底有没有发现对端』的唯一线索"
        );
    }

    /// **release 包必须 keep 住 Rust 按名字调用的 Kotlin 方法**。
    ///
    /// R8 在 release 下会把它们改名（**实测**：`stop`/`start`/`send`/… 全变成 `a`/`b`/`c`/…），
    /// 而 JNI 只按「名字 + 签名」查找 ⇒ release 真机包上蓝牙外设整条路径 `NoSuchMethodError`。
    /// debug 包不做混淆，所以这个坑在开发期完全看不见（我是在打 release 包时才抓到的）。
    /// 这条护栏同时盯两种漂移：规则里**漏了**方法，以及规则里**多留了**已废弃的方法。
    #[test]
    fn release_keeps_every_kotlin_method_called_from_rust() {
        let rust = include_str!("transport/ble_android.rs");
        // 单一事实来源：`scripts/android/proguard-gosslan.pro`（`gen/android` 是生成物，
        // `tauri android init` 会重生它，所以注入脚本每次构建前都把这份搬进去）。
        let source = include_str!("../../scripts/android/proguard-gosslan.pro");
        let generated = include_str!("../gen/android/app/proguard-rules.pro");

        let source_block = proguard_jni_block(source)
            .expect("scripts/android/proguard-gosslan.pro 里没有 GOSSLAN_JNI 标记块");
        let generated_block = proguard_jni_block(generated).expect(
            "gen/android/app/proguard-rules.pro 里没有 GOSSLAN_JNI 标记块 —— \
             跑 `node scripts/inject-android-signing.mjs`（或任意一次 android 构建）就会补上；\
             缺了它，release 包的蓝牙在真机上会 NoSuchMethodError",
        );
        assert_eq!(
            source_block, generated_block,
            "两处 JNI keep 规则漂移了：事实来源是 scripts/android/proguard-gosslan.pro，\
             gen/android/app/proguard-rules.pro 只是构建时注入的副本"
        );

        let registered = parse_kotlin_method_registrations(rust);
        assert!(
            registered.len() >= 5,
            "Rust 侧至少应登记 5 个 Kotlin 方法，实际 {}",
            registered.len()
        );
        // 打开文件的桥（android_open.rs）同样只被 JNI 按名字调用 ⇒ 必须一起 keep。
        // 两条桥放在一起查，避免"新加的桥忘了写 keep 规则"这种只在 release 真机上现形的漏。
        let registered: Vec<(String, String)> = registered
            .into_iter()
            .chain(parse_kotlin_method_registrations(include_str!("android_open.rs")))
            .collect();
        assert!(
            registered.iter().any(|(n, _)| n == "openWith"),
            "没在 android_open.rs 里解析出 `openWith` 的 JNI 登记 —— 解析器失效了，\
             这条护栏会静默变成空转"
        );
        let kept = proguard_kept_method_names(&source_block);
        assert!(
            kept.len() >= 5,
            "从 keep 块里只解析出 {} 个方法名 —— 护栏解析器失效了（这才是真正的风险：\
             它一旦静默返回空，下面的检查就全是空转）",
            kept.len()
        );

        for (name, _) in &registered {
            assert!(
                kept.iter().any(|k| k == name),
                "keep 规则里缺少 `{name}` —— R8 会把它改名，release 真机上 JNI 找不到这个方法"
            );
        }
        for name in &kept {
            assert!(
                registered.iter().any(|(n, _)| n == name),
                "keep 规则里的 `{name}` 在 Rust 侧已经没有调用登记了（陈旧规则），删掉它"
            );
            // ⚠️ 还必须是 **static**：Rust 用 `env.call_static_method(...)` 调它们，而 Kotlin 的
            // `@JvmStatic fun x()` 在 object 里生成"实例方法 + 静态桥"两个条目 —— 只 keep 实例方法
            // 时 R8 会把静态桥当死代码删掉，真机日志：`JNI 调用失败：Method not found: start ()Z`
            // （蓝牙外设整条路径失效，central 角色不受影响，所以症状很隐蔽）。
            // 只看真正的规则行（`public …;`），别把说明注释里引用的同一句当成规则
            let line = source_block
                .lines()
                .map(str::trim)
                .find(|l| {
                    l.starts_with("public ") && l.ends_with(';') && l.contains(&format!(" {name}("))
                })
                .unwrap_or_else(|| panic!("keep 块里找不到 `{name}` 的规则行"));
            assert!(
                line.trim_start().starts_with("public static"),
                "keep 规则里的 `{name}` 必须写成 `public static …`（实际：`{}`）—— \
                 Rust 是按**静态**方法调它的，只 keep 实例方法会让真机报 `Method not found: {name}`",
                line.trim()
            );
        }
    }

    /// 取出 `# GOSSLAN_JNI_BEGIN … # GOSSLAN_JNI_END` 之间的正文（不含两端的标记行）。
    fn proguard_jni_block(src: &str) -> Option<String> {
        const BEGIN: &str = "# GOSSLAN_JNI_BEGIN";
        const END: &str = "# GOSSLAN_JNI_END";
        let start = src.find(BEGIN)? + BEGIN.len();
        let end = src[start..].find(END)? + start;
        Some(src[start..end].trim().to_string())
    }

    /// 从 keep 块里抽出被 keep 的**方法名**（跳过 `native <methods>;` 这类通配与注释）。
    fn proguard_kept_method_names(block: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            if !line.ends_with(';') || line.starts_with('#') || line.starts_with("-keep") {
                continue;
            }
            let Some(open) = line.find('(') else { continue };
            let Some(name) = line[..open].split_whitespace().last() else {
                continue;
            };
            if name.starts_with('<') {
                continue; // `native <methods>;`
            }
            out.push(name.to_string());
        }
        out
    }

    /// 解析 Kotlin 里**单行**的 `[modifiers] fun name(params): Ret` → `(名字, JNI 描述符, 是否 external)`。
    fn parse_kotlin_funs(src: &str) -> Vec<(String, String, bool)> {
        let mut out = Vec::new();
        for line in src.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            let Some(pos) = line.find("fun ") else { continue };
            let after = &line[pos + 4..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let (Some(open), Some(close)) = (after.find('('), after.find(')')) else {
                continue;
            };
            if close < open {
                continue;
            }
            let params = &after[open + 1..close];
            let ret = after[close + 1..]
                .trim_start()
                .strip_prefix(':')
                .and_then(|r| r.split_whitespace().next())
                .map(|r| r.trim_end_matches(['{', '=']))
                .filter(|r| !r.is_empty())
                .unwrap_or("Unit");
            // 只解析"Rust 可能调用"的方法：形参/返回类型里有本项目没映射过的类型
            // （例如 `bootstrap(context: Context)`）就跳过 —— 护栏只关心被登记的那几个。
            let mut desc = String::from("(");
            let mut mapped = true;
            for p in params.split(',').filter(|p| !p.trim().is_empty()) {
                let ty = p.split(':').nth(1).map(str::trim).unwrap_or("Unit");
                match jni_type(ty) {
                    Some(t) => desc.push_str(&t),
                    None => {
                        mapped = false;
                        break;
                    }
                }
            }
            if !mapped {
                continue;
            }
            desc.push(')');
            match jni_type(ret) {
                Some(t) => desc.push_str(&t),
                None => continue,
            }
            out.push((name, desc, line.contains("external")));
        }
        out
    }

    /// 解析 Rust 侧 `kotlin_method!("名字", "描述符")` 的登记。
    fn parse_kotlin_method_registrations(src: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = src;
        const NEEDLE: &str = "kotlin_method!(";
        while let Some(i) = rest.find(NEEDLE) {
            let after = &rest[i + NEEDLE.len()..];
            // 结束括号要在**引号外**找：描述符形如 `"()V"`，里面也有 `)`
            let mut end = None;
            let mut in_quotes = false;
            for (idx, ch) in after.char_indices() {
                match ch {
                    '"' => in_quotes = !in_quotes,
                    ')' if !in_quotes => {
                        end = Some(idx);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            let parts: Vec<&str> = after[..end].split(',').map(str::trim).collect();
            if parts.len() == 2 {
                out.push((
                    parts[0].trim_matches('"').to_string(),
                    parts[1].trim_matches('"').to_string(),
                ));
            }
            rest = &after[end..];
        }
        out
    }

    /// 解析 Rust 侧 `extern fn name(` 的 native 回调名。
    fn parse_extern_fn_names(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = src;
        const NEEDLE: &str = "extern fn ";
        while let Some(i) = rest.find(NEEDLE) {
            let after = &rest[i + NEEDLE.len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
            rest = after;
        }
        out
    }

    fn snake_to_lower_camel(snake: &str) -> String {
        let mut parts = snake.split('_');
        let mut out = parts.next().unwrap_or_default().to_string();
        for part in parts {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.push_str(&first.to_uppercase().collect::<String>());
                out.push_str(chars.as_str());
            }
        }
        out
    }

    /// Kotlin 类型名 → JNI 描述符；本项目没映射过的类型返回 `None`（调用方跳过那个方法）。
    ///
    /// `Context`/`BluetoothGattServer` 这类只出现在 Kotlin 内部方法的形参里，
    /// Rust 从不调用它们，因此不需要（也不该）在这里维护映射。
    ///
    /// 可空标记 `?` 对 JNI 描述符**没有影响**（`String?` 与 `String` 都是
    /// `Ljava/lang/String;`）—— 这里显式去掉再匹配，否则 `OpenWith.openWith` 这种
    /// "返回 String? 表示成功/失败原因" 的方法会被整条跳过，护栏就静默失效了。
    fn jni_type(kotlin: &str) -> Option<String> {
        Some(match kotlin.trim().trim_end_matches('?') {
            "String" => "Ljava/lang/String;".to_string(),
            "ByteArray" => "[B".to_string(),
            "Boolean" => "Z".to_string(),
            "Int" => "I".to_string(),
            "Long" => "J".to_string(),
            "Float" => "F".to_string(),
            "Double" => "D".to_string(),
            "Unit" | "" => "V".to_string(),
            _ => return None,
        })
    }

    /// 主窗口标签在 `tray` 里还有一份（那份是 `#[cfg(desktop)]`），两边不许漂移。
    #[cfg(desktop)]
    #[test]
    fn main_window_label_matches_tray_constant() {
        assert_eq!(tray::MAIN_WINDOW_LABEL, WINDOW_MAIN);
    }

    /// **每一条"同意好友"的路径都必须清掉那条申请**。
    ///
    /// 真实缺陷（用户 2026-09-12 真机实测）：双方互发过申请时，A 点了同意，B 的「新朋友」里
    /// 那条申请**还在** —— 直连路径（`Message::FriendAccept`）只加了好友、忘了清 pending，
    /// 而跨跳路径（`GossipKind::FriendAccept`）清了。同一件事两条路径行为不一致，
    /// 表现成"有时候会清、有时候不清"。这里把"两条路径都要清"钉死。
    #[test]
    fn every_friend_accept_path_forgets_the_pending_request() {
        let transport = include_str!("network/transport.rs");
        assert_eq!(
            transport.matches("forget_pending_request(state, &from)").count(),
            2,
            "两条 FriendAccept 路径（直连 `Message::FriendAccept` + 跨跳 `GossipKind::FriendAccept`）\
             都必须清掉 pending —— 少一条就会让「已经是好友了，申请还挂着」复现"
        );
        let commands = include_str!("commands.rs");
        assert_eq!(
            commands.matches("forget_pending_request(s, &peer_id)").count(),
            1,
            "`respond_friend_request` 的同意路径也要走同一个助手（别各写一遍）"
        );
        let helper = rust_fn_body(transport, "pub fn forget_pending_request(");
        assert!(
            helper.contains("remove(peer_id)"),
            "助手必须真的把内存态的 pending 删掉"
        );
    }

    /// **"申请人已经是我的好友"时，这条申请必须被自动同意**（双方关系必须收敛）。
    ///
    /// 真实缺陷（用户 2026-09-12 真机实测）：B 的好友列表里已经有 A，而 A 是**重置过的账号**、
    /// 列表里没有 B。A 发申请 → 旧实现只在 B 侧插一条 pending，而 `get_pending_requests`
    /// 又会把「申请人已是好友」的条目过滤掉（那是为了修「已经是好友了、申请还挂着」）
    /// ⇒ **两边都看不到、谁也加不上**；用户只能先把 B 里的 A 删掉再加回来。
    ///
    /// 用户给的规则：既然 B 那边已经把 A 当好友，就等于 B 已经同意了 —— 直接走完整的同意路径。
    #[test]
    fn friend_request_from_existing_friend_auto_accepts() {
        let transport = include_str!("network/transport.rs");
        let helper = rust_fn_body(transport, "async fn auto_accept_if_already_friend(");
        assert!(
            helper.contains("db::get_friend") && helper.contains("accept_friend_request"),
            "自动同意必须：① 真的判『他是不是已经是我的好友』；② 走**同一个** accept 实现（别各写一遍）"
        );
        let calls = transport.matches("auto_accept_if_already_friend(state, ").count();
        assert_eq!(
            calls, 2,
            "两条 FriendRequest 路径（直连 `Message::FriendRequest` + 跨跳 `GossipKind::FriendRequest`）\
             都必须先做自动同意 —— 只修一条就会『同一件事两种行为』（这正是上一条缺陷的成因）"
        );
        let commands = include_str!("commands.rs");
        assert_eq!(
            commands.matches("pub(crate) async fn accept_friend_request(").count(),
            1,
            "『同意好友』只能有一份实现：两套路径不一致正是『单边好友关系』这类缺陷的温床"
        );
    }

    /// **`get_pending_requests` 必须按好友关系过滤**（用户明确要求的兜底规则）。
    #[test]
    fn pending_requests_exclude_existing_friends() {
        let commands = include_str!("commands.rs");
        let body = rust_fn_body(commands, "pub fn get_pending_requests(");
        assert!(
            body.contains("is_actionable_request"),
            "列表必须按好友关系过滤 —— 否则「已经是好友了申请还挂着」只能靠每条路径都记得清"
        );
        let pred = rust_fn_body(commands, "pub(crate) fn is_actionable_request(");
        assert!(
            pred.contains("!friend_ids.contains"),
            "判据必须是「不是好友才算待处理」"
        );
    }

    /// **蓝牙通道缺省就是开**（用户 2026-09-12 规则：「有蓝牙就默认开，不用手动开关」）。
    ///
    /// 之前是"手机默认开、桌面默认关"，结果：Mac 上还要手动点一次；更糟的是
    /// "偏好=关 而运行时=开"会互相回灌，触发启停抖动（Mac 日志里那种每秒一次的
    /// `外设角色已启动 → 已停止广播` 循环，会把蓝牙栈和 CPU 打满、整个应用顿卡）。
    #[test]
    fn bt_defaults_on_everywhere() {
        let db = include_str!("db.rs");
        let body = rust_fn_body(db, "pub fn get_bt_enabled(");
        assert!(
            body.contains("let default_on = true;"),
            "缺省必须是**开**（三端一致）；写成按平台分支会重新引入「偏好/运行时互相回灌」的抖动"
        );
        assert!(
            body.contains("set_bt_enabled(conn, default_on)"),
            "缺省值必须立刻持久化（与 `get_lan_enabled` 同一套语义）"
        );
        let tm = include_str!("transport/mod.rs");
        assert!(
            tm.contains("crate::db::get_bt_enabled(&dbc)"),
            "`TransportManager::new` 必须用 `db::get_bt_enabled`（缺省值才不会在各处漂移）"
        );
    }

    /// 通道状态里的蓝牙必须是**真实运行时**状态。
    ///
    /// `TransportManager` 里的 `BluetoothTransport` 是"尚未接线"的占位实现：它的
    /// `running` 恒为 `false`、`peers` 恒为 0。界面直接采信它就会永远显示"蓝牙未运行"，
    /// 用户点了开关也看不出变化（用户 2026-09-12 安卓实测「蓝牙通道打不开」里，
    /// 有一部分就是这个假状态造成的误导）。
    #[test]
    fn runtime_snapshot_reports_real_bluetooth_runtime() {
        let commands = include_str!("commands.rs");
        let body = rust_fn_body(commands, "pub async fn build_runtime_snapshot(");
        assert!(
            body.contains("runtime_state"),
            "蓝牙 running/peers 必须取自 `network::ble::runtime_state`"
        );
        assert!(
            body.contains("bt.enabled = bt_running"),
            "起不来就必须显示为关（否则界面假装已开，用户只会觉得「点了没用」）"
        );
    }

    /// **运行状态只能有"一个快照 + 一个事件"**（用户要求的 ②）。
    ///
    /// 真实缺陷：`channels[lan].enabled`（`get_channel_status`）与 `online`（`get_network_status`）
    /// 是同一件事的两份前端状态，各自被不同事件更新 ⇒ 必然"外面开了、里面还是关的"。
    /// 现在旧的"半份状态"命令必须**不存在**，且事件必须**带载荷**（`RuntimeSnapshot`）。
    #[test]
    fn runtime_state_has_a_single_source() {
        let commands = include_str!("commands.rs");
        for gone in ["pub async fn get_channel_status(", "pub fn get_network_status("] {
            assert!(
                !commands.contains(gone),
                "`{gone}` 应当已经被 `get_runtime_snapshot` + `build_runtime_snapshot` 取代 —— \
                 留着它就等于给同一件事留了第二份状态"
            );
        }
        assert!(
            commands.contains("pub async fn build_runtime_snapshot("),
            "必须有唯一的采集点"
        );
        let state = include_str!("state.rs");
        let body = rust_fn_body(state, "pub fn notify_runtime_changed(");
        assert!(
            body.contains("snapshot: RuntimeSnapshot"),
            "`runtime-changed` 必须**带快照**（无载荷的话接收方只能再全量重拉一遍）"
        );
        assert!(
            body.contains("emit_filter(EVENT_RUNTIME_CHANGED"),
            "必须用 emit_filter 排除发起窗口（发起窗口从命令返回值里已经拿到了）"
        );
        assert!(
            body.contains("event_target_label(target) != origin.as_deref()"),
            "过滤条件必须是『目标窗口 ≠ 发起窗口』"
        );
    }

    /// 取一个顶层函数的函数体（从签名起到第 0 列的 `}` 为止）。
    ///
    /// 用它做"接线守卫"：这类性质（窗口走单例 helper、URL 指向自己的入口）
    /// 编译器管不着，而退化后**功能看起来仍然正常**，只有连点/开窗慢才暴露。
    fn rust_fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src
            .find(signature)
            .unwrap_or_else(|| panic!("源码里找不到 `{signature}` —— 护栏需要同步更新"));
        let rest = &src[start..];
        let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
        &rest[..end]
    }

    /// **独立窗口必须各自一个前端文档与入口**，不许再共用主窗口的 `index.html`。
    ///
    /// 为什么（用户 2026-09-12 实测）：「第二次打开设置，窗口会先刷成主聊天窗口、
    /// 然后立马变成设置界面」+「点一下要等很久」—— 根因就是设置/日志窗口加载的是主窗口的
    /// HTML，前端再按窗口 label 把聊天三栏挂起来换成设置页。窗口 URL 与前端文档的对应关系
    /// 只有这条护栏盯着（前端测试看得到 HTML，看不到 Rust 的 URL）。
    #[cfg(desktop)]
    #[test]
    fn aux_windows_open_their_own_document() {
        let commands = include_str!("commands.rs");

        let mut urls: Vec<String> = Vec::new();
        for part in commands.split("WebviewUrl::App(").skip(1) {
            let rest = part.trim_start().trim_start_matches('"');
            let end = rest
                .find('"')
                .unwrap_or_else(|| panic!("WebviewUrl::App 里的字符串没有闭合"));
            urls.push(rest[..end].to_string());
        }
        assert!(
            urls.len() >= 2,
            "应当能找到设置与日志两个窗口的 URL，实际 {}",
            urls.len()
        );
        assert!(
            !urls.iter().any(|u| u == "index.html"),
            "独立窗口不得再共用主窗口的 index.html（那会让它先把聊天三栏挂起来）：{urls:?}"
        );

        // 每个 URL 都必须真的存在，且它引用的入口也必须存在 —— 这是"改了文件名忘改另一处"
        // 的唯一防线（前端构建不会因为 Rust 里的字符串写错而失败）。
        for (url, entry) in [
            ("settings.html", "src/entries/settings.ts"),
            ("logs.html", "src/entries/logs.ts"),
        ] {
            assert!(urls.iter().any(|u| u == url), "Rust 侧应当打开 {url}：{urls:?}");
            let html = match url {
                "settings.html" => include_str!("../../settings.html").to_string(),
                _ => include_str!("../../logs.html").to_string(),
            };
            assert!(
                html.contains(&format!("/{entry}")),
                "{url} 必须加载自己的入口 /{entry}（窗口差异由文档承担，而不是在一个入口里 if/else）"
            );
            // 入口文件真的存在（`include_str!` 让"文件被删/改名"在编译期就报错）
            match entry {
                "src/entries/settings.ts" => {
                    let _ = include_str!("../../src/entries/settings.ts");
                }
                _ => {
                    let _ = include_str!("../../src/entries/logs.ts");
                }
            }
        }
    }

    /// **独立窗口的打开必须是"单例 + 串行创建 + 关闭即隐藏"**。
    ///
    /// 为什么（用户 2026-09-12 实测）：
    ///   - 连点两下会**开出第二个窗口**：`WebviewWindowBuilder::build()` 的重复 label 检查在
    ///     `prepare_window` 里做，而窗口被登记进 manager 是在主线程创建完成之后 ——
    ///     两个并发调用会双双通过检查（后者还会覆盖 manager 的记录）。
    ///   - 关闭即销毁 ⇒ 每次打开都要重建 WebView + 重新加载前端 + 重新 `app.init()`，
    ///     这正是"点一下要等很久"。
    #[cfg(desktop)]
    #[test]
    fn aux_window_open_is_singleton_serialized_and_resident() {
        let commands = include_str!("commands.rs");

        for signature in [
            "pub fn open_settings_window(",
            "pub fn open_log_window(",
        ] {
            let body = rust_fn_body(commands, signature);
            assert!(
                body.contains("ensure_aux_window("),
                "{signature}…）必须走 `ensure_aux_window`（单例 + 串行创建）——                  自己写 `if let Some(win) = get_webview_window(..)` 会在并发时开出第二个窗口"
            );
            assert!(
                !body.contains("get_webview_window("),
                "{signature}…）不该自己查窗口存在性：那是 `ensure_aux_window` 的职责"
            );
        }

        let helper = rust_fn_body(commands, "fn ensure_aux_window<F>(");
        assert!(
            helper.contains("AUX_WINDOW_CREATE_LOCK"),
            "`ensure_aux_window` 必须用创建锁串行化（否则并发会开出第二个窗口）"
        );
        assert!(
            helper.matches("show_existing_aux_window(app, label)").count() >= 2,
            "`ensure_aux_window` 必须做双重检查（拿锁前后各查一次），实际只有一次"
        );
        assert!(
            helper.contains("AUX_WINDOWS_RESIDENT") && helper.contains("install_hide_on_close(&win)"),
            "`ensure_aux_window` 里必须接上「关闭即隐藏」（常驻）——              否则每次打开都要重新加载 WebView + 前端，用户要等（`AUX_WINDOWS_RESIDENT` 只是开关）"
        );
        let hide = rust_fn_body(commands, "fn install_hide_on_close(");
        assert!(
            hide.contains("prevent_close()") && hide.contains("hide()"),
            "`install_hide_on_close` 必须是 prevent_close + hide（关闭即隐藏）"
        );
    }
}
