// 职责边界：
// - 待发已读回执（pending_reads）
// ---------------- 待发已读回执 ----------------

/// 写入/更新待发已读回执。使用 max 语义：较旧 timestamp 不覆盖较新 timestamp。
pub fn upsert_pending_read(conn: &Connection, peer_id: &str, ts: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_reads(peer_id, last_read_ts) VALUES(?1, ?2)
         ON CONFLICT(peer_id) DO UPDATE SET last_read_ts = MAX(pending_reads.last_read_ts, excluded.last_read_ts)",
        params![peer_id, ts],
    )?;
    Ok(())
}

/// 加载所有待发已读回执（应用启动时恢复内存状态）。
pub fn load_pending_reads(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT peer_id, last_read_ts FROM pending_reads")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 删除指定 peer 的待发已读回执（flush 成功后调用）。
pub fn delete_pending_read(conn: &Connection, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM pending_reads WHERE peer_id = ?1",
        params![peer_id],
    )?;
    Ok(())
}

/// 写入/更新待发群已读回执（max 语义，已读单调前进）。
pub fn upsert_pending_group_read(
    conn: &Connection,
    group_id: &str,
    peer_id: &str,
    ts: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_group_reads(group_id, peer_id, last_read_ts) VALUES(?1, ?2, ?3)
         ON CONFLICT(group_id, peer_id) DO UPDATE SET last_read_ts = MAX(pending_group_reads.last_read_ts, excluded.last_read_ts)",
        params![group_id, peer_id, ts],
    )?;
    Ok(())
}

/// 取某 peer 的全部待发群已读回执。
pub fn list_pending_group_reads(conn: &Connection, peer_id: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt =
        conn.prepare("SELECT group_id, last_read_ts FROM pending_group_reads WHERE peer_id = ?1")?;
    let rows = stmt.query_map(params![peer_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 删除指定 peer 在指定群中的待发群已读回执（flush 成功后调用）。
pub fn delete_pending_group_read(conn: &Connection, group_id: &str, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM pending_group_reads WHERE group_id = ?1 AND peer_id = ?2",
        params![group_id, peer_id],
    )?;
    Ok(())
}

/// 当前毫秒时间戳
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
