// 职责边界：`handle_message` 的「共享目录与单跳中继」分支组 —— 只处理 `ContentRequest`、`ShareTreeRequest`、`ShareTreeResponse`、`ShareFileRequest`、`RelayFileOffer`、`RelayChunk` 这些变体；不做路由、不起新连接，落库/发送都在各自臂体里（与原函数逐字相同）。
// 为什么单独一册：`handle_message` 原来 1,540 行 / 38 个臂挤在一个函数里。已用 AST 现量过
//   **match 之前没有裸 let 绑定、分支之间不共享局部量** ⇒ 按消息族拆出去是机械搬家而不是改控制流，
//   依据与量法见 docs/large-file-split-plan.md §5.1。
// 恒等判据（每族都跑同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点仍恰好命中一次、clippy `-D warnings`、`cargo fmt --check`。
// 为什么并成一族：这 6 个变体都是「替别人取/传」而不是「给我」——ContentRequest 按 cid 拉取、ShareTree/ShareFile 读写共享目录、
//   RelayFileOffer/RelayChunk 是单跳借道。它们共用同一批判据（丢帧必留痕、单跳限制、授权闸）。

async fn handle_share_and_relay_messages(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    // 这层 match 与原函数里的同一层（臂仍是 8 格缩进 ⇒ 逐字未改）。
    // `_ => {}` 不是吞消息：调用方只在命中本族变体时才把 msg 交进来。
    match msg {
        Message::ContentRequest {
            from,
            cid,
            transfer_id,
            from_seq,
            from_bytes,
            ..
        } => {
            if from != peer_id {
                return;
            }
            let source = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                crate::content::store::find_source(&dbc, &cid)
                    .ok()
                    .flatten()
            };
            let Some((_owner, group_id, path)) = source else {
                state.logger.info(
                    "content",
                    format!("ContentRequest：本机没有该内容 cid={cid}"),
                );
                return;
            };
            // 授权：是好友，**或** 是该内容所属群的成员 —— 群聊里 A→B 成功后，
            // 没拿到的 C 可以从已收完的 B 拉（B 是种子，内容寻址的意义所在）。
            let allowed = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                if db::get_friend(&dbc, &from).is_some() {
                    true
                } else if let Some(g) = group_id.as_deref() {
                    db::get_group(&dbc, g)
                        .map(|grp| grp.members.iter().any(|m| m == &from))
                        .unwrap_or(false)
                } else {
                    false
                }
            };
            if !allowed {
                state.logger.warn(
                    "content",
                    format!("ContentRequest：请求方无权限，拒绝服务 cid={cid} from={from}"),
                );
                return;
            }
            // 续传：沿用原 transfer_id（接收端才找得到 <tid>.part），并从已收字节起发。
            let transfer_id = if transfer_id.is_empty() {
                format!("refetch-{}", uuid::Uuid::new_v4())
            } else {
                transfer_id
            };
            match crate::network::file::send_file_from_path_at(
                state,
                &from,
                &transfer_id,
                std::path::PathBuf::from(&path),
                from_seq,
                from_bytes,
            )
            .await
            {
                Ok(()) => state.logger.info(
                    "content",
                    format!("已按 ContentRequest 回发内容 cid={cid} -> {from}"),
                ),
                Err(e) => state.logger.warn(
                    "content",
                    format!("ContentRequest 服务失败 cid={cid} from={from}: {e:?}"),
                ),
            }
        }
        Message::ShareTreeRequest {
            request_id,
            from,
            to: _to,
        } => {
            // 定向中继已在 handle_message 顶部处理（不是给我的帧不会走到这里）。
            if from == state.device_id {
                return;
            }
            // 双好友判定（2026-09-19 自审 P1#4）：`from` 是**自报字段** —— 单查它，
            // 一个好友可以冒用另一个好友的 id 浏览别人的共享目录。也不能简单收紧成
            // from==peer_id：借一跳中继的合法帧到达时 peer 是转投邻居而非原 requester。
            // 残余风险（接受并记录）：中继好友 M 可以转发 from=他人 的请求，
            // 但 M 本身已通过 4.22.9 的中继授权闸，且处于 LAN 信任模型内。
            let allowed = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some() && db::get_friend(&dbc, peer_id).is_some()
            };
            if !allowed {
                return;
            }
            let entries = {
                let share = state
                    .share_dir
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                match share {
                    Some(dir) => file::walk_share_dir(Path::new(&dir)),
                    None => Vec::new(),
                }
            };
            let resp = Message::ShareTreeResponse {
                request_id,
                from: state.device_id.clone(),
                to: Some(from.clone()),
                entries,
            };
            // 有直连直接回；没有则借一跳中继送回（与请求路径对称）。
            if state.has_link(&from).await {
                let _ = try_send(state, &from, &resp).await;
            } else {
                // 邻居接住数不用：请求方自己有 10s 超时兜底，那边才是它的失败证据。
                let _ = relay_send_to_neighbors(state, &from, &resp).await;
            }
        }
        Message::ShareTreeResponse {
            request_id,
            entries,
            ..
        } => {
            if let Some(tx) = state
                .pending_share_tree
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id)
            {
                let _ = tx.send(entries);
            }
        }
        Message::ShareFileRequest {
            transfer_id,
            from,
            path,
            to: _to,
        } => {
            // 定向中继已在 handle_message 顶部处理。
            if from == state.device_id {
                return;
            }
            // 双好友判定，同 ShareTreeRequest（自审 P1#4）：`from` 自报可伪造，
            // 下载别人共享目录的文件必须「声称的 requester 是我的好友」**且**
            // 「提出这条链路的对端也是我的好友」（中继链两跳都在信任圈内）。
            let allowed = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::get_friend(&dbc, &from).is_some() && db::get_friend(&dbc, peer_id).is_some()
            };
            if !allowed {
                return;
            }
            let share = state
                .share_dir
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let Some(root) = share else { return };
            let root = PathBuf::from(root);
            let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let full = root.join(&path);
            let canon_full = full.canonicalize().unwrap_or_else(|_| full.clone());
            if !canon_full.starts_with(&canon_root) || !canon_full.is_file() {
                return;
            }
            let st = state.clone();
            let from = from.clone();
            // 本地提示：对端请求下载你的共享文件（聊天信息内简约系统消息）。
            // ⚠️ 措辞必须是「请求下载」而不是「下载了」：这条插入发生在**推第一个分片之前**，
            // 而中继发送此刻起还没任何成功证据（审计 A1）。写成完成时态就是一句可能被证伪的
            // 陈述句，用户会据此以为文件已经给出去了。
            let file_name = path
                .rsplit('/')
                .next()
                .map(|s| s.to_string())
                .unwrap_or_else(|| path.clone());
            let from_name = resolve_nickname(state, &from);
            crate::commands::insert_system_message(
                state,
                &from,
                &format!("「{from_name}」请求下载你的文件「{file_name}」"),
            );
            tokio::spawn(async move {
                // 有直连走原有可靠直传；没有直连则借一跳中继（RelayFileOffer/RelayChunk）。
                let result = if st.has_link(&from).await {
                    file::send_file_from_path(&st, &from, &transfer_id, canon_full)
                        .await
                        .map_err(|e| e.message)
                } else {
                    file::send_file_via_relay(&st, &from, &transfer_id, canon_full).await
                };
                if let Err(reason) = result {
                    let _ = st.app.emit(
                        "file-failed",
                        &FileFailedInfo {
                            transfer_id,
                            reason,
                        },
                    );
                }
            });
        }
        // ---- Gossip 广播 ----
        Message::RelayFileOffer {
            transfer_id,
            from,
            to,
            name,
            size,
            total_chunks,
            chunk_size,
            sealed_file_key,
            file_sha256,
        } => {
            handle_relay_file_offer(
                state,
                peer_id,
                transfer_id,
                from,
                to,
                name,
                size,
                total_chunks,
                chunk_size,
                sealed_file_key,
                file_sha256,
            )
            .await;
        }
        Message::RelayChunk {
            transfer_id,
            seq,
            data,
            from,
            to,
            ttl,
        } => {
            handle_relay_chunk(state, peer_id, transfer_id, seq, data, from, to, ttl).await;
        }
        // ---- 群密钥分发 ----
        _ => {}
    }
}
