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
    //
    // ★ `WHERE file_transfers.status <> 'done'` 是**终态契约**（第 4 步 P7）：本函数有 39 个
    //   调用点（其中 12 处写 failed、12 处写 active），任何一处晚到一步 —— 清扫器、重复帧、
    //   上一轮 attempt 还堵在链路队列里的残留 —— 都会把"已收到"改成"失败"并把进度从 100%
    //   打回 0%，而那个文件此刻正躺在下载目录里能打开。
    //   集合刻意**只含 `done`**：`failed` / `cancelled` / `pending` 都必须还能被新一轮
    //   attempt 改回 active（`retry_incomplete_content` 复用同一个 transfer_id），
    //   把它们一起钉死就是"一判死永远停在失败"。判据见
    //   `cascade_tests::a_completed_transfer_row_is_never_downgraded`（正向）与
    //   `cascade_tests::a_failed_row_can_be_reactivated_by_the_next_attempt`（反向，
    //   专门给"顺手扩大集合"的下一次准备）。
    conn.execute(
        "INSERT INTO file_transfers(id, peer_id, name, size, direction, status, path, progress, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status,
             path = COALESCE(excluded.path, file_transfers.path),
             progress = excluded.progress
         WHERE file_transfers.status <> 'done'",
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

/// 把一单**离线文件队列**的失败落进 `file_transfers`，返回是否有行被改。
///
/// 与 `mark_transfer_failed_if_active` 的差别只在**合法集合**，不是"写法不同"：
/// 那条服务中继/接收态回收，只有确实在收的 `active` 该被判死；这条服务排队任务判死，
/// 起点是 `pending`（不是 active），所以不能用它。两者共用同一条终态契约 ——
/// **`done` 永远不许被降级**（磁盘证据已经成立，见 `upsert_transfer` 上面那段）。
///
/// 返回值给调用方决定要不要 emit：已经 done 的行改了就该**什么都不发**，
/// 否则用户会为一个明明收好的文件收到一条"传输失败"。
pub fn mark_queued_transfer_failed(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE file_transfers SET status = 'failed', progress = 0.0
         WHERE id = ?1 AND status <> 'done'",
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
