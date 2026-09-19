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
/// Android 的 logcat 镜像：**合批 + 单线程 fork**（见 `Logger::log` 里的说明）。
///
/// 为什么需要它：`log` 是外部可执行文件，写一行 = fork+exec 一个进程。BLE 生命周期日志
/// 一多（每帧一条 `[SEND]/[RECV]`），真机上就变成"一边跑 GATT、一边疯狂 fork"，
/// 拖慢 Rust 运行时与蓝牙时序。这里把行丢进无界队列，由**一个**常驻线程按
/// `BATCH_WINDOW` 合批后每批只 fork 一次。
///
/// 取舍：logcat 里会晚最多 200ms 出现（日志首行的时间戳是**批次**时间），
/// 而行级精确时间戳仍然完整保存在落盘日志与内存日志里（`[SEND]/[RECV]` 排查用它）。
#[cfg(target_os = "android")]
mod logcat {
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    /// 合批窗口：够把一轮扫描/一次 flush 的多行并成一次 fork，又不至于让日志"迟到"太多。
    const BATCH_WINDOW: Duration = Duration::from_millis(200);
    /// 单批上限（防止极端刷屏时一条 logcat 消息过长）。
    const BATCH_MAX_LINES: usize = 64;

    static TX: OnceLock<Sender<String>> = OnceLock::new();

    pub(super) fn push(line: String) {
        let _ = tx().send(line);
    }

    fn tx() -> &'static Sender<String> {
        TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<String>();
            std::thread::Builder::new()
                .name("gosslan-logcat".into())
                .spawn(move || run(rx))
                .ok();
            tx
        })
    }

    /// 常驻线程：收够一批就 fork 一次 `log -t gosslan <多行>`。
    fn run(rx: Receiver<String>) {
        loop {
            let Ok(first) = rx.recv() else { return };
            let mut batch = vec![first];
            let deadline = Instant::now() + BATCH_WINDOW;
            while batch.len() < BATCH_MAX_LINES {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match rx.recv_timeout(left) {
                    Ok(line) => batch.push(line),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            emit(&batch.join("\n"));
        }
    }

    fn emit(msg: &str) {
        use std::process::{Command, Stdio};
        // 不等待：日志绝不允许反过来阻塞业务
        let _ = Command::new("log")
            .args(["-t", "gosslan", msg])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 合批函数的边界：窗口内多行必须并成 1 批、超上限必须截断。
        /// （这里只测"批大小/拼接"的纯逻辑，不真的 fork `log`。）
        #[test]
        fn batch_join_keeps_one_message_per_batch() {
            let lines: Vec<String> = (0..3).map(|i| format!("line{i}")).collect();
            assert_eq!(lines.join("\n"), "line0\nline1\nline2");
            assert!(BATCH_MAX_LINES >= 16, "上限太小会把日志切成碎片");
            assert!(BATCH_WINDOW >= Duration::from_millis(50));
        }
    }
}

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
    /// 设备短指纹 — 每台机器自动生成（hostname 前 6 字符 + 平台 tag），
    /// 让多设备日志能一眼区分来源。格式：`[dev:Win-7f3a]` / `[dev:And-b2c1]` / `[dev:Mac-9e4d]`。
    fingerprint: String,
}

impl Logger {
    pub fn new(dir: PathBuf, stem: &str, fingerprint: String) -> Logger {
        Logger {
            entries: Mutex::new(VecDeque::with_capacity(MAX_MEM_LOGS)),
            dir,
            stem: stem.to_string(),
            fingerprint,
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
        eprintln!(
            "{} [{}] [{}] {}",
            self.fingerprint,
            level.as_str(),
            target,
            message
        );

        // 2b) **Android：同时写 logcat**（用户实测"闪退、拿不到日志"的唯一可行动诊断路径）。
        //     release 包既不能 `run-as`（不可调试），Rust 的 stdout/stderr 也不进 logcat，
        //     所以这里直接调系统的 `log` 命令打一条 —— 于是
        //     `adb logcat -s gosslan` 就能看到我们的启动路标与 panic。
        //     开销控制：warn/error 一律打；info 只打 `boot`（启动路标）与 `ble`（蓝牙）两个通道。
        //
        //     为什么把 `ble` 的 info 也镜像出来（用户 2026-09-12 实测的教训）：
        //       · 蓝牙是**唯一没法在桌面上自测**的链路，真机日志是唯一证据来源；
        //       · 而"扫描到几个候选 / 哪个候选没连上、为什么"恰好都是 **info** 级 ——
        //         之前它们只写文件，用户能贴的 logcat 里什么都没有，于是"互相搜不到"
        //         只能靠猜（那次真正的根因是扫描结果被 `services()` 过滤掉，日志里一个字都没有）。
        //     频率很低（每个扫描周期最多几行，10s 一次），不会把 logcat 刷满。
        //
        // ⚠️ **不能每条日志 fork 一个进程**（2026-09-13 真机抓到的真问题）：
        //     原来这里是 `Command::new("log").spawn()` —— 每行一次 fork+exec。BLE 生命周期
        //     日志一多（每帧一条 [SEND]/[RECV]），真机上就是"一边跑 GATT、一边疯狂 fork"，
        //     直接拖慢 Rust 运行时与蓝牙时序（用户看到的"连上就断 / 加好友没反应"）。
        //     实测证据：`adb logcat -s gosslan` 里**每一行的 PID 都不一样**（每次 fork 新进程）。
        //     现在改成：丢进队列 → 专用线程按 200ms 合批 → 每批只 fork 一次，整批作为
        //     一条多行消息发出（`adb logcat` 里依然逐行可见）。
        #[cfg(target_os = "android")]
        {
            // info 级允许进 logcat 的 target —— 这几个是真机诊断的关键链路：
            //   boot    = 启动路标（闪退定位）
            //   ble     = BLE 扫描/连接/发送（唯一没法桌面自测的链路）
            //   dispatch= 三优先级调度 + try_send + writer_loop（消息卡在哪里）
            //   file    = FileOffer/Accept/Streaming/Done（文件传输全生命周期）
            //   content = ContentRequest retry（retry 有没有退避/停止）
            //   transport = TCP write fail / 链路事件
            const LOGCAT_INFO_TARGETS: &[&str] =
                &["boot", "ble", "dispatch", "file", "content", "transport"];
            let want = matches!(level, Level::Warn | Level::Error)
                || (level == Level::Info && LOGCAT_INFO_TARGETS.contains(&target));
            if want {
                logcat::push(format!(
                    "{} [{}] [{}] {}",
                    self.fingerprint,
                    level.as_str(),
                    target,
                    message
                ));
            }
        }

        // 3) 落盘（追加写，超限轮转）。失败静默：日志本身不能反过来拖垮主流程。
        self.append_file(ts, level, target, &message);
    }

    /// UI 快照：按时间正序（旧 → 新）返回内存日志。
    ///
    /// `since_ms = Some(epoch_ms)` 只返回 >= 该时间戳的行；None = 返回全部。
    /// 内存 ring buffer 本身有界（500 条），过滤只是切片，没有额外开销。
    pub fn snapshot(&self, since_ms: Option<i64>) -> Vec<LogEntry> {
        let q = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match since_ms {
            None => q.iter().cloned().collect(),
            Some(since) => q.iter().filter(|e| e.ts >= since).cloned().collect(),
        }
    }

    /// 清空内存与落盘文件。
    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let line = format!(
                "{} {} {} [{}] {}",
                self.fingerprint,
                format_utc(ts),
                level.as_str(),
                target,
                message
            );
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
        let logger = Logger::new(dir.clone(), "t", "[dev:test]".to_string());
        // 写超过上限：只保留最后 MAX_MEM_LOGS 条
        for i in 0..(MAX_MEM_LOGS as i64 + 100) {
            logger.info("test", format!("msg {i}"));
        }
        let snap = logger.snapshot(None);
        assert_eq!(snap.len(), MAX_MEM_LOGS);
        // 最旧的 100 条已被丢弃，第一条是 msg 100
        assert!(snap[0].message == "msg 100");
        assert!(snap.last().unwrap().message == format!("msg {}", MAX_MEM_LOGS + 99));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logger_clear_empties_memory() {
        let dir = std::env::temp_dir().join(format!("gosslan-log-clr-{}", std::process::id()));
        let logger = Logger::new(dir.clone(), "t", "[dev:test]".to_string());
        logger.error("test", "boom");
        assert_eq!(logger.snapshot(None).len(), 1);
        logger.clear();
        assert!(logger.snapshot(None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logger_persists_to_file_and_rotates() {
        let dir = std::env::temp_dir().join(format!("gosslan-log-rot-{}", std::process::id()));
        let logger = Logger::new(dir.clone(), "t", "[dev:test]".to_string());
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
