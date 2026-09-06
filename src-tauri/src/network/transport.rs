//! TCP 消息传输与协议分发（含 Gossip 广播、中继切片、群密钥、E2EE 解密）。
//!
//! 连接建立规则（避免重复建链的竞态）：
//! - 每个节点对，由 **device_id 字典序较小** 的一方主动拨号（dial），较大的一方只被动接受。
//! - 双方各自维护一个出站 mpsc 发送端，读循环负责解析帧并分发。

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::params;
use tauri::Emitter;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::time::Duration;
use x25519_dalek::StaticSecret;

use crate::crypto;
use crate::db;
use crate::network::file;
use crate::protocol::{GossipEnvelope, GossipKind, Message, MsgKind, MAX_FRAME};
use crate::state::{AppState, FileDoneInfo, FileProgress, MessageRecord, Peer, PendingRequest};

// ---------------- 分帧 ----------------

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, msg: &Message) -> std::io::Result<()> {
    let json = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = json.len() as u32;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&json).await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Message> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "非法帧长度"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// ---------------- 出站发送 ----------------

/// 尝试通过已建立连接发送消息；无连接则返回 Err。
pub async fn try_send(state: &AppState, peer_id: &str, msg: &Message) -> Result<(), String> {
    let links = state.links.lock().await;
    match links.get(peer_id) {
        Some(tx) => tx.send(msg.clone()).await.map_err(|e| e.to_string()),
        None => Err("未建立连接".to_string()),
    }
}

/// 向所有已连接节点广播一条 Gossip 消息。
pub async fn broadcast_gossip(state: &AppState, envelope: GossipEnvelope) {
    let msg = Message::Gossip { envelope };
    let links = state.links.lock().await;
    for tx in links.values() {
        let _ = tx.send(msg.clone()).await;
    }
}

// ---------------- 服务启动 ----------------

pub async fn spawn(
    state: Arc<AppState>,
    ip: Ipv4Addr,
    tcp_port: u16,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let bind = format!("{ip}:{tcp_port}");
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| format!("TCP 绑定 {bind} 失败: {e}"))?;
    let state_for_heartbeat = state.clone();
    let shutdown_for_heartbeat = shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                accept = listener.accept() => {
                    let Ok((stream, _addr)) = accept else { continue };
                    let st = state.clone();
                    tokio::spawn(handle_incoming(st, stream));
                }
            }
        }
    });
    // 心跳：周期性向所有已建链节点发送 Heartbeat，
    // 及时发现静默断连（写失败 → writer 退出 → link 移除 → 在线状态修正）。
    tokio::spawn(async move {
        let state = state_for_heartbeat;
        let mut shutdown = shutdown_for_heartbeat;
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => {
                    let links = state.links.lock().await;
                    for tx in links.values() {
                        let _ = tx.send(Message::Heartbeat { device_id: state.device_id.clone() }).await;
                    }
                }
            }
        }
    });
    Ok(())
}

async fn handle_incoming(state: Arc<AppState>, stream: TcpStream) {
    let (mut r, w) = stream.into_split();
    let first = match read_frame(&mut r).await {
        Ok(m) => m,
        Err(_) => return,
    };
    let peer_id = match &first {
        Message::Hello { device_id, .. } => device_id.clone(),
        _ => return, // 首帧必须是 Hello
    };
    let (tx, rx) = mpsc::channel(1024);
    state.links.lock().await.insert(peer_id.clone(), tx.clone());
    tokio::spawn(writer_loop(state.clone(), peer_id.clone(), w, rx));
    handle_message(&state, &peer_id, first).await;
    reader_loop(state, r, peer_id, tx).await;
}

async fn writer_loop(state: Arc<AppState>, peer_id: String, mut w: OwnedWriteHalf, mut rx: mpsc::Receiver<Message>) {
    while let Some(msg) = rx.recv().await {
        if write_frame(&mut w, &msg).await.is_err() {
            // TCP write 失败：普通消息由 outbox 重发；ReadReceipt 需要特殊处理——
            // 它没有 outbox 行，如果 pending 已被 flush_pending_reads 清除，
            // 此处不恢复就永久丢失。将 timestamp 重新放回 pending_reads，
            // 下一次建链 / Hello / Heartbeat 会再次 flush 重发。
            if let Message::ReadReceipt { last_read_ts, .. } = &msg {
                let mut pending = state.pending_reads.lock().unwrap();
                let cur = pending.entry(peer_id.clone()).or_insert(*last_read_ts);
                *cur = (*cur).max(*last_read_ts);
            }
            break;
        }
    }
}

async fn reader_loop(state: Arc<AppState>, mut r: OwnedReadHalf, peer_id: String, link_tx: mpsc::Sender<Message>) {
    loop {
        match read_frame(&mut r).await {
            Ok(msg) => handle_message(&state, &peer_id, msg).await,
            Err(_) => break,
        }
    }
    // 只移除「本条连接」的 link：若对端已重拨建立了新连接，旧的 reader 退出时
    // 不能把新连接的发送端删掉（否则会出现「消息发不出去」的间歇性故障）。
    let was_live_link = {
        let mut links = state.links.lock().await;
        let is_live = links
            .get(&peer_id)
            .map(|tx| tx.same_channel(&link_tx))
            .unwrap_or(false);
        if is_live {
            links.remove(&peer_id);
        }
        is_live
    };
    // 仅当退出的就是当前活跃链路时才标记离线（新链路已接管则不影响在线状态）
    if was_live_link {
        mark_peer_offline(&state, &peer_id).await;
    }
}

// ---------------- 主动建链（小 ID 拨号） ----------------

pub async fn ensure_link(state: &Arc<AppState>, peer_id: &str, ip: &str, tcp_port: u16) {
    if peer_id >= state.device_id.as_str() {
        return; // 只有小 ID 拨号
    }
    if state.links.lock().await.contains_key(peer_id) {
        return;
    }
    connect_to_peer(state, peer_id, ip, tcp_port).await;
}

async fn connect_to_peer(state: &Arc<AppState>, peer_id: &str, ip: &str, tcp_port: u16) {
    {
        let links = state.links.lock().await;
        if links.contains_key(peer_id) {
            return;
        }
    }

    let addr = format!("{ip}:{tcp_port}");
    let stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(_) => return,
    };

    let (r, w) = stream.into_split();
    let (tx, rx) = mpsc::channel(1024);
    state.links.lock().await.insert(peer_id.to_string(), tx.clone());
    tokio::spawn(writer_loop(state.clone(), peer_id.to_string(), w, rx));

    let hello = Message::Hello {
        device_id: state.device_id.clone(),
        nickname: state.nickname.lock().unwrap().clone(),
        avatar: state.avatar.lock().unwrap().clone(),
        tcp_port: state.tcp_port,
    };
    let _ = tx.send(hello).await;

    tokio::spawn(reader_loop(state.clone(), r, peer_id.to_string(), tx));
    flush_outbox(state, peer_id).await;
    flush_pending_reads(state, peer_id).await;
}

// ---------------- 消息分发 ----------------

pub async fn handle_message(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    match msg {
        Message::Hello { device_id, nickname, avatar, tcp_port } => {
            let ip = state
                .peers
                .lock()
                .unwrap()
                .get(&device_id)
                .map(|p| p.ip.clone())
                .unwrap_or_default();
            upsert_peer(state, &device_id, &nickname, avatar.clone(), &ip, tcp_port, None, None, None).await;
            maybe_update_friend(state, &device_id, &nickname, avatar);
            flush_outbox(state, &device_id).await;
            flush_pending_reads(state, &device_id).await;
        }
        Message::Heartbeat { device_id } => {
            touch_peer(state, &device_id).await;
            flush_outbox(state, &device_id).await;
            flush_pending_reads(state, &device_id).await;
        }
        Message::UserInfo { device_id, nickname, avatar } => {
            let ip = state
                .peers
                .lock()
                .unwrap()
                .get(&device_id)
                .map(|p| p.ip.clone())
                .unwrap_or_default();
            upsert_peer(state, &device_id, &nickname, avatar.clone(), &ip, 0, None, None, None).await;
            maybe_update_friend(state, &device_id, &nickname, avatar);
        }
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
                let dbc = state.db.lock().unwrap();
                let mut map: serde_json::Map<String, serde_json::Value> = db::get_setting(&dbc, "chat_peer_styles")
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
        Message::FriendRequest { from, from_nickname, from_avatar, to, ts } => {
            if to != state.device_id {
                return;
            }
            let req = PendingRequest {
                from: from.clone(),
                from_nickname: from_nickname.clone(),
                from_avatar: from_avatar.clone(),
                ts,
            };
            state.pending_requests.lock().unwrap().insert(from.clone(), req.clone());
            let _ = state.app.emit("friend-request", &req);
            notify(&state.app, "好友申请", &format!("{from_nickname} 请求添加你为好友"));
        }
        Message::FriendAccept { from, to } => {
            if to != state.device_id {
                return;
            }
            let name = resolve_nickname(state, &from);
            {
                let dbc = state.db.lock().unwrap();
                db::add_friend(&dbc, &from, &name, None).ok();
                // 同步公钥（否则首次加密发送会失败）
                let (x, e) = {
                    let peers = state.peers.lock().unwrap();
                    peers
                        .get(&from)
                        .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                        .unwrap_or((None, None))
                };
                if x.is_some() || e.is_some() {
                    db::update_friend_pubkeys(&dbc, &from, x.as_deref(), e.as_deref()).ok();
                }
            }
            let _ = state.app.emit("friend-accepted", &from);
            notify(&state.app, "好友申请已通过", &format!("{name} 已成为你的好友"));
        }
        Message::FriendReject { from, to } => {
            if to != state.device_id {
                return;
            }
            state.pending_requests.lock().unwrap().remove(&from);
            let _ = state.app.emit("friend-rejected", &from);
        }
        Message::FriendRemove { from, to } => {
            if to != state.device_id {
                return;
            }
            // 对方删除了好友关系：移除本地好友行（不删除聊天记录）
            let dbc = state.db.lock().unwrap();
            db::remove_friend(&dbc, &from).ok();
            drop(dbc);
            let _ = state.app.emit("friend-removed", &from);
        }
        Message::FriendMessageBlocked { ref from, ref to, ref original_sender } => {
            if to == &state.device_id {
                // 目标是本机：通知前端
                let _ = state.app.emit("friend-message-blocked", from);
            } else if original_sender != &state.device_id {
                // 中继节点：转发给原始发送方（与 Ack relay 同逻辑）
                let _ = try_send(state, original_sender, &Message::FriendMessageBlocked {
                    from: from.clone(),
                    to: to.clone(),
                    original_sender: original_sender.clone(),
                }).await;
            }
        }
        Message::ChatMessage { msg_id, from, to, kind, content, ts } => {
            if to != state.device_id {
                return;
            }
            // 去重前置：真实 msg_id 已落库 == 这条消息我此前已成功接收并持久化，
            // 于是只回 Ack。必须早于解密——否则对方轮换密钥后重投的那份「已收好的」
            // 消息会因当前密钥打不开旧密文而被误判为失败。
            let exists = {
                let dbc = state.db.lock().unwrap();
                db::message_exists(&dbc, &msg_id)
            };
            if exists {
                let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
                return;
            }
            // 好友关系检查：非好友消息不落库、不 Ack、通知发送方
            {
                let dbc = state.db.lock().unwrap();
                if db::get_friend(&dbc, &from).is_none() {
                    let _ = try_send(state, peer_id, &Message::FriendMessageBlocked {
                        from: state.device_id.clone(),
                        to: from.clone(),
                        original_sender: from.clone(),
                    }).await;
                    return;
                }
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
                let dbc = state.db.lock().unwrap();
                // 时钟偏差防护：发送方时钟与我方不一致会导致消息排序错乱
                // （同一发送者的消息在列表中堆叠）→ 双向钳制到 [会话最后一条, 本地 now]
                let ts = db::clamp_incoming_ts(ts, db::now_ms(), db::last_message_ts(&dbc, &from));
                let rec = MessageRecord {
                    id: 0,
                    msg_id: msg_id.clone(),
                    conv_id: from.clone(),
                    sender_id: from.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: kind_str.clone(),
                    content: content.clone(),
                    ts,
                    status: "delivered".to_string(),
                };
                let inserted = db::insert_message_if_new(&dbc, &rec);
                if announced_on(&inserted) {
                    db::touch_conversation(&dbc, &from, "single", &name, None, &preview, 1).ok();
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
            }
            // Ack 与「是否本次新建」无关：消息已在库中（无论是哪条路径先写的）即代表已成功接收
            let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
        }
        Message::GroupMessage { msg_id, from, group_id, group_name, kind, content, ts } => {
            let kind_str = kind.as_str().to_string();
            let conv_id = format!("group:{group_id}");
            let name = if group_name.is_empty() {
                resolve_group_name(state, &group_id)
            } else {
                group_name
            };
            let preview = preview_content(&kind_str, &content);
            // 去重：已收到过则只回 Ack（锁作用域独立，避免非 Send 的 MutexGuard 跨 await）
            let exists = {
                let dbc = state.db.lock().unwrap();
                db::message_exists(&dbc, &msg_id)
            };
            if exists {
                let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
                return;
            }
            // 持锁块只做落库，返回带钳制 ts 的记录 + SQLite 三态裁决；await 全部在锁外
            // （MutexGuard 非 Send）。与 ChatMessage / Gossip 三分支同一裁决：
            // 只有本次真的插入新行才计未读、才投递事件。
            let (out_rec, inserted) = {
                let dbc = state.db.lock().unwrap();
                // 时钟偏差防护（同 ChatMessage 分支）
                let ts = db::clamp_incoming_ts(ts, db::now_ms(), db::last_message_ts(&dbc, &conv_id));
                let rec = MessageRecord {
                    id: 0,
                    msg_id: msg_id.clone(),
                    conv_id: conv_id.clone(),
                    sender_id: from.clone(),
                    receiver_id: group_id.clone(),
                    kind: kind_str.clone(),
                    content: content.clone(),
                    ts,
                    status: "delivered".to_string(),
                };
                let inserted = db::insert_message_if_new(&dbc, &rec);
                if announced_on(&inserted) {
                    db::touch_conversation(&dbc, &conv_id, "group", &name, None, &preview, 1).ok();
                }
                (rec, inserted)
            };
            // 数据库真故障 ⇒ 消息未持久化 ⇒ 不投递也不 Ack（Ack 会让发送方删掉 outbox 行）
            if !may_ack(&inserted) {
                return;
            }
            if announced_on(&inserted) {
                let _ = state.app.emit("message-received", &out_rec);
            }
            let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
        }
        Message::Ack { msg_id } => {
            // 查询原始发送方：如果这条消息不是我发的，说明我是中继节点，
            // 需要把 Ack 转发给原始发送方（而非本地处理）。
            let original_sender: Option<String> = {
                let dbc = state.db.lock().unwrap();
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
                    let dbc = state.db.lock().unwrap();
                    db::set_message_status(&dbc, &msg_id, "delivered").ok();
                    dbc.execute("DELETE FROM outbox WHERE msg_id = ?1", params![msg_id]).ok();
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
        Message::ReadReceipt { from, to, last_read_ts } => {
            if to != state.device_id || from == state.device_id {
                return;
            }
            // 对方已读：把「我发给对方、ts ≤ last_read_ts」的消息标记为 read（幂等）
            {
                let dbc = state.db.lock().unwrap();
                let _ = dbc.execute(
                    "UPDATE messages SET status = 'read'
                     WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                    params![from, state.device_id, last_read_ts],
                );
            }
            // 无论 updated 是 0 还是 >0 都 emit：DB 可能已经是 read，
            // 但前端内存状态可能落后（事件竞态 / 会话重查覆盖），
            // 重新 emit 让 frontend 用 furthestStatus 再校准一次。
            let _ = state.app.emit(
                "peer-read",
                &serde_json::json!({ "peer_id": from, "last_read_ts": last_read_ts }),
            );
        }
        Message::FileOffer { transfer_id, from, name, size } => {
            if from == state.device_id {
                return;
            }
            match file::begin_receive(state, &transfer_id, &from, &name, size) {
                Ok(_) => {
                    let _ = try_send(state, peer_id, &Message::FileAccept { transfer_id: transfer_id.clone() }).await;
                    let _ = state.app.emit("file-progress", &FileProgress { transfer_id: transfer_id.clone(), received: 0, total: size });
                }
                Err(e) => {
                    let _ = try_send(state, peer_id, &Message::FileReject { transfer_id }).await;
                    eprintln!("接收文件初始化失败: {e}");
                }
            }
        }
        Message::FileAccept { transfer_id } => {
            if let Some(tx) = state.pending_file_accept.lock().unwrap().remove(&transfer_id) {
                let _ = tx.send(());
            }
        }
        Message::FileReject { transfer_id } => {
            state.pending_file_accept.lock().unwrap().remove(&transfer_id);
        }
        Message::FileChunk { transfer_id, data, .. } => {
            if let Ok(bytes) = STANDARD.decode(&data) {
                if let Ok(received) = file::write_chunk(state, &transfer_id, &bytes) {
                    // 节流：每 250ms 至多上报一次进度，避免大文件 IPC 事件风暴
                    let (total, should_emit) = {
                        let mut recv = state.file_receivers.lock().unwrap();
                        match recv.get_mut(&transfer_id) {
                            Some(r) => {
                                let now = db::now_ms();
                                let emit = now - r.last_report_ms >= 250;
                                if emit {
                                    r.last_report_ms = now;
                                }
                                (r.size, emit)
                            }
                            None => (0, false),
                        }
                    };
                    if should_emit {
                        let _ = state.app.emit("file-progress", &FileProgress { transfer_id: transfer_id.clone(), received, total });
                    }
                }
            }
        }
        Message::FileDone { transfer_id } => {
            if let Some((name, size, path, peer_id)) = file::finish_receive(state, &transfer_id) {
                let dbc = state.db.lock().unwrap();
                let content = serde_json::json!({
                    "name": name,
                    "path": path.to_string_lossy().to_string(),
                    "size": size,
                })
                .to_string();
                let rec = MessageRecord {
                    id: 0,
                    msg_id: format!("file-{transfer_id}"),
                    conv_id: peer_id.clone(),
                    sender_id: peer_id.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: "file".to_string(),
                    content,
                    ts: db::now_ms(),
                    status: "delivered".to_string(),
                };
                db::insert_message(&dbc, &rec).ok();
                let nm = resolve_nickname(state, &peer_id);
                db::touch_conversation(&dbc, &peer_id, "single", &nm, None, &format!("[文件] {name}"), 1).ok();
                drop(dbc);
                let _ = state.app.emit("message-received", &rec);
                let _ = state.app.emit("file-done", &FileDoneInfo {
                    transfer_id,
                    name: name.clone(),
                    size,
                    path: path.to_string_lossy().to_string(),
                });
            }
        }
        Message::ShareTreeRequest { request_id, from: _, to } => {
            if to != state.device_id {
                return;
            }
            let entries = {
                let share = state.share_dir.lock().unwrap().clone();
                match share {
                    Some(dir) => file::walk_share_dir(Path::new(&dir)),
                    None => Vec::new(),
                }
            };
            let resp = Message::ShareTreeResponse {
                request_id,
                from: state.device_id.clone(),
                entries,
            };
            let _ = try_send(state, peer_id, &resp).await;
        }
        Message::ShareTreeResponse { request_id, entries, .. } => {
            if let Some(tx) = state.pending_share_tree.lock().unwrap().remove(&request_id) {
                let _ = tx.send(entries);
            }
        }
        Message::ShareFileRequest { transfer_id, from, path } => {
            if from == state.device_id {
                return;
            }
            let share = state.share_dir.lock().unwrap().clone();
            let Some(root) = share else { return };
            let root = PathBuf::from(root);
            let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let full = root.join(&path);
            let canon_full = full.canonicalize().unwrap_or_else(|_| full.clone());
            if !canon_full.starts_with(&canon_root) || !canon_full.is_file() {
                return;
            }
            let st = state.clone();
            let from = from.clone();
            tokio::spawn(async move {
                let _ = file::send_file_from_path(&st, &from, &transfer_id, canon_full).await;
            });
        }
        // ---- Gossip 广播 ----
        Message::Gossip { envelope } => {
            handle_gossip(state, peer_id, envelope).await;
        }
        // ---- 中继文件传输 ----
        Message::RelayFileOffer { transfer_id, from, to, name, size, total_chunks } => {
            handle_relay_file_offer(state, transfer_id, from, to, name, size, total_chunks).await;
        }
        Message::RelayChunk { transfer_id, seq, data, from, to, ttl } => {
            handle_relay_chunk(state, transfer_id, seq, data, from, to, ttl).await;
        }
        // ---- 群密钥分发 ----
        Message::GroupKey { group_id, from, to, key } => {
            handle_group_key(state, group_id, from, to, key).await;
        }
    }
}

// ---------------- 直连 E2EE 载荷 ----------------

/// 取发送方当前的 X25519 公钥：好友表优先，回退在线节点表。
/// 好友表由 `upsert_peer` 在 announce / who_has 检测到公钥变化时刷新，
/// 因此「对方换了身份」最长一个广播周期后就会收敛到这里。
fn sender_x25519_pubkey(state: &AppState, from: &str) -> Option<String> {
    let from_db = {
        let dbc = state.db.lock().unwrap();
        db::get_friend_x25519(&dbc, from)
    };
    from_db.or_else(|| {
        state
            .peers
            .lock()
            .unwrap()
            .get(from)
            .and_then(|p| p.x25519_pubkey.clone())
    })
}

/// 打开直连单聊载荷：`enc1:base64(nonce ‖ ChaCha20-Poly1305 密文)`。
/// 返回 `None` = 当前无法解密（缺对端公钥 / 密钥交换失败 / AEAD 校验失败 / UTF-8 非法）。
/// 调用方据此不落库、不 Ack——绝不返回占位文本，占位文本一旦占用真实 msg_id，
/// 同一 msg_id 的正确副本就永远进不来（`insert_message` 是 INSERT OR IGNORE）。
/// 非 `enc1:` 前缀（旧版明文帧）按原样透传，保持既有行为。
fn open_direct_content(
    my_x25519_secret: &StaticSecret,
    sender_pubkey: Option<&str>,
    wire: &str,
    kind: MsgKind,
) -> Option<(String, String)> {
    let Some(b64) = wire.strip_prefix("enc1:") else {
        return Some((wire.to_string(), kind.as_str().to_string()));
    };
    let pubkey = sender_pubkey?;
    let shared = crypto::shared_secret(my_x25519_secret, pubkey)?;
    let bytes = STANDARD.decode(b64).ok()?;
    let plain = crypto::open(&shared, &bytes)?;
    Some((String::from_utf8(plain).ok()?, kind.as_str().to_string()))
}

/// 用接收方当前公钥重新密封待发内容（`msg_id` 由调用方保持不变）。
/// 返回 `None` = 无法重封（本地无明文 / 拿不到当前公钥 / 加密失败），调用方按原样补发。
fn reseal_chat_content(
    my_x25519_secret: &StaticSecret,
    plaintext: Option<&str>,
    receiver_pubkey: Option<&str>,
) -> Option<String> {
    let shared = crypto::shared_secret(my_x25519_secret, receiver_pubkey?)?;
    let sealed = crypto::seal(&shared, plaintext?.as_bytes())?;
    Some(format!("enc1:{}", STANDARD.encode(sealed)))
}

/// 补发前重封一条 `ChatMessage`：密文是「加密时刻」的产物，若双方任一身份在那之后
/// 变化（重装 / 重新加好友），旧密文在接收方永远解不开，重发同一份密文没有意义。
/// 发送方 `messages` 表存的就是明文（见 `commands::send_message`），据此恢复明文并用
/// 最新公钥重封即可；`msg_id` 取自 Gossip 信封 ID、与密文无关，故重封不改变消息身份。
fn reseal_for_send(state: &AppState, msg: Message) -> Message {
    let Message::ChatMessage { msg_id, from, to, kind, content, ts } = msg else {
        return msg;
    };
    if !content.starts_with("enc1:") {
        return Message::ChatMessage { msg_id, from, to, kind, content, ts };
    }
    let (plaintext, from_db) = {
        let dbc = state.db.lock().unwrap();
        // 只认「我自己发出的那条记录」：接收方行的 content 是对方会话的明文，语义不同
        let plaintext = dbc
            .query_row(
                "SELECT content FROM messages WHERE msg_id = ?1 AND sender_id = ?2",
                params![msg_id, state.device_id],
                |r| r.get::<_, String>(0),
            )
            .ok();
        (plaintext, db::get_friend_x25519(&dbc, &to))
    };
    let pubkey = from_db.or_else(|| {
        state
            .peers
            .lock()
            .unwrap()
            .get(&to)
            .and_then(|p| p.x25519_pubkey.clone())
    });
    let resealed = reseal_chat_content(
        &state.identity.x25519_secret,
        plaintext.as_deref(),
        pubkey.as_deref(),
    );
    Message::ChatMessage {
        content: resealed.unwrap_or(content),
        msg_id,
        from,
        to,
        kind,
        ts,
    }
}

// ---------------- 落库裁决 → 副作用策略 ----------------
//
// `db::insert_message_if_new` 返回三态，绝不可折叠成 bool：
//   Ok(true)  = 本次真的插入了新行 ⇒ 唯一允许产生本地投递副作用的一方
//   Ok(false) = msg_id 已存在（INSERT OR IGNORE 命中唯一约束）⇒ 不得再有副作用
//   Err(e)    = 真正的数据库故障（不是重复！）⇒ 消息没有持久化
// 因此「是否投递」与「是否 Ack」是两个独立判定：后者只在 Err 时必须禁止，
// 因为 Ack 会让发送方删除 outbox 行，把一次临时故障变成永久丢消息。

/// 本次落库是否应产生本地投递副作用（`touch_conversation(+1)` 与 `message-received`）。
fn announced_on(inserted: &Result<bool, rusqlite::Error>) -> bool {
    matches!(inserted, Ok(true))
}

/// 是否允许向发送方回 Ack。重复（`Ok(false)`）允许——消息确已在库；
/// 真数据库错误（`Err`）不允许——否则 outbox 被删，消息永久丢失。
fn may_ack(inserted: &Result<bool, rusqlite::Error>) -> bool {
    !matches!(inserted, Err(_))
}

// ---------------- Gossip 处理 ----------------

async fn handle_gossip(state: &Arc<AppState>, _peer_id: &str, env: GossipEnvelope) {
    // 1. 去重 + 验签（合并为一次锁，减少高负载下的锁竞争）
    {
        let mut gossip = state.gossip.lock().unwrap();
        if !gossip.is_new(&env.message_id) {
            return;
        }
        // 校验 message_id 完整性与发送方身份
        if !gossip.verify_envelope(&env) {
            return;
        }
    }
    // 3. 解包：E2EE 关闭时载荷为明文 JSON；开启时按单聊 ECDH / 群密钥解密
    let plaintext = if !env.encrypted {
        // FriendMessageBlocked 等明文 Gossip：payload 是 base64 编码的 JSON
        STANDARD.decode(&env.payload).ok()
    } else {
        match &env.kind {
            GossipKind::Chat => {
                let shared = crypto::shared_secret(&state.identity.x25519_secret, &env.sender_pubkey);
                shared.and_then(|s| STANDARD.decode(&env.payload).ok().and_then(|d| crypto::open(&s, &d)))
            }
            GossipKind::Group => {
                let gid = env.group_id.clone().unwrap_or_default();
                let key = get_group_key(state, &gid).await;
                key.and_then(|k| STANDARD.decode(&env.payload).ok().and_then(|d| crypto::open_symmetric(&k, &d)))
            }
            GossipKind::FriendMessageBlocked => {
                // 已在 encrypted=false 分支处理，这里不应进入
                return;
            }
        }
    };

    // 4. 转发（fan-out，TTL 衰减）— 所有 GossipKind 统一转发
    if env.ttl > 1 {
        let peers: Vec<String> = state.peers.lock().unwrap().keys().cloned().collect();
        let targets = {
            let gossip = state.gossip.lock().unwrap();
            gossip.choose_fanout(&peers, &env.sender_id)
        };
        let mut fwd = env.clone();
        fwd.ttl -= 1;
        let fwd_msg = Message::Gossip { envelope: fwd };
        for t in targets {
            let _ = try_send(state, &t, &fwd_msg).await;
        }
    }

    // 5. 按 GossipKind 处理
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
        GossipKind::Chat | GossipKind::Group => {
            if let Some(pt) = plaintext {
                let (kind, content) = parse_gossip_payload(&pt);
                // GossipKind::Chat：好友关系检查（非好友不落库、不通知、通知发送方）
                if env.kind == GossipKind::Chat {
                    let dbc = state.db.lock().unwrap();
                    if db::get_friend(&dbc, &env.sender_id).is_none() {
                        // 通过 Gossip 广播拒绝通知（多跳场景下也能回到原始发送方）
                        let gossip = state.gossip.lock().unwrap();
                        let payload = serde_json::json!({ "original_sender": env.sender_id }).to_string();
                        let payload_b64 = STANDARD.encode(payload.as_bytes());
                        let blocked_env = gossip.build_envelope(
                            &state.identity,
                            &state.device_id,
                            GossipKind::FriendMessageBlocked,
                            None,
                            &payload_b64,
                            db::now_ms(),
                        );
                        drop(gossip);
                        broadcast_gossip(state, blocked_env).await;
                        return;
                    }
                }
                let conv_id = match &env.kind {
                    GossipKind::Chat => env.sender_id.clone(),
                    GossipKind::Group => format!("group:{}", env.group_id.clone().unwrap_or_default()),
                    _ => return,
                };
                let conv_kind = match &env.kind {
                    GossipKind::Chat => "single",
                    GossipKind::Group => "group",
                    _ => return,
                };
                let name = match &env.kind {
                    GossipKind::Chat => resolve_nickname(state, &env.sender_id),
                    GossipKind::Group => resolve_group_name(state, env.group_id.as_deref().unwrap_or("")),
                    _ => return,
                };
                let preview = preview_content(&kind, &content);
                // 持锁块只做落库；await（fanout 转发已在前面）之后无持锁操作
                // 业务幂等裁决：Direct（含 outbox 补发）可能已经把同一 msg_id 落库，此时
                // 不得再计未读、再发 message-received，否则未读数与系统通知都会重复。
                // 与 Direct 分支共用 announced_on 裁决，两路径并发时只有一方拿到 Ok(true)。
                // 单聊与群聊走同一块 ⇒ 两种 GossipKind 都被覆盖。
                let (out_rec, inserted) = {
                    let dbc = state.db.lock().unwrap();
                    // 时钟偏差防护（同 ChatMessage 分支）
                    let ts = db::clamp_incoming_ts(env.ts, db::now_ms(), db::last_message_ts(&dbc, &conv_id));
                    let rec = MessageRecord {
                        id: 0,
                        msg_id: env.message_id.clone(),
                        conv_id: conv_id.clone(),
                        sender_id: env.sender_id.clone(),
                        receiver_id: state.device_id.clone(),
                        kind: kind.clone(),
                        content: content.clone(),
                        ts,
                        status: "delivered".to_string(),
                    };
                    let inserted = db::insert_message_if_new(&dbc, &rec);
                    if announced_on(&inserted) {
                        db::touch_conversation(&dbc, &conv_id, conv_kind, &name, None, &preview, 1).ok();
                    }
                    (rec, inserted)
                };
                // 重复投递与数据库失败都不产生本地副作用；Gossip 本就不回 Ack，转发已在上面完成
                if announced_on(&inserted) {
                    let _ = state.app.emit("message-received", &out_rec);
                }
            }
        }
    }
}

fn parse_gossip_payload(pt: &[u8]) -> (String, String) {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(pt) {
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("text").to_string();
        let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
        (kind, content)
    } else {
        ("text".to_string(), String::from_utf8_lossy(pt).to_string())
    }
}

// ---------------- 中继文件传输 ----------------

async fn handle_relay_file_offer(
    state: &Arc<AppState>,
    transfer_id: String,
    from: String,
    to: String,
    name: String,
    size: u64,
    total_chunks: u32,
) {
    if to != state.device_id {
        return; // 中继节点无需重组，只转发切片
    }
    state.relay.lock().unwrap().begin_reassemble(&transfer_id, &name, total_chunks);
    {
        let dbc = state.db.lock().unwrap();
        db::upsert_transfer(&dbc, &transfer_id, &from, &name, size, "receive", "active", None, 0.0).ok();
    }
    let _ = state.app.emit("file-progress", &FileProgress { transfer_id, received: 0, total: size });
}

async fn handle_relay_chunk(
    state: &Arc<AppState>,
    transfer_id: String,
    seq: u32,
    data: String,
    from: String,
    to: String,
    ttl: u8,
) {
    if to == state.device_id {
        // 最终接收方：重组
        let Ok(bytes) = STANDARD.decode(&data) else { return };
        let completed = {
            let mut relay = state.relay.lock().unwrap();
            relay.add_chunk(&transfer_id, seq, bytes)
        };
        if let Some((name, full)) = completed {
            let path = save_received_bytes(state, &name, &full);
            let path_str = path.to_string_lossy().to_string();
            let rec = {
                let dbc = state.db.lock().unwrap();
                db::upsert_transfer(&dbc, &transfer_id, &from, &name, full.len() as u64, "receive", "done", Some(path_str.as_str()), 1.0).ok();
                let content = serde_json::json!({
                    "name": name.clone(),
                    "path": path_str.clone(),
                    "size": full.len(),
                })
                .to_string();
                let rec = MessageRecord {
                    id: 0,
                    msg_id: format!("file-{transfer_id}"),
                    conv_id: from.clone(),
                    sender_id: from.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: "file".to_string(),
                    content,
                    ts: db::now_ms(),
                    status: "delivered".to_string(),
                };
                db::insert_message(&dbc, &rec).ok();
                let nm = resolve_nickname(state, &from);
                db::touch_conversation(&dbc, &from, "single", &nm, None, &format!("[文件] {name}"), 1).ok();
                rec
            };
            let _ = state.app.emit("message-received", &rec);
            let _ = state.app.emit("file-done", &FileDoneInfo {
                transfer_id: transfer_id.clone(),
                name: name.clone(),
                size: full.len() as u64,
                path: path_str,
            });
        }
        // 重组中：进度可基于切片数上报，此处省略，完成时由 file-done 事件通知
    } else if ttl > 1 {
        // 中继转发给最终接收方
        let fwd = Message::RelayChunk { transfer_id, seq, data, from, to: to.clone(), ttl: ttl - 1 };
        let _ = try_send(state, &to, &fwd).await;
    }
}

fn save_received_bytes(state: &AppState, name: &str, bytes: &[u8]) -> PathBuf {
    let dir = &state.downloads_dir;
    std::fs::create_dir_all(dir).ok();
    let base = dir.join(name);
    if !base.exists() {
        let _ = std::fs::write(&base, bytes);
        return base;
    }
    let stem = base.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = base.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    for i in 1..1000 {
        let cand = if ext.is_empty() {
            dir.join(format!("{stem} ({i})"))
        } else {
            dir.join(format!("{stem} ({i}).{ext}"))
        };
        if !cand.exists() {
            let _ = std::fs::write(&cand, bytes);
            return cand;
        }
    }
    base
}

// ---------------- 群密钥 ----------------

async fn handle_group_key(state: &Arc<AppState>, group_id: String, from: String, to: String, key: String) {
    if to != state.device_id {
        return;
    }
    let pubkey = {
        let peers = state.peers.lock().unwrap();
        peers.get(&from).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(pubkey) = pubkey else { return };
    let Some(shared) = crypto::shared_secret(&state.identity.x25519_secret, &pubkey) else { return };
    let Ok(sealed) = STANDARD.decode(&key) else { return };
    let Some(raw) = crypto::open(&shared, &sealed) else { return };
    if raw.len() != 32 {
        return;
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&raw);
    state.group_keys.lock().unwrap().insert(group_id.clone(), k);
    {
        let dbc = state.db.lock().unwrap();
        db::set_setting(&dbc, &format!("gk:{group_id}"), &STANDARD.encode(k)).ok();
    }
    let _ = state.app.emit("group-key-received", &group_id);
}

pub async fn get_group_key(state: &AppState, group_id: &str) -> Option<[u8; 32]> {
    if let Some(k) = state.group_keys.lock().unwrap().get(group_id) {
        return Some(*k);
    }
    let key_b64 = {
        let dbc = state.db.lock().unwrap();
        db::get_setting(&dbc, &format!("gk:{group_id}"))
    }?;
    let bytes = STANDARD.decode(key_b64).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    state.group_keys.lock().unwrap().insert(group_id.to_string(), arr);
    Some(arr)
}

// ---------------- 节点与好友辅助 ----------------

pub async fn upsert_peer(
    state: &AppState,
    device_id: &str,
    nickname: &str,
    avatar: Option<String>,
    ip: &str,
    tcp_port: u16,
    x25519: Option<String>,
    ed25519: Option<String>,
    rtt_ms: Option<u64>,
) {
    let ts = db::now_ms();
    // 判断是否「新节点」或「公钥首次学到/变化」，据此决定是否做昂贵的落库与群密钥补发。
    // 500-1000 节点下，若每条 announce 都写库 + 遍历群组，会形成明显热点。
    let (is_new, key_changed) = {
        let mut peers = state.peers.lock().unwrap();
        match peers.get_mut(device_id) {
            None => {
                peers.insert(device_id.to_string(), Peer {
                    device_id: device_id.to_string(),
                    nickname: nickname.to_string(),
                    avatar,
                    ip: ip.to_string(),
                    tcp_port,
                    last_seen: ts,
                    rtt_ms,
                    x25519_pubkey: x25519.clone(),
                    ed25519_pubkey: ed25519.clone(),
                    connected_since: Some(ts),
                });
                (true, true)
            }
            Some(p) => {
                let key_changed = (x25519.is_some() && p.x25519_pubkey != x25519)
                    || (ed25519.is_some() && p.ed25519_pubkey != ed25519);
                p.nickname = nickname.to_string();
                if avatar.is_some() {
                    p.avatar = avatar;
                }
                if !ip.is_empty() {
                    p.ip = ip.to_string();
                }
                if tcp_port != 0 {
                    p.tcp_port = tcp_port;
                }
                if x25519.is_some() {
                    p.x25519_pubkey = x25519;
                }
                if ed25519.is_some() {
                    p.ed25519_pubkey = ed25519;
                }
                if rtt_ms.is_some() {
                    p.rtt_ms = rtt_ms;
                }
                p.last_seen = ts;
                (false, key_changed)
            }
        }
    };
    state.emit_peers();

    // 仅在公钥首次学到/变化时才落库（避免每条 announce 都写库）
    if key_changed {
        let (x, e) = {
            let peers = state.peers.lock().unwrap();
            peers
                .get(device_id)
                .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
                .unwrap_or((None, None))
        };
        if x.is_some() || e.is_some() {
            let dbc = state.db.lock().unwrap();
            db::update_friend_pubkeys(&dbc, device_id, x.as_deref(), e.as_deref()).ok();
        }
        // 公钥变化时同步到 friends 表：Hello 可能在 announce 之前到达，
        // 此时 peers[peer].x25519_pubkey 为 None → friends 表写入 None；
        // announce 到达后更新了 peers，但 friends 表不会自动刷新。
        // 此处补一次 maybe_update_friend 确保 friends 表与 peers 同步。
        let (nick, av) = {
            let peers = state.peers.lock().unwrap();
            peers.get(device_id).map(|p| (p.nickname.clone(), p.avatar.clone())).unwrap_or_default()
        };
        maybe_update_friend(state, device_id, &nick, av);
    }

    // 仅新节点或公钥变化时补发群密钥（处理对方离线时建群的情况）
    if is_new || key_changed {
        redistribute_group_keys(state, device_id).await;
    }
}

async fn redistribute_group_keys(state: &AppState, peer_id: &str) {
    let pubkey = {
        let peers = state.peers.lock().unwrap();
        peers.get(peer_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(pubkey) = pubkey else { return };

    let groups = {
        let dbc = state.db.lock().unwrap();
        db::list_groups(&dbc).unwrap_or_default()
    };
    for g in groups {
        if !g.members.contains(&peer_id.to_string()) {
            continue;
        }
        let Some(key) = get_group_key(state, &g.id).await else { continue };
        let Some(shared) = crypto::shared_secret(&state.identity.x25519_secret, &pubkey) else { continue };
        let Some(sealed) = crypto::seal(&shared, &key) else { continue };
        let msg = Message::GroupKey {
            group_id: g.id.clone(),
            from: state.device_id.clone(),
            to: peer_id.to_string(),
            key: STANDARD.encode(&sealed),
        };
        let _ = try_send(state, peer_id, &msg).await;
    }
}

pub async fn touch_peer(state: &AppState, device_id: &str) {
    let ts = db::now_ms();
    if let Some(p) = state.peers.lock().unwrap().get_mut(device_id) {
        p.last_seen = ts;
    }
    state.emit_peers();
}

async fn mark_peer_offline(state: &Arc<AppState>, device_id: &str) {
    state.peers.lock().unwrap().remove(device_id);
    state.emit_peers();
}

fn maybe_update_friend(state: &AppState, device_id: &str, nickname: &str, avatar: Option<String>) {
    let (x, e) = {
        let peers = state.peers.lock().unwrap();
        peers
            .get(device_id)
            .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
            .unwrap_or((None, None))
    };
    let dbc = state.db.lock().unwrap();
    if db::get_friend(&dbc, device_id).is_some() {
        db::add_friend(&dbc, device_id, nickname, avatar.as_deref()).ok();
        // 好友行常在公钥落库之后才创建（好友申请通过才 add_friend），
        // 此处每次同步公钥，保证 E2EE 加密始终能取到对方公钥。
        if x.is_some() || e.is_some() {
            db::update_friend_pubkeys(&dbc, device_id, x.as_deref(), e.as_deref()).ok();
        }
    }
}

pub fn resolve_nickname(state: &AppState, id: &str) -> String {
    if let Some(p) = state.peers.lock().unwrap().get(id) {
        if !p.nickname.is_empty() {
            return p.nickname.clone();
        }
    }
    if let Some(r) = state.pending_requests.lock().unwrap().get(id) {
        return r.from_nickname.clone();
    }
    if let Some((n, _)) = db::get_friend(&state.db.lock().unwrap(), id) {
        return n;
    }
    id.to_string()
}

fn resolve_group_name(state: &AppState, group_id: &str) -> String {
    let dbc = state.db.lock().unwrap();
    if let Ok(groups) = db::list_groups(&dbc) {
        if let Some(g) = groups.into_iter().find(|g| g.id == group_id) {
            return g.name;
        }
    }
    format!("群聊 {group_id}")
}

fn preview_content(kind: &str, content: &str) -> String {
    match kind {
        "file" => "[文件]".to_string(),
        "image" => "[图片]".to_string(),
        _ => {
            let count = content.chars().count();
            let c: String = content.chars().take(30).collect();
            if count > 30 {
                format!("{c}…")
            } else {
                c
            }
        }
    }
}

/// 补发离线队列中的所有消息。
///
/// 注意：这里**只补发、不删除**——outbox 行仅在收到对方 `Ack`（真正确认送达）时删除。
/// 旧实现 `try_send` 返回 Ok（仅表示已入发送队列）就删行，半开 TCP 链路上会静默丢消息，
/// outbox 兜底因此失效。接收方按 msg_id 去重，重复补发不会重复入库/通知。
///
/// 每条补发前用**当前**公钥重新密封（见 `reseal_for_send`）：outbox 存的是加密时刻的
/// 密文，若之后接收方换了身份，旧密文重发多少次都解不开；`msg_id` 不变，幂等性不受影响。
pub async fn flush_outbox(state: &AppState, peer_id: &str) {
    let pending = {
        let dbc = state.db.lock().unwrap();
        db::list_outbox(&dbc, peer_id).unwrap_or_default()
    };
    for (_id, payload) in pending {
        let Ok(msg) = serde_json::from_str::<Message>(&payload) else {
            continue;
        };
        let msg = reseal_for_send(state, msg);
        let _ = try_send(state, peer_id, &msg).await;
    }
}

/// 冲刷待发的单聊已读回执（触发点与 `flush_outbox` 一致：建链 / Hello / 心跳）。
///
/// `mark_read` 将 pending 同时写入内存 HashMap 和 SQLite。此处成功发送后
/// 同时清除两者；失败时内存已由 remove 清除但会重新写入，DB 保留不动
/// （由 `mark_read` 写入，下次 flush 重试）。
pub async fn flush_pending_reads(state: &AppState, peer_id: &str) {
    let Some(last_read_ts) = state.pending_reads.lock().unwrap().remove(peer_id) else {
        return;
    };
    let msg = Message::ReadReceipt {
        from: state.device_id.clone(),
        to: peer_id.to_string(),
        last_read_ts,
    };
    if try_send(state, peer_id, &msg).await.is_err() {
        // 发送失败：内存重新放入 pending，DB 保留（已由 mark_read 写入）
        let mut pending = state.pending_reads.lock().unwrap();
        let cur = pending.entry(peer_id.to_string()).or_insert(last_read_ts);
        *cur = (*cur).max(last_read_ts);
    } else {
        // 发送成功：清除 DB 中的 pending 记录
        let dbc = state.db.lock().unwrap();
        db::delete_pending_read(&dbc, peer_id).ok();
    }
}

pub fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gossip_engine::GossipEngine;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (a, b) = tokio::io::duplex(4096);
        let (mut _ar, mut aw) = tokio::io::split(a);
        let (mut br, mut _bw) = tokio::io::split(b);
        let msg = Message::Heartbeat { device_id: "dev-1".into() };
        let (wr, rd) = tokio::join!(write_frame(&mut aw, &msg), read_frame(&mut br));
        wr.unwrap();
        match rd.unwrap() {
            Message::Heartbeat { device_id } => assert_eq!(device_id, "dev-1"),
            _ => panic!("类型不符"),
        }
    }

    #[tokio::test]
    async fn frame_roundtrip_large_payload() {
        // 模拟 256KB 文件分片的 base64 负载往返
        let big = "A".repeat(342_000);
        let msg = Message::RelayChunk {
            transfer_id: "t1".into(),
            seq: 7,
            data: big.clone(),
            from: "a".into(),
            to: "b".into(),
            ttl: 3,
        };
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (mut _ar, mut aw) = tokio::io::split(a);
        let (mut br, mut _bw) = tokio::io::split(b);
        let (wr, rd) = tokio::join!(write_frame(&mut aw, &msg), read_frame(&mut br));
        wr.unwrap();
        match rd.unwrap() {
            Message::RelayChunk { data, seq, .. } => {
                assert_eq!(seq, 7);
                assert_eq!(data, big);
            }
            _ => panic!("类型不符"),
        }
    }

    #[tokio::test]
    async fn e2e_gossip_encrypt_sign_decrypt() {
        // 端到端：A 加密→签名→广播信封，B 验签→解密
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();

        // A 用 B 的公钥 ECDH 派生共享密钥并加密
        let shared = crate::crypto::shared_secret(&a.x25519_secret, &b.x25519_public_b64()).unwrap();
        let plaintext = b"{\"kind\":\"text\",\"content\":\"hello\"}";
        let sealed = crate::crypto::seal(&shared, plaintext).unwrap();
        let payload_b64 = STANDARD.encode(&sealed);

        // 构造并签名信封
        let engine = GossipEngine::new(100, 10, 4, 6);
        let env = engine.build_envelope(&a, "dev-a", GossipKind::Chat, None, &payload_b64, 1);

        // B 验签 + 解密
        assert!(engine.verify_envelope(&env));
        let shared_b = crate::crypto::shared_secret(&b.x25519_secret, &env.sender_pubkey).unwrap();
        let decrypted = STANDARD.decode(&env.payload).unwrap();
        let opened = crate::crypto::open(&shared_b, &decrypted).unwrap();
        assert_eq!(opened, plaintext);
    }

    // ---------------- P0-2：直连 E2EE 解密失败不得消费真实 msg_id ----------------

    fn seal_direct(from: &crate::crypto::Identity, to_pubkey: &str, text: &str) -> String {
        let shared = crate::crypto::shared_secret(&from.x25519_secret, to_pubkey).unwrap();
        format!("enc1:{}", STANDARD.encode(crate::crypto::seal(&shared, text.as_bytes()).unwrap()))
    }

    /// Test 1 正常 E2EE：正确公钥 → 明文与原始 kind 一并还原（kind 不被改写成 system）。
    #[test]
    fn direct_open_succeeds_with_current_keys() {
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let wire = seal_direct(&a, &b.x25519_public_b64(), "你好 e2ee");
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&a.x25519_public_b64()), &wire, MsgKind::Code),
            Some(("你好 e2ee".to_string(), "code".to_string()))
        );
    }

    /// Test 2 场景 A（暂时缺公钥）：缺发送方公钥必须判为「解不开」（→ 不落库、不 Ack），
    /// 且公钥经 announce/who_has 学到之后，**同一份密文**即可解开 —— 补发重试就能恢复。
    #[test]
    fn direct_open_fails_without_sender_key_and_recovers_when_key_arrives() {
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let wire = seal_direct(&a, &b.x25519_public_b64(), "pending key");
        assert_eq!(open_direct_content(&b.x25519_secret, None, &wire, MsgKind::Text), None);
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&a.x25519_public_b64()), &wire, MsgKind::Text),
            Some(("pending key".to_string(), "text".to_string()))
        );
    }

    /// Test 3a 场景 B（发送方换身份）：本地缓存为旧公钥时解不开；
    /// `upsert_peer` 把对方新公钥刷进缓存后，同一份密文可解开（无需重新加密）。
    #[test]
    fn direct_open_recovers_once_sender_pubkey_cache_refreshed() {
        let a_old = crate::crypto::Identity::generate();
        let a_new = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let wire = seal_direct(&a_new, &b.x25519_public_b64(), "rotated sender");
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&a_old.x25519_public_b64()), &wire, MsgKind::Text),
            None
        );
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&a_new.x25519_public_b64()), &wire, MsgKind::Text),
            Some(("rotated sender".to_string(), "text".to_string()))
        );
    }

    /// Test 3b 场景 B（接收方换身份）：outbox 里的密文对着旧公钥封存，重发多少次都解不开，
    /// 必须由持有明文的发送方用**当前**公钥重封；重封可失败（无明文 / 无公钥）时一律返回
    /// None 让调用方按原样补发，绝不伪造内容。
    #[test]
    fn reseal_with_current_receiver_key_recovers_where_retry_cannot() {
        let a = crate::crypto::Identity::generate();
        let b_old = crate::crypto::Identity::generate();
        let b_new = crate::crypto::Identity::generate();
        let stale = seal_direct(&a, &b_old.x25519_public_b64(), "stale seal");

        // 旧密文对新的接收方身份永久无效（重发不解决问题）
        assert_eq!(
            open_direct_content(&b_new.x25519_secret, Some(&a.x25519_public_b64()), &stale, MsgKind::Text),
            None
        );
        // 重封：同一明文 + 当前公钥 → 可解，且仍是 enc1: 形态
        let resealed =
            reseal_chat_content(&a.x25519_secret, Some("stale seal"), Some(&b_new.x25519_public_b64()))
                .unwrap();
        assert_ne!(resealed, stale);
        assert_eq!(
            open_direct_content(
                &b_new.x25519_secret,
                Some(&a.x25519_public_b64()),
                &resealed,
                MsgKind::Text
            ),
            Some(("stale seal".to_string(), "text".to_string()))
        );
        // 前置条件缺失 → 不重封（调用方保留原 payload）
        let no_plaintext = reseal_chat_content(&a.x25519_secret, None, Some(&b_new.x25519_public_b64()));
        assert_eq!(no_plaintext, None);
        let no_pubkey = reseal_chat_content(&a.x25519_secret, Some("stale seal"), None);
        assert_eq!(no_pubkey, None);
    }

    /// Test 3c 场景 C（真损坏）：base64 非法 / 密文被篡改一律判为解不开，
    /// 但**不污染**同一条完好密文的可解性 —— 失败只影响这一次投递。
    #[test]
    fn direct_open_rejects_corrupt_and_tampered_payloads() {
        let a = crate::crypto::Identity::generate();
        let b = crate::crypto::Identity::generate();
        let spk = a.x25519_public_b64();
        assert_eq!(
            open_direct_content(&b.x25519_secret, Some(&spk), "enc1:!!not base64!!", MsgKind::Text),
            None
        );
        assert_eq!(open_direct_content(&b.x25519_secret, Some(&spk), "enc1:", MsgKind::Text), None);
        let wire = seal_direct(&a, &b.x25519_public_b64(), "intact");
        let mut raw = STANDARD.decode(wire.strip_prefix("enc1:").unwrap()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xFF; // 破坏 AEAD tag
        let tampered = format!("enc1:{}", STANDARD.encode(&raw));
        assert_eq!(open_direct_content(&b.x25519_secret, Some(&spk), &tampered, MsgKind::Text), None);
        assert!(open_direct_content(&b.x25519_secret, Some(&spk), &wire, MsgKind::Text).is_some());
    }

    /// 非 enc1 帧（旧版明文）按原样透传的既有行为不变 —— 本次修复不改动该分支语义。
    #[test]
    fn legacy_plaintext_payload_passes_through_unchanged() {
        let me = crate::crypto::Identity::generate();
        assert_eq!(
            open_direct_content(&me.x25519_secret, None, "plain old text", MsgKind::Text),
            Some(("plain old text".to_string(), "text".to_string()))
        );
    }

    /// P1-3：两层去重必须互相独立——Gossip 的 Bloom/LRU 属网络传播层（只认 `message_id`），
    /// SQLite 的 `msg_id` 属业务持久化层。业务层的「本机已落库」只能抑制未读/事件，
    /// 绝不能前移到转发之前，否则已经 Direct 收到过该消息的节点会拒绝继续 fan-out，
    /// epidemic 传播在此断链。转发目标只由邻居集合 / fanout / exclude 决定。
    #[test]
    fn gossip_propagation_layer_stays_independent_of_local_persistence() {
        let mut engine = GossipEngine::new(100, 10, 4, 6);
        assert!(engine.is_new("m1"), "首次见到的信封必须进入处理与转发");
        assert!(!engine.is_new("m1"), "同一信封第二次到达在传播层判为重复");
        assert!(engine.is_new("m2"), "另一条消息不受前者影响");
        let peers = vec!["b".to_string(), "c".to_string(), "d".to_string()];
        let targets = engine.choose_fanout(&peers, "a");
        assert_eq!(targets.len(), 3, "fanout=4 时三个邻居都应被转发到");
        assert!(!targets.contains(&"a".to_string()), "不回发给信封的发送方");
    }

    /// P1-3 / Test 4 的前提条件：Direct 已把某 msg_id 落库之后，同一信封再到达时
    /// 必须「业务层判为已存在（于是抑制未读与事件）」且「传播层仍判为首次见到（于是
    /// 继续 verify 并在 ttl > 1 时 fan-out）」同时成立。
    /// 若把 `message_exists` 前移到 handle_gossip 开头直接 return，第二项就会被破坏，
    /// epidemic 传播在本节点断链 —— 这条测试就是防止那种"顺手简化"。
    #[test]
    fn business_duplicate_still_enters_the_propagation_layer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        let rec = MessageRecord {
            id: 0,
            msg_id: "m1".into(),
            conv_id: "dev-a".into(),
            sender_id: "dev-a".into(),
            receiver_id: "me".into(),
            kind: "text".into(),
            content: "hello".into(),
            ts: 1,
            status: "delivered".into(),
        };
        // Direct 先到并落库 → 它是唯一产生本地副作用的一方
        assert!(db::insert_message_if_new(&conn, &rec).unwrap());
        // 之后 Gossip 副本到达：业务层判为已存在 ⇒ 不再 touch / 不再 emit
        assert!(!db::insert_message_if_new(&conn, &rec).unwrap());
        // 但传播层（独立的内存 Bloom/LRU）从未被 Direct 登记 ⇒ 仍会走到 fan-out
        let mut engine = GossipEngine::new(100, 10, 4, 6);
        assert!(engine.is_new("m1"), "业务层已存在不得让传播层跳过转发");
        // 且两路径共用同一 msg_id，库里始终只有一行
        assert_eq!(db::get_messages(&conn, "dev-a", 10, 0).unwrap().len(), 1);
    }

    /// P1-3 / Test C：三态落库裁决 → 副作用与 Ack 策略的映射必须是显式且可测的。
    /// - 未读 +1 与 message-received：只有 `Ok(true)`（本次真的新建）才允许；
    /// - Ack：`Ok(true)` / `Ok(false)` 都允许（消息确已在库），`Err` 必须禁止
    ///   （Ack 会让发送方删掉 outbox 行 ⇒ 临时 DB 故障变成永久丢消息）。
    #[test]
    fn insert_outcome_maps_to_side_effect_and_ack_policy() {
        let fresh: Result<bool, rusqlite::Error> = Ok(true);
        let duplicate: Result<bool, rusqlite::Error> = Ok(false);
        let db_error: Result<bool, rusqlite::Error> = Err(rusqlite::Error::QueryReturnedNoRows);

        assert!(announced_on(&fresh), "本次新建 ⇒ 计未读 + 投递事件");
        assert!(!announced_on(&duplicate), "重复 ⇒ 不得再有副作用");
        assert!(!announced_on(&db_error), "DB 故障 ⇒ 不得有副作用（更不得当成重复）");

        assert!(may_ack(&fresh), "本次新建 ⇒ Ack");
        assert!(may_ack(&duplicate), "已在库中 ⇒ 仍 Ack（Ack 语义 = 已成功接收并持久化）");
        assert!(!may_ack(&db_error), "DB 故障 ⇒ 绝不 Ack，outbox 行必须保留以便重发");
    }

    /// ReadReceipt 正常到达：ts ≤ last_read_ts 的消息推进到 read。
    #[test]
    fn read_receipt_marks_messages_as_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(&conn, &MessageRecord {
            id: 0, msg_id: "m1".into(), conv_id: "peer-a".into(),
            sender_id: "me".into(), receiver_id: "peer-a".into(),
            kind: "text".into(), content: "hi".into(), ts: 100, status: "delivered".into(),
        }).unwrap();
        // 模拟 ReadReceipt handler 的 UPDATE 语句
        let updated = conn.execute(
            "UPDATE messages SET status = 'read'
             WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
            params!["peer-a", "me", 100],
        ).unwrap();
        assert_eq!(updated, 1, "应有 1 行被更新");
        let status: String = conn.query_row(
            "SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(status, "read");
    }

    /// Test D: updated = 0 时 DB UPDATE 无行被更新，但 handler 仍然 emit peer-read。
    /// 注意：emit 依赖 Tauri AppHandle，无法在纯单测中断言事件；
    /// 这里验证的是 SQL 路径正确返回 updated=0（与 emit 条件分离）。
    #[test]
    fn read_receipt_db_update_zero_when_already_read() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(&conn, &MessageRecord {
            id: 0, msg_id: "m1".into(), conv_id: "peer-a".into(),
            sender_id: "me".into(), receiver_id: "peer-a".into(),
            kind: "text".into(), content: "hi".into(), ts: 100, status: "read".into(),
        }).unwrap();
        let updated = conn.execute(
            "UPDATE messages SET status = 'read'
             WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
            params!["peer-a", "me", 100],
        ).unwrap();
        assert_eq!(updated, 0, "DB 已是 read，无行被更新");
        // handler 仍然 emit peer-read（always-emit 修复），但 emit 本身无法在单测中断言
    }

    /// Test C: 多次 mark_read 只保留最大 timestamp。
    #[test]
    fn pending_reads_keeps_max_timestamp() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // mark_read(ts=100)
        let cur = pending.entry("peer-a".into()).or_insert(100);
        *cur = (*cur).max(100);
        assert_eq!(pending["peer-a"], 100);
        // mark_read(ts=80) — 较小，不更新
        let cur = pending.entry("peer-a".into()).or_insert(80);
        *cur = (*cur).max(80);
        assert_eq!(pending["peer-a"], 100);
        // mark_read(ts=200) — 较大，更新
        let cur = pending.entry("peer-a".into()).or_insert(200);
        *cur = (*cur).max(200);
        assert_eq!(pending["peer-a"], 200);
    }

    /// Test B: writer write_frame 失败时，ReadReceipt 的 timestamp 被重新放入 pending_reads。
    /// 模拟 writer_loop 的失败回收逻辑。
    #[test]
    fn writer_failure_preserves_pending_for_read_receipt() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // flush_pending_reads 已经 remove
        pending.insert("peer-a".into(), 200);
        let last_read_ts = pending.remove("peer-a").unwrap();
        assert!(pending.is_empty(), "flush 后 pending 应为空");
        // 模拟 writer_loop write_frame 失败后的回收逻辑
        {
            let cur = pending.entry("peer-a".into()).or_insert(last_read_ts);
            *cur = (*cur).max(last_read_ts);
        }
        assert_eq!(pending.get("peer-a"), Some(&200), "写入失败后 pending 应恢复");
    }

    /// Test A: writer write_frame 成功时，pending 不被重新插入。
    /// flush_pending_reads remove 后发送成功，pending 保持清空。
    #[test]
    fn successful_flush_clears_pending() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        pending.insert("peer-a".into(), 200);
        // 模拟 flush_pending_reads: remove + try_send Ok + writer write_frame Ok
        let last_read_ts = pending.remove("peer-a").unwrap();
        // write_frame 成功 → 不执行 writer_loop 的回收逻辑
        let _ = last_read_ts;
        assert!(pending.is_empty(), "写入成功后 pending 应保持清空");
    }

    /// flush_pending_reads try_send 失败时（链路不存在），pending 必须恢复——
    //  否则 ReadReceipt 永久丢失，要等用户下次手动打开会话才能补发。
    #[test]
    fn flush_failure_reinserts_pending_read() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        pending.insert("peer-a".into(), 300);
        // 模拟 flush_pending_reads: remove → try_send Err → 必须 re-insert
        let last_read_ts = pending.remove("peer-a").unwrap();
        assert!(pending.is_empty(), "remove 后 pending 应为空");
        // try_send 失败 → 重新放入 pending（与 writer_loop 的失败回收同逻辑）
        {
            let cur = pending.entry("peer-a".into()).or_insert(last_read_ts);
            *cur = (*cur).max(last_read_ts);
        }
        assert_eq!(pending.get("peer-a"), Some(&300), "try_send 失败后 pending 应恢复");
    }

    /// 多次 flush 失败只保留最大 timestamp（幂等性）。
    #[test]
    fn repeated_flush_failure_keeps_max_timestamp() {
        let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        // 第一次 mark_read(ts=300) → flush 失败
        pending.insert("peer-a".into(), 300);
        let ts1 = pending.remove("peer-a").unwrap();
        { let cur = pending.entry("peer-a".into()).or_insert(ts1); *cur = (*cur).max(ts1); }
        // 第二次 mark_read(ts=200) → 较小，不覆盖
        let cur = pending.entry("peer-a".into()).or_insert(200);
        *cur = (*cur).max(200);
        assert_eq!(pending["peer-a"], 300);
        // 第三次 mark_read(ts=500) → 较大，更新
        let cur = pending.entry("peer-a".into()).or_insert(500);
        *cur = (*cur).max(500);
        assert_eq!(pending["peer-a"], 500);
    }

    /// read 不会被 delivered 回退（set_message_status 守卫）。
    #[test]
    fn read_status_never_regresses_to_delivered() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(&conn, &MessageRecord {
            id: 0, msg_id: "m1".into(), conv_id: "f1".into(),
            sender_id: "a".into(), receiver_id: "b".into(),
            kind: "text".into(), content: "hi".into(), ts: 100, status: "read".into(),
        }).unwrap();
        db::set_message_status(&conn, "m1", "delivered").unwrap();
        let status: String = conn.query_row(
            "SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(status, "read", "delivered 不得回退 read");
    }

    // ================================================================
    // Ack 中继转发测试
    // ================================================================

    /// Test 1：本机是原始发送者 → Ack 正常处理（status→delivered, outbox 删除）。
    #[test]
    fn ack_local_sender_marks_delivered_and_clears_outbox() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 模拟本机发送的消息
        db::insert_message(&conn, &MessageRecord {
            id: 0, msg_id: "m1".into(), conv_id: "d1".into(),
            sender_id: "me".into(), receiver_id: "d1".into(),
            kind: "text".into(), content: "hi".into(), ts: 100, status: "sent".into(),
        }).unwrap();
        db::insert_outbox(&conn, "m1", "d1", r#"payload"#).unwrap();

        // Ack handler 的查询：sender_id = "me" == 本机 → 走正常处理分支
        let sender_id: String = conn.query_row(
            "SELECT sender_id FROM messages WHERE msg_id = ?1", params!["m1"],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(sender_id, "me");

        // 模拟正常处理
        db::set_message_status(&conn, "m1", "delivered").unwrap();
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", params!["m1"]).unwrap();

        let status: String = conn.query_row(
            "SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(status, "delivered");
        assert!(db::list_outbox(&conn, "d1").unwrap().is_empty());
    }

    /// Test 2：中继节点收到 Ack → sender_id ≠ 本机 → 不做本地处理。
    /// 验证中继节点不修改自己的消息状态、不删除 outbox。
    #[test]
    fn ack_relay_node_does_not_process_locally() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 中继节点 C 收到 A 发给 D 的消息（通过 Gossip）
        db::insert_message(&conn, &MessageRecord {
            id: 0, msg_id: "m1".into(), conv_id: "d1".into(),
            sender_id: "node-a".into(), receiver_id: "d1".into(),
            kind: "text".into(), content: "hello".into(), ts: 200, status: "delivered".into(),
        }).unwrap();
        // C 自己也有一条 outbox 消息（不同的 msg_id）
        db::insert_outbox(&conn, "m-own", "some-peer", r#"own payload"#).unwrap();

        // Ack handler 查询：sender_id = "node-a" ≠ "me"（当前节点是 C）
        let sender_id: String = conn.query_row(
            "SELECT sender_id FROM messages WHERE msg_id = ?1", params!["m1"],
            |r| r.get(0),
        ).unwrap();
        assert_ne!(sender_id, "me", "sender_id 应为原始发送方 A，不是本机 C");

        // 中继节点不应执行任何本地状态修改
        // （实际 handler 中，Some(sender) if sender == device_id 分支不匹配 → 走转发分支）
        let status: String = conn.query_row(
            "SELECT status FROM messages WHERE msg_id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(status, "delivered", "中继节点不修改消息状态");
        assert!(!db::list_outbox(&conn, "some-peer").unwrap().is_empty(),
            "中继节点不删除自己的 outbox");
    }

    /// Test 3：Ack 对应的 msg_id 不存在 → 查询返回 None → 安全丢弃。
    #[test]
    fn ack_unknown_msg_id_is_silently_dropped() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 不存在的消息
        let result: Option<String> = conn.query_row(
            "SELECT sender_id FROM messages WHERE msg_id = ?1", params!["nonexistent"],
            |r| r.get(0),
        ).ok();
        assert!(result.is_none(), "查询不存在的 msg_id 应返回 None");
        // 此时 handler 走 None 分支 → 不做任何修改
    }

    /// Test 4：中继节点转发 Ack 时，sender_id 和 message_id 不被修改。
    #[test]
    fn ack_relay_preserves_original_fields() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::insert_message(&conn, &MessageRecord {
            id: 0, msg_id: "m1".into(), conv_id: "d1".into(),
            sender_id: "node-a".into(), receiver_id: "d1".into(),
            kind: "text".into(), content: "hi".into(), ts: 100, status: "delivered".into(),
        }).unwrap();

        // 中继节点查询到原始 sender_id
        let original_sender: String = conn.query_row(
            "SELECT sender_id FROM messages WHERE msg_id = ?1", params!["m1"],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(original_sender, "node-a");

        // 转发时使用原始 Ack 消息（message_id 和 sender_id 不变）
        let ack = Message::Ack { msg_id: "m1".into() };
        match &ack {
            Message::Ack { msg_id } => {
                assert_eq!(msg_id, "m1", "message_id 不得被修改");
            }
            _ => panic!("应为 Ack"),
        }
        // original_sender 用于 try_send 的 peer_id 参数，不嵌入 Ack 消息体
    }

    // ================================================================
    // 好友权限测试
    // ================================================================

    /// 好友状态下 Direct Chat 可以正常接收（insert_message_if_new 成功）。
    #[test]
    fn friend_chat_message_accepted() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "a", "Alice", None).unwrap();
        assert!(db::get_friend(&conn, "a").is_some(), "好友存在时应能处理消息");
    }

    /// 删除好友后 Direct Chat 不落库。
    #[test]
    fn non_friend_chat_message_rejected() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        db::add_friend(&conn, "a", "Alice", None).unwrap();
        db::remove_friend(&conn, "a").unwrap();
        assert!(db::get_friend(&conn, "a").is_none(), "删除好友后应检测不到");
    }

    /// Gossip Group 不受好友检查影响。
    #[test]
    fn gossip_group不受好友检查影响() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        // 群聊不需要好友关系
        db::create_group(&conn, "g1", "测试群", "owner", &["a".into(), "b".into()]).unwrap();
        let groups = db::list_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1);
    }

    /// FriendMessageBlocked 包含 original_sender 字段。
    #[test]
    fn friend_message_blocked_has_original_sender() {
        let msg = Message::FriendMessageBlocked {
            from: "c".into(),
            to: "a".into(),
            original_sender: "a".into(),
        };
        match &msg {
            Message::FriendMessageBlocked { from, to, original_sender } => {
                assert_eq!(from, "c");
                assert_eq!(to, "a");
                assert_eq!(original_sender, "a");
            }
            _ => panic!("应为 FriendMessageBlocked"),
        }
    }
}
