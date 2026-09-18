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
