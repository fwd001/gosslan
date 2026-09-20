// ---------------- Gossip 处理 ----------------

// 本文件由 transport.rs 用 include! 展开进同一模块（见该文件末尾的分册登记）。
/// 群信封能否被**本机消费**（= 是否允许进入第 5 步的本地处理）。
///
/// ⚠️ 这条判据**只管"消费"，不管"转发"**（2026-09-13 审计的真缺陷）。
///
/// 旧实现在这里直接 `return`，于是**非成员中继根本不会转发群消息**：
/// 三个 BLE-only 设备串成 A—B—C 时，只要 B 不在群里，A 发的群消息到 B 就没了
/// —— 而同一条链路上单聊是通的（单聊走定向 target 分支）。
/// 表现就是"BLE mesh 上群聊永远不通、私聊却正常"。
///
/// 语义边界（为什么"非成员转发"是安全的）：
/// - 群消息正文用**群密钥**对称加密，非成员没有密钥 ⇒ 解不开（`plaintext = None`），
///   转发它只是搬密文，不泄露任何内容；
/// - 是否愿意替别人转发，由**中继授权（M4）**决定（`decide_forward`），不靠这条判据；
/// - `sender` 必须在成员表里：签名只证明"是谁发的"，不证明"发送者有权把人拉进群"，
///   所以伪造者发的群信封即使广播过来，本机也不消费它。
pub(crate) fn group_envelope_consumable(
    kind: &GossipKind,
    members: &[String],
    me: &str,
    sender: &str,
) -> bool {
    if !matches!(kind, GossipKind::Group) {
        return true;
    }
    // 成员表为空 = 旧端 / 早期实现发的群信封，保持兼容（与旧代码同口径）
    if members.is_empty() {
        return true;
    }
    members.iter().any(|m| m == me) && members.iter().any(|m| m == sender)
}

/// peers 里查不到的发送方，能否按 kind 建立信任。
///
/// 唯一判定点（`handle_gossip` 的 sender_trusted 与回归测试共用）：
/// - `friend_ed25519 = Some`（friends 表在册且绑过键）⇒ **只认绑定值**，任何 kind
///   都不给 TOFU —— friends 是持久化身份锚，「重启后 peers 为空」不是放行冒充的理由；
/// - 不在册、或旧行键列为 NULL ⇒ 只放行 Presence（节点可见性）与加好友流程的
///   FriendRequest/FriendAccept（它们本来就来自陌生节点），其余（Chat/Group/回执）
///   仍然拒绝。NULL 键好友的宽容行为与修复前一致，避免误伤旧数据。
fn gossip_trust_for_unpeer_sender(
    kind: &GossipKind,
    friend_ed25519: Option<&str>,
    env_ed25519: &str,
) -> bool {
    if let Some(stored) = friend_ed25519 {
        return stored == env_ed25519;
    }
    matches!(
        kind,
        GossipKind::Presence | GossipKind::FriendRequest | GossipKind::FriendAccept
    )
}

async fn handle_gossip(state: &Arc<AppState>, peer_id: &str, env: GossipEnvelope) {
    // Gossip 可经第三方转发，不能仅凭信封内自报的 Ed25519 公钥建立身份。
    // 公钥必须先由 Discovery/Hello 绑定到同一个 device_id；若已知 X25519
    // 公钥也发生变化，同样拒绝，避免冒充好友或污染 E2EE 密钥缓存。
    let sender_trusted = {
        if env.sender_id == state.device_id {
            false
        } else {
            let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
            match peers.get(&env.sender_id) {
                // 已认识：公钥必须匹配（无论是直连还是跨跳转发），防冒充。
                Some(p) => {
                    let direct_peer = peer_id == env.sender_id;
                    (p.ed25519_pubkey.as_deref() == Some(env.sender_ed25519.as_str())
                        || (direct_peer && p.ed25519_pubkey.is_none()))
                        && (p
                            .x25519_pubkey
                            .as_deref()
                            .map_or(true, |key| key == env.sender_pubkey)
                            || (direct_peer && p.x25519_pubkey.is_none()))
                }
                // 未认识（peers 里查不到）。⚠️ peers 是**内存态**，进程重启后为空、
                // 好友恰好不在线时也会缺席 —— 「不在 peers」不等于「陌生节点」。
                // friends 表才是持久化的身份锚：已在册的 id 一律按绑定的 ed25519 判，
                // 不给 Presence/FriendRequest/FriendAccept 留 TOFU 后门
                //（2026-09-19 审计 P0#3：此前攻击者可用自签信封冒充重启后缺席的好友）。
                None => {
                    let stored = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::get_friend_ed25519(&dbc, &env.sender_id)
                    };
                    gossip_trust_for_unpeer_sender(
                        &env.kind,
                        stored.as_deref(),
                        &env.sender_ed25519,
                    )
                }
            }
        }
    };
    if !sender_trusted {
        return;
    }
    // 直连 TCP 对端在 Hello 中没有携带公钥时，首次合法 Gossip 可完成 TOFU
    // 绑定；之后所有经中继或直连的信封都必须匹配这组键。
    if peer_id == env.sender_id {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peers.get_mut(&env.sender_id) {
            if p.ed25519_pubkey.is_none() {
                p.ed25519_pubkey = Some(env.sender_ed25519.clone());
            }
            if p.x25519_pubkey.is_none() {
                p.x25519_pubkey = Some(env.sender_pubkey.clone());
            }
        }
    }
    // 1. 先验签，再进入去重缓存。否则攻击者可以用伪造的唯一 message_id
    // 污染 Bloom/LRU，甚至抢先占用真实消息的 id 造成合法消息被丢弃。
    {
        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
        if !gossip.verify_envelope(&env) {
            return;
        }
        drop(gossip);
        // Mesh 层：**全局去重 + TTL 判定**（§15 / §16 / §17）。
        //
        // 必须在**验签之后**才登记去重表，否则攻击者可用伪造的 frame_id 污染
        // Bloom，抢先占用真实帧的 id 造成合法消息被丢弃 —— 与上面 GossipEngine
        // 的防护同理。
        //
        // ⚠️ 这里**只判定不转发**：真正的多跳转发在下面第 4 步（`decide_forward` +
        // `choose_fanout`）。历史上这里还有一条由 `MeshRouter::relay_enabled` 门控的
        // 转发路径（默认关闭、生产从不调用），2026-09-12 已删除 ——
        // **中继授权的唯一真相是 `settings.relay_policy`**（ADR-0016），
        // 接 BLE 时不要再复制第二份转发记账。
        {
            let mut router = state.mesh_router.lock().unwrap_or_else(|e| e.into_inner());
            let frame = MeshFrame {
                frame_id: env.message_id.clone(),
                source_node_id: env.sender_id.clone(),
                destination: MeshDestination::Broadcast,
                ttl: env.ttl,
                kind: MeshFrameKind::Gosslan,
                // 转发在别处做，这里只需要元数据（MeshRouter 不解析载荷，P-A03）
                payload: Vec::new(),
            };
            if let ForwardDecision::Drop(_) = router.on_receive(frame, &state.device_id) {
                return;
            }
        }
        // 业务层去重（Mesh 之后的第二道防线）
        let mut gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
        if !gossip.is_new(&env.message_id) {
            return;
        }
    }
    // 2. 同步发送方公钥：GossipEnvelope 已携带 x25519_pubkey 用于解密，
    //    但此前未写入 peers/friends，导致后续 outbox 重发的直发 ChatMessage
    //    在 open_direct_content() 中因缺 pubkey 被丢弃。此处仅更新已有 peer
    //    条目（不做 insert，避免为未通过 Discovery 的节点创建残缺记录），
    //    同时用 COALESCE 安全地补充 friends 表的 NULL pubkey。
    if env.encrypted && matches!(env.kind, GossipKind::Chat) {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peers.get_mut(&env.sender_id) {
            if p.x25519_pubkey.is_none() {
                p.x25519_pubkey = Some(env.sender_pubkey.clone());
                p.last_seen = db::now_ms();
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::update_friend_pubkeys(&dbc, &env.sender_id, Some(&env.sender_pubkey), None)
                    .ok();
            }
        }
    }
    // 3. 解包：E2EE 关闭时载荷为明文 JSON；开启时按单聊 ECDH / 群密钥解密
    let plaintext = if !env.encrypted {
        // FriendMessageBlocked 等明文 Gossip：payload 是 base64 编码的 JSON
        STANDARD.decode(&env.payload).ok()
    } else {
        match &env.kind {
            GossipKind::Chat => {
                let shared =
                    crypto::shared_secret(&state.identity.x25519_secret, &env.sender_pubkey);
                shared.and_then(|s| {
                    STANDARD
                        .decode(&env.payload)
                        .ok()
                        .and_then(|d| crypto::open(&s, &d))
                })
            }
            GossipKind::Group => {
                let gid = env.group_id.clone().unwrap_or_default();
                let key = get_group_key(state, &gid).await;
                key.and_then(|k| {
                    STANDARD
                        .decode(&env.payload)
                        .ok()
                        .and_then(|d| crypto::open_symmetric(&k, &d))
                })
            }
            GossipKind::FriendMessageBlocked => {
                // 已在 encrypted=false 分支处理，这里不应进入
                return;
            }
            GossipKind::Presence => {
                // Presence 必然是明文；收到 encrypted=true 的是异常，丢弃。
                return;
            }
            GossipKind::FriendRequest | GossipKind::FriendAccept => {
                // 定向好友控制消息：发送方用 target 的 X25519 公钥加密，只有 target
                // 能用自己的私钥解开；中间节点解不开（plaintext=None），无害。
                let shared =
                    crypto::shared_secret(&state.identity.x25519_secret, &env.sender_pubkey);
                shared.and_then(|s| {
                    STANDARD
                        .decode(&env.payload)
                        .ok()
                        .and_then(|d| crypto::open(&s, &d))
                })
            }
            GossipKind::ChatAck | GossipKind::ChatReadReceipt => {
                // 回执/确认必然是明文；收到 encrypted=true 的是异常，丢弃。
                return;
            }
        }
    };

    // 群信封即使签名正确，也只能被群成员**消费**；签名证明“是谁发的”，
    // 不代表发送者有权把任意节点加入一个群。
    //
    // ⚠️ 这里**只记判据、不 return**（2026-09-13 审计）：非成员也要继续走第 4 步的转发，
    // 否则 BLE-only 三点中继里的群聊永远不通（细节见 `group_envelope_consumable` 的注释）。
    let group_consumable = group_envelope_consumable(
        &env.kind,
        &env.group_members,
        &state.device_id,
        &env.sender_id,
    );

    // 4. 转发（fan-out，TTL 衰减）— 先过**中继授权**（P2 / M4），再选人转发。
    //
    // 三条判据全部收在 `mesh::relay_policy::decide_forward` 里（真值表有单测），
    // 这里只负责喂事实，避免把传播语义散落成 if：
    //   ① 定向帧到达目标后**停止转发**（本机就是 target，只消费）：否则目标会把定向帧
    //      再洪泛给其他邻居，邻居又按 target 定向转发回来，形成冗余中转与回环。真机反馈
    //      「同网段好友申请一直中转、清掉还冒出来」正是这个回环造成的；
    //   ② TTL 耗尽不再转发；
    //   ③ 替**别人**转发要过授权策略（自己发的信封不受策略限制）。
    //
    // ⚠️ 默认策略是 `all`（与今天逐字节一致）；好友/白名单查询是**按需**的 ——
    // `all`/`off` 下一次库都不查，转发热路径零额外开销。
    let is_target = env.target.as_deref() == Some(state.device_id.as_str());
    let sender_is_me = env.sender_id == state.device_id;
    let relay_cfg = state.relay_policy_config();
    let may_forward = crate::mesh::relay_policy::decide_forward(
        &relay_cfg,
        is_target,
        env.ttl,
        sender_is_me,
        &env.sender_id,
        || {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_friend(&dbc, &env.sender_id).is_some()
        },
    );
    if may_forward {
        // 定向帧（FriendRequest/FriendAccept/ChatAck/ChatReadReceipt）优先精确定向：
        // target 是本机直连就只发它；无直连路径时洪泛兜底（第一版无路由表）。广播帧保持 fan-out。
        let targets: Vec<String> = match env.target.as_deref() {
            Some(t) => {
                // ⚠️ 必须判「有**非空**链路」（等价于 `has_link`），不能用 `contains_key`：
                // 后者会把残留的空 Vec 当成「直连」→ 走只发 target 的分支 →
                // `try_send` 返回「未建立连接」，而**洪泛兜底不会执行** ⇒
                // 跨跳的好友申请 / 送达回执 / 已读回执可能永久丢失（复核抓到的缺陷）。
                let direct = {
                    let links = state.links.lock().await;
                    links.get(t).is_some_and(|v| !v.is_empty())
                };
                if direct {
                    vec![t.to_string()]
                } else {
                    let neighbors = reachable_neighbors(state, &env.sender_id).await;
                    let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                    gossip.choose_fanout(&neighbors, &env.sender_id)
                }
            }
            None => {
                let neighbors = reachable_neighbors(state, &env.sender_id).await;
                let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                gossip.choose_fanout(&neighbors, &env.sender_id)
            }
        };
        let mut fwd = env.clone();
        fwd.ttl -= 1;
        let fwd_msg = Message::Gossip { envelope: fwd };
        // ⚠️ 转发**不阻塞本连接的读循环**：`try_send` 在信道满时有界补试 500ms，
        // 逐条 await 最坏 4 × 500ms = 2s —— 期间这条连接的后续帧（含心跳）都要排队，
        // shutdown/取消也要等。顺序在这里无关紧要（接收侧按 msg_id 去重，而且这些
        // 只是同一条消息发给**不同**邻居）。用一个任务串行发完：并发度不变、任务数可控。
        let st = state.clone();
        tokio::spawn(async move {
            for t in targets {
                let _ = try_send(&st, &t, &fwd_msg).await;
            }
        });
    }

    // 5. 按 GossipKind 处理
    //
    // 非成员**跳过本地消费**（上面的转发已经做完了）：群密钥不在手上，本来也解不开，
    // 但绝不能因为"我不是这个群的成员"就把整条消息丢掉 —— 那样多跳群聊永远不通。
    if !group_consumable {
        return;
    }
    match env.kind {
        GossipKind::FriendMessageBlocked => {
            // 控制消息：检查本机是否为原始发送方
            if let Some(pt) = plaintext {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                    if let Some(original) = v.get("original_sender").and_then(|s| s.as_str()) {
                        if original == state.device_id {
                            // 本机就是原始发送方 → 通知前端
                            let _ = state.app.emit("friend-message-blocked", &env.sender_id);
                        }
                        // 非本机 → 已在上面 fan-out 转发，不做任何 UI/DB 操作
                    }
                }
            }
        }
        GossipKind::Presence => {
            // TOFU 记录远端节点：跨跳转发的 Presence，sender 可能不在 peers 里。
            // 「去中心化发现」的落地 —— A 经 B 转发看到 C，C 进入 peers 表，
            // 前端「添加好友」列表即出现 C（即使 A 与 C 无直连）。
            if let Some(pt) = plaintext {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                    let nickname = v
                        .get("nickname")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let avatar = v
                        .get("avatar")
                        .and_then(|a| a.as_str())
                        .map(|s| s.to_string());
                    let device_type = v
                        .get("device_type")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    // 首次学到才留痕：peers 是内存结构、不落库，这行日志是唯一可观测
                    // 「跨跳发现了谁」的手段（与 [mesh] ±conn、握手学到身份同理）。
                    let is_new = {
                        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
                        !peers.contains_key(&env.sender_id)
                    };
                    if is_new {
                        state.logger.info(
                            "presence",
                            format!("学到远端节点 peer={} nickname={}", env.sender_id, nickname),
                        );
                        // 🎯 同局域网却走桥接的直接修复（真机 2026-09-14 全 Windows 局域网）：
                        // 新学到的**跨跳**节点只有 ip="" 的 Presence，永远不会触发 LAN 拨号
                        // （ensure_link 只由 UDP announce 驱动）⇒ 同网段也只能一直走中继。
                        // 主动喊一轮 who_has：同网段的节点会用**单播**把 announce 回给我们，
                        // 我们随即建立直连；不在同网段的节点收不到单播、不受影响。
                        if let Some(tx) = state
                            .probe
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_ref()
                        {
                            let next = tx.borrow().saturating_add(1);
                            let _ = tx.send(next);
                        }
                    }
                    // ip 空、tcp_port 0：跨跳转发不知道对端真实地址，仅记录身份
                    // （可被「看到」，但不可直连）。
                    upsert_peer(
                        state,
                        &env.sender_id,
                        &nickname,
                        avatar,
                        &device_type,
                        "",
                        0,
                        Some(env.sender_pubkey.clone()),
                        Some(env.sender_ed25519.clone()),
                        None,
                    )
                    .await;
                }
            }
        }
        GossipKind::FriendRequest => {
            // 定向好友申请：只有 target == 本机才处理（中间节点已转发，不消费）。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                if let Some(pt) = plaintext {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                        let from_nickname = v
                            .get("from_nickname")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let from_avatar = v
                            .get("from_avatar")
                            .and_then(|a| a.as_str())
                            .map(|s| s.to_string());
                        // 同步记录申请方身份与公钥：这样「同意」时才有对方 X25519 公钥
                        // 可加密回发的 FriendAccept（跨跳场景下 Presense 可能还没到）。
                        upsert_peer(
                            state,
                            &env.sender_id,
                            &from_nickname,
                            from_avatar.clone(),
                            "",
                            "",
                            0,
                            Some(env.sender_pubkey.clone()),
                            Some(env.sender_ed25519.clone()),
                            None,
                        )
                        .await;
                        // 他已经是我的好友 ⇒ 直接同意（公钥刚从信封里记下，回执发得出去）
                        if auto_accept_if_already_friend(state, &env.sender_id).await {
                            return;
                        }
                        let req = PendingRequest {
                            from: env.sender_id.clone(),
                            from_nickname,
                            from_avatar,
                            ts: env.ts,
                        };
                        state
                            .pending_requests
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(env.sender_id.clone(), req.clone());
                        let _ = state.app.emit("friend-request", &req);
                        // 留痕：跨跳好友申请是内存态（pending_requests），日志是唯一
                        // 可观测「谁申请了我」的手段（headless 测试与真机排障都靠它）。
                        state.logger.info(
                            "friend",
                            format!(
                                "收到跨跳好友申请 peer={} nickname={}",
                                env.sender_id, req.from_nickname
                            ),
                        );
                        let mut extra = std::collections::HashMap::new();
                        extra.insert("type".to_string(), "friend_request".to_string());
                        // 与直连那条路径同口径：点击唤起窗口 + 跳到「新的朋友」。
                        let click_app = state.app.clone();
                        let _ = crate::notifications::show_click_if_enabled(
                            state,
                            "好友申请",
                            &format!("{} 请求添加你为好友", req.from_nickname),
                            extra,
                            move || {
                                crate::notifications::on_notification_clicked(
                                    &click_app,
                                    "friend_request",
                                    None,
                                )
                            },
                        );
                    }
                }
            }
        }
        GossipKind::FriendAccept => {
            // 定向好友同意：只有 target == 本机才处理（即「我发的申请被对方同意」）。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                let from = env.sender_id.clone();
                let name = resolve_nickname(state, &from);
                // 幂等判据：**这一次是否真的"从不是好友变成好友"**。
                //
                // FriendAccept 没有 ACK 机制，发送方会持续补发（见 `补发好友同意回执`）——
                // 而本分支原先没有任何去重：`add_friend` 是幂等的，但**通知与留痕每次都会执行**
                // ⇒ 用户被"好友申请已通过"反复刷屏（真机日志里同一秒内三次）。
                // 同时它也是"单方面成功"的观感来源：一方在无限重发，另一方被反复打扰。
                let was_friend = {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::get_friend(&dbc, &from).is_some()
                };
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::add_friend(&dbc, &from, &name, None).ok();
                    // 同步公钥（否则首次加密发送会失败）—— 与 Message::FriendAccept 路径一致。
                    let (x, e) = {
                        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
                        peers
                            .get(&from)
                            .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                            .unwrap_or((None, None))
                    };
                    if x.is_some() || e.is_some() {
                        db::update_friend_pubkeys(&dbc, &from, x.as_deref(), e.as_deref()).ok();
                    }
                }
                forget_pending_request(state, &from);
                // emit 每次都发：前端 store 只是据此重拉好友列表（幂等），
                // 而漏发会让「首次那个 emit 恰好没被界面收到」时界面永远不刷新。
                let _ = state.app.emit("friend-accepted", &from);
                if was_friend {
                    // 重复投递：只留一行便于排查的痕迹，**不通知**。
                    state
                        .logger
                        .info("friend", format!("重复的好友同意（已忽略）peer={from}"));
                } else {
                    state
                        .logger
                        .info("friend", format!("收到跨跳好友同意 peer={from}"));
                    let _ = crate::notifications::show_if_enabled(
                        state,
                        "好友申请已通过",
                        &format!("{name} 已成为你的好友"),
                    );
                }
            }
        }
        GossipKind::ChatAck => {
            // 定向送达确认：只有 target == 本机才处理（即「我发的消息被对方收到」）。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                if let Some(pt) = plaintext {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                        if let Some(msg_id) = v.get("msg_id").and_then(|m| m.as_str()) {
                            // 只有「我发给 sender、且仍在 outbox」的 msg_id 才接受：
                            // msg_id 随机不可预测 + 必须命中 outbox 目标，双重防伪造送达。
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            let is_expected = dbc
                                .query_row(
                                    "SELECT 1 FROM outbox WHERE msg_id = ?1 AND peer_id = ?2",
                                    params![msg_id, env.sender_id],
                                    |_| Ok(()),
                                )
                                .is_ok();
                            if !is_expected {
                                return;
                            }
                            db::set_message_status(&dbc, msg_id, "delivered").ok();
                            dbc.execute("DELETE FROM outbox WHERE msg_id = ?1", params![msg_id])
                                .ok();
                            drop(dbc);
                            let _ = state.app.emit("message-acked", msg_id);
                        }
                    }
                }
            }
        }
        GossipKind::ChatReadReceipt => {
            // 定向已读回执：只有 target == 本机才处理。
            if env.target.as_deref() == Some(state.device_id.as_str()) {
                if let Some(pt) = plaintext {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&pt) {
                        let from = env.sender_id.clone();
                        let last_read_ts =
                            v.get("last_read_ts").and_then(|t| t.as_i64()).unwrap_or(0);
                        let last_read_msg_id = v
                            .get("last_read_msg_id")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string());
                        // 与 Message::ReadReceipt 分支同构：用 msg_id 换算回本机时间戳。
                        let effective_ts = {
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            last_read_msg_id
                                .as_deref()
                                .and_then(|msg_id| {
                                    dbc.query_row(
                                        "SELECT ts FROM messages WHERE msg_id = ?1 AND sender_id = ?2 AND conv_id = ?3",
                                        params![msg_id, state.device_id, from],
                                        |r| r.get::<_, i64>(0),
                                    )
                                    .ok()
                                })
                                .unwrap_or(last_read_ts)
                        };
                        {
                            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = dbc.execute(
                                "UPDATE messages SET status = 'read'
                                 WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                                params![from, state.device_id, effective_ts],
                            );
                        }
                        let _ = state.app.emit(
                            "peer-read",
                            &serde_json::json!({ "peer_id": from, "last_read_ts": effective_ts }),
                        );
                    }
                }
            }
        }
        GossipKind::Chat | GossipKind::Group => {
            // 单聊定向：target 存在且不是本机 → 中间节点只转发不消费。即便不判断，
            // 中间节点也会因 ECDH 解不开而 plaintext=None（不会落库），但明确判断
            // 语义更清晰、也避免无谓的好友关系检查。群聊无 target，走原广播消费逻辑。
            if env.kind == GossipKind::Chat
                && env
                    .target
                    .as_deref()
                    .is_some_and(|t| t != state.device_id.as_str())
            {
                return;
            }
            if let Some(pt) = plaintext {
                let (kind, content) = parse_gossip_payload(&pt);
                // GossipKind::Chat：好友关系检查（非好友不落库、不通知、通知发送方）
                if env.kind == GossipKind::Chat {
                    let is_friend = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::get_friend(&dbc, &env.sender_id).is_some()
                    };
                    if !is_friend {
                        // 通过 Gossip 广播拒绝通知（多跳场景下也能回到原始发送方）
                        let mut blocked_env = {
                            let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                            let payload =
                                serde_json::json!({ "original_sender": env.sender_id }).to_string();
                            let payload_b64 = STANDARD.encode(payload.as_bytes());
                            gossip.build_envelope(
                                &state.identity,
                                &state.device_id,
                                GossipKind::FriendMessageBlocked,
                                None,
                                None,
                                &payload_b64,
                                db::now_ms(),
                                0,
                            )
                        };
                        // 这是拒绝通知控制载荷，不是用户聊天内容；显式标记为明文，
                        // 同时不再为用户 Chat/Group 提供明文兼容路径。
                        blocked_env.encrypted = false;
                        blocked_env.sender_sig =
                            state.identity.sign_b64(&blocked_env.signing_bytes());
                        broadcast_gossip(state, blocked_env).await;
                        return;
                    }
                }
                let conv_id = match &env.kind {
                    GossipKind::Chat => env.sender_id.clone(),
                    GossipKind::Group => {
                        format!("group:{}", env.group_id.clone().unwrap_or_default())
                    }
                    _ => return,
                };
                let conv_kind = match &env.kind {
                    GossipKind::Chat => "single",
                    GossipKind::Group => "group",
                    _ => return,
                };
                let name = match &env.kind {
                    GossipKind::Chat => resolve_nickname(state, &env.sender_id),
                    GossipKind::Group => {
                        resolve_group_name(state, env.group_id.as_deref().unwrap_or(""))
                    }
                    _ => return,
                };
                // 群聊删除边界：本机清除过该群（seq ≤ boundary）的旧历史不得回灌。
                // 逻辑序号不依赖墙上时钟，也不猜测发送方时钟。
                if env.kind == GossipKind::Group {
                    let gid = env.group_id.clone().unwrap_or_default();
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    let blocked = db::group_message_blocked_by_boundary(&dbc, &gid, env.seq, &kind);
                    drop(dbc);
                    if blocked {
                        return;
                    }
                }
                // 群消息顺带建群：成员端可能从未收到 GroupKey（本地无 groups 行），
                // 但这条群消息携带了完整成员表 → 据此 upsert 建群，成员面板才能显示。
                // 已有则只刷新（成员随踢人/加人变化时也能及时同步）。
                if env.kind == GossipKind::Group {
                    if let (Some(gid), Some(creator), true) = (
                        env.group_id.clone(),
                        env.group_creator.clone(),
                        !env.group_members.is_empty(),
                    ) {
                        // ⚠️ **信封里的群名只有群主本人能生效**。
                        //
                        // `group_name` / `group_creator` 都是发送方自报的字段，任何持群密钥的
                        // 成员都能填任意文本。但「改名」已经有专用帧 `GroupRename` 且严格要求
                        // 群主 —— 若这里无条件采信，同一个效果就有了两条路径、一条有检查一条
                        // 没有，成员即可绕过授权改掉所有人的群名（伪造成"系统通知"做社工）。
                        //
                        // 两层判断缺一不可：
                        //   ① 发送者必须**自称**群主（否则填别人的 id 就能对上 creator）；
                        //   ② 该自称还要与本地已存的 creator 一致（由 `db::upsert_group` 兜底），
                        //      挡住「自称是群主、但本地记录里群主另有其人」。
                        // 本地无该群时（首次接触，无从校验）按 TOFU 采信信封，与建链口径一致。
                        let mut all = env.group_members.clone();
                        if !all.contains(&state.device_id) {
                            all.push(state.device_id.clone());
                        }
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        let known = db::get_group(&dbc, &gid);
                        let is_self_declared_creator = env.sender_id == creator;
                        let display_name = match &known {
                            // 已有该群：只有群主自称时才更新名字，否则沿用本地名字
                            Some(g) => {
                                if is_self_declared_creator {
                                    env.group_name.clone().unwrap_or_else(|| g.name.clone())
                                } else {
                                    g.name.clone()
                                }
                            }
                            None => env.group_name.clone().unwrap_or_else(|| name.clone()),
                        };
                        // 群名长度与建群/改名一致封顶，避免这条路径塞进超长字符串
                        let display_name: String =
                            display_name.chars().take(MAX_GROUP_NAME_LEN).collect();
                        db::upsert_group(&dbc, &gid, &display_name, &creator, &all).ok();
                        drop(dbc);
                        let _ = state.app.emit("groups-updated", &gid);
                    }
                }
                let preview = preview_content(&kind, &content);
                // 撤回事件：把「已撤回」物化到被撤回的那条消息上（幂等）。
                // 只认**作者本人**的撤回 —— 信封被 Ed25519 签名，sender_id 不可伪造；
                // 接收端不校验时间窗（无法验证发送方的墙上时钟，那是产品规则不是安全边界）。
                let group_id_for_check = conv_id
                    .strip_prefix("group:")
                    .unwrap_or_default()
                    .to_string();
                if kind == crate::protocol::KIND_RECALL {
                    if let Ok(p) = serde_json::from_str::<crate::protocol::RecallPayload>(&content)
                    {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        // ⚠️ **目标消息可能还没落库**（撤回事件先到）。此时不能因为
                        // "查不到作者"就把整条撤回丢掉 —— 那恰好把权威集合存在的意义
                        // （解决先撤后到）封死了：随后消息带着完整正文落库，撤回永久失效。
                        let target_exists =
                            db::get_message_preview_source(&dbc, &p.target).is_some();
                        let allowed = if target_exists {
                            // 目标已落库：只认作者本人撤回
                            db::get_message_preview_source(&dbc, &p.target)
                                .map(|(sid, _)| sid == env.sender_id)
                                .unwrap_or(false)
                        } else if conv_id.starts_with("group:") {
                            // 群消息 + 目标未落库：以「发送者是本群成员」为准 ——
                            // 他能解开群消息就说明持有群密钥、是成员
                            db::get_group(&dbc, &group_id_for_check)
                                .map(|g| g.members.contains(&env.sender_id))
                                .unwrap_or(false)
                        } else {
                            // 单聊 + 目标未落库：允许 ——
                            // env 被 Ed25519 签名，sender_id 不可伪造；
                            // 单聊撤回是定向发给目标个人的，不存在群成员那类授权问题
                            true
                        };
                        if allowed {
                            db::insert_recall(&dbc, &conv_id, &p.target, &env.sender_id, env.seq)
                                .ok();
                            db::materialize_recall(&dbc, &p.target).ok();
                            drop(dbc);
                            let _ = state.app.emit("message-recalled", &p.target);
                        }
                    }
                }
                // 群公告：**仅群主可发布/删除**。发送侧已校验（send_group_announcement），
                // 但接收侧原先没有任何检查 —— 任何持群密钥的成员构造一条
                // kind="announcement" 的群消息就能改掉所有人的公告横幅，
                // 与「公告是发给全群的权威信息」相悖，也与群名那处（同一批修的）口径不一致。
                if kind == crate::protocol::KIND_ANNOUNCEMENT
                    || kind == crate::protocol::KIND_ANNOUNCEMENT_DELETE
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    let is_creator = db::get_group(&dbc, &group_id_for_check)
                        .map(|g| g.creator == env.sender_id)
                        .unwrap_or(false);
                    if !is_creator {
                        state.logger.warn(
                            "group",
                            format!(
                                "丢弃非群主发布的公告：sender={} group={group_id_for_check}",
                                env.sender_id
                            ),
                        );
                        return;
                    }
                }
                // 持锁块只做落库；await（fanout 转发已在前面）之后无持锁操作
                // 业务幂等裁决：Direct（含 outbox 补发）可能已经把同一 msg_id 落库，此时
                // 不得再计未读、再发 message-received，否则未读数与系统通知都会重复。
                // 与 Direct 分支共用 announced_on 裁决，两路径并发时只有一方拿到 Ok(true)。
                // 单聊与群聊走同一块 ⇒ 两种 GossipKind 都被覆盖。
                let (out_rec, inserted) = {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    // 展示时间用本地接收时间；排序用信封携带的逻辑序号 seq。
                    let ts = db::now_ms();
                    let seq = env.seq.max(1);
                    let rec = MessageRecord {
                        id: 0,
                        msg_id: env.message_id.clone(),
                        conv_id: conv_id.clone(),
                        sender_id: env.sender_id.clone(),
                        receiver_id: state.device_id.clone(),
                        kind: kind.clone(),
                        content: content.clone(),
                        ts,
                        seq,
                        status: "delivered".to_string(),
                    };
                    // **先撤后到**：撤回事件可能早于被撤回的消息到达（Gossip 泛洪与
                    // outbox 直发是两条无顺序保证的路径）。命中权威集合就直接以
                    // 「已撤回」形态入库 —— 否则消息会带着完整正文落地，撤回失效。
                    let mut rec = rec;
                    if db::is_recalled(&dbc, &rec.msg_id) {
                        rec.kind = crate::protocol::KIND_RECALLED.to_string();
                        rec.content = String::new();
                    }
                    let inserted = db::insert_message_if_new(&dbc, &rec);
                    if announced_on(&inserted) {
                        // 时钟推进与静默**无关**，必须照常：漏掉它本机后续 seq 会落后，
                        // 之后自己发的消息会排到历史前面。
                        db::observe_clock(&dbc, &conv_id, seq).ok();
                        if crate::protocol::is_non_notifying_kind(&kind) {
                            // 静默事件（表情回应/撤回）与系统提示都不计未读、不改会话预览 ——
                            // 否则「回个表情」或「X 加入了群聊」会把会话顶到列表最前并弹通知。
                            // 但会话行必须存在，前端要靠它把事件归属到正确的会话。
                            db::ensure_conversation(&dbc, &conv_id, conv_kind, &name, None).ok();
                        } else {
                            db::touch_conversation(
                                &dbc, &conv_id, conv_kind, &name, None, &preview, 1,
                            )
                            .ok();
                        }
                    }
                    (rec, inserted)
                };
                // 重复投递与数据库失败都不产生本地副作用；Gossip 的转发已在上面完成。
                if announced_on(&inserted) {
                    let _ = state.app.emit("message-received", &out_rec);
                }
                // 更新会话「当前链路」：单聊消息的 hop 由 Gossip ttl 反推
                // （初始 ttl - 收到 ttl），path 取入站连接的路径（直连准确，桥接为最后一段）。
                // 仅首次落库（非重复投递）才更新，避免「走了不同路径的重复副本」干扰。
                if conv_kind == "single" && announced_on(&inserted) {
                    let hop = {
                        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                        gossip.ttl.saturating_sub(env.ttl)
                    };
                    let path = inbound_path_kind(state, peer_id).await;
                    update_conv_link(state, &conv_id, &path, hop);
                }
                // 群消息现在有 outbox 兜底：只要消息确实已持久化（无论本次是否新建），
                // 就回 GroupAck 让发送方删除对应 (msg_id, peer_id) 的待发记录。
                // 数据库 Err 时不回 Ack，发送方保留 outbox 继续补发。
                if conv_kind == "group" && !inserted.is_err() {
                    let ack = Message::GroupAck {
                        group_id: env.group_id.clone().unwrap_or_default(),
                        msg_id: env.message_id.clone(),
                        from: state.device_id.clone(),
                    };
                    let _ = try_send(state, &env.sender_id, &ack).await;
                }
                // 单聊送达确认：跨跳（无直连）时直连 Ack 到不了原始发送方，改走定向
                // Gossip ChatAck；有直连时也走 Gossip，让「已送达」立即出现，不必等
                // 心跳触发 outbox 直发补 Ack。接收端按 outbox(msg_id, sender) 命中才接受，
                // 防伪造送达。
                if conv_kind == "single" && !inserted.is_err() {
                    let payload = serde_json::json!({ "msg_id": env.message_id }).to_string();
                    let payload_b64 = STANDARD.encode(payload.as_bytes());
                    let mut ack_env = {
                        let gossip = state.gossip.lock().unwrap_or_else(|e| e.into_inner());
                        gossip.build_envelope(
                            &state.identity,
                            &state.device_id,
                            GossipKind::ChatAck,
                            None,
                            None,
                            &payload_b64,
                            db::now_ms(),
                            0,
                        )
                    };
                    ack_env.encrypted = false;
                    ack_env.target = Some(env.sender_id.clone());
                    ack_env.sender_sig = state.identity.sign_b64(&ack_env.signing_bytes());
                    if state.has_link(&env.sender_id).await {
                        let _ = try_send(
                            state,
                            &env.sender_id,
                            &Message::Gossip { envelope: ack_env },
                        )
                        .await;
                    } else {
                        broadcast_gossip(state, ack_env).await;
                    }
                }
            }
        }
    }
}

fn parse_gossip_payload(pt: &[u8]) -> (String, String) {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(pt) {
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("text")
            .to_string();
        let content = v
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        (kind, content)
    } else {
        ("text".to_string(), String::from_utf8_lossy(pt).to_string())
    }
}
