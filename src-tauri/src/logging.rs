//! 应用级运行日志系统。
//!
//! 目标：生产环境（release，Windows 无控制台）出问题时，用户能打开「运行日志」
//! 窗口把日志复制给开发者，据此定位问题。日志走「内存有界缓存 + 落盘文件」双通道，
//! 既不无限增长、也不因进程崩溃而丢光（文件是崩溃兜底，内存是 UI 快照）。
//!
//! # 打日志规范（重要，写功能时务必遵守）
//!
//! **原则：少而准，只记「可能出错」与「关键状态跃迁」，绝不把日志当 printf 调试。**
//!
//! 1. **该记的**（有排查价值）：
//!    - 关键状态跃迁：连接建立 / 断开、握手成功 / 失败、身份学到 / 密钥冲突、链路切换。
//!    - 可恢复的异常：解析失败、绑定失败、拨号失败、文件初始化失败 —— 必须带上
//!      **能定位的上下文**（peer id、地址、错误原因），否则这行日志等于白记。
//!    - 安全事件：验签拒绝、身份伪造、重放。
//! 2. **不该记的**：
//!    - 正常消息正文、密钥、会话密钥、文件名等**敏感内容**（日志会落盘 + 被复制出去）。
//!    - 高频循环里的每条 announce / heartbeat（会刷爆 buffer，淹没真问题）。
//!    - 纯 debug 用的临时 println 式输出。
//! 3. **级别怎么选**：
//!    - `error`：明确失败、且用户/开发者需要立即关注的（文件失败、绑定失败、加密失败）。
//!    - `warn`：异常但可恢复、或值得注意的安全事件（拨号未成功、拒绝未认证 Hello、
//!      配置地址无法解析）。
//!    - `info`：正常的关键状态跃迁（连接建立、握手补全、学到身份、Presence 发现新节点）。
//! 4. **target 怎么起名**：子系统名（`transport` / `lan` / `routed` / `mesh` / `friend` /
//!    `presence` / `link` / `tray` …），与旧 eprintln 的 `[tag]` 对应。
//!
//! # 存储与自动清理（不无限记、不爆内存）
//!
//! - **内存**：有界 ring buffer（`MAX_MEM_LOGS = 500` 条），超出丢最旧 —— UI 只读它。
//! - **文件**：追加写 `logs/<stem>.log`，单文件超过 `MAX_LOG_FILE_BYTES`（512 KB）即
//!   轮转为 `.old.log`（覆盖上一份旧文件），磁盘占用上界约 1 MB。
//! - 清理触发是**惰性**的（每次写时检查），不另起后台任务，避免额外线程与复杂度。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

/// 内存保留的日志条数上限。
const MAX_MEM_LOGS: usize = 500;
/// 单个日志文件大小上限（字节），超出即轮转。
const MAX_LOG_FILE_BYTES: u64 = 512 * 1024;

/// 日志级别。按严重度递增：Info < Warn < Error。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// 单条日志（序列化给前端）。
#[derive(Clone, Serialize, Debug)]
pub struct LogEntry {
    /// Unix 毫秒时间戳。
    pub ts: i64,
    /// "info" | "warn" | "error"
    pub level: String,
    /// 子系统名（transport / lan / routed / mesh / friend …）。
    pub target: String,
    pub message: String,
}

/// 应用级 logger：内存 ring buffer + 落盘文件，线程安全（`Arc<AppState>` 跨线程共享）。
pub struct Logger {
    entries: Mutex<VecDeque<LogEntry>>,
    dir: PathBuf,
    /// 日志文件主名（`gosslan` 或 `gosslan-1` 等，多实例隔离）。
    stem: String,
}

impl Logger {
    pub fn new(dir: PathBuf, stem: &str) -> Logger {
        Logger {
            entries: Mutex::new(VecDeque::with_capacity(MAX_MEM_LOGS)),
            dir,
            stem: stem.to_string(),
        }
    }

    pub fn info(&self, target: &str, message: impl Into<String>) {
        self.log(Level::Info, target, message.into());
    }
    pub fn warn(&self, target: &str, message: impl Into<String>) {
        self.log(Level::Warn, target, message.into());
    }
    pub fn error(&self, target: &str, message: impl Into<String>) {
        self.log(Level::Error, target, message.into());
    }

    /// 记录一条日志：进内存 buffer → debug 下打 stderr → 落盘。
    pub fn log(&self, level: Level, target: &str, message: String) {
        let ts = now_ms();

        // 1) 内存 ring buffer（有界，防无限增长）
        {
            let mut q = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back(LogEntry {
                ts,
                level: level.as_str().to_string(),
                target: target.to_string(),
                message: message.clone(),
            });
            while q.len() > MAX_MEM_LOGS {
                q.pop_front();
            }
        }

        // 2) debug 构建仍打到 stderr，保留开发期控制台可观测（release 无控制台，跳过）。
        #[cfg(debug_assertions)]
        eprintln!("[{}] [{}] {}", level.as_str(), target, message);

        // 2b) **Android：同时写 logcat**（用户实测"闪退、拿不到日志"的唯一可行动诊断路径）。
        //     release 包既不能 `run-as`（不可调试），Rust 的 stdout/stderr 也不进 logcat，
        //     所以这里直接调系统的 `log` 命令打一条 —— 于是
        //     `adb logcat -s gosslan` 就能看到我们的启动路标与 panic。
        //     开销控制：warn/error 一律打；info 只打 `boot` 通道（启动路标），其余 info 走文件。
        #[cfg(target_os = "android")]
        {
            let want = matches!(level, Level::Warn | Level::Error) || target == "boot";
            if want {
                use std::process::{Command, Stdio};
                let line = format!("[{}] [{}] {}", level.as_str(), target, message);
                // 不阻塞主流程：失败就算了（日志不能反过来拖垮应用）
                let _ = Command::new("log")
                    .args(["-t", "gosslan", &line])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        }

        // 3) 落盘（追加写，超限轮转）。失败静默：日志本身不能反过来拖垮主流程。
        self.append_file(ts, level, target, &message);
    }

    /// UI 快照：按时间正序（旧 → 新）返回全部内存日志。
    pub fn snapshot(&self) -> Vec<LogEntry> {
        let q = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        q.iter().cloned().collect()
    }

    /// 清空内存与落盘文件。
    pub fn clear(&self) {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let _ = std::fs::remove_file(self.current_path());
        let _ = std::fs::remove_file(self.old_path());
    }

    fn current_path(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.stem))
    }
    fn old_path(&self) -> PathBuf {
        self.dir.join(format!("{}.old.log", self.stem))
    }

    fn append_file(&self, ts: i64, level: Level, target: &str, message: &str) {
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.current_path();

        // 惰性轮转：当前文件超限时，把旧内容挪到 `.old.log`（覆盖上一份旧文件）。
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_LOG_FILE_BYTES {
                let _ = std::fs::remove_file(self.old_path());
                let _ = std::fs::rename(&path, self.old_path());
            }
        }

        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let line = format!("{} {} [{}] {}", format_utc(ts), level.as_str(), target, message);
            let _ = writeln!(f, "{line}");
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把 Unix 毫秒时间戳格式化为 UTC `YYYY-MM-DD HH:MM:SS`（落盘文件用，可读）。
///
/// 不引入 chrono：标准库没有本地时区，这里给 UTC 时间，配合毫秒戳足够定位。
fn format_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Howard Hinnant 的 civil_from_days：Unix 天数 → (年, 月, 日)，纯函数、无时区依赖。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_utc_epoch_and_known_dates() {
        // 1970-01-01 00:00:00 UTC
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        // 2026-09-11 13:20:54 UTC
        let ms = 1_789_132_854_000i64; // 2026-09-11T13:20:54Z
        assert_eq!(format_utc(ms), "2026-09-11 13:20:54");
        // 一个带毫秒的：秒以下被截断，不影响 HH:MM:SS
        assert_eq!(format_utc(999), "1970-01-01 00:00:00");
    }

    #[test]
    fn logger_memory_ring_buffer_is_bounded() {
        let dir = std::env::temp_dir().join(format!("gosslan-log-test-{}", std::process::id()));
        let logger = Logger::new(dir.clone(), "t");
        // 写超过上限：只保留最后 MAX_MEM_LOGS 条
        for i in 0..(MAX_MEM_LOGS as i64 + 100) {
            logger.info("test", format!("msg {i}"));
        }
        let snap = logger.snapshot();
        assert_eq!(snap.len(), MAX_MEM_LOGS);
        // 最旧的 100 条已被丢弃，第一条是 msg 100
        assert!(snap[0].message == "msg 100");
        assert!(snap.last().unwrap().message == format!("msg {}", MAX_MEM_LOGS + 99));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logger_clear_empties_memory() {
        let dir = std::env::temp_dir().join(format!("gosslan-log-clr-{}", std::process::id()));
        let logger = Logger::new(dir.clone(), "t");
        logger.error("test", "boom");
        assert_eq!(logger.snapshot().len(), 1);
        logger.clear();
        assert!(logger.snapshot().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logger_persists_to_file_and_rotates() {
        let dir = std::env::temp_dir().join(format!("gosslan-log-rot-{}", std::process::id()));
        let logger = Logger::new(dir.clone(), "t");
        // 写一条，确认落盘
        logger.info("test", "hello file");
        let cur = dir.join("t.log");
        let content = std::fs::read_to_string(&cur).unwrap();
        assert!(content.contains("hello file"));
        assert!(content.contains("[test]"));
        // 触发轮转：把文件撑到超限
        let big = "x".repeat(1024);
        for _ in 0..600 {
            logger.info("test", big.clone());
        }
        // 轮转后 old 文件应存在（或当前文件被重置为 < 上限）
        let old = dir.join("t.old.log");
        let cur_len = std::fs::metadata(&cur).map(|m| m.len()).unwrap_or(0);
        assert!(old.exists() || cur_len <= MAX_LOG_FILE_BYTES);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
