// 职责边界：
// - 文件传输记录（FileTransfer CRUD）
// ---------------- 文件传输记录 ----------------

#[allow(clippy::too_many_arguments)]
pub fn upsert_transfer(
    conn: &Connection,
    id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    direction: &str,
    status: &str,
    path: Option<&str>,
    progress: f64,
) -> Result<()> {
    // ⚠️ path 用 COALESCE：进度节流的 upsert 一律传 path=None，没有 COALESCE 时
    // 第一次进度 tick 就把建行时写入的本地路径擦成 NULL。前端把 `file_transfers.path`
    // 当作 content 缺 path 时的唯一兜底来源（useMessageFile），群图片预览失效的机制
    // 就有它一份。写法与 content_transfers 保持同口径。
    conn.execute(
        "INSERT INTO file_transfers(id, peer_id, name, size, direction, status, path, progress, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status,
             path = COALESCE(excluded.path, file_transfers.path),
             progress = excluded.progress",
        params![id, peer_id, name, size as i64, direction, status, path, progress, now_ms()],
    )?;
    Ok(())
}

/// 取单条 transfer 的本地 path（群文件离线投递时校验源文件仍在）。
pub fn get_transfer_path(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT path FROM file_transfers WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn list_transfers(conn: &Connection) -> Result<Vec<TransferInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, peer_id, name, size, direction, status, path, progress FROM file_transfers ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(crate::state::TransferInfo {
            id: r.get(0)?,
            peer_id: r.get(1)?,
            name: r.get(2)?,
            size: r.get(3)?,
            direction: r.get(4)?,
            status: r.get(5)?,
            path: r.get(6)?,
            progress: r.get(7)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
