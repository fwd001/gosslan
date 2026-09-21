// 职责边界：
// - 一对一聊天（send_message、会话查询、已读）
// - 消息删除、撤回、合并转发
// ---------------- 单聊（Gossip + E2EE） ----------------

#[tauri::command(async)]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    content: String,
    kind: String,
) -> Result<MessageRecord, String> {
    let s = state.inner();

    // 「和自己聊天」分流：target 是自己时走**纯本地路径**（见 `insert_self_message`）。
    // 放在最前面是有意的 —— 下面每一步（好友校验 / 公钥查找 / 加密 / outbox / gossip）
    // 对自己都不成立。
    if friend_id == s.device_id {
        return insert_self_message(s, &kind, content);
    }

    let msg_kind = match kind.as_str() {
        "text" => MsgKind::Text,
        "code" => MsgKind::Code,
        "file" => MsgKind::File,
        // 合并转发：载荷先过校验（能解析、条数合法），别等到对方那边才炸。
        "merge" => {
            crate::protocol::parse_merge_payload(&content)?;
            MsgKind::Merge
        }
        _ => return Err("不支持的消息类型".to_string()),
    };

    // 好友关系检查：必须优先于公钥查找
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友，请先扫描添加好友之后再继续聊天。".to_string());
        }
    }

    // 长度保护：text/code 等普通内容超限直接报错（UTF-8 安全，按字符数计）。
    let content = check_message_content(content)?;

    // INV-P24 第 4 条：**不门控不许发**。老对端（v4.20.0 及更早）的 `ChatMessage.kind`
    // 还是嵌套枚举，收到不认识的 kind 会**整帧丢掉** —— v4.22.34 只修了我们这一侧的容忍，
    // 那边没有，所以只能由发送侧挡下来，并给用户一句能照着做的话。
    // 判据只有 `protocol::kind_allowed_by_features` 一处；对端从没交换过 Hello 或已离线
    // ⇒ 位图按 0 处理 = "不知道就当不支持"（宁可少发一条，也不要静默丢帧）。
    // 但"不知道"与"它自己声明过不支持"要分开说：这张表是内存态、离线就被 sweep 回收，
    // 所以缺条目绝大多数时候只意味着对方此刻不在线，把两种情况混成一句"版本较旧"是假指控
    // （文案本身仍只住在 `protocol.rs`，这里只做选择）。
    // 放在公钥探测**之前**：这条本来就不会发出去，不该再触发一次 who_has 探测白等 1.2s。
    if crate::protocol::kind_required_feature(&kind).is_some() {
        let peer_features = s
            .peer_content_features
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&friend_id)
            .copied();
        if !crate::protocol::kind_allowed_by_features(&kind, peer_features.unwrap_or(0)) {
            return Err(crate::protocol::kind_blocked_hint(&kind, peer_features));
        }
    }

    // E2EE 恒开（v0.11.0 起默认且不可关闭）：发送必须拿到对端 X25519 公钥。
    // 好友表优先，回退在线节点表；都缺失时主动探测一次（who_has）等对方/中继
    // announce 落库（约 1.2s）后再查，仍缺失则报错指引。
    let pubkey = {
        let from_db = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_friend_x25519(&dbc, &friend_id)
        };
        let from_peers = s
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&friend_id)
            .and_then(|p| p.x25519_pubkey.clone());
        match from_db.or(from_peers) {
            Some(k) => Some(k),
            None => {
                let triggered =
                    if let Some(tx) = s.probe.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        let next = tx.borrow().saturating_add(1);
                        let _ = tx.send(next);
                        true
                    } else {
                        false
                    };
                if triggered {
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                }
                let again_db = {
                    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::get_friend_x25519(&dbc, &friend_id)
                };
                let again_peers = s
                    .peers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&friend_id)
                    .and_then(|p| p.x25519_pubkey.clone());
                again_db.or(again_peers)
            }
        }
    };
    let Some(pubkey) = pubkey else {
        return Err(format!(
            "尚未获取 {friend_id} 的公钥：对方可能离线或处于不同子网，请让对方上线后重试"
        ));
    };

    let ts = db::now_ms();
    let seq = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, &friend_id).map_err(|e| format!("逻辑时钟推进失败：{e}"))?
    };
    let name = resolve_nickname(s, &friend_id);
    let preview = crate::protocol::preview_text(&kind, &content);

    // E2EE 加密 + Gossip 信封（先于本地落库：msg_id 三处统一用 Gossip 信封 ID）
    let plaintext = serde_json::json!({ "kind": kind, "content": content }).to_string();
    let shared = crypto::shared_secret(&s.identity.x25519_secret, &pubkey).ok_or("密钥交换失败")?;
    // Gossip 载荷与直发内容都走 ChaCha20-Poly1305（直发内容加 "enc1:" 前缀标识）
    let sealed = crypto::seal(&shared, plaintext.as_bytes()).ok_or("加密失败")?;
    let sealed_content = crypto::seal(&shared, content.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let wire_content = format!("enc1:{}", STANDARD.encode(&sealed_content));
    let mut env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::Chat,
            None,
            None,
            &payload_b64,
            ts,
            seq,
        )
    };
    // 信封 encrypted 默认 true（build_envelope 内置），无需改写
    // 统一 msg_id：本地记录 / Gossip 投递 / outbox 补发共用同一确定性 ID，
    // 接收方 message_exists 跨路径去重（防建链竞态窗口内的重复投递）。
    let msg_id = env.message_id.clone();
    // 单聊定向：target = 接收方。中间节点按 target 定向转发（一跳精确，无路由表时洪泛
    // 兜底），直连场景不再全网广播（消除广播放大）。target 参与 signing_bytes，必须重签。
    env.target = Some(friend_id.clone());
    env.sender_sig = s.identity.sign_b64(&env.signing_bytes());

    // 本地落库（明文）
    // 初始状态 = "sending"：消息刚写库、正在入发送队列。
    // try_send 成功（进 mpsc channel）后前进到 "sent"；
    // Ack 到达 → "delivered"；ReadReceipt → "read"。
    // 超时 / 主动取消 → "failed" / "cancelled"（见 P2/P3）。
    let rec = MessageRecord {
        id: 0,
        msg_id: msg_id.clone(),
        conv_id: friend_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: friend_id.clone(),
        kind: kind.clone(),
        content: content.clone(),
        ts,
        seq,
        status: "sending".to_string(),
    };
    // 一律写离线队列兜底（INSERT OR IGNORE 按 msg_id 幂等）：直连链路存在但已失效
    // （半开 TCP）时 broadcast 会静默丢包，此前只在「无链路」时入队导致消息永久丢失。
    // Ack 到达后由 transport.rs 删除该行；若链路中断，对方上线建链（Hello）或心跳
    // 会触发 flush_outbox 自动补发，接收方按 msg_id 去重不会重复入库。
    let queued = Message::ChatMessage {
        msg_id: msg_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
        // 线格式是字符串（见 `ChatMessage::kind`）；发送侧仍只产出 `MsgKind` 的词表。
        kind: msg_kind.as_str().to_string(),
        content: wire_content,
        ts,
        seq,
    };
    let payload = serde_json::to_string(&queued).map_err(|e| e.to_string())?;
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::insert_message_and_outbox(&dbc, &rec, &friend_id, &payload)
            .map_err(|e| format!("消息写入失败：{e}"))?;
        db::touch_conversation(&dbc, &friend_id, "single", &name, None, &preview, 0)
            .map_err(|e| format!("会话写入失败：{e}"))?;
    }

    // 先入队再投递（INV-003）：此前 broadcast 在插队之前，若心跳的 flush_outbox 正好
    // 落在这个窗口，它看不到 outbox 行 ⇒ 这一轮直发缺席 ⇒ Ack 要等下一个心跳（+5s）。
    // 定向投递：目标直连 → 只发它（精确，不再全网广播）；否则广播，靠中间节点按 target
    // 定向转发（跨跳）。投递失败**不返回 Err**：消息已落 outbox 兜底，链路刚断的竞态
    // 下由 flush_outbox 在下次建链/心跳时补发，返回 Err 会让前端误判「发送失败」而重发。
    //
    // 状态前进：try_send 成功（进 writer_loop channel）→ "sent"；
    // 广播模式无条件乐观前进（广播是尽力而为，视为已发出）；
    // try_send 失败则保持 "sending"（在 outbox 等下次 flush_outbox 重试）。
    let has = s.has_link(&friend_id).await;
    s.logger.info(
        "dispatch",
        format!(
            "[SEND-MSG] has_link={has} peer={friend_id} msg_id={msg_id} \
             (links_table_entries={})",
            s.links.lock().await.len()
        ),
    );
    let try_ok = if has {
        let r = try_send(s, &friend_id, &Message::Gossip { envelope: env.clone() })
            .await
            .is_ok();
        s.logger.info(
            "dispatch",
            format!("[SEND-MSG] try_send result={r} peer={friend_id}"),
        );
        r
    } else {
        s.logger.warn(
            "dispatch",
            format!(
                "[SEND-MSG] NO-LINK → broadcast_gossip peer={friend_id} (links table empty!)"
            ),
        );
        broadcast_gossip(s, env).await;
        true // 广播视为乐观已发出
    };
    if try_ok {
        if let Ok(dbc) = s.db.lock() {
            let _ = db::set_message_status(&dbc, &msg_id, "sent");
        }
    }
    // 更新会话「当前链路」（发送方视角）：有直连则 hop=0 + 出站路径；无直连
    // （经中继广播）则乐观记 hop=1（实际跳数发送方不可知，等对端回执侧视角校正）。
    {
        let hop = if s.has_link(&friend_id).await { 0 } else { 1 };
        let path = crate::network::transport::inbound_path_kind(s, &friend_id).await;
        crate::network::transport::update_conv_link(s, &friend_id, &path, hop);
    }

    Ok(rec)
}

// INV-EXCEPTION: INV-P03, INV-P04 — 自聊收发双方都是本机，没有对端可等 Ack：
// 落库即终态 `read`（跳过 queued→sending→waiting_ack→delivered），且**不写 outbox**
// （那一行永远排不掉，反把「outbox 必然排空」破掉）。
// 登记在 docs/protocol-invariants.md §22，由 scripts/check-invariant-exceptions.mjs 双向校验。
/// 给自己发一条消息（「和自己聊天」）—— **纯本地，消息不出本机**。
///
/// 为什么必须是独立路径，而不是"把自己当好友"复用下面的发送流程：
///
/// 1. **没有传输**：收发双方都是本机 ⇒ 没有链路可发、没有对端公钥可用。
///    E2EE 保护的是**传输**（"E2EE 恒开"说的是网络路径）；本地落盘与其它会话一样是
///    SQLite 明文，所以这里不加密**不是**"加密失败就退明文"那种兜底。
/// 2. **绝不能进 outbox**：outbox 的唯一出队条件是收到对端 Ack，给自己发包永远不会有 Ack
///    ⇒ 那一行会永远留在库里、被每次心跳/建链的 `flush_outbox` 重发，
///    把"outbox 必然排空"这条不变量破掉。
/// 3. **绝不能广播**：`target = 自己` 的 gossip 信封对别人是解不开的噪声，
///    本机自己也会在 `handle_gossip` 的 `sender == 自己` 早退里丢掉 —— 纯浪费带宽与 TTL。
///
/// 状态直接给 `"read"`：本机既是发送方也是接收方，不存在"在途"阶段；
/// 前端也不会给自聊消息挂回执（见 `src/utils/selfChat.ts`）。
fn insert_self_message(s: &AppState, kind: &str, content: String) -> Result<MessageRecord, String> {
    // 只支持文本 / 代码：用户 2026-09-16 明确「先只支持文本」。图片与文件要落盘、要文件卡片，
    // 走的是另一条链路（`send_file`），自聊里前端也不会给附件入口 —— 真调到了就明确报错，
    // 不要静默吞掉。
    if kind != "text" && kind != "code" {
        return Err("和自己聊天暂不支持图片或文件".to_string());
    }
    let content = check_message_content(content)?;
    let ts = db::now_ms();
    let me = s.device_id.clone();
    // ⚠️ 名字/头像都在**拿 db 锁之前**取好：`self_display_name`/`self_avatar` 各自还要锁
    // 昵称与头像（`is_zh` 里还会再锁一次 db），持锁期间再回头锁它们就是在赌锁顺序
    // （本文件里"不能在持有 db 锁时调用 resolve_nickname"那条注释说的是同一件事）。
    let name = s.self_display_name();
    let avatar = s.self_avatar();
    let preview = crate::protocol::preview_text(kind, &content);
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let seq = db::next_clock(&dbc, &me).map_err(|e| format!("逻辑时钟推进失败：{e}"))?;
    let rec = MessageRecord {
        id: 0,
        // 前缀 `self-` 让它一眼可辨（日志/排障时不会与网络消息的哈希 id 混淆）
        msg_id: format!("self-{}", Uuid::new_v4()),
        conv_id: me.clone(),
        sender_id: me.clone(),
        receiver_id: me.clone(),
        kind: kind.to_string(),
        content,
        ts,
        seq,
        status: "read".to_string(),
    };
    // ⚠️ `insert_message`（只落库）—— **不是** `insert_message_and_outbox`：
    // 自聊消息没有收件人，进 outbox 就永远排不掉（见上面的第 2 条）。
    db::insert_message(&dbc, &rec).map_err(|e| format!("消息写入失败：{e}"))?;
    // unread_inc = 0：自己发的消息不该让自己"有未读"（与 `send_message` 同口径）
    db::touch_conversation(&dbc, &me, "single", &name, avatar.as_deref(), &preview, 0)
        .map_err(|e| format!("会话写入失败：{e}"))?;
    Ok(rec)
}

/// 读取会话的「当前链路」。前端聊天窗口据此显示连接图标（LAN / 桥接 / 蓝牙 + 节点数）。
///
/// **有直连时以此刻实际选路为准（hop=0）**，而不是返回"上一条消息"的快照 ——
/// 否则链路从蓝牙/中继切回局域网后，聊天头会一直显示「桥接」直到再发一条消息
/// （用户 2026-09-14 真机：两边全在局域网，却显示「已桥接」）。
/// 无直连时才回落到最后一次的快照（桥接跳数由消息路径反推，只在收发时更新）。
/// 注意返回 Result<Option<_>, _>：Tauri 要求"带引用输入的 async 命令"必须返回 Result
/// （State<'_, _> 就是引用输入）。Ok 会被自动解包，前端拿到的仍是 LinkState | null，
/// 契约不变。
#[tauri::command(async)]
pub async fn get_conv_link(
    state: State<'_, Arc<AppState>>,
    conv_id: String,
) -> Result<Option<crate::state::LinkState>, String> {
    let s = state.inner();
    if s.has_link(&conv_id).await {
        let path = crate::network::transport::inbound_path_kind(s, &conv_id).await;
        return Ok(Some(crate::state::LinkState { path, hop: 0 }));
    }
    Ok(s.conv_link
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&conv_id)
        .cloned())
}

#[tauri::command(async)]
pub fn get_messages(
    state: State<'_, Arc<AppState>>,
    conv_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Vec<MessageRecord> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    let safe_limit = limit.unwrap_or(100).clamp(1, 500);
    let safe_offset = offset.unwrap_or(0).max(0);
    db::get_messages(&dbc, &conv_id, safe_limit, safe_offset).unwrap_or_default()
}

#[tauri::command(async)]
pub fn get_message_count(state: State<'_, Arc<AppState>>, conv_id: String) -> i64 {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::count_messages(&dbc, &conv_id)
}

#[tauri::command(async)]
pub fn get_conversations(state: State<'_, Arc<AppState>>) -> Vec<Conversation> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::list_conversations(&dbc).unwrap_or_default()
}

/// 打开与好友的会话时确保会话行存在（新加好友尚未发过消息时，
/// 会话列表无对应项 → 左侧无法高亮选中态）。
#[tauri::command(async)]
pub fn ensure_conversation(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
) -> Result<Conversation, String> {
    let s = state.inner();
    // 「和自己聊天」：名字/头像取**本机**的，否则 `resolve_nickname` 会回落到 device_id 原文，
    // 会话列表里就成了一串 gosslan-xxxx。
    let (name, avatar) = if friend_id == s.device_id {
        (s.self_display_name(), s.self_avatar())
    } else {
        let name = resolve_nickname(s, &friend_id);
        let avatar = {
            let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
            peers.get(&friend_id).and_then(|p| p.avatar.clone())
        };
        (name, avatar)
    };
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::ensure_conversation(&dbc, &friend_id, "single", &name, avatar.as_deref())
            .map_err(|e| e.to_string())?;
        // 回读已存在的行：会话可能早已建立且被置顶，凭空造一个 pinned=false
        // 会让前端把它当成「未置顶」从而覆盖掉用户的置顶状态。
        if let Some(conv) = db::get_conversation(&dbc, &friend_id) {
            return Ok(conv);
        }
    }
    Ok(Conversation {
        id: friend_id.clone(),
        kind: "single".to_string(),
        name,
        avatar,
        last_msg: None,
        last_ts: None,
        unread: 0,
        pinned: false,
    })
}

/// 设置会话置顶（纯本地偏好，不广播、不同步）。
#[tauri::command(async)]
pub fn set_conversation_pinned(
    state: State<'_, Arc<AppState>>,
    conv_id: String,
    pinned: bool,
) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::set_conversation_pinned(&dbc, &conv_id, pinned).map_err(|e| e.to_string())
}

/// 标记会话已读；单聊时向对方发送已读回执（触发对方界面的「已读绿勾」）。
#[tauri::command(async)]
pub async fn mark_read(state: State<'_, Arc<AppState>>, conv_id: String) -> Result<(), String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::mark_read(&dbc, &conv_id).map_err(|e| e.to_string())?;
    }
    if !conv_id.starts_with("group:") {
        // 自聊（会话 id == 自己）：本机既是发送方也是接收方，没有"对方"可收回执。
        // 真发出去只会在 `pending_reads` 里留一条永远排不掉的记录（`flush_pending_reads`
        // 每次建链/心跳都会重试一次）。未读清空已经在上面做完了，这里直接返回。
        if conv_id == s.device_id {
            return Ok(());
        }
        // 通知对方：我已读到「对方最近一条消息」为止。
        // 这里不能取全会话最大 ts：一是可能取到自己发的消息，二是对方消息在本机
        // 落库时被时钟钳制过，直接回传 ts 会让对方用自己的原始时间戳匹配不上。
        // 回传 msg_id，由发送方换算成自己的本地时间戳。
        let last = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::last_message_from_sender(&dbc, &conv_id, &conv_id)
        };
        if let Some((msg_id, ts)) = last {
            // 同网段走直连 ReadReceipt，跨跳走定向 Gossip ChatReadReceipt。
            // try_send 返回 Ok 只代表消息进入 mpsc channel，不代表 TCP writer
            // 真正 write_frame 成功——writer_loop 可能随后发现链路已断而丢弃。
            // 因此无论结果都保留 pending：下一次心跳/建链时 flush 重发。
            let _ =
                crate::network::transport::send_read_receipt_route(s, &conv_id, Some(msg_id), ts)
                    .await;
            {
                let mut pending = s.pending_reads.lock().unwrap_or_else(|e| e.into_inner());
                let cur = pending.entry(conv_id.clone()).or_insert(ts);
                *cur = (*cur).max(ts);
            }
            // 持久化到 DB：进程重启后 pending_reads 内存丢失时可从 DB 恢复。
            // 使用 max 语义（upsert_pending_read）保证较旧 timestamp 不覆盖较新。
            {
                let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_pending_read(&dbc, &conv_id, ts).ok();
            }
        }
    } else if let Some(group_id) = conv_id.strip_prefix("group:") {
        let group = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_group(&dbc, group_id)
        };
        if let Some(group) = group {
            for member in group.members {
                if member == s.device_id {
                    continue;
                }
                // 群回执同样按「该成员最近一条消息」发送，避免跨设备时钟偏差。
                let last = {
                    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::last_message_from_sender(&dbc, &conv_id, &member)
                };
                let Some((msg_id, last_read_ts)) = last else {
                    continue;
                };
                let msg = Message::GroupReadReceipt {
                    from: s.device_id.clone(),
                    group_id: group_id.to_string(),
                    last_read_ts,
                    last_read_msg_id: Some(msg_id),
                };
                let _ = crate::network::transport::try_send(s, &member, &msg).await;
                // 无论即时发送是否成功都持久化待发记录，由建链/Hello/心跳补发；
                // 接收端按 (group_id, reader_id) 单调去重，重复送达无副作用。
                let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_pending_group_read(&dbc, group_id, &member, last_read_ts).ok();
            }
        }
    }
    Ok(())
}

/// 删除本地会话与全部消息（聊天记录清理）。
/// 仅删本地：不影响对方、不广播；前端负责二次确认弹窗。
/// 群聊同样支持（删除 group:xxx 会话及全部消息）。
#[tauri::command(async)]
pub fn delete_conversation(state: State<'_, Arc<AppState>>, conv_id: String) -> Result<(), String> {
    let s = state.inner();
    // 群会话删除时写删除边界：其他成员保留的历史重放不得回灌本机。
    // 边界记录当前逻辑序号，而非墙上时钟。
    if let Some(gid) = conv_id.strip_prefix("group:") {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let boundary = db::get_clock(&dbc, &conv_id);
        db::set_clear_boundary(&dbc, gid, boundary).map_err(|e| e.to_string())?;
    }
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    db::delete_conversation(&dbc, &conv_id).map_err(|e| e.to_string())
}

/// 用户主动取消发送中的消息。
///
/// 语义：**这条消息还没送达对端（outbox 还在），用户不想继续发了**。
/// 撤回是另一个操作（消息已送达后让对方删），不要混成一个 API。
///
/// 处理：
/// 1. 查消息 → 终态（delivered/read/recalled）不能取消
/// 2. 删单聊 outbox + 群 outbox（所有可能的发送队列）
/// 3. set_message_status("cancelled")（终态守卫保证幂等）
/// 4. emit("message-cancelled", msg_id) 通知前端刷新气泡状态
#[tauri::command(async)]
pub async fn cancel_send(
    state: State<'_, Arc<AppState>>,
    msg_id: String,
) -> Result<(), String> {
    let s = state.inner();

    // 0. 如果是文件消息（file-xxx 或 gfile-xxx），转调 cancel_file_transfer
    //    — 那里会触发 file_send_cancels oneshot 打断实际 chunk 循环
    if let Some(transfer_id) = msg_id.strip_prefix("file-").or_else(|| msg_id.strip_prefix("gfile-")) {
        return cancel_file_transfer(
            state,
            transfer_id.to_string(),
        )
        .await
        .map(|_| ());
    }

    // 1. 查消息状态：终态（delivered/read/recalled）不能取消
    let rec = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_message_record(&dbc, &msg_id)
            .ok_or_else(|| format!("消息不存在：{msg_id}"))?
    };

    match rec.status.as_str() {
        "delivered" | "read" | "recalled" => {
            return Err(format!(
                "消息已{}，不能取消发送（请用撤回功能）",
                rec.status
            ));
        }
        "cancelled" => {
            return Ok(()); // 幂等：已取消过就直接成功
        }
        _ => {} // sending / sent / failed → 可以取消
    }

    // 删所有 outbox 队列（单聊 + 群）
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::delete_outbox_by_msg_id(&dbc, &msg_id);
        let _ = db::delete_group_outbox_by_msg_id(&dbc, &msg_id);
        // 置 cancelled
        let _ = db::set_message_status(&dbc, &msg_id, "cancelled");
    }

    let _ = s.app.emit("message-cancelled", &msg_id);
    Ok(())
}

/// 重发失败的消息。
///
/// 语义：消息之前因超时/网络错误被判 failed 或被用户取消，用户点击"重发"。
///
/// 状态流转：failed/cancelled → sending → 有链路则 sent → 等 Ack → delivered
#[tauri::command(async)]
pub async fn resend_message(
    state: State<'_, Arc<AppState>>,
    msg_id: String,
) -> Result<(), String> {
    let s = state.inner();

    // 1. 查消息存在性 + 当前状态
    let rec = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_message_record(&dbc, &msg_id)
            .ok_or_else(|| format!("消息不存在：{msg_id}"))?
    };

    // 终态不可重发
    match rec.status.as_str() {
        "delivered" | "read" => {
            return Err("消息已送达，无需重发".to_string());
        }
        "sending" | "sent" => {
            return Err("消息正在发送中".to_string());
        }
        _ => {} // failed / cancelled → 可以重发
    }

    // 2. 重置状态为 sending
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::set_message_status(&dbc, &msg_id, "sending");
    }

    // 3. 判断是单聊还是群聊
    if rec.conv_id.strip_prefix("group:").is_some() {
        // 群消息简化：通知前端重新发
        Err("群消息重发请删除后重新发送".to_string())
    } else {
        // 单聊：重建 outbox（sweeper 判 failed 时已删），再 try_send。
        // 必须先用对端当前公钥重新密封（与 send_message 同一套 seal 逻辑）：
        // messages 表存的是明文，直接把 rec.content 上线没有 `enc1:` 前缀 ⇒
        // 接收端 open_direct_content 拒收（不落库不 Ack），重发静默变成 no-op，
        // 稍后再被判 failed —— 用户看到「重发没反应」就是这么来的。
        // ts/seq 同理必须沿用原记录：seq=0 会让接收端把重发消息排到会话最前
        //（排序按 seq，INV-P09），两端顺序分裂。
        if rec.sender_id != s.device_id {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::set_message_status(&dbc, &msg_id, &rec.status);
            return Err("只能重发自己发出的消息".to_string());
        }
        let msg_kind = crate::protocol::MsgKind::from_wire_str(&rec.kind);
        let pubkey = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_friend_x25519(&dbc, &rec.receiver_id)
        }
        .or_else(|| {
            s.peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&rec.receiver_id)
                .and_then(|p| p.x25519_pubkey.clone())
        });
        let Some(pubkey) = pubkey else {
            // 没有公钥就发不出去加密消息：回滚状态并明确报错，
            // 而不是写一条接收端永远拒收的明文 outbox 行。
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::set_message_status(&dbc, &msg_id, &rec.status);
            return Err(format!(
                "尚未获取 {} 的公钥，无法重发：对方可能离线或处于不同子网",
                rec.receiver_id
            ));
        };
        let shared = crypto::shared_secret(&s.identity.x25519_secret, &pubkey)
            .ok_or("密钥交换失败")?;
        let sealed = crypto::seal(&shared, rec.content.as_bytes()).ok_or("加密失败")?;
        let queued = Message::ChatMessage {
            msg_id: rec.msg_id.clone(),
            from: s.device_id.clone(),
            to: rec.receiver_id.clone(),
            kind: msg_kind.as_str().to_string(),
            content: format!("enc1:{}", STANDARD.encode(&sealed)),
            ts: rec.ts,
            seq: rec.seq,
        };
        let payload = serde_json::to_string(&queued).map_err(|e| e.to_string())?;

        // 先写 outbox（INSERT OR IGNORE 幂等）— 确保下次建链 flush_outbox 能捞到
        {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::insert_outbox(&dbc, &msg_id, &rec.receiver_id, &payload);
        }

        // 有链路则立即 try_send
        if s.has_link(&rec.receiver_id).await {
            let _ = crate::network::transport::try_send(s, &rec.receiver_id, &queued).await;
            // try_send 成功 → 前进到 sent
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::set_message_status(&dbc, &msg_id, "sent");
        }
        // 无链路 → 保持 sending，等 flush_outbox 下次建链/心跳时捞 outbox 重发（已重建）

        let _ = s.app.emit("message-resending", &msg_id);
        Ok(())
    }
}

/// 撤回一条自己发的**单聊**消息。
///
/// 与群撤回（`recall_group_message`）共享同一套协议语义 —
/// `kind = KIND_RECALL` 的 Gossip envelope，payload = RecallPayload JSON。
/// 接收端在 handle_message 的 Gossip 分支里识别并处理（作者校验 + materialize_recall）。
///
/// 发送侧做：
/// 1. 作者校验 + 5min 时间窗（本地 ts 为准）
/// 2. 构建定向 Gossip envelope（target = friend_id，让中继按 target 定向转发）
/// 3. try_send 直连 + broadcast_gossip 跨跳
/// 4. 本地 insert_recall + materialize_recall（先发送、后物化，顺序不能反）
/// 5. emit("message-recalled", msg_id) 前端刷新
#[tauri::command(async)]
pub async fn recall_message(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    msg_id: String,
) -> Result<(), String> {
    let s = state.inner();

    // 1. 查消息 + 权限 + 时间窗
    let rec = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_message_record(&dbc, &msg_id)
            .ok_or_else(|| format!("消息不存在：{msg_id}"))?
    };
    if rec.sender_id != s.device_id {
        return Err("只能撤回自己发送的消息".to_string());
    }
    if rec.status == "recalled" {
        return Ok(()); // 幂等：已撤回过就直接成功
    }
    let now = db::now_ms();
    if rec.ts > 0 && now - rec.ts > crate::commands::RECALL_WINDOW_MS {
        return Err("超过可撤回时间（5 分钟）".to_string());
    }
    // 已 cancelled/failed 的消息没必要撤回 — 撤回是让对方删，对方可能根本没收到。
    // 但用户仍可撤回（只是本地标 recalled），打一条 warn 提示开发者这种边缘路径被走了。
    if matches!(rec.status.as_str(), "cancelled" | "failed") {
        eprintln!(
            "[gosslan][recall] warn: 撤回的 msg_id={msg_id} 当前 status={} — \
             撤回是让对方删，对方可能根本没收到；仍按用户请求执行本地 recalled",
            rec.status
        );
    }

    // 2. 构建 RecallPayload + Gossip envelope（定向到 friend_id）
    let ts = now;
    let conv_id = friend_id.clone();
    let seq = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, &conv_id).map_err(|e| format!("逻辑时钟推进失败：{e}"))?
    };
    let payload = crate::protocol::RecallPayload {
        target: msg_id.clone(),
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let plaintext = serde_json::json!({
        "kind": crate::protocol::KIND_RECALL,
        "content": content,
    })
    .to_string();
    let payload_b64 = STANDARD.encode(plaintext.as_bytes());

    let mut env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        let mut env = gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::Chat,
            None,
            None,
            &payload_b64,
            ts,
            seq,
        );
        // 定向到 friend_id（让中继按 target 转发，跨跳也能到达）
        env.target = Some(friend_id.clone());
        env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
        env
    };
    // 撤回事件**不需要 E2EE**（只有 target 一个人能收到，加密反而让本地需要解密才能识别 kind）
    env.encrypted = false;

    // 3. 先发协议（直连 + 广播），不进 outbox — 撤回尽力而为
    if s.has_link(&friend_id).await {
        let _ = try_send(s, &friend_id, &Message::Gossip { envelope: env.clone() }).await;
    } else {
        broadcast_gossip(s, env.clone()).await;
    }

    // 4. 本地物化撤回（在发送之后 — 顺序反了会出现"本地已撤回但对端永远没机会收到"）
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::insert_recall(&dbc, &conv_id, &msg_id, &s.device_id, seq);
        let _ = db::materialize_recall(&dbc, &msg_id);
    }
    let _ = s.app.emit("message-recalled", &msg_id);
    Ok(())
}
