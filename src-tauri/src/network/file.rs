//! 文件传输与共享目录服务。
//!
//! 传输流程（E2EE）：
//! - 发送方：`send_file_from_path` 为本 transfer 生成随机文件会话密钥，用接收方
//!   X25519 公钥 ECDH + AEAD 封装后随 `FileOffer` 发出；等待 `FileAccept`
//!   （oneshot 握手）后，以 256KB 分片**逐片加密**为 `FileChunk` 流式发送，最后 `FileDone`。
//! - 接收方：收到 `FileOffer` 后解封会话密钥（只有我能解开），自动接受，
//!   逐片解密写入 `.part` 临时文件，`FileDone` 时改名落盘。密文绝不落盘。
//! - 中继路径：中继节点只透传密文切片，不持有会话密钥、无法解密。

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::Emitter;
use tokio::time::Duration;

use crate::crypto;
use crate::db;
use crate::network::transport::{
    clear_file_wire_progress_in, file_wire_chunks_at, file_wire_progress_at, resolve_member_x25519,
    send_on_link, try_send,
};
use crate::protocol::{Message, ShareEntry, FILE_CHUNK};
use crate::state::{AppState, FileDoneInfo, FileFailedInfo, FileReceiver};

fn emit_failed(state: &AppState, transfer_id: &str, reason: impl Into<String>) {
    let _ = state.app.emit(
        "file-failed",
        &FileFailedInfo {
            transfer_id: transfer_id.to_string(),
            reason: reason.into(),
        },
    );
}

/// 流式计算文件 SHA-256（256KB 分块增量更新，不整读内存），返回小写 hex。
pub fn sha256_file_hex(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; FILE_CHUNK];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// BLE 上每个文件分块的大小（见 `chunk_size_for_path` 的完整推导）。
pub const BLE_FILE_CHUNK: usize = 4 * 1024;

/// 一片分块**上线之后**占多少字节：`seal` 前置 12B nonce + Poly1305 16B tag，
/// 整段再 base64（×4/3 向上取整）。
///
/// 为什么单独把这个算术抽出来：链路队列预算（`transport::low_queue_slots`）、
/// 进度口径、deadline 换算都要用它，各处自己写一遍 `×4/3` 必然漂移
/// （本文件 `send_deadline` 用的是"偏保守的 4/3"，而队列注释当年按明文 256KB 估 ⇒ 差 33%）。
pub fn chunk_wire_bytes(plain: usize) -> usize {
    plain.saturating_add(28).saturating_mul(4).div_ceil(3)
}

/// **文件分块大小必须匹配链路的字节层能力**（真机 2026-09-13：大图"两边都显示成功、
/// 对方列表里却没有"的真因之一）。
///
/// 一对一文件流的每块默认是 [`FILE_CHUNK`] = 256 KiB。在局域网 TCP 上没问题；
/// 但在 **MTU=23 的 BLE** 上，一块 256 KiB 需要 ⌈262144/14⌉ = **18725 个分片**，
/// 而 BLE 分片层的上限是 `MAX_BLE_CHUNKS_PER_MESSAGE` = 8192 ⇒ `fragment()` 直接返回
/// `None` ⇒ 写循环把它当**写失败**并拆掉整条链路（真机日志：
/// `[SEND] 写失败 ⇒ 结束该链路写循环 … type=file_chunk`）⇒ 传输永远完不成，
/// 而发送方界面照样显示"已发送/已读"。
///
/// 4 KiB 在同样链路上只要 293 片（安全余量 28×），单块耗时 ≈ 293 × 12ms ≈ 3.5s。
/// 更大的块没有意义：BLE 的瓶颈是链路速率，不是分块数。
pub fn chunk_size_for_path(path: &str) -> usize {
    if path == crate::mesh::PathKind::Bluetooth.as_str() {
        BLE_FILE_CHUNK
    } else {
        FILE_CHUNK
    }
}

/// 只有蓝牙链路可用时，**超过这个体积的文件不启动发送**：保持 pending，等 LAN/Routed/Relay 回来。
///
/// 为什么必须有这道闸（2026-09-23 真机 600MB 复核）：BLE 上分片被压到 [`BLE_FILE_CHUNK`]，
/// 600MB = 153,600 片；按 `FILE_SEND_DEADLINE` 注释里的 BLE 实测速率（≈14KB/s）算要十几小时，
/// 而单轮 deadline 封顶 1h ⇒ **必然反复超窗重投**。重投又会重新选路、重新从 0 编号分片，
/// 撞上接收端"陈旧分片 ⇒ `ChunkSeq::Gap` ⇒ 整单判死"（见 `chunk_seq_decision`）。
/// 更糟的是 `file_sending` 按 **peer** 去重、一次只跑一个发送任务（`commands/files.rs`）⇒
/// 一个大文件在 BLE 上爬，会把同 peer 的**其它所有文件**一起堵在队列里。
///
/// 16MiB 的取值：14KB/s 下 ≈ 20min，落在 10min 下限与 1h 上限之间 —— 再大就注定要跨 attempt
/// 接力，而接力正是上面那条判死链的入口。
pub const BLE_FILE_SIZE_LIMIT: u64 = 16 * 1024 * 1024;

/// 「这个文件在当前可用链路上该不该发」的判据（纯函数，便于钉住）。
/// 返回 `Some(原因)` = 先别发：调用方保持 pending（不消耗 attempts、不落 failed），
/// 并把原因显示给用户 —— 否则界面只会停在"发送中 0%"，正是要消灭的形状。
///
/// 只认**最佳链路**：只要还有 LAN/Routed/Relay 可用，大文件照发。这道闸不是"BLE 上一律不发"，
/// 而是"只剩 BLE 时不要开始一件注定完不成的事"。
pub fn refuse_reason_for_best_link(
    best: Option<crate::mesh::PathKind>,
    size: u64,
) -> Option<&'static str> {
    match best {
        Some(crate::mesh::PathKind::Bluetooth) if size > BLE_FILE_SIZE_LIMIT => {
            Some("文件过大，蓝牙链路不适合；等局域网恢复后自动重试")
        }
        _ => None,
    }
}

/// `AppState::file_send_cancels` 的键：`transfer_id` + 收件人，用 NUL 连接。
///
/// 为什么不能只用 `transfer_id`（真机：三成员以上群文件只有一人收得到）：
/// 群文件是**每个成员一个投递任务、共用同一个 `transfer_id`**
/// （`group_announcements.rs` 对每个可达成员 `tokio::spawn`）。单键时后注册的
/// `HashMap::insert` 会把前一个任务的 `Sender` 直接挤掉，而 oneshot 的 `Receiver`
/// 在 `Sender` 被 drop 时立刻以 `Err(RecvError)` 完成 —— 投递循环里
/// `_ = &mut cancel_rx => ...` 这条分支分不清「用户真点了取消」和「登记被顶替」，
/// 于是 N-1 个成员以「用户取消发送」这个**假原因**当场中断，而且永远停在 sending。
///
/// 为什么用 NUL 不用 `:`：`transfer_id`（uuid）与 `device_id` 都不会含 NUL，
/// 前缀匹配因此不可能误伤「id 恰好以另一个 id 开头」的兄弟条目。
/// 「一条传输 × 一个收件人」的**唯一**键格式。
///
/// 为什么按收件人：一次群文件投递是**每个成员各 spawn 一个任务、共用同一个 `transfer_id`**
/// （`commands/group_file_dispatch.rs`），单聊+续传也可能同 id 多次尝试。只按 `transfer_id`
/// 记的话，甲还在链路上走的字节会让乙的"最近有写出"一直刷新 ⇒ 真卡死的乙永远判不出停滞。
///
/// 分隔符只此一份：取消登记（`file_cancel_key`）与写出记账用的是同一个格式，
/// 出现第二种拼法就是"同一件事两处各算一遍"——本仓库反复付过钱的那类缺陷。
pub fn file_peer_key(transfer_id: &str, recipient: &str) -> String {
    format!("{transfer_id}\u{0}{recipient}")
}

/// 同一 `transfer_id` 下所有收件人键的公共前缀。
pub fn file_peer_prefix(transfer_id: &str) -> String {
    format!("{transfer_id}\u{0}")
}

/// 取消登记的键（= `file_peer_key`，保留这个名字是给既有调用点与守卫用）。
pub fn file_cancel_key(transfer_id: &str, recipient: &str) -> String {
    file_peer_key(transfer_id, recipient)
}

/// 取消入口用的前缀：同一 `transfer_id` 下所有收件人的键都以它开头。
pub fn file_cancel_prefix(transfer_id: &str) -> String {
    file_peer_prefix(transfer_id)
}

/// SHA-256 hex 表示校验（64 位 hex，大小写均可；比较时统一小写）。
pub fn valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 文件发送 deadline：Offer → chunk loop → FileDone → wait_complete_ack 的**整体墙钟上限**。
///
/// 之前 send_file_from_path 内部 chunk 循环 + wait_complete_ack 各自有局部超时，但没有
/// 整体墙钟上限。BLE 上每断一次链会走 outbox 重试 → FileReject 往返 → 续发 → 又断链
/// 的循环，整体 6+ 分钟都"发送中"。超过这个时间直接 return retryable 让 outbox 重排；
/// outbox 再用自己的 attempts 上限决定最终置 failed。
///
/// 取值 10 分钟：40MB 文件 / BLE（≈ 14KB/s）= 理论 47 分钟，10 分钟不可能完整送达；
/// 但**如果 10 分钟里连续发续到哪里都没取得进展**，基本就是链路反复通一下又断，
/// 继续等没有收益。LAN 上 10 分钟能传 ~600MB（2MB/s），完全够。
pub const FILE_SEND_DEADLINE: Duration = Duration::from_secs(10 * 60);

/// 按体积自适应的整体发送期限（2026-09-19 真机：一次多选里 500-600MB 的视频
/// 在普通 Wi-Fi/中继链路上跑不完 10 分钟固定窗口 ⇒ 必被判超时、界面卡死成失败）。
/// 保守吞吐 512 KiB/s 估算，下限 10min、上限 **1h**（512 KiB/s 下 1h ≈ 1.8GB，
/// 再大的文件也该由断点续传的 `.part` + outbox 重试兜底，而不是无限挂着发送任务）。
///
/// 为什么原来敢给 2h、现在敢压到 1h：真正该管"对端不收"的是下面的**停滞判定**
/// （`stall_verdict`，以 writer 实发为准），它 60s 就退出。deadline 只负责
/// "整件事最多占多久资源"，不再兼任停滞兜底。
///
/// ⚠️ 估算必须按**线上字节**而不是明文（2026-09-23 真机 600MB 复核）：每片 256KiB 明文上线
/// 要过 ChaCha20-Poly1305（+28B）再 Base64（×4/3）⇒ **×1.334**。按明文算等于给每条链路少发
/// 25% 的窗口 —— 600MB 明文口径只给 21min，而 512KiB/s 的链路实需 26.7min ⇒ **单轮注定超窗**，
/// 只能靠 5 次重投 + `.part` 续传接力；而"接力"正是跨链路重试 ⇒ 陈旧分片 ⇒ 接收端 Gap 判死的入口。
pub fn send_deadline_for(size: u64) -> Duration {
    const MIN: Duration = FILE_SEND_DEADLINE;
    const CAP: Duration = Duration::from_secs(60 * 60);
    const BYTES_PER_SEC: u64 = 512 * 1024;
    // 线上字节 ≈ 明文 × 4/3（每片 AEAD 的 28B 在这个量级可忽略，且 ×4/3 本身已偏保守）
    let wire = size.saturating_mul(4).div_ceil(3);
    let est = Duration::from_secs(wire / BYTES_PER_SEC + 60);
    est.clamp(MIN, CAP)
}

/// 连续这么久没有再**写出**任何一片 ⇒ 界面上把这条传输标成「网络停滞」。
/// 取 15s：LAN 上一条 256KiB 片的间隔是毫秒级，15s 静默已经不是抖动而是对端读不动。
pub const FILE_STALL_WARN_MS: i64 = 15_000;

/// 连续停滞超过这个时长 ⇒ 放弃本次尝试（可重试）：outbox 会按对端真实已收字节续传，
/// 比让发送任务在背压里干等到 deadline（最长 1h）诚实得多。
pub const FILE_STALL_ABORT_MS: i64 = 60_000;

/// 接收器静默到这个时长 ⇒ 回收（第 1 步 · 故障隔离 P1）。
///
/// 必须**宽于**发送侧的 abort：`protocol.rs` 里没有任何 cancel 帧（实测 `Cancel` 零命中），
/// 发送侧 60s 停滞只是自己退回 `file_outbox`，接收端**收不到通知**。留太短会在
/// "对端正在重排队、马上重新 Offer" 的间隙里把自己那半截清掉，续传点位与对端分裂。
pub const FILE_RECEIVE_IDLE_ABORT_MS: i64 = 5 * 60_000;

/// 纯判据：静默 `idle_ms` 的接收器该不该回收。副作用留给调用方（落终态 + emit 都在锁外）。
///
/// 只有一条比较，故意不再加"负数不算"的守卫：`now` 与 `fed_at_ms` 取自**同一个**
/// `db::now_ms()`，倒挂只可能来自调用方算错（那是 bug，该由测试钉住，不是靠判据吞掉）。
pub fn receive_is_stale(idle_ms: i64) -> bool {
    idle_ms >= FILE_RECEIVE_IDLE_ABORT_MS
}

/// 停滞判定的档位（**纯函数**输出，副作用留给调用方：发事件 / 退出）。
#[derive(Debug, PartialEq, Eq)]
pub enum StallVerdict {
    /// 链路上还在实发，一切正常。
    Healthy,
    /// 已经静默到该提醒用户的程度，但还在等。
    Warn,
    /// 静默太久 ⇒ 本次尝试该放弃了。
    Abort,
}

/// 只判"停滞到哪个档"。
///
/// `idle_ms` 必须是**距最近一次成功写出该 transfer 分片**的毫秒数（调用方负责
/// 用本次尝试的起始时间做下限，否则上一轮 attempt 留下的旧时间戳会让第一轮就 Abort）。
pub fn stall_verdict(idle_ms: i64) -> StallVerdict {
    if idle_ms >= FILE_STALL_ABORT_MS {
        StallVerdict::Abort
    } else if idle_ms >= FILE_STALL_WARN_MS {
        StallVerdict::Warn
    } else {
        StallVerdict::Healthy
    }
}

/// 中继文件发送 deadline（send_file_via_relay）。比直传短：
/// - relay 没有 FileAccept 握手和 FileCompleteAck，只是 RelayFileOffer + RelayChunk 盲发
/// - 单跳中继理论上比 BLE 快很多，5min 能发几十 MB
/// - relay 链路一旦断了（中继掉线），重试也没意义（relay 不进 outbox）
pub const RELAY_FILE_SEND_DEADLINE: Duration = Duration::from_secs(5 * 60);

/// outbox 重试上限：同一文件连续尝试超过这个次数仍失败（retryable），
/// 就标记永久失败 —— 避免 BLE 反复断链导致无限循环 "sending" 永远挂着。
/// 每次尝试之间有 5s backoff；5 次 = 最多 25s 的 backoff 等待 + 每次尝试的耗时。
pub const MAX_FILE_OUTBOX_RETRIES: i64 = 5;

/// 文件发送失败分类：`retryable = true` 表示链路/超时等可恢复错误，
/// 应保留在 `file_outbox` 等待重试；`false` 表示文件缺失、非好友、缺公钥等永久错误。
#[derive(Debug, Clone)]
pub struct SendFileError {
    pub retryable: bool,
    pub message: String,
}

impl SendFileError {
    fn retryable(message: impl Into<String>) -> Self {
        Self {
            retryable: true,
            message: message.into(),
        }
    }

    fn permanent(message: impl Into<String>) -> Self {
        Self {
            retryable: false,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SendFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SendFileError {}

/// 一次投递失败之后，这条 `file_outbox` 行该怎么办。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RetryVerdict {
    /// 放弃：调用方必须落终态并把这句理由显示出来（"发送中"和"卡死"必须能区分）。
    GiveUp(String),
    /// 回到 pending，`backoff_ms` 之后再试一次。
    Retry { backoff_ms: i64 },
}

/// 重试退避（与 `db::mark_file_outbox_pending` 的 `next_attempt_at` 同一个数）。
pub(crate) const FILE_OUTBOX_RETRY_BACKOFF_MS: i64 = 5_000;

/// 重试裁决（**纯函数**，#25 第 2 段）。
///
/// 为什么必须抽出来：这三行判断决定的是用户看到的是"失败 + 原因"还是"永久转圈"，
/// 而它原先 inline 在 `flush_pending_files` 的循环里 —— 那个循环吃 `AppState`，
/// 测试一次都触发不了。真机 160MB 那次的形状就是从这里漏出去的：
/// 每一轮都"可重试"，于是连续 5 次从头重灌，谁也不报错。
///
/// ⚠️ **`retryable` 不等于"无限重试"**：可恢复错误同样要过预算这一关（少了这一关
/// 就是那条历史缺陷）。反过来，读不到次数（prepare 失败、行已被别的出口删掉）时
/// 按 0 次处理 —— 那是本机自己一时出故障，把它当成"已经试满 5 次"会直接毁掉一次
/// 本可恢复的投递。
pub(crate) fn send_retry_verdict(err: &SendFileError, attempts: Option<i64>) -> RetryVerdict {
    if !err.retryable {
        return RetryVerdict::GiveUp(err.message.clone());
    }
    if attempts.unwrap_or(0) >= MAX_FILE_OUTBOX_RETRIES {
        return RetryVerdict::GiveUp("连续重试超限（链路长时间未恢复）".to_string());
    }
    RetryVerdict::Retry {
        backoff_ms: FILE_OUTBOX_RETRY_BACKOFF_MS,
    }
}

/// 停滞检查的醒来间隔：对端不收时最长 5s 才反映到界面，够用且不制造事件噪声。
pub const FILE_STALL_TICK: Duration = Duration::from_secs(5);

/// 把"发事件"和"要不要放弃"这两件副作用集中在一处（单聊与群发共用）。
///
/// `started_ms` 必须是**本次尝试**的起点：上一轮 attempt 留下的 writer 时间戳会让
/// 第一个 tick 就被判成停滞（`max` 兜住这个下界）。
///
/// `recipient` 参与的是**查哪条记账**（`file_peer_key`），不参与发给前端的那个 id ——
/// 界面按 `transfer_id` 认这条传输，键里带收件人只会让它认不出来。
pub(crate) fn stall_tick(
    state: &AppState,
    transfer_id: &str,
    recipient: &str,
    started_ms: i64,
    shown: &mut bool,
) -> crate::network::transport::Tick {
    let idle = db::now_ms()
        - file_wire_progress_at(state, &file_peer_key(transfer_id, recipient)).max(started_ms);
    let abort = stall_verdict(idle) == StallVerdict::Abort;
    if abort {
        state.logger.warn(
            "file",
            format!(
                "[STALL] transfer={transfer_id} 已静默 {idle}ms ⇒ 放弃本次尝试，交 outbox 续传"
            ),
        );
    }
    let should_show = !abort && stall_verdict(idle) == StallVerdict::Warn;
    if should_show != *shown {
        *shown = should_show;
        let _ = state.app.emit(
            "file-stalled",
            &crate::state::FileStalledInfo {
                transfer_id: transfer_id.to_string(),
                stalled: should_show,
                idle_ms: idle,
                reason: None,
            },
        );
    }
    if abort {
        crate::network::transport::Tick::Abort
    } else {
        crate::network::transport::Tick::Wait
    }
}

impl<'a> WireLedger<'a> {
    /// 装上回收守卫（单聊 `stream_file` 与群发投递各调一次，别自己拼结构体）。
    pub(crate) fn install(
        state: &'a AppState,
        transfer_id: &str,
        recipient: &str,
    ) -> WireLedger<'a> {
        WireLedger {
            table: &state.file_wire_progress,
            wire_key: file_peer_key(transfer_id, recipient),
        }
    }
}

/// `FileCompleteAck` 的等待窗口：**安静**这么久没有"写出进展"才算失败。
const FILE_ACK_IDLE: Duration = Duration::from_secs(30);

include!("file/send.rs");
include!("file/relay_push.rs");
include!("file/receive.rs");
include!("file/group_receive.rs");
include!("file/share_walk.rs");
include!("file_tests.rs");
