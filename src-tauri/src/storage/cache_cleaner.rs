//! 自动缓存清理与 SQLite 整理服务。
//!
//! - **保留时长**：`retention_days`（3 / 7 / 30 天，`None` = 永久）。
//! - **磁盘配额**：`max_bytes`（超过后按「最旧优先」删除，`None` = 不限制）。
//! - 清理后对 SQLite 执行 `VACUUM`，回收被删除消息 / 会话占用的碎片。
//!
//! ⚠️ 注意「清理对象」是**落盘的二进制媒体**（接收的图片 / 文件），
//! 不是聊天文字。历史遗留的 `cache/` 目录已无写入方（P1 重构后媒体改落 downloads），
//! 但仍一并纳入统计与清理，避免早期版本残留在里面既看不见也清不掉。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// 缓存清理策略（Telegram 风格）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachePolicy {
    /// 保留时长（天）。`None` 表示永久保留。
    pub retention_days: Option<u32>,
    /// 磁盘占用上限（字节）。`None` 表示不限制。
    pub max_bytes: Option<u64>,
}

/// 单个缓存文件条目（用于清理决策）。
#[derive(Clone, Copy, Debug)]
pub struct CacheEntry {
    pub mtime_ms: i64,
    pub size: u64,
}

/// 清理结果。
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct CleanupReport {
    pub removed: usize,
    pub freed_bytes: u64,
}

/// 计算需要删除的条目索引（纯函数，便于测试）。
///
/// 策略：
/// 1. 先删除超过保留时长的过期文件；
/// 2. 若仍超过磁盘配额，按「最旧优先」继续删除，直到总大小低于配额。
pub fn plan_removal(
    entries: &[(String, CacheEntry)],
    policy: CachePolicy,
    now_ms: i64,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| entries[i].1.mtime_ms); // 最旧优先

    let mut total: u64 = entries.iter().map(|e| e.1.size).sum();
    let mut to_remove: Vec<usize> = Vec::new();
    // 配额循环里判「已删过」用标记数组而不是 `contains`：大缓存（数万文件）下
    // Vec 线性扫描把配额循环拖成 O(n²)，清理一次卡住全局 db 锁秒级以上。
    let mut marked = vec![false; entries.len()];

    if let Some(days) = policy.retention_days {
        let cutoff = now_ms - (days as i64).saturating_mul(86_400_000);
        for &i in &order {
            if entries[i].1.mtime_ms < cutoff {
                marked[i] = true;
                to_remove.push(i);
                total = total.saturating_sub(entries[i].1.size);
            }
        }
    }

    if let Some(max) = policy.max_bytes {
        for &i in &order {
            if total <= max {
                break;
            }
            if !marked[i] {
                marked[i] = true;
                to_remove.push(i);
                total = total.saturating_sub(entries[i].1.size);
            }
        }
    }

    to_remove
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 递归列出目录下的所有文件（深度栈遍历，不跟随符号链接）。
///
/// 为什么必须递归（2026-09-19 审计）：媒体按 `todo-paste/`、按日子目录等分层落盘，
/// 旧的单层 read_dir 让这些子树**既不被统计也不被清理** —— 配额判的是假数，
/// 「30 天后自动清」对子目录文件形同虚设。
///
/// ⚠️ 符号链接一律跳过（`DirEntry::file_type()` 不跟随链接，与 `path.metadata()`
/// 相反）：缓存目录是**远端输入可达面**（收到的文件名/目录名不受信），一个指向
/// 任意位置的软链若被跟随，其目标文件会被收集进清理列表并 `remove_file` 永久删除
/// —— 那是数据丢失，不是清理（2026-09-23 审计 1.1）。
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                out.push(e.path());
            }
            // 软链 / 套接字 / 其他特殊文件：既不收集也不递归
        }
    }
    out
}

/// 自动调度用：只删文件、**不 VACUUM**。
///
/// VACUUM 的纪律（2026-09-23 审计 2.1a/2.1b）：
/// - 它要独占连接、可能秒~分钟级，**绝不能与文件遍历/删除共用一次锁持有**——
///   旧接口 `clean(dirs, policy, &dbc)` 就是这个形状：调用方持全局 db 锁期间
///   递归删文件 + VACUUM，全 App 的消息落库/发送一起排队等锁。
/// - 是否值得跑由调用方按 `removed > 0` 决定（没删文件空转一次 = 白冻全 App）。
pub fn clean_files(dirs: &[PathBuf], policy: CachePolicy) -> CleanupReport {
    clean_inner(dirs, policy)
}

fn clean_inner(dirs: &[PathBuf], policy: CachePolicy) -> CleanupReport {
    let now = now_ms();
    let mut entries: Vec<(String, CacheEntry)> = Vec::new();
    for dir in dirs {
        for path in walk_files(dir) {
            let Ok(meta) = path.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            entries.push((
                path.to_string_lossy().to_string(),
                CacheEntry {
                    mtime_ms: mtime,
                    size: meta.len(),
                },
            ));
        }
    }

    let mut report = CleanupReport::default();
    for idx in plan_removal(&entries, policy, now) {
        if std::fs::remove_file(&entries[idx].0).is_ok() {
            report.removed += 1;
            report.freed_bytes += entries[idx].1.size;
        }
    }

    report
}

/// 计算若干目录的合计占用（文件数 + 总字节数），用于存储管理页展示。
pub fn usage(dirs: &[PathBuf]) -> (usize, u64) {
    let mut count = 0usize;
    let mut bytes = 0u64;
    for dir in dirs {
        for path in walk_files(dir) {
            if let Ok(meta) = path.metadata() {
                count += 1;
                bytes += meta.len();
            }
        }
    }
    (count, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(mtime_ms: i64, size: u64) -> (String, CacheEntry) {
        (format!("f{mtime_ms}"), CacheEntry { mtime_ms, size })
    }

    #[test]
    fn retention_removes_expired_only() {
        let now = 1_000_000_000_000i64;
        let entries = vec![
            e(now - 10 * 86_400_000, 100), // 10 天前 → 过期（7 天保留）
            e(now - 1 * 86_400_000, 100),  // 1 天前 → 未过期
        ];
        let policy = CachePolicy {
            retention_days: Some(7),
            max_bytes: None,
        };
        let plan = plan_removal(&entries, policy, now);
        assert_eq!(plan, vec![0]);
    }

    #[test]
    fn quota_removes_oldest_first() {
        let now = 1_000_000_000_000i64;
        let entries = vec![
            e(now, 60),        // 最新
            e(now - 1000, 30), // 中
            e(now - 2000, 30), // 最旧
        ];
        // 总 120，配额 80 → 需删 40：先删最旧的 30，再删中间的 30（删 30 后 90>80，继续删）
        let policy = CachePolicy {
            retention_days: None,
            max_bytes: Some(80),
        };
        let plan = plan_removal(&entries, policy, now);
        assert_eq!(plan, vec![2, 1]);
    }

    #[test]
    fn no_policy_removes_nothing() {
        let now = 1_000_000_000_000i64;
        let entries = vec![e(now - 1000, 100)];
        let policy = CachePolicy {
            retention_days: None,
            max_bytes: None,
        };
        assert!(plan_removal(&entries, policy, now).is_empty());
    }

    /// 回归（2026-09-23 审计 1.1）：缓存目录里的符号链接绝不能被跟随 ——
    /// 跟随 = 软链目标位置的文件被当作"缓存"统计并删除（任意位置的数据丢失）。
    /// walk_files 必须既不收集软链本身，也不递归进软链指向的目录。
    #[cfg(unix)] // Windows 建符号链接需要特权
    #[test]
    fn walk_files_never_follows_symlinks() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "gosslan-symlink-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let cache = base.join("cache");
        let outside = base.join("outside");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // 软链目标：缓存目录之外的一个真实文件（绝不能被删）
        let precious = base.join("precious.txt");
        std::fs::write(&precious, b"do-not-delete").unwrap();
        // 外部目录里的文件（通过目录软链可达，同样绝不能被收集）
        let deep = outside.join("deep.txt");
        std::fs::write(&deep, b"also-precious").unwrap();

        symlink(&precious, cache.join("link-to-file")).unwrap();
        symlink(&outside, cache.join("link-to-dir")).unwrap();
        std::fs::write(cache.join("real.txt"), b"cache-file").unwrap();

        let found = walk_files(&cache);
        assert_eq!(
            found,
            vec![cache.join("real.txt")],
            "软链（文件/目录）都不得进入清理列表"
        );

        // 极端配额（0 字节）也只能删缓存内的真实文件，软链目标毫发无损
        let policy = CachePolicy {
            retention_days: None,
            max_bytes: Some(0),
        };
        let report = clean_files(&[cache.clone()], policy);
        assert_eq!(report.removed, 1, "只应删掉 real.txt 一个文件");
        assert!(precious.exists(), "软链指向的外部文件必须幸存");
        assert!(deep.exists(), "目录软链下的文件必须幸存");

        let _ = std::fs::remove_dir_all(&base);
    }
}
