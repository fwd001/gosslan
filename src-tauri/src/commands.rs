//! Tauri 命令层：前端调用的所有后端入口。

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

/// 业务输入长度限制（按字符数，非字节数）
const MAX_NICKNAME_LEN: usize = 40;
pub const MAX_GROUP_NAME_LEN: usize = 40;
const MAX_SEARCH_LEN: usize = 100;
/// 单条消息内容上限（按**字符数**，非字节数）。UTF-8 下一个中文字符 3 字节，
/// 5 万字符对应最大约 150 KB 落库——足够覆盖任何真实聊天输入，又不可能被
/// "一次粘贴"撑爆数据库。
///
/// ⚠️ 超限必须**报错拒发**，绝不能 `chars().take()` 静默截断：静默截断会让用户
/// 以为整段发出去了，实际对方只收到前半段，且本机不留任何痕迹（违反
/// AI_RULES INV-005「不允许静默丢失」）。与 `MAX_OUTGOING_IMAGE_BYTES`
/// 「超限一律报错拒发，绝不静默截断」的既有约定一致。
const MAX_MESSAGE_LEN: usize = 50_000;

/// 校验单条消息内容长度，超限返回面向用户的明确错误（不修改内容）。
fn check_message_content(content: String) -> Result<String, String> {
    let len = content.chars().count();
    if len > MAX_MESSAGE_LEN {
        return Err(format!(
            "消息过长（{len} 字符，上限 {MAX_MESSAGE_LEN} 字符）。请分段发送，或改用文件发送。"
        ));
    }
    Ok(content)
}
/// 粘贴/拖拽图片的解码后字节上限。Base64 解码后 ≈ 3/4 字符数，
/// 8 MiB 对应约 11 MB data URL，封框后仍远低于传输层 MAX_FRAME(64 MiB)。
/// 超限一律报错拒发，绝不静默截断。
const MAX_OUTGOING_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// 头像（base64 data URI）解码后字节上限。头像经前端中心裁剪 + 缩放到 512×512 后再上传，
/// 正常远小于 2 MiB；此处作为兜底，防止超大/恶意 data URL 撑爆 SQLite 与 UDP 发现广播。
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

use crate::crypto;
use crate::db;
use crate::discovery::routed::{
    encode_endpoints, parse_endpoint_addr, parse_endpoints, RoutedEndpoint, ROUTED_ENDPOINTS_KEY,
};
use crate::export;
use crate::logging::LogEntry;
use crate::network::transport::{
    broadcast_gossip, get_group_key, mark_pending_group_key, maybe_update_friend,
    resolve_member_x25519, resolve_nickname, try_send,
};
use crate::network::{self, file};
use crate::protocol::{GossipKind, Message, MsgKind, ShareEntry, FILE_CHUNK};
use crate::state::{
    AppState, Conversation, DeviceInfo, Friend, Group, GroupFile, InterfaceInfo, MessageRecord,
    Peer, PendingRequest, TopologyInfo, TransferInfo,
};
use crate::storage::cache_cleaner::{self, CachePolicy, CleanupReport};
use crate::transport::{ChannelStatus, TransportManager};

#[derive(Serialize)]
pub struct NetworkStatus {
    online: bool,
    bound_ip: Option<String>,
}

/// 存储占用与清理策略（设置页「存储与缓存」展示）。
///
/// ⚠️ 统计的是**真实落盘的媒体**（「文件存储目录」里接收的图片 / 文件）+ 聊天数据库，
/// 而不是历史遗留的 `cache/` 目录：P1 重构后媒体改落 downloads，`cache/` 已无写入方，
/// 只统计它会让「聊了半天还是 0 个文件」，用户完全看不懂。
#[derive(Serialize)]
pub struct CacheInfo {
    /// 已接收的图片 / 文件：文件数与合计占用
    media_count: usize,
    media_bytes: u64,
    /// 聊天记录数据库占用（含 -wal/-shm）
    db_bytes: u64,
    retention_days: Option<u32>,
    max_bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct GroupReadInfo {
    pub reader_id: String,
    pub last_read_ts: i64,
}

// ---------------- 本机信息与配置 ----------------

#[tauri::command]
pub fn get_device_info(state: State<'_, Arc<AppState>>) -> DeviceInfo {
    let s = state.inner();
    DeviceInfo {
        device_id: s.device_id.clone(),
        nickname: s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        avatar: s.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        device_type: crate::protocol::current_device_type().to_string(),
        tcp_port: s.tcp_port,
        online: s.network.lock().unwrap_or_else(|e| e.into_inner()).is_some(),
        x25519_pubkey: s.identity.x25519_public_b64(),
        ed25519_pubkey: s.identity.ed25519_public_b64(),
    }
}

/// 头像 data URL 解码后字节数；非法 base64 返回 usize::MAX（视为超限拒绝）。
fn avatar_decoded_len(data_url: &str) -> usize {
    let payload = data_url.split_once(',').map(|(_, p)| p).unwrap_or(data_url);
    STANDARD.decode(payload).map(|b| b.len()).unwrap_or(usize::MAX)
}

#[tauri::command]
pub async fn update_profile(
    state: State<'_, Arc<AppState>>,
    nickname: String,
    avatar: Option<String>,
) -> Result<DeviceInfo, String> {
    let s = state.inner();
    // 昵称长度保护：按字符截断（UTF-8 安全）
    let nickname: String = nickname.chars().take(MAX_NICKNAME_LEN).collect();
    // 头像大小兜底：超限直接拒绝，防止超大 base64 落库 / 撑爆 UDP 广播
    if let Some(a) = &avatar {
        if avatar_decoded_len(a) > MAX_AVATAR_BYTES {
            return Err("头像过大，请压缩到 2MB 以内".to_string());
        }
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, "nickname", &nickname).map_err(|e| e.to_string())?;
        if let Some(a) = &avatar {
            db::set_setting(&dbc, "avatar", a).map_err(|e| e.to_string())?;
        }
    }
    *s.nickname.lock().unwrap_or_else(|e| e.into_inner()) = nickname.clone();
    *s.avatar.lock().unwrap_or_else(|e| e.into_inner()) = avatar.clone();

    let msg = Message::UserInfo {
        device_id: s.device_id.clone(),
        nickname,
        avatar,
        device_type: crate::protocol::current_device_type().to_string(),
    };
    let links = s.links.lock().await;
    for link in links.values().flatten() {
        let _ = link.priority.send(msg.clone()).await;
    }
    drop(links);

    Ok(DeviceInfo {
        device_id: s.device_id.clone(),
        nickname: s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        avatar: s.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        device_type: crate::protocol::current_device_type().to_string(),
        tcp_port: s.tcp_port,
        online: s.network.lock().unwrap_or_else(|e| e.into_inner()).is_some(),
        x25519_pubkey: s.identity.x25519_public_b64(),
        ed25519_pubkey: s.identity.ed25519_public_b64(),
    })
}

/// 判断 IPv4 是否为常见 VPN / Clash / 虚拟网卡地址段。
///
/// 保守策略：只过滤**几乎不可能出现在真实局域网**的地址段；
/// 10.x.x.x 等模糊段不纳入过滤（真实 LAN 广泛使用 10/8）。
pub fn is_virtual_ip(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    // 198.18.0.0/15 — Clash / sing-box / v2ray fake-ip 段
    (o[0] == 198 && (o[1] == 18 || o[1] == 19))
    // 100.64.0.0/10 — WireGuard / CGNAT / Tailscale 常用段
    || (o[0] == 100 && o[1] >= 64 && o[1] <= 127)
    // 169.254.0.0/16 — link-local
    || (o[0] == 169 && o[1] == 254)
}

#[tauri::command]
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut out = Vec::new();
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in &ifs {
            if let if_addrs::IfAddr::V4(v4) = &i.addr {
                let ip = match i.ip() {
                    std::net::IpAddr::V4(v) => v,
                    _ => continue,
                };
                if ip.is_loopback() {
                    continue;
                }
                // is_lan：有广播地址（真实 LAN 的标志）+ 非 link-local + 非 VPN 地址段
                let has_broadcast = v4.broadcast.is_some();
                let is_link_local = ip.octets()[0] == 169 && ip.octets()[1] == 254;
                let not_vpn = !is_virtual_ip(&ip);
                let is_lan = has_broadcast && !is_link_local && not_vpn;
                out.push(InterfaceInfo {
                    name: i.name.clone(),
                    ip: ip.to_string(),
                    is_lan,
                });
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
    network::start(arc, bind_ip).await?;
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db::set_lan_enabled(&dbc, true).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn stop_network(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    network::stop(state.inner()).await;
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db::set_lan_enabled(&dbc, false).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_network_status(state: State<'_, Arc<AppState>>) -> NetworkStatus {
    let s = state.inner();
    let net = s.network.lock().unwrap_or_else(|e| e.into_inner());
    NetworkStatus {
        online: net.is_some(),
        bound_ip: net.as_ref().map(|n| n.bound_ip.clone()),
    }
}

#[tauri::command]
pub fn get_peers(state: State<'_, Arc<AppState>>) -> Vec<Peer> {
    let mut peers: Vec<Peer> = state
        .inner()
        .peers
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    peers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    peers
}

/// 按需探测周围在线节点：群发一次 `who_has`，等待约 1.5s 收集单播回复后返回当前节点表。
/// 仅在用户打开「添加好友」时调用，避免启动时持续全网扫描。
#[tauri::command]
pub async fn search_nearby_peers(state: State<'_, Arc<AppState>>) -> Result<Vec<Peer>, String> {
    let s = state.inner();
    // 触发一次探测（若网络已启动）
    let triggered = if let Some(tx) = s.probe.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
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
    let mut peers: Vec<Peer> = s.peers.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
    peers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    Ok(peers)
}

/// 从后台唤起并聚焦主窗口（冷启动首显 / 点击系统通知 / 消息点击唤起）。
#[cfg(desktop)]
#[tauri::command]
pub fn focus_window(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Err("主窗口不存在".to_string());
    };
    // 冷启动白闪修复：窗口 show 的第一帧会露出 WebView2 的默认背景色（tauri.conf.json
    // 写死浅色 #edf1f6）。暗色主题用户在骨架合成前会看到"闪一下白"。show 之前把窗口
    // 底色改成跟随主题（浅 #edf1f6 / 深 #0b1220，与 body 的 --gosslan-app-bg 一致），
    // 第一帧即正确底色而非浅色。dark_mode 是"解析后的结果"（跟随系统时已按系统偏好算好），
    // 冷启动直接可用。
    let dark = {
        let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, "dark_mode").map(|v| v == "1").unwrap_or(false)
    };
    let color = if dark {
        tauri::window::Color(11, 18, 32, 255) // #0b1220
    } else {
        tauri::window::Color(237, 241, 246, 255) // #edf1f6
    };
    let _ = win.set_background_color(Some(color));
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
    let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
    let node_count = peers.len();
    let rtts: Vec<u64> = peers.values().filter_map(|p| p.rtt_ms).collect();
    let avg_rtt_ms = if rtts.is_empty() {
        None
    } else {
        Some(rtts.iter().sum::<u64>() / rtts.len() as u64)
    };
    let relay_count = s.relay.lock().unwrap_or_else(|e| e.into_inner()).active_sends();
    let online = s.network.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    TopologyInfo {
        node_count,
        relay_count,
        avg_rtt_ms,
        online,
    }
}

// ---------------- 开发者诊断（隐藏面板用，只读不改网络行为） ----------------

/// 获取 Discovery 运行时诊断状态（供隐藏开发者面板展示）。
#[tauri::command]
pub fn get_discovery_diag(state: State<'_, Arc<AppState>>) -> crate::state::DiscoveryDiag {
    let s = state.inner();
    let net = s.network.lock().unwrap_or_else(|e| e.into_inner());
    let diag = s.diag.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut result = diag;
    if let Some(ref h) = *net {
        result.mode = if h.bound_ip == "0.0.0.0" {
            "auto".into()
        } else {
            "manual".into()
        };
        // auto 模式下 Discovery 实际绑定的是真实 LAN IP，而不是 0.0.0.0。
        // tcp_listen 仍使用用户配置的地址（TCP 监听地址）。
        result.bound_ip = if h.actual_bound_ip.is_empty() {
            h.bound_ip.clone()
        } else {
            h.actual_bound_ip.clone()
        };
        result.tcp_listen = format!("{}:{}", h.bound_ip, h.tcp_port);
        result.udp_port = crate::protocol::UDP_PORT;
    } else {
        result.mode = "offline".into();
    }
    // 附加最近事件
    result.recent_events = s.diag_events.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
    result
}

/// 获取候选网卡列表（含评分），供诊断面板展示 Discovery 自动选择逻辑的实际数据。
#[tauri::command]
pub fn get_interface_candidates() -> Vec<crate::state::InterfaceCandidate> {
    use std::net::Ipv4Addr;

    fn is_virtual_ip(ip: &Ipv4Addr) -> bool {
        let o = ip.octets();
        (o[0] == 198 && (o[1] == 18 || o[1] == 19))
            || (o[0] == 100 && o[1] >= 64 && o[1] <= 127)
            || (o[0] == 169 && o[1] == 254)
    }
    fn is_rfc1918(ip: &Ipv4Addr) -> bool {
        let o = ip.octets();
        (o[0] == 10) || (o[0] == 172 && o[1] >= 16 && o[1] <= 31) || (o[0] == 192 && o[1] == 168)
    }
    fn is_virtual_name(name: &str) -> bool {
        let n = name.to_lowercase();
        [
            "utun",
            "tun",
            "tap",
            "wg",
            "docker",
            "br-",
            "veth",
            "virbr",
            "vmnet",
            "vboxnet",
            "hyper-v",
            "hv_",
            "vethernet",
            "cf-",
            "clash",
            "wintun",
            "tailscale",
            "ts-",
            "ham",
            "vpn",
        ]
        .iter()
        .any(|p| n.contains(p))
    }

    let mut out = Vec::new();
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in &ifs {
            if let if_addrs::IfAddr::V4(v4) = &i.addr {
                let ip = match i.ip() {
                    std::net::IpAddr::V4(v) => v,
                    _ => continue,
                };
                if ip.is_loopback() {
                    continue;
                }
                let has_bc = v4.broadcast.is_some();
                let rfc = is_rfc1918(&ip);
                let virt_ip = is_virtual_ip(&ip);
                let virt_name = is_virtual_name(&i.name);
                let mut score = 0i32;
                if has_bc {
                    score += 10;
                }
                if rfc {
                    score += 5;
                }
                if virt_ip {
                    score -= 50;
                }
                if virt_name {
                    score -= 30;
                }
                out.push(crate::state::InterfaceCandidate {
                    name: i.name.clone(),
                    ip: ip.to_string(),
                    has_broadcast: has_bc,
                    broadcast: v4.broadcast.map(|b| b.to_string()),
                    is_rfc1918: rfc,
                    is_virtual: virt_ip || virt_name,
                    score,
                    selected: false, // 由调用方根据实际 bind_ip 设置
                });
            }
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.ip.cmp(&b.ip)));
    out
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
                // 绑定地址沿用用户已选网卡（settings.bind_ip），不再硬编码 0.0.0.0
                network::start_from_prefs(s.clone()).await?;
            } else {
                network::stop(s).await;
            }
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::set_lan_enabled(&dbc, enabled).ok();
            Ok(())
        }
        "bluetooth" => {
            let mut mgr = TransportManager::new(s.clone());
            if enabled {
                mgr.set_bluetooth_enabled(true).await
            } else {
                let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
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
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let retention = db::get_setting(&dbc, RETENTION_KEY)
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&d| d > 0);
    let max = db::get_setting(&dbc, MAX_BYTES_KEY)
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&m| m > 0);
    CachePolicy {
        retention_days: retention,
        max_bytes: max,
    }
}

/// 纳入统计与清理的目录：接收的图片/文件目录 + 历史遗留的 cache 目录。
/// 旧的 `cache/` 可能残留早期版本抽取的图片，一并纳入，避免"看不见也清不掉"。
fn media_dirs(s: &AppState) -> Vec<PathBuf> {
    let downloads = s
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    vec![downloads, s.cache_dir.clone()]
}

/// SQLite 数据库文件占用（含 -wal / -shm 两个伴随文件）。
fn db_file_bytes(s: &AppState) -> u64 {
    let base = s.db_path.to_string_lossy().to_string();
    let mut total = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        if let Ok(m) = std::fs::metadata(format!("{base}{suffix}")) {
            total += m.len();
        }
    }
    total
}

/// 存储占用与当前清理策略。
#[tauri::command(async)]
pub fn get_cache_info(state: State<'_, Arc<AppState>>) -> CacheInfo {
    let s = state.inner();
    let policy = load_policy(s);
    let (media_count, media_bytes) = cache_cleaner::usage(&media_dirs(s));
    CacheInfo {
        media_count,
        media_bytes,
        db_bytes: db_file_bytes(s),
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
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let d = retention_days.unwrap_or(0);
    db::set_setting(&dbc, RETENTION_KEY, &d.to_string()).map_err(|e| e.to_string())?;
    let m = max_bytes.unwrap_or(0);
    db::set_setting(&dbc, MAX_BYTES_KEY, &m.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

/// 立即执行一次清理：按保留时长 / 配额删除过期的图片与文件（含历史遗留 cache 目录），
/// 并对数据库执行 VACUUM。**不删除聊天文字**；被清理的图片/文件在历史消息里将无法再打开。
#[tauri::command(async)]
pub fn clean_cache_now(state: State<'_, Arc<AppState>>) -> CleanupReport {
    let s = state.inner();
    let policy = load_policy(s);
    let dirs = media_dirs(s);
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    cache_cleaner::clean(&dirs, policy, &*dbc)
}

// ---------------- 应用偏好设置（本地持久化） ----------------

/// 应用偏好：外观、网卡选择等。持久化到本地 SQLite，重启后恢复。
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme_color: Option<String>,
    pub font_family: Option<String>,
    pub dark_mode: Option<bool>,
    /// 外观模式："system" | "light" | "dark"。缺省视为 "system"（跟随系统）。
    /// 与 `dark_mode` 的关系：`appearance_mode` 是**用户意图**，`dark_mode` 是**解析后的结果**
    /// （跟随系统时由前端按系统偏好解析后回写），二者同时持久化，互不冲突。
    pub appearance_mode: Option<String>,
    /// 桌面通知开关（缺省视为开启——否则用户会漏消息且不知道有开关）。
    pub notify_enabled: Option<bool>,
    /// 通知是否显示消息正文（隐私：关掉后只显示"收到新消息"，锁屏/通知中心不泄内容）。
    pub notify_show_content: Option<bool>,
    /// 界面语言："zh-CN" | "en-US"。缺省视为 "zh-CN"。
    pub language: Option<String>,
    pub bind_ip: Option<String>,
    /// 聊天显示样式 JSON：{"preset":"classic","fontSize":"md","compact":true}
    pub chat_style: Option<String>,
    /// 对端样式表 JSON（device_id -> style JSON）。仅由后端在收到 ChatStyle 消息时写入，
    /// 前端只读；save_settings 忽略该字段。
    pub peer_styles: Option<String>,
    /// 中继授权策略："off" | "friends" | "allowlist" | "all"。
    ///
    /// 缺省/脏值 = `all` —— **与今天的行为完全一致**（多跳转发一直是无条件的）。
    /// 为什么默认不是更"安全"的 off：跨跳投递（A—B—C 且 A/C 无直连）依赖中间节点转发，
    /// 默认关掉会让已有拓扑静默丢消息（红线 §8.1 #3：不得在重构里顺手改变传播语义）。
    /// 想限制中继的用户在设置里显式选择。见 `mesh/relay_policy.rs` 与 ADR-0016。
    pub relay_policy: Option<String>,
    /// 中继白名单（JSON 字符串数组，`allowlist` 策略用）。
    pub relay_allowlist: Option<String>,
}

/// e2ee_enabled 键保留在 reset 链中仅为清理 v0.10.0 及更早版本的残留值；
/// v0.11.0 起 E2EE 恒开、不可关闭，该键不再被读写。
const SETTINGS_KEYS: [&str; 13] = [
    "theme_color",
    "font_family",
    "dark_mode",
    "appearance_mode",
    "notify_enabled",
    "notify_show_content",
    "language",
    "bind_ip",
    "chat_style",
    "e2ee_enabled",
    "lan_enabled",
    "relay_policy",
    "relay_allowlist",
];

/// appearance_mode 的合法取值：脏值一律忽略（宁可回落"跟随系统"，也不要写进库）。
const APPEARANCE_MODES: [&str; 3] = ["system", "light", "dark"];

/// language 的合法取值：脏值一律忽略（回落"跟随系统"）。
/// "system" = 前端按系统语言决定（zh* → 中文，其余 → 英文）。
const LANGUAGES: [&str; 3] = ["system", "zh-CN", "en-US"];

/// relay_policy 的合法取值（与 `mesh::relay_policy::RelayPolicy::as_str` 一一对应）。
const RELAY_POLICIES: [&str; 4] = ["off", "friends", "allowlist", "all"];

/// 把前端**解析后**的界面语言推给后端，用于重建 macOS 菜单栏（见 `menu.rs` 顶部注释）。
///
/// 为什么要这条命令：macOS 的菜单栏是原生控件，文案不归 WebView 管；而"跟随系统"
/// 的解析规则只在前端有一份。前端在启动完成与每次切换语言时各推一次。
///
/// 非 macOS 平台下这个模块整体不编译，所以这里必须 cfg 掉函数体（保留命令本身，
/// 让前端调用在其它平台也能拿到 Ok —— 前端不需要按平台分支）。
#[tauri::command]
pub fn set_ui_language(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::menu::apply(&app, crate::menu::UiLang::parse(&lang)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&app, &lang);
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    Settings {
        theme_color: db::get_setting(&dbc, "theme_color"),
        font_family: db::get_setting(&dbc, "font_family"),
        dark_mode: db::get_setting(&dbc, "dark_mode").map(|v| v == "1"),
        appearance_mode: db::get_setting(&dbc, "appearance_mode"),
        // 通知默认开启、默认显示正文：缺省时按 `Some(true)`，旧记录与未设置都能有合理行为。
        notify_enabled: db::get_setting(&dbc, "notify_enabled").map(|v| v != "0").or(Some(true)),
        notify_show_content: db::get_setting(&dbc, "notify_show_content").map(|v| v != "0").or(Some(true)),
        language: db::get_setting(&dbc, "language"),
        relay_policy: db::get_setting(&dbc, "relay_policy"),
        relay_allowlist: db::get_setting(&dbc, "relay_allowlist"),
        bind_ip: db::get_setting(&dbc, "bind_ip"),
        chat_style: db::get_setting(&dbc, "chat_style"),
        peer_styles: db::get_setting(&dbc, "chat_peer_styles"),
    }
}

#[tauri::command]
pub fn save_settings(state: State<'_, Arc<AppState>>, settings: Settings) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = settings.theme_color {
        db::set_setting(&dbc, "theme_color", &v).map_err(|e| e.to_string())?;
    }
    if let Some(v) = settings.font_family {
        db::set_setting(&dbc, "font_family", &v).map_err(|e| e.to_string())?;
    }
    if let Some(v) = settings.dark_mode {
        db::set_setting(&dbc, "dark_mode", if v { "1" } else { "0" }).map_err(|e| e.to_string())?;
    }
    if let Some(v) = settings.appearance_mode {
        if APPEARANCE_MODES.contains(&v.as_str()) {
            db::set_setting(&dbc, "appearance_mode", &v).map_err(|e| e.to_string())?;
        }
    }
    if let Some(v) = settings.notify_enabled {
        db::set_setting(&dbc, "notify_enabled", if v { "1" } else { "0" }).map_err(|e| e.to_string())?;
    }
    if let Some(v) = settings.notify_show_content {
        db::set_setting(&dbc, "notify_show_content", if v { "1" } else { "0" }).map_err(|e| e.to_string())?;
    }
    if let Some(v) = settings.language {
        if LANGUAGES.contains(&v.as_str()) {
            db::set_setting(&dbc, "language", &v).map_err(|e| e.to_string())?;
        }
    }
    if let Some(v) = settings.bind_ip {
        db::set_setting(&dbc, "bind_ip", &v).map_err(|e| e.to_string())?;
    }
    if let Some(v) = settings.chat_style {
        db::set_setting(&dbc, "chat_style", &v).map_err(|e| e.to_string())?;
    }
    // 中继授权：脏值一律忽略（宁可维持现状，也不要写进库让传播语义变得不可预期）
    if let Some(v) = settings.relay_policy.as_deref() {
        if RELAY_POLICIES.contains(&v) {
            db::set_setting(&dbc, "relay_policy", v).map_err(|e| e.to_string())?;
        }
    }
    if let Some(v) = settings.relay_allowlist.as_deref() {
        // 只接受合法 JSON 数组：写进脏值会让 RelayConfig::parse 静默退化成空表，
        // 用户会看到"白名单明明填了却不生效"。
        if serde_json::from_str::<Vec<String>>(v).is_ok() {
            db::set_setting(&dbc, "relay_allowlist", v).map_err(|e| e.to_string())?;
        }
    }
    // 回读一遍写进内存缓存（转发路径热读，不能每次去锁 SQLite）。
    // ⚠️ 先放掉 DB 锁再更新缓存：避免与转发路径形成锁顺序纠缠。
    let relay = crate::mesh::relay_policy::RelayConfig::parse(
        db::get_setting(&dbc, "relay_policy").as_deref(),
        db::get_setting(&dbc, "relay_allowlist").as_deref(),
    );
    drop(dbc);
    state.set_relay_policy_config(relay);
    Ok(())
}

/// 恢复默认设置：清除所有用户可配置设置（外观、昵称、头像、网卡、缓存策略等）。
/// 保留 device_id、x25519_secret、ed25519_secret、好友列表、聊天记录。
#[tauri::command]
pub fn reset_settings(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    for key in SETTINGS_KEYS.iter().chain([
        &RETENTION_KEY,
        &MAX_BYTES_KEY,
        &"bt_enabled",
        &"chat_peer_styles",
        &"nickname",
        &"avatar",
    ]) {
        db::delete_setting(&dbc, key).map_err(|e| e.to_string())?;
    }
    // 「恢复默认」也清掉了 relay_policy / relay_allowlist ⇒ 内存缓存必须回到默认（All），
    // 否则用户点了恢复默认、行为却还是旧的限制策略（要重启才生效）。
    drop(dbc);
    state.set_relay_policy_config(crate::mesh::relay_policy::RelayConfig::default());
    Ok(())
}

/// 广播本机聊天样式到所有已连接节点（样式变更即调用，对方设备与好友同步收到）。
#[tauri::command]
pub async fn broadcast_chat_style(
    state: State<'_, Arc<AppState>>,
    style: String,
) -> Result<(), String> {
    let s = state.inner();
    let msg = Message::ChatStyle {
        from: s.device_id.clone(),
        to: None,
        style,
    };
    let links = s.links.lock().await;
    for link in links.values().flatten() {
        let _ = link.priority.send(msg.clone()).await;
    }
    Ok(())
}

// ---------------- 好友 ----------------

#[tauri::command]
pub fn get_friends(state: State<'_, Arc<AppState>>) -> Vec<Friend> {
    let s = state.inner();
    let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
    // 同时检查活跃 TCP 链接：链路存活但 peer 已被 discovery sweep 清掉时，
    // 仍应显示在线，避免「实际可通信但 UI 显示离线」。
    // ⚠️ 只算**非空** Vec（与 `sweep_peers` / `has_link` 同一口径）：
    // 残留的空 key 会让好友**永久显示在线**。根因已在 reader_loop 修掉，这里是防线。
    let active_links: std::collections::HashSet<String> = s
        .links
        .try_lock()
        .map(|l| {
            l.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut friends = db::list_friends(&dbc).unwrap_or_default();
    for f in friends.iter_mut() {
        f.online = peers.contains_key(&f.device_id) || active_links.contains(&f.device_id);
        // 设备类型从 peers 表现场读取（Hello/UserInfo/Presence 都会更新它）。
        f.device_type = peers
            .get(&f.device_id)
            .map(|p| p.device_type.clone())
            .unwrap_or_default();
    }
    friends
}

/// 删除好友（保留聊天记录；对方仍会出现在扫描列表，可重新添加）。
#[tauri::command]
pub async fn remove_friend(state: State<'_, Arc<AppState>>, peer_id: String) -> Result<(), String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::remove_friend(&dbc, &peer_id).map_err(|e| e.to_string())?;
        db::delete_file_outbox_for_peer(&dbc, &peer_id).ok();
    }
    // 通知对方解除好友关系（对方收到后也会删除本机好友行）
    let msg = Message::FriendRemove {
        from: s.device_id.clone(),
        to: peer_id.clone(),
    };
    let _ = try_send(s, &peer_id, &msg).await;
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
pub async fn send_friend_request(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
) -> Result<(), String> {
    let s = state.inner();
    if peer_id.is_empty() || peer_id == s.device_id {
        return Err("不能向自己发送好友申请".to_string());
    }
    // 目标必须在 peers 表（announce / Presence 学到），且需有 X25519 公钥才能 E2EE 加密。
    let target_pubkey = {
        let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(&peer_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(target_pubkey) = target_pubkey else {
        return Err("未找到该节点或缺少其公钥，请先重新扫描".to_string());
    };
    let nickname = s
        .nickname
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let avatar = s.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // E2EE 加密好友申请内容（昵称/头像）；from/to 已在信封 sender_id / target 里。
    let payload =
        serde_json::json!({ "from_nickname": nickname, "from_avatar": avatar }).to_string();
    let shared = crypto::shared_secret(&s.identity.x25519_secret, &target_pubkey)
        .ok_or("密钥交换失败")?;
    let sealed = crypto::seal(&shared, payload.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let mut env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::FriendRequest,
            None,
            None,
            &payload_b64,
            db::now_ms(),
            0,
        )
    };
    // 定向目标 + 重签（target 参与 signing_bytes）。
    env.target = Some(peer_id.clone());
    env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
    // 目标直连 → 只发它（精确）；否则广播，靠中间节点按 target 定向转发（跨跳）。
    if s.has_link(&peer_id).await {
        try_send(s, &peer_id, &Message::Gossip { envelope: env }).await?;
    } else {
        broadcast_gossip(s, env).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn respond_friend_request(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    accept: bool,
) -> Result<(), String> {
    let s = state.inner();
    if !s.pending_requests.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&peer_id) {
        return Err("好友申请不存在或已处理".to_string());
    }
    if accept {
        let name = resolve_nickname(s, &peer_id);
        {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::add_friend(&dbc, &peer_id, &name, None).ok();
            db::ensure_conversation(&dbc, &peer_id, "single", &name, None).ok();
        }
        // 补写 peers 表已有的公钥到 friends 表：accept 路径此前不写公钥，
        // 而建链（Hello）早于加好友、公钥不变时 key_changed 不触发补写，
        // 导致 friends 公钥永久缺失 → 群密钥分发被静默跳过。
        // 与 transport.rs 中 FriendAccept 接收路径的补写行为一致。
        maybe_update_friend(s, &peer_id, &name, None);
        // FriendAccept 改走 Gossip 定向：跨跳场景下 try_send 直连发不出去。
        // 对方公钥优先从 peers 表读（接收 FriendRequest 时已 upsert_peer 记录），
        // friends 表兜底（maybe_update_friend 可能已持久化）。
        let target_pubkey = {
            let from_peers = {
                let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
                peers.get(&peer_id).and_then(|p| p.x25519_pubkey.clone())
            };
            let from_friends = {
                let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend_x25519(&dbc, &peer_id)
            };
            from_peers.or(from_friends)
        };
        if let Some(target_pubkey) = target_pubkey {
            let shared = crypto::shared_secret(&s.identity.x25519_secret, &target_pubkey)
                .ok_or("密钥交换失败")?;
            let sealed = crypto::seal(&shared, b"{}").ok_or("加密失败")?;
            let payload_b64 = STANDARD.encode(&sealed);
            let mut env = {
                let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
                gossip.build_envelope(
                    &s.identity,
                    &s.device_id,
                    GossipKind::FriendAccept,
                    None,
                    None,
                    &payload_b64,
                    db::now_ms(),
                    0,
                )
            };
            env.target = Some(peer_id.clone());
            env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
            if s.has_link(&peer_id).await {
                try_send(s, &peer_id, &Message::Gossip { envelope: env }).await?;
            } else {
                broadcast_gossip(s, env).await;
            }
        } else {
            // 缺对端公钥时**绝不静默**：本地已加好友，但回执发不出去会导致好友关系
            // 单边成立。打日志留痕（对方 Presence 尚未到达 / 已被 sweep 清理）。
            s.logger.warn(
                "friend",
                format!("同意好友但缺对端公钥，FriendAccept 未发送 peer={peer_id}"),
            );
        }
        s.pending_requests.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer_id);
        let _ = s.app.emit("friend-accepted", &peer_id);
    } else {
        // 拒绝回执：跨跳（无直连）时 try_send 会失败，但**绝不因此阻塞本地清理**——
        // 否则「拒绝」发不出去会导致 pending 不被删除、申请「清掉又冒出来」。
        let msg = Message::FriendReject {
            from: s.device_id.clone(),
            to: peer_id.clone(),
        };
        let _ = try_send(s, &peer_id, &msg).await;
        s.pending_requests.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer_id);
        let _ = s.app.emit("friend-rejected", &peer_id);
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

    let msg_kind = match kind.as_str() {
        "text" => MsgKind::Text,
        "code" => MsgKind::Code,
        "file" => MsgKind::File,
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
            .unwrap()
            .get(&friend_id)
            .and_then(|p| p.x25519_pubkey.clone());
        match from_db.or(from_peers) {
            Some(k) => Some(k),
            None => {
                let triggered = if let Some(tx) = s.probe.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
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
                    .unwrap()
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
    let preview = preview(&kind, &content);

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
        status: "sent".to_string(),
    };
    // 一律写离线队列兜底（INSERT OR IGNORE 按 msg_id 幂等）：直连链路存在但已失效
    // （半开 TCP）时 broadcast 会静默丢包，此前只在「无链路」时入队导致消息永久丢失。
    // Ack 到达后由 transport.rs 删除该行；若链路中断，对方上线建链（Hello）或心跳
    // 会触发 flush_outbox 自动补发，接收方按 msg_id 去重不会重复入库。
    let queued = Message::ChatMessage {
        msg_id: msg_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
        kind: msg_kind,
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
    if s.has_link(&friend_id).await {
        let _ = try_send(s, &friend_id, &Message::Gossip { envelope: env }).await;
    } else {
        broadcast_gossip(s, env).await;
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

/// 读取会话的「当前链路」快照（最近一条消息的链路 + 中间节点数）。
/// 前端聊天窗口据此显示连接图标（LAN / 桥接 / 蓝牙 + 节点数）。
#[tauri::command]
pub fn get_conv_link(
    state: State<'_, Arc<AppState>>,
    conv_id: String,
) -> Option<crate::state::LinkState> {
    state
        .inner()
        .conv_link
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&conv_id)
        .cloned()
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

#[tauri::command]
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
#[tauri::command]
pub fn ensure_conversation(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
) -> Result<Conversation, String> {
    let s = state.inner();
    let name = resolve_nickname(s, &friend_id);
    let avatar = {
        let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(&friend_id).and_then(|p| p.avatar.clone())
    };
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::ensure_conversation(&dbc, &friend_id, "single", &name, avatar.as_deref())
            .map_err(|e| e.to_string())?;
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
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::mark_read(&dbc, &conv_id).map_err(|e| e.to_string())?;
    }
    if !conv_id.starts_with("group:") {
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
#[tauri::command]
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

// ---------------- 群聊（群密钥 + Gossip） ----------------

#[tauri::command]
pub fn create_group(
    state: State<'_, Arc<AppState>>,
    name: String,
    members: Vec<String>,
) -> Result<Group, String> {
    let s = state.inner();
    // 群名称长度保护：按字符截断（UTF-8 安全）
    let name: String = name
        .chars()
        .take(MAX_GROUP_NAME_LEN)
        .collect::<String>()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("群名称不能为空".to_string());
    }
    let id = format!("g-{}", Uuid::new_v4());
    let mut all = members;
    all.retain(|m| !m.is_empty() && m != &s.device_id);
    all.sort();
    all.dedup();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        for member in &all {
            if db::get_friend(&dbc, member).is_none() {
                return Err("只能把好友加入群聊".to_string());
            }
        }
    }
    if !all.contains(&s.device_id) {
        all.push(s.device_id.clone());
    }

    // 生成群密钥并持久化
    let key = crypto::random_key();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO groups(id, name, creator, created_at) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, s.device_id, db::now_ms()],
        )
        .map_err(|e| e.to_string())?;
        for member in &all {
            tx.execute(
                "INSERT OR IGNORE INTO group_members(group_id, device_id) VALUES(?1, ?2)",
                rusqlite::params![id, member],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)",
            rusqlite::params![format!("gk:{id}"), STANDARD.encode(key)],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO conversations(id, kind, name, avatar, unread, updated_at)
             VALUES(?1, 'group', ?2, NULL, 0, ?3)",
            rusqlite::params![format!("group:{id}"), name, db::now_ms()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    s.group_keys.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), key);

    Ok(Group {
        id,
        name,
        creator: s.device_id.clone(),
        members: all,
    })
}

/// 向群成员分发群密钥（用各成员公钥 ECDH 加密）。
/// 同时携带群名与成员列表：成员端据此建本地群记录，否则群名会兜底成「群聊 g-xxxx」。
#[tauri::command]
pub async fn distribute_group_key(
    state: State<'_, Arc<AppState>>,
    group_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;
    // 同时取群名与成员：成员端靠它建立/刷新本地群记录（含成员表）
    let (group_name, members) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| (g.name, g.members))
            .unwrap_or_default()
    };
    let clock = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &format!("group:{group_id}"))
    };
    for m in &members {
        if m == &s.device_id {
            continue;
        }
        // peers 优先、friends 回落：peers 由 announce/Hello 实时维护，
        // friends 表公钥可能因 accept 路径未补写而缺失（曾致 GroupKey 静默跳过）
        let Some(pubkey) = resolve_member_x25519(s, m) else {
            continue;
        };
        let Some(shared) = crypto::shared_secret(&s.identity.x25519_secret, &pubkey) else {
            continue;
        };
        let Some(sealed) = crypto::seal(&shared, &key) else {
            continue;
        };
        let msg = Message::GroupKey {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            to: m.clone(),
            key: STANDARD.encode(&sealed),
            group_name: group_name.clone(),
            members: members.clone(),
            clock,
        };
        if let Err(_) = try_send(s, m, &msg).await {
            // 目标成员尚无 TCP link（建群时 ensure_link 可能尚未执行）：
            // 不再静默丢弃，登记待发，由建链 / Hello / 心跳的
            // flush_pending_group_keys 补发（与 redistribute_group_keys 同一机制）。
            let mut pending = s.pending_group_keys.lock().unwrap_or_else(|e| e.into_inner());
            mark_pending_group_key(&mut pending, m, &group_id);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_groups(state: State<'_, Arc<AppState>>) -> Vec<Group> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::list_groups(&dbc).unwrap_or_default()
}

#[tauri::command]
pub fn get_group_reads(state: State<'_, Arc<AppState>>, group_id: String) -> Vec<GroupReadInfo> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::list_group_reads(&dbc, &group_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(reader_id, last_read_ts)| GroupReadInfo {
            reader_id,
            last_read_ts,
        })
        .collect()
}

/// 重命名群：仅创建者可操作。本地改名 + 同步会话标题后，广播给全部成员。
#[tauri::command]
pub async fn rename_group(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    name: String,
) -> Result<(), String> {
    let s = state.inner();
    let name: String = name.chars().take(MAX_GROUP_NAME_LEN).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("群名称不能为空".to_string());
    }
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以修改群名称".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::rename_group(&dbc, &group_id, &name).map_err(|e| e.to_string())?;
    }
    for m in &group.members {
        if m == &s.device_id {
            continue;
        }
        let msg = Message::GroupRename {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            name: name.clone(),
        };
        let _ = try_send(s, m, &msg).await;
    }
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 向成员列表里的每一位重发当前群密钥（携带群名 + 最新成员表）。
async fn resend_group_key_to(s: &AppState, group_id: &str, members: &[String], key: [u8; 32]) {
    let group_name = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, group_id)
            .map(|g| g.name)
            .unwrap_or_default()
    };
    let clock = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_clock(&dbc, &format!("group:{group_id}"))
    };
    for m in members {
        if m == &s.device_id {
            continue;
        }
        // peers 优先、friends 回落（与 distribute_group_key 同一来源策略）
        let Some(pubkey) = resolve_member_x25519(s, m) else {
            continue;
        };
        let Some(shared) = crypto::shared_secret(&s.identity.x25519_secret, &pubkey) else {
            continue;
        };
        let Some(sealed) = crypto::seal(&shared, &key) else {
            continue;
        };
        let msg = Message::GroupKey {
            group_id: group_id.to_string(),
            from: s.device_id.clone(),
            to: m.clone(),
            key: STANDARD.encode(&sealed),
            group_name: group_name.clone(),
            members: members.to_vec(),
            clock,
        };
        if let Err(_) = try_send(s, m, &msg).await {
            // 目标成员尚无 TCP link：登记待发，由建链 / Hello / 心跳的
            // flush_pending_group_keys 补发（与 redistribute_group_keys 同一机制）。
            let mut pending = s.pending_group_keys.lock().unwrap_or_else(|e| e.into_inner());
            mark_pending_group_key(&mut pending, m, group_id);
        }
    }
}

/// 加人入群：仅创建者。本地落成员后，用**当前**群密钥重发给全体成员（含新成员）。
///
/// ⚠️ 这里刻意**不轮换**群密钥：
/// - 加人没有前向保密收益——新成员本来就没有旧密钥，转不转旧消息他都解不开；
/// - `handle_group_key` 只接受**群主**分发的密钥（防止成员伪造密钥劫持群聊），
///   所以一旦轮换，新密钥就只存在于群主本机。群主一旦离线，其他成员永远拿不到，
///   整群消息都无法解密。不轮换则密钥始终是全体成员都持有的那一个，与群主是否在线无关。
/// - 轮换只在「移除成员」时做（撤销被移除者的解密能力），那个时机群主必然在线。
#[tauri::command]
pub async fn group_add_member(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    device_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以添加成员".to_string());
    }
    if group.members.contains(&device_id) {
        return Err("该成员已在群中".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &device_id).is_none() {
            return Err("只能添加好友入群".to_string());
        }
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::add_group_member(&dbc, &group_id, &device_id).map_err(|e| e.to_string())?;
    }
    let key = get_group_key(s, &group_id)
        .await
        .ok_or_else(|| "群密钥缺失".to_string())?;
    let current = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.members)
            .unwrap_or_default()
    };
    // 现有成员收到的是同一把密钥（幂等刷新），新成员借此首次拿到密钥
    resend_group_key_to(s, &group_id, &current, key).await;
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 移人出群：仅创建者。轮换群密钥发给剩余成员，并向被移除者发 GroupMemberRemoved。
#[tauri::command]
pub async fn group_remove_member(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    device_id: String,
) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以移除成员".to_string());
    }
    if device_id == s.device_id {
        return Err("不能移除自己".to_string());
    }
    if !group.members.contains(&device_id) {
        return Err("该成员不在群中".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::remove_group_member(&dbc, &group_id, &device_id).map_err(|e| e.to_string())?;
        // 被移除者不再属于该群：清掉仍指向它的待补发群消息，避免重连时向群外成员投递。
        db::delete_group_outbox_for_peer_in_group(&dbc, &group_id, &device_id).ok();
    }
    // 轮换群密钥：被移除者失去解密能力
    let key = crypto::random_key();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, &format!("gk:{group_id}"), &STANDARD.encode(key)).ok();
    }
    s.group_keys.lock().unwrap_or_else(|e| e.into_inner()).insert(group_id.clone(), key);
    let remaining = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.members)
            .unwrap_or_default()
    };
    resend_group_key_to(s, &group_id, &remaining, key).await;
    let removed_msg = Message::GroupMemberRemoved {
        group_id: group_id.clone(),
        from: s.device_id.clone(),
        to: device_id.clone(),
    };
    // ① 通知**被移除者本人**清理本地群
    let _ = try_send(s, &device_id, &removed_msg).await;
    // ② **同时通知其余成员**：此前只发给被移除者，而接收端对 `to != 自己` 直接 return，
    //    两头都断 —— 表现为「群里其他人打开群，成员没变少、也没有任何提示」。
    //    现在其余成员收到后同步成员表 + 落一条群内系统消息。
    for m in &remaining {
        if m == &s.device_id || m == &device_id {
            continue;
        }
        let _ = try_send(s, m, &removed_msg).await;
    }
    // ③ 群主自己也要看到这条系统消息（别人靠 ② 各自插入）
    let name = resolve_nickname(s, &device_id);
    crate::network::transport::insert_group_system_message(
        s,
        &group_id,
        &crate::network::transport::group_member_removed_text(s, &name),
    );
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 转让群主：仅**当前**群主可发起，目标必须是群成员。
/// 本地更新创建者后广播 `GroupCreatorChanged` 给全体成员（含新群主本人）。
/// 用于群主更换设备/卸载前移交管理权，避免群永久失去改名/加人/踢人能力。
#[tauri::command]
pub async fn transfer_group_creator(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    new_creator: String,
) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator != s.device_id {
        return Err("只有群创建者可以转让群主".to_string());
    }
    if new_creator == s.device_id {
        return Err("不能把群主转让给自己".to_string());
    }
    if !group.members.contains(&new_creator) {
        return Err("只能转让给群成员".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_group_creator(&dbc, &group_id, &new_creator).map_err(|e| e.to_string())?;
    }
    for m in &group.members {
        if m == &s.device_id {
            continue;
        }
        let msg = Message::GroupCreatorChanged {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
            to: new_creator.clone(),
        };
        let _ = try_send(s, m, &msg).await;
    }
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

/// 退出群聊：群主须先转让（否则该群会永久失去管理权）。
/// 退群后清理本地群记录 / 会话 / 群密钥，并广播 `GroupMemberLeft` 让其余成员更新成员表。
/// 复用与「被移出群」同一套本地清理路径（`db::delete_group`）。
#[tauri::command]
pub async fn leave_group(state: State<'_, Arc<AppState>>, group_id: String) -> Result<(), String> {
    let s = state.inner();
    let group = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?
    };
    if group.creator == s.device_id {
        return Err("群主退群前请先转让群主".to_string());
    }
    // 先通知其余成员（本地记录删除前取成员表）；对方离线时消息会丢失，
    // 但成员表也会随后续群消息（Gossip group_members / GroupKey）自愈。
    for m in &group.members {
        if m == &s.device_id {
            continue;
        }
        let msg = Message::GroupMemberLeft {
            group_id: group_id.clone(),
            from: s.device_id.clone(),
        };
        let _ = try_send(s, m, &msg).await;
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_group(&dbc, &group_id).map_err(|e| e.to_string())?;
        let _ = dbc.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![format!("gk:{group_id}")],
        );
    }
    s.group_keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&group_id);
    let _ = s.app.emit("groups-updated", &group_id);
    Ok(())
}

// ---------------- 自绘标题栏：窗口控制 ----------------

/// 最小化主窗口。
#[cfg(desktop)]
#[tauri::command]
pub fn window_minimize(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

/// 移动端没有独立窗口概念，最小化由系统接管。
#[cfg(mobile)]
#[tauri::command]
pub fn window_minimize(_app: tauri::AppHandle) {}

/// 切换窗口最大化，返回切换后的状态。
#[cfg(desktop)]
#[tauri::command]
pub fn window_toggle_maximize(app: tauri::AppHandle) -> bool {
    let Some(w) = app.get_webview_window("main") else {
        return false;
    };
    match w.is_maximized() {
        Ok(true) => {
            let _ = w.unmaximize();
            false
        }
        _ => {
            let _ = w.maximize();
            true
        }
    }
}

/// 移动端窗口始终铺满屏幕，等价于「不可再最大化」。
#[cfg(mobile)]
#[tauri::command]
pub fn window_toggle_maximize(_app: tauri::AppHandle) -> bool {
    false
}

/// 返回窗口当前是否最大化。
#[tauri::command]
pub fn window_is_maximized(app: tauri::AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_maximized().ok())
        .unwrap_or(false)
}

/// 切换窗口全屏，返回切换后的状态。
/// 用于 macOS 绿灯的 option-click（HIG：缩放按钮按住 Option 即进入/退出全屏）。
#[cfg(desktop)]
#[tauri::command]
pub fn window_toggle_fullscreen(app: tauri::AppHandle) -> bool {
    let Some(w) = app.get_webview_window("main") else {
        return false;
    };
    match w.is_fullscreen() {
        Ok(true) => {
            let _ = w.set_fullscreen(false);
            false
        }
        _ => {
            let _ = w.set_fullscreen(true);
            true
        }
    }
}

/// 移动端无"全屏"概念（窗口本就铺满屏幕），返回 false。
#[cfg(mobile)]
#[tauri::command]
pub fn window_toggle_fullscreen(_app: tauri::AppHandle) -> bool {
    false
}

#[tauri::command]
pub fn window_close(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub async fn send_group_message(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    content: String,
    kind: String,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let kind_enum = match kind.as_str() {
        "text" => MsgKind::Text,
        "code" => MsgKind::Code,
        _ => return Err("群聊不支持该消息类型".to_string()),
    };
    let content = check_message_content(content)?;
    let ts = db::now_ms();
    let conv_id = format!("group:{group_id}");
    let seq = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, &conv_id).map_err(|e| format!("逻辑时钟推进失败：{e}"))?
    };
    // 把群名 + 创建者 + 当前成员一并带上：跨端成员即便从未收到 GroupKey、
    // 只凭这条群消息也能在本地正确建群（含成员表），成员面板因此不为空。
    let group_meta = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id).map(|g| (g.name, g.creator, g.members))
    };
    let (group_name, group_creator, group_members) = match group_meta {
        Some((n, c, m)) => (n, Some(c), m),
        None => return Err("群不存在".to_string()),
    };
    if !group_members.contains(&s.device_id) {
        return Err("你已不在该群中".to_string());
    }
    let key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;
    let preview = preview(&kind, &content);

    // 群密钥加密 + Gossip 信封（E2EE 恒开：载荷用群密钥 ChaCha20-Poly1305 加密）
    let plaintext =
        serde_json::json!({ "kind": kind_enum.as_str(), "content": content }).to_string();
    let sealed = crypto::seal_symmetric(&key, plaintext.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        let mut env = gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::Group,
            Some(group_id.clone()),
            Some(group_name.clone()),
            &payload_b64,
            ts,
            seq,
        );
        env.group_creator = group_creator;
        env.group_members = group_members.clone();
        // group_creator / group_members 属于签名材料（GossipEnvelope::signing_bytes），
        // 而 build_envelope 内部已按「尚未填值」的状态算过 message_id 与 sender_sig。
        // 若此处不重算重签，接收端 verify_envelope 会用最终字段重新计算签名材料，
        // 与旧签名不一致 → 验签失败 → handle_gossip 静默丢弃群消息（群聊收不到的根因）。
        // compute_message_id 只依赖 sender_id + ts + payload，重算后 message_id 不变，
        // 与既有协议语义保持一致。
        env.compute_message_id();
        env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
        env
    };
    // 信封 encrypted 默认 true（build_envelope 内置），无需改写

    // 本地落库：msg_id 统一用 envelope.message_id（与单聊发送路径一致），
    // 保证同一条群消息在本地记录 / Gossip 投递 / 接收端落库三处身份一致。
    let rec = MessageRecord {
        id: 0,
        msg_id: env.message_id.clone(),
        conv_id: conv_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: group_id.clone(),
        kind: kind_enum.as_str().to_string(),
        content: content.clone(),
        ts,
        seq,
        status: "sent".to_string(),
    };
    // 群消息与单聊一样需要可靠投递：本地落库 + 每个成员的 outbox 在同一事务里完成，
    // 再由建链 / Hello / 心跳触发 flush_group_outbox 补发，收到 GroupAck 才删行。
    let gossip_msg = Message::Gossip {
        envelope: env.clone(),
    };
    let payload = serde_json::to_string(&gossip_msg).map_err(|e| e.to_string())?;
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        db::insert_message(&tx, &rec).map_err(|e| format!("消息写入失败：{e}"))?;
        db::touch_conversation(&tx, &conv_id, "group", &group_name, None, &preview, 0)
            .map_err(|e| format!("会话写入失败：{e}"))?;
        for member in &group_members {
            if member == &s.device_id {
                continue;
            }
            db::insert_group_outbox(&tx, &rec.msg_id, &group_id, member, &payload)
                .map_err(|e| format!("群消息入队失败：{e}"))?;
        }
        tx.commit().map_err(|e| format!("群消息写入失败：{e}"))?;
    }

    broadcast_gossip(s, env).await;

    Ok(rec)
}

// ---------------- 群文件（Offer / session-key 阶段） ----------------

/// 发起群文件（本阶段只建立 Offer 与 file session key，不含分片传输）。
///
/// 流程：校验发起者是群成员 → 实时读取当前成员快照 → 事务内创建
/// group_files + 全部 recipient 行（避免半完成状态）→ 生成随机 file_key
/// 存内存 → 对可达成员发送 GroupFileOffer（群密钥封装 file_key）→
/// 流式读取文件、逐 256KB 分片 AEAD 加密后向全部可达 recipient 发送
/// GroupFileChunk（seq 从 0 严格递增）。不可达成员保持 pending。
#[tauri::command]
pub async fn send_group_file(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    path: String,
) -> Result<String, String> {
    let s = state.inner();

    // 文件校验：存在 + 普通文件；size/name 取自本地 metadata，不进入协议
    let p = std::path::PathBuf::from(&path);
    let meta = std::fs::metadata(&p).map_err(|e| format!("文件不存在或不可读：{e}"))?;
    if !meta.is_file() {
        return Err("只能发送普通文件".to_string());
    }
    let size = meta.len();
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    // 文件级 SHA-256：256KB 分块流式计算放阻塞线程池（不整读内存、不卡 async runtime）
    let p_sha = p.clone();
    let sha256 = tokio::task::spawn_blocking(move || file::sha256_file_hex(&p_sha))
        .await
        .map_err(|e| e.to_string())??;

    // 发起者必须是群成员（本地群存在）
    let members: Vec<String> = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let group = db::get_group(&dbc, &group_id).ok_or("群不存在")?;
        if !group.members.contains(&s.device_id) {
            return Err("你不是该群成员".to_string());
        }
        // 成员快照：创建时当前群成员（不含自己），作为 recipient 集合
        group.members.into_iter().filter(|m| m != &s.device_id).collect()
    };
    if members.is_empty() {
        return Err("群内没有其他成员".to_string());
    }

    let transfer_id = Uuid::new_v4().to_string();

    // 事务：group_files + 全部 recipient 行一次写入，避免半完成状态
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let gf = GroupFile {
            transfer_id: transfer_id.clone(),
            group_id: group_id.clone(),
            sender_id: s.device_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            status: "pending".to_string(),
            created_at: db::now_ms(),
        };
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        db::insert_group_file(&tx, &gf).map_err(|e| e.to_string())?;
        for m in &members {
            db::insert_group_file_recipient(&tx, &transfer_id, m).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    // 群密钥获取与 file_key 封装先于运行态写入：任何失败都不残留内存状态
    let group_key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;

    // 随机 file session key：一个 transfer 只生成一次（CSPRNG，仅内存）
    let file_key = crypto::random_key();
    let sealed_file_key = STANDARD.encode(
        crypto::seal_symmetric(&group_key, &file_key).ok_or("封装文件密钥失败")?,
    );
    s.group_file_keys
        .lock()
        .unwrap()
        .insert(transfer_id.clone(), file_key);
    // 密封 file_key 持久化（群密钥封装，非明文）：重启后离线 pending
    // 群文件的投递仍能恢复 file_key（群密钥 gk:% 本身保留）
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, &format!("gfk:{transfer_id}"), &sealed_file_key).ok();
    }

    // 发送者本地气泡先落库：无论成员当前是否在线，用户看到的都是「发送中/待投递」，
    // 而不是一个报错后又偷偷排队的隐藏任务。
    // 图片文件保持 kind="image"，预览摘要为 [图片]，其余走 kind="file"。
    let subtype = file::classify_file_subtype(&name);
    let kind = if subtype == "image" { "image" } else { "file" };
    let content =
        serde_json::json!({ "name": name, "path": path, "size": size, "sha256": sha256, "subtype": subtype })
            .to_string();
    let conv_id = format!("group:{group_id}");
    let seq = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, &conv_id).unwrap_or(1)
    };
    let rec = MessageRecord {
        id: 0,
        msg_id: format!("gfile-{transfer_id}"),
        conv_id,
        sender_id: s.device_id.clone(),
        receiver_id: group_id.clone(),
        kind: kind.to_string(),
        content,
        ts: db::now_ms(),
        seq,
        status: "sending".to_string(),
    };
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::insert_message(&dbc, &rec).ok();
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &group_id,
            &name,
            size,
            "send",
            "active",
            Some(p.to_string_lossy().as_ref()),
            0.0,
        )
        .ok();
        let group_name = db::get_group(&dbc, &group_id).map(|g| g.name).unwrap_or_default();
        let preview = if kind == "image" { "[图片]".to_string() } else { format!("[群文件] {name}") };
        db::touch_conversation(
            &dbc,
            &format!("group:{group_id}"),
            "group",
            &group_name,
            None,
            &preview,
            0,
        )
        .ok();
    }
    let _ = s.app.emit("message-received", &rec);

    // 可达成员：有 TCP link 且 peers 信息完整；其余保持 pending，由上线事件自动投递。
    let mut reachable: Vec<String> = Vec::new();
    for m in &members {
        if s.has_link(m).await && resolve_member_x25519(&s, m).is_some() {
            reachable.push(m.clone());
        }
    }

    // 进度条分母快照：发送那一刻在线的成员（用户口径见 `AppState::group_file_online_targets`）。
    // 冻结在这里的理由：离线成员之后上线补发时**不得**回退进度条（用户明确要求），
    // 动态算分母会让他一上线就把进度条往回拉。
    {
        let mut snap = s.group_file_online_targets.lock().unwrap_or_else(|e| e.into_inner());
        snap.insert(transfer_id.clone(), reachable.iter().cloned().collect());
    }

    // 逐可达成员发送 Offer
    for m in &reachable {
        let msg = Message::GroupFileOffer {
            transfer_id: transfer_id.clone(),
            group_id: group_id.clone(),
            sender_id: s.device_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            sealed_file_key: sealed_file_key.clone(),
        };
        if try_send(s, m, &msg).await.is_ok() {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(&dbc, &transfer_id, m, "sending", 0.0);
        }
    }

    // 逐可达成员并行投递（每个 recipient 独立任务：Offer → Chunk → Done）。
    // 单个 recipient 失败只标它 failed，不阻塞其他；离线成员保持 pending，
    // 由上线事件（flush_pending_group_files）自动投递。
    let s2 = s.clone();
    let tid = transfer_id.clone();
    let gid = group_id.clone();
    let src = p.clone();
    for m in &reachable {
        let s3 = s2.clone();
        let tid3 = tid.clone();
        let gid3 = gid.clone();
        let src3 = src.clone();
        let m3 = m.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatch_group_file_to_peer(&s3, &tid3, &gid3, &m3, src3.to_string_lossy().as_ref()).await {
                app_handle_log(&s3, &format!("group-file dispatch {tid3} -> {m3} failed: {e}"));
            }
        });
    }

    Ok(transfer_id)
}

/// 群文件「已投递到几个成员」的进度聚合 —— **只按发送时在线的成员平均**。
///
/// 用户口径（2026-09-12 反馈）：进度条表示「**在线成员**都收到了」，不是「全员都收到了」。
/// 离线成员不计入分母，他上线后的补发也**不回退**进度条。
///
/// 分母取自 `state.group_file_online_targets`（发送那一刻冻结的快照）；快照缺失
/// （进程重启后内存态丢失）时退回「全体 recipient 平均」—— 仍是单调不减的口径，
/// 不会出现进度条倒退。
///
/// `fallback` 是调用方刚算出的**本条连接**字节进度，用于覆盖 DB 尚未刷新的那一拍。
///
/// ⚠️ 本函数**自己取 `state.db` 锁**：调用方必须在**未持有 db 锁**时调用（std Mutex 不可重入）。
pub(crate) fn group_file_online_progress(
    state: &AppState,
    transfer_id: &str,
    fallback: f64,
) -> f64 {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let recipients = db::list_group_file_recipients(&dbc, transfer_id).unwrap_or_default();
    drop(dbc);
    if recipients.is_empty() {
        return fallback.clamp(0.0, 1.0);
    }
    let snapshot = state
        .group_file_online_targets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(transfer_id)
        .cloned();
    group_file_progress_from(&recipients, snapshot.as_ref(), fallback)
}

/// `group_file_online_progress` 的**纯函数内核**（便于单测，不碰锁/DB）。
///
/// 口径（用户 2026-09-12）：`online` = 发送那一刻在线的 recipient 集合。
/// - `online` 为 `None`（快照丢失，如进程重启）→ 分母 = **全体** recipient；
/// - `online` 为空集（发送时无人在线）→ **0**（没有在线成员可等，进度条不该满格）；
/// - 否则分母 = 快照内成员，进度 = 其各自进度的**平均**（离线成员不参与）。
///
/// `fallback` 是调用方刚算出的本条连接字节进度（DB 可能还没刷新到这一拍），
/// 最终取 `max(聚合, fallback)` ⇒ **单调不减**，符合「补发不回退进度条」。
pub(crate) fn group_file_progress_from(
    recipients: &[crate::state::GroupFileRecipient],
    online: Option<&std::collections::HashSet<String>>,
    fallback: f64,
) -> f64 {
    let fallback = fallback.clamp(0.0, 1.0);
    let denom: Vec<&crate::state::GroupFileRecipient> = match online {
        Some(set) if !set.is_empty() => recipients
            .iter()
            .filter(|r| set.contains(&r.recipient_id))
            .collect(),
        Some(_) => return 0.0,
        None => recipients.iter().collect(),
    };
    if denom.is_empty() {
        return fallback;
    }
    let sum: f64 = denom.iter().map(|r| r.progress.clamp(0.0, 1.0)).sum();
    (sum / denom.len() as f64).max(fallback).clamp(0.0, 1.0)
}

/// 群文件投递失败诊断（emit 给 DevDiag 面板；不打印任何密钥/明文内容）。
fn app_handle_log(state: &Arc<AppState>, msg: &str) {
    let _ = state.app.emit("group-file-log", msg);
}

/// 向单个 recipient 执行完整群文件投递：Offer → 流式 Chunk → Done。
/// 元数据/源路径/密钥均从 DB 与运行态恢复，支持离线 pending 的延迟投递。
async fn dispatch_group_file_to_peer(
    state: &Arc<AppState>,
    transfer_id: &str,
    group_id: &str,
    recipient: &str,
    source_path: &str,
) -> Result<(), String> {
    let gf = db::get_group_file(&state.db.lock().unwrap_or_else(|e| e.into_inner()), transfer_id)
        .ok_or("群文件记录不存在")?;
    let group_key = get_group_key(state, group_id).await.ok_or("群密钥缺失")?;
    let file_key = ensure_group_file_key(state, transfer_id, group_id, &group_key)
        .ok_or("文件会话密钥缺失")?;
    let sealed_file_key = STANDARD.encode(
        crypto::seal_symmetric(&group_key, &file_key).ok_or("封装文件密钥失败")?,
    );

    // 源文件必须仍存在：不存在则该 recipient 置 failed（明确状态变化，
    // 不允许数据库停留在 pending 却永远无法投递）
    let src = std::path::PathBuf::from(source_path);
    let size = match std::fs::metadata(&src) {
        Ok(m) if m.is_file() => m.len(),
        _ => {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(&dbc, transfer_id, recipient, "failed", 0.0);
            return Err("源文件已不存在".to_string());
        }
    };

    // recipient → sending + Offer
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(&dbc, transfer_id, recipient, "sending", 0.0);
    }
    let offer = Message::GroupFileOffer {
        transfer_id: transfer_id.to_string(),
        group_id: group_id.to_string(),
        sender_id: state.device_id.clone(),
        name: gf.name.clone(),
        size,
        sha256: gf.sha256.clone(),
        sealed_file_key,
    };
    try_send(state, recipient, &offer)
        .await
        .map_err(|e| format!("Offer 发送失败：{e}"))?;

    // 流式分片：256KB → AEAD（独立随机 nonce）→ Base64 → GroupFileChunk
    let mut f = tokio::fs::File::open(&src).await.map_err(|e| e.to_string())?;
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; FILE_CHUNK];
    let mut seq: u32 = 0;
    let mut sent: u64 = 0;
    let mut last_report = std::time::Instant::now() - std::time::Duration::from_secs(1);
    loop {
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let Some(key) = state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).get(transfer_id).copied() else {
            return Err("文件会话密钥丢失".to_string());
        };
        let sealed = crypto::seal_symmetric(&key, &buf[..n]).ok_or("分片加密失败")?;
        let data = STANDARD.encode(&sealed);
        let chunk = Message::GroupFileChunk {
            transfer_id: transfer_id.to_string(),
            group_id: group_id.to_string(),
            sender_id: state.device_id.clone(),
            seq,
            data,
        };
        try_send(state, recipient, &chunk)
            .await
            .map_err(|e| format!("分片发送失败：{e}"))?;
        sent += n as u64;
        // 真实本地进度节流落库（250ms），并向前端推送进度事件。
        if last_report.elapsed() >= std::time::Duration::from_millis(250) {
            last_report = std::time::Instant::now();
            let progress = if size == 0 {
                1.0
            } else {
                sent as f64 / size as f64
            };
            // 发送方气泡的进度口径：**只按发送时在线的成员平均**（用户 2026-09-12 反馈）。
            // 离线成员不计入分母、之后补发也不回退进度条；在线成员全部完成即 100%。
            //
            // ⚠️ 先算聚合再取 db 锁：`group_file_online_progress` 内部要读 DB，
            // 若在持有 db 锁时调用就是同锁重入（std Mutex 不可重入，必死锁）。
            // 聚合口径取「在线成员各自进度的平均」，因此这里传入的是**本条连接**的
            // 字节进度，函数内部再与落库值取 max（同一 recipient 的进度单调不减）。
            let max_progress = group_file_online_progress(state, transfer_id, progress);
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(
                &dbc,
                transfer_id,
                recipient,
                "sending",
                progress,
            );
            let _ = db::upsert_transfer(
                &dbc,
                transfer_id,
                group_id,
                &gf.name,
                size,
                "send",
                "active",
                Some(source_path),
                max_progress,
            );
            drop(dbc);
            let _ = state.app.emit(
                "file-progress",
                &crate::state::FileProgress {
                    transfer_id: transfer_id.to_string(),
                    received: (size as f64 * max_progress) as u64,
                    total: size,
                },
            );
        }
        seq += 1;
    }
    // Done：分片全部发出，接收端据此做最终校验
    let done = Message::GroupFileDone {
        transfer_id: transfer_id.to_string(),
        group_id: group_id.to_string(),
        sender_id: state.device_id.clone(),
    };
    try_send(state, recipient, &done)
        .await
        .map_err(|e| format!("Done 发送失败：{e}"))?;
    Ok(())
}

/// 恢复/获取群文件会话密钥：优先内存运行态；
/// 缺失时从持久化的密封密钥（gfk:{tid}，群密钥封装）解封并回填内存。
/// 明文 file_key 仍不落库（gfk 存的是群密钥封装后的密文，与 wire 一致）。
pub fn ensure_group_file_key(
    state: &AppState,
    transfer_id: &str,
    group_id: &str,
    group_key: &[u8; 32],
) -> Option<[u8; 32]> {
    let _ = group_id; // 预留：未来按群隔离密钥命名空间
    if let Some(k) = state.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).get(transfer_id) {
        return Some(*k);
    }
    let sealed_b64 = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, &format!("gfk:{transfer_id}"))
    }?;
    let sealed = STANDARD.decode(sealed_b64).ok()?;
    let key: [u8; 32] = crypto::open_symmetric(group_key, &sealed)?.try_into().ok()?;
    state
        .group_file_keys
        .lock()
        .unwrap()
        .insert(transfer_id.to_string(), key);
    Some(key)
}

/// peer 上线（Hello / 心跳 / 建链）后触发：把该 peer 的 pending 群文件
/// 顺序投递（同一 peer 串行，不同 peer 并行）。
/// 防重入：group_file_sending 标记保证同一 peer 同时只有一个投递任务；
/// 源文件已不存在 → recipient 置 failed（不留永远无法投递的 pending）；
/// 无 link 时保持 pending，本次直接返回（下次连接事件再触发）。
pub async fn flush_pending_group_files(state: &Arc<AppState>, peer_id: &str) {
    if !state
        .group_file_sending
        .lock()
        .unwrap()
        .insert(peer_id.to_string())
    {
        return; // 该 peer 已有投递任务在执行
    }
    let pending = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_pending_group_files_for_recipient(&dbc, peer_id).unwrap_or_default()
    };
    if pending.is_empty() {
        state.group_file_sending.lock().unwrap_or_else(|e| e.into_inner()).remove(peer_id);
        return;
    }
    let mut tasks: Vec<(String, String, String)> = Vec::new();
    for (tid, gid) in pending {
        // 源文件仍在本机（file_transfers send 行的 path）才可投递
        let src = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_transfer_path(&dbc, &tid)
        };
        let ok = src
            .as_deref()
            .map(|p| std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false))
            .unwrap_or(false);
        if ok {
            tasks.push((tid, gid, src.unwrap()));
        } else {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(&dbc, &tid, peer_id, "failed", 0.0);
        }
    }
    if tasks.is_empty() {
        state.group_file_sending.lock().unwrap_or_else(|e| e.into_inner()).remove(peer_id);
        return;
    }
    let s2 = state.clone();
    let peer = peer_id.to_string();
    tauri::async_runtime::spawn(async move {
        for (tid, gid, src) in tasks {
            if let Err(e) = dispatch_group_file_to_peer(&s2, &tid, &gid, &peer, &src).await {
                app_handle_log(&s2, &format!("group-file dispatch {tid} -> {peer} failed: {e}"));
            }
        }
        s2.group_file_sending.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer);
    });
}

// ---------------- 文件传输 ----------------

/// 图片 MIME → 扩展名（仅接受常见格式）。
fn image_extension(mime: &str) -> Option<&'static str> {
    match mime.to_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// 纯函数：验证并解码 data URL 图片，返回 (扩展名, 解码后字节)。
/// 用于单元测试覆盖 MIME/大小/base64 等校验逻辑，不涉及文件系统。
fn decode_outgoing_image(data_url: &str) -> Result<(&'static str, Vec<u8>), String> {
    const PREFIX: &str = "data:";
    if !data_url.starts_with(PREFIX) {
        return Err("非法的 data URL".to_string());
    }
    let rest = &data_url[PREFIX.len()..];
    let Some((meta, encoded)) = rest.split_once(',') else {
        return Err("非法的 data URL".to_string());
    };
    let meta = meta.to_lowercase();
    if !meta.ends_with(";base64") {
        return Err("只接受 base64 编码的 data URL".to_string());
    }
    let mime = meta.trim_end_matches(";base64").trim();
    if !mime.starts_with("image/") {
        return Err("只接受图片文件".to_string());
    }
    let Some(ext) = image_extension(mime) else {
        return Err("不支持的图片格式".to_string());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|e| format!("图片解码失败：{e}"))?;
    if bytes.len() as u64 > MAX_OUTGOING_IMAGE_BYTES {
        return Err(format!(
            "图片过大（{} > {}），请压缩后重试",
            bytes.len(),
            MAX_OUTGOING_IMAGE_BYTES
        ));
    }
    if bytes.is_empty() {
        return Err("图片内容为空".to_string());
    }
    Ok((ext, bytes))
}

/// 把前端 paste 产生的 data URL 解码保存为本地文件。
/// 仅接受 image/* 常见格式，按解码后字节数限制，返回本地路径/文件名/大小。
#[tauri::command(async)]
pub fn save_outgoing_image(
    state: State<'_, Arc<AppState>>,
    data_url: String,
) -> Result<serde_json::Value, String> {
    let (ext, bytes) = decode_outgoing_image(&data_url)?;
    let name = format!("image-{}.{ext}", Uuid::new_v4());
    let dl = state.inner().downloads_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let path = dl.join(&name);
    std::fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    std::fs::write(&path, &bytes).map_err(|e| format!("图片保存失败：{e}"))?;
    Ok(serde_json::json!({
        "path": path.to_string_lossy().to_string(),
        "name": name,
        "size": bytes.len() as u64,
    }))
}

/// 删除本地文件（用于图片发送初始化失败后清理孤儿文件）。
#[tauri::command]
pub fn delete_file(path: String) -> Result<(), String> {
    std::fs::remove_file(&path).map_err(|e| e.to_string())
}

/// 用系统默认应用打开本地文件。
/// macOS 走 NSWorkspace（沙盒下 /usr/bin/open 被拦），Windows/Linux 走 opener。
#[tauri::command]
pub fn open_file_native(path: String) -> Result<(), String> {
    crate::macos_open::open_path_native(std::path::Path::new(&path))
}

/// macOS 窗口圆角：WebView 加载完成后（前端 onMounted 触发）设背景色跟随主题 +
/// contentView 圆角（setup 阶段设会被 wry 替换 contentView 丢失）。非 macOS 无操作。
#[tauri::command]
pub fn apply_macos_window_shape(
    window: tauri::WebviewWindow,
    dark: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return crate::macos_window::apply_rounded_corners(&window, dark);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, dark);
        Ok(())
    }
}

/// 群文件投递摘要（气泡成员状态文案用）：总数/completed/failed/待投递。
#[tauri::command]
pub fn get_group_file_delivery_summary(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> Option<db::GroupFileDeliverySummary> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    db::get_group_file_delivery_summary(&dbc, &transfer_id)
}


/// 构造一条本地文件/图片消息记录（发送方）。
/// kind 由调用方根据 subtype 决定：image 子类型保持 kind="image"，其余为 "file"。
fn build_file_message(
    state: &AppState,
    transfer_id: &str,
    friend_id: &str,
    path: &str,
    name: &str,
    size: u64,
    kind: &str,
    subtype: &str,
) -> MessageRecord {
    let content = serde_json::json!({
        "name": name,
        "path": path,
        "size": size,
        "subtype": subtype,
    })
    .to_string();
    let seq = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, friend_id).unwrap_or(1)
    };
    MessageRecord {
        id: 0,
        msg_id: format!("file-{transfer_id}"),
        conv_id: friend_id.to_string(),
        sender_id: state.device_id.clone(),
        receiver_id: friend_id.to_string(),
        kind: kind.to_string(),
        content,
        ts: db::now_ms(),
        seq,
        status: "sent".to_string(),
    }
}

/// 永久失败收尾：队列置 failed，消息气泡置 failed，并通知前端。
fn fail_file_job(state: &AppState, transfer_id: &str, reason: &str) {
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::mark_file_outbox_failed(&dbc, transfer_id).ok();
        db::set_message_status(&dbc, &format!("file-{transfer_id}"), "failed").ok();
        // 保持 file_transfers 行已有的 name/size/path，仅把状态推进到 failed。
        let _ = dbc.execute(
            "UPDATE file_transfers SET status = 'failed', progress = 0.0 WHERE id = ?1",
            rusqlite::params![transfer_id],
        );
    }
    let _ = state.app.emit(
        "file-failed",
        &crate::state::FileFailedInfo {
            transfer_id: transfer_id.to_string(),
            reason: reason.to_string(),
        },
    );
}

/// 尝试投递某 peer 的全部 pending 文件（同一 peer 串行，不同 peer 并行）。
/// 触发点与 `flush_outbox` / `flush_group_outbox` 一致：建链 / Hello / 心跳。
pub async fn flush_pending_files(state: &Arc<AppState>, peer_id: &str) {
    if !state.file_sending.lock().unwrap_or_else(|e| e.into_inner()).insert(peer_id.to_string()) {
        return;
    }
    // 没有链路时不做无谓尝试，保持 pending，等下一次连接事件再触发。
    if !state.has_link(peer_id).await {
        state.file_sending.lock().unwrap_or_else(|e| e.into_inner()).remove(peer_id);
        return;
    }
    let pending = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_pending_file_outbox(&dbc, peer_id).unwrap_or_default()
    };
    if pending.is_empty() {
        state.file_sending.lock().unwrap_or_else(|e| e.into_inner()).remove(peer_id);
        return;
    }
    let st = state.clone();
    let peer = peer_id.to_string();
    tauri::async_runtime::spawn(async move {
        for (transfer_id, local_path) in pending {
            if !st.has_link(&peer).await {
                break;
            }
            {
                let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                db::mark_file_outbox_sending(&dbc, &transfer_id, 0).ok();
            }
            match file::send_file_from_path(
                &st,
                &peer,
                &transfer_id,
                std::path::PathBuf::from(local_path),
            )
            .await
            {
                Ok(()) => {
                    let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::delete_file_outbox(&dbc, &transfer_id).ok();
                }
                Err(e) if !e.retryable => {
                    fail_file_job(&st, &transfer_id, &e.message);
                }
                Err(_) => {
                    // 可恢复失败：保留 pending，稍后由连接/心跳再次触发。
                    let dbc = st.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::mark_file_outbox_pending(&dbc, &transfer_id, 5_000).ok();
                }
            }
        }
        st.file_sending.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer);
    });
}

#[tauri::command]
pub async fn send_file(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    let s = state.inner();
    // 好友关系检查
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友，请先扫描添加好友之后再继续聊天。".to_string());
        }
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("只能发送普通文件".to_string());
    }
    let size = meta.len();
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let transfer_id = Uuid::new_v4().to_string();
    let subtype = file::classify_file_subtype(&name);
    let kind = if subtype == "image" { "image" } else { "file" };
    let rec = build_file_message(s, &transfer_id, &friend_id, &path, &name, size, kind, subtype);
    // 注意：不能在持有 db 锁时调用 resolve_nickname（其内部会再次锁 db）。
    let nm = resolve_nickname(s, &friend_id);
    let preview = if kind == "image" {
        "[图片]".to_string()
    } else {
        format!("[文件] {name}")
    };
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        db::insert_message(&tx, &rec).map_err(|e| e.to_string())?;
        db::touch_conversation(
            &tx,
            &friend_id,
            "single",
            &nm,
            None,
            &preview,
            0,
        )
        .map_err(|e| e.to_string())?;
        // 先建立 file_transfers 记录，前端刷新传输列表后能立刻拿到进度条载体。
        db::upsert_transfer(
            &tx,
            &transfer_id,
            &friend_id,
            &name,
            size,
            "send",
            "pending",
            Some(path.as_str()),
            0.0,
        )
        .map_err(|e| e.to_string())?;
        db::insert_file_outbox(&tx, &transfer_id, &friend_id, None, &path, &name, size)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    let _ = s.app.emit("message-received", &rec);

    let arc = state.inner().clone();
    let fid = friend_id.clone();
    tokio::spawn(async move {
        flush_pending_files(&arc, &fid).await;
    });
    Ok(transfer_id)
}

/// 统一文件发送入口：当前稳定版统一走「直连 + 离线队列」。
/// 只要好友最终上线，文件就会在连接事件触发时自动补发，不再依赖不可达的中继路径。
#[tauri::command]
pub async fn send_file_auto(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    send_file(state, friend_id, path).await
}

/// 中继切片发送入口：保留命令名以兼容前端，当前实现回退到与直连相同的可靠队列，
/// 避免「看似已发送、实际无法投递」的假成功。
#[tauri::command]
pub async fn send_file_relay(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
    path: String,
) -> Result<String, String> {
    send_file(state, friend_id, path).await
}

#[tauri::command]
pub fn get_transfers(state: State<'_, Arc<AppState>>) -> Vec<TransferInfo> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
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
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, "share_dir", &path).map_err(|e| e.to_string())?;
    }
    *s.share_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    Ok(())
}

#[tauri::command]
pub fn get_share_dir(state: State<'_, Arc<AppState>>) -> Option<String> {
    state.inner().share_dir.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 文件接收目录（接收的文件/图片落盘于此，可改、可在资源管理器打开）。
#[tauri::command]
pub fn get_downloads_dir(state: State<'_, Arc<AppState>>) -> String {
    state
        .inner()
        .downloads_dir
        .lock()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

/// 修改文件接收目录：校验目录存在后持久化，后续新接收的文件落到新目录。
#[tauri::command]
pub fn set_downloads_dir(state: State<'_, Arc<AppState>>, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err("目录不存在".to_string());
    }
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, "downloads_dir", &path).map_err(|e| e.to_string())?;
    }
    *s.downloads_dir.lock().unwrap_or_else(|e| e.into_inner()) = p;
    Ok(())
}

/// 在系统资源管理器中打开文件接收目录。
#[tauri::command]
pub fn open_downloads_dir(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let p = state.inner().downloads_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    open_in_file_manager(&p)
}

/// 跨平台在系统文件管理器里打开指定目录。
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开目录失败：{e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn request_share_tree(
    state: State<'_, Arc<AppState>>,
    friend_id: String,
) -> Result<Vec<ShareEntry>, String> {
    let s = state.inner();
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友".to_string());
        }
    }
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.pending_share_tree
        .lock()
        .unwrap()
        .insert(request_id.clone(), tx);

    let msg = Message::ShareTreeRequest {
        request_id: request_id.clone(),
        from: s.device_id.clone(),
        to: friend_id.clone(),
    };
    if let Err(e) = try_send(s, &friend_id, &msg).await {
        s.pending_share_tree.lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id);
        return Err(e);
    }

    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(entries)) => Ok(entries),
        _ => {
            s.pending_share_tree.lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id);
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
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if db::get_friend(&dbc, &friend_id).is_none() {
            return Err("对方不是好友".to_string());
        }
    }
    let transfer_id = Uuid::new_v4().to_string();
    let msg = Message::ShareFileRequest {
        transfer_id: transfer_id.clone(),
        from: s.device_id.clone(),
        path: remote_path.clone(),
    };
    try_send(s, &friend_id, &msg).await?;
    // 本地提示：你正在下载好友的文件（聊天信息内简约系统消息）
    let file_name = std::path::Path::new(&remote_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| remote_path.clone());
    let friend_name = resolve_nickname(s, &friend_id);
    insert_system_message(
        s,
        &friend_id,
        &format!("你正在下载「{friend_name}」的文件「{file_name}」"),
    );
    Ok(transfer_id)
}

/// 插入一条本地系统消息到指定会话并推送给前端（共享下载提示、身份密钥变更告警等本地事件用）。
/// 取 `&AppState`（而非 `&Arc<AppState>`）以便网络层 `upsert_peer` 等只持有 `&AppState`
/// 的调用点复用；调用方传 `&Arc<AppState>` 时由 deref 自动转换。
pub fn insert_system_message(state: &AppState, conv_id: &str, text: &str) {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let rec = crate::state::MessageRecord {
        id: 0,
        msg_id: format!("sys-{}", Uuid::new_v4()),
        conv_id: conv_id.to_string(),
        sender_id: state.device_id.clone(),
        receiver_id: state.device_id.clone(),
        kind: "system".to_string(),
        content: text.to_string(),
        ts: db::now_ms(),
        seq: db::next_clock(&dbc, conv_id).unwrap_or(1),
        status: "sent".to_string(),
    };
    db::insert_message(&dbc, &rec).ok();
    drop(dbc);
    let _ = state.app.emit("message-received", &rec);
}

// ---------------- 辅助 ----------------

/// 将文件从 source 复制到 destination（用于"另存为"下载功能）。
#[tauri::command(async)]
pub fn copy_file(source: String, destination: String) -> Result<(), String> {
    std::fs::copy(&source, &destination).map_err(|e| e.to_string())?;
    Ok(())
}

/// 把文件本体写入系统剪贴板（Windows CF_HDROP）。
/// 之后既可在资源管理器 / 桌面 Ctrl+V 粘贴出文件，也可粘贴回聊天框直接发送（微信式）。
#[tauri::command]
pub fn copy_file_to_clipboard(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::Setter;
        if !std::path::Path::new(&path).is_file() {
            return Err(format!("文件不存在或不可访问：{path}"));
        }
        let _clip = clipboard_win::Clipboard::new_attempts(10)
            .map_err(|e| format!("无法访问系统剪贴板：{e}"))?;
        clipboard_win::formats::FileList
            .write_clipboard(&[path.as_str()])
            .map_err(|e| format!("复制文件到剪贴板失败：{e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("当前平台暂不支持复制文件到剪贴板".into())
    }
}

/// 读取剪贴板里的文件路径列表（CF_HDROP）。空列表表示剪贴板里没有真实文件
/// （截图 / 网页图片是位图数据，不是文件）。供输入框粘贴时区分「粘贴文件」与「粘贴图片」。
#[tauri::command]
pub fn read_clipboard_file_paths() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        // 读不到（格式不符 / 被占用）一律按"无文件"处理，前端回退到图片粘贴分支。
        let paths: Vec<String> = clipboard_win::get_clipboard(clipboard_win::formats::FileList)
            .unwrap_or_default();
        paths
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// 将 base64 数据写入目标路径（用于图片消息"另存为"：前端把 dataURL 解出 base64 传回）。
#[tauri::command(async)]
pub fn save_data_file(base64_data: String, destination: String) -> Result<(), String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data.as_bytes())
        .map_err(|e| e.to_string())?;
    std::fs::write(&destination, bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// 解析 `msg_id` 指向的本地媒体文件的结果。
///
/// 由 `read_file_preview` 与 `media_present` 共用，避免出现第二份路径解析实现
/// （安全边界必须只有一处）。
enum MediaPath {
    /// 文件存在且通过安全校验。
    Present(Box<PathBuf>),
    /// 查不到这条消息 / 元数据缺路径 / 路径未通过安全校验 —— **无法判断**媒体是否还在。
    /// 尚未落库的乐观消息会落到这里，因此调用方不能据此断言"已被清理"。
    /// 附带面向用户的错误文案。
    Unknown(String),
    /// 消息记录里的路径已解析不到文件 —— 已被「存储清理」删除。
    Gone,
}

/// 校验并解析 `msg_id` 的媒体路径。
///
/// 安全边界：路径必须落在 downloads 目录内（接收方文件），或该消息由本机发出
/// （发送方自选的文件）——两者都不允许对端通过消息内容诱导读取本机任意路径。
fn resolve_media_path(s: &AppState, msg_id: &str) -> MediaPath {
    let Some((sender_id, content)) =
        db::get_message_preview_source(&s.db.lock().unwrap_or_else(|e| e.into_inner()), msg_id)
    else {
        return MediaPath::Unknown("消息不存在".to_string());
    };
    let Some(path) = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(|p| p.to_string()))
    else {
        return MediaPath::Unknown("元数据缺少路径".to_string());
    };

    let Ok(file) = std::fs::canonicalize(&path) else {
        return MediaPath::Gone;
    };
    let under_downloads =
        std::fs::canonicalize(s.downloads_dir.lock().unwrap_or_else(|e| e.into_inner()).as_path())
            .map(|dir| file.starts_with(dir))
            .unwrap_or(false);
    if !under_downloads && sender_id != s.device_id {
        return MediaPath::Unknown("路径越权".to_string());
    }
    match std::fs::metadata(&file) {
        Ok(meta) if meta.is_file() => MediaPath::Present(Box::new(file)),
        Ok(_) => MediaPath::Unknown("非普通文件".to_string()),
        Err(_) => MediaPath::Gone,
    }
}

/// 媒体是否**仍在本机**（未被存储清理删除）。
///
/// 前端据此把"已被清理"的消息渲染成明确提示，而不是一个空白/裂开的图片框——后者
/// 会让人误以为是对端发来的文件本身有问题。只有能确定「文件已被删除」时才返回 `false`；
/// 查不到消息（例如尚未落库的乐观消息）一律按"存在"处理，绝不能把在途消息误标成已清理。
#[tauri::command]
pub fn media_present(state: State<'_, Arc<AppState>>, msg_id: String) -> Result<bool, String> {
    let s = state.inner();
    Ok(!matches!(resolve_media_path(s, &msg_id), MediaPath::Gone))
}

/// 读取附件预览内容（原始字节，不走 base64 IPC）。
///
/// 按 `msg_id` 反查记录里的本地 `path` 再读，前端据此渲染图片（→Blob/objectURL）
/// 或代码（→TextDecoder）。安全边界见 [`resolve_media_path`]。
/// 超过 `max_bytes` 返回 "TOO_LARGE"，由前端回退文件卡片；文件已被清理返回
/// "文件不存在"，由前端渲染成「已清理」占位。
#[tauri::command(async)]
pub fn read_file_preview(
    state: State<'_, Arc<AppState>>,
    msg_id: String,
    max_bytes: u64,
) -> Result<tauri::ipc::Response, String> {
    let s = state.inner();
    let max_bytes = max_bytes.min(15 * 1024 * 1024);
    let file = match resolve_media_path(s, &msg_id) {
        MediaPath::Present(p) => *p,
        MediaPath::Unknown(e) => return Err(e),
        MediaPath::Gone => return Err("文件不存在".to_string()),
    };
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    if meta.len() > max_bytes {
        return Err("TOO_LARGE".to_string());
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 导出全部聊天文字到用户指定文件（Markdown 单文件）。
///
/// 定位：磁盘满 / 换机时的**自救手段**——存储清理只删媒体、不动文字，但一旦库损坏
/// 或要迁机，没有导出入口就只能看着数据丢。只导出文字，媒体仅保留文件名
/// （见 `export` 模块说明：刻意不产出 HTML，避免对端消息在本机浏览器里执行）。
///
/// `utc_offset_minutes` 由前端给出（`-new Date().getTimezoneOffset()`）：Rust 侧不引入
/// 时区库（`AI_RULES §25`），跨夏令时切换的历史消息可能有 1 小时偏差，已在模块注释说明。
#[tauri::command(async)]
pub fn export_chat_text(
    state: State<'_, Arc<AppState>>,
    destination: String,
    utc_offset_minutes: i64,
) -> Result<export::ExportSummary, String> {
    let s = state.inner();
    if destination.trim().is_empty() {
        return Err("导出路径为空".to_string());
    }
    let device_name = s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let my_id = s.device_id.clone();

    // 只把「读库」放进锁里：渲染几十万条消息 + 写盘可能耗时较长，
    // 不能让一次导出把消息落库卡住。
    let sections = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        export::collect_sections(&dbc, &my_id)?
    };

    let messages: usize = sections.iter().map(|(_, m)| m.len()).sum();
    let conversations = sections.len();
    let generated_at = export::format_local_time(db::now_ms(), utc_offset_minutes);
    let text = export::render_markdown(&device_name, &generated_at, &sections, utc_offset_minutes);

    std::fs::write(&destination, text.as_bytes()).map_err(|e| format!("写入导出文件失败：{e}"))?;
    Ok(export::ExportSummary {
        conversations,
        messages,
        path: destination,
    })
}

/// 清除所有聊天数据（保留好友、身份、设置）。
/// SQLite 删除使用 transaction，任一失败则 rollback。
/// 文件系统清理在 DB commit 成功后执行；文件删除失败不影响 DB 结果。
#[tauri::command(async)]
pub fn clear_all_data(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let s = state.inner();

    // 1. SQLite 删除（transaction 保护）。
    //    语义：彻底清除 = 删除本机消息/会话/文件/群记录，**并退出所有群聊**——
    //    否则「清除聊天数据」后群还留在列表里（重新安装后还会被群主/成员的
    //    群密钥分发重新拉回）。
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM messages", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM conversations", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM outbox", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM group_outbox", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM file_outbox", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM file_transfers", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM pending_reads", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM pending_group_reads", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM group_reads", [])
            .map_err(|e| e.to_string())?;
        // 群文件投递数据同属聊天数据（残留会导致 transfer 记录悬挂）
        tx.execute("DELETE FROM group_files", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM group_file_recipients", [])
            .map_err(|e| e.to_string())?;
        // 彻底清除 = 也退出所有群聊：删群成员/群记录/群密钥/群时钟，
        // 否则「清除聊天数据」后群还留在列表里（重新安装后还会被群主/成员
        // 的群密钥分发重新拉回）。
        tx.execute("DELETE FROM group_members", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM groups", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM settings WHERE key LIKE 'gk:%'", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM conversation_clocks WHERE conv_id LIKE 'group:%'", [])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }

    // 2. Runtime state 清理：群密钥内存缓存一并清空（彻底退出群聊）。
    s.pending_requests.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.pending_reads.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.pending_file_accept.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.pending_file_complete.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.pending_share_tree.lock().unwrap_or_else(|e| e.into_inner()).clear();
    // 群文件/文件投递运行态与待发群密钥同属聊天数据运行态（不清会残留
    // 已删群的 file_key，且 pending 群密钥可能在重连时复活已删群记录）
    s.group_file_receivers.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.group_file_keys.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.group_keys.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.pending_group_keys.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.group_file_sending.lock().unwrap_or_else(|e| e.into_inner()).clear();
    s.file_sending.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *s.relay.lock().unwrap_or_else(|e| e.into_inner()) = crate::relay_manager::RelayManager::new();
    // 先关闭未完成接收的文件句柄，再清理 downloads 目录中的 .part 临时文件。
    s.file_receivers.lock().unwrap_or_else(|e| e.into_inner()).clear();

    // 3. 文件系统清理（DB commit 成功后执行）
    //    收集错误而非立即返回，避免文件清理失败伪装成"整个操作失败"
    let mut fs_errors: Vec<String> = Vec::new();

    // 清空 cache_dir 内容（保留目录本身）
    if s.cache_dir.exists() {
        for entry in std::fs::read_dir(&s.cache_dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            let result = if p.is_file() {
                std::fs::remove_file(&p)
            } else if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                fs_errors.push(format!("cache_dir: {} ({})", p.display(), e));
            }
        }
    }
    // 清空 downloads_dir 内容（保留目录本身）
    let dl = s.downloads_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if dl.exists() {
        for entry in std::fs::read_dir(&dl)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            let result = if p.is_file() {
                std::fs::remove_file(&p)
            } else if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                fs_errors.push(format!("downloads_dir: {} ({})", p.display(), e));
            }
        }
    }

    if fs_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "数据记录已清除，但部分缓存文件未能删除：{}",
            fs_errors.join("；")
        ))
    }
}

/// 搜索消息：返回匹配关键词的会话列表及其最新匹配消息。
#[tauri::command(async)]
pub fn search_messages(
    state: State<'_, Arc<AppState>>,
    keyword: String,
) -> Result<Vec<SearchResult>, String> {
    let s = state.inner();
    // 搜索关键词长度保护：按字符截断（UTF-8 安全）
    let keyword: String = keyword.chars().take(MAX_SEARCH_LEN).collect();
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let conv_ids = db::search_messages(&dbc, &keyword, 20).map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for conv_id in conv_ids {
        // 获取会话名称
        let name = db::get_friend(&dbc, &conv_id)
            .map(|(n, _)| n)
            .unwrap_or_else(|| conv_id.clone());
        // 获取最新匹配消息
        let msgs = db::search_messages_in_conv(&dbc, &conv_id, &keyword, 1).unwrap_or_default();
        if let Some(m) = msgs.into_iter().next() {
            results.push(SearchResult {
                conv_id,
                name,
                match_content: m.content,
                match_ts: m.ts,
                match_msg_id: m.msg_id,
            });
        }
    }
    Ok(results)
}

#[derive(Serialize)]
pub struct SearchResult {
    conv_id: String,
    name: String,
    match_content: String,
    match_ts: i64,
    /// 命中消息的 msg_id：前端据此"跳到那一条"（只给 conv_id 的话，
    /// 用户点进去还要自己在会话里翻，搜索就只完成了一半）。
    match_msg_id: String,
}

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

// ---------------- 跨子网（Routed）端点配置 ----------------

/// 校验并**规范化**手动配置的 Routed 端点地址，返回 `ip:port`。
///
/// 两种输入都接受：
/// - `100.64.0.1:60002`（显式端口，对端用了 `--instance` 时需要）
/// - `100.64.0.1`（省略端口 → 用标准 [`TCP_PORT`]）
///
/// 允许省略端口是「少配置」的一部分：端口是内部实现细节，默认单实例场景下用户
/// 没有理由需要知道它，更不该因为漏写端口而被拒绝。
///
/// 只接受 **IPv4**：TCP 监听侧绑的是 `Ipv4Addr`（`network::transport::spawn`），
/// IPv6 端点即使拨出去也连不上。在这里当场拒绝，好过「存下来了但永远连不上」
/// —— 后者对用户完全不可见（配置成功、日志无错、就是没反应）。
fn normalize_routed_address(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    // 解析（含「裸 IP 补标准端口」）由 `parse_endpoint_addr` 单点负责 ——
    // 拨号侧走的是同一个实现，避免两条路径行为不一致。
    let Some(addr) = parse_endpoint_addr(trimmed) else {
        return Err(format!("地址格式应为 ip 或 ip:port，收到：{trimmed}"));
    };
    if addr.is_ipv6() {
        return Err(
            "暂不支持 IPv6 地址（当前 TCP 监听仅 IPv4）。请填写 IPv4，例如 100.64.0.1"
                .to_string(),
        );
    }
    // 规范化后再存储：add / remove 比较的是同一个字符串，避免「加进去了却删不掉」
    Ok(addr.to_string())
}

/// 列出手动配置的跨子网端点（Tailscale / VPN / 跨网段）。
#[tauri::command]
pub fn list_routed_endpoints(state: tauri::State<'_, Arc<AppState>>) -> Vec<RoutedEndpoint> {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    parse_endpoints(&db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default())
}

/// 添加一个跨子网端点。
///
/// `device_id` **可省略**（传 `null` 或空串都当作未指定）：
/// - 提供时：语义是「连接这个**已知**节点」，链路 key 直接用它，行为与历史一致；
/// - 省略时：身份由 TCP 握手学来（§8：`IP:PORT → TCP → Hello → Node ID → Identity`），
///   用户只需要知道对方地址 —— 这才是「少配置」。
///
/// 地址接受 `ip` 或 `ip:port`（省略端口按标准 [`TCP_PORT`] 补全），目前仅 IPv4：
/// TCP 监听侧绑的是 `Ipv4Addr`，IPv6 端点拨出去也连不上。
#[tauri::command]
pub fn add_routed_endpoint(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: Option<String>,
    address: String,
) -> Result<Vec<RoutedEndpoint>, String> {
    // 空串等同「未指定」：UI 上的输入框没填时通常会传空串，不该存下一个没意义的值。
    let device_id = device_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let address = normalize_routed_address(&address)?;
    let candidate = RoutedEndpoint::new(device_id, address);

    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = parse_endpoints(&db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default());
    // 同一个**地址**不重复添加，无论是否带 device_id —— 一个地址只对应一个端点。
    if !list.iter().any(|e| e.address == candidate.address) {
        list.push(candidate);
    }
    db::set_setting(&dbc, ROUTED_ENDPOINTS_KEY, &encode_endpoints(&list))
        .map_err(|e| format!("保存失败: {e}"))?;
    Ok(list)
}

/// 移除一个跨子网端点（只按**地址**匹配）。
///
/// 地址是端点的唯一标识：`device_id` 可以省略，用它当判据会让「只填地址添加、
/// 带指纹删除」匹配不上。地址先规范化（与 `add` 存进去的形式一致）。
///
/// **比对时也把库里已存的那条再规范化一次**，兜住「历史脏数据」（比如老版本直接
/// 写 SQLite 没经过 `add` 的裸 IP 无端口），否则会出现「列表里看得见但删不掉」——
/// 用户的真实反馈：UI 看着有 `100.101.221.60`，删的时候 normalize 成
/// `100.101.221.60:59992`，而库里存的就是裸 `100.101.221.60`，字符串不相等。
#[tauri::command]
pub fn remove_routed_endpoint(
    state: tauri::State<'_, Arc<AppState>>,
    address: String,
) -> Result<Vec<RoutedEndpoint>, String> {
    let address = normalize_routed_address(&address)?;
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = parse_endpoints(&db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default());
    let before = list.len();
    list.retain(|e| {
        // 库里的历史脏数据可能未归一化（裸 IP / 裸 IP 带非标准端口），按当前规则再过一遍
        // 归一化后比较。归一化失败的条目（语法错乱）保守地按字符串相等判，免得误删。
        let stored_norm = normalize_routed_address(&e.address)
            .unwrap_or_else(|_| e.address.clone());
        stored_norm != address
    });
    if list.len() != before {
        db::set_setting(&dbc, ROUTED_ENDPOINTS_KEY, &encode_endpoints(&list))
            .map_err(|e| format!("保存失败: {e}"))?;
    }
    Ok(list)
}

// ---------------- 运行日志 ----------------

/// 读取内存中的全部运行日志（时间正序：旧 → 新）。
#[tauri::command]
pub fn get_logs(state: tauri::State<'_, Arc<AppState>>) -> Vec<LogEntry> {
    state.logger.snapshot()
}

/// 清空运行日志（内存 + 落盘文件）。
#[tauri::command]
pub fn clear_logs(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.logger.clear();
    Ok(())
}

/// 桌面端：打开独立的「运行日志」窗口（已存在则聚焦）。
///
/// 日志窗口加载同一个前端，由前端按窗口 label（`logs`）渲染日志页；窗口用系统标题栏
/// （含关闭按钮），关闭即销毁，下次打开再重建 —— 与主窗口的「关闭到托盘」互不影响。
#[cfg(desktop)]
#[tauri::command]
pub fn open_log_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    if let Some(win) = app.get_webview_window(crate::WINDOW_LOGS) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    // 背景色跟随主题：暗色主题下打开日志窗口「闪一下白」（与主窗口冷启动白闪同源，
    // 窗口静态背景色只能浅/深二选一，这里用当前解析结果先设对底色）。
    let dark = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, "dark_mode").map(|v| v == "1").unwrap_or(false)
    };
    let bg = if dark {
        tauri::window::Color(11, 18, 32, 255) // #0b1220
    } else {
        tauri::window::Color(237, 241, 246, 255) // #edf1f6
    };
    let title = state.display_name();
    let win = WebviewWindowBuilder::new(&app, crate::WINDOW_LOGS, WebviewUrl::App("index.html".into()))
        .title(&title)
        .inner_size(760.0, 560.0)
        .min_inner_size(420.0, 320.0)
        // 注入窗口标识：index.html 内联骨架据此渲染「日志页骨架」而非「聊天三栏骨架」。
        .initialization_script("window.__GOSSLAN_WINDOW__ = 'logs';")
        .build()
        .map_err(|e| format!("创建日志窗口失败: {e}"))?;
    let _ = win.set_background_color(Some(bg));
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

/// 桌面端：打开独立的「设置」窗口（已存在则聚焦）。
///
/// 与日志窗口同一范式：加载同一个前端，由前端按窗口 label（`settings`）渲染设置页。
/// 用户 2026-09-12 反馈：「PC 端的设置页面可以按照这种布局，弹一个单独的窗口」——
/// 参考图是「左侧窄导航 + 右侧内容」的设置窗口，而不是盖在聊天上的居中弹窗。
/// 窗口用系统标题栏（含关闭按钮），关闭即销毁，下次打开再重建。
#[cfg(desktop)]
#[tauri::command]
pub fn open_settings_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    if let Some(win) = app.get_webview_window(crate::WINDOW_SETTINGS) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    // 背景色跟随主题：与日志窗口同源（暗色下打开时"闪一下白"的根因见 open_log_window）。
    let dark = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_setting(&dbc, "dark_mode").map(|v| v == "1").unwrap_or(false)
    };
    let bg = if dark {
        tauri::window::Color(11, 18, 32, 255) // #0b1220
    } else {
        tauri::window::Color(237, 241, 246, 255) // #edf1f6
    };
    let title = if state.is_zh() {
        format!("{} · 设置", state.display_name())
    } else {
        format!("{} · Settings", state.display_name())
    };
    let win = WebviewWindowBuilder::new(&app, crate::WINDOW_SETTINGS, WebviewUrl::App("index.html".into()))
        .title(&title)
        .inner_size(780.0, 600.0)
        .min_inner_size(560.0, 420.0)
        // 注入窗口标识：index.html 内联骨架据此渲染「设置页骨架」，
        // 而不是聊天三栏骨架或日志骨架。
        .initialization_script("window.__GOSSLAN_WINDOW__ = 'settings';")
        .build()
        .map_err(|e| format!("创建设置窗口失败: {e}"))?;
    let _ = win.set_background_color(Some(bg));
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

/// 桌面端：关闭独立的「设置」窗口。
#[cfg(desktop)]
#[tauri::command]
pub fn close_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(crate::WINDOW_SETTINGS) {
        let _ = win.close();
    }
    Ok(())
}

/// 桌面端：关闭独立的「运行日志」窗口。
#[cfg(desktop)]
#[tauri::command]
pub fn close_log_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(crate::WINDOW_LOGS) {
        let _ = win.close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        check_message_content, decode_outgoing_image, group_file_progress_from, image_extension,
        normalize_routed_address, MAX_MESSAGE_LEN, MAX_OUTGOING_IMAGE_BYTES,
    };
    use crate::state::GroupFileRecipient;
    use std::collections::HashSet;

    fn recipient(id: &str, progress: f64) -> GroupFileRecipient {
        GroupFileRecipient {
            recipient_id: id.to_string(),
            status: if progress >= 1.0 { "completed" } else { "sending" }.to_string(),
            progress,
            updated_at: 0,
        }
    }

    fn online(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// 用户口径：进度条只按**发送时在线的成员**算。
    /// 复现场景：群 3 人，1 人离线；两个在线成员都收完 ⇒ 进度必须是 **100%**，
    /// 而不是被离线成员的 0 拖成 50%（这正是 `6e9b96e` 那轮用户反馈的「卡在 50%」）。
    #[test]
    fn group_file_progress_ignores_offline_members() {
        let rs = vec![recipient("a", 1.0), recipient("b", 1.0), recipient("offline", 0.0)];
        let snap = online(&["a", "b"]);
        assert_eq!(group_file_progress_from(&rs, Some(&snap), 0.0), 1.0);
        // 对照组：不传快照（全体口径）时，同一组数据只有 2/3 —— 证明差异来自分母而非巧合。
        let all = group_file_progress_from(&rs, None, 0.0);
        assert!((all - 2.0 / 3.0).abs() < 1e-9, "全体口径应为 2/3，实际 {all}");
    }

    /// 离线成员之后上线补发**不得回退**进度条：分母是冻结快照，与他的进度无关。
    #[test]
    fn late_online_member_does_not_regress_progress() {
        let snap = online(&["a", "b"]);
        let done = vec![recipient("a", 1.0), recipient("b", 1.0), recipient("offline", 0.0)];
        assert_eq!(group_file_progress_from(&done, Some(&snap), 0.0), 1.0);
        // 离线者开始补发（进度 0.5）——仍在快照外，不影响结果
        let catching_up = vec![recipient("a", 1.0), recipient("b", 1.0), recipient("offline", 0.5)];
        assert_eq!(group_file_progress_from(&catching_up, Some(&snap), 0.0), 1.0);
    }

    /// 在线成员未全部完成时，进度是在线成员的平均值（不是 max，也不是全体）。
    #[test]
    fn group_file_progress_averages_online_members() {
        let rs = vec![recipient("a", 1.0), recipient("b", 0.0), recipient("offline", 1.0)];
        let snap = online(&["a", "b"]);
        assert_eq!(group_file_progress_from(&rs, Some(&snap), 0.0), 0.5);
    }

    /// 发送时无人在线 ⇒ 进度恒 0（没有「在线成员都收到了」这件事）。
    #[test]
    fn group_file_progress_is_zero_when_nobody_online_at_send() {
        let rs = vec![recipient("a", 0.0), recipient("b", 0.0)];
        let empty = HashSet::new();
        assert_eq!(group_file_progress_from(&rs, Some(&empty), 0.0), 0.0);
        // 即便调用方传了 fallback（本连接字节进度），空快照也必须压到 0 ——
        // 否则「发给一个刚好在线的成员」会看起来像全群都完成了。
        assert_eq!(group_file_progress_from(&rs, Some(&empty), 0.7), 0.0);
    }

    /// 快照丢失（重启后内存态清空）时退回全体口径；空集合/越界 fallback 都要夹紧。
    #[test]
    fn group_file_progress_fallback_and_clamp() {
        let rs = vec![recipient("a", 0.5), recipient("b", 0.5)];
        assert_eq!(group_file_progress_from(&rs, None, 0.0), 0.5);
        assert_eq!(group_file_progress_from(&[], None, 0.3), 0.3, "无 recipient 时用 fallback");
        assert_eq!(group_file_progress_from(&rs, None, 2.0), 1.0, "fallback 超界要夹到 1");
        assert_eq!(group_file_progress_from(&rs, None, -1.0), 0.5, "负 fallback 不得把进度拉成负");
    }

    /// Routed 端点地址：`ip` 与 `ip:port` 两种写法都收（省略端口补标准 `TCP_PORT`），
    /// 并在**存储前规范化**——这样 add 与 remove 比较的是同一个字符串，
    /// 不会出现「加进去了却删不掉」。IPv6 当场明确拒绝。
    #[test]
    fn routed_endpoint_address_is_normalized_to_ipv4_socket() {
        // 省略端口 → 补标准端口（端口是内部细节，用户不必知道）
        assert_eq!(
            normalize_routed_address("100.64.0.1").unwrap(),
            format!("100.64.0.1:{}", crate::protocol::TCP_PORT)
        );
        // 显式端口 → 原样保留（对端用了 --instance 的场景）
        assert_eq!(
            normalize_routed_address("100.64.0.1:60002").unwrap(),
            "100.64.0.1:60002"
        );
        // 前后空白容错（从聊天窗口复制地址常带空格）
        assert_eq!(
            normalize_routed_address("  192.168.1.5  ").unwrap(),
            format!("192.168.1.5:{}", crate::protocol::TCP_PORT)
        );

        // 格式错误
        assert!(normalize_routed_address("100.64.0.1:").is_err());
        assert!(normalize_routed_address("garbage").is_err());
        assert!(normalize_routed_address("").is_err());

        // IPv6：拒绝，且错误信息要能给出可操作的指引
        let err = normalize_routed_address("[fd7a:115c:a1e0::1]:59992").unwrap_err();
        assert!(err.contains("IPv6"), "错误信息应点明 IPv6：{err}");
        assert!(normalize_routed_address("fd7a:115c:a1e0::1").is_err());
    }
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    // ---------- 单条消息长度上限：超限报错，绝不静默截断 ----------

    #[test]
    fn message_content_within_limit_is_returned_unchanged() {
        let ok = "a".repeat(MAX_MESSAGE_LEN);
        let got = check_message_content(ok.clone()).unwrap();
        assert_eq!(got, ok, "恰好到上限必须原样通过，不能被改动");
    }

    #[test]
    fn message_content_over_limit_is_rejected_not_truncated() {
        let over = "b".repeat(MAX_MESSAGE_LEN + 1);
        let err = check_message_content(over).unwrap_err();
        assert!(err.contains("过长"), "错误文案应说明「过长」：{err}");
        assert!(
            err.contains(&(MAX_MESSAGE_LEN + 1).to_string()),
            "应告知实际长度，便于用户判断如何分段：{err}"
        );
    }

    #[test]
    fn message_length_is_counted_in_chars_not_bytes() {
        // 按字符计数：5 万汉字（15 万字节）合法，5 万零 1 个汉字才拒绝。
        // 若误按字节计数，5 万汉字会被判超限，正常长文就发不出去了。
        let cjk = "中".repeat(MAX_MESSAGE_LEN);
        assert!(check_message_content(cjk).is_ok());

        let cjk_over = "中".repeat(MAX_MESSAGE_LEN + 1);
        assert!(check_message_content(cjk_over).is_err());
    }

    #[test]
    fn empty_message_content_is_allowed_at_this_layer() {
        // 空内容的拦截属于上层（发送键置灰），这里不做业务判断，避免产生第二处规则。
        assert!(check_message_content(String::new()).is_ok());
    }

    #[test]
    fn image_extension_maps_common_mimes() {
        assert_eq!(image_extension("image/png"), Some("png"));
        assert_eq!(image_extension("image/jpeg"), Some("jpg"));
        assert_eq!(image_extension("image/gif"), Some("gif"));
        assert_eq!(image_extension("image/webp"), Some("webp"));
        assert_eq!(image_extension("IMAGE/PNG"), Some("png"));
        assert_eq!(image_extension("image/bmp"), None);
        assert_eq!(image_extension("text/plain"), None);
    }

    fn data_url(mime: &str, bytes: &[u8]) -> String {
        format!("data:{};base64,{}", mime, STANDARD.encode(bytes))
    }

    #[test]
    fn decode_accepts_png_jpeg_gif_webp() {
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            let expected_ext = image_extension(mime).unwrap();
            let url = data_url(mime, b"fake-image-body");
            let (ext, bytes) = decode_outgoing_image(&url).unwrap();
            assert_eq!(ext, expected_ext);
            assert_eq!(bytes, b"fake-image-body");
        }
    }

    #[test]
    fn decode_rejects_non_image_mime() {
        let url = data_url("text/plain", b"hello");
        assert!(decode_outgoing_image(&url).unwrap_err().contains("图片"));
    }

    #[test]
    fn decode_rejects_unsupported_image_mime() {
        let url = data_url("image/bmp", b"hello");
        assert!(decode_outgoing_image(&url).unwrap_err().contains("不支持"));
    }

    #[test]
    fn decode_rejects_invalid_base64() {
        let url = "data:image/png;base64,!!!";
        assert!(decode_outgoing_image(url).unwrap_err().contains("解码失败"));
    }

    #[test]
    fn decode_rejects_malformed_data_url() {
        assert!(decode_outgoing_image("not-a-data-url").is_err());
        assert!(decode_outgoing_image("data:image/png").is_err());
        assert!(decode_outgoing_image("data:image/png,raw").is_err());
    }

    #[test]
    fn decode_rejects_empty_image() {
        let url = data_url("image/png", b"");
        assert!(decode_outgoing_image(&url).unwrap_err().contains("为空"));
    }

    #[test]
    fn decode_rejects_over_byte_limit() {
        let big = vec![0u8; (MAX_OUTGOING_IMAGE_BYTES + 1) as usize];
        let url = data_url("image/png", &big);
        let err = decode_outgoing_image(&url).unwrap_err();
        assert!(err.contains("过大"));
        assert!(err.contains(&MAX_OUTGOING_IMAGE_BYTES.to_string()));
    }

    #[test]
    fn decode_respects_exact_byte_limit() {
        let exact = vec![0u8; MAX_OUTGOING_IMAGE_BYTES as usize];
        let url = data_url("image/png", &exact);
        let (_, bytes) = decode_outgoing_image(&url).unwrap();
        assert_eq!(bytes.len() as u64, MAX_OUTGOING_IMAGE_BYTES);
    }
}
