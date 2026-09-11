//! 系统托盘：关闭主窗口时隐藏到托盘，只有托盘菜单「退出」才真正退出。
//!
//! 设计：所有桌面端（Windows / macOS / Linux）统一行为——
//! - 点击窗口「×」：`prevent_close()` + 隐藏窗口，进程继续在后台运行（消息、发现、通知照常）
//! - 点击托盘图标 / 菜单「显示主窗口」：恢复窗口
//! - 菜单「退出」：`app.exit(0)` 真正结束进程

use std::sync::Arc;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

/// 主窗口标签（tauri.conf.json 中定义）。
pub const MAIN_WINDOW_LABEL: &str = "main";

/// 恢复主窗口（取消最小化 → 显示 → 聚焦）。
pub fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 安装托盘图标与「关闭到托盘」行为。
///
/// 容错：托盘创建失败时**不拦截关闭**（保持系统默认退出行为），
/// 避免出现「窗口关不掉、又没有托盘可恢复」的死角。
pub fn setup<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let state = app.state::<Arc<crate::state::AppState>>();
    match build_tray(app, &state) {
        Ok(()) => {
            install_close_to_tray(app);
        }
        Err(e) => {
            eprintln!("[tray] 系统托盘初始化失败，关闭窗口将直接退出应用：{e}");
        }
    }
    Ok(())
}

/// 构建托盘图标与菜单。
fn build_tray<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &Arc<crate::state::AppState>,
) -> tauri::Result<()> {
    let show_item = MenuItemBuilder::with_id("show", "显示主窗口").build(app)?;
    let restart_item = MenuItemBuilder::with_id("restart", "重启").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&restart_item)
        .item(&quit_item)
        .build()?;

    let tooltip = if state.is_zh() {
        "相闻 · 局域网即时通讯".to_string()
    } else {
        "Gosslan · LAN Messenger".to_string()
    };
    let mut builder = TrayIconBuilder::with_id("gosslan-tray")
        .menu(&menu)
        .tooltip(tooltip)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "restart" => {
                // 重启 = 释放网络资源（TCP listener / UDP socket / shutdown 任务）
                // 后以同一可执行文件重新启动自身。
                // 必须用 network::stop（不改持久化偏好），不能用 stop_network 命令
                // ——后者会写 lan_enabled=false，重启后不再自动联网。
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<std::sync::Arc<crate::state::AppState>>();
                    crate::network::stop(&state).await;
                    app.restart();
                });
            }
            "quit" => {
                // 退出前先停网络：等 TCP listener 与后台任务真正退出后再结束进程。
                // 直接 exit 会让 OS 替我们关闭 socket（Windows 上可能以 FIN 优雅关闭，
                // 在 59992 留下 120s TIME_WAIT），下一次启动就 bind 不上。
                // 与 restart 分支一致：用 network::stop（不改持久化偏好）。
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<std::sync::Arc<crate::state::AppState>>();
                    crate::network::stop(&state).await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击：Windows 上直接恢复窗口（macOS 主要走菜单项）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

/// 点击窗口「×」：阻止关闭并隐藏窗口，进程继续驻留托盘。
fn install_close_to_tray<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let win2 = win.clone();
        win.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = win2.hide();
            }
        });
    }
}
