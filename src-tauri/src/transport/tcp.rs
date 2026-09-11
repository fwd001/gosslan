//! TCP 传输：只懂 bytes（P-A03）。
//!
//! 本模块把「分帧」从「业务协议」里剥离出来：
//! - 帧格式 = **4 字节大端长度 + payload**，与现有 `network::transport::write_frame`
//!   完全一致（字节级，有测试钉住）；
//! - 这里**不认识** `Message` / Gossip / ChatMessage / SQLite，只搬字节；
//! - 业务序列化（serde_json）留在上层，Transport 不做任何领域假设。
//!
//! 因此新旧实现可以互通，协议语义不变（Phase 4 的硬约束）。
//!
//! Phase 4 当前只落地 bytes 原语，**不改变任何现有收发路径**；
//! 由后续步骤用 Adapter 接入 Hello 验签、双队列与 Windows socket 修复。

#![allow(dead_code)] // 旁路阶段：待接线后移除

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use crate::protocol::MAX_FRAME;

/// 写入一帧：`4 字节大端长度 + payload`。
pub async fn write_bytes<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME || payload.len() > u32::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "非法载荷长度",
        ));
    }
    w.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    w.write_all(payload).await?;
    Ok(())
}

/// 读出一帧的 payload（不含长度前缀），上限 `MAX_FRAME`。
pub async fn read_bytes<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    read_bytes_capped(r, MAX_FRAME).await
}

/// 同 `read_bytes`，但允许调用方指定更小的上限（预认证阶段用 `MAX_PREAUTH_FRAME`）。
///
/// ⚠️ 顺序很重要：**先校验长度前缀、再分配缓冲**。当前实现即是如此 ——
/// 反过来的话，一个 4 字节的"声明 64MiB"就足以让对端在本机先拿到一大块内存。
pub async fn read_bytes_capped<R: AsyncRead + Unpin>(
    r: &mut R,
    max: usize,
) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "非法帧长度",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

/// 一条 TCP 连接的 bytes 通道（读写半分离，便于与既有 writer/reader_loop 对齐）。
pub struct TcpTransport {
    write: OwnedWriteHalf,
    read: OwnedReadHalf,
}

impl TcpTransport {
    /// 接管一条已建立的 TCP 连接。
    pub fn new(stream: TcpStream) -> Self {
        let (read, write) = stream.into_split();
        Self { write, read }
    }

    /// 由已拆分的读写半构造（现有 `handle_incoming` 就是先拆半再使用）。
    pub fn from_parts(read: OwnedReadHalf, write: OwnedWriteHalf) -> Self {
        Self { write, read }
    }

    pub async fn send_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        write_bytes(&mut self.write, bytes).await
    }

    pub async fn receive_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_bytes(&mut self.read).await
    }

    /// 拆成独立的接收端 / 发送端。
    ///
    /// 必须拆：`writer_loop` 与 `reader_loop` 是两个并发任务，各自只持有一半
    /// （与既有 `OwnedReadHalf` / `OwnedWriteHalf` 的用法一致）。
    pub fn into_split(self) -> (TcpReceiver, TcpSender) {
        (
            TcpReceiver { read: self.read },
            TcpSender { write: self.write },
        )
    }
}

/// 一条 TCP 连接的**发送端**：只写字节。
pub struct TcpSender {
    write: OwnedWriteHalf,
}

impl TcpSender {
    pub fn new(write: OwnedWriteHalf) -> Self {
        Self { write }
    }

    pub async fn send_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        write_bytes(&mut self.write, bytes).await
    }
}

/// 委托底层写半：让 `TcpSender` 可直接喂给任何 `W: AsyncWrite` 的通用函数
/// （例如既有的 `write_frame`），不必为连接端点重写分帧代码。
impl AsyncWrite for TcpSender {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.write).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_shutdown(cx)
    }
}

/// 一条 TCP 连接的**接收端**：只读字节。
pub struct TcpReceiver {
    read: OwnedReadHalf,
}

impl TcpReceiver {
    pub fn new(read: OwnedReadHalf) -> Self {
        Self { read }
    }

    pub async fn receive_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_bytes(&mut self.read).await
    }
}

/// 同 `TcpSender`：实现 `AsyncRead` 以直接复用既有的 `read_frame`。
impl AsyncRead for TcpReceiver {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.read).poll_read(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Message;

    /// 帧头必须是 4 字节大端长度。
    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let payload = b"hello gosslan";
        let mut buf = Vec::new();
        write_bytes(&mut buf, payload).await.unwrap();

        assert_eq!(&buf[..4], &(payload.len() as u32).to_be_bytes());
        let mut reader = &buf[..];
        assert_eq!(read_bytes(&mut reader).await.unwrap(), payload);
    }

    /// 预认证阶段的小上限必须真的生效：长度前缀声明得比上限大 ⇒ 立刻拒绝，
    /// **且不会先去分配那块缓冲**（先校验、后分配；顺序反了就是一个 4 字节的
    /// "声明 64MiB" 就能让本机先拿到一大块内存）。
    #[tokio::test]
    async fn capped_read_rejects_oversized_length_prefix() {
        let cap = 1024usize;
        // 只给 4 字节长度前缀（声明 1MiB），后面没有数据 —— 正确实现应当在分配前就返回 Err
        let mut reader = &(1u32 << 20).to_be_bytes()[..];
        let err = read_bytes_capped(&mut reader, cap).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        // 恰好等于上限：允许
        let payload = vec![1u8; cap];
        let mut buf = Vec::new();
        write_bytes(&mut buf, &payload).await.unwrap();
        let mut reader = &buf[..];
        assert_eq!(read_bytes_capped(&mut reader, cap).await.unwrap().len(), cap);

        // 比上限多 1 字节：拒绝（边界是闭区间上限，与项目其它长度判据一致）
        let payload = vec![1u8; cap + 1];
        let mut buf = Vec::new();
        write_bytes(&mut buf, &payload).await.unwrap();
        let mut reader = &buf[..];
        assert!(read_bytes_capped(&mut reader, cap).await.is_err());
    }

    /// 大载荷（接近 MAX_FRAME 上限）也要能往返。
    #[tokio::test]
    async fn roundtrip_large_payload() {
        let payload = vec![7u8; 1 << 20]; // 1 MiB
        let mut buf = Vec::new();
        write_bytes(&mut buf, &payload).await.unwrap();

        let mut reader = &buf[..];
        assert_eq!(read_bytes(&mut reader).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn empty_payload_is_rejected_on_write() {
        let mut buf = Vec::new();
        assert!(write_bytes(&mut buf, b"").await.is_err());
    }

    #[tokio::test]
    async fn oversized_payload_is_rejected_on_write() {
        let big = vec![0u8; MAX_FRAME as usize + 1];
        let mut buf = Vec::new();
        assert!(write_bytes(&mut buf, &big).await.is_err());
    }

    /// 读侧同样要拒绝非法长度（防止恶意对端用巨大长度前缀打爆内存）。
    #[tokio::test]
    async fn illegal_length_is_rejected_on_read() {
        let zero = 0u32.to_be_bytes();
        let mut r = &zero[..];
        assert!(read_bytes(&mut r).await.is_err());

        let too_big = ((MAX_FRAME as u32) + 1).to_be_bytes();
        let mut r = &too_big[..];
        assert!(read_bytes(&mut r).await.is_err());
    }

    /// **关键协议护栏**：线上帧格式必须是「4 字节大端长度 + payload」。
    ///
    /// 本测试最初用于比对「新旧两个实现是否字节级一致」；Phase 4 第二步之后
    /// `network::transport::write_frame` 已复用本模块的 bytes 原语，两者合流，
    /// 因此改为直接钉住线格式本身——一旦有人改了长度前缀的字节序 / 宽度，
    /// 或改动了 payload 的摆放，这里立刻失败（那属于协议语义变更）。
    #[tokio::test]
    async fn wire_format_is_length_prefixed_big_endian() {
        let msg = Message::Heartbeat {
            device_id: "dev-1".into(),
        };
        let json = serde_json::to_vec(&msg).unwrap();

        let mut buf = Vec::new();
        crate::network::transport::write_frame(&mut buf, &msg)
            .await
            .unwrap();

        assert_eq!(
            &buf[..4],
            &(json.len() as u32).to_be_bytes(),
            "帧头必须是 4 字节大端长度"
        );
        assert_eq!(&buf[4..], json.as_slice(), "帧体必须是原始 payload");
    }

    /// 真实回环 TCP 上验证 send/receive_bytes（不需要业务协议）。
    #[tokio::test]
    async fn tcp_transport_roundtrip_over_loopback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut t = TcpTransport::new(stream);
            let got = t.receive_bytes().await.unwrap();
            t.send_bytes(b"pong").await.unwrap();
            got
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut t = TcpTransport::new(stream);
        t.send_bytes(b"ping").await.unwrap();
        assert_eq!(t.receive_bytes().await.unwrap(), b"pong");

        assert_eq!(server.await.unwrap(), b"ping");
    }

    /// 拆半后仍能被既有 `write_frame` / `read_frame` 直接使用
    /// （因为 `TcpSender: AsyncWrite`、`TcpReceiver: AsyncRead`）。
    ///
    /// 这是下一步把 writer_loop / reader_loop 换成端点类型的前提：
    /// 只换类型、分帧逻辑不动，因此行为等价。
    #[tokio::test]
    async fn split_halves_work_with_legacy_frame_helpers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rx, mut tx) = TcpTransport::new(stream).into_split();
            let got = crate::network::transport::read_frame(&mut rx).await.unwrap();
            crate::network::transport::write_frame(
                &mut tx,
                &Message::Heartbeat {
                    device_id: "srv".into(),
                },
            )
            .await
            .unwrap();
            got
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let (mut rx, mut tx) = TcpTransport::new(stream).into_split();
        crate::network::transport::write_frame(
            &mut tx,
            &Message::Heartbeat {
                device_id: "cli".into(),
            },
        )
        .await
        .unwrap();
        let reply = crate::network::transport::read_frame(&mut rx).await.unwrap();

        assert!(
            matches!(reply, Message::Heartbeat { ref device_id } if device_id == "srv"),
            "收到: {reply:?}"
        );
        assert!(
            matches!(server.await.unwrap(), Message::Heartbeat { ref device_id } if device_id == "cli")
        );
    }

    /// 拆半后 bytes 层接口（send_bytes / receive_bytes）同样可用。
    #[tokio::test]
    async fn split_halves_send_receive_raw_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rx, _tx) = TcpTransport::new(stream).into_split();
            rx.receive_bytes().await.unwrap()
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let (_rx, mut tx) = TcpTransport::new(stream).into_split();
        tx.send_bytes(b"raw-bytes").await.unwrap();

        assert_eq!(server.await.unwrap(), b"raw-bytes");
    }
}
