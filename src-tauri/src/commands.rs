//! Tauri 命令层：前端调用的所有后端入口。

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
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
    encode_endpoints, parse_endpoint_addr, parse_endpoint_addr_on, parse_endpoints,
    RoutedEndpoint, RELAY_DEFAULT_PORT, ROUTED_ENDPOINTS_KEY,
};
use crate::export;
use crate::logging::LogEntry;
use crate::network::transport::{
    broadcast_gossip, get_group_key, mark_pending_group_key, maybe_update_friend,
    resolve_member_x25519, resolve_nickname, try_send,
};
use crate::network::{self, file};
use crate::protocol::{GossipKind, Message, MsgKind, ShareEntry};
use crate::state::{
    AppState, BleRuntimeFacts, Conversation, DeviceInfo, Favorite, Friend, Group, GroupFile,
    InterfaceInfo, MessageRecord, Peer, PendingRequest, RuntimeSnapshot, TopologyInfo,
    TransferInfo,
};
use crate::storage::cache_cleaner::{self, CachePolicy, CleanupReport};
use crate::transport::TransportManager;

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

// ---- system.rs ----
include!("commands/system.rs");

// ---- network.rs ----
include!("commands/network.rs");

// ---- dev_diag.rs ----
include!("commands/dev_diag.rs");

// ---- channel.rs ----
include!("commands/channel.rs");

// ---- settings.rs ----
include!("commands/settings.rs");

// ---- friends.rs ----
include!("commands/friends.rs");

// ---- mobile_picker.rs ----
include!("commands/mobile_picker.rs");

// ---- chat.rs ----
include!("commands/chat.rs");

// ---- groups.rs ----
include!("commands/groups.rs");

// ---- window.rs ----
include!("commands/window.rs");

// ---- group_files.rs ----
include!("commands/group_files.rs");

// ---- files.rs ----
include!("commands/files.rs");

// ---- share.rs ----
include!("commands/share.rs");

// ---- helpers.rs ----
include!("commands/helpers.rs");

// ---- favorites.rs ----
include!("commands/favorites.rs");

// ---- routed.rs ----
include!("commands/routed.rs");

// ---- relay.rs ----
include!("commands/relay.rs");

// ---- external_links.rs ----
include!("commands/external_links.rs");

// ---- logs.rs ----
include!("commands/logs.rs");
