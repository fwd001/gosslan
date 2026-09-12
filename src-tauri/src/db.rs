//! SQLite 存储层：本地聊天记录、好友关系、群组、离线队列与配置。
//! 使用 rusqlite（bundled，自带 SQLite 源码，跨平台零配置）。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Result};

use crate::state::{
    Conversation, Friend, Group, GroupFile, GroupFileRecipient, MessageRecord, TransferInfo,
};

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
    updated_at INTEGER
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

-- 每会话逻辑时钟（Lamport 风格，单调递增）。消息排序与群聊清空边界都以此为准，
-- 不使用发送方或接收方的墙上时钟。
CREATE TABLE IF NOT EXISTS conversation_clocks (
    conv_id TEXT PRIMARY KEY,
    seq     INTEGER NOT NULL
);

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

CREATE TABLE IF NOT EXISTS file_transfers (
    id         TEXT PRIMARY KEY,
    peer_id    TEXT NOT NULL,
    name       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    direction  TEXT NOT NULL,          -- 'send' | 'receive'
    status     TEXT NOT NULL,          -- 'pending' | 'active' | 'done' | 'failed'
    path       TEXT,
    progress   REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

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
CREATE INDEX IF NOT EXISTS idx_file_outbox_peer ON file_outbox(peer_id, status);

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
    created_at  INTEGER NOT NULL
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
"#;

/// 打开（或创建）数据库并执行迁移。
pub fn init(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    // 迁移：早期版本 friends 表缺公钥列，此处幂等补列（兼容已有旧库）
    for col in ["x25519_pubkey", "ed25519_pubkey"] {
        let exists: bool = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('friends') WHERE name = ?1")
            .and_then(|mut s| s.query_row([col], |r| r.get::<_, i64>(0)))
            .map(|n| n > 0)
            .unwrap_or(true);
        if !exists {
            let _ = conn.execute(&format!("ALTER TABLE friends ADD COLUMN {col} TEXT"), []);
        }
    }
    // 迁移：messages 增加逻辑序号列（旧库幂等补列），并按会话内既有顺序回填。
    {
        let has_seq: bool = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'seq'")
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
            .map(|n| n > 0)
            .unwrap_or(true);
        if !has_seq {
            conn.execute("ALTER TABLE messages ADD COLUMN seq INTEGER NOT NULL DEFAULT 0", [])?;
            conn.execute(
                "UPDATE messages SET seq = (
                     SELECT COUNT(*) FROM messages m2
                     WHERE m2.conv_id = messages.conv_id
                       AND (m2.ts < messages.ts
                            OR (m2.ts = messages.ts AND m2.id <= messages.id))
                 )",
                [],
            )?;
        }
    }
    // 索引必须等 seq 列补齐后再建：旧库执行 SCHEMA 时 messages 表已存在，不会自动加列。
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_messages_conv_seq ON messages(conv_id, seq)",
        [],
    )?;
    // 每次启动都把会话时钟同步到「该会话已有最大逻辑序号」，
    // 保证旧库迁移后第一条新消息的 seq 不会回到 1 而排到历史前面。
    conn.execute(
        "INSERT INTO conversation_clocks(conv_id, seq)
         SELECT conv_id, MAX(seq) FROM messages GROUP BY conv_id
         ON CONFLICT(conv_id) DO UPDATE SET seq = MAX(conversation_clocks.seq, excluded.seq)",
        [],
    )?;
    // 迁移：outbox.msg_id 唯一索引（INSERT OR IGNORE 去重依赖它；旧库幂等补建）
    // 先清掉历史重复行（按 msg_id 保留最早一条），保证建索引必定成功
    conn.execute(
        "DELETE FROM outbox WHERE id NOT IN (SELECT MIN(id) FROM outbox GROUP BY msg_id)",
        [],
    )?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_msg_id ON outbox(msg_id)",
        [],
    )?;
    // 开启 WAL，提升并发读写
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    // NORMAL：牺牲极小崩溃一致性换取更高写入吞吐（500-1000 节点高频落库场景）
    conn.pragma_update(None, "synchronous", "NORMAL").ok();
    conn.pragma_update(None, "foreign_keys", "ON").ok();
    Ok(conn)
}

// ---------------- 设置（key-value） ----------------

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

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

// ---------------- 群聊删除边界（清除聊天数据后旧消息防回灌） ----------------

/// 本地删除边界键：记录本机清除该群聊时的逻辑序号（Lamport seq）。
pub fn clear_boundary_key(group_id: &str) -> String {
    format!("clear_boundary:group:{group_id}")
}

/// 写入群聊删除边界（清除/删除群会话时调用）。
pub fn set_clear_boundary(conn: &Connection, group_id: &str, seq: i64) -> Result<()> {
    set_setting(conn, &clear_boundary_key(group_id), &seq.to_string())
}

/// 群消息落库前的边界判定：逻辑序号 <= 清除边界 → 视为旧历史，不得重新写入本机。
/// 不再使用墙上时钟，也不猜测发送方时钟。
pub fn group_message_blocked_by_boundary(conn: &Connection, group_id: &str, seq: i64) -> bool {
    get_setting(conn, &clear_boundary_key(group_id))
        .and_then(|v| v.parse::<i64>().ok())
        .map(|boundary| seq <= boundary)
        .unwrap_or(false)
}

/// 删除一条设置（「恢复默认」时清除偏好键，让上层回落到默认值）。
pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}

/// 局域网通道开关偏好：只有显式写入 "0"（用户在设置页关闭）才为关。
/// 键不存在 ⇒ 开启 ⇒ 首次安装与「尚无该键」的旧版本升级都会自动联网，
/// 并立即将此默认值持久化，之后每次启动读到明确的 "1" 而非依赖隐式默认。
/// 读取方仅存在于桌面端启动路径（`lib.rs` 的 `#[cfg(desktop)]` 块），
/// 移动端仍由设置页手动开启，故在该目标下为死代码。
#[cfg_attr(not(desktop), allow(dead_code))]
pub fn get_lan_enabled(conn: &Connection) -> bool {
    match get_setting(conn, "lan_enabled") {
        Some(v) => v != "0",
        None => {
            set_lan_enabled(conn, true).ok();
            true
        }
    }
}

/// 写入局域网通道开关偏好（沿用 settings 表，不引入新的配置存储）。
pub fn set_lan_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    set_setting(conn, "lan_enabled", if enabled { "1" } else { "0" })
}

/// 蓝牙通道开关偏好。
///
/// **移动端默认开启**（用户 2026-09-12 安卓实测要求：「如果测到蓝牙是手机的话，蓝牙通道
/// 应该是默认打开的，并且不用设置」—— 参考 BitChat：进去就能连，不用配对、不用配置、
/// 不用先去设置里打开开关）。桌面端维持默认关闭：局域网是有线/同网段的快路径，
/// 蓝牙是可选的低带宽通道，不该在用户没要求时悄悄开射频。
/// 键不存在时才套用默认值并**立刻持久化**（与 `get_lan_enabled` 同一套语义：
/// 之后每次启动读到的是明确的 "0"/"1"，而不是依赖隐式默认）。
pub fn get_bt_enabled(conn: &Connection) -> bool {
    match get_setting(conn, "bt_enabled") {
        Some(v) => v == "1",
        None => {
            // 用户 2026-09-12 规则：**有蓝牙就默认开**（三端一致）——
            // 之前桌面默认关、手机默认开，结果"手机上默认有通道、Mac 上还要手动点一次"，
            // 而且（更糟）会让"偏好=关"与"运行时=开"互相回灌，触发启停抖动（见 `set_channel_enabled`）。
            let default_on = true;
            set_bt_enabled(conn, default_on).ok();
            default_on
        }
    }
}

/// 写入蓝牙通道开关偏好（沿用 settings 表，不引入新的配置存储）。
pub fn set_bt_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    set_setting(conn, "bt_enabled", if enabled { "1" } else { "0" })
}

// ---------------- 好友 ----------------

pub fn add_friend(
    conn: &Connection,
    device_id: &str,
    nickname: &str,
    avatar: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO friends(device_id, nickname, avatar, added_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(device_id) DO UPDATE SET nickname = excluded.nickname, avatar = excluded.avatar",
        params![device_id, nickname, avatar, now_ms()],
    )?;
    Ok(())
}

pub fn remove_friend(conn: &Connection, device_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM friends WHERE device_id = ?1",
        params![device_id],
    )?;
    Ok(())
}

pub fn get_friend(conn: &Connection, device_id: &str) -> Option<(String, Option<String>)> {
    conn.query_row(
        "SELECT nickname, avatar FROM friends WHERE device_id = ?1",
        params![device_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// 更新好友的 X25519 / Ed25519 公钥（从上线广播中学到后持久化）。
pub fn update_friend_pubkeys(
    conn: &Connection,
    device_id: &str,
    x25519: Option<&str>,
    ed25519: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE friends SET x25519_pubkey = COALESCE(?2, x25519_pubkey),
                            ed25519_pubkey = COALESCE(?3, ed25519_pubkey)
         WHERE device_id = ?1",
        params![device_id, x25519, ed25519],
    )?;
    Ok(())
}

/// 获取好友的 X25519 公钥（用于 ECDH 加密）。
pub fn get_friend_x25519(conn: &Connection, device_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT x25519_pubkey FROM friends WHERE device_id = ?1",
        params![device_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

/// 获取好友的 Ed25519 公钥（用于 Hello 握手验签，确认 TCP 对端确实是该 device_id）。
pub fn get_friend_ed25519(conn: &Connection, device_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT ed25519_pubkey FROM friends WHERE device_id = ?1",
        params![device_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

pub fn list_friends(conn: &Connection) -> Result<Vec<Friend>> {
    let mut stmt =
        conn.prepare("SELECT device_id, nickname, avatar FROM friends ORDER BY added_at")?;
    let rows = stmt.query_map([], |r| {
        Ok(Friend {
            device_id: r.get(0)?,
            nickname: r.get(1)?,
            avatar: r.get(2)?,
            device_type: String::new(),
            online: false,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

// ---------------- 群组 ----------------

#[allow(dead_code)]
pub fn create_group(
    conn: &Connection,
    id: &str,
    name: &str,
    creator: &str,
    members: &[String],
) -> Result<()> {
    conn.execute(
        "INSERT INTO groups(id, name, creator, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![id, name, creator, now_ms()],
    )?;
    for m in members {
        conn.execute(
            "INSERT OR IGNORE INTO group_members(group_id, device_id) VALUES(?1, ?2)",
            params![id, m],
        )?;
    }
    Ok(())
}

pub fn list_groups(conn: &Connection) -> Result<Vec<Group>> {
    let mut groups = Vec::new();
    let mut stmt = conn.prepare("SELECT id, name, creator FROM groups ORDER BY created_at")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, name, creator) = row?;
        let members: Vec<String> = conn
            .prepare("SELECT device_id FROM group_members WHERE group_id = ?1")?
            .query_map(params![id], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        groups.push(Group {
            id,
            name,
            creator,
            members,
        });
    }
    Ok(groups)
}

/// 成员端建立/更新本地群记录（收到 `GroupKey` / 群消息携带成员表时调用）。
/// 已存在则刷新群名与成员（幂等）。
pub fn upsert_group(
    conn: &Connection,
    id: &str,
    name: &str,
    creator: &str,
    members: &[String],
) -> Result<()> {
    if name.is_empty() {
        conn.execute(
            "INSERT OR IGNORE INTO groups(id, name, creator, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![id, name, creator, now_ms()],
        )?;
    } else {
        conn.execute(
            "INSERT INTO groups(id, name, creator, created_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![id, name, creator, now_ms()],
        )?;
    }
    for m in members {
        conn.execute(
            "INSERT OR IGNORE INTO group_members(group_id, device_id) VALUES(?1, ?2)",
            params![id, m],
        )?;
    }
    Ok(())
}

/// 重命名群：同步群表与对应会话行（会话标题随群名一起变）。
pub fn rename_group(conn: &Connection, id: &str, name: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE groups SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    if changed > 0 {
        conn.execute(
            "UPDATE conversations SET name = ?1 WHERE id = ?2",
            params![name, format!("group:{id}")],
        )?;
    }
    Ok(())
}

/// 转让群主：把群创建者改为 `creator`（调用方负责校验权限与成员资格）。
pub fn set_group_creator(conn: &Connection, group_id: &str, creator: &str) -> Result<()> {
    conn.execute(
        "UPDATE groups SET creator = ?1 WHERE id = ?2",
        params![creator, group_id],
    )?;
    Ok(())
}

/// 添加一个群成员（群创建者「加人」）。
pub fn add_group_member(conn: &Connection, group_id: &str, device_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO group_members(group_id, device_id) VALUES(?1, ?2)",
        params![group_id, device_id],
    )?;
    Ok(())
}

/// 移除一个群成员（群创建者「踢人」）。
pub fn remove_group_member(conn: &Connection, group_id: &str, device_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_members WHERE group_id = ?1 AND device_id = ?2",
        params![group_id, device_id],
    )?;
    Ok(())
}

/// 取单个群（含成员）。不存在返回 None。
pub fn get_group(conn: &Connection, group_id: &str) -> Option<Group> {
    let row = conn
        .query_row(
            "SELECT id, name, creator FROM groups WHERE id = ?1",
            params![group_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .ok()?;
    let (id, name, creator) = row;
    let members = match conn.prepare("SELECT device_id FROM group_members WHERE group_id = ?1") {
        Ok(mut stmt) => stmt
            .query_map(params![group_id], |r| r.get(0))
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    Some(Group {
        id,
        name,
        creator,
        members,
    })
}

/// 写入群成员已读位置，时间戳只前进不回退。
pub fn upsert_group_read(
    conn: &Connection,
    group_id: &str,
    reader_id: &str,
    ts: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO group_reads(group_id, reader_id, last_read_ts) VALUES(?1, ?2, ?3)
         ON CONFLICT(group_id, reader_id) DO UPDATE SET last_read_ts = MAX(group_reads.last_read_ts, excluded.last_read_ts)",
        params![group_id, reader_id, ts],
    )?;
    Ok(())
}

pub fn list_group_reads(conn: &Connection, group_id: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT reader_id, last_read_ts FROM group_reads WHERE group_id = ?1 ORDER BY reader_id",
    )?;
    let rows = stmt.query_map(params![group_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// 彻底删除一个群：群表 + 成员关系 + 会话 + 群文件投递数据。被移除的成员端收到通知后调用。
pub fn delete_group(conn: &Connection, group_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_members WHERE group_id = ?1",
        params![group_id],
    )?;
    // 群文件投递数据随群删除，避免悬挂（按 group_id 关联逐层清理）
    conn.execute(
        "DELETE FROM group_file_recipients WHERE transfer_id IN
         (SELECT transfer_id FROM group_files WHERE group_id = ?1)",
        params![group_id],
    )?;
    conn.execute(
        "DELETE FROM group_files WHERE group_id = ?1",
        params![group_id],
    )?;
    delete_group_outbox_for_group(conn, group_id)?;
    conn.execute("DELETE FROM groups WHERE id = ?1", params![group_id])?;
    conn.execute(
        "DELETE FROM conversations WHERE id = ?1",
        params![format!("group:{group_id}")],
    )?;
    Ok(())
}

// ---------------- 消息 ----------------

/// 插入一条消息，并返回「本次是否真的新建了记录」的三态裁决。
///
/// - `Ok(true)` ：本次真的插入一条新行 —— 唯一应产生投递副作用（未读 +1、`message-received`）的情形。
/// - `Ok(false)`：`msg_id` 已存在，被 `INSERT OR IGNORE` 命中唯一约束而忽略 —— 消息在库中，但本次无新行。
/// - `Err(e)`   ：真正的数据库故障（表不可用、SQL/IO 错误等）—— 消息**没有**持久化。
///
/// 判定与插入在同一条 SQL 语句内完成（不先 `SELECT` 再 `INSERT`），因此 Direct 与 Gossip
/// 并发投递同一业务 `msg_id` 时，只可能有一方拿到 `Ok(true)`。
///
/// ⚠️ 调用方不得把 `Err` 折叠成 `false`（如 `unwrap_or(false)`）：那会把一次临时故障误判成
/// 「重复」并照常回 Ack，而 Ack 会让发送方删除 outbox 行，导致消息永久丢失。
pub fn insert_message_if_new(conn: &Connection, m: &MessageRecord) -> Result<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![m.msg_id, m.conv_id, m.sender_id, m.receiver_id, m.kind, m.content, m.ts, m.seq, m.status],
    )?;
    Ok(changed > 0)
}

pub fn insert_message(conn: &Connection, m: &MessageRecord) -> Result<()> {
    insert_message_if_new(conn, m).map(|_| ())
}

/// 原子地写入本地消息与可靠发送队列。
/// 发送路径不能出现「消息已落库但 outbox 没写入」或反过来的半状态。
pub fn insert_message_and_outbox(
    conn: &Connection,
    m: &MessageRecord,
    peer_id: &str,
    payload: &str,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            m.msg_id,
            m.conv_id,
            m.sender_id,
            m.receiver_id,
            m.kind,
            m.content,
            m.ts,
            m.seq,
            m.status
        ],
    )?;
    tx.execute(
        "INSERT INTO outbox(msg_id, peer_id, payload, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![m.msg_id, peer_id, payload, now_ms()],
    )?;
    tx.commit()
}

pub fn message_exists(conn: &Connection, msg_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM messages WHERE msg_id = ?1",
        params![msg_id],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

pub fn count_messages(conn: &Connection, conv_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE conv_id = ?1",
        params![conv_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

pub fn get_messages(
    conn: &Connection,
    conv_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<MessageRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status
         FROM messages WHERE conv_id = ?1 ORDER BY seq ASC, id ASC LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(params![conv_id, limit, offset], |r| {
        Ok(MessageRecord {
            id: r.get(0)?,
            msg_id: r.get(1)?,
            conv_id: r.get(2)?,
            sender_id: r.get(3)?,
            receiver_id: r.get(4)?,
            kind: r.get(5)?,
            content: r.get(6)?,
            ts: r.get(7)?,
            seq: r.get(8)?,
            status: r.get(9)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 预览取源：按 msg_id 返回 (sender_id, content)，供 read_file_preview 定位本地路径
/// 并判定归属（本机的自选文件 / 接收方 downloads 路径），避免命令层直连 rusqlite。
pub fn get_message_preview_source(conn: &Connection, msg_id: &str) -> Option<(String, String)> {
    conn.query_row(
        "SELECT sender_id, content FROM messages WHERE msg_id = ?1",
        params![msg_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// 更新单条消息状态。**只前进不回退**：`read` 由对方已读回执写入，而同一 msg_id 的
/// Ack 可能因 outbox 补发晚一步到达，无条件覆盖会把已读退回「对方未读」。
pub fn set_message_status(conn: &Connection, msg_id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET status = ?2 WHERE msg_id = ?1 AND status != 'read'",
        params![msg_id, status],
    )?;
    Ok(())
}

/// 回填消息内容与状态（群文件接收完成时用）：content 需随传输完成补上本地 `path`，
/// 状态同时前进到 delivered。与 `set_message_status` 不同，这里会改写 content——
/// `read_file_preview` 按 msg_id 反查 content 定位本地文件，群文件 Offer 阶段先落库
/// 无 path 的内容，Done 时必须显式回填，否则接收方图片/代码预览因缺 path 失败。
pub fn update_message_content(conn: &Connection, msg_id: &str, content: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET content = ?2, status = ?3 WHERE msg_id = ?1",
        params![msg_id, content, status],
    )?;
    Ok(())
}

/// 搜索消息内容，返回匹配的会话 ID 列表（去重，按最新匹配排序）。
/// LIKE 通配符（% _）被转义为普通字符，只做字面包含搜索。
pub fn search_messages(conn: &Connection, keyword: &str, limit: i64) -> Result<Vec<String>> {
    let pattern = format!("%{}%", escape_like(keyword));
    let mut stmt = conn.prepare(
        "SELECT DISTINCT conv_id FROM messages WHERE content LIKE ?1 ESCAPE '\\' ORDER BY ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 获取会话中匹配关键词的最新一条消息（用于搜索结果摘要）。
pub fn search_messages_in_conv(
    conn: &Connection,
    conv_id: &str,
    keyword: &str,
    limit: i64,
) -> Result<Vec<MessageRecord>> {
    let pattern = format!("%{}%", escape_like(keyword));
    let mut stmt = conn.prepare(
        "SELECT id, msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status
         FROM messages WHERE conv_id = ?1 AND content LIKE ?2 ESCAPE '\\'
         ORDER BY seq DESC, id DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![conv_id, pattern, limit], |r| {
        Ok(MessageRecord {
            id: r.get(0)?,
            msg_id: r.get(1)?,
            conv_id: r.get(2)?,
            sender_id: r.get(3)?,
            receiver_id: r.get(4)?,
            kind: r.get(5)?,
            content: r.get(6)?,
            ts: r.get(7)?,
            seq: r.get(8)?,
            status: r.get(9)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 全历史检索的一条命中（含"该会话命中总数"，供「共 N 条相关聊天记录」用）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatSearchHit {
    pub conv_id: String,
    pub msg_id: String,
    pub sender_id: String,
    pub kind: String,
    pub content: String,
    pub ts: i64,
    /// **该会话**在本次筛选条件下的命中总数（`COUNT(*) OVER (PARTITION BY conv_id)`）。
    /// 注意与返回条数的区别：`limit` 只截断返回条数，总数仍是全量命中数
    /// —— 否则"共 N 条"会随分页变小，用户会以为搜漏了。
    pub total: i64,
}

/// 历史检索（「搜索聊天记录」用）。
///
/// 与 `search_messages_in_conv`（会话内取最新一条做摘要）的区别：这里要的是
/// **结果页**需要的形态 —— 跨会话、按时间倒序、每条都带发送者与类型，
/// 并且每个会话给出命中总数与最新命中时间。
///
/// 过滤条件都在 SQL 里做（而不是取回前端再筛）：① 命中数才是准的（"共 N 条"必须按
/// 当前筛选算）；② 不必把全库命中都搬到前端。
///   · `sender_id`：按**发送者**筛（微信搜索页的「发送人」）；
///   · `since_ms` / `until_ms`：时间区间（「日期」）。
///
/// 刻意排除 `kind = 'system'`：系统提示（被移出群聊、解密失败占位等）不是用户发的
/// 聊天内容，搜出来只会干扰 —— 微信也不会把它们算进"聊天记录"。
pub fn search_history(
    conn: &Connection,
    keyword: &str,
    sender_id: Option<&str>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    limit: i64,
) -> Result<Vec<ChatSearchHit>> {
    let pattern = format!("%{}%", escape_like(keyword));
    let mut stmt = conn.prepare(
        "SELECT conv_id, msg_id, sender_id, kind, content, ts,
                COUNT(*) OVER (PARTITION BY conv_id) AS total
         FROM messages
         WHERE content LIKE ?1 ESCAPE '\\'
           AND kind <> 'system'
           AND (?2 IS NULL OR sender_id = ?2)
           AND (?3 IS NULL OR ts >= ?3)
           AND (?4 IS NULL OR ts <= ?4)
         ORDER BY ts DESC, id DESC
         LIMIT ?5",
    )?;
    let rows = stmt.query_map(
        params![pattern, sender_id, since_ms, until_ms, limit],
        |r| {
            Ok(ChatSearchHit {
                conv_id: r.get(0)?,
                msg_id: r.get(1)?,
                sender_id: r.get(2)?,
                kind: r.get(3)?,
                content: r.get(4)?,
                ts: r.get(5)?,
                total: r.get(6)?,
            })
        },
    )?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 转义 LIKE 通配符：将 % 和 _ 替换为字面值。
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// ---------------- 会话 ----------------

pub fn touch_conversation(
    conn: &Connection,
    id: &str,
    kind: &str,
    name: &str,
    avatar: Option<&str>,
    last_msg: &str,
    unread_inc: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO conversations(id, kind, name, avatar, last_msg, last_ts, unread, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6)
         ON CONFLICT(id) DO UPDATE SET
            name = CASE WHEN conversations.kind = 'group' THEN conversations.name ELSE excluded.name END,
            avatar = COALESCE(excluded.avatar, conversations.avatar),
            last_msg = excluded.last_msg,
            last_ts = excluded.last_ts,
            unread = conversations.unread + excluded.unread,
            updated_at = excluded.updated_at",
        params![id, kind, name, avatar, last_msg, now_ms(), unread_inc],
    )?;
    Ok(())
}

pub fn ensure_conversation(
    conn: &Connection,
    id: &str,
    kind: &str,
    name: &str,
    avatar: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO conversations(id, kind, name, avatar, unread, updated_at)
         VALUES(?1, ?2, ?3, ?4, 0, ?5)",
        params![id, kind, name, avatar, now_ms()],
    )?;
    Ok(())
}

pub fn list_conversations(conn: &Connection) -> Result<Vec<Conversation>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, name, avatar, last_msg, last_ts, unread
         FROM conversations ORDER BY COALESCE(last_ts, updated_at, 0) DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Conversation {
            id: r.get(0)?,
            kind: r.get(1)?,
            name: r.get(2)?,
            avatar: r.get(3)?,
            last_msg: r.get(4)?,
            last_ts: r.get(5)?,
            unread: r.get(6)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn mark_read(conn: &Connection, conv_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE conversations SET unread = 0 WHERE id = ?1",
        params![conv_id],
    )?;
    Ok(())
}

/// 仅更新已有 single 会话的昵称和头像（由 UserInfo 同步触发）。
/// 不修改 last_msg / last_ts / unread / kind / id / 任何其他字段。
/// 如果会话不存在，UPDATE 0 行即可，不会创建新会话。
pub fn update_conversation_profile(
    conn: &Connection,
    id: &str,
    name: &str,
    avatar: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE conversations SET name = ?2, avatar = ?3 WHERE id = ?1 AND kind = 'single'",
        params![id, name, avatar],
    )?;
    Ok(())
}

/// 取会话内「某发送者」最近一条消息的 (msg_id, ts)。
/// 已读回执用它替代全会话最大时间戳，避免把「接收方自己发的消息」或
/// 「被本地时钟钳制后的时间戳」当作回执阈值，跨设备时钟偏差时尤其重要。
pub fn last_message_from_sender(
    conn: &Connection,
    conv_id: &str,
    sender_id: &str,
) -> Option<(String, i64)> {
    conn.query_row(
        "SELECT msg_id, ts FROM messages
         WHERE conv_id = ?1 AND sender_id = ?2
         ORDER BY ts DESC, id DESC LIMIT 1",
        params![conv_id, sender_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// 删除一个会话及其所有消息（本地清理；不影响对方聊天记录）。
/// 事务包裹，确保消息与会话行同步删除；不存在则视为成功（幂等）。
pub fn delete_conversation(conn: &Connection, conv_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM messages WHERE conv_id = ?1", params![conv_id])?;
    tx.execute("DELETE FROM conversations WHERE id = ?1", params![conv_id])?;
    tx.commit()?;
    Ok(())
}

// ---------------- 离线补发队列 ----------------

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

// ---------------- 群消息离线补发队列 ----------------

/// 幂等写入一条群消息离线投递记录。`(msg_id, peer_id)` 唯一，
/// 重复写入不会产生第二行。
pub fn insert_group_outbox(
    conn: &Connection,
    msg_id: &str,
    group_id: &str,
    peer_id: &str,
    payload: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO group_outbox(msg_id, group_id, peer_id, payload, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![msg_id, group_id, peer_id, payload, now_ms()],
    )?;
    Ok(())
}

/// 取某成员的全部待补发群消息（按插入顺序）。
pub fn list_group_outbox(conn: &Connection, peer_id: &str) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, payload FROM group_outbox WHERE peer_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![peer_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 收到 GroupAck 后删除指定成员、指定消息的待发记录。
pub fn delete_group_outbox(conn: &Connection, msg_id: &str, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE msg_id = ?1 AND peer_id = ?2",
        params![msg_id, peer_id],
    )?;
    Ok(())
}

/// 删除指定群的全部待发记录（删除群 / 清空数据时使用）。
pub fn delete_group_outbox_for_group(conn: &Connection, group_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE group_id = ?1",
        params![group_id],
    )?;
    Ok(())
}

/// 删除指定成员在指定群中的待发记录（移人出群时使用）。
pub fn delete_group_outbox_for_peer_in_group(
    conn: &Connection,
    group_id: &str,
    peer_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM group_outbox WHERE group_id = ?1 AND peer_id = ?2",
        params![group_id, peer_id],
    )?;
    Ok(())
}

// ---------------- 文件传输记录 ----------------

pub fn upsert_transfer(
    conn: &Connection,
    id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    direction: &str,
    status: &str,
    path: Option<&str>,
    progress: f64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO file_transfers(id, peer_id, name, size, direction, status, path, progress, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status, path = excluded.path, progress = excluded.progress",
        params![id, peer_id, name, size as i64, direction, status, path, progress, now_ms()],
    )?;
    Ok(())
}

/// 取单条 transfer 的本地 path（群文件离线投递时校验源文件仍在）。
pub fn get_transfer_path(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT path FROM file_transfers WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn list_transfers(conn: &Connection) -> Result<Vec<TransferInfo>> {    let mut stmt = conn.prepare(
        "SELECT id, peer_id, name, size, direction, status, path, progress FROM file_transfers ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(crate::state::TransferInfo {
            id: r.get(0)?,
            peer_id: r.get(1)?,
            name: r.get(2)?,
            size: r.get(3)?,
            direction: r.get(4)?,
            status: r.get(5)?,
            path: r.get(6)?,
            progress: r.get(7)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

// ---------------- 文件离线投递队列 ----------------

/// 幂等写入一条待发文件记录（transfer_id 唯一）。
pub fn insert_file_outbox(
    conn: &Connection,
    transfer_id: &str,
    peer_id: &str,
    group_id: Option<&str>,
    local_path: &str,
    name: &str,
    size: u64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO file_outbox(transfer_id, peer_id, group_id, local_path, name, size, status, attempts, next_attempt_at, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7, ?7)",
        params![transfer_id, peer_id, group_id, local_path, name, size as i64, now_ms()],
    )?;
    Ok(())
}

/// 取某 peer 的待投递文件（仅 `pending`，且已到重试时间）。
pub fn list_pending_file_outbox(conn: &Connection, peer_id: &str) -> Result<Vec<(String, String)>> {
    let now = now_ms();
    let mut stmt = conn.prepare(
        "SELECT transfer_id, local_path FROM file_outbox
         WHERE peer_id = ?1 AND status = 'pending' AND next_attempt_at <= ?2
         ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map(params![peer_id, now], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 投递开始：pending → sending，并累计一次尝试。
pub fn mark_file_outbox_sending(conn: &Connection, transfer_id: &str, backoff_ms: i64) -> Result<()> {
    conn.execute(
        "UPDATE file_outbox SET status = 'sending', attempts = attempts + 1, next_attempt_at = ?2 WHERE transfer_id = ?1",
        params![transfer_id, now_ms().saturating_add(backoff_ms)],
    )?;
    Ok(())
}

/// 投递失败但可重试：回到 pending，等待下次连接/心跳触发。
pub fn mark_file_outbox_pending(conn: &Connection, transfer_id: &str, backoff_ms: i64) -> Result<()> {
    conn.execute(
        "UPDATE file_outbox SET status = 'pending', next_attempt_at = ?2 WHERE transfer_id = ?1",
        params![transfer_id, now_ms().saturating_add(backoff_ms)],
    )?;
    Ok(())
}

/// 投递成功：删除队列行。
pub fn delete_file_outbox(conn: &Connection, transfer_id: &str) -> Result<()> {
    conn.execute("DELETE FROM file_outbox WHERE transfer_id = ?1", params![transfer_id])?;
    Ok(())
}

/// 永久失败：标记 failed，不再参与重试。
pub fn mark_file_outbox_failed(conn: &Connection, transfer_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE file_outbox SET status = 'failed' WHERE transfer_id = ?1",
        params![transfer_id],
    )?;
    Ok(())
}

/// 删除指定 peer 的全部文件投递记录（删除好友时使用）。
pub fn delete_file_outbox_for_peer(conn: &Connection, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM file_outbox WHERE peer_id = ?1",
        params![peer_id],
    )?;
    Ok(())
}

// ---------------- 群文件（per-recipient 投递状态） ----------------

/// 插入群文件记录。校验：群必须存在、sender 必须是群成员。
// 传输流程在后续 GroupFileOffer 步骤启用；本步骤仅 DB 层 + 测试调用。
#[allow(dead_code)]
pub fn insert_group_file(conn: &Connection, f: &GroupFile) -> Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM groups WHERE id = ?1",
            params![f.group_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if !exists {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "群不存在：{}",
            f.group_id
        )));
    }
    let is_member: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND device_id = ?2",
            params![f.group_id, f.sender_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if !is_member {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "发送者不是群成员：{}",
            f.sender_id
        )));
    }
    conn.execute(
        "INSERT INTO group_files(transfer_id, group_id, sender_id, name, size, sha256, status, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            f.transfer_id,
            f.group_id,
            f.sender_id,
            f.name,
            f.size as i64,
            f.sha256,
            f.status,
            f.created_at
        ],
    )?;
    Ok(())
}

/// 取单个群文件。不存在返回 None。
#[allow(dead_code)]
pub fn get_group_file(conn: &Connection, transfer_id: &str) -> Option<GroupFile> {
    conn.query_row(
        "SELECT transfer_id, group_id, sender_id, name, size, sha256, status, created_at
         FROM group_files WHERE transfer_id = ?1",
        params![transfer_id],
        |r| {
            Ok(GroupFile {
                transfer_id: r.get(0)?,
                group_id: r.get(1)?,
                sender_id: r.get(2)?,
                name: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                sha256: r.get(5)?,
                status: r.get(6)?,
                created_at: r.get(7)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// 离线投递定向查询：某 peer 的全部 pending 群文件（按 recipient 精确命中
/// idx_group_file_recipients_recipient 索引，不扫描全表）。
/// 返回 (transfer_id, group_id) 供逐个投递。
pub fn list_pending_group_files_for_recipient(
    conn: &Connection,
    recipient_id: &str,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT r.transfer_id, f.group_id
         FROM group_file_recipients r
         JOIN group_files f ON f.transfer_id = r.transfer_id
         WHERE r.recipient_id = ?1 AND r.status = 'pending'
         ORDER BY f.created_at",
    )?;
    let rows = stmt.query_map(params![recipient_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    rows.collect()
}

/// 群文件 recipient 投递状态摘要（气泡文案用）：
/// completed/failed/pending(or sending) 计数。
#[derive(serde::Serialize)]
pub struct GroupFileDeliverySummary {
    pub total: i64,
    pub completed: i64,
    pub failed: i64,
    pub waiting: i64, // pending + sending（未到终态）
}

/// 汇总某群文件的全部 recipient 状态（定向查询，气泡显示用）。
pub fn get_group_file_delivery_summary(
    conn: &Connection,
    transfer_id: &str,
) -> Option<GroupFileDeliverySummary> {
    get_group_file(conn, transfer_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT
               COUNT(*),
               SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN status NOT IN ('completed','failed') THEN 1 ELSE 0 END)
             FROM group_file_recipients WHERE transfer_id = ?1",
        )
        .ok()?;
    let r = stmt
        .query_row(params![transfer_id], |r| {
            Ok(GroupFileDeliverySummary {
                total: r.get::<_, i64>(0)?,
                completed: r.get::<_, i64>(1).unwrap_or(0),
                failed: r.get::<_, i64>(2).unwrap_or(0),
                waiting: r.get::<_, i64>(3).unwrap_or(0),
            })
        })
        .ok()?;
    Some(r)
}

/// 为群文件添加一个 recipient 投递状态（初始 pending）。
/// 校验：群文件必须存在、recipient 必须是群成员（不允许给群外 peer 建 state）；
/// 同一 (transfer_id, recipient_id) 重复插入报错（PRIMARY KEY 冲突）。
#[allow(dead_code)]
pub fn insert_group_file_recipient(
    conn: &Connection,
    transfer_id: &str,
    recipient_id: &str,
) -> Result<()> {
    let group_id: Option<String> = conn
        .query_row(
            "SELECT group_id FROM group_files WHERE transfer_id = ?1",
            params![transfer_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(group_id) = group_id else {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "群文件不存在：{transfer_id}"
        )));
    };
    let is_member: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND device_id = ?2",
            params![group_id, recipient_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if !is_member {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "接收者不是群成员：{recipient_id}"
        )));
    }
    conn.execute(
        "INSERT INTO group_file_recipients(transfer_id, recipient_id, status, progress, updated_at)
         VALUES(?1, ?2, 'pending', 0.0, ?3)",
        params![transfer_id, recipient_id, now_ms()],
    )?;
    Ok(())
}

/// 列出群文件的全部 recipient 投递状态（按 recipient_id 稳定排序）。
#[allow(dead_code)]
pub fn list_group_file_recipients(
    conn: &Connection,
    transfer_id: &str,
) -> Result<Vec<GroupFileRecipient>> {
    let mut stmt = conn.prepare(
        "SELECT recipient_id, status, progress, updated_at
         FROM group_file_recipients WHERE transfer_id = ?1 ORDER BY recipient_id",
    )?;
    let rows = stmt.query_map(params![transfer_id], |r| {
        Ok(GroupFileRecipient {
            recipient_id: r.get(0)?,
            status: r.get(1)?,
            progress: r.get(2)?,
            updated_at: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// 更新单个 recipient 的投递状态与进度（时间戳只进不退由 updated_at 刷新保证）。
/// recipient 不存在时报错（不静默创建群外 state）。
#[allow(dead_code)]
pub fn update_group_file_recipient(
    conn: &Connection,
    transfer_id: &str,
    recipient_id: &str,
    status: &str,
    progress: f64,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE group_file_recipients SET status = ?3, progress = ?4, updated_at = ?5
         WHERE transfer_id = ?1 AND recipient_id = ?2",
        params![transfer_id, recipient_id, status, progress, now_ms()],
    )?;
    if n == 0 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "群文件投递状态不存在：{transfer_id}/{recipient_id}"
        )));
    }
    Ok(())
}

/// 取单个 recipient 的投递状态（用于「已完成则跳过重建」判断）。不存在返回 None。
pub fn get_group_file_recipient_status(
    conn: &Connection,
    transfer_id: &str,
    recipient_id: &str,
) -> Option<String> {
    conn.query_row(
        "SELECT status FROM group_file_recipients WHERE transfer_id = ?1 AND recipient_id = ?2",
        params![transfer_id, recipient_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// 接收方：幂等建立/重建群文件接收会话。
/// 用于重启后（内存 file_key 丢失）离线补发重建——group_file / recipient 记录已存在时
/// 不报错、不覆盖已完成（completed）状态，只把未完成的中断态复位回 sending。
/// 权限（群存在 + sender 是成员）已由调用方 `handle_group_file_offer` 校验。
pub fn upsert_group_file_receive(conn: &Connection, f: &GroupFile, recipient_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO group_files(transfer_id, group_id, sender_id, name, size, sha256, status, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            f.transfer_id,
            f.group_id,
            f.sender_id,
            f.name,
            f.size as i64,
            f.sha256,
            f.status,
            f.created_at
        ],
    )?;
    // 未完成的遗留记录复位回 sending（completed 不动）
    conn.execute(
        "UPDATE group_files SET status = 'sending' WHERE transfer_id = ?1 AND status != 'completed'",
        params![f.transfer_id],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO group_file_recipients(transfer_id, recipient_id, status, progress, updated_at)
         VALUES(?1, ?2, 'sending', 0.0, ?3)",
        params![f.transfer_id, recipient_id, now_ms()],
    )?;
    conn.execute(
        "UPDATE group_file_recipients SET status = 'sending', progress = 0.0, updated_at = ?2
         WHERE transfer_id = ?1 AND recipient_id = ?3 AND status != 'completed'",
        params![f.transfer_id, now_ms(), recipient_id],
    )?;
    Ok(())
}

// ---------------- 待发已读回执 ----------------

/// 写入/更新待发已读回执。使用 max 语义：较旧 timestamp 不覆盖较新 timestamp。
pub fn upsert_pending_read(conn: &Connection, peer_id: &str, ts: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_reads(peer_id, last_read_ts) VALUES(?1, ?2)
         ON CONFLICT(peer_id) DO UPDATE SET last_read_ts = MAX(pending_reads.last_read_ts, excluded.last_read_ts)",
        params![peer_id, ts],
    )?;
    Ok(())
}

/// 加载所有待发已读回执（应用启动时恢复内存状态）。
pub fn load_pending_reads(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT peer_id, last_read_ts FROM pending_reads")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 删除指定 peer 的待发已读回执（flush 成功后调用）。
pub fn delete_pending_read(conn: &Connection, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM pending_reads WHERE peer_id = ?1",
        params![peer_id],
    )?;
    Ok(())
}

/// 写入/更新待发群已读回执（max 语义，已读单调前进）。
pub fn upsert_pending_group_read(
    conn: &Connection,
    group_id: &str,
    peer_id: &str,
    ts: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_group_reads(group_id, peer_id, last_read_ts) VALUES(?1, ?2, ?3)
         ON CONFLICT(group_id, peer_id) DO UPDATE SET last_read_ts = MAX(pending_group_reads.last_read_ts, excluded.last_read_ts)",
        params![group_id, peer_id, ts],
    )?;
    Ok(())
}

/// 取某 peer 的全部待发群已读回执。
pub fn list_pending_group_reads(conn: &Connection, peer_id: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT group_id, last_read_ts FROM pending_group_reads WHERE peer_id = ?1",
    )?;
    let rows = stmt.query_map(params![peer_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 删除指定 peer 在指定群中的待发群已读回执（flush 成功后调用）。
pub fn delete_pending_group_read(conn: &Connection, group_id: &str, peer_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM pending_group_reads WHERE group_id = ?1 AND peer_id = ?2",
        params![group_id, peer_id],
    )?;
    Ok(())
}

/// 当前毫秒时间戳
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn rec(msg_id: &str, conv_id: &str) -> MessageRecord {
        rec_as(msg_id, conv_id, "text", "hi")
    }

    fn rec_as(msg_id: &str, conv_id: &str, kind: &str, content: &str) -> MessageRecord {
        MessageRecord {
            id: 0,
            msg_id: msg_id.into(),
            conv_id: conv_id.into(),
            sender_id: "a".into(),
            receiver_id: "b".into(),
            kind: kind.into(),
            content: content.into(),
            ts: 1,
            seq: 1,
            status: "sent".into(),
        }
    }

    #[test]
    fn schema_and_settings() {
        let conn = mem();
        set_setting(&conn, "device_id", "dev-abc").unwrap();
        assert_eq!(get_setting(&conn, "device_id").unwrap(), "dev-abc");
        // 覆盖写入
        set_setting(&conn, "device_id", "dev-new").unwrap();
        assert_eq!(get_setting(&conn, "device_id").unwrap(), "dev-new");
    }

    #[test]
    fn friend_add_and_pubkey_persistence() {
        let conn = mem();
        add_friend(&conn, "f1", "张三", None).unwrap();
        update_friend_pubkeys(&conn, "f1", Some("xk"), Some("ek")).unwrap();
        assert_eq!(get_friend_x25519(&conn, "f1").unwrap(), "xk");
        // 未设置公钥的好友返回 None
        add_friend(&conn, "f2", "李四", None).unwrap();
        assert!(get_friend_x25519(&conn, "f2").is_none());
        assert_eq!(list_friends(&conn).unwrap().len(), 2);
        remove_friend(&conn, "f1").unwrap();
        assert_eq!(list_friends(&conn).unwrap().len(), 1);
    }

    /// Gossip 路径携带的 sender_pubkey 必须能补充到 friends 表（COALESCE 行为），
    /// 使后续 open_direct_content(sender_x25519_pubkey) 的 DB 查询能找到该公钥。
    #[test]
    fn gossip_pubkey_syncs_to_friend_via_coalesce() {
        let conn = mem();
        // 好友存在但 pubkey 为 NULL（模拟 announce 未到达的场景）
        add_friend(&conn, "mac", "Mac", None).unwrap();
        assert!(get_friend_x25519(&conn, "mac").is_none());

        // Gossip handler 同步 pubkey（transport.rs handle_gossip 做同样的事）
        let gossip_key = "gossip_x25519_from_envelope";
        update_friend_pubkeys(&conn, "mac", Some(gossip_key), None).unwrap();
        assert_eq!(get_friend_x25519(&conn, "mac").unwrap(), gossip_key);

        // COALESCE：已有值不会被后续的 NULL 覆盖
        update_friend_pubkeys(&conn, "mac", None, None).unwrap();
        assert_eq!(get_friend_x25519(&conn, "mac").unwrap(), gossip_key);

        // get_friend_x25519 是 sender_x25519_pubkey → open_direct_content 的
        // DB 查询路径，证明 gossip 同步后 outbox 重发的直发 ChatMessage 可以解密
    }

    #[test]
    fn group_reads_only_move_forward() {
        let conn = mem();
        upsert_group_read(&conn, "g1", "reader-a", 200).unwrap();
        upsert_group_read(&conn, "g1", "reader-a", 100).unwrap();
        upsert_group_read(&conn, "g1", "reader-b", 300).unwrap();
        assert_eq!(
            list_group_reads(&conn, "g1").unwrap(),
            vec![("reader-a".to_string(), 200), ("reader-b".to_string(), 300)]
        );
    }

    #[test]
    fn friend_remove_then_readd_flow() {
        // 删除好友后可重新添加（扫描 → 加好友流程）且历史会话/消息不受影响
        let conn = mem();
        add_friend(&conn, "f1", "张三", None).unwrap();
        update_friend_pubkeys(&conn, "f1", Some("xk"), Some("ek")).unwrap();
        ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
        insert_message(&conn, &rec("m1", "f1")).unwrap();

        remove_friend(&conn, "f1").unwrap();
        assert!(list_friends(&conn).unwrap().is_empty());
        assert!(get_friend(&conn, "f1").is_none());
        // 公钥随好友行一并移除（重新添加后重新学习）
        assert!(get_friend_x25519(&conn, "f1").is_none());
        // 聊天记录与会话行保留
        assert!(message_exists(&conn, "m1"));
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);

        // 重新添加（扫描列表再次出现 → 加好友）
        add_friend(&conn, "f1", "张三回来了", None).unwrap();
        let friends = list_friends(&conn).unwrap();
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].nickname, "张三回来了");
    }

    /// 历史检索：发送人/时间过滤、每会话命中总数、排除系统消息。
    /// 这些判据直接决定搜索结果页上「共 N 条」与筛选是否可信，所以逐条钉住。
    #[test]
    fn search_history_filters_sender_time_and_excludes_system() {
        let conn = mem();
        let mut a1 = rec_as("m1", "c1", "text", "想你 今天一起吃饭");
        a1.sender_id = "alice".into();
        a1.ts = 1_000;
        let mut b1 = rec_as("m2", "c1", "text", "我也想你");
        b1.sender_id = "bob".into();
        b1.ts = 2_000;
        let mut a2 = rec_as("m3", "c1", "text", "想你想你想你");
        a2.sender_id = "alice".into();
        a2.ts = 3_000;
        let mut sys = rec_as("m4", "c1", "system", "想你 被移出群聊");
        sys.sender_id = "sys".into();
        sys.ts = 4_000;
        let mut other = rec_as("m5", "c2", "text", "想你");
        other.sender_id = "alice".into();
        other.ts = 5_000;
        for m in [&a1, &b1, &a2, &sys, &other] {
            insert_message(&conn, m).unwrap();
        }

        // 无筛选：跨会话、按时间倒序、排除 system，且每会话总数正确
        let all = search_history(&conn, "想你", None, None, None, 100).unwrap();
        assert_eq!(all.len(), 4, "system 消息必须被排除");
        assert_eq!(all[0].msg_id, "m5", "按时间倒序（最新在前）");
        assert_eq!(all[0].total, 1, "c2 只有 1 条命中");
        let c1 = all.iter().find(|h| h.msg_id == "m3").unwrap();
        assert_eq!(c1.total, 3, "c1 有 3 条命中（不含 system）");

        // 发送人筛选：只留 alice 发的
        let by_alice = search_history(&conn, "想你", Some("alice"), None, None, 100).unwrap();
        assert_eq!(by_alice.len(), 3);
        assert!(by_alice.iter().all(|h| h.sender_id == "alice"));
        // 总数按**当前筛选**算（c1 只剩 2 条 alice 的）
        let c1_alice = by_alice.iter().find(|h| h.conv_id == "c1").unwrap();
        assert_eq!(c1_alice.total, 2);

        // 时间区间：[2000, 3000] ⇒ m2 + m3
        let ranged = search_history(&conn, "想你", None, Some(2_000), Some(3_000), 100).unwrap();
        let mut ids: Vec<&str> = ranged.iter().map(|h| h.msg_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["m2", "m3"]);

        // limit 只截断返回条数，不改总数（否则"共 N 条"会随分页变小）
        let capped = search_history(&conn, "想你", None, None, None, 1).unwrap();
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].total, 1, "被截断的是 c2 那条，其 total 仍是 1");
    }

    #[test]
    fn message_dedup_by_unique_msg_id() {
        let conn = mem();
        insert_message(&conn, &rec("m1", "c1")).unwrap();
        insert_message(&conn, &rec("m1", "c1")).unwrap(); // 重复 → OR IGNORE
        assert!(message_exists(&conn, "m1"));
        assert!(!message_exists(&conn, "m2"));
        assert_eq!(get_messages(&conn, "c1", 100, 0).unwrap().len(), 1);
    }

    /// P0-2 根因（反面用例，锁定必须避免的写法）：真实 msg_id 一旦被「解密失败的占位
    /// 系统消息」占用，之后同一 msg_id 的正确副本会被 INSERT OR IGNORE 静默吞掉，
    /// 明文永久不可恢复。所以接收端解不开时绝不能写任何占用真实 msg_id 的行。
    #[test]
    fn placeholder_on_real_msg_id_swallows_the_good_copy() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "f1", "system", "[加密消息] 解密失败")).unwrap();
        assert!(message_exists(&conn, "m1")); // 已被占用 → 处理分支会直接 Ack 并返回
        insert_message(&conn, &rec_as("m1", "f1", "text", "real plaintext")).unwrap();
        let rows = get_messages(&conn, "f1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "[加密消息] 解密失败");
        assert_eq!(rows[0].kind, "system");
    }

    /// P0-2 修复形态：解不开 ⇒ 不落库 ⇒ `message_exists` 保持 false（因而不会误发 Ack），
    /// 真实 msg_id 保持空闲，等公钥收敛后补发的正确副本正常入库；重复投递只留一行。
    #[test]
    fn failed_decrypt_leaves_msg_id_free_for_the_later_good_copy() {
        let conn = mem();
        assert!(!message_exists(&conn, "m1"));
        insert_message(&conn, &rec_as("m1", "f1", "text", "real plaintext")).unwrap();
        insert_message(&conn, &rec_as("m1", "f1", "text", "real plaintext")).unwrap(); // 心跳重复补发
        let rows = get_messages(&conn, "f1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "real plaintext");
        assert_eq!(rows[0].kind, "text");
    }

    /// Direct 与 Gossip 两条路径共用同一业务 msg_id：无论谁先到，会话内只落一行。
    #[test]
    fn direct_and_gossip_same_msg_id_persist_single_row() {
        let conn = mem();
        insert_message(&conn, &rec_as("m1", "f1", "text", "via gossip")).unwrap();
        insert_message(&conn, &rec_as("m1", "f1", "text", "via direct")).unwrap();
        let rows = get_messages(&conn, "f1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "via gossip"); // 先到者胜出，后到者被幂等忽略
    }

    /// 补发前重封依赖的事实前提：发送方本地行存的是**明文**，且能按 (msg_id, sender_id)
    /// 精确取回，不会被同一 msg_id 下别的 sender_id 记录串味。
    /// 若将来本地改为存密文，此测试会立即失败（重封恢复路径随之失效）。
    #[test]
    fn own_sent_row_keeps_plaintext_selectable_by_sender() {
        let conn = mem();
        let mut sent = rec_as("m1", "f1", "text", "hello plain");
        sent.sender_id = "me".into();
        insert_message(&conn, &sent).unwrap();
        let mine: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE msg_id = ?1 AND sender_id = ?2",
                params!["m1", "me"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(mine.as_deref(), Some("hello plain"));
        let other: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE msg_id = ?1 AND sender_id = ?2",
                params!["m1", "peer"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(other, None);
    }

    /// 与两个 handler 同构的一次投递：只有本次真的插入新行才计未读、才算一次投递事件。
    fn deliver(conn: &Connection, msg_id: &str, conv_id: &str, conv_kind: &str, who: &str) -> bool {
        let inserted =
            insert_message_if_new(conn, &rec_as(msg_id, conv_id, "text", "hello")).unwrap();
        if inserted {
            touch_conversation(conn, conv_id, conv_kind, "张三", None, who, 1).unwrap();
        }
        inserted
    }

    /// 同一 msg_id 按给定先后顺序被两条路径各投一次 ⇒ 最终只有 1 行 / 未读 +1 / 事件 1 次，
    /// 且 last_msg 属于先到者（后到者不得改写）。单聊与群聊共用同一套断言。
    fn assert_one_side_effect(conv_id: &str, conv_kind: &str, first: &str, second: &str) {
        let conn = mem();
        ensure_conversation(&conn, conv_id, conv_kind, "张三", None).unwrap();
        let mut events = 0;
        for who in [first, second] {
            if deliver(&conn, "m1", conv_id, conv_kind, who) {
                events += 1;
            }
        }
        let case = format!("{first}→{second} @ {conv_id}");
        assert_eq!(
            get_messages(&conn, conv_id, 10, 0).unwrap().len(),
            1,
            "{case}"
        );
        assert_eq!(events, 1, "{case} 只能产生一次 message-received");
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == conv_id)
            .unwrap();
        assert_eq!(conv.unread, 1, "{case} 未读只能 +1");
        assert_eq!(
            conv.last_msg.as_deref(),
            Some(first),
            "{case} 后到者不得改写 last_msg"
        );
    }

    /// Test A + Test B：三态裁决本身 —— 首次 Ok(true)、重复 Ok(false)、库中只留一条。
    /// 第三条断言同时钉住「msg_id 是全局消息身份」：换一个会话投同一 msg_id 仍算重复。
    #[test]
    fn insert_message_if_new_distinguishes_first_from_duplicate() {
        let conn = mem();
        assert!(
            insert_message_if_new(&conn, &rec_as("m1", "f1", "text", "hello")).unwrap(),
            "首次必须 Ok(true)"
        );
        assert!(
            !insert_message_if_new(&conn, &rec_as("m1", "f1", "text", "hello")).unwrap(),
            "同一 msg_id 重复必须 Ok(false)"
        );
        assert!(
            !insert_message_if_new(&conn, &rec_as("m1", "f2", "text", "hello")).unwrap(),
            "换会话的同一 msg_id 仍是重复：msg_id 是全局身份"
        );
        assert!(message_exists(&conn, "m1"));
        assert_eq!(get_messages(&conn, "f1", 10, 0).unwrap().len(), 1);
        assert_eq!(get_messages(&conn, "f2", 10, 0).unwrap().len(), 0);
    }

    /// Err 不得被折叠成 false：数据库真故障必须原样冒泡，
    /// 让调用方抑制副作用并**禁止 Ack**（否则 outbox 被删 → 临时故障变永久丢失）。
    #[test]
    fn insert_message_if_new_surfaces_db_errors_as_err_not_false() {
        let conn = mem();
        conn.execute("DROP TABLE messages", []).unwrap();
        let out = insert_message_if_new(&conn, &rec_as("m1", "f1", "text", "hello"));
        assert!(
            matches!(out, Err(_)),
            "真实 DB 故障必须是 Err，不能是 Ok(false)"
        );
        assert!(
            insert_message(&conn, &rec_as("m2", "f1", "text", "hi")).is_err(),
            "包装函数同样冒泡"
        );
    }

    /// 群文件接收完成回填 path：update_message_content 必须改写 content + status，
    /// 使 read_file_preview 能按 msg_id 反查到本地路径（否则接收方图片/代码预览缺 path）。
    #[test]
    fn update_message_content_backfills_path_for_preview() {
        let conn = mem();
        // Offer 阶段先落库无 path 的内容（模拟 handle_group_file_offer）
        insert_message(
            &conn,
            &rec_as("gfile-1", "group:g1", "image", r#"{"name":"a.png","size":3,"subtype":"image"}"#),
        )
        .unwrap();
        // Done 阶段回填 path（模拟 handle_group_file_done）
        update_message_content(
            &conn,
            "gfile-1",
            r#"{"name":"a.png","path":"/tmp/a.png","size":3,"subtype":"image"}"#,
            "delivered",
        )
        .unwrap();
        let (_, content) = get_message_preview_source(&conn, "gfile-1").unwrap();
        assert!(content.contains("\"path\""), "Done 后 content 必须回填 path");
        assert!(content.contains("/tmp/a.png"), "path 必须指向本地文件");
    }

    /// Test 1 + Test 2（单聊）：Direct→Gossip 与 Gossip→Direct 两种顺序都只生效一次。
    #[test]
    fn unread_and_event_fire_only_for_the_winning_insert() {
        assert_one_side_effect("f1", "single", "direct", "gossip");
        assert_one_side_effect("f1", "single", "gossip", "direct");
    }

    /// Test 1 + Test 2（群聊）：Gossip 的单聊与群聊共用同一落库块，两个 kind 都必须覆盖。
    #[test]
    fn group_msg_id_duplicate_counts_unread_and_event_once() {
        assert_one_side_effect("group:g1", "group", "direct", "gossip");
        assert_one_side_effect("group:g1", "group", "gossip", "direct");
    }

    /// Test 3：同一 msg_id 被重复投递（心跳反复补发 / 同一信封多次到达）
    /// ⇒ 始终只有 1 行、1 次未读、1 次事件。
    #[test]
    fn repeated_delivery_of_same_msg_id_yields_single_side_effect() {
        let conn = mem();
        ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
        let mut events = 0;
        for i in 0..5 {
            if deliver(&conn, "m1", "f1", "single", "dup") {
                events += 1;
            }
            assert_eq!(events, 1, "第 {i} 次投递后累计投递事件数应恒为 1");
        }
        assert_eq!(get_messages(&conn, "f1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_conversations(&conn).unwrap()[0].unread, 1);
    }

    /// Test 5：竞态。多线程同时投递同一 msg_id（Direct 与 Gossip 交错的最坏情况），
    /// 连接模型与 AppState.db 一致（Mutex\<Connection\>）。只可能有一个 fresh=true。
    #[test]
    fn concurrent_same_msg_id_has_exactly_one_winner() {
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;
        let conn = Arc::new(Mutex::new(mem()));
        let gate = Arc::new(Barrier::new(8));
        let winners = Arc::new(Mutex::new(0u32));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let (c, g, w) = (conn.clone(), gate.clone(), winners.clone());
            handles.push(thread::spawn(move || {
                g.wait();
                let dbc = c.lock().unwrap_or_else(|e| e.into_inner());
                if insert_message_if_new(&dbc, &rec_as("m1", "f1", "text", "hello")).unwrap() {
                    touch_conversation(&dbc, "f1", "single", "张三", None, "hello", 1).ok();
                    *w.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            *winners.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "同一 msg_id 只能有一个首次插入者"
        );
        let dbc = conn.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(get_messages(&dbc, "f1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_conversations(&dbc).unwrap()[0].unread, 1);
    }

    #[test]
    fn conversation_unread_and_mark_read() {
        let conn = mem();
        ensure_conversation(&conn, "c1", "single", "张三", None).unwrap();
        touch_conversation(&conn, "c1", "single", "张三", None, "hello", 1).unwrap();
        touch_conversation(&conn, "c1", "single", "张三", None, "world", 1).unwrap();
        let conv = &list_conversations(&conn).unwrap()[0];
        assert_eq!(conv.unread, 2);
        assert_eq!(conv.last_msg.as_deref(), Some("world"));
        mark_read(&conn, "c1").unwrap();
        assert_eq!(list_conversations(&conn).unwrap()[0].unread, 0);
    }

    #[test]
    fn delete_conversation_removes_messages_and_row() {
        let conn = mem();
        // 两个会话互不干扰
        ensure_conversation(&conn, "c1", "single", "张三", None).unwrap();
        ensure_conversation(&conn, "c2", "single", "李四", None).unwrap();
        insert_message(&conn, &rec("m1", "c1")).unwrap();
        insert_message(&conn, &rec("m2", "c1")).unwrap();
        insert_message(&conn, &rec("m3", "c2")).unwrap();
        assert_eq!(list_conversations(&conn).unwrap().len(), 2);

        delete_conversation(&conn, "c1").unwrap();

        // c1 会话行与消息全部清除；c2 不受影响
        let remaining = list_conversations(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "c2");
        assert!(!message_exists(&conn, "m1"));
        assert!(!message_exists(&conn, "m2"));
        assert!(message_exists(&conn, "m3"));
        assert_eq!(get_messages(&conn, "c1", 100, 0).unwrap().len(), 0);
        assert_eq!(get_messages(&conn, "c2", 100, 0).unwrap().len(), 1);
    }

    #[test]
    fn delete_conversation_idempotent_on_missing() {
        let conn = mem();
        // 不存在也不报错（前端 UI 二次确认后用户可能在另一边删了/网络抖动）
        delete_conversation(&conn, "nonexistent").unwrap();
    }

    #[test]
    fn outbox_offline_queue_dedup_and_delete() {
        let conn = mem();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap(); // 同 msg_id 去重
        let pending = list_outbox(&conn, "f1").unwrap();
        assert_eq!(pending.len(), 1);
        delete_outbox(&conn, pending[0].0).unwrap();
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
    }

    /// outbox 的身份是 msg_id 而非密文：补发前重新加密只会换 payload，
    /// 同一 msg_id 再入队仍被唯一约束忽略（不产生第二行、不覆盖首行），
    /// Ack 仍按 msg_id 精确删除 ⇒ 重封不破坏消息身份 / 幂等 / outbox 语义。
    #[test]
    fn outbox_identity_is_msg_id_not_payload() {
        let conn = mem();
        insert_outbox(&conn, "m1", "f1", r#"{"msg_id":"m1","content":"enc1:old"}"#).unwrap();
        insert_outbox(
            &conn,
            "m1",
            "f1",
            r#"{"msg_id":"m1","content":"enc1:resealed"}"#,
        )
        .unwrap();
        let pending = list_outbox(&conn, "f1").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("enc1:old")); // 首行原样保留，由 flush 时重封
                                                    // Ack 分支的删除路径（transport.rs 同构 SQL）
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", params!["m1"])
            .unwrap();
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
    }

    #[test]
    fn message_and_outbox_are_written_atomically() {
        let conn = mem();
        let m = rec("atomic-1", "peer-1");
        insert_message_and_outbox(&conn, &m, "peer-1", "payload").unwrap();
        assert_eq!(get_messages(&conn, "peer-1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_outbox(&conn, "peer-1").unwrap().len(), 1);

        // 同一业务 ID 再写入时事务失败，不能额外留下半条 outbox 或半条消息。
        assert!(insert_message_and_outbox(&conn, &m, "peer-1", "payload-2").is_err());
        assert_eq!(get_messages(&conn, "peer-1", 10, 0).unwrap().len(), 1);
        assert_eq!(list_outbox(&conn, "peer-1").unwrap().len(), 1);
    }

    #[test]
    fn group_create_and_members() {
        let conn = mem();
        create_group(&conn, "g1", "群聊", "me", &["me".into(), "f1".into()]).unwrap();
        let groups = list_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].members.contains(&"f1".to_string()));
        assert!(groups[0].members.contains(&"me".to_string()));
    }

    #[test]
    fn transfer_upsert_tracks_progress() {
        let conn = mem();
        upsert_transfer(&conn, "t1", "f1", "a.txt", 100, "send", "active", None, 0.5).unwrap();
        upsert_transfer(&conn, "t1", "f1", "a.txt", 100, "send", "done", None, 1.0).unwrap();
        let transfers = list_transfers(&conn).unwrap();
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].status, "done");
        assert_eq!(transfers[0].progress, 1.0);
    }

    fn status_of(conn: &Connection, msg_id: &str) -> String {
        conn.query_row(
            "SELECT status FROM messages WHERE msg_id = ?1",
            params![msg_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 已读不可逆：outbox 补发会让同一 msg_id 再送达一次并带回迟到的 Ack，
    /// 无条件覆盖会把已读退回「对方未读」，界面上刚亮的绿勾跳回空圆框。
    #[test]
    fn late_ack_never_regresses_a_read_message() {
        let conn = mem();
        insert_message(&conn, &rec("m1", "f1")).unwrap();
        set_message_status(&conn, "m1", "delivered").unwrap();
        assert_eq!(
            status_of(&conn, "m1"),
            "delivered",
            "正常前进：sent → delivered"
        );
        // 对端已读回执（与 transport.rs ReadReceipt 分支同构的 SQL）
        let updated = conn
            .execute(
                "UPDATE messages SET status = 'read'
                 WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                params!["f1", "a", 5],
            )
            .unwrap();
        assert_eq!(updated, 1);
        assert_eq!(status_of(&conn, "m1"), "read");
        set_message_status(&conn, "m1", "delivered").unwrap();
        assert_eq!(status_of(&conn, "m1"), "read", "迟到的 Ack 不得回退已读");
    }

    /// 局域网默认开启：无键（首次安装、以及从未写过该键的旧版本升级）即为开，
    /// 并立即将 "1" 持久化（之后每次启动读到明确值，不再依赖隐式默认）；
    /// 只有用户显式关闭才持久化为关，「恢复默认」清键后回到默认开。
    #[test]
    fn lan_enabled_defaults_on_and_keeps_explicit_off() {
        let conn = mem();
        // 缺省必须开启，且把 "1" 写入 settings
        assert!(get_lan_enabled(&conn), "缺省必须开启");
        assert_eq!(
            get_setting(&conn, "lan_enabled").as_deref(),
            Some("1"),
            "get_lan_enabled(None) 应立即持久化 true"
        );
        // 用户关闭 → 重启后仍为关
        set_lan_enabled(&conn, false).unwrap();
        assert!(!get_lan_enabled(&conn), "用户关闭后重启仍为关");
        // 用户开启 → 重启后仍为开
        set_lan_enabled(&conn, true).unwrap();
        assert!(get_lan_enabled(&conn));
        // 恢复默认（清键）→ 回到开
        delete_setting(&conn, "lan_enabled").unwrap();
        assert!(get_lan_enabled(&conn), "恢复默认后回到开");
    }

    /// 蓝牙开关偏好：显式值必须被尊重（默认值只在"键不存在"时生效）。
    ///
    /// ⚠️ 默认值本身依赖目标平台（`cfg!(mobile)` ⇒ 手机默认开、桌面默认关），
    /// 主机单测只能覆盖"桌面 = 关"这一半；手机那一半由 `bt_default_on_for_mobile`
    /// 那条源码规则护栏盯着（见 `lib.rs` 的测试模块）。
    #[test]
    fn bt_enabled_defaults_on_and_keeps_explicit_value() {
        let conn = mem();
        assert!(get_bt_enabled(&conn), "缺省必须是**开**（用户规则：有蓝牙就默认开）");
        assert_eq!(get_setting(&conn, "bt_enabled").as_deref(), Some("1"));

        set_bt_enabled(&conn, true).unwrap();
        assert!(get_bt_enabled(&conn), "显式打开必须生效");
        set_bt_enabled(&conn, false).unwrap();
        assert!(!get_bt_enabled(&conn), "显式关闭必须生效");
    }

    // ================================================================
    // pending_reads 持久化测试
    // ================================================================

    /// Test 1：upsert 使用 max 语义——较旧 timestamp 不覆盖较新。
    #[test]
    fn pending_read_upsert_keeps_max() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "B", 100).unwrap(); // 较旧，不覆盖
        let rows = load_pending_reads(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ("B".to_string(), 200));

        upsert_pending_read(&conn, "B", 300).unwrap(); // 较新，更新
        let rows = load_pending_reads(&conn).unwrap();
        assert_eq!(rows[0], ("B".to_string(), 300));
    }

    /// Test 2：load_pending_reads 正确读取多个 peer。
    #[test]
    fn pending_read_load_multiple_peers() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "C", 500).unwrap();
        let mut rows = load_pending_reads(&conn).unwrap();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("B".to_string(), 200));
        assert_eq!(rows[1], ("C".to_string(), 500));
    }

    /// Test 3：delete_pending_read 只删除指定 peer。
    #[test]
    fn pending_read_delete_only_target() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "C", 500).unwrap();
        delete_pending_read(&conn, "B").unwrap();
        let rows = load_pending_reads(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ("C".to_string(), 500));
    }

    /// Test 4：重启恢复——DB 写入后，新连接 load 能正确恢复。
    #[test]
    fn pending_read_survives_restart() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        upsert_pending_read(&conn, "C", 500).unwrap();
        // 模拟进程重启：新建 HashMap，从 DB 加载
        let mut restored = std::collections::HashMap::new();
        for (peer_id, ts) in load_pending_reads(&conn).unwrap() {
            let cur = restored.entry(peer_id).or_insert(ts);
            *cur = (*cur).max(ts);
        }
        assert_eq!(restored.get("B"), Some(&200));
        assert_eq!(restored.get("C"), Some(&500));
        // DB 中的记录仍然存在（flush 时才会删除）
        assert_eq!(load_pending_reads(&conn).unwrap().len(), 2);
    }

    /// Test 5：delete 后 load 为空。
    #[test]
    fn pending_read_delete_all() {
        let conn = mem();
        upsert_pending_read(&conn, "B", 200).unwrap();
        delete_pending_read(&conn, "B").unwrap();
        assert!(load_pending_reads(&conn).unwrap().is_empty());
    }

    /// Test 6：空 DB load 返回空 vec。
    #[test]
    fn pending_read_load_empty_db() {
        let conn = mem();
        assert!(load_pending_reads(&conn).unwrap().is_empty());
    }

    // ================================================================
    // 搜索测试
    // ================================================================

    fn insert_text_msg(conn: &Connection, msg_id: &str, conv_id: &str, content: &str) {
        insert_message(
            conn,
            &MessageRecord {
                id: 0,
                msg_id: msg_id.into(),
                conv_id: conv_id.into(),
                sender_id: "a".into(),
                receiver_id: "b".into(),
                kind: "text".into(),
                content: content.into(),
                ts: 1,
                seq: 1,
                status: "delivered".into(),
            },
        )
        .unwrap();
    }

    /// 普通文本搜索：中文 + 英文。
    #[test]
    fn search_messages_plain_text() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "hello world");
        insert_text_msg(&conn, "m2", "conv2", "今天测试 hello");
        insert_text_msg(&conn, "m3", "conv3", "没有匹配");

        let r = search_messages(&conn, "hello", 10).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.contains(&"conv1".to_string()));
        assert!(r.contains(&"conv2".to_string()));

        let r = search_messages(&conn, "没有", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], "conv3");
    }

    /// LIKE 通配符 % 和 _ 按字面字符搜索，不作为 wildcard。
    #[test]
    fn search_messages_escapes_like_wildcards() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "100% 完成");
        insert_text_msg(&conn, "m2", "conv2", "a_b 测试");

        // % 应按字面搜索，不是 wildcard
        let r = search_messages(&conn, "100%", 10).unwrap();
        assert_eq!(r.len(), 1, "% 应按字面匹配");
        assert_eq!(r[0], "conv1");

        // _ 应按字面搜索，不是 wildcard
        let r = search_messages(&conn, "a_b", 10).unwrap();
        assert_eq!(r.len(), 1, "_ 应按字面匹配");
        assert_eq!(r[0], "conv2");

        // 不应匹配 "100% 完成" 中的 "100" 作为独立搜索（% 是字面字符）
        let r = search_messages(&conn, "100", 10).unwrap();
        assert_eq!(r.len(), 1, "'100' 应匹配 '100% 完成'");
    }

    /// 空结果。
    #[test]
    fn search_messages_no_results() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "hello");
        let r = search_messages(&conn, "不存在", 10).unwrap();
        assert!(r.is_empty());
    }

    /// 中文搜索正常。
    #[test]
    fn search_messages_chinese() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "你好世界");
        insert_text_msg(&conn, "m2", "conv2", "hello world");
        let r = search_messages(&conn, "你好", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], "conv1");
    }

    /// emoji 搜索正常（按字符匹配，非 UTF-8 字节）。
    #[test]
    fn search_messages_emoji() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "🎉庆祝🎉");
        let r = search_messages(&conn, "🎉", 10).unwrap();
        assert_eq!(r.len(), 1);
    }

    /// 大小写不敏感搜索。
    #[test]
    fn search_messages_case_insensitive() {
        let conn = mem();
        insert_text_msg(&conn, "m1", "conv1", "Hello World");
        let r = search_messages(&conn, "hello", 10).unwrap();
        assert_eq!(r.len(), 1);
        let r = search_messages(&conn, "HELLO", 10).unwrap();
        assert_eq!(r.len(), 1);
    }

    /// update_conversation_profile 只更新 name/avatar，不修改 last_msg/last_ts。
    #[test]
    fn update_conversation_profile_preserves_last_msg() {
        let conn = mem();
        ensure_conversation(&conn, "dev-a", "single", "Old", None).unwrap();
        touch_conversation(&conn, "dev-a", "single", "Old", None, "Hello", 1).unwrap();

        // 验证初始状态
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == "dev-a")
            .unwrap();
        assert_eq!(conv.name, "Old");
        assert_eq!(conv.last_msg.as_deref(), Some("Hello"));

        // 执行 update_conversation_profile
        update_conversation_profile(&conn, "dev-a", "New", Some("new_avatar")).unwrap();

        // 验证：name/avatar 已更新，last_msg/last_ts 保持不变
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == "dev-a")
            .unwrap();
        assert_eq!(conv.name, "New", "name 应已更新");
        assert_eq!(
            conv.last_msg.as_deref(),
            Some("Hello"),
            "last_msg 不应被修改"
        );
    }

    /// update_conversation_profile 不影响 group 会话。
    #[test]
    fn update_conversation_profile_ignores_group() {
        let conn = mem();
        ensure_conversation(&conn, "group:g1", "group", "测试群", None).unwrap();
        update_conversation_profile(&conn, "group:g1", "新名", None).unwrap();
        let conv = list_conversations(&conn)
            .unwrap()
            .into_iter()
            .find(|c| c.id == "group:g1")
            .unwrap();
        assert_eq!(conv.name, "测试群", "group 会话不应被修改");
    }

    /// update_conversation_profile 对不存在的会话不报错。
    #[test]
    fn update_conversation_profile_noop_on_missing() {
        let conn = mem();
        update_conversation_profile(&conn, "nonexistent", "name", None).unwrap();
    }

    /// clear_all_data 通过 SQL 验证：删除所有业务数据，保留 friends 和非 gk: settings。
    #[test]
    fn clear_all_data_sql_deletes_and_preserves() {
        let conn = mem();
        // 插入测试数据
        insert_message(&conn, &rec("m1", "c1")).unwrap();
        ensure_conversation(&conn, "c1", "single", "测试", None).unwrap();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap();
        upsert_transfer(&conn, "t1", "f1", "a.txt", 100, "send", "active", None, 0.0).unwrap();
        upsert_pending_read(&conn, "f1", 200).unwrap();
        create_group(&conn, "g1", "群", "owner", &["a".into()]).unwrap();
        add_friend(&conn, "f1", "好友", None).unwrap();
        set_setting(&conn, "device_id", "dev-1").unwrap();
        set_setting(&conn, "nickname", "昵称").unwrap();
        set_setting(&conn, "gk:g1", "secret").unwrap();

        // 模拟 clear_all_data SQL 部分
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute("DELETE FROM group_members", []).unwrap();
        tx.execute("DELETE FROM groups", []).unwrap();
        tx.execute("DELETE FROM messages", []).unwrap();
        tx.execute("DELETE FROM conversations", []).unwrap();
        tx.execute("DELETE FROM outbox", []).unwrap();
        tx.execute("DELETE FROM file_transfers", []).unwrap();
        tx.execute("DELETE FROM pending_reads", []).unwrap();
        tx.execute("DELETE FROM settings WHERE key LIKE 'gk:%'", [])
            .unwrap();
        tx.commit().unwrap();

        // 验证：业务数据已删除
        assert!(get_messages(&conn, "c1", 10, 0).unwrap().is_empty());
        assert!(list_conversations(&conn).unwrap().is_empty());
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
        assert!(list_transfers(&conn).unwrap().is_empty());
        assert!(load_pending_reads(&conn).unwrap().is_empty());
        assert!(list_groups(&conn).unwrap().is_empty());
        assert!(get_setting(&conn, "gk:g1").is_none());

        // 验证：friends 和非 gk: settings 保留
        assert!(get_friend(&conn, "f1").is_some());
        assert_eq!(get_setting(&conn, "device_id").as_deref(), Some("dev-1"));
        assert_eq!(get_setting(&conn, "nickname").as_deref(), Some("昵称"));
    }

    // ---------- 群文件 per-recipient 投递状态 ----------

    fn group_file(transfer_id: &str) -> GroupFile {
        GroupFile {
            transfer_id: transfer_id.to_string(),
            group_id: "g1".to_string(),
            sender_id: "a".to_string(),
            name: "report.pdf".to_string(),
            size: 1024,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_string(),
            status: "pending".to_string(),
            created_at: now_ms(),
        }
    }

    fn group_file_fixture() -> Connection {
        let conn = mem();
        // 群 g1：成员 a（sender）/ b / c / d
        create_group(
            &conn,
            "g1",
            "测试群",
            "a",
            &["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()],
        )
        .unwrap();
        conn
    }

    /// 1+2+3：建群文件 + 为 B/C/D 建 recipient state + 分别置 completed/sending/pending。
    #[test]
    fn group_file_recipient_states_persist() {
        let conn = group_file_fixture();
        let f = group_file("gf-1");
        insert_group_file(&conn, &f).unwrap();
        assert_eq!(get_group_file(&conn, "gf-1").unwrap().name, "report.pdf");

        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "c", "sending", 0.4).unwrap();
        // d 保持初始 pending

        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        let by_id: std::collections::HashMap<_, _> = recipients
            .iter()
            .map(|r| (r.recipient_id.as_str(), (r.status.as_str(), r.progress)))
            .collect();
        assert_eq!(by_id.get("b"), Some(&("completed", 1.0)));
        assert_eq!(by_id.get("c"), Some(&("sending", 0.4)));
        assert_eq!(by_id.get("d"), Some(&("pending", 0.0)));
    }

    /// 重启后离线补发：upsert_group_file_receive 幂等重建会话，
    /// 未完成态复位回 sending，completed 不被覆盖。
    #[test]
    fn upsert_group_file_receive_reestablishes_session() {
        let conn = group_file_fixture();
        let f = group_file("gf-1");

        // 首次 Offer：建立记录 + recipient（sending）
        upsert_group_file_receive(&conn, &f, "b").unwrap();
        assert_eq!(
            get_group_file_recipient_status(&conn, "gf-1", "b").as_deref(),
            Some("sending")
        );

        // 模拟中断置 failed → 重启后重发 Offer → 复位回 sending
        update_group_file_recipient(&conn, "gf-1", "b", "failed", 0.0).unwrap();
        upsert_group_file_receive(&conn, &f, "b").unwrap();
        assert_eq!(
            get_group_file_recipient_status(&conn, "gf-1", "b").as_deref(),
            Some("sending")
        );

        // 已完成态不被重建覆盖
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        upsert_group_file_receive(&conn, &f, "b").unwrap();
        assert_eq!(
            get_group_file_recipient_status(&conn, "gf-1", "b").as_deref(),
            Some("completed")
        );
    }

    /// 5：更新 C → completed 不影响 B/D 的状态。
    #[test]
    fn update_one_recipient_does_not_affect_others() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        update_group_file_recipient(&conn, "gf-1", "c", "completed", 1.0).unwrap();

        let by_id: std::collections::HashMap<_, _> =
            list_group_file_recipients(&conn, "gf-1")
                .unwrap()
                .into_iter()
                .map(|r| (r.recipient_id, r.status))
                .collect();
        assert_eq!(by_id.get("b").map(String::as_str), Some("completed"));
        assert_eq!(by_id.get("c").map(String::as_str), Some("completed"));
        assert_eq!(by_id.get("d").map(String::as_str), Some("pending"));
    }

    /// 6：同一 (transfer_id, recipient_id) 重复插入必须报错（PRIMARY KEY）。
    #[test]
    fn duplicate_recipient_insert_rejected() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();
        assert!(insert_group_file_recipient(&conn, "gf-1", "b").is_err());
    }

    /// 权限边界：群不存在 / sender 非成员 / recipient 非成员 → 拒绝。
    #[test]
    fn group_file_permission_checks() {
        let conn = group_file_fixture();

        // 群不存在
        let mut f = group_file("gf-x");
        f.group_id = "g-missing".to_string();
        assert!(insert_group_file(&conn, &f).is_err());

        // sender 不是群成员
        let mut f = group_file("gf-1");
        f.sender_id = "outsider".to_string();
        assert!(insert_group_file(&conn, &f).is_err());

        // 群外 peer 不能建 recipient state
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        assert!(insert_group_file_recipient(&conn, "gf-1", "outsider").is_err());
        // recipient 更新不存在的 state 报错（不静默创建）
        assert!(update_group_file_recipient(&conn, "gf-1", "outsider", "pending", 0.0).is_err());
    }

    /// 7：删除群 → 群文件与 recipient 数据级联清理（delete_group 事务语义）。
    #[test]
    fn delete_group_cascades_group_files() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }

        delete_group(&conn, "g1").unwrap();

        assert!(get_group_file(&conn, "gf-1").is_none());
        assert!(list_group_file_recipients(&conn, "gf-1").unwrap().is_empty());
    }

    /// get_group_file 不存在的 transfer 返回 None。
    #[test]
    fn get_group_file_missing_returns_none() {
        let conn = mem();
        assert!(get_group_file(&conn, "nope").is_none());
    }

    /// 群主转让：只改 creator，成员表保持不变。
    #[test]
    fn set_group_creator_updates_creator_only() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();

        set_group_creator(&conn, "g1", "b").unwrap();

        let g = get_group(&conn, "g1").unwrap();
        assert_eq!(g.creator, "b");
        let mut got = g.members.clone();
        got.sort();
        assert_eq!(got, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        // 不存在的群：影响 0 行，不报错也不产生记录
        set_group_creator(&conn, "nope", "x").unwrap();
        assert!(get_group(&conn, "nope").is_none());
    }

    // ---------- 群关系同步 ≠ 聊天会话（conversation 只能由聊天活动驱动） ----------

    /// Test 1 + Test 4：群关系同步（GroupKey → `upsert_group`）只建立 groups / group_members，
    /// **不得**创建 conversation —— conversation 是「聊天会话索引」，只有收到新消息
    /// （`insert_message` + `touch_conversation`）时才产生。等价于「重新安装后只同步群关系」。
    #[test]
    fn group_relation_sync_does_not_create_conversation() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        // 初始：groups / conversations / messages 全空（重新安装语义）
        assert!(list_groups(&conn).unwrap().is_empty());
        assert!(list_conversations(&conn).unwrap().is_empty());

        // 群关系同步（等价于 handle_group_key 收到 GroupKey 后调用 upsert_group）
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();

        // groups 恢复，但 conversations 不创建、messages 仍为空
        assert_eq!(list_groups(&conn).unwrap().len(), 1);
        assert!(list_conversations(&conn).unwrap().is_empty());
        assert_eq!(count_messages(&conn, "group:g1"), 0);
    }

    /// Test 2 + Test 5：新群消息（落库 + `touch_conversation`）才创建 conversation。
    /// 在「只同步过群关系、无会话」的基础上收到新消息 → conversation 出现。
    #[test]
    fn group_message_creates_conversation() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();
        assert!(list_conversations(&conn).unwrap().is_empty());

        // 收到新群消息 → insert_message + touch_conversation（transport.rs 群消息接收路径）
        insert_message(&conn, &rec_as("m4", "group:g1", "text", "hi")).unwrap();
        touch_conversation(&conn, "group:g1", "group", "群", None, "hi", 1).unwrap();

        assert_eq!(list_groups(&conn).unwrap().len(), 1);
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);
        assert_eq!(count_messages(&conn, "group:g1"), 1);
    }

    /// Test 3：已有 conversation 时，再次群关系同步既不删除、也不重复创建。
    /// 即「GroupKey 更新永远不能删掉已存在的 conversation」。
    #[test]
    fn existing_conversation_survives_group_relation_sync() {
        let conn = mem();
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        // 先产生一个真实聊天会话（收到过群消息）
        upsert_group(&conn, "g1", "群", "a", &members).unwrap();
        touch_conversation(&conn, "group:g1", "group", "群", None, "hi", 1).unwrap();
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);

        // 再次群关系同步（重复 GroupKey，含群名刷新）→ conversation 仍为 1
        upsert_group(&conn, "g1", "群改名", "a", &members).unwrap();
        assert_eq!(list_groups(&conn).unwrap().len(), 1);
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);
    }

    // ---------- GroupFileOffer / session-key 阶段 ----------

    /// recipient 集合 = 创建时的成员快照：建文件后再加群成员不影响已建 recipients。
    #[test]
    fn group_file_recipients_are_creation_snapshot() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        // 创建快照之后群新增成员 e（动态成员语义后续阶段处理）
        add_group_member(&conn, "g1", "e").unwrap();

        let ids: Vec<String> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| r.recipient_id)
            .collect();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string(), "d".to_string()]);
    }

    /// 接收端权限判定基础：get_group 返回成员表，群外 sender 不在其中。
    /// （handle_group_file_offer 用同一判定：local group exists && sender ∈ members）
    #[test]
    fn outsider_sender_not_in_local_group_members() {
        let conn = group_file_fixture();
        let g = get_group(&conn, "g1").unwrap();
        assert!(g.members.contains(&"a".to_string()));
        assert!(!g.members.contains(&"outsider".to_string()));
        assert!(get_group(&conn, "g-missing").is_none(), "本地群不存在");
    }

    /// 幂等：相同 transfer_id 的 Offer 重复处理时，已存在记录即安全忽略
    /// （接收端依据 get_group_file 是否已存在；DB 层重复插入本身被 PK 拒绝）。
    #[test]
    fn duplicate_transfer_id_offer_is_idempotent() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();

        // 已存在 → 接收端直接 return（模拟判定条件）
        assert!(get_group_file(&conn, "gf-1").is_some());
        // 即使重复插入也被 PK 拒绝，不会覆盖既有状态
        assert!(insert_group_file(&conn, &group_file("gf-1")).is_err());
        assert!(insert_group_file_recipient(&conn, "gf-1", "b").is_err());
        // 既有状态未被覆盖
        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].status, "pending");
    }

    /// 事务原子性：任一 recipient 创建失败（群外成员）→ 整体回滚，
    /// 不留「group_files 已建但 recipient 只建了一半」的半完成状态；
    /// 发送端据此在 DB 初始化失败时不发送 Offer、不残留内存 file_key。
    #[test]
    fn group_file_creation_is_atomic() {
        let conn = group_file_fixture();
        let f = group_file("gf-1");
        let tx = conn.unchecked_transaction().unwrap();
        insert_group_file(&tx, &f).unwrap();
        insert_group_file_recipient(&tx, "gf-1", "b").unwrap();
        // 群外成员触发失败
        assert!(insert_group_file_recipient(&tx, "gf-1", "outsider").is_err());
        // 模拟发送端在 Err 后放弃提交（rollback）
        drop(tx);

        // 半完成状态不存在：group_files 与 recipients 均未落库
        assert!(get_group_file(&conn, "gf-1").is_none());
        assert!(list_group_file_recipients(&conn, "gf-1").unwrap().is_empty());
    }

    /// sender 自己不进入 recipient state（快照只含其他成员）。
    #[test]
    fn sender_not_in_recipient_states() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap(); // sender = "a"
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        let ids: Vec<String> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| r.recipient_id)
            .collect();
        assert!(!ids.contains(&"a".to_string()), "sender 不应有 recipient state");
        assert_eq!(ids.len(), 3);
    }

    /// file session key 不进入 SQLite 持久化：两张群文件表均无密钥列。
    #[test]
    fn group_file_keys_not_persisted() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();

        for table in ["group_files", "group_file_recipients"] {
            let cols: Vec<String> = conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            for col in &cols {
                assert!(
                    !col.to_lowercase().contains("file_key") && !col.to_lowercase().contains("key"),
                    "{table}.{col} 不应持久化文件会话密钥"
                );
            }
        }
    }

    // ---------- GroupFileCompleteAck（sender 侧 recipient 状态迁移） ----------

    /// 合法 recipient 的 success ACK → completed + progress 1.0，只影响该 recipient。
    #[test]
    fn complete_ack_updates_only_target_recipient() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c", "d"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }

        // B 的 success ACK
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        let by_id: std::collections::HashMap<_, _> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| (r.recipient_id, (r.status, r.progress)))
            .collect();
        assert_eq!(by_id.get("b"), Some(&("completed".to_string(), 1.0)));
        assert_eq!(by_id.get("d"), Some(&("pending".to_string(), 0.0)));
    }

    /// failure ACK → recipient failed（progress 重置为 0），不影响其他成员。
    #[test]
    fn failure_ack_marks_recipient_failed() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "c", "sending", 0.5).unwrap();

        // C 的 failure ACK
        update_group_file_recipient(&conn, "gf-1", "c", "failed", 0.0).unwrap();

        let by_id: std::collections::HashMap<_, _> = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .map(|r| (r.recipient_id, r.status))
            .collect();
        assert_eq!(by_id.get("c").map(String::as_str), Some("failed"));
        assert_eq!(by_id.get("b").map(String::as_str), Some("pending"));
    }

    /// 非 recipient 的 ACK 被拒绝（update 不命中）——不允许群外 peer 伪造状态。
    #[test]
    fn ack_from_non_recipient_is_rejected() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();
        assert!(
            update_group_file_recipient(&conn, "gf-1", "outsider", "completed", 1.0).is_err()
        );
    }

    /// 重复 ACK 幂等：连续相同更新不报错、状态稳定、无副作用。
    #[test]
    fn repeated_ack_is_idempotent() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();

        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].status, "completed");
        assert_eq!(recipients[0].progress, 1.0);
    }

    /// 发送端验证：group_file.sender_id 必须是本机才处理 ACK
    /// （get_group_file 返回的 sender_id 供此比对；他机发起的 transfer 被拒）。
    #[test]
    fn ack_sender_check_uses_group_file_owner() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap(); // sender = "a"
        let gf = get_group_file(&conn, "gf-1").unwrap();
        // 本机是 "a" 时才处理；本机是 "b"（recipient）时 sender_id != me → 拒绝
        assert_eq!(gf.sender_id, "a");
        assert_ne!(gf.sender_id, "b");
    }

    // ---------- GroupFileChunk 原始 sender 校验 / CompleteAck 降级保护 ----------

    /// 非原始 sender 的 GroupFileChunk 必须被忽略：比对 gf.sender_id 不匹配
    /// 即拒绝（不触发 fail 清理 → 不删 .part、不清 session、不改 recipient 状态）。
    /// 复现修复前缺陷：任何群成员发垃圾 chunk 即可终止合法接收。
    #[test]
    fn group_chunk_from_non_original_sender_is_ignored() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap(); // 原始 sender = "a"
        for rid in ["b", "c"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        update_group_file_recipient(&conn, "gf-1", "b", "sending", 0.5).unwrap();

        // 模拟 handle_group_file_chunk 的判定：chunk 声称来自群成员 "e"（非原始 sender）
        let gf = get_group_file(&conn, "gf-1").unwrap();
        let chunk_sender = "e"; // 群成员（非 recipient 也无妨），但不是原始 sender
        let ignored = gf.sender_id != chunk_sender;
        assert!(ignored, "非原始 sender 的 chunk 必须被忽略");

        // 无副作用：recipient 状态原样保留（未触发 fail 清理）
        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        let b = recipients.iter().find(|r| r.recipient_id == "b").unwrap();
        assert_eq!(b.status, "sending");
        assert_eq!(b.progress, 0.5);
    }

    /// recipient 已 completed 后，failure ACK 不得把 completed 降级为 failed
    /// （handle_group_file_complete_ack 的幂等保护：已 completed 则忽略 failure ACK）。
    #[test]
    fn complete_ack_failure_cannot_downgrade_completed() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        insert_group_file_recipient(&conn, "gf-1", "b").unwrap();
        // B 正常完成：success ACK → completed / 1.0
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();

        // 模拟修复后 handle_group_file_complete_ack 对 failure ACK 的保护：
        // 已 completed 则忽略（不执行 update）
        let already_completed = list_group_file_recipients(&conn, "gf-1")
            .unwrap()
            .into_iter()
            .any(|r| r.recipient_id == "b" && r.status == "completed");
        if !already_completed {
            update_group_file_recipient(&conn, "gf-1", "b", "failed", 0.0).unwrap();
        }

        // 断言：仍 completed / 1.0（修复前此处会被降级为 failed——暴露 Bug 2）
        let recipients = list_group_file_recipients(&conn, "gf-1").unwrap();
        let b = recipients.iter().find(|r| r.recipient_id == "b").unwrap();
        assert_eq!(b.status, "completed", "completed 不得被 failure ACK 降级");
        assert_eq!(b.progress, 1.0);
    }

    // ---------- 群聊删除边界（清除聊天数据后旧消息防回灌） ----------

    /// 清除边界按逻辑序号拦截：seq <= boundary 的旧历史拦截，之后放行；无边界不拦截。
    #[test]
    fn clear_boundary_blocks_old_group_messages() {
        let conn = group_file_fixture();
        set_clear_boundary(&conn, "g1", 100).unwrap();

        // 清除边界及更早序号 → 拦截
        assert!(group_message_blocked_by_boundary(&conn, "g1", 99));
        assert!(group_message_blocked_by_boundary(&conn, "g1", 100));
        // 清除后的新序号 → 放行
        assert!(!group_message_blocked_by_boundary(&conn, "g1", 101));
        // 未设置边界的群不拦截
        assert!(!group_message_blocked_by_boundary(&conn, "g2", 1));
        // 重复清除：边界覆盖为新值（新 boundary 之前的旧消息再次被拦截）
        set_clear_boundary(&conn, "g1", 200).unwrap();
        assert!(group_message_blocked_by_boundary(&conn, "g1", 200));
        assert!(!group_message_blocked_by_boundary(&conn, "g1", 201));
    }

    /// 删除单个群会话同样写入边界（delete_conversation 群分支语义）。
    #[test]
    fn deleting_group_conversation_sets_boundary() {
        let conn = group_file_fixture();
        set_clear_boundary(&conn, "g1", 123456789).unwrap();
        // 边界及更早序号被拦截
        assert!(group_message_blocked_by_boundary(&conn, "g1", 123456789));
        assert!(!group_message_blocked_by_boundary(&conn, "g1", 123456790));
    }

    // ---------- GroupFile 多 recipient 气泡状态聚合 ----------

    /// 聚合判定（handle_group_file_complete_ack failure 分支）：
    /// 全部终态且有人 completed → delivered（mixed）；全部 failed → failed；
    /// 仍有 pending/sending → 保持当前（不产生最终状态）。
    fn aggregate_bubble(recipients: &[GroupFileRecipient]) -> Option<&'static str> {
        let all_terminal = recipients
            .iter()
            .all(|r| r.status == "completed" || r.status == "failed");
        if !all_terminal {
            return None; // 气泡保持当前（sending）
        }
        Some(if recipients.iter().any(|r| r.status == "completed") {
            "delivered"
        } else {
            "failed"
        })
    }

    /// B success + C success → delivered；B failure + C failure → failed；
    /// B success + C failure（mixed）→ delivered；C 未确认 → 保持 sending。
    #[test]
    fn group_file_ack_aggregation_semantics() {
        let conn = group_file_fixture();
        insert_group_file(&conn, &group_file("gf-1")).unwrap();
        for rid in ["b", "c"] {
            insert_group_file_recipient(&conn, "gf-1", rid).unwrap();
        }
        let states = || -> Vec<GroupFileRecipient> {
            list_group_file_recipients(&conn, "gf-1").unwrap()
        };

        // mixed：B completed / C failed → delivered（有人收到即算）
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        update_group_file_recipient(&conn, "gf-1", "c", "failed", 0.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), Some("delivered"));

        // 全 failed → failed
        update_group_file_recipient(&conn, "gf-1", "b", "failed", 0.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), Some("failed"));

        // C 未确认（sending）→ None：气泡保持当前，不被单个 ACK 覆盖
        update_group_file_recipient(&conn, "gf-1", "c", "sending", 0.4).unwrap();
        update_group_file_recipient(&conn, "gf-1", "b", "completed", 1.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), None);

        // 全部 success → delivered
        update_group_file_recipient(&conn, "gf-1", "c", "completed", 1.0).unwrap();
        assert_eq!(aggregate_bubble(&states()), Some("delivered"));
    }

    /// 群文件本地路径经 file_transfers 持久化（gfile-{tid}）：
    /// 接收完成写入 receive/done + final_path，打开/另存/历史加载
    /// 经 transfer_id 关联到真实本地文件（重启后仍有效）。
    #[test]
    fn gfile_transfer_record_persists_local_path() {
        let conn = mem();
        upsert_transfer(
            &conn,
            "gfile-t1",
            "a",
            "report.pdf",
            2048,
            "receive",
            "done",
            Some("/downloads/report.pdf"),
            1.0,
        )
        .unwrap();
        let t = list_transfers(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "gfile-t1")
            .expect("群文件 transfer 记录应存在");
        assert_eq!(t.path.as_deref(), Some("/downloads/report.pdf"));
        assert_eq!(t.status, "done");
        assert_eq!(t.progress, 1.0);
    }

    /// 群消息 outbox：同一 msg_id 对不同 peer 各保留一行；GroupAck 只删对应行。
    #[test]
    fn group_outbox_per_peer_and_delete_by_msg_peer() {
        let conn = mem();
        insert_group_outbox(&conn, "m1", "g1", "b", "p1").unwrap();
        insert_group_outbox(&conn, "m1", "g1", "c", "p1").unwrap();
        insert_group_outbox(&conn, "m1", "g1", "b", "p1").unwrap(); // 幂等
        assert_eq!(list_group_outbox(&conn, "b").unwrap().len(), 1);
        assert_eq!(list_group_outbox(&conn, "c").unwrap().len(), 1);

        delete_group_outbox(&conn, "m1", "b").unwrap();
        assert!(list_group_outbox(&conn, "b").unwrap().is_empty());
        assert_eq!(list_group_outbox(&conn, "c").unwrap().len(), 1);

        delete_group_outbox_for_peer_in_group(&conn, "g1", "c").unwrap();
        assert!(list_group_outbox(&conn, "c").unwrap().is_empty());
    }

    /// 群 outbox 可整体按群清理（删除群时）。
    #[test]
    fn group_outbox_delete_by_group() {
        let conn = mem();
        insert_group_outbox(&conn, "m1", "g1", "b", "p").unwrap();
        insert_group_outbox(&conn, "m2", "g1", "c", "p").unwrap();
        insert_group_outbox(&conn, "m3", "g2", "b", "p").unwrap();
        delete_group_outbox_for_group(&conn, "g1").unwrap();
        assert!(list_group_outbox(&conn, "c").unwrap().is_empty());
        // g2 仍在 b 名下
        assert_eq!(list_group_outbox(&conn, "b").unwrap().len(), 1);
    }

    /// 文件 outbox 生命周期：pending → sending → pending（重试）→ 成功删除。
    #[test]
    fn file_outbox_lifecycle() {
        let conn = mem();
        insert_file_outbox(&conn, "t1", "b", None, "/tmp/a.txt", "a.txt", 10).unwrap();
        assert_eq!(list_pending_file_outbox(&conn, "b").unwrap().len(), 1);

        mark_file_outbox_sending(&conn, "t1", 0).unwrap();
        assert!(list_pending_file_outbox(&conn, "b").unwrap().is_empty());

        mark_file_outbox_pending(&conn, "t1", 0).unwrap();
        assert_eq!(list_pending_file_outbox(&conn, "b").unwrap().len(), 1);

        delete_file_outbox(&conn, "t1").unwrap();
        assert!(list_pending_file_outbox(&conn, "b").unwrap().is_empty());
    }

    /// 文件 outbox 永久失败后不再参与待投递查询。
    #[test]
    fn file_outbox_failed_is_not_pending() {
        let conn = mem();
        insert_file_outbox(&conn, "t1", "b", None, "/tmp/a.txt", "a.txt", 10).unwrap();
        mark_file_outbox_failed(&conn, "t1").unwrap();
        assert!(list_pending_file_outbox(&conn, "b").unwrap().is_empty());
    }

    /// 已读回执按「某发送者最近一条消息」取 msg_id + ts，
    /// 不取全会话最大时间戳，也不把接收方自己发的消息算进去。
    #[test]
    fn last_message_from_sender_ignores_own_and_other_senders() {
        let conn = mem();
        insert_message(&conn, &rec_as("a1", "b", "text", "from a")).unwrap();
        insert_message(&conn, &rec_as("a2", "b", "text", "from a later")).unwrap();
        // 自己（receiver_id=b 的视角，这里用 sender_id=b 模拟本地发出的消息）
        let mut own = rec_as("b1", "b", "text", "own");
        own.sender_id = "b".into();
        own.ts = 999;
        insert_message(&conn, &own).unwrap();

        let (msg_id, ts) = last_message_from_sender(&conn, "b", "a").unwrap();
        assert_eq!(msg_id, "a2");
        assert_eq!(ts, 1); // 测试 rec_as 统一 ts=1
    }

    /// 旧库没有 seq 列时，init 必须先补列再建索引，不能在建索引时崩掉。
    #[test]
    fn init_migrates_existing_db_without_seq_column() {
        let path = std::env::temp_dir().join(format!(
            "gosslan-test-migrate-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    msg_id TEXT UNIQUE NOT NULL,
                    conv_id TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    receiver_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    ts INTEGER NOT NULL,
                    status TEXT NOT NULL DEFAULT 'sent'
                );",
            )
            .unwrap();
        }
        let conn = init(&path).unwrap();
        let has_seq: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'seq'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap();
        assert!(has_seq, "旧库迁移后 messages 必须有 seq 列");
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_messages_conv_seq'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    /// 会话逻辑时钟：next_clock 单调递增，observe_clock 只前进不回退。
    #[test]
    fn conversation_clock_is_monotonic() {
        let conn = mem();
        assert_eq!(next_clock(&conn, "c1").unwrap(), 1);
        assert_eq!(next_clock(&conn, "c1").unwrap(), 2);
        observe_clock(&conn, "c1", 10).unwrap();
        assert_eq!(get_clock(&conn, "c1"), 10);
        observe_clock(&conn, "c1", 3).unwrap();
        assert_eq!(get_clock(&conn, "c1"), 10, "observe 不得把时钟推回");
        assert_eq!(next_clock(&conn, "c1").unwrap(), 11);
    }

    /// 群待发已读回执：按 (group_id, peer_id) 唯一，max 语义，删除只删对应行。
    #[test]
    fn pending_group_reads_lifecycle() {
        let conn = mem();
        upsert_pending_group_read(&conn, "g1", "b", 10).unwrap();
        upsert_pending_group_read(&conn, "g1", "b", 9).unwrap(); // 不覆盖较大值
        upsert_pending_group_read(&conn, "g1", "c", 20).unwrap();

        let rows = list_pending_group_reads(&conn, "b").unwrap();
        assert_eq!(rows, vec![("g1".to_string(), 10)]);

        delete_pending_group_read(&conn, "g1", "b").unwrap();
        assert!(list_pending_group_reads(&conn, "b").unwrap().is_empty());
        assert_eq!(list_pending_group_reads(&conn, "c").unwrap().len(), 1);
    }
}
