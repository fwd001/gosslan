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

    assert!(
        fill_message_sha256(&conn, "file-t1", "aa11").unwrap(),
        "第一次回填必须报告「真的改了」—— 调用方据此决定是否通知前端"
    );
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

    // 同值再进来（= outbox 重试的第二次尝试）不得再写一次，也不得报"改了"
    assert!(
        !fill_message_sha256(&conn, "file-t1", "aa11").unwrap(),
        "同值回填必须是 no-op"
    );
    assert_eq!(
        count(&conn, "SELECT n FROM write_log"),
        1,
        "同值回填不得产生第二次写入"
    );

    // 空值不覆盖已有值；不存在的 msg_id 静默通过（内容补发复用同一个投递函数）
    assert!(
        !fill_message_sha256(&conn, "file-t1", "").unwrap(),
        "空 sha256 必须直接跳过"
    );
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
    assert!(
        !fill_message_sha256(&conn, "file-missing", "bb22").unwrap(),
        "缺行必须返回 false（没有东西被补，也就没必要通知前端）"
    );
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

/// 进度节流的 upsert 传 `path=None` 时**不得擦掉**已记录的本地路径。
///
/// 钉的事故：发送行在建行时写入真实 path，而每 250ms 的进度 upsert 一律传 None ——
/// 没有 COALESCE 时第一次 tick 就把路径擦成 NULL，前端把 `file_transfers.path` 当作
/// content 缺 path 时的唯一兜底来源（useMessageFile），于是群图片预览整类失效。
#[test]
fn transfer_progress_upsert_never_erases_the_known_path() {
    let conn = fresh_db();
    upsert_transfer(
        &conn,
        "t1",
        "p1",
        "a.bin",
        10,
        "send",
        "pending",
        Some("/d/a.bin"),
        0.0,
    )
    .unwrap();
    let read = |conn: &Connection| {
        conn.query_row("SELECT path FROM file_transfers WHERE id = 't1'", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .unwrap()
    };
    assert_eq!(read(&conn).as_deref(), Some("/d/a.bin"));

    // 进度 tick：只有 progress 变，path 必须留着
    upsert_transfer(&conn, "t1", "p1", "a.bin", 10, "send", "active", None, 0.5).unwrap();
    assert_eq!(
        read(&conn).as_deref(),
        Some("/d/a.bin"),
        "传 None 表示「这次不知道」，不是「把它清空」"
    );
    let progress: f64 = conn
        .query_row(
            "SELECT progress FROM file_transfers WHERE id = 't1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        progress, 0.5,
        "其余字段照常更新，不能为了保 path 把进度也冻住"
    );

    // 真知道新路径时（收完 rename 到最终名）必须覆盖
    upsert_transfer(
        &conn,
        "t1",
        "p1",
        "a.bin",
        10,
        "receive",
        "done",
        Some("/d/a (1).bin"),
        1.0,
    )
    .unwrap();
    assert_eq!(read(&conn).as_deref(), Some("/d/a (1).bin"));
}

/// **传输行的终态契约（第 4 步 P7）**：`done` 是唯一不可降级的状态。
///
/// 为什么只有它：`done` 有磁盘证据（长度对 + sha256 对 + `sync_all()` 之后才 rename），
/// 用户此刻能在文件管理器里打开那个文件；而 `failed` / `cancelled` / `sent` 都**没有**
/// 这种证据 —— 尤其 `failed` 必须还能改回 `active`（见下面那条反向断言）。
///
/// 钉住的形状：39 个 `upsert_transfer` 调用点里有 12 处写 `failed`、12 处写 `active`，
/// 任何一处晚到一步（清扫器 / 重复帧 / 上一轮 attempt 的残留）就会把"已收到"改成
/// "失败"并把进度条从 100% 打回 0% —— 用户看到的是一个**打开就在那儿**的文件显示失败。
#[test]
fn a_completed_transfer_row_is_never_downgraded() {
    let conn = fresh_db();
    upsert_transfer(
        &conn,
        "t1",
        "p1",
        "a.bin",
        10,
        "receive",
        "done",
        Some("/d/a.bin"),
        1.0,
    )
    .unwrap();

    let read = |col: &str| -> String {
        conn.query_row(
            &format!("SELECT {col} FROM file_transfers WHERE id = 't1'"),
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    // progress 是 REAL 列，rusqlite 不会替它转成 String ⇒ 单独一个读数器
    let progress = || -> f64 {
        conn.query_row(
            "SELECT progress FROM file_transfers WHERE id = 't1'",
            [],
            |r| r.get::<_, f64>(0),
        )
        .unwrap()
    };
    assert_eq!(read("status"), "done");

    // 晚到的失败判定：状态、进度、路径三样都不许动
    upsert_transfer(
        &conn, "t1", "p1", "a.bin", 10, "receive", "failed", None, 0.0,
    )
    .unwrap();
    assert_eq!(
        read("status"),
        "done",
        "done 行不许被晚到的 failed 判定降级（文件确实在盘上，界面却说失败）"
    );
    assert_eq!(
        progress(),
        1.0,
        "进度条不许从 100% 打回去 —— 它和 status 是同一条 DO UPDATE 的三个字段"
    );
    assert_eq!(
        read("path"),
        "/d/a.bin",
        "已完成行的本地路径必须留住（前端靠它打开文件）"
    );

    // 晚到的"重新开收"同样不许把它复活成 active
    upsert_transfer(
        &conn, "t1", "p1", "a.bin", 10, "receive", "active", None, 0.1,
    )
    .unwrap();
    assert_eq!(
        read("status"),
        "done",
        "同一 transfer_id 的迟到帧/残留不得把 done 改回 active"
    );
}

/// 反向断言：**非 done 的终态必须还能改回 `active`** —— 断点续传复用同一个 transfer_id
/// （`retry_incomplete_content` 直接取 `rec.transfer_id` 发 `ContentRequest`）。
///
/// 所以"终态不可覆盖"这条规则**不能**顺手扩到 failed/cancelled/pending：那样一判死
/// 就永远停在失败，而字节其实还在流 —— 症状恰好是本次要修的那个的反面。
/// 这条测试今天就会过，它存在的意义是当下一次有人把集合写成
/// `NOT IN ('done','failed','cancelled')` 时，让它红。
#[test]
fn a_failed_row_can_be_reactivated_by_the_next_attempt() {
    let conn = fresh_db();
    for (id, status) in [
        ("t-failed", "failed"),
        ("t-cancelled", "cancelled"),
        ("t-pending", "pending"),
    ] {
        upsert_transfer(&conn, id, "p1", "x.bin", 10, "receive", status, None, 0.0).unwrap();
        upsert_transfer(&conn, id, "p1", "x.bin", 10, "receive", "active", None, 0.0).unwrap();
        let got: String = conn
            .query_row(
                "SELECT status FROM file_transfers WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            got, "active",
            "{status} 行必须能被新一轮 attempt 改回 active，否则续传永远停在旧终态"
        );
    }
}

/// **终态写入只有一个家**：除 `db/file_transfer.rs` 之外，不许再有第二处直接
/// `UPDATE file_transfers SET status = …`。
///
/// 为什么单独钉这条：`upsert_transfer` 加上"不得降级 done"之后，契约看起来就齐了 ——
/// 可 `commands/files.rs::fail_file_job` 里还有一句**绕过它**的裸 UPDATE，
/// 同一件事有两个家时，改一个忘一个是常态（本仓 P5/§9 那族"平行实现"反复就是这个形状）。
/// 状态迁移一律走 `db/file_transfer.rs` 里的具名助手，判据测试才能只认一处。
#[test]
fn terminal_status_writes_have_one_home() {
    let files = include_str!("../commands/files.rs");
    let leaked: Vec<&str> = files
        .lines()
        .filter(|l| {
            l.contains("UPDATE file_transfers")
                && l.contains("status")
                && !l.trim_start().starts_with("//")
                && !l.trim_start().starts_with("///")
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "commands/files.rs 里出现了 {} 处绕过 db 助手的裸状态写：{:?}",
        leaked.len(),
        leaked
    );
}

/// 队列判死助手：`pending` / `active` 都要能判死（起点不是 active，所以它不是
/// `mark_transfer_failed_if_active` 的别名），而 `done` 必须挡住并回报 `false` ——
/// 回报值就是调用方"要不要 emit"的依据（与审计 A3 的「emit 由写库结果门控」同一条口径）。
#[test]
fn queue_failure_fails_live_rows_but_never_a_done_one() {
    let conn = fresh_db();
    for (id, status) in [
        ("q-pending", "pending"),
        ("q-active", "active"),
        ("q-done", "done"),
    ] {
        upsert_transfer(
            &conn,
            id,
            "p1",
            "x.bin",
            10,
            "send",
            status,
            Some("/d/x.bin"),
            0.4,
        )
        .unwrap();
    }
    assert!(mark_queued_transfer_failed(&conn, "q-pending").unwrap());
    assert!(mark_queued_transfer_failed(&conn, "q-active").unwrap());
    assert_eq!(
        conn.query_row(
            "SELECT status FROM file_transfers WHERE id IN ('q-pending','q-active')
             ORDER BY id",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
        "failed"
    );
    assert!(
        !mark_queued_transfer_failed(&conn, "q-done").unwrap(),
        "done 行不许被队列判死改写，且必须回报 false"
    );
    let kept: (String, f64, String) = conn
        .query_row(
            "SELECT status, progress, path FROM file_transfers WHERE id = 'q-done'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        kept,
        ("done".to_string(), 0.4, "/d/x.bin".to_string()),
        "状态、进度、路径三样都得原样留住"
    );
    // 库里没有的行：false，不 panic
    assert!(!mark_queued_transfer_failed(&conn, "q-missing").unwrap());
}

/// 离线文件队列的状态集合里，`cancelled` 必须是"**不会再被任何一条重取/过期查询捞起来**"的。
///
/// 这条是"取消改写成 cancelled"的**前置证据**，不是它的回归：今天 `cancelled` 还没人写，
/// 而三条队列查询的判据都是"只认 pending / sending"⇒ 写进去就等于永久出局。
/// 不先钉这一条就改判死口径，等于凭直觉引入一个新状态。
#[test]
fn cancelled_file_outbox_rows_are_never_requeued() {
    let conn = fresh_db();
    let ins = |id: &str, status: &str| {
        conn.execute(
            "INSERT INTO file_outbox(transfer_id, peer_id, local_path, name, size, status,
                                     attempts, next_attempt_at, created_at)
             VALUES(?1, 'p1', '/d/x.bin', 'x.bin', 10, ?2, 0, 0, 1)",
            params![id, status],
        )
        .unwrap();
    };
    for (id, status) in [
        ("f-pending", "pending"),
        ("f-sending", "sending"),
        ("f-failed", "failed"),
        ("f-cancelled", "cancelled"),
    ] {
        ins(id, status);
    }

    let mut due: Vec<String> = list_pending_file_outbox(&conn, "p1")
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    due.sort();
    assert_eq!(
        due,
        vec!["f-pending".to_string()],
        "flush 只准捞 pending —— cancelled/sending/failed 都不该被重发"
    );

    // created_at=1 < 今天 ⇒ 三条 pending/sending 都算过期候选，cancelled 不在其中
    let expired: Vec<String> = list_expired_file_outbox(&conn, i64::MAX)
        .unwrap()
        .into_iter()
        .map(|r| r.0.clone())
        .collect();
    assert!(
        !expired.contains(&"f-cancelled".to_string()),
        "过期清扫器不得把已取消的任务再判一次死：{expired:?}"
    );
    assert_eq!(expired.len(), 2, "只有 pending 与 sending 是清扫器的候选");

    // 崩溃恢复同理：只回滚 sending，不许把 cancelled 复活
    assert_eq!(reset_sending_to_pending(&conn).unwrap(), 1);
    let after: String = conn
        .query_row(
            "SELECT status FROM file_outbox WHERE transfer_id = 'f-cancelled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, "cancelled", "崩溃恢复不许复活用户主动取消的任务");
}

/// **用户主动取消不许记成失败**（第 4 步 P7 第 3 条）。
///
/// `cancel_file_transfer` 自己的注释写着「用户主动停止用 cancelled，自动失败用 failed」，
/// 而它紧接着调的是 `mark_file_outbox_failed` ⇒ `file_outbox.status` 落在 'failed'。
/// 三条队列查询都只认 pending/sending，所以**功能上没坏** —— 坏的是台账：
/// 下一次有人按 status 统计/排查"为什么这单失败"，读到的是一个用户自己按下的取消。
/// 注释与代码相反是本仓点过名的那类漂移（它会把下一个 AI 引去"照代码改注释"）。
#[test]
fn a_user_cancel_is_not_recorded_as_a_failure() {
    let files = include_str!("../commands/files.rs");
    let at = files
        .find("pub async fn cancel_file_transfer(")
        .expect("cancel_file_transfer 不见了 ⇒ 本测试的锚点失效");
    let tail = &files[at..];
    let stop = tail
        .find("pub ")
        .map(|i| if i == 0 { tail.len() } else { i })
        .unwrap_or(tail.len());
    // 取到下一个顶层 `pub` 之前（本文件里取消命令之后紧跟 request_content）
    let body = &tail[..stop.max(1)];
    assert!(
        !body.contains("mark_file_outbox_failed("),
        "取消路径调了 mark_file_outbox_failed ⇒ file_outbox 里用户取消被记成失败（应有的是 mark_file_outbox_cancelled）"
    );
    assert!(
        body.contains("mark_file_outbox_cancelled("),
        "取消路径必须走 mark_file_outbox_cancelled —— 与它自己那句注释同口径"
    );
}
