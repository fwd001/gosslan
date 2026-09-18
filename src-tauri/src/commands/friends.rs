// 职责边界：
// - 好友列表查询、安全号
// - 删除好友、pending request 登记
// - 建链补发路径（send_friend_request_via_link）
// ---------------- 好友 ----------------

/// 好友「在线」判据：**最近 [`FRIEND_ONLINE_GRACE_MS`] 内见过**（announce / Presence /
/// 握手都算 `last_seen`）**或**手里有活链路。
///
/// 为什么不沿用「在不在 `peers` 表里」：`mark_peer_offline` 现在**故意保留**刚掉线的
/// 节点条目（BLE 上"连上→被对端退让→断开"是常态；删掉的话 Mac 的「添加好友」列表里
/// 安卓只闪一下、用户根本点不到），所以"在表里"不再等价于"在线"，必须看
/// `last_seen` 的新鲜度 —— 否则就是 2026-09-12 复核抓到的那个 High 缺陷
/// （一次「连过又掉线」的节点永久显示在线）。
///
/// 15s ≈ 3 个 announce 周期（5s 基础 + 0~3s 抖动）：够容忍局域网丢一两轮广播，
/// 又不会把早已离开的节点长时间标成在线。
fn friend_is_online(last_seen: i64, now: i64, has_active_link: bool) -> bool {
    has_active_link || last_seen >= now - FRIEND_ONLINE_GRACE_MS
}

const FRIEND_ONLINE_GRACE_MS: i64 = 15_000;

/// 安全码：本机与指定对端之间那串**双方一致**的核对码（见 `crypto::safety_number`）。
///
/// 返回 `None` 表示**还算不出来** —— 缺对方的公钥（尚未通过 Hello/announce 学到）。
/// 这时**必须如实返回 None 而不是拿 device_id 凑一个**：凑出来的码在真正的中间人
/// 攻击下与真实对端的码不同，用户核对后会以为"对得上"，比没有更糟。
///
/// 对端公钥取 peers 优先、friends 回落（与 `resolve_member_x25519` 同一口径）。
#[tauri::command(async)]
pub fn get_safety_number(state: State<'_, Arc<AppState>>, peer_id: String) -> Option<String> {
    let s = state.inner();
    let (their_x, their_e) = {
        let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
        let p = peers.get(&peer_id);
        (
            p.and_then(|p| p.x25519_pubkey.clone()),
            p.and_then(|p| p.ed25519_pubkey.clone()),
        )
    };
    let (their_x, their_e) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        (
            their_x.or_else(|| db::get_friend_x25519(&dbc, &peer_id)),
            their_e.or_else(|| db::get_friend_ed25519(&dbc, &peer_id)),
        )
    };
    let (their_x, their_e) = (their_x?, their_e?);
    Some(crypto::safety_number(
        &crypto::SafetyParty {
            device_id: &s.device_id,
            x25519_pubkey: &s.identity.x25519_public_b64(),
            ed25519_pubkey: &s.identity.ed25519_public_b64(),
        },
        &crypto::SafetyParty {
            device_id: &peer_id,
            x25519_pubkey: &their_x,
            ed25519_pubkey: &their_e,
        },
    ))
}

#[tauri::command(async)]
pub fn get_friends(state: State<'_, Arc<AppState>>) -> Vec<Friend> {
    let s = state.inner();
    let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
    // 同时检查活跃 TCP 链接：链路存活但 peer 已被 discovery sweep 清掉时，
    // 仍应显示在线，避免「实际可通信但 UI 显示离线」。
    // ⚠️ 只算**非空** Vec（与 `sweep_peers` / `has_link` 同一口径）：
    // 残留的空 key 会让好友**永久显示在线**。根因已在 reader_loop 修掉，这里是防线。
    let active_links: std::collections::HashSet<String> = s
        .links
        .try_lock()
        .map(|l| {
            l.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let now = crate::db::now_ms();
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut friends = db::list_friends(&dbc).unwrap_or_default();
    for f in friends.iter_mut() {
        let last_seen = peers.get(&f.device_id).map(|p| p.last_seen).unwrap_or(0);
        f.online = friend_is_online(last_seen, now, active_links.contains(&f.device_id));
        // 设备类型从 peers 表现场读取（Hello/UserInfo/Presence 都会更新它）。
        f.device_type = peers
            .get(&f.device_id)
            .map(|p| p.device_type.clone())
            .unwrap_or_default();
    }
    friends
}

/// 删除好友（保留聊天记录；对方仍会出现在扫描列表，可重新添加）。
#[tauri::command(async)]
pub async fn remove_friend(state: State<'_, Arc<AppState>>, peer_id: String) -> Result<(), String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::remove_friend(&dbc, &peer_id).map_err(|e| e.to_string())?;
        db::delete_file_outbox_for_peer(&dbc, &peer_id).ok();
    }
    // ⚠️ **同时解除内存里的身份绑定**（用户 2026-09-13 真机：不这么做就"必须重启"）。
    // `friends` 表那一行删掉只解除了一条腿；`verify_hello` 还会回落到内存 `peers` 表里
    // 广播学来的旧公钥 ⇒ 对方重装换过公钥时，删了好友重新加也照样被硬拒。
    crate::network::transport::forget_peer_identity(s, &peer_id);
    // 通知对方解除好友关系（对方收到后也会删除本机好友行）
    let msg = Message::FriendRemove {
        from: s.device_id.clone(),
        to: peer_id.clone(),
    };
    let _ = try_send(s, &peer_id, &msg).await;
    let _ = s.app.emit("friend-removed", &peer_id);
    Ok(())
}

// 待处理的好友申请。
//
// **规则（用户 2026-09-12 真机实测要求）**：已经在好友列表里的人，其申请不该再出现
// ——「如果该好友已在好友列表的话，列表里的那个好友申请就应该自动清除掉」。
// 主修在各条"同意"路径上清 `pending_requests`（见 `transport::forget_pending_request`），
// 这里按 friends 表再过滤一遍并**顺手把内存态收敛掉**：万一哪条路径漏了（或对方是走
// 别的消息把我加上的），「新朋友」里也不会留着一条永远处理不掉的过期申请。
