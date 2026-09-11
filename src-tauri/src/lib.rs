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
mod state;
mod storage;
mod transport;
#[cfg(desktop)]
mod tray;
/// macOS 原生菜单栏（自绘标题栏 + `decorations: false` 导致系统菜单栏缺失，需补回）。
/// 只在 macOS 建：Windows / Linux 用自绘标题栏，加系统菜单条会顶在标题栏之上破坏布局。
#[cfg(target_os = "macos")]
mod menu;
/// 打开本地文件：macOS 用 NSWorkspace（沙盒下 /usr/bin/open 被拦），Windows/Linux 走 opener。
mod macos_open;
/// macOS 窗口外观：运行时加 squircle 圆角 + 关掉与圆角不兼容的系统阴影。
/// 见 macos_window.rs 注释（与自绘标题栏的取舍）。
#[cfg(target_os = "macos")]
mod macos_window;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            let state = state::AppState::init(app.handle().clone())?;
            state::AppState::spawn_peer_emitter(&state);
            app.manage(state.clone());
            // 系统托盘：关闭主窗口仅隐藏到托盘，退出需走托盘菜单
            #[cfg(desktop)]
            tray::setup(app.handle())?;
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
            commands::get_network_status,
            commands::start_network,
            commands::stop_network,
            commands::get_peers,
            commands::search_nearby_peers,
            commands::focus_window,
            commands::get_topology,
            commands::get_channel_status,
            commands::set_channel_enabled,
            commands::get_cache_info,
            commands::set_cache_policy,
            commands::clean_cache_now,
            commands::get_settings,
            commands::set_ui_language,
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

    /// 会阻塞主线程的"重"命令必须声明 `#[tauri::command(async)]`。
    ///
    /// 依据（读过上游源码，不是猜的）：
    /// - `tauri-macros` 的 `body_blocking` 把命令函数**内联调用**在 IPC 处理器里；
    ///   只有 `ExecutionContext::Async` 才走 `respond_async_serialized`（异步运行时线程）。
    /// - wry 的 `WKScriptMessageHandler::did_receive` 在 AppKit 消息循环里同步回调
    ///   （`wry-*/src/wkwebview/class/wry_web_view_delegate.rs`），即 **macOS 主线程**。
    ///
    /// 于是同步命令 = 在 UI 主线程上跑：读大文件、遍历目录、删文件、`VACUUM`
    /// （`clean_cache_now`）、全量导出/搜索都会把整个应用卡住（不只是那一个窗口）。
    /// 这条守门测试盯住已知的重命令，别让 `(async)` 在后续重构里被去掉。
    #[test]
    fn heavy_commands_run_off_the_main_thread() {
        let src = include_str!("commands.rs");
        // 名字 -> 为什么重（写在这里，改列表时顺手交代理由）
        for name in [
            "read_file_preview",   // 读磁盘（图片/文件预览，热路径）
            "get_messages",        // 分页读库（每次切会话）
            "get_conversations",   // 列表 + 解密最后一条
            "search_messages",     // 全表扫描 + 解密
            "get_cache_info",      // 目录遍历统计
            "clean_cache_now",     // 删文件 + VACUUM（可能数秒）
            "export_chat_text",    // 渲染 + 写盘
            "clear_all_data",      // 递归删除
            "save_data_file",      // base64 解码 + 写盘
            "save_outgoing_image", // 写图片
            "copy_file",           // 文件复制
        ] {
            let want = format!("#[tauri::command(async)]\npub fn {name}(");
            assert!(
                src.contains(&want),
                "重命令 `{name}` 必须写成 `#[tauri::command(async)]`：同步命令会在 macOS 主线程上                 执行（wry 的 IPC 回调在 AppKit 消息循环里），读盘/遍历/删除会卡住整个应用"
            );
        }
    }

    /// 主窗口标签在 `tray` 里还有一份（那份是 `#[cfg(desktop)]`），两边不许漂移。
    #[cfg(desktop)]
    #[test]
    fn main_window_label_matches_tray_constant() {
        assert_eq!(tray::MAIN_WINDOW_LABEL, WINDOW_MAIN);
    }
}
