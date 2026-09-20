//! 应用全局状态与前端交互类型。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, watch, Notify};

use crate::crypto::Identity;
use crate::db;
use crate::device::{hardware_fingerprint, hostname_fingerprint};
use crate::file_relay::RelayManager;
use crate::gossip_engine::GossipEngine;
use crate::logging::Logger;
use crate::mesh::manager::PeerManager;
use crate::mesh::path::PathKind;
use crate::mesh::router::MeshRouter;
use crate::protocol::{Message, TCP_PORT};

/// 系统语言是否为中文 —— **仅**「前端还没把解析结果推过来」时的兜底。
///
/// 后端不引入系统 locale 库，用 POSIX 环境变量 `LANG` / `LC_ALL` / `LC_MESSAGES`
/// 兜底：macOS/Linux 的 `LANG` 通常是 `zh_CN.UTF-8` / `en_US.UTF-8`，能正确判断。
///
/// **⚠️ Windows 上这里恒为「否」**（Windows 没有 `LANG` 这类变量；macOS 从 Finder 启动的
/// GUI 进程通常也没有）。所以它只能兜住"进程刚起来、前端还没推语言"的那一小段 ——
/// 正常路径以 [`AppState::is_zh`] 的优先级为准（前端推来的解析结果优先）。
fn system_lang_is_zh() -> bool {
    ["LANG", "LC_ALL", "LC_MESSAGES"].iter().any(|k| {
        std::env::var(k)
            .map(|v| v.to_lowercase().contains("zh"))
            .unwrap_or(false)
    })
}

/// 「跟随系统」时到底是不是中文 —— 纯函数，便于单测。
///
/// 优先级：**显式偏好 > 前端推来的解析结果 > 环境变量兜底**。
///
/// 为什么中间那一层必不可少（用户 2026-09-16 实测「加群的提示怎么是英文？」）：
/// 「跟随系统」的解析规则（`navigator.language`）**只在前端有一份**，而后端的兜底在
/// Windows 上恒为「否」—— 于是中文用户在**默认设置**下，后端生成的所有文案
/// （群成员变更 / 文件下载 / 托盘提示 / 窗口标题）全变英文，而界面本身是中文。
/// 前端启动时与每次切换语言都会把结果推过来（`set_ui_language` 命令），这里只负责取舍。
fn resolve_is_zh(preference: Option<&str>, ui_hint: Option<bool>, system: bool) -> bool {
    match preference {
        Some("zh-CN") => true,
        Some("en-US") => false,
        // 其它值（含 "system" 与历史脏值）一律按「跟随系统」处理
        _ => ui_hint.unwrap_or(system),
    }
}

/// 前端推来的「界面实际语言」三态（0 = 还没推过）。
const UI_LANG_UNKNOWN: u8 = 0;
const UI_LANG_ZH: u8 = 1;
const UI_LANG_EN: u8 = 2;

/// **运行状态的唯一快照**（用户要求的第 ② 项）。
///
/// 为什么必须合并：以前"局域网到底开着没有"在前端有**两份**表示 ——
/// `channels[lan].enabled`（来自 `get_channel_status`）与 `online`（来自 `get_network_status`），
/// 由两个命令 + 两个事件各自维护 ⇒ 必然出现"外面开了、里面还是关的"（用户实测过）。
/// 现在只有**一条路径**：`get_runtime_snapshot` 命令与 `runtime-changed` 事件（**带载荷**），
/// 前端只认这一份。
///
/// 注意 `peers`：完整节点列表仍走 `peers-updated`（它最多 3/s、只在脏时推，见 `emit_peers`），
/// 放进快照会让每次通道开关都搬一遍全表；这里只带**节点数**（空态文案要用）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    /// 各通道状态（lan / bluetooth）：enabled / available / running / peers
    pub channels: Vec<crate::transport::ChannelStatus>,
    /// 局域网运行时是否在跑
    pub online: bool,
    /// 局域网绑定的本机地址（未运行时为 None）
    pub bound_ip: Option<String>,
    /// 蓝牙里"通道状态装不下"的那部分事实
    pub ble: BleRuntimeFacts,
    /// 当前在线节点数（不是完整列表）
    pub peer_count: usize,
    /// **我自己的在线状态**（用户 2026-09-12 定的规则）：
    /// 只要**任一通道在跑**（局域网 或 蓝牙）就算在线；**两个都关了才是离线**。
    ///
    /// 为什么单独给一个字段、而不是继续用 `online`（= 局域网在跑）：
    /// 手机端蓝牙是自动开启的（用户规则「有蓝牙就默认开」），此时即使没连 Wi-Fi，
    /// 用户也应该显示"在线"——用 `online` 会在这种场景下把用户标成离线。
    /// `online` 仍然保留（它是"局域网在跑"，界面里"局域网：N 个节点"那类 LAN 专属文案要用）。
    pub present: bool,
}

/// 蓝牙运行时事实（`ChannelStatus` 表达不了的部分）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleRuntimeFacts {
    /// 本次构建是否编译了 `bluetooth` feature（没编译时界面该说"此版本不含蓝牙"，
    /// 而不是含糊的"已关闭"）
    pub feature_compiled: bool,
}

/// 「设置已变更」事件名：设置窗口与主窗口靠它同步（见 `notify_settings_changed`）。
pub const EVENT_SETTINGS_CHANGED: &str = "settings-changed";
/// 「数据被清空」事件名（清除聊天数据 / 清缓存后的破坏性操作）。
///
/// 用户实测（Mac 4.1.10）：在设置里清了缓存、目录和聊天记录，**主界面毫无反应** ——
/// 因为"清除"只发生在设置窗口自己的 store 里（`ResetSection` 调的是那个窗口的
/// `chat.clearAllData()` + `refreshFriends()`），主窗口是**另一个 WebView**，
/// 它手里的会话列表/消息一条都没变。破坏性操作必须广播，否则用户会以为没清掉。
pub const EVENT_DATA_CLEARED: &str = "data-cleared";
/// 运行状态（通道/在线/绑定 IP）发生变化 —— 让**所有**窗口与页面立刻刷新同一份状态。
/// 用户实测「外面把局域网打开、里面还是关的」就是缺这条推送：两处 UI 各自持一份快照，
/// 谁都不知道对方改了。现在任何一次通道开关都会广播，前端统一重拉（唯一真相源在后端）。
pub const EVENT_RUNTIME_CHANGED: &str = "runtime-changed";

/// 「设置已变更」的载荷：**带补丁、且不回发给发起窗口**。
///
/// 旧实现是 `emit(EVENT_SETTINGS_CHANGED, ())` —— 无载荷、广播给所有人，于是每个窗口
/// （包括刚写完的那个）都要 `get_settings + get_device_info + get_share_dir` 全量重拉一遍。
/// 三个后果，用户都实测到了：
/// 1. **白拉**：改一次主题，两个窗口都重拉三份数据；
/// 2. **回灌**：发起窗口读到的是**写入前**的旧快照（去抖写入期间尤其明显）——
///    这正是"点了主题又跳回去"的根因，为此额外养了 `settingsDirty`/grace 一整套守卫；
/// 3. **事件乒乓**：重拉会走到 `pushUiLanguage()`，它又调 `set_ui_language()`，
///    后者再发一次 `settings-changed` ⇒ 两个窗口互相触发，形成高频 IPC 环。
///
/// 现在：`changed` 说明"哪些键变了"、`settings` 只带**变了的那些键的值**（接收方零 IPC 应用）、
/// `origin` 说明"谁改的"，而**发起窗口根本收不到**这个事件 ⇒ 上面三条一起消失。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    /// 变了的键（与 `Settings` 的 camelCase 序列化名一致）。
    /// 特殊值 `"*"` 表示"全量都变了"（恢复默认）⇒ 接收方做一次完整重拉。
    pub changed: Vec<String>,
    /// 发起窗口的标签（诊断用；发起窗口本身不会收到本事件）。
    pub origin: Option<String>,
    /// 只含 `changed` 里那些键的一小块快照，直接可被前端 `applySettingsSnapshot` 应用。
    pub settings: serde_json::Value,
}

/// 取出事件目标（监听方）的窗口标签。
///
/// 前端每个窗口的 `listen()` 在 Tauri 里注册为 `EventTarget::AnyLabel { label }`
/// （见 tauri 的 `filter_target`），其余变体是 Rust 侧监听时用的 —— 四种都取标签，
/// 才能保证"发起窗口收不到自己的事件"这件事对两种监听方式都成立。
fn event_target_label(target: &tauri::EventTarget) -> Option<&str> {
    match target {
        tauri::EventTarget::AnyLabel { label }
        | tauri::EventTarget::Window { label }
        | tauri::EventTarget::Webview { label }
        | tauri::EventTarget::WebviewWindow { label } => Some(label.as_str()),
        _ => None,
    }
}

/// 对端当前"**实际会走**"的链路类型（与 `pick_link` 的优先级一致：LAN > Routed > Bluetooth）。
///
/// 为什么要它：界面上的「蓝牙直连」以前是**反推**出来的（`p.ip || 蓝牙直连`），
/// 于是同一 Tailscale 网段（`Routed`）的设备也会被标成"蓝牙直连"（用户 2026-09-12 实测）。
/// 链路类型只有后端知道（`Link::path_kind` 由**来路**决定，不能从 IP 段反推），所以在这里判。
pub fn best_link_kind(kinds: &[crate::mesh::PathKind]) -> Option<crate::mesh::PathKind> {
    use crate::mesh::PathKind::*;
    [Lan, Routed, Bluetooth]
        .into_iter()
        .find(|&want| kinds.contains(&want))
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
    /// 这对公钥是否**经过签名验证**（Hello 验签通过，或已验签的 Gossip 信封携带）。
    ///
    /// `false` 表示它只来自**未签名**的 UDP announce 广播 —— 那是一条任何人都能伪造的
    /// 信道（`UdpPacket` 里 device_id 与公钥都是明文，没有签名字段）。
    /// 因此未验证的公钥**只能用于发现与拨号**，绝不允许：
    ///   · 作为 `verify_hello` 的身份绑定（否则攻击者抢先广播即可让真实好友的 Hello 被拒）；
    ///   · 写入持久化的 `friends` 表（否则一次广播就能永久改掉好友的真实公钥，
    ///     我发给该好友的消息会改用攻击者公钥加密，E2EE 被击穿且重启不恢复）。
    #[serde(default)]
    pub keys_verified: bool,
    /// 首次发现该节点的时间戳（announce / Presence 首次学到）。用于「小 ID 兜底拨号」
    /// 判断「对端在线却迟迟连不上」（单向可达）——语义是**发现时间**，不是建链时间。
    #[serde(default)]
    pub first_seen: Option<i64>,
    /// 当前与它的**实际链路类型**：`"lan"` / `"routed"` / `"bluetooth"`（无链路则 None）。
    ///
    /// 由命令层（`get_peers` / `search_nearby_peers`）在读取时从 `links` 填上 —— 事件推送的
    /// peer 表不带它（那里是同步上下文，拿不到异步的 links 锁），界面以"字段缺失"为准不猜。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// 对端在 Hello 里声明的版本（`device_id -> 声明`）。
///
/// 只记录**声明值**，不做任何推断：老端不发这两个字段 ⇒ 两项都是 `None`，
/// 面板上如实显示"未声明"（而不是替它猜一个版本）。
///
/// 为什么单独一张表而不是挂到 `Peer` 上：`Peer` 会被 UDP announce 反复重建，
/// 而 announce 不带版本 —— 挂上去就会在每次广播后丢掉真值。
/// 这张表只在**验签通过的 Hello** 里写入，回收点与 `peer_content_features` 同一个。
#[derive(Clone, Debug, Default, Serialize)]
pub struct PeerVersion {
    /// 对端线格式版本（`None` = 老端未声明 ⇒ 按最低版本处理）
    pub protocol_version: Option<u32>,
    /// 对端应用版本串，**只给人看**（不参与兼容判断）
    pub app_version: Option<String>,
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
    /// 三级发送通道：High（控制帧，必须立即）、Normal（聊天/FileOffer）、Low（bulk 文件分片/大头像）。
    ///
    /// 替代旧的 bulk + priority 两级。BLE writer_loop 在 Low bulk 写入过程中会周期性
    /// yield 检查 High/Normal，避免 4KB FileChunk（273 片 MTU × 15ms = 4s）期间
    /// 控制帧 / 聊天消息排队等待。
    pub high: mpsc::Sender<Message>,
    pub normal: mpsc::Sender<Message>,
    pub low: mpsc::Sender<Message>,
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

/// 一条收藏（与前端一致）。
///
/// `content` 是**收藏当时的快照**：图片/文件类收藏的 `content.path` 已被改写成收藏副本路径
/// （而不是原消息里的下载目录路径），所以原消息被清理、会话被删之后这条记录仍然能打开。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Favorite {
    pub id: String,
    pub msg_id: String,
    pub conv_id: String,
    pub sender_id: String,
    pub kind: String, // text | code | image | file
    pub content: String,
    /// 原消息时间（列表里显示"这条内容是什么时候的"）
    pub ts: i64,
    /// 收藏时间（列表排序键）
    pub favorited_at: i64,
    /// 收藏副本的绝对路径（仅 image/file，其余为 None）
    pub media_path: Option<String>,
    pub media_size: i64,
    /// 副本是否还在磁盘上。**由命令层填充**（db 层不碰文件系统）：列表里给前端渲染
    /// 「已清理」占位用，避免用户点开才发现打不开。
    #[serde(default)]
    pub available: bool,
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
    /// 本机置顶（纯本地偏好，不广播不同步）。列表排序时优先于 last_ts。
    #[serde(default)]
    pub pinned: bool,
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

/// 链路候选（含评分），供开发者诊断面板展示 Discovery 实际看到的候选列表。
///
/// 用户 2026-09-13 要求「网卡-候选 也可以加上蓝牙」：于是 `kind` 把两类候选
/// 统一进一张表 —— `lan`（真实网卡）与 `bluetooth`（BLE 适配器这一条链路）。
/// 蓝牙行没有 IP/广播（蓝牙链路上没有这些概念），所以 `ip` / `broadcast` 为空，
/// 人类可读的状态放进 `detail`。
#[derive(Serialize, Clone, Debug)]
pub struct InterfaceCandidate {
    /// `lan` | `bluetooth`
    pub kind: String,
    pub name: String,
    pub ip: String,
    pub has_broadcast: bool,
    pub broadcast: Option<String>,
    pub is_rfc1918: bool,
    pub is_virtual: bool,
    pub score: i32,
    pub selected: bool,
    /// 一句话状态（蓝牙行用：「运行中 · 前台节奏 · 1 个对端」；网卡行为空）
    pub detail: String,
}

/// 蓝牙候选的失败退避条目（诊断面板展示"为什么这一轮没拨它"）。
#[derive(Serialize, Clone, Debug)]
pub struct BleBackoff {
    /// BLE 外设标识（**不是身份** —— 身份由 Hello 验签建立）
    pub id: String,
    /// 连续失败次数
    pub failures: u32,
    /// 距下次允许尝试还有多少毫秒（≤0 表示已到期）
    pub remaining_ms: i64,
}

/// 蓝牙运行时的可观测事实（诊断面板用）。
///
/// 为什么要单独一组：原来的诊断数据**全是局域网的**，纯蓝牙用户看到的是
/// `mode = offline`（用户 2026-09-13 反馈）。蓝牙是独立通道，必须有自己的状态。
#[derive(Serialize, Clone, Debug, Default)]
pub struct BleDiag {
    /// 本次构建是否编译了蓝牙特性（没编译时其余字段无意义）
    pub feature_compiled: bool,
    /// 用户开关状态（= 后端真实运行状态，见 `channels[bluetooth].enabled`）
    pub enabled: bool,
    /// 适配器是否可用（没有适配器 / 没有权限 = false）
    pub available: bool,
    pub running: bool,
    /// 已建立 BLE 链路的对端数
    pub peers: usize,
    /// 当前扫描节奏来源：`active`（前台/聚焦）| `idle`（后台/失焦）
    pub activity: String,
    pub scan_window_ms: u64,
    pub scan_interval_ms: u64,
    /// 最近一次扫描完成时间（ms since epoch，0=从未）
    pub last_scan_ts: i64,
    /// 最近一次扫描收到的广播总数
    pub last_scan_total: u32,
    /// 其中属于本应用服务的个数
    pub last_scan_matched: u32,
    /// 正在退避中的候选（按剩余时间倒序）
    pub backoff: Vec<BleBackoff>,
    /// 「不要再拨」名单大小（对端是指定拨号方时登记）
    pub no_dial: usize,
}

/// BLE 最近一轮扫描的统计（不直接序列化，由 `BleDiag` 组装给前端）。
#[cfg(feature = "bluetooth")]
#[derive(Clone, Copy, Debug, Default)]
pub struct BleScanStats {
    /// 该轮扫描完成时间（ms since epoch）
    pub last_ts: i64,
    /// 收到多少个广播
    pub total: u32,
    /// 其中多少个带本应用的服务 UUID
    pub matched: u32,
}

/// Discovery 运行时诊断状态（只读，供开发者面板展示）。
///
/// ⚠️ 用户 2026-09-13：「最近事件」已**合并进运行日志**（见 `AppState::push_diag_event`），
/// 不再在这里保留 ring buffer —— 事件该和别的日志在一起被搜索/复制，而不是孤零零一个面板。
#[derive(Serialize, Clone, Debug, Default)]
pub struct DiscoveryDiag {
    /// 当前网络模式：auto / manual / offline（**仅指局域网**；蓝牙状态看 `bluetooth`）
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
    /// 候选链路列表（网卡 + 蓝牙，含评分）
    pub candidates: Vec<InterfaceCandidate>,
    /// 蓝牙通道事实（独立于局域网，纯蓝牙用户也看得见自己的状态）
    pub bluetooth: BleDiag,
    /// 本机线格式版本（与 `peer_versions` 同一屏对齐，跨版本排查先看这一行）
    pub protocol_version: u32,
    /// 本机应用版本（只给人看，不参与任何兼容判断）
    pub app_version: String,
    /// 各对端在 Hello 里声明的版本（按 device_id 排序）
    pub peer_versions: Vec<PeerVersionDiag>,
}

/// 诊断面板的一行「对端声明了什么版本」。
///
/// 昵称在这里补而不是存进 `peer_versions`：那张表只记 Hello 那一次的声明，
/// 而昵称会随 UserInfo 变 —— 面板要说的是"现在这是谁"。
#[derive(Serialize, Clone, Debug)]
pub struct PeerVersionDiag {
    pub device_id: String,
    pub nickname: String,
    /// `None` = 对端是没报版本的老版本（面板显示"未声明"，不当成版本 0 也不当成版本 1）
    pub protocol_version: Option<u32>,
    pub app_version: Option<String>,
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

/// 链路停滞状态变化（只在**状态翻转**时发一次，不是每 tick 都发）。
///
/// 为什么需要它：`send_on_link` 在对端不收时会一直挂在背压上 —— 既不再发进度也不报错，
/// 界面就冻在同一个百分比最长到 deadline（1h）。用户需要知道"卡在网络上"而不是"软件死了"。
/// `idle_ms` = 距最近一次真正写出分片的毫秒数（以 writer 实发为准，不是投进队列）。
#[derive(Serialize, Clone, Debug)]
pub struct FileStalledInfo {
    pub transfer_id: String,
    pub stalled: bool,
    pub idle_ms: i64,
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
    /// 归属场景：`chat` = 普通群文件；`todo` = 待办描述图片（不进时间线）
    pub scope: String,
    /// `scope == "todo"` 时关联的 todo_id
    pub todo_id: String,
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
    /// 收到 RelayFileOffer 的时刻。**必须有**：本表按 transfer_id 索引，
    /// 而对端可以一直发新 offer 却永不发分片 —— 没有时间戳就无法回收，
    /// 内存会随对端行为单调增长（见 `sweep_stale_relay`）。
    pub created_at: i64,
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
    /// 前端推来的「界面实际用的是哪种语言」（三态，见 [`Self::is_zh`] 与 `set_ui_language`）。
    ///
    /// 只存内存、**不落库**：它是前端解析结果的缓存，每次启动前端都会重新推一次；
    /// 落库反而会多出一份可能与 `settings.language`（那是**偏好**，不是结果）不一致的状态。
    ui_lang: AtomicU8,
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
    /// **不要再主动拨的 BLE 外设标识**（central 侧）。
    ///
    /// 为什么需要（用户 2026-09-12 实测的"点加好友：发送失败，连接已关闭"）：
    /// 两端都同时跑 central + peripheral，于是**互相拨号**，形成两条镜像 BLE 链路：
    ///   · 手机（大 id）拨 Mac（小 id）→ 成功建链 A；
    ///   · Mac（小 id）也在扫手机 → 每轮扫描都去拨一次 → 被手机按"镜像链路"规则拒掉
    ///     （不回 Hello）——**拒是拒了，但每次连接本身都会打断手机那条 GATT server 连接**，
    ///     于是链路 A 45s 收不到一帧、被看门狗拆掉、再重来。
    /// 规则与 TCP 的 `should_dial` 一致：**大 id 是这台链路上的指定拨号方，小 id 只接受**。
    /// 小 id 在握手验签后就知道"对端比我大 ⇒ 该它拨我"，把该外设标识记进这里，
    /// 之后扫描直接跳过它 —— 不再去打扰那条好的链路。
    ///
    /// 清空时机：蓝牙通道停止时（下次开启重新学一遍）。
    #[cfg(feature = "bluetooth")]
    pub ble_no_dial: Mutex<std::collections::HashSet<String>>,
    /// **确认在广播的 BLE 外设标识**（central 侧，每次开启蓝牙时清空后重新学）。
    ///
    /// 为什么需要（ADR-0015 §7-f，Windows Phase 1）：BLE 上"谁拨号"原本是
    /// `should_dial_ble(my_id, peer_id) = my_id > peer_id`，而这条规则**只在两端都能广播
    /// 时才成立**（它的前提是"我不拨，对方也会拨我"）。Windows 这一轮只做 central
    /// （不能广播、不能被连），照搬该规则就会在"Windows 的 id 更小"时**两侧都不拨**，
    /// 链路永远建不起来。
    ///
    /// 于是判据补一条：**对端不广播 ⇒ 必须由我们拨**。这个集合就是"对端确实在广播"的
    /// 事实来源 —— 只有真的在扫描结果里见过它的外围广播才登记，不是靠平台常量猜。
    ///
    /// 只记内存、不落库：广播是每次开机重新发生的事，持久化只会带来陈旧数据。
    #[cfg(feature = "bluetooth")]
    pub ble_peer_advertises: Mutex<std::collections::HashSet<String>>,
    /// **立刻扫一轮 BLE** 的触发通道（与 LAN 的 `probe` 同一个范式）。
    ///
    /// 为什么需要（用户 2026-09-13 要求「扫描快一点」）：BLE 的扫描循环是
    /// 「扫 3s → 等 10s」的周期任务，用户点开「添加好友」时**最多要等一整个周期**
    /// 才可能看到对端。LAN 那条路早就有按需探测（`search_nearby_peers` + `who_has`），
    /// BLE 一直缺 —— 用户侧的体感就是"蓝牙搜不到/很慢"。
    ///
    /// 语义：发一个新值 ⇒ 扫描循环立刻结束当前的等待、马上开扫一轮（不重置退避，见
    /// `scan_loop` 里对 `user_triggered` 的处理）。
    #[cfg(feature = "bluetooth")]
    pub ble_scan_now: Mutex<Option<tokio::sync::watch::Sender<u64>>>,
    /// BLE 候选的**失败退避**：外设标识 → (连续失败次数, 下次允许尝试的时间)。
    ///
    /// 为什么需要：BLE 上"连过去被拒"是常态，而**每次连接尝试都会打扰对端**
    /// （Android 的 GATT server 对同一 central 的新连接会替换旧连接）。
    /// 不冷却就会变成"每轮扫描都去打扰一次"的抖动 —— 用户真机日志里正是 13s 一轮、
    /// 把手机拨过来的那条好链路反复打断。
    #[cfg(feature = "bluetooth")]
    pub ble_dial_failures: Mutex<std::collections::HashMap<String, (u32, i64)>>,
    /// 应用是否处于「前台且窗口聚焦」（用户 2026-09-13 提的功耗策略）。
    ///
    /// 由前端在 `visibilitychange` / `focus` / `blur` 时调 `set_app_active` 更新，
    /// 蓝牙扫描循环据此选快/慢节奏（见 `network::ble::SCAN_INTERVAL_*`）：
    /// 前台（用户在用/PC 窗口聚焦）= 提高刷新率；后台/失焦 = 降频；进程退出/被杀 = 天然停止。
    pub app_active: AtomicBool,
    /// 唤醒 BLE 扫描循环**立刻扫一轮**：从后台切回前台、或用户打开「添加好友」时，
    /// 不必再等一个（后台节奏下最长 30s 的）慢周期。
    #[cfg(feature = "bluetooth")]
    pub ble_wake: Arc<Notify>,
    /// BLE 最近一轮扫描统计（诊断面板用）。
    #[cfg(feature = "bluetooth")]
    pub ble_scan: Mutex<BleScanStats>,
    /// 中继授权配置（P2 / M4）：策略 + 白名单。
    ///
    /// 为什么缓存在内存：转发热路径上 gossip 可能每秒几十条，为了一个策略字段去锁
    /// SQLite 是纯浪费。启动时从 settings 读入，`save_settings` 时更新。
    pub relay_policy: Mutex<crate::mesh::relay_policy::RelayConfig>,
    /// 文件接收落盘目录（可变：设置页可改，改后新接收的文件落到新目录）。
    pub downloads_dir: Mutex<PathBuf>,
    /// 缓存目录：图片 / 音频 / 文件等二进制落盘于此（SQLite 不存 BLOB）
    pub cache_dir: PathBuf,
    /// 收藏媒体的**独立副本**目录（`app_data/favorites/media`）。
    ///
    /// 刻意与 `cache_dir` / `downloads_dir` 分开：那两个目录会被「存储清理」按配额与保留期
    /// 删除（见 `storage/cache_cleaner.rs`），而收藏是"用户明确要留住的东西"
    /// —— 微信的收藏也是独立存储，删聊天记录、清缓存都不该把它弄丢。
    /// 所以它**不在** `media_dirs` 里，清理逻辑不会碰它；只有「清除数据」会显式清空。
    pub favorites_dir: PathBuf,
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

    /// **已发出、还没被同意/拒绝的好友申请**（peer_id 集合）。
    ///
    /// 为什么必须登记（用户 2026-09-12 真机）：「好友已发送，等待对方确认」，但对方
    /// **什么都没收到** —— 好友申请是一条**没有回执**的定向帧，链路正好在那一刻抖动
    /// （BLE 镜像互拨把链路打断）时它就**静默丢了**，而发送方界面依然显示"已发送"。
    /// 现在：发出即登记；对端建链/Hello 补全时重发一次（`flush_pending_friend_request`）；
    /// 收到同意（`FriendAccept`）或拒绝（`FriendReject`）后清除。
    pub pending_out_requests: Mutex<std::collections::HashSet<String>>,

    /// **待补发的好友同意回执**：peer_id -> (首次登记时刻 ms, 已补发次数, 上次补发时刻 ms)。
    ///
    /// 与 `pending_out_requests` 对称、但**必须分开**：申请是"等对方动作"（收到同意/拒绝才清），
    /// 而同意回执**没有回执**（发送方无从得知对方是否收到），只能"窗口内补发若干次"。
    ///
    /// 为什么必须有（用户 2026-09-13 真机）：Android 点了「接受」，Android 侧好友列表已出现，
    /// 但 **Mac 端状态一直没同步** —— 因为 `accept_friend_request` 只发一次
    /// （直连 `try_send` 或广播兜底），而 BLE 链路正好在那一刻抖动/还没建好时，
    /// 这一帧**静默丢失**且永不重发 ⇒ 单边好友关系（我这儿有他、他那儿没我）。
    pub pending_out_accepts: Mutex<std::collections::HashMap<String, (i64, u32, i64)>>,

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
    pub pending_file_accept: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Result<(), u64>>>>,
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
    /// 文件发送取消信号注册表：transfer_id -> oneshot Sender。
    /// 用户点"取消发送"时我们 send(())，send_file_from_path_deadline_inner 的 chunk loop
    /// 里 select! 这个信号，cleanup + 返回 retryable 让 outbox 走超限失败。
    /// （取消不会直接 mark failed —— 让它走正常 outbox 路径，cancel 只是"让这次尝试立刻返回"。）
    pub file_send_cancels: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    /// 群文件接收端 `.part` 状态：transfer_id -> 接收状态。
    /// 与一对一 `file_receivers` 生命周期独立；复用 FileReceiver 结构
    /// （file_key/next_seq/hasher 语义相同），不写 file_transfers 表。
    pub group_file_receivers: Mutex<HashMap<String, FileReceiver>>,
    /// 正在接收的文件：transfer_id -> FileReceiver
    pub file_receivers: Mutex<HashMap<String, FileReceiver>>,
    /// **文件发送的"真的写出去了"进展**：transfer_id -> 最近一次有分块离开链路的时刻（ms）。
    ///
    /// 用途：把 `FileCompleteAck` 的等待从"固定 30s 墙钟"改成"**安静** 30s 才算失败"
    /// （见 `network/file.rs::wait_complete_ack`）。判据必须落在**写出**而不是"入队"上 ——
    /// 队列能装 1024 帧，1MB 文件会在 1 秒内全部入队，而链路上要跑几分钟。
    pub file_wire_progress: Mutex<HashMap<String, i64>>,
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
    /// 各对端在 Hello 里声明的内容能力位图（device_id -> bits）。**不参与签名**，
    /// 仅用于"能不能对它发 ContentRequest"；旧端不声明 ⇒ 默认 0 ⇒ 不发新帧（向后兼容）。
    pub peer_content_features: Mutex<HashMap<String, u32>>,
    /// 各对端在 Hello 里声明的版本（device_id -> 声明）。**不参与签名**，当前只被诊断面板
    /// 与"忽略未知帧"的降级日志读（INV-P24：对端版本更高必须可解释）。
    pub peer_versions: Mutex<HashMap<String, PeerVersion>>,
    /// 按需探测触发：值递增 → 发现任务立即群发一次 `who_has`（好友搜索用）
    pub probe: Mutex<Option<watch::Sender<u64>>>,

    /// Discovery 诊断状态（隐藏开发者面板用，只读展示不改变网络行为）
    pub diag: Mutex<DiscoveryDiag>,
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
        // 收藏副本目录：独立于 cache/downloads，存储清理不碰（见字段注释）。
        let favorites_dir = app_data.join("favorites").join("media");
        std::fs::create_dir_all(&favorites_dir).ok();

        // 多开支持：`--instance N`（或环境变量 GOSSLAN_INSTANCE）→ 独立数据库 / 端口 / 设备指纹
        let instance = instance_id();
        let db_name = if instance > 0 {
            format!("gosslan-{instance}.db")
        } else {
            "gosslan.db".to_string()
        };
        let db_path = app_data.join(db_name);
        let conn = db::init(&db_path)?;

        // 🟦 崩溃恢复：把所有 sending 状态的 file_outbox 重置回 pending。
        // 进程可能在 mark sending 后、delete 前崩溃 —— 这些条目会永远卡在 sending，
        // list_pending_file_outbox 只捞 pending 的，它们就被彻底遗忘了。
        // 重置后下次 Hello 触发 flush_pending_files 会重新捞出来重试；
        // 重复传输由对端的幂等 FileOffer 处理。
        match crate::db::reset_sending_to_pending(&conn) {
            Ok(n) if n > 0 => {
                eprintln!("[gosslan] 崩溃恢复：{n} 条文件 outbox 从 sending 重置为 pending");
            }
            _ => {}
        }

        // 运行日志：多开实例用独立文件（gosslan-1.log），避免测试实例互相覆盖。
        let log_stem = if instance > 0 {
            format!("gosslan-{instance}")
        } else {
            "gosslan".to_string()
        };
        // 设备短指纹：平台 tag + hostname 前 6 字符。
        // 让多台设备的日志能一眼区分 — 用户贴多段日志时 AI 自动归类。
        let dev_fingerprint = {
            let plat = match std::env::consts::OS {
                "windows" => "Win",
                "android" => "And",
                "macos" => "Mac",
                "ios" => "iOS",
                "linux" => "Lin",
                _ => "Oth",
            };
            let host = hostname::get()
                .map(|h| h.to_string_lossy().chars().take(6).collect::<String>())
                .unwrap_or_else(|_| "unknown".to_string());
            format!("[dev:{plat}-{host}]")
        };
        let logger = Logger::new(app_data.join("logs"), &log_stem, dev_fingerprint);

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
            // ⚠️ **历史值迁移**（2026-09-13 合并评审补）：那时候的兜底路径多套了一层前缀，
            // 已装的安卓库里存的是 `dev-gosslan-…`。光改生成代码**治不了已经装上的设备** ——
            // 它们的 id 还是"三端里恒最小、永远不主动拨号"的那个值，
            // 除非用户清一次应用数据（丢聊天/好友）。所以这里把废弃前缀**就地剥掉**再写回。
            match crate::device::strip_legacy_dev_prefix(&id) {
                Some(migrated) => {
                    db::set_setting(&conn, "device_id", migrated).ok();
                    migrated.to_string()
                }
                None => id,
            }
        } else {
            // ⚠️ 这里**不能**再套一层前缀（真机 2026-09-13 第七轮发现）。
            //
            // 旧写法是 `format!("dev-{}", hostname_fingerprint())`，而
            // `hostname_fingerprint()` 本身已经产出 `gosslan-xxxxxxxxxxxxxxxx`，
            // 于是安卓的 device_id 变成 **`dev-gosslan-…`**，而桌面端（有机器码）是
            // **`gosslan-…`**。两个后果都是真的：
            //   ① **身份不一致**：同一个 `gosslan-` 命名约定被打断，日志/库/UI 里出现两种形状；
            //   ② **永远是"较小 id"**：`'d' < 'g'` ⇒ 安卓在三端里恒为最小
            //      ⇒ 按「大 id 拨、小 id 只接受」的镜像规则，**安卓永远不主动拨任何人**。
            //      它只能等别人来连它，而"别人能不能连到它"取决于对方的扫描 —— 这正是
            //      2026-09-13 那几轮"安卓搜不到 Windows"里最容易被忽略的一层。
            // `hostname_fingerprint()` 已带前缀，这里直接用。
            let id = hostname_fingerprint();
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

        // 默认昵称：**不用设备用户名/hostname**，改用「形容词 + 动物 + 设备短码」的英文名
        // （用户 2026-09-12 要求：长度合适、不改也好看、又有想改的欲望；规则见 `nickname.rs`）。
        // 同一台设备名字稳定（由 device_id 派生），而且**不含设备信息**。
        let hostname_now = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();
        let nickname = match db::get_setting(&conn, "nickname") {
            Some(n) => {
                // 一次性迁移：旧的默认名（空 / 旧字面值 / 恰好是 hostname）换成新规则。
                // 用户自己取过的名字一律不动 —— 判据是"精确等于旧默认的产物"。
                if crate::nickname::is_legacy_default(&n, &hostname_now) {
                    let fresh = crate::nickname::default_nickname(&device_id);
                    db::set_setting(&conn, "nickname", &fresh).ok();
                    fresh
                } else {
                    n
                }
            }
            None => {
                let fresh = crate::nickname::default_nickname(&device_id);
                db::set_setting(&conn, "nickname", &fresh).ok();
                fresh
            }
        };
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
            // 「前端还没推语言」的初值；前端 `app.init()` 随后就会推一次真实值。
            ui_lang: AtomicU8::new(UI_LANG_UNKNOWN),
            downloads_dir: Mutex::new(downloads_dir),
            cache_dir,
            favorites_dir,
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
            #[cfg(feature = "bluetooth")]
            ble_no_dial: Mutex::new(std::collections::HashSet::new()),
            #[cfg(feature = "bluetooth")]
            ble_peer_advertises: Mutex::new(std::collections::HashSet::new()),
            #[cfg(feature = "bluetooth")]
            ble_scan_now: Mutex::new(None),
            #[cfg(feature = "bluetooth")]
            ble_dial_failures: Mutex::new(std::collections::HashMap::new()),
            dialing: Mutex::new(std::collections::HashSet::new()),
            dial_permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DIALS)),
            inbound_permits: Arc::new(tokio::sync::Semaphore::new(MAX_INBOUND_CONNECTIONS)),
            pending_out_requests: Mutex::new(std::collections::HashSet::new()),
            pending_out_accepts: Mutex::new(std::collections::HashMap::new()),
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
            file_send_cancels: Mutex::new(HashMap::new()),
            file_receivers: Mutex::new(HashMap::new()),
            file_wire_progress: Mutex::new(HashMap::new()),
            pending_share_tree: Mutex::new(HashMap::new()),
            peers_dirty: AtomicBool::new(false),
            network_generation: AtomicU64::new(0),
            peers_notify: Arc::new(Notify::new()),
            peer_content_features: Mutex::new(HashMap::new()),
            peer_versions: Mutex::new(HashMap::new()),
            probe: Mutex::new(None),
            diag: Mutex::new(DiscoveryDiag::default()),
            app_active: AtomicBool::new(true),
            #[cfg(feature = "bluetooth")]
            ble_wake: Arc::new(Notify::new()),
            #[cfg(feature = "bluetooth")]
            ble_scan: Mutex::new(BleScanStats::default()),
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

    /// 当前界面语言是否为中文。偏好存 settings.language（三态 system / zh-CN /
    /// en-US，由前端维护）；「跟随系统」时**先看前端推来的解析结果**，最后才用
    /// 环境变量兜底（判定规则与理由见 [`resolve_is_zh`]）。
    pub fn is_zh(&self) -> bool {
        let pref = {
            let dbc = self.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_setting(&dbc, "language")
        };
        resolve_is_zh(pref.as_deref(), self.ui_lang_hint(), system_lang_is_zh())
    }

    /// 记录前端**解析后**的界面语言（`set_ui_language` 命令调用；见 [`Self::is_zh`]）。
    pub fn set_ui_language_hint(&self, lang: &str) {
        let v = if lang.starts_with("zh") {
            UI_LANG_ZH
        } else {
            UI_LANG_EN
        };
        self.ui_lang.store(v, Ordering::Relaxed);
    }

    /// 前端推来的解析结果；`None` = 还没推过（用环境变量兜底）。
    fn ui_lang_hint(&self) -> Option<bool> {
        match self.ui_lang.load(Ordering::Relaxed) {
            UI_LANG_ZH => Some(true),
            UI_LANG_EN => Some(false),
            _ => None,
        }
    }

    /// 应用显示名：中文系统「相闻」、英文系统 "Gosslan"。
    ///
    /// 用于独立窗口创建时的**初始标题**（文档标题就绪后由前端按语言接管，见
    /// `open_settings_window`）等后端自行生成的用户可见文案。
    // 移动端用不到后端生成的显示名（托盘/独立窗口标题都是桌面概念）⇒ 显式允许未使用
    #[cfg_attr(mobile, allow(dead_code))]
    pub fn display_name(&self) -> String {
        if self.is_zh() {
            "相闻".to_string()
        } else {
            "Gosslan".to_string()
        }
    }

    /// **本机自己的显示名**（「和自己聊天」里那个会话的名字，也用于把自己当发送者时的文案）。
    ///
    /// 为什么不能走 `resolve_nickname`：它只查好友表/在线节点表，自己两边都不在，
    /// 会回落到 `device_id` 原文 —— 会话列表里就会显示一串 `gosslan-xxxxxxxx`。
    pub fn self_display_name(&self) -> String {
        let n = self
            .nickname
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if !n.trim().is_empty() {
            return n;
        }
        // 昵称还没写进内存（极早期）时才用的兜底：与其它后端文案同口径（`is_zh`）。
        if self.is_zh() {
            "我".to_string()
        } else {
            "Me".to_string()
        }
    }

    /// 本机自己的头像（data URI，可能为空）。
    pub fn self_avatar(&self) -> Option<String> {
        self.avatar
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 记录一个 Hello nonce，返回 false 表示该 nonce 近期已出现过（重放）。
    ///
    /// 有界 FIFO（容量 `HELLO_NONCE_CACHE`）：握手是低频事件，线性查重成本可忽略；
    /// 不依赖墙上时钟，避免设备间时间偏差影响判定。
    pub fn accept_hello_nonce(&self, nonce: &str) -> bool {
        const HELLO_NONCE_CACHE: usize = 512;
        let mut q = self
            .seen_hello_nonces
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    /// 广播"设置已变更"给**除发起窗口外**的所有窗口（见 [`SettingsPatch`]）。
    ///
    /// `changed` 是"哪些键变了"（camelCase），`values` 是这些小键的**当前值**（由调用方在
    /// **持有 db 锁时**用 [`crate::commands::settings_patch_values`] 读好）—— 刻意不在这里
    /// 自己加锁：本函数被多个命令在"刚写完库"的位置调用，若内部再锁一次 `db`，
    /// 与那些仍持有锁的调用点会**直接死锁**（std Mutex 不可重入）。所以锁的边界留在调用方。
    pub fn notify_settings_changed(
        &self,
        changed: &[&str],
        origin: Option<&str>,
        values: serde_json::Value,
    ) {
        if changed.is_empty() {
            return;
        }
        let patch = SettingsPatch {
            changed: changed.iter().map(|k| (*k).to_string()).collect(),
            origin: origin.map(str::to_string),
            settings: values,
        };
        let origin = origin.map(str::to_string);
        let _ = self
            .app
            .emit_filter(EVENT_SETTINGS_CHANGED, patch, move |target| {
                // 发起窗口已经自己应用过了（而且它手里的值比库里更新）—— 绝不回发。
                event_target_label(target) != origin.as_deref()
            });
    }

    /// 广播"数据被清空了"：**除发起窗口外**的窗口要重建自己的列表（会话/消息/好友申请）。
    pub fn notify_data_cleared(&self, origin: Option<&str>) {
        let payload = serde_json::json!({ "origin": origin });
        let origin = origin.map(str::to_string);
        let _ = self
            .app
            .emit_filter(EVENT_DATA_CLEARED, payload, move |target| {
                event_target_label(target) != origin.as_deref()
            });
    }

    /// 广播"运行状态变了"——**带上完整快照**，且**不回发给发起窗口**（它在命令返回值里已经拿到了）。
    ///
    /// 与 ①（`settings-changed` 带补丁）同一个模式：快照由调用方用
    /// `commands::build_runtime_snapshot` 采好（那里同时要读 peers/network/transport 三种状态，
    /// 不适合塞进 `AppState` 自己），这里只负责"发给谁"。
    pub fn notify_runtime_changed(&self, snapshot: RuntimeSnapshot, origin: Option<&str>) {
        let origin = origin.map(str::to_string);
        let _ = self
            .app
            .emit_filter(EVENT_RUNTIME_CHANGED, snapshot, move |target| {
                event_target_label(target) != origin.as_deref()
            });
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
        let mut peers: Vec<Peer> = self
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        // 顺带补上"这个节点此刻有没有活跃链路 + 走哪条路"（link 字段）。
        //
        // 为什么必须带上：前端原来只按"在不在节点表"判在线，于是「链路活着、但广播没收到
        // （被防火墙/组播限制吞掉）或刚被 sweep」的好友会显示离线，而后端 get_friends 的
        // friend_is_online 却认为在线 ⇒ 用户看到"局域网都连上了，在线状态却不实时"。
        // try_lock：拿不到锁（网络任务正忙）就照旧发推送，不能让统计阻塞网络。
        if let Ok(links) = self.links.try_lock() {
            for p in peers.iter_mut() {
                if let Some(list) = links.get(&p.device_id) {
                    if list.is_empty() {
                        continue;
                    }
                    let kinds: Vec<crate::mesh::PathKind> =
                        list.iter().map(|l| l.path_kind).collect();
                    p.link = best_link_kind(&kinds).map(|k| k.as_str().to_string());
                }
            }
        }
        let _ = self.app.emit("peers-updated", peers);
    }

    /// 记录一条网络诊断事件 —— **直接进运行日志**（用户 2026-09-13 要求）。
    ///
    /// ## 为什么不再用独立 ring buffer
    ///
    /// 原来这里只把事件塞进一个"最近 50 条"的内存环形缓冲，只有隐藏的开发者面板能看，
    /// 用户能贴出来的「运行日志」里一个字都没有 —— 让这两处数据各活各的，等于白记。
    /// 现在统一进 `Logger`：可搜索、可复制、可落盘。
    ///
    /// ## 哪些留、哪些丢（不是无脑全打）
    ///
    /// 日志规范（见 `logging.rs` 顶部）明确要求**不记高频循环**。这里的调用点里，
    /// `announce_recv` 每收一个节点广播一条（秒级）、`broadcast_sent` / `multicast_sent`
    /// 每 10s 一条 —— 全打进去会在几分钟内把 500 条内存 buffer 冲干净，真问题反而被淹没。
    /// 所以只把**有排查价值的异常/状态跃迁**落日志，纯心跳成功事件直接丢弃：
    ///
    /// | kind | 去留 |
    /// |---|---|
    /// | `discovery_started` | 留（info，一次性状态跃迁） |
    /// | `broadcast_error` 等 `*_error` | 留（warn） |
    /// | `hello_rejected` / `hello_mismatch` / `identity_key_conflict` | **丢** —— 见下 |
    /// | `announce_recv` / `*_sent` | **丢**（高频心跳，没有排查价值） |
    ///
    /// 后一类「丢」不是漏了：它们的**每一个调用点旁边都已经有一条更完整的 `logger.warn`**
    /// （`transport.rs` 的「拒绝未通过身份认证的 Hello: …」「握手身份不符：…」「握手验签失败：…」，
    /// 以及 `warn_key_conflict_once`）。在这里再打一遍只会让日志出现成对的近似重复行 ——
    /// 用户的诉求是「别单开一块，进日志里」，而这些本来就在日志里。
    pub fn push_diag_event(&self, kind: &str, detail: &str) {
        const DROPPED: &[&str] = &[
            // 纯心跳（高频，且没有排查价值）
            "announce_recv",
            "broadcast_sent",
            "multicast_sent",
            "who_has_sent",
            // 已在各自的调用点旁边用更完整的上下文记过（重复打只会刷屏）
            "hello_rejected",
            "hello_mismatch",
            "identity_key_conflict",
        ];
        // 子网广播的**成功**必须留痕：真机排查「局域网只通一半」时，
        // 「这条到底发出去没有」是区分"发不出去"与"发出去了但对方没收到"的唯一证据。
        // 其余 `*_sent` 仍按高频丢弃（见 DROPPED）。
        if kind != "bc_directed_sent" && DROPPED.contains(&kind) {
            return;
        }
        let message = format!("diag/{kind}: {detail}");
        if kind.ends_with("_error") {
            self.logger.warn("discovery", message);
        } else {
            self.logger.info("discovery", message);
        }
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

#[cfg(test)]
mod tests {
    use super::{resolve_is_zh, UI_LANG_EN, UI_LANG_UNKNOWN, UI_LANG_ZH};

    /// 界面语言判定的优先级：**显式偏好 > 前端推来的解析结果 > 环境变量兜底**。
    ///
    /// 为什么必须钉住（用户 2026-09-16 实测「加群的提示怎么是英文？」）：
    /// 「跟随系统」的解析规则（`navigator.language`）只在前端有一份，而后端的兜底在
    /// Windows 上恒为「否」—— 少了中间那一层，中文用户在默认设置下会看到英文的系统消息
    /// （群成员变更 / 文件下载 / 托盘提示 / 窗口标题），而界面本身是中文。
    #[test]
    fn ui_language_prefers_preference_then_frontend_hint_then_env() {
        // ① 显式偏好最高：不受前端与系统影响
        assert!(resolve_is_zh(Some("zh-CN"), Some(false), false));
        assert!(!resolve_is_zh(Some("en-US"), Some(true), true));
        // ② 「跟随系统」：前端推来的结果说了算（环境兜底在 Windows 上是错的，不能盖过它）
        assert!(resolve_is_zh(Some("system"), Some(true), false));
        assert!(!resolve_is_zh(Some("system"), Some(false), true));
        // ③ 前端还没推过：才用环境变量兜底
        assert!(resolve_is_zh(None, None, true));
        assert!(!resolve_is_zh(None, None, false));
        // ④ 历史脏值一律按「跟随系统」处理（与前端 isLanguagePreference 同口径）
        assert!(resolve_is_zh(Some("fr-FR"), Some(true), false));
        assert!(!resolve_is_zh(Some(""), None, false));
    }

    /// 三态编码不许撞车（0 是"还没推过"的哨兵，不能被当成某种语言）。
    #[test]
    fn ui_language_hint_states_are_distinct() {
        assert_ne!(UI_LANG_UNKNOWN, UI_LANG_ZH);
        assert_ne!(UI_LANG_UNKNOWN, UI_LANG_EN);
        assert_ne!(UI_LANG_ZH, UI_LANG_EN);
    }
}
