//! SQLite 存储层：本地聊天记录、好友关系、群组、离线队列与配置。
//! 使用 rusqlite（bundled，自带 SQLite 源码，跨平台零配置）。
//! 数据库迁移由 PRAGMA user_version 驱动，见 `run_migrations()` / `MIGRATIONS` 数组。

use std::path::Path;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Result};

use crate::state::{
    Conversation, Favorite, Friend, Group, GroupFile, GroupFileRecipient, MessageRecord,
    TransferInfo,
};

/// 当前数据库版本。每次 schema 变更递增一次，并在 `MIGRATIONS` 数组末尾追加一个 step。
///
/// ⚠️ **只有"SCHEMA 表达不了的事"才配一个 step**（加列、回填、清孤儿行、**删**被取代的索引）。
/// 建表 / 建索引不需要 step：`init()` 在版本分支**之前**跑 `execute_batch(SCHEMA)`，
/// 而 SCHEMA 全是 `IF NOT EXISTS` ⇒ 新老库每次启动都会被补齐。
/// 同一条索引同时写在 SCHEMA 与 MIGRATIONS 里 = 两个家（历史上有过，v8→v9 起收敛）。
pub const DB_VERSION: u32 = 9;

/// 迁移 step：(from_version, to_version, 迁移闭包)。
struct Migration {
    from: u32,
    to: u32,
    description: &'static str,
    run: fn(&Connection) -> Result<()>,
}

/// 小工具：安全地读 `pragma_table_info` 判断某表是否有某列。
fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    let exists: bool = conn
        .prepare(&format!(
            "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?1",
            table
        ))
        .and_then(|mut s| s.query_row([col], |r| r.get::<_, i64>(0)))
        .map(|n| n > 0)?;
    Ok(exists)
}

/// `init()` 的失败分类 —— **必须是有类型的**，不许让调用方按错误字符串猜。
///
/// 协议层已经在这上面摔过一次：`decode_frame` 用 `unknown variant` 前缀分类，结果把
/// "未知嵌套枚举值"也误判成"未知帧类型"，整条消息被吞（INV-P24 第 2 条，v4.22.34 才修掉）。
/// 启动期的降级判定是同一类判断，所以一开始就给它一个类型。
#[derive(Debug)]
pub enum InitError {
    /// 数据文件的 `user_version` 比本机 `DB_VERSION` 新 ⇒ **本机程序比数据旧**。
    /// 唯一正确的处置是拒绝打开（AI_RULES 的"能升不能毁"红线），但必须给用户
    /// 可读、可行动的解释（`downgrade_message`），不能只是一个错误。
    Downgrade { current: u32 },
    /// 其它 SQLite 错误（打不开、迁移失败……）。
    Sqlite(rusqlite::Error),
}

impl InitError {
    /// **只有降级这一种情况**给人看的那句话（启动期原生对话框与日志共用这一份文案）。
    ///
    /// 刻意不做成"每种错误各自配一句人话"的形状：其它 SQLite 错误没有
    /// "该升级还是该修文件"这种可行动作，硬凑一句反而把用户支到错误方向 —— 类型上直接
    /// 不给"把降级文案套到别的错误上"的机会，比写一条测试去禁止它更省事。
    ///
    /// 中英各一句不是敷衍：此刻数据库还没打开，读不到用户存的 `language` 偏好，
    /// 而这个弹窗必须在 webview 之外显示（应用已经没有可用的后端状态了）——
    /// 所以两种语言都给，而不是猜一种。
    pub fn downgrade_message(current: u32) -> String {
        format!(
                "本机 Gosslan 比这份数据旧，无法打开。\n\
                 数据版本 v{current}，本机支持到 v{DB_VERSION}。\n\
                 \n\
                 你的聊天记录没有被修改（迁移在写入任何数据之前就已中止）。\n\
                 请升级到不低于创建这份数据的 Gosslan 版本后再打开；\n\
                 如果必须留在当前版本，请先从更新版本里导出的备份恢复。\n\
                 \n\
                 --- \n\
                 This Gosslan build is older than the data file \
                 (data v{current}, this app supports up to v{DB_VERSION}), so it refuses to open it.\n\
                 Your messages were NOT modified — the check runs before any write.\n\
                 Install a Gosslan version at least as new as the one that created this data."
        )
    }
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 日志用**单行**技术描述（`downgrade_message` 那段长文案给人看，别刷进日志文件）。
        match self {
            Self::Downgrade { current } => write!(
                f,
                "DB user_version={current} > app DB_VERSION={DB_VERSION}: \
                 数据来自更新的版本，拒绝降级打开（未执行任何迁移）"
            ),
            Self::Sqlite(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Downgrade { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for InitError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// 执行迁移（**只在 `current <= DB_VERSION` 时**被调用，见 `init` 里唯一的降级判定）。
fn run_migrations(conn: &Connection, current: u32) -> Result<()> {
    if current == DB_VERSION {
        return Ok(());
    }
    for step in MIGRATIONS.iter() {
        if step.from >= DB_VERSION {
            break;
        }
        if step.from < current {
            continue;
        }
        eprintln!(
            "[gosslan-db] running v{}→v{}: {}",
            step.from, step.to, step.description
        );
        (step.run)(conn)?;
        conn.pragma_update(None, "user_version", step.to)?;
    }
    Ok(())
}

/// **SCHEMA 表达不了、又必须每条启动路径都拿到**的形状，集中在这一个地方补。
///
/// 为什么存在：`init()` 里 `execute_batch(SCHEMA)` 跑在迁移**之前**，所以任何
/// "建在只有迁移才会加的列上"的对象都进不了 SCHEMA —— 老库那一刻还没那一列，
/// 写进去会让 `init()` 直接失败（`no such column: seq`，2026-09-25 真撞过一次，
/// 判据是 `migration_tests::index_on_a_migration_added_column_must_not_live_in_schema`）。
///
/// 为什么**两条分支都要经过**：全新库走 `is_fresh` 不重放迁移，而会话内排序的
/// 热查询（`db/messages.rs` 按 `(conv_id, seq)`）没有这条索引就是全表扫 ——
/// 那比"少一条日志"严重得多。判据是恒等的：
/// `migration_tests::fresh_schema_alone_has_exactly_the_migrated_shape`
/// 逐条比对「SCHEMA + 这里」与「SCHEMA + 整条迁移链」的 `sqlite_master` 形状。
///
/// 加进来的每一条都必须是**幂等**的（`IF NOT EXISTS` / 先判存在），因为老库那条分支
/// 已经由迁移建过一遍。
fn ensure_post_schema_shape(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_messages_conv_seq ON messages(conv_id, seq);",
    )?;
    Ok(())
}

const MIGRATIONS: &[Migration] = &[
    // v1 → v2：friends 加公钥列
    Migration {
        from: 1,
        to: 2,
        description: "friends 加 x25519_pubkey / ed25519_pubkey",
        run: |conn| {
            let tx = conn.unchecked_transaction()?;
            for (col, sql) in [
                (
                    "x25519_pubkey",
                    "ALTER TABLE friends ADD COLUMN x25519_pubkey TEXT",
                ),
                (
                    "ed25519_pubkey",
                    "ALTER TABLE friends ADD COLUMN ed25519_pubkey TEXT",
                ),
            ] {
                match column_exists(&tx, "friends", col) {
                    Ok(false) => {
                        tx.execute(sql, [])?;
                    }
                    Ok(true) => {}
                    Err(e) => {
                        eprintln!("[gosslan-db] v1→v2: skip {col}: {e}");
                    }
                }
            }
            tx.commit()?;
            Ok(())
        },
    },
    // v2 → v3：messages 加 seq + 回填 + 索引。
    // ⚠️ 索引必须留在这条迁移里（建列**之后**）：SCHEMA 跑在迁移之前，
    //   把 `idx_messages_conv_seq` 写进 SCHEMA 会让没有 seq 列的老库直接开不起来。
    //   判据测试：`migration_tests::index_on_a_migration_added_column_must_not_live_in_schema`。
    Migration {
        from: 2,
        to: 3,
        description: "messages 加 seq 列 + 回填 + idx_messages_conv_seq",
        run: |conn| {
            let tx = conn.unchecked_transaction()?;
            match column_exists(&tx, "messages", "seq") {
                Ok(false) => {
                    tx.execute(
                        "ALTER TABLE messages ADD COLUMN seq INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                    tx.execute(
                        "UPDATE messages SET seq = (SELECT COUNT(*) FROM messages m2 WHERE m2.conv_id = messages.conv_id AND (m2.ts < messages.ts OR (m2.ts = messages.ts AND m2.id <= messages.id)))",
                        [],
                    )?;
                }
                Ok(true) => {}
                Err(e) => {
                    eprintln!("[gosslan-db] v2→v3: skip: {e}");
                }
            }
            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_messages_conv_seq ON messages(conv_id, seq)",
                [],
            )?;
            tx.commit()?;
            Ok(())
        },
    },
    // v3 → v4：conversations 加 pinned
    Migration {
        from: 3,
        to: 4,
        description: "conversations 加 pinned 列",
        run: |conn| {
            let tx = conn.unchecked_transaction()?;
            match column_exists(&tx, "conversations", "pinned") {
                Ok(false) => {
                    tx.execute(
                        "ALTER TABLE conversations ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }
                Ok(true) => {}
                Err(e) => {
                    eprintln!("[gosslan-db] v3→v4: skip: {e}");
                }
            }
            tx.commit()?;
            Ok(())
        },
    },
    // v4 → v5：group_files 加 scope / todo_id
    Migration {
        from: 4,
        to: 5,
        description: "group_files 加 scope / todo_id 列",
        run: |conn| {
            let tx = conn.unchecked_transaction()?;
            for (col, default) in [("scope", "'chat'"), ("todo_id", "''")] {
                match column_exists(&tx, "group_files", col) {
                    Ok(false) => {
                        let _ = tx.execute(
                            &format!("ALTER TABLE group_files ADD COLUMN {col} TEXT NOT NULL DEFAULT {default}"),
                            [],
                        );
                    }
                    Ok(true) => {}
                    Err(e) => {
                        eprintln!("[gosslan-db] v4→v5: skip {col}: {e}");
                    }
                }
            }
            tx.commit()?;
            Ok(())
        },
    },
    // v5 → v6：outbox 清重复 + 唯一索引
    Migration {
        from: 5,
        to: 6,
        description: "outbox 清重复行 + 建 idx_outbox_msg_id 唯一索引",
        run: |conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM outbox WHERE id NOT IN (SELECT MIN(id) FROM outbox GROUP BY msg_id)",
                [],
            )?;
            tx.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_msg_id ON outbox(msg_id)",
                [],
            )?;
            tx.commit()?;
            Ok(())
        },
    },
    // v6 → v7：清除历史孤儿群消息与投递残留（2026-09-19 删除级联收口）。
    // 旧版 delete_group / leave_group 不删 messages 等表，退群成员库里留着
    // 一整段搜得到、点不开的历史；delete_conversation 也不清 outbox/file_outbox。
    // 新代码已同事务收口，这里把**存量**垃圾清掉（只清群行已不确定的数据，
    // 保守口径：groups 表里没有的 group_id ⇒ 一定是孤儿）。
    Migration {
        from: 6,
        to: 7,
        description: "清理历史孤儿群消息与投递/回执残留",
        run: |conn| {
            // ⚠️ 口径修正（自审 #2）：**双锚都要不在册**才算孤儿 ——
            // `NOT IN conversations` 单独用是数据事故：用户删群会话是合法操作
            // （commands::delete_conversation），groups 行还在，v7 升级会把
            // 一整段在册群的历史不可逆清空。
            // 与 v3-v5 同风格：尽力清理、eprintln 容错，绝不 `?` 上抛把
            // db::init 变成 Err 让应用起不来（清不干净下次升级还会再来一遍）。
            let orphan_group = "id NOT IN (SELECT id FROM groups) \
                 AND ('group:' || id) NOT IN (SELECT id FROM conversations)";
            let orphan_conv = "substr(conv_id, 7) NOT IN (SELECT id FROM groups) \
                 AND conv_id NOT IN (SELECT id FROM conversations)";
            let cleanup = |sql: &str| -> rusqlite::Result<()> { conn.execute(sql, []).map(|_| ()) };
            for sql in [
                &format!("DELETE FROM messages WHERE conv_id LIKE 'group:%' AND {orphan_conv}"),
                &format!("DELETE FROM group_outbox WHERE {orphan_group}"),
                &format!("DELETE FROM file_outbox WHERE group_id IS NOT NULL AND {orphan_group}"),
                &format!("DELETE FROM group_reads WHERE {orphan_group}"),
                &format!("DELETE FROM pending_group_reads WHERE {orphan_group}"),
                &format!(
                    "DELETE FROM group_recalled_messages WHERE conv_id LIKE 'group:%' AND {orphan_conv}"
                ),
            ] {
                if let Err(e) = cleanup(sql) {
                    eprintln!("[gosslan-db] v7 孤儿清理跳过一条语句（不影响启动）：{e}");
                }
            }
            Ok(())
        },
    },
    // v7 → v8：sweeper 三条队列的 created_at 索引（2026-09-19 自审建议#4）。
    // sweeper 每 30s 按 created_at 扫三张表；离线保留窗改为 7 天后行数会显著变多，
    // 无索引就是每 tick 三次全表扫。软失败：建不动索引不拦启动（下次再来）。
    Migration {
        from: 7,
        to: 8,
        description: "outbox/group_outbox/file_outbox 建 created_at 索引（sweeper 扫描成本）",
        run: |conn| {
            for sql in [
                "CREATE INDEX IF NOT EXISTS idx_outbox_created ON outbox(created_at)",
                "CREATE INDEX IF NOT EXISTS idx_group_outbox_created ON group_outbox(created_at)",
                "CREATE INDEX IF NOT EXISTS idx_file_outbox_created ON file_outbox(created_at)",
            ] {
                if let Err(e) = conn.execute(sql, []) {
                    eprintln!("[gosslan-db] v8 索引跳过一条（不影响启动）：{e}");
                }
            }
            Ok(())
        },
    },
    // v8 → v9：删掉两条**已被取代**的索引（0-B）。
    //
    // 为什么这必须是迁移而不是 SCHEMA：`CREATE INDEX IF NOT EXISTS` 只会建不会删，
    // 而这两条留着就是热写路径上白付的写放大 ——
    // ① `idx_outbox_msg_id`（v5→v6 建的唯一索引）与 `outbox.msg_id` 的**内联 UNIQUE**
    //    是同一件事的两份。约束的权威是内联那个（`sqlite_autoindex` 一直在守），
    //    这条测试会证明删掉索引后重复插入仍然被挡。
    // ② `idx_file_outbox_peer(peer_id, status)` 是新的
    //    `idx_file_outbox_peer_due(peer_id, status, next_attempt_at)` 的严格前缀。
    // 软失败：删不动不拦启动（它们留着只是慢一点，不是错）。
    Migration {
        from: 8,
        to: 9,
        description: "删掉被取代的 idx_outbox_msg_id / idx_file_outbox_peer（写放大）",
        run: |conn| {
            for sql in [
                "DROP INDEX IF EXISTS idx_outbox_msg_id",
                "DROP INDEX IF EXISTS idx_file_outbox_peer",
            ] {
                if let Err(e) = conn.execute(sql, []) {
                    eprintln!("[gosslan-db] v9 删冗余索引跳过一条（不影响启动）：{e}");
                }
            }
            Ok(())
        },
    },
];

/// 建表脚本（与 `schema.sql` 保持一致）
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS friends (
    device_id TEXT PRIMARY KEY,
    nickname  TEXT NOT NULL,
    avatar    TEXT,
    x25519_pubkey  TEXT,
    ed25519_pubkey TEXT,
    added_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
    id        TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,           -- 'single' | 'group'
    name      TEXT NOT NULL,
    avatar    TEXT,
    last_msg  TEXT,
    last_ts   INTEGER,
    unread    INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER,
    -- 本机置顶（纯本地偏好，不广播不同步）：列表排序时优先于 last_ts
    pinned    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id      TEXT UNIQUE NOT NULL,
    conv_id     TEXT NOT NULL,
    sender_id   TEXT NOT NULL,
    receiver_id TEXT NOT NULL,
    kind        TEXT NOT NULL,          -- text | code | image | file | system
    content     TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    seq         INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'sent'
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conv_id, ts);
-- ⚠️ `idx_messages_conv_seq` **不在这里**，而且不能放进来：`seq` 是迁移 v2→v3 才加到
-- `messages` 上的列，而 `execute_batch(SCHEMA)` 跑在迁移**之前** ⇒ 老库里这句会
-- `no such column: seq` 直接让 `init()` 失败（用户打不开自己的库）。
-- 它的家在 `ensure_post_schema_shape`（两条分支都经过：老库由 v2→v3 在建列之后建，
-- 新库不重放迁移、由那个函数补）—— 顺序是语义，不是风格问题。
-- 判据测试：`migration_tests::index_on_a_migration_added_column_must_not_live_in_schema`
-- + `migration_tests::fresh_schema_alone_has_exactly_the_migrated_shape`。

-- 每会话逻辑时钟（Lamport 风格，单调递增）。消息排序与群聊清空边界都以此为准，
-- 不使用发送方或接收方的墙上时钟。
CREATE TABLE IF NOT EXISTS conversation_clocks (
    conv_id TEXT PRIMARY KEY,
    seq     INTEGER NOT NULL
);

-- 撤回的**权威集合**（G-Set，只增不减）。`messages` 行的 content 置空 / kind='recalled'
-- 只是它的**物化视图** —— 两者都必需：撤回事件可能先于被撤回消息到达
-- （Gossip 泛洪与 outbox 直发是两条无顺序保证的路径），只靠 UPDATE 会打到 0 行，
-- 随后消息正常落库 ⇒ 撤回失效。落库前查这张表即可解决「先撤后到」。
CREATE TABLE IF NOT EXISTS group_recalled_messages (
    conv_id     TEXT NOT NULL,
    msg_id      TEXT NOT NULL,
    recaller_id TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    PRIMARY KEY (conv_id, msg_id)
);
-- 判定"这条消息是否已被撤回"只给 msg_id（`is_recalled`），PK 的前缀是 conv_id 用不上
-- ⇒ 每次群收件都要全表扫这张**只增不减**的 G-Set。这条索引在收件热路径上。
CREATE INDEX IF NOT EXISTS idx_group_recalled_msg ON group_recalled_messages(msg_id);

CREATE TABLE IF NOT EXISTS groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    creator    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS group_members (
    group_id  TEXT NOT NULL,
    device_id TEXT NOT NULL,
    PRIMARY KEY (group_id, device_id)
);

-- 群成员已读位置：每个成员只保留读到的最大时间戳
CREATE TABLE IF NOT EXISTS group_reads (
    group_id      TEXT NOT NULL,
    reader_id     TEXT NOT NULL,
    last_read_ts  INTEGER NOT NULL,
    PRIMARY KEY (group_id, reader_id)
);

-- 离线补发队列：发给离线/未连接好友的消息
CREATE TABLE IF NOT EXISTS outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id     TEXT NOT NULL UNIQUE,
    peer_id    TEXT NOT NULL,
    payload    TEXT NOT NULL,          -- 序列化后的 Message JSON
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_outbox_peer ON outbox(peer_id);
CREATE INDEX IF NOT EXISTS idx_outbox_created ON outbox(created_at);

-- 群消息离线补发队列：同一 msg_id 需按成员各自维护投递状态。
CREATE TABLE IF NOT EXISTS group_outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id     TEXT NOT NULL,
    group_id   TEXT NOT NULL,
    peer_id    TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(msg_id, peer_id)
);
CREATE INDEX IF NOT EXISTS idx_group_outbox_peer ON group_outbox(peer_id);
CREATE INDEX IF NOT EXISTS idx_group_outbox_group ON group_outbox(group_id);
CREATE INDEX IF NOT EXISTS idx_group_outbox_created ON group_outbox(created_at);

CREATE TABLE IF NOT EXISTS file_transfers (
    id         TEXT PRIMARY KEY,
    peer_id    TEXT NOT NULL,
    name       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    direction  TEXT NOT NULL,          -- 'send' | 'receive'
    status     TEXT NOT NULL,          -- 'pending'|'active'|'done'|'failed'|'sent'（sent=send 侧已写出但无对端回执，终态）
    path       TEXT,
    progress   REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
-- 传输面板首屏是 `ORDER BY created_at DESC` 且**没有任何 WHERE** —— 零索引时
-- 每次打开都要全表扫 + 临时 B-tree 排序，而行数是只增的（历史传输不删）。
CREATE INDEX IF NOT EXISTS idx_file_transfers_created ON file_transfers(created_at DESC);

-- 文件离线投递队列：断线后重启可恢复，重连后自动补发。
CREATE TABLE IF NOT EXISTS file_outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    transfer_id     TEXT NOT NULL UNIQUE,
    peer_id         TEXT NOT NULL,
    group_id        TEXT,
    local_path      TEXT NOT NULL,
    name            TEXT NOT NULL,
    size            INTEGER NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'sending' | 'failed'
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
);
-- 取某 peer 待投递的文件是「每次建链 / 每次心跳」都跑一遍的读：判据是
-- peer_id + status + `next_attempt_at <= now`。原先的 (peer_id, status) 只能定位到
-- "这个 peer 的所有待发行"，时间窗还得逐行回表比 ⇒ 把 next_attempt_at 收进第三列。
-- ⚠️ 被它取代的旧 `idx_file_outbox_peer` **不在这里**（SCHEMA 造不出"删"），
-- 由迁移 v8→v9 一次性删掉；两处都写会变成每次启动"建了又删"。
CREATE INDEX IF NOT EXISTS idx_file_outbox_peer_due ON file_outbox(peer_id, status, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_file_outbox_created ON file_outbox(created_at);

-- 群文件：一个 transfer_id 对应一个群文件。
-- 每个群成员的投递状态在 group_file_recipients 中独立维护（DB 是最终状态来源）。
CREATE TABLE IF NOT EXISTS group_files (
    transfer_id TEXT PRIMARY KEY,
    group_id    TEXT NOT NULL,
    sender_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'sending' | 'completed' | 'failed'
    created_at  INTEGER NOT NULL,
    scope       TEXT NOT NULL DEFAULT 'chat',      -- 'chat' | 'todo'
    todo_id     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_group_files_group ON group_files(group_id);

-- 群文件 per-recipient 投递状态：同一 (transfer_id, recipient_id) 唯一
CREATE TABLE IF NOT EXISTS group_file_recipients (
    transfer_id  TEXT NOT NULL,
    recipient_id TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'sending' | 'completed' | 'failed'
    progress     REAL NOT NULL DEFAULT 0,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (transfer_id, recipient_id)
);
CREATE INDEX IF NOT EXISTS idx_group_file_recipients_status
    ON group_file_recipients(transfer_id, status);
-- 离线投递定向查询：peer 上线时按 recipient 精确取其 pending 群文件
CREATE INDEX IF NOT EXISTS idx_group_file_recipients_recipient
    ON group_file_recipients(recipient_id, status);

-- 待发已读回执：进程重启后从 DB 恢复，避免 ReadReceipt 丢失
CREATE TABLE IF NOT EXISTS pending_reads (
    peer_id       TEXT PRIMARY KEY,
    last_read_ts  INTEGER NOT NULL
);

-- 待发群已读回执：成员离线/链路不可用时暂存，建链/心跳时补发。
CREATE TABLE IF NOT EXISTS pending_group_reads (
    group_id      TEXT NOT NULL,
    peer_id       TEXT NOT NULL,
    last_read_ts  INTEGER NOT NULL,
    PRIMARY KEY (group_id, peer_id)
);
CREATE INDEX IF NOT EXISTS idx_pending_group_reads_peer ON pending_group_reads(peer_id);

-- 收藏（微信式）：**独立本地存储**，不随会话删除、不随消息清理消失。
--
-- 为什么是独立表而不是像置顶/待办那样发一条 kind='pin' 的静默消息：那些是"消息的派生状态"，
-- 随消息生命周期走；收藏是"用户对某条内容的**独立副本**"——原消息被删、会话被删、
-- 本机存储清理之后，收藏仍要能打开。所以这里存 content 快照，并把媒体**复制**一份到
-- 收藏专用目录（`media_path`），副本路径已改写进 `content.path`。
--
-- UNIQUE(msg_id)：同一条消息重复收藏是幂等的（INSERT OR IGNORE），不会攒出两条。
CREATE TABLE IF NOT EXISTS favorites (
    id           TEXT PRIMARY KEY,              -- uuid：删除/预览都按它定位
    msg_id       TEXT NOT NULL,
    conv_id      TEXT NOT NULL,
    sender_id    TEXT NOT NULL,
    kind         TEXT NOT NULL,                 -- text | code | image | file
    content      TEXT NOT NULL,                 -- 收藏时的快照（媒体已改写成副本路径）
    ts           INTEGER NOT NULL,              -- 原消息时间
    favorited_at INTEGER NOT NULL,              -- 收藏时间（列表排序键）
    media_path   TEXT,                          -- 收藏副本绝对路径（仅 image/file）
    media_size   INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_favorites_msg  ON favorites(msg_id);
CREATE INDEX        IF NOT EXISTS idx_favorites_time ON favorites(favorited_at DESC);
"#;

/// 打开（或创建）数据库并执行迁移。
pub fn init(path: &Path) -> std::result::Result<Connection, InitError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)?;
    // S13: 数据库版本高于当前程序 — 说明用户从更新版本的 App 降级了，旧代码不认识新
    // schema，继续使用会导致数据损坏。**拒绝继续，不降级、不删除。**
    //
    // ★ 位置是**语义的一部分**：必须早于下面任何一句写操作。`execute_batch(SCHEMA)`
    //   看着是 `CREATE TABLE IF NOT EXISTS`（对老表无害），但它确实写文件，而且一旦先跑它，
    //   "你的数据没有被改动"这句提示就成了谎话。全仓只允许这一处降级判定。
    let current: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if current > DB_VERSION {
        let err = InitError::Downgrade { current };
        eprintln!("[gosslan-db] FATAL: {err}");
        return Err(err);
    }
    // ★ 数表必须发生在 `execute_batch(SCHEMA)` **之前**：SCHEMA 会建出全部 19 张表，
    //   之后再数就区分不了"全新库"与"老库"—— 那正是本仓曾经的样子（`is_fresh` 恒为假，
    //   于是全新库把整条迁移链重放一遍：9 行假的「running v1→v2…」日志、v7 两条
    //   "孤儿清理跳过一条语句"的告警、以及先建 `idx_outbox_msg_id` 再在 v8→v9 删掉它）。
    let pre_table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute_batch(SCHEMA)?;
    // 内容传输逻辑层自己的表（schema 归它所有，保持分层）。
    crate::content::store::ensure_schema(&conn)?;
    // 判据：`user_version` 没写过 **且** 一张业务表都没有。只看前者会把
    // "老库（v1/v2 时代根本不存在 user_version）"误判成新库 ⇒ 那条库的形状就永远补不回来。
    let is_fresh = current == 0 && pre_table_count == 0;

    if is_fresh {
        conn.pragma_update(None, "user_version", DB_VERSION)?;
    } else {
        run_migrations(&conn, current)?;
    }
    // **两条分支都要经过**这里：有形状是 SCHEMA 表达不了的（见 `ensure_post_schema_shape`），
    // 而 fresh 分支不重放迁移 —— 这个函数就是那笔差额，缺了它新安装会少一条热查询索引。
    ensure_post_schema_shape(&conn)?;
    // 每次启动都把会话时钟同步到「该会话已有最大逻辑序号」
    conn.execute(
        "INSERT INTO conversation_clocks(conv_id, seq)
         SELECT conv_id, MAX(seq) FROM messages GROUP BY conv_id
         ON CONFLICT(conv_id) DO UPDATE SET seq = MAX(conversation_clocks.seq, excluded.seq)",
        [],
    )?;
    // 开启 WAL，提升并发读写
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    // NORMAL：牺牲极小崩溃一致性换取更高写入吞吐（500-1000 节点高频落库场景）
    conn.pragma_update(None, "synchronous", "NORMAL").ok();
    conn.pragma_update(None, "foreign_keys", "ON").ok();
    Ok(conn)
}

include!("db/settings.rs");

include!("db/clocks.rs");

include!("db/group_delete_boundary.rs");

include!("db/friends.rs");

include!("db/groups.rs");

include!("db/messages.rs");

include!("db/conversations.rs");

include!("db/message_delete.rs");

include!("db/offline_queue.rs");

include!("db/group_offline_queue.rs");

include!("db/file_transfer.rs");

include!("db/file_offline.rs");

include!("db/group_files.rs");

include!("db/read_receipts.rs");

include!("db/favorites.rs");

include!("db/recalls.rs");

#[cfg(test)]
mod cascade_tests;
#[cfg(test)]
mod migration_tests;
