//! 大文件切片中继管理器：BitTorrent 式 Mesh 分发。
//!
//! 设计：发送方把文件切成 64KB~512KB 的 Chunk，将不同 Chunk **并行**分发给周围多个
//! 空闲节点（RelayPeer），由这些节点二次转发（`RelayChunk` 消息携带 TTL）到最终接收方；
//! 接收方按 `seq` 乱序重组。这在不依赖中央服务器的前提下，把传输吞吐分摊到多条链路。

use std::collections::HashMap;
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};

pub const MIN_CHUNK_SIZE: usize = 64 * 1024;
pub const DEFAULT_CHUNK_SIZE: usize = 256 * 1024;
pub const MAX_CHUNK_SIZE: usize = 512 * 1024;

/// 一个文件切片（base64 编码后的负载）。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ChunkData {
    pub seq: u32,
    pub data: String,
}

/// 并行分发计划：某一切片交给某个中继节点。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct RelayPlan {
    pub peer_id: String,
    pub chunk: ChunkData,
}

/// 接收方重组状态。
pub struct Reassembly {
    pub name: String,
    pub total_chunks: u32,
    pub expected_size: u64,
    pub chunks: HashMap<u32, Vec<u8>>,
    /// 开始重组的时刻。**必须有**：本表按 transfer_id 索引，而对端可以持续发新
    /// RelayFileOffer 却永不发分片 —— 没有时间戳就无法回收（见 `sweep_stale_relay`）。
    /// 注意 `name` 也由对端控制且长度可达单帧上限，同样是内存放大的来源。
    pub created_at: i64,
}

impl Reassembly {
    pub fn received(&self) -> u32 {
        self.chunks.len() as u32
    }
    pub fn complete(&self) -> bool {
        self.received() >= self.total_chunks
    }
}

pub struct RelayManager {
    pub chunk_size: usize,
    /// 发送任务：transfer_id -> 待发送切片（FIFO）
    senders: HashMap<String, Vec<ChunkData>>,
    /// 接收任务：transfer_id -> 重组状态
    reassemblies: HashMap<String, Reassembly>,
}

impl Default for RelayManager {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl RelayManager {
    pub fn new() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            senders: HashMap::new(),
            reassemblies: HashMap::new(),
        }
    }

    /// 将字节流切成块。
    pub fn split_bytes(bytes: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
        let cs = chunk_size.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE);
        bytes.chunks(cs).map(|c| c.to_vec()).collect()
    }

    /// 读取文件并切片，返回 (name, size, chunks)。
    pub fn slice_file(&self, path: &Path) -> std::io::Result<(String, u64, Vec<ChunkData>)> {
        Self::slice_file_with(path, self.chunk_size)
    }

    /// 独立于实例的切片入口：供阻塞线程池调用（避免长时间持有 relay 锁）。
    pub fn slice_file_with(
        path: &Path,
        chunk_size: usize,
    ) -> std::io::Result<(String, u64, Vec<ChunkData>)> {
        let meta = std::fs::metadata(path)?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let bytes = std::fs::read(path)?;
        let chunks = Self::split_bytes(&bytes, chunk_size)
            .into_iter()
            .enumerate()
            .map(|(i, b)| ChunkData {
                seq: i as u32,
                data: STANDARD.encode(&b),
            })
            .collect();
        Ok((name, meta.len(), chunks))
    }

    // ---------------- 发送方 ----------------

    pub fn register_send(&mut self, transfer_id: &str, chunks: Vec<ChunkData>) {
        self.senders.insert(transfer_id.to_string(), chunks);
    }

    pub fn next_chunk(&mut self, transfer_id: &str) -> Option<ChunkData> {
        self.senders.get_mut(transfer_id).and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(v.remove(0))
            }
        })
    }

    pub fn is_send_done(&self, transfer_id: &str) -> bool {
        self.senders
            .get(transfer_id)
            .map(|v| v.is_empty())
            .unwrap_or(true)
    }

    /// 将剩余切片按轮询分配给多个中继节点（并行分发计划）。
    pub fn plan_distribution(&self, transfer_id: &str, peers: &[String]) -> Vec<RelayPlan> {
        let Some(chunks) = self.senders.get(transfer_id) else {
            return Vec::new();
        };
        if peers.is_empty() {
            return Vec::new();
        }
        chunks
            .iter()
            .enumerate()
            .map(|(i, c)| RelayPlan {
                peer_id: peers[i % peers.len()].clone(),
                chunk: c.clone(),
            })
            .collect()
    }

    pub fn ack_chunk(&mut self, transfer_id: &str, seq: u32) {
        if let Some(v) = self.senders.get_mut(transfer_id) {
            v.retain(|c| c.seq != seq);
        }
    }

    pub fn finish_send(&mut self, transfer_id: &str) {
        self.senders.remove(transfer_id);
    }

    // ---------------- 接收方 ----------------

    pub fn begin_reassemble(
        &mut self,
        transfer_id: &str,
        name: &str,
        total_chunks: u32,
        expected_size: u64,
    ) {
        // 幂等：中继路径下同一份 RelayFileOffer 可能从多条邻居各来一份（泛洪），
        // 覆盖式 insert 会把已经组好的切片清空 ⇒ 文件永远缺片。已存在就保留。
        self.reassemblies
            .entry(transfer_id.to_string())
            .or_insert_with(|| Reassembly {
                name: name.to_string(),
                total_chunks,
                expected_size,
                chunks: HashMap::new(),
                created_at: crate::db::now_ms(),
            });
    }

    /// 写入一个切片；返回 `Some((name, 完整字节))` 表示重组完成（乱序安全）。
    pub fn add_chunk(
        &mut self,
        transfer_id: &str,
        seq: u32,
        data: Vec<u8>,
    ) -> Option<(String, u64, Vec<u8>)> {
        let done = {
            let r = self.reassemblies.get_mut(transfer_id)?;
            if seq >= r.total_chunks || r.chunks.contains_key(&seq) {
                return None;
            }
            r.chunks.insert(seq, data);
            r.complete()
        };
        if done {
            let r = self.reassemblies.remove(transfer_id)?;
            let mut out = Vec::new();
            for i in 0..r.total_chunks {
                if let Some(c) = r.chunks.get(&i) {
                    out.extend_from_slice(c);
                }
            }
            Some((r.name.clone(), r.expected_size, out))
        } else {
            None
        }
    }

    /// 重组进度（0.0 ~ 1.0）。
    pub fn progress(&self, transfer_id: &str) -> f64 {
        self.reassemblies
            .get(transfer_id)
            .map(|r| r.received() as f64 / r.total_chunks.max(1) as f64)
            .unwrap_or(0.0)
    }

    /// 当前进行中的发送任务数。
    pub fn active_sends(&self) -> usize {
        self.senders.values().filter(|v| !v.is_empty()).count()
    }

    /// 回收在 `cutoff` 之前开始、且仍未完成的重组。
    ///
    /// 返回被清掉的 transfer_id 列表（2026-09-23 审计 A2：调用方要据此给
    /// file_transfers 里仍 active 的行标失败终态——静默消失 = 前端永久卡 X%）。
    pub fn sweep_stale_reassemblies(&mut self, cutoff: i64) -> Vec<String> {
        let mut removed = Vec::new();
        self.reassemblies.retain(|k, r| {
            let keep = r.created_at > cutoff;
            if !keep {
                removed.push(k.clone());
            }
            keep
        });
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassemble_out_of_order() {
        let mut m = RelayManager::new();
        let data = b"abcdefghijklmnopqrstuvwxyz";
        // 直接手工切片（split_bytes 会把尺寸钳到 MIN_CHUNK_SIZE，不适合小数据测试）
        let chunks: Vec<Vec<u8>> = data.chunks(7).map(|c| c.to_vec()).collect();
        m.begin_reassemble("t1", "f.bin", chunks.len() as u32, data.len() as u64);
        // 乱序写入
        assert!(m.add_chunk("t1", 2, chunks[2].clone()).is_none());
        assert!(m.add_chunk("t1", 0, chunks[0].clone()).is_none());
        assert!(m.add_chunk("t1", 1, chunks[1].clone()).is_none());
        let (name, size, out) = m.add_chunk("t1", 3, chunks[3].clone()).unwrap();
        assert_eq!(name, "f.bin");
        assert_eq!(size, data.len() as u64);
        assert_eq!(out, data);
    }

    /// 回收过期的重组态：只清超时的，正在进行的必须原样保留。
    /// 没有这条回收，对端持续发 RelayFileOffer 却永不发分片就能让内存单调增长到 OOM。
    #[test]
    fn sweep_stale_reassemblies_keeps_active_and_drops_expired() {
        let mut m = RelayManager::new();
        m.begin_reassemble("fresh", "a.bin", 2, 4);
        m.begin_reassemble("stale", "b.bin", 2, 4);

        // 把 stale 的时间戳推到很久以前，fresh 保持"刚插入"
        if let Some(r) = m.reassemblies.get_mut("stale") {
            r.created_at = 1_000;
        }
        let cutoff = 2_000; // 只有 "stale"（1_000）早于它

        // 返回的是被回收的 transfer_id 列表（审计 A2：调用方据此给仍 active
        // 的传输标失败终态，所以必须能知道"清掉了谁"）
        assert_eq!(
            m.sweep_stale_reassemblies(cutoff),
            vec!["stale".to_string()],
            "应只清掉过期的那一条，且报出它的 id"
        );
        assert!(m.reassemblies.contains_key("fresh"), "进行中的重组不得被清");
        assert!(!m.reassemblies.contains_key("stale"));

        // 再清一次：已无可清项（幂等）
        assert!(m.sweep_stale_reassemblies(cutoff).is_empty());
    }

    /// 重复的 RelayFileOffer（多邻居泛洪）不得清空已收到的切片。
    #[test]
    fn begin_reassemble_is_idempotent() {
        let mut m = RelayManager::new();
        m.begin_reassemble("t1", "f.bin", 2, 4);
        assert!(m.add_chunk("t1", 0, vec![1, 2]).is_none());
        m.begin_reassemble("t1", "f.bin", 2, 4); // 重复的 offer
        let done = m.add_chunk("t1", 1, vec![3, 4]);
        assert!(done.is_some(), "重复 begin_reassemble 不能丢已收到的切片");
    }

    #[test]
    fn rejects_out_of_range_chunks() {
        let mut m = RelayManager::new();
        m.begin_reassemble("t1", "f.bin", 1, 1);
        assert!(m.add_chunk("t1", 1, vec![1]).is_none());
        assert!(m.add_chunk("t1", 0, vec![1]).is_some());
    }
}
