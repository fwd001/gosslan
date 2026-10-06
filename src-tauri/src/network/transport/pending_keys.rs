// 待发群密钥登记表：登记 / 判留 / 清理 / 重发（以纯逻辑为主，便于单测）。
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。

/// 登记一个待发群密钥（幂等）。
pub(crate) fn mark_pending_group_key(
    pending: &mut HashMap<String, HashSet<String>>,
    peer_id: &str,
    group_id: &str,
) {
    pending
        .entry(peer_id.to_string())
        .or_default()
        .insert(group_id.to_string());
}

/// 取指定 peer 的待发 group_id 快照（无登记项时为空）。
fn pending_group_key_ids(pending: &HashMap<String, HashSet<String>>, peer_id: &str) -> Vec<String> {
    match pending.get(peer_id) {
        Some(set) => set.iter().cloned().collect(),
        None => Vec::new(),
    }
}

/// 清除一个待发登记项；该 peer 的集合空了则一并移除键，避免无意义增长。
fn clear_pending_group_key(
    pending: &mut HashMap<String, HashSet<String>>,
    peer_id: &str,
    group_id: &str,
) {
    let empty = match pending.get_mut(peer_id) {
        Some(set) => {
            set.remove(group_id);
            set.is_empty()
        }
        None => false,
    };
    if empty {
        pending.remove(peer_id);
    }
}

/// 是否保留登记项等待重试：只有「链路不可用」才保留；
/// 发送成功或失败原因重试无意义（非成员 / 无密钥 / 缺公钥）都清除。
fn should_retain_pending_group_key(result: &Result<(), GroupKeySendErr>) -> bool {
    matches!(result, Err(GroupKeySendErr::NoLink))
}

/// 向指定 peer 发送一次群密钥。GroupKey 消息格式、加密方式与公钥来源
/// （peers 表）均与既有 `redistribute_group_keys` 保持一致。
async fn try_send_group_key(
    state: &AppState,
    peer_id: &str,
    group_id: &str,
) -> Result<(), GroupKeySendErr> {
    let pubkey = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(peer_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(pubkey) = pubkey else {
        return Err(GroupKeySendErr::Fatal);
    };
    let found_group = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_groups(&dbc)
            .unwrap_or_default()
            .into_iter()
            .find(|g| g.id == group_id)
    };
    let Some(g) = found_group else {
        return Err(GroupKeySendErr::Fatal);
    };
    if !g.members.contains(&peer_id.to_string()) {
        return Err(GroupKeySendErr::Fatal);
    }
    let Some(key) = get_group_key(state, group_id).await else {
        return Err(GroupKeySendErr::Fatal);
    };
    let Some(shared) = crypto::shared_secret(&state.identity.x25519_secret, &pubkey) else {
        return Err(GroupKeySendErr::Fatal);
    };
    let Some(sealed) = crypto::seal(&shared, &key) else {
        return Err(GroupKeySendErr::Fatal);
    };
    let clock = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &format!("group:{group_id}"))
    };
    let msg = Message::GroupKey {
        group_id: group_id.to_string(),
        from: state.device_id.clone(),
        to: peer_id.to_string(),
        key: STANDARD.encode(&sealed),
        group_name: g.name.clone(),
        members: g.members.clone(),
        clock,
    };
    // try_send 返回 Err 只可能是「未建立连接」（links 无该 peer）
    try_send(state, peer_id, &msg)
        .await
        .map_err(|_| GroupKeySendErr::NoLink)
}

async fn redistribute_group_keys(state: &AppState, peer_id: &str) {
    let groups = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_groups(&dbc).unwrap_or_default()
    };
    for g in groups {
        if !g.members.contains(&peer_id.to_string()) {
            continue;
        }
        match try_send_group_key(state, peer_id, &g.id).await {
            Ok(()) => {}
            Err(GroupKeySendErr::NoLink) => {
                // announce 先于 ensure_link 执行时 link 尚未建立，此前会静默丢弃
                // 且后续 is_new/key_changed 不再触发 → 成员永久拿不到群密钥。
                // 登记待发，由建链 / Hello / 心跳的 flush_pending_group_keys 重试。
                let mut pending = state
                    .pending_group_keys
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                mark_pending_group_key(&mut pending, peer_id, &g.id);
            }
            // 缺公钥 / 非成员 / 无密钥：重试无意义，不登记
            Err(GroupKeySendErr::Fatal) => {}
        }
    }
}

/// 冲刷指定 peer 的待发群密钥：仅处理该 peer，发送成功即移除登记项；
/// link 仍不可用则保留，等下一次 flush（Hello / 心跳 / 建链）重试。
/// 纯函数：这个 peer 属于哪些群 ⇒ 该给谁重发群密钥。
///
/// 判据单拎出来是为了可测：`members` 里有没有他，是唯一的事实（不看"是否已发过"，
/// 因为下面这条要的就是"每次都确保送到"）。
pub(crate) fn group_ids_containing(groups: &[crate::state::Group], peer_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = groups
        .iter()
        .filter(|g| g.members.iter().any(|m| m == peer_id))
        .map(|g| g.id.clone())
        .collect();
    ids.sort();
    ids
}

/// 链路建立 / Hello / 心跳时，把该 peer 所属的群**全部重新登记**一遍密钥。
///
/// 为什么必须重发而不是"发出去过就算了"（2026-09-24 真机群不同步的根因之一）：
/// `try_send` 返回 Ok 只代表帧进了那条链路的 mpsc 队列（A1-L1 已经把这个教训写进过注释），
/// `flush_pending_group_keys` 据此清除登记 ⇒ 链路随即死掉时 GroupKey 跟着队列一起没了。
/// 而 GroupKey **没有回执帧**，旧代码只在"公钥变化 / 新节点"时才再发一次 ⇒ 密钥永久缺席，
/// 之后每一条群消息都因解不开被静默丢弃（且 msg_id 已进去重表，重发多少次都没人消费）。
/// 接收侧 `handle_group_key` 是幂等的（upsert 群与密钥），所以每次建链多一发小帧
/// 远比"这个群永远不同步"便宜。
fn requeue_group_keys_for_peer(state: &AppState, peer_id: &str) {
    let ids = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let groups = db::list_groups(&dbc).unwrap_or_default();
        group_ids_containing(&groups, peer_id)
    };
    if ids.is_empty() {
        return;
    }
    let mut pending = state
        .pending_group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let set = pending.entry(peer_id.to_string()).or_default();
    for gid in ids {
        set.insert(gid);
    }
}

pub async fn flush_pending_group_keys(state: &AppState, peer_id: &str) {
    let group_ids: Vec<String> = {
        let pending = state
            .pending_group_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending_group_key_ids(&pending, peer_id)
    };
    for gid in group_ids {
        let result = try_send_group_key(state, peer_id, &gid).await;
        if should_retain_pending_group_key(&result) {
            // 仍无链路：保留登记项，等待下一次 flush（Hello / 心跳 / 建链）
            continue;
        }
        let mut pending = state
            .pending_group_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_pending_group_key(&mut pending, peer_id, &gid);
    }
}
