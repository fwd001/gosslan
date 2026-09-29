// 职责边界：
// - 群组表 CRUD + 成员管理
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
///
/// ⚠️ **群名只由群主改**（`groups.creator == 传入的 creator` 才更新 name）。
///
/// 本函数有一条来自**群 Gossip 消息**的调用路径：信封里的 `group_name` / `group_creator`
/// 都是发送方自报的，任何持群密钥的成员都能填任意文本。若无条件覆盖群名，
/// 「改名」就出现了两条路径 —— 专用的 `GroupRename` 帧严格要求群主，
/// 而这条没有任何检查，等于把授权绕过去了（可伪造成"系统通知""群主"做社工）。
/// 群成员表是 `INSERT OR IGNORE`（只增不减），creator 也只在 INSERT 时写入，故不受影响。
///
/// 注意：这只是**持久层的兜底**，调用点还应校验 `env.sender_id == creator` ——
/// 否则成员只要把 `group_creator` 填成真群主的 id，creator 就对得上，闸门又会失效。
/// 合法的改名走 `rename_group`（已由 `handle_group_rename` 限群主）。
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
             ON CONFLICT(id) DO UPDATE SET name = CASE
                 WHEN groups.creator = excluded.creator THEN excluded.name
                 ELSE groups.name
             END",
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

/// 彻底删除一个群：群表 + 成员关系 + 会话 + 群文件投递数据 + 聊天历史与其投递残留。
/// 被移除的成员端收到通知后调用；退群（leave_group）复用同一收口。
///
/// ⚠️ 全程单事务：这张清单曾经散着写，漏删过消息与已读水位 ——
/// 孤儿消息留在 `messages` 里会继续命中全局搜索（点又点不开），
/// `group_outbox`/`file_outbox` 残留会让补发任务给已不存在的关系发帧。
/// 刻意**保留**的：`conversation_clocks`（逻辑序号只增不减，删了会撞历史序号）、
/// `content_transfers`（按 cid 键，跨会话共享，删除需引用计数——登记为已知限制）。
pub fn delete_group(conn: &Connection, group_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let conv_id = format!("group:{group_id}");
    tx.execute(
        "DELETE FROM group_members WHERE group_id = ?1",
        params![group_id],
    )?;
    // 群文件投递数据随群删除，避免悬挂（按 group_id 关联逐层清理）
    tx.execute(
        "DELETE FROM group_file_recipients WHERE transfer_id IN
         (SELECT transfer_id FROM group_files WHERE group_id = ?1)",
        params![group_id],
    )?;
    tx.execute(
        "DELETE FROM group_files WHERE group_id = ?1",
        params![group_id],
    )?;
    // —— 以下为聊天历史与投递/回执残留 ——
    tx.execute("DELETE FROM messages WHERE conv_id = ?1", params![conv_id])?;
    delete_group_outbox_for_group(&tx, group_id)?;
    tx.execute(
        "DELETE FROM file_outbox WHERE group_id = ?1",
        params![group_id],
    )?;
    tx.execute(
        "DELETE FROM group_reads WHERE group_id = ?1",
        params![group_id],
    )?;
    tx.execute(
        "DELETE FROM pending_group_reads WHERE group_id = ?1",
        params![group_id],
    )?;
    tx.execute(
        "DELETE FROM group_recalled_messages WHERE conv_id = ?1",
        params![conv_id],
    )?;
    tx.execute(
        "DELETE FROM settings WHERE key = ?1",
        params![crate::db::clear_boundary_key(group_id)],
    )?;
    tx.execute("DELETE FROM groups WHERE id = ?1", params![group_id])?;
    tx.execute(
        "DELETE FROM conversations WHERE id = ?1",
        params![conv_id],
    )?;
    tx.commit()?;
    Ok(())
}

// 模块名刻意不叫 `tests`：本文件被 `db.rs` 用 `include!` 贴进 `db` 命名空间，
// 同名模块会和 `favorites_tests.rs` 里的 `mod tests` 撞成 E0428。
#[cfg(test)]
mod group_rename_tests {
    use super::{rename_group, upsert_group};
    use crate::db::{ensure_conversation, SCHEMA};
    use rusqlite::Connection;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn name_of(conn: &Connection, sql: &str, id: &str) -> Option<String> {
        conn.query_row(sql, [id], |r| r.get::<_, String>(0)).ok()
    }

    /// 群改名是**两处写**：`groups.name` 与去规范化到会话行的 `conversations.name`。
    /// 第二处漏掉时，会话列表停在旧标题、群详情已是新名 —— 两处用户都直接看得见。
    /// 这一条只判"落库这一半"；跨进程那一半（成员那头收不收得到）今天仍没有判据，
    /// 记在 docs/final-architecture-review.md 的 §七 取数纪律第 6 条。
    #[test]
    fn rename_group_updates_both_group_row_and_its_conversation_title() {
        let conn = mem();
        upsert_group(&conn, "g1", "旧名", "me", &["me".to_string(), "b".to_string()]).unwrap();
        ensure_conversation(&conn, "group:g1", "group", "旧名", None).unwrap();
        upsert_group(&conn, "g2", "隔壁群", "me", &["me".to_string()]).unwrap();
        ensure_conversation(&conn, "group:g2", "group", "隔壁群", None).unwrap();

        rename_group(&conn, "g1", "新名").unwrap();

        assert_eq!(
            name_of(&conn, "SELECT name FROM groups WHERE id=?1", "g1").as_deref(),
            Some("新名"),
            "groups.name 要跟着改"
        );
        assert_eq!(
            name_of(&conn, "SELECT name FROM conversations WHERE id=?1", "group:g1").as_deref(),
            Some("新名"),
            "会话行那份去规范化的名字必须一起改（漏了就是标题漂移）"
        );
        assert_eq!(
            name_of(&conn, "SELECT name FROM conversations WHERE id=?1", "group:g2").as_deref(),
            Some("隔壁群"),
            "别的群不许被带动"
        );
    }

    /// 反例输入：`groups` 里没有这一行时，`changed > 0` 那道闸必须把第二处写挡住 ——
    /// 否则一条对不上号的改名请求会凭空改掉一个同名会话的标题。
    /// 没有这一条，上面那个正例挡不住"去掉 if changed > 0 看着更简洁"的回归。
    #[test]
    fn rename_group_of_missing_group_leaves_conversation_row_untouched() {
        let conn = mem();
        ensure_conversation(&conn, "group:nope", "group", "残留标题", None).unwrap();

        rename_group(&conn, "nope", "不该生效").unwrap();

        assert_eq!(
            name_of(&conn, "SELECT name FROM conversations WHERE id=?1", "group:nope").as_deref(),
            Some("残留标题"),
            "群不存在时会话标题必须一字不动"
        );
    }
}

/// 群任务编号的**高水位**键（不另建表：`settings` 就是现成的 kv）。
fn todo_number_key(group_id: &str) -> String {
    format!("todo_num:{group_id}")
}

/// 群里"已经用过的最大任务编号"。两个来源缺一不可：
/// - `settings` 的高水位 ⇒ 即便有人删了聊天记录，号也不会降回去（**永不复用**）；
/// - 扫 `messages` 里 todo / todo_update 载荷的最大 `number` ⇒ **对端自己分配的号**
///   本机没写过水位也能看见（Lamport 式：见过就要躲开，否则两边同时建就是同一条 #N）。
/// 取两者较大；扫到坏 JSON 直接跳过（不能让一条脏行把整个建任务卡死）。
pub fn todo_number_high_water(conn: &Connection, group_id: &str) -> i64 {
    let stored: i64 = crate::db::get_setting(conn, &todo_number_key(group_id))
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let mut seen = 0i64;
    if let Ok(mut stmt) = conn.prepare(
        "SELECT content FROM messages WHERE conv_id = ?1 AND kind IN ('todo', 'todo_update')",
    ) {
        let conv_id = format!("group:{group_id}");
        if let Ok(rows) = stmt.query_map(params![conv_id], |r| r.get::<_, String>(0)) {
            for row in rows.flatten() {
                if let Ok(p) = serde_json::from_str::<crate::protocol::TodoPayload>(&row) {
                    if p.number > seen {
                        seen = p.number;
                    }
                }
            }
        }
    }
    stored.max(seen)
}

/// 分配群里下一个任务编号 = 已用最大 + 1，同时把高水位推上去。
///
/// ⚠️ 只在**创建**一条任务时调用。改状态 / 改标题 / 归档都不走这里（编号不重分配），
/// 这是"发起生成后后续编辑修改归档都不会改变"的全部实现。
pub fn next_todo_number(conn: &Connection, group_id: &str) -> Result<i64> {
    let next = todo_number_high_water(conn, group_id) + 1;
    crate::db::set_setting(conn, &todo_number_key(group_id), &next.to_string())?;
    Ok(next)
}

// 模块名不叫 `tests`：本文件被 `db.rs` 用 `include!` 贴进 `db` 命名空间，模块名要在 db 作用域里全局唯一。
#[cfg(test)]
mod todo_number_tests {
    use super::{next_todo_number, todo_number_high_water};
    use crate::db::{ensure_conversation, insert_message, MessageRecord, SCHEMA};
    use rusqlite::Connection;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        ensure_conversation(&conn, "group:g1", "group", "g1", None).unwrap();
        conn
    }

    /// 往群里写一条"某成员建的任务"（只关心载荷里的 `number`，其余字段走缺省）。
    fn seed_todo(conn: &Connection, msg_id: &str, todo_id: &str, number: i64) {
        let rec = MessageRecord {
            id: 0,
            msg_id: msg_id.into(),
            conv_id: "group:g1".into(),
            sender_id: "peer".into(),
            receiver_id: String::new(),
            kind: "todo".into(),
            content: format!(r#"{{"todo_id":"{todo_id}","title":"t","number":{number}}}"#),
            ts: 1,
            seq: 1,
            status: "sent".into(),
            mention_targets: None,
        };
        insert_message(conn, &rec).unwrap();
    }

    #[test]
    fn numbers_start_at_one_and_increment_within_the_group() {
        let conn = mem();
        assert_eq!(todo_number_high_water(&conn, "g1"), 0);
        assert_eq!(next_todo_number(&conn, "g1").unwrap(), 1);
        assert_eq!(next_todo_number(&conn, "g1").unwrap(), 2);
        assert_eq!(next_todo_number(&conn, "g1").unwrap(), 3);
    }

    /// 编号永不复用：把聊天记录删光也不许把号降回去（否则"这条是 #1"会指到两条上）。
    #[test]
    fn numbers_are_not_reused_after_messages_are_deleted() {
        let conn = mem();
        let first = next_todo_number(&conn, "g1").unwrap();
        seed_todo(&conn, "m1", "t1", first);
        // ⚠️ 这里用裸 DELETE 而不是 `delete_messages`：实测后者**删不动 todo 行**
        // （`db/message_delete.rs` 的谓词只圈 Bubble 类 kind，`todo` 是 Card 类）。
        // 于是"消息行没了"的真实路径是**删会话 / 清空聊天记录**那一条，夹具必须走那条，
        // 否则断言建在一次根本没发生的删除上（本轮第一版就是这么假绿的，被上面那行计数断言抓住）。
        assert_eq!(
            conn.execute("DELETE FROM messages WHERE conv_id = 'group:g1'", []).unwrap(),
            1
        );
        // 只钉"行真的没了"这一件事。`todo_number_high_water` 在这里**不该**归零 ——
        // 高水位正是靠 settings 那份不受消息删除影响的记录活下来的（上一行已经删掉了消息行）。
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM messages WHERE conv_id = 'group:g1'", [], |r| r.get(0))
                .unwrap(),
            0
        );
        assert_eq!(next_todo_number(&conn, "g1").unwrap(), first + 1);
    }

    /// 对端自己分配的号也算"用过"：本机的水位只记自己发过的号，
    /// 少了这一半，两台离线各建一条就会给出同一个 #N。
    #[test]
    fn peer_allocated_numbers_are_avoided() {
        let conn = mem();
        seed_todo(&conn, "m1", "peer-task", 7);
        assert_eq!(todo_number_high_water(&conn, "g1"), 7);
        assert_eq!(next_todo_number(&conn, "g1").unwrap(), 8);
    }

    /// 旧版本发来的载荷没有 `number` ⇒ 解析成 0（无号），而不是解析失败。
    /// 这条是"不迁移、不破坏旧数据"的前提：它红了就说明新字段让老载荷读不出来。
    #[test]
    fn legacy_payload_without_number_still_parses_as_zero() {
        let p: crate::protocol::TodoPayload =
            serde_json::from_str(r#"{"todo_id":"t","title":"x"}"#).unwrap();
        assert_eq!(p.number, 0);
        // 0 不参与高水位：不能因为见过一条无号任务就把号推到 1 之后
        let conn = mem();
        let rec = MessageRecord {
            id: 0,
            msg_id: "m0".into(),
            conv_id: "group:g1".into(),
            sender_id: "peer".into(),
            receiver_id: String::new(),
            kind: "todo".into(),
            content: r#"{"todo_id":"legacy","title":"x"}"#.into(),
            ts: 1,
            seq: 1,
            status: "sent".into(),
            mention_targets: None,
        };
        insert_message(&conn, &rec).unwrap();
        assert_eq!(todo_number_high_water(&conn, "g1"), 0);
        assert_eq!(next_todo_number(&conn, "g1").unwrap(), 1);
    }
}
