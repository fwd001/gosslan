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

/// 主动向 `peer_id` 发送本地文件。
///
/// 这是一个低层投递原语：只负责把文件完整送达并拿到接收方完成确认。
/// 不负责重试与持久化队列；调用方（`commands::flush_pending_files`）根据
/// `SendFileError::retryable` 决定保留重试还是标记失败。
pub async fn send_file_from_path(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
) -> Result<(), SendFileError> {
    send_file_from_path_at(state, peer_id, transfer_id, path, 0, 0).await
}

/// 同 send_file_from_path，但支持**断点续传**：从 from_bytes 偏移读文件、
/// 分片序号从 from_seq 起编号（接收端据此接着它已持有的前缀继续）。
pub async fn send_file_from_path_at(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
    from_seq: u32,
    from_bytes: u64,
) -> Result<(), SendFileError> {
    let meta = std::fs::metadata(&path)
        .map_err(|e| SendFileError::permanent(format!("文件不存在或不可读：{e}")))?;
    if !meta.is_file() {
        return Err(SendFileError::permanent("只能发送普通文件"));
    }
    let size = meta.len();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    // 📄 关键日志：文件发送入口
    state.logger.info(
        "file",
        format!(
            "[FILE] offer-sending peer={peer_id} tid={transfer_id} name={name} size={size} resume_from={from_bytes}"
        ),
    );

    // ---- E2EE：本 transfer 独立的随机文件会话密钥（CSPRNG），仅存内存 ----
    let file_key = crypto::random_key();
    // 整文件哈希放阻塞线程池（群路径 group_announcements.rs 早就是这么做的）：这是 async
    // 任务，600MB 的同步整读会把一个 tokio worker 占满，连带别的路径一起卡。
    let hash_path = path.clone();
    let file_sha256 = tokio::task::spawn_blocking(move || sha256_file_hex(&hash_path))
        .await
        .map_err(|e| SendFileError::permanent(format!("哈希任务失败：{e}")))?
        .map_err(SendFileError::permanent)?;
    // 发送行的 sha256 在建记录时是空的（不能让气泡等一次 O(体积) 扫描），这里用真正用于
    // 校验的那份补上 —— 同一值、幂等，重试的后续尝试进来也是 no-op。
    let msg_id = format!("file-{transfer_id}");
    let backfilled = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::fill_message_sha256(&dbc, &msg_id, &file_sha256).unwrap_or(false)
    };
    if backfilled {
        // 改了库还必须让前端也看到：只写库时内存里那条记录仍是 cid 空的版本，
        // 用户在同一会话里把刚发出去的文件转成合并卡片时，卡片按 `sha256`(=cid) 带出去，
        // 对端就再也拉不回这份内容（v4.22.16 引入的回归）。
        // 自记录不会触发通知（`maybeNotify` 对 sender_id==自己直接 return）。
        let rec = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_message_record(&dbc, &msg_id)
        };
        if let Some(rec) = rec {
            let _ = state.app.emit("message-received", &rec);
        }
    }
    let receiver_pubkey = resolve_member_x25519(state, peer_id);
    let sealed_key_b64 = (|| {
        let pubkey = receiver_pubkey.as_deref()?;
        let shared = crypto::shared_secret(&state.identity.x25519_secret, pubkey)?;
        Some(STANDARD.encode(crypto::seal(&shared, &file_key)?))
    })();
    let Some(sealed_key_b64) = sealed_key_b64 else {
        return Err(SendFileError::permanent("无法获取对方公钥，无法加密文件"));
    };

    let path_str = path.to_string_lossy().to_string();
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            &name,
            size,
            "send",
            "pending",
            Some(path_str.as_str()),
            0.0,
        )
        .ok();
        // 内容索引（ADR-0019 Phase 3）：按 cid 记录"本机持有这份完整字节"，
        // 之后任何人发 ContentRequest 都能直接回发（拥有即授权，无需确认）。
        let now = db::now_ms();
        let rec = crate::content::model::TransferRecord {
            cid: file_sha256.clone(),
            transfer_id: Some(transfer_id.to_string()),
            peer_id: peer_id.to_string(),
            group_id: None,
            name: name.clone(),
            size,
            direction: crate::content::model::Direction::Send,
            status: crate::content::model::TransferStatus::Active,
            received: size,
            attempts: 0,
            next_attempt_at: 0,
            last_error: None,
            path: Some(path_str.clone()),
            created_at: now,
            updated_at: now,
        };
        let _ = crate::content::store::upsert(&dbc, &rec);
    }

    let _ = from_seq;
    // ---- 用户取消注册表（L3 fix: 提前到 deadline 外层注册，覆盖 E2E） ----
    // 键含收件人：见 `file_cancel_key`（单键会让同 transfer_id 的并发投递互相挤掉登记）。
    let cancel_key = file_cancel_key(transfer_id, peer_id);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    state
        .file_send_cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cancel_key.clone(), cancel_tx);
    let cancel_cleanup = || {
        state
            .file_send_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&cancel_key);
    };

    // ---- 整体 deadline（L3 fix: 从 Offer 循环入口开始计时，覆盖 E2E） ----
    // 之前 FILE_SEND_DEADLINE 只包住 stream_file（Offer 接受后），
    // 但如果 BLE 断链在 Offer 阶段反复续发（FileReject.received = N → continue），
    // 总耗时会超过 10min 但 deadline 不会触发 —— 因为每次都是新的 accept 周期。
    //
    // 现在 timeout 包住 Offer 循环全部（3 次 attempt + 每次 accept 后的 stream_file），
    // 从第一次发 Offer 开始计时，确保 E2E 有硬上限。
    let send_deadline = send_deadline_for(size);
    // 整轮（含 Offer 循环里的 3 次重投）共用同一个 attempt 号：Offer 只在**未被接受**时才会重来，
    // 那时链路上还没有任何分片，所以不需要区分；真正需要区分的是"这一整轮 vs 上一整轮"。
    let attempt = send_attempt(state, peer_id, transfer_id);
    let result = tokio::time::timeout(send_deadline, async {
        // 发送 Offer → 等接受。接收端若回 FileReject.received = N（它已有 N 字节），
        // 就从该偏移续发 —— 发送端永远以接收端的真实进度为准，绝不重头覆盖。
        let mut resume_from = from_bytes;
        for _attempt in 0..3 {
            let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), u64>>();
            state
                .pending_file_accept
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(transfer_id.to_string(), tx);
            let offer = Message::FileOffer {
                transfer_id: transfer_id.to_string(),
                from: state.device_id.clone(),
                name: name.clone(),
                size,
                sealed_file_key: sealed_key_b64.clone(),
                file_sha256: file_sha256.clone(),
                from_seq: 0,
                from_bytes: resume_from,
                attempt,
            };
            if let Err(e) = try_send(state, peer_id, &offer).await {
                state
                    .pending_file_accept
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(transfer_id);
                state.logger.error(
                    "file",
                    format!(
                        "[FILE] offer SEND FAILED peer={peer_id} tid={transfer_id} err={e}"
                    ),
                );
                return Err(SendFileError::retryable(format!("建立文件传输失败：{e}")));
            }
            state.logger.info(
                "file",
                format!(
                    "[FILE] offer SENT peer={peer_id} tid={transfer_id} waiting FileAccept (15s)"
                ),
            );
            match tokio::time::timeout(Duration::from_secs(15), rx).await {
                Ok(Ok(Ok(()))) => {
                    state.logger.info(
                        "file",
                        format!(
                            "[FILE] accept RECEIVED peer={peer_id} tid={transfer_id} → start streaming"
                        ),
                    );
                    // H1 fix: stream_file 现在直接返回 SendFileError，外层不再需要
                    // 字符串 contains 手动判定 retryable/permanent。
                    return stream_file(
                        state,
                        peer_id,
                        transfer_id,
                        path,
                        name,
                        size,
                        file_key,
                        0,
                        resume_from,
                        attempt,
                        &mut cancel_rx,
                    )
                    .await;
                }
                Ok(Ok(Err(n))) if n >= size && size > 0 => {
                    // 接收端已完整持有这份文件（2026-09-23 审计 A6）：它的
                    // decide_offer 判定 AlreadyHave 后回 received = size ⇒ 一发分片
                    // 都不必再发。旧实现会继续重发 Offer（n > resume_from 不再成立 →
                    // 落入超时分支），接收端则整份重推一遍落"名字(1)"副本。
                    state.logger.info(
                        "file",
                        format!("接收端已完整收下该文件，直接完成 transfer={transfer_id}"),
                    );
                    // 收尾与 stream_file 拿到成功回执时**共用同一份**：少了它，对方明明
                    // 已经有了，本机气泡却永久转圈、transfer 行停在 pending。
                    finalize_send_accepted(state, transfer_id, peer_id, &name, size, &path);
                    return Ok(());
                }
                Ok(Ok(Err(n))) if n > resume_from => {
                    state.logger.info(
                        "file",
                        format!("接收端已有 {n} 字节，从断点续发 transfer={transfer_id}"),
                    );
                    resume_from = n;
                    continue;
                }
                _ => {
                    state
                        .pending_file_accept
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(transfer_id);
                    state.logger.warn(
                        "file",
                        format!(
                            "[FILE] offer TIMEOUT/REJECTED peer={peer_id} tid={transfer_id} → abort"
                        ),
                    );
                    return Err(SendFileError::retryable("对方未接受文件"));
                }
            }
        }
        Err(SendFileError::retryable("对方始终未接受文件"))
    })
    .await;

    // 无论成功、失败还是 timeout，都从 state 里移除 cancel sender。
    // timeout 时内部 stream_file/Offer 循环被 drop，但 sender 已注册 —— 必须显式清理。
    cancel_cleanup();

    match result {
        Ok(inner) => {
            state.logger.info(
                "file",
                format!(
                    "[FILE] COMPLETED peer={peer_id} tid={transfer_id} ok={}",
                    inner.is_ok()
                ),
            );
            inner
        }
        Err(_elapsed) => {
            state.logger.warn(
                "file",
                format!(
                    "[FILE] DEADLINE peer={peer_id} tid={transfer_id} elapsed > {}s → abort",
                    send_deadline.as_secs()
                ),
            );
            Err(SendFileError::retryable(format!(
                "文件发送超时（本次尝试超过 {}s 链路未恢复，将从断点自动续传）",
                send_deadline.as_secs()
            )))
        }
    }
}
/// 无直连时，借**一跳中继**把文件发给 peer_id（接收方是请求下载的共享目录主人）。
///
/// 与 send_file_from_path 的区别：
/// - 不做 FileAccept 握手（对方已显式请求下载），也不等 FileCompleteAck（中继无回执）；
/// - 走 RelayFileOffer + RelayChunk：中继只透传密文，E2EE 与直传一致；
/// - 单跳：中继必须与目标有直连（与既有 RelayChunk 的限制一致）。
///
/// **失败必须落 DB 终态**（2026-09-23 审计 A1）：推流建的行写的是 `active`，而**发送方向
/// 没有任何清扫器**（`sweep_stale_relay` 清的是接收侧那两张内存表）⇒ 旧实现里取消 / 读盘
/// 失败 / 整体超时 / 分片失败**每一条 Err 路径**都留一行 active 在库里 —— 界面当场收到
/// `file-failed`，重启后那条传输又变回"进行中 X%"并永久挂着。本包装是这条链唯一出口，
/// 失败时统一把仍 active 的行标 failed（已 done 的行不动，绝不改写既有终态）。
pub async fn send_file_via_relay(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    let outcome = relay_push_file(state, peer_id, transfer_id, path).await;
    if let Err(reason) = &outcome {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = db::mark_transfer_failed_if_active(&dbc, transfer_id) {
            state.logger.warn(
                "file",
                format!(
                    "中继发送失败后落终态也失败 transfer={transfer_id}: {e}（发送失败原因：{reason}）"
                ),
            );
        }
    }
    outcome
}

/// 真正推分片出去：见 [`send_file_via_relay`] —— 终态落库在外层，保证所有 Err 路径同一条出口。
async fn relay_push_file(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("文件不存在或不可读：{e}"))?;
    if !meta.is_file() {
        return Err("只能发送普通文件".to_string());
    }
    let size = meta.len();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let file_key = crypto::random_key();
    let file_sha256 = sha256_file_hex(&path)?;
    let receiver_pubkey = resolve_member_x25519(state, peer_id)
        .ok_or_else(|| "无法获取对方公钥，无法加密文件".to_string())?;
    let shared = crypto::shared_secret(&state.identity.x25519_secret, &receiver_pubkey)
        .ok_or_else(|| "密钥交换失败".to_string())?;
    let sealed_key_b64 =
        STANDARD.encode(crypto::seal(&shared, &file_key).ok_or_else(|| "加密失败".to_string())?);
    // ⚠️ **逐片读盘，不整读进内存**。这里原先 `std::fs::read(&path)` 把整个文件读进来再切片：
    // 中继发送的是共享目录里的文件（可能很大），整读后逐片 base64（×1.33）会让内存峰值
    // 超过文件大小本身。改成按需 seek + read_exact，峰值只剩一个分片。
    //
    // 分片大小必须迁就链路中最受限的邻居（2026-09-23 审计 B1）：中继帧会发给
    // **所有**有直连的邻居，任一邻居是 BLE 时，64KiB 分片（base64 后 ~87KB）会反复
    // 撑爆 BLE 写超时（整帧一个 deadline，重试数次即拆链）—— 纯蓝牙链路必现停摆
    // 与丢片。直传早有 `chunk_size_for_path` 门控，中继路径在此补齐：有 BLE 邻居就
    // 整单用 BLE 尺寸（迁就最慢路径；协议无感知，total_chunks 相应变化，跨版本兼容）。
    //
    // 判据刻意用「邻居名下**存在** BLE 链路」而不是「邻居的最佳链路是 BLE」：
    // `send_over_order` 在首选链路队列满（Full）时会顺延到 order 里的下一条，
    // 因此同一邻居同时有 LAN+BLE 时，拥塞的那一帧照样会落到 BLE 上并把链路拆掉。
    // 按最佳链路判会把罕见但致命的拆链换成"LAN 邻居多吃些小帧"，不划算。
    let chunk_size = {
        let links = state.links.lock().await;
        let any_ble = links
            .iter()
            .filter(|(p, _)| p.as_str() != peer_id)
            .flat_map(|(_, ls)| ls.iter())
            .any(|l| l.path_kind == crate::mesh::PathKind::Bluetooth);
        if any_ble {
            BLE_FILE_CHUNK
        } else {
            crate::file_relay::MIN_CHUNK_SIZE
        }
    };
    let total = size as usize;
    let chunk_count = total.div_ceil(chunk_size).max(1) as u32;
    let mut src = std::fs::File::open(&path).map_err(|e| format!("读取文件失败：{e}"))?;

    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            &name,
            size,
            "send",
            "active",
            None,
            0.0,
        )
        .ok();
    }

    let offer = Message::RelayFileOffer {
        transfer_id: transfer_id.to_string(),
        from: state.device_id.clone(),
        to: peer_id.to_string(),
        name: name.clone(),
        size,
        total_chunks: chunk_count,
        // 接收方按 `seq × chunk_size` 直接落盘，所以这个数必须随 offer 一起过去
        chunk_size: chunk_size as u32,
        sealed_file_key: sealed_key_b64,
        file_sha256,
    };

    // ---- cancel + timeout 注册 ----
    // 键含收件人（与直传同口径，见 `file_cancel_key`）。
    let cancel_key = file_cancel_key(transfer_id, peer_id);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    state
        .file_send_cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cancel_key.clone(), cancel_tx);
    let cancel_cleanup = || {
        state
            .file_send_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&cancel_key);
    };

    let result = tokio::time::timeout(RELAY_FILE_SEND_DEADLINE, async {
        // Offer 一个邻居都没接住 ⇒ 这一帧从未离开本机：直接判失败，既不读盘也不推分片。
        // 方向要说清：这里只用「0 必然没送出」这一侧的下限判据；`≥1` **不等于**送达
        // （邻居未必与对方有直连），真送达要等接收端回执（A1-L2，尚未实施）。
        if crate::network::transport::relay_send_to_neighbors(state, peer_id, &offer).await == 0 {
            return Err("没有可达的中继邻居：文件未发出".to_string());
        }

        let mut last_report = std::time::Instant::now() - Duration::from_secs(1);
        for seq in 0..chunk_count {
            // 每片开始前先查 cancel —— 用户点了就立刻停，不浪费下一片 I/O
            if cancel_rx.try_recv().is_ok() {
                return Err("用户取消发送".to_string());
            }
            let start = (seq as usize * chunk_size).min(total);
            let end = (start + chunk_size).min(total);
            let mut plain = vec![0u8; end - start];
            src.seek(std::io::SeekFrom::Start(start as u64))
                .map_err(|e| format!("定位文件失败：{e}"))?;
            src.read_exact(&mut plain)
                .map_err(|e| format!("读取文件分片失败：{e}"))?;
            let sealed = crypto::seal_symmetric(&file_key, &plain)
                .ok_or_else(|| "文件分片加密失败".to_string())?;
            let data = STANDARD.encode(&sealed);
            let msg = Message::RelayChunk {
                transfer_id: transfer_id.to_string(),
                seq,
                data,
                from: state.device_id.clone(),
                to: peer_id.to_string(),
                ttl: 3,
            };
            // 同一判据用在每一片上：某片开始没有任何邻居接住，说明链路在这中间断了，
            // 继续推剩余分片只是把日志刷满并让界面停在最后一个报过的百分比上。
            if crate::network::transport::relay_send_to_neighbors(state, peer_id, &msg).await == 0 {
                return Err(format!("中继链路中断：第 {seq} 片没有任何邻居接住"));
            }
            let sent = end as u64;
            if last_report.elapsed() >= Duration::from_millis(250) {
                last_report = std::time::Instant::now();
                let progress = if size == 0 {
                    1.0
                } else {
                    sent as f64 / size as f64
                };
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        transfer_id,
                        peer_id,
                        &name,
                        size,
                        "send",
                        "active",
                        None,
                        progress,
                    )
                    .ok();
                }
                let _ = state.app.emit(
                    "file-progress",
                    &crate::state::FileProgress {
                        transfer_id: transfer_id.to_string(),
                        received: sent,
                        total: size,
                    },
                );
            }
        }
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::upsert_transfer(
                &dbc,
                transfer_id,
                peer_id,
                &name,
                size,
                "send",
                "sent",
                Some(path.to_string_lossy().as_ref()),
                1.0,
            )
            .ok();
        }
        // 最后一片可能因节流(250ms)跳过了 emit，收尾必须补一次完整进度，
        // 否则前端可能卡在"发送中 63%"（DB 已终态但前端没收到事件推进）。
        let _ = state.app.emit(
            "file-progress",
            &crate::state::FileProgress {
                transfer_id: transfer_id.to_string(),
                received: size,
                total: size,
            },
        );
        // ⚠️ **刻意不发 `file-done`**（2026-09-23 审计 A1 的 L2 那一半）：走到这里只代表
        // "每一片都被至少一个邻居接住"，**不代表**对端收全、SHA 校验通过、落了盘。而 `file-done`
        // 的语义是"本机这条传输已完成"，前端 `onFileDone` 会把内存里那行写成 done ——
        // 发它就是"库里 sent、界面上 ✓"，正撞验收红线「界面显示成功与对端实际收到不一致」。
        // 这条链没有接收端回执（`file.rs` 开头自述：中继无握手无回执），所以终态只能停在 `sent`；
        // 要把它升成 done，得给 `FileCompleteAck` 加 `to` 走定向一跳中继并按能力位门控。
        Ok(())
    })
    .await;

    cancel_cleanup();

    match result {
        Ok(inner) => inner,
        Err(_) => Err(format!(
            "中继文件发送超时（超过 {}s）",
            RELAY_FILE_SEND_DEADLINE.as_secs()
        )),
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

/// 一次发送尝试的"写出记账"，离开作用域就回收（v4.22.38）。
///
/// 为什么用 `Drop` 而不是在函数末尾调一次清理：`stream_file` 有十来处提前
/// `return Err(...)`（用户取消 / 链路已关闭 / 加密失败 / 没建上连接 / `?`），
/// 手写清理注定漏掉几个。漏掉的后果不是内存问题那么大，而是**口径**问题：
/// 重试时读到上一次尝试残留的 `chunks`，进度会悄悄退回"按入队算"（v4.22.37 那条
/// 修复等于白做，而且现场只在"发失败又重试"时才出现）。`Drop` 覆盖 `?`、提前
/// return 与 panic 展开，只此一处，所以也不会有第二份"什么时候删"。
///
/// 键是 `file_peer_key(transfer_id, recipient)` 而不是 `transfer_id`：群发是 N 个任务共用
/// 一个 id，按 id 回收会互相删（甲先收尾就把还在写的乙的记账清了）。
pub(crate) struct WireLedger<'a> {
    table: &'a std::sync::Mutex<std::collections::HashMap<String, crate::state::FileWireProgress>>,
    wire_key: String,
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

impl Drop for WireLedger<'_> {
    fn drop(&mut self) {
        clear_file_wire_progress_in(self.table, &self.wire_key);
    }
}

/// 发送进度换算：**已真的写出链路的分块**折算成明文字节，而不是"已入队"的字节。
///
/// 为什么要单独一个函数（v4.22.37）：`send_on_link` 返回成功只代表这一帧进了那条链路的
/// mpsc 队列（容量 1024，见 `transport.rs::writer_loop`）。LAN 一片 256KB ⇒ 最多
/// **262MB** 可以"界面已经 100%、实际还在排队"，用户据此以为传完了。真实写出量由 writer
/// 在 `write_frame` 成功后记账（`mark_file_wire_progress`），TCP 与 BLE 同一个证据点。
///
/// 两处钳制各有各的原因，都不是防御性装饰：
///   · `min(enqueued)`：计数器按 transfer_id 记，万一残留着上一次尝试的更大值，
///     进度也绝不能超过本机已经读出来的字节；
///   · `min(size)`：最后一片是短片，按整片折算会越过文件总大小（进度 >100%）。
fn wire_progress_bytes(
    from_bytes: u64,
    chunk_size: usize,
    enqueued: u64,
    flushed_chunks: u64,
    size: u64,
) -> u64 {
    let on_wire = from_bytes.saturating_add(flushed_chunks.saturating_mul(chunk_size as u64));
    enqueued.min(on_wire).min(size)
}

#[allow(clippy::too_many_arguments)]
async fn stream_file(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
    name: String,
    size: u64,
    file_key: [u8; 32],
    from_seq: u32,
    from_bytes: u64,
    // 本轮发送的轮次号（`None` = 对端不支持 ⇒ 帧里不带，行为与旧版完全一致）。
    attempt: Option<u32>,
    cancel_rx: &mut tokio::sync::oneshot::Receiver<()>,
) -> Result<(), SendFileError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    // 本次尝试的写出记账，离开作用域（含所有提前 return）自动回收。见 `WireLedger`。
    // 按 (transfer, 收件人) 成键：单聊这里只有一个收件人，与旧口径等价。
    let _wire_ledger = WireLedger::install(state, transfer_id, peer_id);
    let wire_key = file_peer_key(transfer_id, peer_id);

    let mut f = tokio::fs::File::open(&path)
        .await
        .map_err(|e| SendFileError::permanent(format!("打开文件失败：{e}")))?;
    // 整条分片流**钉死在一条链路**上（保序），分块大小也按这条链路决定
    // （BLE 上必须小，见 `chunk_size_for_path`）。为什么不能逐片 `try_send`：
    // 见 `transport::resolve_stream_link`。
    let link = crate::network::transport::resolve_stream_link(state, peer_id)
        .await
        .ok_or_else(|| SendFileError::retryable("未建立连接"))?;
    let chunk_size = chunk_size_for_path(link.path_kind.as_str());
    let mut buf = vec![0u8; chunk_size];
    // 断点续传：从接收端已持有的前缀之后开始读（分片序号也从 from_seq 接着数）。
    if from_bytes > 0 {
        f.seek(std::io::SeekFrom::Start(from_bytes))
            .await
            .map_err(|e| SendFileError::permanent(format!("定位文件失败：{e}")))?;
    }
    let mut seq = from_seq;
    let mut sent = from_bytes;
    // 本次尝试的起点（停滞判定的下界，见 `stall_tick`）+ 「停滞」提示的当前状态。
    let stream_started_ms = db::now_ms();
    let mut stalled_shown = false;
    // 进度节流：避免每片一次 SQLite 写 + IPC 事件（大文件会形成事件风暴卡死界面）
    let mut last_report = std::time::Instant::now() - Duration::from_secs(1);

    // 完成/否定确认的登记**提前到分片循环之前**：接收端一旦对某一片判死就能立刻把
    // `FileCompleteAck{success:false}` 送回来。放在循环之后注册的话，这条否定确认会因为
    // 找不到 rx 而被丢掉，发送端只能把剩下的字节全部灌完、再干等 `FILE_ACK_IDLE` 才发现失败。
    let (tx, mut ack_rx) = tokio::sync::oneshot::channel::<bool>();
    state
        .pending_file_complete
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(transfer_id.to_string(), tx);

    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            &name,
            size,
            "send",
            "active",
            None,
            0.0,
        )
        .ok();
    }

    loop {
        // 每片开始前先检查 cancel —— 用户点了"取消发送"就立刻停，不浪费那片 I/O。
        // biased: cancel 优先，避免与文件读公平竞争导致取消延迟。
        let n = tokio::select! {
            biased;
            _ = &mut *cancel_rx => return Err(SendFileError::permanent("用户取消发送")),
            _ = &mut ack_rx => {
                // 接收端已对某一片判死并回了否定确认 ⇒ 立刻停手，别再把剩余字节灌进一条
                // 已经死掉的传输。旧行为：整份发完 → FileDone 无人应答 → 干等一个
                // FILE_ACK_IDLE → 判"可重试" → 再整发 5 次（真机：两边都显示失败）。
                state
                    .pending_file_complete
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(transfer_id);
                return Err(SendFileError::retryable(
                    "接收端提前终止该传输（分片校验失败/超出声明大小），下次按对端已收字节续传",
                ));
            }
            n = f.read(&mut buf) => n.map_err(|e| SendFileError::permanent(format!("读取文件失败：{e}")))?,
        };
        if n == 0 {
            break;
        }
        // E2EE：每片独立随机 nonce 的 AEAD 密文（crypto::seal = nonce || ct），
        // 同一密钥不同片 nonce 必不相同，无 nonce 重用。
        let sealed = crypto::seal_symmetric(&file_key, &buf[..n])
            .ok_or_else(|| SendFileError::permanent("文件分片加密失败"))?;
        let data = STANDARD.encode(&sealed);
        let chunk = Message::FileChunk {
            transfer_id: transfer_id.to_string(),
            seq,
            data,
            attempt,
        };
        // 投递到**这条**链路：队列满时原地等背压，绝不换链路（换路 = 分片失序 = 整条传输判死）。
        // 等待期间每 `FILE_STALL_TICK` 醒一次做停滞检查 —— 对端不收时这里就是唯一的观测点。
        tokio::select! {
            biased;
            _ = &mut *cancel_rx => return Err(SendFileError::permanent("用户取消发送")),
            r = crate::network::transport::send_on_link_with_tick(
                &link,
                &chunk,
                FILE_STALL_TICK,
                || stall_tick(
                    state,
                    transfer_id,
                    peer_id,
                    stream_started_ms,
                    &mut stalled_shown,
                ),
            ) => {
                r.map_err(SendFileError::retryable)?;
            }
        }
        seq += 1;
        sent += n as u64;

        if last_report.elapsed() >= Duration::from_millis(250) {
            last_report = std::time::Instant::now();
            // 进度按**已写出链路**的分块算，不按入队算：入队最多能领先一整条队列
            // （1024 片 ≈ 262MB），旧口径下用户看到 100% 时其实还有大半文件没上链路。
            let on_wire = wire_progress_bytes(
                from_bytes,
                chunk_size,
                sent,
                file_wire_chunks_at(state, &wire_key),
                size,
            );
            let progress = if size == 0 {
                1.0
            } else {
                on_wire as f64 / size as f64
            };
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_transfer(
                    &dbc,
                    transfer_id,
                    peer_id,
                    &name,
                    size,
                    "send",
                    "active",
                    None,
                    progress,
                )
                .ok();
            }
            let _ = state.app.emit(
                "file-progress",
                &crate::state::FileProgress {
                    transfer_id: transfer_id.to_string(),
                    received: on_wire,
                    total: size,
                },
            );
        }
    }

    // 分片全部投完 ⇒ 收回停滞提示（接下来是 FileDone + ack 等待，那是另一段语义）。
    if stalled_shown {
        let _ = state.app.emit(
            "file-stalled",
            &crate::state::FileStalledInfo {
                transfer_id: transfer_id.to_string(),
                stalled: false,
                idle_ms: 0,
                reason: None,
            },
        );
    }

    // 发送方在 FileDone 之后必须等待接收方 FileCompleteAck：
    // TCP write 成功不代表文件已成功持久化，只有接收方 size/SHA-256 校验通过并落盘，
    // 才允许把本地消息推进到 delivered。（登记在分片循环之前，见上面 `ack_rx` 的注释。）
    // FileDone 必须排在**自己那串分片之后**：它和分片同为 Low 优先级，若走 `try_send`
    // 逐条选路，队列满时会 failover 到另一条空闲连接 —— 于是完成帧超过仍在路上的分片
    // （最多 1024 片 ≈ 262MB）先到，接收端判 "文件传输未完成" 直接把传输打死。
    // 真机症状：多文件并发时 600MB 的大文件跑到 100% 报分片/接收失败，单发同一文件必成功。
    let done = Message::FileDone {
        transfer_id: transfer_id.to_string(),
        attempt,
    };
    tokio::select! {
        biased;
        _ = &mut *cancel_rx => return Err(SendFileError::permanent("用户取消发送")),
        r = send_on_link(&link, &done) => {
            r.map_err(SendFileError::retryable)?;
        }
    }
    // wait_complete_ack 也支持 cancel —— ack 窗口最长 ~90s，用户不想等就该立刻释放。
    let completed = tokio::select! {
        biased;
        _ = &mut *cancel_rx => return Err(SendFileError::permanent("用户取消发送")),
        done = wait_complete_ack(state, transfer_id, peer_id, ack_rx) => done,
    };
    state
        .pending_file_complete
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id);
    // 进展记录由 `_wire_ledger` 在离开作用域时回收（成功与所有失败路径同一处）。
    if !completed {
        return Err(SendFileError::retryable("接收方未确认文件完成"));
    }
    finalize_send_accepted(state, transfer_id, peer_id, &name, size, &path);
    Ok(())
}

/// 发送端拿到"对方已完整收下"的证据后统一的收尾：落 `done` + 推进 `delivered` + 三个事件。
///
/// 必须只有一份，两个调用点共用：`stream_file` 收到成功 `FileCompleteAck` 之后，以及
/// Offer 阶段对方直接回 `received = size`（2026-09-23 审计 A6 的 `AlreadyHave`）。
/// 少一份的表现很具体：对方明明已经有了，本机气泡永久转圈、`file_transfers` 停在 pending。
fn finalize_send_accepted(
    state: &Arc<AppState>,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    path: &std::path::Path,
) {
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            name,
            size,
            "send",
            "done",
            None,
            1.0,
        )
        .ok();
        // 接收方已确认完成，此时推进 delivered 才是真实的。
        db::set_message_status(&dbc, &format!("file-{transfer_id}"), "delivered").ok();
    }
    // 通知前端发送方文件消息已完成（前端 onMessageAcked 会把 spinner 切为空圆框）
    let _ = state
        .app
        .emit("message-acked", &format!("file-{transfer_id}"));
    // 发送方也需要 file-done 事件来更新 transfer 状态（进度条消失 + transfer.status → done）
    let _ = state.app.emit(
        "file-done",
        &FileDoneInfo {
            transfer_id: transfer_id.to_string(),
            name: name.to_string(),
            size,
            path: path.to_string_lossy().to_string(),
        },
    );
    let _ = state.app.emit(
        "file-progress",
        &crate::state::FileProgress {
            transfer_id: transfer_id.to_string(),
            received: size,
            total: size,
        },
    );
}

/// 该 transfer_id 已保留的 .part 前缀字节数（无则 0）。
///
/// 用于把"接收端已持有多少"回给发送端（FileReject.received），让发送端从真实进度续发。
pub fn retained_part_len(state: &AppState, transfer_id: &str) -> u64 {
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    std::fs::metadata(dl.join(format!("{transfer_id}.part")))
        .map(|m| if m.is_file() { m.len() } else { 0 })
        .unwrap_or(0)
}

/// 定期清扫：删除超过 TTL、且当前不在接收中的 .part。
///
/// 为什么需要（审计 §7 风险 2）：可恢复失败会**保留** .part 作续传前缀，若对端一去不回，
/// 这些前缀会一直占空间。清扫只删"够旧 + 没人正在用"的，绝不碰活跃接收。
pub fn sweep_stale_parts(state: &AppState) -> usize {
    const TTL_MS: u64 = 24 * 60 * 60 * 1000;
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let active: std::collections::HashSet<String> = {
        let a: Vec<String> = state
            .file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        let b: Vec<String> = state
            .group_file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        a.into_iter().chain(b).collect()
    };
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(&dl) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("part") {
                continue;
            }
            if let Some(tid) = path.file_stem().and_then(|s| s.to_str()) {
                if active.contains(tid) {
                    continue;
                }
            }
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_millis() as u64 > TTL_MS)
                .unwrap_or(false);
            if stale && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// 断点续传：从已保留的 .part 前缀继续接收。
///
/// 与 begin_receive 的区别：**不再 truncate**，而是读入已有前缀播种 hasher，
/// received 从 from_bytes 接上；next_seq 归零（发送端从 from_seq=0 重编，只对本段排序）。
/// 任何不一致都返回 Err ⇒ 上层回 FileReject ⇒ 发送端整份重传（安全兜底）。
#[allow(clippy::too_many_arguments)]
pub fn resume_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
    from_bytes: u64,
) -> Result<PathBuf, String> {
    const TTL_MS: i64 = 24 * 60 * 60 * 1000;
    let safe_name = safe_file_name(name).ok_or("文件名非法")?;
    // 续传同样要落 `{id}.part`，消毒口径必须与 make_receiver 完全一致 ——
    // 否则「首次收被拦、续传绕过」就会留下一条可用的攻击路径。
    let transfer_id = safe_transfer_id(transfer_id).ok_or("传输标识非法")?;
    let transfer_id = transfer_id.as_str();
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let tmp_path = dl.join(format!("{transfer_id}.part"));
    let meta = std::fs::metadata(&tmp_path).map_err(|_| "续传前缀不存在".to_string())?;
    if !meta.is_file() || meta.len() != from_bytes || from_bytes == 0 || from_bytes > size {
        return Err("续传前缀与请求不一致".to_string());
    }
    // TTL：太旧的前缀不复用（避免无穷增长），删掉并让上层整份重传。
    if let Ok(modified) = meta.modified() {
        if let Ok(age) = modified.elapsed() {
            if age.as_millis() as i64 > TTL_MS {
                let _ = std::fs::remove_file(&tmp_path);
                return Err("续传前缀已过期".to_string());
            }
        }
    }
    // ⚠️ **分块喂哈希器，不整读进内存**。这曾经是 `std::fs::read(&tmp_path)`：
    // `.part` 前缀最长就等于整个文件，于是「几个 GB 的文件传到 90% 断链、对端续传」
    // 会让本进程瞬间占用 ≈ 文件大小的内存 —— 而续传恰恰是为了处理这种大文件场景。
    let hasher = {
        use sha2::Digest as _;
        let mut h = sha2::Sha256::new();
        let mut src = std::fs::File::open(&tmp_path).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; FILE_CHUNK];
        let mut counted: u64 = 0;
        loop {
            let n = src.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            counted += n as u64;
        }
        // 只记录、不改行为：`received` 仍以 from_bytes 为准（发送端的分片编号是据此推出来的，
        // 这里单方面改会让两端的 seq 对不上）。不一致说明该 .part 已被外部改动，
        // 后续 SHA-256 整体校验会拦下，此处先留下可诊断的痕迹。
        if counted != from_bytes {
            state.logger.warn(
                "file",
                format!(
                    "续传前缀长度与声明不符：磁盘 {counted} 字节 / 声明 {from_bytes} 字节（transfer={transfer_id}）"
                ),
            );
        }
        h
    };
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(&tmp_path)
        .map_err(|e| e.to_string())?;
    let final_path = unique_path(&dl, &safe_name);
    state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id);
    state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            transfer_id.to_string(),
            FileReceiver {
                file: f,
                name: safe_name,
                size,
                received: from_bytes,
                next_seq: 0,
                attempt: 0,
                stale_dropped: 0,
                tmp_path,
                final_path: final_path.clone(),
                peer_id: peer_id.to_string(),
                last_report_ms: crate::db::now_ms(),
                fed_at_ms: crate::db::now_ms(),
                file_key,
                expected_sha256,
                hasher,
            },
        );
    Ok(final_path)
}

/// 接收方：准备接收文件，返回最终落盘路径。
pub fn begin_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
) -> Result<PathBuf, String> {
    let final_path = make_receiver(
        state,
        transfer_id,
        peer_id,
        name,
        size,
        file_key,
        expected_sha256,
        &state.file_receivers,
    )?;
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            name,
            size,
            "receive",
            "active",
            Some(final_path.to_string_lossy().as_ref()),
            0.0,
        )
        .ok();
    }
    Ok(final_path)
}

/// 构造接收端状态（路径安全 + `.part` 创建 + 插入对应接收表），
/// 收到 `FileOffer` 时接收端的答复。
#[derive(Debug, PartialEq, Eq)]
pub enum OfferDecision {
    /// 回 `FileReject { received = 文件总大小 }`：本机已完整收下这份内容，
    /// 发送端不得再发任何分片（收到 `received ≥ size` 应直接宣布完成）。
    ///
    /// 为什么必须有（2026-09-23 审计 A6）：旧判据只有「活跃接收器 / .part 前缀 /
    /// from_bytes」三输入——**没有"本机已收完"**。收完之后 `.part` 已改名、接收器
    /// 已清空 ⇒ 三输入全归零 ⇒ 重复 Offer 判成 Accept ⇒ 整份重推落「名字(1)」副本
    /// （重复 Offer 的常见来源：A4 丢回执 → 发送端 outbox 重试）。
    AlreadyHave,
    /// 回 `FileAccept`，其它什么都不动（全新，或"同一起点的重复 offer"）。
    Accept,
    /// 回 `FileAccept`，**并且**把活跃接收器切到"新段从 `seq = 0` 重编"。
    ///
    /// 为什么必须有这一档：发送端续传时分片编号是按段从 0 重编的（`FileOffer.from_seq` 恒为 0）。
    /// 活跃接收器的 `next_seq` 还停在上一段末尾 ⇒ 不重置就会把这些**新数据**判成"迟到的重复片"
    /// 静默丢掉，文件永远差一截，而且不报错。
    AcceptResumeSegment,
    /// 回 `FileReject { received = 本端真实已收字节 }`：让发送端从真实位置续发。
    ResumeFrom(u64),
}

/// 「收到 offer 时该怎么答」的**唯一**判据（纯函数，能被单测直接钉住）。
///
/// 一句话：**接收端是"我有什么"的唯一权威，而且每次都要把这个回答出去。**
/// 之前这里有两套规矩 —— 没有活跃接收器时比对 `.part` 前缀、有活跃接收器时**一律** `Accept`
/// —— 后者在真机上把 160MB 的大文件判了死刑：发送端每一轮重试都从 `from_bytes = 0` 重发
/// （`flush_pending_files` 走的就是不带位置的 `send_file_from_path`），接收端回 Accept
/// 之后把那 40MB 已收前缀当"迟到的重复片"静默丢掉（`chunk_seq_decision` 的 `Duplicate` 分支），
/// 于是**每一轮都要重传一遍已收部分**，慢链路上永远跑不完 ⇒ 界面恒 0%、最后报"分片失败"。
///
/// `held` 的取法也是判据的一部分：有活跃接收器时用**内存里的 `received`**（每片 `write_all`
/// 之后就更新），没有时才退回磁盘 `.part` 的大小 —— 报小了会让发送端重灌已写进文件的字节，
/// 文件超长、SHA-256 必不匹配；报大了会让发送端以为对端有它没有的东西，永远等不齐。
///
/// ⚠️ 归零只在 `from_bytes > 0` 时做，无条件归零会引入另一个故障：上一轮 attempt 被 timeout
/// 丢掉时它**已入队的分片还在 writer_loop 里往外排**（队列 1024 槽 ≈ 262MB，丢 future 不排空
/// 队列），那些片的 seq 已到 160+，此时因一个 `from_bytes = 0` 的重复 offer 把 `next_seq`
/// 拍回 0 ⇒ 它们变成「跳号」⇒ `Err(文件分片顺序错误)` ⇒ 整单被判死。
///
/// ⚠️ **磁盘前缀的长度上限也是判据的一部分**（A-12，2026-09-26 由注入⑧在真实双实例照出）：
/// `FileReject.received` 承载两个含义 —— 发送端把 `received >= size` 读成"对方已完整收下"
/// 并直接收尾（记 done、**删掉队列行**）。所以"`.part` 字节数够了"绝不能当续传位置报出去：
/// 上一次收尾 `rename` 失败（目录只读 / 磁盘满 / 进程被杀在半步）时字节是整份的，
/// **但磁盘上没有成品文件**，而"成品"才是「rename 才算完成」的唯一凭证。
/// 那一格实测的形状就是这条判据要拦的：`A=done 且队列行已删 / B=failed / .part 整份 / final 不存在`
/// —— 两边各给了用户一个结论，而且互相矛盾，且再也没人重试。
/// 故：≥ size 的磁盘前缀**不算进度**，按 0 处理 ⇒ 重新整份收，走真实的哈希校验与真实的 rename。
/// 活跃接收器的内存计数不走这条（它收完那一刻收尾就已经跑过了，见 `write_chunk` 的裁决分支）。
pub fn decide_offer(
    has_active: bool,
    active_received: u64,
    disk_retained: u64,
    size: u64,
    from_bytes: u64,
    already_completed: bool,
) -> OfferDecision {
    // 「本机已收完」优先于一切位置判据（审计 A6）：收完后三输入全归零，
    // 任何位置比较都会退化成"整份重推"。
    if already_completed {
        return OfferDecision::AlreadyHave;
    }
    let held = if has_active {
        active_received
    } else if size > 0 && disk_retained >= size {
        0
    } else {
        disk_retained
    };
    if from_bytes != held {
        return OfferDecision::ResumeFrom(held);
    }
    if has_active && from_bytes > 0 {
        return OfferDecision::AcceptResumeSegment;
    }
    OfferDecision::Accept
}

/// 该 transfer 当前是否有活跃接收器，以及它真实收下了多少字节。
///
/// 返回 `None` = 没有活跃接收器（此时 `.part` 磁盘前缀才是唯一事实）。
pub fn receiver_progress(state: &AppState, transfer_id: &str) -> Option<u64> {
    state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(transfer_id)
        .map(|r| r.received)
}

/// 把活跃接收器切到"新的一段从 `seq = 0` 重新编号"，**不动**已收字节、文件位置与增量哈希。
///
/// 为什么必须做：发送端续传时分片编号是**按段**从 0 重编的（`FileOffer.from_seq` 恒为 0，
/// `stream_file` 也只按 `from_bytes` 定位文件偏移）。接收器如果还留着上一段推进到的
/// `next_seq = 160`，新段那些从 0 开始、内容其实是**新数据**的分片会被
/// `chunk_seq_decision` 判成"迟到的重复片"静默丢掉 ⇒ 文件永远差一截。
/// 只在"位置对得上、决定 Accept 而接收器已推进过"的情况下调用；重置的是段号，不是进度。
pub fn restart_segment(state: &AppState, transfer_id: &str) {
    if let Some(r) = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(transfer_id)
    {
        r.next_seq = 0;
    }
}

/// 一对一与群文件共用；差异只在写入哪个接收 map 与是否记录 file_transfers。
#[allow(clippy::too_many_arguments)]
fn make_receiver(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
    receivers: &std::sync::Mutex<HashMap<String, FileReceiver>>,
) -> Result<PathBuf, String> {
    let safe_name = safe_file_name(name).ok_or("文件名非法")?;
    // transfer_id 会成为 `{id}.part` 的文件名，必须与文件名同级消毒（见 safe_transfer_id）
    let transfer_id = safe_transfer_id(transfer_id).ok_or("传输标识非法")?;
    let transfer_id = transfer_id.as_str();
    if size > i64::MAX as u64 {
        return Err("文件过大，无法安全保存".to_string());
    }
    // 同一个 transfer_id 又收到一次 Offer = 发送方在**重试**（它每次都从 seq 0 重新开始）。
    // 旧实现这里直接返回「重复的文件传输」⇒ 接收方拒收 ⇒ 发送方 15s 等 accept 超时 ⇒
    // 可恢复失败 ⇒ 再重试 —— 死循环；而新 attempt 的分片与旧状态交错，就报出
    // 「文件分片顺序错误」。现在：把旧状态丢掉、从零重新开始。
    // 安全性：临时文件按 `transfer_id` 命名，下面 `File::create` 会**截断**它，
    // 新 attempt 不会与旧字节混写；`final_path` 仍走 `unique_path`（旧 final 未完成 ⇒ 不存在）。
    if let Some(old) = receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id)
    {
        let _ = std::fs::remove_file(&old.tmp_path);
    }
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    std::fs::create_dir_all(&dl).ok();
    let final_path = unique_path(&dl, &safe_name);
    // ⚠️ 临时文件必须按 **transfer_id** 命名，不能从 final_path 派生：
    // 两张同名图同时在途时，begin 那一刻磁盘上还没有同名文件 → unique_path 会给两者同一个 final_path
    // → 由它派生的 .part 也相同 → 两份字节交错写进同一文件 → sha256 校验失败
    // （表现为"第一张能看、第二张加载不出来"）。
    let tmp_path = dl.join(format!("{transfer_id}.part"));
    let f = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;

    receivers.lock().unwrap_or_else(|e| e.into_inner()).insert(
        transfer_id.to_string(),
        FileReceiver {
            file: f,
            name: safe_name,
            size,
            received: 0,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: tmp_path.clone(),
            final_path: final_path.clone(),
            peer_id: peer_id.to_string(),
            last_report_ms: 0,
            fed_at_ms: crate::db::now_ms(),
            file_key,
            expected_sha256,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        },
    );
    Ok(final_path)
}

/// 群文件接收：创建 `.part` 接收状态。
/// 路径安全（safe_file_name + downloads 目录内 unique_path）与一对一相同；
/// 不写 file_transfers——群文件的进度/状态记录在 group_files /
/// group_file_recipients 表中。
pub fn begin_group_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
) -> Result<PathBuf, String> {
    let final_path = make_receiver(
        state,
        transfer_id,
        peer_id,
        name,
        size,
        file_key,
        expected_sha256.clone(),
        &state.group_file_receivers,
    )?;
    // 持久化本地路径到 file_transfers（复用现有表，无 schema 变更）：
    // 群文件气泡的打开/另存/历史加载经 transfer_id 关联到该真实本地路径。
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            name,
            size,
            "receive",
            "active",
            Some(final_path.to_string_lossy().as_ref()),
            0.0,
        )
        .ok();
        // 统一状态：群文件接收一开始就登记 Active（cid → 暂无 path）。
        // 中途失败由 fail_group_receive 标 Incomplete ⇒ 建链自动重取
        // （群成员也能做种，原发送方不在也能从别人取）。
        let now = db::now_ms();
        let rec = crate::content::model::TransferRecord {
            cid: expected_sha256.clone(),
            transfer_id: Some(transfer_id.to_string()),
            peer_id: peer_id.to_string(),
            group_id: None,
            name: name.to_string(),
            size,
            direction: crate::content::model::Direction::Receive,
            status: crate::content::model::TransferStatus::Active,
            received: 0,
            attempts: 0,
            next_attempt_at: 0,
            last_error: None,
            path: None,
            created_at: now,
            updated_at: now,
        };
        let _ = crate::content::store::upsert(&dbc, &rec);
    }
    Ok(final_path)
}

/// 群文件接收失败：删除 `.part` 并移除接收状态（不 rename、不标 done）。
pub fn fail_group_receive(state: &AppState, transfer_id: &str) {
    if let Some(r) = state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id)
    {
        fail_taken_group_receive(state, &r);
    }
}

/// 对端确认一条链路都不剩时，**原子摘取**它名下全部群接收（判据与摘表在同一次持锁里完成）。
///
/// 为什么必须有这一份：`handle_peer_unlinked` 以前是"锁内 `collect()` 出 id、锁外逐个收尾"，
/// 而同一个文件里 `take_stalled_receive` / `take_stalled_group_receive` 的注释早就把那个形状判死过
/// —— 那两步之间完全可以挤进一个新 FileOffer（同一个 transfer_id 重建接收器），
/// 按 id 收尾就会把**正在正常收**的那一单判死。返回 (id, FileReceiver) 与那条家族同形状。
pub fn take_group_receives_for_peer(
    state: &AppState,
    peer_id: &str,
) -> Vec<(String, FileReceiver)> {
    let mut recv = state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = recv
        .iter()
        .filter(|(_, r)| r.peer_id == peer_id)
        .map(|(k, _)| k.clone())
        .collect();
    ids.into_iter()
        .filter_map(|k| recv.remove(&k).map(|r| (k, r)))
        .collect()
}

/// 静默群接收器的**原子**回收单位（理由同 `take_stalled_receive`：判据与摘表必须同一次持锁）。
pub fn take_stalled_group_receive(state: &AppState, now: i64) -> Option<(String, FileReceiver)> {
    let mut recv = state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = recv
        .iter()
        .find(|(_, r)| receive_is_stale(now - r.fed_at_ms))
        .map(|(k, _)| k.clone())?;
    recv.remove(&key).map(|r| (key, r))
}

/// 摘表之后的群接收收尾（与单聊同一份口径：`.part` 保留、状态记 Incomplete 可重取）。
pub fn fail_taken_group_receive(state: &AppState, r: &FileReceiver) {
    // **保留 .part**（不删）：断点续传的前缀（群友从种子拉取时也走 resume_receive）。
    let _ = &r.tmp_path;
    // 统一状态：群文件中途失败/断链 ⇒ Incomplete（可恢复）⇒ 建链时按退避自动重取。
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let _ = crate::content::store::record_failure(
        &dbc,
        &r.expected_sha256,
        &r.peer_id,
        crate::content::model::Direction::Receive,
        crate::content::model::FailReason::Partial,
        db::now_ms(),
    );
}

/// `FileCompleteAck` 的等待窗口：**安静**这么久没有"写出进展"才算失败。
const FILE_ACK_IDLE: Duration = Duration::from_secs(30);

/// 等 `FileCompleteAck`：按**链路上还有没有进展**判定，而不是固定墙钟。
///
/// ## 为什么必须改（2026-09-13 审计抓到的真缺陷）
///
/// 分块是**一次性全部入队**的（mpsc 容量 1024），而 `FileDone` 排在所有分块**后面**：
/// 1MB 文件在 BLE 上把 256 个分块在 **1 秒内**塞满队列，30s 只走得掉约 30KB
/// ⇒ 固定 30s **必然超时** ⇒ `retryable` ⇒ 每 5s 心跳从头重传（`seq` 也从 0 重来）
/// ⇒ 接收方要么报「重复的文件传输」、要么报「文件分片顺序错误」
/// ⇒ **BLE 上超过 ~20KB 的文件事实上永远传不完**（用户实测：500KB 图片传很久、最后报分片错误）。
///
/// ## 现在的判据
///
/// 每次超时只问一句：**这块传输最近有没有分块真的离开过链路**（`file_wire_progress`，
/// 由两条 writer_loop 在**写成功**时刷新）。有 ⇒ 重置窗口继续等；没有 ⇒ 才判失败。
/// 真断链 / 真丢包仍然会在 30s 安静之后失败，可靠性判据一点没放松。
async fn wait_complete_ack(
    state: &Arc<AppState>,
    transfer_id: &str,
    recipient: &str,
    mut rx: tokio::sync::oneshot::Receiver<bool>,
) -> bool {
    // 与 writer 记账同键（见 `mark_file_wire_progress`）：读错键等于永远"没有进展"，
    // 30s 安静窗口会提前把还在正常传输的链路判死。
    let wire_key = file_peer_key(transfer_id, recipient);
    let mut last_progress = file_wire_progress_at(state, &wire_key);
    loop {
        match tokio::time::timeout(FILE_ACK_IDLE, &mut rx).await {
            // 明确收到确认（true）/ 明确的否定或发送端被 drop（false）
            Ok(Ok(true)) => return true,
            Ok(_) => return false,
            Err(_) => {
                let now_progress = file_wire_progress_at(state, &wire_key);
                if now_progress > last_progress {
                    last_progress = now_progress;
                    continue; // 还有分块在真的往链路上走 ⇒ 继续等，不算失败
                }
                return false; // 安静了整整一个窗口 ⇒ 真的没进展
            }
        }
    }
}

/// 收到一片文件分片时该怎么处理（**纯函数**，便于单测钉住"重复不致命"这条规则）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChunkSeq {
    /// 重复或迟到的分片（`seq` 落在已收范围内）⇒ **忽略**，绝不让整单失败。
    Duplicate,
    /// 正好是下一片 ⇒ 写入。
    Accept,
    /// 跳号（中间真缺片）⇒ 只能靠重传解决。
    Gap,
}

/// 判据：`seq < next_seq` **忽略**、相等**接受**、大于**报错**。
///
/// 为什么"重复"不能报错（2026-09-13 审计的真缺陷）：发送方一次 attempt 超时后会**从头重传**
/// （`seq` 从 0 重来），上一轮的残片可能仍在链路上。旧实现一律报「文件分片顺序错误」，
/// 于是接收方整单失败、状态被清 ⇒ 新 attempt 也永远拼不齐 ⇒
/// **BLE 上 >20KB 的文件事实上永远传不完**（用户实测的那条报错就是它）。
/// 只挡"跳号"仍然安全：整份字节由文件级 SHA-256 兜底校验。
pub(crate) fn chunk_seq_decision(seq: u32, next_seq: u32) -> ChunkSeq {
    use std::cmp::Ordering;
    match seq.cmp(&next_seq) {
        Ordering::Less => ChunkSeq::Duplicate,
        Ordering::Equal => ChunkSeq::Accept,
        Ordering::Greater => ChunkSeq::Gap,
    }
}

/// 这片明文的长度会不会**越过声明的 `size`**（§七「错误 size」那格唯一的裁决点）。
///
/// 为什么这条判据必须在写盘**之前**、而不是留到收尾的"字节数与声明不符"：`.part` 是按片增长的，
/// 不设上限就等于让对端决定这台机器往磁盘上写多少字节，而用户看到的是一条会跑到 100% 再失败的单。
///
/// ⚠️ 边界刻意取"恰好填满 = 放行"：`stream_file` 的最后一片通常正好把 `size` 补齐，
/// 写成 `>=` 会让**每一单**都在最后一片上打死（与 [`chunk_seq_decision`] 只挡真空缺是同一类设计）。
pub(crate) fn chunk_exceeds_declared(size: u64, received: u64, plaintext_len: u64) -> bool {
    plaintext_len > size.saturating_sub(received)
}

/// 这一帧属于**当前这一轮**发送尝试吗（attempt epoch 判据，2026-09-23 真机 600MB 复核）。
///
/// 病根：一轮超时后 outbox 重投，但**上一轮已经塞进链路队列的分片不会被撤回**
/// （Low 队列 1024 槽 ≈ 262MB）。接收端每段的 `seq` 都是从 0 重编的，于是旧轮的高 `seq`
/// 落在新轮的 `next_seq` 之上 ⇒ 判成"跳号"⇒ 整单被打死。有了轮次号就能把它安静丢掉。
///
/// 三条刻意的设计：
/// 1. `None`（老端不带这个字段，或对方没声明 `CONTENT_FEATURE_FILE_EPOCH`）⇒ **恒真**，
///    完全退回旧语义 —— 新 behaviour 只在两端都支持时才生效。
/// 2. 用 `==` 而不是 `>=`：**比本机新的轮次也算"不是我的"**。"未来的分片"只可能是它的
///    Offer 还在另一条链路上排队；先收下会把文件拼坏，而拼坏由末尾 SHA 兜住 ⇒ 宁可丢这一片，
///    等它自己的 Offer 到达后由发送端从 `.part` 前缀续发（自愈，最多多一轮）。
/// 3. **Offer 永远照单全收并借此设定轮次**（不走这个判据）：否则本机重启后计数器回到 1，
///    而接收端存的是上一轮的 5 ⇒ 新 Offer 被判陈旧 ⇒ 这份文件永久饿死。Offer 是权威，
///    分片/完成帧才是被过滤的对象。
pub(crate) fn frame_is_current(frame: Option<u32>, current: u32) -> bool {
    match frame {
        Some(a) => a == current,
        None => true,
    }
}

/// `None`（老端不带 attempt）在本机记作这一轮。
pub const LEGACY_ATTEMPT: u32 = 0;

/// Offer 一到达就把接收器的"当前轮次"设成它带来的值 —— **Offer 是权威，不参与过滤**
/// （见 `frame_is_current` 第 3 条：否则本机重启后计数器回到小值，新 Offer 会被自己判成陈旧，
/// 那份文件就永久饿死）。
pub fn note_offer_attempt(state: &AppState, transfer_id: &str, attempt: Option<u32>) {
    if let Some(r) = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(transfer_id)
    {
        r.attempt = attempt.unwrap_or(LEGACY_ATTEMPT);
    }
}

/// `FileDone` 是否属于当前轮次（只读，不摘接收器）。
///
/// 为什么必须在 `finish_receive` **之前**判：那份函数一进来就把接收器摘掉了，
/// 陈旧完成帧会被当成"重复 FileDone"去补一个成功 Ack，或更糟 —— 拿这一轮刚开头的
/// `received ≠ size` 把整单打死。没有接收器时返回 true，交给原有"未知传输"分支处理。
pub fn done_is_current(state: &AppState, transfer_id: &str, attempt: Option<u32>) -> bool {
    let recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match recv.get(transfer_id) {
        Some(r) => frame_is_current(attempt, r.attempt),
        None => true,
    }
}

/// 本轮发送的 attempt 号（返回 `None` = 对端没声明能力 ⇒ 一个字段都不带，退回旧语义）。
///
/// 值**必须取自持久化的 `file_outbox.attempts`**，不能用进程内计数器：重启后 outbox 会重投，
/// 而计数器从 1 重新开始 ⇒ 比接收端已存的轮次还小，配上"`==` 才算当前"的判据，
/// 这份文件的每一轮都会被自己判成陈旧（就是上面第 3 条要避开的那个坑）。
fn send_attempt(state: &Arc<AppState>, peer_id: &str, transfer_id: &str) -> Option<u32> {
    let caps = {
        state
            .peer_content_features
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer_id)
            .copied()
            .unwrap_or(0)
    };
    if caps & crate::protocol::CONTENT_FEATURE_FILE_EPOCH == 0 {
        return None;
    }
    let attempts = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_file_outbox_attempts(&dbc, transfer_id)
    };
    // 没有 outbox 行（直发路径）按第 1 轮算：此刻链路上不可能有上一轮的残留。
    Some(attempts.and_then(|a| u32::try_from(a).ok()).unwrap_or(1))
}

/// 接收方：写入一个分片，返回累计字节数。
/// 入参 `data` 为 AEAD 密文（nonce || ciphertext）：先解密再写盘，
/// 解密失败直接报错——密文绝不落盘。
pub fn write_chunk(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    seq: u32,
    data: &[u8],
    attempt: Option<u32>,
) -> Result<u64, String> {
    use std::io::Write;
    // 锁作用域：先算完，把要落库的进度取出来，**释放 file_receivers 锁之后**再动 db
    // （避免 file_receivers -> db 的嵌套锁顺序）。
    let (received, cid, owner, report) = {
        let mut recv = state
            .file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = recv.get_mut(transfer_id).ok_or("未知传输")?;
        if r.peer_id != peer_id {
            return Err("文件传输来源不匹配".to_string());
        }
        // **上一轮 attempt 的残留分片**安静丢掉（2026-09-23 真机 600MB 的根治，attempt epoch）。
        // 不丢会怎样：它们带着上一轮的高 `seq`，落到下面的判据里就是"跳号"⇒ 整单打死 ——
        // 而旧注释里"重传时 seq 从 0 重来 ⇒ 残片算 Duplicate"那条推理，只在残片**先到**、
        // 新轮 Offer 把 next_seq 归零**之后**才成立；队列里还压着最多 262MB 时顺序是反的。
        if !frame_is_current(attempt, r.attempt) {
            if r.stale_dropped == 0 {
                state.logger.info(
                    "file",
                    format!(
                        "丢掉非当前轮次的分片（本机在接收第 {} 轮）transfer={transfer_id}",
                        r.attempt
                    ),
                );
            }
            r.stale_dropped += 1;
            return Ok(r.received);
        }
        // 重复/迟到的分片必须**忽略**，而不是整单失败：发送方一次 attempt 超时后会**从头重传**
        // （seq 从 0 重来），而上一轮的残片可能仍在链路上。只挡"跳号"（真缺片，只能重传）；
        // 整份字节仍由文件级 SHA-256 兜底。
        match chunk_seq_decision(seq, r.next_seq) {
            ChunkSeq::Duplicate => return Ok(r.received),
            ChunkSeq::Gap => return Err("文件分片顺序错误".to_string()),
            ChunkSeq::Accept => {}
        }
        let plaintext = crypto::open_symmetric(&r.file_key, data)
            .ok_or_else(|| "文件分片解密失败".to_string())?;
        if chunk_exceeds_declared(r.size, r.received, plaintext.len() as u64) {
            return Err("文件分片超出声明大小".to_string());
        }
        // 文件级完整性：明文增量哈希（与写盘同一份数据，无二次磁盘读取）
        use sha2::Digest;
        r.hasher.update(&plaintext);
        r.file.write_all(&plaintext).map_err(|e| e.to_string())?;
        r.received += plaintext.len() as u64;
        // 每片都记一次"还被喂得动"—— 这是 `receive_is_stale` 唯一的证据来源。
        // 代价是一次 epoch 毫秒读取，相对上面的 `write_all` 可以忽略。
        r.fed_at_ms = crate::db::now_ms();
        r.next_seq = r.next_seq.checked_add(1).ok_or("文件分片序号溢出")?;
        // 节流 500ms 落一次进度：这是断点续传的起点，也让统一状态显示真实进度。
        let now = crate::db::now_ms();
        let report = now - r.last_report_ms >= 500;
        if report {
            r.last_report_ms = now;
        }
        (
            r.received,
            r.expected_sha256.clone(),
            r.peer_id.clone(),
            report,
        )
    };
    if report {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = crate::content::store::touch_received(
            &dbc,
            &cid,
            &owner,
            received,
            crate::db::now_ms(),
        );
    }
    Ok(received)
}

/// 终止损坏或超时的接收：摘表 → 收尾（见 `fail_taken_receive`）。
pub fn fail_receive(state: &AppState, transfer_id: &str, peer_id: &str, reason: &str) -> bool {
    let mut recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(r) = recv.remove(transfer_id) else {
        return false;
    };
    if r.peer_id != peer_id {
        recv.insert(transfer_id.to_string(), r);
        return false;
    }
    drop(recv);
    fail_taken_receive(state, transfer_id, &r, reason);
    true
}

/// 静默接收器的**原子**回收单位：判据与摘表在同一次持锁里完成。
///
/// 为什么不能"先快照一批 id、再逐个收尾"：那两步之间完全可以挤进一个新 FileOffer
/// （同一 transfer_id 重新建接收器、`fed_at_ms` 就是现在）—— 按 id 收尾会把那个
/// **正在收**的传输判死。宁可每轮只摘一个、循环到没有，也不留这个缝。
pub fn take_stalled_receive(state: &AppState, now: i64) -> Option<(String, FileReceiver)> {
    let mut recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = recv
        .iter()
        .find(|(_, r)| receive_is_stale(now - r.fed_at_ms))
        .map(|(k, _)| k.clone())?;
    recv.remove(&key).map(|r| (key, r))
}

/// 摘表之后的收尾：落 failed 终态、记 Incomplete、emit。
/// `fail_receive` 与清扫器共用这一份 —— 终态口径不许有两套。
pub fn fail_taken_receive(state: &AppState, transfer_id: &str, r: &FileReceiver, reason: &str) {
    // **保留 .part**（不删）：这是断点续传的前缀。只有"确定是永久失败"（校验不符）
    // 才删；超时/断链属于可恢复。陈旧 .part 由 resume_receive 的 TTL 与后续清理收割。
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        // 注意：**不能**把 .part 写进 path —— find_source 只看 path 非空就当作可服务内容，
        // 那样会把"半截文件"当成完整种子发出去。.part 的位置由 transfer_id 推导。
        let _ = &r.tmp_path;
        // 统一状态：中途失败/超时/断链 ⇒ **Incomplete**（可恢复）。
        // 于是建链时 retry_incomplete_content 会按退避自动重取，而不是永远停在 Active。
        let _ = crate::content::store::record_failure(
            &dbc,
            &r.expected_sha256,
            &r.peer_id,
            crate::content::model::Direction::Receive,
            crate::content::model::FailReason::Partial,
            db::now_ms(),
        );
    }
    emit_failed(state, transfer_id, reason);
}

/// 对端断链时终止其所有未完成接收，避免下载目录长期堆积临时文件。
/// 对端确认一条链路都不剩时，**原子摘取**它名下全部单聊接收（判据与摘表同一次持锁）。
///
/// 与群侧 `take_group_receives_for_peer` 同一族。旧写法是"锁内 `collect()` 出 id、锁外逐个
/// `fail_receive`"：`fail_receive` 虽然会核对 `peer_id`（别人的原样插回），但**同 peer、同
/// transfer_id 的新 attempt** 正好可以在那两步之间建起来 ⇒ 上一轮的 teardown 会把这一轮
/// 正在收的传输判死。后果有界（内容记 Incomplete、建链时按退避自动重取），
/// 但"多等一轮"本身就是用户看到的"下载卡在重试"。
pub fn take_receives_for_peer(state: &AppState, peer_id: &str) -> Vec<(String, FileReceiver)> {
    let mut recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = recv
        .iter()
        .filter(|(_, r)| r.peer_id == peer_id)
        .map(|(k, _)| k.clone())
        .collect();
    ids.into_iter()
        .filter_map(|k| recv.remove(&k).map(|r| (k, r)))
        .collect()
}

pub fn fail_receives_for_peer(state: &AppState, peer_id: &str) {
    // 摘与收尾分开：摘在锁内一次做完，收尾（写库 + emit）一律在锁外。
    for (id, r) in take_receives_for_peer(state, peer_id) {
        fail_taken_receive(state, &id, &r, "对端连接已断开");
    }
}

/// 接收方：收尾，返回 (name, size, final_path, peer_id)。
pub fn finish_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
) -> Result<Option<(String, u64, PathBuf, String)>, String> {
    // ⚠️ 锁只用来"把接收器摘出来"，摘完立刻放（2026-09-23 真机 600MB 复核）：
    // 下面这段是 SHA finalize + `sync_all()` + rename —— 600MB 的 fsync 是秒级慢活，
    // 而它跑在 reader_loop 里；持锁期间**同一时刻其它并发文件的 `write_chunk` 全部堵在同一把锁上**
    // ⇒ 那些传输不再写出 ⇒ 发送端 60s 停滞判据（`FILE_STALL_ABORT_MS`）把它们判死。
    // 摘出来之后这份接收器已经不在表里，锁外独占使用它是安全的。
    let r = {
        let mut recv = state
            .file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match recv.remove(transfer_id) {
            Some(r) => {
                if r.peer_id != peer_id {
                    recv.insert(transfer_id.to_string(), r);
                    return Err("文件传输来源不匹配".to_string());
                }
                r
            }
            None => return Ok(None),
        }
    };
    finish_receiver_into(&state.db, transfer_id, r).map(Some)
}

/// 摘出接收器之后的收尾：完整性裁决 → fsync → 改名落盘 → 落库终态。
///
/// 三个 `Err` 出口都必须让 `file_transfers` 停在 `failed`（而不是留在 `sending` / `done`）：
/// ① 字节数与声明的 `size` 不等（丢片/截断）；② 实际 SHA-256 与发送方声明不符
/// （**含**"发送方没声明"这一种 —— 空期望值不通过，fail-closed）；③ fsync 或 rename 失败。
/// 这三条合起来就是界面上那句「已收到」的证据，少任何一条都是假成功。
///
/// 为什么要独立成一个函数（#25）：它只要一个 `Mutex<Connection>`，不要 `AppState`
/// （那个要 tauri `AppHandle`，单测造不出来）⇒ 「显示成功是不是真成功」第一次可以拿
/// **生产码**驱动，而不是像现有几处接收端测试那样各自重写一遍操作序列（生产码改坏它们不红）。
/// 另外它拿不到 `file_receivers` 那张表 ⇒ "持锁 fsync" 那个形状不可能从这里长回去。
fn finish_receiver_into(
    db_lock: &std::sync::Mutex<rusqlite::Connection>,
    transfer_id: &str,
    mut r: FileReceiver,
) -> Result<(String, u64, PathBuf, String), String> {
    if r.received != r.size {
        let _ = std::fs::remove_file(&r.tmp_path);
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        return Err("文件传输未完成".to_string());
    }
    // 文件级完整性校验：实际 SHA-256 必须与发送方声明一致，否则不落盘
    {
        use sha2::Digest;
        let actual = r.hasher.clone().finalize();
        let actual_hex: String = actual.iter().map(|b| format!("{b:02x}")).collect();
        if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
            let _ = std::fs::remove_file(&r.tmp_path);
            let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
            db::upsert_transfer(
                &dbc,
                transfer_id,
                &r.peer_id,
                &r.name,
                r.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
            return Err("文件完整性校验失败".to_string());
        }
    }
    if let Err(e) = r.file.sync_all() {
        let reason = e.to_string();
        let _ = std::fs::remove_file(&r.tmp_path);
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        return Err(reason);
    }
    drop(r.file);
    // §七「两个 offer 都在任一次 rename 之前到达」：`final_path` 是 begin 时用 `unique_path` 定的，
    // 而那一刻两份都还没落地 ⇒ 同名两单会拿到**同一个**名字，直接 rename 会在 POSIX 上覆盖掉先落地的
    // 那一份（两行台账都 done、一个气泡指着已经不存在的字节）。落地前再确认一次：被占走就换名。
    // （与群聊收尾同形状 —— 那边早就这么做了，其注释声称"单聊在写盘时才定名"与代码不符。）
    if r.final_path.exists() {
        if let Some(dir) = r.final_path.parent() {
            r.final_path = unique_path(dir, &r.name);
        }
    }
    if let Err(e) = std::fs::rename(&r.tmp_path, &r.final_path) {
        let reason = e.to_string();
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        return Err(reason);
    }
    {
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "done",
            Some(r.final_path.to_string_lossy().as_ref()),
            1.0,
        )
        .ok();
    }
    Ok((
        r.name.clone(),
        r.size,
        r.final_path.clone(),
        r.peer_id.clone(),
    ))
}

/// 递归枚举共享目录树（限制深度 8，跳过隐藏文件）。
pub fn walk_share_dir(root: &Path) -> Vec<ShareEntry> {
    let mut out = Vec::new();
    walk(root, "", &mut out, 0);
    out
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<ShareEntry>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(file_type) = e.file_type() else {
            continue;
        };
        // 不跟随符号链接，避免共享目录枚举泄露共享根目录之外的路径。
        if file_type.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let rel_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let is_dir = file_type.is_dir();
        let size = if is_dir {
            0
        } else {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        out.push(ShareEntry {
            name,
            path: rel_path.clone(),
            is_dir,
            size,
        });
        if is_dir {
            walk(&path, &rel_path, out, depth + 1);
        }
    }
}

/// 按文件扩展名（大小写不敏感）保守分类附件类型：`image` / `code` / `file`。
///
/// 这是纯函数，**仅依赖 basename**：FileOffer 已把 `name` 带到接收端，两端各自调用
/// 同一实现 → 分类结果天然一致，无需给文件传输协议增加字段。
///
/// 从路径提取最可靠的文件名。
///
/// Tauri Android file picker 有时把 content:// URI 转存到临时文件，
/// `Path::file_name()` 返回无扩展名的 `xxx`（比如 `478812312`），
/// 但原始路径字符串里可能仍保留着正确的扩展名。
/// 这个函数做三级 fallback：
/// 1. Path::file_name() 正常返回且有扩展名 → 直接用
/// 2. Path::file_name() 没扩展名 → 从完整 path 字符串找最后一个 `.xxx` 模式补上
/// 3. 都没有 → 返回 file_name() 的原值
pub(crate) fn derive_file_name(raw_path: &str) -> String {
    let p = Path::new(raw_path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    // Case 1: file_name 已经有扩展名 → 直接返回
    if p.extension().is_some() {
        return name;
    }

    // Case 2: file_name 没扩展名，但完整路径字符串末尾有类似 .mp4 / .jpg 的后缀
    let last_dot = raw_path.rfind('.');
    let last_slash = raw_path.rfind('/').unwrap_or(0);
    if let Some(dot) = last_dot {
        if dot > last_slash && dot + 1 < raw_path.len() {
            let ext_candidate = &raw_path[dot + 1..];
            if (1..=10).contains(&ext_candidate.len())
                && ext_candidate.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return format!("{name}.{ext_candidate}");
            }
        }
    }

    name
}

/// 未用 MIME 魔数嗅探的原因：那要么需要给 FileOffer/RelayFileOffer 加 kind 字段
/// （违反「不修改文件传输协议」），要么两端各自读字节嗅探（引入 sender/receiver 分歧）。
/// 任务给出的图片/代码清单本身即扩展名，扩展名判定已足够保守且确定。
///
/// ⚠️ `md`（Markdown）是**文档**而非代码，刻意排除在 `code` 之外——若把 .md 归为 code，
/// 接收端会按「代码附件」渲染成代码预览块而非文件卡片（用户明确反馈：复制 md 文件发送
/// 不应变成代码块）。
pub fn classify_file_subtype(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // 图片：所有主流格式（含移动端 iPhone 默认 HEIC/HEIF、Android 各种、无损 BMP/TIFF/AVIF）
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "bmp" | "tiff" | "tif"
        | "avif" | "apng" | "svg" | "ico" | "raw" | "dng" => "image",
        // 视频：移动端最常见（MP4/MOV/3GP/AVI/MKV/WebM）
        "mp4" | "mov" | "m4v" | "3gp" | "3gpp" | "avi" | "mkv" | "webm" | "flv" | "wmv" => "video",
        // 音频
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" => "audio",
        // 代码/文本（刻意排除 md/txt/log/Makefile — 用户明确反馈 md 文件发送应保持文件卡片）
        "rs" | "ts" | "tsx" | "js" | "jsx" | "vue" | "py" | "go" | "java" | "kt" | "c" | "cpp"
        | "cc" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "scala" | "r" | "pl" | "sh"
        | "bash" | "zsh" | "fish" | "ps1" | "bat" | "toml" | "json" | "yaml" | "yml" | "xml"
        | "html" | "htm" | "css" | "scss" | "less" | "sql" | "ini" | "cfg" | "conf" => "code",
        _ => "file",
    }
}

/// 避免重名：`a.txt` -> `a (1).txt`
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let base = dir.join(name);
    if !base.exists() {
        return base;
    }
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for i in 1..1000 {
        let cand = if ext.is_empty() {
            dir.join(format!("{stem} ({i})"))
        } else {
            dir.join(format!("{stem} ({i}).{ext}"))
        };
        if !cand.exists() {
            return cand;
        }
    }
    // 同名文件已达 999 个（异常）：退回随机后缀。
    // 旧实现在此直接 `return base`，而 base 必定已存在 → 静默覆盖用户已有文件。
    for _ in 0..32 {
        let token = STANDARD.encode(crypto::random_key());
        let cand = if ext.is_empty() {
            dir.join(format!("{stem}-{}", &token[..8]))
        } else {
            dir.join(format!("{stem}-{}.{ext}", &token[..8]))
        };
        if !cand.exists() {
            return cand;
        }
    }
    // 32 次随机后缀仍冲突：用完整随机串兜底（实际不可能发生）
    let token = STANDARD.encode(crypto::random_key());
    if ext.is_empty() {
        dir.join(format!("{stem}-{token}"))
    } else {
        dir.join(format!("{stem}-{token}.{ext}"))
    }
}

/// 文件名来自远端协议，必须只允许 basename，避免 `../` / Windows `\\` 穿越下载目录。
pub(crate) fn safe_file_name(name: &str) -> Option<String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
        return None;
    }
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(name.to_string())
}

/// `transfer_id` 同样来自远端协议，而且**会被直接拼进落盘路径**（`{transfer_id}.part`）——
/// 必须和 `safe_file_name` 同级校验，否则一个 `../../../../Users/me/Documents/x` 就能逃出
/// 下载目录，而 `File::create` 会**创建或截断**目标文件（内容由对端控制，
/// `Path::join` 遇到绝对路径还会整体替换前缀）。失败收尾路径同样会 `remove_file` 它。
///
/// 白名单而非黑名单：只接受 UUID / 测试用的连字符短 id 形态（`[A-Za-z0-9_-]{1,64}`）。
/// 生产端的 transfer_id 一律是 `Uuid::new_v4().to_string()`。
pub(crate) fn safe_transfer_id(id: &str) -> Option<String> {
    if id.is_empty() || id.len() > 64 {
        return None;
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(id.to_string())
}

/// 人类可读的文件大小。
#[allow(dead_code)]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

include!("file_tests.rs");
