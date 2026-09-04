//! 自动缓存清理与 SQLite 整理服务。
//!
//! - **保留时长**：`retention_days`（3 / 7 / 30 天，`None` = 永久）。
//! - **磁盘配额**：`max_bytes`（超过后按「最旧优先」删除，`None` = 不限制）。
//! - 清理后对 SQLite 执行 `VACUUM`，回收被删除消息 / 会话占用的碎片。

use std::path::Path;
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
pub fn plan_removal(entries: &[(String, CacheEntry)], policy: CachePolicy, now_ms: i64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| entries[i].1.mtime_ms); // 最旧优先

    let mut total: u64 = entries.iter().map(|e| e.1.size).sum();
    let mut to_remove: Vec<usize> = Vec::new();

    if let Some(days) = policy.retention_days {
        let cutoff = now_ms - (days as i64).saturating_mul(86_400_000);
        for &i in &order {
            if entries[i].1.mtime_ms < cutoff {
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
            if !to_remove.contains(&i) {
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

/// 执行一次缓存清理：按策略删除过期 / 超配额文件，并对数据库执行 `VACUUM`。
pub fn clean(cache_dir: &Path, policy: CachePolicy, db: &rusqlite::Connection) -> CleanupReport {
    let now = now_ms();
    let mut entries: Vec<(String, CacheEntry)> = Vec::new();

    if let Ok(rd) = std::fs::read_dir(cache_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let size = meta.len();
            entries.push((p.to_string_lossy().to_string(), CacheEntry { mtime_ms: mtime, size }));
        }
    }

    let mut report = CleanupReport::default();
    for idx in plan_removal(&entries, policy, now) {
        if std::fs::remove_file(&entries[idx].0).is_ok() {
            report.removed += 1;
            report.freed_bytes += entries[idx].1.size;
        }
    }

    // 整理 SQLite 碎片（忽略失败：内存库 / 只读等情况）
    let _ = db.execute_batch("VACUUM");

    report
}

/// 计算缓存目录的当前占用（文件数 + 总字节数），用于存储管理页展示。
pub fn usage(cache_dir: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(cache_dir) {
        for e in rd.flatten() {
            if let Ok(meta) = e.metadata() {
                if meta.is_file() {
                    count += 1;
                    bytes += meta.len();
                }
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
        let policy = CachePolicy { retention_days: Some(7), max_bytes: None };
        let plan = plan_removal(&entries, policy, now);
        assert_eq!(plan, vec![0]);
    }

    #[test]
    fn quota_removes_oldest_first() {
        let now = 1_000_000_000_000i64;
        let entries = vec![
            e(now, 60),         // 最新
            e(now - 1000, 30),  // 中
            e(now - 2000, 30),  // 最旧
        ];
        // 总 120，配额 80 → 需删 40：先删最旧的 30，再删中间的 30（删 30 后 90>80，继续删）
        let policy = CachePolicy { retention_days: None, max_bytes: Some(80) };
        let plan = plan_removal(&entries, policy, now);
        assert_eq!(plan, vec![2, 1]);
    }

    #[test]
    fn no_policy_removes_nothing() {
        let now = 1_000_000_000_000i64;
        let entries = vec![e(now - 1000, 100)];
        let policy = CachePolicy { retention_days: None, max_bytes: None };
        assert!(plan_removal(&entries, policy, now).is_empty());
    }
}
