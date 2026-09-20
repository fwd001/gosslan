//! Gosslan 应用入口（库目标，供 Tauri 加载）。

// ⚠️ 注意：不要在这里加 crate-level 的 clippy lint（`#![warn(...)]`）。
// CI 的 clippy 用 `-D warnings`，会把所有 warn 升级成 error，
// 而 transport.rs / state.rs / lib.rs::run() 等**未重构的旧代码**里
// 已经存在 25+ 个 `too_many_lines` / `cognitive_complexity` 命中，
// 一启用就秒挂。
//
// 膨胀守卫放在**已经物理拆分的模块**里（commands.rs / db.rs / 子模块），
// 那些文件已控制在 700 行以内，不会误伤。

/// Android 的「打开文件」JNI 桥（FileProvider：私有目录文件必须以 content:// 交出去）。
#[cfg(target_os = "android")]
mod android_open;
mod commands;
/// 内容传输**逻辑层**（统一生命周期 + 状态机 + 重试策略）；见 content/mod.rs 的分层说明。
pub mod content;
/// 公开给 `examples/e2e_peer.rs` 协议级 E2E 测试对端复用（线格式与密码学原语）。
pub mod crypto;
mod db;
mod device;
pub mod discovery;
/// 聊天记录导出（纯文字单文件）：磁盘满 / 换机时的自救手段。
pub mod export;
mod file_relay;
mod gossip_engine;
/// JNI 方法登记宏（Android 的两条桥共用，见该文件注释）。
#[cfg(target_os = "android")]
mod jni_method;
mod logging;
/// macOS App Sandbox 的安全作用域书签（共享目录重启后不失访，见该文件注释）。
#[cfg(target_os = "macos")]
mod macos_bookmark;
/// macOS 窗口外观：运行时加 squircle 圆角 + 关掉与圆角不兼容的系统阴影。
/// 见 macos_window.rs 注释（与自绘标题栏的取舍）。
#[cfg(target_os = "macos")]
mod macos_window;
/// macOS 原生菜单栏（自绘标题栏 + `decorations: false` 导致系统菜单栏缺失，需补回）。
/// 只在 macOS 建：Windows / Linux 用自绘标题栏，加系统菜单条会顶在标题栏之上破坏布局。
#[cfg(target_os = "macos")]
mod menu;
pub mod mesh;
mod network;
mod nickname;
mod notifications;
/// 打开本地文件的平台实现：macOS 用 NSWorkspace（沙盒下 /usr/bin/open 被拦）、
/// Android 用 FileProvider + ACTION_VIEW（opener 只发 file:// 会被系统拒绝）、
/// Windows/Linux 走 opener。
mod open_path;
pub mod protocol;
mod state;
mod storage;
mod transport;
#[cfg(desktop)]
mod tray;
mod user_dirs;

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
/// 群任务窗口的 label **前缀**（实际 label = `todo-<groupId>`，见 `open_group_todos_window`）。
///
/// 它是**动态** label（每群一个窗口），所以不进 [`WINDOW_LABELS`]（那份清单只列固定 label）；
/// capability 里用 `todo-*` 通配覆盖。前缀字符串必须与前端 `src/utils/auxWindowLabels.ts`
/// 的 `GROUP_TODOS_LABEL_PREFIX` 完全一致（有 Rust 守卫做交叉核对）。
pub const WINDOW_GROUP_TODOS_PREFIX: &str = "todo-";
/// 独立的「外部链接」窗口（`open_link_window`）。**刻意不进 [`WINDOW_LABELS`]、也不进
/// capabilities**：它加载的是**远端页面**，不给它任何 capability 才能保证远端内容
/// 调不动本应用的任何命令（`link_window_is_not_capability_covered` 测试锁死这一点）。
pub const WINDOW_LINK: &str = "link";
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
                // 设置/日志窗口是**关闭即销毁**（每次开窗由 `aux_window_geometry` 重新摆位），
                // 不让插件按 label 恢复旧几何/最大化 —— 否则会与重新居中打架
                // （用户 2026-09-17：这两个窗口改为"关闭即销毁"，见 commands.rs 的
                // `AUX_WINDOWS_RESIDENT`）。群任务窗口 label 是动态的 `todo-*`，
                // 已由 `apply_aux_geometry` 里的 `unmaximize()` 兜底。
                .with_denylist(&[WINDOW_SETTINGS, WINDOW_LOGS])
                .build(),
        );
    }

    let app = builder
        .setup(|app| {
            state_mark("boot", "AppState::init 之前");
            let state = match state::AppState::init(app.handle().clone()) {
                Ok(state) => state,
                Err(e) => {
                    // 数据来自**更新**的版本 ⇒ 拒绝打开（"能升不能毁"红线），但"拒绝"
                    // 必须是用户看得懂、知道该干什么的一句话，而不是一个悄悄消失的窗口：
                    // setup 返回 Err 会被 Tauri 变成 panic，而 Windows 发布版是
                    // `windows_subsystem = "windows"`（无控制台）⇒ stderr 无处可去 = 用户
                    // 看到窗口闪一下就没了，什么提示都没有。
                    //
                    // 按**类型**分支而不是错误字符串：协议层已经因为"按前缀分类"吞过一整条
                    // 消息（INV-P24 第 2 条，v4.22.34 才修掉）。
                    if let Some(db::InitError::Downgrade { current }) =
                        e.downcast_ref::<db::InitError>()
                    {
                        let msg = db::InitError::downgrade_message(*current);
                        state_mark("boot", "数据库版本比本机程序新，拒绝启动");
                        eprintln!("[gosslan]{msg}");
                        // 弹窗**只在桌面端**做。移动端不是"顺手也弹一下"：那个时点原生对话框
                        // 到底能不能显示，我在这里没有任何实测手段，而"弹出来了但没有 state"
                        // 与"压根没弹"在 Android 上的分别是**崩溃 vs 永久白屏** —— 后者更糟。
                        // 宁可让移动端保持现状（panic + logcat 一行），也不把未验证的行为写进
                        // 移动端启动路径。
                        #[cfg(desktop)]
                        {
                            use tauri_plugin_dialog::DialogExt as _;
                            let handle = app.handle().clone();
                            // ⚠️ **只能非阻塞 `show` + 提前 `return Ok(())`**。
                            // 插件的桌面实现是 `run_on_main_thread(...)`
                            // （tauri-plugin-dialog-2.7.3/src/desktop.rs:222），而 setup 正跑在
                            // 主线程上 ⇒ 阻塞版 API 的 `rx.recv()` 会钉住主线程，排在队列
                            // 里的弹窗任务永远执行不到 = 开机自锁（与 v4.22.30 修掉的 Windows
                            // 开窗卡死同一个形状）。让主线程回到事件循环，弹窗才会出现。
                            handle
                                .dialog()
                                .message(msg)
                                .title("Gosslan")
                                .show(move |_| {
                                    // 用户点掉提示之后才退出（exit 走事件循环代理，跨线程安全）
                                    handle.exit(1);
                                });
                            // 提前返回 ⇒ 不 `manage(state)`、不建托盘：前端所有命令都会拿到
                            // "state not found" 的 IPC 错误（不 panic），界面停在空白 ——
                            // 这比"半死的界面"诚实，而且原生弹窗盖在上面。
                            return Ok(());
                        }
                    }
                    // 非降级的启动错误、以及移动端（上面那段 cfg(desktop) 不参与编译）
                    // 都走原路：Err → 框架 panic → 退出。文案已经进 stderr，
                    // 安卓上 `state_mark` 还会把它送进 logcat。
                    return Err(e);
                }
            };
            state
                .logger
                .info("boot", "AppState::init 完成（设置/目录/局域网就绪）");
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
                    state
                        .logger
                        .warn("menu", format!("菜单栏初始化失败（不影响启动）：{e}"));
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
                    state
                        .logger
                        .warn("window", format!("恢复 macOS Cmd+W 关闭能力失败: {e}"));
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
            // ⚠️ 不在 cfg(desktop) 里：移动端同样需要读 lan_enabled 设置并自动启动 ——
            // 用户在设置里打开后、杀掉 App 再进来，LAN 应该恢复上次的状态。
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
                            st_log
                                .logger
                                .error("lan", format!("自动开启局域网通道失败: {e}"));
                        }
                    }
                });
            }
            // 定期清扫过期的 .part 断点前缀（审计 §7 风险 2）：启动后立即一次，之后每小时一次。
            {
                let st = state.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let removed = crate::network::file::sweep_stale_parts(&st);
                        if removed > 0 {
                            st.logger
                                .info("file", format!("清理过期 .part：{removed} 个"));
                        }
                        // 同一趟里清扫内存态的中继表（见 sweep_stale_relay 的说明）。
                        let swept = crate::network::transport::sweep_stale_relay(&st);
                        if swept > 0 {
                            st.logger
                                .info("relay", format!("清理过期中继态：{swept} 项"));
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
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
            commands::request_attention,
            commands::set_unread_badge,
            commands::list_favorites,
            commands::add_favorite,
            commands::remove_favorite,
            commands::read_favorite_preview,
            commands::delete_messages,
            commands::notify_desktop,
            commands::send_test_notification,
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
            commands::get_safety_number,
            commands::remove_friend,
            commands::get_pending_requests,
            commands::send_friend_request,
            commands::respond_friend_request,
            commands::send_message,
            commands::cancel_send,
            commands::resend_message,
            commands::recall_message,
            commands::get_messages,
            commands::get_conv_link,
            commands::get_message_count,
            commands::get_conversations,
            commands::ensure_conversation,
            commands::set_conversation_pinned,
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
            commands::send_group_reaction,
            commands::recall_group_message,
            commands::pin_group_message,
            commands::send_group_announcement,
            commands::send_group_todo,
            commands::update_group_todo,
            commands::send_group_poll,
            commands::cast_group_poll_vote,
            commands::send_group_file,
            commands::send_todo_image,
            commands::todo_image_meta,
            commands::get_group_file_delivery_summary,
            commands::list_group_files,
            commands::send_file,
            commands::cancel_file_transfer,
            commands::request_content,
            commands::request_content_by_cid,
            commands::get_content_transfers,
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
            commands::read_content_preview,
            commands::media_present,
            commands::export_chat_text,
            commands::search_messages,
            commands::clear_all_data,
            commands::get_discovery_diag,
            commands::get_interface_candidates,
            commands::set_app_active,
            commands::list_routed_endpoints,
            commands::add_routed_endpoint,
            commands::remove_routed_endpoint,
            commands::get_logs,
            commands::clear_logs,
            commands::open_log_window,
            commands::close_log_window,
            commands::open_settings_window,
            commands::close_settings_window,
            commands::open_group_todos_window,
            commands::open_link_window,
            commands::list_external_links,
            commands::add_external_link,
            commands::update_external_link,
            commands::remove_external_link,
            commands::save_todo_image_bytes,
            commands::delete_group_announcement,
            commands::list_active_group_announcements,
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

    /// 把 commands.rs 头部 + 所有子模块文件拼接成一份完整源码。
    ///
    /// `include!` 只做编译期拼接，`include_str!` 看不到展开后的结果。
    /// 这条辅助让源码守卫测试拿到"等于原始单文件"的视图。
    fn all_commands_src() -> &'static str {
        concat!(
            include_str!("commands.rs"),
            "\n",
            include_str!("commands/system.rs"),
            "\n",
            include_str!("commands/network.rs"),
            "\n",
            include_str!("commands/dev_diag.rs"),
            "\n",
            include_str!("commands/channel.rs"),
            "\n",
            include_str!("commands/settings.rs"),
            "\n",
            include_str!("commands/friends.rs"),
            "\n",
            include_str!("commands/mobile_picker.rs"),
            "\n",
            include_str!("commands/chat.rs"),
            "\n",
            include_str!("commands/groups.rs"),
            "\n",
            include_str!("commands/window.rs"),
            "\n",
            include_str!("commands/group_files.rs"),
            "\n",
            include_str!("commands/files.rs"),
            "\n",
            include_str!("commands/share.rs"),
            "\n",
            include_str!("commands/helpers.rs"),
            "\n",
            include_str!("commands/favorites.rs"),
            "\n",
            include_str!("commands/routed.rs"),
            "\n",
            include_str!("commands/external_links.rs"),
            "\n",
            include_str!("commands/logs.rs"),
            "\n",
            include_str!("commands/chat_search.rs"),
            "\n",
            include_str!("commands/group_announcements.rs"),
            "\n",
            include_str!("commands/group_todo_media.rs"),
            "\n",
            include_str!("commands/group_file_dispatch.rs"),
            "\n",
            include_str!("commands/group_file_keys.rs"),
        )
    }

    /// 把 db.rs 头部 + 所有子模块文件拼接成一份完整源码。
    fn all_db_src() -> &'static str {
        concat!(
            include_str!("db.rs"),
            "\n",
            include_str!("db/settings.rs"),
            "\n",
            include_str!("db/clocks.rs"),
            "\n",
            include_str!("db/group_delete_boundary.rs"),
            "\n",
            include_str!("db/friends.rs"),
            "\n",
            include_str!("db/groups.rs"),
            "\n",
            include_str!("db/messages.rs"),
            "\n",
            include_str!("db/conversations.rs"),
            "\n",
            include_str!("db/message_delete.rs"),
            "\n",
            include_str!("db/offline_queue.rs"),
            "\n",
            include_str!("db/group_offline_queue.rs"),
            "\n",
            include_str!("db/file_transfer.rs"),
            "\n",
            include_str!("db/file_offline.rs"),
            "\n",
            include_str!("db/group_files.rs"),
            "\n",
            include_str!("db/read_receipts.rs"),
            "\n",
            include_str!("db/favorites.rs"),
            "\n",
            include_str!("db/recalls.rs"),
        )
    }

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
            .map(|v| v.as_str().expect("windows 数组项必须是字符串").to_string())
            .collect();
        // 防"清单被删空 ⇒ 循环空转"：固定窗口至少这三个。
        assert!(
            WINDOW_LABELS.len() >= 3,
            "WINDOW_LABELS 至少要有 main/settings/logs 三个"
        );
        // 群任务窗口是**动态** label（`todo-<groupId>`），这里用一个代表性 label 校验通配覆盖。
        let dynamic_todo =
            format!("{WINDOW_GROUP_TODOS_PREFIX}g-00000000-0000-0000-0000-000000000000");
        for label in WINDOW_LABELS
            .iter()
            .copied()
            .chain(std::iter::once(dynamic_todo.as_str()))
        {
            assert!(
                patterns.iter().any(|p| window_pattern_matches(p, label)),
                "窗口 `{label}` 没有被任何 capability 覆盖（windows = {patterns:?}）"
            );
        }
    }

    /// **反向**守卫：外链窗口**必须不被**任何 capability 覆盖。
    ///
    /// 它加载的是远端页面；一旦被 capability 覆盖，第三方内容就能调用本应用的
    /// `plugin:*` / `core:*`（对话框、打开器、事件……）—— 等于把命令面交出去。
    /// 与上一条互为镜像：上一条保证"该覆盖的都被覆盖"，这条保证"不该覆盖的没被覆盖"。
    #[test]
    fn link_window_is_not_capability_covered() {
        let raw = include_str!("../capabilities/default.json");
        let caps: serde_json::Value =
            serde_json::from_str(raw).expect("capabilities/default.json 必须是合法 JSON");
        let patterns: Vec<String> = caps
            .get("windows")
            .and_then(|v| v.as_array())
            .expect("capabilities/default.json 缺少 windows 数组")
            .iter()
            .map(|v| v.as_str().expect("windows 数组项必须是字符串").to_string())
            .collect();
        assert!(
            !patterns
                .iter()
                .any(|p| window_pattern_matches(p, WINDOW_LINK)),
            "外链窗口 `{WINDOW_LINK}` 不得被任何 capability 覆盖（远端页面会因此拿到命令面）：{patterns:?}"
        );
        // 同时确认它没被误加进固定窗口清单。
        assert!(
            !WINDOW_LABELS.contains(&WINDOW_LINK),
            "WINDOW_LINK 不该进 WINDOW_LABELS（那份清单是 capability 覆盖的正向清单）"
        );
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
        let src = all_commands_src();
        // ⚠️ 例外清单已清空（2026-09-20）：v4.22.2 曾把 4 个开窗命令登记成例外（理由是
        // "macOS 必须主线程调 AppKit"），结果在 Windows 上换来了更严重的故障 ——
        // 同步命令在 IPC 回调里内联跑主线程，而 Windows 建 WebView2 会在调用线程里泵消息
        // ⇒ 与 IPC 重入 ⇒ 持锁自锁 ⇒ 整个界面永久无响应。macOS 的约束改由
        // `commands::logs::decorate_aux_window` 把 AppKit 调用单独投回主线程解决，
        // 不再需要任何例外。

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

        // 建窗命令单独判：`open_*_window` 的函数体里没有上面那些重资源标记（db 访问都在
        // 已 try_lock 的辅助函数里），光靠 markers 抓不到它 —— 而它恰恰是最不能同步的一条。
        let window_markers = ["ensure_aux_window(", "WebviewWindowBuilder::new("];

        let mut offenders: Vec<String> = Vec::new();
        for (name, is_async, body) in command_bodies(src) {
            if is_async {
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
            if window_markers.iter().any(|m| body.contains(m)) {
                offenders.push(format!(
                    "  {name}: 创建窗口 —— Windows 上主线程内联建 WebView2 会泵消息、\
                     与 IPC 回调重入 ⇒ 永久挂死（见 commands/logs.rs 的 ensure_aux_window）"
                ));
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
            let is_async = attr_async || src[attr_end..fn_pos].contains("async");
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
        let kotlin =
            include_str!("../gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt");
        let rust = include_str!("transport/ble_android.rs");
        // 第二条 JNI 桥：「用系统里的其它应用打开文件」（FileProvider）。同一个坑，
        // 所以必须同一条护栏盯着 —— 新桥单独立一份检查只会漂移。
        let kotlin_open =
            include_str!("../gen/android/app/src/main/java/com/gosslan/app/OpenWith.kt");
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
        // 打开/保存文件的桥：Rust 调 openWith / saveWith / writeBytesWith / convertHeicToJpeg
        // / isHevcVideo / isMotionPhoto，Kotlin 调 nativeAttachOpenWith
        for (file, fns, expected) in [
            ("OpenWith.kt", &open_fns, "openWith"),
            ("OpenWith.kt", &open_fns, "saveWith"),
            ("OpenWith.kt", &open_fns, "writeBytesWith"),
            ("OpenWith.kt", &open_fns, "convertHeicToJpeg"),
            ("OpenWith.kt", &open_fns, "isHevcVideo"),
            ("OpenWith.kt", &open_fns, "isMotionPhoto"),
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
            6,
            "android_open.rs 应恰好登记 6 个 Kotlin 方法（openWith + saveWith + writeBytesWith + convertHeicToJpeg + isHevcVideo + isMotionPhoto），实际 {} —— \
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
            static_checked >= 9,
            "应检查 ≥9 个 Kotlin 方法（OpenWith 3 个 + BlePeripheral 6 个），实际 {static_checked} —— 护栏失效了"
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
        let ble_kt =
            include_str!("../gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt");
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

    /// **好友申请必须"发出即登记、建链补发"**（用户真机："已发送"但对方什么都没收到）。
    ///
    /// 好友申请是**没有回执**的定向帧：链路正好在那一刻抖动（BLE 镜像互拨打断链路）时
    /// 它会静默丢失，而发送方界面依然显示「已发送，等待对方确认」。
    /// 现在：命令**先登记再发**；任何传输建链/Hello 补全时补发一次；
    /// 收到同意或拒绝（`forget_pending_request`）后清除 —— 所以不会无限重发。
    #[test]
    fn friend_request_survives_a_dropped_link() {
        let commands = all_commands_src();
        let i = commands
            .find("pub async fn send_friend_request(")
            .expect("send_friend_request 必须在");
        let body = &commands[i..i + 1200];
        assert!(
            body.contains("pending_out_requests"),
            "命令必须**先登记**再发（丢了才知道要补发）"
        );
        assert!(
            body.contains("send_friend_request_via_link"),
            "发送逻辑要复用同一个实现（补发走的是同一条路径）"
        );
        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("pub async fn flush_pending_friend_request("),
            "必须有补发入口"
        );
        assert!(
            transport.contains("flush_pending_friend_request(state, &device_id).await;"),
            "建链/Hello 补全时必须调用补发（所有传输的建链都会走到那里）"
        );
        let forget = rust_fn_body(&transport, "pub fn forget_pending_request(");
        assert!(
            forget.contains("pending_out_requests"),
            "收到同意/拒绝后必须清掉登记，否则会一直补发"
        );
    }

    /// **BLE 链路上必须只有一个"指定拨号方"**（大 id 拨、小 id 只接受）。
    ///
    /// 真实缺陷（用户 2026-09-12 真机："点加好友：发送失败，连接已关闭"）：
    /// 两端都同时跑 central + peripheral ⇒ **互相拨号**，形成两条镜像 BLE 链路；
    /// 小 id 那一侧拨过去的连接会被对端按镜像规则拒掉（不回 Hello），但**每次连接本身
    /// 都会打断对端拨过来的那条好链路** ⇒ 好链路 45s 收不到帧被看门狗拆掉 ⇒ 再重来。
    /// 修法：小 id 在握手验签后**记住"不要再拨这个外设"**并主动放弃该镜像链路。
    ///
    /// ⚠️ 2026-09-13（Windows Phase 1）本条判据多了 `peer_advertises` 前缀：
    /// "大 id 拨"只在**两端都能广播**时成立。Windows 只做 central，若照搬会让
    /// "Windows id 更小"变成两侧都不拨的死局（真机现象：搜到了但永远连不上）。
    /// 所以既要**保留**镜像护栏（对端在广播时仍按 id 比较），又要
    /// **保留**"对端不广播 ⇒ 必须我们拨"这个活口 —— 两条都不能被删掉。
    #[test]
    fn ble_link_has_a_designated_dialer() {
        let ble = include_str!("network/ble.rs");
        let body = rust_fn_body(ble, "fn should_dial_ble(");
        assert!(
            body.contains("my_id > peer_id"),
            "判据必须保留 TCP 的 `should_dial` 同款：两端都能广播时**大 id 拨、小 id 只接受**"
        );
        assert!(
            body.contains("peer_advertises"),
            "对端不广播时必须由我们拨（`peer_advertises` 前缀）—— \
             少了它，只做 central 的平台（Windows）在 id 更小时会两侧都不拨，链路永远建不起来"
        );
        assert!(
            ble.contains("if !should_dial_ble(&state.device_id, &peer_id, peer_advertises)"),
            "central 侧握手后必须用它决定要不要放弃镜像链路"
        );
        assert!(
            ble.contains("ble_no_dial"),
            "必须记住『不要再拨』的名单 —— 否则每轮扫描还会去拨它、反复打断好的那条链路"
        );
        assert!(
            ble.contains("ble_peer_advertises"),
            "『对端是否在广播』必须有事实来源（扫描结果登记），不能靠平台常量猜"
        );
        assert!(
            ble.contains("contains(&peripheral.id().to_string())"),
            "扫描循环必须跳过名单里的外设"
        );
    }

    /// **BLE 握手必须容忍前导帧，且前导帧绝不进业务处理**（真机 2026-09-13）。
    ///
    /// 日志证据：`[GATT] 已就绪 → [DISCONNECT] 对端首帧不是 Hello（收到 chat_message）`。
    /// Android 的 notify 按 **central 地址**投递 ⇒ 上一条链路的待发帧会落在新连接上，
    /// "首帧必须是 Hello"这条旧判据于是把**本来能建起来的链路**全部打死。
    ///
    /// 这条护栏盯两件事：① central 侧走"读到 Hello 为止"的辅助函数（不是裸 read_one）；
    /// ② 被丢掉的前导帧**不能**进 `handle_message`（身份未验签，那是一条安全边界）。
    #[test]
    fn ble_handshake_skips_leading_frames_without_processing_them() {
        let ble = include_str!("network/ble.rs");
        let ble_f = code_flat(ble);
        assert!(
            ble_f.contains("read_hello_frame(&mutreader,HANDSHAKE_TIMEOUT"),
            "central 侧必须用 read_hello_frame（容忍前导帧），不能退回裸 read_one + 首帧断言"
        );
        let helper = rust_fn_body(ble, "async fn read_hello_frame(");
        assert!(
            !helper.contains("handle_message"),
            "前导帧绝不能进 handle_message —— 身份来自 Hello 验签，验签之前它只是字节"
        );
        assert!(
            helper.contains("preamble_action("),
            "额度判定必须走纯函数 preamble_action（可单测）"
        );
        // 外设侧同理；但只有**有外设角色**的平台才存在那段代码
        //（Windows Phase 1 只做 central —— ADR-0015 §7-f，外设分支不编译）。
        #[cfg(any(target_os = "macos", target_os = "android"))]
        assert!(
            ble.contains("丢弃外设侧握手前导帧"),
            "外设侧同样要丢前导帧（同一条 Android notify 语义）"
        );
    }

    /// **BLE 拨号必须去重 + 失败必须断开**（真机 2026-09-13：Mac 一个字节都收不到）。
    ///
    /// 真机证据：Mac 侧 `[GATT] 已就绪` 之后 `握手超时：对端未回 Hello` 反复出现，
    /// 而安卓侧 `[SEND] type=gossip kind=FriendRequest` 全部"成功"。根因是**同一对端上叠了
    /// 多条连接**：扫描每 10s 一轮、握手最长 10s ⇒ 每轮都新起一个拨号任务；
    /// 失败后又从不 `disconnect()`（drop 一个 btleplug `Peripheral` 不会断开 CoreBluetooth），
    /// 于是留下"已经没人读"的幽灵连接 + 多个通知流订阅；Android 的 GATT server 对同一地址
    /// 只保留最后一条连接，通知于是被投给没人读的那条。
    ///
    /// 这条护栏盯四件事（少一件就会退回原状）：
    /// ① 拨号前用 `DialGuard` 去重；② 复用旧连接前先断开；③ 握手失败必须显式断开；
    /// ④ 分片级统计必须存在（否则"没发出去"与"没收到"永远分不清）。
    #[test]
    fn ble_dial_is_deduplicated_and_disconnects_on_failure() {
        let ble = include_str!("network/ble.rs");
        let dial = rust_fn_body(ble, "async fn dial_and_register(");
        assert!(
            dial.contains("DialGuard::try_acquire") && dial.contains("ble:{ble_id}"),
            "拨号前必须按外设 id 做在途去重（否则每轮扫描叠一条连接）"
        );
        assert!(
            dial.contains("is_connected()"),
            "复用旧连接前必须先断开（否则 connect() 会复用幽灵连接、新订阅收不到任何通知）"
        );
        assert!(
            dial.contains("peripheral.disconnect().await"),
            "失败路径必须显式断开（drop 不会断开 CoreBluetooth 连接）"
        );
        assert!(
            ble.contains("[FRAG] 收到通知"),
            "读循环必须打**分片级**日志：否则『对端没发』与『发了我没收到』无法区分"
        );
        let bt = include_str!("transport/bluetooth.rs");
        assert!(
            bt.contains("fn stats(") && bt.contains("seen_notifications"),
            "BleReader 必须统计收到的通知条数/字节数（分片级可见性的来源）"
        );
    }

    /// **Android 外设的通知必须节流**（真机 2026-09-13：742B 的帧只到了 38/53 片）。
    ///
    /// 算术证据：Mac 侧 `[FRAG] 收到通知 38 条 / 747 字节`；而 742 字节的帧在 MTU=23
    /// （每片 14 字节载荷 + 6 字节分片头）下需要 **53 片** ⇒ 丢了 15 片 ⇒ 永远拼不出完整帧
    /// ⇒ 表现就是"安卓收到了好友申请并且加上了，Mac 什么都收不到"。
    /// `notifyCharacteristicChanged` 连发会被 Android 协议栈丢包，`send()` 返回 true 只代表
    /// 调用被接受；必须每片之间留一个连接间隔。
    #[test]
    fn android_peripheral_paces_its_notifications() {
        let android = include_str!("transport/ble_android.rs");
        assert!(
            android.contains("const NOTIFY_CHUNK_INTERVAL: Duration"),
            "必须显式定义通知间隔常量（可调、可测），而不是散落的 magic number"
        );
        let send = rust_fn_body(
            android,
            "    pub async fn send_frame(&self, central: &str, payload: &[u8])",
        );
        assert!(
            send.contains("sleep(NOTIFY_CHUNK_INTERVAL).await"),
            "逐片发送的循环里必须真的 sleep 这个间隔 —— 否则连发丢片会重现"
        );
        assert!(
            send.contains("idx + 1 < total"),
            "最后一片之后不该再等（否则每帧白等一个间隔）"
        );
        // 发送侧的分片数必须留痕：与对端的 [FRAG] 数字对照才能判"发少了 / 收丢了"
        let ble = include_str!("network/ble.rs");
        assert!(
            ble.contains("分片={n}"),
            "写循环必须打出发出的分片数（与对端 [FRAG] 对照）"
        );
    }

    /// **BLE 文件分块必须小，且不能让一帧打死链路**（真机 2026-09-13：大图"两边都成功"）。
    ///
    /// 日志证据：`[SEND] 写失败 ⇒ 结束该链路写循环 … type=file_chunk`；而接收侧反复
    /// `接收文件初始化失败: 重复的文件传输` + `file_reject`。根因：一对一文件流每块默认
    /// 256 KiB，在 MTU=23 的 BLE 上需要 18725 个分片 > `MAX_BLE_CHUNKS_PER_MESSAGE`(8192)
    /// ⇒ `fragment()` 返回 None ⇒ 写循环把**整条链路**拆掉；同时重复的 offer 被 reject
    /// ⇒ 对端停止重试 ⇒ 文件永远到不了，而发送方界面显示"已发送/已读"。
    ///
    /// 这条护栏盯三件事：① 分块按链路选路结果决定；② "帧无法分片"只丢这一帧、不拆链路；
    /// ③ 重复的 offer 必须幂等回 accept。
    #[test]
    fn ble_file_transfer_respects_link_limits() {
        let file = include_str!("network/file.rs");
        let stream = rust_fn_body(file, "async fn stream_file(");
        assert!(
            stream.contains("chunk_size_for_path("),
            "文件分块大小必须按链路能力决定（BLE 上 256 KiB 分不出片）"
        );
        assert!(
            stream.contains("resolve_stream_link("),
            "分块大小必须取自**实际选路结果**，不能按平台写死"
        );
        // 保序不变量：一条分片流只能待在同一条连接上（真机：多文件并发时 600MB
        // 大文件跑到 100% 报"文件分片顺序错误"，单发同一文件必成功）。
        // 一律用 code_flat：裸子串会被 rustfmt 拆行而静默空转。
        let stream_f = code_flat(&stream);
        assert!(
            stream_f.contains("send_on_link_with_tick(&link,&chunk,FILE_STALL_TICK,")
                && stream_f
                    .contains("stall_tick(state,transfer_id,stream_started_ms,&mutstalled_shown)"),
            "分片必须投到钉住的那条链路，且等待期间必须做停滞检查 —— 对端不收时发送就挂在\
             背压上，这里是唯一的观测点（摘掉检查 = 界面冻在同一个百分比直到 deadline）"
        );
        assert!(
            stream_f.contains("send_on_link(&link,&done)"),
            "FileDone 必须排在**自己那串分片之后**走同一条链路：走 try_send 时队列满会换到\
             空闲连接，完成帧超过在途分片先到 ⇒ 接收端判「文件传输未完成」"
        );
        assert!(
            !stream_f.contains("try_send(state,peer_id,&chunk)"),
            "文件分片不得逐条选路（跨连接乱序）"
        );

        let ble = include_str!("network/ble.rs");
        assert!(
            ble.contains("丢弃无法分片的帧（链路保留）"),
            "「帧无法分片」只该丢这一帧：拆链路会让同连接上其它传输一起失败"
        );

        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("file::has_receiver(state, &transfer_id)"),
            "重复的 FileOffer 必须幂等回 accept（旧行为 reject ⇒ 对端停止重试、文件永远收不到）"
        );

        // 群路径同理：Offer / Chunk / Done 三类帧必须全在同一条链路上。
        // 只钉分片不钉 Offer 会造出更隐蔽的分裂：Offer 是 Normal、分片是 Low，Normal 满掉
        // 时 failover 到另一条连接 ⇒ 分片先到、密钥后到，而接收端没有该 transfer 的密钥时
        // 是**静默 return**（不报错、不回执），这批分片就永久丢了。
        let g = code_flat(include_str!("commands/group_file_dispatch.rs"));
        assert!(
            g.contains("send_on_link(&link,&offer)")
                && g.contains("send_on_link_with_tick(&link,&chunk,file::FILE_STALL_TICK,"),
            "群文件的 Offer 与分片必须走同一条钉住的链路（分片还要带停滞检查）"
        );
        assert!(
            !g.contains("try_send(state,recipient,"),
            "群文件三类帧都不得再走逐条选路的 try_send（跨连接乱序）"
        );
    }

    /// **气泡前不得做 O(体积) 的整文件扫描**（真机 Mac 发 600MB：点完"卡一会儿"才出现
    /// 发送中气泡 —— 建发送记录时先整读一遍文件算 sha256，而投递任务待会儿还要再读一遍）。
    ///
    /// 这条不变量很容易"顺手写回去"：它看起来只是"提前把 cid 准备好"，代价却挂在用户
    /// 点下发送按钮之后。cid 的正确来源是投递任务（它本来就要算，用于 FileOffer 校验值），
    /// 算完回填发送行。
    #[test]
    fn no_whole_file_scan_before_the_file_bubble() {
        let files = include_str!("commands/files.rs");
        let build = rust_fn_body(files, "fn build_file_message(");
        assert!(
            !build.contains("sha256_file_hex("),
            "建发送记录前不得整读文件算哈希：那会把气泡挡在一次 O(体积) 扫描之后"
        );
        assert!(
            build.contains("\"sha256\": \"\""),
            "cid 必须以空串占位（载荷形状不能变，前端按 sha256 键取值）"
        );

        let file = include_str!("network/file.rs");
        let body = rust_fn_body(file, "pub async fn send_file_from_path_at(");
        assert!(
            body.contains("spawn_blocking"),
            "整文件哈希必须在阻塞线程池：占住 async worker 会连带拖慢其它传输"
        );
        assert!(
            body.contains("fill_message_sha256"),
            "投递任务算出的 cid 必须回填发送行，否则本地合并卡片按 cid 找不回字节"
        );
        assert!(
            body.contains("backfilled") && body.contains("emit(\"message-received\", &rec)"),
            "回填改了库就必须通知前端：只写库时内存里那条记录仍是 cid 空的版本，\
             同一会话把刚发的文件转成合并卡片会丢掉对端的拉取钥匙（v4.22.16 的回归）"
        );
    }

    /// **接收端判死必须把否定确认送回，而且发送端要能在分片循环里立刻看到**。
    ///
    /// 旧行为是"判死但谁也不告诉"：发送端把剩下的整份文件继续灌进一条已经死掉的传输，
    /// FileDone 无人应答 → 干等一个 `FILE_ACK_IDLE` → 判"可重试" → 再整发 5 次。
    /// 真机形状：600MB 跑到 100% 两边都显示失败，中间几十分钟界面一直"发送中"。
    /// 这条链有**三个环节**（回帧、早注册、循环里盯），断掉任意一个都看不出差别，
    /// 所以三个点一起钉。
    #[test]
    fn receiver_abort_notifies_the_sender_inside_the_loop() {
        let tr = code_flat(&crate::network::transport_src_for_guards());
        assert!(
            tr.contains("iffile::fail_receive(state,&transfer_id,peer_id,&e){")
                && tr.contains(
                    "Message::FileCompleteAck{transfer_id:transfer_id.clone(),success:false,}"
                ),
            "分片判死必须回 FileCompleteAck{{success:false}}（fail_receive 返回 false 表示没有活的\
             接收器，不重复回）"
        );
        assert!(
            !tr.contains("let _=file::fail_receive(state,&transfer_id,peer_id,&e);"),
            "不得退回「abort 了但不告诉发送端」的旧写法"
        );
        let f = code_flat(include_str!("network/file.rs"));
        assert!(
            f.contains("_=&mutack_rx=>"),
            "分片循环必须盯着否定确认，才能当场停手而不是把剩余字节灌完"
        );
        assert_eq!(
            f.matches(".insert(transfer_id.to_string(),tx)").count(),
            2,
            "pending_file_accept 与 pending_file_complete 各登记一次；出现第三次说明有人\
             又把 complete 注册挪回了循环之后（那样循环里的 ack_rx 就成了死代码）"
        );
    }

    /// **群文件投递的取消登记必须按 (transfer_id, recipient) 分键**（真机：三成员以上
    /// 的群文件只有一个人收得到）。
    ///
    /// 群发是「每个可达成员各 `tokio::spawn` 一个任务、共用同一个 `transfer_id`」。
    /// 单键时后注册的 `HashMap::insert` 会挤掉前一个任务的 `Sender`，对方的 oneshot 立刻
    /// 以 `Err(RecvError)` 完成，而投递循环的取消分支分不清「用户真点了取消」和
    /// 「登记被顶替」⇒ N-1 个成员以「用户取消发送」这个假原因当场中断。
    /// 只能钉源码：这是语言级细节，行为测试要先构造出"顶替"才看得到。
    #[test]
    fn group_file_cancel_registry_is_scoped_per_recipient() {
        let dispatch = code_flat(include_str!("commands/group_file_dispatch.rs"));
        let fanout = code_flat(include_str!("commands/group_announcements.rs"));
        assert!(
            dispatch.contains("file_cancel_key(transfer_id,recipient)"),
            "群投递的取消登记必须带 recipient（否则同 transfer_id 的任务互相挤掉登记）"
        );
        assert!(
            !dispatch.contains(".insert(transfer_id.to_string(),cancel_tx)"),
            "不得回退成 transfer_id 单键"
        );
        assert!(
            fanout.contains("update_group_file_recipient(&dbc,&tid3,&m3,status,0.0)"),
            "群投递出错必须给成员落终态：离线补发只捞 status='pending'，留在 sending \
             就是「永远在发、重启也不重试」"
        );
    }

    /// **INV-P24 第 1 条：未知帧类型必须降级，不得成为连接错误**（ADR-0007 落地①）。
    ///
    /// 为什么只能钉源码：这条的行为是"什么都没发生"—— 没有测试会因为**缺少**它而失败，
    /// 而它的缺失后果极重：新版本上线任何一种新帧，老设备不是少收一条消息，
    /// 而是**跟新设备连不上**（反序列化失败 → io::Error → reader 退出 → 重连再失败）。
    /// 同时**握手首帧不许降级**：未认证的连接没有"看不懂就放过"的理由。
    #[test]
    fn unknown_wire_frame_is_tolerated_after_auth() {
        let out = code_flat(include_str!("network/transport/outbound.rs"));
        assert!(
            out.contains("Err(e)ife.to_string().starts_with(\"unknownvariant\")"),
            "未知变体必须由 serde 自己的措辞识别（不维护第二份类型清单，清单会漂移）"
        );
        assert!(
            out.contains("Ok(Message::Unknown{wire_type:tag})"),
            "未知 type 必须降级成 Message::Unknown"
        );
        assert!(
            out.contains("fnread_frame") && out.contains("decode_frame(&buf)"),
            "数据面 read_frame 必须走 decode_frame，否则降级形同不存在"
        );
        let preauth = rust_fn_body(
            include_str!("network/transport/outbound.rs"),
            "async fn read_frame_preauth",
        );
        assert!(
            !preauth.contains("decode_frame"),
            "握手首帧不得降级：非 Hello / 看不懂的帧在认证前就该拒掉"
        );
        let agg = code_flat(&crate::network::transport_src_for_guards());
        assert!(
            agg.contains("Message::Unknown{wire_type}=>{"),
            "handle_message 必须有 Unknown 分支（忽略 + 节流日志），否则会退化成 panic 或误判"
        );
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
            body.contains("ScanFilter::default()"),
            "不得在**平台层**用服务 UUID 过滤：macOS 把 128 位 UUID 放进扫描响应，\
             Android 的硬件过滤只匹配主广播包 ⇒ 会永远收不到 Mac 的广播（真机实测）"
        );
        assert!(
            body.contains("properties()"),
            "必须按**广播内容**（`properties().services`）判定，而不是按连接后才发现的服务"
        );
        assert!(
            body.contains("discover_services()") || body.contains("连接"),
            "必须在注释里写清为什么不能过滤 —— 否则下一个人很容易『顺手补一个校验』"
        );
        // 扫到候选必须留痕（否则这类缺陷在日志里完全不可见）
        let ble = include_str!("network/ble.rs");
        assert!(
            ble.contains("BLE 扫描：收到"),
            "每次扫描都要打日志（收到的广播总数 + 其中本服务的个数）：真机上这是区分\
             『扫描收不到广播』与『收到了但都不是本服务』的唯一线索"
        );
    }

    /// **外设侧收到的 Hello 必须换路由重新握手**（重连时的经典坑，真机踩过）。
    ///
    /// 2026-09-12 真机：Mac（central）反复报 `对端首帧不是 Hello` / `握手超时：对端未回 Hello`。
    /// BLE 上同一个 central 的地址在**重连**时复用，旧连接的链路任务可能还没清理：
    /// 新连接的 Hello 一旦被投给**旧链路的管道**，旧链路的写句柄写的是旧连接
    /// ⇒ 新连接永远收不到 Hello 回应。用户侧表现就是"蓝牙时好时坏、加好友没反应"。
    ///
    /// 这条护栏盯三件事：① 判据函数存在且语义正确（真值表由 `ble::tests` 钉住）；
    /// ② 接收循环**真的调用**它（判据再好，调用点写错也白搭）；③ 诊断信息必须带上
    /// 收到的类型名 —— 否则下次真机日志里还是只有一句"不是 Hello"，无从下手。
    #[test]
    fn peripheral_reconnect_hello_replaces_the_stale_route() {
        let ble = include_str!("network/ble.rs");
        let action = rust_fn_body(ble, "fn peripheral_route_action(");
        assert!(
            action.contains("!has_route || frame_is_hello"),
            "判据必须是『没有活路由 或 这帧是 Hello ⇒ 走握手』，其余才投已有链路"
        );
        assert!(
            ble.contains("peripheral_route_action(has_route, has_route && frame_is_hello(&bytes))"),
            "接收循环必须**真的调用**这个判据（只在有路由时才解析首帧，避免给大分片白烧一次解析）"
        );
        assert!(
            ble.contains("外设侧收到新连接的 Hello"),
            "换路由必须留痕：真机上这是区分『重连接管』与『链路抖动』的唯一日志"
        );
        // 诊断信息必须带类型名（两处：central 侧拨号 + 外设侧接收入站）
        assert!(
            ble.contains("对端首帧不是 Hello（连续 {dropped} 帧都不是，最后一帧 type="),
            "central 侧失败时必须带**最后一帧的类型**（`wire_kind()`）"
        );
        assert!(
            ble.contains("外设侧首帧不是 Hello（收到 {") && ble.contains("first.wire_kind()"),
            "外设侧失败时同样必须带**收到的类型**（`wire_kind()`）"
        );
        assert!(
            ble.contains("wire_kind()"),
            "类型名要走 `Message::wire_kind()`（与 serde tag 同一份事实来源）"
        );
    }

    /// **掉线的节点要留在发现列表里，但不能算"在线"**（两个坑必须同时躲开）。
    ///
    /// 2026-09-12 真机（用户 4.2.11，只开蓝牙）：**手机能看到 Mac（"已发现未建联"），
    /// Mac 里安卓什么都不显示**。根因是链路一断就把节点条目删了 —— BLE 上
    /// "连上 → 被对端按指定拨号方退让 → 断开"是常态，Mac 刚学到身份就被删，
    /// 「添加好友」列表里只闪一下，用户根本点不到。
    ///
    /// 反过来，如果只是"保留条目"而沿用旧的「在 peers 表里就算在线」判据，
    /// 就会把复核抓到过的那个 High 缺陷重新引入（一次"连过又掉线"的节点永久在线）。
    /// 所以两件事必须一起成立：**条目保留**（`mark_peer_offline` 不删 peer）+
    /// **在线看 last_seen 新鲜度**（`friend_is_online`）。
    #[test]
    fn offline_peer_stays_listed_but_is_not_online() {
        let transport = crate::network::transport_src_for_guards();
        let body = rust_fn_body(&transport, "pub(crate) async fn mark_peer_offline(");
        assert!(
            !body.contains("remove(device_id)"),
            "链路断了**不能**立刻删节点条目：BLE 上「连上→退让→断开」是常态，\
             删掉的话「添加好友」列表里对端只闪一下（真机症状）"
        );
        assert!(
            body.contains("clear_conv_link"),
            "链路快照必须立刻清掉 —— 否则聊天头部会一直显示「桥接 N」（用户实测过）"
        );

        let commands = all_commands_src();
        assert!(
            commands.contains("fn friend_is_online("),
            "在线判据必须是独立纯函数（可单测、可护栏）"
        );
        assert!(
            commands
                .contains("friend_is_online(last_seen, now, active_links.contains(&f.device_id))"),
            "get_friends 必须走 friend_is_online：**只看「在不在节点表里」会让刚掉线的节点\
             保持在线的假象**（就是那个 High 缺陷）"
        );
        assert!(
            !commands.contains("f.online = peers.contains_key(&f.device_id)"),
            "旧的 presence 判据不得复活（条目现在会被保留，presence 不再等价于在线）"
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
            .chain(parse_kotlin_method_registrations(include_str!(
                "android_open.rs"
            )))
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
            let Some(pos) = line.find("fun ") else {
                continue;
            };
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
        let transport = crate::network::transport_src_for_guards();
        let transport_f = code_flat(&transport);
        assert_eq!(
            transport_f.matches("forget_pending_request(state,&from)").count(),
            2,
            "两条 FriendAccept 路径（直连 `Message::FriendAccept` + 跨跳 `GossipKind::FriendAccept`）\
             都必须清掉 pending —— 少一条就会让「已经是好友了，申请还挂着」复现"
        );
        let commands = all_commands_src();
        let commands_f = code_flat(commands);
        assert_eq!(
            commands_f
                .matches("forget_pending_request(s,peer_id)")
                .count(),
            1,
            "`respond_friend_request` 的同意路径也要走同一个助手（别各写一遍）"
        );
        let helper = rust_fn_body(&transport, "pub fn forget_pending_request(");
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
        let transport = crate::network::transport_src_for_guards();
        let helper = rust_fn_body(&transport, "async fn auto_accept_if_already_friend(");
        assert!(
            helper.contains("db::get_friend") && helper.contains("accept_friend_request"),
            "自动同意必须：① 真的判『他是不是已经是我的好友』；② 走**同一个** accept 实现（别各写一遍）"
        );
        let calls = transport
            .matches("auto_accept_if_already_friend(state, ")
            .count();
        assert_eq!(
            calls, 2,
            "两条 FriendRequest 路径（直连 `Message::FriendRequest` + 跨跳 `GossipKind::FriendRequest`）\
             都必须先做自动同意 —— 只修一条就会『同一件事两种行为』（这正是上一条缺陷的成因）"
        );
        let commands = all_commands_src();
        assert_eq!(
            commands
                .matches("pub(crate) async fn accept_friend_request(")
                .count(),
            1,
            "『同意好友』只能有一份实现：两套路径不一致正是『单边好友关系』这类缺陷的温床"
        );
    }

    /// **`get_pending_requests` 必须按好友关系过滤**（用户明确要求的兜底规则）。
    #[test]
    fn pending_requests_exclude_existing_friends() {
        let commands = all_commands_src();
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
        let db = all_db_src();
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

    /// **通道偏好必须与运行状态分开表达**（用户 2026-09-14 桌面实测：关掉蓝牙、退出重进又被打开）。
    ///
    /// 根因：快照里 channels[bluetooth].enabled 用的是"运行时是否在跑"，应用刚启动、BLE 还没
    /// 拉起时必然是 false ⇒ 前端 ensureBluetoothOn 无法区分"用户明确关掉"与"还没启动"，
    /// 于是把偏好覆盖成开。判据：通道状态必须有独立的 preferred 字段，且快照从持久化键
    /// （lan_enabled / bt_enabled）填充。
    #[test]
    fn channel_status_exposes_persisted_preference() {
        let tm = include_str!("transport/mod.rs");
        assert!(
            tm.contains("pub preferred: bool"),
            "ChannelStatus 必须有独立的 preferred 字段（与 running 分开），否则前端只能拿运行状态猜偏好"
        );
        let cmds = all_commands_src();
        let body = rust_fn_body(cmds, "pub async fn build_runtime_snapshot(");
        assert!(
            body.contains("get_lan_enabled") && body.contains("get_bt_enabled"),
            "快照必须从持久化键填充 preferred（开机后偏好不能丢）"
        );
        assert!(
            body.contains("c.preferred"),
            "必须把 db 里的偏好写回通道状态"
        );
    }

    /// **系统通知必须真的发得出去、且能被观察**（用户 2026-09-14：Windows 同事收不到任何通知）。
    ///
    /// 三个必须同时成立的判据：
    /// 1. Rust 侧通知统一走 crate::notifications（能返回错误），不再用插件那个把错误 spawn
    ///    掉丢掉的 show()；
    /// 2. **不经前端**的好友申请/好友通过通知必须尊重 notify_enabled（否则关了通知还会被弹）；
    /// 3. 设置页要有能如实报告失败的“发送测试通知”入口，否则 Windows 上（未安装 / 勿扰）
    ///    永远只能靠猜。
    #[test]
    fn notifications_are_observable_and_respect_the_switch() {
        let transport = crate::network::transport_src_for_guards();
        assert!(
            !transport.contains("tauri_plugin_notification::NotificationExt"),
            "network 层不得再直接用插件的 show()（它把错误 spawn 掉丢了）—— 统一走 crate::notifications"
        );
        assert_eq!(
            transport.matches("crate::notifications::show").count(),
            4,
            "四处 Rust 侧通知（好友申请×2 + 好友通过×2）都必须走 notifications（含开关与错误）"
        );
        let notif = include_str!("notifications.rs");
        assert!(
            notif.contains("pub fn show_if_enabled") && notif.contains("notify_enabled"),
            "notifications 必须提供“尊重总开关”的入口"
        );
        assert!(
            notif.contains("map_err(|e| e.to_string())"),
            "notify-rust 的错误必须返回出来，不能吞"
        );
        let commands = all_commands_src();
        assert!(
            commands.contains("pub fn send_test_notification("),
            "必须有设置页可调用的测试通知命令"
        );
    }

    /// **链路徽标与在线状态必须"实时"**（用户 2026-09-14：两边全在局域网，却显示「已桥接」，
    /// 且好友在线状态不实时）。
    ///
    /// 两个必须同时成立的判据：
    /// 1. get_conv_link 有直连时按**此刻实际选路**返回 hop=0，不再回放"上一条消息"的快照；
    /// 2. peers-updated 要带上每个节点的活跃链路（link 字段），前端才能把"有链路但广播没收到"
    ///    的节点也算在线（与后端 friend_is_online 同口径）。
    #[test]
    fn link_badge_and_presence_are_live() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "pub async fn get_conv_link(");
        assert!(
            body.contains("has_link") && body.contains("hop: 0"),
            "有直连时 get_conv_link 必须以实时链路 + hop=0 返回，不能回放消息快照"
        );
        let state = include_str!("state.rs");
        assert!(
            state.contains("p.link = best_link_kind(&kinds)"),
            "peers-updated 必须带上活跃链路（link 字段），否则前端判不出「有链路但广播缺席」的在线"
        );
    }

    /// **在途文件不能被当成"已被清理"**（用户 2026-09-14：群里收图时好时坏，点几次/
    /// 等一会儿/重发才出来）。
    ///
    /// 接收方在 FileDone 之前写的是 <transfer_id>.part，final 路径尚不存在。若
    /// resolve_media_path 直接报 Gone，前端会把"正在接收"的图片标成「已被清理」并缓存。
    #[test]
    fn in_flight_media_is_not_reported_as_deleted() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "fn resolve_media_path(");
        assert!(
            body.contains("file_receivers")
                && body.contains("group_file_receivers")
                && body.contains("仍在接收"),
            "缺失的 final 文件必须先在途判定（file_receivers / group_file_receivers），在途报 Unknown 而不是 Gone"
        );
    }

    /// **未完成的内容要能自动重试，且同样受能力协商约束**（ADR-0019 Phase 1）。
    #[test]
    fn incomplete_content_is_auto_retried_behind_capability_gate() {
        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("async fn retry_incomplete_content("),
            "必须实现建链自动重试"
        );
        assert!(
            transport.contains("CONTENT_FEATURE_PULL"),
            "自动重试也必须走能力协商（旧端不发新帧）"
        );
        let cmds = all_commands_src();
        assert!(
            cmds.contains("pub fn get_content_transfers("),
            "必须有统一状态查询命令（前端气泡据此显示）"
        );
        let model = include_str!("content/model.rs");
        assert!(
            model.contains("Serialize, Deserialize, Clone, Debug, PartialEq"),
            "TransferRecord 必须可序列化给前端"
        );
        let file = include_str!("network/file.rs");
        assert!(
            file.contains("record_failure"),
            "中途失败/断链必须在 fail_receive 里记 Incomplete，否则记录永远停在 Active、自动重试不触发"
        );
        assert!(
            file.contains("pub fn resume_receive("),
            "必须有断点续传接收（从 .part 前缀继续）"
        );
        assert!(
            transport.contains("send_file_from_path_at"),
            "服务端必须支持从偏移续发（from_bytes）"
        );
        // 审计 §7 风险 1：接收端必须把"我已有多少字节"回给发送端，发送端据此续发（不重头覆盖）。
        assert!(
            transport.contains("retained_part_len") && transport.contains("received: retained"),
            "接收端必须按真实前缀长度回 FileReject.received，发送端据此续发"
        );
        // 审计 §7 风险 2：必须有过期 .part 的定期清扫。
        assert!(
            file.contains("pub fn sweep_stale_parts("),
            "必须有 .part 定期清扫（可恢复失败会保留前缀，不能让它们无限堆积）"
        );
    }

    /// 不得「先绑定 `links` 守卫、再在循环里 await 发送」。
    ///
    /// `links` 是 `tokio::sync::Mutex`，跨 await 持锁**编译器不拦**，而发送目标都是有界队列
    /// （1024）：对端僵死（半开 TCP / 休眠 / 写缓冲满）时 `send().await` 会一直挂起却握着
    /// 全局 links 锁 ⇒ try_send、心跳、get_peers、mark_peer_offline、teardown_link 以及
    /// 看门狗全部阻塞。看门狗恰恰是唯一能发 cancel 拆掉那条卡死连接、让队列排空的机制，
    /// 它被同一把锁挡住就是自锁死循环，只能靠用户手动重开局域网。
    /// 正确写法：锁内只 `clone` 发送端快照，发送放到锁外（与心跳发送同一纪律）。
    #[test]
    fn never_awaits_while_holding_the_links_lock() {
        let cmds = all_commands_src();
        for f in [
            "pub async fn update_profile(",
            "pub async fn broadcast_chat_style(",
        ] {
            let body = rust_fn_body(cmds, f);
            assert!(
                !body.contains("for link in links"),
                "{f} 又回到「持有 links 守卫时 await 发送」的写法：\
                 队列有界，对端僵死会让 send().await 永久挂起并握着全局 links 锁，\
                 连看门狗都拿不到锁 ⇒ 网络层自锁死。必须先 collect 发送端快照、再在锁外发送。"
            );
        }
    }

    /// **内容拉取必须走能力协商**（ADR-0019 Phase 3）：旧端不发新帧、新端才拉；
    /// 且能力位**不能进 Hello 签名材料**，否则老端验签会失败（向后兼容的硬前提）。
    #[test]
    fn content_pull_requires_capability_negotiation() {
        let proto = include_str!("protocol.rs");
        assert!(proto.contains("CONTENT_FEATURE_PULL"));
        let sig = rust_fn_body(proto, "pub fn hello_signing_bytes(");
        assert!(
            !sig.contains("content_features"),
            "content_features 不得进入签名材料（否则老端验签失败）"
        );
        let cmds = all_commands_src();
        let body = rust_fn_body(cmds, "pub async fn request_content(");
        assert!(
            body.contains("CONTENT_FEATURE_PULL"),
            "拉取必须按对端能力位协商，旧端不发新帧"
        );
        // 按裸 cid 的变体（合并转发卡片的读侧）同样必须协商 —— 卡片载荷里的 cid
        // 来自对端声明，不协商就发帧会让旧端收到理解不了的帧。
        let body_by_cid = rust_fn_body(cmds, "pub async fn request_content_by_cid(");
        assert!(
            body_by_cid.contains("CONTENT_FEATURE_PULL"),
            "request_content_by_cid 也必须按对端能力位协商"
        );
        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("find_source"),
            "服务端必须按 cid 找本地内容"
        );
        assert!(
            transport.contains("db::get_group"),
            "群成员也应能作为拉取请求方（A→B 成功后，C 可从已收完的 B 拉）"
        );
    }

    /// **对端版本必须「声明但不签名、记录但会回收」**（ADR-0007 决策 1 / INV-P24）。
    ///
    /// 为什么只能钉源码：这三个环节各自的行为都是"什么都没发生"，
    /// 没有哪个测试会因为**漏了**其中一条而失败，而每条漏掉的后果都不轻：
    /// - 进了签名材料 ⇒ 老端验签失败 ⇒ "报版本"本身变成破坏性变更（老设备直接连不上）；
    /// - 不记录 ⇒ 未知帧日志与诊断面板都无从解释"到底谁版本高"；
    /// - 不回收 ⇒ 节点进出比删好友频繁得多，长跑后这张表无界增长。
    #[test]
    fn peer_version_is_declared_not_signed_and_reclaimed() {
        let proto = include_str!("protocol.rs");
        let sig = rust_fn_body(proto, "pub fn hello_signing_bytes(");
        for f in ["protocol_version", "app_version"] {
            assert!(
                !sig.contains(f),
                "{f} 不得进入 Hello 签名材料（否则老端验签失败，加字段=断兼容）"
            );
        }
        let transport = crate::network::transport_src_for_guards();
        let built = rust_fn_body(&transport, "pub fn build_signed_hello(");
        assert!(
            code_flat(&built).contains("protocol_version:Some(crate::protocol::PROTOCOL_VERSION)"),
            "本机 Hello 必须声明 PROTOCOL_VERSION，否则对端永远看不到我们的版本"
        );
        assert!(
            code_flat(&built).contains("app_version:Some(crate::protocol::current_app_version("),
            "本机 Hello 必须声明 app_version（只给人看，但诊断面板要靠它认人）"
        );
        let hello_arm = rust_fn_body(&transport, "pub async fn handle_message(");
        assert!(
            hello_arm.contains("peer_versions"),
            "Hello 到达时必须记录对端声明的版本（TCP 与 BLE 共用这一个写入点）"
        );
        let cmds = all_commands_src();
        assert!(
            cmds.contains("result.peer_versions = collect_peer_versions("),
            "诊断面板必须真的把对端版本取出来 —— 只存不读等于没有"
        );
        let disc = include_str!("network/discovery.rs");
        assert!(
            disc.contains("peer_versions"),
            "sweep_peers 必须回收 peer_versions（与 peer_content_features 同一回收点）"
        );
    }

    /// 启动期"数据比本机新 ⇒ 拒绝打开"这条路径的三个形状都必须钉住（AI_RULES §13）。
    ///
    /// ① **判定早于任何写操作**：`execute_batch(SCHEMA)` 是 `CREATE TABLE IF NOT EXISTS`，
    ///    看着无害，但它确实写文件；先写再判，`downgrade_message()` 里"数据没有被修改"就成了
    ///    谎话，而用户正是凭这句话决定"可以放心装新版本"。（回归过的位置：v4.22.36 之前
    ///    判定在 `run_migrations` 里 = SCHEMA 之后。）
    /// ② **调用方按类型分支**，不许 `to_string().contains("user_version")` —— 协议层已经
    ///    因为按错误字符串分类吞过一整条消息（INV-P24 第 2 条，v4.22.34）。
    /// ③ **弹窗只能非阻塞**：`blocking_show()` 的桌面实现是 `run_on_main_thread`
    ///    （tauri-plugin-dialog-2.7.3/src/desktop.rs:222），而 `setup` 正跑在主线程上 ⇒
    ///    排在队列里的弹窗永远执行不到 = 开机自锁，与 v4.22.30 修掉的 Windows 开窗卡死
    ///    同一个形状。写错成 blocking 版本不会报错，只会**永远白屏**，所以必须机器拦。
    #[test]
    fn boot_downgrade_refusal_is_typed_precedes_writes_and_non_blocking() {
        /// 只留**代码行**：这条守卫判的是"有没有真的调用"，注释里提到 API 名字是
        /// 常事（本条第一次跑就被自己的注释判红了），所以先把行注释剥掉 ——
        /// 既不让散文误伤守卫，也不让散文冒充成实现。
        fn code_only(src: &str) -> String {
            src.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        }
        let db = code_only(include_str!("db.rs"));
        // ⚠️ 只取**测试模块之前**那段代码：守卫自己就在 lib.rs 里，全文一起扫的话
        // 下面这几个字面串会先命中守卫自己的源码 —— 正判据永远为真、负判据永远为假，
        // 这条守卫会变成看着严密实际空转的那种。
        let lib = code_only(
            include_str!("lib.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default(),
        );

        let check = "if current > DB_VERSION";
        let write = "conn.execute_batch(SCHEMA)";
        assert_eq!(
            db.matches(check).count(),
            1,
            "降级判定全仓只许一处（第二处迟早与第一处口径不同）"
        );
        assert_eq!(
            db.matches(write).count(),
            1,
            "本机 schema 只许在一处写入，否则这条顺序断言无法判定位置"
        );
        assert!(
            db.find(check).unwrap() < db.find(write).unwrap(),
            "降级判定必须早于 execute_batch(SCHEMA)：先写再判 = 「数据未被修改」是谎话"
        );

        assert!(
            lib.contains("downcast_ref::<db::InitError>()"),
            "启动期必须按 db::InitError 类型分支，而不是按错误字符串猜"
        );
        assert!(
            !lib.contains("blocking_show"),
            "主线程上 blocking_show() 会自锁（弹窗任务排在被钉住的主线程队列里）"
        );
        assert!(
            lib.contains(".show(move |_|"),
            "降级提示必须非阻塞排队 + 提前 return，让主线程回到事件循环"
        );
    }

    /// 发送进度必须落在"已写出链路"上，不许回到"已入队"（v4.22.37 的用户可见症状）。
    ///
    /// 三处同时成立才有意义，所以一起钉：
    ///   ① writer 那唯一的证据点必须**累加片数**（只记时刻的话进度算不出来）；
    ///   ② 状态表的值必须是带 `chunks` 的结构，而不是裸时间戳；
    ///   ③ `stream_file` 的进度与 `file-progress` 事件必须用换算后的 `on_wire` ——
    ///      旧写法 `received: sent` 就是那个"262MB 还在队列里就 100%"的假象。
    /// 判据都取**函数体**而不是全文：`received: sent` 在中继发文件那条路径里是合法的
    /// （另一套语义），全文一扫会误伤。
    #[test]
    fn file_send_progress_counts_wire_not_queue() {
        let file = include_str!("network/file.rs");
        let body = rust_fn_body(file, "async fn stream_file(");
        assert!(
            body.contains("wire_progress_bytes("),
            "stream_file 的进度必须经 wire_progress_bytes 换算"
        );
        assert!(
            body.contains("file_wire_chunks_at("),
            "换算必须读 writer 记的已写出片数，否则等于没换"
        );
        assert!(
            !body.contains("received: sent"),
            "file-progress 不许再直接发入队量（那就是 100% 假象）"
        );

        let transport = crate::network::transport_src_for_guards();
        let mark = rust_fn_body(&transport, "pub(crate) fn mark_file_wire_progress(");
        assert!(
            code_flat(&mark).contains("p.chunks=p.chunks.saturating_add(1)"),
            "writer 的写出证据点必须累加片数"
        );
        let state = include_str!("state.rs");
        assert!(
            state.contains("Mutex<HashMap<String, FileWireProgress>>"),
            "进展表的值必须是 {{at_ms, chunks}} 结构，时间戳单独一个字段撑不起进度口径"
        );
    }

    /// 外设侧**每次订阅都必须清掉该 central 的重组器**（用户优先级 ①：加入 mesh 的稳定性）。
    ///
    /// 为什么（2026-09-13 框架审计）：对端的 `msg_id` **每条连接都从 1 重新开始**，而 macOS
    /// 外设角色**没有** didDisconnect 回调、`didUnsubscribe` 也不保证在断连时到达 ⇒ 上一轮
    /// 残留的半截消息会和重连后的第一帧撞在同一个 `msg_id` 上（分片数/序号对不上）⇒ 那条帧
    /// 被当坏片丢掉（真机体感："重连之后第一条消息丢了"）。只靠 `BleReassembler` 的 30s TTL
    /// 兜底太慢 —— 真机重连通常就在几秒内发生。
    ///
    /// 判据：`did_subscribe` 里必须**按 central id `remove`**（不能整体 `clear`：会误伤其它
    /// 正在线的对端）。
    #[test]
    fn peripheral_subscribe_resets_that_centrals_reassembler() {
        let src = include_str!("transport/bluetooth_peripheral.rs");
        let start = src
            .find("fn did_subscribe(")
            .expect("源码里找不到 `fn did_subscribe(` —— 护栏需要同步更新");
        let end = src[start..]
            .find("fn did_unsubscribe(")
            .map(|i| start + i)
            .expect("源码里找不到 `fn did_unsubscribe(` —— 护栏需要同步更新");
        let body = &src[start..end];
        assert!(
            body.contains("reassemblers"),
            "`did_subscribe` 必须清掉该 central 的分片重组器 —— 否则重连后第一条帧会和上一轮的半截消息撞车"
        );
        assert!(
            body.contains(".remove(&id)"),
            "必须**按 central id** remove（整体 clear 会误伤其它在线对端）"
        );
    }

    /// **BLE 写失败必须"退避重试 → 拆链路"，绝不能只结束写循环**（真机 2026-09-13 安卓）。
    ///
    /// 真机链路：Android 与 Mac/Windows 的 BLE 会话都 `[SESSION] 已就绪`，随后一阵群 gossip
    /// 洪水把链路写满 ⇒ `[SEND] 写失败 ⇒ 结束该链路写循环`（两条链路各一次）⇒ 从此**发不出去**，
    /// 界面报「发送失败，连接已关闭」，而**读**还在正常收 ⇒ 看门狗按读活性判健康、45s 也不拆
    /// ⇒ 只能重启应用。根因是"只结束写循环、把链路留成能收不能发的僵尸"。
    ///
    /// 判据：写循环必须有重试上限常量、最终失败要走链路表的 `cancel.send(true)`（让读循环
    /// 收尾时 `teardown_link` 清链路+清退避+wake_scan），并且失败日志要带上**原因**（旧实现
    /// 只打 `type=?`，真机上完全看不出为什么写失败）。
    #[test]
    fn ble_write_failure_retries_then_tears_the_link_down() {
        let ble = include_str!("network/ble.rs");
        let body = rust_fn_body(ble, "async fn ble_writer_loop<S: FrameSink + 'static>(");
        assert!(
            body.contains("WRITE_RETRY_ATTEMPTS"),
            "写失败必须**退避重试**（瞬态失败一次性判死会把链路变成僵尸）"
        );
        assert!(
            body.contains("l.cancel.send(true)"),
            "写循环最终失败必须去链路表里取消这一条（否则读循环还活着 ⇒ \
             能收不能发的僵尸链路，看门狗按读活性判健康、永远不拆，只能重启应用）"
        );
        assert!(
            body.contains("原因={e}"),
            "失败日志必须带上真实原因（旧实现只打 `type=?`，真机上无从判断）"
        );
        assert!(
            ble.contains("WRITE_RETRY_WAIT"),
            "重试间隔必须是常量（可读、可调）"
        );
    }

    /// **⌘W 必须由我们自己的菜单项处理**（用户 2026-09-13 真机：主窗口 ⌘W 只会"滴滴滴"）。
    ///
    /// 系统预定义项 `PredefinedMenuItem::close_window` 的动作是 `performClose:`，AppKit 按
    /// 窗口的 `Closable` 样式位校验可用性；而本项目**所有应用窗口**都是 `decorations: false`
    /// ⇒ Borderless ⇒ 该项被判不可用 ⇒ 只有系统提示音，且**没有任何日志**。
    /// （2026-09-17 之前只有主窗口是无边框的、辅助窗口靠系统标题栏所以"正常"，更容易让人以为
    /// 是"主窗口特有的 bug"；现在辅助窗口也由 `decorate_aux_window` 补回 `Closable` 位。）
    ///
    /// 判据：窗口菜单不得再用预定义关闭项；必须有一个带 `CmdOrCtrl+W` 的自定义项，
    /// 且菜单事件处理里真的关掉"当前聚焦窗口"（回落到主窗口）。
    #[test]
    fn cmd_w_is_handled_by_our_own_menu_item() {
        let m = include_str!("menu.rs");
        // ⚠️ 先**去掉注释**再查"有没有用错项"：menu.rs 里那段解释恰恰写着这个名字，
        // 不剥注释就会把"解释这个坑"误判成"又踩了这个坑"（本项目已踩过一次这种假阳性）。
        let code: String = m
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("PredefinedMenuItem::close_window"),
            "不许用系统预定义的关闭项 —— 它会被 AppKit 按 `Closable` 样式位判为不可用，\
             表现就是主窗口 ⌘W 只响一声提示音（用户 2026-09-13 真机）"
        );
        assert!(
            m.contains("\"close-window\"") && m.contains("CmdOrCtrl+W"),
            "必须有一个 id=close-window、快捷键 CmdOrCtrl+W 的自定义菜单项"
        );
        let handler = m
            .find("\"close-window\" =>")
            .expect("菜单事件处理里必须接住 close-window（否则点了没反应）");
        let body = &m[handler..(handler + 700).min(m.len())];
        assert!(
            body.contains("is_focused"),
            "关的应当是**当前聚焦**的窗口（与 macOS 原生 ⌘W 语义一致）"
        );
        assert!(
            body.contains(".close()"),
            "走 `close()` 才会触发各窗口自己的 CloseRequested → 隐藏处理器（与「×」一致）"
        );
    }

    /// **解除好友关系必须同时解除内存里的身份绑定**（用户 2026-09-13 真机：不然"必须重启"）。
    ///
    /// 真机链路：对方重装后公钥变了 → 我们的身份表只补空、不覆盖（INV-P11）→
    /// 用户按提示删好友重新加，`friends` 表那行没了，但 `verify_hello` 还会回落到
    /// **内存 `peers` 表**里广播学来的旧公钥 ⇒ Hello 一直被硬拒 ⇒ 消息与好友申请都进不来，
    /// **只有重启**（内存清空）才回落到 TOFU。
    ///
    /// 判据：两条解除关系的路径（本地删好友 / 对方发来 FriendRemove）都必须调用
    /// `forget_peer_identity`；并且给用户的提示必须写出**可行动的下一步**
    /// （否则用户只能自己猜，或者干脆重启）。
    #[test]
    fn removing_a_friend_also_drops_the_in_memory_identity_binding() {
        let cmd = all_commands_src();
        let body = rust_fn_body(cmd, "pub async fn remove_friend(");
        assert!(
            body.contains("forget_peer_identity"),
            "`remove_friend` 必须调 `forget_peer_identity` —— 只删 friends 表那一行，\
             内存里的旧公钥会继续当信任根用（症状：删了好友重新加也没用，必须重启）"
        );
        let tr = crate::network::transport_src_for_guards();
        // 对方解除关系那条路径（Message::FriendRemove）同样要清
        let start = tr
            .find("Message::FriendRemove {")
            .expect("必须还有 FriendRemove 分支（本护栏锚点）");
        let tail = &tr[start..];
        // 取一个足够覆盖该分支的窗口（分支实现变了也不会假绿：下面断言的锚点就在分支里）
        let branch = &tail[..tail.len().min(1500)];
        assert!(
            branch.contains("forget_peer_identity"),
            "收到 FriendRemove（对方删了我）时也要解除身份绑定，与 `remove_friend` 对称"
        );
        // 提示必须可行动：说清"删掉好友重新添加"且"不用重启"
        let warn = tr
            .find("fn warn_key_conflict_once")
            .expect("必须还有密钥冲突提示（本护栏锚点）");
        let warn_body = &tr[warn..warn + 2000];
        assert!(
            warn_body.contains("重新添加") && warn_body.contains("不用重启"),
            "密钥变化的安全提示必须写清可行动的下一步（重新添加 + 不用重启）—— \
             只写「建议当面核对」等于把用户扔在原地"
        );
    }

    /// **`driver::connect` 的"便宜路径"必须先判 `is_connected()`**（2026-09-13 合并评审）。
    ///
    /// 判据：12 × 250ms = 3 秒的 `discover_services()` 重试只在**已经连着**时才有意义。
    /// 无条件先跑它，等于给 macOS/Android 的每次拨号凭空加 3 秒（它们本来第一次
    /// `connect()` 就成功），既拖慢握手又可能撞上 10s 的握手超时 —— 而这种退化
    /// **功能看起来还在**（最终连得上，只是慢/偶尔超时），只能靠结构护栏盯住。
    #[test]
    fn ble_connect_cheap_path_requires_an_existing_connection() {
        let src = include_str!("transport/bluetooth.rs");
        let body = rust_fn_body(src, "pub async fn connect(peripheral: &Peripheral)");
        assert!(
            body.contains("is_connected"),
            "`connect()` 的便宜路径必须先用 `is_connected()` 判一下 —— 否则未连接时白等 3 秒"
        );
        let gate = body.find("is_connected").expect("上面刚断言过");
        let cheap = body
            .find("LINK_READY_ATTEMPTS")
            .expect("必须还有便宜路径（LINK_READY_ATTEMPTS）");
        assert!(
            gate < cheap,
            "`is_connected()` 的判断必须在便宜路径的循环**之前**（先判再跑，否则等于没判）"
        );
    }

    /// 蓝牙开关**不许卡在"等 CoreBluetooth 回报状态"上**（用户 2026-09-13）。
    ///
    /// `ble::start` 里 `start_peripheral` 要等 `peripheral::STATE_WAIT = 3s`，而它在
    /// `set_channel_enabled` 的关键路径上 —— 一旦 `await` 它，用户点开关就要干等 3 秒
    /// （用户原话："点了一下，过了好一会儿才会开"）。外设角色本来就是**独立失败**的
    /// （起不来只影响"别人连我们"），所以必须丢到后台任务里。
    ///
    /// 同时**句柄必须先写进 `state.ble` 再 spawn 外设**：反过来会出现"刚开就关"时
    /// `stop()` 拿不到 handle ⇒ 发不出停机信号 ⇒ 那个外设任务永远活着（蓝牙关不掉）。
    #[test]
    fn ble_start_does_not_block_on_the_peripheral_state_wait() {
        let ble = include_str!("network/ble.rs");
        let body = rust_fn_body(ble, "pub async fn start(state: Arc<AppState>)");
        assert!(
            !body.contains("start_peripheral(state.clone(), shutdown_tx.subscribe()).await"),
            "不许 await 外设启动（要等最多 3s 的 CoreBluetooth 状态回调）—— 必须 tokio::spawn"
        );
        let spawn = body
            .find("tokio::spawn(start_peripheral(")
            .expect("外设启动必须在后台任务里跑（`tokio::spawn(start_peripheral(...))`）");
        let store = body
            .find("*state.ble.lock()")
            .expect("必须把 ble 句柄写进 state.ble");
        assert!(
            store < spawn,
            "句柄必须先写进 state.ble、再 spawn 外设，否则「刚开就关」时 stop() 发不出停机信号"
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
        let commands = all_commands_src();
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
        let commands = all_commands_src();
        for gone in [
            "pub async fn get_channel_status(",
            "pub fn get_network_status(",
        ] {
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
    ///
    /// ⚠️ **必须先归一化行尾**（2026-09-13）：本仓库在 Windows 上会被 git
    /// （`core.autocrlf`）检出成 **CRLF**，此时函数结尾的字节是 `\n}\r\n`，
    /// **不含**锚点 `"\n}\n"`。旧实现直接在原文上 `find` ⇒ 永远找不到锚点 ⇒
    /// 静默退化成 `&rest[..]`（**整个文件剩余部分**），于是：
    ///   · `assert!(body.contains(..))` 全部"通过"（假绿）；
    ///   · `assert!(!body.contains(..))` 全部**误报失败**（真缺陷在别处也会报到这里）。
    /// 这正是本项目最该防的那类问题：护栏还在跑，却既盯不住真缺陷、又误报无关代码。
    ///
    /// 返回 `Cow`：LF 检出（macOS/Linux）零拷贝借用原文，CRLF 检出才复制一份。
    fn rust_fn_body<'a>(src: &'a str, signature: &str) -> std::borrow::Cow<'a, str> {
        let normalized = if src.contains('\r') {
            std::borrow::Cow::Owned(src.replace("\r\n", "\n"))
        } else {
            std::borrow::Cow::Borrowed(src)
        };
        let start = normalized
            .find(signature)
            .unwrap_or_else(|| panic!("源码里找不到 `{signature}` —— 护栏需要同步更新"));
        let rest = &normalized[start..];
        // 找不到收尾锚点时返回剩余全部：**这是刻意的**（宁可多看一点，也不要 panic
        // 让护栏本身变成构建阻塞），但上面那段注释说明了它为什么会掩盖问题。
        let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
        match normalized {
            // 借用原文时可以直接切原文（偏移一致）
            std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Borrowed(&src[start..start + end]),
            std::borrow::Cow::Owned(s) => {
                std::borrow::Cow::Owned(s[start..start + end].to_string())
            }
        }
    }

    /// 把源码字符串里的**所有空白**（空格 / 换行 / 制表 / CR）都去掉。
    ///
    /// 护栏用 `include_str!` 读源码然后 `.contains()` 搜特定调用格式；
    /// `cargo fmt` 会把单行调用拆成多行（`fn(a, b, c)` → 每行一个参数），
    /// 带空格的 `.contains("fn(a, b")` 就会误报。
    /// 先 flatten 再搜，让 fmt 怎么拆都不怕。
    fn code_flat(src: &str) -> String {
        src.chars().filter(|c| !c.is_whitespace()).collect()
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
        let commands = all_commands_src();

        let mut urls: Vec<String> = Vec::new();
        for part in commands.split("WebviewUrl::App(").skip(1) {
            let rest = part.trim_start().trim_start_matches('"');
            let end = rest
                .find('"')
                .unwrap_or_else(|| panic!("WebviewUrl::App 里的字符串没有闭合"));
            urls.push(rest[..end].to_string());
        }
        assert!(
            urls.len() >= 3,
            "应当能找到设置 / 日志 / 群任务三个窗口的 URL，实际 {}",
            urls.len()
        );
        assert!(
            !urls.iter().any(|u| u == "index.html"),
            "独立窗口不得再共用主窗口的 index.html（那会让它先把聊天三栏挂起来）：{urls:?}"
        );
        // 外链窗口是唯一**不走 App 文档**的窗口（它加载远端 URL），必须真的是 External。
        assert!(
            commands.contains("WebviewUrl::External("),
            "外链窗口必须用 `WebviewUrl::External` 加载远端 URL"
        );

        // 每个 URL 都必须真的存在，且它引用的入口也必须存在 —— 这是"改了文件名忘改另一处"
        // 的唯一防线（前端构建不会因为 Rust 里的字符串写错而失败）。
        for (url, entry) in [
            ("settings.html", "src/entries/settings.ts"),
            ("logs.html", "src/entries/logs.ts"),
            ("todos.html", "src/entries/todos.ts"),
        ] {
            assert!(
                urls.iter().any(|u| u == url),
                "Rust 侧应当打开 {url}：{urls:?}"
            );
            let html = match url {
                "settings.html" => include_str!("../../settings.html").to_string(),
                "logs.html" => include_str!("../../logs.html").to_string(),
                _ => include_str!("../../todos.html").to_string(),
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
                "src/entries/logs.ts" => {
                    let _ = include_str!("../../src/entries/logs.ts");
                }
                _ => {
                    let _ = include_str!("../../src/entries/todos.ts");
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
        let commands = all_commands_src();

        for signature in [
            "pub fn open_settings_window(",
            "pub fn open_log_window(",
            "pub fn open_group_todos_window(",
            "pub fn open_link_window(",
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
        // 防"锚点过期 ⇒ 拿到空/残段 ⇒ 下面的断言空转"。
        assert!(
            !helper.is_empty(),
            "找不到 `ensure_aux_window` 的函数体（这条护栏会变成空转）"
        );
        assert!(
            helper.contains("AUX_WINDOW_CREATE_LOCK"),
            "`ensure_aux_window` 必须用创建锁串行化（否则并发会开出第二个窗口）"
        );
        assert!(
            // 签名在 2026-09-16 多了一个 `geo`（创建时摆尺寸/位置，已存在时重新摆回主窗口那块屏），
            // 判据本身没变：**拿锁前后各查一次**，少一次就会在并发下开出第二个窗口。
            helper
                .matches("show_existing_aux_window(app, label, geo)")
                .count()
                >= 2,
            "`ensure_aux_window` 必须做双重检查（拿锁前后各查一次），实际只有一次"
        );
        assert!(
            // 2026-09-17：`resident` 改成参数（设置/日志常驻；群任务窗口每群一个 ⇒ 关闭即销毁）。
            // 判据是"常驻开关接上了 hide-on-close"，不再要求 helper 里出现某个具体常量。
            helper.contains("resident: bool") && helper.contains("install_hide_on_close(&win)"),
            "`ensure_aux_window` 里必须接上「关闭即隐藏」（常驻）——              否则每次打开都要重新加载 WebView + 前端，用户要等（`resident` 是开关）"
        );
        assert!(
            commands.contains("const AUX_GROUP_TODOS_RESIDENT: bool = false"),
            "群任务窗口必须「关闭即销毁」（每群一个窗口，常驻会无界增长）"
        );
        let hide = rust_fn_body(commands, "fn install_hide_on_close(");
        assert!(
            hide.contains("prevent_close()") && hide.contains("hide()"),
            "`install_hide_on_close` 必须是 prevent_close + hide（关闭即隐藏）"
        );

        // 尺寸/位置必须走**物理像素**的 setter，不能走 builder 的逻辑坐标 `position()`：
        // `tao` 创建窗口时会把逻辑坐标**逐个显示器**按各自缩放换回物理、取第一个命中的显示器
        // （见 commands.rs 的 `fit_aux_window`），所以多屏不同缩放时子窗口会跑到另一块屏上
        // —— 用户 2026-09-16 实测报的正是这个，且它在单屏上完全看不出来。
        for signature in [
            "pub fn open_settings_window(",
            "pub fn open_log_window(",
            "pub fn open_group_todos_window(",
            "pub fn open_link_window(",
        ] {
            let body = rust_fn_body(commands, signature);
            assert!(
                body.contains("apply_aux_geometry("),
                "{signature}…）必须用 `apply_aux_geometry` 在 `build()` 之后按物理像素落地尺寸与位置"
            );
            assert!(
                !body.contains(".position("),
                "{signature}…）不得用 builder 的 `.position()`：它只有逻辑坐标，多屏不同缩放时\n\
                 会被 tao 换算到另一块显示器上（改用 apply_aux_geometry）"
            );
            assert!(
                body.contains(".visible(false)"),
                "{signature}…）必须**隐藏创建**：`ensure_aux_window` 随后才 show，\n\
                 中间这段用来摆位置/尺寸，否则窗口会先在默认位置闪一下再跳过去"
            );
        }
    }

    /// 外链 URL 的**协议白名单**（用户配置的网址会被 `WebviewUrl::External` 直接加载）。
    ///
    /// 为什么必须表驱动钉死：`javascript:` / `data:` / `file:` / `tauri:` 一旦漏过，
    /// 等于把"在应用 WebView 里执行脚本 / 读本机文件"的能力交给一段用户随手粘贴的字符串。
    #[test]
    fn external_link_rejects_non_http_schemes() {
        use crate::commands::validate_external_url;
        for ok in [
            "http://example.com",
            "https://example.com/a?b=c",
            "  https://a.b.c  ",
        ] {
            assert!(validate_external_url(ok).is_ok(), "应当放行：{ok}");
        }
        for bad in [
            "",
            "   ",
            "example.com",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "tauri://localhost",
            "https://",
        ] {
            assert!(
                validate_external_url(bad).is_err(),
                "必须拒绝非 http/https 或缺少主机名的输入：{bad}"
            );
        }
    }

    /// 群任务窗口 label 由 groupId 派生（`todo-<groupId>`），且前缀必须与前端一致。
    ///
    /// 窗口靠**自己的 label** 找回是哪个群，所以"前缀两边一致"是功能成立的前提；
    /// 而 Rust 与 TS 各写一份字面量必然漂移 —— 这条护栏做交叉核对。
    #[cfg(desktop)]
    #[test]
    fn group_todos_window_label_derives_from_group_id() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "pub fn open_group_todos_window(");
        assert!(
            !body.is_empty(),
            "找不到 open_group_todos_window（护栏会空转）"
        );
        assert!(
            body.contains("WINDOW_GROUP_TODOS_PREFIX"),
            "label 必须由 `WINDOW_GROUP_TODOS_PREFIX` 前缀拼出（别写字面量）"
        );
        assert!(
            body.contains("is_ascii_alphanumeric()") && body.contains("is_empty()"),
            "groupId 会被拼进窗口 label，必须先做字符集/非空校验"
        );
        // 与前端 `auxWindowLabels.ts` 的前缀交叉核对（两份字面量必须一致）。
        let ts = include_str!("../../src/utils/auxWindowLabels.ts");
        let expected = format!(
            "export const GROUP_TODOS_LABEL_PREFIX = \"{}\"",
            WINDOW_GROUP_TODOS_PREFIX
        );
        assert!(
            ts.contains(&expected),
            "前端 GROUP_TODOS_LABEL_PREFIX 必须与 Rust WINDOW_GROUP_TODOS_PREFIX 一致（期望 `{expected}`）"
        );
    }

    /// 「和自己聊天」必须是**纯本地**路径（用户 2026-09-16 的功能）。
    ///
    /// 为什么必须守：自聊一旦走成网络路径，会同时破坏两条不变量 ——
    /// ① 自己的消息进 outbox 后**永远等不到 Ack**（没有对端），那一行会被每次心跳/建链的
    ///    `flush_outbox` 重发，"outbox 必然排空"直接失效；
    /// ② `target = 自己` 的 gossip 信封对本机（`handle_gossip` 里 `sender == 自己` 早退）
    ///    和别人（解不开）都是噪声。
    /// 这两条在界面上**都看不出来**（消息照样显示、列表照样刷新），只有库里悄悄长出一条
    /// 永不消失的 outbox 行 —— 属于只能靠护栏拦的那类退化。
    #[test]
    fn self_chat_stays_local() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "fn insert_self_message(");
        assert!(
            !body.is_empty(),
            "找不到 insert_self_message（这条护栏会变成空转）"
        );
        // 先查"不该有的"：注入成 `insert_message_and_outbox(` 时下面那条 contains 也会失败，
        // 但真正该说的是"你接回了网络路径"——所以把这条放在前面报。
        for forbidden in [
            "insert_message_and_outbox(",
            "broadcast_gossip(",
            "try_send(",
            "crypto::seal(",
            "seal_symmetric(",
        ] {
            assert!(
                !body.contains(forbidden),
                "自聊消息里不得出现 `{forbidden}`：消息不出本机、也不该进 outbox（详见该函数文档）"
            );
        }
        assert!(
            body.contains("db::insert_message("),
            "自聊消息必须只落本地库（`db::insert_message`）"
        );
    }

    /// 单聊重发必须先重新密封再入队（2026-09-19 P0 回归护栏）。
    ///
    /// 为什么必须守：`messages` 表存的是**明文**（见 send_message 的「本地落库（明文）」），
    /// resend 若直接把 `rec.content` 当线上内容，就没有 `enc1:` 前缀 ⇒
    /// 接收端 `open_direct_content` 拒收（不落库、不 Ack）⇒ 重发实际是 no-op，
    /// 那一行 outbox 还会被 sweeper 再次判 failed —— 用户看到的是「点重发没反应」。
    /// 同理 `seq: 0` 会让接收端把重发消息排到会话最前（排序按 seq，INV-P09），两端顺序分裂。
    /// 两侧都能「正常加密」，普通单测测不出来，只能源码护栏钉死。
    #[test]
    fn resend_reseals_before_enqueue() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "async fn resend_message(");
        assert!(
            !body.is_empty(),
            "找不到 resend_message（这条护栏会变成空转）"
        );
        assert!(
            !body.contains("content: rec.content.clone()"),
            "重发不得把库内明文直接上线：没有 enc1: 前缀接收端会拒收"
        );
        assert!(
            !body.contains("seq: 0"),
            "重发必须沿用原逻辑 seq：seq=0 会让接收端排到会话最前（INV-P09）"
        );
        assert!(
            body.contains("crypto::seal") && body.contains("enc1:"),
            "重发必须用对端当前公钥重新密封（crypto::seal + enc1: 前缀），与 send_message 同口径"
        );
    }

    /// gossip / 不透明帧的转发候选必须来自**可达链路集**（2026-09-19 审计 P0#7）。
    ///
    /// 为什么必须守：`peers` 是知识集（Presence/announce 跨跳登记，异网段节点在里面
    /// 却没有链路）。历史版本 `choose_fanout` 的三个调用点都拿 `peers.keys()` 当候选，
    /// 跨网段时扇出全部拨向不可达节点、又被 `let _ =` 静默吞掉 —— 「节点互相帮转发」
    /// 在最需要它的场景无声失效。界面上**看不出任何异常**（本地收发一切正常），
    /// 只有跨网段压测才暴露，正是只能靠护栏钉死的那类退化。
    #[test]
    fn gossip_fanout_targets_reachable_links() {
        let transport = crate::network::transport_src_for_guards();
        assert!(
            !transport.contains("choose_fanout(&peers"),
            "转发候选不得再来自 peers（知识集）—— 用 reachable_neighbors（links 中有非空链路的邻居）"
        );
        // 定义 1 处 + 调用 3 处（gossip 广播分支 / 定向洪泛兜底 / OpaqueExternal）
        let uses = transport.matches("reachable_neighbors(").count();
        assert!(
            uses >= 4,
            "reachable_neighbors 应有定义+3 个转发调用点，实际 {uses} 处 —— 有新转发点没走可达集？"
        );
    }

    /// 数据面转发必须吃中继策略（2026-09-19 审计 P0#5 回归护栏）。
    ///
    /// 为什么必须守：控制面（gossip）从 ADR-0016 起就走 `decide_forward`，而数据面
    /// （定向借道 / RelayChunk / OpaqueExternal）曾长期裸奔 —— 用户把中继设成
    /// 「关闭」，文件分片照样借他的带宽一跳一跳地跑，**设置项只有一半是真的**。
    /// 这类"开关只管一条路径"的分裂在 UI 上完全看不出来，只能源码钉死。
    #[test]
    fn relay_data_plane_respects_policy() {
        let transport = crate::network::transport_src_for_guards();
        let wired = transport.matches("decide_relay_from_peer(").count();
        assert!(
            wired >= 3,
            "数据面三个转发点（定向借道 / RelayChunk / OpaqueExternal）都要过授权闸，实际 {wired} 处"
        );
        // gossip 控制面原有闸不得被拆掉
        assert!(
            transport.contains("decide_forward("),
            "gossip 转发的 relay 授权闸（decide_forward）被删了？"
        );
    }
}
