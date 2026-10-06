// 节点表写入、公钥冲突告警、聊天样式补发
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第三批；守卫视图与领域图的分册清单一同登记。

/// 同一 device_id 报出与已绑定值不同的公钥时，向用户给出**一次**可见告警。
///
/// 为什么必须让用户看见：这是区分「对方重装了应用」与「有人冒名顶替」的唯一外部信号。
/// 静默处理会让用户无法判断，违反 AI_RULES §19「不得静默接受不可验证的密钥」。
/// 为什么只在首次告警：announce 每 5s 一次、冲突会持续存在，不去重会把聊天记录刷爆。
///
/// 安全行为不变：冲突时**绝不覆盖**已绑定的公钥（见 `upsert_peer` 的 key_conflict 分支），
/// 因此最坏情况只是对方真的重装后我方需要重新建立信任，而不会把消息发给冒充者的密钥。
fn warn_key_conflict_once(state: &AppState, device_id: &str) {
    {
        let mut warned = state
            .key_conflict_warned
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !warned.insert(device_id.to_string()) {
            return;
        }
    }
    // 非好友不建会话：避免陌生节点刷出一串空会话
    let name = resolve_nickname(state, device_id);
    let is_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend(&dbc, device_id).is_some()
    };
    if !is_friend {
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::ensure_conversation(&dbc, device_id, "single", &name, None).ok();
    }
    crate::commands::insert_system_message(
        state,
        device_id,
        &format!(
            "⚠️「{name}」的身份密钥发生变化，已保留原密钥未替换。\
             如果对方刚重装过应用：**删掉这个好友再重新添加即可**（聊天记录会保留、不用重启）；\
             如果不是本人操作，就别继续，对方可能被人冒名顶替。"
        ),
    );
}

/// 「将要新建的 peers 条目」与 friends 锚是否冲突。
///
/// 判据与 `upsert_peer` Some 分支的 key_conflict 完全同口径：锚列有值、自报也有值、
/// 且不同才算冲突；好友行的 NULL 列（旧数据/键未同步）不构成冲突，维持既有的宽容。
/// 抽成纯函数是为了让这条安全判定有名字、有单测（同 `hello_auth_decision` 的做法）。
fn new_peer_conflicts_with_friend(
    friend: Option<(Option<&str>, Option<&str>)>,
    x25519: Option<&str>,
    ed25519: Option<&str>,
) -> bool {
    let Some((fx, fe)) = friend else {
        return false;
    };
    matches!((fx, x25519), (Some(o), Some(n)) if o != n)
        || matches!((fe, ed25519), (Some(o), Some(n)) if o != n)
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_peer(
    state: &AppState,
    device_id: &str,
    nickname: &str,
    avatar: Option<String>,
    device_type: &str,
    ip: &str,
    tcp_port: u16,
    x25519: Option<String>,
    ed25519: Option<String>,
    rtt_ms: Option<u64>,
) {
    let ts = db::now_ms();
    // friends 锚冲突预判（2026-09-19 审计 P0#3）：重启后 peers 是空的，若「新建条目」
    // 不看持久化的好友公钥，一条伪造 Presence/FriendRequest 就能抢先把好友 id 绑上
    // 攻击者的键 —— 之后攻击者的 Chat 信封通过「已认识」校验，假消息直接冒充好友。
    // 判据与 Some 分支的 key_conflict 完全同口径，走同一个告警出口。
    // ⚠️ 只对「peers 里还没有的条目」查锚：announce 是 5s×N 节点的热路径，
    // 每条都打一次 DB 读会在 500 节点下放大成明显热点（自查发现，评审补记）。
    // 预检与真正的插入之间的竞态无害：条目恰好挤进来时走 Some 分支，同样有冲突判定。
    let needs_anchor = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        !peers.contains_key(device_id)
    };
    let friend_anchor = if needs_anchor && (x25519.is_some() || ed25519.is_some()) {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend_pubkeys(&dbc, device_id)
    } else {
        None
    };
    // 判断是否「新节点」或「公钥首次学到/变化」，据此决定是否做昂贵的落库与群密钥补发。
    // 500-1000 节点下，若每条 announce 都写库 + 遍历群组，会形成明显热点。
    let (is_new, key_changed, key_conflict) = {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        match peers.get_mut(device_id) {
            None if friend_anchor.as_ref().is_some_and(|(fx, fe)| {
                new_peer_conflicts_with_friend(
                    Some((fx.as_deref(), fe.as_deref())),
                    x25519.as_deref(),
                    ed25519.as_deref(),
                )
            }) =>
            {
                // 不建立条目：这个 device_id 的公钥由 friends 表说了算。
                // 走到下面与 key_conflict 同一出口（诊断事件 + 系统消息告警）。
                (false, false, true)
            }
            None => {
                peers.insert(
                    device_id.to_string(),
                    Peer {
                        device_id: device_id.to_string(),
                        nickname: nickname.to_string(),
                        avatar,
                        device_type: device_type.to_string(),
                        ip: ip.to_string(),
                        tcp_port,
                        last_seen: ts,
                        rtt_ms,
                        x25519_pubkey: x25519.clone(),
                        ed25519_pubkey: ed25519.clone(),
                        // 本函数由 announce（未签名 UDP）与 Hello 后的同步共同调用。
                        // 这里一律先标未验证；只有验签通过的路径可以把它改成 true
                        // （见 `mark_peer_keys_verified`）。宁可保守：未验证的公钥
                        // 只配用于发现，不配用于身份绑定。
                        keys_verified: false,
                        first_seen: Some(ts),
                        // 事件推送里的 peer 不带链路类型（同步上下文拿不到 links 锁）；
                        // 界面读的是命令返回的那份（那里会填），见 `Peer::link` 注释。
                        link: None,
                    },
                );
                (true, true, false)
            }
            Some(p) => {
                let x_conflict =
                    matches!((&p.x25519_pubkey, &x25519), (Some(old), Some(new)) if old != new);
                let e_conflict =
                    matches!((&p.ed25519_pubkey, &ed25519), (Some(old), Some(new)) if old != new);
                let key_conflict = x_conflict || e_conflict;
                let key_changed = !key_conflict
                    && ((x25519.is_some() && p.x25519_pubkey != x25519)
                        || (ed25519.is_some() && p.ed25519_pubkey != ed25519));
                p.nickname = nickname.to_string();
                if avatar.is_some() {
                    p.avatar = avatar;
                }
                if !device_type.is_empty() {
                    p.device_type = device_type.to_string();
                }
                if !ip.is_empty() {
                    p.ip = ip.to_string();
                }
                if tcp_port != 0 {
                    p.tcp_port = tcp_port;
                }
                if x25519.is_some() && !x_conflict {
                    p.x25519_pubkey = x25519;
                }
                if ed25519.is_some() && !e_conflict {
                    p.ed25519_pubkey = ed25519;
                }
                if rtt_ms.is_some() {
                    p.rtt_ms = rtt_ms;
                }
                p.last_seen = ts;
                (false, key_changed, key_conflict)
            }
        }
    };
    state.emit_peers();

    if key_conflict {
        state.push_diag_event("identity_key_conflict", &format!("device_id={device_id}"));
        warn_key_conflict_once(state, device_id);
        return;
    }

    // 仅在公钥首次学到/变化时才落库（避免每条 announce 都写库）。
    //
    // ⚠️ **必须同时要求 `keys_verified`**：本函数同时服务两条来源完全不同的路径 ——
    // 未签名的 UDP announce（任何人可伪造 device_id + 公钥）与验签通过的 Hello。
    // 若不加这道闸，局域网内一个伪造 announce 就能把攻击者的公钥写进持久化的 friends 表，
    // 覆盖好友的真实公钥：此后我发给该好友的消息都用攻击者公钥加密，而消息是广播给
    // 所有已连接节点的 ⇒ 攻击者用自己的私钥即可解开（E2EE 被击穿，且重启不恢复）。
    let verified = peer_keys_trusted(state, device_id);
    if key_changed && verified {
        let (x, e) = {
            let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
            peers
                .get(device_id)
                .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                .unwrap_or((None, None))
        };
        if x.is_some() || e.is_some() {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::update_friend_pubkeys(&dbc, device_id, x.as_deref(), e.as_deref()).ok();
        }
        // 公钥变化时同步到 friends 表：Hello 可能在 announce 之前到达，
        // 此时 peers[peer].x25519_pubkey 为 None → friends 表写入 None；
        // announce 到达后更新了 peers，但 friends 表不会自动刷新。
        // 此处补一次 maybe_update_friend 确保 friends 表与 peers 同步。
        let (nick, av) = {
            let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
            peers
                .get(device_id)
                .map(|p| (p.nickname.clone(), p.avatar.clone()))
                .unwrap_or_default()
        };
        maybe_update_friend(state, device_id, &nick, av);
    }

    // 仅新节点或公钥变化时补发群密钥（处理对方离线时建群的情况）
    if is_new || key_changed {
        redistribute_group_keys(state, device_id).await;
        // 重发本机聊天样式：对方离线期间错过 broadcastChatStyle 广播，
        // 且样式广播无离线补偿——对方上线后必须补发，否则永远看不到配色
        resend_chat_style(state, device_id).await;
    }
}

/// 向指定 peer 重发本机聊天样式（复用既有 ChatStyle 消息，无新协议）。
async fn resend_chat_style(state: &AppState, peer_id: &str) {
    let style = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, "chat_style")
    };
    let Some(style) = style else {
        return;
    };
    if style.is_empty() {
        return;
    }
    let msg = Message::ChatStyle {
        from: state.device_id.clone(),
        to: Some(peer_id.to_string()),
        style,
    };
    let _ = try_send(state, peer_id, &msg).await;
}
