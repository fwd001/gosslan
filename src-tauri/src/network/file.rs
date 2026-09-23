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
                tmp_path,
                final_path: final_path.clone(),
                peer_id: peer_id.to_string(),
                last_report_ms: crate::db::now_ms(),
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
pub fn decide_offer(
    has_active: bool,
    active_received: u64,
    disk_retained: u64,
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
            tmp_path: tmp_path.clone(),
            final_path: final_path.clone(),
            peer_id: peer_id.to_string(),
            last_report_ms: 0,
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

/// 接收方：写入一个分片，返回累计字节数。
/// 入参 `data` 为 AEAD 密文（nonce || ciphertext）：先解密再写盘，
/// 解密失败直接报错——密文绝不落盘。
pub fn write_chunk(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    seq: u32,
    data: &[u8],
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
        if plaintext.len() as u64 > r.size.saturating_sub(r.received) {
            return Err("文件分片超出声明大小".to_string());
        }
        // 文件级完整性：明文增量哈希（与写盘同一份数据，无二次磁盘读取）
        use sha2::Digest;
        r.hasher.update(&plaintext);
        r.file.write_all(&plaintext).map_err(|e| e.to_string())?;
        r.received += plaintext.len() as u64;
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

/// 终止损坏或超时的接收，删除临时文件，避免留下永远占空间的 `.part` 文件。
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
    // **保留 .part**（不删）：这是断点续传的前缀。只有"确定是永久失败"（校验不符）
    // 才删；超时/断链属于可恢复。陈旧 .part 由 resume_receive 的 TTL 与后续清理收割。
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
    emit_failed(state, transfer_id, reason);
    true
}

/// 对端断链时终止其所有未完成接收，避免下载目录长期堆积临时文件。
pub fn fail_receives_for_peer(state: &AppState, peer_id: &str) {
    let ids: Vec<String> = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, r)| r.peer_id == peer_id)
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        let _ = fail_receive(state, &id, peer_id, "对端连接已断开");
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
    if r.received != r.size {
        let _ = std::fs::remove_file(&r.tmp_path);
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
        return Err("文件传输未完成".to_string());
    }
    // 文件级完整性校验：实际 SHA-256 必须与发送方声明一致，否则不落盘
    {
        use sha2::Digest;
        let actual = r.hasher.clone().finalize();
        let actual_hex: String = actual.iter().map(|b| format!("{b:02x}")).collect();
        if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
            let _ = std::fs::remove_file(&r.tmp_path);
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
            return Err("文件完整性校验失败".to_string());
        }
    }
    if let Err(e) = r.file.sync_all() {
        let reason = e.to_string();
        let _ = std::fs::remove_file(&r.tmp_path);
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
        return Err(reason);
    }
    drop(r.file);
    if let Err(e) = std::fs::rename(&r.tmp_path, &r.final_path) {
        let reason = e.to_string();
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
        return Err(reason);
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
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
    Ok(Some((
        r.name.clone(),
        r.size,
        r.final_path.clone(),
        r.peer_id.clone(),
    )))
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

#[cfg(test)]
mod tests {
    use super::{
        chunk_seq_decision, classify_file_subtype, clear_file_wire_progress_in, derive_file_name,
        file_peer_key, safe_file_name, safe_transfer_id, unique_path, wire_progress_bytes,
        ChunkSeq, WireLedger,
    };

    /// 写出记账必须**随发送尝试一起回收**（v4.22.38）。
    ///
    /// 重点是"提前 return 那条路"：`stream_file` 有十来处早退（取消/链路关闭/加密失败/`?`），
    /// 旧写法只在成功路径清一次 ⇒ 失败后重试会读到上一次的 `chunks`，进度悄悄退回
    /// "按入队算"，也就是把 v4.22.37 那条修复抹掉。这里用本地表复现同一个 Drop 语义
    /// （造不出也不需要造 AppState —— 要验的就是"离开作用域就没残留"）。
    ///
    /// 键用的是 `file_peer_key(transfer, 收件人)`：回收的单位是"这一次给这个人的尝试"。
    #[test]
    fn wire_ledger_is_reclaimed_on_every_exit_path() {
        use crate::state::FileWireProgress;
        use std::collections::HashMap;
        use std::sync::Mutex;

        let table: Mutex<HashMap<String, FileWireProgress>> = Mutex::new(HashMap::new());
        let key = file_peer_key("t1", "peer-a");
        let attempt = |early: bool| -> Result<(), ()> {
            let _ledger = WireLedger {
                table: &table,
                wire_key: key.clone(),
            };
            // 模拟 writer 记账：这一片真的写出去了
            crate::network::transport::bump_file_wire_progress_in(&table, &key, 1);
            if early {
                return Err(());
            }
            Ok(())
        };

        assert!(attempt(false).is_ok());
        assert!(table.lock().unwrap().is_empty(), "正常结束必须回收这条记账");
        assert!(attempt(true).is_err());
        assert!(
            table.lock().unwrap().is_empty(),
            "提前 return 也必须回收 —— 旧写法漏的就是这一条"
        );
    }

    /// 同一次群投递里，每个收件人必须有**自己那份**写出记账（#35）。
    ///
    /// 为什么单独立一条：群发是 N 个任务共用一个 `transfer_id`（`group_file_dispatch.rs`），
    /// 键只按 id 记时两种坏行为都是静默的 ——
    ///   · `at_ms` 被任何一个人的写出刷新 ⇒ 真卡死的那个人永远判不出停滞；
    ///   · 进度按 `chunks` 折算 ⇒ 别人走过的量算进这一条链路，界面比实际快。
    /// 回收同理：谁先收尾就把整条传输的记录删掉，剩下还在写的人从此没有记账。
    #[test]
    fn wire_progress_is_counted_per_recipient_within_one_group_transfer() {
        use crate::state::FileWireProgress;
        use std::collections::HashMap;
        use std::sync::Mutex;

        let table: Mutex<HashMap<String, FileWireProgress>> = Mutex::new(HashMap::new());
        let a = file_peer_key("t1", "peer-a");
        let b = file_peer_key("t1", "peer-b");
        assert_ne!(a, b, "同一个 transfer 的两个收件人必须各自成键");
        assert_ne!(
            a,
            "t1".to_string(),
            "裸 transfer_id 不能是合法键 —— 否则新旧口径会互相读到"
        );

        for i in 0..3 {
            crate::network::transport::bump_file_wire_progress_in(&table, &a, i + 1);
        }
        crate::network::transport::bump_file_wire_progress_in(&table, &b, 9);

        let snap = table.lock().unwrap();
        assert_eq!(snap.get(&a).unwrap().chunks, 3);
        assert_eq!(
            snap.get(&b).unwrap().chunks,
            1,
            "甲走过的片数不许算到乙头上（进度与停滞判定都会偏）"
        );
        assert_eq!(snap.get(&b).unwrap().at_ms, 9, "各自的时刻也必须各自记");
        drop(snap);

        // 甲先收尾：只许删甲自己那一条。
        clear_file_wire_progress_in(&table, &a);
        let after = table.lock().unwrap();
        assert!(
            after.contains_key(&b),
            "回收必须按收件人，否则先结束的成员会把还在写的成员的记账删掉"
        );
        assert_eq!(after.get(&b).unwrap().chunks, 1);
    }

    /// 发送进度必须按"**已写出链路**"算，不按入队算（v4.22.37）。
    ///
    /// 症状（真机 600MB）：`send_on_link` 成功只代表进了那条链路的 1024 槽队列，
    /// LAN 一片 256KB ⇒ 最多 262MB 还在排队时界面已经 100%。①⑤ 就是这条主症状；
    /// ②③④ 各钉一个换算边界（续传前缀、短片、计数器残留）。
    #[test]
    fn progress_counts_written_chunks_not_enqueued_bytes() {
        let chunk = 256 * 1024usize;
        let enqueued = 1024 * chunk as u64; // 一整条队列都灌满了
                                            // ① 主症状：1024 片全入队、链路只走了 1 片 ⇒ 进度就是 1 片
        assert_eq!(
            wire_progress_bytes(0, chunk, enqueued, 1, enqueued * 2),
            chunk as u64
        );
        // ⑤ 一片都没写出 ⇒ 0（旧口径这里已经是 262MB）
        assert_eq!(wire_progress_bytes(0, chunk, enqueued, 0, enqueued * 2), 0);
        // ② 断点续传：from_bytes 是对端已持有的前缀，天然算"已上路"，必须计入
        assert_eq!(
            wire_progress_bytes(
                1_000_000,
                chunk,
                1_000_000 + 5 * chunk as u64,
                3,
                10_000_000
            ),
            1_000_000 + 3 * chunk as u64
        );
        // ③ 最后一片是短片：按整片折算会越过文件总大小 ⇒ 钳到 size
        let size = 3 * chunk as u64 + 10;
        assert_eq!(wire_progress_bytes(0, chunk, size, 4, size), size);
        // ④ 计数器残留得比本机读出来的还多 ⇒ 绝不能超过已入队量
        assert_eq!(wire_progress_bytes(0, chunk, 7, 999, 10_000), 7);
    }

    /// **收到分片的判定规则**（2026-09-13 审计的真缺陷，必须钉住）。
    ///
    /// 反例就是用户报的那条「文件分片顺序错误」：发送方重试时 `seq` 从 0 重来，
    /// 而旧实现把"重复/迟到"也当成致命错误 ⇒ 接收方整单失败 ⇒ 新 attempt 永远拼不齐。
    #[test]
    fn chunk_seq_rule_only_rejects_real_gaps() {
        // 正好下一片 ⇒ 写入
        assert_eq!(chunk_seq_decision(0, 0), ChunkSeq::Accept);
        assert_eq!(chunk_seq_decision(7, 7), ChunkSeq::Accept);
        // 重复 / 迟到（重传时上一轮的残片）⇒ **忽略**，不许失败
        assert_eq!(chunk_seq_decision(0, 3), ChunkSeq::Duplicate);
        assert_eq!(chunk_seq_decision(2, 3), ChunkSeq::Duplicate);
        // 跳号（中间真缺片）⇒ 报错，靠重传补齐
        assert_eq!(chunk_seq_decision(4, 3), ChunkSeq::Gap);
        // 边界：u32 极值也不 panic
        assert_eq!(chunk_seq_decision(u32::MAX, u32::MAX - 1), ChunkSeq::Gap);
    }

    #[test]
    fn image_extensions() {
        for n in ["a.png", "a.jpg", "a.jpeg", "a.gif", "a.webp"] {
            assert_eq!(classify_file_subtype(n), "image", "{n}");
        }
    }

    #[test]
    fn code_extensions() {
        for n in [
            "a.rs", "a.ts", "a.tsx", "a.js", "a.jsx", "a.vue", "a.py", "a.go", "a.java", "a.c",
            "a.cpp", "a.h", "a.hpp", "a.json", "a.yaml", "a.yml", "a.html", "a.css", "a.sql",
            "a.sh",
        ] {
            assert_eq!(classify_file_subtype(n), "code", "{n}");
        }
    }

    #[test]
    fn markdown_is_a_document_not_code() {
        // Markdown 是文档不是代码：发送 .md 文件应按文件卡片渲染，而不是代码预览块。
        assert_eq!(classify_file_subtype("a.md"), "file");
        assert_eq!(classify_file_subtype("README.MD"), "file");
    }

    #[test]
    fn other_files() {
        for n in [
            "a.exe", "a.zip", "a.pdf", "a.docx", "a.txt", "Makefile", "LICENSE",
        ] {
            assert_eq!(classify_file_subtype(n), "file", "{n}");
        }
    }

    #[test]
    fn case_insensitive_and_multidot_and_chinese() {
        assert_eq!(classify_file_subtype("PHOTO.PNG"), "image");
        assert_eq!(classify_file_subtype("App.Vue"), "code");
        assert_eq!(classify_file_subtype("archive.tar.gz"), "file"); // 末段 gz 不在清单
        assert_eq!(classify_file_subtype("min.bundle.js"), "code"); // 末段 js
        assert_eq!(classify_file_subtype("报告 截图.JPG"), "image"); // 中文名 + 空格
        assert_eq!(classify_file_subtype("代码.rs"), "code");
        assert_eq!(classify_file_subtype(".gitignore"), "file"); // 隐藏文件无有效扩展
        assert_eq!(classify_file_subtype(""), "file");
    }

    #[test]
    fn rejects_path_traversal_file_names() {
        for name in ["../secret.txt", "..\\secret.txt", "/tmp/secret", "..", ""] {
            assert!(safe_file_name(name).is_none(), "{name} must be rejected");
        }
        assert_eq!(safe_file_name("report.txt").as_deref(), Some("report.txt"));
    }

    #[test]
    fn derive_file_name_normal() {
        // 正常路径有扩展名 → 直接取
        assert_eq!(
            derive_file_name("/storage/emulated/0/DCIM/Camera/VID_001.mp4"),
            "VID_001.mp4"
        );
        assert_eq!(
            derive_file_name("/home/user/Downloads/report.pdf"),
            "report.pdf"
        );
    }

    #[test]
    fn derive_file_name_temporal_file_missing_ext() {
        // Tauri Android 临时文件：file_name() 没扩展名，但完整路径末尾有 .mp4
        assert_eq!(
            derive_file_name("content://media/external/video/media/123456/VID_20250918.mp4"),
            "VID_20250918.mp4"
        );
        assert_eq!(
            derive_file_name("/data/data/com.gosslan.app/cache/478812312.jpg"),
            "478812312.jpg"
        );
    }

    #[test]
    fn derive_file_name_no_ext_anywhere() {
        // 真的没有扩展名 → 原样返回
        assert_eq!(derive_file_name("/tmp/README"), "README");
        assert_eq!(derive_file_name("/tmp/478812312"), "478812312");
    }

    /// `transfer_id` 会被拼成 `{id}.part` 落盘，必须与文件名同级消毒。
    /// 未校验时一个 `../../../../Users/me/Documents/x` 就能让 `File::create`
    /// 在下载目录之外创建/截断文件（内容由对端控制）。
    #[test]
    fn rejects_path_traversal_transfer_ids() {
        for id in [
            "../../../../Users/me/Documents/report",
            "..\\..\\windows\\system32\\x",
            "/etc/passwd",
            "a/b",
            "a\\b",
            "..",
            ".",
            "",
            "with space",
            "null\0byte",
            "中文 id",
        ] {
            assert!(safe_transfer_id(id).is_none(), "{id:?} 必须被拒");
        }
        // 长度上限：超长 id 会成为超长文件名
        assert!(safe_transfer_id(&"a".repeat(65)).is_none());
        assert!(safe_transfer_id(&"a".repeat(64)).is_some());
    }

    /// 生产端用 UUID、E2E 用连字符短 id —— 合法形态一个都不能被误杀。
    #[test]
    fn accepts_real_world_transfer_ids() {
        for id in [
            "3f2504e0-4f89-11d3-9a0c-0305e82c3301", // Uuid::new_v4()
            "e2e-file-001",
            "e2e-group-image-001",
            "e2e-dl-001",
            "ABCdef123_-",
        ] {
            assert_eq!(safe_transfer_id(id).as_deref(), Some(id), "{id} 不应被拒");
        }
    }

    /// `unique_path` 在任何分支下都不得返回已存在的路径。
    /// 旧实现在「同名文件已达 999 个」时直接 `return base`（base 必定已存在），
    /// 会静默覆盖用户已有文件——这条测试锁定该兜底分支。
    #[test]
    fn unique_path_never_returns_existing_path() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("gosslan-uniquepath-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 无冲突：原样返回
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a.txt"));

        // 占满基准名与 1..999 全部候选名，逼出随机后缀分支
        fs::write(dir.join("a.txt"), b"x").unwrap();
        for i in 1..1000 {
            fs::write(dir.join(format!("a ({i}).txt")), b"x").unwrap();
        }
        let got = unique_path(&dir, "a.txt");
        assert!(!got.exists(), "返回了已存在的路径，会覆盖用户文件：{got:?}");
        assert_ne!(got, dir.join("a.txt"));

        // 无扩展名走同一分支
        fs::write(dir.join("README"), b"x").unwrap();
        for i in 1..1000 {
            fs::write(dir.join(format!("README ({i})")), b"x").unwrap();
        }
        let got2 = unique_path(&dir, "README");
        assert!(!got2.exists(), "返回了已存在的路径：{got2:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    // ---------- 文件传输 E2EE（协议层模拟，不依赖 AppState） ----------

    use super::super::super::crypto;
    use super::chunk_size_for_path;
    use super::{
        refuse_reason_for_best_link, send_deadline_for, sha256_file_hex, stall_verdict,
        valid_sha256_hex, StallVerdict, BLE_FILE_SIZE_LIMIT, FILE_SEND_DEADLINE,
        FILE_STALL_ABORT_MS, FILE_STALL_WARN_MS,
    };
    use crate::protocol::FILE_CHUNK;
    use std::time::Duration;

    /// **分块大小必须能真的被 BLE 分片层发出去**（真机 2026-09-13：大图两边都显示成功、
    /// 对方列表里却没有）。这条测试是**行为级**的：直接把两种分块大小喂给真正的
    /// `fragment()`，用默认 MTU（23 ⇒ 20 字节载荷 ⇒ 每片 14 字节）。
    ///
    /// 旧行为（256 KiB）在这一步会返回 `None` ⇒ 写循环把它当写失败并**拆掉整条链路**
    /// ⇒ 传输永远完不成，而发送方界面照样显示"已发送/已读"。
    #[test]
    fn ble_file_chunk_actually_fits_the_ble_fragment_layer() {
        use crate::transport::ble_framing::{fragment, MAX_BLE_CHUNKS_PER_MESSAGE};
        let mtu = 20; // MTU 23 - 3 字节 ATT 头

        // ① BLE 的分块大小必须能分片成功，且离上限有充足余量
        let ble_chunk = vec![0u8; chunk_size_for_path("bluetooth")];
        let chunks = fragment(&ble_chunk, mtu, 1)
            .expect("BLE 分块大小必须能被分片 —— 否则写循环会拆掉整条链路");
        assert!(
            chunks.len() * 4 <= MAX_BLE_CHUNKS_PER_MESSAGE,
            "分片数 {} 必须离上限 {} 有 ≥4× 余量",
            chunks.len(),
            MAX_BLE_CHUNKS_PER_MESSAGE
        );

        // ② 对照：桌面默认的 256 KiB 在小 MTU 上**分不出片**（这正是那个 bug 的形态）
        assert!(
            fragment(&vec![0u8; FILE_CHUNK], mtu, 1).is_none(),
            "256 KiB 在 MTU=23 上必然超过分片上限 —— 这条断言把 bug 的成因钉在测试里"
        );

        // ③ 非蓝牙链路仍用大块（局域网带宽高，小块会拖慢吞吐）
        assert_eq!(chunk_size_for_path("lan"), FILE_CHUNK);
        assert_eq!(chunk_size_for_path("routed"), FILE_CHUNK);
    }

    use base64::Engine as _;

    /// 在系统临时目录创建唯一的 .part 文件（测试接收端用），返回句柄与路径。
    fn temp_part(tag: &str) -> (std::fs::File, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-{tag}-{}.part", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let f = std::fs::File::create(&path).unwrap();
        (f, path)
    }

    /// 模拟接收端 write_chunk 的核心序列：AEAD 解密 → 增量哈希 → 写 .part。
    /// （write_chunk 本体需要 AppState，此处按相同操作序列驱动 FileReceiver。）
    fn receive_one_chunk(r: &mut crate::state::FileReceiver, seq: u32, sealed: &[u8]) {
        assert_eq!(seq, r.next_seq, "write_chunk 语义：seq 必须严格递增");
        use sha2::Digest;
        let plaintext = crypto::open_symmetric(&r.file_key, sealed).expect("解密失败");
        r.hasher.update(&plaintext);
        std::io::Write::write_all(&mut r.file, &plaintext).unwrap();
        r.received += plaintext.len() as u64;
        r.next_seq += 1;
    }

    /// 模拟 finish_receive 的最终裁决：size 一致 + SHA-256 一致才算完成。
    fn finish_verdict(r: &mut crate::state::FileReceiver) -> Result<(), String> {
        use sha2::Digest;
        if r.received != r.size {
            return Err("文件传输未完成".to_string());
        }
        let actual_hex: String = r
            .hasher
            .clone()
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
            return Err("文件完整性校验失败".to_string());
        }
        Ok(())
    }

    fn hex_of(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 1. FileOffer.sealed_file_key：发送方以接收方公钥封装、接收方解封，
    ///    必须还原出同一个文件会话密钥（ECDH 对称性）。
    #[test]
    fn file_offer_sealed_key_roundtrip() {
        let sender = crypto::Identity::generate();
        let receiver = crypto::Identity::generate();
        let file_key = crypto::random_key();

        // 发送端（send_file_from_path 同逻辑）：receiver 公钥封装
        let shared = crypto::shared_secret(&sender.x25519_secret, &receiver.x25519_public_b64())
            .expect("ECDH 失败");
        let sealed_key_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal(&shared, &file_key).expect("封装失败"));

        // 接收端（handle_message FileOffer 同逻辑）：sender 公钥解封
        let shared_rx = crypto::shared_secret(&receiver.x25519_secret, &sender.x25519_public_b64())
            .expect("ECDH 失败");
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&sealed_key_b64)
            .expect("base64 非法");
        let opened = crypto::open(&shared_rx, &sealed).expect("解封失败");
        assert_eq!(opened.len(), 32);
        assert_eq!(opened, file_key, "解封出的文件会话密钥必须与原密钥一致");
    }

    /// 2. FileChunk 加密→解密 roundtrip：原始 bytes 完整还原。
    #[test]
    fn file_chunk_encrypt_roundtrip() {
        let file_key = crypto::random_key();
        let plaintext: Vec<u8> = (0u8..=255).cycle().take(FILE_CHUNK).collect();
        let sealed = crypto::seal_symmetric(&file_key, &plaintext).expect("加密失败");
        let opened = crypto::open_symmetric(&file_key, &sealed).expect("解密失败");
        assert_eq!(opened, plaintext);
    }

    /// 3. 密文被篡改后解密必须失败（AEAD 完整性），不能产出可用明文。
    #[test]
    fn tampered_chunk_fails_to_decrypt() {
        let file_key = crypto::random_key();
        let plaintext = b"gosslan file chunk";
        let mut sealed = crypto::seal_symmetric(&file_key, plaintext).expect("加密失败");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF; // 翻转密文最后一比特
        assert!(
            crypto::open_symmetric(&file_key, &sealed).is_none(),
            "篡改后的密文必须解密失败"
        );
    }

    /// 4. 每个 transfer 生成独立的随机文件会话密钥，不得共用。
    #[test]
    fn distinct_transfers_have_distinct_keys() {
        let a = crypto::random_key();
        let b = crypto::random_key();
        assert_ne!(a, b, "两次 random_key() 必须产生不同密钥（CSPRNG）");
        // 密文互换后必须解不开：证明密钥确实互不通用
        let msg = b"content of transfer";
        let sealed_with_a = crypto::seal_symmetric(&a, msg).unwrap();
        assert!(crypto::open_symmetric(&b, &sealed_with_a).is_none());
    }

    /// 5. 中继节点原样转发密文（RelayChunk 只透传 data），
    ///    接收端用自己解封的会话密钥仍可解密——中继无需也无法解密。
    #[test]
    fn relay_forwarded_ciphertext_still_decryptable() {
        let sender = crypto::Identity::generate();
        let receiver = crypto::Identity::generate();
        let file_key = crypto::random_key();

        // 发送端：封装会话密钥 + 加密 chunk
        let shared =
            crypto::shared_secret(&sender.x25519_secret, &receiver.x25519_public_b64()).unwrap();
        let sealed_key_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal(&shared, &file_key).unwrap());
        let plaintext = b"chunk travels through relay nodes";
        let ciphertext_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal_symmetric(&file_key, plaintext).unwrap());

        // 模拟中继：data 原样透传（无密钥、无修改）——中继不可见明文
        let forwarded = ciphertext_b64.clone();

        // 接收端：解封密钥 → 解密转发的密文
        let shared_rx =
            crypto::shared_secret(&receiver.x25519_secret, &sender.x25519_public_b64()).unwrap();
        let opened_key: [u8; 32] = crypto::open(
            &shared_rx,
            &base64::engine::general_purpose::STANDARD
                .decode(&sealed_key_b64)
                .unwrap(),
        )
        .unwrap()
        .try_into()
        .unwrap();
        let decrypted = crypto::open_symmetric(
            &opened_key,
            &base64::engine::general_purpose::STANDARD
                .decode(&forwarded)
                .unwrap(),
        )
        .expect("中继转发后的密文必须仍可解密");
        assert_eq!(decrypted, plaintext);
    }

    /// 6. 完整传输生命周期（协议层）：解封密钥 → 多分片逐片加解密 →
    ///    拼接还原 + 大小校验通过——与 finish_receive 的裁决一致。
    #[test]
    fn full_transfer_lifecycle_still_completes() {
        let sender = crypto::Identity::generate();
        let receiver = crypto::Identity::generate();

        // 原始文件：3 片（末片不满 256KB，覆盖边界）
        let mut original: Vec<u8> = Vec::new();
        for i in 0..(FILE_CHUNK * 3 - 1234) {
            original.push((i % 251) as u8);
        }
        let chunks: Vec<&[u8]> = original.chunks(FILE_CHUNK).collect();

        // 发送端生命周期：random_key → 封装 → 逐片加密
        let file_key = crypto::random_key();
        let shared =
            crypto::shared_secret(&sender.x25519_secret, &receiver.x25519_public_b64()).unwrap();
        let sealed_key_b64 = base64::engine::general_purpose::STANDARD
            .encode(crypto::seal(&shared, &file_key).unwrap());
        let wire_chunks: Vec<Vec<u8>> = chunks
            .iter()
            .map(|c| crypto::seal_symmetric(&file_key, c).unwrap())
            .collect();

        // 接收端生命周期：解封密钥 → 逐片解密重组 → 大小校验
        let shared_rx =
            crypto::shared_secret(&receiver.x25519_secret, &sender.x25519_public_b64()).unwrap();
        let restored_key: [u8; 32] = crypto::open(
            &shared_rx,
            &base64::engine::general_purpose::STANDARD
                .decode(&sealed_key_b64)
                .unwrap(),
        )
        .unwrap()
        .try_into()
        .unwrap();
        let mut assembled: Vec<u8> = Vec::new();
        for (seq, wire) in wire_chunks.iter().enumerate() {
            // seq 严格递增校验（write_chunk 语义）：乱序片在这里被拒绝
            assert_eq!(seq as usize, assembled.chunks(FILE_CHUNK).count());
            let plain = crypto::open_symmetric(&restored_key, wire)
                .unwrap_or_else(|| panic!("分片 {seq} 解密失败"));
            assembled.extend_from_slice(&plain);
        }
        assert_eq!(assembled.len(), original.len(), "重组大小必须一致");
        assert_eq!(assembled, original, "重组内容必须与原文件一致");
    }

    // ---------- 文件级 SHA-256 完整性校验 ----------

    /// 只剩蓝牙链路时，不许启动一件"注定完不成"的大文件（2026-09-23 真机 600MB 复核）。
    ///
    /// 钉的是**判据**而不是接线：BLE 上分片被压到 4KiB、带宽 ≈14KB/s，600MB 要十几小时，
    /// 而单轮 deadline 封顶 1h ⇒ 必然反复超窗重投 ⇒ 重新选路 + 重新编号 ⇒ 接收端 Gap 判死，
    /// 并且 `file_sending` 按 peer 去重，会把同 peer 的其它文件一起堵死。
    #[test]
    fn ble_only_link_must_not_start_a_hopeless_large_file() {
        use crate::mesh::PathKind::*;
        let mb = 1024 * 1024;
        // 阈值内照发：这道闸不是"BLE 上一律不发文件"，那会牺牲既有的小图能力。
        assert_eq!(refuse_reason_for_best_link(Some(Bluetooth), 4 * mb), None);
        assert_eq!(
            refuse_reason_for_best_link(Some(Bluetooth), BLE_FILE_SIZE_LIMIT),
            None,
            "边界取「不超过就发」，别把阈值当成开区间悄悄改语义"
        );
        // 超阈值 ⇒ 不启动，且原因是给用户看的句子（要能看出是"等更好的链路"不是故障）
        let reason = refuse_reason_for_best_link(Some(Bluetooth), 600 * mb);
        assert!(reason.is_some(), "600MB 在只有蓝牙时必须拒绝启动");
        assert!(
            reason.unwrap().contains("蓝牙"),
            "原因必须点名是哪条链路不适合，不能只说“发送失败”"
        );
        // 只要还有别的链路可选就与蓝牙无关（判据只看**最佳**链路，不看是否存在蓝牙）
        assert_eq!(refuse_reason_for_best_link(Some(Lan), 600 * mb), None);
        assert_eq!(refuse_reason_for_best_link(Some(Routed), 2048 * mb), None);
        assert_eq!(refuse_reason_for_best_link(Some(Relay), 2048 * mb), None);
        // 完全没有链路时这里不表态（调用方另有 has_link 分支，两处不许互相抢判据）
        assert_eq!(refuse_reason_for_best_link(None, 600 * mb), None);
    }

    /// 大文件的发送期限必须随体积伸缩，**且按线上字节估**（2026-09-23 真机 600MB 复核）。
    /// 两个真机根因都钉在这里：固定 10min 窗口 = 必失败；按明文估 = 少给 25% 窗口 ⇒
    /// 慢链路上单轮注定超窗 ⇒ 只能靠重投 + `.part` 接力 ⇒ 撞上接收端的 Gap 判死。
    #[test]
    fn send_deadline_scales_with_size() {
        assert_eq!(
            send_deadline_for(1024),
            FILE_SEND_DEADLINE,
            "小文件保持 10min 下限"
        );
        // 600MiB：线上 = ×4/3 = 800MiB ⇒ 800MiB ÷ 512KiB/s = 1600s，+60s 余量 = 1660s。
        // 明文口径只会给 1260s（21min），所以这个精确值同时钉住了"不许退回明文估算"。
        assert_eq!(
            send_deadline_for(600 * 1024 * 1024),
            Duration::from_secs(1660),
            "600MiB 的窗口必须按线上字节算（明文口径是 1260s）"
        );
        assert!(
            send_deadline_for(600 * 1024 * 1024) > Duration::from_secs(25 * 60),
            "512KiB/s 下 600MiB 实需 26.7min，窗口必须容得下"
        );
        assert_eq!(
            send_deadline_for(u64::MAX),
            Duration::from_secs(60 * 60),
            "再大也封顶 1h，超出交给断点续传重试而不是吊死任务"
        );
    }

    /// 停滞判定的三档边界。钉的是"什么时候该提醒、什么时候该放弃"，
    /// 阈值本身写死在常量里，改常量必须同时改这里（防止有人顺手把 abort 调成 warn）。
    #[test]
    fn stall_verdict_boundaries() {
        assert_eq!(stall_verdict(0), StallVerdict::Healthy);
        assert_eq!(
            stall_verdict(FILE_STALL_WARN_MS - 1),
            StallVerdict::Healthy,
            "还没到提醒线不得提前吓用户"
        );
        assert_eq!(stall_verdict(FILE_STALL_WARN_MS), StallVerdict::Warn);
        assert_eq!(
            stall_verdict(FILE_STALL_ABORT_MS - 1),
            StallVerdict::Warn,
            "提醒与放弃之间只有 Warn"
        );
        assert_eq!(stall_verdict(FILE_STALL_ABORT_MS), StallVerdict::Abort);
        assert_eq!(stall_verdict(i64::MAX), StallVerdict::Abort);
        assert!(
            FILE_STALL_WARN_MS < FILE_STALL_ABORT_MS,
            "提醒必须早于放弃，否则用户只看到突然失败"
        );
        assert!(
            FILE_STALL_ABORT_MS < FILE_SEND_DEADLINE.as_millis() as i64,
            "停滞放弃要早于最短 deadline，否则 deadline 才是唯一出口（界面会冻住十分钟）"
        );
    }

    /// sha256_file_hex：流式分块结果必须与一次性内存计算一致（发送端正确性）。
    #[test]
    fn sha256_file_hex_matches_in_memory_hash() {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-sha-{}.bin", std::process::id()));
        std::fs::write(&path, b"gosslan sha-256 streaming test body").unwrap();
        let got = sha256_file_hex(&path).unwrap();
        let want = hex_of(b"gosslan sha-256 streaming test body");
        let _ = std::fs::remove_file(&path);
        assert_eq!(got, want);
        assert_eq!(got.len(), 64, "hex 表示必须为 64 字符");
    }

    /// SHA-256 hex 字段格式校验（FileOffer 元数据），非法即拒绝。
    #[test]
    fn invalid_sha256_format_is_rejected() {
        assert!(valid_sha256_hex(&hex_of(b"ok")));
        assert!(
            valid_sha256_hex(&hex_of(b"ok").to_uppercase()),
            "大写 hex 也合法"
        );
        assert!(!valid_sha256_hex(""), "空串");
        assert!(!valid_sha256_hex("abc"), "长度不足");
        assert!(!valid_sha256_hex(&"a".repeat(63)), "63 位");
        assert!(!valid_sha256_hex(&"a".repeat(65)), "65 位");
        assert!(
            !valid_sha256_hex(&format!("{}g", "a".repeat(63))),
            "非 hex 字符"
        );
    }

    /// 空文件边界：SHA-256 已知值 + sha256_file_hex 对 0 字节文件正确。
    #[test]
    fn empty_file_sha256_matches_known_value() {
        let path =
            std::env::temp_dir().join(format!("gosslan-test-empty-{}.bin", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let got = sha256_file_hex(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            got, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "空文件 SHA-256 必须是标准已知值"
        );
    }

    /// 取消登记必须**按收件人分键**（真机：三成员以上群文件只有一人收得到）。
    ///
    /// 钉的判据：单键时后注册的 `insert` 挤掉前一个任务的 `Sender`，对方的 oneshot
    /// 立刻 `Err(RecvError)` 完成，而投递循环的取消分支分不清"被取消"与"登记被顶替"
    /// ⇒ N-1 个成员以「用户取消发送」这个假原因当场中断。
    #[test]
    fn file_cancel_keys_are_scoped_per_recipient() {
        let a = super::file_cancel_key("t1", "peer-a");
        let b = super::file_cancel_key("t1", "peer-b");
        assert_ne!(a, b, "同一 transfer_id 的不同收件人必须各自成键");
        let prefix = super::file_cancel_prefix("t1");
        assert!(
            a.starts_with(prefix.as_str()) && b.starts_with(prefix.as_str()),
            "取消入口要能按前缀一次命中该 transfer 的全部在途流"
        );
        assert!(
            !super::file_cancel_key("t11", "peer-a").starts_with(prefix.as_str()),
            "前缀匹配不得误伤 id 恰好同前缀的兄弟传输"
        );
        assert_eq!(
            a.split('\u{0}').count(),
            2,
            "键必须恰好 transfer_id + recipient 两段"
        );
    }

    /// 正常文件：多分片经「解密 → 增量哈希 → 写盘」后，最终 SHA-256 一致 → 完成。
    #[test]
    fn receiver_hash_lifecycle_success() {
        let original: Vec<u8> = (0..FILE_CHUNK * 2 + 777u32 as usize)
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = hex_of(&original);
        let file_key = crypto::random_key();

        let (f, part_path) = temp_part("ok");
        let mut r = crate::state::FileReceiver {
            file: f,
            name: "ok.bin".into(),
            size: original.len() as u64,
            received: 0,
            next_seq: 0,
            tmp_path: part_path.clone(),
            final_path: part_path.clone(),
            peer_id: "a".into(),
            last_report_ms: 0,
            file_key,
            expected_sha256: expected.clone(),
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        };

        for (seq, chunk) in original.chunks(FILE_CHUNK).enumerate() {
            let sealed = crypto::seal_symmetric(&file_key, chunk).unwrap();
            receive_one_chunk(&mut r, seq as u32, &sealed);
        }
        assert!(finish_verdict(&mut r).is_ok(), "内容一致时校验必须通过");
        let _ = std::fs::remove_file(&part_path);
    }

    /// 篡改某个明文分片：最终 SHA-256 不一致 → failed（不得视为完成）。
    #[test]
    fn receiver_hash_mismatch_fails() {
        let original: Vec<u8> = (0..FILE_CHUNK + 100u32 as usize)
            .map(|i| (i % 199) as u8)
            .collect();
        let expected = hex_of(&original);
        let file_key = crypto::random_key();

        let (f, part_path) = temp_part("bad");
        let mut r = crate::state::FileReceiver {
            file: f,
            name: "bad.bin".into(),
            size: original.len() as u64,
            received: 0,
            next_seq: 0,
            tmp_path: part_path.clone(),
            final_path: part_path.clone(),
            peer_id: "a".into(),
            last_report_ms: 0,
            file_key,
            expected_sha256: expected,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        };

        for (seq, chunk) in original.chunks(FILE_CHUNK).enumerate() {
            let mut plain = chunk.to_vec();
            if seq == 0 {
                plain[0] ^= 0x01; // 篡改首片一个比特
            }
            let sealed = crypto::seal_symmetric(&file_key, &plain).unwrap();
            receive_one_chunk(&mut r, seq as u32, &sealed);
        }
        let verdict = finish_verdict(&mut r);
        assert_eq!(verdict.unwrap_err(), "文件完整性校验失败");
        let _ = std::fs::remove_file(&part_path);
    }

    /// relay 场景（2026-09-23 审计 1.8 修复后）：最终接收方逐片解密，重组完成后
    /// 对按 seq 组装的明文**一次性**算 SHA-256；中继只透传密文，不参与哈希。
    #[test]
    fn relay_receiver_hash_lifecycle_success() {
        use crate::state::RelayFileReceive;
        let original: Vec<u8> = (0..FILE_CHUNK + 500u32 as usize)
            .map(|i| (i % 241) as u8)
            .collect();
        let expected = hex_of(&original);
        let file_key = crypto::random_key();

        let rs = RelayFileReceive {
            file_key,
            expected_sha256: expected,
            created_at: crate::db::now_ms(),
        };

        // 模拟 handle_relay_chunk 的接收路径：解密 → add_chunk（去重 + 按 seq 组装）。
        // 分片**故意乱序到达且 seq=0 重复投递一次**（多邻居泛洪 + 多路径时延不同
        // 是该链路的常态）—— 修复前的增量哈希在这两种情况下都会算错，导致
        // 「分片齐了却报文件完整性校验失败」，发送端却显示成功。
        let mut relay = crate::file_relay::RelayManager::new();
        relay.begin_reassemble("t", "f.bin", 2, original.len() as u64);
        let sealed: Vec<Vec<u8>> = original
            .chunks(FILE_CHUNK)
            .map(|c| crypto::seal_symmetric(&rs.file_key, c).unwrap())
            .collect();
        // 乱序：先到 seq=1，再到 seq=0（此刻重组完成），然后 seq=0 再来一份（重复）
        let mut completed: Option<(String, u64, Vec<u8>)> = None;
        for (seq, s) in [(1u32, &sealed[1]), (0, &sealed[0]), (0, &sealed[0])] {
            let plain = crypto::open_symmetric(&rs.file_key, s).unwrap();
            if let Some(done) = relay.add_chunk("t", seq, plain) {
                completed = Some(done);
            }
        }
        let Some((_, _, full)) = completed else {
            panic!("三条分片后必须完成重组");
        };
        assert_eq!(full, original, "乱序+重复到达也要组装出原始明文");

        // 修复后的校验点：对组装结果一次性算哈希
        use sha2::Digest;
        let actual_hex: String = sha2::Sha256::digest(&full)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(
            actual_hex.eq_ignore_ascii_case(&rs.expected_sha256),
            "乱序+重复分片场景最终校验必须通过"
        );
    }

    // ---------- 群文件 session key（GroupFileOffer 阶段） ----------

    /// 5. file_key 用 GroupKey seal/open round-trip：群内成员可解封。
    #[test]
    fn group_file_key_roundtrip_with_group_key() {
        let group_key = crypto::random_key();
        let file_key = crypto::random_key();
        let sealed = crypto::seal_symmetric(&group_key, &file_key).unwrap();
        let opened: [u8; 32] = crypto::open_symmetric(&group_key, &sealed)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(opened, file_key);
    }

    /// 6. 错误 GroupKey / 篡改密文 → 解封失败（群外与篡改者无法获得 file_key）。
    #[test]
    fn group_file_key_rejects_wrong_key_or_tampered_ciphertext() {
        let group_key = crypto::random_key();
        let wrong_key = crypto::random_key();
        let file_key = crypto::random_key();
        let sealed = crypto::seal_symmetric(&group_key, &file_key).unwrap();

        // 错误群密钥（群外 peer 用自己的“群密钥”）
        assert!(crypto::open_symmetric(&wrong_key, &sealed).is_none());
        // 篡改密文
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        assert!(crypto::open_symmetric(&group_key, &tampered).is_none());
    }

    /// 9. 一个 transfer 的多个 recipient 使用同一个 file session key：
    ///    同一份 sealed 密文被每个成员解封，得到同一个 file_key。
    #[test]
    fn all_recipients_share_one_session_key() {
        let group_key = crypto::random_key(); // 全体成员相同的群密钥
        let file_key = crypto::random_key();
        let sealed = crypto::seal_symmetric(&group_key, &file_key).unwrap();

        let mut opened_keys = Vec::new();
        for _recipient in ["b", "c", "d"] {
            let opened: [u8; 32] = crypto::open_symmetric(&group_key, &sealed)
                .unwrap()
                .try_into()
                .unwrap();
            opened_keys.push(opened);
        }
        assert!(opened_keys.iter().all(|k| *k == file_key));
    }

    /// 4. 每个 transfer 生成独立的随机 file session key（群文件版断言）。
    #[test]
    fn group_file_keys_distinct_across_transfers() {
        let k1 = crypto::random_key();
        let k2 = crypto::random_key();
        assert_ne!(k1, k2);
        let msg = b"group file content";
        let sealed = crypto::seal_symmetric(&k1, msg).unwrap();
        assert!(crypto::open_symmetric(&k2, &sealed).is_none());
    }

    // ---------- GroupFileChunk（协议 + 接收语义） ----------

    use crate::protocol::Message;

    /// 1+2. GroupFileChunk JSON round-trip：字段完整保留（serde tag + 字段名）。
    #[test]
    fn group_file_chunk_roundtrip_preserves_fields() {
        let msg = Message::GroupFileChunk {
            transfer_id: "gf-1".into(),
            group_id: "g-1".into(),
            sender_id: "dev-a".into(),
            seq: 7,
            data: "bm9uY2UrY2lwaGVydGV4dA==".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"group_file_chunk\""),
            "serde tag 必须是 group_file_chunk"
        );
        assert!(json.contains("\"transfer_id\":\"gf-1\""));
        assert!(json.contains("\"group_id\":\"g-1\""));
        assert!(json.contains("\"sender_id\":\"dev-a\""));
        assert!(json.contains("\"seq\":7"));
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::GroupFileChunk {
                transfer_id,
                group_id,
                sender_id,
                seq,
                data,
            } => {
                assert_eq!(transfer_id, "gf-1");
                assert_eq!(group_id, "g-1");
                assert_eq!(sender_id, "dev-a");
                assert_eq!(seq, 7);
                assert_eq!(data, "bm9uY2UrY2lwaGVydGV4dA==");
            }
            _ => panic!("应为 GroupFileChunk"),
        }
    }

    /// 6. 同一 file_key 加密同一明文两次 → 密文不同（每片独立随机 nonce，无重用）。
    #[test]
    fn group_chunks_use_distinct_nonces() {
        let file_key = crypto::random_key();
        let plaintext = vec![42u8; 1024];
        let c1 = crypto::seal_symmetric(&file_key, &plaintext).unwrap();
        let c2 = crypto::seal_symmetric(&file_key, &plaintext).unwrap();
        assert_ne!(c1, c2, "随机 nonce 下相同明文的两次密文必须不同");
        // 但都能解回同一明文
        assert_eq!(crypto::open_symmetric(&file_key, &c1).unwrap(), plaintext);
        assert_eq!(crypto::open_symmetric(&file_key, &c2).unwrap(), plaintext);
    }

    /// 构造群文件接收状态的测试 helper（与 handle_group_file_chunk 写入序列一致）。
    fn group_receiver(
        tag: &str,
        size: u64,
        expected: String,
        file_key: [u8; 32],
    ) -> crate::state::FileReceiver {
        let (f, part_path) = temp_part(tag);
        crate::state::FileReceiver {
            file: f,
            name: format!("{tag}.bin"),
            size,
            received: 0,
            next_seq: 0,
            tmp_path: part_path.clone(),
            final_path: part_path,
            peer_id: "dev-a".into(),
            last_report_ms: 0,
            file_key,
            expected_sha256: expected,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        }
    }

    /// 模拟 handle_group_file_chunk 的单分片处理：seq 校验 → 解密 → 大小校验 → 哈希/写盘。
    /// 返回 Err 表示该分片被拒绝（调用方应终止接收）。
    fn receive_group_chunk(
        r: &mut crate::state::FileReceiver,
        seq: u32,
        sealed: &[u8],
    ) -> Result<f64, String> {
        use std::io::Write;
        if seq != r.next_seq {
            return Err("分片顺序错误".to_string());
        }
        let plaintext = crypto::open_symmetric(&r.file_key, sealed)
            .ok_or_else(|| "分片解密失败".to_string())?;
        if plaintext.len() as u64 > r.size.saturating_sub(r.received) {
            return Err("超出声明大小".to_string());
        }
        use sha2::Digest;
        r.hasher.update(&plaintext);
        r.file.write_all(&plaintext).map_err(|e| e.to_string())?;
        r.received += plaintext.len() as u64;
        r.next_seq += 1;
        Ok(if r.size == 0 {
            1.0
        } else {
            (r.received as f64 / r.size as f64).min(1.0)
        })
    }

    /// 8+11+17+20. seq 0→1→2 正常、明文写入 `.part`、进度递增且不超过 1.0。
    #[test]
    fn group_receive_seq_progress_and_part_writes() {
        let original: Vec<u8> = (0..1024).map(|i| (i % 97) as u8).collect();
        let file_key = crypto::random_key();
        let mut r = group_receiver("seq-ok", original.len() as u64, hex_of(&original), file_key);

        let mut last_progress = 0.0;
        for (seq, chunk) in original.chunks(400).enumerate() {
            let sealed = crypto::seal_symmetric(&file_key, chunk).unwrap();
            let progress = receive_group_chunk(&mut r, seq as u32, &sealed).unwrap();
            assert!(progress > last_progress && progress <= 1.0);
            last_progress = progress;
        }
        assert_eq!(r.received, original.len() as u64);
        assert_eq!(r.next_seq, 3);
        assert_eq!(last_progress, 1.0);
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    /// 9+10. 跳号（0→2）与重复 seq（0→0）都被拒绝。
    #[test]
    fn group_receive_rejects_gap_and_duplicate_seq() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("seq-bad", 4096, hex_of(b"whatever"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"chunk0").unwrap();

        // seq 0 成功
        receive_group_chunk(&mut r, 0, &sealed).unwrap();
        // 跳号 2 → 拒绝
        assert!(receive_group_chunk(&mut r, 2, &sealed).is_err());
        // 重复 0 → 拒绝
        assert!(receive_group_chunk(&mut r, 0, &sealed).is_err());
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    /// 12. 分片总明文超过声明大小 → 拒绝（防恶意 sender 溢出写）。
    #[test]
    fn group_receive_rejects_oversize() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("oversize", 10, hex_of(b"0123456789"), file_key);
        // 第一片 6 字节 OK
        let s0 = crypto::seal_symmetric(&file_key, b"012345").unwrap();
        receive_group_chunk(&mut r, 0, &s0).unwrap();
        // 第二片 6 字节：6+6 > 10 → 拒绝
        let s1 = crypto::seal_symmetric(&file_key, b"abcdef").unwrap();
        assert!(receive_group_chunk(&mut r, 1, &s1).is_err());
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    /// 14（AEAD 面）. 错误 file_key 解密失败 → 调用方终止接收（返回 Err）。
    #[test]
    fn group_receive_rejects_wrong_file_key() {
        let right_key = crypto::random_key();
        let wrong_key = crypto::random_key();
        // 接收端持有 wrong_key（模拟 session key 不匹配）
        let mut r = group_receiver("wrong-key", 1024, hex_of(b"x"), wrong_key);
        let sealed = crypto::seal_symmetric(&right_key, b"secret chunk").unwrap();
        // 接收端持有 wrong_key：解密失败 → handle_group_file_chunk 走 fail 收尾
        assert!(receive_group_chunk(&mut r, 0, &sealed).is_err());
        let _ = std::fs::remove_file(&r.tmp_path);
    }

    // ---------- GroupFileDone（最终校验 + 正式文件落盘） ----------

    use crate::protocol::Message as ProtocolMessage;

    /// 1. GroupFileDone JSON round-trip：字段完整保留。
    #[test]
    fn group_file_done_roundtrip_preserves_fields() {
        let msg = ProtocolMessage::GroupFileDone {
            transfer_id: "gf-1".into(),
            group_id: "g-1".into(),
            sender_id: "dev-a".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"group_file_done\""),
            "serde tag 必须是 group_file_done"
        );
        assert!(json.contains("\"transfer_id\":\"gf-1\""));
        assert!(json.contains("\"group_id\":\"g-1\""));
        assert!(json.contains("\"sender_id\":\"dev-a\""));
        let back: ProtocolMessage = serde_json::from_str(&json).unwrap();
        match back {
            ProtocolMessage::GroupFileDone {
                transfer_id,
                group_id,
                sender_id,
            } => {
                assert_eq!(transfer_id, "gf-1");
                assert_eq!(group_id, "g-1");
                assert_eq!(sender_id, "dev-a");
            }
            _ => panic!("应为 GroupFileDone"),
        }
    }

    /// 模拟 handle_group_file_done 的最终校验与落盘序列：
    /// size → SHA-256 → sync_all → drop(file) → rename（与生产代码同序）。
    fn finalize_group_receive(r: crate::state::FileReceiver) -> Result<std::path::PathBuf, String> {
        if r.received != r.size {
            let _ = std::fs::remove_file(&r.tmp_path);
            return Err("文件传输未完成".to_string());
        }
        {
            use sha2::Digest;
            let actual_hex: String = r
                .hasher
                .clone()
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
                let _ = std::fs::remove_file(&r.tmp_path);
                return Err("文件完整性校验失败".to_string());
            }
        }
        r.file.sync_all().map_err(|e| {
            let _ = std::fs::remove_file(&r.tmp_path);
            e.to_string()
        })?;
        drop(r.file);
        std::fs::rename(&r.tmp_path, &r.final_path).map_err(|e| {
            let _ = std::fs::remove_file(&r.tmp_path);
            e.to_string()
        })?;
        Ok(r.final_path)
    }

    /// 2+3+4. 正常多 chunk + Done：SHA-256 正确 → rename 成功 → 正式文件内容一致，
    /// progress 对应 1.0 / completed 语义。
    #[test]
    fn group_done_success_renames_part() {
        let original: Vec<u8> = (0..2048).map(|i| (i % 173) as u8).collect();
        let file_key = crypto::random_key();
        let mut r = group_receiver(
            "done-ok",
            original.len() as u64,
            hex_of(&original),
            file_key,
        );
        for (seq, chunk) in original.chunks(700).enumerate() {
            let sealed = crypto::seal_symmetric(&file_key, chunk).unwrap();
            receive_group_chunk(&mut r, seq as u32, &sealed).unwrap();
        }
        assert_eq!(
            r.received as f64 / r.size as f64,
            1.0,
            "progress 必须为 1.0"
        );

        let final_path = finalize_group_receive(r).expect("最终校验应通过");
        assert!(!final_path.as_os_str().is_empty());
        let saved = std::fs::read(&final_path).unwrap();
        assert_eq!(saved, original, "正式文件内容必须与原文件一致");
        let _ = std::fs::remove_file(&final_path);
    }

    /// 5. received < size → failed（不 rename、删 .part）。
    #[test]
    fn group_done_short_receive_fails() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("done-short", 1024, hex_of(b"0123456789"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"012345").unwrap();
        receive_group_chunk(&mut r, 0, &sealed).unwrap(); // 只收 6 字节 < 1024

        let part = r.tmp_path.clone();
        let err = finalize_group_receive(r).unwrap_err();
        assert_eq!(err, "文件传输未完成");
        assert!(!part.exists(), "失败后 .part 必须被删除");
    }

    /// 7. SHA-256 mismatch → failed + .part 删除（绝不 rename）。
    #[test]
    fn group_done_sha_mismatch_fails_and_cleans_part() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("done-mismatch", 8, hex_of(b"deadbeef"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"content8").unwrap();
        receive_group_chunk(&mut r, 0, &sealed).unwrap(); // size 对但内容不同

        let part = r.tmp_path.clone();
        let err = finalize_group_receive(r).unwrap_err();
        assert_eq!(err, "文件完整性校验失败");
        assert!(!part.exists(), "SHA 不匹配后 .part 必须被删除");
    }

    /// 9. rename 失败 → failed（final_path 非法/被占用），不报告完成。
    #[test]
    fn group_done_rename_failure_fails() {
        let file_key = crypto::random_key();
        let mut r = group_receiver("done-rename", 4, hex_of(b"data"), file_key);
        let sealed = crypto::seal_symmetric(&file_key, b"data").unwrap();
        receive_group_chunk(&mut r, 0, &sealed).unwrap();

        // final_path 指向一个已存在的目录 → rename 必然失败
        let dir = std::env::temp_dir().join(format!("gosslan-test-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        r.final_path = dir.clone();

        let part = r.tmp_path.clone();
        assert!(finalize_group_receive(r).is_err());
        assert!(!part.exists(), "rename 失败后 .part 必须被清理");
        let _ = std::fs::remove_dir(&dir);
    }

    /// 19. 空文件：无 Chunk，Done 阶段 size==0 → 空文件 SHA-256 → 正式文件创建，
    /// completed / progress 1.0 语义成立。
    #[test]
    fn group_done_empty_file_creates_zero_byte_file() {
        // 空文件 SHA-256（发送端对 0 字节文件计算的结果）
        let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let file_key = crypto::random_key();
        let r = group_receiver("done-empty", 0, expected.to_string(), file_key);
        assert_eq!(r.received, 0, "空文件无任何 Chunk");

        let final_path = finalize_group_receive(r).expect("空文件必须能正常完成");
        let meta = std::fs::metadata(&final_path).unwrap();
        assert_eq!(meta.len(), 0, "正式文件必须是 0 字节");
        let _ = std::fs::remove_file(&final_path);
    }

    // ---------- GroupFileCompleteAck（协议 round-trip） ----------

    /// 1+2+3. GroupFileCompleteAck JSON round-trip：success=true / false 均完整保留。
    #[test]
    fn group_file_complete_ack_roundtrip() {
        for success in [true, false] {
            let msg = ProtocolMessage::GroupFileCompleteAck {
                transfer_id: "gf-1".into(),
                group_id: "g-1".into(),
                sender_id: "dev-b".into(),
                success,
            };
            let json = serde_json::to_string(&msg).unwrap();
            assert!(json.contains("\"group_file_complete_ack\""));
            assert!(json.contains(&format!("\"success\":{}", success)));
            let back: ProtocolMessage = serde_json::from_str(&json).unwrap();
            match back {
                ProtocolMessage::GroupFileCompleteAck {
                    transfer_id,
                    group_id,
                    sender_id,
                    success: s,
                } => {
                    assert_eq!(transfer_id, "gf-1");
                    assert_eq!(group_id, "g-1");
                    assert_eq!(sender_id, "dev-b");
                    assert_eq!(s, success);
                }
                _ => panic!("应为 GroupFileCompleteAck"),
            }
        }
    }

    // ---------------- 续传：offer 位置判据（`decide_offer`）----------------

    /// 真机事故形状（2026-09-22 跨网首测，160MB 永远传不完）：接收器**还活着**、已收 40MB，
    /// 而发送端每一轮重试都从 `from_bytes = 0` 重发（`flush_pending_files` 就是调
    /// `send_file_from_path`，不携带位置）。这时接收端必须把真实位置回给它，
    /// 而不是回 `Accept` 再把前 40MB 当"迟到的重复片"静默丢掉 ——
    /// 后者等于"每一轮都要重新传一遍已收前缀"，慢链路上永远跑不完。
    #[test]
    fn live_receiver_must_tell_the_sender_where_it_actually_is() {
        use super::{decide_offer, OfferDecision};
        let held = 40 * 1024 * 1024;
        assert_eq!(
            decide_offer(true, held, held, 0, false),
            OfferDecision::ResumeFrom(held),
            "活跃接收器已收 40MB、对方却从 0 重发 ⇒ 必须回真实位置，不能裸 Accept"
        );
    }

    /// 位置本来就对得上 ⇒ 仍然要 `Accept`。这条不许被上一条"顺手改坏"：
    /// 幂等 Accept 修的是真机缺陷（"两边都显示成功、接收侧列表里没有"），
    /// 对端没收到我们的 accept 时会**重发同一个 offer**，那时 from_bytes 是一致的。
    #[test]
    fn matching_position_still_accepts_idempotently() {
        use super::{decide_offer, OfferDecision};
        let held = 40 * 1024 * 1024;
        // 位置对得上 ⇒ 答复仍然属于"接受"这一族，绝不退回 reject（那是被真机教育过的旧行为：
        // 两边都显示成功、接收侧列表里没有）。但**续传段**必须连带把段号归零 ⇒ 判据要能区分。
        assert_eq!(
            decide_offer(true, held, held, held, false),
            OfferDecision::AcceptResumeSegment,
            "位置一致的续传段：接受 + 段号归零"
        );
        // 同一起点的重复 offer ⇒ **不许**归零：上一轮 attempt 已入队的分片还在排空，
        // 归零会把它们判成「跳号」⇒ `Err(文件分片顺序错误)` ⇒ 整单死。
        assert_eq!(decide_offer(true, 0, 0, 0, false), OfferDecision::Accept);
        // 全新传输：什么都没有，对方也从 0 开始
        assert_eq!(decide_offer(false, 0, 0, 0, false), OfferDecision::Accept);
    }

    /// 回归（2026-09-23 审计 A6）：「本机已收完」必须优先于一切位置判据。
    ///
    /// 收完之后 `.part` 已改名、活跃接收器已清空 ⇒ 三输入全归零（与全新传输同形），
    /// 旧判据把重复 Offer 判成 Accept ⇒ 整份重推落「名字(1)」副本
    /// （重复 Offer 的常见来源：终态回执丢失 → 发送端 outbox 重试）。
    #[test]
    fn completed_transfer_rejects_duplicate_offer_without_resend() {
        use super::{decide_offer, OfferDecision};
        // 三输入全零但已收完：必须 AlreadyHave，绝不能 Accept
        assert_eq!(
            decide_offer(false, 0, 0, 0, true),
            OfferDecision::AlreadyHave,
            "已收完的传输收到重复 Offer：拒绝重推（审计 A6）"
        );
        // 即使残留了活跃接收器/磁盘前缀的形态，已收完也一票否决
        assert_eq!(
            decide_offer(true, 1024, 1024, 1024, true),
            OfferDecision::AlreadyHave
        );
    }

    /// 权威是"**我有什么**"，而活跃接收器的内存计数比磁盘 `.part` 更靠前
    /// （`write_chunk` 每片都 `write_all`，但进度落库是 500ms 节流）。
    /// 回一个偏小的位置会让发送端重灌已写进文件的字节 ⇒ 文件超长、校验必失败。
    #[test]
    fn the_active_receiver_is_the_authority_not_the_disk_prefix() {
        use super::{decide_offer, OfferDecision};
        let (live, disk) = (30 * 1024 * 1024, 20 * 1024 * 1024);
        assert_eq!(
            decide_offer(true, live, disk, 0, false),
            OfferDecision::ResumeFrom(live),
            "有活跃接收器时必须报内存里的真实值，不是 .part 大小"
        );
        // 没有活跃接收器（断链后进程没重启）⇒ 磁盘前缀才是唯一事实
        assert_eq!(
            decide_offer(false, 0, disk, 0, false),
            OfferDecision::ResumeFrom(disk),
            "无活跃接收器时仍以 .part 前缀为准（这条是既有行为，锁住别退回去）"
        );
    }
}
