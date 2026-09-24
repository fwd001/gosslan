//! 内容传输的**持久化**（逻辑层的一部分：表结构由本模块拥有，业务/网络层不碰）。
//!
//! 与旧表的区别：这里显式保存 received / attempts / next_attempt_at / last_error，
//! 让"断网后重启还能继续"成为事实而不是口号（见 ADR-0019 §3.1）。

use rusqlite::{params, Connection, OptionalExtension};

use crate::content::model::{Direction, FailReason, TransferRecord, TransferStatus};
use crate::content::policy;

pub const TABLE: &str = "content_transfers";

/// 建表（幂等）。由 db::init 调用 —— 本层自己拥有 schema，符合分层。
pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS content_transfers (
            cid            TEXT NOT NULL,
            transfer_id    TEXT,
            peer_id        TEXT NOT NULL,
            group_id       TEXT,
            name           TEXT NOT NULL,
            size           INTEGER NOT NULL,
            direction      TEXT NOT NULL,   -- 'send' | 'receive'
            status         TEXT NOT NULL,   -- queued|active|verifying|complete|incomplete|rejected
            received       INTEGER NOT NULL DEFAULT 0,
            attempts       INTEGER NOT NULL DEFAULT 0,
            next_attempt_at INTEGER NOT NULL DEFAULT 0,
            last_error     TEXT,
            path           TEXT,
            created_at     INTEGER NOT NULL,
            updated_at     INTEGER NOT NULL,
            PRIMARY KEY (cid, peer_id, direction)
         );
         CREATE INDEX IF NOT EXISTS idx_content_transfers_status
            ON content_transfers(status);
         -- 每次建链都要问一次「这个 peer 名下还有哪些可恢复的内容」：
         -- WHERE peer_id + status IN(...) ORDER BY updated_at。原先只有 (status) 一条，
         -- peer_id 不是任何索引的前缀 ⇒ 全表扫。第三列让排序也省掉。
         -- 为什么不并进 idx_content_transfers_status：status 单独那条服务的是
         -- 「按状态横切所有 peer」的查询，前缀不同、删不掉。
         CREATE INDEX IF NOT EXISTS idx_content_transfers_peer
            ON content_transfers(peer_id, status, updated_at);",
    )?;
    // 防御性迁移：早期 content_transfers 可能没有 transfer_id 列
    // （断点续传要按它找 <tid>.part）。CREATE TABLE IF NOT EXISTS 不会自动补列。
    //
    // ⚠️ 探测失败时按「没有该列」处理（审计 2.3q）：旧默认 true 在真缺列的老库上
    // 会跳过补列 ⇒ row_to_record 的 r.get("transfer_id") 让**所有行**不可读。
    // 按 false 处理最坏只是对已有列的库多跑一次幂等 ALTER（duplicate column 无害）。
    let has_tid: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('content_transfers') WHERE name = 'transfer_id'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|n| n > 0)
        .unwrap_or(false);
    if !has_tid {
        // 幂等：列其实已存在时报 duplicate column，这种失败无害直接吞；
        // 其它失败必须留痕 —— 静默 = 断点续传记录悄悄全表不可读、无从排查。
        if let Err(e) = conn.execute(
            "ALTER TABLE content_transfers ADD COLUMN transfer_id TEXT",
            [],
        ) {
            if !e.to_string().to_lowercase().contains("duplicate column") {
                eprintln!("[gosslan-db] content_transfers 补列 transfer_id 失败: {e}");
            }
        }
    }
    Ok(())
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<TransferRecord> {
    let dir: String = r.get("direction")?;
    let st: String = r.get("status")?;
    Ok(TransferRecord {
        cid: r.get("cid")?,
        transfer_id: r.get("transfer_id")?,
        peer_id: r.get("peer_id")?,
        group_id: r.get("group_id")?,
        name: r.get("name")?,
        size: r.get::<_, i64>("size")?.max(0) as u64,
        direction: Direction::parse(&dir).unwrap_or(Direction::Receive),
        status: TransferStatus::parse(&st).unwrap_or(TransferStatus::Queued),
        received: r.get::<_, i64>("received")?.max(0) as u64,
        attempts: r.get::<_, i64>("attempts")?.max(0) as u32,
        next_attempt_at: r.get("next_attempt_at")?,
        last_error: r.get("last_error")?,
        path: r.get("path")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

pub fn upsert(conn: &Connection, rec: &TransferRecord) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO content_transfers
            (cid, transfer_id, peer_id, group_id, name, size, direction, status, received,
             attempts, next_attempt_at, last_error, path, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(cid, peer_id, direction) DO UPDATE SET
            transfer_id = COALESCE(excluded.transfer_id, content_transfers.transfer_id),
            group_id = excluded.group_id,
            name = excluded.name,
            size = excluded.size,
            status = excluded.status,
            received = MAX(content_transfers.received, excluded.received),
            -- 自审必改#1（4.22.1 接线不完整）：receive 起点的 upsert 一律带 attempts=0，
            -- 直接覆盖会把已累计的重试次数**压回 0** —— 封顶永不触发。计数语义是
            -- 「这条内容对这个 peer 试过几次」，只增不减；新传输要清计数请删行重建。
            attempts = MAX(content_transfers.attempts, excluded.attempts),
            next_attempt_at = excluded.next_attempt_at,
            last_error = excluded.last_error,
            path = COALESCE(excluded.path, content_transfers.path),
            updated_at = excluded.updated_at",
        params![
            rec.cid,
            rec.transfer_id,
            rec.peer_id,
            rec.group_id,
            rec.name,
            rec.size as i64,
            rec.direction.as_str(),
            rec.status.as_str(),
            rec.received as i64,
            rec.attempts as i64,
            rec.next_attempt_at,
            rec.last_error,
            rec.path,
            rec.created_at,
            rec.updated_at,
        ],
    )?;
    Ok(())
}

pub fn get(
    conn: &Connection,
    cid: &str,
    peer_id: &str,
    direction: Direction,
) -> rusqlite::Result<Option<TransferRecord>> {
    conn.query_row(
        "SELECT * FROM content_transfers WHERE cid=?1 AND peer_id=?2 AND direction=?3",
        params![cid, peer_id, direction.as_str()],
        row_to_record,
    )
    .optional()
}

pub fn list(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<TransferRecord>> {
    let mut stmt =
        conn.prepare("SELECT * FROM content_transfers ORDER BY updated_at DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit as i64], row_to_record)?;
    rows.collect()
}

/// 该 peer 下仍可恢复的记录（建链时自动重试用）。
pub fn list_resumable_for_peer(
    conn: &Connection,
    peer_id: &str,
) -> rusqlite::Result<Vec<TransferRecord>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM content_transfers
         WHERE peer_id=?1 AND status IN ('queued','active','incomplete')
         ORDER BY updated_at ASC",
    )?;
    let rows = stmt.query_map(params![peer_id], row_to_record)?;
    rows.collect()
}

/// 找一份**可用于服务**的完整内容：status=complete 且 path 非空。
///
/// 返回 (peer_id, group_id, path)，多份时取最近更新的那份。
/// 这是 ContentRequest「拥有即授权」的判据来源（ADR-0019 Phase 3）。
pub fn find_source(
    conn: &Connection,
    cid: &str,
) -> rusqlite::Result<Option<(String, Option<String>, String)>> {
    conn.query_row(
        // 判据是"本地是否真的有这份完整字节" = path 非空；接收侧只在
        // FileDone 落盘后才写 path，发送侧本来就有整份文件。status 不参与 ——
        // 内容可用性与"这条投递送没送出去"是两件事。
        "SELECT peer_id, group_id, path FROM content_transfers
         WHERE cid=?1 AND path IS NOT NULL
         ORDER BY updated_at DESC LIMIT 1",
        params![cid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
}

/// 本机是否持有该内容的完整字节：有就返回本地路径（读侧用：预览 / 另存为）。
///
/// 判据与 [`find_source`] 相同（path 非空即认为可用），只是读侧不关心 owner/group。
/// 合并转发卡片的图片预览按它取字节 —— 卡片是快照，对端机器上没有原始消息行，
/// 只有卡片载荷里的 cid 可用。
pub fn find_local_path(conn: &Connection, cid: &str) -> Option<String> {
    conn.query_row(
        "SELECT path FROM content_transfers
         WHERE cid=?1 AND path IS NOT NULL
         ORDER BY updated_at DESC LIMIT 1",
        params![cid],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

/// 传输过程中**节流**更新 received（只前进）。断点续传的起点就是它。
pub fn touch_received(
    conn: &Connection,
    cid: &str,
    peer_id: &str,
    received: u64,
    now_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_transfers
            SET received = MAX(received, ?3), updated_at = ?4
          WHERE cid = ?1 AND peer_id = ?2 AND direction = 'receive'",
        params![cid, peer_id, received as i64, now_ms],
    )?;
    Ok(())
}

/// 把一条记录标记为完成（收/发皆可）。
pub fn mark_complete(
    conn: &Connection,
    cid: &str,
    peer_id: &str,
    direction: Direction,
    path: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_transfers
            SET status='complete', received=size, path=?4,
                next_attempt_at=0, last_error=NULL, updated_at=?5
          WHERE cid=?1 AND peer_id=?2 AND direction=?3",
        params![cid, peer_id, direction.as_str(), path, now_ms],
    )?;
    Ok(())
}

/// 记录一份**本机完整持有**的内容（发送成功，或接收落盘完成）。
///
/// 之后 find_source 就能按 cid 为任何请求方服务 —— 群聊里 A→B 成功后，C 也能从 B 拉
/// （已收完的成员一样是种子）。
#[allow(clippy::too_many_arguments)]
pub fn record_local(
    conn: &Connection,
    cid: &str,
    peer_id: &str,
    group_id: Option<&str>,
    name: &str,
    size: u64,
    direction: Direction,
    path: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let rec = TransferRecord {
        cid: cid.to_string(),
        transfer_id: None,
        peer_id: peer_id.to_string(),
        group_id: group_id.map(str::to_string),
        name: name.to_string(),
        size,
        direction,
        status: TransferStatus::Complete,
        received: size,
        attempts: 0,
        next_attempt_at: 0,
        last_error: None,
        path: Some(path.to_string()),
        created_at: now_ms,
        updated_at: now_ms,
    };
    upsert(conn, &rec)
}

/// 记录一次失败：按【纯策略】算新状态/退避，再落库。返回更新后的记录。
pub fn record_failure(
    conn: &Connection,
    cid: &str,
    peer_id: &str,
    direction: Direction,
    reason: FailReason,
    now_ms: i64,
) -> rusqlite::Result<Option<TransferRecord>> {
    let Some(mut rec) = get(conn, cid, peer_id, direction)? else {
        return Ok(None);
    };
    if !policy::can_transition(rec.status, policy::status_after_failure(reason)) {
        return Ok(Some(rec)); // 终态不再改写
    }
    let (status, attempts, next_at) = policy::on_failure(rec.attempts, reason, now_ms);
    rec.status = status;
    rec.attempts = attempts;
    rec.next_attempt_at = next_at;
    rec.last_error = Some(reason.message().to_string());
    rec.updated_at = now_ms;
    upsert(conn, &rec)?;
    Ok(Some(rec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::model::{Direction, FailReason, TransferRecord, TransferStatus};

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    /// 自审必改#1 回归：起点 upsert 带 attempts=0 **不得**抹掉已累计的重试次数。
    #[test]
    fn upsert_never_walks_attempts_backwards() {
        let conn = mem();
        let mut r = rec("c1", TransferStatus::Incomplete, 0);
        r.attempts = 5;
        upsert(&conn, &r).unwrap();
        r.attempts = 0; // receive 起点的常规写法
        r.status = TransferStatus::Active;
        upsert(&conn, &r).unwrap();
        assert_eq!(
            get(&conn, "c1", "peer-a", Direction::Receive)
                .unwrap()
                .unwrap()
                .attempts,
            5
        );
    }

    /// 无人应答的重试链必须封顶：8 次 record_failure 后收口 Rejected，
    /// 并从「可恢复列表」里消失（对端重装/文件已删不再无限重发）。
    #[test]
    fn unanswered_retries_reach_cap_and_stop() {
        let conn = mem();
        let r = rec("c2", TransferStatus::Incomplete, 0);
        upsert(&conn, &r).unwrap();
        let mut now = 1_000i64;
        for i in 0..policy::MAX_CONTENT_RETRIES {
            let updated = record_failure(
                &conn,
                "c2",
                "peer-a",
                Direction::Receive,
                FailReason::Timeout,
                now,
            )
            .unwrap()
            .unwrap();
            assert_eq!(updated.attempts, i + 1);
            now += 120_000;
        }
        let final_rec = get(&conn, "c2", "peer-a", Direction::Receive)
            .unwrap()
            .unwrap();
        assert_eq!(
            final_rec.status,
            TransferStatus::Rejected,
            "到封顶必须收口终态"
        );
        let resumable = list_resumable_for_peer(&conn, "peer-a").unwrap();
        assert!(
            resumable.iter().all(|x| x.cid != "c2"),
            "Rejected 行不得再被建链重试捞出（旧缺陷：计数被起点 upsert 压回 0 ⇒ 永远打不到封顶）"
        );
    }

    fn rec(cid: &str, status: TransferStatus, received: u64) -> TransferRecord {
        TransferRecord {
            cid: cid.into(),
            transfer_id: None,
            peer_id: "peer-a".into(),
            group_id: None,
            name: "a.png".into(),
            size: 100,
            direction: Direction::Receive,
            status,
            received,
            attempts: 0,
            next_attempt_at: 0,
            last_error: None,
            path: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn upsert_round_trips_and_keeps_max_received() {
        let conn = mem();
        upsert(&conn, &rec("c1", TransferStatus::Active, 40)).unwrap();
        let got = get(&conn, "c1", "peer-a", Direction::Receive)
            .unwrap()
            .unwrap();
        assert_eq!(got.received, 40);
        assert_eq!(got.status, TransferStatus::Active);
        // 进度回退（旧的 40 → 新的 10）不允许把 received 变回去
        let mut older = rec("c1", TransferStatus::Active, 10);
        older.path = Some("/tmp/a.png".into());
        upsert(&conn, &older).unwrap();
        let got2 = get(&conn, "c1", "peer-a", Direction::Receive)
            .unwrap()
            .unwrap();
        assert_eq!(got2.received, 40, "received 只能前进");
        assert_eq!(got2.path.as_deref(), Some("/tmp/a.png"));
    }

    #[test]
    fn failure_persists_resumable_or_terminal() {
        let conn = mem();
        upsert(&conn, &rec("c2", TransferStatus::Active, 0)).unwrap();
        let r = record_failure(
            &conn,
            "c2",
            "peer-a",
            Direction::Receive,
            FailReason::Timeout,
            1_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(r.status, TransferStatus::Incomplete);
        assert_eq!(r.attempts, 1);
        assert!(r.next_attempt_at > 1_000);
        assert!(r.last_error.is_some());
        let list = list_resumable_for_peer(&conn, "peer-a").unwrap();
        assert_eq!(list.len(), 1, "可恢复记录必须能被建链时捞出来");
        // 终态
        let r2 = record_failure(
            &conn,
            "c2",
            "peer-a",
            Direction::Receive,
            FailReason::HashMismatch,
            2_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(r2.status, TransferStatus::Rejected);
        assert!(list_resumable_for_peer(&conn, "peer-a").unwrap().is_empty());
    }

    /// 接收落盘 / 发送成功都登记为"本机持有"⇒ find_source 能按 cid 找到并带上群上下文
    /// （群聊里已收完的成员也能当种子）。
    #[test]
    fn record_local_makes_content_servable_with_group_scope() {
        let conn = mem();
        record_local(
            &conn,
            "cid-1",
            "dev-a",
            Some("g1"),
            "a.png",
            10,
            Direction::Receive,
            "/tmp/a.png",
            5,
        )
        .unwrap();
        let (owner, group, path) = find_source(&conn, "cid-1").unwrap().unwrap();
        assert_eq!(owner, "dev-a");
        assert_eq!(group.as_deref(), Some("g1"));
        assert_eq!(path, "/tmp/a.png");
        assert!(find_source(&conn, "nope").unwrap().is_none());
    }

    /// 卡片图片预览的读侧：按 cid 取**本地路径**。与 find_source 同判据
    /// （path 非空 = 本机持有完整字节），在途/失败记录（path 为空）不能算持有。
    #[test]
    fn find_local_path_only_returns_records_with_path() {
        let conn = mem();
        // 在途：path 为 None（begin_receive 的初始形态）⇒ 不算持有
        upsert(&conn, &rec("cid-2", TransferStatus::Active, 0)).unwrap();
        assert_eq!(
            find_local_path(&conn, "cid-2"),
            None,
            "path 为空不能当作持有"
        );
        // 落盘后：按 cid 能取回路径
        record_local(
            &conn,
            "cid-2",
            "peer-a",
            None,
            "a.png",
            10,
            Direction::Receive,
            "/downloads/a.png",
            6,
        )
        .unwrap();
        assert_eq!(
            find_local_path(&conn, "cid-2").as_deref(),
            Some("/downloads/a.png")
        );
        assert_eq!(find_local_path(&conn, "nope"), None);
    }
}
