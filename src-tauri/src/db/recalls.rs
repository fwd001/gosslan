// 职责边界：
// - 撤回事件（G-Set 去重 + 物化视图）
// ---------------- 撤回（G-Set + 物化视图） ----------------

/// 记下一条撤回事件（幂等）。返回 true 表示本次是首次记录。
pub fn insert_recall(
    conn: &Connection,
    conv_id: &str,
    msg_id: &str,
    recaller_id: &str,
    seq: i64,
) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO group_recalled_messages(conv_id, msg_id, recaller_id, seq)
         VALUES(?1, ?2, ?3, ?4)",
        params![conv_id, msg_id, recaller_id, seq],
    )?;
    Ok(n > 0)
}

/// 该消息是否已被撤回（权威判定）。
pub fn is_recalled(conn: &Connection, msg_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM group_recalled_messages WHERE msg_id = ?1",
        params![msg_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

/// 把撤回**物化**到消息行：清空正文、改 kind。
/// `content` 清空后，搜索 / 导出 / 会话预览 / 已读水位**一行都不用改就自动正确**。
/// 返回 true 表示确实改到了行（消息已在本机）。
pub fn materialize_recall(conn: &Connection, msg_id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE messages SET kind = ?2, content = '' WHERE msg_id = ?1 AND kind != ?2",
        params![msg_id, crate::protocol::KIND_RECALLED],
    )?;
    Ok(n > 0)
}
