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

/// 只把**仍处 active** 的传输行标为 failed，返回是否有行被改。
///
/// 回收中继态时给接收端一个显式失败终态（2026-09-23 审计 A2：旧行为只从内存
/// retain 掉，DB 行永远停在 active/某个百分比，前端永久卡 X%）。只改 active 行：
/// done/failed 等既有终态不许被回收动作改写。
pub fn mark_transfer_failed_if_active(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE file_transfers SET status = 'failed', progress = 0.0
         WHERE id = ?1 AND status = 'active'",
        params![id],
    )?;
    Ok(n > 0)
}

/// 该传输是否已完整收下（status='done'）。Offer 判据的单行查询（审计 A6）。
///
/// 不用 `list_transfers()` 全表扫：那是在 db 锁内按行数收费，而 Offer 每收到一次
/// 就扫一遍，`file_transfers` 恰好是随使用单调增长的表。
///
/// `Err` 交给调用方裁决（不折叠成 `false`）：`false` 在这里意味着「还要收」，
/// 判错方向会让已完成的文件重传落一份"名字(1)"副本，必须留痕。
pub fn is_transfer_done(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.query_row(
        "SELECT 1 FROM file_transfers WHERE id = ?1 AND status = 'done'",
        params![id],
        |_| Ok(()),
    )
    .optional()?;
    Ok(n.is_some())
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
