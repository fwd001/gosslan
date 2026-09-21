// 职责边界：
// - Todo 图片元数据（todo_image_meta，不落字节）
// - 群文件在线进度聚合（group_file_online_progress_from）

/// 读一张待办图片的**元数据**（不投递字节）：选图后先拿 `TodoImage` 写进任务定义，
/// 字节随后经 `send_todo_image` 投递。`id = sha256` 与投递时 content store 的 cid 同源
/// （同一份文件字节算出的 sha256 必然一致），缩略图才能按 cid 找回。
///
/// **副作用（2026-09-21 补，别再当它是纯读）**：顺手把这份文件登记为**本机持有的内容副本**
/// （cid=sha256 → 原路径）。理由：表单是在**投递之前**就要显示缩略图的，而缩略图一律走
/// `read_content_preview`（按 cid 找回本地副本）—— 不登记的话 `find_local_path` /
/// `find_source` 两条路都查不到，新建任务时选好的图**永远**是空占位（用户 2026-09-21：
/// 「新增群任务的时候图片无法预览」）。投递时 `send_todo_image` 里的 `record_local` 是
/// **同一行**（按 cid+peer+direction upsert），不会重复记账。
/// 记的是用户原路径，落在 `media_dirs`（downloads/cache/favorites）之外 ⇒ 缓存清理
/// （`cache_cleaner` 只删那三个目录内的文件）**动不到**用户的原始文件。
#[tauri::command(async)]
pub fn todo_image_meta(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<crate::protocol::TodoImage, String> {
    let p = std::path::Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|e| format!("读取文件失败：{e}"))?;
    if !meta.is_file() {
        return Err("不是文件".to_string());
    }
    let sha256 = file::sha256_file_hex(p)?;
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let subtype = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let image = crate::protocol::TodoImage {
        id: sha256.clone(),
        name,
        size: meta.len(),
        sha256,
        subtype,
    };
    {
        let s = state.inner();
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = crate::content::store::record_local(
            &dbc,
            &image.sha256,
            &s.device_id,
            None,
            &image.name,
            image.size,
            crate::content::model::Direction::Send,
            &path,
            db::now_ms(),
        );
    }
    Ok(image)
}

/// 保存一张**粘贴**进任务表单的图片（截图 / 复制的位图没有本地路径）。
///
/// 前端把图片字节走 **raw IPC** 直传（`invoke("save_todo_image_bytes", new Uint8Array(buf))`），
/// 这里按内容嗅探扩展名、以 sha256 命名落盘到 `cache_dir/todo-paste/` 并返回路径 ——
/// 之后与选图同一条路：`todo_image_meta`（元数据进任务定义）+ `send_todo_image`（字节走群文件管线）。
///
/// 为什么按 sha256 命名：同一张图反复粘贴天然幂等（同名覆盖），不会在缓存目录里膨胀。
#[tauri::command(async)]
pub async fn save_todo_image_bytes(
    state: State<'_, Arc<AppState>>,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    const MAX_PASTE_BYTES: usize = 25 * 1024 * 1024;
    let bytes: Vec<u8> = match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => b.clone(),
        _ => return Err("图片数据格式不正确".to_string()),
    };
    if bytes.is_empty() {
        return Err("图片数据为空".to_string());
    }
    if bytes.len() > MAX_PASTE_BYTES {
        return Err("图片过大".to_string());
    }
    let ext = sniff_media_ext(&bytes[..bytes.len().min(16)]);
    if !matches!(ext, "jpg" | "png" | "gif" | "webp" | "bmp") {
        return Err("只支持粘贴图片".to_string());
    }
    let s = state.inner();
    let dir = s.cache_dir.join("todo-paste");
    // 写盘放阻塞线程池（几 MB 的截图不卡 async runtime）。
    let save = tokio::task::spawn_blocking(move || -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&bytes);
        let sha256: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;
        let path = dir.join(format!("{sha256}.{ext}"));
        std::fs::write(&path, &bytes).map_err(|e| format!("写入图片失败：{e}"))?;
        Ok(path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(save)
}

/// 群文件「已投递到几个成员」的进度聚合 —— **只按发送时在线的成员平均**。
///
/// 用户口径（2026-09-12 反馈）：进度条表示「**在线成员**都收到了」，不是「全员都收到了」。
/// 离线成员不计入分母，他上线后的补发也**不回退**进度条。
///
/// 分母取自 `state.group_file_online_targets`（发送那一刻冻结的快照）；快照缺失
/// （进程重启后内存态丢失）时退回「全体 recipient 平均」—— 仍是单调不减的口径，
/// 不会出现进度条倒退。
///
/// `fallback` 是调用方刚算出的**本条连接**字节进度，用于覆盖 DB 尚未刷新的那一拍。
///
/// ⚠️ 本函数**自己取 `state.db` 锁**：调用方必须在**未持有 db 锁**时调用（std Mutex 不可重入）。
pub(crate) fn group_file_online_progress(
    state: &AppState,
    transfer_id: &str,
    fallback: f64,
) -> f64 {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let recipients = db::list_group_file_recipients(&dbc, transfer_id).unwrap_or_default();
    drop(dbc);
    if recipients.is_empty() {
        return fallback.clamp(0.0, 1.0);
    }
    let snapshot = state
        .group_file_online_targets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(transfer_id)
        .cloned();
    group_file_progress_from(&recipients, snapshot.as_ref(), fallback)
}

/// `group_file_online_progress` 的**纯函数内核**（便于单测，不碰锁/DB）。
///
/// 口径（用户 2026-09-12）：`online` = 发送那一刻在线的 recipient 集合。
/// - `online` 为 `None`（快照丢失，如进程重启）→ 分母 = **全体** recipient；
/// - `online` 为空集（发送时无人在线）→ **0**（没有在线成员可等，进度条不该满格）；
/// - 否则分母 = 快照内成员，进度 = 其各自进度的**平均**（离线成员不参与）。
///
/// `fallback` 是调用方刚算出的本条连接字节进度（DB 可能还没刷新到这一拍），
/// 最终取 `max(聚合, fallback)` ⇒ **单调不减**，符合「补发不回退进度条」。
pub(crate) fn group_file_progress_from(
    recipients: &[crate::state::GroupFileRecipient],
    online: Option<&std::collections::HashSet<String>>,
    fallback: f64,
) -> f64 {
    let fallback = fallback.clamp(0.0, 1.0);
    let denom: Vec<&crate::state::GroupFileRecipient> = match online {
        Some(set) if !set.is_empty() => recipients
            .iter()
            .filter(|r| set.contains(&r.recipient_id))
            .collect(),
        Some(_) => return 0.0,
        None => recipients.iter().collect(),
    };
    if denom.is_empty() {
        return fallback;
    }
    let sum: f64 = denom.iter().map(|r| r.progress.clamp(0.0, 1.0)).sum();
    (sum / denom.len() as f64).max(fallback).clamp(0.0, 1.0)
}

/// 群文件投递失败诊断（emit 给 DevDiag 面板；不打印任何密钥/明文内容）。
fn app_handle_log(state: &Arc<AppState>, msg: &str) {
    let _ = state.app.emit("group-file-log", msg);
}
