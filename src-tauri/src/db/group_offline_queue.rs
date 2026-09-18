// 职责边界：
// - 群消息离线补发队列（flush_group_outbox）
// ---------------- 群消息离线补发队列 ----------------

/// 幂等写入一条群消息离线投递记录。`(msg_id, peer_id)` 唯一，
/// 重复写入不会产生第二行。
pub fn insert_group_outbox(
    conn: &Connection,
    msg_id: &str,
    group_id: &str,
    peer_id: &str,
    payload: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO group_outbox(msg_id, group_id, peer_id, payload, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![msg_id, group_id, peer_id, payload, now_ms()],
    )?;
    Ok(())
}

/// 取某成员的全部待补发群消息（按插入顺序）。
pub fn list_group_outbox(conn: &Connection, peer_id: &str) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, payload FROM group_outbox WHERE peer_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![peer_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 收到 GroupAck 后删除指定成员、指定消息的待发记录。
pub fn delete_group_outbox(conn: &Connection, msg_id: &str, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE msg_id = ?1 AND peer_id = ?2",
        params![msg_id, peer_id],
    )?;
    Ok(())
}

/// 删除指定群的全部待发记录（删除群 / 清空数据时使用）。
pub fn delete_group_outbox_for_group(conn: &Connection, group_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE group_id = ?1",
        params![group_id],
    )?;
    Ok(())
}

/// 删除指定成员在指定群中的待发记录（移人出群时使用）。
pub fn delete_group_outbox_for_peer_in_group(
    conn: &Connection,
    group_id: &str,
    peer_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE group_id = ?1 AND peer_id = ?2",
        params![group_id, peer_id],
    )?;
    Ok(())
}

/// 列出**超时未 GroupAck**的群 outbox 条目（按 msg_id 去重）。
/// 返回 Vec<(msg_id, group_id)>，供清扫任务删 outbox + 置 failed。
///
/// 群 outbox 一条 msg_id 对应 N 个 peer_id 行，超时判定按 msg_id 粒度
/// （同一条群消息对所有接收方要么一起成功、要么一起放弃）。
pub fn list_expired_group_outbox(
    conn: &Connection,
    deadline_ms: i64,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT msg_id, group_id FROM group_outbox WHERE created_at < ?1",
    )?;
    let rows = stmt.query_map(params![deadline_ms], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 按 msg_id 删除**所有** peer 的 group_outbox 行。
pub fn delete_group_outbox_by_msg_id(conn: &Connection, msg_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE msg_id = ?1",
        params![msg_id],
    )?;
    Ok(())
}
