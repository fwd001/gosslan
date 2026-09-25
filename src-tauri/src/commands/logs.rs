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

/// **默认策略**：辅助窗口关闭时**销毁**。目前只剩设置与日志走这一条。
///
/// 为什么这两个仍然是销毁（用户 2026-09-24 定的口径："除了设置日志关闭就销毁，其他新窗口
/// 默认不销毁"）：设置页要的是**当前**环境（网卡 / 共享目录 / 数据量），日志要的是**最新**
/// 尾部；常驻就得自己实现"重新显示时刷新"，而它们本来每次都要重读，销毁重建反而少一层状态。
/// 其余窗口（群任务 / 图片预览）改成常驻 ⇒ 第二次打开是 show + 换内容，不再冷启动。
///
/// ⚠️ 因此 `tauri_plugin_window_state` 对这两个 label 在 `lib.rs` 里做了**拒绝列表**：
/// 否则插件会按 label 恢复"上次的最大化/位置"，与 `apply_aux_geometry` 的居中逻辑打架。
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

/// 群任务窗口**关闭即隐藏**（常驻）。
///
/// 用户 2026-09-24 的两条口径合到这里：① "任务列表窗口第二次打开快点" ⇒ 常驻；
/// ② "可不传参数、后台默默先把 WebView 建好，用的时候瞬间激活" ⇒ label 必须是**固定**的
/// `tasks`（每群一扇的 `todo-<groupId>` 在启动时不知道该建哪一扇，预热无从下手）。
/// 固定 label 之后全局就一扇 ⇒ 不再需要淘汰逻辑；而"每次打开都是新数据"以前靠销毁重建
/// 顺带保证，现在改由**复用时定向通知窗口重读**顶上（见 [`open_group_todos_window`]）。
#[cfg(desktop)]
const AUX_TASKS_RESIDENT: bool = true;

/// 图片预览窗口**关闭即隐藏**（常驻）。
///
/// "全局只有一个"本来就靠固定 label + `ensure_aux_window` 的单例，与是否常驻无关；
/// 改成常驻之后 ✕ 不再释放相册，所以**隐藏时要让那个文档自己把内容放掉** ——
/// 但这件事不在 `install_hide_on_close` 里做（那个回调只 `prevent_close + hide`，不 await、
/// 不发 IPC，见它自己的注释），而是前端自己监听关闭请求（`PreviewWindow.vue` 的
/// `onCloseRequested`）。本注释原先写着"见 `aux-hidden` 事件"，而**全仓不存在那个事件**
/// —— 照着它去 Rust 里找会一无所获，所以改成写实：释放动作在前端，入口是关闭请求。
#[cfg(desktop)]
const AUX_PREVIEW_RESIDENT: bool = true;

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
    reveal: bool,
) -> Option<tauri::WebviewWindow> {
    let win = app.get_webview_window(label)?;
    if !reveal {
        // 预热路径命中一个已存在的窗口：什么都不做（既不能显示、也不能改位置）。
        return Some(win);
    }
    if let Some(g) = geo {
        recenter_aux_window(&win, &g);
    }
    // `unminimize`：窗口被最小化过的话，只 show 不会把它拉回前台。
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    Some(win)
}

/// 关闭 → 隐藏（配合各窗口自己的 `AUX_*_RESIDENT`），让下一次打开是瞬时的。
///
/// ⚠️ 这个回调跑在 **wry 的主线程事件循环里**：本文件已经为"在主线程里做需要等主线程的
/// 调用"死锁过一次（见 `log_window_body` 的说明），所以这里**只做两件事** —— 拦下关闭、
/// 隐藏。不 await、不发 IPC、也不去销毁别的窗口。
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
///
/// ## 为什么是"把这几行投回主线程"而不是"把整个命令搬回主线程"
///
/// AppKit 调用必须在主线程；但**命令本身不能跑在主线程** —— 同步命令会在 wry 的 IPC 回调里
/// 内联执行，而 Windows 上建 WebView2 会在**调用线程里泵消息**（见 [`ensure_aux_window`] 的
/// 详细说明）⇒ 与正在处理的 IPC 重入 ⇒ 主线程永久挂死（用户 2026-09-20 真机：Win 端点「设置」
/// 整个界面无响应，只能杀进程）。两边的约束只能这样同时满足。
///
/// `run_on_main_thread` 只入队不等待（投的是 `Message::Task`），而窗口创建（`CreateWindow`）
/// 走同一条队列**且先入队** ⇒ 这几行一定发生在窗口建出来之后（`ns_window()` 不会拿到空指针）。
#[cfg(desktop)]
fn decorate_aux_window(win: &tauri::WebviewWindow, app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let w = win.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = w.set_closable(true);
            crate::macos_window::disable_shadow(&w);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (win, app);
}

/// 打开（或聚焦）一个独立窗口：**单例 + 串行创建**。
///
/// 所有独立窗口都走这里，别在各自的命令里各写一遍 —— 单例与并发安全是"每个窗口都要有"的
/// 性质，散着写就一定会漏（这正是用户 2026-09-12 报的"连点会出怪事"的来源）。
///
/// `geo` 是**按主窗口**算好的几何（见 [`aux_window_geometry`]）：创建时用它摆尺寸与位置，
/// 已存在时用它把窗口重新摆回主窗口那块屏（见 [`show_existing_aux_window`]）。
///
/// `resident`：关闭时"隐藏（常驻）"还是"销毁"。2026-09-24 之后的分布是
/// 常驻 = 群任务 / 预览 / 外链，销毁 = 设置 / 日志（这两个每次都要读最新的环境与日志尾部）。
/// 每扇的取舍理由写在各自的 `AUX_*_RESIDENT` 常量上，别在这里再数一遍（数一遍就会过时）。
///
/// `reveal`：真的要打开（显示 + 抢焦点），还是**预热**（只把 WebView 与文档准备好，不显示）。
/// 见 [`prewarm_aux_windows`]。
///
/// 返回 `(窗口, 是否本次新建)`：外链窗口需要在"已存在"这条路径上 `navigate()` 到新网址。
///
/// ## ⚠️ 调用方**必须**是 async 命令（Windows 上否则永久挂死主线程）
///
/// 同步命令在 Tauri 里是**在 wry 的 IPC 回调里、主线程内联执行**的。而窗口创建在 Windows 上会
/// 走 WebView2：`CreateCoreWebView2Controller` + `wait_with_pump`，**在调用线程里泵消息**
/// —— 主线程一边建窗一边泵消息，就会重入正在处理的 IPC 回调：
/// 本函数持有 `AUX_WINDOW_CREATE_LOCK`（std Mutex，**不可重入**），重入后第二次 `lock()`
/// 由同一线程发起 ⇒ 永久自锁，界面整体无响应、只能杀进程（用户 2026-09-20 Win 真机实证）。
/// `tauri-runtime-wry` 源码里那行 "must be called from a separate thread, otherwise the channel
/// will introduce a deadlock" 说的就是这件事。
///
/// 工作线程发起则安全：`build()` 只是把创建**入队**到事件循环并立刻返回（`proxy.send_event`），
/// 主线程不在任何 IPC 回调里 ⇒ 没有可重入的现场。macOS 需要的"AppKit 必须在主线程"由
/// [`decorate_aux_window`] 单独投回主线程。
///
/// `reveal = false` 是**预热**用的：把这扇窗建好、文档加载好，但不显示也不抢焦点
/// （用户 2026-09-24：「启动的时候后台异步把高频窗口的 WebView 开销开好，用的时候秒开」）。
/// 建完之后真正的"打开"走的是 `show_existing_aux_window` 那条快路径 ⇒ 只剩摆位与 show。
#[cfg(desktop)]
fn ensure_aux_window<F>(
    app: &tauri::AppHandle,
    label: &str,
    geo: Option<AuxWindowGeometry>,
    resident: bool,
    reveal: bool,
    build: F,
) -> Result<(tauri::WebviewWindow, bool), String>
where
    F: FnOnce() -> Result<tauri::WebviewWindow, tauri::Error>,
{
    // 快路径：已经建过（包括"上次关掉只是隐藏了"、也包括"预热过还没用过"）⇒ 按需显示 + 聚焦。
    if let Some(win) = show_existing_aux_window(app, label, geo, reveal) {
        return Ok((win, false));
    }
    // 慢路径：同一时刻只允许一个创建者。等锁期间别人可能已经建好了 ⇒ 拿到锁后**再查一次**。
    let _guard = AUX_WINDOW_CREATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(win) = show_existing_aux_window(app, label, geo, reveal) {
        return Ok((win, false));
    }
    let win = build().map_err(|e| format!("创建 {label} 窗口失败: {e}"))?;
    if resident {
        install_hide_on_close(&win);
    }
    if reveal {
        let _ = win.show();
        let _ = win.set_focus();
    }
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

/// 把「用户上次拉的尺寸」还原回来（**只还原尺寸**，位置仍旧居中在主窗口上）。
///
/// 为什么必须自己再调一次 `restore_state`：`tauri_plugin_window_state` 是在
/// `on_window_ready`（= **build 期间**）还原的，而我们随后就用 `apply_aux_geometry` 落地了
/// 设计尺寸 ⇒ 顺序上"用户尺寸"被盖掉，群任务窗口**每次打开都回到设计尺寸**，
/// 用户拉过的大小等于没被记住（用户 2026-09-21：「这个窗口都没记住用户的尺寸吗？」）。
/// 这里在几何落地**之后**按 label 再还原一次，与插件天然的顺序无关。
///
/// 只还原 `SIZE`、不还原 `POSITION`：位置仍由 [`recenter_aux_window`] 居中 ——
/// 多屏下"窗口弹到另一块屏"是用户报过的老问题（2026-09-16），不能被历史位置带回去；
/// 用户也确认过这个口径：「窗口只用记住大小就行啊，不用记住位置」。
/// ⚠️ 调用方在它之后**必须再居中一次**：本函数会改尺寸，而居中位置是按外框尺寸算的
/// （那是 [`AuxWindowGeometry::centered_pos`] 的输入）。
///
/// 还原出来的尺寸**以设计尺寸为上限**（下钳到最小尺寸）：换默认尺寸时不会被状态文件里
/// 的历史大尺寸顶回去（本轮刚把 1080×780 改小，那把已经落过盘）。想放开上限就改这里。
#[cfg(desktop)]
fn restore_aux_window_size(win: &tauri::WebviewWindow, geo: &AuxWindowGeometry) {
    use tauri::{PhysicalSize, Size};
    use tauri_plugin_window_state::{StateFlags, WindowExt};
    // 没有保存过状态时它会把"当前尺寸"写进缓存（等于设计尺寸），不影响下面的钳制。
    if win.restore_state(StateFlags::SIZE).is_err() {
        return;
    }
    let Ok(cur) = win.inner_size() else {
        return;
    };
    let (w, h) = clamp_aux_size((cur.width, cur.height), geo.min, geo.size);
    if (w, h) != (cur.width, cur.height) {
        let _ = win.set_size(Size::Physical(PhysicalSize::new(w, h)));
    }
}

/// [`restore_aux_window_size`] 的纯计算部分（便于单测）：把还原出来的尺寸夹进 `[min, max]`。
///
/// `max` 取 `.max(min)` 是防御：几何不变式说 `min ≤ size`，但真被违反时 `clamp` 会 panic，
/// 而"窗口打不开"比"窗口尺寸不合意"严重得多。
#[cfg(desktop)]
fn clamp_aux_size(cur: (u32, u32), min: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    (
        cur.0.clamp(min.0, max.0.max(min.0)),
        cur.1.clamp(min.1, max.1.max(min.1)),
    )
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
/// ⚠️ 这个命令**必须**是 `#[tauri::command(async)]`——理由见 [`ensure_aux_window`]：同步命令在
/// wry 的 IPC 回调里内联跑主线程，而 Windows 上建 WebView2 会**在调用线程里泵消息**，
/// 与正在处理的 IPC 重入后会永久挂死主线程（2026-09-20 Win 真机：点「设置」整个界面无响应，
/// 只能杀进程）。macOS 那几行 AppKit 调用改由 [`decorate_aux_window`] 单独投回主线程，
/// 所以这里不再需要"整个命令搬回主线程"。
#[cfg(desktop)]
#[tauri::command(async)]
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
        true, // reveal：真的要把这扇窗显示出来（不是预热）
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
            decorate_aux_window(&win, &build_app);
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
#[tauri::command(async)]
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
        true, // reveal：真的要把这扇窗显示出来（不是预热）
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
            decorate_aux_window(&win, &build_app);
            if let Some(g) = geo {
                apply_aux_geometry(&win, g);
            }
            Ok(win)
        },
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 桌面端：打开（或复用）**全局唯一**的「群任务」窗口，并把它要显示哪个群写成当前上下文。
///
/// 一扇窗、固定 label [`crate::WINDOW_TASKS`]，看的是哪个群由
/// [`AppState::task_window_group`] 决定 ⇒ 切群 = 换内容，与图片预览窗口同一套做法。
/// 之所以必须这样：用户 2026-09-24 要"不传参数就能在后台先把 WebView 建好，用的时候瞬间
/// 激活"，而每群一扇的动态 label 在启动时不知道该建哪一扇。代价是任务栏里不再按群区分。
///
/// `focus_todo_id` = 打开之后要**展开哪一条任务**（#39：点聊天里的任务卡片要直接看到那条的
/// 详情）。三条路都要覆盖，缺一不可：
/// - **新建 / 预热过但没内容**：窗口会经历挂载 ⇒ 只写暂存，由它自己 `take_group_todo_focus`
///   取走（此刻还没有监听者，发事件等于发丢，表现就是"第一次点没反应"）；
/// - **复用**（窗口已经存在，不再经历挂载）：写完上下文与暂存之后**无条件**定向 emit
///   一条 `group-todos-target` —— 它现在承担两件事：换群 + 重读数据（以前"新数据"是
///   销毁重建顺带保证的，常驻之后没人保证了）。
#[cfg(desktop)]
#[tauri::command(async)]
pub fn open_group_todos_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    group_id: String,
    focus_todo_id: Option<String>,
) -> Result<(), String> {
    use tauri::Emitter;
    // groupId 会跨窗口当成指令传回前端（前端拿它去查数据）⇒ 按不透明 ID 的最短规则夹紧。
    if group_id.is_empty()
        || group_id.len() > 40
        || !group_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("群 ID 非法".to_string());
    }
    // todoId 同理：这条通道没有任何"是不是真任务"的判断，前端查不到就什么都不做。
    if let Some(id) = &focus_todo_id {
        if id.is_empty() || id.len() > 80 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("任务 ID 非法".to_string());
        }
    }
    // "该展开哪一条"：带 id 就写入，**不带就清掉** —— 不清的话，用户下一次从标题栏按钮
    // （不带目标）打开时会被上次那条任务莫名展开。
    {
        let mut pending = state
            .todo_focus_request
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match &focus_todo_id {
            Some(id) => {
                pending.insert(group_id.clone(), id.clone());
            }
            None => {
                pending.remove(&group_id);
            }
        }
    }
    // 这扇窗现在该显示哪个群（**当前值**，不是一次性请求：窗口挂载时取的就是它）。
    set_task_window_group(&state, &group_id);
    // 系统标题带上群名；文档加载后前端会按同样口径接管（切群时它会再改一次）。
    let group_name = {
        // ⚠️ 拿不到锁只能**降级成"没有群名"**，绝不能中止开窗（用户 2026-09-24：
        // 「点群里的查看任务，弹窗弹出很慢，我还以为没点了，点了好几下一会才弹出来」）：
        // 这一步原先在锁被占住时直接把整个开窗判成失败 —— 群名只是系统标题栏那一点装饰，
        // 为它牺牲一次用户动作，表现就是"DB 一忙这扇窗就打不开"。
        // 判据由 `cosmetic_title_read_cannot_abort_the_window` 钉住。
        match state.db.try_lock() {
            Ok(dbc) => db::get_group(&dbc, &group_id)
                .map(|g| g.name)
                .unwrap_or_default(),
            Err(_) => String::new(),
        }
    };
    let (win, created) = ensure_tasks_window(&app, &state, &group_name, true)?;
    if !created {
        let _ = win.emit(
            "group-todos-target",
            serde_json::json!({ "groupId": group_id.clone() }),
        );
    }
    Ok(())
}

/// 记下"那扇群任务窗口该显示哪个群"。两条写路径（开窗 / 只投递目标）共用这一份取锁形状。
#[cfg(desktop)]
fn set_task_window_group(state: &tauri::State<'_, Arc<AppState>>, group_id: &str) {
    *state
        .task_window_group
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(group_id.to_string());
}

/// 群任务窗口本体（打开与预热**共用同一份**构造，差别只在要不要显示）。
///
/// 与预览窗口同一条理由：两条路各写一份 builder，"预热出来的窗口和真正打开的不一样"
/// 这种缺陷查不出来（尺寸/装饰/背景少一项，秒开就变成"开出来是另一个样子"）。
#[cfg(desktop)]
fn ensure_tasks_window(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, Arc<AppState>>,
    group_name: &str,
    reveal: bool,
) -> Result<(tauri::WebviewWindow, bool), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let bg = aux_window_background(state);
    let title = aux_window_title(state, "群任务", "Group Tasks", Some(group_name));
    let geo = aux_window_geometry(app, (780.0, 620.0), (360.0, 420.0));
    let build_app = app.clone();
    ensure_aux_window(
        app,
        crate::WINDOW_TASKS,
        geo,
        AUX_TASKS_RESIDENT,
        reveal,
        move || {
            let win = WebviewWindowBuilder::new(
                &build_app,
                crate::WINDOW_TASKS,
                WebviewUrl::App("todos.html".into()),
            )
            .title(title)
            .devtools(AUX_DEVTOOLS)
            // 设计尺寸 780×620（用户 2026-09-21 实机反馈：560 太瘦长、1080 太大）；
            // `fit_aux_window` 会夹到「主窗口 − 两侧各 24px」⇒ 永不比主窗口大。
            .inner_size(780.0, 620.0)
            .min_inner_size(360.0, 420.0)
            .background_color(bg)
            // 隐藏创建：`ensure_aux_window` 在几何落地后才 show，中间这段用来摆位置与尺寸。
            .visible(false)
            .decorations(false)
            .resizable(true)
            .maximizable(false)
            .minimizable(true)
            .closable(true)
            .build()?;
            decorate_aux_window(&win, &build_app);
            if let Some(g) = geo {
                apply_aux_geometry(&win, g);
                // 先还原用户拉过的尺寸，再按**当时**的外框重算居中 —— 顺序反了就偏半个差值
                // （用户 2026-09-21：「每次弹出来都不居中」）。只记尺寸、不记位置。
                restore_aux_window_size(&win, &g);
                recenter_aux_window(&win, &g);
            }
            Ok(win)
        },
    )
}

/// 那扇群任务窗口**现在该显示哪个群**（不清，理由见 [`AppState::task_window_group`]）。
///
/// 不分桌面/移动：只是读一块内存。预热过、还没被真正打开时返回 `None` ⇒ 窗口显示
/// "还没选群"那一档，而不是拿一个脏 ID 去查。
#[tauri::command]
pub fn get_group_todos_context(
    state: tauri::State<'_, Arc<AppState>>,
) -> Option<String> {
    state
        .task_window_group
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}
/// 群任务窗口启动时取走"该展开哪条任务"（**取走即清空**，一次性）。
///
/// 与 `open_group_todos_window` 里那条定向 emit 是同一个需求的**两条互补路径**
/// （用户 2026-09-24 #39）：窗口是新建设的 ⇒ 事件发给了还没有监听者的文档，只能靠这里取。
/// 不分桌面/移动：只是读写一块内存，移动端没有窗口会去调它，留着比加一份 cfg 桩更清楚。
#[tauri::command]
pub fn take_group_todo_focus(
    state: tauri::State<'_, Arc<AppState>>,
    group_id: String,
) -> Option<String> {
    state
        .todo_focus_request
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&group_id)
}

/// 「只投递展开请求」，不碰窗口本身 —— 因为前端的启动器有**单飞 + 防抖**。
///
/// 不拆出来会有的洞（用户 2026-09-24 #39）：`launchAuxWindow` 判定"这次不算新打开"时
/// **根本不会调用** `open_group_todos_window`，于是第二张卡片带着的 `todoId` 就地消失，
/// 表现还是"点了没反应"。所以投递展开这件事必须独立于"要不要开窗口"：
/// - 窗口还没建好（在途）⇒ 只写暂存，窗口挂载时自己取走 ⇒ 后一次点击的目标会覆盖前一次，
///   这正是想要的（用户最后点的那条才该被展开）；
/// - 窗口已经开着 ⇒ 写暂存 + 定向叫醒它来取。
///
/// 桌面专属（移动端没有独立任务窗口，展开走同文档传参）。
#[cfg(desktop)]
#[tauri::command(async)]
pub fn request_group_todo_focus(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    group_id: String,
    todo_id: String,
) -> Result<(), String> {
    use tauri::Emitter;
    if group_id.is_empty()
        || group_id.len() > 40
        || !group_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("群 ID 非法".to_string());
    }
    if todo_id.is_empty()
        || todo_id.len() > 80
        || !todo_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("任务 ID 非法".to_string());
    }
    state
        .todo_focus_request
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(group_id.clone(), todo_id);
    set_task_window_group(&state, &group_id);
    // 窗口已经开着 ⇒ 它不会经历挂载，只能靠这条通知去"换群 + 重读 + 展开"。
    // 窗口不存在（或只是预热过）⇒ 什么都不发：它挂载时会自己取上下文与暂存。
    if let Some(win) = app.get_webview_window(crate::WINDOW_TASKS) {
        let _ = win.emit(
            "group-todos-target",
            serde_json::json!({ "groupId": group_id }),
        );
    }
    Ok(())
}

/// 移动端桩：与 `open_group_todos_window` 同因 —— 移动端没有独立任务窗口，
/// 展开目标由 `ChatWindow` 直接把 prop 传给应用内弹窗里的看板。
#[cfg(mobile)]
#[tauri::command]
pub fn request_group_todo_focus(
    _app: tauri::AppHandle,
    _state: tauri::State<'_, Arc<AppState>>,
    _group_id: String,
    _todo_id: String,
) -> Result<(), String> {
    Err("移动端没有独立群任务窗口（任务面板是应用内弹窗）".to_string())
}

/// 移动端桩：独立群任务窗口是桌面概念（移动端用应用内弹窗 `GroupTasksPanel`）。
/// 移动端走的是**同文档传参**（`ChatWindow` 把 `focusTodoId` 直接传给弹窗里的看板），
/// 不经过这条命令 ⇒ 桩只需要保持签名一致。
#[cfg(mobile)]
#[tauri::command]
pub fn open_group_todos_window(
    _app: tauri::AppHandle,
    _group_id: String,
    _focus_todo_id: Option<String>,
) -> Result<(), String> {
    Err("移动端没有独立群任务窗口（任务面板是应用内弹窗）".to_string())
}

/// 一次投递给预览窗口的相册上限（条数 / 名字长度 / data URL 总量）。
///
/// 为什么要上限：这是**远端不可信输入能直接塞进内存**的一条路径（条目来自消息载荷里的
/// 名字与 base64），而且它是全局唯一窗口 —— 一份畸形相册会一直挂在那儿。
/// 超限就整份拒绝，前端退回应用内覆盖层（不是"少显示几张"那种半成品）。
#[cfg(desktop)]
const PREVIEW_MAX_ITEMS: usize = 500;
#[cfg(desktop)]
const PREVIEW_MAX_NAME_LEN: usize = 200;
#[cfg(desktop)]
const PREVIEW_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;

/// 校验一次预览投递（纯函数，便于单测）。
///
/// 只验形状不验"图取不取得到"：取不到字节是预览窗口自己的事（它会显示"文件已被清理"
/// 那套既有状态），在这里拒绝反而会让用户点了完全没反应。
#[cfg(desktop)]
fn validate_preview(g: &crate::state::PreviewGallery) -> Result<(), String> {
    if g.items.is_empty() {
        return Err("没有可预览的图片".to_string());
    }
    if g.items.len() > PREVIEW_MAX_ITEMS {
        return Err(format!("一次最多预览 {PREVIEW_MAX_ITEMS} 张图"));
    }
    if g.index >= g.items.len() {
        return Err("预览起始位置越界".to_string());
    }
    let mut bytes = 0usize;
    for it in &g.items {
        if it.name.chars().count() > PREVIEW_MAX_NAME_LEN {
            return Err(format!("图片名最长 {PREVIEW_MAX_NAME_LEN} 字符"));
        }
        // 每条都必须可寻址：`data_src` 只接受 data URL —— `blob:` 是**发起文档**的句柄，
        // 预览窗口拿到只会渲染成破图（用户 #40 要求"任何界面都能调用这个组件"，
        // 那就更要把"能不能跨文档"说清楚，而不是默默显示一个坏掉的框）。
        match (&it.msg_id, &it.cid, &it.data_src) {
            (Some(m), _, _) if !m.is_empty() => {}
            (_, Some(c), _) if !c.is_empty() => {}
            (
                _,
                _,
                Some(d),
            ) if d.starts_with("data:") => {
                bytes += d.len();
            }
            _ => return Err("图片数据不可跨窗口预览".to_string()),
        }
    }
    if bytes > PREVIEW_MAX_TOTAL_BYTES {
        return Err("图片数据过大，无法跨窗口预览".to_string());
    }
    Ok(())
}

/// 桌面端：打开（或复用）**全局唯一**的「图片预览」窗口，并把这份相册设为它当前内容。
///
/// 用户 2026-09-24 #40 的口径："不同界面查看图片会替换里面的内容"，所以：
/// - label 固定 [`crate::WINDOW_PREVIEW`] ⇒ 从会话、任务、收藏点进来的都是同一个窗口；
/// - 内容写进 [`AppState::preview_gallery`]（**当前值**，不是一次性请求）；
/// - **已开着**：定向 `emit` 一条 `preview-gallery` 叫它重新取；
/// - **刚新建**：那条事件一定发丢（监听器还没注册），由窗口挂载时自己 `get_image_preview_gallery`。
///   两条路都读同一份当前值，所以谁先到都一样，不存在"丢一次点击"。
#[cfg(desktop)]
#[tauri::command(async)]
pub fn open_image_preview(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    gallery: crate::state::PreviewGallery,
) -> Result<(), String> {
    use tauri::Emitter;
    validate_preview(&gallery)?;
    *state
        .preview_gallery
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(gallery);
    let (_win, created) = ensure_preview_window(&app, &state, true)?;
    if !created {
        let _ = _win.emit("preview-gallery", ());
    }
    Ok(())
}

/// **预热**：先把那扇全局唯一的预览窗口建好、文档加载好，但**不显示、不抢焦点**。
///
/// 用户 2026-09-24："启动的时候后台异步把这些不需要销毁、高频使用的新窗口的 WebView 开销
/// 开好，到用的时候就能秒开"。第一次点图要付的成本几乎全在"建 WebView + 载入文档"上，
/// 预热把它挪到空闲时段；之后每次点图走的都是 `show_existing_aux_window` 那条快路径。
///
/// 为什么**只**预热这一扇（其余各有各的理由，别顺手往这里加）：
/// - 设置 / 日志：按口径仍然是"关窗即销毁"（每次都要读最新的环境数据与日志尾部），
///   预热出来的窗口一关就被丢掉，白占一份 WebView；
/// - 群任务窗口：label 是 `todo-<groupId>`，**启动时还不知道是哪个群** ⇒ 预热没有确定性，
///   而它现在已经是常驻的（第一次付一次，之后秒开）。
///
/// 已经存在就直接返回（既不重复建、也绝不 show）—— 预热的正确性不依赖它跑几次。
///
/// ⚠️ 这里**不自己查窗口存在性**：`ensure_aux_window(reveal = false)` 的快路径就是
/// "已存在 ⇒ 返回它、什么都不做"。自己写一次 `get_webview_window` 判存在，正是本仓库
/// 那条护栏禁止的形状（并发下会开出第二个窗口），而且这里连必要性都没有。
#[cfg(desktop)]
#[tauri::command(async)]
pub fn prewarm_aux_windows(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    // 顺序无含义：两扇都是"建好就放着"，谁先谁后不影响结果。任何一扇失败都不影响另一扇，
    // 也不影响后续真正打开（那时的错误才是用户需要看到的）。
    let _ = ensure_preview_window(&app, &state, false);
    let _ = ensure_tasks_window(&app, &state, "", false);
    Ok(())
}

/// 预览窗口本体（打开与预热**共用这一份**构造，差别只在最后要不要显示出来）。
///
/// 为什么要共用：两条路各自写一份 builder 的话，"预热出来的窗口和真正打开的不一样"
/// 这种缺陷是查不出来的（尺寸/标题栏/背景色/装饰少一项，秒开就变成"开出来是另一个样子"）。
#[cfg(desktop)]
fn ensure_preview_window(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, Arc<AppState>>,
    reveal: bool,
) -> Result<(tauri::WebviewWindow, bool), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let bg = aux_window_background(state);
    let title = aux_window_title(state, "图片预览", "Image preview", None);
    // 设计尺寸 1120×820：用户要的是"比聊天界面大一圈"。`fit_aux_window` 会把它夹到
    // 「主窗口 − 两侧各 24px」，所以**永不比主窗口大**（那条几何不变式见 logs_tests），
    // 主窗口本身小的时候跟着缩 —— 这正是想要的，不去破不变式。
    let geo = aux_window_geometry(app, (1120.0, 820.0), (480.0, 360.0));
    let build_app = app.clone();
    ensure_aux_window(
        app,
        crate::WINDOW_PREVIEW,
        geo,
        AUX_PREVIEW_RESIDENT,
        reveal,
        move || {
            let win = WebviewWindowBuilder::new(
                &build_app,
                crate::WINDOW_PREVIEW,
                WebviewUrl::App("preview.html".into()),
            )
            .title(title)
            .devtools(AUX_DEVTOOLS)
            .inner_size(1120.0, 820.0)
            .min_inner_size(480.0, 360.0)
            .background_color(bg)
            .visible(false)
            .decorations(false)
            .resizable(true)
            .maximizable(false)
            .minimizable(true)
            .closable(true)
            .build()?;
            decorate_aux_window(&win, &build_app);
            if let Some(g) = geo {
                apply_aux_geometry(&win, g);
                restore_aux_window_size(&win, &g);
                recenter_aux_window(&win, &g);
            }
            Ok(win)
        },
    )
}

/// 预览窗口取当前该显示的相册（**不清**，理由见 [`AppState::preview_gallery`]）。
/// 不分桌面/移动：只是读一块内存。
#[tauri::command]
pub fn get_image_preview_gallery(
    state: tauri::State<'_, Arc<AppState>>,
) -> Option<crate::state::PreviewGallery> {
    state
        .preview_gallery
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 移动端桩：预览窗口是桌面概念（移动端仍是应用内全屏覆盖层）。
#[cfg(mobile)]
#[tauri::command]
pub fn open_image_preview(
    _app: tauri::AppHandle,
    _state: tauri::State<'_, Arc<AppState>>,
    _gallery: crate::state::PreviewGallery,
) -> Result<(), String> {
    Err("移动端图片预览用应用内覆盖层（没有独立窗口）".to_string())
}

/// 移动端桩：没有独立窗口，也就没有"预付 WebView 开销"这件事。
///
/// 与上面那条同口径返回 `Err`：前端的预热调用只在桌面上发起，走到这里就是调用点写错了，
/// 写错要被看见，而不是被一个假的 `Ok(())` 掩盖。
#[cfg(mobile)]
#[tauri::command]
pub fn prewarm_aux_windows(
    _app: tauri::AppHandle,
    _state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    Err("移动端没有独立窗口可预热".to_string())
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
#[tauri::command(async)]
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
        true, // reveal：真的要把这扇窗显示出来（不是预热）
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
/// 移动端桩：见 `open_log_window` 的说明。
#[cfg(mobile)]
#[tauri::command]
pub fn close_log_window(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

// ---------------- 测试 ----------------
include!("logs_tests.rs");
