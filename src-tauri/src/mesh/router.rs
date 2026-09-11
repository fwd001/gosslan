//! MeshRouter：全网统一的帧转发核心（Phase 5 第一步）。
//!
//! 职责边界（设计 §15）：
//! - 负责：verify 后的 **dedup / TTL / source exclusion / 目标判定 / 转发决策**；
//! - **不负责**：UI、SQLite、E2EE 明文、好友关系、任何具体 Transport 的 socket 实现。
//!
//! 全局去重（§16）：去重必须在 Mesh 层统一做，不能每个 Transport 各做一份，
//! 否则 `LAN → Phone → BLE` 与 `BLE → Phone → LAN` 会形成循环。
//!
//! TTL 跨 Transport（§17）：TTL 属于 Mesh 帧而非某条链路，`LAN → BLE → Tailscale`
//! 全程共用同一个递减计数。
//!
//! Phase 5 第一步只落地**纯逻辑**：不接管任何收发路径，也不碰 transport。

use crate::gossip_engine::{BloomFilter, LruSet};

/// 帧载荷的种类：Gosslan 自己的消息 与 外部协议的不透明包，**必须分开**（§13/§14）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshFrameKind {
    /// Gosslan 业务帧（E2EE 密文；MeshRouter 只转发，不解密）。
    Gosslan,
    /// 外部协议的不透明载荷（如 BitChat packet）：不解密、不落库、不建用户，只转发。
    OpaqueExternal,
}

/// 帧的目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshDestination {
    /// 定向到某个节点（device_id）。
    Node(String),
    /// 广播 / Gossip：所有可达节点都可能需要，按有界 fanout 扩散。
    Broadcast,
}

/// 统一的 Mesh 帧（§13）。
#[derive(Clone, Debug, PartialEq)]
pub struct MeshFrame {
    /// 全网唯一的帧 id —— 去重与防环的键。
    pub frame_id: String,
    /// 产生该帧的**源节点**（不是中继节点）：source exclusion 依据（§18）。
    pub source_node_id: String,
    pub destination: MeshDestination,
    /// 剩余跳数；跨 Transport 统一递减（§17）。
    pub ttl: u8,
    pub kind: MeshFrameKind,
    /// 不透明载荷：Gosslan 密文或 BitChat packet，MeshRouter 不解析内容。
    pub payload: Vec<u8>,
}

/// 丢弃原因（诊断 / 可观测性用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// 已见过该 frame_id（重复到达 / 成环）。
    Duplicate,
    /// TTL 耗尽，无法继续转发。
    TtlExhausted,
}

/// MeshRouter 对一帧的处理决策（§15 流水线的输出）。
#[derive(Debug, PartialEq)]
pub enum ForwardDecision {
    /// 交付给本机上层（且不再转发）。
    Deliver,
    /// 继续转发。`deliver_locally` 为 true 表示广播帧：本机也要收，同时扩散出去。
    Forward {
        deliver_locally: bool,
        frame: MeshFrame,
    },
    /// 丢弃（附原因，便于诊断）。
    Drop(DropReason),
}

/// 有界环形缓冲：容量满时覆盖最旧元素，保证内存有上界。
///
/// 用途：待转发帧队列。§39 要求所有 Transport 都有 bounded queue——
/// 不能因为 BLE 慢就无限积压，也不能让 LAN 的高速把 BLE 的队列撑爆。
/// 满时**丢弃最旧的**而不是阻塞或无限增长。
pub struct RingBuffer<T> {
    buf: Vec<Option<T>>,
    head: usize,
    len: usize,
    cap: usize,
}

impl<T> RingBuffer<T> {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            buf: (0..cap).map(|_| None).collect(),
            head: 0,
            len: 0,
            cap,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 压入一个元素；容量满时覆盖最旧元素。
    pub fn push(&mut self, v: T) {
        if self.len == self.cap {
            self.buf[self.head] = Some(v);
            self.head = (self.head + 1) % self.cap;
        } else {
            let idx = (self.head + self.len) % self.cap;
            self.buf[idx] = Some(v);
            self.len += 1;
        }
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let v = self.buf[self.head].take();
        self.head = (self.head + 1) % self.cap;
        self.len -= 1;
        v
    }

    /// 取出全部元素（发送循环消费）。
    pub fn drain(&mut self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.len);
        while let Some(v) = self.pop_front() {
            out.push(v);
        }
        out
    }
}

/// Mesh 转发路由器。
pub struct MeshRouter {
    bloom: BloomFilter,
    seen: LruSet,
    /// TTL 上限：防止对端自报超大 TTL 导致无限扩散（§7 有界传播）。
    max_ttl: u8,
    /// 有界 fanout：广播时最多向几个下一跳扩散（§20 第一版不做复杂路由算法）。
    fanout: usize,
    /// 待转发帧队列（有界，§39 背压）。
    pending: RingBuffer<MeshFrame>,
}

impl MeshRouter {
    pub fn new(
        bloom_capacity: usize,
        seen_capacity: usize,
        max_ttl: u8,
        fanout: usize,
        pending_capacity: usize,
    ) -> Self {
        Self {
            bloom: BloomFilter::new(bloom_capacity, 0.01),
            seen: LruSet::new(seen_capacity),
            max_ttl,
            fanout,
            pending: RingBuffer::new(pending_capacity),
        }
    }

    /// 是否为首次见到的帧（并完成去重登记）。
    fn is_new_frame(&mut self, frame_id: &str) -> bool {
        if self.seen.contains(frame_id) || self.bloom.contains(frame_id) {
            return false;
        }
        self.bloom.insert(frame_id);
        self.seen.insert(frame_id.to_string());
        true
    }

    /// 收到一帧后的处理（§15）：dedup → TTL → 目标判定 → 转发决策。
    ///
    /// 注意顺序：**先去重再判 TTL**。反过来的话，TTL 耗尽的帧每次都会被重新登记
    /// 进去重表，既污染缓存也让同一帧的重复到达无法被识别。
    pub fn on_receive(&mut self, frame: MeshFrame, my_node_id: &str) -> ForwardDecision {
        // 1. 全局去重
        if !self.is_new_frame(&frame.frame_id) {
            return ForwardDecision::Drop(DropReason::Duplicate);
        }

        // 2. TTL（先按上限裁剪，防止对端自报超大 TTL）
        let effective = frame.ttl.min(self.max_ttl);
        if effective == 0 {
            return ForwardDecision::Drop(DropReason::TtlExhausted);
        }
        let remaining = effective - 1;

        // 3. 目标判定
        let for_me = match &frame.destination {
            MeshDestination::Node(id) => id == my_node_id,
            MeshDestination::Broadcast => true,
        };
        // 定向帧到达目标后停止；广播帧只要还有 TTL 就继续扩散。
        let should_forward = match &frame.destination {
            MeshDestination::Node(id) => id != my_node_id && remaining > 0,
            MeshDestination::Broadcast => remaining > 0,
        };

        if for_me && !should_forward {
            return ForwardDecision::Deliver;
        }
        if !for_me && !should_forward {
            // 定向给他人但 TTL 已耗尽：既不该本机消费，也送不出去了
            return ForwardDecision::Drop(DropReason::TtlExhausted);
        }

        let mut next = frame;
        next.ttl = remaining;
        ForwardDecision::Forward {
            deliver_locally: for_me,
            frame: next,
        }
    }

    /// 选择转发的下一跳：**排除源节点**（§18 source exclusion）。
    ///
    /// 多 Transport 场景下（A 同时经 LAN 与 BLE 连到 B），两边都视为同一 source node A，
    /// 因此这里按**节点**排除而不是按连接排除。
    ///
    /// 结果按 `fanout` 截断，保证有界扩散（§20：第一版不做最短路径算法）。
    pub fn select_outgoing<'a>(
        &self,
        candidates: &'a [String],
        source_node_id: &str,
    ) -> Vec<&'a str> {
        candidates
            .iter()
            .filter(|c| c.as_str() != source_node_id)
            .map(|c| c.as_str())
            .take(self.fanout)
            .collect()
    }

    /// 只做 source exclusion，**不按 fanout 截断**。
    ///
    /// 用于**源发广播**（发送方把帧发给自己的所有直连节点）：此时必须保证覆盖，
    /// 一旦截断，连接数超过 fanout 的节点就收不到 —— 群消息会静默漏发。
    ///
    /// `fanout` 截断只适用于**转发**（§20 控制风暴），见 [`select_outgoing`]。
    /// 两者不可混用。
    pub fn exclude_source<'a>(
        &self,
        candidates: &'a [String],
        source_node_id: &str,
    ) -> Vec<&'a str> {
        candidates
            .iter()
            .filter(|c| c.as_str() != source_node_id)
            .map(|c| c.as_str())
            .collect()
    }

    // ---------------- 待转发队列（§39 有界背压） ----------------

    /// 把待转发帧入队。队列满时**覆盖最旧**的一帧，绝不无限增长。
    pub fn enqueue(&mut self, frame: MeshFrame) {
        self.pending.push(frame);
    }

    /// 取出全部待转发帧（由发送循环消费）。
    pub fn drain_pending(&mut self) -> Vec<MeshFrame> {
        self.pending.drain()
    }

    /// 当前积压的待转发帧数（诊断 / 背压判断用）。
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> MeshRouter {
        MeshRouter::new(1000, 100, 7, 3, 10)
    }

    fn frame(id: &str, src: &str, dest: MeshDestination, ttl: u8) -> MeshFrame {
        MeshFrame {
            frame_id: id.into(),
            source_node_id: src.into(),
            destination: dest,
            ttl,
            kind: MeshFrameKind::Gosslan,
            payload: b"cipher".to_vec(),
        }
    }

    /// 广播帧：本机要收，同时以 ttl-1 继续扩散。
    #[test]
    fn broadcast_frame_is_delivered_and_forwarded() {
        let mut r = router();
        let d = r.on_receive(frame("f1", "A", MeshDestination::Broadcast, 7), "me");

        match d {
            ForwardDecision::Forward {
                deliver_locally,
                frame,
            } => {
                assert!(deliver_locally, "广播帧本机也要收");
                assert_eq!(frame.ttl, 6, "TTL 必须递减");
            }
            other => panic!("期望 Forward，实际 {other:?}"),
        }
    }

    /// 全局去重：同一 frame_id 第二次到达必须丢弃（§16 防环的关键）。
    #[test]
    fn duplicate_frame_is_dropped() {
        let mut r = router();
        r.on_receive(frame("f1", "A", MeshDestination::Broadcast, 7), "me");
        let d = r.on_receive(frame("f1", "A", MeshDestination::Broadcast, 7), "me");

        assert_eq!(d, ForwardDecision::Drop(DropReason::Duplicate));
    }

    /// TTL 为 0 直接丢弃，不得再扩散。
    #[test]
    fn zero_ttl_is_dropped() {
        let mut r = router();
        let d = r.on_receive(frame("f1", "A", MeshDestination::Broadcast, 0), "me");
        assert_eq!(d, ForwardDecision::Drop(DropReason::TtlExhausted));
    }

    /// 广播帧 TTL=1：本机仍要收，但不再向外扩散（remaining=0）。
    #[test]
    fn broadcast_with_ttl_one_delivers_without_forwarding() {
        let mut r = router();
        let d = r.on_receive(frame("f1", "A", MeshDestination::Broadcast, 1), "me");
        assert_eq!(d, ForwardDecision::Deliver);
    }

    /// 定向给他人：本机不消费，继续转发。
    #[test]
    fn directed_frame_for_other_node_is_relayed() {
        let mut r = router();
        let d = r.on_receive(
            frame("f1", "A", MeshDestination::Node("C".into()), 5),
            "me",
        );

        match d {
            ForwardDecision::Forward {
                deliver_locally,
                frame,
            } => {
                assert!(!deliver_locally, "不是给我的，本机不消费");
                assert_eq!(frame.ttl, 4);
            }
            other => panic!("期望 Forward，实际 {other:?}"),
        }
    }

    /// 定向给自己：交付上层，停止转发。
    #[test]
    fn directed_frame_for_me_is_delivered_only() {
        let mut r = router();
        let d = r.on_receive(
            frame("f1", "A", MeshDestination::Node("me".into()), 5),
            "me",
        );
        assert_eq!(d, ForwardDecision::Deliver);
    }

    /// 定向给他人但 TTL 耗尽：既不能消费也送不出去 → 丢弃。
    #[test]
    fn directed_frame_for_other_with_exhausted_ttl_is_dropped() {
        let mut r = router();
        let d = r.on_receive(
            frame("f1", "A", MeshDestination::Node("C".into()), 1),
            "me",
        );
        assert_eq!(d, ForwardDecision::Drop(DropReason::TtlExhausted));
    }

    /// 对端自报超大 TTL 必须被 max_ttl 裁剪（防无界扩散）。
    #[test]
    fn ttl_is_clamped_to_max_ttl() {
        let mut r = router(); // max_ttl = 7
        let d = r.on_receive(frame("f1", "A", MeshDestination::Broadcast, 255), "me");

        match d {
            ForwardDecision::Forward { frame, .. } => assert_eq!(frame.ttl, 6),
            other => panic!("期望 Forward，实际 {other:?}"),
        }
    }

    /// Source exclusion：绝不把帧发回源节点（§18）。
    #[test]
    fn select_outgoing_excludes_source_node() {
        let r = router();
        let candidates = vec!["A".to_string(), "B".into(), "C".into(), "D".into()];
        let picked = r.select_outgoing(&candidates, "A");

        assert!(!picked.contains(&"A"));
        assert!(picked.contains(&"B"));
    }

    /// 源发广播只排除源、**不截断**：截断会导致连接数超过 fanout 时漏发（群消息丢失）。
    /// 与 `select_outgoing_is_bounded_by_fanout` 对照，钉住两者的区别。
    #[test]
    fn exclude_source_does_not_truncate() {
        let r = router(); // fanout = 3
        let candidates: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();

        // 源不在候选里 → 全部保留（10 > fanout 3，仍不截断）
        let picked = r.exclude_source(&candidates, "nobody");
        assert_eq!(picked.len(), 10, "源发必须覆盖全部，不能按 fanout 截断");

        // 但仍要排除源
        let picked2 = r.exclude_source(&candidates, "n0");
        assert_eq!(picked2.len(), 9);
        assert!(!picked2.contains(&"n0"));
    }

    /// fanout 有界：即使候选很多，也最多选 fanout 个（§20）。
    #[test]
    fn select_outgoing_is_bounded_by_fanout() {
        let r = router(); // fanout = 3
        let candidates: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
        let picked = r.select_outgoing(&candidates, "n99");

        assert_eq!(picked.len(), 3);
    }

    /// RingBuffer：容量有界，满时覆盖最旧（§39 内存上界）。
    #[test]
    fn ring_buffer_is_bounded_and_evicts_oldest() {
        let mut rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4); // 覆盖 1
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.drain(), vec![2, 3, 4]);
        assert!(rb.is_empty());
    }

    /// 待转发队列：入队 → 取出；容量满时覆盖最旧，绝不无限增长。
    #[test]
    fn pending_queue_is_bounded_and_drainable() {
        let mut r = MeshRouter::new(100, 10, 7, 3, 2);
        r.enqueue(frame("f1", "A", MeshDestination::Broadcast, 3));
        r.enqueue(frame("f2", "A", MeshDestination::Broadcast, 3));
        assert_eq!(r.pending_count(), 2);

        // 第三个超出容量 → 覆盖最旧，仍是有界的 2
        r.enqueue(frame("f3", "A", MeshDestination::Broadcast, 3));
        assert_eq!(r.pending_count(), 2);

        let out = r.drain_pending();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].frame_id, "f2", "最旧的 f1 应已被覆盖");
        assert_eq!(r.pending_count(), 0);
    }

    /// 外部不透明帧（BitChat）走同一条流水线：不解密、只按帧规则处理（§29）。
    #[test]
    fn opaque_external_frame_follows_same_pipeline() {
        let mut r = router();
        let mut f = frame("b1", "A", MeshDestination::Broadcast, 3);
        f.kind = MeshFrameKind::OpaqueExternal;
        f.payload = b"bitchat-opaque".to_vec();

        match r.on_receive(f, "me") {
            ForwardDecision::Forward { frame, .. } => {
                assert_eq!(frame.kind, MeshFrameKind::OpaqueExternal);
                // 载荷原样透传，不被解析
                assert_eq!(frame.payload, b"bitchat-opaque".to_vec());
            }
            other => panic!("期望 Forward，实际 {other:?}"),
        }
    }
}
