// 职责边界：
// - 单聊文件传输（Offer/Chunk/Done/取消）
// - 离线投递队列（flush_pending_files）
// ---------------- 文件传输 ----------------

/// 图片 MIME → 扩展名（仅接受常见格式）。
fn image_extension(mime: &str) -> Option<&'static str> {
    match mime.to_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// 纯函数：验证并解码 data URL 图片，返回 (扩展名, 解码后字节)。
/// 用于单元测试覆盖 MIME/大小/base64 等校验逻辑，不涉及文件系统。
fn decode_outgoing_image(data_url: &str) -> Result<(&'static str, Vec<u8>), String> {
    const PREFIX: &str = "data:";
    if !data_url.starts_with(PREFIX) {
        return Err("非法的 data URL".to_string());
    }
    let rest = &data_url[PREFIX.len()..];
    let Some((meta, encoded)) = rest.split_once(',') else {
        return Err("非法的 data URL".to_string());
    };
    let meta = meta.to_lowercase();
    if !meta.ends_with(";base64") {
        return Err("只接受 base64 编码的 data URL".to_string());
    }
    let mime = meta.trim_end_matches(";base64").trim();
    if !mime.starts_with("image/") {
        return Err("只接受图片文件".to_string());
    }
    let Some(ext) = image_extension(mime) else {
        return Err("不支持的图片格式".to_string());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|e| format!("图片解码失败：{e}"))?;
    if bytes.len() as u64 > MAX_OUTGOING_IMAGE_BYTES {
        return Err(format!(
            "图片过大（{} > {}），请压缩后重试",
            bytes.len(),
            MAX_OUTGOING_IMAGE_BYTES
        ));
    }
    if bytes.is_empty() {
        return Err("图片内容为空".to_string());
    }
    Ok((ext, bytes))
}

/// 把前端 paste 产生的 data URL 解码保存为本地文件。
/// 仅接受 image/* 常见格式，按解码后字节数限制，返回本地路径/文件名/大小。
#[tauri::command(async)]
pub fn save_outgoing_image(
    state: State<'_, Arc<AppState>>,
    data_url: String,
) -> Result<serde_json::Value, String> {
    let (ext, bytes) = decode_outgoing_image(&data_url)?;
    let name = format!("image-{}.{ext}", Uuid::new_v4());
    let dl = state
        .inner()
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let path = dl.join(&name);
    std::fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    std::fs::write(&path, &bytes).map_err(|e| format!("图片保存失败：{e}"))?;
    Ok(serde_json::json!({
        "path": path.to_string_lossy().to_string(),
        "name": name,
        "size": bytes.len() as u64,
    }))
}

/// 删除本地文件（用于图片发送初始化失败后清理孤儿文件）。
///
/// ⚠️ **只允许删除下载目录内的文件**。读取侧早有这条边界（见 `resolve_media_path`：
/// canonicalize 后必须落在 downloads 内，或该消息确由本机发出），删除侧原先却接受任意路径。
/// 当前唯一调用方只清理 `save_outgoing_image` 刚写进 downloads 的孤儿图片，
/// 所以这条限制不影响任何既有功能；但若哪天有 UI 把它接到消息里的 `path`
/// （该字段由对端控制），没有它就会变成「对端点一下按钮删掉本机任意文件」。
///
/// 用 canonicalize 比对，避免 `../` 或符号链接绕过前缀匹配。
#[tauri::command(async)]
pub fn delete_file(state: State<'_, Arc<AppState>>, path: String) -> Result<(), String> {
    let s = state.inner();
    let file = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    let dl = s
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let under_downloads = std::fs::canonicalize(&dl)
        .map(|dir| file.starts_with(dir))
        .unwrap_or(false);
    if !under_downloads {
        return Err("只能删除下载目录内的文件".to_string());
    }
    std::fs::remove_file(&file).map_err(|e| e.to_string())
}

/// 用系统默认应用打开本地文件。
/// macOS 走 NSWorkspace（沙盒下 /usr/bin/open 被拦）；Android 走 FileProvider + ACTION_VIEW
/// （私有目录的文件不能以 file:// 交给别的应用，见 android_open.rs）；Windows/Linux 走 opener。
#[tauri::command(async)]
pub fn open_file_native(path: String) -> Result<(), String> {
    crate::open_path::open_path_native(std::path::Path::new(&path))
}

/// macOS 窗口圆角：WebView 加载完成后（前端 onMounted 触发）设背景色跟随主题 +
/// contentView 圆角（setup 阶段设会被 wry 替换 contentView 丢失）。非 macOS 无操作。
#[tauri::command]
pub fn apply_macos_window_shape(window: tauri::WebviewWindow, dark: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return crate::macos_window::apply_rounded_corners(&window, dark);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, dark);
        Ok(())
    }
}

/// 群文件投递摘要（气泡成员状态文案用）：总数/completed/failed/待投递。
#[tauri::command(async)]
pub fn get_group_file_delivery_summary(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> Option<db::GroupFileDeliverySummary> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::get_group_file_delivery_summary(&dbc, &transfer_id)
}

/// 群文件列表里的一项（前端「群文件」面板）。
#[derive(serde::Serialize)]
pub struct GroupFileEntry {
    pub transfer_id: String,
    pub name: String,
    pub size: u64,
    pub sender_id: String,
    pub created_at: i64,
    /// 本机视角的持有状态：`local`（在本机可打开）/ `receiving`（传输中）/
    /// `remote`（未取到）/ `failed`（取失败，可重试）。
    /// **不由群投递状态推导**：我发出去的文件对别人是否送达，与我本机能不能打开无关。
    pub local_state: String,
    /// 本机完整文件的真实路径（仅 `local_state == "local"` 时给出）。
    pub local_path: Option<String>,
    /// 该文件对全群的投递进度（已完成成员数 / 成员总数）。
    pub delivered: i64,
    pub total: i64,
}

/// 列出某群的全部群文件，附带「本机是否持有」与「对全群投递进度」。
///
/// 本机持有状态的判定必须**看磁盘**：路径在 DB 里存在不代表文件还在
/// （缓存清理会删掉媒体文件，见 `clean_cache_now`）。只信 DB 会让面板列出
/// 一堆点了打不开的条目。
#[tauri::command(async)]
pub fn list_group_files(
    state: State<'_, Arc<AppState>>,
    group_id: String,
) -> Result<Vec<GroupFileEntry>, String> {
    let s = state.inner();
    let me = s.device_id.clone();
    // 先把 DB 该给的都取出来，**随即释放 db 锁** —— 下面的磁盘 stat 是阻塞 I/O，
    // 持着全局 db 锁做 N 次 stat 会把整条消息链路的落库一起堵住。
    type Row = (crate::state::GroupFile, i64, i64, String, Option<String>);
    let rows: Vec<Row> = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let files = db::list_group_files(&dbc, &group_id).map_err(|e| e.to_string())?;
        files
            .into_iter()
            .map(|f| {
                let summary = db::get_group_file_delivery_summary(&dbc, &f.transfer_id);
                let (delivered, total) = match summary {
                    Some(x) => (x.completed, x.total),
                    None => (0, 0),
                };
                let my_status = db::get_group_file_recipient_status(&dbc, &f.transfer_id, &me)
                    .unwrap_or_default();
                let path = db::get_transfer_path(&dbc, &f.transfer_id);
                (f, delivered, total, my_status, path)
            })
            .collect()
    };
    Ok(rows
        .into_iter()
        .map(|(f, delivered, total, my_status, path)| {
            let (local_state, local_path) = if f.sender_id == me {
                // 发送者不参与 recipients（见 send_group_file 的成员过滤），其 recipient 行
                // 恒为空 —— 按 my_status 判定会让自己发的文件永远显示"未取到"。
                // 本机是否还留着原件，只能看磁盘。
                local_path_state(path)
            } else {
                match my_status.as_str() {
                    "completed" => local_path_state(path),
                    "sending" | "pending" => ("receiving".to_string(), None),
                    "failed" => ("failed".to_string(), None),
                    // 没有 recipient 行：该文件早于本机入群，尚未登记接收
                    _ => ("remote".to_string(), None),
                }
            };
            GroupFileEntry {
                transfer_id: f.transfer_id,
                name: f.name,
                size: f.size,
                sender_id: f.sender_id,
                created_at: f.created_at,
                local_state,
                local_path,
                delivered,
                total,
            }
        })
        .collect())
}

/// 路径 → 持有状态：路径存在且**文件仍在磁盘上**才算 `local`，否则回落到 `remote`。
fn local_path_state(path: Option<String>) -> (String, Option<String>) {
    match path {
        Some(p) if std::path::Path::new(&p).is_file() => ("local".to_string(), Some(p)),
        _ => ("remote".to_string(), None),
    }
}

/// 构造一条本地文件/图片消息记录（发送方）。
/// kind 由调用方根据 subtype 决定：image 子类型保持 kind="image"，其余为 "file"。
#[allow(clippy::too_many_arguments)]
fn build_file_message(
    state: &AppState,
    transfer_id: &str,
    friend_id: &str,
    path: &str,
    name: &str,
    size: u64,
    kind: &str,
    subtype: &str,
) -> MessageRecord {
    // cid = 明文 sha256：接收方据此在需要时按 cid 拉取（ADR-0019 Phase 3）。
    let cid = file::sha256_file_hex(std::path::Path::new(path)).unwrap_or_default();
    let content = serde_json::json!({
        "name": name,
        "path": path,
        "size": size,
        "sha256": cid,
        "subtype": subtype,
    })
    .to_string();
    let seq = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, friend_id).unwrap_or(1)
    };
    MessageRecord {
        id: 0,
        msg_id: format!("file-{transfer_id}"),
        conv_id: friend_id.to_string(),
        sender_id: state.device_id.clone(),
        receiver_id: friend_id.to_string(),
        kind: kind.to_string(),
        content,
        ts: db::now_ms(),
        seq,
        status: "sent".to_string(),
    }
}

/// 永久失败收尾：队列置 failed，消息气泡置 failed，并通知前端。
fn fail_file_job(state: &AppState, transfer_id: &str, reason: &str) {
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::mark_file_outbox_failed(&dbc, transfer_id).ok();
        db::set_message_status(&dbc, &format!("file-{transfer_id}"), "failed").ok();
        // 保持 file_transfers 行已有的 name/size/path，仅把状态推进到 failed。
        let _ = dbc.execute(
            "UPDATE file_transfers SET status = 'failed', progress = 0.0 WHERE id = ?1",
            rusqlite::params![transfer_id],
        );
    }
    let _ = state.app.emit(
        "file-failed",
        &crate::state::FileFailedInfo {
            transfer_id: transfer_id.to_string(),
            reason: reason.to_string(),
        },
    );
}

/// 尝试投递某 peer 的全部 pending 文件（同一 peer 串行，不同 peer 并行）。
/// 触发点与 `flush_outbox` / `flush_group_outbox` 一致：建链 / Hello / 心跳。
pub async fn flush_pending_files(state: &Arc<AppState>, peer_id: &str) {
    if !state
        .file_sending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(peer_id.to_string())
    {
        return;
    }
    // 没有链路时不做无谓尝试，保持 pending，等下一次连接事件再触发。
    if !state.has_link(peer_id).await {
        state
            .file_sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer_id);
        return;
    }
    let pending = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_pending_file_outbox(&dbc, peer_id).unwrap_or_default()
    };
    if pending.is_empty() {
        state
            .file_sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer_id);
        return;
    }
    let st = state.clone();
    let peer = peer_id.to_string();
    tauri::async_runtime::spawn(async move {
        for (transfer_id, local_path) in pending {
            if !st.has_link(&peer).await {
                break;
            }
            {
                let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                db::mark_file_outbox_sending(&dbc, &transfer_id, 0).ok();
            }
            match file::send_file_from_path(
                &st,
                &peer,
                &transfer_id,
                std::path::PathBuf::from(local_path),
            )
            .await
            {
                Ok(()) => {
                    let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::delete_file_outbox(&dbc, &transfer_id).ok();
                }
                Err(e) => {
                    // retryable 错误也要检查超限 —— 超限直接 fail 不再重试。
                    let over_limit = {
                        let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::get_file_outbox_attempts(&dbc, &transfer_id)
                            .map(|a| a >= crate::network::file::MAX_FILE_OUTBOX_RETRIES)
                            .unwrap_or(false)
                    };
                    if !e.retryable || over_limit {
                        let reason = if over_limit {
                            "连续重试超限（链路长时间未恢复）".to_string()
                        } else {
                            e.message.clone()
                        };
                        st.logger.warn(
                            "file",
                            format!(
                                "[FAILED] transfer={transfer_id} reason={reason} (retryable={}, attempts_over={over_limit})",
                                e.retryable
                            ),
                        );
                        fail_file_job(&st, &transfer_id, &reason);
                    } else {
                        let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::mark_file_outbox_pending(&dbc, &transfer_id, 5_000).ok();
                    }
                }
            }
        }
        st.file_sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer);
    });
}

/// 用户手动取消一条**正在发送中**的文件消息（单聊/群聊共用这个入口）。
///
/// 做三件事：
/// 1. 从 `file_send_cancels` 取 sender send(()) —— send_file_from_path / dispatch_group_file_to_peer
///    的 chunk loop 会 select! 到这个信号，cleanup + return "用户取消发送"。
/// 2. DB 层：outbox → failed；消息状态 → failed；transfer → failed。
/// 3. 通知前端 emit `file-cancelled` + `message-status-changed`。
///
/// 如果文件已经发完或 sender 已关闭，signalled=false 但仍会 mark failed + emit 事件 ——
/// 前端 UI 立刻切到失败态。
///
/// 注意：file_sending（spawn loop 里按 peer_id 存的 HashSet）不需要我们清 ——
/// spawn loop 收到 cancel 信号后会自己走到下一条或清掉。
#[tauri::command(async)]
pub async fn cancel_file_transfer(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> Result<bool, String> {
    let s = state.inner();

    // 1. 发 cancel 信号
    let signalled = {
        let mut cancels = s
            .file_send_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = cancels.remove(&transfer_id) {
            let _ = tx.send(());
            true
        } else {
            false
        }
    };

    // 2. DB 层：单聊 outbox + 消息状态 + transfer
    //    群文件不走 file_outbox（走 group_files 表），但我们仍然 mark cancelled ——
    //    语义：用户主动停止用 "cancelled"，自动失败用 "failed"。
    //    终态守卫保证幂等：已 delivered/read 的不会被覆盖。
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::mark_file_outbox_failed(&dbc, &transfer_id);
        let _ = db::set_message_status(&dbc, &format!("file-{transfer_id}"), "cancelled");
        // 群文件消息前缀是 gfile-，也处理一下
        let _ = db::set_message_status(&dbc, &format!("gfile-{transfer_id}"), "cancelled");
        let _ = db::upsert_transfer(&dbc, &transfer_id, "", "", 0, "send", "cancelled", None, 0.0);
    }

    // 3. 通知前端
    let _ = s.app.emit("file-cancelled", &transfer_id);
    let _ = s
        .app
        .emit("message-status-changed", &format!("file-{transfer_id}"));
    let _ = s
        .app
        .emit("message-status-changed", &format!("gfile-{transfer_id}"));

    s.logger.info(
        "file",
        format!("用户取消文件发送 transfer={transfer_id} (cancel_signal={signalled})"),
    );

    Ok(signalled)
}

/// 请对端**按 cid 再发一份**内容（ADR-0019 Phase 3「点击重取」）。
///
/// 用户点一下未完成/校验失败的图片或文件时调用。对方**无需确认**：它按 cid 找到本地
/// 完整字节就直接回发一份 FileOffer（拥有即授权）。只发给 Hello 里声明了
/// CONTENT_FEATURE_PULL 的对端；旧端返回 Ok(false)（不打扰、不报错）。
#[tauri::command(async)]
pub async fn request_content(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    msg_id: String,
) -> Result<bool, String> {
    let s = state.inner();
    let (cid, name, size) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let content = db::get_message_preview_source(&dbc, &msg_id)
            .map(|(_sender, c)| c)
            .ok_or_else(|| "消息不存在".to_string())?;
        let v: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        (
            v.get("sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            v.get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("file")
                .to_string(),
            v.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        )
    };
    if cid.is_empty() {
        return Err("这条内容没有内容指纹（对方版本较旧），无法重新获取".to_string());
    }
    // 能力协商：对方没声明拉取能力就不发新帧（向后兼容）。
    let supports = s
        .peer_content_features
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&peer_id)
        .copied()
        .unwrap_or(0)
        & crate::protocol::CONTENT_FEATURE_PULL
        != 0;
    if !supports {
        return Ok(false);
    }
    // 手动重取也尽量续传：读该内容在统一状态里的 transfer_id / 已收字节。
    let (transfer_id, from_bytes) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::content::store::get(
            &dbc,
            &cid,
            &peer_id,
            crate::content::model::Direction::Receive,
        )
        .ok()
        .flatten()
        .map(|r| (r.transfer_id.unwrap_or_default(), r.received))
        .unwrap_or_default()
    };
    let msg = Message::ContentRequest {
        from: s.device_id.clone(),
        cid,
        transfer_id,
        from_seq: 0,
        from_bytes,
        name,
        size,
    };
    match try_send(s, &peer_id, &msg).await {
        Ok(()) => Ok(true),
        Err(e) => Err(format!("无法联系对方：{e}")),
    }
}

/// 按**裸 cid** 请求内容 —— 合并转发卡片的读侧配套（ADR-0019 Phase 3）。
///
/// [`request_content`] 从**本机消息行**反查 cid；而卡片是快照（sender/kind/content/ts），
/// 对端机器上没有原始消息行，cid 与元信息只能来自卡片载荷本身。
/// 网络行为与 [`request_content`] 完全一致：服务端（`handle_message` 的
/// ContentRequest 分支）本来就只认 cid（`find_source`），授权规则也不变 ——
/// 好友、或该内容所属群的成员，拥有即授权。
/// 对端不具备拉取能力（旧版本）返回 Ok(false)，不打扰、不报错。
#[tauri::command(async)]
pub async fn request_content_by_cid(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    cid: String,
    name: String,
    size: u64,
) -> Result<bool, String> {
    let s = state.inner();
    if cid.is_empty() {
        return Err("这条内容没有内容指纹，无法重新获取".to_string());
    }
    // 能力协商：与 request_content 同一条规则 —— 对端没声明拉取能力就不发新帧。
    let supports = s
        .peer_content_features
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&peer_id)
        .copied()
        .unwrap_or(0)
        & crate::protocol::CONTENT_FEATURE_PULL
        != 0;
    if !supports {
        return Ok(false);
    }
    // 续传：已有 receive 记录就沿用 transfer_id / 已收字节（与 request_content 同口径）。
    let (transfer_id, from_bytes) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::content::store::get(
            &dbc,
            &cid,
            &peer_id,
            crate::content::model::Direction::Receive,
        )
        .ok()
        .flatten()
        .map(|r| (r.transfer_id.unwrap_or_default(), r.received))
        .unwrap_or_default()
    };
    let msg = Message::ContentRequest {
        from: s.device_id.clone(),
        cid,
        transfer_id,
        from_seq: 0,
        from_bytes,
        name,
        size,
    };
    match try_send(s, &peer_id, &msg).await {
        Ok(()) => Ok(true),
        Err(e) => Err(format!("无法联系对方：{e}")),
    }
}

/// 统一的内容传输状态（ADR-0019 Phase 1）：前端据此在气泡上显示
/// 发送中 / 等待对方在线 / 网络不佳 / 未完成·点击重试 / 完成。
#[tauri::command(async)]
pub fn get_content_transfers(
    state: State<'_, Arc<AppState>>,
) -> Vec<crate::content::TransferRecord> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    crate::content::store::list(&dbc, 200).unwrap_or_default()
}

#[tauri::command(async)]
pub async fn send_file(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    let s = state.inner();
    // 「和自己聊天」暂不支持附件（用户 2026-09-16：先只支持文本）。这里给明确原因，
    // 而不是让用户看到下面那句"对方不是好友"——那与自聊场景完全对不上。
    if friend_id == s.device_id {
        return Err("和自己聊天暂不支持图片或文件".to_string());
    }
    // 好友关系检查
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友，请先扫描添加好友之后再继续聊天。".to_string());
        }
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("只能发送普通文件".to_string());
    }
    let size = meta.len();
    let name = file::derive_file_name(&path);
    let transfer_id = Uuid::new_v4().to_string();
    let subtype = file::classify_file_subtype(&name);
    // kind 只区分 image / file；subtype 通过 content JSON 保留细分
    let kind = if subtype == "image" { "image" } else { "file" };
    let rec = build_file_message(
        s,
        &transfer_id,
        &friend_id,
        &path,
        &name,
        size,
        kind,
        subtype,
    );
    // 注意：不能在持有 db 锁时调用 resolve_nickname（其内部会再次锁 db）。
    let nm = resolve_nickname(s, &friend_id);
    let preview = if kind == "image" {
        "[图片]".to_string()
    } else {
        format!("[文件] {name}")
    };
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        db::insert_message(&tx, &rec).map_err(|e| e.to_string())?;
        db::touch_conversation(&tx, &friend_id, "single", &nm, None, &preview, 0)
            .map_err(|e| e.to_string())?;
        // 先建立 file_transfers 记录，前端刷新传输列表后能立刻拿到进度条载体。
        db::upsert_transfer(
            &tx,
            &transfer_id,
            &friend_id,
            &name,
            size,
            "send",
            "pending",
            Some(path.as_str()),
            0.0,
        )
        .map_err(|e| e.to_string())?;
        db::insert_file_outbox(&tx, &transfer_id, &friend_id, None, &path, &name, size)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    let _ = s.app.emit("message-received", &rec);

    let arc = state.inner().clone();
    let fid = friend_id.clone();
    tokio::spawn(async move {
        flush_pending_files(&arc, &fid).await;
    });
    Ok(transfer_id)
}

/// 统一文件发送入口：当前稳定版统一走「直连 + 离线队列」。
/// 只要好友最终上线，文件就会在连接事件触发时自动补发，不再依赖不可达的中继路径。
#[tauri::command]
pub async fn send_file_auto(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    send_file(state, friend_id, path).await
}

/// 中继切片发送入口：保留命令名以兼容前端，当前实现回退到与直连相同的可靠队列，
/// 避免「看似已发送、实际无法投递」的假成功。
#[tauri::command]
pub async fn send_file_relay(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    send_file(state, friend_id, path).await
}

#[tauri::command(async)]
pub fn get_transfers(state: State<'_, Arc<AppState>>) -> Vec<TransferInfo> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::list_transfers(&dbc).unwrap_or_default()
}
