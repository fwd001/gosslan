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

// ★ 模块名刻意不叫 `tests`：本文件是被 `db.rs` 用 `include!` 贴进去的，`mod tests` 会落进
// `db` 那层作用域，与同层已有的测试模块撞名。
#[cfg(test)]
mod clock_contention_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(SCHEMA).expect("SCHEMA 建表");
        conn
    }

    fn rec(msg_id: &str, conv: &str, seq: i64) -> MessageRecord {
        MessageRecord {
            id: 0,
            msg_id: msg_id.into(),
            conv_id: conv.into(),
            sender_id: "a".into(),
            receiver_id: "b".into(),
            kind: "text".into(),
            content: "hi".into(),
            ts: 1,
            seq,
            status: "sent".into(),
        }
    }

    fn seqs_of(conn: &Connection, conv: &str) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT seq FROM messages WHERE conv_id=?1 ORDER BY seq")
            .unwrap();
        stmt.query_map(params![conv], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// §十二「DB 锁竞争 / 网络线程持锁 / UI IPC 抢锁」的第一份**动态**证据 —— 此前这条
    /// 只有静态的锁作用域守卫，没有任何一条测试真把两个线程压到同一条连接上。
    ///
    /// 生产形态要先说清，否则这条会被读错：全进程**只有一条** `Connection`，外面一把 `Mutex`
    /// ⇒ 竞争发生在 **Mutex 层而不是 SQLite 层**（`db.rs` 里根本没有 `busy_timeout`，因为没有
    /// 第二个连接去抢表锁）。所以这里断言的不是"会不会 BUSY"，而是**一把锁 + 生产函数在并发下
    /// 还能不能保住三条数据事实**：`seq` 连续不重号（受护不变量「`seq` 是排序权威」）、
    /// `msg_id` 唯一、一条都不丢。
    ///
    /// ⚠️ 这条**抓不到**什么（写出来免得下一个人误信它，也别照着加第二遍）：
    /// 「把取 seq 挪到锁外」它抓不到 —— `next_clock` 自己是一段事务，加上 mutex 本来就串行，
    /// 拆开取号也不会重号。要拦那一族得靠锁作用域静态守卫（`db 锁作用域守卫`），不是靠这里。
    ///
    /// 时间维度**故意不设预算**：这套门禁没有"容忍已知红"那一档，紧的时间断言在 CI 上必抖，
    /// 而一条常红的判据比没有判据更坏。"慢活进锁"由下面那条长事务测试负责。
    #[test]
    fn concurrent_writers_keep_seq_contiguous_and_lose_nothing() {
        const THREADS: usize = 8;
        const PER: usize = 12;
        let db = Arc::new(Mutex::new(mem_db()));
        let mut handles = Vec::new();
        for t in 0..THREADS {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                for i in 0..PER {
                    // 与生产写路径同形：一次持锁内做完「取 seq → 落消息 → 动会话」
                    // （`transport.rs` 落接收文件那支就是这个形状：`next_clock` 与
                    // `insert_message` 在同一段锁里）。
                    let g = db.lock().unwrap_or_else(|e| e.into_inner());
                    let seq = next_clock(&g, "conv-1").unwrap();
                    insert_message(&g, &rec(&format!("m-{t}-{i}"), "conv-1", seq)).unwrap();
                    touch_conversation(&g, "conv-1", "single", "n", None, "hi", 1).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().expect("并发写入不许 panic");
        }
        let g = db.lock().unwrap_or_else(|e| e.into_inner());
        let want = (1..=(THREADS * PER) as i64).collect::<Vec<_>>();
        assert_eq!(seqs_of(&g, "conv-1"), want, "seq 必须连续且不重号");
        let distinct: i64 = g
            .query_row("SELECT COUNT(DISTINCT msg_id) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            distinct,
            (THREADS * PER) as i64,
            "并发下 msg_id 不许重复（唯一 msg_id 是受护不变量）"
        );
        let clock: i64 = g
            .query_row(
                "SELECT seq FROM conversation_clocks WHERE conv_id='conv-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(clock, (THREADS * PER) as i64, "时钟表必须落在最后一号");
    }

    /// §十二「长事务 test」：一个持有连接的批量事务**不许把普通写入卡住**。
    /// 这是本模块唯一带时间的判据，上限给得极宽（5 秒），它只用来抓"把秒级慢活搬进锁内"
    /// 那一族回归 —— 真机 600MB 那次正是这么坏的：`finish_receive` 在锁内 `fsync`，
    /// 同一时刻其它并发文件的 `write_chunk` 全堵在同一把锁上，被 60s 停滞判据挨个打死。
    /// ⚠️ 它**不是**性能预算，别拿这条证明"够快"。
    #[test]
    fn long_batch_transaction_does_not_wedge_plain_writers() {
        const WRITERS: usize = 4;
        const PER: usize = 5;
        const BATCH: usize = 500;
        let db = Arc::new(Mutex::new(mem_db()));
        let started = std::time::Instant::now();
        let mut handles = Vec::new();
        {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                let g = db.lock().unwrap_or_else(|e| e.into_inner());
                let tx = g.unchecked_transaction().unwrap();
                for i in 0..BATCH {
                    insert_message(&tx, &rec(&format!("b-{i}"), "conv-2", i as i64 + 1)).unwrap();
                }
                tx.commit().unwrap();
            }));
        }
        for w in 0..WRITERS {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                for i in 0..PER {
                    let g = db.lock().unwrap_or_else(|e| e.into_inner());
                    let seq = next_clock(&g, "conv-1").unwrap();
                    insert_message(&g, &rec(&format!("w-{w}-{i}"), "conv-1", seq)).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().expect("长事务在跑时普通写入不许 panic");
        }
        let elapsed = started.elapsed();
        let g = db.lock().unwrap_or_else(|e| e.into_inner());
        let batched: i64 = g
            .query_row("SELECT COUNT(*) FROM messages WHERE conv_id='conv-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(batched, BATCH as i64, "长事务那一整批必须全部落库");
        assert_eq!(
            seqs_of(&g, "conv-1"),
            (1..=(WRITERS * PER) as i64).collect::<Vec<_>>(),
            "长事务抢锁期间，别处的 seq 照样连续不重号"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "锁内不许出现秒级慢活：{elapsed:?}（这条判的是形状，不是速度）"
        );
    }
}
