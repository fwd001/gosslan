//! Migration 专项测试 — 覆盖 S24 规格的 9 个场景。
//!
//! 这些测试直接操作 PRAGMA user_version 和 SQLite 底层，验证：
//!   1. 全新库 → 直接标 DB_VERSION
//!   2. 旧库 user_version=0（有业务表）→ Legacy 检测 + 完整 Migration
//!   3. 旧库从 v1 → 逐版本升到 v6
//!   4. Identity（device_id/密钥）跨 Migration 保留
//!   5. 消息数据跨 Migration 保留
//!   6. 重复启动 → 不重复执行 Migration
//!   7. Migration 成功执行 → 原数据完好 + 新增列存在
//!   8. old_version > DB_VERSION → 拒绝降级（且必须是**带类型**的 InitError::Downgrade）
//!   8b. 拒绝降级**不建任何表** —— 降级判定必须早于 execute_batch(SCHEMA)
//!   8c. 降级文案必须可读可行动（两个版本号 + "数据未被修改" + 该升级）。
//!       非降级错误**没有这份文案可拿** —— `downgrade_message` 只服务降级那一种，
//!       所以"把降级文案套到文件损坏上"这件事在类型上就不成立，不需要测试去禁。
//!   9. Legacy user_version=0 + 空表 → 当作全新库
//!  10. 0-B 索引一族：热查询必须真的走索引（判据是 EXPLAIN 计划，不是"索引存在"）、
//!      新库首启即有（`is_fresh` 跳过迁移 ⇒ 只写在 MIGRATIONS 的索引新库没有）、
//!      老库靠 SCHEMA 自愈（**纯加索引不该写迁移**，两处都写=双份）、
//!      被取代的冗余索引走迁移删掉（且 `outbox.msg_id` 唯一性必须还在）、
//!      热表索引数量封顶（写放大的确定性代理，不用计时断言）。

use rusqlite::{params, Connection};
use std::env;
use std::path::PathBuf;

use super::{ensure_post_schema_shape, init, run_migrations, InitError, DB_VERSION, SCHEMA};

/// 每个测试创建一个独立临时 DB 文件，避免跨测试污染。
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_db_path() -> PathBuf {
    let mut p = env::temp_dir();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    p.push(format!("gosslan-migration-test-{pid}-{unique}.db"));
    // 清理可能的残留
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(p.with_extension("db-wal"));
    let _ = std::fs::remove_file(p.with_extension("db-shm"));
    p
}

fn read_user_version(conn: &Connection) -> u32 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// S24-TEST 1：全新数据库
// ---------------------------------------------------------------------------

#[test]
fn migration_fresh_db_gets_latest_version() {
    let path = temp_db_path();
    let conn = init(&path).expect("init fresh db");

    assert_eq!(
        read_user_version(&conn),
        DB_VERSION,
        "全新库应该直接标到 DB_VERSION={}",
        DB_VERSION
    );

    // 核心表应该都存在
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    for required in &[
        "settings",
        "friends",
        "conversations",
        "messages",
        "groups",
        "group_members",
        "outbox",
        "group_outbox",
        "file_transfers",
        "file_outbox",
        "group_files",
        "favorites",
    ] {
        assert!(
            tables.contains(&required.to_string()),
            "全新库缺少必需表 '{}'，实际表: {:?}",
            required,
            tables
        );
    }
}

// ---------------------------------------------------------------------------
// S24-TEST 2：Legacy user_version=0 但有完整旧 Schema
// ---------------------------------------------------------------------------

#[test]
fn migration_legacy_user_version_0_with_tables_runs_all_steps() {
    let path = temp_db_path();
    let conn = Connection::open(&path).unwrap();

    // 手动建 v1 Schema（没有 seq/pinned/scope 等后续列），user_version=0
    conn.execute_batch(
        "CREATE TABLE friends (device_id TEXT PRIMARY KEY, nickname TEXT NOT NULL, added_at INTEGER NOT NULL);
         CREATE TABLE conversations (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, unread INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT UNIQUE NOT NULL, conv_id TEXT NOT NULL, sender_id TEXT NOT NULL, receiver_id TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL);
         CREATE TABLE outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT NOT NULL, peer_id TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
         PRAGMA user_version = 0;",
    )
    .unwrap();

    // 验证 pre-table-count 检测
    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(table_count > 0, "应该检测到有旧表");

    // 关闭再走正式 init 流程
    drop(conn);
    let conn = init(&path).expect("init legacy db");

    assert_eq!(
        read_user_version(&conn),
        DB_VERSION,
        "Legacy 库应该跑完所有 Migration 到 v{}",
        DB_VERSION
    );

    // v1→v2: friends 应该有 x25519_pubkey / ed25519_pubkey
    let has_x = super::column_exists(&conn, "friends", "x25519_pubkey").unwrap();
    let has_y = super::column_exists(&conn, "friends", "ed25519_pubkey").unwrap();
    assert!(has_x && has_y, "v1→v2 应该加了公钥列");

    // v2→v3: messages 应该有 seq
    assert!(
        super::column_exists(&conn, "messages", "seq").unwrap(),
        "v2→v3 应该加了 seq 列"
    );

    // v3→v4: conversations 应该有 pinned
    assert!(
        super::column_exists(&conn, "conversations", "pinned").unwrap(),
        "v3→v4 应该加了 pinned 列"
    );
}

// ---------------------------------------------------------------------------
// S24-TEST 3：多版本升级（v1 → v6 全链路）
// ---------------------------------------------------------------------------

#[test]
fn migration_v1_to_v6_full_chain() {
    let path = temp_db_path();

    // 用 Connection::open 建 v1 Schema 但 user_version=1
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE friends (device_id TEXT PRIMARY KEY, nickname TEXT NOT NULL, added_at INTEGER NOT NULL);
             CREATE TABLE conversations (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, unread INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT UNIQUE NOT NULL, conv_id TEXT NOT NULL, sender_id TEXT NOT NULL, receiver_id TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL);
             CREATE TABLE outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT NOT NULL, peer_id TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }

    let conn = init(&path).expect("init v1 db");
    assert_eq!(read_user_version(&conn), DB_VERSION);

    // 逐版本验证所有列都存在
    assert!(super::column_exists(&conn, "friends", "x25519_pubkey").unwrap());
    assert!(super::column_exists(&conn, "friends", "ed25519_pubkey").unwrap());
    assert!(super::column_exists(&conn, "messages", "seq").unwrap());
    assert!(super::column_exists(&conn, "conversations", "pinned").unwrap());
    assert!(super::column_exists(&conn, "group_files", "scope").unwrap_or(true));
    // group_files 可能不存在于这个最小 Schema
}

// ---------------------------------------------------------------------------
// S24-TEST 4：Identity 保留
// ---------------------------------------------------------------------------

#[test]
fn migration_preserves_identity_data() {
    let path = temp_db_path();

    // 建一个有 identity（存 settings 里）的旧库
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE friends (device_id TEXT PRIMARY KEY, nickname TEXT NOT NULL, added_at INTEGER NOT NULL);
             PRAGMA user_version = 0;",
        )
        .unwrap();

        // 模拟 identity 数据（实际代码存在 settings 里）
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)",
            params!("device_id", "test-device-abc123"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)",
            params!(
                "x25519_secret",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)",
            params!(
                "ed25519_secret",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ),
        )
        .unwrap();
    }

    let conn = init(&path).expect("init legacy identity db");

    // Identity 必须完整保留
    let device_id: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'device_id'",
            [],
            |r| r.get(0),
        )
        .expect("device_id 应该还在");
    assert_eq!(device_id, "test-device-abc123");

    let x25519: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'x25519_secret'",
            [],
            |r| r.get(0),
        )
        .expect("x25519_secret 应该还在");
    assert!(x25519.starts_with("01234567"));

    let ed25519: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ed25519_secret'",
            [],
            |r| r.get(0),
        )
        .expect("ed25519_secret 应该还在");
    assert!(ed25519.starts_with("01234567"));

    assert_eq!(read_user_version(&conn), DB_VERSION);
}

// ---------------------------------------------------------------------------
// S24-TEST 5：消息数据保留
// ---------------------------------------------------------------------------

#[test]
fn migration_preserves_message_data() {
    let path = temp_db_path();

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE friends (device_id TEXT PRIMARY KEY, nickname TEXT NOT NULL, added_at INTEGER NOT NULL);
             CREATE TABLE conversations (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, unread INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT UNIQUE NOT NULL, conv_id TEXT NOT NULL, sender_id TEXT NOT NULL, receiver_id TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL);
             CREATE TABLE outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT NOT NULL, peer_id TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
             PRAGMA user_version = 0;",
        )
        .unwrap();

        // 插入 3 条消息
        for (i, content) in ["hello", "world", "test message"].iter().enumerate() {
            conn.execute(
                "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts)
                 VALUES (?1, ?2, ?3, ?4, 'text', ?5, ?6)",
                params!(
                    format!("msg-{i}"),
                    "conv-1",
                    "sender-A",
                    "receiver-B",
                    content,
                    1700000000 + i as i64,
                ),
            )
            .unwrap();
        }

        // 插入 outbox 数据
        conn.execute(
            "INSERT INTO outbox(msg_id, peer_id, payload, created_at) VALUES ('outbox-msg-1', 'peer-X', '{}', 1700000000)",
            [],
        )
        .unwrap();
    }

    let conn = init(&path).expect("init legacy with messages");

    // 消息数量不变
    let msg_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(msg_count, 3, "3 条消息应该全部保留");

    // 消息内容不变
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE msg_id = 'msg-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(content, "world");

    // outbox 数据保留
    let ob_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ob_count, 1, "outbox 数据应该保留");

    // v2→v3 seq 应该被回填（seq > 0）
    let seq: i64 = conn
        .query_row("SELECT seq FROM messages WHERE msg_id = 'msg-2'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(seq > 0, "v2→v3 应该回填 seq");
}

// ---------------------------------------------------------------------------
// S24-TEST 6：重复启动不重复执行 Migration
// ---------------------------------------------------------------------------

#[test]
fn migration_double_start_does_not_reapply() {
    let path = temp_db_path();

    // 第一次启动
    let conn = init(&path).expect("first init");
    assert_eq!(read_user_version(&conn), DB_VERSION);
    drop(conn);

    // 第二次启动 — Migration 不应该再跑
    let conn = init(&path).expect("second init");
    assert_eq!(
        read_user_version(&conn),
        DB_VERSION,
        "第二次启动 user_version 应该保持不变"
    );

    // 第三次
    let conn = init(&path).expect("third init");
    assert_eq!(read_user_version(&conn), DB_VERSION);
}

// ---------------------------------------------------------------------------
// S24-TEST 7：Migration 成功执行后数据完好 + 新增列存在
// ---------------------------------------------------------------------------

#[test]
fn migration_success_preserves_data_and_adds_columns() {
    let path = temp_db_path();

    // 建 v1 Schema + user_version=1
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE friends (device_id TEXT PRIMARY KEY, nickname TEXT NOT NULL, added_at INTEGER NOT NULL);
             PRAGMA user_version = 1;",
        )
        .unwrap();
        // 插入一条数据验证 rollback 后仍存在
        conn.execute(
            "INSERT INTO friends(device_id, nickname, added_at) VALUES ('f1', 'test', 0)",
            [],
        )
        .unwrap();
    }

    // 直接跑 run_migrations — 它会尝试 v1→v2
    // 但这里 conn 是独立的，走正常路径
    // column_exists 守卫让 Migration 极难自然失败，
    // 这里只验证：正常跑成功后 user_version 更新，
    // 且 friends 里的数据完好
    let conn = init(&path).expect("init should succeed");
    assert_eq!(read_user_version(&conn), DB_VERSION);

    // 原数据完好
    let n: String = conn
        .query_row(
            "SELECT nickname FROM friends WHERE device_id = 'f1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, "test");

    // v1→v2 新增列也存在
    assert!(super::column_exists(&conn, "friends", "x25519_pubkey").unwrap());
}

// ---------------------------------------------------------------------------
// S24-TEST 8：old_version > DB_VERSION 拒绝降级
// ---------------------------------------------------------------------------

#[test]
fn migration_refuses_downgrade() {
    let path = temp_db_path();

    // 建库后手动把 user_version 设得比 DB_VERSION 大
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE friends (device_id TEXT PRIMARY KEY, nickname TEXT NOT NULL, added_at INTEGER NOT NULL);
             PRAGMA user_version = 99;",
        )
        .unwrap();
    }

    // init 应该返回**带类型的**降级错误（调用方按类型分支，不许按错误字符串猜）
    let err = init(&path)
        .err()
        .expect("user_version=99 > DB_VERSION 应该拒绝打开，实际 Ok");
    assert!(
        matches!(err, InitError::Downgrade { current } if current == 99),
        "必须是 InitError::Downgrade{{current:99}}，实际 {err:?}"
    );

    // 确认数据库没被修改
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        read_user_version(&conn),
        99,
        "拒绝降级后 user_version 不应被篡改"
    );

    // 原数据完好
    drop(conn);
}

// ---------------------------------------------------------------------------
// S24-TEST 8b：拒绝降级**一个字节都不许写**（"数据未被修改"这句提示的可验证版本）
// ---------------------------------------------------------------------------

/// 这条才是真正拦住"检测写晚了"的那一条。
///
/// `init()` 里降级判定**必须早于** `execute_batch(SCHEMA)`：那句看着只是
/// `CREATE TABLE IF NOT EXISTS`（对已有表无害），但它确实写文件，而且一旦先跑，
/// `downgrade_message()` 里"迁移在写入任何数据之前就已中止"就成了谎话 —— 用户据此
/// 判断"我可以放心装新版本"，所以这句话必须是事实而不是安慰。
#[test]
fn downgrade_refusal_writes_nothing() {
    let path = temp_db_path();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
    }
    assert!(init(&path).is_err(), "v99 数据必须被拒绝");

    let conn = Connection::open(&path).unwrap();
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        tables, 0,
        "拒绝降级时不许建任何表（本机 schema 一旦写进新库，那份数据就回不去了）"
    );
}

// ---------------------------------------------------------------------------
// S24-TEST 8c：降级提示必须可读且可行动（AI_RULES：拒绝必须给解释，不能只是一个错误）
// ---------------------------------------------------------------------------

#[test]
fn downgrade_message_says_who_is_old_and_what_to_do() {
    let msg = InitError::downgrade_message(DB_VERSION + 7);
    // ① 两个版本号都要在场 —— 只说"版本不支持"用户没法判断该装哪个版本
    assert!(msg.contains(&(DB_VERSION + 7).to_string()), "{msg}");
    assert!(msg.contains(&DB_VERSION.to_string()), "{msg}");
    // ② 说清数据没动 + 该干什么
    assert!(msg.contains("没有被修改"), "必须承诺数据未被改动：{msg}");
    assert!(msg.contains("升级"), "必须给出可行动作（升级）：{msg}");
    // ③ 老文案是英文 + 靠 rusqlite 变体名传出，用户读不懂；确认它不再回来
    assert!(
        !msg.contains("refusing to open"),
        "不该把内部报错当用户文案：{msg}"
    );
    assert!(!msg.contains("InvalidParameterName"), "{msg}");
    // ④ 不许出现裸 JSON / 结构体 Debug 输出（同一类"把内部形态露给用户"的问题）
    assert!(!msg.contains('{'), "文案里不该有未替换的占位符：{msg}");
}

// ---------------------------------------------------------------------------
// 0-B：热查询索引。**判据是查询计划，不是"索引存在"** —— 索引建了但查询用不上
// 等于没建（列序不对时 SQLite 会照常 SELECT 出一个全表扫）。
// 五条查询都是抄自生产语句本体，改了生产 SQL 的 WHERE/ORDER BY 这里必须红。
// ---------------------------------------------------------------------------

/// 跑 EXPLAIN QUERY PLAN，把每一行的 detail 拼成一段文本返回。
///
/// 参数个数必须与 SQL 一致：rusqlite 对"多绑"是**报错**而不是忽略，
/// 少写一个占位符就会让这条用例红在错误的原因上（真跑过一次才写下的注释）。
fn query_plan(conn: &Connection, sql: &str, n_params: usize) -> String {
    let nulls: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Null; n_params];
    let refs: Vec<&dyn rusqlite::ToSql> = nulls.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let mut out = String::new();
    let mut rows = stmt
        .query_map(refs.as_slice(), |r| r.get::<_, String>(3))
        .unwrap();
    while let Some(Ok(detail)) = rows.next() {
        out.push_str(&detail);
        out.push('\n');
    }
    out
}

/// 这五条热查询必须走索引（0-B1 四条 + 0-B2 的会话时间线）。
#[test]
fn hot_queries_use_their_indexes() {
    let path = temp_db_path();
    let conn = init(&path).expect("init fresh db");

    // 每条：(生产 SQL 本体, 绑参个数, 期望出现在计划里的索引名)
    let cases: &[(&str, usize, &str)] = &[
        // db/recalls.rs `is_recalled` —— 在**每条群收件路径**上跑（gossip.rs）
        (
            "SELECT 1 FROM group_recalled_messages WHERE msg_id = ?1",
            1,
            "idx_group_recalled_msg",
        ),
        // db/file_transfer.rs `list_transfers` —— 传输面板首屏
        ("SELECT id FROM file_transfers ORDER BY created_at DESC", 0, "idx_file_transfers_created"),
        // content/store.rs `list_resumable_for_peer` —— 每次建链
        (
            "SELECT cid FROM content_transfers \
             WHERE peer_id=?1 AND status IN ('queued','active','incomplete') \
             ORDER BY updated_at ASC",
            1,
            "idx_content_transfers_peer",
        ),
        // db/file_offline.rs `list_pending_file_outbox` —— 每次建链 / 心跳
        (
            "SELECT transfer_id FROM file_outbox \
             WHERE peer_id = ?1 AND status = 'pending' AND next_attempt_at <= ?2 AND attempts < ?3 \
             ORDER BY created_at, id",
            3,
            "idx_file_outbox_peer_due",
        ),
        // db/messages.rs `get_messages` —— **最热的读**：打开任意会话
        (
            "SELECT id FROM messages WHERE conv_id = ?1 ORDER BY seq ASC, id ASC LIMIT ?2 OFFSET ?3",
            3,
            "idx_messages_conv_seq",
        ),
    ];

    for (sql, n, index) in cases {
        let plan = query_plan(&conn, sql, *n);
        assert!(
            plan.contains(index),
            "查询没走 {index}：计划是\n{plan}SQL: {sql}"
        );
    }
}

/// **新库首启就必须有这些索引**（判据：只跑一次 `init`，不借任何迁移的历史）。
///
/// 这条为什么值得单独钉：`idx_messages_conv_seq` / `idx_outbox_msg_id` 今天**只存在于迁移里**，
/// 新库却照样有 —— 原因是 `is_fresh` 那支**永远为假**（`pre_table_count` 在
/// `execute_batch(SCHEMA)` 之后才数表，新库此时已有 19 张表 ⇒ 走的是 else 分支，
/// 迁移链在空库上被整条重放了一遍）。也就是说"新库不缺索引"这件事现在靠的是一个
/// 没人打算依赖的巧合，而不是靠 SCHEMA。0-B 起：4 条新索引全部归 SCHEMA（新老库都靠它补齐），
/// 只有 `idx_messages_conv_seq` 留在迁移里 —— 原因见下面那条 `must_not_live_in_schema`。
#[test]
fn fresh_db_has_every_hot_query_index() {
    let path = temp_db_path();
    let conn = init(&path).expect("init fresh db");

    let have: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    for want in &[
        "idx_group_recalled_msg",
        "idx_file_transfers_created",
        "idx_content_transfers_peer",
        "idx_file_outbox_peer_due",
        "idx_messages_conv_seq",
    ] {
        assert!(
            have.iter().any(|h| h == want),
            "新库缺索引 {want}，实际: {have:?}"
        );
    }
}

/// **`SCHEMA` 是"形状"的唯一事实源**：版本已经是最新的库少了索引也会被补回来。
///
/// 为什么用 `user_version = DB_VERSION` 而不是退回上一档：那样 `run_migrations` 一进来就
/// `return Ok(())`，"索引回来了"这件事**只可能是 SCHEMA 干的**。
/// `init()` 里 `execute_batch(SCHEMA)` 在版本分支**之前**，每次启动都跑，而 SCHEMA 全是
/// `IF NOT EXISTS` ⇒ 纯加索引**不需要**迁移步骤（再加一遍就是两处都写，0-B 验收第 ④ 条要的
/// 正是"每个对象只有一个家"）。
#[test]
fn schema_alone_repairs_a_current_database() {
    let path = temp_db_path();
    {
        let conn = init(&path).expect("first init");
        // 模拟"这些索引还没有"的库，但版本**保持最新** ⇒ 迁移不会跑。
        // 不含 `idx_messages_conv_seq`：它建在迁移 v2→v3 **加的列**上，只能留在迁移里
        // （见下面 `index_on_a_migration_added_column_must_not_live_in_schema`）。
        for doomed in [
            "idx_group_recalled_msg",
            "idx_file_transfers_created",
            "idx_content_transfers_peer",
            "idx_file_outbox_peer_due",
        ] {
            conn.execute(&format!("DROP INDEX IF EXISTS {doomed}"), [])
                .unwrap();
        }
        assert_eq!(read_user_version(&conn), DB_VERSION);
    }

    let conn = init(&path).expect("re-init");
    assert_eq!(read_user_version(&conn), DB_VERSION);
    for want in [
        "idx_group_recalled_msg",
        "idx_file_transfers_created",
        "idx_content_transfers_peer",
        "idx_file_outbox_peer_due",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
                params![want],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "只靠 SCHEMA 就该把 {want} 补齐");
    }
}

/// **索引建在"迁移后加的列"上时只能留在迁移里 —— 写进 SCHEMA 会把老库锁死。**
///
/// 这条是 0-B 过程中真撞出来的：把 `idx_messages_conv_seq` 并进 SCHEMA 之后，
/// 三个"老库升级"用例全红 —— `init()` 先跑 `execute_batch(SCHEMA)`、**后**跑迁移，
/// 而 `messages.seq` 是 v2→v3 才 ADD 的列 ⇒ 老库上那句 `CREATE INDEX ... (conv_id, seq)`
/// 报 `no such column: seq`，整个 `init` 失败 = **用户打不开自己的数据库**。
/// "形状集中在 SCHEMA"是个好规矩，但它的边界是"列必须已经存在"，越界就是数据不可用。
#[test]
fn index_on_a_migration_added_column_must_not_live_in_schema() {
    let path = temp_db_path();
    {
        let conn = Connection::open(&path).unwrap();
        // v1 形状的 messages：**没有 seq 列**
        conn.execute_batch(
            "CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, msg_id TEXT UNIQUE NOT NULL, conv_id TEXT NOT NULL, sender_id TEXT NOT NULL, receiver_id TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL);
             PRAGMA user_version = 2;",
        )
        .unwrap();
    }

    // 必须开得起来（这条在任何断言之前 —— 开不起来本身就是那次回归的形态）
    let conn = init(&path).expect("没有 seq 列的老库必须能升级到最新并打开");

    assert!(
        super::column_exists(&conn, "messages", "seq").unwrap(),
        "v2→v3 应该补上 seq 列"
    );
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = 'idx_messages_conv_seq'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "补列之后索引必须跟上（顺序反了就是这条红）");
}

/// **SCHEMA 表达不了的才走迁移**：删掉两条被取代的冗余索引。
///
/// - `idx_outbox_msg_id`（迁移 v5→v6 建的唯一索引）与 `outbox.msg_id` 的**内联 UNIQUE**
///   是同一件事的两份 ⇒ 热表上白多一份写放大。
/// - `idx_file_outbox_peer(peer_id, status)` 被 `(peer_id, status, next_attempt_at)` 完全覆盖。
///
/// "删"是迁移的正当职责（`CREATE INDEX IF NOT EXISTS` 删不掉任何东西），
/// 而 `outbox.msg_id` 的唯一性必须由内联约束继续守住 —— 所以下面顺带证明它还在生效。
#[test]
fn superseded_indexes_are_dropped_and_uniquity_survives() {
    let path = temp_db_path();
    {
        let conn = init(&path).expect("first init");
        // 造出"老库历史上确实建过这两条"的形状，再把版本退回上一档
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_outbox_msg_id ON outbox(msg_id);
             CREATE INDEX IF NOT EXISTS idx_file_outbox_peer ON file_outbox(peer_id, status);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", DB_VERSION - 1)
            .unwrap();
    }

    let conn = init(&path).expect("re-init");
    assert_eq!(read_user_version(&conn), DB_VERSION, "迁移跑完必须落到最新");
    for gone in ["idx_outbox_msg_id", "idx_file_outbox_peer"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
                params![gone],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "{gone} 应被迁移删掉（它已被取代）");
    }

    // 再启一次：删掉的东西**不许被 SCHEMA 造回来**（否则每次启动都"建了又删"，
    // 而"删"这件事再也不是幂等的）。这条要求 SCHEMA 里根本不再出现这两个名字。
    drop(conn);
    let conn = init(&path).expect("third init");
    for gone in ["idx_outbox_msg_id", "idx_file_outbox_peer"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
                params![gone],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 0,
            "{gone} 在第二次启动又出现了 ⇒ SCHEMA 与迁移在互相打架"
        );
    }

    // 删索引不等于删约束：内联 UNIQUE 必须仍然挡重复
    conn.execute(
        "INSERT INTO outbox(msg_id, peer_id, payload, created_at) VALUES ('m-dup','p','{}',1)",
        [],
    )
    .unwrap();
    let dup = conn.execute(
        "INSERT INTO outbox(msg_id, peer_id, payload, created_at) VALUES ('m-dup','p','{}',2)",
        [],
    );
    assert!(
        dup.is_err(),
        "outbox.msg_id 的内联 UNIQUE 必须还生效（否则这次删除真的丢了约束）"
    );
}

/// 每条写路径上的表不得长出**第二份**被取代的索引 —— 写放大在这里是可数的。
///
/// 为什么钉数量而不是钉耗时：计时断言在 CI 上必然飘（同一台机器差几倍很常见），
/// 而"这张表上现在有几个索引"是耗时的确定性代理。真要量性能是另一次单独测（结论进 CHANGELOG）。
#[test]
fn hot_tables_carry_exactly_the_intended_indexes() {
    let path = temp_db_path();
    let conn = init(&path).expect("init fresh db");

    let count = |table: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name = ?1 \
             AND name LIKE 'idx_%'",
            params![table],
            |r| r.get(0),
        )
        .unwrap()
    };
    // 期望值 = 这次定下来的形状。要加第三个索引的人必须先回答"写路径受不受得起"。
    for (table, want) in [
        ("group_recalled_messages", 1),
        ("file_transfers", 1),
        ("content_transfers", 2),
        ("file_outbox", 2),
        ("outbox", 2),
        ("messages", 2),
    ] {
        assert_eq!(
            count(table),
            want,
            "{table} 上的显式索引数不是 {want} —— 多一个就是热写路径多一份写放大"
        );
    }
}

// ---------------------------------------------------------------------------
// S24-TEST 9：Legacy user_version=0 + 空表 → 当作全新库
// ---------------------------------------------------------------------------

#[test]
fn migration_zero_version_with_zero_tables_is_fresh() {
    let path = temp_db_path();

    // 空库，user_version=0
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
    }

    let conn = init(&path).expect("init empty db");
    assert_eq!(
        read_user_version(&conn),
        DB_VERSION,
        "user_version=0 + 零表 = 全新库，直接标 DB_VERSION"
    );
}

// ---------------------------------------------------------------------------
// S24-TEST 11：「只跑 SCHEMA」与「SCHEMA + 整条迁移链」必须长出同一个形状
// ---------------------------------------------------------------------------

/// sqlite_master 的全形状指纹（type + name + sql），排除 SQLite 自己的内部对象。
fn shape(conn: &Connection) -> Vec<(String, String, String)> {
    let mut st = conn
        .prepare(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|x| x.unwrap())
        .collect()
}

/// `init()` 的 fresh 分支设计上是「标 DB_VERSION、不重放迁移」，可今天 `is_fresh` **恒为假**
/// （数表发生在 `execute_batch(SCHEMA)` 之后 ⇒ 新库也 19 张）⇒ 全新库实际是**靠重放整条迁移链**
/// 才拿到只写在 `MIGRATIONS` 里的那几样东西。
///
/// 修法不是"把索引塞进 SCHEMA"（老库那一刻还没有 `seq` 列，塞进去 `init()` 直接失败），
/// 而是给两条分支共用一个 `ensure_post_schema_shape` —— 于是这条测试的左半边就是
/// "fresh 分支实际会经过的全部写"，右半边是"今天新库的重放结果"，两者必须逐字节相等。
/// 它红过一次是真的抓到了东西：当时的差集恰好一项 `index idx_messages_conv_seq`。
#[test]
fn fresh_schema_alone_has_exactly_the_migrated_shape() {
    let schema_only = Connection::open_in_memory().unwrap();
    schema_only.execute_batch(SCHEMA).unwrap();
    crate::content::store::ensure_schema(&schema_only).unwrap();
    ensure_post_schema_shape(&schema_only).unwrap();

    let with_migrations = Connection::open_in_memory().unwrap();
    with_migrations.execute_batch(SCHEMA).unwrap();
    crate::content::store::ensure_schema(&with_migrations).unwrap();
    // current = 0 ⇒ 与今天新库实际走的那条路一字不差
    run_migrations(&with_migrations, 0).unwrap();

    let only_migrated: Vec<_> = shape(&with_migrations)
        .into_iter()
        .filter(|x| !shape(&schema_only).contains(x))
        .collect();
    assert!(
        only_migrated.is_empty(),
        "有 {} 项形状**只有迁移链会给**（新库今天靠重放拿到）：{:?}\
         ⇒ 想让 fresh 分支真的跳过迁移，必须先把它们挪进两条路都会经过的地方",
        only_migrated.len(),
        only_migrated
            .iter()
            .map(|(ty, name, _)| format!("{ty} {name}"))
            .collect::<Vec<_>>()
    );
}

/// `pre_table_count` 必须在 `execute_batch(SCHEMA)` **之前**。
///
/// 这是 `is_fresh` 唯一的有效条件：SCHEMA 会建出全部 19 张表，只要这句数表跑到 SCHEMA
/// 后面去，`is_fresh` 就**恒为假** —— 全新库于是重放整条迁移链（假日志、v7 的两条
/// "跳过一条语句"告警、先建 `idx_outbox_msg_id` 再在 v8→v9 删掉它），而这份代价在
/// 形状上完全看不出来（迁移都是幂等的），所以只有源码顺序能钉住它。
/// 判据只取 `init()` 的函数体，避免被本文件与 SCHEMA 注释里的同名文本干扰。
#[test]
fn tables_are_counted_before_the_schema_is_applied() {
    let src = include_str!("../db.rs");
    let body = {
        let at = src
            .find("pub fn init(")
            .expect("init() 的签名变了？本测试的切片锚点失效");
        let tail = &src[at..];
        let end = tail
            .find("\ninclude!(\"db/settings.rs\")")
            .expect("init() 之后应当还有分册登记行；找不到说明切片终点锚点失效");
        &tail[..end]
    };
    // ⚠️ 锚点必须带 `conn.` 与 `?;`：`init()` 顶上那段降级注释里也**提到了**
    // `execute_batch(SCHEMA)`（不带前缀），只找 `execute_batch(SCHEMA)` 会先撞上注释、
    // 于是这条守卫红在错误的理由上 —— 实测过。
    let counted = body
        .find("let pre_table_count")
        .expect("init() 里必须数一次表（is_fresh 的条件之一）");
    let applied = body
        .find("conn.execute_batch(SCHEMA)?;")
        .expect("init() 必须应用 SCHEMA");
    assert_eq!(
        body.matches("conn.execute_batch(SCHEMA)?;").count(),
        1,
        "SCHEMA 只准应用一次；两次说明锚点已经不能唯一定位这条守卫要判的语句"
    );
    assert!(
        counted < applied,
        "数表（第 {counted} 字符）发生在应用 SCHEMA（第 {applied} 字符）之后 ⇒ is_fresh 恒为假 ⇒ 全新库重放整条迁移链"
    );
}

// ---------------------------------------------------------------------------
// #60：v6→v7 的孤儿清理**只准清掉真孤儿**。
//
// v7 的 `orphan_group` 谓词写的是 `id NOT IN (SELECT id FROM groups)`，但
// `group_outbox` / `file_outbox` 的 `id` 是 `INTEGER PRIMARY KEY AUTOINCREMENT`
// （行号），`groups.id` 是 TEXT 群 id ⇒ 两个域根本不相交。"保守口径"因此不是保守，
// 而是**每次有库从 v6 升到 v7 就清空这两张表的群行**（用户什么都没删）。
// 同一个迁移里 `orphan_conv`（messages / group_recalled_messages 用的那条）是对的
// —— 它比的是 `conv_id`。所以这条判据要同时钉住两面：在册的必须活、真孤儿必须死，
// 缺一面都挡不住"整表不删"这种反向退化。
// ---------------------------------------------------------------------------

#[test]
fn v7_orphan_cleanup_keeps_live_group_rows_and_drops_true_orphans() {
    let path = temp_db_path();
    let conn = init(&path).expect("init fresh db");

    conn.execute_batch(
        "INSERT INTO groups(id, name, creator, created_at) VALUES ('g-live','在册群','me',1);
         INSERT INTO conversations(id, kind, name) VALUES ('group:g-live','group','在册群');
         INSERT INTO group_outbox(msg_id, group_id, peer_id, payload, created_at)
             VALUES ('m-live','g-live','peer-1','{}',1);
         INSERT INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size,
                                 status, attempts, next_attempt_at, created_at)
             VALUES ('t-live','peer-1','g-live','/tmp/live','live',1,'pending',0,1,1);
         INSERT INTO group_reads VALUES ('g-live','me',1),('g-gone','me',1);
         INSERT INTO pending_group_reads VALUES ('g-live','peer-1',1),('g-gone','peer-1',1);
         INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts)
             VALUES ('x-live','group:g-live','peer-1','me','text','hi',1);
         -- 1:1 待发行（group_id 为 NULL）：v7 只清群行，这一行必须一个字节都不动
         INSERT INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size,
                                 status, attempts, next_attempt_at, created_at)
             VALUES ('t-1to1','peer-1',NULL,'/tmp/one','one',1,'pending',0,1,1);

         -- 真孤儿：群已不在册、会话也不在（delete_group 之前那版留下的历史）
         INSERT INTO group_outbox(msg_id, group_id, peer_id, payload, created_at)
             VALUES ('m-dead','g-gone','peer-1','{}',1);
         INSERT INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size,
                                 status, attempts, next_attempt_at, created_at)
             VALUES ('t-dead','peer-1','g-gone','/tmp/dead','dead',1,'pending',0,1,1);
         INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts)
             VALUES ('x-dead','group:g-gone','peer-1','me','text','bye',1);",
    )
    .unwrap();

    // 退回 v6，重放真实的 6→DB_VERSION 升级路径。
    conn.execute_batch("PRAGMA user_version = 6;").unwrap();
    run_migrations(&conn, 6).expect("v6→v7 迁移本身不许报错");

    let count = |sql: &str| -> i64 {
        conn.query_row(sql, [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{sql} 查不动：{e}"))
    };

    // ① 在册群的三行都必须还在。
    assert_eq!(
        count("SELECT COUNT(*) FROM group_outbox WHERE msg_id='m-live'"),
        1,
        "在册群（groups 有行、会话也在）的待投递消息被 v7 清掉了 ⇒ 孤儿谓词锚错了列"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM file_outbox WHERE transfer_id='t-live'"),
        1,
        "在册群的群文件待发行被 v7 清掉了 ⇒ 用户升级后群文件静默不再补发"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM messages WHERE msg_id='x-live'"),
        1,
        "在册群的历史消息被 v7 清掉了（这条走的是 orphan_conv，本来应该是对的）"
    );
    // 这两张表**没有 `id` 列**，谓词用 `id` 会让语句直接报错、被 `if let Err` 吞掉
    // ⇒ 清理从来没发生过（而且日志里只有 eprintln，没人看）。断言它们活着，
    // 才把"静默失败"这一半也钉住。
    assert_eq!(
        count("SELECT COUNT(*) FROM group_reads WHERE group_id='g-live'"),
        1,
        "在册群的已读位被删（或这张表的清理语句根本没跑通）"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM pending_group_reads WHERE group_id='g-live'"),
        1,
        "在册群的待发已读回执被删（这张表同样没有 id 列）"
    );
    // 1:1 待发行（group_id IS NULL）不属于这次清理的范围。
    assert_eq!(
        count("SELECT COUNT(*) FROM file_outbox WHERE transfer_id='t-1to1'"),
        1,
        "v7 只该清群文件行，把 1:1 待发行也带走是扩大伤害"
    );

    // ② 真孤儿必须清干净（否则①可以靠"整表不删"混过去）。
    assert_eq!(
        count("SELECT COUNT(*) FROM group_outbox WHERE group_id='g-gone'"),
        0,
        "真孤儿没被清 ⇒ 这条用例的另一半失效"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM file_outbox WHERE group_id='g-gone'"),
        0,
        "真孤儿没被清 ⇒ 这条用例的另一半失效"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM messages WHERE conv_id='group:g-gone'"),
        0,
        "真孤儿历史没被清"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM group_reads WHERE group_id='g-gone'"),
        0,
        "真孤儿的已读位没被清"
    );
    // `list_pending_group_reads` 只按 peer_id 查（db/read_receipts.rs），所以这张表
    // 留下的孤儿行会在对端每次上线时被重新捞出来 —— 它是活着的幽灵，不是死数据。
    assert_eq!(
        count("SELECT COUNT(*) FROM pending_group_reads WHERE group_id='g-gone'"),
        0,
        "真孤儿的待发群已读回执没被清 ⇒ 对端每次上线都会被重新投递一次"
    );
}
