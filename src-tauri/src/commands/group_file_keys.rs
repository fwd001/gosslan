// 职责边界：
// - 群文件会话密钥恢复（ensure_group_file_key）
// - 运行态缺失时从密封密钥解封并回填

/// 恢复/获取群文件会话密钥：优先内存运行态；
/// 缺失时从持久化的密封密钥（gfk:{tid}，群密钥封装）解封并回填内存。
/// 明文 file_key 仍不落库（gfk 存的是群密钥封装后的密文，与 wire 一致）。
pub fn ensure_group_file_key(
    state: &AppState,
    transfer_id: &str,
    group_id: &str,
    group_key: &[u8; 32],
) -> Option<[u8; 32]> {
    let _ = group_id; // 预留：未来按群隔离密钥命名空间
    if let Some(k) = state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(transfer_id)
    {
        return Some(*k);
    }
    let sealed_b64 = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, &format!("gfk:{transfer_id}"))
    }?;
    let sealed = STANDARD.decode(sealed_b64).ok()?;
    let key: [u8; 32] = crypto::open_symmetric(group_key, &sealed)?
        .try_into()
        .ok()?;
    state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(transfer_id.to_string(), key);
    Some(key)
}

/// peer 上线（Hello / 心跳 / 建链）后触发：把该 peer 的 pending 群文件
/// 顺序投递（同一 peer 串行，不同 peer 并行）。
/// 防重入：group_file_sending 标记保证同一 peer 同时只有一个投递任务；
/// 源文件已不存在 → recipient 置 failed（不留永远无法投递的 pending）；
/// 无 link 时保持 pending，本次直接返回（下次连接事件再触发）。
pub async fn flush_pending_group_files(state: &Arc<AppState>, peer_id: &str) {
    if !state
        .group_file_sending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(peer_id.to_string())
    {
        return; // 该 peer 已有投递任务在执行
    }
    let pending = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_pending_group_files_for_recipient(&dbc, peer_id).unwrap_or_default()
    };
    if pending.is_empty() {
        state
            .group_file_sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer_id);
        return;
    }
    let mut tasks: Vec<(String, String, String)> = Vec::new();
    for (tid, gid) in pending {
        // 源文件仍在本机（file_transfers send 行的 path）才可投递
        let src = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_transfer_path(&dbc, &tid)
        };
        let ok = src
            .as_deref()
            .map(|p| std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false))
            .unwrap_or(false);
        if ok {
            tasks.push((tid, gid, src.unwrap()));
        } else {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(&dbc, &tid, peer_id, "failed", 0.0);
        }
    }
    if tasks.is_empty() {
        state
            .group_file_sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer_id);
        return;
    }
    let s2 = state.clone();
    let peer = peer_id.to_string();
    tauri::async_runtime::spawn(async move {
        for (tid, gid, src) in tasks {
            if let Err(e) = dispatch_group_file_to_peer(&s2, &tid, &gid, &peer, &src).await {
                app_handle_log(
                    &s2,
                    &format!("group-file dispatch {tid} -> {peer} failed: {e}"),
                );
            }
        }
        s2.group_file_sending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer);
    });
}
