//! Gossip 广播引擎：去中心化消息分发 + 去重。
//!
//! 采用 **Epidemic（流行病）Gossip**：节点收到新消息后，向随机选取的 `fanout` 个邻居转发，
//! 消息经 TTL 衰减逐步覆盖全网。为防止风暴，使用两级去重：
//! - **Bloom Filter**：O(1) 概率去重，处理海量历史；
//! - **LRU 集合**：精确去重近期消息，兜底 Bloom 误判。

use std::collections::{HashMap, VecDeque};

use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

use crate::crypto::Identity;
use crate::protocol::{GossipEnvelope, GossipKind};

/// 简易 Bloom Filter（双哈希扩展）。
pub struct BloomFilter {
    bits: Vec<u64>,
    num_bits: usize,
    num_hashes: usize,
}

impl BloomFilter {
    /// `capacity`：期望元素数；`false_positive`：误判率（如 0.01）。
    pub fn new(capacity: usize, false_positive: f64) -> Self {
        let ln2 = std::f64::consts::LN_2;
        let num_bits = (-(capacity as f64) * false_positive.ln() / (ln2 * ln2)).ceil() as usize;
        let num_hashes = ((num_bits as f64 / capacity as f64) * ln2).ceil() as usize;
        let num_bits = num_bits.max(64);
        let num_hashes = num_hashes.clamp(2, 16);
        Self {
            bits: vec![0u64; (num_bits + 63) / 64],
            num_bits,
            num_hashes,
        }
    }

    fn positions(&self, data: &str) -> Vec<usize> {
        let h1 = hash_u64(data, 0u8);
        let h2 = hash_u64(data, 1u8).max(1);
        (0..self.num_hashes)
            .map(|i| ((h1.wrapping_add((i as u64).wrapping_mul(h2))) as usize) % self.num_bits)
            .collect()
    }

    pub fn insert(&mut self, data: &str) {
        for p in self.positions(data) {
            self.bits[p / 64] |= 1u64 << (p % 64);
        }
    }

    pub fn contains(&self, data: &str) -> bool {
        self.positions(data).iter().all(|&p| (self.bits[p / 64] >> (p % 64)) & 1 == 1)
    }
}

fn hash_u64(data: &str, seed: u8) -> u64 {
    let mut h = Sha256::new();
    h.update(data.as_bytes());
    h.update([seed]);
    let d = h.finalize();
    u64::from_be_bytes(d[..8].try_into().unwrap())
}

/// 固定容量的 LRU 精确去重集合（插入序淘汰）。
pub struct LruSet {
    map: HashMap<String, ()>,
    order: VecDeque<String>,
    capacity: usize,
}

impl LruSet {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn contains(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }

    pub fn insert(&mut self, k: String) {
        if self.map.contains_key(&k) {
            return;
        }
        self.map.insert(k.clone(), ());
        self.order.push_back(k);
        if self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
}

/// Gossip 引擎：去重 + 扇出 + 信封构造。
pub struct GossipEngine {
    bloom: BloomFilter,
    seen: LruSet,
    pub fanout: usize,
    pub ttl: u8,
}

impl GossipEngine {
    pub fn new(bloom_capacity: usize, seen_capacity: usize, fanout: usize, ttl: u8) -> Self {
        Self {
            bloom: BloomFilter::new(bloom_capacity, 0.01),
            seen: LruSet::new(seen_capacity),
            fanout,
            ttl,
        }
    }

    /// 返回该 message_id 是否为“首次见到”（并完成去重登记）。
    pub fn is_new(&mut self, message_id: &str) -> bool {
        if self.seen.contains(message_id) || self.bloom.contains(message_id) {
            return false;
        }
        self.bloom.insert(message_id);
        self.seen.insert(message_id.to_string());
        true
    }

    /// 从候选节点中随机选取 `fanout` 个（排除 `exclude`）用于转发。
    pub fn choose_fanout(&self, candidates: &[String], exclude: &str) -> Vec<String> {
        let mut pool: Vec<String> = candidates
            .iter()
            .filter(|p| p.as_str() != exclude)
            .cloned()
            .collect();
        let mut rng = OsRng;
        for i in (1..pool.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            pool.swap(i, j);
        }
        pool.truncate(self.fanout.min(pool.len()));
        pool
    }

    /// 构造一个单聊 / 群聊 Gossip 信封（加密 payload 由调用方传入）。
    pub fn build_envelope(
        &self,
        identity: &Identity,
        sender_id: &str,
        kind: GossipKind,
        group_id: Option<String>,
        payload_b64: &str,
        ts: i64,
    ) -> GossipEnvelope {
        let mut env = GossipEnvelope {
            message_id: String::new(),
            sender_id: sender_id.to_string(),
            sender_pubkey: identity.x25519_public_b64(),
            sender_ed25519: identity.ed25519_public_b64(),
            sender_sig: String::new(),
            ttl: self.ttl,
            kind,
            group_id,
            payload: payload_b64.to_string(),
            ts,
            encrypted: true, // 默认加密；调用方可按 E2EE 开关改写
        };
        env.compute_message_id();
        env.sender_sig = identity.sign_b64(env.message_id.as_bytes());
        env
    }

    /// 校验信封签名与 message_id 完整性。
    pub fn verify_envelope(&self, env: &GossipEnvelope) -> bool {
        let mut check = env.clone();
        let expected = check.message_id.clone();
        check.compute_message_id();
        expected == check.message_id
            && crate::crypto::verify_signature(&env.sender_ed25519, expected.as_bytes(), &env.sender_sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_and_lru_dedup() {
        let mut engine = GossipEngine::new(1000, 100, 3, 5);
        assert!(engine.is_new("abc"));
        assert!(!engine.is_new("abc"));
        assert!(engine.is_new("def"));
    }

    #[test]
    fn fanout_excludes_sender() {
        let engine = GossipEngine::new(100, 10, 2, 5);
        let peers = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let picked = engine.choose_fanout(&peers, "b");
        assert!(picked.len() <= 2);
        assert!(!picked.contains(&"b".to_string()));
    }

    #[test]
    fn envelope_sign_verify_and_tamper_detection() {
        use crate::crypto::Identity;
        let id = Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);

        let env = engine.build_envelope(&id, "dev-a", GossipKind::Chat, None, "cipher", 42);
        assert!(engine.verify_envelope(&env));

        // 篡改 payload 后签名校验应失败
        let mut tampered = env.clone();
        tampered.payload = "tampered".into();
        assert!(!engine.verify_envelope(&tampered));

        // 篡改 message_id 后也应失败
        let mut tampered_id = env.clone();
        tampered_id.message_id = "forged".into();
        assert!(!engine.verify_envelope(&tampered_id));
    }

    #[test]
    fn envelope_ttl_is_preserved() {
        let id = crate::crypto::Identity::generate();
        let engine = GossipEngine::new(100, 10, 4, 6);
        let env = engine.build_envelope(&id, "dev-a", GossipKind::Chat, None, "cipher", 42);
        assert_eq!(env.ttl, 6);
    }
}
