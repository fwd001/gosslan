// 中继推文件：`RelayFileOffer` / `RelayChunk` 那条**借道**路径。
//
// 为什么单独一册：它复用同一套分片与加密，但**收件人不是链路对端**（对端是中继节点），
// 所以授权闸、节流、失败归因都走 `transport::relay_*` 而不是 `file::send_*` 那一条。
//

// 恒等判据（与 transport 那五刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（本模块的锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 搬家同批必须做的两件事：`network/mod.rs::file_src_for_guards()` 登记本册（漏了=形状守卫看不见这段生产码，假绿），
//   以及 `docs/domains.data.mjs` 的 transport/file 领域 paths（漏了=判据 D 报无主文件）。

/// 无直连时，借**一跳中继**把文件发给 peer_id（接收方是请求下载的共享目录主人）。
///
/// 与 send_file_from_path 的区别：
/// - 不做 FileAccept 握手（对方已显式请求下载），也不等 FileCompleteAck（中继无回执）；
/// - 走 RelayFileOffer + RelayChunk：中继只透传密文，E2EE 与直传一致；
/// - 单跳：中继必须与目标有直连（与既有 RelayChunk 的限制一致）。
///
/// **失败必须落 DB 终态**（2026-09-23 审计 A1）：推流建的行写的是 `active`，而**发送方向
/// 没有任何清扫器**（`sweep_stale_relay` 清的是接收侧那两张内存表）⇒ 旧实现里取消 / 读盘
/// 失败 / 整体超时 / 分片失败**每一条 Err 路径**都留一行 active 在库里 —— 界面当场收到
/// `file-failed`，重启后那条传输又变回"进行中 X%"并永久挂着。本包装是这条链唯一出口，
/// 失败时统一把仍 active 的行标 failed（已 done 的行不动，绝不改写既有终态）。
pub async fn send_file_via_relay(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    let outcome = relay_push_file(state, peer_id, transfer_id, path).await;
    if let Err(reason) = &outcome {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = db::mark_transfer_failed_if_active(&dbc, transfer_id) {
            state.logger.warn(
                "file",
                format!(
                    "中继发送失败后落终态也失败 transfer={transfer_id}: {e}（发送失败原因：{reason}）"
                ),
            );
        }
    }
    outcome
}

/// 真正推分片出去：见 [`send_file_via_relay`] —— 终态落库在外层，保证所有 Err 路径同一条出口。
async fn relay_push_file(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: &str,
    path: PathBuf,
) -> Result<(), String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("文件不存在或不可读：{e}"))?;
    if !meta.is_file() {
        return Err("只能发送普通文件".to_string());
    }
    let size = meta.len();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let file_key = crypto::random_key();
    let file_sha256 = sha256_file_hex(&path)?;
    let receiver_pubkey = resolve_member_x25519(state, peer_id)
        .ok_or_else(|| "无法获取对方公钥，无法加密文件".to_string())?;
    let shared = crypto::shared_secret(&state.identity.x25519_secret, &receiver_pubkey)
        .ok_or_else(|| "密钥交换失败".to_string())?;
    let sealed_key_b64 =
        STANDARD.encode(crypto::seal(&shared, &file_key).ok_or_else(|| "加密失败".to_string())?);
    // ⚠️ **逐片读盘，不整读进内存**。这里原先 `std::fs::read(&path)` 把整个文件读进来再切片：
    // 中继发送的是共享目录里的文件（可能很大），整读后逐片 base64（×1.33）会让内存峰值
    // 超过文件大小本身。改成按需 seek + read_exact，峰值只剩一个分片。
    //
    // 分片大小必须迁就链路中最受限的邻居（2026-09-23 审计 B1）：中继帧会发给
    // **所有**有直连的邻居，任一邻居是 BLE 时，64KiB 分片（base64 后 ~87KB）会反复
    // 撑爆 BLE 写超时（整帧一个 deadline，重试数次即拆链）—— 纯蓝牙链路必现停摆
    // 与丢片。直传早有 `chunk_size_for_path` 门控，中继路径在此补齐：有 BLE 邻居就
    // 整单用 BLE 尺寸（迁就最慢路径；协议无感知，total_chunks 相应变化，跨版本兼容）。
    //
    // 判据刻意用「邻居名下**存在** BLE 链路」而不是「邻居的最佳链路是 BLE」：
    // `send_over_order` 在首选链路队列满（Full）时会顺延到 order 里的下一条，
    // 因此同一邻居同时有 LAN+BLE 时，拥塞的那一帧照样会落到 BLE 上并把链路拆掉。
    // 按最佳链路判会把罕见但致命的拆链换成"LAN 邻居多吃些小帧"，不划算。
    let chunk_size = {
        let links = state.links.lock().await;
        let any_ble = links
            .iter()
            .filter(|(p, _)| p.as_str() != peer_id)
            .flat_map(|(_, ls)| ls.iter())
            .any(|l| l.path_kind == crate::mesh::PathKind::Bluetooth);
        if any_ble {
            BLE_FILE_CHUNK
        } else {
            crate::file_relay::MIN_CHUNK_SIZE
        }
    };
    let total = size as usize;
    let chunk_count = total.div_ceil(chunk_size).max(1) as u32;
    let mut src = std::fs::File::open(&path).map_err(|e| format!("读取文件失败：{e}"))?;

    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            &name,
            size,
            "send",
            "active",
            None,
            0.0,
        )
        .ok();
    }

    let offer = Message::RelayFileOffer {
        transfer_id: transfer_id.to_string(),
        from: state.device_id.clone(),
        to: peer_id.to_string(),
        name: name.clone(),
        size,
        total_chunks: chunk_count,
        // 接收方按 `seq × chunk_size` 直接落盘，所以这个数必须随 offer 一起过去
        chunk_size: chunk_size as u32,
        sealed_file_key: sealed_key_b64,
        file_sha256,
    };

    // ---- cancel + timeout 注册 ----
    // 键含收件人（与直传同口径，见 `file_cancel_key`）。
    let cancel_key = file_cancel_key(transfer_id, peer_id);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    state
        .file_send_cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cancel_key.clone(), cancel_tx);
    let cancel_cleanup = || {
        state
            .file_send_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&cancel_key);
    };

    let result = tokio::time::timeout(RELAY_FILE_SEND_DEADLINE, async {
        // Offer 一个邻居都没接住 ⇒ 这一帧从未离开本机：直接判失败，既不读盘也不推分片。
        // 方向要说清：这里只用「0 必然没送出」这一侧的下限判据；`≥1` **不等于**送达
        // （邻居未必与对方有直连），真送达要等接收端回执（A1-L2，尚未实施）。
        if crate::network::transport::relay_send_to_neighbors(state, peer_id, &offer).await == 0 {
            return Err("没有可达的中继邻居：文件未发出".to_string());
        }

        let mut last_report = std::time::Instant::now() - Duration::from_secs(1);
        for seq in 0..chunk_count {
            // 每片开始前先查 cancel —— 用户点了就立刻停，不浪费下一片 I/O
            if cancel_rx.try_recv().is_ok() {
                return Err("用户取消发送".to_string());
            }
            let start = (seq as usize * chunk_size).min(total);
            let end = (start + chunk_size).min(total);
            let mut plain = vec![0u8; end - start];
            src.seek(std::io::SeekFrom::Start(start as u64))
                .map_err(|e| format!("定位文件失败：{e}"))?;
            src.read_exact(&mut plain)
                .map_err(|e| format!("读取文件分片失败：{e}"))?;
            let sealed = crypto::seal_symmetric(&file_key, &plain)
                .ok_or_else(|| "文件分片加密失败".to_string())?;
            let data = STANDARD.encode(&sealed);
            let msg = Message::RelayChunk {
                transfer_id: transfer_id.to_string(),
                seq,
                data,
                from: state.device_id.clone(),
                to: peer_id.to_string(),
                ttl: 3,
            };
            // 同一判据用在每一片上：某片开始没有任何邻居接住，说明链路在这中间断了，
            // 继续推剩余分片只是把日志刷满并让界面停在最后一个报过的百分比上。
            if crate::network::transport::relay_send_to_neighbors(state, peer_id, &msg).await == 0 {
                return Err(format!("中继链路中断：第 {seq} 片没有任何邻居接住"));
            }
            let sent = end as u64;
            if last_report.elapsed() >= Duration::from_millis(250) {
                last_report = std::time::Instant::now();
                let progress = if size == 0 {
                    1.0
                } else {
                    sent as f64 / size as f64
                };
                {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    db::upsert_transfer(
                        &dbc,
                        transfer_id,
                        peer_id,
                        &name,
                        size,
                        "send",
                        "active",
                        None,
                        progress,
                    )
                    .ok();
                }
                let _ = state.app.emit(
                    "file-progress",
                    &crate::state::FileProgress {
                        transfer_id: transfer_id.to_string(),
                        received: sent,
                        total: size,
                    },
                );
            }
        }
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::upsert_transfer(
                &dbc,
                transfer_id,
                peer_id,
                &name,
                size,
                "send",
                "sent",
                Some(path.to_string_lossy().as_ref()),
                1.0,
            )
            .ok();
        }
        // 最后一片可能因节流(250ms)跳过了 emit，收尾必须补一次完整进度，
        // 否则前端可能卡在"发送中 63%"（DB 已终态但前端没收到事件推进）。
        let _ = state.app.emit(
            "file-progress",
            &crate::state::FileProgress {
                transfer_id: transfer_id.to_string(),
                received: size,
                total: size,
            },
        );
        // ⚠️ **刻意不发 `file-done`**（2026-09-23 审计 A1 的 L2 那一半）：走到这里只代表
        // "每一片都被至少一个邻居接住"，**不代表**对端收全、SHA 校验通过、落了盘。而 `file-done`
        // 的语义是"本机这条传输已完成"，前端 `onFileDone` 会把内存里那行写成 done ——
        // 发它就是"库里 sent、界面上 ✓"，正撞验收红线「界面显示成功与对端实际收到不一致」。
        // 这条链没有接收端回执（`file.rs` 开头自述：中继无握手无回执），所以终态只能停在 `sent`；
        // 要把它升成 done，得给 `FileCompleteAck` 加 `to` 走定向一跳中继并按能力位门控。
        Ok(())
    })
    .await;

    cancel_cleanup();

    match result {
        Ok(inner) => inner,
        Err(_) => Err(format!(
            "中继文件发送超时（超过 {}s）",
            RELAY_FILE_SEND_DEADLINE.as_secs()
        )),
    }
}
