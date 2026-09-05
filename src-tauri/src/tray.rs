//! 系统托盘：关闭主窗口时隐藏到托盘，只有托盘菜单「退出」才真正退出。
//!
//! 设计：所有桌面端（Windows / macOS / Linux）统一行为——
//! - 点击窗口「×」：`prevent_close()` + 隐藏窗口，进程继续在后台运行（消息、发现、通知照常）
//! - 点击托盘图标 / 菜单「显示主窗口」：恢复窗口
//! - 菜单「退出」：`app.exit(0)` 真正结束进程

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
    match build_tray(app) {
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
fn build_tray<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let show_item = MenuItemBuilder::with_id("show", "显示主窗口").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&quit_item)
        .build()?;

    let mut builder = TrayIconBuilder::with_id("gosslan-tray")
        .menu(&menu)
        .tooltip("Gosslan · 局域网即时通讯")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
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
