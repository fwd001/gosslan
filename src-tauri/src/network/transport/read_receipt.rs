// 已读回执的路由与待发队列（本机读 / 群读）
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第二批，判据与顺序见 docs/large-file-split-plan.md。

/// 发送单聊已读回执：同网段有直连走 `Message::ReadReceipt`（可被 pending 重试），
/// 跨跳（无直连）改走定向 Gossip `ChatReadReceipt`（广播靠中继按 target 转发）。
///
/// 返回是否「已发出」：直连失败返回 false（供 flush 决定是否重新入队），
/// Gossip 广播是尽力而为、视为已发出返回 true。
pub async fn send_read_receipt_route(
    state: &AppState,
    peer_id: &str,
    msg_id: Option<String>,
    last_read_ts: i64,
) -> bool {
    if state.has_link(peer_id).await {
        let msg = Message::ReadReceipt {
            from: state.device_id.clone(),
            to: peer_id.to_string(),
            last_read_ts,
            last_read_msg_id: msg_id,
        };
        try_send(state, peer_id, &msg).await.is_ok()
    } else {
        let payload = serde_json::json!({
            "last_read_ts": last_read_ts,
            "last_read_msg_id": msg_id,
        })
        .to_string();
        let payload_b64 = STANDARD.encode(payload.as_bytes());
        let mut env = {
            let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
            gossip.build_envelope(
                &state.identity,
                &state.device_id,
                GossipKind::ChatReadReceipt,
                None,
                None,
                &payload_b64,
                db::now_ms(),
                0,
            )
        };
        env.encrypted = false;
        env.target = Some(peer_id.to_string());
        env.sender_sig = state.identity.sign_b64(&env.signing_bytes());
        broadcast_gossip(state, env).await;
        true
    }
}

/// 冲刷待发的单聊已读回执（触发点与 `flush_outbox` 一致：建链 / Hello / 心跳）。
///
/// `mark_read` 将 pending 同时写入内存 HashMap 和 SQLite。此处成功发送后
/// 同时清除两者；失败时内存已由 remove 清除但会重新写入，DB 保留不动
/// （由 `mark_read` 写入，下次 flush 重试）。
pub async fn flush_pending_reads(state: &AppState, peer_id: &str) {
    let Some(_last_read_ts) = state
        .pending_reads
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(peer_id)
    else {
        return;
    };
    // 补发时重新取「对方最近一条消息」的 msg_id + ts，而不是使用之前内存里的 ts。
    // 因为 ts 可能只是被钳制后的值，msg_id 才能让发送方换算回自己的本地时间戳。
    let last = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::last_message_from_sender(&dbc, peer_id, peer_id)
    };
    let Some((msg_id, last_read_ts)) = last else {
        // 对方没有可标记已读的消息，直接清掉 pending 即可。
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_pending_read(&dbc, peer_id).ok();
        return;
    };
    if !send_read_receipt_route(state, peer_id, Some(msg_id), last_read_ts).await {
        // 直连发送失败：内存重新放入 pending，DB 保留（已由 mark_read 写入）
        let mut pending = state
            .pending_reads
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cur = pending.entry(peer_id.to_string()).or_insert(last_read_ts);
        *cur = (*cur).max(last_read_ts);
    } else {
        // 发送成功：清除 DB 中的 pending 记录
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_pending_read(&dbc, peer_id).ok();
    }
}

/// 冲刷指定 peer 的待发群已读回执（触发点与单聊 pending_reads 一致）。
pub async fn flush_pending_group_reads(state: &AppState, peer_id: &str) {
    let rows = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_pending_group_reads(&dbc, peer_id).unwrap_or_default()
    };
    for (group_id, _last_read_ts) in rows {
        let conv_id = format!("group:{group_id}");
        let last = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::last_message_from_sender(&dbc, &conv_id, peer_id)
        };
        let Some((msg_id, last_read_ts)) = last else {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::delete_pending_group_read(&dbc, &group_id, peer_id).ok();
            continue;
        };
        let msg = Message::GroupReadReceipt {
            from: state.device_id.clone(),
            group_id: group_id.clone(),
            last_read_ts,
            last_read_msg_id: Some(msg_id),
        };
        if try_send(state, peer_id, &msg).await.is_ok() {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::delete_pending_group_read(&dbc, &group_id, peer_id).ok();
        }
    }
}

// Rust 侧的系统通知统一走 crate::notifications（尊重开关 + 错误可观察），
// 不再在此处直接调用插件那个会把错误 spawn 掉丢掉的 show()。
