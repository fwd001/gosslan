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
//! ## P4 已收口：边收边写 `.part`，内存与文件大小无关
//! 原先这里是 `chunks: HashMap<u32, Vec<u8>>` —— 整份文件驻内存，完成时再组装出第二份，
//! 峰值 ≈ 2× 文件大小（600MB 文件 = 1.2GB 内存，移动端必被系统杀掉），而尺寸闸门
//! 只有一句 `size > i64::MAX`，等于没有。直连路径早有流式纪律，中继这次把它补齐：
//! `RelayFileOffer` 带上发送方**实际用的** `chunk_size`，接收方按 `seq × chunk_size`
//! 直接 `seek + write_all` 落进预分配的 `.part`，收齐后**流式**算 SHA-256 再改名落盘。
//! 峰值只剩一个分片 + 一个读缓冲。
//!
//! ⚠️ `chunk_size` 不能从 `(size, total_chunks)` 反推 —— 见 `Reassembly::chunk_size` 那条。
//! 因此老发送方（不发这个字段）的 offer 会被**明确拒收**并给出一句话原因，
//! 而不是静默失败，也不是退回内存重组。

use std::collections::{HashMap, HashSet};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 中继分片的**下限**尺寸（字节）。`network/file.rs` 在按链路挑 chunk 大小时用它兜底，
/// 所以这个常量必须留在这里与"分片"这个概念同源，不能挪去别处再抄一份。
pub const MIN_CHUNK_SIZE: usize = 64 * 1024;

/// 中继 `.part` 的后缀。与直连的 `{id}.part` 区分开是有意的：
/// 两套接收器各自回收自己的文件，`sweep_stale_parts` 的"活跃接收器"名单里没有中继的 id。
const RELAY_PART_SUFFIX: &str = ".relay.part";

/// 一次 `add_chunk` 的结果。**为什么是枚举而不是 `Option`**：
/// 调用方对"重复分片"（忽略）、"形状不合法"（判死这一单 + 告诉用户）、"还差几片"
/// （推进度）三种情形的处理完全不同，用一个 `Option` 表达就得在调用方再猜一次。
#[derive(Debug)]
pub enum ChunkOutcome {
    /// 没有这个会话（offer 从未到达，或已被回收/已完成）
    Unknown,
    /// 重复分片：多邻居泛洪会把同一帧送多份，必须安静忽略
    Duplicate,
    /// 对端声明的形状自相矛盾 ⇒ 这一单判死（`.part` 已删），原因给界面
    Rejected(String),
    /// 已写入，仍在等后续分片
    Partial {
        received_bytes: u64,
        total_bytes: u64,
    },
    /// 全部分片就位、已 `sync_all`，文件此刻在临时路径上等着改名落盘
    Complete {
        name: String,
        size: u64,
        path: PathBuf,
    },
}

/// 接收方重组状态：**字节直接落盘，内存里只留"收到过哪些 seq"**。
///
/// 原先这里是 `chunks: HashMap<u32, Vec<u8>>`（整份文件驻内存），完成时再
/// `extend_from_slice` 组装出第二份 ⇒ 峰值 ≈ 2× 文件大小，600MB 文件在手机上必死。
/// 直连路径早有流式纪律（边收边写 `.part`），中继把它补上；两片之间的空隙由
/// `set_len` 预分配填零，落盘的顺序仍是 `seq` 递增的字节序。
pub struct Reassembly {
    pub name: String,
    pub total_chunks: u32,
    pub expected_size: u64,
    /// 发送方实际使用的分片尺寸（从 `RelayFileOffer.chunk_size` 来）。
    /// **必须有，不能从 `size / total_chunks` 反推**：`total_chunks = ceil(size / chunk_size)`
    /// 是不可逆的 —— 例如 size=65537、chunk_size=65536 ⇒ total=2，反推出 32769，
    /// 偏移全部错位，写完的文件哈希必然对不上。
    pub chunk_size: u64,
    pub path: PathBuf,
    file: std::fs::File,
    received: HashSet<u32>,
    received_bytes: u64,
    /// 开始重组的时刻。**必须有**：本表按 transfer_id 索引，而对端可以持续发新
    /// RelayFileOffer 却永不发分片 —— 没有时间戳就无法回收（见 `sweep_stale_relay`）。
    /// 注意 `name` 也由对端控制且长度可达单帧上限，同样是资源放大的来源。
    pub created_at: i64,
}

impl Reassembly {
    pub fn received(&self) -> u32 {
        self.received.len() as u32
    }
    pub fn complete(&self) -> bool {
        self.received() >= self.total_chunks
    }

    /// 判死这一单：从磁盘上拿走 `.part`（只从表里摘掉 = 留一份占满尺寸的文件）。
    fn abandon(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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

    /// 为一个借用中的文件开好 `.part`（预分配成声明尺寸）。
    ///
    /// `chunk_size == 0` = 对端没声明分片尺寸（4 个版本之前的老对端）⇒ **拒收**并说明原因：
    /// 没有它就落不出正确偏移，而"退回整份进内存"就是这次要消灭的那个 OOM。
    /// 老的接收方对新 offer 是兼容的（serde 会忽略它不认识的字段），所以只有
    /// "新接收 ← 老发送"这一侧需要升级，方向是明确的。
    pub fn begin_reassemble(
        &mut self,
        transfer_id: &str,
        name: &str,
        total_chunks: u32,
        expected_size: u64,
        chunk_size: u32,
        dir: &Path,
    ) -> Result<(), String> {
        // transfer_id 会变成文件名的一部分，必须与直连路径**同一份**消毒口径
        // （见 `network/file.rs::safe_transfer_id`）：放行 `../x` 等于让对端一句话
        // 把文件写到下载目录之外。
        let id = crate::network::file::safe_transfer_id(transfer_id).ok_or("传输标识非法")?;
        if chunk_size == 0 {
            return Err("对端未声明分片尺寸（版本过旧），无法经中继接收该文件".to_string());
        }
        // 幂等：中继路径下同一份 RelayFileOffer 可能从多条邻居各来一份（泛洪），
        // 重新开档会把已写好的分片 truncate 掉 ⇒ 文件永远缺片。已存在就原样保留。
        if self.reassemblies.contains_key(&id) {
            return Ok(());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("创建下载目录失败：{e}"))?;
        let path = dir.join(format!("{id}{RELAY_PART_SUFFIX}"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("创建临时文件失败：{e}"))?;
        // 预分配到声明的尺寸：一是让"写到 offset 之外"当场可判，二是稀疏文件
        // 不真占空间（只有写过的页才落盘）。
        if let Err(e) = file.set_len(expected_size) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("预分配文件失败：{e}"));
        }
        self.reassemblies.insert(
            id.clone(),
            Reassembly {
                name: name.to_string(),
                total_chunks,
                expected_size,
                chunk_size: u64::from(chunk_size),
                path,
                file,
                received: HashSet::new(),
                received_bytes: 0,
                created_at: crate::db::now_ms(),
            },
        );
        Ok(())
    }

    /// 写入一个切片（乱序安全、重复忽略）。返回 `Complete` 时文件已 `sync_all`，
    /// 调用方只需做完整性校验 + 改名落盘。
    pub fn add_chunk(&mut self, transfer_id: &str, seq: u32, data: &[u8]) -> ChunkOutcome {
        let Some(r) = self.reassemblies.get_mut(transfer_id) else {
            return ChunkOutcome::Unknown;
        };
        if r.received.contains(&seq) {
            return ChunkOutcome::Duplicate;
        }
        // 三条形状判据任何一条不成立，都说明"两端的分块口径不一致"或对端在撒谎 ——
        // 继续写只会写出一份哈希必然不符、还可能占满磁盘的文件，当场判死。
        let reason = if seq >= r.total_chunks {
            Some(format!("分片编号越界：seq {seq} ≥ 共 {}", r.total_chunks))
        } else if seq + 1 < r.total_chunks && data.len() as u64 != r.chunk_size {
            Some(format!(
                "非末片长度与声明的分片尺寸不符：{} ≠ {}",
                data.len(),
                r.chunk_size
            ))
        } else {
            let offset = u64::from(seq) * r.chunk_size;
            (offset + data.len() as u64 > r.expected_size)
                .then(|| "分片超出声明的文件尺寸".to_string())
        };
        if let Some(reason) = reason {
            r.abandon();
            self.reassemblies.remove(transfer_id);
            return ChunkOutcome::Rejected(reason);
        }
        let offset = u64::from(seq) * r.chunk_size;
        if let Err(e) = r.file.seek(SeekFrom::Start(offset)).and_then(|_| {
            r.file.write_all(data)?;
            Ok(())
        }) {
            // 写盘失败（多半是磁盘满）：这一单不可能自己好，删掉并让调用方标失败。
            let reason = format!("写入临时文件失败：{e}");
            r.abandon();
            self.reassemblies.remove(transfer_id);
            return ChunkOutcome::Rejected(reason);
        }
        r.received.insert(seq);
        r.received_bytes += data.len() as u64;
        if !r.complete() {
            return ChunkOutcome::Partial {
                received_bytes: r.received_bytes,
                total_bytes: r.expected_size,
            };
        }
        let r = self.reassemblies.remove(transfer_id).expect("刚拿_mut过");
        // 改名之前必须 flush 到盘：`file-done` 之后界面立刻给"打开文件"，
        // 而崩溃时留在 page cache 里的尾巴会变成"打开就是缺一段"。
        if let Err(e) = r.file.sync_all() {
            let _ = std::fs::remove_file(&r.path);
            return ChunkOutcome::Rejected(format!("落盘失败：{e}"));
        }
        ChunkOutcome::Complete {
            name: r.name.clone(),
            size: r.expected_size,
            path: r.path.clone(),
        }
    }

    /// 回收在 `cutoff` 之前开始、且仍未完成的重组（**连 `.part` 一起删**）。
    ///
    /// 返回被清掉的 transfer_id 列表（2026-09-23 审计 A2：调用方要据此给
    /// file_transfers 里仍 active 的行标失败终态——静默消失 = 前端永久卡 X%）。
    pub fn sweep_stale_reassemblies(&mut self, cutoff: i64) -> Vec<String> {
        let mut removed = Vec::new();
        let mut stale: Vec<PathBuf> = Vec::new();
        self.reassemblies.retain(|k, r| {
            let keep = r.created_at > cutoff;
            if !keep {
                removed.push(k.clone());
                stale.push(r.path.clone());
            }
            keep
        });
        // 先摘表、后删文件：`remove` 会 drop 掉 `File` 关掉句柄，反过来的顺序在
        // Windows 上会撞"文件正被占用"而删不掉。
        for path in stale {
            let _ = std::fs::remove_file(path);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每用例一个独立临时目录（`.part` 是真文件，共用目录会互相看见）。
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("gosslan-relay-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const CS: u32 = 8;

    fn begin(m: &mut RelayManager, dir: &Path, id: &str, name: &str, total: u32, size: u64) {
        m.begin_reassemble(id, name, total, size, CS, dir).unwrap();
    }

    /// 分片必须**边收边写盘**：完成时给出的是文件路径，而不是整个文件的字节。
    /// 这条判据就是 P4 本身 —— 原先 `chunks: HashMap<u32, Vec<u8>>` 把整份文件留在内存，
    /// 600MB 文件经邻居借用要 1.2GB 内存（重组时还要再组装一份），移动端必被系统杀掉。
    #[test]
    fn chunks_stream_to_disk_and_completion_hands_back_a_path() {
        let dir = TmpDir::new("stream");
        let mut m = RelayManager::new();
        let data: Vec<u8> = (0u8..20).collect();
        begin(&mut m, &dir.0, "t1", "f.bin", 3, data.len() as u64);

        // 乱序 + 中途只写了 1 片时，磁盘上那个 .part 必须已经存在并且是预分配的全尺寸
        assert!(matches!(
            m.add_chunk("t1", 2, &data[16..]),
            ChunkOutcome::Partial { .. }
        ));
        let part = dir.0.join("t1.relay.part");
        assert!(
            part.exists(),
            "收到第一片就该有 .part 文件，而不是等收完才写"
        );
        assert_eq!(std::fs::metadata(&part).unwrap().len(), data.len() as u64);

        assert!(matches!(
            m.add_chunk("t1", 0, &data[0..8]),
            ChunkOutcome::Partial { .. }
        ));
        let out = match m.add_chunk("t1", 1, &data[8..16]) {
            ChunkOutcome::Complete { name, size, path } => {
                assert_eq!(name, "f.bin");
                assert_eq!(size, data.len() as u64);
                path
            }
            other => panic!("应当完成，实际 {other:?}"),
        };
        assert_eq!(
            std::fs::read(&out).unwrap(),
            data,
            "按 seq 落盘的字节必须还原成原文件"
        );
    }

    /// 重复分片（多邻居泛洪同一条路径各送一份）必须**忽略**：不重复计进度、不重写。
    #[test]
    fn duplicate_chunk_is_ignored_not_double_counted() {
        let dir = TmpDir::new("dup");
        let mut m = RelayManager::new();
        begin(&mut m, &dir.0, "t1", "f.bin", 2, 12);
        assert!(matches!(
            m.add_chunk("t1", 0, &[1u8; 8]),
            ChunkOutcome::Partial {
                received_bytes: 8,
                total_bytes: 12,
            }
        ));
        assert!(matches!(
            m.add_chunk("t1", 0, &[1u8; 8]),
            ChunkOutcome::Duplicate
        ));
        // 完成时文件仍是 12 字节（第二片只写了 4 字节）
        match m.add_chunk("t1", 1, &[2u8; 4]) {
            ChunkOutcome::Complete { size, path, .. } => {
                assert_eq!(size, 12);
                assert_eq!(std::fs::read(&path).unwrap().len(), 12);
            }
            other => panic!("{other:?}"),
        }
    }

    /// 形状不合法的分片必须**当场判死这一单**并删掉 .part：
    /// `size` / `total_chunks` / `chunk_size` 三样都由对端声明，任何不自洽的组合
    /// 都不许变成"往声明之外的偏移写"—— 那正是把磁盘写爆的入口。
    #[test]
    fn malformed_shapes_are_rejected_and_the_part_file_is_gone() {
        let dir = TmpDir::new("shape");
        let mut m = RelayManager::new();

        // ① seq 越界
        begin(&mut m, &dir.0, "a", "f.bin", 2, 16);
        assert!(matches!(
            m.add_chunk("a", 9, &[0u8; 8]),
            ChunkOutcome::Rejected(_)
        ));
        assert!(
            !dir.0.join("a.relay.part").exists(),
            "判死后必须删 .part，不能留垃圾"
        );

        // ② 非末片长度不等于 chunk_size（说明两端的分块口径根本不一致）
        begin(&mut m, &dir.0, "b", "f.bin", 3, 24);
        assert!(matches!(
            m.add_chunk("b", 0, &[0u8; 5]),
            ChunkOutcome::Rejected(_)
        ));

        // ③ 末片超长：offset + len > size
        begin(&mut m, &dir.0, "c", "f.bin", 2, 12);
        assert!(matches!(
            m.add_chunk("c", 1, &[0u8; 8]),
            ChunkOutcome::Rejected(_)
        ));

        // ④ 没有活跃会话（offer 从未到达 / 已被回收）
        assert!(matches!(
            m.add_chunk("zz", 0, &[0u8; 8]),
            ChunkOutcome::Unknown
        ));
    }

    /// **`chunk_size == 0` 必须当场拒绝**：那是"对端没声明分片尺寸"的编码，
    /// 也就是 4 个版本之前的老对端。没有这个数就落不出正确的偏移，
    /// 而宁可可报错也不能退回"整份进内存" —— 那条路就是 P4 的 OOM 本身。
    #[test]
    fn offer_without_chunk_size_is_refused_instead_of_buffering() {
        let dir = TmpDir::new("nocs");
        let mut m = RelayManager::new();
        assert!(m.begin_reassemble("t1", "f.bin", 2, 16, 0, &dir.0).is_err());
        assert!(!dir.0.join("t1.relay.part").exists());
    }

    /// `transfer_id` 会变成文件名，必须与直连路径同一份消毒口径
    /// （`safe_transfer_id`）：`../escape` 这类 id 一旦放行，就是"对端一句话把文件
    /// 写到下载目录之外"。
    #[test]
    fn unsafe_transfer_id_cannot_create_a_file_outside_the_dir() {
        let dir = TmpDir::new("sanitize");
        let mut m = RelayManager::new();
        for bad in ["../escape", "a/b", "", &"x".repeat(65)] {
            assert!(
                m.begin_reassemble(bad, "f.bin", 1, 8, CS, &dir.0).is_err(),
                "非法 transfer_id {bad:?} 必须被拒"
            );
        }
        assert_eq!(
            read_dir_names(&dir.0),
            Vec::<String>::new(),
            "非法 id 不该留下任何文件"
        );
    }

    /// 回收过期重组时**连 .part 一起删**（只从表里摘掉 = 磁盘上永久留一份占满的文件）。
    #[test]
    fn sweep_removes_the_part_files_too() {
        let dir = TmpDir::new("sweep");
        let mut m = RelayManager::new();
        begin(&mut m, &dir.0, "fresh", "a.bin", 2, 16);
        begin(&mut m, &dir.0, "stale", "b.bin", 2, 16);
        m.reassemblies.get_mut("stale").unwrap().created_at = 1_000;
        assert_eq!(
            m.sweep_stale_reassemblies(2_000),
            vec!["stale".to_string()],
            "只清过期的那条，并报出它的 id（调用方要据此给传输行标失败终态）"
        );
        assert!(dir.0.join("fresh.relay.part").exists(), "进行中的不得被清");
        assert!(
            !dir.0.join("stale.relay.part").exists(),
            "过期的必须连文件一起删"
        );
        assert!(m.sweep_stale_reassemblies(2_000).is_empty());
    }

    /// 重复的 RelayFileOffer（多邻居泛洪）不得清空已收到的切片、也不得重开文件
    /// （`set_len` 会把已写内容截掉）。
    #[test]
    fn begin_reassemble_is_idempotent() {
        let dir = TmpDir::new("idem");
        let mut m = RelayManager::new();
        begin(&mut m, &dir.0, "t1", "f.bin", 2, 12);
        assert!(matches!(
            m.add_chunk("t1", 0, &[7u8; 8]),
            ChunkOutcome::Partial { .. }
        ));
        begin(&mut m, &dir.0, "t1", "f.bin", 2, 12); // 重复的 offer
        match m.add_chunk("t1", 1, &[9u8; 4]) {
            ChunkOutcome::Complete { path, .. } => {
                let got = std::fs::read(&path).unwrap();
                assert_eq!(
                    got,
                    [vec![7u8; 8], vec![9u8; 4]].concat(),
                    "已收的切片不能被重复 offer 抹掉"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// 0 字节文件：`total_chunks` 发送侧固定 `.max(1)`，所以这里是"一片、长度为 0"。
    /// 不许因为"空片"被判成形状不合法（用户发一个空文件是完全正常的操作）。
    #[test]
    fn zero_byte_file_completes() {
        let dir = TmpDir::new("zero");
        let mut m = RelayManager::new();
        begin(&mut m, &dir.0, "t1", "empty.bin", 1, 0);
        match m.add_chunk("t1", 0, &[]) {
            ChunkOutcome::Complete { size, path, .. } => {
                assert_eq!(size, 0);
                assert_eq!(std::fs::read(&path).unwrap().len(), 0);
            }
            other => panic!("空文件应当正常完成：{other:?}"),
        }
    }

    fn read_dir_names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    }
}
