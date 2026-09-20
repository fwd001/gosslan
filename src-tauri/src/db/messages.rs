// 职责边界：
// - 消息表 CRUD（insert/get/list/update_message_status）
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
            kind: crate::protocol::display_kind(&r.get::<_, String>(5)?),
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

/// 按 msg_id 取完整消息记录（cancel_send / resend_message 等需要跨字段判断时用）。
pub fn get_message_record(conn: &Connection, msg_id: &str) -> Option<MessageRecord> {
    conn.query_row(
        "SELECT id, msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status
         FROM messages WHERE msg_id = ?1",
        params![msg_id],
        |r| {
            Ok(MessageRecord {
                id: r.get(0)?,
                msg_id: r.get(1)?,
                conv_id: r.get(2)?,
                sender_id: r.get(3)?,
                receiver_id: r.get(4)?,
                kind: crate::protocol::display_kind(&r.get::<_, String>(5)?),
                content: r.get(6)?,
                ts: r.get(7)?,
                seq: r.get(8)?,
                status: r.get(9)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// 收藏取源：按 msg_id 返回 `(sender_id, kind, content, ts)`。
///
/// 收藏的**内容一律以库里的消息为准**（前端只传 msg_id）：如果让前端把 content 传上来，
/// 收藏夹里就可能存进一份与消息记录不一致的副本，而"收藏"最不该做的事就是记录失真。
pub fn get_favorite_source(
    conn: &Connection,
    msg_id: &str,
) -> Option<(String, String, String, i64)> {
    conn.query_row(
        "SELECT sender_id, kind, content, ts FROM messages WHERE msg_id = ?1",
        params![msg_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// 更新单条消息状态。**硬终态不可逆**：`read`（已读回执）、`delivered`（Ack）、
/// `recalled`（被撤回）一旦写入，任何后续 set 都会被拒绝。
///
/// 注意：`failed` 和 `cancelled` 是**软终态**（可通过 resend_message 回到 sending），
/// 所以它们**不在** NOT IN 里 — sweeper 判 failed 后用户还能点重发。
///
/// 典型竞态：Ack 因 outbox 补发晚于 ReadReceipt 到达，`read` 不能被回退成 `delivered`。
pub fn set_message_status(conn: &Connection, msg_id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET status = ?2
         WHERE msg_id = ?1
           AND status NOT IN ('read', 'delivered', 'recalled')",
        params![msg_id, status],
    )?;
    Ok(())
}

/// 回填消息内容与状态（群文件接收完成时用）：content 需随传输完成补上本地 `path`，
/// 状态同时前进到 delivered。与 `set_message_status` 不同，这里会改写 content——
/// `read_file_preview` 按 msg_id 反查 content 定位本地文件，群文件 Offer 阶段先落库
/// 无 path 的内容，Done 时必须显式回填，否则接收方图片/代码预览因缺 path 失败。
pub fn update_message_content(
    conn: &Connection,
    msg_id: &str,
    content: &str,
    status: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE messages SET content = ?2, status = ?3 WHERE msg_id = ?1",
        params![msg_id, content, status],
    )?;
    Ok(())
}

/// 回填文件消息 `content` 里的单个 `sha256` 字段（发送方建行时还算不起，见
/// `commands/files.rs::build_file_message`：整文件哈希是 O(体积) 的，不能挡在气泡前面）。
///
/// 返回 `true` = 这次真的改了行；`false` = 幂等跳过 / 缺行 / 载荷不是 JSON。
/// 调用方据此决定要不要通知前端（只改库不 emit 的话，内存里那条记录仍是 cid 空的版本）。
/// 只做读-改-写一次，且**幂等**：值已经是目标值就不写（投递任务每次尝试都会进来一次）。
/// `msg_id` 不存在时静默返回 `false` —— 内容补发（ContentRequest）复用同一个投递函数，
/// 那条路径的 msg_id 根本不是 `file-*`，此时没有任何东西要补。
pub fn fill_message_sha256(conn: &Connection, msg_id: &str, sha256: &str) -> Result<bool> {
    if sha256.is_empty() {
        return Ok(false);
    }
    let current: Option<String> = conn
        .query_row(
            "SELECT content FROM messages WHERE msg_id = ?1",
            params![msg_id],
            |r| r.get(0),
        )
        .ok();
    let Some(content) = current else {
        return Ok(false);
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Ok(false); // 非 JSON 载荷（理论上不会出现在 file 消息上）：不动它
    };
    if value.get("sha256").and_then(|v| v.as_str()) == Some(sha256) {
        return Ok(false);
    }
    value["sha256"] = serde_json::Value::String(sha256.to_string());
    conn.execute(
        "UPDATE messages SET content = ?2 WHERE msg_id = ?1",
        params![msg_id, value.to_string()],
    )?;
    Ok(true)
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
            kind: crate::protocol::display_kind(&r.get::<_, String>(5)?),
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
    // 清单从 WIRE_KINDS 派生（不手写）：system 是历史遗留的不可搜索项，
    // 静默类（表情回应/撤回）没有可搜正文。
    let unsearchable =
        crate::protocol::sql_kind_list(&["system"], |c| c == crate::protocol::KindClass::Silent);
    let mut stmt = conn.prepare(&format!(
        "SELECT conv_id, msg_id, sender_id, kind, content, ts,
                COUNT(*) OVER (PARTITION BY conv_id) AS total
         FROM messages
         WHERE content LIKE ?1 ESCAPE '\\'
           AND kind NOT IN ({unsearchable})
           -- 只搜**还在会话列表里**的会话：删除级联收口之前入库的历史孤儿
           -- （退群/删会话没清干净的消息）不该继续出现在搜索结果里——
           -- 点又点不开，还挤占「共 N 条」。新数据已在 delete_group/delete_conversation
           -- 同事务清掉，这一条兜住存量与未来任何漏网路径。
           AND conv_id IN (SELECT id FROM conversations)
           AND (?2 IS NULL OR sender_id = ?2)
           AND (?3 IS NULL OR ts >= ?3)
           AND (?4 IS NULL OR ts <= ?4)
         ORDER BY ts DESC, id DESC
         LIMIT ?5",
    ))?;
    let rows = stmt.query_map(
        params![pattern, sender_id, since_ms, until_ms, limit],
        |r| {
            Ok(ChatSearchHit {
                conv_id: r.get(0)?,
                msg_id: r.get(1)?,
                sender_id: r.get(2)?,
                kind: crate::protocol::display_kind(&r.get::<_, String>(3)?),
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
