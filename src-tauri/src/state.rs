//! 应用全局状态与前端交互类型。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, watch, Notify};

use crate::crypto::Identity;
use crate::db;
use crate::device::{hardware_fingerprint, hostname_fingerprint};
use crate::gossip_engine::GossipEngine;
use crate::protocol::{Message, TCP_PORT};
use crate::relay_manager::RelayManager;

/// 局域网在线节点（Peer Table 条目）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Peer {
    pub device_id: String,
    pub nickname: String,
    pub avatar: Option<String>,
    pub ip: String,
    pub tcp_port: u16,
    pub last_seen: i64,
    /// 最近一次心跳往返时延（毫秒）
    pub rtt_ms: Option<u64>,
    /// X25519 公钥（base64，ECDH 用）
    pub x25519_pubkey: Option<String>,
    /// Ed25519 公钥（base64，验签用）
    pub ed25519_pubkey: Option<String>,
    /// 建链时间戳
    pub connected_since: Option<i64>,
}

/// 待处理的好友申请
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PendingRequest {
    pub from: String,
    pub from_nickname: String,
    pub from_avatar: Option<String>,
    pub ts: i64,
}

/// 单条消息记录（与前端一致）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MessageRecord {
    pub id: i64,
    pub msg_id: String,
    pub conv_id: String,
    pub sender_id: String,
    pub receiver_id: String,
    pub kind: String,
    pub content: String,
    pub ts: i64,
    pub status: String,
}

/// 会话摘要（会话列表）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Conversation {
    pub id: String,
    pub kind: String, // single | group
    pub name: String,
    pub avatar: Option<String>,
    pub last_msg: Option<String>,
    pub last_ts: Option<i64>,
    pub unread: i64,
}

/// 好友
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Friend {
    pub device_id: String,
    pub nickname: String,
    pub avatar: Option<String>,
    pub online: bool,
}

/// 群组
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub creator: String,
    pub members: Vec<String>,
}

/// 本机信息
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeviceInfo {
    pub device_id: String,
    pub nickname: String,
    pub avatar: Option<String>,
    pub tcp_port: u16,
    pub online: bool,
    /// 本机 X25519 公钥（base64）
    pub x25519_pubkey: String,
    /// 本机 Ed25519 公钥（base64）
    pub ed25519_pubkey: String,
}

/// 网卡信息
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InterfaceInfo {
    pub name: String,
    pub ip: String,
}

/// 网络拓扑摘要（供拓扑状态栏展示）
#[derive(Serialize, Clone, Debug)]
pub struct TopologyInfo {
    pub node_count: usize,
    pub relay_count: usize,
    pub avg_rtt_ms: Option<u64>,
    pub online: bool,
}

/// 文件传输进度事件
#[derive(Serialize, Clone, Debug)]
pub struct FileProgress {
    pub transfer_id: String,
    pub received: u64,
    pub total: u64,
}

/// 文件接收完成事件
#[derive(Serialize, Clone, Debug)]
pub struct FileDoneInfo {
    pub transfer_id: String,
    pub name: String,
    pub size: u64,
    pub path: String,
}

/// 文件传输状态
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TransferInfo {
    pub id: String,
    pub peer_id: String,
    pub name: String,
    pub size: u64,
    pub direction: String,
    pub status: String,
    pub path: Option<String>,
    pub progress: f64,
}

/// 正在接收的文件状态
pub struct FileReceiver {
    pub file: std::fs::File,
    pub name: String,
    pub size: u64,
    pub received: u64,
    pub tmp_path: PathBuf,
    pub final_path: PathBuf,
    pub peer_id: String,
    /// 上次进度上报时间（毫秒），用于节流 IPC 事件
    pub last_report_ms: i64,
}

/// 网络运行时句柄
pub struct NetworkHandle {
    pub shutdown: tokio::sync::watch::Sender<bool>,
    pub bound_ip: String,
    pub tcp_port: u16,
}

pub struct AppState {
    pub app: AppHandle,
    pub db: Mutex<Connection>,
    pub device_id: String,
    pub tcp_port: u16,
    pub downloads_dir: PathBuf,
    /// 缓存目录：图片 / 音频 / 文件等二进制落盘于此（SQLite 不存 BLOB）
    pub cache_dir: PathBuf,

    /// 节点身份（X25519 + Ed25519）
    pub identity: Identity,
    /// Gossip 去重 + 扇出引擎
    pub gossip: Mutex<GossipEngine>,
    /// 大文件切片中继管理器
    pub relay: Mutex<RelayManager>,
    /// 群密钥缓存：group_id -> 对称密钥
    pub group_keys: Mutex<HashMap<String, [u8; 32]>>,

    /// 在线节点表：device_id -> Peer
    pub peers: Mutex<HashMap<String, Peer>>,
    /// 已建立的 TCP 连接出站发送端：device_id -> mpsc Sender
    pub links: tokio::sync::Mutex<HashMap<String, mpsc::Sender<Message>>>,
    /// 待处理好友申请：from_id -> request
    pub pending_requests: Mutex<HashMap<String, PendingRequest>>,
    /// 网络运行时（None 表示未启动）
    pub network: Mutex<Option<NetworkHandle>>,
    /// 待发已读回执：peer_id -> last_read_ts。链路不可用时暂存，
    /// 由建链 / Hello / 心跳（与 outbox 补发同一批触发点）冲刷，只保留最大值。
    pub pending_reads: Mutex<HashMap<String, i64>>,

    /// 共享目录（本机）
    pub share_dir: Mutex<Option<String>>,
    /// 当前昵称缓存
    pub nickname: Mutex<String>,
    /// 当前头像缓存（base64 data URI）
    pub avatar: Mutex<Option<String>>,

    /// 等待对方接受的文件传输：transfer_id -> 接受信号
    pub pending_file_accept: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    /// 正在接收的文件：transfer_id -> FileReceiver
    pub file_receivers: Mutex<HashMap<String, FileReceiver>>,
    /// 等待共享目录树响应：request_id -> 应答通道
    pub pending_share_tree: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Vec<crate::protocol::ShareEntry>>>>,

    /// 节点表是否需要向前端推送（节流合并用，见 `spawn_peer_emitter`）
    pub peers_dirty: AtomicBool,
    /// 节点表变更通知（节流合并的唤醒信号）
    pub peers_notify: Arc<Notify>,
    /// 按需探测触发：值递增 → 发现任务立即群发一次 `who_has`（好友搜索用）
    pub probe: Mutex<Option<watch::Sender<u64>>>,
}

impl AppState {
    /// 初始化应用状态：解析目录、打开数据库、加载/生成设备指纹与身份密钥。
    pub fn init(app: AppHandle) -> Result<Arc<AppState>, Box<dyn std::error::Error>> {
        let app_data = app.path().app_data_dir()?;
        std::fs::create_dir_all(&app_data).ok();
        let downloads_dir = app_data.join("downloads");
        std::fs::create_dir_all(&downloads_dir).ok();
        let cache_dir = app_data.join("cache");
        std::fs::create_dir_all(&cache_dir).ok();

        // 多开支持：`--instance N`（或环境变量 GOSSLAN_INSTANCE）→ 独立数据库 / 端口 / 设备指纹
        let instance = instance_id();
        let db_name = if instance > 0 {
            format!("gosslan-{instance}.db")
        } else {
            "gosslan.db".to_string()
        };
        let conn = db::init(&app_data.join(db_name))?;

        // 设备指纹：优先机器码，回退持久化 UUID
        let base_device = if let Some(id) = hardware_fingerprint() {
            id
        } else if let Some(id) = db::get_setting(&conn, "device_id") {
            id
        } else {
            let id = format!("dev-{}", hostname_fingerprint());
            db::set_setting(&conn, "device_id", &id).ok();
            id
        };
        // 多开时给不同实例不同 device_id，使其成为互相可发现的独立节点
        let device_id = if instance > 0 {
            format!("{base_device}-i{instance}")
        } else {
            base_device.clone()
        };
        if instance == 0 {
            db::set_setting(&conn, "device_id", &base_device).ok();
        }

        // 多开时偏移 TCP 端口（UDP 端口保持共享的发现通道，配合 SO_REUSEADDR 多实例共存）
        let tcp_port: u16 = if instance > 0 {
            TCP_PORT.saturating_add((instance * 10) as u16)
        } else {
            db::get_setting(&conn, "tcp_port")
                .and_then(|s| s.parse().ok())
                .unwrap_or(TCP_PORT)
        };

        let nickname = db::get_setting(&conn, "nickname").unwrap_or_else(|| {
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "Gosslan 用户".to_string())
        });
        let avatar = db::get_setting(&conn, "avatar");
        let share_dir = db::get_setting(&conn, "share_dir");

        // 启动时从 DB 恢复待发已读回执（进程重启后 pending_reads 内存丢失的恢复路径）
        let mut pending_reads_map = HashMap::new();
        if let Ok(rows) = db::load_pending_reads(&conn) {
            for (peer_id, ts) in rows {
                let cur = pending_reads_map.entry(peer_id).or_insert(ts);
                *cur = (*cur).max(ts);
            }
        }

        // 加载或生成身份密钥（X25519 + Ed25519）
        let identity = match (
            db::get_setting(&conn, "x25519_secret"),
            db::get_setting(&conn, "ed25519_secret"),
        ) {
            (Some(xs), Some(es)) => Identity::from_secrets(&xs, &es).unwrap_or_else(Identity::generate),
            _ => {
                let id = Identity::generate();
                db::set_setting(&conn, "x25519_secret", &id.x25519_secret_b64()).ok();
                db::set_setting(&conn, "ed25519_secret", &id.ed25519_secret_b64()).ok();
                id
            }
        };

        Ok(Arc::new(AppState {
            app,
            db: Mutex::new(conn),
            device_id,
            tcp_port,
            downloads_dir,
            cache_dir,
            identity,
            gossip: Mutex::new(GossipEngine::new(100_000, 10_000, 4, 6)),
            relay: Mutex::new(RelayManager::new()),
            group_keys: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            links: tokio::sync::Mutex::new(HashMap::new()),
            pending_requests: Mutex::new(HashMap::new()),
            network: Mutex::new(None),
            pending_reads: Mutex::new(pending_reads_map),
            share_dir: Mutex::new(share_dir),
            nickname: Mutex::new(nickname),
            avatar: Mutex::new(avatar),
            pending_file_accept: Mutex::new(HashMap::new()),
            file_receivers: Mutex::new(HashMap::new()),
            pending_share_tree: Mutex::new(HashMap::new()),
            peers_dirty: AtomicBool::new(false),
            peers_notify: Arc::new(Notify::new()),
            probe: Mutex::new(None),
        }))
    }

    /// 标记节点表已变更，并唤醒节流推送任务。
    ///
    /// 500-1000 节点场景下，每秒会收到数百条 `announce`/`heartbeat`，若每次
    /// 都全量序列化节点表并推给前端会拖垮 IPC。这里改为「置脏 + 通知」，
    /// 由 `spawn_peer_emitter` 在约 300ms 合并窗口内最多推送一次。
    pub fn emit_peers(&self) {
        self.peers_dirty.store(true, Ordering::SeqCst);
        self.peers_notify.notify_one();
    }

    /// 实际序列化并推送节点表（仅由节流任务调用）。
    fn emit_peers_now(&self) {
        let peers: Vec<Peer> = self.peers.lock().unwrap().values().cloned().collect();
        let _ = self.app.emit("peers-updated", peers);
    }

    /// 启动节点表节流推送任务（合并高频更新，避免 IPC 风暴）。
    pub fn spawn_peer_emitter(state: &Arc<AppState>) {
        let st = state.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                st.peers_notify.notified().await;
                // 合并窗口：窗口内的所有变更只触发一次推送
                tokio::time::sleep(Duration::from_millis(300)).await;
                if st.peers_dirty.swap(false, Ordering::SeqCst) {
                    st.emit_peers_now();
                }
            }
        });
    }
}

/// 解析多开实例号：优先 `--instance N` / `-i N` 启动参数，其次 `GOSSLAN_INSTANCE` 环境变量。
/// 返回 0 表示默认单实例。
fn instance_id() -> u32 {
    let args: Vec<String> = std::env::args().collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--instance" || a == "-i" {
            if let Some(v) = it.next().and_then(|s| s.parse::<u32>().ok()) {
                return v;
            }
        }
    }
    std::env::var("GOSSLAN_INSTANCE")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}
