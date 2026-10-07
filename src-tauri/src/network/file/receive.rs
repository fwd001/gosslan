// 接收侧：offer 决策 → 建 `.part` 接收器 → 逐片解密落盘 → 校验改名 → 失败与接管。
//
// 为什么单独一册：这一册的不变量集中在**幂等与磁盘证据**（重发的片要丢、`FileDone` 必须
// 字节数与 sha256 都对才 rename、被别的接收器占用的会话要怎么接管），与发送侧的"进度记账"
// 是两套判据；`stream_file` 的早退与这里的 `fail_taken_receive` 隔了 600 行时最难核对。
//

// 恒等判据（与 transport 那五刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（本模块的锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 搬家同批必须做的两件事：`network/mod.rs::file_src_for_guards()` 登记本册（漏了=形状守卫看不见这段生产码，假绿），
//   以及 `docs/domains.data.mjs` 的 transport/file 领域 paths（漏了=判据 D 报无主文件）。

/// 该 transfer_id 已保留的 .part 前缀字节数（无则 0）。
///
/// 用于把"接收端已持有多少"回给发送端（FileReject.received），让发送端从真实进度续发。
pub fn retained_part_len(state: &AppState, transfer_id: &str) -> u64 {
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    std::fs::metadata(dl.join(format!("{transfer_id}.part")))
        .map(|m| if m.is_file() { m.len() } else { 0 })
        .unwrap_or(0)
}

/// 定期清扫：删除超过 TTL、且当前不在接收中的 .part。
///
/// 为什么需要（审计 §7 风险 2）：可恢复失败会**保留** .part 作续传前缀，若对端一去不回，
/// 这些前缀会一直占空间。清扫只删"够旧 + 没人正在用"的，绝不碰活跃接收。
pub fn sweep_stale_parts(state: &AppState) -> usize {
    const TTL_MS: u64 = 24 * 60 * 60 * 1000;
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let active: std::collections::HashSet<String> = {
        let a: Vec<String> = state
            .file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        let b: Vec<String> = state
            .group_file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        a.into_iter().chain(b).collect()
    };
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(&dl) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("part") {
                continue;
            }
            if let Some(tid) = path.file_stem().and_then(|s| s.to_str()) {
                if active.contains(tid) {
                    continue;
                }
            }
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_millis() as u64 > TTL_MS)
                .unwrap_or(false);
            if stale && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// 断点续传：从已保留的 .part 前缀继续接收。
///
/// 与 begin_receive 的区别：**不再 truncate**，而是读入已有前缀播种 hasher，
/// received 从 from_bytes 接上；next_seq 归零（发送端从 from_seq=0 重编，只对本段排序）。
/// 任何不一致都返回 Err ⇒ 上层回 FileReject ⇒ 发送端整份重传（安全兜底）。
#[allow(clippy::too_many_arguments)]
pub fn resume_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
    from_bytes: u64,
) -> Result<PathBuf, String> {
    const TTL_MS: i64 = 24 * 60 * 60 * 1000;
    let safe_name = safe_file_name(name).ok_or("文件名非法")?;
    // 续传同样要落 `{id}.part`，消毒口径必须与 make_receiver 完全一致 ——
    // 否则「首次收被拦、续传绕过」就会留下一条可用的攻击路径。
    let transfer_id = safe_transfer_id(transfer_id).ok_or("传输标识非法")?;
    let transfer_id = transfer_id.as_str();
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let tmp_path = dl.join(format!("{transfer_id}.part"));
    let meta = std::fs::metadata(&tmp_path).map_err(|_| "续传前缀不存在".to_string())?;
    if !meta.is_file() || meta.len() != from_bytes || from_bytes == 0 || from_bytes > size {
        return Err("续传前缀与请求不一致".to_string());
    }
    // TTL：太旧的前缀不复用（避免无穷增长），删掉并让上层整份重传。
    if let Ok(modified) = meta.modified() {
        if let Ok(age) = modified.elapsed() {
            if age.as_millis() as i64 > TTL_MS {
                let _ = std::fs::remove_file(&tmp_path);
                return Err("续传前缀已过期".to_string());
            }
        }
    }
    // ⚠️ **分块喂哈希器，不整读进内存**。这曾经是 `std::fs::read(&tmp_path)`：
    // `.part` 前缀最长就等于整个文件，于是「几个 GB 的文件传到 90% 断链、对端续传」
    // 会让本进程瞬间占用 ≈ 文件大小的内存 —— 而续传恰恰是为了处理这种大文件场景。
    let hasher = {
        use sha2::Digest as _;
        let mut h = sha2::Sha256::new();
        let mut src = std::fs::File::open(&tmp_path).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; FILE_CHUNK];
        let mut counted: u64 = 0;
        loop {
            let n = src.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            counted += n as u64;
        }
        // 只记录、不改行为：`received` 仍以 from_bytes 为准（发送端的分片编号是据此推出来的，
        // 这里单方面改会让两端的 seq 对不上）。不一致说明该 .part 已被外部改动，
        // 后续 SHA-256 整体校验会拦下，此处先留下可诊断的痕迹。
        if counted != from_bytes {
            state.logger.warn(
                "file",
                format!(
                    "续传前缀长度与声明不符：磁盘 {counted} 字节 / 声明 {from_bytes} 字节（transfer={transfer_id}）"
                ),
            );
        }
        h
    };
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(&tmp_path)
        .map_err(|e| e.to_string())?;
    let final_path = unique_path(&dl, &safe_name);
    state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id);
    state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            transfer_id.to_string(),
            FileReceiver {
                file: f,
                name: safe_name,
                size,
                received: from_bytes,
                next_seq: 0,
                attempt: 0,
                stale_dropped: 0,
                tmp_path,
                final_path: final_path.clone(),
                peer_id: peer_id.to_string(),
                last_report_ms: crate::db::now_ms(),
                fed_at_ms: crate::db::now_ms(),
                file_key,
                expected_sha256,
                hasher,
            },
        );
    Ok(final_path)
}

/// 接收方：准备接收文件，返回最终落盘路径。
pub fn begin_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
) -> Result<PathBuf, String> {
    let final_path = make_receiver(
        state,
        transfer_id,
        peer_id,
        name,
        size,
        file_key,
        expected_sha256,
        &state.file_receivers,
    )?;
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            name,
            size,
            "receive",
            "active",
            Some(final_path.to_string_lossy().as_ref()),
            0.0,
        )
        .ok();
    }
    Ok(final_path)
}

/// 构造接收端状态（路径安全 + `.part` 创建 + 插入对应接收表），
/// 收到 `FileOffer` 时接收端的答复。
#[derive(Debug, PartialEq, Eq)]
pub enum OfferDecision {
    /// 回 `FileReject { received = 文件总大小 }`：本机已完整收下这份内容，
    /// 发送端不得再发任何分片（收到 `received ≥ size` 应直接宣布完成）。
    ///
    /// 为什么必须有（2026-09-23 审计 A6）：旧判据只有「活跃接收器 / .part 前缀 /
    /// from_bytes」三输入——**没有"本机已收完"**。收完之后 `.part` 已改名、接收器
    /// 已清空 ⇒ 三输入全归零 ⇒ 重复 Offer 判成 Accept ⇒ 整份重推落「名字(1)」副本
    /// （重复 Offer 的常见来源：A4 丢回执 → 发送端 outbox 重试）。
    AlreadyHave,
    /// 回 `FileAccept`，其它什么都不动（全新，或"同一起点的重复 offer"）。
    Accept,
    /// 回 `FileAccept`，**并且**把活跃接收器切到"新段从 `seq = 0` 重编"。
    ///
    /// 为什么必须有这一档：发送端续传时分片编号是按段从 0 重编的（`FileOffer.from_seq` 恒为 0）。
    /// 活跃接收器的 `next_seq` 还停在上一段末尾 ⇒ 不重置就会把这些**新数据**判成"迟到的重复片"
    /// 静默丢掉，文件永远差一截，而且不报错。
    AcceptResumeSegment,
    /// 回 `FileReject { received = 本端真实已收字节 }`：让发送端从真实位置续发。
    ResumeFrom(u64),
}

/// 「收到 offer 时该怎么答」的**唯一**判据（纯函数，能被单测直接钉住）。
///
/// 一句话：**接收端是"我有什么"的唯一权威，而且每次都要把这个回答出去。**
/// 之前这里有两套规矩 —— 没有活跃接收器时比对 `.part` 前缀、有活跃接收器时**一律** `Accept`
/// —— 后者在真机上把 160MB 的大文件判了死刑：发送端每一轮重试都从 `from_bytes = 0` 重发
/// （`flush_pending_files` 走的就是不带位置的 `send_file_from_path`），接收端回 Accept
/// 之后把那 40MB 已收前缀当"迟到的重复片"静默丢掉（`chunk_seq_decision` 的 `Duplicate` 分支），
/// 于是**每一轮都要重传一遍已收部分**，慢链路上永远跑不完 ⇒ 界面恒 0%、最后报"分片失败"。
///
/// `held` 的取法也是判据的一部分：有活跃接收器时用**内存里的 `received`**（每片 `write_all`
/// 之后就更新），没有时才退回磁盘 `.part` 的大小 —— 报小了会让发送端重灌已写进文件的字节，
/// 文件超长、SHA-256 必不匹配；报大了会让发送端以为对端有它没有的东西，永远等不齐。
///
/// ⚠️ 归零只在 `from_bytes > 0` 时做，无条件归零会引入另一个故障：上一轮 attempt 被 timeout
/// 丢掉时它**已入队的分片还在 writer_loop 里往外排**（队列 1024 槽 ≈ 262MB，丢 future 不排空
/// 队列），那些片的 seq 已到 160+，此时因一个 `from_bytes = 0` 的重复 offer 把 `next_seq`
/// 拍回 0 ⇒ 它们变成「跳号」⇒ `Err(文件分片顺序错误)` ⇒ 整单被判死。
///
/// ⚠️ **磁盘前缀的长度上限也是判据的一部分**（A-12，2026-09-26 由注入⑧在真实双实例照出）：
/// `FileReject.received` 承载两个含义 —— 发送端把 `received >= size` 读成"对方已完整收下"
/// 并直接收尾（记 done、**删掉队列行**）。所以"`.part` 字节数够了"绝不能当续传位置报出去：
/// 上一次收尾 `rename` 失败（目录只读 / 磁盘满 / 进程被杀在半步）时字节是整份的，
/// **但磁盘上没有成品文件**，而"成品"才是「rename 才算完成」的唯一凭证。
/// 那一格实测的形状就是这条判据要拦的：`A=done 且队列行已删 / B=failed / .part 整份 / final 不存在`
/// —— 两边各给了用户一个结论，而且互相矛盾，且再也没人重试。
/// 故：≥ size 的磁盘前缀**不算进度**，按 0 处理 ⇒ 重新整份收，走真实的哈希校验与真实的 rename。
/// 活跃接收器的内存计数不走这条（它收完那一刻收尾就已经跑过了，见 `write_chunk` 的裁决分支）。
pub fn decide_offer(
    has_active: bool,
    active_received: u64,
    disk_retained: u64,
    size: u64,
    from_bytes: u64,
    already_completed: bool,
) -> OfferDecision {
    // 「本机已收完」优先于一切位置判据（审计 A6）：收完后三输入全归零，
    // 任何位置比较都会退化成"整份重推"。
    if already_completed {
        return OfferDecision::AlreadyHave;
    }
    let held = if has_active {
        active_received
    } else if size > 0 && disk_retained >= size {
        0
    } else {
        disk_retained
    };
    if from_bytes != held {
        return OfferDecision::ResumeFrom(held);
    }
    if has_active && from_bytes > 0 {
        return OfferDecision::AcceptResumeSegment;
    }
    OfferDecision::Accept
}

/// 该 transfer 当前是否有活跃接收器，以及它真实收下了多少字节。
///
/// 返回 `None` = 没有活跃接收器（此时 `.part` 磁盘前缀才是唯一事实）。
pub fn receiver_progress(state: &AppState, transfer_id: &str) -> Option<u64> {
    state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(transfer_id)
        .map(|r| r.received)
}

/// 把活跃接收器切到"新的一段从 `seq = 0` 重新编号"，**不动**已收字节、文件位置与增量哈希。
///
/// 为什么必须做：发送端续传时分片编号是**按段**从 0 重编的（`FileOffer.from_seq` 恒为 0，
/// `stream_file` 也只按 `from_bytes` 定位文件偏移）。接收器如果还留着上一段推进到的
/// `next_seq = 160`，新段那些从 0 开始、内容其实是**新数据**的分片会被
/// `chunk_seq_decision` 判成"迟到的重复片"静默丢掉 ⇒ 文件永远差一截。
/// 只在"位置对得上、决定 Accept 而接收器已推进过"的情况下调用；重置的是段号，不是进度。
pub fn restart_segment(state: &AppState, transfer_id: &str) {
    if let Some(r) = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(transfer_id)
    {
        r.next_seq = 0;
    }
}

/// 一对一与群文件共用；差异只在写入哪个接收 map 与是否记录 file_transfers。
#[allow(clippy::too_many_arguments)]
fn make_receiver(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
    receivers: &std::sync::Mutex<HashMap<String, FileReceiver>>,
) -> Result<PathBuf, String> {
    let safe_name = safe_file_name(name).ok_or("文件名非法")?;
    // transfer_id 会成为 `{id}.part` 的文件名，必须与文件名同级消毒（见 safe_transfer_id）
    let transfer_id = safe_transfer_id(transfer_id).ok_or("传输标识非法")?;
    let transfer_id = transfer_id.as_str();
    if size > i64::MAX as u64 {
        return Err("文件过大，无法安全保存".to_string());
    }
    // 同一个 transfer_id 又收到一次 Offer = 发送方在**重试**（它每次都从 seq 0 重新开始）。
    // 旧实现这里直接返回「重复的文件传输」⇒ 接收方拒收 ⇒ 发送方 15s 等 accept 超时 ⇒
    // 可恢复失败 ⇒ 再重试 —— 死循环；而新 attempt 的分片与旧状态交错，就报出
    // 「文件分片顺序错误」。现在：把旧状态丢掉、从零重新开始。
    // 安全性：临时文件按 `transfer_id` 命名，下面 `File::create` 会**截断**它，
    // 新 attempt 不会与旧字节混写；`final_path` 仍走 `unique_path`（旧 final 未完成 ⇒ 不存在）。
    if let Some(old) = receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id)
    {
        let _ = std::fs::remove_file(&old.tmp_path);
    }
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    std::fs::create_dir_all(&dl).ok();
    let final_path = unique_path(&dl, &safe_name);
    // ⚠️ 临时文件必须按 **transfer_id** 命名，不能从 final_path 派生：
    // 两张同名图同时在途时，begin 那一刻磁盘上还没有同名文件 → unique_path 会给两者同一个 final_path
    // → 由它派生的 .part 也相同 → 两份字节交错写进同一文件 → sha256 校验失败
    // （表现为"第一张能看、第二张加载不出来"）。
    let tmp_path = dl.join(format!("{transfer_id}.part"));
    let f = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;

    receivers.lock().unwrap_or_else(|e| e.into_inner()).insert(
        transfer_id.to_string(),
        FileReceiver {
            file: f,
            name: safe_name,
            size,
            received: 0,
            next_seq: 0,
            attempt: 0,
            stale_dropped: 0,
            tmp_path: tmp_path.clone(),
            final_path: final_path.clone(),
            peer_id: peer_id.to_string(),
            last_report_ms: 0,
            fed_at_ms: crate::db::now_ms(),
            file_key,
            expected_sha256,
            hasher: {
                use sha2::Digest as _;
                sha2::Sha256::new()
            },
        },
    );
    Ok(final_path)
}

/// 收到一片文件分片时该怎么处理（**纯函数**，便于单测钉住"重复不致命"这条规则）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChunkSeq {
    /// 重复或迟到的分片（`seq` 落在已收范围内）⇒ **忽略**，绝不让整单失败。
    Duplicate,
    /// 正好是下一片 ⇒ 写入。
    Accept,
    /// 跳号（中间真缺片）⇒ 只能靠重传解决。
    Gap,
}

/// 判据：`seq < next_seq` **忽略**、相等**接受**、大于**报错**。
///
/// 为什么"重复"不能报错（2026-09-13 审计的真缺陷）：发送方一次 attempt 超时后会**从头重传**
/// （`seq` 从 0 重来），上一轮的残片可能仍在链路上。旧实现一律报「文件分片顺序错误」，
/// 于是接收方整单失败、状态被清 ⇒ 新 attempt 也永远拼不齐 ⇒
/// **BLE 上 >20KB 的文件事实上永远传不完**（用户实测的那条报错就是它）。
/// 只挡"跳号"仍然安全：整份字节由文件级 SHA-256 兜底校验。
pub(crate) fn chunk_seq_decision(seq: u32, next_seq: u32) -> ChunkSeq {
    use std::cmp::Ordering;
    match seq.cmp(&next_seq) {
        Ordering::Less => ChunkSeq::Duplicate,
        Ordering::Equal => ChunkSeq::Accept,
        Ordering::Greater => ChunkSeq::Gap,
    }
}

/// 这片明文的长度会不会**越过声明的 `size`**（§七「错误 size」那格唯一的裁决点）。
///
/// 为什么这条判据必须在写盘**之前**、而不是留到收尾的"字节数与声明不符"：`.part` 是按片增长的，
/// 不设上限就等于让对端决定这台机器往磁盘上写多少字节，而用户看到的是一条会跑到 100% 再失败的单。
///
/// ⚠️ 边界刻意取"恰好填满 = 放行"：`stream_file` 的最后一片通常正好把 `size` 补齐，
/// 写成 `>=` 会让**每一单**都在最后一片上打死（与 [`chunk_seq_decision`] 只挡真空缺是同一类设计）。
pub(crate) fn chunk_exceeds_declared(size: u64, received: u64, plaintext_len: u64) -> bool {
    plaintext_len > size.saturating_sub(received)
}

/// 这一帧属于**当前这一轮**发送尝试吗（attempt epoch 判据，2026-09-23 真机 600MB 复核）。
///
/// 病根：一轮超时后 outbox 重投，但**上一轮已经塞进链路队列的分片不会被撤回**
/// （Low 队列 1024 槽 ≈ 262MB）。接收端每段的 `seq` 都是从 0 重编的，于是旧轮的高 `seq`
/// 落在新轮的 `next_seq` 之上 ⇒ 判成"跳号"⇒ 整单被打死。有了轮次号就能把它安静丢掉。
///
/// 三条刻意的设计：
/// 1. `None`（老端不带这个字段，或对方没声明 `CONTENT_FEATURE_FILE_EPOCH`）⇒ **恒真**，
///    完全退回旧语义 —— 新 behaviour 只在两端都支持时才生效。
/// 2. 用 `==` 而不是 `>=`：**比本机新的轮次也算"不是我的"**。"未来的分片"只可能是它的
///    Offer 还在另一条链路上排队；先收下会把文件拼坏，而拼坏由末尾 SHA 兜住 ⇒ 宁可丢这一片，
///    等它自己的 Offer 到达后由发送端从 `.part` 前缀续发（自愈，最多多一轮）。
/// 3. **Offer 永远照单全收并借此设定轮次**（不走这个判据）：否则本机重启后计数器回到 1，
///    而接收端存的是上一轮的 5 ⇒ 新 Offer 被判陈旧 ⇒ 这份文件永久饿死。Offer 是权威，
///    分片/完成帧才是被过滤的对象。
pub(crate) fn frame_is_current(frame: Option<u32>, current: u32) -> bool {
    match frame {
        Some(a) => a == current,
        None => true,
    }
}

/// `None`（老端不带 attempt）在本机记作这一轮。
pub const LEGACY_ATTEMPT: u32 = 0;

/// Offer 一到达就把接收器的"当前轮次"设成它带来的值 —— **Offer 是权威，不参与过滤**
/// （见 `frame_is_current` 第 3 条：否则本机重启后计数器回到小值，新 Offer 会被自己判成陈旧，
/// 那份文件就永久饿死）。
pub fn note_offer_attempt(state: &AppState, transfer_id: &str, attempt: Option<u32>) {
    if let Some(r) = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(transfer_id)
    {
        r.attempt = attempt.unwrap_or(LEGACY_ATTEMPT);
    }
}

/// `FileDone` 是否属于当前轮次（只读，不摘接收器）。
///
/// 为什么必须在 `finish_receive` **之前**判：那份函数一进来就把接收器摘掉了，
/// 陈旧完成帧会被当成"重复 FileDone"去补一个成功 Ack，或更糟 —— 拿这一轮刚开头的
/// `received ≠ size` 把整单打死。没有接收器时返回 true，交给原有"未知传输"分支处理。
pub fn done_is_current(state: &AppState, transfer_id: &str, attempt: Option<u32>) -> bool {
    let recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match recv.get(transfer_id) {
        Some(r) => frame_is_current(attempt, r.attempt),
        None => true,
    }
}

/// 接收方：写入一个分片，返回累计字节数。
/// 入参 `data` 为 AEAD 密文（nonce || ciphertext）：先解密再写盘，
/// 解密失败直接报错——密文绝不落盘。
pub fn write_chunk(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    seq: u32,
    data: &[u8],
    attempt: Option<u32>,
) -> Result<u64, String> {
    use std::io::Write;
    // 锁作用域：先算完，把要落库的进度取出来，**释放 file_receivers 锁之后**再动 db
    // （避免 file_receivers -> db 的嵌套锁顺序）。
    let (received, cid, owner, report) = {
        let mut recv = state
            .file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = recv.get_mut(transfer_id).ok_or("未知传输")?;
        if r.peer_id != peer_id {
            return Err("文件传输来源不匹配".to_string());
        }
        // **上一轮 attempt 的残留分片**安静丢掉（2026-09-23 真机 600MB 的根治，attempt epoch）。
        // 不丢会怎样：它们带着上一轮的高 `seq`，落到下面的判据里就是"跳号"⇒ 整单打死 ——
        // 而旧注释里"重传时 seq 从 0 重来 ⇒ 残片算 Duplicate"那条推理，只在残片**先到**、
        // 新轮 Offer 把 next_seq 归零**之后**才成立；队列里还压着最多 262MB 时顺序是反的。
        if !frame_is_current(attempt, r.attempt) {
            if r.stale_dropped == 0 {
                state.logger.info(
                    "file",
                    format!(
                        "丢掉非当前轮次的分片（本机在接收第 {} 轮）transfer={transfer_id}",
                        r.attempt
                    ),
                );
            }
            r.stale_dropped += 1;
            return Ok(r.received);
        }
        // 重复/迟到的分片必须**忽略**，而不是整单失败：发送方一次 attempt 超时后会**从头重传**
        // （seq 从 0 重来），而上一轮的残片可能仍在链路上。只挡"跳号"（真缺片，只能重传）；
        // 整份字节仍由文件级 SHA-256 兜底。
        match chunk_seq_decision(seq, r.next_seq) {
            ChunkSeq::Duplicate => return Ok(r.received),
            ChunkSeq::Gap => return Err("文件分片顺序错误".to_string()),
            ChunkSeq::Accept => {}
        }
        let plaintext = crypto::open_symmetric(&r.file_key, data)
            .ok_or_else(|| "文件分片解密失败".to_string())?;
        if chunk_exceeds_declared(r.size, r.received, plaintext.len() as u64) {
            return Err("文件分片超出声明大小".to_string());
        }
        // 文件级完整性：明文增量哈希（与写盘同一份数据，无二次磁盘读取）
        use sha2::Digest;
        r.hasher.update(&plaintext);
        r.file.write_all(&plaintext).map_err(|e| e.to_string())?;
        r.received += plaintext.len() as u64;
        // 每片都记一次"还被喂得动"—— 这是 `receive_is_stale` 唯一的证据来源。
        // 代价是一次 epoch 毫秒读取，相对上面的 `write_all` 可以忽略。
        r.fed_at_ms = crate::db::now_ms();
        r.next_seq = r.next_seq.checked_add(1).ok_or("文件分片序号溢出")?;
        // 节流 500ms 落一次进度：这是断点续传的起点，也让统一状态显示真实进度。
        let now = crate::db::now_ms();
        let report = now - r.last_report_ms >= 500;
        if report {
            r.last_report_ms = now;
        }
        (
            r.received,
            r.expected_sha256.clone(),
            r.peer_id.clone(),
            report,
        )
    };
    if report {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = crate::content::store::touch_received(
            &dbc,
            &cid,
            &owner,
            received,
            crate::db::now_ms(),
        );
    }
    Ok(received)
}

/// 终止损坏或超时的接收：摘表 → 收尾（见 `fail_taken_receive`）。
pub fn fail_receive(state: &AppState, transfer_id: &str, peer_id: &str, reason: &str) -> bool {
    let mut recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(r) = recv.remove(transfer_id) else {
        return false;
    };
    if r.peer_id != peer_id {
        recv.insert(transfer_id.to_string(), r);
        return false;
    }
    drop(recv);
    fail_taken_receive(state, transfer_id, &r, reason);
    true
}

/// 静默接收器的**原子**回收单位：判据与摘表在同一次持锁里完成。
///
/// 为什么不能"先快照一批 id、再逐个收尾"：那两步之间完全可以挤进一个新 FileOffer
/// （同一 transfer_id 重新建接收器、`fed_at_ms` 就是现在）—— 按 id 收尾会把那个
/// **正在收**的传输判死。宁可每轮只摘一个、循环到没有，也不留这个缝。
pub fn take_stalled_receive(state: &AppState, now: i64) -> Option<(String, FileReceiver)> {
    let mut recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = recv
        .iter()
        .find(|(_, r)| receive_is_stale(now - r.fed_at_ms))
        .map(|(k, _)| k.clone())?;
    recv.remove(&key).map(|r| (key, r))
}

/// 摘表之后的收尾：落 failed 终态、记 Incomplete、emit。
/// `fail_receive` 与清扫器共用这一份 —— 终态口径不许有两套。
pub fn fail_taken_receive(state: &AppState, transfer_id: &str, r: &FileReceiver, reason: &str) {
    // **保留 .part**（不删）：这是断点续传的前缀。只有"确定是永久失败"（校验不符）
    // 才删；超时/断链属于可恢复。陈旧 .part 由 resume_receive 的 TTL 与后续清理收割。
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        // 注意：**不能**把 .part 写进 path —— find_source 只看 path 非空就当作可服务内容，
        // 那样会把"半截文件"当成完整种子发出去。.part 的位置由 transfer_id 推导。
        let _ = &r.tmp_path;
        // 统一状态：中途失败/超时/断链 ⇒ **Incomplete**（可恢复）。
        // 于是建链时 retry_incomplete_content 会按退避自动重取，而不是永远停在 Active。
        let _ = crate::content::store::record_failure(
            &dbc,
            &r.expected_sha256,
            &r.peer_id,
            crate::content::model::Direction::Receive,
            crate::content::model::FailReason::Partial,
            db::now_ms(),
        );
    }
    emit_failed(state, transfer_id, reason);
}

/// 对端断链时终止其所有未完成接收，避免下载目录长期堆积临时文件。
/// 对端确认一条链路都不剩时，**原子摘取**它名下全部单聊接收（判据与摘表同一次持锁）。
///
/// 与群侧 `take_group_receives_for_peer` 同一族。旧写法是"锁内 `collect()` 出 id、锁外逐个
/// `fail_receive`"：`fail_receive` 虽然会核对 `peer_id`（别人的原样插回），但**同 peer、同
/// transfer_id 的新 attempt** 正好可以在那两步之间建起来 ⇒ 上一轮的 teardown 会把这一轮
/// 正在收的传输判死。后果有界（内容记 Incomplete、建链时按退避自动重取），
/// 但"多等一轮"本身就是用户看到的"下载卡在重试"。
pub fn take_receives_for_peer(state: &AppState, peer_id: &str) -> Vec<(String, FileReceiver)> {
    let mut recv = state
        .file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = recv
        .iter()
        .filter(|(_, r)| r.peer_id == peer_id)
        .map(|(k, _)| k.clone())
        .collect();
    ids.into_iter()
        .filter_map(|k| recv.remove(&k).map(|r| (k, r)))
        .collect()
}

pub fn fail_receives_for_peer(state: &AppState, peer_id: &str) {
    // 摘与收尾分开：摘在锁内一次做完，收尾（写库 + emit）一律在锁外。
    for (id, r) in take_receives_for_peer(state, peer_id) {
        fail_taken_receive(state, &id, &r, "对端连接已断开");
    }
}

/// 接收方：收尾，返回 (name, size, final_path, peer_id)。
pub fn finish_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
) -> Result<Option<(String, u64, PathBuf, String)>, String> {
    // ⚠️ 锁只用来"把接收器摘出来"，摘完立刻放（2026-09-23 真机 600MB 复核）：
    // 下面这段是 SHA finalize + `sync_all()` + rename —— 600MB 的 fsync 是秒级慢活，
    // 而它跑在 reader_loop 里；持锁期间**同一时刻其它并发文件的 `write_chunk` 全部堵在同一把锁上**
    // ⇒ 那些传输不再写出 ⇒ 发送端 60s 停滞判据（`FILE_STALL_ABORT_MS`）把它们判死。
    // 摘出来之后这份接收器已经不在表里，锁外独占使用它是安全的。
    let r = {
        let mut recv = state
            .file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match recv.remove(transfer_id) {
            Some(r) => {
                if r.peer_id != peer_id {
                    recv.insert(transfer_id.to_string(), r);
                    return Err("文件传输来源不匹配".to_string());
                }
                r
            }
            None => return Ok(None),
        }
    };
    finish_receiver_into(&state.db, transfer_id, r).map(Some)
}

/// 摘出接收器之后的收尾：完整性裁决 → fsync → 改名落盘 → 落库终态。
///
/// 三个 `Err` 出口都必须让 `file_transfers` 停在 `failed`（而不是留在 `sending` / `done`）：
/// ① 字节数与声明的 `size` 不等（丢片/截断）；② 实际 SHA-256 与发送方声明不符
/// （**含**"发送方没声明"这一种 —— 空期望值不通过，fail-closed）；③ fsync 或 rename 失败。
/// 这三条合起来就是界面上那句「已收到」的证据，少任何一条都是假成功。
///
/// 为什么要独立成一个函数（#25）：它只要一个 `Mutex<Connection>`，不要 `AppState`
/// （那个要 tauri `AppHandle`，单测造不出来）⇒ 「显示成功是不是真成功」第一次可以拿
/// **生产码**驱动，而不是像现有几处接收端测试那样各自重写一遍操作序列（生产码改坏它们不红）。
/// 另外它拿不到 `file_receivers` 那张表 ⇒ "持锁 fsync" 那个形状不可能从这里长回去。
fn finish_receiver_into(
    db_lock: &std::sync::Mutex<rusqlite::Connection>,
    transfer_id: &str,
    mut r: FileReceiver,
) -> Result<(String, u64, PathBuf, String), String> {
    if r.received != r.size {
        let _ = std::fs::remove_file(&r.tmp_path);
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        return Err("文件传输未完成".to_string());
    }
    // 文件级完整性校验：实际 SHA-256 必须与发送方声明一致，否则不落盘
    {
        use sha2::Digest;
        let actual = r.hasher.clone().finalize();
        let actual_hex: String = actual.iter().map(|b| format!("{b:02x}")).collect();
        if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
            let _ = std::fs::remove_file(&r.tmp_path);
            let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
            db::upsert_transfer(
                &dbc,
                transfer_id,
                &r.peer_id,
                &r.name,
                r.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
            return Err("文件完整性校验失败".to_string());
        }
    }
    if let Err(e) = r.file.sync_all() {
        let reason = e.to_string();
        let _ = std::fs::remove_file(&r.tmp_path);
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        return Err(reason);
    }
    drop(r.file);
    // §七「两个 offer 都在任一次 rename 之前到达」：`final_path` 是 begin 时用 `unique_path` 定的，
    // 而那一刻两份都还没落地 ⇒ 同名两单会拿到**同一个**名字，直接 rename 会在 POSIX 上覆盖掉先落地的
    // 那一份（两行台账都 done、一个气泡指着已经不存在的字节）。落地前再确认一次：被占走就换名。
    // （与群聊收尾同形状 —— 那边早就这么做了，其注释声称"单聊在写盘时才定名"与代码不符。）
    if r.final_path.exists() {
        if let Some(dir) = r.final_path.parent() {
            r.final_path = unique_path(dir, &r.name);
        }
    }
    if let Err(e) = std::fs::rename(&r.tmp_path, &r.final_path) {
        let reason = e.to_string();
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "failed",
            None,
            0.0,
        )
        .ok();
        return Err(reason);
    }
    {
        let dbc = db_lock.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            &r.peer_id,
            &r.name,
            r.size,
            "receive",
            "done",
            Some(r.final_path.to_string_lossy().as_ref()),
            1.0,
        )
        .ok();
    }
    Ok((
        r.name.clone(),
        r.size,
        r.final_path.clone(),
        r.peer_id.clone(),
    ))
}
