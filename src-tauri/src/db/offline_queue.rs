// 职责边界：
// - 单聊离线补发队列（flush_pending_friend_request 等）
// - **超时自动失败**：outbox 里 created_at + OUTBOX_FAIL_DEADLINE < now 的条目
//   表示这条消息等了足够久还没 Ack（要么发不出去、要么发出去了但对端没回应），
//   清扫时删除 outbox 行 + 把 messages.status 置 "failed"。
// ---------------- 离线补发队列 ----------------
// 注意：本文件通过 include_str! 展开到 db.rs 模块内，
// 所以 crate::db 内的函数（如 now_ms）可以直接调用，不需要额外 use。

/// 单聊 outbox 超时阈值（毫秒）。
/// 120s 是"弱连接但仍应能发到对端"与"真的发不出去该放弃了"之间的合理分界：
/// 心跳 5s 一次，120s 内至少有 24 次 flush_outbox 机会。
pub const OUTBOX_FAIL_DEADLINE_MS: i64 = 120_000;

#[allow(dead_code)]
pub fn insert_outbox(conn: &Connection, msg_id: &str, peer_id: &str, payload: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO outbox(msg_id, peer_id, payload, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![msg_id, peer_id, payload, now_ms()],
    )?;
    Ok(())
}

pub fn list_outbox(conn: &Connection, peer_id: &str) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, payload FROM outbox WHERE peer_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![peer_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

#[allow(dead_code)]
pub fn delete_outbox(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM outbox WHERE id = ?1", params![id])?;
    Ok(())
}

/// 列出**超时未 Ack**的单聊 outbox 条目。
/// 返回 (outbox_id, msg_id) 对，供清扫任务删除 outbox + 置 failed。
///
/// 只扫 outbox.created_at 超过 `deadline_ms` 的行。Ack 到达时 outbox 行已被删除，
/// 所以残留的必然是"发不出去"或"发出去但对端没回应"的条目。
pub fn list_expired_outbox(
    conn: &Connection,
    deadline_ms: i64,
) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, msg_id FROM outbox WHERE created_at < ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![deadline_ms], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 按 msg_id 删除单聊 outbox（Ack 到达时按 outbox 行 id 删，
/// 超时清扫时按 msg_id 删更方便，因为同一条 msg_id 可能有多行重试）。
pub fn delete_outbox_by_msg_id(conn: &Connection, msg_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM outbox WHERE msg_id = ?1",
        params![msg_id],
    )?;
    Ok(())
}
