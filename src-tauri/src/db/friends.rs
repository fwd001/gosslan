// 职责边界：
// - 好友表 CRUD（insert/get/list/delete_friend）
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

/// 补齐好友**尚缺**的 X25519 / Ed25519 公钥（首次学到时持久化）。
///
/// ⚠️ **只填空位，绝不覆盖已有值**（`COALESCE(x25519_pubkey, ?2)` 而非反过来）。
///
/// 这是身份绑定不被劫持的最后一道闸：调用方来自多个信任级别不同的路径，
/// 只要有一个环节漏判（或将来新增了一条），覆盖式写入就会让一次伪造广播**永久**
/// 改掉好友的真实公钥 —— 此后发给该好友的消息改用攻击者公钥加密，而消息是广播给
/// 所有已连接节点的，攻击者用自己的私钥即可解开；重启也不恢复（只有删好友才清）。
/// 填充式写入把最坏后果从「E2EE 被击穿」降级为「公钥为空时被抢先填一次」。
///
/// 密钥**变更**（对方重装应用）不走这里：那需要用户明确确认，
/// 现有路径是删好友后重新添加（`remove_friend` 会连带删除公钥列，重新添加即重新学习）。
pub fn update_friend_pubkeys(
    conn: &Connection,
    device_id: &str,
    x25519: Option<&str>,
    ed25519: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE friends SET x25519_pubkey = COALESCE(x25519_pubkey, ?2),
                            ed25519_pubkey = COALESCE(ed25519_pubkey, ?3)
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

/// 一次取回好友绑定的两把公钥（X25519, Ed25519）。
///
/// 外层 Option 区分「不是好友」（None）与「是好友但键列还是 NULL」
/// （Some((None, None))，旧行/早期版本）—— 信任锚判定两者语义不同。
pub fn get_friend_pubkeys(
    conn: &Connection,
    device_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    conn.query_row(
        "SELECT x25519_pubkey, ed25519_pubkey FROM friends WHERE device_id = ?1",
        params![device_id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// 列出所有**已绑定 Ed25519 公钥**的好友 `(device_id, ed25519_pubkey)`。
///
/// 公网中继的会合循环用它算通道哈希并做协商验签（`network/transport/relay.rs`）。
/// 两个刻意的选择：
/// - 单条查询而不是"N 次 `get_friend_ed25519`"：这个循环每 10 秒跑一轮，好友上百时
///   N+1 会成为它唯一的成本来源。
/// - `ed25519_pubkey IS NULL` 的老记录**直接不进候选**：没有绑定值就没有可验的身份。
///   走公网时绝不拿"自报公钥 + TOFU"凑数（ADR-0020 D4）—— 那等于把冒充的门开在公网侧。
pub fn list_bound_friend_identities(
    conn: &Connection,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT device_id, ed25519_pubkey FROM friends WHERE ed25519_pubkey IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    // 单行读取失败**跳过而不是整表作废**：一个脏行不该让其余好友都失去公网通路。
    // 但错误本身必须能被调用方看见，所以这里只在行级容忍，语句级错误由 `?` 上抛。
    rows.collect()
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
            // 版本是**内存里的东西**（Hello 时写入），DB 层给空值，由命令层读时富化
            peer_app_version: None,
            peer_version_newer: false,
            online: false,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
