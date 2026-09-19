// 职责边界：
// - 运行日志查询（get/clear_logs）
// - 辅助窗口系统（show_*_window / ensure_aux_window / geometry 计算）
// - ⚠️ 包含 open_settings/open_log/open_group_todos/open_link 四个辅助窗口命令
// ---------------- 运行日志 ----------------

/// 读取内存中的运行日志（时间正序：旧 → 新）。
///
/// `since_secs = Some(N)` 只返回最近 N 秒内的日志；None / 0 = 返回全部。
/// 例：since_secs = Some(30) → 近 30 秒；Some(60) → 近 1 分钟。
#[tauri::command(async)]
pub fn get_logs(state: tauri::State<'_, Arc<AppState>>, since_secs: Option<i64>) -> Vec<LogEntry> {
    let since_ms = since_secs
        .filter(|&s| s > 0)
        .map(|s| crate::db::now_ms() - s * 1000);
    state.logger.snapshot(since_ms)
}

/// 清空运行日志（内存 + 落盘文件）。
#[tauri::command(async)]
pub fn clear_logs(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.logger.clear();
    Ok(())
}

/// 独立窗口（设置 / 日志）关闭时**销毁**（用户 2026-09-17：侧边栏的设置/日志收进二级菜单，
/// 窗口不再常驻 —— 用完即关，内存不常驻；重开冷启动一次，由窗口状态插件**之外**的
/// `aux_window_geometry` 统一摆位）。
///
/// ⚠️ 因此 `tauri_plugin_window_state` 对这两个 label 在 `lib.rs` 里做了**拒绝列表**：
/// 否则插件会按 label 恢复"上次的最大化/位置"，与 `apply_aux_geometry` 的居中逻辑打架。
///
/// 外链窗口（`WINDOW_LINK`）是**唯一例外**：它加载远端页面、复用同一个窗口导航，
/// 保持常驻（见 [`AUX_LINK_RESIDENT`]）。
#[cfg(desktop)]
const AUX_WINDOWS_RESIDENT: bool = false;

/// 外链窗口**关闭即隐藏**（常驻）：复用同一个窗口导航到新网址是它的核心交互，
/// 销毁重建会让"再点一条链接"变成一次冷启动。
#[cfg(desktop)]
const AUX_LINK_RESIDENT: bool = true;

/// 独立窗口的开发者工具开关：**调试构建开、正式构建关**（用户 2026-09-17：
/// 「debug 要能调出开发者工具」——调试时要在独立窗口里排查问题）。
///
/// 背景：Tauri 调试构建默认全开；正式构建没开 `devtools` feature，`devtools(true)`
/// 也不会生效 ⇒ 这一条等价于"只跟构建类型走"，显式写出是为了把意图钉住。
/// 外链窗口加载远端页面，发布前若想彻底关掉检查器，把这里改成 `false` 即可。
#[cfg(desktop)]
const AUX_DEVTOOLS: bool = cfg!(debug_assertions);

/// 群任务窗口**关闭即销毁**（不常驻）。
///
/// 为什么与设置/日志不同：群任务窗口是**每群一个**（label = `todo-<groupId>`），常驻的话
/// "打开过 N 个群"就留下 N 个隐藏 WebView，无界增长。而且它每次打开都该是**新数据**
/// （成员可能刚改过任务），销毁重建顺带保证了这一点。代价是重开要冷启动一次
/// （约等于设置窗口第一次打开的成本，小面板可接受）。
#[cfg(desktop)]
const AUX_GROUP_TODOS_RESIDENT: bool = false;

/// 串行化"创建独立窗口"这一步 —— 并发打开同一个窗口是有真实竞态的。
///
/// `WebviewWindowBuilder::build()` 的重复 label 检查在 `prepare_window` 里做
/// （`tauri/src/manager/window.rs`），而窗口被真正登记进 manager 是在主线程创建**完成之后**；
/// 两个并发调用会**双双通过检查**，后者还会覆盖 manager 里的记录（留下一个前台看不见、
/// 也没人管得住的窗口）。用户"连点两下设置"正好会撞上：表现为第二个设置窗口闪一下、
/// 甚至先显示成主聊天界面（前端还没切到设置页的窗口期）。
#[cfg(desktop)]
static AUX_WINDOW_CREATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 已存在就显示并聚焦（返回该窗口 = 处理完了，不需要新建）。
///
/// 这就是"它已经打开了，我再点一下，还是它，不会开出第二个"：
/// 命令层与按钮层都不再需要自己去记"开没开过"。
///
/// `geo` 非空时会**重新把它摆到主窗口正中**（只动位置，不动尺寸）：窗口是常驻的，而主窗口
/// 可以被拖到另一块屏幕上 —— 摆着不动的话，第二次打开它就留在**上一块屏**上
/// （用户 2026-09-16 多屏反馈的"弹到另一个屏幕上"有一半来自这里）。尺寸不重设，
/// 是为了留住用户自己拉过的大小。
///
/// 返回 `Option<WebviewWindow>`（而不是 `bool`）：外链窗口需要在"已存在"这条路径上
/// `navigate()` 到新网址 —— 调用方得拿到窗口句柄。
#[cfg(desktop)]
fn show_existing_aux_window(
    app: &tauri::AppHandle,
    label: &str,
    geo: Option<AuxWindowGeometry>,
) -> Option<tauri::WebviewWindow> {
    let win = app.get_webview_window(label)?;
    if let Some(g) = geo {
        recenter_aux_window(&win, &g);
    }
    // `unminimize`：窗口被最小化过的话，只 show 不会把它拉回前台。
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    Some(win)
}

/// 关闭 → 隐藏（配合 [`AUX_WINDOWS_RESIDENT`]），让下一次打开是瞬时的。
#[cfg(desktop)]
fn install_hide_on_close(win: &tauri::WebviewWindow) {
    let w = win.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = w.hide();
        }
    });
}

/// 无边框辅助窗口（`.decorations(false)`）的 macOS 配套处理。
///
/// 与主窗口（`lib.rs` 里对 `WINDOW_MAIN` 的那两行）同一套：
/// - `set_closable(true)`：`decorations:false` 让 tao 建出的 NSWindow 是 Borderless，**没有 Closable 位**，
///   于是 ⌘W（`menu.rs` 的自定义项走 `.close()`）会失效；
/// - `disable_shadow`：系统阴影是**矩形**，与窗口自身的圆角冲突（会露出四个直角）。
///
/// ⚠️ **不要**对外链窗口（`WINDOW_LINK`）调用：它保留系统标题栏，去掉阴影会是可见的退化。
#[cfg(desktop)]
fn decorate_aux_window(win: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let _ = win.set_closable(true);
        crate::macos_window::disable_shadow(win);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = win;
}

/// 打开（或聚焦）一个独立窗口：**单例 + 串行创建**。
///
/// 所有独立窗口都走这里，别在各自的命令里各写一遍 —— 单例与并发安全是"每个窗口都要有"的
/// 性质，散着写就一定会漏（这正是用户 2026-09-12 报的"连点会出怪事"的来源）。
///
/// `geo` 是**按主窗口**算好的几何（见 [`aux_window_geometry`]）：创建时用它摆尺寸与位置，
/// 已存在时用它把窗口重新摆回主窗口那块屏（见 [`show_existing_aux_window`]）。
///
/// `resident`：关闭时"隐藏（常驻）"还是"销毁"。外链窗口传
/// [`AUX_LINK_RESIDENT`]（复用同一窗口导航）；设置/日志/群任务均销毁（每次开窗拿新数据）。
///
/// 返回 `(窗口, 是否本次新建)`：外链窗口需要在"已存在"这条路径上 `navigate()` 到新网址。
#[cfg(desktop)]
fn ensure_aux_window<F>(
    app: &tauri::AppHandle,
    label: &str,
    geo: Option<AuxWindowGeometry>,
    resident: bool,
    build: F,
) -> Result<(tauri::WebviewWindow, bool), String>
where
    F: FnOnce() -> Result<tauri::WebviewWindow, tauri::Error>,
{
    // 快路径：已经建过（包括"上次关掉只是隐藏了"）⇒ 显示 + 聚焦。
    if let Some(win) = show_existing_aux_window(app, label, geo) {
        return Ok((win, false));
    }
    // 慢路径：同一时刻只允许一个创建者。等锁期间别人可能已经建好了 ⇒ 拿到锁后**再查一次**。
    let _guard = AUX_WINDOW_CREATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(win) = show_existing_aux_window(app, label, geo) {
        return Ok((win, false));
    }
    let win = build().map_err(|e| format!("创建 {label} 窗口失败: {e}"))?;
    if resident {
        install_hide_on_close(&win);
    }
    let _ = win.show();
    let _ = win.set_focus();
    Ok((win, true))
}

/// 独立窗口衬在主窗口里的留边（**逻辑**像素）：子窗口与主窗口边缘至少隔开这么多。
#[cfg(desktop)]
const AUX_WINDOW_MARGIN: f64 = 24.0;

/// 独立窗口的几何 —— **全部是物理像素**（理由见 [`fit_aux_window`]）。
#[cfg(desktop)]
#[derive(Clone, Copy)]
struct AuxWindowGeometry {
    /// 主窗口外框尺寸 / 左上角（物理像素，虚拟桌面坐标）：子窗口要在它里面居中。
    main_size: (u32, u32),
    main_pos: (i32, i32),
    /// 子窗口内尺寸（物理像素）。
    size: (u32, u32),
    /// 子窗口最小内尺寸（物理像素）。必须 ≤ `size`，否则系统会把窗口顶回最小值。
    min: (u32, u32),
}

#[cfg(desktop)]
impl AuxWindowGeometry {
    /// 居中位置：子窗口**外框**在主窗口外框内居中（物理像素）。
    ///
    /// 用外框而不是内尺寸：`tao` 的 WM_DPICHANGED 与创建路径都会按缩放重新算边框厚度，
    /// 按内尺寸居中会带上半个边框的偏差 —— 换到不同缩放的屏上更明显。
    /// （应用窗口现在都是 `decorations(false)`，外框≈内尺寸；但外链窗口仍有系统标题栏，
    /// 而且保留 `outer_size()` 一视同仁更省心。）
    fn centered_pos(&self, aux_outer: (u32, u32)) -> (i32, i32) {
        (
            self.main_pos.0 + ((self.main_size.0 as i64 - aux_outer.0 as i64) / 2) as i32,
            self.main_pos.1 + ((self.main_size.1 as i64 - aux_outer.1 as i64) / 2) as i32,
        )
    }
}

/// 按**主窗口**算独立窗口的尺寸：装得下就用设计尺寸，装不下按主窗口缩到装得下；
/// 位置在主窗口内居中（见 [`AuxWindowGeometry::centered_pos`]）。
///
/// ## 为什么参照物是主窗口
///
/// 用户 2026-09-16 反馈「新窗口打开时没有居中，而且比主窗口还大很多」：主窗口是**可缩放**的，
/// 用户把它拉小之后，一块按屏幕算出来的子窗口就会比它大、而且落在屏幕正中 —— 离它该贴着的
/// 那个窗口很远。子窗口的参照物只能是它**从哪来**。
///
/// ## 为什么全程物理像素
///
/// 用户 2026-09-16 多屏反馈「子窗口弹到另一个屏幕上、位置也没居中」：
/// `WebviewWindowBuilder::position/inner_size` 只收**逻辑**坐标，而 `tao` 在创建窗口时会把
/// 逻辑坐标**逐个显示器**地按该显示器的缩放换回物理，取第一个"换算结果落在自己范围内"的显示器
/// （`tao/src/platform_impl/windows/window.rs` 的 `available_monitors().find_map(..)`）；
/// 一个都没命中就退回 `CW_USEDEFAULT`（主屏层叠位置）：
///
/// - 主窗口在 150% 的副屏、另一块屏 100% 时：按副屏缩放算出的逻辑坐标，再按 100% 换算回来，
///   正好落进那块屏 ⇒ **子窗口跑到另一块屏幕上**；
/// - 尺寸也走同一条换算（按"选中显示器"的缩放）⇒ 大小同样不对。
///
/// 物理坐标没有这一步换算。所以尺寸/位置都不走 builder，而是 `build()` **之后**用物理值落地
/// （`apply_aux_geometry`）—— 窗口以 `visible(false)` 创建，摆好再 `show()`，用户看不到跳变。
///
/// `None` = 拿不到主窗口（还没建出来 / 平台查询失败），此时保持 builder 的设计尺寸 + 系统默认摆位。
#[cfg(desktop)]
fn aux_window_geometry(
    app: &tauri::AppHandle,
    ideal: (f64, f64),
    min: (f64, f64),
) -> Option<AuxWindowGeometry> {
    let main = app.get_webview_window(crate::WINDOW_MAIN)?;
    // 主窗口所在显示器的缩放：子窗口要跟主窗口"看起来"一样大，就用它的缩放把设计尺寸换成物理。
    let scale = main.scale_factor().ok()?;
    let outer = main.outer_size().ok()?; // 物理像素：外框（主窗口无边框，外框≈内尺寸）
    let origin = main.outer_position().ok()?; // 物理像素：虚拟桌面坐标（副屏可能是负的/上千）
    if outer.width == 0 || outer.height == 0 {
        return None;
    }
    Some(fit_aux_window(
        (outer.width, outer.height),
        (origin.x, origin.y),
        scale,
        ideal,
        min,
    ))
}

/// [`aux_window_geometry`] 的**纯计算**部分（拿不到主窗口尺寸的那些查询不在里面）。
///
/// 抽出来的理由只有一个：能在 `cargo test` 里直接验"永远不比主窗口大 + 居中"这两条 ——
/// 它们都是**几何不变式**，靠真机肉眼是量不准的（用户报的就是"没居中、还比主窗口大"）。
#[cfg(desktop)]
fn fit_aux_window(
    main_size: (u32, u32),
    main_pos: (i32, i32),
    scale: f64,
    ideal: (f64, f64),
    min: (f64, f64),
) -> AuxWindowGeometry {
    // 逻辑 → 物理（用主窗口所在显示器的缩放）
    let px = |v: f64| (v * scale).round().max(1.0) as u32;
    let margin = px(AUX_WINDOW_MARGIN);
    // 目标是主窗口内"两侧各留 margin"的那块区域；设计尺寸装不下就缩到刚好装下。
    let fit = |want: u32, avail: u32| want.min(avail.saturating_sub(2 * margin).max(1));
    let (w, h) = (fit(px(ideal.0), main_size.0), fit(px(ideal.1), main_size.1));
    AuxWindowGeometry {
        main_size,
        main_pos,
        size: (w, h),
        // 最小尺寸不能大于实际尺寸：否则系统会把窗口顶回最小值，"缩小"等于白做。
        // 顺带一提，这里的最小尺寸也必须用**物理**值下发：builder 上的 `min_inner_size`
        // 是逻辑值，会在另一块缩放的屏上被换算成别的物理下限。
        min: (px(min.0).min(w), px(min.1).min(h)),
    }
}

/// 把几何**落地到新窗口**上（物理像素）。必须在 `show()` 之前调用，且窗口要隐藏着建。
#[cfg(desktop)]
fn apply_aux_geometry(win: &tauri::WebviewWindow, geo: AuxWindowGeometry) {
    use tauri::{PhysicalSize, Size};
    let _ = win.set_min_size(Some(Size::Physical(PhysicalSize::new(
        geo.min.0, geo.min.1,
    ))));
    let _ = win.set_size(Size::Physical(PhysicalSize::new(geo.size.0, geo.size.1)));
    recenter_aux_window(win, &geo);
    // 窗口状态插件（`tauri_plugin_window_state`，flags 含 MAXIMIZED/FULLSCREEN）会按 label 记住
    // 最大化/全屏状态，而辅助窗口的 label 是固定的 ⇒ 用户最大化过一次，之后**每次打开都会恢复成
    // 最大化**，直接破坏"小窗口不给最大化"。这里在显示前强制拉回窗口态。
    let _ = win.unmaximize();
    let _ = win.set_fullscreen(false);
}

/// 把窗口摆到主窗口正中（**只动位置，不动尺寸**）。
///
/// 位置按**真实外框**算：外框含标题栏与边框，而这两样在不同缩放的屏上厚度不同，
/// 事先估算不出来（所以要在尺寸确定之后再问窗口自己）。尺寸不动，是为了留住用户自己拉过的大小。
#[cfg(desktop)]
fn recenter_aux_window(win: &tauri::WebviewWindow, geo: &AuxWindowGeometry) {
    use tauri::{PhysicalPosition, Position};
    let outer = match win.outer_size() {
        Ok(s) => (s.width, s.height),
        Err(_) => geo.size, // 拿不到就按内尺寸居中（差半个标题栏，无伤）
    };
    let (x, y) = geo.centered_pos(outer);
    let _ = win.set_position(Position::Physical(PhysicalPosition::new(x, y)));
}

/// 桌面端：打开独立的「运行日志」窗口（已存在则聚焦）。
///
/// 窗口加载 `logs.html`（它自己的文档与入口，见 `src/entries/logs.ts`）—— 只加载日志页
/// 需要的代码，不会把聊天界面挂起来再换掉。窗口**无系统标题栏**（`decorations(false)`），
/// 顶部由前端自绘（`TitleBar`，功能名 = 运行日志）；关闭即**销毁**
/// （用户 2026-09-17：侧边栏收进二级菜单后，这两个窗口改"用完即关"），每次打开都是新数据。
/// 窗口标题由 `logs.html` 的 `data-title-*` + 前端按语言设置 `document.title`
/// （Tauri 会把 document title 同步到窗口标题），Rust 侧不再维护第二份标题文案。
/// macOS 必须在**主线程**调 AppKit（`ns_window().setHasShadow` 等）。async 命令跑在 Tokio worker
/// 线程上，会 EXC_BAD_ACCESS。改成同步命令（Tauri 在 wry 的主线程/IPC 回调里内联执行）。
/// 原来的 async 理由是「窗口创建耗时会卡住主线程」——但 `WebviewWindowBuilder::build()` 本身
/// 内部就会把创建分派到主线程、同步等返回，所以 async/同步**对窗口创建耗时没影响**；真正的阻塞
/// 风险是 db 锁，所以 `aux_window_background` 改成了 try_lock（见下方）。
#[cfg(desktop)]
#[tauri::command]
pub fn open_log_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let bg = aux_window_background(&state);
    // 初始标题：文档标题（`logs.html` 的 data-title-* + 前端按语言）加载完成后会被 Tauri
    // 自动同步过去，所以这里只需要一个"还没加载完时不至于空着"的占位。
    let title = aux_window_title(&state, "运行日志", "Runtime Logs", None);
    // 克隆一份给闭包：`ensure_aux_window` 同时借用 `app` 做存在性检查，
    // 闭包再 move 走同一个 handle 会借不过（且闭包必须 `'static` 才能交给 Tauri 创建）。
    let build_app = app.clone();
    // 尺寸与位置按主窗口算（见 aux_window_geometry 的说明）；窗口是**关闭即销毁**的，
    // 每次打开都新建 ⇒ 每次都按主窗口重新居中（不存在"用户摆好的窗口"）。
    let geo = aux_window_geometry(&app, (760.0, 560.0), (420.0, 320.0));
    ensure_aux_window(
        &app,
        crate::WINDOW_LOGS,
        geo,
        AUX_WINDOWS_RESIDENT,
        move || {
            let win = WebviewWindowBuilder::new(
                &build_app,
                crate::WINDOW_LOGS,
                WebviewUrl::App("logs.html".into()),
            )
            .title(title)
            .devtools(AUX_DEVTOOLS)
            // 设计尺寸只作**初值**（拿不到主窗口时它就是最终值）：真正的几何在 build 之后用
            // 物理像素落地 —— builder 的 `position` / `inner_size` 只有逻辑坐标，多屏不同缩放时
            // 会被 tao 按"逐个显示器试算"选错屏（详见 aux_window_geometry）。
            .inner_size(760.0, 560.0)
            .min_inner_size(420.0, 320.0)
            // 背景色跟随主题：窗口的静态背景色只能是浅/深之一，暗色主题下不先设对就会"闪一下白"
            // （与主窗口冷启动白闪同源）。放在 builder 上（而不是 build 之后再 set），
            // 少一帧错色。
            .background_color(bg)
            // 隐藏创建：`ensure_aux_window` 随后就会 `show()`，中间这段正好用来摆位置与尺寸，
            // 用户不会看到窗口先在默认位置上闪一下、再跳到正确的位置。
            .visible(false)
            // 无系统标题栏 + 小窗口不给最大化（用户 2026-09-17）：改用与主窗口同一套自绘标题栏
            // （`TitleBar.vue`），否则顶部那条**系统**标题栏的底色与内容撞色、出现明显接缝。
            .decorations(false)
            .resizable(true)
            .maximizable(false)
            .minimizable(true)
            .closable(true)
            .build()?;
            decorate_aux_window(&win);
            if let Some(g) = geo {
                apply_aux_geometry(&win, g);
            }
            Ok(win)
        },
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 移动端桩：移动端的日志是**整页**（`LogViewer` 的全屏分支），没有独立窗口。
/// 必须有这个桩，否则 `generate_handler!` 在移动端编译不过（见 `open_settings_window`）。
#[cfg(mobile)]
#[tauri::command]
pub fn open_log_window(_app: tauri::AppHandle) -> Result<(), String> {
    Err("移动端没有独立日志窗口（日志是整页）".to_string())
}

/// 桌面端：打开独立的「设置」窗口（已存在则聚焦）。
///
/// 与日志窗口同一范式（同一个 [`ensure_aux_window`]）：加载 `settings.html`（自己的入口），
/// 由前端按窗口自己的文档渲染设置页。用户 2026-09-12 反馈：「PC 端的设置页面可以按照这种
/// 布局，弹一个单独的窗口」——参考图是「左侧窄导航 + 右侧内容」的设置窗口，不是盖在聊天上的弹窗。
#[cfg(desktop)]
#[tauri::command]
pub fn open_settings_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let bg = aux_window_background(&state);
    let title = aux_window_title(&state, "设置", "Settings", None); // 同 open_log_window：文档加载后由前端接管
    let build_app = app.clone(); // 同 open_log_window：闭包要 `'static`，不能再借 `app`
                                 // 尺寸与位置按主窗口算，理由见 aux_window_geometry。
    let geo = aux_window_geometry(&app, (780.0, 600.0), (560.0, 420.0));
    ensure_aux_window(
        &app,
        crate::WINDOW_SETTINGS,
        geo,
        AUX_WINDOWS_RESIDENT,
        move || {
            let win = WebviewWindowBuilder::new(
                &build_app,
                crate::WINDOW_SETTINGS,
                WebviewUrl::App("settings.html".into()),
            )
            .title(title)
            .devtools(AUX_DEVTOOLS)
            // 同 open_log_window：设计尺寸只作初值，几何在 build 之后按物理像素落地。
            .inner_size(780.0, 600.0)
            .min_inner_size(560.0, 420.0)
            .background_color(bg)
            .visible(false)
            // 同 open_log_window：无系统标题栏（共用自绘标题栏）+ 小窗口不给最大化。
            .decorations(false)
            .resizable(true)
            .maximizable(false)
            .minimizable(true)
            .closable(true)
            .build()?;
            decorate_aux_window(&win);
            if let Some(g) = geo {
                apply_aux_geometry(&win, g);
            }
            Ok(win)
        },
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 桌面端：打开独立「群任务」窗口（已存在则聚焦）。**每个群一个窗口**（label = `todo-<groupId>`）。
///
/// 与设置/日志同一范式（同一个 [`ensure_aux_window`]），但**关闭即销毁**（见
/// [`AUX_GROUP_TODOS_RESIDENT`]）：每群一个窗口，常驻会无界增长，且每次打开都该拿新数据。
/// 窗口加载 `todos.html`（自己的文档与入口），并从**自己的 label** 解析群 ID
/// （见 `src/utils/auxWindowLabels.ts`）—— 所以不做"窗口内切群"，一个窗口只服务一个群。
#[cfg(desktop)]
#[tauri::command]
pub fn open_group_todos_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    group_id: String,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    // groupId 会被拼进窗口 label ⇒ 严格校验字符集（非法字符会让 label 失效，也可能撞上别的窗口）。
    if group_id.is_empty()
        || group_id.len() > 40
        || !group_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("群 ID 非法".to_string());
    }
    let label = format!("{}{group_id}", crate::WINDOW_GROUP_TODOS_PREFIX);
    let bg = aux_window_background(&state);
    // 系统标题带上群名（每群一个窗口，任务栏里得能分清）；文档加载后由前端按同样口径接管。
    let group_name = {
        // try_lock：同 aux_window_background，主线程安全降级（拿不到锁就用空群名当兜底）。
        let Ok(dbc) = state.db.try_lock() else {
            return Err("数据库暂时被占用，请稍后再试".to_string());
        };
        db::get_group(&dbc, &group_id)
            .map(|g| g.name)
            .unwrap_or_default()
    };
    let title = aux_window_title(&state, "群任务", "Group Tasks", Some(&group_name));
    let build_app = app.clone();
    let build_label = label.clone();
    let geo = aux_window_geometry(&app, (560.0, 620.0), (360.0, 420.0));
    ensure_aux_window(&app, &label, geo, AUX_GROUP_TODOS_RESIDENT, move || {
        let win = WebviewWindowBuilder::new(
            &build_app,
            build_label.as_str(),
            WebviewUrl::App("todos.html".into()),
        )
        .title(title)
        .devtools(AUX_DEVTOOLS)
        // 同 open_log_window：设计尺寸只作初值，几何在 build 之后按物理像素落地。
        .inner_size(560.0, 620.0)
        .min_inner_size(360.0, 420.0)
        .background_color(bg)
        .visible(false)
        // 同 open_log_window：无系统标题栏（共用自绘标题栏）+ 小窗口不给最大化。
        .decorations(false)
        .resizable(true)
        .maximizable(false)
        .minimizable(true)
        .closable(true)
        .build()?;
        decorate_aux_window(&win);
        if let Some(g) = geo {
            apply_aux_geometry(&win, g);
        }
        Ok(win)
    })
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 移动端桩：独立群任务窗口是桌面概念（移动端用应用内弹窗 `GroupTasksPanel`）。
#[cfg(mobile)]
#[tauri::command]
pub fn open_group_todos_window(_app: tauri::AppHandle, _group_id: String) -> Result<(), String> {
    Err("移动端没有独立群任务窗口（任务面板是应用内弹窗）".to_string())
}

/// 桌面端：打开（或复用）独立的「外部链接」窗口，在窗口内加载该网址。
///
/// **安全边界**（本仓库唯一一处在本机渲染第三方网页，三道锁）：
/// 1. `url` 先过 [`validate_external_url`]（只允许 http/https 且 host 非空）；
/// 2. 窗口 label [`crate::WINDOW_LINK`] **刻意不在 capabilities 里**（也不在 `WINDOW_LABELS`）
///    ⇒ 远端页面调不动本应用的任何命令；
/// 3. `on_navigation` 只放行 http/https ⇒ 远端页面跳不进 `file://` / `tauri://`。
///
/// 复用同一个窗口：已在打开状态时**导航到新网址**而不是再开一个（避免无界增长远端 WebView）。
///
/// **唯一保留系统标题栏的窗口**（其余应用窗口都是 `decorations(false)` + 自绘）：它加载的是
/// 远端页面，我们自己的文档不在这里，套自绘标题栏只能改用 iframe 包一层 —— 而大量站点有
/// `X-Frame-Options`，会直接白屏。关闭即隐藏（常驻），再点是瞬时的。
#[cfg(desktop)]
#[tauri::command]
pub fn open_link_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    url: String,
    name: String,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let url = validate_external_url(&url)?;
    let parsed = url::Url::parse(&url).map_err(|_| "网址格式不正确".to_string())?;
    let bg = aux_window_background(&state);
    let trimmed = name.trim().to_string();
    let title = if trimmed.is_empty() {
        state.display_name()
    } else {
        trimmed
    };
    let build_app = app.clone();
    let build_title = title.clone();
    let build_url = parsed.clone();
    let geo = aux_window_geometry(&app, (1000.0, 720.0), (420.0, 320.0));
    let (win, created) = ensure_aux_window(
        &app,
        crate::WINDOW_LINK,
        geo,
        AUX_LINK_RESIDENT,
        move || {
            let win = WebviewWindowBuilder::new(
                &build_app,
                crate::WINDOW_LINK,
                WebviewUrl::External(build_url),
            )
            .title(build_title)
            .devtools(AUX_DEVTOOLS)
            // 只放行 http/https 的后续导航：远端页面若想跳到 file:// / tauri:// 一律拒绝。
            .on_navigation(|u| matches!(u.scheme(), "http" | "https"))
            .inner_size(1000.0, 720.0)
            .min_inner_size(420.0, 320.0)
            .background_color(bg)
            .visible(false)
            .build()?;
            if let Some(g) = geo {
                apply_aux_geometry(&win, g);
            }
            Ok(win)
        },
    )?;
    // 复用已有窗口：导航到新网址并更新标题。新建时它已加载该网址，无需再 navigate。
    if !created {
        let _ = win.navigate(parsed);
        let _ = win.set_title(&title);
    }
    Ok(())
}

/// 移动端桩：窗口内加载外部网页是桌面概念。
#[cfg(mobile)]
#[tauri::command]
pub fn open_link_window(_app: tauri::AppHandle, _url: String, _name: String) -> Result<(), String> {
    Err("移动端没有链接窗口".to_string())
}

/// 独立窗口的初始背景色：跟随当前亮暗主题（暗色下打开时不"闪一下白"）。
///
/// ⚠️ 用 try_lock 而非 lock：辅助窗口命令已改成**同步命令**（主线程跑，
/// 见 `open_log_window` 上方注释），lock 会在 db 被其他线程持有时阻塞主线程
/// → 整个 App 卡死。try_lock 失败时返回**默认浅色**（安全降级：暗色会闪一下白，
/// 但下一个 async 命令读 db 更新主题很快，用户感知不到；比卡死好得多）。
#[cfg(desktop)]
fn aux_window_background(state: &tauri::State<'_, Arc<AppState>>) -> tauri::window::Color {
    let dark = {
        let Ok(dbc) = state.db.try_lock() else {
            return tauri::window::Color(237, 241, 246, 255); // 默认浅色兜底
        };
        db::get_setting(&dbc, "dark_mode")
            .map(|v| v == "1")
            .unwrap_or(false)
    };
    if dark {
        tauri::window::Color(11, 18, 32, 255) // #0b1220
    } else {
        tauri::window::Color(237, 241, 246, 255) // #edf1f6
    }
}

/// 独立窗口的**系统标题**（任务栏 / Alt–Tab / 系统窗口列表显示的名字）。
///
/// 为什么不能只靠文档标题接管：`document.title` 的同步要等页面加载完成，在那之前
/// （以及同步失败时）任务栏里所有窗口都只叫应用名，分不清哪个是哪个
/// （用户 2026-09-17：「独立窗口在系统里显示的窗口名字不对，没给系统设置名字」）。
/// 所以创建时就给功能名；**不带应用名前缀**（用户同日反馈，任务栏本身已按应用分组）。
/// 前端加载后仍会按语言把 `document.title` 设成同样的格式，两边口径一致。
#[cfg(desktop)]
fn aux_window_title(
    state: &tauri::State<'_, Arc<AppState>>,
    feature_zh: &str,
    feature_en: &str,
    extra: Option<&str>,
) -> String {
    let feature = if state.is_zh() {
        feature_zh
    } else {
        feature_en
    };
    match extra {
        Some(x) if !x.is_empty() => format!("{feature} · {x}"),
        _ => feature.to_string(),
    }
}

/// 移动端桩：独立的设置窗口是**桌面**概念（`decorations:false` 自绘标题栏 + 多窗口），
/// 移动端用的是整页设置页（`SettingsPanel` 的全屏分支）。
///
/// 为什么必须有这个桩：`lib.rs` 的 `generate_handler!` 是**无条件**列出命令的，
/// 而 `#[tauri::command]` 生成的包装宏跟着函数一起被 `#[cfg(desktop)]` 裁掉
/// ⇒ Android/iOS 目标上 `generate_handler!` 找不到它、**整个移动端编译不过**。
/// （这是真实缺陷：本轮为了验证蓝牙在 Android 上能否编译时才发现。）
#[cfg(mobile)]
#[tauri::command]
pub fn open_settings_window(_app: tauri::AppHandle) -> Result<(), String> {
    Err("移动端没有独立设置窗口（设置是整页，见 SettingsPanel）".to_string())
}

/// 桌面端：关闭独立的「设置」窗口。
///
/// 设置窗口是**关闭即销毁**（[`AUX_WINDOWS_RESIDENT`] = false），`close()` 直接销毁 ——
/// 标题栏关闭键走的是同一条路（`window_close` → `close()`）。
#[cfg(desktop)]
#[tauri::command]
pub fn close_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(crate::WINDOW_SETTINGS) {
        let _ = win.close();
    }
    Ok(())
}

/// 移动端桩：见 `open_settings_window` 的说明（`generate_handler!` 无条件列出，
/// 桌面专属命令必须有移动端对应物，否则移动端编译不过）。
#[cfg(mobile)]
#[tauri::command]
pub fn close_settings_window(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

/// 桌面端：关闭独立的「运行日志」窗口（与设置窗口一致：关闭即销毁）。
#[cfg(desktop)]
#[tauri::command]
pub fn close_log_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(crate::WINDOW_LOGS) {
        let _ = win.close();
    }
    Ok(())
}

/// 移动端桩：见 `open_log_window` 的说明。
#[cfg(mobile)]
#[tauri::command]
pub fn close_log_window(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

// ---------------- 测试 ----------------
include!("logs_tests.rs");
