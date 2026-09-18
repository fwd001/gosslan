// 职责边界：
// - 群聊核心（创建、加入、离开、重命名、成员管理）
// - 群密钥协商与轮换
// ---------------- 群聊（群密钥 + Gossip） ----------------

#[tauri::command(async)]
pub fn create_group(
    state: State<'_, Arc<AppState>>,
    name: String,
    members: Vec<String>,
) -> Result<Group, String> {
    let s = state.inner();
    // 群名称长度保护：按字符截断（UTF-8 安全）
    let name: String = name
        .chars()
        .take(MAX_GROUP_NAME_LEN)
        .collect::<String>()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("群名称不能为空".to_string());
    }
    let id = format!("g-{}", Uuid::new_v4());
    let mut all = members;
    all.retain(|m| !m.is_empty() && m != &s.device_id);
    all.sort();
    all.dedup();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        for member in &all {
            if db::get_friend(&dbc, member).is_none() {
                return Err("只能把好友加入群聊".to_string());
            }
        }
    }
    if !all.contains(&s.device_id) {
        all.push(s.device_id.clone());
    }

    // 生成群密钥并持久化
    let key = crypto::random_key();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO groups(id, name, creator, created_at) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, s.device_id, db::now_ms()],
        )
        .map_err(|e| e.to_string())?;
        for member in &all {
            tx.execute(
                "INSERT OR IGNORE INTO group_members(group_id, device_id) VALUES(?1, ?2)",
                rusqlite::params![id, member],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)",
            rusqlite::params![format!("gk:{id}"), STANDARD.encode(key)],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO conversations(id, kind, name, avatar, unread, updated_at)
             VALUES(?1, 'group', ?2, NULL, 0, ?3)",
            rusqlite::params![format!("group:{id}"), name, db::now_ms()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    s.group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), key);

    Ok(Group {
        id,
        name,
        creator: s.device_id.clone(),
        members: all,
    })
}

/// 向群成员分发群密钥（用各成员公钥 ECDH 加密）。
/// 同时携带群名与成员列表：成员端据此建本地群记录，否则群名会兜底成「群聊 g-xxxx」。
#[tauri::command(async)]
pub async fn distribute_group_key(
    state: State<'_, Arc<AppState>>,
    group_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;
    // 同时取群名与成员：成员端靠它建立/刷新本地群记录（含成员表）
    let (group_name, members) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| (g.name, g.members))
            .unwrap_or_default()
    };
    let clock = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &format!("group:{group_id}"))
    };
    for m in &members {
        if m == &s.device_id {
            continue;
        }
        // peers 优先、friends 回落：peers 由 announce/Hello 实时维护，
        // friends 表公钥可能因 accept 路径未补写而缺失（曾致 GroupKey 静默跳过）
        let Some(pubkey) = resolve_member_x25519(s, m) else {
            continue;
        };
        let Some(shared) = crypto::shared_secret(&s.identity.x25519_secret, &pubkey) else {
            continue;
        };
        let Some(sealed) = crypto::seal(&shared, &key) else {
            continue;
        };
        let msg = Message::GroupKey {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            to: m.clone(),
            key: STANDARD.encode(&sealed),
            group_name: group_name.clone(),
            members: members.clone(),
            clock,
        };
        if try_send(s, m, &msg).await.is_err() {
            // 目标成员尚无 TCP link（建群时 ensure_link 可能尚未执行）：
            // 不再静默丢弃，登记待发，由建链 / Hello / 心跳的
            // flush_pending_group_keys 补发（与 redistribute_group_keys 同一机制）。
            let mut pending = s
                .pending_group_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            mark_pending_group_key(&mut pending, m, &group_id);
        }
    }
    Ok(())
}

#[tauri::command(async)]
pub fn get_groups(state: State<'_, Arc<AppState>>) -> Vec<Group> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::list_groups(&dbc).unwrap_or_default()
}

#[tauri::command(async)]
pub fn get_group_reads(state: State<'_, Arc<AppState>>, group_id: String) -> Vec<GroupReadInfo> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::list_group_reads(&dbc, &group_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(reader_id, last_read_ts)| GroupReadInfo {
            reader_id,
            last_read_ts,
        })
        .collect()
}

/// 重命名群：仅创建者可操作。本地改名 + 同步会话标题后，广播给全部成员。
#[tauri::command(async)]
pub async fn rename_group(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    name: String,
) -> Result<(), String> {
    let s = state.inner();
    let name: String = name.chars().take(MAX_GROUP_NAME_LEN).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("群名称不能为空".to_string());
    }
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以修改群名称".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::rename_group(&dbc, &group_id, &name).map_err(|e| e.to_string())?;
    }
    for m in &group.members {
        if m == &s.device_id {
            continue;
        }
        let msg = Message::GroupRename {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            name: name.clone(),
        };
        let _ = try_send(s, m, &msg).await;
    }
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 向成员列表里的每一位重发当前群密钥（携带群名 + 最新成员表）。
async fn resend_group_key_to(s: &AppState, group_id: &str, members: &[String], key: [u8; 32]) {
    let group_name = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, group_id)
            .map(|g| g.name)
            .unwrap_or_default()
    };
    let clock = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &format!("group:{group_id}"))
    };
    for m in members {
        if m == &s.device_id {
            continue;
        }
        // peers 优先、friends 回落（与 distribute_group_key 同一来源策略）
        let Some(pubkey) = resolve_member_x25519(s, m) else {
            continue;
        };
        let Some(shared) = crypto::shared_secret(&s.identity.x25519_secret, &pubkey) else {
            continue;
        };
        let Some(sealed) = crypto::seal(&shared, &key) else {
            continue;
        };
        let msg = Message::GroupKey {
            group_id: group_id.to_string(),
            from: s.device_id.clone(),
            to: m.clone(),
            key: STANDARD.encode(&sealed),
            group_name: group_name.clone(),
            members: members.to_vec(),
            clock,
        };
        if try_send(s, m, &msg).await.is_err() {
            // 目标成员尚无 TCP link：登记待发，由建链 / Hello / 心跳的
            // flush_pending_group_keys 补发（与 redistribute_group_keys 同一机制）。
            let mut pending = s
                .pending_group_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            mark_pending_group_key(&mut pending, m, group_id);
        }
    }
}

/// 加人入群：仅创建者。本地落成员后，用**当前**群密钥重发给全体成员（含新成员）。
///
/// ⚠️ 这里刻意**不轮换**群密钥：
/// - 加人没有前向保密收益——新成员本来就没有旧密钥，转不转旧消息他都解不开；
/// - `handle_group_key` 只接受**群主**分发的密钥（防止成员伪造密钥劫持群聊），
///   所以一旦轮换，新密钥就只存在于群主本机。群主一旦离线，其他成员永远拿不到，
///   整群消息都无法解密。不轮换则密钥始终是全体成员都持有的那一个，与群主是否在线无关。
/// - 轮换只在「移除成员」时做（撤销被移除者的解密能力），那个时机群主必然在线。
#[tauri::command(async)]
pub async fn group_add_member(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    device_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以添加成员".to_string());
    }
    if group.members.contains(&device_id) {
        return Err("该成员已在群中".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &device_id).is_none() {
            return Err("只能添加好友入群".to_string());
        }
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::add_group_member(&dbc, &group_id, &device_id).map_err(|e| e.to_string())?;
    }
    let key = get_group_key(s, &group_id)
        .await
        .ok_or_else(|| "群密钥缺失".to_string())?;
    let current = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.members)
            .unwrap_or_default()
    };
    // 现有成员收到的是同一把密钥（幂等刷新），新成员借此首次拿到密钥
    resend_group_key_to(s, &group_id, &current, key).await;
    // 加人通知：此前完全缺失（见 group_member_added_text 的说明）。
    //
    // ⚠️ 必须走 `send_group_payload`（群密钥加密 + gossip + 每个成员的 outbox），
    // **不能**用 `insert_group_system_message` —— 那个只写本机，其他成员看不到，
    // 就失去了"通知全体"的意义。踢人/退群之所以用本地插入，是因为它们本来就有
    // 专用控制帧广播（GroupMemberRemoved / GroupMemberLeft）；而加人没有控制帧
    // （靠 GroupKey 重发携带新成员表），所以直接借用消息管道。
    let name = resolve_nickname(s, &device_id);
    let text = crate::network::transport::group_member_added_text(s, &name);
    send_group_payload(s, &group_id, "system", text).await?;
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 移人出群：仅创建者。轮换群密钥发给剩余成员，并向被移除者发 GroupMemberRemoved。
#[tauri::command(async)]
pub async fn group_remove_member(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    device_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以移除成员".to_string());
    }
    if device_id == s.device_id {
        return Err("不能移除自己".to_string());
    }
    if !group.members.contains(&device_id) {
        return Err("该成员不在群中".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::remove_group_member(&dbc, &group_id, &device_id).map_err(|e| e.to_string())?;
        // 被移除者不再属于该群：清掉仍指向它的待补发群消息，避免重连时向群外成员投递。
        db::delete_group_outbox_for_peer_in_group(&dbc, &group_id, &device_id).ok();
    }
    // 轮换群密钥：被移除者失去解密能力
    let key = crypto::random_key();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, &format!("gk:{group_id}"), &STANDARD.encode(key)).ok();
    }
    s.group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(group_id.clone(), key);
    let remaining = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.members)
            .unwrap_or_default()
    };
    resend_group_key_to(s, &group_id, &remaining, key).await;
    let removed_msg = Message::GroupMemberRemoved {
        group_id: group_id.clone(),
        from: s.device_id.clone(),
        to: device_id.clone(),
    };
    // ① 通知**被移除者本人**清理本地群
    let _ = try_send(s, &device_id, &removed_msg).await;
    // ② **同时通知其余成员**：此前只发给被移除者，而接收端对 `to != 自己` 直接 return，
    //    两头都断 —— 表现为「群里其他人打开群，成员没变少、也没有任何提示」。
    //    现在其余成员收到后同步成员表 + 落一条群内系统消息。
    for m in &remaining {
        if m == &s.device_id || m == &device_id {
            continue;
        }
        let _ = try_send(s, m, &removed_msg).await;
    }
    // ③ 群主自己也要看到这条系统消息（别人靠 ② 各自插入）
    let name = resolve_nickname(s, &device_id);
    crate::network::transport::insert_group_system_message(
        s,
        &group_id,
        &crate::network::transport::group_member_removed_text(s, &name),
    );
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 转让群主：仅**当前**群主可发起，目标必须是群成员。
/// 本地更新创建者后广播 `GroupCreatorChanged` 给全体成员（含新群主本人）。
/// 用于群主更换设备/卸载前移交管理权，避免群永久失去改名/加人/踢人能力。
#[tauri::command(async)]
pub async fn transfer_group_creator(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    new_creator: String,
) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以转让群主".to_string());
    }
    if new_creator == s.device_id {
        return Err("不能把群主转让给自己".to_string());
    }
    if !group.members.contains(&new_creator) {
        return Err("只能转让给群成员".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_group_creator(&dbc, &group_id, &new_creator).map_err(|e| e.to_string())?;
    }
    for m in &group.members {
        if m == &s.device_id {
            continue;
        }
        let msg = Message::GroupCreatorChanged {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            to: new_creator.clone(),
        };
        let _ = try_send(s, m, &msg).await;
    }
    // 群内提示：其余成员各自在 `handle_group_creator_changed` 里插一条，发起方（原群主）
    // 走的是 `from == 自己 ⇒ 直接 return` 那条早退，所以必须在这里补上，否则只有发起方
    // 看不到这次转让 —— 与「踢人」的 ③ 是同一个坑。
    let name = resolve_nickname(s, &new_creator);
    crate::network::transport::insert_group_system_message(
        s,
        &group_id,
        &crate::network::transport::group_creator_changed_text(s, &name),
    );
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 退出群聊：群主须先转让（否则该群会永久失去管理权）。
/// 退群后清理本地群记录 / 会话 / 群密钥，并广播 `GroupMemberLeft` 让其余成员更新成员表。
/// 复用与「被移出群」同一套本地清理路径（`db::delete_group`）。
#[tauri::command(async)]
pub async fn leave_group(state: State<'_, Arc<AppState>>, group_id: String) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator == s.device_id {
        return Err("群主退群前请先转让群主".to_string());
    }
    // 先通知其余成员（本地记录删除前取成员表）；对方离线时消息会丢失，
    // 但成员表也会随后续群消息（Gossip group_members / GroupKey）自愈。
    for m in &group.members {
        if m == &s.device_id {
            continue;
        }
        let msg = Message::GroupMemberLeft {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
        };
        let _ = try_send(s, m, &msg).await;
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_group(&dbc, &group_id).map_err(|e| e.to_string())?;
        let _ = dbc.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![format!("gk:{group_id}")],
        );
    }
    s.group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&group_id);
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}
