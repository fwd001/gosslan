// 会话链路快照清理、好友资料更新、群成员 X25519 取值（peers 优先 / friends 回落）。
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。

/// 清掉某会话的链路快照（`conv_link` 是内存态；无条目时无操作）。
pub(crate) fn clear_conv_link(state: &AppState, conv_id: &str) {
    state
        .conv_link
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(conv_id);
}

pub(crate) fn maybe_update_friend(
    state: &AppState,
    device_id: &str,
    nickname: &str,
    avatar: Option<String>,
) {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if db::get_friend(&dbc, device_id).is_some() {
        db::add_friend(&dbc, device_id, nickname, avatar.as_deref()).ok();
        // 好友行常在公钥落库之后才创建（好友申请通过才 add_friend），此处每次补一次绑定：
        // x25519 保证 E2EE 取得到对方公钥，ed25519 只认被证明过的来源（见 helper 的注释）。
        bind_friend_keys_on_accept(state, &dbc, device_id);
    }
}

/// 群成员公钥选择：peers 表优先、friends 表回落（纯逻辑，便于单测）。
/// peers 表由 announce / Hello 实时维护，几乎总是最新；
/// friends 表可能缺失——accept 方路径此前不补写公钥，
/// 且公钥不变时 key_changed 不触发 maybe_update_friend。
fn pick_member_x25519(peers_key: Option<String>, friends_key: Option<String>) -> Option<String> {
    peers_key.or(friends_key)
}

/// 解析群成员的 X25519 公钥：peers 优先、friends 回落，都缺失才返回 None。
/// 群密钥分发（distribute_group_key / resend_group_key_to）统一走这里，
/// 避免 friends 表公钥缺失导致 GroupKey 被静默跳过、成员永久拿不到群密钥。
pub(crate) fn resolve_member_x25519(state: &AppState, member_id: &str) -> Option<String> {
    let peers_key = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(member_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let friends_key = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend_x25519(&dbc, member_id)
    };
    pick_member_x25519(peers_key, friends_key)
}

pub async fn touch_peer(state: &AppState, device_id: &str) {
    let ts = db::now_ms();
    if let Some(p) = state
        .peers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(device_id)
    {
        p.last_seen = ts;
    }
    state.emit_peers();
}

/// 链路全断时调用：清掉"当前可达路径"，但**保留节点条目**。
///
/// ## 为什么不再立刻删节点（2026-09-12 真机，用户 4.2.11 复测）
///
/// 用户真机（只开蓝牙）：**手机能看到 Mac（显示"已发现未建联"），Mac 里安卓什么都不显示**。
/// 根因就在这一行：BLE 上"连上 → 被对端按指定拨号方退让 → 断开"是**常态**
/// （见 `dial_and_register` 的 `should_dial_ble`），Mac 每次刚学到手机身份（`new_peer=true`）
/// 就因对端退让而断链，于是立刻被这里删掉 ⇒ 「添加好友」列表（数据源就是节点表）里
/// **只闪一下就没了**，用户根本没机会点"加好友"；而小 id 那一侧（手机）自退让时
/// 从未登记过链路，自然不会调到这里，所以它反而一直显示"已发现未建联"。
///
/// 现在保留条目、交给 `sweep_peers` 的 45s 超时收割：
///   · 「添加好友」能在 45s 窗口里列出刚见过的节点（与手机侧行为一致）；
///   · 待发的好友申请也能在这段时间里随下一次建链补发
///     （`flush_pending_friend_request` 在建链/Hello 时触发）。
/// "在线"不再靠"在不在节点表里"判定 —— 见 `commands::friend_is_online`
/// （那条 presence 判据正是 2026-09-12 复核抓到的"连过又掉线 ⇒ 永久在线"的 High 缺陷来源）。
pub(crate) async fn mark_peer_offline(state: &Arc<AppState>, device_id: &str) {
    // 掉线：与「建链」配对，是判断「真离线」还是「被误清」的关键。
    // 注意它与 sweep_peers 的超时清理是**两条不同的路径**，只有日志能区分。
    state
        .logger
        .info("link", format!("掉线 peer={device_id}（链路断开）"));
    // 链路快照必须立刻失效：`conv_link` 记的是"当前可达路径"，链路没了路径就没了。
    // 不清掉的话，聊天头部的链路徽标会在离线后继续显示（用户 2026-09-12 反馈的
    // 「离线却显示『桥接 1』」）。
    clear_conv_link(state, device_id);
    state.emit_peers();
}
