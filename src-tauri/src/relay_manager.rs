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
#[derive(Clone, Debug)]
pub struct ChunkData {
    pub seq: u32,
    pub data: String,
}

/// 并行分发计划：某一切片交给某个中继节点。
#[derive(Clone, Debug)]
pub struct RelayPlan {
    pub peer_id: String,
    pub chunk: ChunkData,
}

/// 接收方重组状态。
pub struct Reassembly {
    pub name: String,
    pub total_chunks: u32,
    pub chunks: HashMap<u32, Vec<u8>>,
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
    pub fn slice_file_with(path: &Path, chunk_size: usize) -> std::io::Result<(String, u64, Vec<ChunkData>)> {
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
        self.senders
            .get_mut(transfer_id)
            .and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) })
    }

    pub fn is_send_done(&self, transfer_id: &str) -> bool {
        self.senders.get(transfer_id).map(|v| v.is_empty()).unwrap_or(true)
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

    pub fn begin_reassemble(&mut self, transfer_id: &str, name: &str, total_chunks: u32) {
        self.reassemblies.insert(
            transfer_id.to_string(),
            Reassembly {
                name: name.to_string(),
                total_chunks,
                chunks: HashMap::new(),
            },
        );
    }

    /// 写入一个切片；返回 `Some((name, 完整字节))` 表示重组完成（乱序安全）。
    pub fn add_chunk(&mut self, transfer_id: &str, seq: u32, data: Vec<u8>) -> Option<(String, Vec<u8>)> {
        let done = {
            let Some(r) = self.reassemblies.get_mut(transfer_id) else {
                return None;
            };
            if r.chunks.contains_key(&seq) {
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
            Some((r.name.clone(), out))
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
        m.begin_reassemble("t1", "f.bin", chunks.len() as u32);
        // 乱序写入
        assert!(m.add_chunk("t1", 2, chunks[2].clone()).is_none());
        assert!(m.add_chunk("t1", 0, chunks[0].clone()).is_none());
        assert!(m.add_chunk("t1", 1, chunks[1].clone()).is_none());
        let (name, out) = m.add_chunk("t1", 3, chunks[3].clone()).unwrap();
        assert_eq!(name, "f.bin");
        assert_eq!(out, data);
    }
}
