//! `db::get_todo_messages_for_conns` 的行为判据（#149 功能→判据对账补的第一条）。
//!
//! 为什么先钉这一条：它是群任务「与我相关」徽标的**唯一取数口**，而它的契约恰好是
//! "整会话全量、不分页" —— 与看板读的 `chat.messages[convId]`（前端只缓存一页）
//! 正是两个源（见 #154 第 9 条：徽标亮着而列表是空的）。
//! 那条缺陷要跨实例才判得动，但它依赖的数据契约可以先钉在这里：
//! **这条读不许被改成"最新一页"**，一改徽标就会跟着列表一起漏。
//!
//! 钉住的四件事：① 只回 todo/todo_update 两种 kind；② 不受消息总量限制（全量语义）；
//! ③ 按 seq 正序且 `mention_targets` 一并带回；④ 多会话各回各的，不许串。

use super::*;
use rusqlite::Connection;

fn fresh() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn
}

/// `kind` 走 `display_kind` 那道映射，所以断言只看 `msg_id` 与顺序，不看 kind 的字面值。
fn put(conn: &Connection, msg_id: &str, conv: &str, kind: &str, seq: i64, content: &str) {
    conn.execute(
        "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq)
         VALUES (?1, ?2, 'me', 'peer', ?3, ?4, ?5, ?5)",
        params![msg_id, conv, kind, content, seq],
    )
    .unwrap();
}

fn ids(rows: &[MessageRecord]) -> Vec<String> {
    rows.iter().map(|r| r.msg_id.clone()).collect()
}

/// ① 只有两种 kind 进得来 —— 混进 text/file 会让前端 fold 出错位的任务。
#[test]
fn todo_read_returns_only_todo_kinds() {
    let conn = fresh();
    put(&conn, "t1", "group:g1", "todo", 10, "{}");
    put(&conn, "u1", "group:g1", "todo_update", 11, "{}");
    put(&conn, "x1", "group:g1", "text", 12, "正文");
    put(&conn, "f1", "group:g1", "file", 13, "{}");
    let rows = get_todo_messages_for_conns(&conn, &["group:g1".to_string()]).unwrap();
    assert_eq!(ids(&rows), vec!["t1".to_string(), "u1".to_string()]);
}

/// ② 全量语义：任务行**自己**超过任何一页的容量时也不许被截断。
///
/// ⚠️ 这一条被反证救过一次：第一版只塞了 150 条正文 + 2 条任务，
/// 给取数临时加 `LIMIT 100` 之后它**照样绿**（WHERE 先过滤掉正文，剩下 2 行够不到 100）
/// —— 也就是说它当时什么都没钉住。现在的写法是让**任务行本身**过百，
/// 并把"加 LIMIT 就红"当成这条判据的验收（反证记录见 CHANGELOG）。
#[test]
fn todo_read_is_not_a_paged_read() {
    let conn = fresh();
    // 250 条任务行：任何"按页取"的实现（100 / 200 都试得过）都会在这里露出来。
    for i in 0..250 {
        put(
            &conn,
            &format!("t-{i:03}"),
            "group:g1",
            "todo",
            1000 + i,
            "{}",
        );
    }
    // 再混进正文，确认过滤与容量是两件事（正文再多也不占任务的额度）
    for i in 0..150 {
        put(&conn, &format!("x-{i}"), "group:g1", "text", 10 + i, "正文");
    }
    let rows = get_todo_messages_for_conns(&conn, &["group:g1".to_string()]).unwrap();
    assert_eq!(
        rows.len(),
        250,
        "任务行必须全量返回；这条读一旦被改成分页取，徽标就会与看板一起漏"
    );
    assert_eq!(rows.first().unwrap().msg_id, "t-000");
    assert_eq!(rows.last().unwrap().msg_id, "t-249");
    assert!(
        rows.windows(2).all(|w| w[0].seq < w[1].seq),
        "必须按 seq 严格递增"
    );
}

/// ③ 顺序与 @ 落点列：折叠端按 seq 递增吃，`mention_targets` 不落库就丢（迁移 v11 的那一列）。
#[test]
fn todo_read_returns_ascending_seq_and_carries_mention_targets() {
    let conn = fresh();
    put(&conn, "u-later", "group:g1", "todo_update", 30, "{}");
    put(&conn, "t-earlier", "group:g1", "todo", 20, "{\"id\":\"a\"}");
    conn.execute(
        "UPDATE messages SET mention_targets = ?1 WHERE msg_id = ?2",
        params![r#"[{"id":"p2","name":"小明","n":1}]"#, "t-earlier"],
    )
    .unwrap();
    let rows = get_todo_messages_for_conns(&conn, &["group:g1".to_string()]).unwrap();
    assert_eq!(
        ids(&rows),
        vec!["t-earlier".to_string(), "u-later".to_string()]
    );
    assert_eq!(rows[0].seq, 20);
    assert_eq!(rows[1].seq, 30);
    // 读出来是**结构体**而不是 JSON 字符串 —— 这一层解析也是契约的一部分：
    // 库里存的是 v11 那列的原文，界面拿到的是已解析的 (id, name, n)。
    let targets = rows[0]
        .mention_targets
        .clone()
        .expect("@ 落点要随任务行一起回来，否则重启后只能按昵称判");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, "p2");
    assert_eq!(targets[0].name, "小明");
    assert_eq!(targets[0].n, 1);
    assert_eq!(
        rows[1].mention_targets, None,
        "没有 @ 的任务行不许被凭空造出入口"
    );
}

/// ④ 多会话各回各的：串了就等于把 A 群的任务算进 B 群的徽标。
#[test]
fn todo_read_keeps_conversations_apart() {
    let conn = fresh();
    put(&conn, "a1", "group:ga", "todo", 1, "{}");
    put(&conn, "b1", "group:gb", "todo", 2, "{}");
    put(&conn, "c1", "group:gc", "todo", 3, "{}");
    let rows =
        get_todo_messages_for_conns(&conn, &["group:ga".to_string(), "group:gc".to_string()])
            .unwrap();
    assert_eq!(ids(&rows), vec!["a1".to_string(), "c1".to_string()]);
    let empty = get_todo_messages_for_conns(&conn, &[]).unwrap();
    assert!(empty.is_empty(), "没给会话就什么都不能回");
}
