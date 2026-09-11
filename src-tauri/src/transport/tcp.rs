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

/// 读出一帧的 payload（不含长度前缀）。
pub async fn read_bytes<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME {
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

    /// **关键兼容性护栏**：bytes 抽象必须与现有 `write_frame` 字节级一致。
    /// 一旦不一致，新旧客户端就无法互通 —— 属于协议语义变更。
    #[tokio::test]
    async fn wire_format_matches_legacy_write_frame() {
        let msg = Message::Heartbeat {
            device_id: "dev-1".into(),
        };

        let mut legacy = Vec::new();
        crate::network::transport::write_frame(&mut legacy, &msg)
            .await
            .unwrap();

        let json = serde_json::to_vec(&msg).unwrap();
        let mut new = Vec::new();
        write_bytes(&mut new, &json).await.unwrap();

        assert_eq!(
            legacy, new,
            "bytes 抽象的线格式必须与现有 write_frame 完全一致"
        );
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
}
