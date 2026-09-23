-- Gosslan SQLite Schema
-- 与 src-tauri/src/db.rs 中的 SCHEMA 保持一致（应用启动时自动执行迁移）。
-- 本文件供文档/手动建库参考。

-- 键值配置（设备指纹、昵称、头像、TCP 端口、共享目录等）
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 好友关系（P2P：双方各自存储）
CREATE TABLE IF NOT EXISTS friends (
    device_id TEXT PRIMARY KEY,   -- 对方设备指纹 ID
    nickname  TEXT NOT NULL,
    avatar    TEXT,               -- base64 data URI
    x25519_pubkey TEXT,           -- 对方 X25519 公钥（ECDH 用，从上线广播学到后持久化）
    ed25519_pubkey TEXT,          -- 对方 Ed25519 公钥（验签用）
    added_at  INTEGER NOT NULL
);

-- 会话（单聊 device_id / 群聊 group:<id>）
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

-- 消息
CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id      TEXT UNIQUE NOT NULL,  -- UUID，用于去重与离线补发
    conv_id     TEXT NOT NULL,
    sender_id   TEXT NOT NULL,
    receiver_id TEXT NOT NULL,
    kind        TEXT NOT NULL,          -- text | code | image | file | system
    content     TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    seq         INTEGER NOT NULL DEFAULT 0,  -- 每会话逻辑序号（Lamport）
    status      TEXT NOT NULL DEFAULT 'sent'  -- sent | delivered | queued
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conv_id, ts);
CREATE INDEX IF NOT EXISTS idx_messages_conv_seq ON messages(conv_id, seq);

-- 每会话逻辑时钟（Lamport 风格）
CREATE TABLE IF NOT EXISTS conversation_clocks (
    conv_id TEXT PRIMARY KEY,
    seq     INTEGER NOT NULL
);

-- 群组
CREATE TABLE IF NOT EXISTS groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    creator    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- 群成员
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

-- 离线补发队列（发给离线/未连接好友的消息）
CREATE TABLE IF NOT EXISTS outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id     TEXT NOT NULL,
    peer_id    TEXT NOT NULL,
    payload    TEXT NOT NULL,          -- 序列化后的 Message JSON
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_outbox_peer ON outbox(peer_id);
CREATE INDEX IF NOT EXISTS idx_outbox_created ON outbox(created_at);

-- 群消息离线补发队列
CREATE TABLE IF NOT EXISTS group_outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_id     TEXT NOT NULL,
    group_id   TEXT NOT NULL,
    peer_id    TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(msg_id, peer_id)
);

-- 文件传输记录
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

-- 文件离线投递队列
CREATE TABLE IF NOT EXISTS file_outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    transfer_id     TEXT NOT NULL UNIQUE,
    peer_id         TEXT NOT NULL,
    group_id        TEXT,
    local_path      TEXT NOT NULL,
    name            TEXT NOT NULL,
    size            INTEGER NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
);

-- 待发已读回执
CREATE TABLE IF NOT EXISTS pending_reads (
    peer_id       TEXT PRIMARY KEY,
    last_read_ts  INTEGER NOT NULL
);

-- 待发群已读回执
CREATE TABLE IF NOT EXISTS pending_group_reads (
    group_id      TEXT NOT NULL,
    peer_id       TEXT NOT NULL,
    last_read_ts  INTEGER NOT NULL,
    PRIMARY KEY (group_id, peer_id)
);
