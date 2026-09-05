//! 网络协议层：定义 UDP 发现包与 TCP 帧的线格式，以及与前端交互的公开类型。
//!
//! 设计要点：
//! - 所有消息均为 `{ "type": "...", ... }` 形态的 JSON，便于未来在 QUIC / WebSocket 中继上复用。
//! - TCP 帧 = 4 字节大端长度前缀 + JSON 负载，最大 64MB（足以承载 256KB 文件的 base64 分片）。

use serde::{Deserialize, Serialize};

/// UDP 发现端口（局域网广播）
pub const UDP_PORT: u16 = 59991;
/// TCP 消息/文件传输端口
pub const TCP_PORT: u16 = 59992;
/// 单帧最大字节数（64MB）
pub const MAX_FRAME: usize = 64 * 1024 * 1024;
/// 文件分片原始大小（256KB，base64 后约 342KB）
pub const FILE_CHUNK: usize = 256 * 1024;
/// 广播/发现周期（秒）
pub const ANNOUNCE_INTERVAL_SECS: u64 = 5;
/// 节点离线判定阈值（秒）
pub const PEER_TIMEOUT_SECS: i64 = 15;

/// 消息内容类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MsgKind {
    Text,
    Code,
    Image,
    File,
    System,
}

impl MsgKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MsgKind::Text => "text",
            MsgKind::Code => "code",
            MsgKind::Image => "image",
            MsgKind::File => "file",
            MsgKind::System => "system",
        }
    }

    pub fn from_str(s: &str) -> MsgKind {
        match s {
            "code" => MsgKind::Code,
            "image" => MsgKind::Image,
            "file" => MsgKind::File,
            "system" => MsgKind::System,
            _ => MsgKind::Text,
        }
    }
}

/// 共享目录条目
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ShareEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Gossip 消息类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GossipKind {
    /// 单聊（点对点 E2EE，仅接收方可解密）
    Chat,
    /// 群聊（群密钥对称加密）
    Group,
}

/// Gossip 广播信封（Epidemic 协议消息体）。
/// - `message_id`：SHA-256 十六进制（去重键）
/// - `sender_pubkey` / `sender_ed25519`：发送方 X25519 / Ed25519 公钥
/// - `sender_sig`：对 `message_id` 的 Ed25519 签名（身份校验）
/// - `ttl`：生存时间，每转发一次减一，归零丢弃
/// - `payload`：base64（`encrypted=true` 时为 `nonce || ChaCha20-Poly1305 密文`，否则为明文 JSON）
/// - `encrypted`：载荷是否加密（E2EE 开关；缺省按 true 兼容旧版本）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GossipEnvelope {
    pub message_id: String,
    pub sender_id: String,
    pub sender_pubkey: String,
    pub sender_ed25519: String,
    pub sender_sig: String,
    pub ttl: u8,
    pub kind: GossipKind,
    pub group_id: Option<String>,
    pub payload: String,
    pub ts: i64,
    #[serde(default = "default_encrypted")]
    pub encrypted: bool,
}

/// 旧版本信封无 `encrypted` 字段时按「已加密」处理（历史行为）。
fn default_encrypted() -> bool {
    true
}

impl GossipEnvelope {
    /// 计算并填充 message_id（SHA-256 of sender_id + ts + payload）。
    pub fn compute_message_id(&mut self) {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.sender_id.as_bytes());
        h.update(self.ts.to_le_bytes());
        h.update(self.payload.as_bytes());
        self.message_id = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    }
}

/// TCP 帧消息（P2P 节点间传输）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// 连接建立后首先发送的握手包
    Hello {
        device_id: String,
        nickname: String,
        avatar: Option<String>,
        tcp_port: u16,
    },
    /// 心跳
    Heartbeat {
        device_id: String,
    },
    /// 用户资料变更同步（昵称/头像）
    UserInfo {
        device_id: String,
        nickname: String,
        avatar: Option<String>,
    },
    /// 聊天样式同步：发送方广播自己的气泡/字体偏好，接收方持久化并按其偏好渲染该发送者的消息
    ChatStyle {
        from: String,
        /// 目标节点（None = 广播给所有已连接节点）
        to: Option<String>,
        /// 样式 JSON，如 {"preset":"classic","fontSize":"md","compact":true}
        style: String,
    },
    /// 加好友申请
    FriendRequest {
        from: String,
        from_nickname: String,
        from_avatar: Option<String>,
        to: String,
        ts: i64,
    },
    FriendAccept {
        from: String,
        to: String,
    },
    FriendReject {
        from: String,
        to: String,
    },
    /// 单聊消息
    ChatMessage {
        msg_id: String,
        from: String,
        to: String,
        kind: MsgKind,
        content: String,
        ts: i64,
    },
    /// 群聊消息（携带群名，便于接收方本地展示）
    GroupMessage {
        msg_id: String,
        from: String,
        group_id: String,
        group_name: String,
        kind: MsgKind,
        content: String,
        ts: i64,
    },
    /// 送达确认（用于离线补发去重）
    Ack {
        msg_id: String,
    },
    /// 已读回执：接收方打开会话时告知发送方「读到 last_read_ts 为止的消息都看了」
    ReadReceipt {
        from: String,
        to: String,
        last_read_ts: i64,
    },
    // ---- 文件传输 ----
    FileOffer {
        transfer_id: String,
        from: String,
        name: String,
        size: u64,
    },
    FileAccept {
        transfer_id: String,
    },
    FileReject {
        transfer_id: String,
    },
    FileChunk {
        transfer_id: String,
        seq: u32,
        data: String,
    },
    FileDone {
        transfer_id: String,
    },
    // ---- 共享目录 ----
    ShareTreeRequest {
        request_id: String,
        from: String,
        to: String,
    },
    ShareTreeResponse {
        request_id: String,
        from: String,
        entries: Vec<ShareEntry>,
    },
    /// 请求对方共享目录中的文件（触发对方向我方发起文件传输）
    ShareFileRequest {
        transfer_id: String,
        from: String,
        path: String,
    },
    /// Gossip 广播信封（去中心化消息分发）
    Gossip {
        envelope: GossipEnvelope,
    },
    /// 大文件切片中继转发（BitTorrent 式 Mesh 分发）
    RelayChunk {
        transfer_id: String,
        seq: u32,
        data: String,
        from: String,
        to: String,
        ttl: u8,
    },
    /// 群密钥分发（用成员公钥 ECDH 加密的群密钥）
    GroupKey {
        group_id: String,
        from: String,
        to: String,
        key: String,
    },
    /// 中继文件传输元数据（切片总数等，先于 RelayChunk）
    RelayFileOffer {
        transfer_id: String,
        from: String,
        to: String,
        name: String,
        size: u64,
        total_chunks: u32,
    },
}

/// UDP 广播/回复包
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UdpPacket {
    /// "announce"（主动广播自身） | "who_has"（询问局域网内谁在线）
    pub kind: String,
    pub device_id: String,
    pub nickname: String,
    pub avatar: Option<String>,
    pub tcp_port: u16,
    /// X25519 公钥（base64，用于 ECDH）
    pub x25519_pubkey: Option<String>,
    /// Ed25519 公钥（base64，用于验签）
    pub ed25519_pubkey: Option<String>,
    pub ts: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> GossipEnvelope {
        GossipEnvelope {
            message_id: String::new(),
            sender_id: "dev-a".into(),
            sender_pubkey: "xk".into(),
            sender_ed25519: "ek".into(),
            sender_sig: "sig".into(),
            ttl: 6,
            kind: GossipKind::Chat,
            group_id: None,
            payload: "ciphertext".into(),
            ts: 123456,
            encrypted: true,
        }
    }

    #[test]
    fn envelope_encrypted_flag_roundtrip_and_default() {
        // 显式 false 往返保持 false
        let mut e = env();
        e.encrypted = false;
        e.compute_message_id();
        let json = serde_json::to_string(&Message::Gossip { envelope: e.clone() }).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => assert!(!envelope.encrypted),
            _ => panic!("expect gossip"),
        }

        // 旧版本 JSON 缺失 encrypted 字段 → 默认按「已加密」兼容处理
        let legacy = r#"{"type":"gossip","envelope":{"message_id":"m","sender_id":"a","sender_pubkey":"x","sender_ed25519":"e","sender_sig":"s","ttl":6,"kind":"chat","group_id":null,"payload":"p","ts":1}}"#;
        let back: Message = serde_json::from_str(legacy).unwrap();
        match back {
            Message::Gossip { envelope } => assert!(envelope.encrypted),
            _ => panic!("expect gossip"),
        }
    }

    #[test]
    fn gossip_message_id_deterministic_and_sensitive_to_payload() {
        let mut e1 = env();
        e1.compute_message_id();
        let id1 = e1.message_id.clone();
        assert_eq!(id1.len(), 64); // SHA-256 hex

        let mut e2 = e1.clone();
        e2.compute_message_id();
        assert_eq!(id1, e2.message_id); // 同内容同 id

        e2.payload = "tampered".into();
        e2.compute_message_id();
        assert_ne!(id1, e2.message_id); // 篡改 payload → id 变化
    }

    #[test]
    fn message_json_roundtrip() {
        let mut e = env();
        e.compute_message_id();
        let msg = Message::Gossip { envelope: e.clone() };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => {
                assert_eq!(envelope.message_id, e.message_id);
                assert_eq!(envelope.sender_id, "dev-a");
            }
            _ => panic!("应还原为 Gossip 消息"),
        }
    }

    #[test]
    fn msg_kind_mapping() {
        assert_eq!(MsgKind::from_str("code"), MsgKind::Code);
        assert_eq!(MsgKind::from_str("unknown"), MsgKind::Text);
        assert_eq!(MsgKind::Code.as_str(), "code");
    }
}
