//! BLE 消息分片 / 重组（ADR-0015 的 7-b 字节层）。
//!
//! ## 为什么需要这一层
//! GATT 不是字节流：写特征要按 MTU 切、notify 每次只送一小段（默认 MTU 23 ⇒ 有效载荷
//! 20 字节，协商后常见 185/512）。而我们的协议帧是**长度前缀的整帧**（`transport::tcp`
//! 那套 4 字节大端长度 + payload），最大可到几十 KB（文件分片 base64 后约 342KB）。
//! 所以必须自己分片与重组 —— 这正是 `writer_loop`/`reader_loop` 不能原样复用的原因。
//!
//! ## 为什么单独成一个模块（而不是直接写在 btleplug 驱动里）
//! ① 分片/重组是**纯逻辑**，与具体蓝牙后端无关 ⇒ 可以在没有射频、没有依赖的情况下
//!    用单测把所有边界钉死（乱序、丢片、重复、超长、超时、并发多消息）；
//! ② 等接入 `btleplug` 时，驱动只需把 notify 收到的字节喂进 [`BleReassembler`]、
//!    把要发的整帧交给 [`fragment`] —— 驱动本身薄到几乎不可能出错。
//!
//! ## 分片格式
//! 每片 = `6 字节头 + 数据`：
//! ```text
//! [0..2) msg_id   u16 大端：同一条消息的所有分片共用（由发送方按连接内递增分配）
//! [2..4) index    u16 大端：分片序号，从 0 开始
//! [4..6) count    u16 大端：总分片数（≥1）
//! [6..)  data     该片携带的字节
//! ```
//! 头里的 `count` 让接收方**不需要**额外信令就知道何时收齐（BLE 没有"消息结束"事件）。
//!
//! ## 有意不做的
//! - 不做重传/确认：GATT 的写与 notify 本身是有确认链路的（链路层保证顺序与重传），
//!   真丢包发生在**断连**，那时整条连接都要重建，重传没有意义；
//! - 不做压缩/加密：上层 `Message` 已序列化，E2EE 在业务层完成，这一层只搬字节。
//!
//! ## 安全边界（对端不可信）
//! 分片头由对端给出，所以三条限制必须在**分配内存之前**判断：
//! ① 单条消息不超过 [`MAX_BLE_MESSAGE_BYTES`]；② 总分片数不超过
//! [`MAX_BLE_CHUNKS_PER_MESSAGE`]；③ 同时进行的不完整消息不超过 [`MAX_INFLIGHT_MESSAGES`]。
//! 另外不完整消息有 TTL（[`PARTIAL_TTL_MS`]），断连后残留的分片会被回收。

// 旁路阶段：本模块是 BLE 的**字节层编解码**，驱动（btleplug）接入前没有任何生产调用点。
// 与 `transport/tcp.rs` 顶部同样的处理：先让编译保持 0 warning，接线后删除这一行
// （项目里"pub 项不报 dead_code"不是理由 —— 这条 allow 就是明说"还没接线"）。
// 接线点见 ADR-0015：`BluetoothTransport` 的 start/send/broadcast 改为走真实 GATT。
#![allow(dead_code)]

use std::collections::HashMap;

/// 分片头长度（字节）。
pub const BLE_CHUNK_HEADER_LEN: usize = 6;

/// 单条 BLE 消息的字节上限。
///
/// 取 512KiB：够装下一个文件分片（256KiB 原始 ⇒ base64 后约 342KB）与常见聊天帧，
/// 又远小于 TCP 侧的 `MAX_FRAME`（64MiB）—— BLE 带宽只有几十 KB/s，
/// 允许更大的消息只会把一条连接堵死几十秒。
pub const MAX_BLE_MESSAGE_BYTES: usize = 512 * 1024;

/// 单条消息的分片数上限（防御 `count` 字段撒谎）。
///
/// 8192 对合法流量绰绰有余：协商 MTU 185 时能覆盖 1.4MB（远超 512KiB 的字节上限），
/// 只有 MTU 极小的链路才会先撞到它，而那种链路上传 512KiB 本来就要几十分钟。
pub const MAX_BLE_CHUNKS_PER_MESSAGE: usize = 8192;

/// 同时进行的不完整消息数上限。
///
/// 一条 BLE 链路的 notify 是**有序单流**，正常只有 1~2 条消息在重组中；
/// 取 8 是为了给"乱序/重传导致短暂交错"留余量，同时把最坏内存钉住：
/// 每条消息最多 `8192 个已收分片 × ~48B 条目 + 512KiB payload ≈ 0.9MB`，
/// 8 条 ≈ 7MB 上限 —— 而且只有**已通过 Hello 验签的对端**才可能触发。
pub const MAX_INFLIGHT_MESSAGES: usize = 8;

/// 不完整消息的存活时间：超过即整条丢弃（断连/对端消失时不会永久占内存）。
pub const PARTIAL_TTL_MS: i64 = 30_000;

/// 把一条完整 payload 切成 BLE 分片。
///
/// `mtu` 是**有效载荷**上限（已扣掉 GATT 的 3 字节 ATT 头），必须能容纳头 + 至少 1 字节数据。
/// 返回 `None` 表示参数非法或消息过大（调用方据此拒绝发送，而不是切出一堆坏分片）。
pub fn fragment(payload: &[u8], mtu: usize, msg_id: u16) -> Option<Vec<Vec<u8>>> {
    if payload.is_empty() || payload.len() > MAX_BLE_MESSAGE_BYTES {
        return None;
    }
    // 至少要能装下"头 + 1 字节"
    if mtu <= BLE_CHUNK_HEADER_LEN {
        return None;
    }
    let per_chunk = mtu - BLE_CHUNK_HEADER_LEN;
    let count = payload.len().div_ceil(per_chunk);
    if count > MAX_BLE_CHUNKS_PER_MESSAGE || count > u16::MAX as usize {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for (i, data) in payload.chunks(per_chunk).enumerate() {
        let mut chunk = Vec::with_capacity(BLE_CHUNK_HEADER_LEN + data.len());
        chunk.extend_from_slice(&msg_id.to_be_bytes());
        chunk.extend_from_slice(&(i as u16).to_be_bytes());
        chunk.extend_from_slice(&(count as u16).to_be_bytes());
        chunk.extend_from_slice(data);
        out.push(chunk);
    }
    Some(out)
}

/// 一条正在重组中的消息。
///
/// ⚠️ 分片存在 **map 里而不是按 `count` 预分配的 Vec**：`count` 由对端给出，
/// 若拿它 `vec![None; count]` 就等于让对端决定我们分配多少（`count=65535` 一次 1.5MB）。
/// 惰性存储让内存只随**真实收到的字节**增长，另由 `bytes` 上限兜住总量。
struct Partial {
    chunks: HashMap<usize, Vec<u8>>,
    /// 总分片数（首个分片声明，后续必须一致）。
    count: usize,
    /// 最近一次收到分片的时间（用于 TTL 回收）。
    updated_at: i64,
    /// 已接收的字节数（内存总量上限的判据）。
    bytes: usize,
}

/// 分片重组器：把 notify 收到的分片拼回整帧。
///
/// 一个实例对应**一条 BLE 连接**（msg_id 是连接内唯一的，不同连接的 msg_id 互不相干）。
#[derive(Default)]
pub struct BleReassembler {
    partials: HashMap<u16, Partial>,
}

/// 一次 `push` 的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// 收下了，但消息还没齐。
    Incomplete,
    /// 消息已收齐（返回完整 payload）。
    Complete(Vec<u8>),
    /// 这一片被丢弃（重复 / 越界 / 参数非法 / 超出上限），消息不受影响。
    Dropped(&'static str),
}

impl Default for PushOutcome {
    fn default() -> Self {
        PushOutcome::Incomplete
    }
}

impl BleReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前进行中的不完整消息数（诊断/测试用）。
    pub fn in_flight(&self) -> usize {
        self.partials.len()
    }

    /// 收到一片。返回值语义见 [`PushOutcome`]。
    pub fn push(&mut self, chunk: &[u8], now_ms: i64) -> PushOutcome {
        if chunk.len() <= BLE_CHUNK_HEADER_LEN {
            // 只有头没有数据 ⇒ 无意义（空 payload 在业务层就该被拒）
            return PushOutcome::Dropped("分片无数据");
        }
        let msg_id = u16::from_be_bytes([chunk[0], chunk[1]]);
        let index = u16::from_be_bytes([chunk[2], chunk[3]]) as usize;
        let count = u16::from_be_bytes([chunk[4], chunk[5]]) as usize;
        if count == 0 || count > MAX_BLE_CHUNKS_PER_MESSAGE {
            return PushOutcome::Dropped("分片数非法");
        }
        if index >= count {
            return PushOutcome::Dropped("分片序号越界");
        }
        let data = &chunk[BLE_CHUNK_HEADER_LEN..];

        // 新消息：先做上限检查，**再**插状态（顺序很重要 —— 这里全是对端可控的输入）
        if !self.partials.contains_key(&msg_id) {
            // `count` 的合法范围已在入口统一校验（见上面 "分片数非法"），这里只查在途上限
            if self.partials.len() >= MAX_INFLIGHT_MESSAGES {
                return PushOutcome::Dropped("在途消息过多");
            }
            self.partials.insert(
                msg_id,
                Partial {
                    chunks: HashMap::new(),
                    count,
                    updated_at: now_ms,
                    bytes: 0,
                },
            );
        }

        let Some(p) = self.partials.get_mut(&msg_id) else {
            return PushOutcome::Dropped("状态丢失");
        };
        if p.count != count {
            // 同一条消息的分片对 count 说法不一致 ⇒ 这条消息已不可信，整条丢掉
            self.partials.remove(&msg_id);
            return PushOutcome::Dropped("分片数不一致");
        }
        if p.chunks.contains_key(&index) {
            return PushOutcome::Dropped("重复分片");
        }
        if p.bytes + data.len() > MAX_BLE_MESSAGE_BYTES {
            self.partials.remove(&msg_id);
            return PushOutcome::Dropped("消息超上限");
        }
        p.bytes += data.len();
        p.chunks.insert(index, data.to_vec());
        p.updated_at = now_ms;

        if p.chunks.len() < p.count {
            return PushOutcome::Incomplete;
        }
        // 收齐：按序拼接
        let Some(p) = self.partials.remove(&msg_id) else {
            return PushOutcome::Dropped("状态丢失");
        };
        let mut out = Vec::with_capacity(p.bytes);
        for i in 0..p.count {
            let Some(bytes) = p.chunks.get(&i) else {
                return PushOutcome::Dropped("内部状态不一致");
            };
            out.extend_from_slice(bytes);
        }
        PushOutcome::Complete(out)
    }

    /// 回收超时的不完整消息，返回被丢弃的条数（断连/对端消失后调用，也可周期性调用）。
    pub fn gc(&mut self, now_ms: i64) -> usize {
        let before = self.partials.len();
        self.partials
            .retain(|_, p| now_ms.saturating_sub(p.updated_at) < PARTIAL_TTL_MS);
        before - self.partials.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MTU: usize = 20; // GATT 默认 MTU 23 - 3 字节 ATT 头

    fn reassemble(chunks: &[Vec<u8>]) -> Option<Vec<u8>> {
        let mut r = BleReassembler::new();
        let mut out = None;
        for c in chunks {
            if let PushOutcome::Complete(payload) = r.push(c, 0) {
                out = Some(payload);
            }
        }
        out
    }

    #[test]
    fn roundtrip_across_sizes_and_mtus() {
        for len in [1usize, 2, 14, 15, 20, 21, 300, 4096] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            for mtu in [8usize, 20, 185, 512] {
                let chunks = fragment(&payload, mtu, 7).expect("应能分片");
                // 每片都不能超过 MTU，且都带头
                for c in &chunks {
                    assert!(c.len() <= mtu, "分片不能超过 MTU");
                    assert!(c.len() > BLE_CHUNK_HEADER_LEN);
                }
                assert_eq!(reassemble(&chunks).as_deref(), Some(payload.as_slice()), "len={len} mtu={mtu}");
            }
        }
    }

    #[test]
    fn rejects_invalid_inputs() {
        assert!(fragment(&[], MTU, 1).is_none(), "空 payload 没有意义");
        assert!(fragment(&[1, 2, 3], BLE_CHUNK_HEADER_LEN, 1).is_none(), "MTU 放不下头+1");
        assert!(fragment(&[1, 2, 3], 0, 1).is_none());
        let too_big = vec![0u8; MAX_BLE_MESSAGE_BYTES + 1];
        assert!(fragment(&too_big, MTU, 1).is_none(), "超过单条上限应拒绝");
    }

    #[test]
    fn out_of_order_chunks_still_complete() {
        let payload: Vec<u8> = (0..100).map(|i| i as u8).collect();
        let chunks = fragment(&payload, MTU, 3).unwrap();
        let mut shuffled = chunks.clone();
        shuffled.reverse();
        assert_eq!(reassemble(&shuffled).as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn duplicate_and_partial_do_not_complete() {
        let payload: Vec<u8> = (0..100).map(|i| i as u8).collect();
        let chunks = fragment(&payload, MTU, 5).unwrap();
        let mut r = BleReassembler::new();
        // 重复投递第一片：第二次必须被判为重复，且不影响进度
        assert_eq!(r.push(&chunks[0], 0), PushOutcome::Incomplete);
        assert_eq!(r.push(&chunks[0], 0), PushOutcome::Dropped("重复分片"));
        // 缺最后一片 ⇒ 永远不 Complete
        for c in &chunks[..chunks.len() - 1] {
            let _ = r.push(c, 0);
        }
        assert_eq!(r.in_flight(), 1, "残缺消息应仍在重组中（由 TTL 回收）");
    }

    #[test]
    fn interleaved_messages_are_independent() {
        let a: Vec<u8> = vec![0xAA; 50];
        let b: Vec<u8> = vec![0xBB; 50];
        let ca = fragment(&a, MTU, 1).unwrap();
        let cb = fragment(&b, MTU, 2).unwrap();
        let mut r = BleReassembler::new();
        let mut done: Vec<Vec<u8>> = Vec::new();
        for i in 0..ca.len().max(cb.len()) {
            if let Some(c) = ca.get(i) {
                if let PushOutcome::Complete(p) = r.push(c, 0) {
                    done.push(p);
                }
            }
            if let Some(c) = cb.get(i) {
                if let PushOutcome::Complete(p) = r.push(c, 0) {
                    done.push(p);
                }
            }
        }
        assert_eq!(done.len(), 2);
        assert!(done.contains(&a) && done.contains(&b));
    }

    #[test]
    fn malicious_headers_are_rejected_before_allocating() {
        let mut r = BleReassembler::new();
        // 只有头没有数据
        assert_eq!(r.push(&[0, 1, 0, 0, 0, 1], 0), PushOutcome::Dropped("分片无数据"));
        // count = 0
        assert_eq!(r.push(&[0, 1, 0, 0, 0, 0, 9], 0), PushOutcome::Dropped("分片数非法"));
        // index >= count
        assert_eq!(r.push(&[0, 1, 0, 5, 0, 2, 9], 0), PushOutcome::Dropped("分片序号越界"));
        // count 撒谎成超过上限 ⇒ 不分配、直接拒
        let huge = (MAX_BLE_CHUNKS_PER_MESSAGE as u16 + 1).to_be_bytes();
        assert_eq!(
            r.push(&[0, 1, 0, 0, huge[0], huge[1], 9], 0),
            PushOutcome::Dropped("分片数非法")
        );
        assert_eq!(r.in_flight(), 0, "被拒的分片不得留下状态");
        // 同一条消息前后 count 不一致 ⇒ 整条丢弃
        let c1 = fragment(&[1u8; 40], MTU, 9).unwrap();
        assert_eq!(r.push(&c1[0], 0), PushOutcome::Incomplete);
        let mut bogus = c1[1].clone();
        bogus[4] = 0;
        bogus[5] = 99; // 声称 99 片
        assert!(matches!(r.push(&bogus, 0), PushOutcome::Dropped(_)));
        assert_eq!(r.in_flight(), 0, "不一致的消息应被整条清掉");
    }

    #[test]
    fn inflight_cap_blocks_state_flood() {
        let mut r = BleReassembler::new();
        // 造 MAX_INFLIGHT_MESSAGES 条各缺一片的消息（每条 2 片）
        for id in 0..MAX_INFLIGHT_MESSAGES as u16 {
            let chunks = fragment(&[7u8; 40], MTU, id).unwrap();
            assert_eq!(r.push(&chunks[0], 0), PushOutcome::Incomplete);
        }
        assert_eq!(r.in_flight(), MAX_INFLIGHT_MESSAGES);
        // 再来一条新的 ⇒ 直接拒（不再分配）
        let extra = fragment(&[7u8; 40], MTU, 9999).unwrap();
        assert_eq!(r.push(&extra[0], 0), PushOutcome::Dropped("在途消息过多"));
        assert_eq!(r.in_flight(), MAX_INFLIGHT_MESSAGES);
    }

    #[test]
    fn stale_partials_are_collected() {
        let chunks = fragment(&[1u8; 40], MTU, 11).unwrap();
        let mut r = BleReassembler::new();
        assert_eq!(r.push(&chunks[0], 1_000), PushOutcome::Incomplete);
        assert_eq!(r.gc(1_000 + PARTIAL_TTL_MS - 1), 0, "未到期不回收");
        assert_eq!(r.gc(1_000 + PARTIAL_TTL_MS), 1, "到期即回收（断连后不残留）");
        assert_eq!(r.in_flight(), 0);
    }
}
