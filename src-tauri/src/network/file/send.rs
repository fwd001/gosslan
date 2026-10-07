// 发送侧：offer → accept → 分片流式发送 → 收尾。
//
// 为什么单独一册：这四段是一条**只有发送方**才有的生命周期（本地文件读、`WireLedger` 记账、
// `FileDone` 后等 `FileCompleteAck`），而接收方那条对端有自己的状态机（`.part` 文件、游标、
// 幂等判定）。两本册各自能顺着读，混在一起时"谁推进度、谁清进度"要跨 600 行找。
//

// 恒等判据（与 transport 那五刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（本模块的锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 搬家同批必须做的两件事：`network/mod.rs::file_src_for_guards()` 登记本册（漏了=形状守卫看不见这段生产码，假绿），
//   以及 `docs/domains.data.mjs` 的 transport/file 领域 paths（漏了=判据 D 报无主文件）。

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
