//! 系统通知（桌面直接用 notify-rust，移动端走 tauri-plugin-notification）。
//!
//! ## 为什么不直接用插件的 show()
//!
//! 插件的桌面实现把真正的一次 toast 放进 spawn，然后丢掉结果：
//!
//!   tauri::async_runtime::spawn(async move { let _ = notification.show(); });
//!   Ok(())
//!
//! 于是用户报“收不到通知”时，日志里连“有没有尝试发”都看不到，更别说失败原因。
//! Windows 上尤其要命：未安装的 exe、AUMID 未注册、专注助手（勿扰）都会让 toast
//! 静默失败；插件自己的平台说明也写着 “Only works for installed apps.”。
//!
//! 这里直接用同一个底层库 notify-rust（版本与插件一致），额外做三件事：
//! 1. 把错误**返回出来**（命令层能如实告诉用户，日志也会记）；
//! 2. 每次尝试都打一条 info（含标题），排障时能确认“到底调没调”；
//! 3. 后端也判一次 notify_enabled —— 好友申请/好友通过是**不经前端**的，
//!    只在命令层判开关会漏掉它们（用户关掉通知仍会被弹）。
//!
//! 平台细节与插件保持一致：
//! - Windows：只有**已安装**的包才设 AppUserModelID（target/debug|release 下不设，
//!   否则开发期会因没有对应开始菜单快捷方式而更糟）；
//! - macOS：先 set_application（dev 下用 Terminal 标识）。
//!
//! 移动端没有 notify-rust，继续走插件（通知上带 extra，供点击回调识别类型）。
//!
//! ## 点击路由（2026-09-16；2026-10 扩展到 macOS/Linux）
//!
//! 「点系统通知 → 唤起窗口并定位到会话」此前在桌面端**完全没接通**：`show()` 把
//! `notify-rust` 的 `NotificationHandle` 立刻丢掉，而那个 handle 正是点击响应的唯一通道。
//! 现在聊天消息与好友申请都走 [`show_click_if_enabled`]：把 handle 留在独立线程里等
//! `wait_for_response`，命中"点正文"就调 [`on_notification_clicked`]（唤起主窗口 +
//! 广播 `EVENT_NOTIFICATION_CLICKED`，前端据此切会话、强提醒还会继续定位到原消息）。
//! macOS 上通知的实际投递发生在 `wait_for_response` 内，且必须带一个 action 按钮
//! 等待机制才会真正挂住（见该平台实现的说明）；按钮文案由调用方经 `extra["action_label"]`
//! 给（前端按界面语言本地化；后端自发的好友申请用固定中文，与其标题同口径）。
//! 移动端不变（插件自带 actionPerformed）。

use std::collections::HashMap;
use std::sync::Arc;

use tauri::Emitter;

use crate::state::AppState;

/// 通知总开关（notify_enabled，缺省开）。
///
/// 与前端 app.notifyEnabled 是同一份持久化键；这里是后端也判一次的第二道闸门。
pub fn notifications_enabled(conn: &rusqlite::Connection) -> bool {
    crate::db::get_config_bool(conn, "notify_enabled", true)
}

/// 当前平台的通知排障说明（测试通知与失败日志里带上，避免“发不出去也不知道为什么”）。
pub fn platform_hint() -> &'static str {
    #[cfg(windows)]
    {
        "Windows：系统通知只对**已安装**的应用生效 —— 请用 NSIS 安装包安装、从开始菜单启动；         便携版 / cargo dev 下不会显示。若已安装仍收不到，检查「设置 → 系统 → 通知」里          Gosslan 是否被关闭、以及是否开了「专注助手 / 勿扰」。"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS：请在「系统设置 → 通知」里允许 Gosslan（开发构建归属 Terminal）。"
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "Linux：需要桌面通知守护进程（如 dunst / GNOME 通知）。"
    }
}

/// 桌面（macOS / Windows / Linux）：组装一条通知。
///
/// 平台差异只在这里：Windows 的 AppUserModelID、macOS 的 application 标识与 action 按钮。
/// 抽出来的理由：`show` 与 `show_with_click` 必须发出**完全一样**的通知，
/// 两个入口各写一份平台分支就一定会漂移（而这里的每条分支都只在某一个平台上编译，
/// 漂移了在本地根本看不出来）。
///
/// `extra` 在 Windows/Linux 上不用（点击只有一个"点正文"信号）；macOS 用其中的
/// `action_label` 作通知按钮文案。
#[cfg(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn build(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    sound: bool,
    extra: &HashMap<String, String>,
) -> notify_rust::Notification {
    let identifier = app.config().identifier.clone();
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body).auto_icon();

    #[cfg(windows)]
    {
        use std::path::MAIN_SEPARATOR as SEP;
        if let Ok(exe) = std::env::current_exe() {
            let curr_dir = exe
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            // .as_str() 不能省：str::ends_with 要 Pattern，而 String 没实现它（只实现了
            // &String）—— 漏了会在 **Windows** 上编译失败，而 macOS 这条分支被 cfg 掉、
            // 本地看不出来（2026-09-14 CI 两个 Windows job 都挂在这）。
            let in_dev = curr_dir.ends_with(format!("{SEP}target{SEP}debug").as_str())
                || curr_dir.ends_with(format!("{SEP}target{SEP}release").as_str());
            if !in_dev {
                notification.app_id(&identifier);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = notify_rust::set_application(if tauri::is_dev() {
            "com.apple.Terminal"
        } else {
            identifier.as_str()
        });
        // ⚠️ macOS（NSUserNotificationCenter 后端）上，没有 action 按钮 / wait_for_click
        // 的通知在 mac-notification-sys 里按 shouldWait=false 投递：**点正文的回调会被
        // 直接丢弃**（PENDING 条目投递完即移除）。挂一个按钮才会走"同步投递 + 等交互"
        // 路径；点正文仍然映射成 Default（见 show_click_impl 的双判定），
        // 点这个按钮则映射成 Action("open")。文案由调用方本地化后经 extra 给。
        let label = extra
            .get("action_label")
            .map(String::as_str)
            .unwrap_or("Show");
        notification.action("open", label);
    }

    // 声音只在强提醒路径显式开启（sound=true）：走**系统通知通道**的声音，
    // 系统静音/勿扰时由系统自己决定放不放，应用不另接播放器。
    if sound {
        #[cfg(target_os = "macos")]
        {
            // 必须用完整常量：mac-notification-sys 只对这个字面值映射 Sound::Default，
            // 传 "default" 会被当成一个叫 default 的声音文件，找不到 ⇒ 静默不响。
            notification.sound_name("NSUserNotificationDefaultSoundName");
        }
        #[cfg(windows)]
        {
            // winrt-notification 的 Sound::from_str 接受 "Default"（省略 <audio> 时
            // 系统本来也会放默认声，这里显式写清）。
            notification.sound_name("Default");
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            notification.hint(notify_rust::Hint::SoundName(
                "message-new-instant".to_string(),
            ));
        }
    }

    notification
}

/// 桌面（macOS / Windows / Linux）：直接用 notify-rust，错误返回给调用方。
///
/// handle 在这里**立刻丢掉**，语义随平台而不同（这是刻意的，要点击回调请走
/// [`show_click_if_enabled`]）：
/// - Windows：toast 已交给系统，但**点击响应从此收不到**；
/// - macOS：`notify-rust` 的 `.show()` 只克隆出 handle，handle 的 `Drop` 以
///   **异步、无点击捕获**的方式投递（见 macos/nsusernotifications.rs 的 Drop 实现）。
#[cfg(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
pub fn show(app: &tauri::AppHandle, title: &str, body: &str) -> Result<(), String> {
    build(app, title, body, false, &HashMap::new())
        .show()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 通知 + 点击回调（Windows 实现）—— **平台差异的唯一切口**，对外只有一个入口
/// [`show_click_if_enabled`]。
///
/// 为什么必须有这条路径：`notify-rust` 的 `NotificationHandle` 是点击响应的**唯一**通道 ——
/// 一丢掉就再也收不到任何响应（`build().show()` 那个写法就是"点通知没反应"的根因）。
///
/// 等待放在**独立线程**上：`wait_for_response` 会一直阻塞到"用户点了正文 / 通知被关掉"，
/// 放在命令的 async 上下文里会把这次 IPC 挂住。（Windows 的 toast 会自动消失并触发
/// `Dismissed`，所以线程不会长期滞留。）
#[cfg(windows)]
fn show_click_impl(
    app: tauri::AppHandle,
    title: &str,
    body: &str,
    extra: HashMap<String, String>,
    sound: bool,
    on_click: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let handle = build(&app, title, body, sound, &extra)
        .show()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        // 闭包参数必须写全类型：`wait_for_response` 收的是 `impl ResponseHandler`，
        // 编译器无法从 trait 约束反推出闭包的参数类型（E0282）。
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            // 只有"点了正文"才算点击：`Closed(..)` 是超时/被划掉，不该抢用户的焦点。
            if response.is_default_action() {
                on_click();
            }
        });
    });
    Ok(())
}

/// 通知 + 点击回调（macOS 实现）。
///
/// notify-rust 的 macOS 后端（NSUserNotificationCenter 路径）里，`.show()` 只是把通知
/// 克隆进 handle（未投递），handle 被 `Drop` 时才异步投递（无点击捕获）；真正的
/// "投递 + 等交互"发生在 `wait_for_response` 内的一次同步投递，delegate 回调在主线程
/// RunLoop 上送达（Tauri 主窗口的 NSApplication RunLoop 一直在跑）。
///
/// 两个只有读了 mac-notification-sys 源码才能确定的坑（2026-10 取证）：
/// 1. 通知必须带一个 action 按钮（见 [`build`] 里的 `.action("open", …)`）。
///    `needs_response()` 只在有按钮 / wait_for_click 时为真；无按钮时按
///    shouldWait=false 投递，PENDING 条目投递完即删，**之后点正文没有任何回调**。
/// 2. 点正文 → delegate activationType=contentsClicked → notify-rust 映射成
///    `Default`；点那个按钮 → `Action("open")`（label→id 再兜底首个 id）。
///    两个都是"用户要打开"，都必须触发 on_click。
///
/// 代价（如实记录）：该线程会存活到通知被点击/自动消失；若通知长期留在通知中心，
/// 线程也会长期挂住 —— 强提醒低频且按 remind id 去重，普通消息走 1.5s 去抖合并，
/// 量级可接受，但不宣称已最优。
#[cfg(target_os = "macos")]
fn show_click_impl(
    app: tauri::AppHandle,
    title: &str,
    body: &str,
    extra: HashMap<String, String>,
    sound: bool,
    on_click: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let handle = build(&app, title, body, sound, &extra)
        .show()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            let opened = response.is_default_action()
                || matches!(response, notify_rust::NotificationResponse::Action(k) if k == "open");
            if opened {
                on_click();
            }
        });
    });
    Ok(())
}

/// 通知 + 点击回调（Linux/BSD 实现）。
///
/// XDG 后端里 `.show()` 已经把通知经 D-Bus 发出，`wait_for_response` 只是注册信号监听，
/// 所以与 Windows 同构：先 show 拿 handle，再丢给独立线程等待。很多通知守护进程不支持
/// actions，等待会以 Closed 结束 —— 不影响通知本身。
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn show_click_impl(
    app: tauri::AppHandle,
    title: &str,
    body: &str,
    extra: HashMap<String, String>,
    sound: bool,
    on_click: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let handle = build(&app, title, body, sound, &extra)
        .show()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            if response.is_default_action() {
                on_click();
            }
        });
    });
    Ok(())
}

/// 移动端：点击由插件自己的 `actionPerformed` 送进前端，后端不需要回调；
/// 但 `extra` 必须带上 —— Android 的点击回调靠它识别"点的是哪一类通知"。
#[cfg(not(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
fn show_click_impl(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    extra: HashMap<String, String>,
    sound: bool,
    on_click: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let _ = (on_click, sound);
    let mut builder = app.notification().builder().title(title).body(body);
    for (k, v) in &extra {
        builder = builder.extra(k, v);
    }
    builder.show().map_err(|e| e.to_string())
}

/// 移动端：没有 notify-rust，继续走 tauri-plugin-notification。
#[cfg(not(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
pub fn show(app: &tauri::AppHandle, title: &str, body: &str) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

fn enabled(state: &Arc<AppState>) -> bool {
    // 走 AppState 的内存缓存（审计 2.1i）：每条通知都判一次总开关，
    // 逐次抢全局 db 锁会在消息洪峰时加剧争用；未加载时读库一次回填。
    state.notify_enabled_cached()
}

fn log_result(state: &Arc<AppState>, title: &str, r: Result<(), String>) -> Result<bool, String> {
    match r {
        Ok(()) => {
            state
                .logger
                .info("notify", format!("已发送系统通知：{title}"));
            Ok(true)
        }
        Err(e) => {
            state.logger.warn(
                "notify",
                format!("系统通知发送失败：{e}（{}）", platform_hint()),
            );
            Err(e)
        }
    }
}

/// 尊重总开关的通知（前端消息通知走这里）。
///
/// 返回 Ok(false) 表示“用户关了通知，跳过”；Ok(true) 表示已提交给系统。
pub fn show_if_enabled(state: &Arc<AppState>, title: &str, body: &str) -> Result<bool, String> {
    if !enabled(state) {
        return Ok(false);
    }
    log_result(state, title, show(&state.app, title, body))
}

/// 尊重总开关 + **可点击**的通知：聊天消息与好友申请都走这里。
///
/// `extra` 在移动端由插件带进通知（Android 的 `actionPerformed` 靠它识别类型）；
/// macOS 桌面端读其中的 `action_label` 作通知按钮文案（点击捕获的承重件，见
/// [`show_click_impl`]）；Windows/Linux 不读它。
/// `sound` 仅强提醒传 true（系统通知声音），普通消息与好友申请传 false。
///
/// 返回 Ok(false) 表示“用户关了通知，跳过”；Ok(true) 表示已提交给系统。
pub fn show_click_if_enabled(
    state: &Arc<AppState>,
    title: &str,
    body: &str,
    extra: HashMap<String, String>,
    sound: bool,
    on_click: impl FnOnce() + Send + 'static,
) -> Result<bool, String> {
    if !enabled(state) {
        return Ok(false);
    }
    log_result(
        state,
        title,
        show_click_impl(state.app.clone(), title, body, extra, sound, on_click),
    )
}

/// 「用户点了系统通知」的事件名：主窗口收到后把对应会话（或「新的朋友」）拉到前台。
pub const EVENT_NOTIFICATION_CLICKED: &str = "notification-clicked";

/// 事件载荷 —— **与移动端插件通知的 `extra` 同形**（`type` / `conv_id` / `msg_id`），
/// 于是前端两条路径（桌面事件 / 移动端 `actionPerformed`）能共用同一个路由函数。
#[derive(Clone, serde::Serialize)]
pub struct NotificationClick {
    /// `"chat"` = 聊天消息（带 `conv_id`）；`"friend_request"` = 好友申请。
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conv_id: Option<String>,
    /// 强提醒通知：被提醒的**原消息** msg_id（点击后定位到它，不只是打开会话）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
}

/// 通知被点击之后要做的事：**唤起主窗口** + 告诉前端"点的是哪一条"。
///
/// 顺序是先唤起、后发事件：窗口此时可能正藏在托盘里（点 × 只隐藏不退出），
/// 前端收到事件要立刻切会话 —— 窗口还没出来就切会在浮出时看到一次跳变。
pub fn on_notification_clicked(
    app: &tauri::AppHandle,
    kind: &'static str,
    conv_id: Option<String>,
    msg_id: Option<String>,
) {
    // 托盘是桌面概念（`mod tray` 本身是 `#[cfg(desktop)]`）；移动端的系统通知自带
    // "点开就回到前台"的行为，不需要也无法从后端唤起窗口。
    #[cfg(desktop)]
    crate::tray::show_main_window(app);
    let _ = app.emit(
        EVENT_NOTIFICATION_CLICKED,
        NotificationClick {
            kind,
            conv_id,
            msg_id,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn
    }

    #[test]
    fn notify_enabled_defaults_on_and_uses_the_same_key_as_the_frontend() {
        let conn = mem();
        assert!(
            notifications_enabled(&conn),
            "缺省必须是开（与前端 notifyEnabled 一致）"
        );
        crate::db::set_setting(&conn, "notify_enabled", "0").unwrap();
        assert!(!notifications_enabled(&conn), "显式关掉必须生效");
        crate::db::set_setting(&conn, "notify_enabled", "1").unwrap();
        assert!(notifications_enabled(&conn));
    }

    /// **Windows-only 分支的编译错误必须在本地就能拦住**（2026-09-14 真实事故：
    /// `ends_with(format!(...))` 漏了 `.as_str()` —— `String` 没实现 `Pattern`，
    /// 于是两个 Windows CI job 全挂，而 macOS 上这段被 `#[cfg(windows)]` 掉、死活看不出来）。
    ///
    /// 这里做源码级断言：每个 `ends_with(format!(` 都必须以 `.as_str())` 收尾。
    #[test]
    fn windows_only_branch_is_source_checkable_for_pattern_bounds() {
        let src = include_str!("notifications.rs");
        // 只看生产代码：测试模块自己的断言消息里也会出现这个模式，扫进去会自我误伤。
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        let mut checked = 0usize;
        for (i, line) in code.lines().enumerate() {
            let code = line.trim_start();
            // 跳过注释行：文档与说明里会原样提到该模式，不能把它们当代码
            // （本项目踩过这种“护栏被自己的说明误伤”的假阳性）。
            if code.starts_with("//") {
                continue;
            }
            if line.contains("ends_with(format!(") {
                checked += 1;
                assert!(
                    line.contains(".as_str())"),
                    "第 {} 行 ends_with(format!(...)) 少 .as_str() ⇒ Windows 编译失败（String 未实现 Pattern）：{}",
                    i + 1,
                    line.trim()
                );
            }
        }
        assert!(
            checked >= 2,
            "预期至少两处 ends_with(format!(（Windows dev 路径判定）"
        );
    }

    #[test]
    fn platform_hint_is_never_empty() {
        assert!(!platform_hint().trim().is_empty());
        #[cfg(windows)]
        assert!(
            platform_hint().contains("已安装"),
            "Windows 的排障说明必须点出“只对已安装应用生效”"
        );
    }
}
