// 职责边界：
// - Lamport 风格每会话逻辑时钟（next_clock / get_clock）
// ---------------- 每会话逻辑时钟（Lamport 风格） ----------------

/// 读取会话当前逻辑时钟（无记录返回 0）。
pub fn get_clock(conn: &Connection, conv_id: &str) -> i64 {
    conn.query_row(
        "SELECT seq FROM conversation_clocks WHERE conv_id = ?1",
        params![conv_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or(0)
}

/// 发送前取下一个逻辑序号：`max(local, 0) + 1` 并持久化。
/// 逻辑序号只增不减；本地发送与接收共享同一会话时钟。
pub fn next_clock(conn: &Connection, conv_id: &str) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    let cur: i64 = tx
        .query_row(
            "SELECT seq FROM conversation_clocks WHERE conv_id = ?1",
            params![conv_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let next = cur.saturating_add(1);
    tx.execute(
        "INSERT INTO conversation_clocks(conv_id, seq) VALUES(?1, ?2)
         ON CONFLICT(conv_id) DO UPDATE SET seq = excluded.seq",
        params![conv_id, next],
    )?;
    tx.commit()?;
    Ok(next)
}

/// 收到消息后推进本地会话时钟：`seq = max(local, observed)`。
pub fn observe_clock(conn: &Connection, conv_id: &str, observed: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO conversation_clocks(conv_id, seq) VALUES(?1, ?2)
         ON CONFLICT(conv_id) DO UPDATE SET seq = MAX(conversation_clocks.seq, excluded.seq)",
        params![conv_id, observed],
    )?;
    Ok(())
}
