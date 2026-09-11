//! macOS 原生菜单栏。
//!
//! 为什么需要：本应用为了自绘标题栏用了 `decorations: false`，于是**没有系统菜单栏**。
//! 而在 macOS 上菜单栏是基础体验的一部分（HIG *The menu bar*）：⌘Q 退出、⌘, 偏好设置、
//! ⌘W 关闭窗口、⌘M 最小化、以及标准的「编辑」菜单（撤销/剪切/复制/粘贴/全选），
//! 用户都会下意识地去按、去菜单里找。没有它，应用会显得"不像 Mac 应用"。
//!
//! 为什么只在 macOS 建：Windows / Linux 用的是自绘标题栏（无边框），
//! 在这些平台上加一条系统菜单条会顶在自绘标题栏之上，破坏已经调好的窗口布局。
//!
//! 失败处理：菜单属于"锦上添花"，**初始化失败不得阻断启动**（与托盘同一策略），
//! 只打印日志。快捷键另有前端兜底，不会因此完全失效。
//!
//! ## 文案为什么由前端推（而不是后端自己判断系统语言）
//! 「跟随系统」的三态语言偏好已经在 `src/i18n/index.ts` 里实现（含 `navigator.languages`
//! 解析与回落规则）。后端再实现一遍检测就会有**第二份真相**：中文系统 + 用户显式选英文时，
//! 菜单和界面会不一致。所以启动时先用**持久化的显式偏好**建一次菜单（避免英文用户看到中文），
//! 随后由前端在启动完成、以及每次切换语言时调用 `set_ui_language` 推送解析后的 locale 重建。
//! 这属于启动期的一次轻量调用，不阻塞渲染（前端 fire-and-forget）。

use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder},
    AppHandle, Emitter,
};

/// 菜单事件名（前端 `api/index.ts` 监听后转成 window 事件）。
const MENU_SETTINGS: &str = "menu://settings";
const MENU_ADD_FRIEND: &str = "menu://add-friend";
const MENU_SEARCH: &str = "menu://search";
const MENU_LOGS: &str = "menu://logs";

/// 菜单栏语言。只区分中/英，与本项目 `Locale` 的两种取值对应。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiLang {
    Zh,
    En,
}

impl UiLang {
    /// 解析前端推来的标签。前端推的是解析后的 locale（`zh-CN` / `en-US`），
    /// 这里同时容忍 `zh` / `en` 短标签；**其余一律英文**，与 i18n 的回落规则一致
    /// （非中英系统回落英文）。
    pub fn parse(tag: &str) -> Self {
        let t = tag.trim().to_ascii_lowercase();
        if t.starts_with("zh") {
            Self::Zh
        } else {
            Self::En
        }
    }
}

/// 一处集中管理的中英文案：加项时编译器会强制补齐另一种语言（结构体字面量必须全字段）。
struct Labels {
    edit: &'static str,
    session: &'static str,
    window: &'static str,
    help: &'static str,
    settings: &'static str,
    add_friend: &'static str,
    search: &'static str,
    logs: &'static str,
}

fn labels(lang: UiLang) -> Labels {
    match lang {
        UiLang::Zh => Labels {
            edit: "编辑",
            session: "会话",
            window: "窗口",
            help: "帮助",
            settings: "偏好设置…",
            add_friend: "添加好友…",
            search: "搜索",
            logs: "打开日志窗口",
        },
        UiLang::En => Labels {
            edit: "Edit",
            session: "Session",
            window: "Window",
            help: "Help",
            settings: "Settings…",
            add_friend: "Add Friend…",
            search: "Search",
            logs: "Open Log Window",
        },
    }
}

/// 已经从持久化偏好推出过一次的语言（仅初始值，供 `initial_lang` 使用）。
/// 用 `Mutex<Option<..>>` 而不是 `OnceLock`：语言可以来回切换，需要记录"当前生效值"。
static APPLIED: Mutex<Option<UiLang>> = Mutex::new(None);

/// 启动时的初始菜单语言：只认**显式**偏好（`zh-CN` / `en-US`）。
///
/// "跟随系统"（以及首次安装没有该键）在这里**故意当作中文**：应用是中文优先的，
/// 而前端会在启动流程里立刻推一次真实 locale 覆盖它 —— 那时窗口还没显示，
/// 用户看不到这一瞬间的中间态。反过来若先按英文建、中文用户闪一下英文菜单，
/// 代价更大。
pub fn initial_lang(persisted: Option<&str>) -> UiLang {
    match persisted {
        Some("en-US") => UiLang::En,
        Some("zh-CN") => UiLang::Zh,
        _ => UiLang::Zh,
    }
}

fn build(app: &AppHandle, lang: UiLang) -> tauri::Result<Menu<tauri::Wry>> {
    let l = labels(lang);

    // ---- 应用菜单（macOS 上第一个子菜单自动成为应用菜单）----
    let app_menu = SubmenuBuilder::new(app, "Gosslan")
        .item(&PredefinedMenuItem::about(app, None, None)?)
        .separator()
        .item(&MenuItem::with_id(
            app,
            "settings",
            l.settings,
            true,
            Some("CmdOrCtrl+,"),
        )?)
        .separator()
        .item(&PredefinedMenuItem::services(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::hide(app, None)?)
        .item(&PredefinedMenuItem::hide_others(app, None)?)
        .item(&PredefinedMenuItem::show_all(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, None)?)
        .build()?;

    // ---- 编辑：标准项（转发给 WebView 的第一响应者）----
    let edit_menu = SubmenuBuilder::new(app, l.edit)
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;

    // ---- 会话：本应用自己的动作 ----
    let session_menu = SubmenuBuilder::new(app, l.session)
        .item(&MenuItem::with_id(
            app,
            "add-friend",
            l.add_friend,
            true,
            Some("CmdOrCtrl+N"),
        )?)
        .item(&MenuItem::with_id(
            app,
            "search",
            l.search,
            true,
            Some("CmdOrCtrl+F"),
        )?)
        .build()?;

    // ---- 窗口：交给系统的标准项（⌘W / ⌘M / 全屏等）----
    // set_as_windows_menu_for_nsapp 让这个子菜单被识别为「窗口」菜单，
    // 系统会自动往里补窗口列表、并接管全屏/缩放等标准行为。
    let window_menu = SubmenuBuilder::new(app, l.window)
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::bring_all_to_front(app, None)?)
        .build()?;
    window_menu.set_as_windows_menu_for_nsapp()?;

    // ---- 帮助（HIG *The menu bar* 要求有；macOS 会自带一个搜索框）----
    // 本项目没有在线文档站，所以不放"打开网页"这类假帮助项 —— 放真正能解决问题的
    // 「打开日志窗口」（排查连不上/收不到消息时的第一手信息，与设置页同一入口）。
    let help_menu = SubmenuBuilder::new(app, l.help)
        .item(&MenuItem::with_id(app, "logs", l.logs, true, None::<&str>)?)
        .build()?;

    MenuBuilder::new(app)
        .item(&app_menu)
        .item(&edit_menu)
        .item(&session_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()
}

/// 建菜单栏并挂上事件监听（**只调用一次**：重复调用会重复注册事件处理器）。
pub fn setup(app: &AppHandle, lang: UiLang) -> tauri::Result<()> {
    // 自定义项 → 发事件给前端（统一由前端执行动作，避免菜单与快捷键两条路径行为不一致）
    let handle = app.clone();
    app.on_menu_event(move |_app: &AppHandle, event| match event.id().as_ref() {
        "settings" => {
            let _ = handle.emit(MENU_SETTINGS, ());
        }
        "add-friend" => {
            let _ = handle.emit(MENU_ADD_FRIEND, ());
        }
        "search" => {
            let _ = handle.emit(MENU_SEARCH, ());
        }
        "logs" => {
            let _ = handle.emit(MENU_LOGS, ());
        }
        _ => {}
    });

    apply(app, lang)
}

/// 按语言重建菜单栏。**同一语言重复调用直接返回**：语言切换在设置页里是点击即触发，
/// 不必每次都做一次主线程的菜单重建（响应性优先）。
pub fn apply(app: &AppHandle, lang: UiLang) -> tauri::Result<()> {
    {
        // 锁中毒不影响正确性（只是记一个枚举值），用 into_inner 兜底而不是 unwrap panic。
        let mut cur = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
        if *cur == Some(lang) {
            return Ok(());
        }
        *cur = Some(lang);
    }
    let menu = build(app, lang)?;
    app.set_menu(menu)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_locale_tags() {
        assert_eq!(UiLang::parse("zh-CN"), UiLang::Zh);
        assert_eq!(UiLang::parse("zh"), UiLang::Zh);
        assert_eq!(UiLang::parse(" ZH-hans "), UiLang::Zh);
        assert_eq!(UiLang::parse("en-US"), UiLang::En);
        assert_eq!(UiLang::parse("en"), UiLang::En);
        // 非中英系统回落英文（与 i18n detectSystemLocale 的规则一致）
        assert_eq!(UiLang::parse("ja-JP"), UiLang::En);
        assert_eq!(UiLang::parse(""), UiLang::En);
    }

    #[test]
    fn initial_lang_uses_only_explicit_preference() {
        // 只认显式偏好；"system" / 未设置 / 脏值一律按中文（中文优先，且随后会被前端纠正）
        assert_eq!(initial_lang(Some("en-US")), UiLang::En);
        assert_eq!(initial_lang(Some("zh-CN")), UiLang::Zh);
        assert_eq!(initial_lang(Some("system")), UiLang::Zh);
        assert_eq!(initial_lang(None), UiLang::Zh);
    }

    #[test]
    fn every_label_is_present_in_both_languages() {
        // 非空 + 两种语言不相同：防止"加了字段却漏翻"或复制粘贴成同一串
        for pair in [
            (labels(UiLang::Zh).edit, labels(UiLang::En).edit),
            (labels(UiLang::Zh).session, labels(UiLang::En).session),
            (labels(UiLang::Zh).window, labels(UiLang::En).window),
            (labels(UiLang::Zh).help, labels(UiLang::En).help),
            (labels(UiLang::Zh).settings, labels(UiLang::En).settings),
            (labels(UiLang::Zh).add_friend, labels(UiLang::En).add_friend),
            (labels(UiLang::Zh).search, labels(UiLang::En).search),
            (labels(UiLang::Zh).logs, labels(UiLang::En).logs),
        ] {
            assert!(!pair.0.trim().is_empty(), "中文文案为空: {pair:?}");
            assert!(!pair.1.trim().is_empty(), "英文文案为空: {pair:?}");
            assert_ne!(pair.0, pair.1, "中英文案相同（漏翻？）: {pair:?}");
        }
    }
}
