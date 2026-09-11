//! P1-2「镜像重复拨号」的真机级决定性验证对端（含对照实验）。
//!
//! ## 要复现的场景
//!
//! 被动方（被验证的实例）会同时做两件事：
//! 1. **接受**大 ID 拨进来的连接 —— 此时 `handle_incoming` 记录的
//!    `Link.endpoint` 是 TCP **源地址（临时端口）**；
//! 2. 之后收到该 peer 的 UDP `announce` —— `ensure_link` 拿到的是 announce
//!    自报的**监听地址**。
//!
//! 两个地址永不相等 ⇒ 只按端点判的旧实现认为「还没连上」，于是**反向再拨一条**，
//! 同一对节点稳定停留 2 条镜像 TCP。修好后（判据提升为「有没有连接」）不再拨。
//!
//! ## 两个对端：被测对象 + 对照
//!
//! | device_id | 是否先拨入实例 | 期望 `+conn` 条数 | 作用 |
//! |---|---|---|---|
//! | `aaa-mirror-peer` | **是**（建立 has_any_link） | 新代码 **1** / 旧代码 **2** | 被测对象 |
//! | `aab-control-peer` | 否 | **恒 1**（两端代码都一样） | **对照**：证明 announce 真的送达实例、且 announce→ensure_link→拨号链路是活的 |
//!
//! 没有对照的话，「没有第 2 条连接」也可能只是 announce 根本没送到 —— 那会把
//! 「没触发」误读成「没复现」。对照 peer 从未连接过，因此**任何**实现都会拨它。
//!
//! 两个 id 都以 `aa` 开头 —— 字典序恒小于项目的 `gosslan-*` 前缀，所以被验证的
//! 实例一定是「大 ID」，旧实现收到 announce 时**立即**拨号（不必等 10s 兜底阈值）。
//!
//! 用法（需先启动实例，建议 `--instance 1`，TCP 60002）：
//! ```bash
//! cargo run --example mirror_dial -- 60002 61999
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

// 注意：lib 名是 `gosslan_lib`（Cargo.toml `[lib] name`），不是 `gosslan`。
use gosslan_lib::crypto::Identity;
use gosslan_lib::protocol::{hello_signing_bytes, Message, UdpPacket, UDP_PORT};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

const SUBJECT_ID: &str = "aaa-mirror-peer";
const CONTROL_ID: &str = "aab-control-peer";

/// 分帧：`4 字节大端长度 + JSON`（`network::transport` 是私有模块，此处自包含一份）。
async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, m: &Message) -> std::io::Result<()> {
    let json = serde_json::to_vec(m)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    w.write_all(&(json.len() as u32).to_be_bytes()).await?;
    w.write_all(&json).await?;
    Ok(())
}

async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Message> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "空帧"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// 拨入实例并完成 Hello 握手（返回写半以便保持连接不关）。
async fn dial_in(endpoint: &str, id: &Identity) -> Result<tokio::net::tcp::OwnedWriteHalf, String> {
    let stream = TcpStream::connect(endpoint)
        .await
        .map_err(|e| format!("连接 {endpoint} 失败: {e}"))?;
    let nonce = format!(
        "nonce-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let x25519 = id.x25519_public_b64();
    let ed25519 = id.ed25519_public_b64();
    let sig = id.sign_b64(&hello_signing_bytes(
        SUBJECT_ID, 0, &nonce, &x25519, &ed25519,
    ));
    let hello = Message::Hello {
        device_id: SUBJECT_ID.to_string(),
        nickname: "mirror-dial".to_string(),
        avatar: None,
        device_type: "desktop".to_string(),
        tcp_port: 0,
        x25519_pubkey: x25519,
        ed25519_pubkey: ed25519,
        conv_clock: 0,
        nonce,
        sig,
    };
    let (mut r, mut w) = stream.into_split();
    write_msg(&mut w, &hello)
        .await
        .map_err(|e| format!("发送 Hello 失败: {e}"))?;
    // 等实例回发的 Hello（「握手补全」）—— 收到即证明链路已被接受。
    match tokio::time::timeout(Duration::from_secs(5), read_msg(&mut r)).await {
        Ok(Ok(_)) => Ok(w),
        Ok(Err(e)) => Err(format!("读取对端帧失败（Hello 可能被拒）: {e}")),
        Err(_) => Err("握手超时：5s 内未收到对端任何帧".to_string()),
    }
}

/// announce 的单播目标：实例在 Auto 模式下把 UDP socket 绑在**真实 LAN IP** 上，
/// 因此必须单播到各候选 IP（同 `e2e_peer` 的做法），广播/回环仅作兜底。
fn announce_targets() -> Vec<String> {
    let mut t = vec!["127.0.0.1".to_string()];
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in &ifs {
            if let if_addrs::IfAddr::V4(v4) = &i.addr {
                if i.ip().is_loopback() {
                    continue;
                }
                // 只挑有广播地址的接口（排除 utun 等点对点隧道）
                if v4.broadcast.is_some() {
                    t.push(i.ip().to_string());
                }
            }
        }
    }
    t.push("255.255.255.255".to_string());
    t
}

fn announce_packet(device_id: &str, id: &Identity, tcp_port: u16) -> UdpPacket {
    UdpPacket {
        kind: "announce".to_string(),
        device_id: device_id.to_string(),
        nickname: "mirror-dial".to_string(),
        tcp_port,
        x25519_pubkey: Some(id.x25519_public_b64()),
        ed25519_pubkey: Some(id.ed25519_public_b64()),
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let tcp_port: u16 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "60002".into())
        .parse()
        .map_err(|_| "实例 TCP 端口不合法".to_string())?;
    let fake_listen: u16 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "61999".into())
        .parse()
        .map_err(|_| "announce 自报端口不合法".to_string())?;

    let id = Identity::generate();

    // ---- 步骤 1：被测对端先拨入实例，建立一条真实连接（实例侧 has_any_link = true）----
    let _w = dial_in(&format!("127.0.0.1:{tcp_port}"), &id).await?;
    println!("[1/4] 被测对端 {SUBJECT_ID} 已拨入 127.0.0.1:{tcp_port} 并完成 Hello 握手");
    println!("      → 实例侧该 peer 现在已有 1 条连接（端点 = 本进程的 TCP 临时端口）");

    // ---- 步骤 2：起监听端口，代表 announce 自报的监听端口（拨号打过来要能成功）----
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{fake_listen}"))
        .await
        .map_err(|e| format!("监听 {fake_listen} 失败: {e}"))?;
    tokio::spawn(async move {
        // 接受并把连接**保持**住（连接一断实例会记 -conn，干扰判定）
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    println!("[2/4] 假对端监听 0.0.0.0:{fake_listen}（作为 announce 自报端口）");

    // ---- 步骤 3：周期发 announce（被测 + 对照），自报与已记录端点不同的监听端口 ----
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    sock.set_broadcast(true).ok();
    let subject = serde_json::to_vec(&announce_packet(SUBJECT_ID, &id, fake_listen))
        .map_err(|e| e.to_string())?;
    let control = serde_json::to_vec(&announce_packet(CONTROL_ID, &id, fake_listen))
        .map_err(|e| e.to_string())?;
    let targets = announce_targets();
    println!("[3/4] 发 announce（自报 tcp_port={fake_listen}）→ {targets:?}:{UDP_PORT}");
    println!("      · 被测 {SUBJECT_ID}：已连接，期望**不再**拨（修好后）");
    println!("      · 对照 {CONTROL_ID}：从未连接，**必然**会拨 —— 证明 announce 送达且链路是活的");
    let deadline = tokio::time::Instant::now() + Duration::from_millis(7000);
    while tokio::time::Instant::now() < deadline {
        for ip in &targets {
            let _ = sock.send_to(&subject, (ip.as_str(), UDP_PORT)).await;
            let _ = sock.send_to(&control, (ip.as_str(), UDP_PORT)).await;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    // ---- 步骤 4：留时间让实例完成（或不完成）拨号 ----
    println!("[4/4] 观察 6s，等待实例是否产生镜像拨号…");
    tokio::time::sleep(Duration::from_millis(6000)).await;

    println!("完成。请由脚本按实例日志判定：对照 1 条（否则说明 announce 未送达）／被测 1 条 = PASS、2 条 = 复现镜像 bug。");
    Ok(())
}
