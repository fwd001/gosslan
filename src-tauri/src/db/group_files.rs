// 职责边界：
// - 群文件 per-recipient 投递状态（每群文件每行一个 recipient）
// ---------------- 群文件（per-recipient 投递状态） ----------------

/// 插入群文件记录。校验：群必须存在、sender 必须是群成员。
// 传输流程在后续 GroupFileOffer 步骤启用；本步骤仅 DB 层 + 测试调用。
#[allow(dead_code)]
pub fn insert_group_file(conn: &Connection, f: &GroupFile) -> Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM groups WHERE id = ?1",
            params![f.group_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if !exists {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "群不存在：{}",
            f.group_id
        )));
    }
    let is_member: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND device_id = ?2",
            params![f.group_id, f.sender_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if !is_member {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "发送者不是群成员：{}",
            f.sender_id
        )));
    }
    conn.execute(
        "INSERT INTO group_files(transfer_id, group_id, sender_id, name, size, sha256, status, created_at, scope, todo_id)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            f.transfer_id,
            f.group_id,
            f.sender_id,
            f.name,
            f.size as i64,
            f.sha256,
            f.status,
            f.created_at,
            f.scope,
            f.todo_id
        ],
    )?;
    Ok(())
}

/// 取单个群文件。不存在返回 None。
#[allow(dead_code)]
/// group_files 行 → GroupFile 的唯一映射（get / list 共用，避免两处列序漂移）。
fn row_to_group_file(r: &rusqlite::Row<'_>) -> rusqlite::Result<GroupFile> {
    Ok(GroupFile {
        transfer_id: r.get(0)?,
        group_id: r.get(1)?,
        sender_id: r.get(2)?,
        name: r.get(3)?,
        size: r.get::<_, i64>(4)? as u64,
        sha256: r.get(5)?,
        status: r.get(6)?,
        created_at: r.get(7)?,
        scope: r.get(8)?,
        todo_id: r.get(9)?,
    })
}

pub fn get_group_file(conn: &Connection, transfer_id: &str) -> Option<GroupFile> {
    conn.query_row(
        "SELECT transfer_id, group_id, sender_id, name, size, sha256, status, created_at, scope, todo_id
         FROM group_files WHERE transfer_id = ?1",
        params![transfer_id],
        row_to_group_file,
    )
    .optional()
    .ok()
    .flatten()
}

/// 列出某群的全部群文件（按创建时间倒序，最新在前）。
/// 走 idx_group_files_group 索引。注意：这张表**不随「删除聊天记录」清空** ——
/// 群文件是群级资产（同钉盘/群文件语义），清空聊天历史不应连带删掉文件记录。
pub fn list_group_files(conn: &Connection, group_id: &str) -> Result<Vec<GroupFile>> {
    let mut stmt = conn.prepare(
        "SELECT transfer_id, group_id, sender_id, name, size, sha256, status, created_at, scope, todo_id
         FROM group_files WHERE group_id = ?1 ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map(params![group_id], row_to_group_file)?;
    rows.collect()
}

/// 离线投递定向查询：某 peer 的全部 pending 群文件（按 recipient 精确命中
/// idx_group_file_recipients_recipient 索引，不扫描全表）。
/// 返回 (transfer_id, group_id) 供逐个投递。
pub fn list_pending_group_files_for_recipient(
    conn: &Connection,
    recipient_id: &str,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT r.transfer_id, f.group_id
         FROM group_file_recipients r
         JOIN group_files f ON f.transfer_id = r.transfer_id
         WHERE r.recipient_id = ?1 AND r.status = 'pending'
         ORDER BY f.created_at",
    )?;
    let rows = stmt.query_map(params![recipient_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    rows.collect()
}

/// 群文件 recipient 投递状态摘要（气泡文案用）：
/// completed/failed/pending(or sending) 计数。
#[derive(serde::Serialize)]
pub struct GroupFileDeliverySummary {
    pub total: i64,
    pub completed: i64,
    pub failed: i64,
    pub waiting: i64, // pending + sending（未到终态）
}

/// 汇总某群文件的全部 recipient 状态（定向查询，气泡显示用）。
pub fn get_group_file_delivery_summary(
    conn: &Connection,
    transfer_id: &str,
) -> Option<GroupFileDeliverySummary> {
    get_group_file(conn, transfer_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT
               COUNT(*),
               SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN status NOT IN ('completed','failed') THEN 1 ELSE 0 END)
             FROM group_file_recipients WHERE transfer_id = ?1",
        )
        .ok()?;
    let r = stmt
        .query_row(params![transfer_id], |r| {
            Ok(GroupFileDeliverySummary {
                total: r.get::<_, i64>(0)?,
                completed: r.get::<_, i64>(1).unwrap_or(0),
                failed: r.get::<_, i64>(2).unwrap_or(0),
                waiting: r.get::<_, i64>(3).unwrap_or(0),
            })
        })
        .ok()?;
    Some(r)
}

/// 为群文件添加一个 recipient 投递状态（初始 pending）。
/// 校验：群文件必须存在、recipient 必须是群成员（不允许给群外 peer 建 state）；
/// 同一 (transfer_id, recipient_id) 重复插入报错（PRIMARY KEY 冲突）。
#[allow(dead_code)]
pub fn insert_group_file_recipient(
    conn: &Connection,
    transfer_id: &str,
    recipient_id: &str,
) -> Result<()> {
    let group_id: Option<String> = conn
        .query_row(
            "SELECT group_id FROM group_files WHERE transfer_id = ?1",
            params![transfer_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(group_id) = group_id else {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "群文件不存在：{transfer_id}"
        )));
    };
    let is_member: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND device_id = ?2",
            params![group_id, recipient_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if !is_member {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "接收者不是群成员：{recipient_id}"
        )));
    }
    conn.execute(
        "INSERT INTO group_file_recipients(transfer_id, recipient_id, status, progress, updated_at)
         VALUES(?1, ?2, 'pending', 0.0, ?3)",
        params![transfer_id, recipient_id, now_ms()],
    )?;
    Ok(())
}

/// 列出群文件的全部 recipient 投递状态（按 recipient_id 稳定排序）。
#[allow(dead_code)]
pub fn list_group_file_recipients(
    conn: &Connection,
    transfer_id: &str,
) -> Result<Vec<GroupFileRecipient>> {
    let mut stmt = conn.prepare(
        "SELECT recipient_id, status, progress, updated_at
         FROM group_file_recipients WHERE transfer_id = ?1 ORDER BY recipient_id",
    )?;
    let rows = stmt.query_map(params![transfer_id], |r| {
        Ok(GroupFileRecipient {
            recipient_id: r.get(0)?,
            status: r.get(1)?,
            progress: r.get(2)?,
            updated_at: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// 更新单个 recipient 的投递状态与进度（时间戳只进不退由 updated_at 刷新保证）。
/// recipient 不存在时报错（不静默创建群外 state）。
#[allow(dead_code)]
pub fn update_group_file_recipient(
    conn: &Connection,
    transfer_id: &str,
    recipient_id: &str,
    status: &str,
    progress: f64,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE group_file_recipients SET status = ?3, progress = ?4, updated_at = ?5
         WHERE transfer_id = ?1 AND recipient_id = ?2",
        params![transfer_id, recipient_id, status, progress, now_ms()],
    )?;
    if n == 0 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "群文件投递状态不存在：{transfer_id}/{recipient_id}"
        )));
    }
    Ok(())
}

/// 取单个 recipient 的投递状态（用于「已完成则跳过重建」判断）。不存在返回 None。
pub fn get_group_file_recipient_status(
    conn: &Connection,
    transfer_id: &str,
    recipient_id: &str,
) -> Option<String> {
    conn.query_row(
        "SELECT status FROM group_file_recipients WHERE transfer_id = ?1 AND recipient_id = ?2",
        params![transfer_id, recipient_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// 接收方：幂等建立/重建群文件接收会话。
/// 用于重启后（内存 file_key 丢失）离线补发重建——group_file / recipient 记录已存在时
/// 不报错、不覆盖已完成（completed）状态，只把未完成的中断态复位回 sending。
/// 权限（群存在 + sender 是成员）已由调用方 `handle_group_file_offer` 校验。
pub fn upsert_group_file_receive(
    conn: &Connection,
    f: &GroupFile,
    recipient_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO group_files(transfer_id, group_id, sender_id, name, size, sha256, status, created_at, scope, todo_id)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            f.transfer_id,
            f.group_id,
            f.sender_id,
            f.name,
            f.size as i64,
            f.sha256,
            f.status,
            f.created_at,
            f.scope,
            f.todo_id
        ],
    )?;
    // 未完成的遗留记录复位回 sending（completed 不动）
    conn.execute(
        "UPDATE group_files SET status = 'sending' WHERE transfer_id = ?1 AND status != 'completed'",
        params![f.transfer_id],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO group_file_recipients(transfer_id, recipient_id, status, progress, updated_at)
         VALUES(?1, ?2, 'sending', 0.0, ?3)",
        params![f.transfer_id, recipient_id, now_ms()],
    )?;
    conn.execute(
        "UPDATE group_file_recipients SET status = 'sending', progress = 0.0, updated_at = ?2
         WHERE transfer_id = ?1 AND recipient_id = ?3 AND status != 'completed'",
        params![f.transfer_id, now_ms(), recipient_id],
    )?;
    Ok(())
}
