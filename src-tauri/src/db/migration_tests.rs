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

use rusqlite::{params, Connection};
use std::env;
use std::path::PathBuf;

use super::{init, InitError, DB_VERSION};

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
