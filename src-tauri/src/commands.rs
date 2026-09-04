//! Tauri 命令层：前端调用的所有后端入口。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

use crate::crypto;
use crate::db;
use crate::network::{self, file};
use crate::network::transport::{broadcast_gossip, get_group_key, resolve_nickname, try_send};
use crate::protocol::{GossipKind, Message, ShareEntry};
use crate::storage::cache_cleaner::{self, CachePolicy, CleanupReport};
use crate::transport::{ChannelStatus, TransportManager};
use crate::state::{
    AppState, Conversation, DeviceInfo, Friend, Group, InterfaceInfo, MessageRecord, Peer,
    PendingRequest, TopologyInfo, TransferInfo,
};

#[derive(Serialize)]
struct NetworkStatus {
    online: bool,
    bound_ip: Option<String>,
}

/// 缓存目录占用与策略（存储管理页展示）。
#[derive(Serialize)]
struct CacheInfo {
    file_count: usize,
    total_bytes: u64,
    retention_days: Option<u32>,
    max_bytes: Option<u64>,
}

// ---------------- 本机信息与配置 ----------------

#[tauri::command]
pub fn get_device_info(state: State<'_, Arc<AppState>>) -> DeviceInfo {
    let s = state.inner();
    DeviceInfo {
        device_id: s.device_id.clone(),
        nickname: s.nickname.lock().unwrap().clone(),
        avatar: s.avatar.lock().unwrap().clone(),
        tcp_port: s.tcp_port,
        online: s.network.lock().unwrap().is_some(),
        x25519_pubkey: s.identity.x25519_public_b64(),
        ed25519_pubkey: s.identity.ed25519_public_b64(),
    }
}

#[tauri::command]
pub async fn update_profile(
    state: State<'_, Arc<AppState>>,
    nickname: String,
    avatar: Option<String>,
) -> Result<DeviceInfo, String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap();
        db::set_setting(&dbc, "nickname", &nickname).ok();
        if let Some(a) = &avatar {
            db::set_setting(&dbc, "avatar", a).ok();
        }
    }
    *s.nickname.lock().unwrap() = nickname.clone();
    *s.avatar.lock().unwrap() = avatar.clone();

    let msg = Message::UserInfo {
        device_id: s.device_id.clone(),
        nickname,
        avatar,
    };
    let links = s.links.lock().await;
    for tx in links.values() {
        let _ = tx.send(msg.clone()).await;
    }
    drop(links);

    Ok(DeviceInfo {
        device_id: s.device_id.clone(),
        nickname: s.nickname.lock().unwrap().clone(),
        avatar: s.avatar.lock().unwrap().clone(),
        tcp_port: s.tcp_port,
        online: s.network.lock().unwrap().is_some(),
        x25519_pubkey: s.identity.x25519_public_b64(),
        ed25519_pubkey: s.identity.ed25519_public_b64(),
    })
}

#[tauri::command]
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut out = Vec::new();
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if let std::net::IpAddr::V4(ip) = i.addr.ip() {
                if !ip.is_loopback() {
                    out.push(InterfaceInfo {
                        name: i.name.clone(),
                        ip: ip.to_string(),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.ip.cmp(&b.ip));
    out
}

// ---------------- 网络控制 ----------------

#[tauri::command]
pub async fn start_network(state: State<'_, Arc<AppState>>, bind_ip: String) -> Result<(), String> {
    let arc = state.inner().clone();
    network::start(arc, bind_ip).await
}

#[tauri::command]
pub async fn stop_network(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    network::stop(state.inner()).await;
    Ok(())
}

#[tauri::command]
pub fn get_network_status(state: State<'_, Arc<AppState>>) -> NetworkStatus {
    let s = state.inner();
    let net = s.network.lock().unwrap();
    NetworkStatus {
        online: net.is_some(),
        bound_ip: net.as_ref().map(|n| n.bound_ip.clone()),
    }
}

#[tauri::command]
pub fn get_peers(state: State<'_, Arc<AppState>>) -> Vec<Peer> {
    let mut peers: Vec<Peer> = state.inner().peers.lock().unwrap().values().cloned().collect();
    peers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    peers
}

/// 按需探测周围在线节点：群发一次 `who_has`，等待约 1.5s 收集单播回复后返回当前节点表。
/// 仅在用户打开「添加好友」时调用，避免启动时持续全网扫描。
#[tauri::command]
pub async fn search_nearby_peers(state: State<'_, Arc<AppState>>) -> Vec<Peer> {
    let s = state.inner();
    // 触发一次探测（若网络已启动）
    let triggered = if let Some(tx) = s.probe.lock().unwrap().as_ref() {
        let next = tx.borrow().saturating_add(1);
        let _ = tx.send(next);
        true
    } else {
        false
    };
    // 等待节点单播回复
    if triggered {
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    let mut peers: Vec<Peer> = s.peers.lock().unwrap().values().cloned().collect();
    peers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    peers
}

/// 从后台唤起并聚焦主窗口（点击系统通知后调用）。
#[tauri::command]
pub fn focus_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Err("主窗口不存在".to_string());
    };
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

/// 网络拓扑摘要：节点数、中继数、平均时延。
#[tauri::command]
pub fn get_topology(state: State<'_, Arc<AppState>>) -> TopologyInfo {
    let s = state.inner();
    let peers = s.peers.lock().unwrap();
    let node_count = peers.len();
    let rtts: Vec<u64> = peers.values().filter_map(|p| p.rtt_ms).collect();
    let avg_rtt_ms = if rtts.is_empty() {
        None
    } else {
        Some(rtts.iter().sum::<u64>() / rtts.len() as u64)
    };
    let relay_count = s.relay.lock().unwrap().active_sends();
    let online = s.network.lock().unwrap().is_some();
    TopologyInfo {
        node_count,
        relay_count,
        avg_rtt_ms,
        online,
    }
}

// ---------------- 双通道与缓存 ----------------

/// 局域网 / 蓝牙通道状态（设置页开关 + 状态监控）。
#[tauri::command]
pub fn get_channel_status(state: State<'_, Arc<AppState>>) -> Vec<ChannelStatus> {
    TransportManager::new(state.inner().clone()).status()
}

/// 切换通道开关。局域网复用 `network`；蓝牙后端未编译，开启时返回明确错误。
#[tauri::command]
pub async fn set_channel_enabled(
    state: State<'_, Arc<AppState>>,
    channel: String,
    enabled: bool,
) -> Result<(), String> {
    let s = state.inner();
    match channel.as_str() {
        "lan" => {
            if enabled {
                let arc = s.clone();
                network::start(arc, "0.0.0.0".to_string()).await?;
            } else {
                network::stop(s).await;
            }
            Ok(())
        }
        "bluetooth" => {
            let mut mgr = TransportManager::new(s.clone());
            if enabled {
                mgr.set_bluetooth_enabled(true).await
            } else {
                let dbc = s.db.lock().unwrap();
                db::set_setting(&dbc, "bt_enabled", "0").ok();
                Ok(())
            }
        }
        _ => Err(format!("未知通道: {channel}")),
    }
}

const RETENTION_KEY: &str = "cache_retention_days";
const MAX_BYTES_KEY: &str = "cache_max_bytes";

fn load_policy(s: &AppState) -> CachePolicy {
    let dbc = s.db.lock().unwrap();
    let retention = db::get_setting(&dbc, RETENTION_KEY)
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&d| d > 0);
    let max = db::get_setting(&dbc, MAX_BYTES_KEY)
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&m| m > 0);
    CachePolicy { retention_days: retention, max_bytes: max }
}

/// 缓存目录占用与当前清理策略。
#[tauri::command]
pub fn get_cache_info(state: State<'_, Arc<AppState>>) -> CacheInfo {
    let s = state.inner();
    let (file_count, total_bytes) = cache_cleaner::usage(&s.cache_dir);
    let policy = load_policy(s);
    CacheInfo {
        file_count,
        total_bytes,
        retention_days: policy.retention_days,
        max_bytes: policy.max_bytes,
    }
}

/// 设置缓存清理策略（保留时长 / 磁盘配额；`None` 或 `0` 表示不限制）。
#[tauri::command]
pub fn set_cache_policy(
    state: State<'_, Arc<AppState>>,
    retention_days: Option<u32>,
    max_bytes: Option<u64>,
) -> Result<(), String> {
    let s = state.inner();
    let dbc = s.db.lock().unwrap();
    let d = retention_days.unwrap_or(0);
    db::set_setting(&dbc, RETENTION_KEY, &d.to_string()).ok();
    let m = max_bytes.unwrap_or(0);
    db::set_setting(&dbc, MAX_BYTES_KEY, &m.to_string()).ok();
    Ok(())
}

/// 立即执行一次缓存清理（过期 / 超配额删除 + SQLite VACUUM）。
#[tauri::command]
pub fn clean_cache_now(state: State<'_, Arc<AppState>>) -> CleanupReport {
    let s = state.inner();
    let policy = load_policy(s);
    let dbc = s.db.lock().unwrap();
    cache_cleaner::clean(&s.cache_dir, policy, &*dbc)
}

// ---------------- 好友 ----------------

#[tauri::command]
pub fn get_friends(state: State<'_, Arc<AppState>>) -> Vec<Friend> {
    let s = state.inner();
    let peers = s.peers.lock().unwrap();
    let dbc = s.db.lock().unwrap();
    let mut friends = db::list_friends(&dbc).unwrap_or_default();
    for f in friends.iter_mut() {
        f.online = peers.contains_key(&f.device_id);
    }
    friends
}

#[tauri::command]
pub fn get_pending_requests(state: State<'_, Arc<AppState>>) -> Vec<PendingRequest> {
    state
        .inner()
        .pending_requests
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect()
}

#[tauri::command]
pub async fn send_friend_request(state: State<'_, Arc<AppState>>, peer_id: String) -> Result<(), String> {
    let s = state.inner();
    let msg = Message::FriendRequest {
        from: s.device_id.clone(),
        from_nickname: s.nickname.lock().unwrap().clone(),
        from_avatar: s.avatar.lock().unwrap().clone(),
        to: peer_id.clone(),
        ts: db::now_ms(),
    };
    try_send(s, &peer_id, &msg).await
}

#[tauri::command]
pub async fn respond_friend_request(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    accept: bool,
) -> Result<(), String> {
    let s = state.inner();
    s.pending_requests.lock().unwrap().remove(&peer_id);
    if accept {
        let name = resolve_nickname(s, &peer_id);
        {
            let dbc = s.db.lock().unwrap();
            db::add_friend(&dbc, &peer_id, &name, None).ok();
            db::ensure_conversation(&dbc, &peer_id, "single", &name, None).ok();
        }
        let msg = Message::FriendAccept {
            from: s.device_id.clone(),
            to: peer_id.clone(),
        };
        try_send(s, &peer_id, &msg).await?;
        let _ = s.app.emit("friend-accepted", &peer_id);
    } else {
        let msg = Message::FriendReject {
            from: s.device_id.clone(),
            to: peer_id.clone(),
        };
        try_send(s, &peer_id, &msg).await?;
    }
    Ok(())
}

// ---------------- 单聊（Gossip + E2EE） ----------------

#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    content: String,
    kind: String,
) -> Result<MessageRecord, String> {
    let s = state.inner();

    // 获取对方 X25519 公钥
    let pubkey = {
        let dbc = s.db.lock().unwrap();
        db::get_friend_x25519(&dbc, &friend_id)
    };
    let Some(pubkey) = pubkey else {
        return Err("尚未获取对方公钥，请确认对方在线后重试".to_string());
    };

    let ts = db::now_ms();
    let name = resolve_nickname(s, &friend_id);
    let preview = preview(&kind, &content);

    // 本地落库（明文）
    let rec = MessageRecord {
        id: 0,
        msg_id: Uuid::new_v4().to_string(),
        conv_id: friend_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: friend_id.clone(),
        kind: kind.clone(),
        content: content.clone(),
        ts,
        status: "sent".to_string(),
    };
    {
        let dbc = s.db.lock().unwrap();
        db::insert_message(&dbc, &rec).ok();
        db::touch_conversation(&dbc, &friend_id, "single", &name, None, &preview, 0).ok();
    }

    // E2EE 加密 + Gossip 广播
    let plaintext = serde_json::json!({ "kind": kind, "content": content }).to_string();
    let shared = crypto::shared_secret(&s.identity.x25519_secret, &pubkey).ok_or("密钥交换失败")?;
    let sealed = crypto::seal(&shared, plaintext.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let env = {
        let gossip = s.gossip.lock().unwrap();
        gossip.build_envelope(&s.identity, &s.device_id, GossipKind::Chat, None, &payload_b64, ts)
    };
    broadcast_gossip(s, env).await;

    Ok(rec)
}

#[tauri::command]
pub fn get_messages(
    state: State<'_, Arc<AppState>>,
    conv_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Vec<MessageRecord> {
    let dbc = state.inner().db.lock().unwrap();
    db::get_messages(&dbc, &conv_id, limit.unwrap_or(100), offset.unwrap_or(0)).unwrap_or_default()
}

#[tauri::command]
pub fn get_conversations(state: State<'_, Arc<AppState>>) -> Vec<Conversation> {
    let dbc = state.inner().db.lock().unwrap();
    db::list_conversations(&dbc).unwrap_or_default()
}

#[tauri::command]
pub fn mark_read(state: State<'_, Arc<AppState>>, conv_id: String) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap();
    db::mark_read(&dbc, &conv_id).map_err(|e| e.to_string())
}

// ---------------- 群聊（群密钥 + Gossip） ----------------

#[tauri::command]
pub fn create_group(
    state: State<'_, Arc<AppState>>,
    name: String,
    members: Vec<String>,
) -> Result<Group, String> {
    let s = state.inner();
    let id = format!("g-{}", Uuid::new_v4());
    let mut all = members;
    if !all.contains(&s.device_id) {
        all.push(s.device_id.clone());
    }

    // 生成群密钥并持久化
    let key = crypto::random_key();
    s.group_keys.lock().unwrap().insert(id.clone(), key);
    {
        let dbc = s.db.lock().unwrap();
        db::set_setting(&dbc, &format!("gk:{id}"), &STANDARD.encode(key)).ok();
    }

    {
        let dbc = s.db.lock().unwrap();
        db::create_group(&dbc, &id, &name, &s.device_id, &all).map_err(|e| e.to_string())?;
        db::ensure_conversation(&dbc, &format!("group:{id}"), "group", &name, None).ok();
    }

    Ok(Group {
        id,
        name,
        creator: s.device_id.clone(),
        members: all,
    })
}

/// 向群成员分发群密钥（用各成员公钥 ECDH 加密）。
#[tauri::command]
pub async fn distribute_group_key(
    state: State<'_, Arc<AppState>>,
    group_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;
    let members = {
        let dbc = s.db.lock().unwrap();
        db::list_groups(&dbc)
            .unwrap_or_default()
            .into_iter()
            .find(|g| g.id == group_id)
            .map(|g| g.members)
            .unwrap_or_default()
    };
    for m in members {
        if m == s.device_id {
            continue;
        }
        let pubkey = {
            let dbc = s.db.lock().unwrap();
            db::get_friend_x25519(&dbc, &m)
        };
        let Some(pubkey) = pubkey else { continue };
        let Some(shared) = crypto::shared_secret(&s.identity.x25519_secret, &pubkey) else { continue };
        let Some(sealed) = crypto::seal(&shared, &key) else { continue };
        let msg = Message::GroupKey {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            to: m.clone(),
            key: STANDARD.encode(&sealed),
        };
        let _ = try_send(s, &m, &msg).await;
    }
    Ok(())
}

#[tauri::command]
pub fn get_groups(state: State<'_, Arc<AppState>>) -> Vec<Group> {
    let dbc = state.inner().db.lock().unwrap();
    db::list_groups(&dbc).unwrap_or_default()
}

#[tauri::command]
pub async fn send_group_message(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    content: String,
    kind: String,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let ts = db::now_ms();
    let group_name = {
        let dbc = s.db.lock().unwrap();
        db::list_groups(&dbc)
            .unwrap_or_default()
            .into_iter()
            .find(|g| g.id == group_id)
            .map(|g| g.name)
            .unwrap_or_default()
    };
    let conv_id = format!("group:{group_id}");
    let preview = preview(&kind, &content);

    let rec = MessageRecord {
        id: 0,
        msg_id: Uuid::new_v4().to_string(),
        conv_id: conv_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: group_id.clone(),
        kind: kind.clone(),
        content: content.clone(),
        ts,
        status: "sent".to_string(),
    };
    {
        let dbc = s.db.lock().unwrap();
        db::insert_message(&dbc, &rec).ok();
        db::touch_conversation(&dbc, &conv_id, "group", &group_name, None, &preview, 0).ok();
    }

    // 群密钥加密 + Gossip 广播
    let key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;
    let plaintext = serde_json::json!({ "kind": kind, "content": content }).to_string();
    let sealed = crypto::seal_symmetric(&key, plaintext.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let env = {
        let gossip = s.gossip.lock().unwrap();
        gossip.build_envelope(&s.identity, &s.device_id, GossipKind::Group, Some(group_id), &payload_b64, ts)
    };
    broadcast_gossip(s, env).await;

    Ok(rec)
}

// ---------------- 文件传输 ----------------

#[tauri::command]
pub async fn send_file(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    let s = state.inner();
    let transfer_id = Uuid::new_v4().to_string();
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let size = meta.len();
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let content = serde_json::json!({ "name": name.clone(), "path": path, "size": size }).to_string();
    let rec = MessageRecord {
        id: 0,
        msg_id: format!("file-{transfer_id}"),
        conv_id: friend_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: friend_id.clone(),
        kind: "file".to_string(),
        content,
        ts: db::now_ms(),
        status: "sent".to_string(),
    };
    {
        let dbc = s.db.lock().unwrap();
        db::insert_message(&dbc, &rec).ok();
        let nm = resolve_nickname(s, &friend_id);
        db::touch_conversation(&dbc, &friend_id, "single", &nm, None, &format!("[文件] {name}"), 0).ok();
    }
    let _ = s.app.emit("message-received", &rec);

    let arc = state.inner().clone();
    let fid = friend_id;
    let tid = transfer_id.clone();
    let p = PathBuf::from(path);
    tokio::spawn(async move {
        let _ = file::send_file_from_path(&arc, &fid, &tid, p).await;
    });
    Ok(transfer_id)
}

/// 中继切片发送：把文件切片并行分发给接收方 + 空闲中继节点。
#[tauri::command]
pub async fn send_file_relay(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    let s = state.inner();
    let transfer_id = Uuid::new_v4().to_string();

    let (name, size, chunks) = {
        let relay = s.relay.lock().unwrap();
        relay.slice_file(std::path::Path::new(&path)).map_err(|e| e.to_string())?
    };
    let total_chunks = chunks.len() as u32;
    s.relay.lock().unwrap().register_send(&transfer_id, chunks);

    // 元数据直接发给接收方
    let offer = Message::RelayFileOffer {
        transfer_id: transfer_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
        name: name.clone(),
        size,
        total_chunks,
    };
    try_send(s, &friend_id, &offer).await?;

    // 选择中继节点：在线且非接收方（最多 3 个）
    let relays: Vec<String> = {
        let peers = s.peers.lock().unwrap();
        peers
            .keys()
            .filter(|k| k.as_str() != friend_id)
            .cloned()
            .take(3)
            .collect()
    };
    let mut targets = vec![friend_id.clone()];
    targets.extend(relays);

    // 轮询分发切片
    let plan = { s.relay.lock().unwrap().plan_distribution(&transfer_id, &targets) };
    for p in plan {
        let chunk = Message::RelayChunk {
            transfer_id: transfer_id.clone(),
            seq: p.chunk.seq,
            data: p.chunk.data,
            from: s.device_id.clone(),
            to: friend_id.clone(),
            ttl: 3,
        };
        let _ = try_send(s, &p.peer_id, &chunk).await;
    }
    s.relay.lock().unwrap().finish_send(&transfer_id);

    // 本机消息记录
    let content = serde_json::json!({ "name": name.clone(), "path": path, "size": size }).to_string();
    let rec = MessageRecord {
        id: 0,
        msg_id: format!("file-{transfer_id}"),
        conv_id: friend_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: friend_id.clone(),
        kind: "file".to_string(),
        content,
        ts: db::now_ms(),
        status: "sent".to_string(),
    };
    {
        let dbc = s.db.lock().unwrap();
        db::insert_message(&dbc, &rec).ok();
        let nm = resolve_nickname(s, &friend_id);
        db::touch_conversation(&dbc, &friend_id, "single", &nm, None, &format!("[文件] {name}"), 0).ok();
    }
    let _ = s.app.emit("message-received", &rec);

    Ok(transfer_id)
}

#[tauri::command]
pub fn get_transfers(state: State<'_, Arc<AppState>>) -> Vec<TransferInfo> {
    let dbc = state.inner().db.lock().unwrap();
    db::list_transfers(&dbc).unwrap_or_default()
}

// ---------------- 共享目录 ----------------

#[tauri::command]
pub fn set_share_dir(state: State<'_, Arc<AppState>>, path: String) -> Result<(), String> {
    if !PathBuf::from(&path).is_dir() {
        return Err("目录不存在".to_string());
    }
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap();
        db::set_setting(&dbc, "share_dir", &path).ok();
    }
    *s.share_dir.lock().unwrap() = Some(path);
    Ok(())
}

#[tauri::command]
pub fn get_share_dir(state: State<'_, Arc<AppState>>) -> Option<String> {
    state.inner().share_dir.lock().unwrap().clone()
}

#[tauri::command]
pub async fn request_share_tree(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
) -> Result<Vec<ShareEntry>, String> {
    let s = state.inner();
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.pending_share_tree.lock().unwrap().insert(request_id.clone(), tx);

    let msg = Message::ShareTreeRequest {
        request_id: request_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
    };
    try_send(s, &friend_id, &msg).await?;

    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(entries)) => Ok(entries),
        _ => {
            s.pending_share_tree.lock().unwrap().remove(&request_id);
            Err("获取共享目录超时".to_string())
        }
    }
}

#[tauri::command]
pub async fn download_shared_file(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    remote_path: String,
) -> Result<String, String> {
    let s = state.inner();
    let transfer_id = Uuid::new_v4().to_string();
    let msg = Message::ShareFileRequest {
        transfer_id: transfer_id.clone(),
        from: s.device_id.clone(),
        path: remote_path,
    };
    try_send(s, &friend_id, &msg).await?;
    Ok(transfer_id)
}

// ---------------- 辅助 ----------------

fn preview(kind: &str, content: &str) -> String {
    match kind {
        "file" => "[文件]".to_string(),
        "image" => "[图片]".to_string(),
        "code" => "[代码]".to_string(),
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
