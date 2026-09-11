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
            if let Err(e) = menu::setup(app.handle()) {
                eprintln!("[gosslan] 菜单栏初始化失败（不影响启动）：{e}");
            }
            // macOS：`decorations: false` 使 tao 以 `Borderless`（不含 `Closable` 位）样式
            // 掩码创建 NSWindow，AppKit 据此把「关闭窗口」菜单项（Cmd+W / performClose:）
            // 判为不可用，导致 Cmd+W 无效。窗口创建后补回 `Closable` 位，恢复系统原生
            // Cmd+W（仍无标题栏/关闭按钮，因未加 `Titled` 位）；关闭动作照旧走
            // CloseRequested → 托盘隐藏路径，与点击自定义标题栏「×」行为一致。
            #[cfg(all(desktop, target_os = "macos"))]
            if let Some(win) = app.handle().get_webview_window(tray::MAIN_WINDOW_LABEL) {
                if let Err(e) = win.set_closable(true) {
                    eprintln!("[window] 恢复 macOS Cmd+W 关闭能力失败: {e}");
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
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                    if let Some(win) = handle.get_webview_window(tray::MAIN_WINDOW_LABEL) {
                        if matches!(win.is_visible(), Ok(false)) {
                            eprintln!("[window] 前端未在超时内显示窗口，兜底显示");
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
                        let started = if forced {
                            network::start(st, "0.0.0.0".to_string()).await
                        } else {
                            network::start_from_prefs(st).await
                        };
                        if let Err(e) = started {
                            eprintln!("[lan] 自动开启局域网通道失败: {e}");
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
