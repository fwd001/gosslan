// 职责边界：
// - 共享目录设置（list_shares / set_share）
// ---------------- 共享目录 ----------------

#[tauri::command(async)]
pub fn set_share_dir(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    path: String,
) -> Result<(), String> {
    if !PathBuf::from(&path).is_dir() {
        return Err("目录不存在".to_string());
    }
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        // 路径 +（macOS）安全作用域书签一起落库：沙盒里书签是重启后唯一还带权限的来源。
        // 书签建不出来不能让这个动作失败（未沙盒构建会失败，而那时路径本来就能用）。
        crate::user_dirs::store(&dbc, crate::user_dirs::SHARE, &path)?;
    }
    *s.share_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    // 目录是"解析后的路径"，不是 `Settings` 里的值 ⇒ patch 不带值，接收方定向重拉一次。
    state.notify_settings_changed(&["shareDir"], Some(window.label()), json!({}));
    Ok(())
}

#[tauri::command(async)]
pub fn get_share_dir(state: State<'_, Arc<AppState>>) -> Option<String> {
    state
        .inner()
        .share_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 文件接收目录（接收的文件/图片落盘于此，可改、可在资源管理器打开）。
#[tauri::command(async)]
pub fn get_downloads_dir(state: State<'_, Arc<AppState>>) -> String {
    state
        .inner()
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .to_string_lossy()
        .to_string()
}

/// 修改文件接收目录：校验目录存在后持久化，后续新接收的文件落到新目录。
#[tauri::command(async)]
pub fn set_downloads_dir(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    path: String,
) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err("目录不存在".to_string());
    }
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        // 与共享目录同一套处理：接收目录也是"用户自选的目录"，沙盒里同样需要书签
        crate::user_dirs::store(&dbc, crate::user_dirs::RECEIVE, &path)?;
    }
    *s.downloads_dir.lock().unwrap_or_else(|e| e.into_inner()) = p;
    state.notify_settings_changed(&["downloadsDir"], Some(window.label()), json!({}));
    Ok(())
}

/// 在系统资源管理器中打开文件接收目录。
#[tauri::command(async)]
pub fn open_downloads_dir(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let p = state
        .inner()
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    open_in_file_manager(&p)
}

/// 跨平台在系统文件管理器里打开指定目录。
///
/// ⚠️ 移动端（Android/iOS）**没有**"文件管理器"这种东西：那里三个平台分支全被裁掉，
/// `path` 于是成了未使用变量。显式声明"移动端不使用该参数"而不是加 `_`——后者会让
/// 桌面端也丢掉名字（编译器就再也帮不上忙）。
#[cfg_attr(mobile, allow(unused_variables))]
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    Ok(())
}

#[tauri::command(async)]
pub async fn request_share_tree(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
) -> Result<Vec<ShareEntry>, String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友".to_string());
        }
    }
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.pending_share_tree
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(request_id.clone(), tx);

    let msg = Message::ShareTreeRequest {
        request_id: request_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
    };
    // 有直连就精确发；没有直连则借**一跳中继**（邻居需与目标有直连）。
    // 这是「即使在桥接状态下，共享目录也要能用」的入口（真机 2026-09-14 全 Windows 局域网）。
    if s.has_link(&friend_id).await {
        if let Err(e) = try_send(s, &friend_id, &msg).await {
            s.pending_share_tree
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id);
            return Err(e);
        }
    } else {
        crate::network::transport::relay_send_to_neighbors(s, &friend_id, &msg).await;
    }

    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(entries)) => Ok(entries),
        _ => {
            s.pending_share_tree
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id);
            Err("获取共享目录超时".to_string())
        }
    }
}

#[tauri::command(async)]
pub async fn download_shared_file(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    remote_path: String,
) -> Result<String, String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友".to_string());
        }
    }
    let transfer_id = Uuid::new_v4().to_string();
    let msg = Message::ShareFileRequest {
        transfer_id: transfer_id.clone(),
        from: s.device_id.clone(),
        path: remote_path.clone(),
        to: Some(friend_id.clone()),
    };
    if s.has_link(&friend_id).await {
        try_send(s, &friend_id, &msg).await?;
    } else {
        crate::network::transport::relay_send_to_neighbors(s, &friend_id, &msg).await;
    }
    // 本地提示：你正在下载好友的文件（聊天信息内简约系统消息）
    let file_name = std::path::Path::new(&remote_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| remote_path.clone());
    let friend_name = resolve_nickname(s, &friend_id);
    insert_system_message(
        s,
        &friend_id,
        &format!("你正在下载「{friend_name}」的文件「{file_name}」"),
    );
    Ok(transfer_id)
}

/// 插入一条本地系统消息到指定会话并推送给前端（共享下载提示、身份密钥变更告警等本地事件用）。
/// 取 `&AppState`（而非 `&Arc<AppState>`）以便网络层 `upsert_peer` 等只持有 `&AppState`
/// 的调用点复用；调用方传 `&Arc<AppState>` 时由 deref 自动转换。
pub fn insert_system_message(state: &AppState, conv_id: &str, text: &str) {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let rec = crate::state::MessageRecord {
        id: 0,
        msg_id: format!("sys-{}", Uuid::new_v4()),
        conv_id: conv_id.to_string(),
        sender_id: state.device_id.clone(),
        receiver_id: state.device_id.clone(),
        kind: "system".to_string(),
        content: text.to_string(),
        ts: db::now_ms(),
        seq: db::next_clock(&dbc, conv_id).unwrap_or(1),
        status: "sent".to_string(),
    };
    db::insert_message(&dbc, &rec).ok();
    drop(dbc);
    let _ = state.app.emit("message-received", &rec);
}
