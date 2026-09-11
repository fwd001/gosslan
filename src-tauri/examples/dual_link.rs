//! 双连接（Phase 6b + M3-b）验证对端：**同一 device_id 从两个不同端点**连入同一实例，
//! 并验证「切断被选中那条 → 消息仍送达」这条**消息级** failover 判据（ADR-0014 §8）。
//!
//! ## 为什么能拿到「消息级」判据（关键设计）
//! 只有当实例**主动发一条定向消息**给我们时，才真的走了 `try_send`（M3-b 的选路路径）。
//! 但实例只给**好友**发消息，而本示例无法成为好友（需要人工点同意或改库）。
//! 找到的合法触发点是：**非好友单聊** —— 实例在 `is_friend` 判定失败时会用 `try_send`
//! 回一条 `Message::FriendMessageBlocked`（见 `network/transport.rs` 的非好友分支，
//! 且发生在解密**之前**，所以内容可以是任意垃圾）。这条回复：
//!   · 是**定向**的（只走一条链路，走 M3-b 选路）⇒ 可用来判定"实例选了哪条"；
//!   · 不需要好友关系、不需要合法密文 ⇒ 本示例可独立触发。
//!
//! ## 判据（三步，含对照）
//! 1. 两条链路都建立后发一条单聊 → **只有被选中的那条**收到 `FriendMessageBlocked`；
//!    **对照**：另一条在短窗口内**不得**收到 —— 否则说明是播发而非定向，判据不成立；
//! 2. **切断被选中的那条**；
//! 3. 在幸存链路上再发一条单聊 → 必须**仍收到** `FriendMessageBlocked`
//!    ⇒ 实例把定向发送切到了幸存链路 = **真 failover**。
//!
//! ## 局限（如实标注，不假装覆盖）
//! 两条链路对本示例都是 LAN（loopback 与私网地址都判为 `PathKind::Lan`），所以本示例验证的是
//! **failover（断一条仍送达）**；**选路优先级 LAN > Routed** 由
//! `network::transport::route_order_*` 单测覆盖（那里能构造 Routed 端点）。
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
use gosslan_lib::protocol::{hello_signing_bytes, Message, MsgKind};
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
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "空帧"));
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// 建立一条连接并完成 Hello 握手，返回 (读半, 写半, **对端（实例）的 device_id**)。
///
/// 学到实例的 device_id 是必需的：单聊帧的 `to` 必须等于它，否则实例会静默丢弃
/// （`to != state.device_id → return`），我们也就拿不到那条定向回复。
async fn connect_and_hello(
    endpoint: &str,
    device_id: &str,
    id: &Identity,
) -> Result<
    (
        tokio::net::tcp::OwnedReadHalf,
        tokio::net::tcp::OwnedWriteHalf,
        String,
    ),
    String,
> {
    let stream = TcpStream::connect(endpoint)
        .await
        .map_err(|e| format!("连接 {endpoint} 失败: {e}"))?;

    let nonce = format!("nonce-{}", now_ms());
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
        Ok(Ok(Message::Hello { device_id, .. })) => Ok((r, w, device_id)),
        Ok(Ok(other)) => Err(format!("握手首帧不是 Hello：{other:?}")),
        Ok(Err(e)) => Err(format!("读取对端帧失败（Hello 可能被拒）: {e}")),
        Err(_) => Err("握手超时：5s 内未收到对端任何帧".to_string()),
    }
}

/// 发出一条**单聊**帧（内容任意：实例对非好友在解密前就会回 `FriendMessageBlocked`）。
async fn send_chat<W: AsyncWrite + Unpin>(
    w: &mut W,
    my_id: &str,
    app_id: &str,
    seq: i64,
) -> Result<(), String> {
    let msg = Message::ChatMessage {
        msg_id: format!("dual-link-{}-{seq}", now_ms()),
        from: my_id.to_string(),
        to: app_id.to_string(),
        kind: MsgKind::Text,
        content: "dual-link-probe".to_string(),
        ts: now_ms(),
        seq,
    };
    write_msg(w, &msg)
        .await
        .map_err(|e| format!("发送单聊失败: {e}"))
}

/// 等待 `FriendMessageBlocked`；跳过心跳等无关帧。
/// `Ok(true)` = 收到；`Ok(false)` = 窗口内没收到；`Err` = 连接出错。
async fn wait_blocked<R: AsyncRead + Unpin>(r: &mut R, secs: u64) -> Result<bool, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        match tokio::time::timeout(left, read_msg(r)).await {
            Ok(Ok(Message::FriendMessageBlocked { .. })) => return Ok(true),
            Ok(Ok(_)) => continue, // 心跳 / 其它帧：继续等
            Ok(Err(e)) => return Err(format!("读帧出错: {e}")),
            Err(_) => return Ok(false),
        }
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

    println!("[1/5] 端点1 = {e1}");
    println!("      端点2 = {e2}   （同一 device_id={device_id}，不同端点）");
    let (r1, w1, app1) = connect_and_hello(&e1, &device_id, &id).await?;
    println!("      ✓ 端点1 握手成功（实例 device_id={app1}）");
    let (r2, w2, app2) = connect_and_hello(&e2, &device_id, &id).await?;
    if app1 != app2 {
        return Err(format!(
            "INCONCLUSIVE | 两个端点连到的不是同一个实例（{app1} vs {app2}）"
        ));
    }
    println!("      ✓ 端点2 握手成功 —— 对端应为该 peer 建立 2 条连接");

    // 两半都放进 Option：步骤 4 要按"被选中的一侧"整体丢弃，Option::take 才好表达
    // （Rust 的移动语义下，直接 drop 之后再引用另一分支会被借用检查拒绝）。
    let (mut r1, mut w1) = (Some(r1), Some(w1));
    let (mut r2, mut w2) = (Some(r2), Some(w2));

    // ---- 步骤 2：发单聊，看实例把定向回复发到哪条 ----
    println!("[2/5] 端点1 发一条单聊，观察实例的定向回复走哪条链路…");
    send_chat(w1.as_mut().unwrap(), &device_id, &app1, 1).await?;
    let (selected, got) = tokio::select! {
        got = wait_blocked(r1.as_mut().unwrap(), 8) => (0usize, got),
        got = wait_blocked(r2.as_mut().unwrap(), 8) => (1usize, got),
    };
    match got {
        Ok(true) => println!(
            "      ✓ 实例选中了**端点{}**（该链路收到 FriendMessageBlocked）",
            selected + 1
        ),
        Ok(false) => {
            return Err(
                "FAIL | 8s 内两条链路都没收到 FriendMessageBlocked —— 实例没有回定向帧\
                 （实例未运行 / 版本不同 / 单聊分支被改）"
                    .to_string(),
            )
        }
        Err(e) => return Err(format!("INCONCLUSIVE | 端点{} 读帧出错: {e}", selected + 1)),
    }

    // ---- 步骤 3：对照 —— 另一条**不得**收到定向帧 ----
    //
    // 没有这一步，「收到回复」证明不了"定向"：若实例把回复播发到所有链路，
    // 那么「切断被选中那条、另一条仍收到」就毫无意义（本来就都收得到）。
    println!("[3/5] 对照：另一条链路在 2s 内**不得**收到定向帧…");
    let other = if selected == 0 { r2.as_mut().unwrap() } else { r1.as_mut().unwrap() };
    match wait_blocked(other, 2).await {
        Ok(false) => println!("      ✓ 对照成立：未被选中的那条没有收到定向帧（确实是定向投递）"),
        Ok(true) => {
            return Err(
                "INCONCLUSIVE | 两条链路都收到了定向帧 ⇒ 实例是播发而非定向，本判据不成立"
                    .to_string(),
            )
        }
        Err(e) => return Err(format!("INCONCLUSIVE | 对照链路读帧出错: {e}")),
    }

    // ---- 步骤 4：切断被选中那条，验证幸存链路上仍能送达 ----
    println!("[4/5] 切断被选中的**端点{}**，只留另一条…", selected + 1);
    if selected == 0 {
        w1.take();
        r1.take();
    } else {
        w2.take();
        r2.take();
    }
    // 给实例一点时间感知 FIN：读循环报错 → 摘掉该链路（或 writer 先因 channel 关闭退出）。
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    println!("[5/5] 在幸存链路上再发一条单聊，必须**仍收到**定向回复…");
    let (survivor_r, survivor_w) = if selected == 0 {
        (r2.as_mut().unwrap(), w2.as_mut().unwrap())
    } else {
        (r1.as_mut().unwrap(), w1.as_mut().unwrap())
    };
    send_chat(survivor_w, &device_id, &app2, 2).await?;
    match wait_blocked(survivor_r, 10).await {
        Ok(true) => {
            println!();
            println!("PASS | 消息级 failover 成立：");
            println!("       · 实例的定向回复确实只走被选中的一条（对照通过）；");
            println!("       · 切断那条之后，在幸存链路上发单聊**仍收到定向回复**；");
            println!("       · ⇒ try_send 的选路真的切到了存活链路（不是投进死路）。");
            println!();
            println!("判据范围：failover（断一条仍送达）。选路**优先级**（LAN > Routed）由");
            println!("`route_order_*` 单测覆盖 —— 本示例两条链路都是 LAN。");
            Ok(())
        }
        Ok(false) => Err(
            "FAIL | 切断被选中链路后，幸存链路上 10s 内没有收到定向回复\
             ⇒ 实例没有把定向发送切到存活链路（这正是 M3-b 要解决的问题）"
                .to_string(),
        ),
        Err(e) => Err(format!("INCONCLUSIVE | 幸存链路读帧出错: {e}")),
    }
}
