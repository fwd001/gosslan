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
/// 跨跳（无直连）节点离线判定阈值（秒）。
///
/// 跨跳节点没有直连 TCP，`last_seen` 只能靠 Presence（10s 周期）经中继转发刷新。
/// 若沿用 15s，10s 周期只留 5s 余量，Tailscale 等高延迟中继一旦抖动，某次 Presence
/// 迟到超过 15s 就被 `sweep_peers` 误删 → 在线状态「一会儿绿一会儿灰」。
/// 45s ≈ 4.5 个 Presence 周期，给中继延迟留足余量；代价是跨跳节点真正离线后
/// 最多约 45s 才判离线（可接受）。
///
/// 有直连 TCP 的节点不依赖本阈值：`sweep_peers` 用 `active_links` 直接豁免，
/// 且连接断开时由 `mark_peer_offline` 立即移除（无需超时兜底）。
pub const RELAY_PEER_TIMEOUT_SECS: i64 = 45;

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
    /// 好友关系拦截通知（明文 JSON payload，携带 original_sender）
    FriendMessageBlocked,
    /// 节点通告：周期广播自身身份，跨跳传播让全网节点互相可见（TOFU 语义）。
    /// 明文（encrypted=false），payload 为 JSON（昵称 / 头像）。
    Presence,
    /// 好友申请（定向跨跳）：payload 为 E2EE 密文（用目标 X25519 公钥加密），
    /// `target` 指定接收方 device_id；中间节点按 target 定向转发（一跳精确，
    /// 无路由表时洪泛兜底）。
    FriendRequest,
    /// 好友申请同意（定向跨跳）：方向与 FriendRequest 相反，其余同理。
    FriendAccept,
    /// 单聊送达确认（定向跨跳）：接收方成功持久化某条单聊 Gossip 后回给原始发送方。
    /// 明文（encrypted=false），payload 为 JSON `{"msg_id":"..."}`，`target` = 原始发送方。
    /// 与直连 `Message::Ack` 语义一致，但可跨跳（跨 Tailscale 无直连时 Ack 到不了发送方）。
    ChatAck,
    /// 单聊已读回执（定向跨跳）：接收方读到某发送方消息后回执。明文，
    /// payload 为 JSON `{"last_read_ts":n,"last_read_msg_id":"..."}`，`target` = 原始发送方。
    /// 与直连 `Message::ReadReceipt` 语义一致，但可跨跳。
    ChatReadReceipt,
}

/// Gossip 广播信封（Epidemic 协议消息体）。
/// - `message_id`：SHA-256 十六进制（去重键）
/// - `sender_pubkey` / `sender_ed25519`：发送方 X25519 / Ed25519 公钥
/// - `sender_sig`：对信封不可变字段的 Ed25519 签名（身份校验；TTL 不签名，因为转发会递减）
/// - `ttl`：生存时间，每转发一次减一，归零丢弃
/// - `payload`：base64（用户聊天 `encrypted=true` 时为 `nonce || ChaCha20-Poly1305 密文`）
/// - `encrypted`：载荷是否加密；用户聊天必须为 true，内部拒绝通知可使用明文控制载荷
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GossipEnvelope {
    pub message_id: String,
    pub sender_id: String,
    /// 每条消息的随机 nonce：message_id = SHA-256(sender_id + nonce + payload)。
    /// 不再用本地时间戳参与消息身份，避免同毫秒碰撞，也避免业务身份依赖系统时间。
    #[serde(default)]
    pub nonce: String,
    pub sender_pubkey: String,
    pub sender_ed25519: String,
    pub sender_sig: String,
    pub ttl: u8,
    pub kind: GossipKind,
    pub group_id: Option<String>,
    /// 群名快照：随消息广播，接收方本地无群记录时可直接展示正确群名
    /// （不参与 `compute_message_id` 哈希，不影响跨路径去重）。
    #[serde(default)]
    pub group_name: Option<String>,
    /// 群创建者 ID + 当前成员列表：随群消息广播，使只收到群消息、
    /// 从未收到 GroupKey 的成员也能据此在本地建立/刷新群记录（含成员）。
    /// 与 `group_name` 同理，不参与 message_id 哈希。
    #[serde(default)]
    pub group_creator: Option<String>,
    #[serde(default)]
    pub group_members: Vec<String>,
    pub payload: String,
    pub ts: i64,
    /// 会话逻辑序号（Lamport），签名覆盖；接收方按此排序。
    #[serde(default)]
    pub seq: i64,
    pub encrypted: bool,
    /// 定向目标 device_id（仅 `FriendRequest` 使用；`None` = 广播）。
    /// 参与签名，中间节点不可篡改目标；不参与 message_id（nonce 已保证唯一）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl GossipEnvelope {
    /// 计算并填充 message_id（SHA-256 of sender_id + nonce + payload）。
    /// 用随机 nonce 而非时间戳：消息身份不依赖本地时钟，也不存在同毫秒碰撞。
    pub fn compute_message_id(&mut self) {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.sender_id.as_bytes());
        h.update(self.nonce.as_bytes());
        h.update(self.payload.as_bytes());
        self.message_id = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    }

    /// 生成签名材料。TTL 是唯一允许中继节点修改的字段；其余路由、身份、
    /// 群成员和载荷字段都必须被签名，避免“签名仍有效但把 Chat 改成 Group”之类的
    /// 元数据篡改。
    pub fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            &self.message_id,
            &self.sender_id,
            &self.nonce,
            &self.sender_pubkey,
            &self.sender_ed25519,
            &self.kind,
            &self.group_id,
            &self.group_name,
            &self.group_creator,
            &self.group_members,
            &self.payload,
            &self.ts,
            &self.seq,
            &self.encrypted,
            &self.target,
        ))
        .unwrap_or_default()
    }
}

/// TCP 帧消息（P2P 节点间传输）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// 连接建立后首先发送的握手包。
    ///
    /// `nonce` + `sig` 是**连接身份认证**：只有持有 `device_id` 绑定私钥的一方
    /// 能对 `hello_signing_bytes()` 产出合法签名。接收方在建立链路前用它确认
    /// 「这个 TCP 对端确实是 device_id 本人」，杜绝任意节点冒用他人（好友/群主）
    /// device_id 建链后伪造明文控制消息（GroupMemberRemoved / GroupRename 等）。
    Hello {
        device_id: String,
        nickname: String,
        avatar: Option<String>,
        tcp_port: u16,
        x25519_pubkey: String,
        ed25519_pubkey: String,
        /// 与对方单聊会话的本地逻辑时钟：用于建链时快速对齐，
        /// 避免双方时钟长期不同步导致新消息序号偏小。
        #[serde(default)]
        conv_clock: i64,
        /// 每次握手新生成的随机串（base64），参与签名并供接收方防重放去重。
        #[serde(default)]
        nonce: String,
        /// Ed25519 签名（base64），覆盖 `hello_signing_bytes()` 的全部字段。
        #[serde(default)]
        sig: String,
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
    FriendRemove {
        from: String,
        to: String,
    },
    FriendMessageBlocked {
        from: String,
        to: String,
        original_sender: String,
    },
    /// 单聊消息
    ChatMessage {
        msg_id: String,
        from: String,
        to: String,
        kind: MsgKind,
        content: String,
        ts: i64,
        /// 会话逻辑序号（Lamport），接收方按此排序，而非发送方墙上时钟。
        #[serde(default)]
        seq: i64,
    },
    /// 送达确认（用于离线补发去重）
    Ack {
        msg_id: String,
    },
    /// 已读回执：接收方打开会话时告知发送方「读到 last_read_ts 为止的消息都看了」。
    /// `last_read_msg_id` 指向接收方最近读到的一条**发送方消息**，发送方用它
    /// 换算回自己的本地时间戳，避免设备间时钟偏差导致回执失效。
    ReadReceipt {
        from: String,
        to: String,
        last_read_ts: i64,
        #[serde(default)]
        last_read_msg_id: Option<String>,
    },
    /// 群聊成员级已读回执：接收方读到群消息的时间点。
    /// `last_read_msg_id` 与单聊回执同理，指向该成员最近读到的一条群消息。
    GroupReadReceipt {
        from: String,
        group_id: String,
        last_read_ts: i64,
        #[serde(default)]
        last_read_msg_id: Option<String>,
    },
    /// 群消息送达确认：接收方成功持久化某条群消息后回给原始发送者，
    /// 发送方据此删除对应 `group_outbox(msg_id, peer_id)` 行。
    GroupAck {
        group_id: String,
        msg_id: String,
        from: String,
    },
    // ---- 文件传输 ----
    /// 发起文件传输。`sealed_file_key`：发送方为本 transfer 生成的随机
    /// 32B 文件会话密钥，用接收方 X25519 公钥 ECDH + AEAD 封装——
    /// 只有接收方能解封；后续 FileChunk.data 均用该密钥加密。
    /// `file_sha256`：整个原文件的 SHA-256（64 位小写 hex），仅用于
    /// 文件级完整性验证；分片级防篡改由 AEAD 承担。
    FileOffer {
        transfer_id: String,
        from: String,
        name: String,
        size: u64,
        sealed_file_key: String,
        file_sha256: String,
    },
    FileAccept {
        transfer_id: String,
    },
    FileReject {
        transfer_id: String,
    },
    /// `data`：文件会话密钥 AEAD 加密后的 base64（nonce || ciphertext），
    /// 密文在 TCP / 中继上均不透明。
    FileChunk {
        transfer_id: String,
        seq: u32,
        data: String,
    },
    FileDone {
        transfer_id: String,
    },
    /// 接收方对文件传输的最终确认（成功持久化并校验完成后才允许回 success=true）。
    /// 发送方只有收到 success=true 才能把本地文件消息推进到 delivered。
    FileCompleteAck {
        transfer_id: String,
        success: bool,
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
    /// 群密钥分发（用成员公钥 ECDH 加密的群密钥）。
    /// 同时携带群名与成员列表：成员端据此在本地建群记录，
    /// 否则收到首条群消息时只能兜底成「群聊 g-xxxx」。
    GroupKey {
        group_id: String,
        from: String,
        to: String,
        key: String,
        #[serde(default)]
        group_name: String,
        #[serde(default)]
        members: Vec<String>,
        /// 该群的当前逻辑时钟：成员上线拿到密钥时同步本地时钟，
        /// 保证其后续新消息序号大于清空边界等本地水位。
        #[serde(default)]
        clock: i64,
    },
    /// 群文件发起（不含文件内容）。`sealed_file_key`：发送方为本 transfer
    /// 生成的随机 32B 文件会话密钥，用**群密钥** AEAD 封装（seal_symmetric）——
    /// 群内成员用本地 GroupKey 解封，群外与中继无法解开。
    /// 后续群文件分片均以该 file_key 加密（下一阶段实现）。
    GroupFileOffer {
        transfer_id: String,
        group_id: String,
        sender_id: String,
        name: String,
        size: u64,
        sha256: String,
        sealed_file_key: String,
    },
    /// 群文件分片。`data` = Base64(nonce || AEAD(file_key, plaintext))，
    /// file_key 仅存在于收发双方内存（AppState.group_file_keys），
    /// 群密钥只负责封装 file_key，绝不直接加密文件内容。
    GroupFileChunk {
        transfer_id: String,
        group_id: String,
        sender_id: String,
        seq: u32,
        data: String,
    },
    /// 群文件发送完毕（发送方全部分片已发出）。
    /// 接收端据此做最终校验（size + SHA-256）并落盘正式文件；
    /// 接收完成与否以接收端本地校验结果为准，本消息不是完成确认。
    GroupFileDone {
        transfer_id: String,
        group_id: String,
        sender_id: String,
    },
    /// 群文件接收完成确认（receiver → 原始 sender）。
    /// `sender_id` = ACK 发送者（即原 recipient），发送方必须校验
    /// sender_id == TCP peer_id，且 transfer 的 group_file.sender_id 是本机。
    /// success = 本地 size/SHA-256 校验通过并已 rename 落盘。
    GroupFileCompleteAck {
        transfer_id: String,
        group_id: String,
        sender_id: String,
        success: bool,
    },
    /// 群名变更广播（创建者改名后通知各成员同步本地群名）
    GroupRename {
        group_id: String,
        from: String,
        name: String,
    },
    /// 成员被移出群：仅群创建者发起，发给被移除的成员本人。
    /// 接收方删除本地群记录与会话，并撤销群密钥。
    GroupMemberRemoved {
        group_id: String,
        from: String,
        to: String,
    },
    /// 群主转让：仅**当前**创建者可发起。接收方校验 `from` 是本地记录的创建者、
    /// `to` 是群成员后，把本地群创建者改为 `to`。用于群主更换设备/卸载前移交
    /// 管理权，避免群永久失去改名/加人/踢人能力。
    GroupCreatorChanged {
        group_id: String,
        from: String,
        to: String,
    },
    /// 成员主动退群（非群主）。接收方把 `from` 从本地群成员中移除。
    /// 群主退出前必须先转让（由 `leave_group` 命令强制）。
    GroupMemberLeft {
        group_id: String,
        from: String,
    },
    /// 中继文件传输元数据（切片总数等，先于 RelayChunk）。
    /// `sealed_file_key`：与 FileOffer 同义——用接收方公钥封装的文件会话密钥，
    /// 中继节点不持有也不解封，仅接收方能解开。
    /// `file_sha256`：原文件 SHA-256（hex），中继不解密不校验，仅透传给接收方。
    RelayFileOffer {
        transfer_id: String,
        from: String,
        to: String,
        name: String,
        size: u64,
        total_chunks: u32,
        sealed_file_key: String,
        file_sha256: String,
    },
}

/// Hello 帧的签名材料（版本前缀 + 全部连接身份字段）。
///
/// 用 `serde_json` 序列化元组而非手写字符串拼接：避免字段里出现分隔符时产生
/// 「不同字段组合出同一段字节」的歧义（长度前缀/分隔符逃逸问题）。
/// 接收方以 `device_id` 绑定的 Ed25519 公钥验签，从而确认 peer_id 不可冒充。
pub fn hello_signing_bytes(
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
) -> Vec<u8> {
    serde_json::to_vec(&(
        "gosslan-hello-v1",
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
    ))
    .unwrap_or_default()
}

/// UDP 广播/回复包
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UdpPacket {
    /// "announce"（主动广播自身） | "who_has"（询问局域网内谁在线）
    pub kind: String,
    pub device_id: String,
    pub nickname: String,
    pub tcp_port: u16,
    /// X25519 公钥（base64，用于 ECDH）
    pub x25519_pubkey: Option<String>,
    /// Ed25519 公钥（base64，用于验签）
    pub ed25519_pubkey: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> GossipEnvelope {
        GossipEnvelope {
            message_id: String::new(),
            sender_id: "dev-a".into(),
            nonce: "nonce-1".into(),
            sender_pubkey: "xk".into(),
            sender_ed25519: "ek".into(),
            sender_sig: "sig".into(),
            ttl: 6,
            kind: GossipKind::Chat,
            group_id: None,
            group_name: None,
            group_creator: None,
            group_members: Vec::new(),
            payload: "ciphertext".into(),
            ts: 123456,
            seq: 1,
            encrypted: true,
            target: None,
        }
    }

    /// 好友申请（定向）信封：加密、签名、验签、解密、target 完整性。
    #[test]
    fn friend_request_envelope_encrypt_sign_decrypt_and_target_integrity() {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        use crate::crypto::Identity;
        use crate::gossip_engine::GossipEngine;

        let a = Identity::generate();
        let c = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        // A 构造 FriendRequest（target=C，用 C 的 X25519 公钥加密内容）
        let payload = r#"{"from_nickname":"Alice","from_avatar":null}"#;
        let shared = crate::crypto::shared_secret(&a.x25519_secret, &c.x25519_public_b64())
            .unwrap();
        let sealed = crate::crypto::seal(&shared, payload.as_bytes()).unwrap();
        let payload_b64 = B64.encode(&sealed);
        let mut env = engine.build_envelope(
            &a,
            "dev-a",
            GossipKind::FriendRequest,
            None,
            None,
            &payload_b64,
            123456,
            0,
        );
        env.target = Some("dev-c".into());
        env.sender_sig = a.sign_b64(&env.signing_bytes());

        // 验签通过（target 参与签名）
        assert!(engine.verify_envelope(&env));

        // C 用自己的私钥解开内容
        let shared2 = crate::crypto::shared_secret(&c.x25519_secret, &env.sender_pubkey).unwrap();
        let pt = crate::crypto::open(&shared2, &B64.decode(&env.payload).unwrap()).unwrap();
        assert_eq!(String::from_utf8(pt).unwrap(), payload);

        // 中间节点篡改 target → 验签失败（target 不可篡改）
        let mut tampered = env.clone();
        tampered.target = Some("dev-eve".into());
        assert!(!engine.verify_envelope(&tampered));

        // target 序列化：None 不写键，Some 写入
        let json_none = serde_json::to_string(&GossipEnvelope {
            target: None,
            ..env.clone()
        })
        .unwrap();
        assert!(!json_none.contains("target"), "None 不应写 target 键: {json_none}");
        let json_some = serde_json::to_string(&env).unwrap();
        assert!(json_some.contains("dev-c"), "Some 应写 target: {json_some}");
    }

    /// ChatAck / ChatReadReceipt（定向、明文）信封：签名、验签、target 完整性、明文往返。
    #[test]
    fn chat_ack_and_read_receipt_plaintext_directed_envelope_integrity() {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        use crate::crypto::Identity;
        use crate::gossip_engine::GossipEngine;

        let c = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        // ChatAck：接收方 C 回给原始发送方 A，明文 { msg_id }
        let ack_payload = r#"{"msg_id":"deadbeef"}"#;
        let mut ack = engine.build_envelope(
            &c,
            "dev-c",
            GossipKind::ChatAck,
            None,
            None,
            &B64.encode(ack_payload.as_bytes()),
            123456,
            0,
        );
        ack.encrypted = false;
        ack.target = Some("dev-a".into());
        ack.sender_sig = c.sign_b64(&ack.signing_bytes());

        // 验签通过（target 参与签名）
        assert!(engine.verify_envelope(&ack));
        // 明文：直接 base64 解码即可得到原始 JSON，无需解密
        assert_eq!(B64.decode(&ack.payload).unwrap(), ack_payload.as_bytes());
        // 篡改 target → 验签失败
        let mut tampered = ack.clone();
        tampered.target = Some("dev-eve".into());
        assert!(!engine.verify_envelope(&tampered));

        // ChatReadReceipt：定向明文，payload 含 last_read_ts / last_read_msg_id
        let rr_payload = r#"{"last_read_ts":99,"last_read_msg_id":"m-1"}"#;
        let mut rr = engine.build_envelope(
            &c,
            "dev-c",
            GossipKind::ChatReadReceipt,
            None,
            None,
            &B64.encode(rr_payload.as_bytes()),
            123456,
            0,
        );
        rr.encrypted = false;
        rr.target = Some("dev-a".into());
        rr.sender_sig = c.sign_b64(&rr.signing_bytes());
        assert!(engine.verify_envelope(&rr));
        assert_eq!(B64.decode(&rr.payload).unwrap(), rr_payload.as_bytes());
    }

    #[test]
    fn envelope_encrypted_flag_roundtrip() {
        // 显式 false 往返保持 false
        let mut e = env();
        e.encrypted = false;
        e.compute_message_id();
        let json = serde_json::to_string(&Message::Gossip {
            envelope: e.clone(),
        })
        .unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Gossip { envelope } => assert!(!envelope.encrypted),
            _ => panic!("expect gossip"),
        }

        // 未声明加密标志的旧信封直接拒绝，不再兼容旧协议。
        let legacy = r#"{"type":"gossip","envelope":{"message_id":"m","sender_id":"a","sender_pubkey":"x","sender_ed25519":"e","sender_sig":"s","ttl":6,"kind":"chat","group_id":null,"payload":"p","ts":1}}"#;
        assert!(serde_json::from_str::<Message>(legacy).is_err());
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

        // 时间戳不参与消息身份：改变 ts 不应改变 message_id。
        let mut e3 = e1.clone();
        e3.ts = 999_999;
        e3.compute_message_id();
        assert_eq!(id1, e3.message_id);

        // nonce 参与消息身份：改变 nonce 必须改变 message_id。
        let mut e4 = e1.clone();
        e4.nonce = "nonce-2".into();
        e4.compute_message_id();
        assert_ne!(id1, e4.message_id);
    }

    #[test]
    fn message_json_roundtrip() {
        let mut e = env();
        e.compute_message_id();
        let msg = Message::Gossip {
            envelope: e.clone(),
        };
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

    #[test]
    fn hello_signing_bytes_sensitive_to_every_field() {
        let base = hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek");
        // 相同输入必须产出相同字节（签名可复现）
        assert_eq!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek"));
        // 任一字段变化都必须改变签名材料：否则攻击者可平移字段伪造身份
        assert_ne!(base, hello_signing_bytes("dev-b", 59992, "n1", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 1, "n1", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n2", "xk", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk2", "ek"));
        assert_ne!(base, hello_signing_bytes("dev-a", 59992, "n1", "xk", "ek2"));
        // 拼接歧义防护：把不同字段切成另一种组合不应撞车
        assert_ne!(
            hello_signing_bytes("ab", 1, "c", "d", "e"),
            hello_signing_bytes("a", 1, "bc", "d", "e")
        );
    }

    #[test]
    fn hello_carries_nonce_and_sig_roundtrip() {
        let hello = Message::Hello {
            device_id: "dev-a".into(),
            nickname: "A".into(),
            avatar: None,
            tcp_port: 59992,
            x25519_pubkey: "xk".into(),
            ed25519_pubkey: "ek".into(),
            conv_clock: 7,
            nonce: "n1".into(),
            sig: "sig".into(),
        };
        let json = serde_json::to_string(&hello).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::Hello { nonce, sig, .. } => {
                assert_eq!(nonce, "n1");
                assert_eq!(sig, "sig");
            }
            _ => panic!("expect hello"),
        }
        // 不带 nonce/sig 的旧 Hello 仍可解析（serde default），但会在验证层被拒
        let legacy = r#"{"type":"hello","device_id":"a","nickname":"A","avatar":null,"tcp_port":1,"x25519_pubkey":"x","ed25519_pubkey":"e","conv_clock":0}"#;
        match serde_json::from_str::<Message>(legacy).unwrap() {
            Message::Hello { nonce, sig, .. } => {
                assert!(nonce.is_empty() && sig.is_empty());
            }
            _ => panic!("expect hello"),
        }
    }

    #[test]
    fn group_lifecycle_messages_roundtrip() {
        let changed = Message::GroupCreatorChanged {
            group_id: "g1".into(),
            from: "old".into(),
            to: "new".into(),
        };
        let json = serde_json::to_string(&changed).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupCreatorChanged { group_id, from, to } => {
                assert_eq!((group_id.as_str(), from.as_str(), to.as_str()), ("g1", "old", "new"));
            }
            _ => panic!("expect group_creator_changed"),
        }

        let left = Message::GroupMemberLeft {
            group_id: "g1".into(),
            from: "dev-a".into(),
        };
        let json = serde_json::to_string(&left).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupMemberLeft { group_id, from } => {
                assert_eq!((group_id.as_str(), from.as_str()), ("g1", "dev-a"));
            }
            _ => panic!("expect group_member_left"),
        }
    }

    #[test]
    fn group_ack_and_file_complete_ack_roundtrip() {
        let group_ack = Message::GroupAck {
            group_id: "g1".into(),
            msg_id: "m1".into(),
            from: "dev-a".into(),
        };
        let json = serde_json::to_string(&group_ack).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::GroupAck {
                group_id,
                msg_id,
                from,
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(msg_id, "m1");
                assert_eq!(from, "dev-a");
            }
            _ => panic!("expect group_ack"),
        }

        let file_ack = Message::FileCompleteAck {
            transfer_id: "t1".into(),
            success: true,
        };
        let json = serde_json::to_string(&file_ack).unwrap();
        match serde_json::from_str::<Message>(&json).unwrap() {
            Message::FileCompleteAck {
                transfer_id,
                success,
            } => {
                assert_eq!(transfer_id, "t1");
                assert!(success);
            }
            _ => panic!("expect file_complete_ack"),
        }
    }
}
