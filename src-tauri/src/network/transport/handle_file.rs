// 职责边界：`handle_message` 的「1:1 文件收发」分支组 —— 只处理 `FileOffer`、`FileAccept`、`FileReject`、`FileCompleteAck`、`FileChunk`、`FileDone` 这些变体；不做路由、不起新连接，落库/发送都在各自臂体里（与原函数逐字相同）。
// 为什么单独一册：`handle_message` 原来 1,540 行 / 38 个臂挤在一个函数里。已用 AST 现量过
//   **match 之前没有裸 let 绑定、分支之间不共享局部量** ⇒ 按消息族拆出去是机械搬家而不是改控制流，
//   依据与量法见 docs/large-file-split-plan.md §5.1。
// 恒等判据（每族都跑同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点仍恰好命中一次、clippy `-D warnings`、`cargo fmt --check`。
// 为什么这一族先拆：它是 handle_message 里最厚的一组（6 个变体 499 行），且锚点最密（5 条护栏用例钉在臂体里）——
//   先动它能把「搬完锚点必须改 file=」这条流程走通，后面四族就是重复同一条路径。

async fn handle_file_messages(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    // 这层 match 与原函数里的同一层（臂仍是 8 格缩进 ⇒ 逐字未改）。
    // `_ => {}` 不是吞消息：调用方只在命中本族变体时才把 msg 交进来。
    match msg {
        Message::FileOffer {
            transfer_id,
            from,
            name,
            size,
            sealed_file_key,
            file_sha256,
            from_bytes,
            attempt,
            ..
        } => {
            if from != peer_id || from == state.device_id {
                return;
            }
            let is_friend = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some()
            };
            if !is_friend {
                let _ = try_send(
                    state,
                    peer_id,
                    &Message::FileReject {
                        transfer_id,
                        received: 0,
                    },
                )
                .await;
                return;
            }
            // SHA-256 元数据格式校验：非法即拒绝（文件级完整性无法验证）
            if !file::valid_sha256_hex(&file_sha256) {
                let _ = try_send(
                    state,
                    peer_id,
                    &Message::FileReject {
                        transfer_id,
                        received: 0,
                    },
                )
                .await;
                return;
            }
            // E2EE：解封文件会话密钥（发送方用我方公钥封装，只有我能解开）。
            // 解封失败必须拒绝传输——密文分片绝不能落盘。
            let file_key = (|| {
                let sender_pub = resolve_member_x25519(state, &from)?;
                let shared = crypto::shared_secret(&state.identity.x25519_secret, &sender_pub)?;
                let sealed = STANDARD.decode(&sealed_file_key).ok()?;
                crypto::open(&shared, &sealed).and_then(|k| k.try_into().ok())
            })();
            let Some(file_key) = file_key else {
                let _ = try_send(
                    state,
                    peer_id,
                    &Message::FileReject {
                        transfer_id,
                        received: 0,
                    },
                )
                .await;
                return;
            };
            // 位置判据**只有一份**（`file::decide_offer`）：有活跃接收器时以它的内存计数为准，
            // 没有时以磁盘 `.part` 前缀为准。
            //
            // 以前这里是两套规矩：`!has_receiver` 才比对前缀、`has_receiver` 一律裸 Accept。
            // 后者在真机上把 160MB 判了死刑 —— 发送端每轮重试都从 `from_bytes = 0` 重发
            // （`flush_pending_files` 走不带位置的 `send_file_from_path`），接收端回 Accept 后
            // 把已收的几十 MB 当"迟到的重复片"静默丢掉 ⇒ 每轮都重传一遍前缀 ⇒ 永不收敛、
            // 界面恒 0%、最后报"分片失败"。回真实位置才是正解（发送端 `file.rs` 那一侧
            // 早就听得懂 `FileReject.received`，它只是从来没被这样告诉过）。
            let active = file::receiver_progress(state, &transfer_id);
            let disk_retained = file::retained_part_len(state, &transfer_id);
            // 「本机已收完」也是判据输入（2026-09-23 审计 A6）：收完后 .part 已改名、
            // 接收器已清空，三输入全归零 ⇒ 旧判据把重复 Offer 判成 Accept ⇒ 整份
            // 重推落"名字(1)"副本。已 done 的传输一律回 AlreadyHave。
            let done_query = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::is_transfer_done(&dbc, &transfer_id)
            };
            // 查询失败按「还没收完」处理（保守方向：最坏重传一次，不会丢数据），但必须留痕。
            let already_completed = done_query.unwrap_or_else(|e| {
                state.logger.warn(
                    "file",
                    format!("查询传输终态失败，按未收完处理 transfer={transfer_id}: {e}"),
                );
                false
            });
            let decided = file::decide_offer(
                active.is_some(),
                active.unwrap_or(0),
                disk_retained,
                size,
                from_bytes,
                already_completed,
            );
            match decided {
                file::OfferDecision::AlreadyHave => {
                    state.logger.info(
                        "file",
                        format!(
                            "本机已完整收下该文件，拒绝重推 transfer={transfer_id}（回 received=size）"
                        ),
                    );
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileReject {
                            transfer_id,
                            received: size,
                        },
                    )
                    .await;
                    return;
                }
                file::OfferDecision::ResumeFrom(held) => {
                    state.logger.info(
                        "file",
                        format!(
                            "接收端已有 {held} 字节，要求发送端从此续发 transfer={transfer_id}"
                        ),
                    );
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileReject {
                            transfer_id: transfer_id.clone(),
                            received: held,
                        },
                    )
                    .await;
                    return;
                }
                file::OfferDecision::Accept | file::OfferDecision::AcceptResumeSegment => {
                    if active.is_some() {
                        // **重复的 offer 必须幂等接受**：对端没收到我们的 accept 时会重发同一个
                        // transfer_id，旧行为回 `FileReject("重复的文件传输")` ⇒ 对端判定失败、
                        // 停止重试 ⇒ 文件永远到不了（真机：大图两边都显示成功、接收侧列表里没有）。
                        let resumed = matches!(decided, file::OfferDecision::AcceptResumeSegment);
                        state.logger.info(
                            "file",
                            format!(
                                "重复的文件请求 ⇒ 幂等回 accept{} transfer={transfer_id}",
                                if resumed {
                                    "（续传段：段号归零）"
                                } else {
                                    ""
                                }
                            ),
                        );
                        // 段号归零：发送端续传时按段从 `seq = 0` 重编，而归零后那些**新数据**
                        // 会被 `write_chunk` 判成"迟到的重复片"静默丢掉 ⇒ 文件永远差一截。
                        // 判据（只有续传段才归零，`from_bytes = 0` 的重复 offer 不归零）在
                        // `file::decide_offer` 里 —— 那边写了为什么：无条件归零会把上一轮
                        // 还在排空的分片打成「跳号」，那是一整单死的另一种死法。
                        if resumed {
                            file::restart_segment(state, &transfer_id);
                        }
                        // Offer 是权威：它带来的轮次就是"当前轮次"，此后只有同轮次的
                        // 分片/完成帧会被处理（`file::frame_is_current`）。
                        file::note_offer_attempt(state, &transfer_id, attempt);
                        let _ = try_send(
                            state,
                            peer_id,
                            &Message::FileAccept {
                                transfer_id: transfer_id.clone(),
                            },
                        )
                        .await;
                        return;
                    }
                    // 位置对得上、且没有活跃接收器 ⇒ 落到下面照常建/续接收器。
                }
            }
            let received = if from_bytes > 0 {
                file::resume_receive(
                    state,
                    &transfer_id,
                    &from,
                    &name,
                    size,
                    file_key,
                    file_sha256.clone(),
                    from_bytes,
                )
            } else {
                file::begin_receive(
                    state,
                    &transfer_id,
                    &from,
                    &name,
                    size,
                    file_key,
                    file_sha256.clone(),
                )
            };
            match received {
                Ok(_) => {
                    // 新建/续建的接收器一律以这份 Offer 的轮次为准（Offer 是权威，见
                    // `file::frame_is_current` 第 3 条）。
                    file::note_offer_attempt(state, &transfer_id, attempt);
                    // Phase 1：接收一开始就登记一条 Active 记录（cid → 暂无 path）。
                    // 中途断链 / 超时由 record_failure 标成 Incomplete ⇒ 建链时自动重取。
                    {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        let now = db::now_ms();
                        let rec = crate::content::model::TransferRecord {
                            cid: file_sha256.clone(),
                            transfer_id: Some(transfer_id.clone()),
                            peer_id: from.clone(),
                            group_id: None,
                            name: name.clone(),
                            size,
                            direction: crate::content::model::Direction::Receive,
                            status: crate::content::model::TransferStatus::Active,
                            received: 0,
                            attempts: 0,
                            next_attempt_at: 0,
                            last_error: None,
                            path: None,
                            created_at: now,
                            updated_at: now,
                        };
                        let _ = crate::content::store::upsert(&dbc, &rec);
                    }
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileAccept {
                            transfer_id: transfer_id.clone(),
                        },
                    )
                    .await;
                    let _ = state.app.emit(
                        "file-progress",
                        &FileProgress {
                            transfer_id: transfer_id.clone(),
                            received: 0,
                            total: size,
                        },
                    );
                }
                Err(e) => {
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileReject {
                            transfer_id,
                            received: 0,
                        },
                    )
                    .await;
                    state
                        .logger
                        .error("file", format!("接收文件初始化失败: {e}"));
                }
            }
        }
        Message::FileAccept { transfer_id } => {
            if let Some(tx) = state
                .pending_file_accept
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id)
            {
                let _ = tx.send(Ok(()));
            }
        }
        Message::FileReject {
            transfer_id,
            received,
        } => {
            if let Some(tx) = state
                .pending_file_accept
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id)
            {
                // 把"接收端已持有多少字节"回给发送端 ⇒ 它从该偏移续发，无需重头。
                let _ = tx.send(Err(received));
            }
        }
        Message::FileCompleteAck {
            transfer_id,
            success,
        } => {
            // 只有该 transfer 的实际接收方发来的完成确认才有效。
            let expected_peer = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                dbc.query_row(
                    "SELECT peer_id FROM file_transfers WHERE id = ?1",
                    params![transfer_id],
                    |r| r.get::<_, String>(0),
                )
                .ok()
            };
            if expected_peer.as_deref() != Some(peer_id) {
                return;
            }
            if let Some(tx) = state
                .pending_file_complete
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id)
            {
                let _ = tx.send(success);
            }
        }
        Message::FileChunk {
            transfer_id,
            seq,
            data,
            attempt,
        } => {
            match STANDARD
                .decode(&data)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    file::write_chunk(state, &transfer_id, peer_id, seq, &bytes, attempt)
                }) {
                Ok(received) => {
                    // 节流：每 250ms 至多上报一次进度，避免大文件 IPC 事件风暴
                    let (total, should_emit) = {
                        let mut recv = state
                            .file_receivers
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        match recv.get_mut(&transfer_id) {
                            Some(r) => {
                                let now = db::now_ms();
                                let emit = now - r.last_report_ms >= 250;
                                if emit {
                                    r.last_report_ms = now;
                                }
                                (r.size, emit)
                            }
                            None => (0, false),
                        }
                    };
                    if should_emit {
                        let _ = state.app.emit(
                            "file-progress",
                            &FileProgress {
                                transfer_id: transfer_id.clone(),
                                received,
                                total,
                            },
                        );
                    }
                }
                Err(e) => {
                    // true = 本次真的中止了一个在途接收器（false = 早已中止/未知传输/来源不符，
                    // 再发通知只会重复）。**必须回否定确认**：旧行为是"判死但谁也不告诉"，
                    // 发送端于是把剩下的整份文件继续灌进一条已经死掉的传输，最后 FileDone
                    // 无人应答、干等一个 FILE_ACK_IDLE，再按"可重试"整发 5 次 —— 真机上
                    // 表现为两边都显示失败，而中间几十分钟界面上一直"发送中"。
                    if file::fail_receive(state, &transfer_id, peer_id, &e) {
                        let _ = try_send(
                            state,
                            peer_id,
                            &Message::FileCompleteAck {
                                transfer_id: transfer_id.clone(),
                                success: false,
                            },
                        )
                        .await;
                    }
                }
            }
        }
        Message::FileDone {
            transfer_id,
            attempt,
        } => {
            // 上一轮残留的完成帧：这一轮才刚开始，拿它去 finish_receive 会把整单
            // 判成"文件传输未完成"打死（或补一个莫须有的成功 Ack）。先按轮次挡掉。
            if !file::done_is_current(state, &transfer_id, attempt) {
                state.logger.info(
                    "file",
                    format!("丢掉非当前轮次的完成帧 transfer={transfer_id}"),
                );
                return;
            }
            match file::finish_receive(state, &transfer_id, peer_id) {
                Err(e) => {
                    let _ = state.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id: transfer_id.clone(),
                            reason: e,
                        },
                    );
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileCompleteAck {
                            transfer_id,
                            success: false,
                        },
                    )
                    .await;
                }
                Ok(None) => {
                    // 重复 FileDone：若本机此前已成功完成该 transfer，则补一个成功确认，
                    // 避免发送方因重试而一直等待。
                    let already_done = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        db::is_transfer_done(&dbc, &transfer_id).unwrap_or(false)
                    };
                    if already_done {
                        let _ = try_send(
                            state,
                            peer_id,
                            &Message::FileCompleteAck {
                                transfer_id,
                                success: true,
                            },
                        )
                        .await;
                    } else {
                        // 接收器已清且库非 done（接收态被 TTL 回收 / 本机从未接受该
                        // 传输）：也必须回一帧失败确认（2026-09-23 审计 A5）—— 旧实现
                        // 这里静默，发送端收不到任何帧只能干等静默窗口超时。立刻拿到
                        // 失败终态才不会白等；发送端只认 file_transfers 里登记的
                        // peer_id，伪造面不变。
                        let _ = try_send(
                            state,
                            peer_id,
                            &Message::FileCompleteAck {
                                transfer_id,
                                success: false,
                            },
                        )
                        .await;
                    }
                }
                Ok(Some((name, size, path, sender_id))) => {
                    let rec = {
                        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        let subtype = file::classify_file_subtype(&name);
                        // 与发送端同口径（`commands::send_file`，4.22.1 起；现为 send_file_auto 的私有实现）：kind 只区分
                        // image|file，细分留 content.subtype —— 接收端不再按文件名
                        // 重新猜一遍（旧写法把 mp4 标成 kind="video"，前端渲染链
                        // 不认，气泡整个退化成 JSON；真机 2026-09-19）。
                        let kind = if subtype == "image" { "image" } else { "file" };
                        let content = serde_json::json!({
                            "name": name,
                            "path": path.to_string_lossy().to_string(),
                            "size": size,
                            "subtype": subtype,
                        })
                        .to_string();
                        let seq = db::next_clock(&dbc, &sender_id).unwrap_or(1);
                        let rec = MessageRecord {
                            id: 0,
                            msg_id: format!("file-{transfer_id}"),
                            conv_id: sender_id.clone(),
                            sender_id: sender_id.clone(),
                            receiver_id: state.device_id.clone(),
                            kind: kind.to_string(),
                            content,
                            ts: db::now_ms(),
                            seq,
                            status: "delivered".to_string(),
                            mention_targets: None,
                        };
                        db::insert_message(&dbc, &rec).ok();
                        let nm = resolve_nickname(state, &sender_id);
                        let preview = if kind == "image" {
                            "[图片]".to_string()
                        } else {
                            format!("[文件] {name}")
                        };
                        db::touch_conversation(&dbc, &sender_id, "single", &nm, None, &preview, 1)
                            .ok();
                        rec
                    };
                    let _ = state.app.emit("message-received", &rec);
                    let _ = state.app.emit(
                        "file-done",
                        &FileDoneInfo {
                            transfer_id: transfer_id.clone(),
                            name: name.clone(),
                            size,
                            path: path.to_string_lossy().to_string(),
                        },
                    );
                    let _ = try_send(
                        state,
                        peer_id,
                        &Message::FileCompleteAck {
                            transfer_id,
                            success: true,
                        },
                    )
                    .await;
                }
            }
        }
        _ => {}
    }
}
