// 中继文件传输（offer / chunk 落盘 / 收尾改名）
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第二批，判据与顺序见 docs/large-file-split-plan.md。

#[allow(clippy::too_many_arguments)]
async fn handle_relay_file_offer(
    state: &Arc<AppState>,
    _peer_id: &str,
    transfer_id: String,
    from: String,
    to: String,
    name: String,
    size: u64,
    total_chunks: u32,
    chunk_size: u32,
    sealed_file_key: String,
    file_sha256: String,
) {
    if to != state.device_id {
        return; // 中继节点无需重组，只转发切片
    }
    // 中继场景下 from 是**原始发送方**，peer_id 是上一跳邻居 —— 不再要求二者相等；
    // 由 relay_send_to_neighbors + 顶部定向中继保证帧只被转投给 to，且文件会话密钥
    // 只能用 from 的私钥解开（伪造 from 无法解封），因此这里是安全的。
    if from == state.device_id || total_chunks == 0 || size > i64::MAX as u64 {
        return;
    }
    if file::safe_file_name(&name).is_none() {
        return;
    }
    let is_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend(&dbc, &from).is_some()
    };
    if !is_friend {
        return;
    }
    // SHA-256 元数据格式校验（中继不解密不校验内容，仅最终接收方校验）
    if !file::valid_sha256_hex(&file_sha256) {
        return;
    }
    // E2EE：解封文件会话密钥（发送方用我方公钥封装）。中继节点不持有密钥；
    // 解封失败直接放弃——密文分片绝不落盘。
    let file_key = (|| {
        let sender_pub = resolve_member_x25519(state, &from)?;
        let shared = crypto::shared_secret(&state.identity.x25519_secret, &sender_pub)?;
        let sealed = STANDARD.decode(&sealed_file_key).ok()?;
        crypto::open(&shared, &sealed).and_then(|k| k.try_into().ok())
    })();
    let Some(file_key) = file_key else {
        return;
    };
    // 幂等：重复的 RelayFileOffer（多邻居泛洪）不得覆盖已有会话状态
    //（file_key/expected_sha256/重组表都要保留），否则后续分片全部无处安放。
    state
        .relay_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(transfer_id.clone())
        .or_insert_with(|| crate::state::RelayFileReceive {
            file_key,
            expected_sha256: file_sha256,
            created_at: db::now_ms(),
            last_progress_at: 0,
        });
    // 开好 `.part`（预分配、按 seq 落盘）。失败的两条出路都是**当场拒收**：
    // `chunk_size == 0` = 对端没声明分片尺寸（老版本），不声明就没法流式接收，
    // 而宁可可报错也不退回"整份进内存"—— 那条路就是 600MB 文件把手机撑死的入口。
    let dl = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    // `begin_reassemble` 要在**出锁之后**再判错误：Edition 2021 的 `if let` 会把 scrutinee
    // 的临时量（这里是 MutexGuard）留到整个块结束 ⇒ 直接写成 `if let Err(..) = 锁().方法()`
    // 就等于"在 relay 锁里 emit"，而这个文件上面的自订规则是"锁只圈住写库，emit 一律出锁再做"
    // （前端收到 file-failed 后的下一次 IPC 要抢同一把锁）。
    let began = {
        let mut relay = state.relay.lock().unwrap_or_else(|e| e.into_inner());
        relay.begin_reassemble(&transfer_id, &name, total_chunks, size, chunk_size, &dl)
    };
    if let Err(reason) = began {
        let _ = state.app.emit(
            "file-failed",
            &FileFailedInfo {
                transfer_id: transfer_id.clone(),
                reason: reason.clone(),
            },
        );
        state
            .logger
            .warn("file", format!("拒收中继文件 offer：{reason}"));
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &from,
            &name,
            size,
            "receive",
            "active",
            None,
            0.0,
        )
        .ok();
    }
    let _ = state.app.emit(
        "file-progress",
        &FileProgress {
            transfer_id,
            received: 0,
            total: size,
        },
    );
}

#[allow(clippy::too_many_arguments)] // 入队即转发上下文全量传递，打包结构体反而更难读
async fn handle_relay_chunk(
    state: &Arc<AppState>,
    requester: &str,
    transfer_id: String,
    seq: u32,
    data: String,
    from: String,
    to: String,
    ttl: u8,
) {
    // 三种结果的处理完全不同（忽略 / 判死 / 等下一片），见 `file_relay::ChunkOutcome`。
    use crate::file_relay::ChunkOutcome;
    if to == state.device_id {
        // 最终接收方：先解密（E2EE，密文不落盘），再交重组表去重/按 seq 落盘。
        // ⚠️ 不在这里做任何增量哈希：分片按到达顺序解密，可能重复（多邻居泛洪
        // 每条路径都送一份）、可能乱序（多中继路径时延不同）—— 按到达顺序喂哈希
        // 在去重/排序之前必然算错。完整性校验在重组完成后对组装出的明文一次性
        // 计算（2026-09-23 审计 1.8）。
        let bytes = {
            let keys = state
                .relay_file_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(rs) = keys.get(&transfer_id) else {
                return;
            };
            let Ok(sealed) = STANDARD.decode(&data) else {
                return;
            };
            let Some(bytes) = crypto::open_symmetric(&rs.file_key, &sealed) else {
                return;
            };
            bytes
        };
        let completed = {
            let mut relay = state.relay.lock().unwrap_or_else(|e| e.into_inner());
            relay.add_chunk(&transfer_id, seq, &bytes)
        };
        // 三种非完成结果各自的处理完全不同，所以按枚举分派（见 `file_relay::ChunkOutcome`）。
        let (name, expected_size, part_path) = match completed {
            ChunkOutcome::Complete { name, size, path } => (name, size, path),
            ChunkOutcome::Duplicate | ChunkOutcome::Unknown => return,
            ChunkOutcome::Partial {
                received_bytes,
                total_bytes,
            } => {
                // 进度必须报（否则整单停在 0%），但**每秒最多一条**：BLE 上 4KiB 一片，
                // 几百 KB 的文件就是上万片，逐片 emit 会把"正在收文件"变成"界面卡顿"。
                // 判据与更新都在锁内，emit 出锁再做（INV-P25：状态锁内不 emit）。
                let now = db::now_ms();
                let announce = {
                    let mut keys = state
                        .relay_file_keys
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    match keys.get_mut(&transfer_id) {
                        Some(rs) if now - rs.last_progress_at >= 1_000 => {
                            rs.last_progress_at = now;
                            true
                        }
                        _ => false,
                    }
                };
                if announce {
                    let _ = state.app.emit(
                        "file-progress",
                        &FileProgress {
                            transfer_id,
                            received: received_bytes,
                            total: total_bytes,
                        },
                    );
                }
                return;
            }
            ChunkOutcome::Rejected(reason) => {
                // 对端声明的形状自相矛盾 ⇒ 这一单当场判死。`.part` 已由 Reassembly 删掉，
                // 这里只负责落库 + 说给人听（静默消失 = 前端永久卡 X%，审计 A2 那条老账）。
                // 不重写 `upsert_transfer`：offer 到达时那行已经带着 name/size 落过库了，
                // 这里要的是"把仍 active 的行推进到 failed"，正是 `mark_transfer_failed_if_active`。
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::mark_transfer_failed_if_active(&dbc, &transfer_id).ok();
                    state
                        .relay_file_keys
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&transfer_id);
                }
                let _ = state.app.emit(
                    "file-failed",
                    &FileFailedInfo {
                        transfer_id: transfer_id.clone(),
                        reason: format!("中继分片形状不合法：{reason}"),
                    },
                );
                return;
            }
        };
        {
            // 重组结束（无论成败）：会话状态一次性收走，不留"半拆"的中间态。
            let rs = state
                .relay_file_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id);
            // `rs` 为空只可能是回收器抢先摘走了（TTL 到点），此时没有任何依据可以
            // 宣称这份文件可信 ⇒ 判失败。**不许**沿用旧的"跳过校验照样落盘"。
            let Some(rs) = rs else {
                let _ = std::fs::remove_file(&part_path);
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        &transfer_id,
                        &from,
                        &name,
                        expected_size,
                        "receive",
                        "failed",
                        None,
                        0.0,
                    )
                    .ok();
                }
                let _ = state.app.emit(
                    "file-failed",
                    &FileFailedInfo {
                        transfer_id: transfer_id.clone(),
                        reason: "中继接收状态已过期，文件未完成".to_string(),
                    },
                );
                return;
            };
            // 尺寸：声明与实际落盘必须一致（分片形状已在 add_chunk 判过，这里是最后一道）
            let on_disk = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
            if on_disk != expected_size {
                let _ = std::fs::remove_file(&part_path);
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        &transfer_id,
                        &from,
                        &name,
                        expected_size,
                        "receive",
                        "failed",
                        None,
                        0.0,
                    )
                    .ok();
                }
                let _ = state.app.emit(
                    "file-failed",
                    &FileFailedInfo {
                        transfer_id: transfer_id.clone(),
                        reason: "中继文件大小校验失败".to_string(),
                    },
                );
                return;
            }
            // 文件级完整性：**流式**算 SHA-256（边读边算，峰值只有一个读缓冲）。
            // 分片按到达顺序解密时可能重复（多邻居泛洪）也可能乱序（多路径时延不同），
            // 所以哈希只能在字节归位后整体算一次（2026-09-23 审计 1.8）。
            let actual_hex = match file::sha256_file_hex(&part_path) {
                Ok(hex) => hex,
                Err(reason) => {
                    let _ = std::fs::remove_file(&part_path);
                    {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::upsert_transfer(
                            &dbc,
                            &transfer_id,
                            &from,
                            &name,
                            expected_size,
                            "receive",
                            "failed",
                            None,
                            0.0,
                        )
                        .ok();
                    }
                    let _ = state.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id: transfer_id.clone(),
                            reason,
                        },
                    );
                    return;
                }
            };
            if !actual_hex.eq_ignore_ascii_case(&rs.expected_sha256) {
                let _ = std::fs::remove_file(&part_path);
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        &transfer_id,
                        &from,
                        &name,
                        expected_size,
                        "receive",
                        "failed",
                        None,
                        0.0,
                    )
                    .ok();
                    // 统一状态：校验失败 ⇒ Rejected（换源重取是唯一出路）。
                    let _ = crate::content::store::record_failure(
                        &dbc,
                        &rs.expected_sha256,
                        &from,
                        crate::content::model::Direction::Receive,
                        crate::content::model::FailReason::HashMismatch,
                        db::now_ms(),
                    );
                }
                let _ = state.app.emit(
                    "file-failed",
                    &FileFailedInfo {
                        transfer_id: transfer_id.clone(),
                        reason: "文件完整性校验失败".to_string(),
                    },
                );
                return;
            }
            // 校验已过 ⇒ 内容指纹就是这个已确证的 sha256，不再重算一遍整份文件。
            let cid = rs.expected_sha256;
            let path = match move_received_file(state, &name, &part_path) {
                Ok(path) => path,
                Err(reason) => {
                    {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::upsert_transfer(
                            &dbc,
                            &transfer_id,
                            &from,
                            &name,
                            expected_size,
                            "receive",
                            "failed",
                            None,
                            0.0,
                        )
                        .ok();
                    }
                    let _ = state.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id: transfer_id.clone(),
                            reason,
                        },
                    );
                    return;
                }
            };
            let path_str = path.to_string_lossy().to_string();
            let rec = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::upsert_transfer(
                    &dbc,
                    &transfer_id,
                    &from,
                    &name,
                    expected_size,
                    "receive",
                    "done",
                    Some(path_str.as_str()),
                    1.0,
                )
                .ok();
                crate::content::store::record_local(
                    &dbc,
                    &cid,
                    &from,
                    None,
                    &name,
                    expected_size,
                    crate::content::model::Direction::Receive,
                    &path_str,
                    db::now_ms(),
                )
                .ok();
                let content = serde_json::json!({
                    "name": name.clone(),
                    "path": path_str.clone(),
                    "size": expected_size,
                    "sha256": cid.clone(),
                    "subtype": file::classify_file_subtype(&name),
                })
                .to_string();
                let seq = db::next_clock(&dbc, &from).unwrap_or(1);
                let rec = MessageRecord {
                    id: 0,
                    msg_id: format!("file-{transfer_id}"),
                    conv_id: from.clone(),
                    sender_id: from.clone(),
                    receiver_id: state.device_id.clone(),
                    kind: "file".to_string(),
                    content,
                    ts: db::now_ms(),
                    seq,
                    status: "delivered".to_string(),
                    mention_targets: None,
                };
                db::insert_message(&dbc, &rec).ok();
                let nm = resolve_nickname(state, &from);
                db::touch_conversation(
                    &dbc,
                    &from,
                    "single",
                    &nm,
                    None,
                    &format!("[文件] {name}"),
                    1,
                )
                .ok();
                rec
            };
            let _ = state.app.emit("message-received", &rec);
            let _ = state.app.emit(
                "file-done",
                &FileDoneInfo {
                    transfer_id: transfer_id.clone(),
                    name: name.clone(),
                    size: expected_size,
                    path: path_str,
                },
            );
        }
    } else if ttl > 1 {
        // 中继转发给最终接收方 —— 授权闸同「定向借道」（2026-09-19 P0#5）：
        // 策略 Off/Friends/Allowlist 必须真正拦得下文件分片，而不是只拦 gossip。
        // 2026-10-07：闸收进 transport.rs 的 relay_denied 一个家（这里与定向借道、
        // 外部帧转投那两处原来是同一段判断抄三遍）。
        if relay_denied(state, requester, || format!("RelayChunk 转投 tid={transfer_id}")) {
            return;
        }
        let fwd = Message::RelayChunk {
            transfer_id,
            seq,
            data,
            from,
            to: to.clone(),
            ttl: ttl - 1,
        };
        if let Err(e) = try_send(state, &to, &fwd).await {
            // 分片丢弃必须留痕：上一跳的发送队列满/半开时，整文件会因缺片校验失败重来，
            // 以前这里 `let _ =` 连一行日志都没有（INV-005）。
            if log_throttled("relay_drop", 10_000) {
                state.logger.warn(
                    "mesh",
                    format!("RelayChunk 转投失败 to={to} seq={seq}：{e}（缺片将由整体重试收敛）"),
                );
            }
        }
    }
}

/// 把已校验通过的 `.part` **改名**放进下载目录（重名自动加 `(N)`）。
///
/// 与旧的 `save_received_bytes` 的区别是要害：旧的那份收 `&[u8]`，等于"先把整份文件
/// 读回内存再写一遍"—— 那正是 P4 的内存峰值来源。这里只做一次 `rename`，
/// 峰值与文件大小无关。
fn move_received_file(state: &AppState, name: &str, part: &Path) -> Result<PathBuf, String> {
    let dir = state
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe_name = file::safe_file_name(name).ok_or("文件名非法")?;
    let base = dir.join(safe_name);
    if !base.exists() {
        std::fs::rename(part, &base)
            .or_else(|_| std::fs::copy(part, &base).and_then(|_| std::fs::remove_file(part)))
            .map_err(|e| e.to_string())?;
        return Ok(base);
    }
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for i in 1..1000 {
        let cand = if ext.is_empty() {
            dir.join(format!("{stem} ({i})"))
        } else {
            dir.join(format!("{stem} ({i}).{ext}"))
        };
        if !cand.exists() {
            std::fs::rename(part, &cand)
                .or_else(|_| std::fs::copy(part, &cand).and_then(|_| std::fs::remove_file(part)))
                .map_err(|e| e.to_string())?;
            return Ok(cand);
        }
    }
    Err("下载目录重名文件过多".to_string())
}
