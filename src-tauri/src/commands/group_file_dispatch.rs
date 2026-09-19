// 职责边界：
// - 群文件 Offer→Chunk→Done 投递核心（dispatch_group_file_to_peer）
// - 失败诊断日志（app_handle_log）

/// 向单个 recipient 执行完整群文件投递：Offer → 流式 Chunk → Done。
/// 元数据/源路径/密钥均从 DB 与运行态恢复，支持离线 pending 的延迟投递。
async fn dispatch_group_file_to_peer(
    state: &Arc<AppState>,
    transfer_id: &str,
    group_id: &str,
    recipient: &str,
    source_path: &str,
) -> Result<(), String> {
    let gf = db::get_group_file(
        &state.db.lock().unwrap_or_else(|e| e.into_inner()),
        transfer_id,
    )
    .ok_or("群文件记录不存在")?;
    let group_key = get_group_key(state, group_id).await.ok_or("群密钥缺失")?;
    let file_key = ensure_group_file_key(state, transfer_id, group_id, &group_key)
        .ok_or("文件会话密钥缺失")?;
    let sealed_file_key =
        STANDARD.encode(crypto::seal_symmetric(&group_key, &file_key).ok_or("封装文件密钥失败")?);

    // 源文件必须仍存在：不存在则该 recipient 置 failed（明确状态变化，
    // 不允许数据库停留在 pending 却永远无法投递）
    let src = std::path::PathBuf::from(source_path);
    let size = match std::fs::metadata(&src) {
        Ok(m) if m.is_file() => m.len(),
        _ => {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(&dbc, transfer_id, recipient, "failed", 0.0);
            return Err("源文件已不存在".to_string());
        }
    };

    // recipient → sending + Offer
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(&dbc, transfer_id, recipient, "sending", 0.0);
    }
    let offer = Message::GroupFileOffer {
        transfer_id: transfer_id.to_string(),
        group_id: group_id.to_string(),
        sender_id: state.device_id.clone(),
        name: gf.name.clone(),
        size,
        sha256: gf.sha256.clone(),
        sealed_file_key,
        scope: gf.scope.clone(),
        todo_id: gf.todo_id.clone(),
    };
    try_send(state, recipient, &offer)
        .await
        .map_err(|e| format!("Offer 发送失败：{e}"))?;

    // ---- cancel + timeout 注册（H3 fix: 群文件也支持用户取消 + 整体 deadline）----
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    state
        .file_send_cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(transfer_id.to_string(), cancel_tx);
    let cancel_cleanup = || {
        state
            .file_send_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(transfer_id);
    };

    // 期限按体积自适应（与单聊同一口径，见 `file::send_deadline_for`）：
    // 固定 10min 窗口下 500-600MB 的视频在群发里必被误判超时。
    let deadline = file::send_deadline_for(size);
    let result = tokio::time::timeout(deadline, async {
        // 流式分片：按**该接收者的实际链路**选块大小 → AEAD（独立随机 nonce）→ Base64 → GroupFileChunk。
        //
        // ⚠️ 不能再用固定的 256KiB（原 `FILE_CHUNK`）：BLE 上 256KiB base64 后约 350KB，
        // MTU=23 时需 ≈25000 片 > `MAX_BLE_CHUNKS_PER_MESSAGE`(8192) ⇒ `fragment()` 返回 `None`
        // ⇒ 整帧被丢（只留一条 warn），而发送方界面照旧显示"已发送"
        // —— 即**群文件在 BLE 上等于 0 字节可达**。
        // 单聊路径早已用 `chunk_size_for_path` 修掉同一个坑（推导见 `network/file.rs`），这里补齐。
        //
        // 分片流**钉死在该接收者的单条链路**上（与单聊同一套修复，见
        // `transport::resolve_stream_link`）：逐片 `try_send` 会在队列满时换链路，
        // 群聊接收端要求 seq 严格递增，一旦跨连接失序整条传输判死 ——
        // 真机表现为群里连发 9-10 张图总有 1-2 张收不全、单发同一张必成功。
        let link = crate::network::transport::resolve_stream_link(state, recipient)
            .await
            .ok_or_else(|| "未建立连接".to_string())?;
        let chunk_size = file::chunk_size_for_path(link.path_kind.as_str());
        let mut f = tokio::fs::File::open(&src)
            .await
            .map_err(|e| e.to_string())?;
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; chunk_size];
        let mut seq: u32 = 0;
        let mut sent: u64 = 0;
        let mut last_report = std::time::Instant::now() - std::time::Duration::from_secs(1);
        loop {
            // 每片开始前先查 cancel —— 用户点了"取消发送"就立刻停
            let n = tokio::select! {
                biased;
                _ = &mut cancel_rx => return Err("用户取消发送".to_string()),
                n = f.read(&mut buf) => n.map_err(|e| e.to_string())?,
            };
            if n == 0 {
                break;
            }
            let Some(key) = state
                .group_file_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(transfer_id)
                .copied()
            else {
                return Err("文件会话密钥丢失".to_string());
            };
            let sealed = crypto::seal_symmetric(&key, &buf[..n]).ok_or("分片加密失败")?;
            let data = STANDARD.encode(&sealed);
            let chunk = Message::GroupFileChunk {
                transfer_id: transfer_id.to_string(),
                group_id: group_id.to_string(),
                sender_id: state.device_id.clone(),
                seq,
                data,
            };
            // 投到**钉住的这条**链路：队列满时原地等背压，绝不换链路（换路 = 分片失序）。
            tokio::select! {
                biased;
                _ = &mut cancel_rx => return Err("用户取消发送".to_string()),
                r = crate::network::transport::send_on_link(&link, &chunk) => {
                    r.map_err(|e| format!("分片发送失败：{e}"))?;
                }
            }
            sent += n as u64;
            // 真实本地进度节流落库（250ms），并向前端推送进度事件。
            if last_report.elapsed() >= std::time::Duration::from_millis(250) {
                last_report = std::time::Instant::now();
                let progress = if size == 0 {
                    1.0
                } else {
                    sent as f64 / size as f64
                };
                // 发送方气泡的进度口径：**只按发送时在线的成员平均**（用户 2026-09-12 反馈）。
                // 离线成员不计入分母、之后补发也不回退进度条；在线成员全部完成即 100%。
                //
                // ⚠️ 先算聚合再取 db 锁：`group_file_online_progress` 内部要读 DB，
                // 若在持有 db 锁时调用就是同锁重入（std Mutex 不可重入，必死锁）。
                // 聚合口径取「在线成员各自进度的平均」，因此这里传入的是**本条连接**的
                // 字节进度，函数内部再与落库值取 max（同一 recipient 的进度单调不减）。
                let max_progress = group_file_online_progress(state, transfer_id, progress);
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ = db::update_group_file_recipient(
                    &dbc,
                    transfer_id,
                    recipient,
                    "sending",
                    progress,
                );
                let _ = db::upsert_transfer(
                    &dbc,
                    transfer_id,
                    group_id,
                    &gf.name,
                    size,
                    "send",
                    "active",
                    Some(source_path),
                    max_progress,
                );
                drop(dbc);
                let _ = state.app.emit(
                    "file-progress",
                    &crate::state::FileProgress {
                        transfer_id: transfer_id.to_string(),
                        received: (size as f64 * max_progress) as u64,
                        total: size,
                    },
                );
            }
            seq += 1;
        }
        // Done：分片全部发出，接收端据此做最终校验
        let done = Message::GroupFileDone {
            transfer_id: transfer_id.to_string(),
            group_id: group_id.to_string(),
            sender_id: state.device_id.clone(),
        };
        // Done 也必须排在**自己那串分片之后**：走 `try_send` 时队列满会 failover 到另一条
        // 空闲连接，完成帧超过仍在路上的分片先到 ⇒ 接收端判"未完成"打死整条传输。
        crate::network::transport::send_on_link(&link, &done)
            .await
            .map_err(|e| format!("Done 发送失败：{e}"))?;
        // 收尾：与单聊 send_file_from_path 同理 — 确保前端收到 100% progress + done 事件。
        let _ = state.app.emit(
            "file-progress",
            &crate::state::FileProgress {
                transfer_id: transfer_id.to_string(),
                received: size,
                total: size,
            },
        );
        let _ = state.app.emit(
            "file-done",
            &crate::state::FileDoneInfo {
                transfer_id: transfer_id.to_string(),
                name: gf.name.clone(),
                size,
                path: source_path.to_string(),
            },
        );
        Ok(())
    })
    .await;

    cancel_cleanup();

    match result {
        Ok(inner) => inner,
        Err(_) => Err(format!("群文件发送超时（超过 {}s）", deadline.as_secs())),
    }
}
