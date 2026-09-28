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

/// 收到对方的已读回执后，把「**我发给** `conv_id`、且 `ts <= upto_ts`」的消息一次推到 `read`。
/// 返回被推进的行数（0 是正常结果：早就都是 read 了）。
///
/// ## 为什么这条要有一个家
/// 直连回执（`network/transport.rs` 的 `Message::ReadReceipt`）与跨跳回执
/// （`network/transport/gossip.rs`）原先各写一份一模一样的 UPDATE，而两份谓词都比
/// `db::set_message_status` 那条"硬终态不可逆"弱一格 —— 只排除了 `read` 自己。
/// **取到最弱的那一条就是缺陷**：`failed`（本端根本没发出去）会被后面那条已被读的
/// 消息一起点亮成「已读」，用户看到的是一句对方读过、其实从没送达的话。
///
/// ## 排除清单为什么和 `set_message_status` 不完全一样
/// 那里排除 `read`/`delivered`/`recalled`（单条推进不许回退硬终态）。这里额外排除
/// `failed` —— 已读回执是**对端**的证据，而 `failed` 是**本端**关于这次发送的事实，
/// 一条没发出去的消息不可能被对方读过。`delivered` 保留在可推进的一侧（回执比 Ack 更新
/// 时正是往前推），`sending` 也保留（回执是本端状态卡住时唯一的自愈路径）。
pub fn mark_own_messages_read_upto(
    conn: &Connection,
    conv_id: &str,
    my_device_id: &str,
    upto_ts: i64,
) -> Result<usize> {
    conn.execute(
        "UPDATE messages SET status = 'read'
         WHERE conv_id = ?1 AND sender_id = ?2
           AND status NOT IN ('read', 'failed', 'recalled')
           AND ts <= ?3",
        params![conv_id, my_device_id, upto_ts],
    )
}
