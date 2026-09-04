//! 异构 Mesh 中继路由：跨链路桥接寻址 + TTL 衰减 + 有限内存限流。
//!
//! 目标：
//! - **桥接**：同时具备局域网与蓝牙可达性的节点作为 Bridge，为「仅蓝牙」与「仅局域网」
//!   节点之间转发加密数据（中继节点无法解密）。
//! - **节点降压**：每条中继消息设置 TTL（默认 5 跳），逐跳递减；中继节点仅用
//!   有界 [`RingBuffer`] 暂存第三方消息，严格控制内存与 CPU 开销。
//! - **兜底**：目标离线或链路断开时，上层放弃无休止广播，将未达消息写入 `pending_queue`
//!   （本地离线队列），待路径重建或对方上线后补发。
//!
//! 本模块为可独立测试的路由构建块；接入蓝牙后端后，由传输层在转发路径中调用
//! [`MeshRouter::forward`] / [`MeshRouter::drain`] 完成跨链路桥接。

#![allow(dead_code)]

use std::collections::HashMap;

/// 链路类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkKind {
    Lan,
    Bluetooth,
}

/// 到某节点的路由条目（下一跳 + 代价 + 链路）。
#[derive(Clone, Debug)]
pub struct Route {
    pub via: String,
    pub cost: u32,
    pub link: LinkKind,
    pub last_seen: i64,
}

/// 待转发的中继消息（有限内存暂存）。
#[derive(Clone, Debug)]
pub struct RelayedMsg {
    pub msg_id: String,
    pub to: String,
    pub payload: Vec<u8>,
    pub ttl: u8,
}

/// 有界环形缓冲：容量满时覆盖最旧元素，保证内存有上界。
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

/// 异构 Mesh 中继路由。
pub struct MeshRouter {
    routes: HashMap<String, Route>,
    links: HashMap<String, Vec<LinkKind>>,
    ring: RingBuffer<RelayedMsg>,
    max_ttl: u8,
}

impl MeshRouter {
    pub fn new(max_ring_entries: usize, max_ttl: u8) -> Self {
        Self {
            routes: HashMap::new(),
            links: HashMap::new(),
            ring: RingBuffer::new(max_ring_entries),
            max_ttl,
        }
    }

    /// 更新 / 插入到某节点的路由（下一跳、代价、链路）。
    pub fn upsert_route(&mut self, peer: String, via: String, link: LinkKind, cost: u32, now: i64) {
        self.routes.insert(peer, Route { via, cost, link, last_seen: now });
    }

    /// 记录某节点可达的链路集合（用于识别桥接节点）。
    pub fn set_links(&mut self, peer: String, links: Vec<LinkKind>) {
        self.links.insert(peer, links);
    }

    /// 判断某节点是否为桥接节点（同时具备局域网与蓝牙可达性）。
    pub fn is_bridge(&self, peer: &str) -> bool {
        self.links.get(peer).map(|l| l.len() > 1).unwrap_or(false)
    }

    /// 获取到目标节点的最优路由。
    pub fn best_route(&self, peer: &str) -> Option<&Route> {
        self.routes.get(peer)
    }

    /// 转发一条中继消息：TTL 递减；TTL 耗尽则丢弃。返回 `Some(剩余 ttl)` 表示已入队。
    pub fn forward(&mut self, msg: RelayedMsg) -> Option<u8> {
        if msg.ttl == 0 {
            return None;
        }
        let ttl = msg.ttl.min(self.max_ttl) - 1;
        self.ring.push(RelayedMsg { ttl, ..msg });
        Some(ttl)
    }

    /// 取出所有待转发消息（由发送循环消费）。
    pub fn drain(&mut self) -> Vec<RelayedMsg> {
        self.ring.drain()
    }

    /// 当前暂存的待转发消息数。
    pub fn pending(&self) -> usize {
        self.ring.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_is_bounded_and_evicts_oldest() {
        let mut rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4); // 覆盖 1
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.drain(), vec![2, 3, 4]);
    }

    #[test]
    fn forward_decays_ttl_and_drops_at_zero() {
        let mut router = MeshRouter::new(10, 5);
        let msg = |ttl| RelayedMsg { msg_id: "m".into(), to: "b".into(), payload: vec![], ttl };
        // TTL=5 → 入队后剩余 4
        assert_eq!(router.forward(msg(5)), Some(4));
        assert_eq!(router.pending(), 1);
        // TTL=0 → 丢弃
        assert_eq!(router.forward(msg(0)), None);
        assert_eq!(router.pending(), 1);
        router.drain();
        assert_eq!(router.pending(), 0);
    }

    #[test]
    fn bridge_detection_by_multi_link() {
        let mut router = MeshRouter::new(10, 5);
        router.set_links("a".into(), vec![LinkKind::Lan]);
        router.set_links("b".into(), vec![LinkKind::Lan, LinkKind::Bluetooth]);
        assert!(!router.is_bridge("a"));
        assert!(router.is_bridge("b"));
    }

    #[test]
    fn route_upsert_and_lookup() {
        let mut router = MeshRouter::new(10, 5);
        router.upsert_route("target".into(), "bridge".into(), LinkKind::Bluetooth, 2, 1000);
        let r = router.best_route("target").unwrap();
        assert_eq!(r.via, "bridge");
        assert_eq!(r.cost, 2);
        assert_eq!(r.link, LinkKind::Bluetooth);
    }
}
