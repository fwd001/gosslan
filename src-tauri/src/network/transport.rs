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

use crate::crypto;
use crate::db;
use crate::network::file;
use crate::protocol::{GossipEnvelope, GossipKind, Message, MAX_FRAME};
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
    tokio::spawn(writer_loop(w, rx));
    handle_message(&state, &peer_id, first).await;
    reader_loop(state, r, peer_id, tx).await;
}

async fn writer_loop(mut w: OwnedWriteHalf, mut rx: mpsc::Receiver<Message>) {
    while let Some(msg) = rx.recv().await {
        if write_frame(&mut w, &msg).await.is_err() {
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
    tokio::spawn(writer_loop(w, rx));

    let hello = Message::Hello {
        device_id: state.device_id.clone(),
        nickname: state.nickname.lock().unwrap().clone(),
        avatar: state.avatar.lock().unwrap().clone(),
        tcp_port: state.tcp_port,
    };
    let _ = tx.send(hello).await;

    tokio::spawn(reader_loop(state.clone(), r, peer_id.to_string(), tx));
    flush_outbox(state, peer_id).await;
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
        }
        Message::Heartbeat { device_id } => {
            touch_peer(state, &device_id).await;
            flush_outbox(state, &device_id).await;
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
        Message::ChatMessage { msg_id, from, to, kind, content, ts } => {
            if to != state.device_id {
                return;
            }
            // E2EE："enc1:" 前缀 = 发送方→我的 ChaCha20-Poly1305 加密内容，用对方 X25519 公钥解密
            // 解密失败（缺公钥 / 公钥已更新）：写入系统消息「需开启 E2EE 或更新对端公钥才能查看」，
            // 而非静默丢弃，避免开启 E2EE 的一方发来的消息在另一端凭空消失。
            let (content, kind_str) = match content.strip_prefix("enc1:") {
                Some(b64) => {
                    let spk_opt = {
                        let dbc = state.db.lock().unwrap();
                        db::get_friend_x25519(&dbc, &from)
                    }
                    .or_else(|| {
                        state
                            .peers
                            .lock()
                            .unwrap()
                            .get(&from)
                            .and_then(|p| p.x25519_pubkey.clone())
                    });
                    match spk_opt {
                        None => (
                            format!("[加密消息] 尚未获取 {from} 的公钥，请让对方重新上线后重发"),
                            "system".to_string(),
                        ),
                        Some(spk) => match crypto::shared_secret(&state.identity.x25519_secret, &spk) {
                            Some(shared) => match STANDARD
                                .decode(b64)
                                .ok()
                                .and_then(|d| crypto::open(&shared, &d))
                            {
                                Some(bytes) => match String::from_utf8(bytes) {
                                    Ok(s) => (s, kind.as_str().to_string()),
                                    Err(_) => (
                                        "[加密消息] 内容解码失败，对方可能更换了密钥".to_string(),
                                        "system".to_string(),
                                    ),
                                },
                                None => (
                                    format!("[加密消息] 解密失败（{from} 的公钥可能已更新）"),
                                    "system".to_string(),
                                ),
                            },
                            None => (
                                "[加密消息] 密钥交换失败".to_string(),
                                "system".to_string(),
                            ),
                        },
                    }
                }
                None => (content, kind.as_str().to_string()),
            };
            let name = resolve_nickname(state, &from);
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
            {
                let dbc = state.db.lock().unwrap();
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
                db::insert_message(&dbc, &rec).ok();
                db::touch_conversation(&dbc, &from, "single", &name, None, &preview, 1).ok();
            }
            let rec = MessageRecord {
                id: 0,
                msg_id: msg_id.clone(),
                conv_id: from.clone(),
                sender_id: from.clone(),
                receiver_id: state.device_id.clone(),
                kind: kind_str,
                content,
                ts,
                status: "delivered".to_string(),
            };
            let _ = state.app.emit("message-received", &rec);
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
            {
                let dbc = state.db.lock().unwrap();
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
                db::insert_message(&dbc, &rec).ok();
                db::touch_conversation(&dbc, &conv_id, "group", &name, None, &preview, 1).ok();
            }
            let rec = MessageRecord {
                id: 0,
                msg_id: msg_id.clone(),
                conv_id,
                sender_id: from,
                receiver_id: group_id,
                kind: kind_str,
                content,
                ts,
                status: "delivered".to_string(),
            };
            let _ = state.app.emit("message-received", &rec);
            let _ = try_send(state, peer_id, &Message::Ack { msg_id }).await;
        }
        Message::Ack { msg_id } => {
            let dbc = state.db.lock().unwrap();
            db::set_message_status(&dbc, &msg_id, "delivered").ok();
            dbc.execute("DELETE FROM outbox WHERE msg_id = ?1", params![msg_id]).ok();
            drop(dbc);
            let _ = state.app.emit("message-acked", &msg_id);
        }
        Message::ReadReceipt { from, to, last_read_ts } => {
            if to != state.device_id || from == state.device_id {
                return;
            }
            // 对方已读：把「我发给对方、ts ≤ last_read_ts」的消息标记为 read（幂等）
            let updated = {
                let dbc = state.db.lock().unwrap();
                dbc.execute(
                    "UPDATE messages SET status = 'read'
                     WHERE conv_id = ?1 AND sender_id = ?2 AND status != 'read' AND ts <= ?3",
                    params![from, state.device_id, last_read_ts],
                )
                .unwrap_or(0)
            };
            if updated > 0 {
                let _ = state.app.emit(
                    "peer-read",
                    &serde_json::json!({ "peer_id": from, "last_read_ts": last_read_ts }),
                );
            }
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
        }
    };

    // 4. 转发（fan-out，TTL 衰减）
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

    // 5. 解密成功则落库 + 通知
    if let Some(pt) = plaintext {
        let (kind, content) = parse_gossip_payload(&pt);
        let conv_id = match &env.kind {
            GossipKind::Chat => env.sender_id.clone(),
            GossipKind::Group => format!("group:{}", env.group_id.clone().unwrap_or_default()),
        };
        let conv_kind = match &env.kind {
            GossipKind::Chat => "single",
            GossipKind::Group => "group",
        };
        let name = match &env.kind {
            GossipKind::Chat => resolve_nickname(state, &env.sender_id),
            GossipKind::Group => resolve_group_name(state, env.group_id.as_deref().unwrap_or("")),
        };
        let preview = preview_content(&kind, &content);
        let rec = MessageRecord {
            id: 0,
            // 不加前缀：与 outbox 补发的直连 ChatMessage 共用同一 msg_id，
            // 接收方 message_exists 可跨路径去重（防建链竞态下的重复消息）
            msg_id: env.message_id.clone(),
            conv_id: conv_id.clone(),
            sender_id: env.sender_id.clone(),
            receiver_id: state.device_id.clone(),
            kind: kind.clone(),
            content: content.clone(),
            ts: env.ts,
            status: "delivered".to_string(),
        };
        {
            let dbc = state.db.lock().unwrap();
            db::insert_message(&dbc, &rec).ok();
            db::touch_conversation(&dbc, &conv_id, conv_kind, &name, None, &preview, 1).ok();
        }
        let _ = state.app.emit("message-received", &rec);
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
pub async fn flush_outbox(state: &AppState, peer_id: &str) {
    let pending = {
        let dbc = state.db.lock().unwrap();
        db::list_outbox(&dbc, peer_id).unwrap_or_default()
    };
    for (_id, payload) in pending {
        let Ok(msg) = serde_json::from_str::<Message>(&payload) else {
            continue;
        };
        let _ = try_send(state, peer_id, &msg).await;
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
}
