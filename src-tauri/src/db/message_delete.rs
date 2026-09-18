// 职责边界：
// - 单条/多条消息的本地删除（标记 deleted_at 而非硬删）
// ---------------- 单条/多条消息的本地删除 ----------------

/// 本地删除若干条消息（微信语义：**只删本机**，对方那边照常保留）。返回实际删除条数。
///
/// ## 为什么只删 Bubble
/// `system` / `recalled` 也是 Bubble（时间线上占一行），可以删；而 Silent（回应/撤回/置顶）
/// 与 Card（公告/任务/投票）**不在可删范围内** —— 它们不是"聊天内容"，删掉会让聚合视图
/// （置顶条、投票结果、任务面板）凭空缺一块。UI 侧本来也只允许勾选时间线上的消息，
/// 这里是第二道闸门。
///
/// ## 连带处理（少一样都会留下用户看得见的错状态）
/// · `outbox` / `group_outbox`：队列里若还留着这些 msg_id，删完消息后它们仍会被补发 ——
///   用户会看到"我删掉的消息又冒出来了"；
/// · 会话摘要 `last_msg` / `last_ts`：删掉的正好是末条时，列表会停在一条已不存在的消息上；
/// · 未读计数：只做"不超过剩余条数"的收敛（本地没有单聊已读水位，精确重算无从谈起，
///   但至少不会出现"删光了还挂着 5 条未读"）。
///
/// 媒体文件**不删**（磁盘清理由存储策略负责）；收藏**不受影响**（那是独立副本，
/// 见 `favorites` 表的设计说明）；已读水位（`pending_reads` / `group_reads`）也不动 ——
/// 它们是"读到哪个时间点"，不指向具体消息。
pub fn delete_messages(conn: &Connection, msg_ids: &[String]) -> Result<usize> {
    if msg_ids.is_empty() {
        return Ok(0);
    }
    let bubble = crate::protocol::sql_kind_list(&[], |c| c == crate::protocol::KindClass::Bubble);
    let ph = vec!["?"; msg_ids.len()].join(",");
    let tx = conn.unchecked_transaction()?;

    // 受影响会话必须在**删除前**取（删完就查不到它属于谁了）
    let convs: Vec<String> = {
        let sql = format!(
            "SELECT DISTINCT conv_id FROM messages WHERE msg_id IN ({ph}) AND kind IN ({bubble})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(msg_ids.iter()), |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let deleted = tx.execute(
        &format!("DELETE FROM messages WHERE msg_id IN ({ph}) AND kind IN ({bubble})"),
        params_from_iter(msg_ids.iter()),
    )?;
    tx.execute(
        &format!("DELETE FROM outbox WHERE msg_id IN ({ph})"),
        params_from_iter(msg_ids.iter()),
    )?;
    tx.execute(
        &format!("DELETE FROM group_outbox WHERE msg_id IN ({ph})"),
        params_from_iter(msg_ids.iter()),
    )?;

    for conv_id in &convs {
        refresh_conversation_summary(&tx, conv_id)?;
    }
    tx.commit()?;
    Ok(deleted)
}

/// 重算会话的末条摘要与未读（删消息后调用）。
///
/// 预览文案走 `protocol::preview_text` —— 与发送时的口径**同一份实现**，
/// 否则会出现"删掉末条后列表里显示的摘要格式与平时不一样"这种漂移。
fn refresh_conversation_summary(conn: &Connection, conv_id: &str) -> Result<()> {
    let bubble = crate::protocol::sql_kind_list(&[], |c| c == crate::protocol::KindClass::Bubble);
    let last: Option<(String, String, i64)> = conn
        .query_row(
            &format!(
                "SELECT kind, content, ts FROM messages
                 WHERE conv_id = ?1 AND kind IN ({bubble})
                 ORDER BY ts DESC, id DESC LIMIT 1"
            ),
            params![conv_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let remaining: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM messages WHERE conv_id = ?1 AND kind IN ({bubble})"),
        params![conv_id],
        |r| r.get(0),
    )?;
    match last {
        Some((kind, content, ts)) => {
            conn.execute(
                "UPDATE conversations SET last_msg = ?2, last_ts = ?3, unread = MIN(unread, ?4)
                 WHERE id = ?1",
                params![
                    conv_id,
                    crate::protocol::preview_text(&kind, &content),
                    ts,
                    remaining
                ],
            )?;
        }
        // 全删光了：摘要与未读一起清空（否则列表上会留一条指向空会话的行）
        None => {
            conn.execute(
                "UPDATE conversations SET last_msg = NULL, last_ts = NULL, unread = 0 WHERE id = ?1",
                params![conv_id],
            )?;
        }
    }
    Ok(())
}
