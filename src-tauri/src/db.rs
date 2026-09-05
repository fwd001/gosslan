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

pub fn insert_message(conn: &Connection, m: &MessageRecord) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, status)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![m.msg_id, m.conv_id, m.sender_id, m.receiver_id, m.kind, m.content, m.ts, m.status],
    )?;
    Ok(())
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
        MessageRecord {
            id: 0,
            msg_id: msg_id.into(),
            conv_id: conv_id.into(),
            sender_id: "a".into(),
            receiver_id: "b".into(),
            kind: "text".into(),
            content: "hi".into(),
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
    fn outbox_offline_queue_dedup_and_delete() {
        let conn = mem();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap();
        insert_outbox(&conn, "m1", "f1", "payload").unwrap(); // 同 msg_id 去重
        let pending = list_outbox(&conn, "f1").unwrap();
        assert_eq!(pending.len(), 1);
        delete_outbox(&conn, pending[0].0).unwrap();
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
