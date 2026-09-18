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
