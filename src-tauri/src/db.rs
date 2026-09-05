//! SQLite 存储层：本地聊天记录、好友关系、群组、离线队列与配置。
//! 使用 rusqlite（bundled，自带 SQLite 源码，跨平台零配置）。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Result};

use crate::state::{Conversation, Friend, Group, MessageRecord, TransferInfo};

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
    status      TEXT NOT NULL DEFAULT 'sent'
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conv_id, ts);

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

-- 离线补发队列：发给离线/未连接好友的消息
CREATE TABLE IF NOT EXISTS outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id     TEXT NOT NULL UNIQUE,
    peer_id    TEXT NOT NULL,
    payload    TEXT NOT NULL,          -- 序列化后的 Message JSON
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_outbox_peer ON outbox(peer_id);

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

/// 删除一条设置（「恢复默认」时清除偏好键，让上层回落到默认值）。
pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}

// ---------------- 好友 ----------------

pub fn add_friend(conn: &Connection, device_id: &str, nickname: &str, avatar: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO friends(device_id, nickname, avatar, added_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(device_id) DO UPDATE SET nickname = excluded.nickname, avatar = excluded.avatar",
        params![device_id, nickname, avatar, now_ms()],
    )?;
    Ok(())
}

pub fn remove_friend(conn: &Connection, device_id: &str) -> Result<()> {
    conn.execute("DELETE FROM friends WHERE device_id = ?1", params![device_id])?;
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

pub fn list_friends(conn: &Connection) -> Result<Vec<Friend>> {
    let mut stmt = conn.prepare("SELECT device_id, nickname, avatar FROM friends ORDER BY added_at")?;
    let rows = stmt.query_map([], |r| {
        Ok(Friend {
            device_id: r.get(0)?,
            nickname: r.get(1)?,
            avatar: r.get(2)?,
            online: false,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

// ---------------- 群组 ----------------

pub fn create_group(conn: &Connection, id: &str, name: &str, creator: &str, members: &[String]) -> Result<()> {
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
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;
    for row in rows {
        let (id, name, creator) = row?;
        let members: Vec<String> = conn
            .prepare("SELECT device_id FROM group_members WHERE group_id = ?1")?
            .query_map(params![id], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        groups.push(Group { id, name, creator, members });
    }
    Ok(groups)
}

// ---------------- 消息 ----------------

/// 插入一条消息，并返回「本次是否真的新建了记录」。
///
/// `true` = 本次插入；`false` = `msg_id` 已存在（INSERT OR IGNORE 命中唯一约束被忽略）
/// 或写入失败。判定与插入在同一条 SQL 语句内完成，不依赖先查后写的时序 —— 因此
/// Direct 与 Gossip 并发投递同一业务 msg_id 时，只可能有一方拿到 `true`，
/// 未读 +1 与 `message-received` 事件随之只发生一次。
pub fn insert_message_if_new(conn: &Connection, m: &MessageRecord) -> Result<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, status)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![m.msg_id, m.conv_id, m.sender_id, m.receiver_id, m.kind, m.content, m.ts, m.status],
    )?;
    Ok(changed > 0)
}

pub fn insert_message(conn: &Connection, m: &MessageRecord) -> Result<()> {
    insert_message_if_new(conn, m).map(|_| ())
}

pub fn message_exists(conn: &Connection, msg_id: &str) -> bool {
    conn.query_row("SELECT 1 FROM messages WHERE msg_id = ?1", params![msg_id], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn get_messages(conn: &Connection, conv_id: &str, limit: i64, offset: i64) -> Result<Vec<MessageRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, msg_id, conv_id, sender_id, receiver_id, kind, content, ts, status
         FROM messages WHERE conv_id = ?1 ORDER BY ts ASC, id ASC LIMIT ?2 OFFSET ?3",
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
            status: r.get(8)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn set_message_status(conn: &Connection, msg_id: &str, status: &str) -> Result<()> {
    conn.execute("UPDATE messages SET status = ?2 WHERE msg_id = ?1", params![msg_id, status])?;
    Ok(())
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
            name = excluded.name,
            avatar = COALESCE(excluded.avatar, conversations.avatar),
            last_msg = excluded.last_msg,
            last_ts = excluded.last_ts,
            unread = conversations.unread + excluded.unread,
            updated_at = excluded.updated_at",
        params![id, kind, name, avatar, last_msg, now_ms(), unread_inc],
    )?;
    Ok(())
}

pub fn ensure_conversation(conn: &Connection, id: &str, kind: &str, name: &str, avatar: Option<&str>) -> Result<()> {
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
    conn.execute("UPDATE conversations SET unread = 0 WHERE id = ?1", params![conv_id])?;
    Ok(())
}

/// 会话内最后一条消息的 ts（无消息返回 0）。用于已读回执与接收时间戳钳制。
pub fn last_message_ts(conn: &Connection, conv_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(ts), 0) FROM messages WHERE conv_id = ?1",
        params![conv_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// 接收消息时间戳钳制（防设备间时钟偏差导致排序错乱）：
/// - 上限：不晚于本地当前时间（对方时钟快 → 消息不能出现在「未来」）；
/// - 下限：不早于会话内最后一条消息（对方时钟慢 → 消息不能插到历史之前，
///   否则同一发送者的消息会因时钟偏差在列表中堆叠错位）。
/// 同毫秒冲突由自增 id 稳定排序兜底（到达顺序）。
pub fn clamp_incoming_ts(sender_ts: i64, now: i64, prev_ts: i64) -> i64 {
    sender_ts.min(now).max(prev_ts)
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

pub fn delete_outbox(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM outbox WHERE id = ?1", params![id])?;
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

pub fn list_transfers(conn: &Connection) -> Result<Vec<TransferInfo>> {
    let mut stmt = conn.prepare(
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

    /// Direct 分支与 Gossip 分支的投递效果只允许发生一次。
    /// 这里用与两个 handler 完全同构的模型（insert_message_if_new → 仅 fresh 才
    /// touch_conversation + 计一次事件），覆盖两种先后顺序。
    fn deliver(conn: &Connection, who: &str, content: &str) -> bool {
        let fresh = insert_message_if_new(conn, &rec_as("m1", "f1", "text", content)).unwrap();
        if fresh {
            touch_conversation(conn, "f1", "single", "张三", None, who, 1).unwrap();
        }
        fresh
    }

    /// Test 1 + Test 2：Direct→Gossip 与 Gossip→Direct 两种顺序，
    /// 都必须收敛为「1 条消息 / unread +1 / 1 次投递事件」，且 last_msg 属于先到者。
    #[test]
    fn unread_and_event_fire_only_for_the_winning_insert() {
        for gossip_first in [false, true] {
            let conn = mem();
            ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
            let order = if gossip_first { ["gossip", "direct"] } else { ["direct", "gossip"] };
            let mut events = 0;
            for who in order {
                if deliver(&conn, who, "hello") {
                    events += 1;
                }
            }
            let case = if gossip_first { "Gossip→Direct" } else { "Direct→Gossip" };
            assert_eq!(get_messages(&conn, "f1", 10, 0).unwrap().len(), 1, "{case}");
            assert_eq!(events, 1, "{case} 只能产生一次 message-received");
            let conv = &list_conversations(&conn).unwrap()[0];
            assert_eq!(conv.unread, 1, "{case} 未读只能 +1");
            assert_eq!(conv.last_msg.as_deref(), Some(order[0]), "{case} 后到者不得改写 last_msg");
        }
    }

    /// Test 3：同一 msg_id 被重复投递（心跳反复补发 / 同一信封多次到达）
    /// ⇒ 始终只有 1 行、1 次未读、1 次事件。
    #[test]
    fn repeated_delivery_of_same_msg_id_yields_single_side_effect() {
        let conn = mem();
        ensure_conversation(&conn, "f1", "single", "张三", None).unwrap();
        let mut events = 0;
        for i in 0..5 {
            if deliver(&conn, "dup", "hello") {
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
                let dbc = c.lock().unwrap();
                if insert_message_if_new(&dbc, &rec_as("m1", "f1", "text", "hello")).unwrap() {
                    touch_conversation(&dbc, "f1", "single", "张三", None, "hello", 1).ok();
                    *w.lock().unwrap() += 1;
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*winners.lock().unwrap(), 1, "同一 msg_id 只能有一个首次插入者");
        let dbc = conn.lock().unwrap();
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
    fn clamp_incoming_ts_guards_clock_skew() {
        let now = 1_000_000;
        // 正常时间（略有偏差但在合理范围）→ 不动
        assert_eq!(clamp_incoming_ts(now - 500, now, 0), now - 500);
        // 对方时钟快 10 分钟（消息出现在「未来」）→ 钳到本地 now
        assert_eq!(clamp_incoming_ts(now + 600_000, now, 0), now);
        // 对方时钟慢 10 分钟（消息早于会话历史）→ 钳到最后一条消息时间
        assert_eq!(clamp_incoming_ts(now - 600_000, now, now - 3_000), now - 3_000);
        // 钳制后与 prev 同毫秒：保持相等（由自增 id 稳定排序兜底），不越过 now
        assert_eq!(clamp_incoming_ts(now - 600_000, now, now), now);
        // 空会话（prev=0）：仅做「未来」钳制
        assert_eq!(clamp_incoming_ts(now - 1, now, 0), now - 1);
    }

    #[test]
    fn last_message_ts_returns_max_or_zero() {
        let conn = mem();
        ensure_conversation(&conn, "c1", "single", "张三", None).unwrap();
        assert_eq!(last_message_ts(&conn, "c1"), 0);
        let mut m1 = rec("m1", "c1");
        m1.ts = 100;
        let mut m2 = rec("m2", "c1");
        m2.ts = 300;
        insert_message(&conn, &m1).unwrap();
        insert_message(&conn, &m2).unwrap();
        assert_eq!(last_message_ts(&conn, "c1"), 300);
        assert_eq!(last_message_ts(&conn, "missing"), 0);
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
        insert_outbox(&conn, "m1", "f1", r#"{"msg_id":"m1","content":"enc1:resealed"}"#).unwrap();
        let pending = list_outbox(&conn, "f1").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("enc1:old")); // 首行原样保留，由 flush 时重封
        // Ack 分支的删除路径（transport.rs 同构 SQL）
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", params!["m1"]).unwrap();
        assert!(list_outbox(&conn, "f1").unwrap().is_empty());
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
}
