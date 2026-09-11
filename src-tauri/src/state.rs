//! 应用全局状态与前端交互类型。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use crate::logging::Logger;
use crate::mesh::manager::PeerManager;
use crate::mesh::path::PathKind;
use crate::mesh::router::MeshRouter;
use crate::protocol::{Message, TCP_PORT};
use crate::relay_manager::RelayManager;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 系统语言是否为中文（供「跟随系统」语言偏好时判断应用显示名）。
///
/// 后端不引入系统 locale 库，用 POSIX 环境变量 `LANG` / `LC_ALL` / `LC_MESSAGES`
/// 兜底：macOS/Linux 的 `LANG` 通常是 `zh_CN.UTF-8` / `en_US.UTF-8`，能正确判断。
/// **Windows 边界**：Windows 一般无 `LANG`，这里会回落「英文」——中文 Windows 用户若
/// 未在设置里显式选中文，后端自行生成的次要文案（托盘提示 / 日志窗口标题）会显示英文。
/// 影响有限：主界面由前端 `navigator.language` 正确判断；如需彻底对齐，后续可加
/// Windows API（GetUserDefaultUILanguage）判断系统 UI 语言。
fn system_lang_is_zh() -> bool {
    ["LANG", "LC_ALL", "LC_MESSAGES"].iter().any(|k| {
        std::env::var(k)
            .map(|v| v.to_lowercase().contains("zh"))
            .unwrap_or(false)
    })
}

/// 局域网在线节点（Peer Table 条目）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Peer {
    pub device_id: String,
    pub nickname: String,
    pub avatar: Option<String>,
    /// 设备类型（"desktop" / "mobile"，空串 = 未知/旧端）。来自 Hello/UserInfo/Presence。
    #[serde(default)]
    pub device_type: String,
    pub ip: String,
    pub tcp_port: u16,
    pub last_seen: i64,
    /// 最近一次心跳往返时延（毫秒）
    pub rtt_ms: Option<u64>,
    /// X25519 公钥（base64，ECDH 用）
    pub x25519_pubkey: Option<String>,
    /// Ed25519 公钥（base64，验签用）
    pub ed25519_pubkey: Option<String>,
    /// 首次发现该节点的时间戳（announce / Presence 首次学到）。用于「小 ID 兜底拨号」
    /// 判断「对端在线却迟迟连不上」（单向可达）——语义是**发现时间**，不是建链时间。
    #[serde(default)]
    pub first_seen: Option<i64>,
}

/// 一条已建立的 TCP 连接。
///
/// `endpoint` 让连接可以按**端点**去重（同一 peer 的 LAN 与 Tailscale 是两条不同连接），
/// 也是 6b「同一 Peer 多条 Connection」的判据。
#[derive(Clone)]
pub struct Link {
    /// 该连接对端的端点（transport 无关：TCP 地址或 BLE 标识）。
    ///
    /// 为什么不是 `SocketAddr`：BLE 端点没有 IP。见 `mesh::Endpoint` 与 ADR-0015。
    pub endpoint: crate::mesh::Endpoint,
    /// 这条连接**走的是哪条路径**（LAN / Routed / Bluetooth）。
    ///
    /// 为什么必须显式携带、而不是从 `endpoint` 的 IP 段反推（`path_kind_for`）：
    /// 用户把**私有网段地址**（10.x / 192.168.x / 172.16.x）填进「路由端点」时，
    /// 按 IP 反推会判成 LAN ⇒ ① `has_lan_path` 误判为真 ⇒ `ensure_link` 不再拨真正的
    /// LAN 路径（把 D5 的修复绕过去了）；② 选路时按最高优先级当成 LAN。
    /// 而 BLE 端点根本没有 IP，更无解。⇒ 路径类型必须由**来路**决定。
    pub path_kind: PathKind,
    /// bulk 通道：大文件分片等，避免挤占聊天。
    pub bulk: mpsc::Sender<Message>,
    /// priority 通道：聊天 / 控制 / 心跳，避免被大文件分片饿死（INV-P20）。
    pub priority: mpsc::Sender<Message>,
    /// **本连接**的取消信号（M3#6 死链路拆除用）。
    ///
    /// 为什么需要它：全局 `shutdown` 只能整体停网，无法单独断开一条僵尸连接。
    /// 半开 TCP 上读循环会**永久**阻塞在 `read_frame`（既无数据也无错误），
    /// 于是链路一直留在 `links` 里 ⇒ `ensure_link` 认为 LAN 已连通、不再重拨，
    /// `try_send` 又只把消息投进 mpsc 就返回 Ok ⇒ **消息静默投进死路**。
    /// 有了它，健康 watchdog 可以在读活性长期过期时精确拆掉这一条连接，
    /// 让发现层重新建链。读写循环都会 select 这个信号。
    pub cancel: watch::Sender<bool>,
}

/// 待处理的好友申请
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PendingRequest {
    pub from: String,
    pub from_nickname: String,
    pub from_avatar: Option<String>,
    pub ts: i64,
}

/// 会话的「当前链路」快照：最近一条消息走的链路 + 中间节点数。
///
/// - `path`：`"lan"` / `"routed"` / `"bluetooth"`。直连时是入站连接的真实路径；
///   桥接时是「最后一段」的路径（发送方第一段链路需信封携带，Phase 后续补齐）。
/// - `hop`：中间节点数（0 = 直连）。由 Gossip 的 ttl 反推（初始 ttl - 收到 ttl）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LinkState {
    pub path: String,
    pub hop: u8,
}

impl Default for LinkState {
    fn default() -> Self {
        LinkState {
            path: "lan".to_string(),
            hop: 0,
        }
    }
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
    /// 每会话逻辑序号（Lamport 风格），排序与清空边界都以此为准。
    pub seq: i64,
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
    /// 设备类型（"desktop" / "mobile"，空串 = 未知）。从 peers 表现场读取。
    #[serde(default)]
    pub device_type: String,
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
    /// 本机设备类型（"desktop" / "mobile"）。
    pub device_type: String,
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
    /// 是否为真实局域网卡（有广播地址、非 link-local、非 VPN 虚拟地址）。
    /// false 表示疑似 VPN / Clash / tun / tap 等虚拟网卡。
    #[serde(default = "default_true")]
    pub is_lan: bool,
}

fn default_true() -> bool {
    true
}

/// 网络接口候选（含评分），供开发者诊断面板展示 Discovery 实际看到的候选列表。
#[derive(Serialize, Clone, Debug)]
pub struct InterfaceCandidate {
    pub name: String,
    pub ip: String,
    pub has_broadcast: bool,
    pub broadcast: Option<String>,
    pub is_rfc1918: bool,
    pub is_virtual: bool,
    pub score: i32,
    pub selected: bool,
}

/// Discovery 诊断事件（ring buffer 条目）。
#[derive(Serialize, Clone, Debug)]
pub struct DiscoveryEvent {
    /// 事件发生时的 Unix 毫秒时间戳
    pub ts: i64,
    /// 事件类型
    pub kind: String,
    /// 简短描述（不含密钥/私密数据，IP 地址可显示）
    pub detail: String,
}

/// Discovery 运行时诊断状态（只读，供开发者面板展示）。
#[derive(Serialize, Clone, Debug, Default)]
pub struct DiscoveryDiag {
    /// 当前网络模式：auto / manual / offline
    pub mode: String,
    /// 实际绑定的 IP
    pub bound_ip: String,
    /// 选中的接口名称（如有）
    pub selected_interface: String,
    /// 选中的接口 IP（如有）
    pub selected_ip: String,
    /// TCP 监听地址
    pub tcp_listen: String,
    /// UDP 发现端口
    pub udp_port: u16,
    /// 广播目标地址
    pub broadcast_target: String,
    /// 组播地址
    pub multicast_group: String,
    /// 组播 join 结果
    pub multicast_join_result: String,
    /// set_multicast_if_v4 结果
    pub multicast_if_result: String,
    /// 最近一次广播发送时间（ms since epoch，0=从未）
    pub last_broadcast_send: i64,
    /// 最近一次组播发送时间
    pub last_multicast_send: i64,
    /// 最近一次 announce 接收时间
    pub last_announce_recv: i64,
    /// 候选接口列表（含评分）
    pub candidates: Vec<InterfaceCandidate>,
    /// 最近事件（ring buffer，最新在末尾）
    pub recent_events: Vec<DiscoveryEvent>,
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

#[derive(Serialize, Clone, Debug)]
pub struct FileFailedInfo {
    pub transfer_id: String,
    pub reason: String,
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

/// 群文件元数据（DB 表 group_files）。一个 transfer_id 对应一个群文件；
/// 成员投递状态在 group_file_recipients 中按 (transfer_id, recipient_id) 独立维护。
pub struct GroupFile {
    pub transfer_id: String,
    pub group_id: String,
    pub sender_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// 'pending' | 'sending' | 'completed' | 'failed'
    pub status: String,
    pub created_at: i64,
}

/// 群文件单个成员的投递状态（DB 表 group_file_recipients）。
// 传输流程在后续 GroupFileOffer 步骤启用；本步骤仅 DB 层 + 测试调用。
#[allow(dead_code)]
pub struct GroupFileRecipient {
    pub recipient_id: String,
    /// 'pending' | 'sending' | 'completed' | 'failed'
    pub status: String,
    /// 0.0 ~ 1.0，与 file_transfers.progress 同一表示
    pub progress: f64,
    pub updated_at: i64,
}

/// 正在接收的文件状态
pub struct FileReceiver {
    pub file: std::fs::File,
    pub name: String,
    pub size: u64,
    pub received: u64,
    /// 直连 TCP 虽然有序，但仍校验序号，避免错误/恶意帧把文件静默拼坏。
    pub next_seq: u32,
    pub tmp_path: PathBuf,
    pub final_path: PathBuf,
    pub peer_id: String,
    /// 上次进度上报时间（毫秒），用于节流 IPC 事件
    pub last_report_ms: i64,
    /// 本 transfer 的文件会话密钥：FileOffer 中以我方公钥 E2EE 封装，
    /// 解封后仅存于内存；每个分片以此 AEAD 解密，密文绝不落盘。
    pub file_key: [u8; 32],
    /// 发送方声明的原文件 SHA-256（hex），FileDone 时与实际哈希比对
    pub expected_sha256: String,
    /// 明文增量哈希：write_chunk 解密后 update，finish 时 finalize 比对
    pub hasher: sha2::Sha256,
}

/// 中继接收的文件会话状态（仅最终接收方持有；中继节点不解密不校验）。
pub struct RelayFileReceive {
    /// 本 transfer 的文件会话密钥（RelayFileOffer 中以我方公钥封装）
    pub file_key: [u8; 32],
    /// 发送方声明的原文件 SHA-256（hex）
    pub expected_sha256: String,
    /// 明文增量哈希：逐片解密后 update，重组完成时 finalize 比对
    pub hasher: sha2::Sha256,
}

/// 网络运行时句柄
pub struct NetworkHandle {
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// 用户配置的绑定地址（"0.0.0.0" 表示 auto，否则为手动指定的 IP）。
    pub bound_ip: String,
    /// Discovery 实际绑定的 UDP IP（auto 模式下为 find_lan_interface 选出的 LAN IP）。
    /// 用于诊断面板显示真实 bind 地址。
    pub actual_bound_ip: String,
    pub tcp_port: u16,
    /// 后台任务句柄（discovery 收包 / 广播、transport accept / 心跳）。
    ///
    /// `stop()` 必须等它们真正退出再返回：否则旧进程（或同一进程内旧实例）的
    /// TCP listener 仍挂在端口上，紧接着的 `start()`/新进程 bind 就会撞上
    /// AddrInUse（Windows 上表现为重启后永久掉线）。
    pub tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// 全局应用状态。
///
/// 加锁约定：所有 `std::sync::Mutex` 一律用
/// `lock().unwrap_or_else(|e| e.into_inner())` 取锁，**不要写 `.lock().unwrap()`**。
/// 原因：Rust 的互斥量在「持锁线程 panic」后会进入中毒状态，若后续都用 `.unwrap()`，
/// 一次 panic 就会让之后**所有**加锁点级联 panic（整个应用不可用）；
/// `into_inner()` 取回内部数据继续用，把影响限制在最初那次 panic。
/// （`tokio::sync::Mutex` 无中毒概念，正常 `.lock().await` 即可。）
/// 同时进行的拨号上限（见 `AppState::dial_permits`）。
/// 取 16：3 台设备的日常场景远远用不到，而伪造 announce 的洪泛会被它挡住。
pub const MAX_CONCURRENT_DIALS: usize = 16;

/// 同时保持的入站连接上限（见 `AppState::inbound_permits`）。
pub const MAX_INBOUND_CONNECTIONS: usize = 128;

/// 拨号在途标记的 RAII 守卫：Drop 即释放（任何提前 return / panic 都不会漏放）。
pub struct DialGuard {
    state: Arc<AppState>,
    key: String,
}

impl DialGuard {
    /// 尝试登记一个在途拨号；已被其他拨号占用时返回 `None`（调用方应直接返回）。
    pub fn try_acquire(state: &Arc<AppState>, key: String) -> Option<Self> {
        let inserted = state
            .dialing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone());
        if !inserted {
            return None;
        }
        Some(Self {
            state: state.clone(),
            key,
        })
    }
}

impl Drop for DialGuard {
    fn drop(&mut self) {
        self.state
            .dialing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

pub struct AppState {
    pub app: AppHandle,
    pub db: Mutex<Connection>,
    pub device_id: String,
    pub tcp_port: u16,
    /// **在途拨号**集合（D6）：正在 connect/握手的拨号键（peer_id 或端点字符串）。
    ///
    /// 为什么需要：`connect_to_peer` 的"已连接？"检查与"登记链路"之间隔着 connect +
    /// 握手（最长 10s），是典型的 check-then-act。两条并发路径（Routed 每端点一个任务、
    /// LAN announce 触发的 ensure_link）会同时看到"还没连上" ⇒ 同一目标被拨两次，
    /// `links[peer]` 出现两条完全相同的端点。有了在途标记，第二个拨号直接返回。
    pub dialing: Mutex<std::collections::HashSet<String>>,
    /// 并发拨号上限信号量（**不是**在途去重，是资源上限）。
    ///
    /// 为什么还要它：在途集合只挡"同一目标"，挡不住"一万个不同目标"——伪造 announce
    /// 可以让发现层对着大量黑洞地址同时发起 connect，每个最长 10s。给一个上限后，
    /// 超出部分本轮直接放弃（下一个 announce 周期还会再来），不会堆积任务与 socket。
    pub dial_permits: Arc<tokio::sync::Semaphore>,
    /// 入站连接上限（`handle_incoming` 任务持一个许可直到连接结束）。
    ///
    /// 与 `dial_permits` 对称：一个管"我拨出去"，一个管"别人拨进来"。
    /// 128 对 3 台设备绰绰有余，而伪造 announce/洪水连接会被它挡在 accept 之后立刻丢弃。
    pub inbound_permits: Arc<tokio::sync::Semaphore>,
    /// 蓝牙运行时句柄（`feature = "bluetooth"` 才有；见 `network/ble.rs`）。
    ///
    /// 与 LAN 的 `network` 字段同思路：句柄在 AppState 里，`start/stop` 由命令层驱动。
    /// 默认 `None`，且只有用户在设置里打开「蓝牙」才会启动 —— 局域网不受影响。
    #[cfg(feature = "bluetooth")]
    pub ble: Mutex<Option<crate::network::ble::BleHandle>>,
    /// 中继授权配置（P2 / M4）：策略 + 白名单。
    ///
    /// 为什么缓存在内存：转发热路径上 gossip 可能每秒几十条，为了一个策略字段去锁
    /// SQLite 是纯浪费。启动时从 settings 读入，`save_settings` 时更新。
    pub relay_policy: Mutex<crate::mesh::relay_policy::RelayConfig>,
    /// 文件接收落盘目录（可变：设置页可改，改后新接收的文件落到新目录）。
    pub downloads_dir: Mutex<PathBuf>,
    /// 缓存目录：图片 / 音频 / 文件等二进制落盘于此（SQLite 不存 BLOB）
    pub cache_dir: PathBuf,
    /// SQLite 数据库文件路径（存储页展示占用用；含 -wal/-shm 伴生文件）。
    pub db_path: PathBuf,
    /// 应用级运行日志（内存 ring buffer + 落盘文件），供「运行日志」页读取与排查。
    pub logger: Logger,

    /// 节点身份（X25519 + Ed25519）
    pub identity: Identity,
    /// Gossip 去重 + 扇出引擎
    pub gossip: Mutex<GossipEngine>,
    /// Mesh 层的 Peer 生命周期管理（Phase 2 建、6b-3 接线）。
    ///
    /// 与 `links` 的关系：`links` 是**传输层**的连接表（每条 TCP 连接一个 `Link`），
    /// 这里是 **mesh 层**的 Peer/Connection 模型。两者由 `register_connection` /
    /// `unregister_connection` 保持同步 —— 6b-3 之后，「任一 Connection 健康 ⇒ Online」
    /// 的语义才有真实的连接数据支撑。
    pub peer_manager: Mutex<PeerManager>,
    /// Mesh 转发路由：全局去重 + TTL + 转发决策（Phase 5）。
    ///
    /// 与 `gossip` 的关系：本字段是 **Mesh 层**去重（§16 要求必须在 Mesh 层统一做），
    /// `gossip` 是业务信封层去重。两者参数一致、键同为 message_id，构成两道防线；
    /// 待 Mesh 路径稳定后收敛为一道。
    pub mesh_router: Mutex<MeshRouter>,
    /// 大文件切片中继管理器
    pub relay: Mutex<RelayManager>,
    /// 群密钥缓存：group_id -> 对称密钥
    pub group_keys: Mutex<HashMap<String, [u8; 32]>>,

    /// 在线节点表：device_id -> Peer
    pub peers: Mutex<HashMap<String, Peer>>,
    /// 已建立的 TCP 连接：device_id -> 该节点的**各条**连接。
    ///
    /// Phase 6 起一个 device_id 可以有多条连接（LAN + Tailscale + BLE），每条连接的
    /// 端点与两个发送通道放在一起，避免「端点 / bulk / priority」三处平行结构
    /// 需要手工同步下标 —— 那是 Phase 6 最容易出错的地方。
    pub links: tokio::sync::Mutex<HashMap<String, Vec<Link>>>,
    /// 待处理好友申请：from_id -> request
    pub pending_requests: Mutex<HashMap<String, PendingRequest>>,
    /// 网络运行时（None 表示未启动）
    pub network: Mutex<Option<NetworkHandle>>,
    /// 待发已读回执：peer_id -> last_read_ts。链路不可用时暂存，
    /// 由建链 / Hello / 心跳（与 outbox 补发同一批触发点）冲刷，只保留最大值。
    pub pending_reads: Mutex<HashMap<String, i64>>,
    /// 待发群密钥：peer_id -> 待补发的 group_id 集合。
    /// `redistribute_group_keys` 在 TCP link 尚未建立（announce 先于 ensure_link）
    /// 时发送会失败，此前被静默丢弃导致成员永久拿不到群密钥。
    /// 此处登记失败项，由建链 / Hello / 心跳的 flush_pending_group_keys 重试。
    pub pending_group_keys: Mutex<HashMap<String, std::collections::HashSet<String>>>,

    /// 会话的「当前链路」快照：conv_id -> LinkState（最近一条消息的链路 + 跳数）。
    /// 收发单聊消息时更新，前端聊天窗口据此显示连接图标（LAN / 桥接 / 蓝牙）。
    pub conv_link: Mutex<HashMap<String, LinkState>>,

    /// 共享目录（本机）
    pub share_dir: Mutex<Option<String>>,
    /// 当前昵称缓存
    pub nickname: Mutex<String>,
    /// 当前头像缓存（base64 data URI）
    pub avatar: Mutex<Option<String>>,

    /// 等待对方接受的文件传输：transfer_id -> 接受信号
    pub pending_file_accept: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    /// 等待接收方完成确认的直连文件传输：transfer_id -> 完成信号。
    /// 发送方在 FileDone 之后等待 FileCompleteAck，只有 success=true 才推进 delivered。
    pub pending_file_complete: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// 中继接收的文件会话状态：transfer_id -> 密钥 + 预期 SHA-256 + 增量哈希。
    /// RelayFileOffer 中以我方公钥 E2EE 封装，解封后仅存内存；
    /// 中继节点不持有密钥、不参与解密与校验，只透传密文切片。
    pub relay_file_keys: Mutex<HashMap<String, RelayFileReceive>>,
    /// 群文件会话密钥：transfer_id -> file_key（仅内存，不落库、不进日志）。
    /// 发送侧：send_group_file 生成后暂存，供后续 GroupFileChunk 加密使用；
    /// 接收侧：GroupFileOffer 解封后保存，供后续解密使用。
    /// 同一 transfer 全体成员使用同一个 file_key（经群密钥封装分发）。
    pub group_file_keys: Mutex<HashMap<String, [u8; 32]>>,
    /// 群文件离线投递进行中标记：同一 peer 同时最多一个投递任务
    /// （顺序发送其 pending 群文件）；不同 peer 之间并行。
    pub group_file_sending: Mutex<std::collections::HashSet<String>>,
    /// 群文件「进度条分母」快照：transfer_id -> **发送时在线的** recipient 集合。
    ///
    /// 用户口径（2026-09-12 反馈）：进度条只按**当前在线成员**算 —— 所有在线成员都收到
    /// 即 100%；**离线成员不计入分母**，他上线后的补发也**不回退**进度条。
    /// 因此分母必须在**发送那一刻冻结**：若用「此刻在线」动态算，后上线的成员会把分母
    /// 变大、进度条倒退（正是用户明确不要的「补发算进进度条」）。
    /// 不落库：它只是展示口径，进程重启后丢失不影响投递正确性（此后进度条按
    /// 「已完成 / 全体」的兜底口径显示）。
    pub group_file_online_targets: Mutex<HashMap<String, std::collections::HashSet<String>>>,
    /// 一对一文件离线投递进行中标记：同一 peer 同时最多一个投递任务。
    pub file_sending: Mutex<std::collections::HashSet<String>>,
    /// 群文件接收端 `.part` 状态：transfer_id -> 接收状态。
    /// 与一对一 `file_receivers` 生命周期独立；复用 FileReceiver 结构
    /// （file_key/next_seq/hasher 语义相同），不写 file_transfers 表。
    pub group_file_receivers: Mutex<HashMap<String, FileReceiver>>,
    /// 正在接收的文件：transfer_id -> FileReceiver
    pub file_receivers: Mutex<HashMap<String, FileReceiver>>,
    /// 等待共享目录树响应：request_id -> 应答通道
    pub pending_share_tree:
        Mutex<HashMap<String, tokio::sync::oneshot::Sender<Vec<crate::protocol::ShareEntry>>>>,

    /// 节点表是否需要向前端推送（节流合并用，见 `spawn_peer_emitter`）
    pub peers_dirty: AtomicBool,
    /// **网络世代号**（D8-4）：每次 `start()`/`stop()` 自增。
    ///
    /// 为什么需要：`handle_incoming` 是 accept 时就 spawn 的长任务，它要**握手完成后**
    /// 才登记链路。若期间用户切换了网卡/重开通道（stop→start），这个"上个世代"的任务
    /// 仍会把链路登记进**新世代**的 `links`/`peer_manager` —— 一条没人认识、也不会被
    /// 新世代 shutdown 覆盖的连接（旧 socket 早已断，但状态是真的）。
    /// 世代号让"登记前先确认自己还属于当前世代"成为一行判断。
    pub network_generation: AtomicU64,
    /// 节点表变更通知（节流合并的唤醒信号）
    pub peers_notify: Arc<Notify>,
    /// 按需探测触发：值递增 → 发现任务立即群发一次 `who_has`（好友搜索用）
    pub probe: Mutex<Option<watch::Sender<u64>>>,

    /// Discovery 诊断状态（隐藏开发者面板用，只读展示不改变网络行为）
    pub diag: Mutex<DiscoveryDiag>,
    /// Discovery 事件 ring buffer（最近 50 条，防无限增长）
    pub diag_events: Mutex<VecDeque<DiscoveryEvent>>,
    /// Hello 握手防重放：近期已接受的 nonce（有界 FIFO，超出丢弃最旧）。
    /// 与本地时钟无关，因此不受设备间时间偏差影响。
    pub seen_hello_nonces: Mutex<VecDeque<String>>,
    /// 已就「身份密钥冲突」告警过的 device_id（本进程内去重）。
    /// announce 每 5s 一次、冲突会持续存在，不去重会把聊天记录刷爆。
    pub key_conflict_warned: Mutex<std::collections::HashSet<String>>,
}

impl AppState {
    /// 初始化应用状态：解析目录、打开数据库、加载/生成设备指纹与身份密钥。
    pub fn init(app: AppHandle) -> Result<Arc<AppState>, Box<dyn std::error::Error>> {
        let app_data = app.path().app_data_dir()?;
        std::fs::create_dir_all(&app_data).ok();
        let default_downloads = app_data.join("downloads");
        std::fs::create_dir_all(&default_downloads).ok();
        let cache_dir = app_data.join("cache");
        std::fs::create_dir_all(&cache_dir).ok();

        // 多开支持：`--instance N`（或环境变量 GOSSLAN_INSTANCE）→ 独立数据库 / 端口 / 设备指纹
        let instance = instance_id();
        let db_name = if instance > 0 {
            format!("gosslan-{instance}.db")
        } else {
            "gosslan.db".to_string()
        };
        let db_path = app_data.join(db_name);
        let conn = db::init(&db_path)?;

        // 运行日志：多开实例用独立文件（gosslan-1.log），避免测试实例互相覆盖。
        let log_stem = if instance > 0 {
            format!("gosslan-{instance}")
        } else {
            "gosslan".to_string()
        };
        let logger = Logger::new(app_data.join("logs"), &log_stem);

        // 文件接收目录：默认 app_data/downloads，允许用户在设置里改（持久化到 settings）。
        // 与共享目录同理：用户自选的目录在沙盒里重启后会失访，必须靠书签把权限带回来，
        // 否则"收到的文件写不进去"（而且同样没有报错弹窗）。
        let downloads_dir = crate::user_dirs::load(&conn, crate::user_dirs::RECEIVE)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(default_downloads);
        std::fs::create_dir_all(&downloads_dir).ok();

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
        // 共享目录：macOS 沙盒里**必须**先解析安全作用域书签（解析即开始访问），
        // 否则重启后目录还在、权限没了 —— 现象是"共享目录列表变空"，且没有任何报错。
        let share_dir = crate::user_dirs::load(&conn, crate::user_dirs::SHARE);

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
            (Some(xs), Some(es)) => Identity::from_secrets(&xs, &es).unwrap_or_else(|| {
                // 损坏/截断的密钥不能只在内存中临时修复；否则每次重启都会
                // 生成另一套身份，导致好友公钥与历史 E2EE 消息永久失配。
                let id = Identity::generate();
                db::set_setting(&conn, "x25519_secret", &id.x25519_secret_b64()).ok();
                db::set_setting(&conn, "ed25519_secret", &id.ed25519_secret_b64()).ok();
                id
            }),
            _ => {
                let id = Identity::generate();
                db::set_setting(&conn, "x25519_secret", &id.x25519_secret_b64()).ok();
                db::set_setting(&conn, "ed25519_secret", &id.ed25519_secret_b64()).ok();
                id
            }
        };

        // 中继授权（P2 / M4）：启动时读一次，之后由 save_settings 更新内存缓存。
        // 缺失/脏值 → 默认 `All`（= 今天的行为，见 mesh/relay_policy.rs 顶部说明）。
        let relay_policy = crate::mesh::relay_policy::RelayConfig::parse(
            db::get_setting(&conn, "relay_policy").as_deref(),
            db::get_setting(&conn, "relay_allowlist").as_deref(),
        );

        Ok(Arc::new(AppState {
            app,
            db: Mutex::new(conn),
            device_id,
            tcp_port,
            downloads_dir: Mutex::new(downloads_dir),
            cache_dir,
            db_path,
            logger,
            identity,
            gossip: Mutex::new(GossipEngine::new(100_000, 10_000, 4, 6)),
            // max_ttl / fanout 与 GossipEngine 对齐（6 / 4），保证行为一致。
            // health_timeout 10s（约两个心跳周期）、max_failures 3
            // 健康超时 = 3 × 心跳周期（心跳在 `network::transport` 每 5s 一次）。
            // ⚠️ 不要退回 10_000：那**恰好等于 2 个心跳周期**，丢一拍就到边界、
            // 丢两拍即判不健康（判据是闭区间 `<=`），网络抖动会被误报成链路故障。
            // 上限受 `RELAY_PEER_TIMEOUT_SECS = 45s` 约束（跨跳节点无直连，
            // 只靠 10s 一轮的 Presence 保活），15s 留足余量且远小于 45s。
            // 不变量由 `mesh::manager::tests::health_timeout_outlives_three_heartbeats` 守住。
            peer_manager: Mutex::new(PeerManager::new(15_000, 3)),
            mesh_router: Mutex::new(MeshRouter::new(100_000, 10_000, 6, 4, 256)),
            relay: Mutex::new(RelayManager::new()),
            relay_policy: Mutex::new(relay_policy),
            #[cfg(feature = "bluetooth")]
            ble: Mutex::new(None),
            dialing: Mutex::new(std::collections::HashSet::new()),
            dial_permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DIALS)),
            inbound_permits: Arc::new(tokio::sync::Semaphore::new(MAX_INBOUND_CONNECTIONS)),
            group_keys: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            links: tokio::sync::Mutex::new(HashMap::new()),
            pending_requests: Mutex::new(HashMap::new()),
            network: Mutex::new(None),
            pending_reads: Mutex::new(pending_reads_map),
            pending_group_keys: Mutex::new(HashMap::new()),
            conv_link: Mutex::new(HashMap::new()),
            share_dir: Mutex::new(share_dir),
            nickname: Mutex::new(nickname),
            avatar: Mutex::new(avatar),
            pending_file_accept: Mutex::new(HashMap::new()),
            pending_file_complete: Mutex::new(HashMap::new()),
            relay_file_keys: Mutex::new(HashMap::new()),
            group_file_keys: Mutex::new(HashMap::new()),
            group_file_receivers: Mutex::new(HashMap::new()),
            group_file_sending: Mutex::new(std::collections::HashSet::new()),
            group_file_online_targets: Mutex::new(HashMap::new()),
            file_sending: Mutex::new(std::collections::HashSet::new()),
            file_receivers: Mutex::new(HashMap::new()),
            pending_share_tree: Mutex::new(HashMap::new()),
            peers_dirty: AtomicBool::new(false),
            network_generation: AtomicU64::new(0),
            peers_notify: Arc::new(Notify::new()),
            probe: Mutex::new(None),
            diag: Mutex::new(DiscoveryDiag::default()),
            diag_events: Mutex::new(VecDeque::with_capacity(50)),
            seen_hello_nonces: Mutex::new(VecDeque::new()),
            key_conflict_warned: Mutex::new(std::collections::HashSet::new()),
        }))
    }

    /// 当前网络世代号。
    pub fn network_generation(&self) -> u64 {
        self.network_generation.load(Ordering::Acquire)
    }

    /// 进入新世代（`start()` 成功后 / `stop()` 开始时调用）并返回新值。
    pub fn bump_network_generation(&self) -> u64 {
        self.network_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 读取中继授权配置（克隆一份：一次短锁 + 小结构体拷贝，热路径可接受）。
    ///
    /// ⚠️ 返回的是**快照**：调用方不要在持有它的时候再去读库（避免锁顺序纠缠）。
    pub fn relay_policy_config(&self) -> crate::mesh::relay_policy::RelayConfig {
        self.relay_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 更新中继授权内存缓存（`save_settings` 写入设置后调用）。
    pub fn set_relay_policy_config(&self, cfg: crate::mesh::relay_policy::RelayConfig) {
        *self.relay_policy.lock().unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    /// 当前界面语言是否为中文。语言偏好存 settings.language（三态 system / zh-CN /
    /// en-US，由前端维护）；「跟随系统」时用 `system_lang_is_zh()` 判系统语言。
    pub fn is_zh(&self) -> bool {
        let lang = {
            let dbc = self.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_setting(&dbc, "language").unwrap_or_else(|| "system".to_string())
        };
        match lang.as_str() {
            "zh-CN" => true,
            "en-US" => false,
            _ => system_lang_is_zh(),
        }
    }

    /// 应用显示名：中文系统「相闻」、英文系统 "Gosslan"。
    ///
    /// 用于托盘提示、日志窗口标题、通知等后端自行生成的用户可见文案。
    // 移动端用不到后端生成的显示名（托盘/独立窗口标题都是桌面概念）⇒ 显式允许未使用
    #[cfg_attr(mobile, allow(dead_code))]
    pub fn display_name(&self) -> String {
        if self.is_zh() {
            "相闻".to_string()
        } else {
            "Gosslan".to_string()
        }
    }

    /// 记录一个 Hello nonce，返回 false 表示该 nonce 近期已出现过（重放）。
    ///
    /// 有界 FIFO（容量 `HELLO_NONCE_CACHE`）：握手是低频事件，线性查重成本可忽略；
    /// 不依赖墙上时钟，避免设备间时间偏差影响判定。
    pub fn accept_hello_nonce(&self, nonce: &str) -> bool {
        const HELLO_NONCE_CACHE: usize = 512;
        let mut q = self.seen_hello_nonces.lock().unwrap_or_else(|e| e.into_inner());
        if q.iter().any(|n| n == nonce) {
            return false;
        }
        q.push_back(nonce.to_string());
        while q.len() > HELLO_NONCE_CACHE {
            q.pop_front();
        }
        true
    }

    /// 该 peer 是否至少有一条可用连接。
    ///
    /// **不要用 `links.contains_key` 代替**：Phase 6 起 `links` 的值是 `Vec`
    /// （一个 peer 可有多条连接），条目可能存在但 Vec 已空——此时 `contains_key`
    /// 仍返回 true，会误判为「已连接」。
    pub async fn has_link(&self, peer_id: &str) -> bool {
        self.links
            .lock()
            .await
            .get(peer_id)
            .is_some_and(|v| !v.is_empty())
    }

    /// 该 peer 是否已连到**指定端点**。
    ///
    /// 与 `has_link` 的区别：6b 起一个 peer 可有多条连接（LAN + Tailscale），
    /// 判断「要不要再拨号」必须**按端点**，而不是按 peer —— 否则永远只能建一条。
    pub async fn has_endpoint(&self, peer_id: &str, endpoint: &crate::mesh::Endpoint) -> bool {
        self.links
            .lock()
            .await
            .get(peer_id)
            .is_some_and(|v| v.iter().any(|l| l.endpoint == *endpoint))
    }

    /// 是否有**任意** peer 已连到指定端点（不看身份）。
    ///
    /// 用于「还不知道对端 device_id」的拨号去重：Routed 端点可以只填地址
    /// （身份由握手学来），此时拨号任务的 10s 重试无法按 peer 判断「是否已连上」，
    /// 只能按端点 —— 否则每轮都会重复建链。
    ///
    /// 方向性说明：主动方记录的 endpoint 是**对端的监听地址**（与配置一致）；
    /// 被动方记录的是对端拨入时的**临时源端口**，不会与配置地址相同，故不会误判。
    pub async fn has_endpoint_addr(&self, endpoint: &crate::mesh::Endpoint) -> bool {
        self.links
            .lock()
            .await
            .values()
            .flatten()
            .any(|l| l.endpoint == *endpoint)
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
        let peers: Vec<Peer> = self.peers.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        let _ = self.app.emit("peers-updated", peers);
    }

    /// 推送一条诊断事件到 ring buffer（最多保留 50 条，淘汰最旧）。
    pub fn push_diag_event(&self, kind: &str, detail: &str) {
        let ev = DiscoveryEvent {
            ts: now_ms(),
            kind: kind.to_string(),
            detail: detail.to_string(),
        };
        let mut buf = self.diag_events.lock().unwrap_or_else(|e| e.into_inner());
        if buf.len() >= 50 {
            buf.pop_front();
        }
        buf.push_back(ev);
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
