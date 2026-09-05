//! Tauri 命令层：前端调用的所有后端入口。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
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
pub struct NetworkStatus {
    online: bool,
    bound_ip: Option<String>,
}

/// 缓存目录占用与策略（存储管理页展示）。
#[derive(Serialize)]
pub struct CacheInfo {
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
pub async fn search_nearby_peers(state: State<'_, Arc<AppState>>) -> Result<Vec<Peer>, String> {
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
    Ok(peers)
}

/// 从后台唤起并聚焦主窗口（点击系统通知后调用）。
#[cfg(desktop)]
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

/// 移动端无独立窗口概念，系统通知自带唤起行为，无需额外处理。
#[cfg(mobile)]
#[tauri::command]
pub fn focus_window(_app: tauri::AppHandle) -> Result<(), String> {
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

// ---------------- 应用偏好设置（本地持久化） ----------------

/// 应用偏好：外观、网卡选择等。持久化到本地 SQLite，重启后恢复。
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme_color: Option<String>,
    pub font_family: Option<String>,
    pub dark_mode: Option<bool>,
    pub bind_ip: Option<String>,
    /// 聊天显示样式 JSON：{"preset":"classic","fontSize":"md","compact":true}
    pub chat_style: Option<String>,
    /// 端到端加密开关（默认关闭；开启后单聊/群聊载荷 ChaCha20-Poly1305 加密）
    pub e2ee_enabled: Option<bool>,
    /// 对端样式表 JSON（device_id -> style JSON）。仅由后端在收到 ChatStyle 消息时写入，
    /// 前端只读；save_settings 忽略该字段。
    pub peer_styles: Option<String>,
}

const SETTINGS_KEYS: [&str; 6] = [
    "theme_color",
    "font_family",
    "dark_mode",
    "bind_ip",
    "chat_style",
    "e2ee_enabled",
];

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    let dbc = state.inner().db.lock().unwrap();
    Settings {
        theme_color: db::get_setting(&dbc, "theme_color"),
        font_family: db::get_setting(&dbc, "font_family"),
        dark_mode: db::get_setting(&dbc, "dark_mode").map(|v| v == "1"),
        bind_ip: db::get_setting(&dbc, "bind_ip"),
        chat_style: db::get_setting(&dbc, "chat_style"),
        e2ee_enabled: Some(db::get_setting(&dbc, "e2ee_enabled").map(|v| v == "1").unwrap_or(false)),
        peer_styles: db::get_setting(&dbc, "chat_peer_styles"),
    }
}

#[tauri::command]
pub fn save_settings(state: State<'_, Arc<AppState>>, settings: Settings) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap();
    if let Some(v) = settings.theme_color {
        db::set_setting(&dbc, "theme_color", &v).ok();
    }
    if let Some(v) = settings.font_family {
        db::set_setting(&dbc, "font_family", &v).ok();
    }
    if let Some(v) = settings.dark_mode {
        db::set_setting(&dbc, "dark_mode", if v { "1" } else { "0" }).ok();
    }
    if let Some(v) = settings.bind_ip {
        db::set_setting(&dbc, "bind_ip", &v).ok();
    }
    if let Some(v) = settings.chat_style {
        db::set_setting(&dbc, "chat_style", &v).ok();
    }
    if let Some(v) = settings.e2ee_enabled {
        db::set_setting(&dbc, "e2ee_enabled", if v { "1" } else { "0" }).ok();
    }
    Ok(())
}

/// 恢复默认设置：清除外观 / 网卡 / 缓存策略 / 聊天样式等偏好键，上层回落到默认值。
/// 不触碰用户数据（好友、聊天记录、昵称、头像、共享目录）。
#[tauri::command]
pub fn reset_settings(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap();
    for key in SETTINGS_KEYS
        .iter()
        .chain([&RETENTION_KEY, &MAX_BYTES_KEY, &"bt_enabled", &"chat_peer_styles"])
    {
        db::delete_setting(&dbc, key).ok();
    }
    Ok(())
}

/// 广播本机聊天样式到所有已连接节点（样式变更即调用，对方设备与好友同步收到）。
#[tauri::command]
pub async fn broadcast_chat_style(state: State<'_, Arc<AppState>>, style: String) -> Result<(), String> {
    let s = state.inner();
    let msg = Message::ChatStyle {
        from: s.device_id.clone(),
        to: None,
        style,
    };
    let links = s.links.lock().await;
    for tx in links.values() {
        let _ = tx.send(msg.clone()).await;
    }
    Ok(())
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

/// 删除好友（保留聊天记录；对方仍会出现在扫描列表，可重新添加）。
#[tauri::command]
pub fn remove_friend(state: State<'_, Arc<AppState>>, peer_id: String) -> Result<(), String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap();
        db::remove_friend(&dbc, &peer_id).map_err(|e| e.to_string())?;
    }
    let _ = s.app.emit("friend-removed", &peer_id);
    Ok(())
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

    // E2EE 开关（关闭则不加密 → 不需要对端公钥；最常见场景：双方都关闭或一方关闭）
    let e2ee = {
        let dbc = s.db.lock().unwrap();
        db::get_setting(&dbc, "e2ee_enabled").map(|v| v == "1").unwrap_or(false)
    };

    // 仅在 E2EE 开启时才需要对方 X25519 公钥；找不到时尝试主动探测一次在线节点，
    // 给对方发 announce 的窗口（Windows / 不同子网首次上线时常需要这一刷新）。
    let pubkey = if e2ee {
        let from_db = {
            let dbc = s.db.lock().unwrap();
            db::get_friend_x25519(&dbc, &friend_id)
        };
        let from_peers = s
            .peers
            .lock()
            .unwrap()
            .get(&friend_id)
            .and_then(|p| p.x25519_pubkey.clone());
        match from_db.or(from_peers) {
            Some(k) => Some(k),
            None => {
                // 主动触发一次 who_has → 等对方/中继 announce → 再查一次
                let triggered = if let Some(tx) = s.probe.lock().unwrap().as_ref() {
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
                    let dbc = s.db.lock().unwrap();
                    db::get_friend_x25519(&dbc, &friend_id)
                };
                let again_peers = s
                    .peers
                    .lock()
                    .unwrap()
                    .get(&friend_id)
                    .and_then(|p| p.x25519_pubkey.clone());
                again_db.or(again_peers)
            }
        }
    } else {
        None
    };
    if e2ee && pubkey.is_none() {
        return Err(format!(
            "尚未获取 {friend_id} 的公钥：对方可能离线或处于不同子网。请让对方上线后重试，或临时关闭 E2EE 后明文发送"
        ));
    }

    let ts = db::now_ms();
    let name = resolve_nickname(s, &friend_id);
    let preview = preview(&kind, &content);

    // E2EE 加密 + Gossip 信封（先于本地落库：msg_id 三处统一用 Gossip 信封 ID）
    let plaintext = serde_json::json!({ "kind": kind, "content": content }).to_string();
    // e2ee 关：明文 + pubkey 为 None → 不调用密钥派生；e2ee 开：必须拿到对方公钥
    let shared = if e2ee {
        let pk = pubkey.as_deref().expect("e2ee=true 时 pubkey 已守卫");
        Some(crypto::shared_secret(&s.identity.x25519_secret, pk).ok_or("密钥交换失败")?)
    } else {
        None
    };
    // E2EE 开：Gossip 载荷与直发内容都走 ChaCha20-Poly1305（直发内容加 "enc1:" 前缀标识）；
    // E2EE 关：载荷明文（信封 encrypted=false），性能优先。
    let (payload_b64, wire_content) = if e2ee {
        let s_secret = shared.as_ref().expect("e2ee=true 时 shared 已派生");
        let sealed = crypto::seal(s_secret, plaintext.as_bytes()).ok_or("加密失败")?;
        let sealed_content = crypto::seal(s_secret, content.as_bytes()).ok_or("加密失败")?;
        (
            STANDARD.encode(&sealed),
            format!("enc1:{}", STANDARD.encode(&sealed_content)),
        )
    } else {
        (STANDARD.encode(plaintext.as_bytes()), content.clone())
    };
    let mut env = {
        let gossip = s.gossip.lock().unwrap();
        gossip.build_envelope(&s.identity, &s.device_id, GossipKind::Chat, None, &payload_b64, ts)
    };
    env.encrypted = e2ee;
    // 统一 msg_id：本地记录 / Gossip 投递 / outbox 补发共用同一确定性 ID，
    // 接收方 message_exists 跨路径去重（防建链竞态窗口内的重复投递）。
    let msg_id = env.message_id.clone();

    // 本地落库（明文）
    let rec = MessageRecord {
        id: 0,
        msg_id: msg_id.clone(),
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

    broadcast_gossip(s, env).await;

    // 一律写离线队列兜底（INSERT OR IGNORE 按 msg_id 幂等）：直连链路存在但已失效
    // （半开 TCP）时 broadcast 会静默丢包，此前只在「无链路」时入队导致消息永久丢失。
    // Ack 到达后由 transport.rs 删除该行；若链路中断，对方上线建链（Hello）或心跳
    // 会触发 flush_outbox 自动补发，接收方按 msg_id 去重不会重复入库。
    let queued = Message::ChatMessage {
        msg_id: msg_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
        kind: crate::protocol::MsgKind::from_str(&kind),
        content: wire_content,
        ts,
    };
    if let Ok(payload) = serde_json::to_string(&queued) {
        let dbc = s.db.lock().unwrap();
        db::insert_outbox(&dbc, &msg_id, &friend_id, &payload).ok();
    }

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

/// 打开与好友的会话时确保会话行存在（新加好友尚未发过消息时，
/// 会话列表无对应项 → 左侧无法高亮选中态）。
#[tauri::command]
pub fn ensure_conversation(state: State<'_, Arc<AppState>>, friend_id: String) -> Result<Conversation, String> {
    let s = state.inner();
    let name = resolve_nickname(s, &friend_id);
    let avatar = {
        let peers = s.peers.lock().unwrap();
        peers.get(&friend_id).and_then(|p| p.avatar.clone())
    };
    {
        let dbc = s.db.lock().unwrap();
        db::ensure_conversation(&dbc, &friend_id, "single", &name, avatar.as_deref()).map_err(|e| e.to_string())?;
    }
    Ok(Conversation {
        id: friend_id.clone(),
        kind: "single".to_string(),
        name,
        avatar,
        last_msg: None,
        last_ts: None,
        unread: 0,
    })
}

/// 标记会话已读；单聊时向对方发送已读回执（触发对方界面的「已读绿勾」）。
#[tauri::command]
pub async fn mark_read(state: State<'_, Arc<AppState>>, conv_id: String) -> Result<(), String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap();
        db::mark_read(&dbc, &conv_id).map_err(|e| e.to_string())?;
    }
    if !conv_id.starts_with("group:") {
        // 通知对方：我已读到该会话最新一条消息为止
        let last_ts: Option<i64> = {
            let dbc = s.db.lock().unwrap();
            dbc.query_row(
                "SELECT COALESCE(MAX(ts), 0) FROM messages WHERE conv_id = ?1",
                rusqlite::params![conv_id],
                |r| r.get(0),
            )
            .ok()
        };
        if let Some(ts) = last_ts.filter(|t| *t > 0) {
            let msg = Message::ReadReceipt {
                from: s.device_id.clone(),
                to: conv_id.clone(),
                last_read_ts: ts,
            };
            let _ = crate::network::transport::try_send(s, &conv_id, &msg).await;
        }
    }
    Ok(())
}

/// 删除本地会话与全部消息（聊天记录清理）。
/// 仅删本地：不影响对方、不广播；前端负责二次确认弹窗。
/// 群聊同样支持（删除 group:xxx 会话及全部消息）。
#[tauri::command]
pub fn delete_conversation(state: State<'_, Arc<AppState>>, conv_id: String) -> Result<(), String> {
    let s = state.inner();
    let dbc = s.db.lock().unwrap();
    db::delete_conversation(&dbc, &conv_id).map_err(|e| e.to_string())
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

    // 群密钥加密 + Gossip 广播（受 E2EE 开关控制；关闭时载荷明文 + encrypted=false）
    let key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;
    let e2ee = {
        let dbc = s.db.lock().unwrap();
        db::get_setting(&dbc, "e2ee_enabled").map(|v| v == "1").unwrap_or(false)
    };
    let plaintext = serde_json::json!({ "kind": kind, "content": content }).to_string();
    let payload_b64 = if e2ee {
        let sealed = crypto::seal_symmetric(&key, plaintext.as_bytes()).ok_or("加密失败")?;
        STANDARD.encode(&sealed)
    } else {
        STANDARD.encode(plaintext.as_bytes())
    };
    let mut env = {
        let gossip = s.gossip.lock().unwrap();
        gossip.build_envelope(&s.identity, &s.device_id, GossipKind::Group, Some(group_id), &payload_b64, ts)
    };
    env.encrypted = e2ee;
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

/// 统一文件发送入口：自动路由。
/// - 与对方有直连 TCP 链路 → 直连分片流（可靠有序）。
/// - 无直连但周围有在线节点 → 切片中继（经其他节点转发）。
/// - 都不可达 → 返回明确错误。
#[tauri::command]
pub async fn send_file_auto(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    let s = state.inner();
    let direct = s.links.lock().await.contains_key(&friend_id);
    if direct {
        return send_file(state.clone(), friend_id, path).await;
    }

    // 无直连：若周围完全没有任何已建链节点，中继也走不通
    let relay_available = { !s.links.lock().await.is_empty() };
    if !relay_available {
        return Err("对方与周围节点均不在线，无法发送文件".to_string());
    }
    send_file_relay(state, friend_id, path).await
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

    // 文件读取 + base64 切片是同步重活：放阻塞线程池，避免卡住 async runtime（界面卡死根因）
    let chunk_size = { s.relay.lock().unwrap().chunk_size };
    let p = std::path::PathBuf::from(&path);
    let (name, size, chunks) = tokio::task::spawn_blocking(move || {
        crate::relay_manager::RelayManager::slice_file_with(&p, chunk_size)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

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
