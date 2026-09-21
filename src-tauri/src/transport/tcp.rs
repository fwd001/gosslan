//! TCP 传输：只懂 bytes（P-A03）。
//!
//! 本模块把「分帧」从「业务协议」里剥离出来：
//! - 帧格式 = **4 字节大端长度 + payload**，与既有 `network::transport::write_frame`
//!   字节级一致（有测试钉住）；
//! - 这里**不认识** `Message` / Gossip / ChatMessage / SQLite，只搬字节；
//! - 业务序列化（serde_json）留在上层，Transport 不做任何领域假设。
//!
//! ## 接线状态：**大部分已接线**（2026-09-16 逐项核对调用点，修正了此前的"旁路阶段"）
//!
//! | 项 | 状态 |
//! |---|---|
//! | `write_bytes` / `read_bytes` / `read_bytes_capped` | ✅ **已接线**：`network/transport.rs:57,62,69`（`read_bytes_capped` 传 `MAX_PREAUTH_FRAME`）；那边注释自述「单一真相源见 `transport::tcp`（P-A03）」 |
//! | `TcpReceiver` / `TcpSender` + 它们的 `AsyncRead` / `AsyncWrite` 实现 | ✅ **已接线**：`network/transport.rs:1231,1232,1529,1613,2368,2369` |
//! | `TcpTransport`（组合结构体） | ⚠️ **未接线**：只在本文件测试里用（它是"先拆半再交给 writer_loop / reader_loop"的旧形态） |
//!
//! ⚠️ **历史（2026-09-16 修正）**：本文件此前挂着**文件级** `#![allow(dead_code)]`
//! 并注明「旁路阶段：待接线后移除」—— 那句话只对 `TcpTransport` **一个结构体**成立，
//! 而文件里的帧原语**早已在跑**。文件级 allow 的坏处正是"把已经接线的事实也一起静音"
//! （同类问题已在 `ble_framing.rs`（Phase 3）与 `transport/bluetooth.rs`（Phase 5）各发现一次）。
//! 现改为**逐个标注**：只有 `TcpTransport` 及其 `impl` 带 `#[allow(dead_code)]`，其余交给编译器守。
//!
//! `TcpTransport` 之所以留着：把 `writer_loop` / `reader_loop` 换成端点类型时要用它
//! （只换类型、分帧逻辑不动 ⇒ 行为等价，见本文件测试
//! `split_halves_work_with_legacy_frame_helpers`）。

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use crate::protocol::MAX_FRAME;
use crate::transport::relay_seal::{OpenPipe, SealPipe};

/// 写入一帧：`4 字节大端长度 + payload`。
pub async fn write_bytes<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME || payload.len() > u32::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "非法载荷长度",
        ));
    }
    w.write_all(&(payload.len() as u32).to_be_bytes()).await?;
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
///
/// ⚠️ **当前无生产调用点**：`network/transport.rs` 用的是下面拆开的
/// [`TcpReceiver`] / [`TcpSender`]（它先拆半再交给两个并发任务，与
/// `writer_loop` / `reader_loop` 的形态一致）。本类型是"不拆半"的便捷形态，
/// 留作把那两个循环换成端点类型时使用 —— **只有它的 `impl` 需要 allow**，
/// 帧原语与端点类型都在跑（见模块头的接线状态表）。
#[allow(dead_code)]
pub struct TcpTransport {
    write: OwnedWriteHalf,
    read: OwnedReadHalf,
}

#[allow(dead_code)]
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
            TcpReceiver {
                read: Box::new(self.read),
                open: None,
            },
            TcpSender {
                write: Box::new(self.write),
                seal: None,
            },
        )
    }
}

/// 一条连接的**发送端**：只写字节；可选地套一层中继记录层封装。
///
/// `write` 是装箱的 `AsyncWrite` 而不是写死 `OwnedWriteHalf`，只有一个理由：
/// 记录层必须能在测试里对着 `tokio::io::duplex` 跑（真 socket 造不出"每次只能写 8 字节"
/// 的背压，而那正是 `SealPipe` 唯一会写错的地方 —— 字节被吞两遍或漏一遍）。
/// 对外类型仍是具体的 `TcpSender` ⇒ `writer_loop` 的签名一个字都不用改。
pub struct TcpSender {
    write: Box<dyn AsyncWrite + Unpin + Send>,
    /// `Some` = 这条链路的帧流必须再封一层（只有公网中继电路会启用，见 `relay_seal`）。
    seal: Option<SealPipe>,
}

impl TcpSender {
    pub fn new<W: AsyncWrite + Unpin + Send + 'static>(write: W) -> Self {
        Self {
            write: Box::new(write),
            seal: None,
        }
    }

    /// 启用记录层封装。**一旦启用不可撤销** —— 半程明文就是给中继留注入口子。
    pub fn enable_seal(&mut self, key: [u8; 32]) {
        self.seal = Some(SealPipe::new(key));
    }

    /// 直接把 bytes 写出去（`write_bytes` 的便捷包装）。
    ///
    /// ⚠️ 当前**只有本文件测试**在用：生产路径（`network/transport.rs`）走的是
    /// `write_frame(&mut sender, …)` —— 即通过 `TcpSender: AsyncWrite` 的实现，
    /// 而不是这个直接方法。留它是为了不经过业务序列化时也能写裸字节。
    #[allow(dead_code)]
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        write_bytes(&mut self.write, bytes).await
    }
}

/// 委托底层写半（未启用记录层时字节级等价于此前的实现）。
///
/// 启用记录层后的关键约束：**要么整批认下（`Ok(buf.len())`），要么 `Pending`**，
/// 绝不返回"吞了一半"。因为 `write_all` 在 `Pending` 后会拿同一个切片重试，
/// 而 `SealPipe::absorb` 已经幂等地记过"这批吞过了"（`staged`），所以既不会少一遍
/// 也不会多一遍。少这条判据的后果是帧错位 —— 整条链路解不开。
impl AsyncWrite for TcpSender {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        // 按字段解构：`seal` 与 `write` 是两个不相交的字段，否则借用检查会当成
        // 同时可变借用整个 self。
        let TcpSender { write, seal } = self.get_mut();
        let Some(pipe) = seal.as_mut() else {
            return Pin::new(write.as_mut()).poll_write(cx, buf);
        };
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // ① 先把上一批遗留的密文推干净；推不完就 Pending，且**不吞**本次字节。
        while pipe.has_out() {
            match Pin::new(write.as_mut()).poll_write(cx, pipe.out_slice()) {
                Poll::Ready(Ok(0)) => return Poll::Ready(Err(write_zero())),
                Poll::Ready(Ok(n)) => pipe.advance_out(n),
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        // ② 吸收本次字节（内部可能产出一条记录）。
        if let Err(e) = pipe.absorb(buf) {
            return Poll::Ready(Err(e));
        }
        // ③ 把密文写出去；写干净了才算这批被消费。
        loop {
            if !pipe.has_out() {
                pipe.clear_staged();
                return Poll::Ready(Ok(buf.len()));
            }
            match Pin::new(write.as_mut()).poll_write(cx, pipe.out_slice()) {
                Poll::Ready(Ok(0)) => return Poll::Ready(Err(write_zero())),
                Poll::Ready(Ok(n)) => pipe.advance_out(n),
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let TcpSender { write, seal } = self.get_mut();
        if let Some(pipe) = seal.as_mut() {
            while pipe.has_out() {
                match Pin::new(write.as_mut()).poll_write(cx, pipe.out_slice()) {
                    Poll::Ready(Ok(0)) => return Poll::Ready(Err(write_zero())),
                    Poll::Ready(Ok(n)) => pipe.advance_out(n),
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        }
        Pin::new(write.as_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(self.get_mut().write.as_mut()).poll_shutdown(cx)
    }
}

fn write_zero() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::WriteZero, "底层写半接受了 0 字节")
}

/// 一条连接的**接收端**：只读字节；可选地解一层中继记录层封装。
///
/// 装箱理由同 `TcpSender`。
pub struct TcpReceiver {
    read: Box<dyn AsyncRead + Unpin + Send>,
    open: Option<OpenPipe>,
}

impl TcpReceiver {
    pub fn new<R: AsyncRead + Unpin + Send + 'static>(read: R) -> Self {
        Self {
            read: Box::new(read),
            open: None,
        }
    }

    /// 启用记录层解密。**一旦启用不可撤销**（同 `TcpSender::enable_seal`）。
    pub fn enable_seal(&mut self, key: [u8; 32]) {
        self.open = Some(OpenPipe::new(key));
    }

    /// 直接读出一帧 bytes（`read_bytes` 的便捷包装）。
    ///
    /// ⚠️ 同 `TcpSender::send_bytes`：当前只有本文件测试在用，生产路径走 `read_frame`。
    #[allow(dead_code)]
    pub async fn receive_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_bytes(&mut self.read).await
    }
}

/// 单次从底层拉取的缓冲大小。记录层要在内部分组成帧，太小会syscall过多；
/// 也不能太大 —— 未认证的对端不得让我们一次分配大块内存。
const READ_SCRATCH: usize = 16 * 1024;

impl AsyncRead for TcpReceiver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let TcpReceiver { read, open } = self.get_mut();
        let Some(pipe) = open.as_mut() else {
            return Pin::new(read.as_mut()).poll_read(cx, buf);
        };
        // 已有明文先送明文：这样一次 socket 读能喂很多次 poll_read。
        // ⚠️ 必须 `advance(n)`：`initialize_unfilled()` 只是借出那块内存，
        // 不调 advance 的话 ReadBuf 的 filled 游标仍是 0，`read_exact` 会把"0 字节"
        // 当成 EOF ⇒ 密封链路上一帧都送不出去（这个 bug 是字节等价用例抓到的）。
        let n = pipe.take_plain(buf.initialize_unfilled());
        if n > 0 {
            buf.advance(n);
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut scratch = [0u8; READ_SCRATCH];
            let mut rb = tokio::io::ReadBuf::new(&mut scratch);
            let n = match Pin::new(read.as_mut()).poll_read(cx, &mut rb) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => rb.filled().len(),
                // 底层暂时没数据 ⇒ **就是 Pending**，不是截断。一条记录合法地横跨多次
                // socket 读（真机上 16KB 分片必然横跨），把"已攒半条 + 此刻无数据"判成
                // EOF 会让每条大帧链路刚起步就自杀 —— 这个 bug 是 duplex(1) 的用例抓到的。
                // 截断只在下面 n==0（底层真的 EOF）时判。
                Poll::Pending => return Poll::Pending,
            };
            if n == 0 {
                // 底层 EOF 且没有明文可交付 ⇒ 真 EOF（`read_exact` 自己判 UnexpectedEof）。
                if pipe.has_partial() {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "记录不完整",
                    )));
                }
                return Poll::Ready(Ok(()));
            }
            if let Err(e) = pipe.absorb_encrypted(&scratch[..n]) {
                return Poll::Ready(Err(e));
            }
            let n = pipe.take_plain(buf.initialize_unfilled());
            if n > 0 {
                buf.advance(n);
                return Poll::Ready(Ok(()));
            }
        }
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
        assert_eq!(
            read_bytes_capped(&mut reader, cap).await.unwrap().len(),
            cap
        );

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
            let got = crate::network::transport::read_frame(&mut rx)
                .await
                .unwrap();
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
        let reply = crate::network::transport::read_frame(&mut rx)
            .await
            .unwrap();

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
