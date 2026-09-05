//! 协议级 E2E 测试对端：无 GUI 验证运行中的 Gosslan 实例的连通性与文件接收。
//!
//! 前置：目标实例以 `GOSSLAN_AUTOSTART=1` 启动（网络通道自动开启）。
//! 用法：
//!   cargo run --example e2e_peer -- [--i1] [--full] ["<gosslan.db 路径>"]
//!   --i1   锁定多开实例 1（device_id 带 -i1 后缀；否则锁定主实例）
//!   --full 全功能模式：除基础连通性外，覆盖多类型消息、群聊、心跳、
//!          资料同步、好友申请、共享目录、下载方向文件传输
//! 传入 DB 路径时额外做落库 / 去重 / 文件落盘校验。
//!
//! 验证项：
//! 1. UDP who_has 单播探测（不依赖广播路由，VPN/TUN 环境可用）
//! 2. TCP 建链 + Hello 握手
//! 3. 直连 ChatMessage 送达（收到 Ack）+ 同 msg_id 重复投递被去重
//! 4. Gossip E2EE（X25519 ECDH + ChaCha20-Poly1305 + Ed25519 验签）投递落库
//! 5. 文件传输（FileOffer → FileAccept → FileChunk 流 → FileDone → 落盘）
//! 6. outbox 离线补发（注入待补发行 → Heartbeat 触发 flush → 收到补发消息）
//! 7. --full：代码/图片/1MB 大文本消息、乱序消息、群消息落库、
//!    心跳保活、UserInfo 同步、好友申请（等待 UI 同意）、
//!    共享目录树、下载方向文件传输（app→peer 发送路径）

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use gosslan_lib::crypto::{self, Identity};
use gosslan_lib::protocol::{
    GossipEnvelope, GossipKind, Message, MsgKind, UdpPacket, FILE_CHUNK, UDP_PORT,
};

const PEER_ID: &str = "e2e-peer";
const DIRECT_MSG_ID: &str = "e2e-direct-001";
const GOSSIP_TEXT: &str = "e2e-gossip-ok";
const TRANSFER_ID: &str = "e2e-file-001";
const FILE_NAME: &str = "e2e-peer-file.txt";
const OUTBOX_MSG_ID: &str = "e2e-outbox-001";
const OUTBOX_TEXT: &str = "e2e-outbox-flush-ok";

// ---- --full 扩展项常量 ----
const CODE_MSG_ID: &str = "e2e-code-001";
const CODE_CONTENT: &str = "fn main() { println!(\"e2e-code-ok\"); }";
const IMAGE_MSG_ID: &str = "e2e-image-001";
const IMAGE_CONTENT: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
const BIG_MSG_ID: &str = "e2e-big-001";
const BIG_SIZE: usize = 1_000_000;
const OOO_A_ID: &str = "e2e-ooo-a";
const OOO_B_ID: &str = "e2e-ooo-b";
const GROUP_ID: &str = "g-e2e-dev";
const GROUP_MSG_ID: &str = "e2e-group-001";
const GROUP_TEXT: &str = "e2e-group-ok";
const SHARE_REQ_ID: &str = "e2e-share-req-001";
const SHARE_FILE: &str = "hello.txt";
/// 与 scripts/e2e-dev.sh 写入共享目录的文件内容保持一致（下载方向内容比对基准）
const SHARE_FILE_CONTENT: &str = "gosslan-dev-share-v1\n";
const DL_TRANSFER_ID: &str = "e2e-dl-001";
/// 广播的聊天样式（对端应持久化到 chat_peer_styles）
const STYLE_JSON: &str = r#"{"preset":"mint","fontSize":"md","compact":true}"#;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct Report {
    items: Vec<(&'static str, u8, String)>, // 0=FAIL 1=PASS 2=SKIP
}

impl Report {
    fn new() -> Self {
        Self { items: Vec::new() }
    }
    fn add(&mut self, name: &'static str, ok: bool, detail: String) {
        self.items.push((name, if ok { 1 } else { 0 }, detail));
    }
    /// 需要人工交互（如 UI 点击同意）或前置条件不满足时跳过，不计失败。
    fn add_skip(&mut self, name: &'static str, detail: String) {
        self.items.push((name, 2, detail));
    }
    fn print(&self) -> bool {
        println!("\n================ E2E 验证结果 ================");
        for (name, st, detail) in &self.items {
            let tag = match st {
                1 => "PASS",
                2 => "SKIP",
                _ => "FAIL",
            };
            println!("{tag} | {name} | {detail}");
        }
        let failed = self.items.iter().filter(|(_, s, _)| *s == 0).count();
        let skipped = self.items.iter().filter(|(_, s, _)| *s == 2).count();
        let total = self.items.len();
        println!("----------------------------------------------");
        println!("共 {total} 项，通过 {}，失败 {failed}，跳过 {skipped}", total - failed - skipped);
        failed == 0
    }
}

/// 绑定与实例共享的 UDP 端口（SO_REUSEADDR + unix 下 SO_REUSEPORT）监听广播。
#[allow(dead_code)]
fn bind_udp_shared() -> Result<tokio::net::UdpSocket, String> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| e.to_string())?;
    sock.set_reuse_address(true).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    sock.set_reuse_port(true).map_err(|e| e.to_string())?;
    // tokio 要求非阻塞 fd（与主程序 bind_udp_reusable 同样的坑）
    sock.set_nonblocking(true).map_err(|e| e.to_string())?;
    let addr: std::net::SocketAddr = format!("0.0.0.0:{UDP_PORT}")
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    sock.bind(&addr.into()).map_err(|e| e.to_string())?;
    let std_sock: std::net::UdpSocket = sock.into();
    tokio::net::UdpSocket::from_std(std_sock).map_err(|e| e.to_string())
}

/// 通过 who_has 单播探测获取目标实例身份（announce 广播）。
/// 说明：本机为 VPN/TUN 网络环境时 255.255.255.255 无路由（ENETUNREACH），
/// 广播不可用；who_has 走 127.0.0.1 单播，可验证实例的 UDP 接收 + 应答路径。
async fn probe_instance_via_who_has(
    target_i1: bool,
    timeout: Duration,
) -> Result<(String, u16, String, String), String> {
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let who = UdpPacket {
        kind: "who_has".to_string(),
        device_id: PEER_ID.into(),
        nickname: "E2E-Peer".into(),
        avatar: None,
        tcp_port: 0,
        x25519_pubkey: None,
        ed25519_pubkey: None,
        ts: now_ms(),
    };
    let data = serde_json::to_vec(&who).map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    let mut attempt = 0;
    loop {
        sock.send_to(&data, ("127.0.0.1", UDP_PORT)).await.map_err(|e| e.to_string())?;
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remain.is_zero() {
            return Err("who_has 探测超时（实例未应答 announce）".into());
        }
        match tokio::time::timeout(remain, sock.recv_from(&mut buf)).await {
            Ok(Ok((len, _))) => {
                let Ok(pkt) = serde_json::from_slice::<UdpPacket>(&buf[..len]) else { continue };
                if pkt.kind != "announce" || pkt.device_id == PEER_ID {
                    continue;
                }
                // 只接受目标实例：--i1 取多开实例（device_id 带 -iN 后缀），否则取主实例
                let is_multi = pkt.device_id.contains("-i")
                    && pkt.device_id.split("-i").last().unwrap_or("").chars().all(|c| c.is_ascii_digit());
                if target_i1 != is_multi {
                    continue;
                }
                let Some(x25519) = pkt.x25519_pubkey else { continue };
                let nickname = pkt.nickname.clone();
                return Ok((pkt.device_id, pkt.tcp_port, x25519, nickname));
            }
            _ => {
                attempt += 1;
                if attempt >= 5 {
                    return Err("who_has 探测重试 5 次无应答".into());
                }
            }
        }
    }
}

async fn send_frame(w: &mut tokio::net::tcp::OwnedWriteHalf, msg: &Message) -> Result<(), String> {
    let json = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    w.write_all(&(json.len() as u32).to_be_bytes()).await.map_err(|e| e.to_string())?;
    w.write_all(&json).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 从接收通道中按谓词等待一帧（带超时），无关帧暂存到 backlog。
struct FrameWaiter {
    rx: mpsc::Receiver<Message>,
    backlog: Vec<Message>,
}

impl FrameWaiter {
    async fn expect(
        &mut self,
        ms: u64,
        what: &str,
        pred: &(dyn Fn(&Message) -> bool + Send + Sync),
    ) -> Result<Message, String> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            if let Some(i) = self.backlog.iter().position(|m| pred(m)) {
                return Ok(self.backlog.remove(i));
            }
            let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remain.is_zero() {
                return Err(format!("等待{what}超时"));
            }
            match tokio::time::timeout(remain, self.rx.recv()).await {
                Ok(Some(m)) => self.backlog.push(m),
                Ok(None) => return Err("连接已关闭".into()),
                Err(_) => return Err(format!("等待{what}超时")),
            }
        }
    }
}

fn open_db(path: &str) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(5000)).map_err(|e| e.to_string())?;
    Ok(conn)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target_i1 = args.iter().any(|a| a == "--i1");
    let full = args.iter().any(|a| a == "--full");
    let db_path = args.iter().find(|a| !a.starts_with("--") && a.ends_with(".db")).cloned();
    let mut report = Report::new();

    // ---- 1. UDP 发现（who_has 单播探测 → 实例应答 announce）----
    let (app_id, app_port, app_x25519, _app_name) = match probe_instance_via_who_has(target_i1, Duration::from_secs(15)).await {
        Ok(v) => {
            let head: String = v.0.chars().take(10).collect();
            report.add("UDP 发现（who_has 探测 → 实例应答 announce）", true, format!("对端 {}（{head}…）tcp:{}", v.3, v.1));
            v
        }
        Err(e) => {
            report.add("UDP 发现（who_has 探测 → 实例应答 announce）", false, e);
            let _ = report.print();
            std::process::exit(1);
        }
    };

    // ---- 2. TCP 建链 ----
    let stream = match TcpStream::connect(("127.0.0.1", app_port)).await {
        Ok(s) => s,
        Err(e) => {
            report.add("TCP 建链 + Hello 握手", false, format!("连接 127.0.0.1:{app_port} 失败: {e}"));
            let _ = report.print();
            std::process::exit(1);
        }
    };
    let (mut r, mut w) = stream.into_split();
    let (tx, rx) = mpsc::channel::<Message>(64);
    tokio::spawn(async move {
        let mut len_buf = [0u8; 4];
        loop {
            if r.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            if r.read_exact(&mut buf).await.is_err() {
                break;
            }
            if let Ok(msg) = serde_json::from_slice(&buf) {
                let _ = tx.send(msg).await;
            }
        }
    });
    let mut waiter = FrameWaiter { rx, backlog: Vec::new() };

    if let Err(e) = send_frame(&mut w, &Message::Hello {
        device_id: PEER_ID.into(),
        nickname: "E2E-Peer".into(),
        avatar: None,
        tcp_port: 0,
    }).await {
        report.add("TCP 建链 + Hello 握手", false, e);
        let _ = report.print();
        std::process::exit(1);
    }
    report.add("TCP 建链 + Hello 握手", true, format!("127.0.0.1:{app_port} 已建立"));
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ---- 好友申请（路径覆盖，结果在内存/通知，不在本工具断言范围）----
    let _ = send_frame(&mut w, &Message::FriendRequest {
        from: PEER_ID.into(),
        from_nickname: "E2E-Peer".into(),
        from_avatar: None,
        to: app_id.clone(),
        ts: now_ms(),
    }).await;

    // ---- 3. 直连消息 + 去重 ----
    let direct = Message::ChatMessage {
        msg_id: DIRECT_MSG_ID.into(),
        from: PEER_ID.into(),
        to: app_id.clone(),
        kind: MsgKind::Text,
        content: "e2e-direct-ok".into(),
        ts: now_ms(),
    };
    let _ = send_frame(&mut w, &direct).await;
    match waiter.expect(3000, "Ack（直连消息送达回执）", &|m| {
        matches!(m, Message::Ack { msg_id } if msg_id == DIRECT_MSG_ID)
    }).await {
        Ok(_) => report.add("直连 ChatMessage 送达（收到 Ack 回执）", true, format!("msg_id={DIRECT_MSG_ID}")),
        Err(e) => report.add("直连 ChatMessage 送达（收到 Ack 回执）", false, e),
    }
    // 同 msg_id 重发一次 → 接收端去重（仍回 Ack，但不重复入库）
    let _ = send_frame(&mut w, &direct).await;
    match waiter.expect(3000, "Ack（重复投递去重后仍回执）", &|m| {
        matches!(m, Message::Ack { msg_id } if msg_id == DIRECT_MSG_ID)
    }).await {
        Ok(_) => report.add("重复投递处理（去重路径有响应）", true, "同 msg_id 二次投递仍回 Ack".into()),
        Err(e) => report.add("重复投递处理（去重路径有响应）", false, e),
    }

    // ---- 4. Gossip E2EE ----
    let identity = Identity::generate();
    let gossip_msg_id;
    match crypto::shared_secret(&identity.x25519_secret, &app_x25519) {
        Some(shared) => {
            let plaintext = serde_json::json!({ "kind": "text", "content": GOSSIP_TEXT }).to_string();
            match crypto::seal(&shared, plaintext.as_bytes()) {
                Some(sealed) => {
                    let mut env = GossipEnvelope {
                        message_id: String::new(),
                        sender_id: PEER_ID.into(),
                        sender_pubkey: identity.x25519_public_b64(),
                        sender_ed25519: identity.ed25519_public_b64(),
                        sender_sig: String::new(),
                        ttl: 6,
                        kind: GossipKind::Chat,
                        group_id: None,
                        payload: STANDARD.encode(&sealed),
                        ts: now_ms(),
                        encrypted: true,
                    };
                    env.compute_message_id();
                    env.sender_sig = identity.sign_b64(env.message_id.as_bytes());
                    gossip_msg_id = env.message_id.clone();
                    match send_frame(&mut w, &Message::Gossip { envelope: env }).await {
                        Ok(()) => report.add("Gossip E2EE 信封发送（X25519+AEAD+Ed25519 签名）", true, "已按线上格式构造并发送".into()),
                        Err(e) => report.add("Gossip E2EE 信封发送（X25519+AEAD+Ed25519 签名）", false, e),
                    }
                }
                None => {
                    gossip_msg_id = String::new();
                    report.add("Gossip E2EE 信封发送（X25519+AEAD+Ed25519 签名）", false, "加密失败".into());
                }
            }
        }
        None => {
            gossip_msg_id = String::new();
            report.add("Gossip E2EE 信封发送（X25519+AEAD+Ed25519 签名）", false, "ECDH 失败".into());
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- 5. 文件传输 ----
    let content: Vec<u8> = (0..700_000usize).map(|i| (b'a' + (i % 26) as u8) as u8).collect();
    let _ = send_frame(&mut w, &Message::FileOffer {
        transfer_id: TRANSFER_ID.into(),
        from: PEER_ID.into(),
        name: FILE_NAME.into(),
        size: content.len() as u64,
    }).await;
    match waiter.expect(5000, "FileAccept（接收端自动接受）", &|m| {
        matches!(m, Message::FileAccept { transfer_id } if transfer_id == TRANSFER_ID)
    }).await {
        Ok(_) => report.add("文件传输握手（FileOffer → 自动 FileAccept）", true, format!("{FILE_NAME}（{}）", content.len())),
        Err(e) => report.add("文件传输握手（FileOffer → 自动 FileAccept）", false, e),
    }
    let mut seq = 0u32;
    for chunk in content.chunks(FILE_CHUNK) {
        let _ = send_frame(&mut w, &Message::FileChunk {
            transfer_id: TRANSFER_ID.into(),
            seq,
            data: STANDARD.encode(chunk),
        }).await;
        seq += 1;
    }
    let _ = send_frame(&mut w, &Message::FileDone { transfer_id: TRANSFER_ID.into() }).await;
    report.add("文件分片流发送（FileChunk → FileDone）", true, format!("{seq} 片 × 256KB"));
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // ---- 6. outbox 离线补发 ----
    if let Some(db) = &db_path {
        let queued = Message::ChatMessage {
            msg_id: OUTBOX_MSG_ID.into(),
            from: PEER_ID.into(),
            to: app_id.clone(),
            kind: MsgKind::Text,
            content: OUTBOX_TEXT.into(),
            ts: now_ms(),
        };
        let injected = (|| -> Result<usize, String> {
            let conn = open_db(db)?;
            let payload = serde_json::to_string(&queued).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR IGNORE INTO outbox(msg_id, peer_id, payload, created_at) VALUES(?1,?2,?3,?4)",
                rusqlite::params![OUTBOX_MSG_ID, PEER_ID, payload, now_ms()],
            ).map_err(|e| e.to_string())
        })();
        match injected {
            Ok(n) if n > 0 => {
                // Heartbeat 触发接收端 flush_outbox
                let _ = send_frame(&mut w, &Message::Heartbeat { device_id: PEER_ID.into() }).await;
                match waiter.expect(6000, "outbox 补发（Heartbeat 触发 flush）", &|m| {
                    matches!(m, Message::ChatMessage { msg_id, .. } if msg_id == OUTBOX_MSG_ID)
                }).await {
                    Ok(_) => report.add("outbox 离线补发（对方上线 Heartbeat 触发自动 flush）", true, "补发消息已送达".into()),
                    Err(e) => report.add("outbox 离线补发（对方上线 Heartbeat 触发自动 flush）", false, e),
                }
            }
            Ok(_) => report.add("outbox 离线补发（对方上线 Heartbeat 触发自动 flush）", false, "outbox 已存在同 id 行（环境未清理）".into()),
            Err(e) => report.add("outbox 离线补发（对方上线 Heartbeat 触发自动 flush）", false, format!("注入 outbox 行失败: {e}")),
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }

    // ---- 6.5 --full：全功能扩展（除网络发现外的所有聊天功能）----
    if full {
        // a. 心跳保活：发心跳后连接仍可收发
        let _ = send_frame(&mut w, &Message::Heartbeat { device_id: PEER_ID.into() }).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        // b. UserInfo 资料同步（昵称变更广播）
        let _ = send_frame(&mut w, &Message::UserInfo {
            device_id: PEER_ID.into(),
            nickname: "E2E-Peer-Renamed".into(),
            avatar: None,
        }).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        // c. 多类型消息：代码 / 图片 / 1MB 大文本
        for (label, msg_id, kind, content) in [
            ("代码消息（kind=code）", CODE_MSG_ID, MsgKind::Code, CODE_CONTENT),
            ("图片消息（kind=image，dataUrl）", IMAGE_MSG_ID, MsgKind::Image, IMAGE_CONTENT),
            ("1MB 大文本消息（大帧分片）", BIG_MSG_ID, MsgKind::Text, &"x".repeat(BIG_SIZE)),
        ] {
            let _ = send_frame(&mut w, &Message::ChatMessage {
                msg_id: msg_id.into(),
                from: PEER_ID.into(),
                to: app_id.clone(),
                kind: kind.clone(),
                content: content.to_string(),
                ts: now_ms(),
            }).await;
            match waiter.expect(4000, "Ack", &|m| matches!(m, Message::Ack { msg_id: id } if *id == msg_id)).await {
                Ok(_) => report.add(label, true, format!("msg_id={msg_id}, {} bytes", content.len())),
                Err(e) => report.add(label, false, e),
            }
        }

        // d. 乱序消息：ts 较大的先发，校验连接与 Ack 不受影响（前端按 ts 排序）
        let ts_base = now_ms();
        for (msg_id, ts) in [(OOO_A_ID, ts_base + 2000), (OOO_B_ID, ts_base + 1000)] {
            let _ = send_frame(&mut w, &Message::ChatMessage {
                msg_id: msg_id.into(),
                from: PEER_ID.into(),
                to: app_id.clone(),
                kind: MsgKind::Text,
                content: format!("out-of-order-{msg_id}"),
                ts,
            }).await;
        }
        let mut acked = 0;
        for _ in 0..2 {
            if waiter.expect(4000, "Ack", &|m| {
                matches!(m, Message::Ack { msg_id } if *msg_id == OOO_A_ID || *msg_id == OOO_B_ID)
            }).await.is_ok() {
                acked += 1;
            }
        }
        report.add("乱序消息处理（ts 乱序发送均收到 Ack）", acked == 2, format!("acked={acked}/2"));

        // e. 群消息（Gossip 群路径之外的非加密直连群消息入库）
        let _ = send_frame(&mut w, &Message::GroupMessage {
            msg_id: GROUP_MSG_ID.into(),
            from: PEER_ID.into(),
            group_id: GROUP_ID.into(),
            group_name: "E2E Dev 群".into(),
            kind: MsgKind::Text,
            content: GROUP_TEXT.into(),
            ts: now_ms(),
        }).await;
        match waiter.expect(4000, "Ack（群消息回执）", &|m| matches!(m, Message::Ack { msg_id } if msg_id == GROUP_MSG_ID)).await {
            Ok(_) => report.add("群消息送达（GroupMessage → Ack）", true, format!("group={GROUP_ID}")),
            Err(e) => report.add("群消息送达（GroupMessage → Ack）", false, e),
        }

        // f. 心跳后连接活性复验（心跳不破坏链路）
        let _ = send_frame(&mut w, &Message::ChatMessage {
            msg_id: "e2e-alive-001".into(),
            from: PEER_ID.into(),
            to: app_id.clone(),
            kind: MsgKind::Text,
            content: "alive".into(),
            ts: now_ms(),
        }).await;
        match waiter.expect(4000, "Ack（心跳后活性复验）", &|m| matches!(m, Message::Ack { msg_id } if msg_id == "e2e-alive-001")).await {
            Ok(_) => report.add("心跳保活（Heartbeat 后链路仍可收发）", true, "连接活性正常".into()),
            Err(e) => report.add("心跳保活（Heartbeat 后链路仍可收发）", false, e),
        }

        // g. 好友申请：开头已发过 FriendRequest，等待 UI 同意/拒绝（人工交互）
        println!("[提示] 如需验证好友申请通过流程，请在实例窗口「联系人」页点击同意/拒绝（等待 10 秒）…");
        match waiter.expect(10_000, "FriendAccept/FriendReject", &|m| {
            matches!(m, Message::FriendAccept { from, .. } | Message::FriendReject { from, .. } if *from == app_id)
        }).await {
            Ok(Message::FriendAccept { .. }) => report.add("好友申请通过（UI 同意 → FriendAccept）", true, "对方已同意，好友关系建立".into()),
            Ok(Message::FriendReject { .. }) => report.add("好友申请通过（UI 同意 → FriendAccept）", false, "对方点击了拒绝".into()),
            Ok(_) => unreachable!(),
            Err(_) => report.add_skip("好友申请通过（UI 同意 → FriendAccept）", "10s 内未在实例 UI 点击同意（人工交互项）".into()),
        }

        // h. 共享目录树（需 scripts/e2e-dev.sh 预置 share_dir）
        let _ = send_frame(&mut w, &Message::ShareTreeRequest {
            request_id: SHARE_REQ_ID.into(),
            from: PEER_ID.into(),
            to: app_id.clone(),
        }).await;
        match waiter.expect(6000, "ShareTreeResponse", &|m| {
            matches!(m, Message::ShareTreeResponse { request_id, .. } if request_id == SHARE_REQ_ID)
        }).await {
            Ok(Message::ShareTreeResponse { entries, .. }) => {
                let found = entries.iter().find(|e| e.name == SHARE_FILE);
                match found {
                    Some(e) => report.add("共享目录浏览（ShareTreeRequest → 目录树）", true, format!("{} 个条目，含 {SHARE_FILE}（{}B）", entries.len(), e.size)),
                    None => report.add("共享目录浏览（ShareTreeRequest → 目录树）", false, format!("{} 个条目，但不含 {SHARE_FILE}（share_dir 未预置？）", entries.len())),
                }
            }
            Ok(_) => unreachable!(),
            Err(e) => report.add("共享目录浏览（ShareTreeRequest → 目录树）", false, format!("{e}（可能未预置 share_dir，用 scripts/e2e-dev.sh 跑）")),
        }

        // i. 下载方向文件传输：app 主动发起（覆盖 app 侧发送路径，即「发送文件」卡死修复验证）
        let _ = send_frame(&mut w, &Message::ShareFileRequest {
            transfer_id: DL_TRANSFER_ID.into(),
            from: PEER_ID.into(),
            path: SHARE_FILE.into(),
        }).await;
        let dl_result: Result<Vec<u8>, String> = async {
            let offer = waiter.expect(8000, "FileOffer（app 发起下载方向传输）", &|m| {
                matches!(m, Message::FileOffer { transfer_id, .. } if transfer_id == DL_TRANSFER_ID)
            }).await?;
            let (name, size) = match offer {
                Message::FileOffer { name, size, .. } => (name, size),
                _ => unreachable!(),
            };
            let _ = send_frame(&mut w, &Message::FileAccept { transfer_id: DL_TRANSFER_ID.into() }).await;
            // 收集分片直到 FileDone
            let mut parts: Vec<(u32, Vec<u8>)> = Vec::new();
            loop {
                let m = waiter.expect(15_000, "FileChunk/FileDone", &|m| {
                    matches!(m, Message::FileChunk { transfer_id, .. } | Message::FileDone { transfer_id } if transfer_id == DL_TRANSFER_ID)
                }).await?;
                match m {
                    Message::FileChunk { seq, data, .. } => {
                        let bytes = STANDARD.decode(&data).map_err(|e| e.to_string())?;
                        parts.push((seq, bytes));
                    }
                    Message::FileDone { .. } => break,
                    _ => unreachable!(),
                }
            }
            parts.sort_by_key(|(seq, _)| *seq);
            let mut full = Vec::with_capacity(size as usize);
            for (_, b) in parts {
                full.extend_from_slice(&b);
            }
            Ok::<Vec<u8>, String>(if name != SHARE_FILE { Err(format!("文件名不符: {name}"))? } else { full })
        }.await;
        match dl_result {
            Ok(bytes) => {
                let match_ok = bytes == SHARE_FILE_CONTENT.as_bytes();
                report.add("下载方向文件传输（app→peer 发送路径全链路）", match_ok,
                    if match_ok { format!("{SHARE_FILE} 内容逐字节一致（{}B）", bytes.len()) } else { format!("内容不一致（{}B）", bytes.len()) });
            }
            Err(e) => report.add("下载方向文件传输（app→peer 发送路径全链路）", false, e),
        }

        // j. 聊天样式同步（ChatStyle 广播 → 对端持久化）
        let _ = send_frame(&mut w, &Message::ChatStyle {
            from: PEER_ID.into(),
            to: None,
            style: STYLE_JSON.into(),
        }).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        if let Some(db) = &db_path {
            let stored: Option<String> = open_db(db)
                .ok()
                .and_then(|conn| {
                    conn.query_row(
                        "SELECT value FROM settings WHERE key = 'chat_peer_styles'",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .ok()
                });
            let ok = stored.as_deref().map(|s| s.contains(PEER_ID) && s.contains("mint")).unwrap_or(false);
            report.add("聊天样式同步（ChatStyle 广播 → 对端持久化）", ok, format!("{stored:?}"));
        } else {
            report.add_skip("聊天样式同步（ChatStyle 广播 → 对端持久化）", "未传 DB 路径，跳过落库校验".into());
        }
    }

    // ---- 7. DB / 落盘校验 ----
    if let Some(db) = &db_path {
        let Ok(conn) = open_db(db) else {
            report.add("落库校验（SQLite 可读）", false, "打开 DB 失败".into());
            let ok = report.print();
            std::process::exit(if ok { 0 } else { 1 });
        };

        // 直连消息去重
        let direct_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages WHERE msg_id = ?1", rusqlite::params![DIRECT_MSG_ID], |r| r.get(0))
            .unwrap_or(-1);
        report.add("直连消息去重落库（同 msg_id 仅 1 行）", direct_count == 1, format!("count={direct_count}"));

        // Gossip 解密落库
        let gossip_content: Option<String> = conn
            .query_row("SELECT content FROM messages WHERE msg_id = ?1", rusqlite::params![gossip_msg_id], |r| r.get(0))
            .ok();
        let gossip_ok = gossip_content.as_deref() == Some(GOSSIP_TEXT);
        report.add("Gossip E2EE 解密落库（密文正确解为明文）", gossip_ok, format!("content={gossip_content:?}"));

        // 文件消息 + 传输记录
        let file_msg: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages WHERE msg_id = ?1", rusqlite::params![format!("file-{TRANSFER_ID}")], |r| r.get(0))
            .unwrap_or(-1);
        report.add("文件消息落库（接收端生成消息行）", file_msg == 1, format!("count={file_msg}"));
        let transfer: Option<(String, f64)> = conn
            .query_row("SELECT status, progress FROM file_transfers WHERE id = ?1", rusqlite::params![TRANSFER_ID], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        let transfer_ok = transfer.as_ref().map(|(s, p)| s == "done" && *p >= 1.0).unwrap_or(false);
        report.add("传输记录完成（status=done, progress=1.0）", transfer_ok, format!("{transfer:?}"));

        // 会话行
        let conv: i64 = conn
            .query_row("SELECT COUNT(*) FROM conversations WHERE id = ?1", rusqlite::params![PEER_ID], |r| r.get(0))
            .unwrap_or(-1);
        report.add("会话行创建（左侧列表可显示）", conv >= 1, format!("count={conv}"));

        // outbox 清空
        let outbox_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM outbox WHERE peer_id = ?1", rusqlite::params![PEER_ID], |r| r.get(0))
            .unwrap_or(-1);
        report.add("outbox 队列清空（补发后删除）", outbox_left == 0, format!("left={outbox_left}"));

        // 文件落盘内容一致
        let downloads = Path::new(db).parent().unwrap_or(Path::new(".")).join("downloads").join(FILE_NAME);
        let file_ok = std::fs::read(&downloads).map(|bytes| bytes == content).unwrap_or(false);
        report.add("接收文件完整落盘（内容逐字节一致）", file_ok, downloads.display().to_string());

        // ---- --full 落库校验 ----
        if full {
            // 代码 / 图片 / 大文本消息落库
            for (label, msg_id, expect) in [
                ("代码消息落库（kind=code）", CODE_MSG_ID, CODE_CONTENT),
                ("图片消息落库（kind=image）", IMAGE_MSG_ID, IMAGE_CONTENT),
            ] {
                let c: Option<String> = conn
                    .query_row("SELECT content FROM messages WHERE msg_id = ?1", rusqlite::params![msg_id], |r| r.get(0))
                    .ok();
                let ok = c.as_deref() == Some(expect);
                report.add(label, ok, format!("len={}", c.map(|v| v.len()).unwrap_or(0)));
            }
            let big_len: Option<i64> = conn
                .query_row("SELECT LENGTH(content) FROM messages WHERE msg_id = ?1", rusqlite::params![BIG_MSG_ID], |r| r.get(0))
                .ok();
            report.add("1MB 大文本落库（长度完整）", big_len == Some(BIG_SIZE as i64), format!("len={big_len:?}"));

            // 群消息落库 + 群会话行
            let group_content: Option<String> = conn
                .query_row("SELECT content FROM messages WHERE msg_id = ?1", rusqlite::params![GROUP_MSG_ID], |r| r.get(0))
                .ok();
            let group_ok = group_content.as_deref() == Some(GROUP_TEXT);
            report.add("群消息落库（group 会话）", group_ok, format!("content={group_content:?}"));
            let group_conv: i64 = conn
                .query_row("SELECT COUNT(*) FROM conversations WHERE id = ?1", rusqlite::params![format!("group:{GROUP_ID}")], |r| r.get(0))
                .unwrap_or(-1);
            report.add("群会话行创建（群聊列表可显示）", group_conv == 1, format!("count={group_conv}"));

            // 乱序消息 ts 保真（DB 保留发送方时间戳）
            let ts_a: Option<i64> = conn
                .query_row("SELECT ts FROM messages WHERE msg_id = ?1", rusqlite::params![OOO_A_ID], |r| r.get(0))
                .ok();
            let ts_b: Option<i64> = conn
                .query_row("SELECT ts FROM messages WHERE msg_id = ?1", rusqlite::params![OOO_B_ID], |r| r.get(0))
                .ok();
            let ordered = matches!((ts_a, ts_b), (Some(a), Some(b)) if a > b);
            report.add("乱序消息 ts 保真（DB 按发送 ts 存储）", ordered, format!("a={ts_a:?} b={ts_b:?}"));
        }
    }

    let ok = report.print();
    std::process::exit(if ok { 0 } else { 1 });
}
