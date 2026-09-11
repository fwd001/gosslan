//! 双连接（Phase 6b）验证对端：**同一 device_id 从两个不同端点**连入同一实例。
//!
//! 验证 6b 的核心语义：
//! 1. 同一 peer 的两个端点 = 两条连接（不再是「一个 peer 只能有一条连接」）；
//! 2. **断开其中一条，另一条仍然存活**；
//! 3. 断一条**不会**让对端把本节点整条删掉（旧实现会 `remove(peer_id)` 全删）。
//!
//! 判据：实例每 **5 秒**向 `links` 里的每条连接发一次 Heartbeat。
//! 所以「断开端点1 后端点2 仍能收到帧」= 实例仍在维护该 peer 的另一条连接。
//! 若对端把整个 peer 删了，端点2 就再也收不到任何帧。
//!
//! 用法（需先启动实例）：
//! ```bash
//! GOSSLAN_AUTOSTART=1 ./target/debug/gosslan --instance 1 &
//! cargo run --example dual_link -- 60002
//! ```
//! 第二个端点可用 `DUAL_LINK_IP2` 覆盖（默认取本机第一个非回环 IPv4）。

use std::time::{SystemTime, UNIX_EPOCH};

// 注意：lib 名是 `gosslan_lib`（Cargo.toml `[lib] name`），不是 `gosslan`。
use gosslan_lib::crypto::Identity;
use gosslan_lib::protocol::{hello_signing_bytes, Message};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// 分帧：`4 字节大端长度 + JSON`。与 `network::transport` 的线格式一致
/// （该模块是私有的，example 访问不到，故在此自包含一份）。
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
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "空帧",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// 本机第一个非回环 IPv4（用于构造「不同端点」）。
fn second_ip() -> Option<String> {
    if let Ok(forced) = std::env::var("DUAL_LINK_IP2") {
        return Some(forced);
    }
    if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .find(|i| {
            !i.is_loopback()
                && matches!(i.addr, if_addrs::IfAddr::V4(ref v) if !v.ip.is_link_local())
        })
        .map(|i| i.ip().to_string())
}

/// 建立一条连接并完成 Hello 握手（成功 = 能读到对端后续帧）。
async fn connect_and_hello(
    endpoint: &str,
    device_id: &str,
    id: &Identity,
) -> Result<(tokio::net::tcp::OwnedReadHalf, tokio::net::tcp::OwnedWriteHalf), String> {
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
    let sig = id.sign_b64(&hello_signing_bytes(device_id, 0, &nonce, &x25519, &ed25519));

    let hello = Message::Hello {
        device_id: device_id.to_string(),
        nickname: "dual-link".to_string(),
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

    match tokio::time::timeout(std::time::Duration::from_secs(5), read_msg(&mut r)).await {
        Ok(Ok(_)) => Ok((r, w)),
        Ok(Err(e)) => Err(format!("读取对端帧失败（Hello 可能被拒）: {e}")),
        Err(_) => Err("握手超时：5s 内未收到对端任何帧".to_string()),
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let port = std::env::args().nth(1).unwrap_or_else(|| "60002".into());
    let device_id = "dual-peer".to_string();
    let id = Identity::generate();

    let ip2 = second_ip().ok_or("找不到第二个（非回环）IPv4 端点，可用 DUAL_LINK_IP2 指定")?;
    let e1 = format!("127.0.0.1:{port}");
    let e2 = format!("{ip2}:{port}");

    println!("[1/4] 端点1 = {e1}");
    println!("[2/4] 端点2 = {e2}   （同一 device_id={device_id}，不同端点）");

    let (r1, w1) = connect_and_hello(&e1, &device_id, &id).await?;
    println!("     ✓ 端点1 握手成功");
    let (mut r2, _w2) = connect_and_hello(&e2, &device_id, &id).await?;
    println!("     ✓ 端点2 握手成功 —— 对端应为该 peer 建立 2 条连接");

    println!("[3/4] 断开端点1（只断这一条）…");
    drop(w1);
    drop(r1);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    println!("[4/4] 等端点2 的下一帧（实例每 5s 心跳一次，最多等 10s）…");
    match tokio::time::timeout(std::time::Duration::from_secs(10), read_msg(&mut r2)).await {
        Ok(Ok(m)) => {
            println!("PASS | 断开端点1 后端点2 仍收到帧：{m:?}");
            println!("     → 6b 双连接语义成立：断一条不影响另一条，也未把 peer 整条删除");
            Ok(())
        }
        Ok(Err(e)) => Err(format!("FAIL | 端点2 读帧出错: {e}")),
        Err(_) => Err(
            "FAIL | 10s 内端点2 未收到任何帧 —— 对端可能把整个 peer 删掉了（旧的单连接行为）"
                .to_string(),
        ),
    }
}
