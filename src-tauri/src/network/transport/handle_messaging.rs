// 职责边界：`handle_message` 的「消息与送达回执」分支组 —— 只处理 `ChatStyle`、`ChatMessage`、`Ack`、`ReadReceipt` 这些变体；不做路由、不起新连接，落库/发送都在各自臂体里（与原函数逐字相同）。
// 为什么单独一册：`handle_message` 原来 1,540 行 / 38 个臂挤在一个函数里。已用 AST 现量过
//   **match 之前没有裸 let 绑定、分支之间不共享局部量** ⇒ 按消息族拆出去是机械搬家而不是改控制流，
//   依据与量法见 docs/large-file-split-plan.md §5.1。
// 恒等判据（每族都跑同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点仍恰好命中一次、clippy `-D warnings`、`cargo fmt --check`。
// 为什么并成一族：这 4 个变体是同一条送达链的四段——落库（ChatMessage）、对端确认（Ack 清 outbox）、已读（ReadReceipt 只推游标不改摘要）、
//   气泡样式（ChatStyle）。它们在 handle_message 里隔着两百多行，而 INV-006/007 的钉法要求
//   「落库 → 回执 → 乐观清零」三段一起读才看得出谁点谁。

async fn handle_messaging_arm(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    // 这层 match 与原函数里的同一层（臂仍是 8 格缩进 ⇒ 逐字未改）。
    // `_ => {}` 不是吞消息：调用方只在命中本族变体时才把 msg 交进来。
    match msg {
        Message::ChatStyle { from, to, style } => {
            if from == state.device_id {
                return;
            }
            if let Some(t) = &to {
                if t != &state.device_id {
                    return; // 定向给别人，忽略
                }
            }
            // 持久化对端样式表（device_id -> style JSON），前端按发送者渲染其消息气泡
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let mut map: serde_json::Map<String, serde_json::Value> =
                    db::get_setting(&dbc, "chat_peer_styles")
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default();
                map.insert(from.clone(), serde_json::Value::String(style.clone()));
                if let Ok(json) = serde_json::to_string(&map) {
                    db::set_setting(&dbc, "chat_peer_styles", &json).ok();
                }
            }
            let _ = state.app.emit(
                "peer-style-updated",
                &serde_json::json!({ "device_id": from, "style": style }),
            );
        }
        Message::ChatMessage {
            msg_id,
            from,
            to,
            kind,
            content,
            ts: _ts,
            seq,
        } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 去重前置：真实 msg_id 已落库 == 这条消息我此前已成功接收并持久化，
            // 于是只回 Ack。必须早于解密——否则对方轮换密钥后重投的那份「已收好的」
            // 消息会因当前密钥打不开旧密文而被误判为失败。
            let exists = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::message_exists(&dbc, &msg_id)
            };
            if exists {
                // 重复投递也要回 Ack（否则一次丢 ACK = 发送方永远"发送中"）：
                // 留痕区分"没收到"与"收到了但 ACK 丢了"。
                state.logger.info(
                    "ble",
                    format!("[ACK] 重复消息仍回执 msg_id={msg_id} ← peer={peer_id}"),
                );
                let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
                return;
            }
            // 好友关系检查：非好友消息不落库、不 Ack、通知发送方
            let is_friend = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some()
            };
            if !is_friend {
                let _ = try_send(
                    state,
                    peer_id,
                    &Message::FriendMessageBlocked {
                        from: state.device_id.clone(),
                        to: from.clone(),
                        original_sender: from.clone(),
                    },
                )
                .await;
                return;
            }
            // E2EE："enc1:" = 发送方→我的 ChaCha20-Poly1305 密文，用发送方 X25519 公钥打开。
            // 打不开（缺公钥 / 公钥已轮换 / 密文损坏）时**既不落库也不 Ack**，原因：
            //  - Ack 的语义是「已成功接收并持久化」，发送方一收到就会删掉 outbox 行；
            //  - 若用真实 msg_id 写一条占位系统消息，同一 msg_id 的后续正确副本会被
            //    INSERT OR IGNORE 静默吞掉，明文永久不可恢复（P0-2 的原始故障形态）。
            // 不 Ack ⇒ outbox 行保留 ⇒ Hello/心跳继续补发；期间 announce·who_has 会把
            // 双方公钥刷进 peers 与 friends 表，补发前还会用最新公钥重新密封
            // （见 flush_outbox / reseal_for_send），消息随自动恢复且不改变 msg_id。
            let Some((content, kind_str)) = open_direct_content(
                &state.identity.x25519_secret,
                sender_x25519_pubkey(state, &from).as_deref(),
                &content,
                kind,
            ) else {
                return;
            };
            let name = resolve_nickname(state, &from);
            let preview = preview_content(&kind_str, &content);
            // 持锁块只做落库，返回带钳制 ts 的记录 + SQLite 的三态裁决；await 全部在锁外
            // （MutexGuard 非 Send）。首次与否由 INSERT 的受影响行数裁决，而不是先查后写：
            // 与 Gossip 并发时只有一方拿到 Ok(true)，未读 +1 / message-received 因此各只一次。
            let (out_rec, inserted) = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                // 展示时间用本地接收时间；排序用对端给出的逻辑序号 seq。
                let ts = db::now_ms();
                let seq = seq.max(1);
                let rec = MessageRecord {
                    id: 0,
                    msg_id: msg_id.clone(),
                    conv_id: from.clone(),
                    sender_id: from.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: kind_str.clone(),
                    content: content.clone(),
                    ts,
                    seq,
                    status: "delivered".to_string(),
                    mention_targets: None,
                };
                let inserted = db::insert_message_if_new(&dbc, &rec);
                if announced_on(&inserted) {
                    // 与群聊分支同一套口径：时钟照常推进，静默类不计未读/不改预览。
                    db::observe_clock(&dbc, &from, seq).ok();
                    if crate::protocol::is_non_notifying_kind(&kind_str) {
                        db::ensure_conversation(&dbc, &from, "single", &name, None).ok();
                    } else {
                        db::touch_conversation(&dbc, &from, "single", &name, None, &preview, 1)
                            .ok();
                    }
                }
                (rec, inserted)
            };
            // 真数据库错误 ⇒ 消息没有持久化 ⇒ 既不投递也绝不 Ack：Ack 会让发送方删除
            // outbox 行，把一次临时故障变成永久丢消息（与 P0-2 同源的红线）。
            if !may_ack(&inserted) {
                return;
            }
            if announced_on(&inserted) {
                let _ = state.app.emit("message-received", &out_rec);
                // 直连单聊消息：链路 = 入站连接的路径，0 个中间节点。
                let path = inbound_path_kind(state, peer_id).await;
                update_conv_link(state, &from, &path, 0);
            }
            // Ack 与「是否本次新建」无关：消息已在库中（无论是哪条路径先写的）即代表已成功接收
            // 留痕（用户要求）：没有这条日志时，"发送中"到底是"没收到"还是"ACK 丢了"分不清。
            state.logger.info(
                "ble",
                format!("[ACK] 已持久化 ⇒ 回执 msg_id={msg_id} → peer={peer_id}"),
            );
            let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
        }
        Message::Ack { msg_id } => {
            // 查询原始发送方：如果这条消息不是我发的，说明我是中继节点，
            // 需要把 Ack 转发给原始发送方（而非本地处理）。
            let original_sender: Option<String> = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                dbc.query_row(
                    "SELECT sender_id FROM messages WHERE msg_id = ?1",
                    params![msg_id],
                    |r| r.get(0),
                )
                .ok()
            };
            match original_sender {
                Some(sender) if sender == state.device_id => {
                    // 情况 1：Ack 对应的原始消息是我发的 → 正常处理
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    // Ack 没有可伪造的 sender 字段，必须同时命中本连接对应的
                    // outbox 目标，避免任意 LAN 节点猜到 msg_id 后伪造送达。
                    let is_expected_peer: bool = dbc
                        .query_row(
                            "SELECT 1 FROM outbox WHERE msg_id = ?1 AND peer_id = ?2",
                            params![msg_id, peer_id],
                            |_| Ok(()),
                        )
                        .is_ok();
                    if !is_expected_peer {
                        return;
                    }
                    db::set_message_status(&dbc, &msg_id, "delivered").ok();
                    dbc.execute("DELETE FROM outbox WHERE msg_id = ?1", params![msg_id])
                        .ok();
                    drop(dbc);
                    let _ = state.app.emit("message-acked", &msg_id);
                }
                Some(sender) => {
                    // 情况 2：中继节点 → 转发 Ack 给原始发送方
                    // sender_id / message_id 保持不变，中继节点不做任何本地状态修改。
                    let _ = try_send(state, &sender, &Message::Ack { msg_id }).await;
                }
                None => {
                    // 查询不到 sender_id（消息不在本地 DB）→ 安全丢弃，不做任何修改。
                }
            }
        }
        Message::ReadReceipt {
            from,
            to,
            last_read_ts,
            last_read_msg_id,
        } => {
            if from != peer_id || to != state.device_id || from == state.device_id {
                return;
            }
            // 优先用 msg_id 换算回「我」的本地时间戳：接收方落库时对时间做过钳制，
            // 直接拿 last_read_ts 在跨设备时钟偏差下会匹配不到我发出的原始消息。
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
            // 对方已读：把「我发给对方、ts ≤ effective_ts」的消息标记为 read（幂等）。
            // 判据只有 `db::mark_own_messages_read_upto` 一个家 —— 这里原先内联的那份
            // 只排除 `read`，会把本端**发送失败**的消息一起点亮成「已读」。
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ =
                    db::mark_own_messages_read_upto(&dbc, &from, &state.device_id, effective_ts);
            }
            // 无论 updated 是 0 还是 >0 都 emit：DB 可能已经是 read，
            // 但前端内存状态可能落后（事件竞态 / 会话重查覆盖），
            // 重新 emit 让 frontend 用 furthestStatus 再校准一次。
            // 这里必须发换算后的 effective_ts，前端才能用同一阈值正确标绿。
            let _ = state.app.emit(
                "peer-read",
                &serde_json::json!({ "peer_id": from, "last_read_ts": effective_ts }),
            );
        }
        // 「群聊与群文件」11 个变体的处理体在 `transport/handle_group.rs::handle_group_messages`
        // （2026-10-07 拆出；分发臂用 `{ .. }` 不绑字段，绑定留在分册里那条同形状的模式上）。
        _ => {}
    }
}
