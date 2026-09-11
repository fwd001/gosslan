//! macOS App Sandbox 的 **security-scoped bookmark**（安全作用域书签）读写封装。
//!
//! ## 为什么非它不可
//! 沙盒里用户在 `NSOpenPanel` 里选的目录，系统只把访问权授予**本次进程**。
//! 我们把路径字符串存进数据库，重启后路径还在、**权限没了** ——
//! `read_dir` 直接失败，现象是「共享目录列表变空 / 对方拉不到文件列表」，
//! 而且**没有任何报错弹窗**（最容易被误判成"网络问题"的一类故障）。
//!
//! 唯一解：把 NSOpenPanel 给的 URL 存成 security-scoped bookmark（含沙盒授权信息），
//! 下次启动 `URLByResolvingBookmarkData:` 解析。**解析本身就是隐式开始访问**
//! （Apple 文档：默认隐式开始，除非显式传 `NSURLBookmarkResolutionWithoutImplicitStartAccessing`），
//! 所以我们不再额外调 `startAccessingSecurityScopedResource`（那会让引用计数 +1 而我们不 stop）。
//!
//! ## 两级书签：安全作用域优先，普通书签兜底
//! `NSURLBookmarkCreationWithSecurityScope` **被拒**的场合是真实存在的：
//! 未沙盒的运行（`cargo test` / 开发模式）没有沙盒授权，沙盒构建若缺
//! `com.apple.security.files.bookmarks.app-scope` 权限同样会被拒（已一并补进
//! `entitlements.plist`）。所以这里的策略是**两级**：
//!
//! 1. 先试**安全作用域书签** —— 沙盒里唯一能跨重启保住权限的东西；
//! 2. 被拒就退回**普通书签** —— 不带沙盒授权，但至少能跟踪"目录被移动/重命名"，
//!    而且在未沙盒环境里它就是完整可用的；
//! 3. 两者都失败才退回"只存路径"（调用方 `share_dir::store` 处理）。
//!
//! 解析侧对称地先按安全作用域解、失败再按普通书签解 —— 因为**选项必须与创建时一致**。
//! 这条兜底让本机（未沙盒的测试二进制）也能真实验证 objc2 调用姿势，
//! 而不是让用例悄悄走进 `Err` 分支、什么都没断言。
#![cfg(target_os = "macos")]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use objc2::runtime::Bool;
use objc2_foundation::{
    NSData, NSString, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};

/// 解析结果。
pub struct Resolved {
    /// 书签里的路径。**可能与数据库里存的那条不同** —— 书签记的是"这个资源"，
    /// 用户在 Finder 里重命名/移动后，它依然指向正确位置（这正是书签比路径强的地方）。
    pub path: String,
    /// 系统认为书签已过期（资源被移动/重命名）时，我们已顺手用解析出来的 URL
    /// 续了一个**新书签**（base64）。`Some(_)` 就等价于"刚才那个是过期的"，
    /// 调用方应把它写回数据库，否则下次启动还要再续一次。
    /// （不再单独暴露 `stale` 布尔量：没有任何调用方需要它，留着只会变成死字段。）
    pub refreshed: Option<String>,
}

/// 生成书签：**安全作用域优先，普通书签兜底**（见模块头注释）。
pub fn create(path: &str) -> Result<String, String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    create_with(&url, true)
        .or_else(|_| create_with(&url, false))
        .map_err(|e| format!("书签创建失败：{e}"))
}

/// `scoped = true` ⇒ 带沙盒授权的安全作用域书签；`false` ⇒ 普通书签。
fn create_with(url: &NSURL, scoped: bool) -> Result<String, String> {
    let options = if scoped {
        NSURLBookmarkCreationOptions::WithSecurityScope
    } else {
        NSURLBookmarkCreationOptions::empty()
    };
    url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
        options, None, None,
    )
    .map(|data| STANDARD.encode(data.to_vec()))
    .map_err(|e| format!("创建{}书签失败：{e}", if scoped { "安全作用域" } else { "普通" }))
}

/// 解析书签（**解析即开始访问**，见模块头注释）。
///
/// 先按安全作用域解、失败再按普通书签解 —— **选项必须与创建时一致**，
/// 而数据库里存的只是 base64，不带"是哪一种"的标记（少一个字段就少一处不一致）。
pub fn resolve(b64: &str) -> Result<Resolved, String> {
    let bytes = STANDARD
        .decode(b64)
        .map_err(|e| format!("书签不是合法 base64：{e}"))?;
    let data = NSData::with_bytes(&bytes);
    let mut url = resolve_with(&data, true);
    if url.is_none() {
        url = resolve_with(&data, false);
    }
    let (url, stale) = url.ok_or_else(|| "书签已失效（目录被删除或权限被撤销）".to_string())?;

    let path = url
        .path()
        .ok_or_else(|| "书签解析出的 URL 没有文件路径".to_string())?
        .to_string();
    // 过期书签的官方续期方式：从**解析出来的 URL** 重新生成（而不是从旧路径重来）
    let refreshed = if stale { create(&path).ok() } else { None };
    Ok(Resolved { path, refreshed })
}

/// 返回 `(url, is_stale)`；解析不了就 `None`（由调用方决定要不要换一种选项再试）。
fn resolve_with(data: &NSData, scoped: bool) -> Option<(objc2::rc::Retained<NSURL>, bool)> {
    let options = if scoped {
        NSURLBookmarkResolutionOptions::WithSecurityScope
    } else {
        NSURLBookmarkResolutionOptions::empty()
    };
    let mut is_stale = Bool::new(false);
    // SAFETY: `is_stale` 是有效的栈上 `Bool` 指针（文档要求"有效指针或 null"）；
    // 其余参数的生命周期都覆盖本次调用。
    unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            data,
            options,
            None,
            &mut is_stale,
        )
    }
    .ok()
    .map(|url| (url, is_stale.as_bool()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }

    fn temp_dir_path() -> String {
        std::env::temp_dir()
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string()
    }

    /// 书签解析出来的是**规范化路径**（例如 macOS 上 `/var` 是符号链接，
    /// 解析结果会是 `/private/var/...`），所以比较前两边都要 canonicalize。
    fn same_dir(a: &str, b: &str) -> bool {
        let (ca, cb) = (
            std::path::Path::new(a).canonicalize(),
            std::path::Path::new(b).canonicalize(),
        );
        matches!((ca, cb), (Ok(x), Ok(y)) if x == y)
    }

    /// **普通书签必须真往返**（这一条在本机未沙盒的测试二进制里也真的跑得通，
    /// 因此它实测了 objc2 的调用姿势：选项位、指针参数、返回类型）。
    #[test]
    fn plain_bookmark_really_round_trips() {
        let dir = temp_dir_path();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&dir));
        let bookmark = create_with(&url, false).expect("普通书签在任何环境都该能创建");

        let data = NSData::with_bytes(&STANDARD.decode(&bookmark).unwrap());
        let (resolved, stale) = resolve_with(&data, false).expect("自己刚创建的书签必须能解析");
        assert!(
            same_dir(&resolved.path().unwrap().to_string(), &dir),
            "往返后必须指向同一个目录（解析结果是规范化路径）"
        );
        assert!(!stale, "刚创建的书签不该是过期的");
    }

    /// 公开入口 `create` → `resolve` 必须端到端可用：
    /// 安全作用域被拒时走普通书签兜底，**路径仍然要能拿回来**。
    #[test]
    fn create_then_resolve_returns_the_same_directory() {
        let dir = temp_dir_path();
        let bookmark = create(&dir).expect("至少普通书签该能创建");
        let resolved = resolve(&bookmark).expect("create 的产物必须能被 resolve 解析");
        assert!(same_dir(&resolved.path, &dir), "解析结果必须还是那个目录");
        assert!(
            resolved.refreshed.is_none(),
            "刚创建的书签不该需要续期（refreshed 只在过期时才有）"
        );
    }

    /// 坏输入必须是干净的 `Err`，**绝不能 panic**（它是从数据库读出来的、可能被外部改坏）。
    #[test]
    fn garbage_bookmark_is_an_error_not_a_panic() {
        assert!(resolve("这显然不是 base64!!").is_err(), "非法 base64 应报错");
        // 合法 base64 但内容不是书签
        assert!(
            resolve(&b64(b"just some random bytes, not a bookmark")).is_err(),
            "伪书签应报错"
        );
        assert!(resolve("").is_err(), "空书签应报错");
    }

    /// 目录消失后，解析必须报错而不是返回一个不存在的路径
    /// （这样 `share_dir::load` 才会清掉坏书签、退回数据库里的路径）。
    #[test]
    fn bookmark_to_a_deleted_directory_fails_cleanly() {
        let doomed = std::env::temp_dir().join("gosslan-bookmark-probe-dir");
        // 先确保不存在，再创建一个空目录并立刻删掉
        let _ = std::fs::remove_dir_all(&doomed);
        std::fs::create_dir_all(&doomed).unwrap();
        let bookmark = create(&doomed.to_string_lossy()).expect("至少普通书签该能创建");
        std::fs::remove_dir_all(&doomed).unwrap();

        match resolve(&bookmark) {
            // 若系统仍能解析（有些情况只是标 stale），路径必须不再存在 —— 也就是
            // **不能**让上层以为目录还好着
            Ok(resolved) => assert!(
                !std::path::Path::new(&resolved.path).is_dir(),
                "目录已删除，解析结果不能还当成可用目录：{}",
                resolved.path
            ),
            Err(_) => {}
        }
    }
}
