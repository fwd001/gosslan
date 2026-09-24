//! 中继借用的**接收侧**重组：按 `seq` 乱序收片、齐了再一次性交付。
//!
//! 场景：与目标没有直连链路时，文件可以经一个在线邻居"借用"过去
//! （`network/transport.rs` 的 `RelayFileOffer` / `RelayChunk` 分支）。邻居按 `seq`
//! 收片、乱序缓存、集齐后交给落盘流程。
//!
//! ⚠️ 这里曾有完整的**发送侧**（BitTorrent 式多邻居并行分发：`split_bytes` / `slice_file` /
//! `register_send` / `next_chunk` / `plan_distribution` / `ack_chunk` / `finish_send` /
//! `progress` / `is_send_done` / `active_sends` + `ChunkData` / `RelayPlan` /
//! `DEFAULT_CHUNK_SIZE` / `MAX_CHUNK_SIZE`），整个 `impl` 块头上挂着 `#[allow(dead_code)]`，
//! **零生产调用点** ⇒ 2026-09-24 架构复审 0-A2 删除。真正的文件发送是 `network/file.rs`
//! 的 `stream_file`（单链路固定 seq + 断点续传 + attempt epoch），它才是活路径。
//!
//! 删除时唯一被牵连到的活代码是 `get_topology` 的 `relay_count`：它原先取
//! `active_sends()`，而 `senders` 只有 `register_send` 会写 ⇒ **那个数永远是 0**。
//! 现在它改成数"当前有几条 `path_kind == Relay` 的活跃链路"（见 `state::link_is_relay_circuit`），
//! 与 `RuntimeSnapshot::relay.connected` 用同一个判据，界面里"N 中继"从此是真话。
//!
//! ⚠️ 已知遗留（架构复审 P4）：重组是**全量驻内存**的（`chunks: HashMap<u32, Vec<u8>>`，
//! `add_chunk` 完成时返回整个文件的字节），而直连接收走的是流式 `.part`。
//! 尺寸闸门目前只有一句 `size > i64::MAX`，等于没有 ⇒ 大文件经邻居借用会 OOM。
//! 修它属于第 2 步（缓冲与内存），本次只删不改行为。

use std::collections::HashMap;

/// 中继分片的**下限**尺寸（字节）。`network/file.rs` 在按链路挑 chunk 大小时用它兜底，
/// 所以这个常量必须留在这里与"分片"这个概念同源，不能挪去别处再抄一份。
pub const MIN_CHUNK_SIZE: usize = 64 * 1024;

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
            reassemblies: HashMap::new(),
        }
    }

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
        // 手工切片：真实分片尺寸由 `MIN_CHUNK_SIZE` 下限约束，这里只测乱序重组逻辑
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
