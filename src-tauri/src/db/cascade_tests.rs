//! 删除级联与消息字段回填的行为测试（2026-09-19 审计 P0#6 + 2026-09-20 大文件体验轮）。
//!
//! 钉住的几类事故：
//!   ① 删会话后 `outbox`/`group_outbox`/`file_outbox` 残留 → 下次建链把已删消息补发回去；
//!   ② 退群/删群不删 `messages` 与已读水位 → 全局搜索命中「点不开的孤儿」；
//!   ③ 收口前入库的历史孤儿由 v7 迁移一次性清掉；
//!   ④ 发送行的 `sha256` 由投递任务回填 —— 只补一个字段，且同值不得产生第二次写入。

use super::*;
use rusqlite::Connection;

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn
}

fn ins_msg(conn: &Connection, msg_id: &str, conv: &str, kind: &str, content: &str, seq: i64) {
    conn.execute(
        "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq)
         VALUES (?1, ?2, 'me', 'peer', ?3, ?4, 1000, ?5)",
        params![msg_id, conv, kind, content, seq],
    )
    .unwrap();
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn delete_conversation_purges_inflight_queues_and_keeps_cards() {
    let conn = fresh_db();
    conn.execute(
        "INSERT INTO conversations(id, kind, name) VALUES ('p1','single','P')",
        [],
    )
    .ok();
    ins_msg(&conn, "m1", "p1", "text", "hello", 1);
    ins_msg(&conn, "m2", "p1", "file", "{\"name\":\"a\"}", 2);
    ins_msg(&conn, "c1", "p1", "announcement", "{\"text\":\"g\"}", 3); // Card：不属于聊天历史
    conn.execute(
        "INSERT INTO outbox(msg_id, peer_id, payload, created_at) VALUES ('m1','p1','{}',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO group_outbox(msg_id, group_id, peer_id, payload, created_at) VALUES ('m2','p1','x','{}',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size, status, attempts, next_attempt_at, created_at)
         VALUES ('t1','p1',NULL,'/tmp/a','a',1,'pending',0,0,1)",
        [],
    )
    .unwrap();

    delete_conversation(&conn, "p1").unwrap();

    // Bubble 消息与三条在途队列全清；Card 与会话外数据不动
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE kind IN ('text','file')"
        ),
        0,
        "被删会话的 Bubble 消息必须清零"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE kind = 'announcement'"
        ),
        1,
        "Card 不属于聊天历史，不得顺手删"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM outbox"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM group_outbox"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM file_outbox"), 0);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM conversations WHERE id='p1'"),
        0
    );
}

#[test]
fn delete_group_cascade_clears_history_reads_and_queues() {
    let conn = fresh_db();
    conn.execute(
        "INSERT INTO groups(id, name, creator, created_at) VALUES ('g1','G','me',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO group_members(group_id, device_id) VALUES ('g1','me'),('g1','a')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO conversations(id, kind, name) VALUES ('group:g1','group','G')",
        [],
    )
    .ok();
    ins_msg(&conn, "gm1", "group:g1", "text", "hi", 5);
    conn.execute(
        "INSERT INTO group_outbox(msg_id, group_id, peer_id, payload, created_at) VALUES ('gm1','g1','a','{}',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size, status, attempts, next_attempt_at, created_at)
         VALUES ('t9','a','g1','/tmp/x','x',1,'pending',0,0,1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO group_reads(group_id, reader_id, last_read_ts) VALUES ('g1','a',9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO pending_group_reads(group_id, peer_id, last_read_ts) VALUES ('g1','a',9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO group_recalled_messages(conv_id, msg_id, recaller_id, seq) VALUES ('group:g1','gm1','me',5)",
        [],
    )
    .unwrap();
    set_setting(&conn, &clear_boundary_key("g1"), "5").unwrap();
    conn.execute(
        "INSERT INTO conversation_clocks(conv_id, seq) VALUES ('group:g1', 5)",
        [],
    )
    .ok();

    delete_group(&conn, "g1").unwrap();

    for (label, sql) in [
        (
            "messages 历史",
            "SELECT COUNT(*) FROM messages WHERE conv_id='group:g1'",
        ),
        ("group_outbox", "SELECT COUNT(*) FROM group_outbox"),
        (
            "file_outbox(group)",
            "SELECT COUNT(*) FROM file_outbox WHERE group_id='g1'",
        ),
        ("group_reads", "SELECT COUNT(*) FROM group_reads"),
        (
            "pending_group_reads",
            "SELECT COUNT(*) FROM pending_group_reads",
        ),
        ("撤回墓碑", "SELECT COUNT(*) FROM group_recalled_messages"),
        (
            "boundary 设置",
            "SELECT COUNT(*) FROM settings WHERE key='clear_boundary:group:g1'",
        ),
        ("群行", "SELECT COUNT(*) FROM groups WHERE id='g1'"),
        (
            "会话行",
            "SELECT COUNT(*) FROM conversations WHERE id='group:g1'",
        ),
    ] {
        assert_eq!(count(&conn, sql), 0, "{label} 未被级联清理");
    }
    // 刻意保留：逻辑时钟只增不减，删了会让重建的同名会话撞历史序号
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM conversation_clocks WHERE conv_id='group:g1'"
        ),
        1,
        "conversation_clocks 必须保留（单调性护栏）"
    );
}

#[test]
fn search_history_skips_orphan_conversations() {
    let conn = fresh_db();
    ins_msg(&conn, "o1", "group:gone", "text", "kw orphan", 1);
    assert!(
        search_history(&conn, "kw", None, None, None, 10)
            .unwrap()
            .is_empty(),
        "会话行不在列表里的孤儿消息不得出现在搜索结果（点不开还挤占计数）"
    );
    conn.execute(
        "INSERT INTO conversations(id, kind, name) VALUES ('group:gone','group','G')",
        [],
    )
    .ok();
    assert_eq!(
        search_history(&conn, "kw", None, None, None, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn migration_v7_purges_legacy_group_orphans_only() {
    use std::env;
    let mut p = env::temp_dir();
    p.push(format!(
        "gosslan-cascade-v7-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&p);
    {
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        ins_msg(&conn, "live", "p1", "text", "stays", 1);
        conn.execute(
            "INSERT INTO conversations(id, kind, name) VALUES ('p1','single','P')",
            [],
        )
        .ok();
        // 孤儿：群已不在 groups 表、会话也不在 conversations
        ins_msg(&conn, "orph", "group:dead", "text", "legacy garbage", 1);
        // ⚠️ 陷阱用例（自审 #2）：**用户删过群会话但群还在** —— 合法状态，
        // 一条都不能清（旧口径 `NOT IN conversations` 单锚会在这里造成不可逆数据丢失）
        conn.execute(
            "INSERT INTO groups(id, name, creator, created_at) VALUES ('alive','A','me',1)",
            [],
        )
        .unwrap();
        ins_msg(&conn, "kept", "group:alive", "text", "在册群历史", 1);
        conn.execute(
            "INSERT INTO group_outbox(msg_id, group_id, peer_id, payload, created_at) VALUES ('orph','dead','a','{}',1)",
            [],
        )
        .unwrap();
        conn.execute("PRAGMA user_version = 6", []).unwrap();
    }
    let conn = init(&p).unwrap();
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM messages WHERE msg_id='orph'"),
        0,
        "v7 迁移必须清掉存量孤儿群消息"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM group_outbox"), 0);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM messages WHERE msg_id='live'"),
        1,
        "在册会话的消息一条都不能误伤"
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM messages WHERE msg_id='kept'"),
        1,
        "群在册（groups 有行）而会话被删过 —— 历史必须保留，这不是孤儿"
    );
    let uv: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(uv as u32, DB_VERSION, "迁移链必须把库带到最新版本");
    drop(conn);
    let _ = std::fs::remove_file(&p);
}

#[test]
fn expired_rows_carry_peer_and_age_for_group_and_file_queues() {
    let conn = fresh_db();
    conn.execute(
        "INSERT INTO group_outbox(msg_id, group_id, peer_id, payload, created_at)
         VALUES ('gm1','g1','offline-meet','{}',1),('gm1','g1','online-a','{}',1),
               ('gm2','g1','offline-meet','{}',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size, status, attempts, next_attempt_at, created_at)
         VALUES ('t1','offline-meet','g1','/p','p',1,'pending',0,0,1)",
        [],
    )
    .unwrap();
    let g = list_expired_group_outbox(&conn, 500).unwrap();
    // 行级：同一 msg 的离线成员与在线成员各自成行，sweeper 才能做「全部放弃才算失败」
    assert_eq!(g.len(), 3);
    assert!(g
        .iter()
        .any(|(m, _g, p, _c)| m == "gm1" && p == "offline-meet"));
    let f = list_expired_file_outbox(&conn, 500).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].1, "offline-meet");
    assert_eq!(f[0].2.as_deref(), Some("g1"));
}

/// 发送行的 `sha256` 回填：只补一个字段、同值不重复写、缺行/空值静默通过。
///
/// 为什么要钉"不重复写"：回填点在投递任务里，每次 outbox 重试都会再进来一次；
/// 若同值也写，就是在拥塞链路上给全局单连接 DB 多加无谓的写放大。
#[test]
fn fill_message_sha256_backfills_once_and_keeps_other_fields() {
    let conn = fresh_db();
    conn.execute(
        "INSERT INTO conversations(id, kind, title, ts) VALUES ('p1','single','P',1)",
        [],
    )
    .ok();
    ins_msg(
        &conn,
        "file-t1",
        "p1",
        "file",
        "{\"name\":\"a.bin\",\"size\":7,\"sha256\":\"\",\"subtype\":\"bin\"}",
        1,
    );
    // 用触发器数真实写入次数（不看返回值，返回值两种情况都是 Ok）
    conn.execute("CREATE TABLE write_log(n INTEGER NOT NULL DEFAULT 0)", [])
        .unwrap();
    conn.execute("INSERT INTO write_log(n) VALUES (0)", [])
        .unwrap();
    conn.execute(
        "CREATE TRIGGER trg AFTER UPDATE ON messages BEGIN UPDATE write_log SET n = n + 1; END",
        [],
    )
    .unwrap();

    fill_message_sha256(&conn, "file-t1", "aa11").unwrap();
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE msg_id = 'file-t1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        content.contains("\"sha256\":\"aa11\""),
        "回填没生效：{content}"
    );
    assert!(
        content.contains("\"name\":\"a.bin\"") && content.contains("\"size\":7"),
        "只补一个字段，同载荷的其余键不得丢：{content}"
    );

    // 同值再进来（= outbox 重试的第二次尝试）不得再写一次
    fill_message_sha256(&conn, "file-t1", "aa11").unwrap();
    assert_eq!(
        count(&conn, "SELECT n FROM write_log"),
        1,
        "同值回填必须是 no-op"
    );

    // 空值不覆盖已有值；不存在的 msg_id 静默通过（内容补发复用同一个投递函数）
    fill_message_sha256(&conn, "file-t1", "").unwrap();
    let after: String = conn
        .query_row(
            "SELECT content FROM messages WHERE msg_id = 'file-t1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        after.contains("\"sha256\":\"aa11\""),
        "空值不得擦掉已有 cid"
    );
    fill_message_sha256(&conn, "file-missing", "bb22").unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE msg_id = 'file-missing'"
        ),
        0,
        "缺行必须静默通过，且不得凭空插入"
    );
    assert_eq!(
        count(&conn, "SELECT n FROM write_log"),
        1,
        "空值/缺行两条路径都不该产生写入"
    );
}
