//! 文件传输与共享目录服务。
//!
//! 传输流程：
//! - 发送方：`send_file_from_path` 发送 `FileOffer`，等待 `FileAccept`（oneshot 握手），
//!   随后以 256KB 分片 base64 编码的 `FileChunk` 流式发送，最后 `FileDone`。
//! - 接收方：收到 `FileOffer` 后自动接受，把分片写入 `.part` 临时文件，`FileDone` 时改名落盘。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::Emitter;
use tokio::time::Duration;

use crate::db;
use crate::network::transport::try_send;
use crate::protocol::{Message, ShareEntry, FILE_CHUNK};
use crate::state::{AppState, FileReceiver};

/// 主动向 `peer_id` 发送本地文件。
pub async fn send_file_from_path(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let size = meta.len();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    {
        let dbc = state.db.lock().unwrap();
        db::upsert_transfer(&dbc, transfer_id, peer_id, &name, size, "send", "pending", Some(path.to_string_lossy().as_ref()), 0.0).ok();
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .pending_file_accept
        .lock()
        .unwrap()
        .insert(transfer_id.to_string(), tx);

    let offer = Message::FileOffer {
        transfer_id: transfer_id.to_string(),
        from: state.device_id.clone(),
        name: name.clone(),
        size,
    };
    try_send(state, peer_id, &offer).await?;

    // 等待对方接受（超时 15 秒）
    match tokio::time::timeout(Duration::from_secs(15), rx).await {
        Ok(Ok(())) => {}
        _ => {
            state.pending_file_accept.lock().unwrap().remove(transfer_id);
            {
                let dbc = state.db.lock().unwrap();
                db::upsert_transfer(&dbc, transfer_id, peer_id, &name, size, "send", "failed", None, 0.0).ok();
            }
            return Err("对方未接受文件".to_string());
        }
    }

    stream_file(state, peer_id, transfer_id, path, name, size).await
}

async fn stream_file(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
    name: String,
    size: u64,
) -> Result<(), String> {
    use std::io::Read;

    let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; FILE_CHUNK];
    let mut seq = 0u32;
    let mut sent = 0u64;

    {
        let dbc = state.db.lock().unwrap();
        db::upsert_transfer(&dbc, transfer_id, peer_id, &name, size, "send", "active", None, 0.0).ok();
    }

    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let data = STANDARD.encode(&buf[..n]);
        let chunk = Message::FileChunk {
            transfer_id: transfer_id.to_string(),
            seq,
            data,
        };
        try_send(state, peer_id, &chunk).await?;
        seq += 1;
        sent += n as u64;
        let progress = if size == 0 { 1.0 } else { sent as f64 / size as f64 };
        {
            let dbc = state.db.lock().unwrap();
            db::upsert_transfer(&dbc, transfer_id, peer_id, &name, size, "send", "active", None, progress).ok();
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

    try_send(state, peer_id, &Message::FileDone { transfer_id: transfer_id.to_string() }).await.ok();
    {
        let dbc = state.db.lock().unwrap();
        db::upsert_transfer(&dbc, transfer_id, peer_id, &name, size, "send", "done", None, 1.0).ok();
    }
    Ok(())
}

/// 接收方：准备接收文件，返回最终落盘路径。
pub fn begin_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&state.downloads_dir).ok();
    let final_path = unique_path(&state.downloads_dir, name);
    let tmp_path = PathBuf::from(format!("{}.part", final_path.display()));
    let f = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;

    state.file_receivers.lock().unwrap().insert(
        transfer_id.to_string(),
        FileReceiver {
            file: f,
            name: name.to_string(),
            size,
            received: 0,
            tmp_path: tmp_path.clone(),
            final_path: final_path.clone(),
            peer_id: peer_id.to_string(),
        },
    );

    {
        let dbc = state.db.lock().unwrap();
        db::upsert_transfer(&dbc, transfer_id, peer_id, name, size, "receive", "active", Some(final_path.to_string_lossy().as_ref()), 0.0).ok();
    }
    Ok(final_path)
}

/// 接收方：写入一个分片，返回累计字节数。
pub fn write_chunk(state: &AppState, transfer_id: &str, data: &[u8]) -> Result<u64, String> {
    use std::io::Write;
    let mut recv = state.file_receivers.lock().unwrap();
    let r = recv.get_mut(transfer_id).ok_or("未知传输")?;
    r.file.write_all(data).map_err(|e| e.to_string())?;
    r.received += data.len() as u64;
    Ok(r.received)
}

/// 接收方：收尾，返回 (name, size, final_path, peer_id)。
pub fn finish_receive(state: &AppState, transfer_id: &str) -> Option<(String, u64, PathBuf, String)> {
    let mut recv = state.file_receivers.lock().unwrap();
    let r = recv.remove(transfer_id)?;
    let _ = r.file.sync_all();
    drop(r.file);
    let _ = std::fs::rename(&r.tmp_path, &r.final_path);
    {
        let dbc = state.db.lock().unwrap();
        db::upsert_transfer(&dbc, transfer_id, &r.peer_id, &r.name, r.size, "receive", "done", Some(r.final_path.to_string_lossy().as_ref()), 1.0).ok();
    }
    Some((r.name.clone(), r.size, r.final_path.clone(), r.peer_id.clone()))
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
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let rel_path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let is_dir = path.is_dir();
        let size = if is_dir { 0 } else { std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) };
        out.push(ShareEntry { name, path: rel_path.clone(), is_dir, size });
        if is_dir {
            walk(&path, &rel_path, out, depth + 1);
        }
    }
}

/// 避免重名：`a.txt` -> `a (1).txt`
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let base = dir.join(name);
    if !base.exists() {
        return base;
    }
    let stem = base.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = base.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
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
    base
}

/// 人类可读的文件大小。
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
