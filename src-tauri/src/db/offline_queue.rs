// 职责边界：
// - 单聊离线补发队列（flush_pending_friend_request 等）
// - **超时自动失败**：outbox 里 created_at + OUTBOX_FAIL_DEADLINE < now 的条目
//   表示这条消息等了足够久还没 Ack（要么发不出去、要么发出去了但对端没回应），
//   清扫时删除 outbox 行 + 把 messages.status 置 "failed"。
// ---------------- 离线补发队列 ----------------
// 注意：本文件通过 include_str! 展开到 db.rs 模块内，
// 所以 crate::db 内的函数（如 now_ms）可以直接调用，不需要额外 use。

/// 单聊 outbox 超时阈值（毫秒）。
/// 120s 是"弱连接但仍应能发到对端"与"真的发不出去该放弃了"之间的合理分界：
/// 心跳 5s 一次，120s 内至少有 24 次 flush_outbox 机会。
/// ⚠️ 只对**可达对端**生效 —— 对端离线时按 `OUTBOX_OFFLINE_HOLD_MS` 保留。
pub const OUTBOX_FAIL_DEADLINE_MS: i64 = 120_000;

/// 离线对端的 outbox 保留窗口（毫秒）。
///
/// 产品承诺「对方离线时消息自动暂存，上线建链后自动补发」（README / INV-P04）：
/// 好友关机两分钟就把行删掉、置 failed，等于用 sweeper 击穿这条承诺
///（2026-09-19 审计 P0：sweeper 不区分「发出去没 Ack」和「对端根本不在网上」）。
/// 离线行保留 7 天后才当僵尸清理 —— 这是防止永久离场节点让队列无限增长的兜底，
/// 不是补发期限。
pub const OUTBOX_OFFLINE_HOLD_MS: i64 = 7 * 24 * 3600 * 1000;

/// sweeper 判定：一条已过 `OUTBOX_FAIL_DEADLINE_MS` 的候选行现在该不该判 failed。
///
/// 可达（当前有 TCP 链路）⇒ 等了至少 120s 仍无 Ack，再等也没有意义 → 放弃；
/// 不可达（对端离线）⇒ 保留，直到超出 `OUTBOX_OFFLINE_HOLD_MS`。
pub fn should_fail_expired_outbox(peer_reachable: bool, age_ms: i64) -> bool {
    if peer_reachable {
        age_ms >= OUTBOX_FAIL_DEADLINE_MS
    } else {
        age_ms >= OUTBOX_OFFLINE_HOLD_MS
    }
}

#[allow(dead_code)]
pub fn insert_outbox(conn: &Connection, msg_id: &str, peer_id: &str, payload: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO outbox(msg_id, peer_id, payload, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![msg_id, peer_id, payload, now_ms()],
    )?;
    Ok(())
}

pub fn list_outbox(conn: &Connection, peer_id: &str) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, payload FROM outbox WHERE peer_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![peer_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

#[allow(dead_code)]
pub fn delete_outbox(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM outbox WHERE id = ?1", params![id])?;
    Ok(())
}

/// 列出**超时未 Ack**的单聊 outbox 候选条目（created_at 早于 deadline 的行）。
/// 返回 (outbox_id, msg_id, peer_id, created_at)——peer_id 与 created_at 供
/// 调用方用 `should_fail_expired_outbox` 区分「对端离线（保留）」与「可达无 Ack（放弃）」。
///
/// Ack 到达时 outbox 行已被删除，所以残留的必然是"发不出去"或"发出去但对端没回应"的条目。
pub fn list_expired_outbox(
    conn: &Connection,
    deadline_ms: i64,
) -> Result<Vec<(i64, String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, msg_id, peer_id, created_at FROM outbox WHERE created_at < ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![deadline_ms], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 按 msg_id 删除单聊 outbox（Ack 到达时按 outbox 行 id 删，
/// 超时清扫时按 msg_id 删更方便，因为同一条 msg_id 可能有多行重试）。
pub fn delete_outbox_by_msg_id(conn: &Connection, msg_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM outbox WHERE msg_id = ?1",
        params![msg_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod offline_queue_tests {
    // 注意：本文件经 include! 拼进 db 模块，模块名在 db 作用域里必须全局唯一
    // （favorites_tests.rs 已经占了 `tests`）。
    use super::*;

    #[test]
    fn offline_peer_is_held_while_reachable_peer_fails_at_deadline() {
        // 可达：过 120s 即放弃；未过 120s 不放弃
        assert!(should_fail_expired_outbox(true, OUTBOX_FAIL_DEADLINE_MS));
        assert!(!should_fail_expired_outbox(true, OUTBOX_FAIL_DEADLINE_MS - 1));
        // 离线：120s / 1 小时都不构成失败理由（承诺是上线后补发）
        assert!(!should_fail_expired_outbox(false, OUTBOX_FAIL_DEADLINE_MS));
        assert!(!should_fail_expired_outbox(false, 3600_000));
        // 离线保留窗口的意义是防僵尸行：到点才清
        assert!(!should_fail_expired_outbox(false, OUTBOX_OFFLINE_HOLD_MS - 1));
        assert!(should_fail_expired_outbox(false, OUTBOX_OFFLINE_HOLD_MS));
    }

    #[test]
    fn list_expired_outbox_returns_peer_and_age_for_classification() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                msg_id TEXT NOT NULL UNIQUE,
                peer_id TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        // 一条过期（created_at=0）、一条新鲜
        insert_outbox(&conn, "m-old", "peer-a", "{}").unwrap();
        conn.execute(
            "UPDATE outbox SET created_at = 0 WHERE msg_id = 'm-old'",
            [],
        )
        .unwrap();
        insert_outbox(&conn, "m-fresh", "peer-b", "{}").unwrap();

        let expired = list_expired_outbox(&conn, now_ms() - OUTBOX_FAIL_DEADLINE_MS).unwrap();
        assert_eq!(expired.len(), 1);
        let (_, msg_id, peer_id, created_at) = &expired[0];
        assert_eq!(msg_id, "m-old");
        // peer_id 与 created_at 必须随行为一起返回：没有它们 sweeper 就无法区分
        // 「对端离线（保留）」与「可达无 Ack（放弃）」——回归 2026-09-19 P0。
        assert_eq!(peer_id, "peer-a");
        assert_eq!(*created_at, 0);
    }
}
