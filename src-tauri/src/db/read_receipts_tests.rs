//! 已读回执那条谓词的行为判据（2026-09-28 架构复审抓到）。
//!
//! 钉住的四件事：
//!   ① `failed`（本端根本没发出去）不许被后面那条已被读的消息一起点亮成「已读」；
//!   ② `recalled` 是硬终态，与 `set_message_status` 同一口径；
//!   ③ 窗口只往前推：`ts > upto_ts` 的不动，不是本端发的不动，别的会话不动；
//!   ④ `sending` 要能被推到 `read`（这是"自愈"那一半，别连同它一起挡掉）。

use super::*;
use rusqlite::Connection;

fn fresh() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn
}

fn put(conn: &Connection, msg_id: &str, sender: &str, conv: &str, ts: i64, status: &str) {
    conn.execute(
        "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status)
         VALUES (?1, ?2, ?3, 'peer', 'text', 'x', ?4, ?4, ?5)",
        params![msg_id, conv, sender, ts, status],
    )
    .unwrap();
}

fn status_of(conn: &Connection, msg_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM messages WHERE msg_id = ?1",
        params![msg_id],
        |r| r.get::<_, String>(0),
    )
    .unwrap()
}

/// ① 这条就是抓到的那个缺陷：同一会话里后面一条被读了，不许把前面**发送失败**的那条一起标成已读。
#[test]
fn read_receipt_does_not_promote_a_failed_message() {
    let conn = fresh();
    put(&conn, "m-fail", "me", "p1", 100, "failed");
    put(&conn, "m-sent", "me", "p1", 150, "sent");
    let n = mark_own_messages_read_upto(&conn, "p1", "me", 200).unwrap();
    assert_eq!(n, 1, "只该推进真发出去的那一条");
    assert_eq!(
        status_of(&conn, "m-fail"),
        "failed",
        "从未送达的话不许显示「已读」"
    );
    assert_eq!(status_of(&conn, "m-sent"), "read");
}

/// ② 硬终态与 `set_message_status` 同一口径：撤回过的行不参与批量推进。
#[test]
fn read_receipt_leaves_recalled_rows_alone() {
    let conn = fresh();
    put(&conn, "m-recalled", "me", "p1", 100, "recalled");
    let n = mark_own_messages_read_upto(&conn, "p1", "me", 200).unwrap();
    assert_eq!(n, 0);
    assert_eq!(status_of(&conn, "m-recalled"), "recalled");
}

/// ③ 作用域三条：时间窗、发送者、会话，各自都必须挡住，且已经 read 的不重复计入。
#[test]
fn read_receipt_stays_inside_its_window_conversation_and_sender() {
    let conn = fresh();
    put(&conn, "m-newer", "me", "p1", 210, "sent");
    put(&conn, "m-theirs", "peer", "p1", 90, "sent");
    put(&conn, "m-other-conv", "me", "p2", 90, "sent");
    put(&conn, "m-already", "me", "p1", 90, "read");
    let n = mark_own_messages_read_upto(&conn, "p1", "me", 200).unwrap();
    assert_eq!(
        n, 0,
        "超出窗口 / 别人发的 / 别的会话 / 已经是 read 的，都不该被算进推进数"
    );
    assert_eq!(status_of(&conn, "m-newer"), "sent");
    assert_eq!(status_of(&conn, "m-theirs"), "sent");
    assert_eq!(status_of(&conn, "m-other-conv"), "sent");
    assert_eq!(status_of(&conn, "m-already"), "read");
}

/// ④ 反面里的正面：`sending` 必须推得动 —— 回执比本端状态更新时，这是唯一的自愈路径。
#[test]
fn read_receipt_heals_a_stuck_sending_row() {
    let conn = fresh();
    put(&conn, "m-sending", "me", "p1", 100, "sending");
    let n = mark_own_messages_read_upto(&conn, "p1", "me", 200).unwrap();
    assert_eq!(n, 1);
    assert_eq!(status_of(&conn, "m-sending"), "read");
}
