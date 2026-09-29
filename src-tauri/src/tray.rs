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

/// 托盘图标 id（`build_tray` 创建时用的那个）。换图标 / 改 tooltip 都要按 id 找回来。
pub const TRAY_ID: &str = "gosslan-tray";

/// 托盘 tooltip 文案。`unread > 0` 时带上条数 —— 红点只能表达"有"，条数只有文字能给。
fn tray_tooltip(zh: bool, unread: u32) -> String {
    let base = if zh {
        "相闻 · 局域网即时通讯"
    } else {
        "Gosslan · LAN Messenger"
    };
    if unread == 0 {
        return base.to_string();
    }
    if zh {
        format!("{base}（{unread} 条未读）")
    } else {
        format!("{base} ({unread} unread)")
    }
}

/// macOS **菜单栏（状态项）**上的未读数字（用户 2026-09-29 需求汇总五：
/// 「Mac 端顶部状态栏 / 菜单栏有未读消息时，增加数字提醒」）。
///
/// 只有 macOS 用它：Windows 的托盘没有"标题"这个概念（`set_title` 在 Windows 不支持），
/// 那边继续靠红点 + tooltip 条数；Linux 桌面环境大多也不显示标题。所以函数连同它的
/// 测试都挂在 `#[cfg(target_os = "macos")]` 上 —— 否则 Windows/Linux 那两条 CI 腿会把
/// 没人调用的它判成死代码，而 `clippy -- -D warnings` 直接失败（本文件 `dot` 模块
/// 那段注释记的就是同一个坑）。
///
/// `0` 返回 `None` = **把标题清空**，不是留着上一次的数字（Dock 角标已经踩过一次
/// "红点清了、数字还挂着"）。
#[cfg(target_os = "macos")]
fn tray_title(unread: u32) -> Option<String> {
    // 上限只是**位数**：菜单栏宽度有限，"1284" 会把别的状态项挤掉。
    // 超过就写 `99+` —— 宁可少说几位，也不许侵占别人的地盘。
    const TRAY_TITLE_CAP: u32 = 99;
    match unread {
        0 => None,
        n if n > TRAY_TITLE_CAP => Some(format!("{TRAY_TITLE_CAP}+")),
        n => Some(n.to_string()),
    }
}

/// 未读红点（**只有非 macOS 用得上**）。
///
/// 为什么把它整个收进一个模块：这份东西在 macOS 上完全不参与渲染（那边走 Dock 数字角标），
/// 若只给每个 const/fn 各挂一次 `#[cfg]`，漏掉任何一个都会变成 macOS 上的死代码 ——
/// 而 `cargo clippy -- -D warnings` 会因为死代码直接判失败。本地（Windows）与
/// `cargo test` 都看不出来，只有 CI 的 macOS 腿会红（2026-09-17 就是这样踩到的）。
/// 收成一个模块，出口就只有一处。
#[cfg(not(target_os = "macos"))]
mod dot {
    /// 未读红点颜色 = `#D43D43`（与样式表里的 `--gosslan-danger` 同一个值）。
    ///
    /// 这个值是**故意**与两处对齐的：
    /// - 本应用自己的未读徽标色 `--gosslan-danger` —— 托盘的点和应用内的红点必须是同一个红，
    ///   否则同屏对比就是两种红（用户 2026-09-17：「红点直接和微信靠拢」）。
    /// - 2026-09-26 那档 a11y（#86）把填充档压深到 `#d43d43` 以让白字达到 4.5:1，
    ///   **而这一侧当时没跟着改** ⇒ Windows 托盘的红点与应用内的徽标从此是两种红，
    ///   而这条测试只在**非 macOS** 编译（本地看不见），只有 CI 的 Windows 腿会红。
    ///   现在跟着样式表走：应用的徽标优先于「微信那个经典值」——同屏一致性才是这条的判据。
    ///
    /// ⚠️ 这里写死十六进制而不是读 CSS 变量：托盘图标是 **Rust 侧**画的，拿不到样式表。
    /// 改 `--gosslan-danger` 时这里要一起改（`unread_dot_is_the_app_badge_red` 测试会在色值
    /// 与样式表不一致时报错）。
    pub(super) const UNREAD_RED: (u8, u8, u8) = (0xD4, 0x3D, 0x43);

    /// 红点几何：半径 = 最小边的 8.5%，圆心在 (82%, 18%)。
    ///
    /// **刻意画得小**：托盘图标最终只有 16~24px，红点占太大会变成"角上贴了块红膏药"，
    /// 而不是未读提示（第一版按 22% 画，实物糊住右上角，用户 2026-09-17 直接反馈"太丑了"，
    /// 第二版 10% 仍被要求再小一点 —— 现在是 8.5%，16px 图标上约 1.4px 半径）。
    const DOT_R_RATIO: f32 = 0.085;
    const DOT_CX_RATIO: f32 = 0.82;
    const DOT_CY_RATIO: f32 = 0.18;

    /// 白边外径 = 半径的 1.35 倍（约 1/3 半径宽的一圈白）。
    ///
    /// 为什么需要白边：托盘图标底色深浅不定，红点直接压在深色图标上会糊成一团。
    /// 但不能宽 —— 宽了红点反而变小、白圈喧宾夺主。
    const DOT_RING_RATIO: f32 = 1.35;

    /// 把 `color` 按覆盖率 `cov` 混到 `px` 上（straight-alpha 混合）。
    fn blend(px: [f32; 4], color: (u8, u8, u8), cov: f32) -> [f32; 4] {
        let inv = 1.0 - cov;
        [
            color.0 as f32 * cov + px[0] * inv,
            color.1 as f32 * cov + px[1] * inv,
            color.2 as f32 * cov + px[2] * inv,
            255.0 * cov + px[3] * inv,
        ]
    }

    /// 在应用图标右上角叠一个小未读红点（逐像素处理，不引入图像库）。
    ///
    /// 几何按图标**等比**算：图标本身多大都不用管 —— 系统还会再缩放一次，
    /// 等比画上去的红点跟着一起缩，视觉比例不变。
    ///
    /// 边缘按覆盖率做 1px 抗锯齿：不做的话缩到 16px 后圆点边缘会带毛刺，浅色任务栏上尤其明显。
    pub(super) fn with_unread_dot(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
        let mut out = rgba.to_vec();
        if out.len() < (width * height * 4) as usize {
            return out; // 数据长度不对（理论上不会）→ 原样返回，宁可不画也别越界
        }
        let min_side = width.min(height) as f32;
        let cx = width as f32 * DOT_CX_RATIO;
        let cy = height as f32 * DOT_CY_RATIO;
        let r = min_side * DOT_R_RATIO;
        let ring = r * DOT_RING_RATIO;
        // 距圆心 d 的像素被半径 rad 的圆覆盖多少：0=完全在外，1=完全在内，中间就是边缘那一圈。
        let coverage = |d: f32, rad: f32| (rad + 0.5 - d).clamp(0.0, 1.0);
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let red = coverage(d, r);
                let white = (coverage(d, ring) - red).max(0.0);
                if red <= 0.0 && white <= 0.0 {
                    continue; // 绝大多数像素走这里 —— 一个都不碰
                }
                let i = ((y * width + x) * 4) as usize;
                let mut px = [
                    out[i] as f32,
                    out[i + 1] as f32,
                    out[i + 2] as f32,
                    out[i + 3] as f32,
                ];
                // 顺序不能反：先白边打底、再红点盖上去，否则白边会啃掉红点一圈。
                if white > 0.0 {
                    px = blend(px, (255, 255, 255), white);
                }
                if red > 0.0 {
                    px = blend(px, UNREAD_RED, red);
                }
                out[i] = px[0].round() as u8;
                out[i + 1] = px[1].round() as u8;
                out[i + 2] = px[2].round() as u8;
                out[i + 3] = px[3].round() as u8;
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 托盘红点必须与应用内的未读徽标**同一个红**（用户 2026-09-17：红点要和微信靠拢）。
        #[test]
        fn unread_dot_is_the_app_badge_red() {
            let css = include_str!("../../src/style.css");
            assert!(
                css.contains("--gosslan-danger: #d43d43"),
                "样式表里的未读红改了？本测试与 UNREAD_RED 要一起更新"
            );
            assert_eq!(
                UNREAD_RED,
                (0xD4, 0x3D, 0x43),
                "托盘红点必须等于 --gosslan-danger（#d43d43）"
            );
        }

        /// 红点必须是**小红点**：右上角、圆心是红的、面积很小、其余像素一个都不许动。
        /// 面积上限这条是用户 2026-09-17 的反馈固化下来的（第一版 22% 被评「太丑」）。
        #[test]
        fn unread_dot_is_small_and_lands_top_right() {
            const W: u32 = 32;
            const H: u32 = 32;
            let base = vec![9u8; (W * H * 4) as usize];
            let out = with_unread_dot(&base, W, H);
            assert_eq!(
                out.len(),
                base.len(),
                "长度必须不变（托盘图标按这个长度解析）"
            );
            let px = |x: u32, y: u32| {
                let i = ((y * W + x) * 4) as usize;
                (out[i], out[i + 1], out[i + 2], out[i + 3])
            };
            // 圆心 = (W×0.82, H×0.18) ⇒ 该像素（26, 6）完全落在红点内部
            assert_eq!(
                px(26, 6),
                (UNREAD_RED.0, UNREAD_RED.1, UNREAD_RED.2, 255),
                "圆心附近必须是纯未读红"
            );
            assert_eq!(px(0, 0), (9, 9, 9, 9), "左上角不该被碰");
            assert_eq!(px(0, H - 1), (9, 9, 9, 9), "左下角不该被碰");
            assert_eq!(px(W - 1, H - 1), (9, 9, 9, 9), "右下角不该被碰");
            assert_eq!(
                px(W - 1, 0),
                (9, 9, 9, 9),
                "右上角尖上也不该被碰（红点要留边）"
            );
            let changed = (0..(W * H))
                .filter(|i| out[(*i * 4) as usize] != base[(*i * 4) as usize])
                .count();
            let ratio = changed as f32 / (W * H) as f32;
            assert!(
                ratio < 0.08,
                "红点（含白边与抗锯齿）占图标面积应 < 8%，实测 {:.1}%",
                ratio * 100.0
            );
        }

        /// 边缘必须有抗锯齿：整图里存在既不是底色、也不是纯白、也不是纯红的像素。
        #[test]
        fn unread_dot_edges_are_antialiased() {
            const W: u32 = 64;
            const H: u32 = 64;
            let base = vec![9u8; (W * H * 4) as usize];
            let out = with_unread_dot(&base, W, H);
            let midtones = (0..(W * H))
                .filter(|i| {
                    let v = out[(*i * 4) as usize];
                    v != 9 && v != 255 && v != UNREAD_RED.0
                })
                .count();
            assert!(midtones > 0, "圆点边缘应出现混合色（否则就是硬边）");
        }

        /// 输入长度不对时原样返回：宁可不画，也不能越界写（崩溃级缺陷）。
        #[test]
        fn malformed_buffer_is_returned_as_is() {
            let short = vec![1u8; 10];
            assert_eq!(with_unread_dot(&short, 32, 32), short);
        }
    }
}
/// 更新未读提醒：托盘红点 + tooltip 条数 + Dock / 启动器角标。
///
/// 这几件事一起做，是因为它们**表达同一件事**（还有未读），分几处状态就会漂移：
/// 用户报过"红点清了但 Dock 上还挂着 3"。
///
/// 平台分工（都由 Tauri/tao 提供，无新增依赖）：
/// - **Windows**：托盘图标叠**小红点**（见 `DOT_R_RATIO`）+ tooltip 带条数。
///   ⚠️ 刻意**不用** `set_overlay_icon`（任务栏按钮覆盖图标）：它在任务栏上是一整块纯红
///   圆盘、直接压住应用图标，实物很难看（2026-09-17 用户反馈"太丑了"，已撤掉）。
///   任务栏那侧已经有窗口闪烁（`request_attention`）负责提醒，不需要再叠一张图。
/// - **macOS**：Dock 数字角标走 `set_badge_label`（系统原生渲染，数字比红点信息量大）；
///   托盘图标保持不变（macOS 把托盘图标换成红点不符合平台习惯）。
/// - **Linux**：`set_badge_count`（Unity 系启动器支持，其它桌面环境静默忽略）。
///
/// 幂等、可重复调用；失败一律静默（角标只是锦上添花，不该影响聊天）。
pub fn set_unread_badge<R: tauri::Runtime>(app: &tauri::AppHandle<R>, unread: u32) {
    let zh = app.state::<Arc<crate::state::AppState>>().is_zh();
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tray_tooltip(zh, unread)));
        // macOS：菜单栏直接印数字（托盘图标本身保持原样，那侧的红点不符合平台习惯）。
        #[cfg(target_os = "macos")]
        {
            let _ = tray.set_title(tray_title(unread).as_deref());
        }
        #[cfg(not(target_os = "macos"))]
        {
            let icon = app.default_window_icon().map(|base| {
                if unread == 0 {
                    base.clone()
                } else {
                    tauri::image::Image::new_owned(
                        dot::with_unread_dot(base.rgba(), base.width(), base.height()),
                        base.width(),
                        base.height(),
                    )
                }
            });
            if let Some(icon) = icon {
                let _ = tray.set_icon(Some(icon));
            }
        }
    }

    // Dock（macOS）/ 启动器（Linux）角标。Windows 侧没有等价的"小角标"API（见上），
    // 所以整块都不编译，也就不会有多余的窗口查询。
    #[cfg(not(target_os = "windows"))]
    {
        let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            let label = (unread > 0).then(|| unread.to_string());
            let _ = win.set_badge_label(label);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let count = (unread > 0).then_some(unread as i64);
            let _ = win.set_badge_count(count);
        }
    }
}

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

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(tray_tooltip(state.is_zh(), 0))
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
                //
                // ⚠️ 但**绝不能因此退不出去**（用户 2026-09-12 实测：macOS 托盘「退出」点了没反应）。
                // 原实现是「spawn 里 await network::stop 之后才 app.exit(0)」——
                // 只要那一步卡住（任务不结束 / 锁竞争 / BLE 收尾慢），退出就永远不会发生，
                // 而用户看到的就是"点了没反应"。现在的做法：
                //   ① 一条**守护线程**先兜底：1.5s 后无论如何 `process::exit(0)`；
                //   ② 正常路径仍然走 `app.exit(0)`（让插件有机会保存窗口状态），
                //      但网络停止最多等 800ms（超时就放弃，宁可留 TIME_WAIT 也不能退不出去）。
                let handle = app.clone();
                handle
                    .state::<std::sync::Arc<crate::state::AppState>>()
                    .logger
                    .info("app", "托盘「退出」被点击：开始收尾");
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    eprintln!("[gosslan][app] 正常退出路径未在 1.5s 内完成，强制退出");
                    std::process::exit(0);
                });
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<std::sync::Arc<crate::state::AppState>>();
                    let stopped = tokio::time::timeout(
                        std::time::Duration::from_millis(800),
                        crate::network::stop(&state),
                    )
                    .await
                    .is_ok();
                    state
                        .logger
                        .info("app", format!("网络已停止（{stopped}），正在退出"));
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

#[cfg(test)]
mod tests {
    use super::tray_tooltip;

    /// tooltip 必须带上条数：红点只表达"有未读"，**有几条**只有文字能给。
    /// 中英两种界面都要有，且 0 条时不能留下"（0 条未读）"这种噪音。
    #[test]
    fn tooltip_carries_unread_count_only_when_positive() {
        assert_eq!(tray_tooltip(true, 0), "相闻 · 局域网即时通讯");
        assert_eq!(tray_tooltip(false, 0), "Gosslan · LAN Messenger");
        assert!(tray_tooltip(true, 3).contains("3 条未读"));
        assert!(tray_tooltip(false, 3).contains("3 unread"));
        assert!(!tray_tooltip(true, 0).contains('0'));
    }

    /// 菜单栏数字：0 条必须**清空**（不能留着上一次的数字），超过 99 只写 `99+`。
    #[cfg(target_os = "macos")]
    #[test]
    fn menu_bar_number_clears_at_zero_and_caps_at_99() {
        use super::tray_title;
        assert_eq!(tray_title(0), None, "没有未读时菜单栏上不许留数字");
        assert_eq!(tray_title(1).as_deref(), Some("1"));
        assert_eq!(tray_title(99).as_deref(), Some("99"));
        assert_eq!(tray_title(100).as_deref(), Some("99+"));
        assert_eq!(tray_title(u32::MAX).as_deref(), Some("99+"));
    }
}
