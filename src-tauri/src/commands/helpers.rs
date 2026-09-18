// 职责边界：
// - 辅助工具：clipboard、媒体 present、文件预览路径解析
// ---------------- 辅助 ----------------

/// 将文件从 source 复制到 destination（用于"另存为"下载功能）。
#[tauri::command(async)]
pub fn copy_file(source: String, destination: String) -> Result<(), String> {
    // Android 的「另存为」对话框返回的是 content:// URI，std::fs::copy 写不了，
    // 必须经 ContentResolver（见 android_open::save_path / OpenWith.saveWith）。
    #[cfg(target_os = "android")]
    {
        if destination.starts_with("content://") {
            return crate::android_open::save_path(&source, &destination);
        }
    }
    std::fs::copy(&source, &destination).map_err(|e| e.to_string())?;
    Ok(())
}

/// 把文件本体写入系统剪贴板（Windows CF_HDROP）。
/// 之后既可在资源管理器 / 桌面 Ctrl+V 粘贴出文件，也可粘贴回聊天框直接发送（微信式）。
#[tauri::command(async)]
pub fn copy_file_to_clipboard(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::Setter;
        if !std::path::Path::new(&path).is_file() {
            return Err(format!("文件不存在或不可访问：{path}"));
        }
        let _clip = clipboard_win::Clipboard::new_attempts(10)
            .map_err(|e| format!("无法访问系统剪贴板：{e}"))?;
        clipboard_win::formats::FileList
            .write_clipboard(&[path.as_str()])
            .map_err(|e| format!("复制文件到剪贴板失败：{e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("当前平台暂不支持复制文件到剪贴板".into())
    }
}

/// 读取剪贴板里的文件路径列表（CF_HDROP）。空列表表示剪贴板里没有真实文件
/// （截图 / 网页图片是位图数据，不是文件）。供输入框粘贴时区分「粘贴文件」与「粘贴图片」。
#[tauri::command(async)]
pub fn read_clipboard_file_paths() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        // 读不到（格式不符 / 被占用）一律按"无文件"处理，前端回退到图片粘贴分支。
        let paths: Vec<String> =
            clipboard_win::get_clipboard(clipboard_win::formats::FileList).unwrap_or_default();
        paths
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// 将 base64 数据写入目标路径（用于图片消息"另存为"：前端把 dataURL 解出 base64 传回）。
#[tauri::command(async)]
pub fn save_data_file(base64_data: String, destination: String) -> Result<(), String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data.as_bytes())
        .map_err(|e| e.to_string())?;
    // Android：目标可能是 content:// URI，std::fs::write 写不了，走 ContentResolver。
    #[cfg(target_os = "android")]
    {
        if destination.starts_with("content://") {
            return crate::android_open::save_bytes(&bytes, &destination);
        }
    }
    std::fs::write(&destination, bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// 解析 `msg_id` 指向的本地媒体文件的结果。
///
/// 由 `read_file_preview` 与 `media_present` 共用，避免出现第二份路径解析实现
/// （安全边界必须只有一处）。
enum MediaPath {
    /// 文件存在且通过安全校验。
    Present(Box<PathBuf>),
    /// 查不到这条消息 / 元数据缺路径 / 路径未通过安全校验 —— **无法判断**媒体是否还在。
    /// 尚未落库的乐观消息会落到这里，因此调用方不能据此断言"已被清理"。
    /// 附带面向用户的错误文案。
    Unknown(String),
    /// 消息记录里的路径已解析不到文件 —— 已被「存储清理」删除。
    Gone,
}

/// 校验并解析 `msg_id` 的媒体路径。
///
/// 安全边界：路径必须落在 downloads 目录内（接收方文件），或该消息由本机发出
/// （发送方自选的文件）——两者都不允许对端通过消息内容诱导读取本机任意路径。
fn resolve_media_path(s: &AppState, msg_id: &str) -> MediaPath {
    let Some((sender_id, content)) =
        db::get_message_preview_source(&s.db.lock().unwrap_or_else(|e| e.into_inner()), msg_id)
    else {
        return MediaPath::Unknown("消息不存在".to_string());
    };
    let Some(path) = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| {
            v.get("path")
                .and_then(|p| p.as_str())
                .map(|p| p.to_string())
        })
    else {
        return MediaPath::Unknown("元数据缺少路径".to_string());
    };

    // msg_id → transfer_id：接收侧单聊是 file-{id}，群文件是 gfile-{id}。
    let transfer_id = msg_id
        .strip_prefix("file-")
        .or_else(|| msg_id.strip_prefix("gfile-"));
    let in_flight = transfer_id
        .map(|tid| {
            s.file_receivers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(tid)
                || s.group_file_receivers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(tid)
        })
        .unwrap_or(false);
    let Ok(file) = std::fs::canonicalize(&path) else {
        // **文件还没落盘 ≠ 已被清理**：接收方在 FileDone 之前写的是 <transfer_id>.part，
        // final 路径尚不存在。若这里报 Gone，前端会把"正在接收的图片"标成「已被清理」并
        // 缓存下来，从此再也不会重读（真机：图片时好时坏、要重发才出来）。
        // 在途 ⇒ 报 Unknown，让前端保持"加载中"，等 FileDone 落盘后再读。
        return if in_flight {
            MediaPath::Unknown("仍在接收".to_string())
        } else {
            MediaPath::Gone
        };
    };
    let under_downloads = std::fs::canonicalize(
        s.downloads_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_path(),
    )
    .map(|dir| file.starts_with(dir))
    .unwrap_or(false);
    if !under_downloads && sender_id != s.device_id {
        return MediaPath::Unknown("路径越权".to_string());
    }
    match std::fs::metadata(&file) {
        Ok(meta) if meta.is_file() => MediaPath::Present(Box::new(file)),
        Ok(_) => MediaPath::Unknown("非普通文件".to_string()),
        Err(_) => MediaPath::Gone,
    }
}

/// 媒体是否**仍在本机**（未被存储清理删除）。
///
/// 前端据此把"已被清理"的消息渲染成明确提示，而不是一个空白/裂开的图片框——后者
/// 会让人误以为是对端发来的文件本身有问题。只有能确定「文件已被删除」时才返回 `false`；
/// 查不到消息（例如尚未落库的乐观消息）一律按"存在"处理，绝不能把在途消息误标成已清理。
#[tauri::command(async)]
pub fn media_present(state: State<'_, Arc<AppState>>, msg_id: String) -> Result<bool, String> {
    let s = state.inner();
    Ok(!matches!(resolve_media_path(s, &msg_id), MediaPath::Gone))
}

/// 读取附件预览内容（原始字节，不走 base64 IPC）。
///
/// 按 `msg_id` 反查记录里的本地 `path` 再读，前端据此渲染图片（→Blob/objectURL）
/// 或代码（→TextDecoder）。安全边界见 [`resolve_media_path`]。
/// 超过 `max_bytes` 返回 "TOO_LARGE"，由前端回退文件卡片；文件已被清理返回
/// "文件不存在"，由前端渲染成「已清理」占位。
#[tauri::command(async)]
pub fn read_file_preview(
    state: State<'_, Arc<AppState>>,
    msg_id: String,
    max_bytes: u64,
) -> Result<tauri::ipc::Response, String> {
    let s = state.inner();
    let max_bytes = max_bytes.min(15 * 1024 * 1024);
    let file = match resolve_media_path(s, &msg_id) {
        MediaPath::Present(p) => *p,
        MediaPath::Unknown(e) => return Err(e),
        MediaPath::Gone => return Err("文件不存在".to_string()),
    };
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    if meta.len() > max_bytes {
        return Err("TOO_LARGE".to_string());
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}
