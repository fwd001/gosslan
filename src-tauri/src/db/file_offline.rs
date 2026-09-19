// 职责边界：
// - 文件离线投递队列（pending_files CRUD）
// ---------------- 文件离线投递队列 ----------------

/// 幂等写入一条待发文件记录（transfer_id 唯一）。
pub fn insert_file_outbox(
    conn: &Connection,
    transfer_id: &str,
    peer_id: &str,
    group_id: Option<&str>,
    local_path: &str,
    name: &str,
    size: u64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size, status, attempts, next_attempt_at, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7, ?7)",
        params![transfer_id, peer_id, group_id, local_path, name, size as i64, now_ms()],
    )?;
    Ok(())
}

/// 取某 peer 的待投递文件（仅 `pending`，且已到重试时间）。
pub fn list_pending_file_outbox(conn: &Connection, peer_id: &str) -> Result<Vec<(String, String)>> {
    let now = now_ms();
    let mut stmt = conn.prepare(
        "SELECT transfer_id, local_path FROM file_outbox
         WHERE peer_id = ?1 AND status = 'pending' AND next_attempt_at <= ?2 AND attempts < ?3
         ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map(
        params![peer_id, now, crate::network::file::MAX_FILE_OUTBOX_RETRIES],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 投递开始：pending → sending，并累计一次尝试。
/// 加 `WHERE status = 'pending'` 守卫：cancel_file_transfer 可能已把这条标记 failed，
/// 此时不应该再被我们推进 sending —— 否则 spawn loop 后续可能再 mark pending 把 failed 覆盖掉。
pub fn mark_file_outbox_sending(
    conn: &Connection,
    transfer_id: &str,
    backoff_ms: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE file_outbox SET status = 'sending', attempts = attempts + 1, next_attempt_at = ?2 WHERE transfer_id = ?1 AND status = 'pending'",
        params![transfer_id, now_ms().saturating_add(backoff_ms)],
    )?;
    Ok(())
}

/// 投递失败但可重试：回到 pending，等待下次连接/心跳触发。
/// 加 `WHERE status = 'sending'` 守卫：用户已取消（status=failed）的 outbox 不能再被拉回 pending ——
/// 否则 cancel_file_transfer 刚 mark failed，spawn loop 又 mark pending，
/// 下一轮 flush_pending_files 又会把它捞出来重试，用户就看它永远失败不了。
pub fn mark_file_outbox_pending(
    conn: &Connection,
    transfer_id: &str,
    backoff_ms: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE file_outbox SET status = 'pending', next_attempt_at = ?2 WHERE transfer_id = ?1 AND status = 'sending'",
        params![transfer_id, now_ms().saturating_add(backoff_ms)],
    )?;
    Ok(())
}

/// 投递成功：删除队列行。
pub fn delete_file_outbox(conn: &Connection, transfer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM file_outbox WHERE transfer_id = ?1",
        params![transfer_id],
    )?;
    Ok(())
}

/// 永久失败：标记 failed，不再参与重试。
pub fn mark_file_outbox_failed(conn: &Connection, transfer_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE file_outbox SET status = 'failed' WHERE transfer_id = ?1",
        params![transfer_id],
    )?;
    Ok(())
}

/// 文件 outbox 超时阈值（毫秒）。
/// 文件比普通消息大很多，给 30min 总等待窗口。
pub const FILE_OUTBOX_FAIL_DEADLINE_MS: i64 = 30 * 60 * 1000;

/// 列出超时未发送完成的文件 outbox 条目（pending/sending 且 created_at + deadline < now）。
/// 返回 transfer_id 列表，供 sweeper 标记 failed。
/// 过期候选行：(transfer_id, peer_id, group_id, created_at)
type ExpiredFileRow = (String, String, Option<String>, i64);

pub fn list_expired_file_outbox(
    conn: &Connection,
    deadline_ms: i64,
) -> Result<Vec<ExpiredFileRow>> {
    // (transfer_id, peer_id, group_id, created_at)：sweeper 按接收方可达性分类，
    // 离线接收方的文件保留到统一离线窗口（与单聊/群 outbox 同一判据）。
    let mut stmt = conn.prepare(
        "SELECT transfer_id, peer_id, group_id, created_at FROM file_outbox
         WHERE status IN ('pending', 'sending') AND created_at < ?1",
    )?;
    let rows = stmt.query_map(params![deadline_ms], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 崩溃恢复：AppState 初始化时把所有 `sending` 状态的 outbox 重置回 `pending`。
///
/// 为什么必须有：`flush_pending_files` 先 mark sending，再 `send_file_from_path`，成功后
/// 才 delete。如果进程在 sending 期间崩溃（80 张图并发触发 OOM / ANR / 系统杀进程），
/// 重启后这些条目永远卡在 `sending` —— `list_pending_file_outbox` 只捞 `pending` 的，
/// 它们就被彻底遗忘了（真机：用户"发了一半的图重启后再也没到"）。
///
/// 重置为 pending 后：下次 Hello 触发 flush → 这些会被重新捞出来重试。
/// 重复传输由对端的幂等 FileOffer 处理（同 transfer_id 的重复 offer → 幂等 accept）。
pub fn reset_sending_to_pending(conn: &Connection) -> Result<i64> {
    let n = conn.execute(
        "UPDATE file_outbox SET status = 'pending' WHERE status = 'sending'",
        [],
    )?;
    Ok(n as i64)
}

/// 读某 transfer 当前已尝试次数 —— flush_pending_files 超限检查用。
pub fn get_file_outbox_attempts(conn: &Connection, transfer_id: &str) -> Option<i64> {
    let mut stmt = match conn.prepare("SELECT attempts FROM file_outbox WHERE transfer_id = ?1") {
        Ok(s) => s,
        Err(e) => {
            // prepare 失败（schema 迁移中、磁盘满、DB 锁）—— 降级返回 None。
            // 调用方 .map(|a| a >= MAX).unwrap_or(false) 得到 false → 不会判超限，继续重试。
            // 这是正确的降级（prepare 失败不该把所有文件直接判 fail），
            // 但 eprintln 让开发/运维能看到这条异常路径被走到了。
            eprintln!("[file_outbox] get_file_outbox_attempts: prepare failed for transfer_id={transfer_id}: {e}");
            return None;
        }
    };
    stmt.query_row(params![transfer_id], |r| r.get::<_, i64>(0))
        .ok()
}

/// 删除指定 peer 的全部文件投递记录（删除好友时使用）。
pub fn delete_file_outbox_for_peer(conn: &Connection, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM file_outbox WHERE peer_id = ?1",
        params![peer_id],
    )?;
    Ok(())
}
