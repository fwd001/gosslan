//! **用户选定的目录**的持久化：路径 +（macOS）安全作用域书签。
//!
//! 目前有两处同构的需求（同一个坑踩两次，所以抽成一份）：
//! - 共享目录（`share_dir`）—— 对外提供文件列表；
//! - 文件接收目录（`downloads_dir`）—— 收到的文件写到这里。
//!
//! 两者都是"用户在目录选择器里挑的目录"，而沙盒里那次选择的授权**只属于本次进程**。
//! 所以数据库里对每个目录存两样东西：
//! - `<name>`：路径字符串（跨平台通用，非沙盒环境全靠它）；
//! - `<name>_bookmark`：**仅 macOS** 的 security-scoped bookmark（base64）——
//!   沙盒里它是重启后唯一还带权限的来源（见 `macos_bookmark.rs`）。
//!
//! 决策收在一个纯函数 [`pick`] 里，这样"谁优先、坏了怎么办"是**可测**的，
//! 而不是散落在启动流程的 `if let` 里。
use rusqlite::Connection;

use crate::db;

/// 一对数据库键：路径 + 它的书签。
#[derive(Clone, Copy)]
pub struct Keys {
    /// 路径字符串的键（跨平台）。
    pub path: &'static str,
    /// 书签的键（base64，仅 macOS 会写）。
    /// 非 macOS 平台这个键永远不会被读写 —— 显式标注，免得在那些目标上冒出 dead_code 警告。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub bookmark: &'static str,
}

/// 共享目录（对外提供文件列表）。
pub const SHARE: Keys = Keys {
    path: "share_dir",
    bookmark: "share_dir_bookmark",
};

/// 文件接收目录（收到的文件写到这里）。
pub const RECEIVE: Keys = Keys {
    path: "downloads_dir",
    bookmark: "downloads_dir_bookmark",
};

/// 启动期决策（**纯函数**）。
///
/// 规则只有两条，但顺序很关键：
/// 1. **书签优先**：沙盒里它是唯一重启后仍有效的东西；而且书签记的是"资源"本身，
///    用户在 Finder 里移动/重命名目录后，解析出来的路径**比数据库里那条更新**；
/// 2. 书签没有 / 解析失败 / 解析出空路径 ⇒ **退回数据库里的路径**。
///    绝不能因为书签坏了就让用户重新选一次 —— 未沙盒的构建（开发模式）本来就靠这条。
pub fn pick(
    stored: Option<String>,
    from_bookmark: Option<Result<String, String>>,
) -> Option<String> {
    if let Some(Ok(path)) = from_bookmark {
        if !path.is_empty() {
            return Some(path);
        }
    }
    stored
}

/// 启动时读该目录。macOS 上会顺带解析书签（**解析即开始访问**）、
/// 过期则续期写回、失效则清掉（免得每次启动都白试一次）。
pub fn load(conn: &Connection, keys: Keys) -> Option<String> {
    let stored = db::get_setting(conn, keys.path);

    #[cfg(target_os = "macos")]
    let from_bookmark = db::get_setting(conn, keys.bookmark).map(|b64| {
        match crate::macos_bookmark::resolve(&b64) {
            Ok(resolved) => {
                if let Some(new_bookmark) = &resolved.refreshed {
                    // 续期成功：写回，下次启动不用再续
                    let _ = db::set_setting(conn, keys.bookmark, new_bookmark);
                }
                Ok(resolved.path)
            }
            Err(_) => {
                // 失效/损坏：清掉。否则每次启动都拿一条坏书签去试
                let _ = db::delete_setting(conn, keys.bookmark);
                Err("目录书签已失效".to_string())
            }
        }
    });
    #[cfg(not(target_os = "macos"))]
    let from_bookmark: Option<Result<String, String>> = None;

    pick(stored, from_bookmark)
}

/// 用户选好目录后落库：先写路径，再（macOS）尽力写书签。
///
/// 书签建不出来**不能**让"设置目录"这个动作失败：未沙盒的构建会创建失败，
/// 而那时路径本来就能用。失败时清掉旧书签，避免下次启动拿一条与当前目录不匹配的书签
/// 去解析（那会把这个目录指回**上一个**目录）。
pub fn store(conn: &Connection, keys: Keys, path: &str) -> Result<(), String> {
    db::set_setting(conn, keys.path, path).map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    match crate::macos_bookmark::create(path) {
        Ok(bookmark) => {
            db::set_setting(conn, keys.bookmark, &bookmark).map_err(|e| e.to_string())?;
        }
        Err(_) => {
            let _ = db::delete_setting(conn, keys.bookmark);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn
    }

    /// 书签解析出的是**规范化路径**（macOS 上 `/var` 是符号链接 ⇒ `/private/var/...`），
    /// 所以涉及真实目录的断言要比 canonicalize 之后的结果。
    fn same_dir(a: Option<String>, b: &str) -> bool {
        let (Some(a), Ok(b)) = (a, std::path::Path::new(b).canonicalize()) else {
            return false;
        };
        matches!(std::path::Path::new(&a).canonicalize(), Ok(x) if x == b)
    }

    fn ok(path: &str) -> Option<Result<String, String>> {
        Some(Ok(path.to_string()))
    }

    fn err() -> Option<Result<String, String>> {
        Some(Err("书签已失效".to_string()))
    }

    /// 书签优先 —— 且**必须**优先，因为沙盒里只有它带权限。
    #[test]
    fn bookmark_wins_over_the_stored_path() {
        assert_eq!(
            pick(Some("/old/path".to_string()), ok("/moved/here")),
            Some("/moved/here".to_string()),
            "书签解析出的路径更新，必须盖过数据库里那条"
        );
    }

    /// 书签坏了要退回路径，而不是把共享目录清空（否则用户会莫名其妙要重选一次）。
    #[test]
    fn broken_bookmark_falls_back_to_the_stored_path() {
        assert_eq!(
            pick(Some("/stored".to_string()), err()),
            Some("/stored".to_string())
        );
        assert_eq!(pick(Some("/stored".to_string()), None), Some("/stored".to_string()));
    }

    /// 书签解析出空路径等同于失败（不能把空串当目录）。
    #[test]
    fn empty_resolved_path_is_not_accepted() {
        assert_eq!(
            pick(Some("/stored".to_string()), ok("")),
            Some("/stored".to_string()),
            "空路径必须被拒绝并退回"
        );
        assert_eq!(pick(None, ok("")), None);
    }

    /// 都没配就是没配（设置页据此显示"未设置"）。
    #[test]
    fn nothing_configured_stays_none() {
        assert_eq!(pick(None, None), None);
        assert_eq!(pick(None, err()), None);
    }

    /// 端到端：`store` 之后 `load` 必须拿到同一个目录 ——
    /// **无论沙盒允不允许创建书签**（不允许就走"只有路径"的降级路）。
    /// 这条守着最容易犯的错：书签建失败把整个设置也一起丢了。
    #[test]
    fn store_then_load_returns_the_same_directory_in_both_modes() {
        let conn = mem();
        let dir = std::env::temp_dir();
        let dir = dir.to_string_lossy().trim_end_matches('/').to_string();

        store(&conn, SHARE, &dir).expect("设置共享目录不该失败");
        assert_eq!(
            db::get_setting(&conn, SHARE.path).as_deref(),
            Some(dir.as_str()),
            "路径必须无条件落库（书签只是附加信息）"
        );
        assert!(same_dir(load(&conn, SHARE), &dir), "load 必须拿回同一个目录");
    }

    /// 两个目录（共享 / 接收）必须**各存各的**：键写串了会让"改接收目录"
    /// 顺手把共享目录指到别处 —— 这类串键错误在单目录测试里完全看不出来。
    #[test]
    fn the_two_user_directories_do_not_interfere() {
        let conn = mem();
        let a = std::env::temp_dir();
        let a = a.to_string_lossy().trim_end_matches('/').to_string();
        let b = format!("{a}/gosslan-user-dirs-test");
        std::fs::create_dir_all(&b).unwrap();

        store(&conn, SHARE, &a).unwrap();
        store(&conn, RECEIVE, &b).unwrap();

        assert!(same_dir(load(&conn, SHARE), &a), "共享目录不能被接收目录覆盖");
        assert!(same_dir(load(&conn, RECEIVE), &b), "接收目录必须各自独立");
        assert_ne!(SHARE.path, RECEIVE.path, "两处路径键必须不同");
        assert_ne!(SHARE.bookmark, RECEIVE.bookmark, "两处书签键必须不同");
        let _ = std::fs::remove_dir_all(&b);
    }

    /// 坏书签不能让启动拿不到目录：清了书签、退回路径。
    #[test]
    fn corrupted_bookmark_is_cleared_and_path_still_used() {
        let conn = mem();
        db::set_setting(&conn, SHARE.path, "/stored/dir").unwrap();
        db::set_setting(&conn, SHARE.bookmark, "这不是书签!!").unwrap();

        assert_eq!(load(&conn, SHARE), Some("/stored/dir".to_string()));
        #[cfg(target_os = "macos")]
        assert_eq!(
            db::get_setting(&conn, SHARE.bookmark),
            None,
            "坏书签必须被清掉，免得每次启动都白试"
        );
    }

    /// 书签与路径不匹配时以书签为准（模拟"用户在 Finder 里移动了目录"）。
    #[cfg(target_os = "macos")]
    #[test]
    fn moved_directory_is_followed_through_the_bookmark() {
        let conn = mem();
        let dir = std::env::temp_dir();
        let dir = dir.to_string_lossy().trim_end_matches('/').to_string();
        if let Ok(bookmark) = crate::macos_bookmark::create(&dir) {
            db::set_setting(&conn, SHARE.path, "/somewhere/else").unwrap();
            db::set_setting(&conn, SHARE.bookmark, &bookmark).unwrap();
            assert!(same_dir(load(&conn, SHARE), &dir), "书签指向的目录才是真目录");
        }
    }
}
