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

/// 一单文件"从此不再重发"的三种原因 —— 落库形状相同、口径不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileJobEnd {
    /// 超时（outbox 清扫器判死）
    Expired,
    /// 重试耗尽 / 明确不可重试的错误（发送侧放弃）
    GiveUp,
    /// 用户主动取消
    Cancelled,
}

impl FileJobEnd {
    fn status(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Expired | Self::GiveUp => "failed",
        }
    }
}

/// **三份收尾合并成一份**（第 4 步 P7）：文件队列任务的终态落库只有这一个出口。
///
/// 合并之前有三份各写一遍的实现（清扫器 / 发送放弃 / 用户取消），后果都实测到了：
///  · "把行踢出重试集合的那一步排最后"这条规矩只在其中两处成立（第三刀修的正是那两处）；
///  · `done` 不可降级（INV-P26）的闸门只装在两扇门上，清扫器那扇没有；
///  · 清扫器还多写了一句 `gfile-` —— 群文件的气泡不归这条路径管（群文件的逐人台账在
///    `group_file_recipients`），今天撞不到只是因为没人往 `file_outbox` 写 `group_id`。
///
/// ★ 顺序与"关行"的条件都是语义，不是风格：气泡 → 台账 → **只有面向用户的写都落地了**
/// 才关队列行。`mark_file_outbox_*` 一跑，`list_expired_file_outbox` 就再也扫不到这一行，
/// 所以前面任何一步失败时它必须**还在**集合里，否则"返回 false、下一 tick 再试"是假话
/// （审计 A3 的自身缺陷；判据 `finalize_expired_file_failure_leaves_the_row_retryable`）。
/// 唯一的例外是"台账已 done"：那时没有任何面向用户的写要做，行必须关掉 ——
/// 不然它每个 tick 被重扫一遍又什么都不做（活锁）。
///
/// 台账那一笔写刻意走 `upsert_transfer` 而不是"返回改动与否"的助手：后者在**台账行不存在**时
/// 报 false，会让上面的关行条件永不成立 ⇒ 清扫器空转。少一次 emit 不值得换来一个活锁，
/// 而"不许把 done 降级"这件事由 `upsert_transfer` 自己守（INV-P26）。
///
/// 返回 `true` = 面向用户的状态真的推进了 ⇒ 调用方据此决定要不要 emit。
pub fn finalize_file_failure(
    conn: &Connection,
    transfer_id: &str,
    end: FileJobEnd,
) -> Result<bool> {
    let status = end.status();
    let cancelled = end == FileJobEnd::Cancelled;
    // 读失败当作"没收成"继续判失败 —— 那正是加这道闸门之前的行为：宁可维持现状，
    // 也不要在读不到状态时静默放过一个真的失败。
    let already_done = is_transfer_done(conn, transfer_id).unwrap_or(false);

    let mut changed = false;
    if !already_done || cancelled {
        // 取消是用户动作：界面必须立刻响应。已 done 的行不会被真降级（闸门在 upsert 里）。
        let mut bubble = set_message_status(conn, &format!("file-{transfer_id}"), status).is_ok();
        // `gfile-` 只在取消时写：取消入口是 1:1 与群文件**共用**的（用户取消整条消息），
        // 而超时/放弃只代表"某一个收件人没收到"，群气泡由 `group_file_recipients` 决定。
        // 两个方向各有判据：`expired_file_does_not_touch_the_group_bubble` /
        // `a_cancelled_group_file_marks_its_own_bubble`。
        if cancelled {
            bubble &= set_message_status(conn, &format!("gfile-{transfer_id}"), status).is_ok();
        }
        if bubble {
            changed =
                upsert_transfer(conn, transfer_id, "", "", 0, "send", status, None, 0.0).is_ok();
        }
    }

    if changed || already_done {
        // ★ 永远排在最后
        match end {
            FileJobEnd::Cancelled => mark_file_outbox_cancelled(conn, transfer_id)?,
            _ => mark_file_outbox_failed(conn, transfer_id)?,
        }
    }
    Ok(changed)
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
