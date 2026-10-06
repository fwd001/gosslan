// 群文件传输：offer / chunk / done / 完成回执、失败与停滞回收
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第三批；守卫视图与领域图的分册清单一同登记。

/// 处理群文件发起（Offer → 验证 → file_key 解封 → 保存会话状态）。
/// 本阶段不写文件、不创建 `.part`、不自动 FileAccept——分片传输在下一阶段。
#[allow(clippy::too_many_arguments)]
async fn handle_group_file_offer(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
    name: String,
    size: u64,
    sha256: String,
    sealed_file_key: String,
    scope: String,
    todo_id: String,
) {
    // 链路上报的 sender 必须与 Offer 声明一致，且不能是自己
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    // 幂等：会话已激活（内存有 file_key）→ 重复 Offer 忽略。
    // 注意不能因为「group_files 里有记录」就跳过：重启后内存 key 丢失但记录还在，
    // 跳过会让离线补发永远无法重建会话（接收卡死）。改为下方「已完成则跳过」+「幂等重建」。
    if state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&transfer_id)
    {
        return;
    }
    // 权限：本地群存在，且 sender ∈ group_members（防群外 peer 伪造 Offer）
    let (group_exists, sender_is_member) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => (true, g.members.contains(&sender_id)),
            None => (false, false),
        }
    };
    if !group_exists || !sender_is_member {
        return;
    }
    // 本地 GroupKey 解封 file_key（GroupKey 不离开设备；群外无法解开）
    let Some(group_key) = get_group_key(state, &group_id).await else {
        return;
    };
    let Ok(sealed) = STANDARD.decode(&sealed_file_key) else {
        return;
    };
    let Some(file_key) =
        crypto::open_symmetric(&group_key, &sealed).and_then(|k| k.try_into().ok())
    else {
        return;
    };

    // 已完成的 transfer → 忽略（避免重复接收 / 重复 emit）
    let already_done = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group_file_recipient_status(&dbc, &transfer_id, &state.device_id).as_deref()
            == Some("completed")
    };
    if already_done {
        return;
    }

    // 幂等建立/重建接收会话（重启后内存 file_key 丢失，这里回填 key + 复位 recipient）
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let gf = crate::state::GroupFile {
            transfer_id: transfer_id.clone(),
            group_id: group_id.clone(),
            sender_id: sender_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            status: "sending".to_string(),
            created_at: db::now_ms(),
            scope: scope.clone(),
            todo_id: todo_id.clone(),
        };
        if db::upsert_group_file_receive(&dbc, &gf, &state.device_id).is_err() {
            return;
        }
    }
    // `scope == "todo"`：待办描述图片，仅走传输管线把字节投递给全员，
    // 不进聊天时间线、不弹气泡、不改会话预览（与发送端对称）。
    if scope != "todo" {
        // 接收气泡：与发送端同一 msg_id（gfile-{transfer_id}），前端据 file-progress
        // 之外的状态事件推进。此处 status=sending，Done 校验通过后转 delivered。
        // 图片文件保持 kind="image"，业务语义不降级。
        let subtype = file::classify_file_subtype(&name);
        // 同口径收敛（见单聊 FileOffer 处的说明）
        let kind = if subtype == "image" { "image" } else { "file" };
        let conv_id = format!("group:{group_id}");
        let seq = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::next_clock(&dbc, &conv_id).unwrap_or(1)
        };
        let rec = crate::state::MessageRecord {
            id: 0,
            msg_id: format!("gfile-{transfer_id}"),
            conv_id: conv_id.clone(),
            sender_id: sender_id.clone(),
            receiver_id: state.device_id.clone(),
            kind: kind.to_string(),
            content: serde_json::json!({
                "name": name,
                "size": size,
                "sha256": sha256,
                "subtype": subtype,
                "progress": 0.0,
            })
            .to_string(),
            ts: db::now_ms(),
            seq,
            status: "sending".to_string(),
            // 群文件这条没有 @ 落点这回事（发送侧从来不算 mention_targets）⇒ NULL = 不知道。
            mention_targets: None,
        };
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::insert_message(&dbc, &rec).ok();
            db::touch_conversation(
                &dbc,
                &format!("group:{group_id}"),
                "group",
                &name,
                None,
                &format!("[群文件] {name}"),
                1,
            )
            .ok();
        }
        let _ = state.app.emit("message-received", &rec);
    }
    // 会话密钥仅存内存，供下一阶段解密 GroupFileChunk
    state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(transfer_id, file_key);
}

/// 失败收尾：删除 `.part`、移除接收状态与会话密钥、recipient 置 failed。
/// 只影响本 transfer，不 panic、不影响其他群文件。
fn fail_group_file_chunk(state: &Arc<AppState>, transfer_id: &str) {
    file::fail_group_receive(state, transfer_id);
    finalize_failed_group_receive(state, transfer_id);
}

/// 摘表之后的群接收**收尾**三件事：气泡转 failed、丢掉内存里的 file_key、台账写 failed。
///
/// 从 `fail_group_file_chunk` 里单列出来，是因为对端下线那条路必须先"原子摘"再"逐个收尾"
/// （见 `file::take_group_receives_for_peer`）：摘与收尾合成一个函数的话，
/// 调用点就只能走"快照 id → 逐个收尾"，而那正是本文件 `:5232` 注释判死过的形状。
fn finalize_failed_group_receive(state: &Arc<AppState>, transfer_id: &str) {
    set_gfile_bubble_status(state, transfer_id, "failed");
    state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id);
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let _ = db::update_group_file_recipient(&dbc, transfer_id, &state.device_id, "failed", 0.0);
}

/// 回收"再也没被喂过片"的接收器（第 1 步 · 故障隔离 P1 的另一半）。
///
/// 为什么必须有：断链清理现在只在**该 peer 已无任何链路**时才动手（断一条 ≠ peer 下线），
/// 于是"对端还在线、但这一单被发送侧放弃"的接收器没人回收。协议里**没有 cancel 帧**
/// （`protocol.rs` 里 `Cancel` 零命中），发送侧 60s 停滞只是自己退回 `file_outbox`，
/// 不会通知接收端。另外两个 TTL 都救不了它：`sweep_stale_parts` 明确跳过"还在表里"的
/// `.part`（把泄漏的接收器当活跃证据），而 `resume_receive` 的 24h TTL 只在**有人再来敲
/// 这一单**时才生效 —— 被放弃的单没人再来敲 ⇒ 表项与文件句柄永久留着。
///
/// 这里只摘**内存态**；摘掉之后那条 `.part` 就重新落回 `sweep_stale_parts` 的 24h 管辖，
/// 磁盘侧不另立第二份清理 policy。判据只有一份：`file::receive_is_stale`，
/// 且**与摘表同一次持锁**（`take_stalled_receive`）。落终态与 emit 一律在锁外，同 `sweep_stale_relay`。
pub fn sweep_stalled_receives(state: &Arc<AppState>) -> usize {
    const REASON: &str = "接收超时：对端久未继续发送";
    let now = db::now_ms();
    let mut reclaimed = 0usize;
    // 每轮只摘一条：判据与摘表在**同一次持锁**里完成（见 `file::take_stalled_receive`）。
    // 不能"先快照一批 id 再逐个收尾"—— 那两步之间完全可以挤进一个新 FileOffer
    // （同一 transfer_id 重建接收器、`fed_at_ms` 就是现在），按 id 收尾会把**正在收**的
    // 那一单判死。循环必然终止：`take_*` 每次真的 remove 一条。
    while let Some((id, r)) = file::take_stalled_receive(state, now) {
        file::fail_taken_receive(state, &id, &r, REASON);
        reclaimed += 1;
    }
    while let Some((id, r)) = file::take_stalled_group_receive(state, now) {
        file::fail_taken_group_receive(state, &r);
        // 剩下的收尾（气泡 / 会话密钥 / recipient）按 transfer_id 定位，与表项在不在无关；
        // 其中的 `fail_group_receive` 会因为已摘而跳过 —— 正好复用同一份收尾，不另写一份。
        fail_group_file_chunk(state, &id);
        reclaimed += 1;
    }
    reclaimed
}

/// 群文件气泡状态推进：msg_id = gfile-{transfer_id}（收发双方本地记录）。
fn set_gfile_bubble_status(state: &AppState, transfer_id: &str, status: &str) {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db::set_message_status(&dbc, &format!("gfile-{transfer_id}"), status).ok();
}

/// 处理群文件发送完毕：最终校验（size + SHA-256）→ sync_all → rename → completed。
/// 幂等：session 已清理（已完成或从未建立）时安全忽略。
/// sender-side 的 completed 只代表「本机接收完成」，不是发送端 recipient 状态。
async fn handle_group_file_done(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
) {
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    // 幂等 / 无 session：已完成或从未建立 Offer session → 安全忽略
    if !state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&transfer_id)
    {
        return;
    }
    // 权限：群存在 && sender 是群成员
    let (group_exists, sender_is_member) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => (true, g.members.contains(&sender_id)),
            None => (false, false),
        }
    };
    if !group_exists || !sender_is_member {
        return;
    }
    let Some(gf) = db::get_group_file(
        &state.db.lock().unwrap_or_else(|e| e.into_inner()),
        &transfer_id,
    ) else {
        return;
    };

    // 空文件：无 Chunk 阶段，Done 时才建立接收状态（0 字节 .part）
    if !state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&transfer_id)
    {
        if gf.size == 0 {
            if file::begin_group_receive(
                state,
                &transfer_id,
                &sender_id,
                &gf.name,
                0,
                [0u8; 32],
                gf.sha256.clone(),
            )
            .is_err()
            {
                return;
            }
        } else {
            // 有声明大小但一个分片都没收到 → 不完整：failed + 清理
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
    }

    // 从接收表移除（取得所有权），做最终校验与落盘
    let mut r = match state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&transfer_id)
    {
        Some(r) => r,
        None => return,
    };

    // 1. size 校验：received 必须等于声明大小
    if r.received != r.size {
        let _ = std::fs::remove_file(&r.tmp_path);
        set_gfile_bubble_status(state, &transfer_id, "failed");
        state
            .group_file_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&transfer_id);
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(
                &dbc,
                &transfer_id,
                &state.device_id,
                "failed",
                0.0,
            );
            // 接收 transfer 同步 failed
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &sender_id,
                &gf.name,
                gf.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
        }
        send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
        return;
    }
    // 2. SHA-256 校验：finalize 增量哈希（不重读 .part），与发送方声明比对
    {
        use sha2::Digest;
        let actual_hex: String = r
            .hasher
            .clone()
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if !actual_hex.eq_ignore_ascii_case(&r.expected_sha256) {
            let _ = std::fs::remove_file(&r.tmp_path);
            set_gfile_bubble_status(state, &transfer_id, "failed");
            state
                .group_file_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id);
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ = db::update_group_file_recipient(
                    &dbc,
                    &transfer_id,
                    &state.device_id,
                    "failed",
                    0.0,
                );
            }
            send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
            return;
        }
    }
    // 3. sync_all：落盘前确保数据写透
    if let Err(e) = r.file.sync_all() {
        let _ = e.to_string();
        let _ = std::fs::remove_file(&r.tmp_path);
        set_gfile_bubble_status(state, &transfer_id, "failed");
        state
            .group_file_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&transfer_id);
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(
                &dbc,
                &transfer_id,
                &state.device_id,
                "failed",
                0.0,
            );
            // 接收 transfer 同步 failed
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &sender_id,
                &gf.name,
                gf.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
        }
        send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
        return;
    }
    // 4. drop 文件句柄后 rename（Windows 不允许 rename 打开中的文件）
    drop(r.file);
    // 最终名是"开始接收"时就定下的，那一刻同样在途的同名传输还没落盘、unique_path 看不到它
    // → 两条同名传输可能选中同一个名字。这里在真正落盘前再确认一次：被占走就换名。
    // （单聊路径本来就是在写盘时才定名，所以没有这个竞态——这也是"群聊出问题、单聊正常"的原因。）
    if r.final_path.exists() {
        let dl = state
            .downloads_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        r.final_path = file::unique_path(&dl, &r.name);
    }
    if std::fs::rename(&r.tmp_path, &r.final_path).is_err() {
        let _ = std::fs::remove_file(&r.tmp_path);
        set_gfile_bubble_status(state, &transfer_id, "failed");
        state
            .group_file_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&transfer_id);
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(
                &dbc,
                &transfer_id,
                &state.device_id,
                "failed",
                0.0,
            );
            // 接收 transfer 同步 failed
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &sender_id,
                &gf.name,
                gf.size,
                "receive",
                "failed",
                None,
                0.0,
            )
            .ok();
        }
        send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, false).await;
        return;
    }
    // 全部成功：正式文件已落盘 → completed / progress 1.0 → 气泡转 delivered → 清理
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ =
            db::update_group_file_recipient(&dbc, &transfer_id, &state.device_id, "completed", 1.0);
        // 群文件本地路径持久化到 transfer 记录：打开/另存/历史加载经
        // transfer_id（gfile-{tid}）关联到该真实本地路径（重启后仍有效）
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &sender_id,
            &gf.name,
            gf.size,
            "receive",
            "done",
            Some(r.final_path.to_string_lossy().as_ref()),
            1.0,
        )
        .ok();
        // 群聊里"已收完的成员"同样登记为种子：C 可从 B 拉（ADR-0019 Phase 3）。
        let _ = crate::content::store::record_local(
            &dbc,
            &gf.sha256,
            &sender_id,
            Some(&group_id),
            &gf.name,
            gf.size,
            crate::content::model::Direction::Receive,
            &r.final_path.to_string_lossy(),
            db::now_ms(),
        );
    }
    state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&transfer_id);
    state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&transfer_id);
    // `scope == "todo"`：待办描述图片只走传输管线，不回填聊天气泡、不 emit message-received
    // （文件已按 sha256 经 content store 登记，待办卡片按 sha256 即可解析本地路径）。
    if gf.scope != "todo" {
        // 重发带本地路径的记录（applyIncoming 按 msg_id 合并更新，未读不重复）：
        // 前端气泡 content.path 就绪 → 打开/另存/图片代码预览立即可用
        let msg_id = format!("gfile-{transfer_id}");
        let seq = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            dbc.query_row(
                "SELECT seq FROM messages WHERE msg_id = ?1",
                params![msg_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(1)
        };
        let subtype = file::classify_file_subtype(&gf.name);
        // 同口径收敛（完成回填路径的气泡重发也不能再把 kind 打回 video/audio）
        let kind = if subtype == "image" { "image" } else { "file" };
        let done_rec = crate::state::MessageRecord {
            id: 0,
            msg_id,
            conv_id: format!("group:{group_id}"),
            sender_id: sender_id.clone(),
            receiver_id: state.device_id.clone(),
            kind: kind.to_string(),
            content: serde_json::json!({
                "name": gf.name,
                "path": r.final_path.to_string_lossy(),
                "size": gf.size,
                "sha256": gf.sha256,
                "subtype": subtype,
                "progress": 1.0,
            })
            .to_string(),
            ts: db::now_ms(),
            seq,
            status: "delivered".to_string(),
            // 同上：群文件收尾这条不带 @ 落点 ⇒ NULL。
            mention_targets: None,
        };
        // 回填本地 path 到 messages 表：read_file_preview 按 msg_id 反查 content 定位文件。
        // 单聊 FileDone 走 insert_message 直接落库带 path 的内容；群聊 Offer 先落库无 path 的
        // 内容（文件尚未下载），Done 时必须显式更新，否则接收方图片/代码预览因缺 path 失败。
        {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::update_message_content(&dbc, &done_rec.msg_id, &done_rec.content, &done_rec.status)
                .ok();
        }
        let _ = state.app.emit("message-received", &done_rec);
        // 群**接收**端也必须 emit `file-done`：单聊、中继、群发送方都发，唯独这里漏了。
        // 漏掉不是"少一个事件"这么轻 —— 接收端在字节落盘前读预览会得到「已被清理」，
        // 而那个确定性失败判定被**永久缓存**（utils/filePreview.ts 只缓存确定性失败），
        // 清缓存的唯一入口就是 `onFileDone` 里的 invalidateFilePreview。不触发 ⇒
        // 文件早就好好躺在磁盘上，气泡却永远空白；重启前都不会自己好回来。
        let _ = state.app.emit(
            "file-done",
            &crate::state::FileDoneInfo {
                transfer_id: transfer_id.to_string(),
                name: gf.name.clone(),
                size: gf.size,
                path: r.final_path.to_string_lossy().to_string(),
            },
        );
    }
    // 完成确认：无论 ACK 发送成败，file_key 已清理不再保留（ACK 丢失由后续阶段处理）
    send_group_file_complete_ack(state, &transfer_id, &group_id, &sender_id, true).await;
}

/// 向原始群文件发送者回送接收完成确认（receiver → sender）。
/// ACK 丢失可接受（发送端保持 sending，等待后续 retry/offline recovery），
/// 不因此重新保留 file_key。
async fn send_group_file_complete_ack(
    state: &Arc<AppState>,
    transfer_id: &str,
    group_id: &str,
    original_sender: &str,
    success: bool,
) {
    let ack = Message::GroupFileCompleteAck {
        transfer_id: transfer_id.to_string(),
        group_id: group_id.to_string(),
        sender_id: state.device_id.clone(),
        success,
    };
    let _ = try_send(state, original_sender, &ack).await;
}

/// 处理接收完成确认（sender 侧）：更新对应 recipient 的 completed/failed 状态。
///
/// 身份验证（防伪造）：
/// 1. ACK.sender_id == TCP peer_id（不能只相信消息字段）；
/// 2. sender_id != 本机；
/// 3. transfer 对应的 group_file.sender_id == 本机（只有本机发起的群文件
///    的 ACK 才会被处理，B 发给 A 的 transfer 的 ACK 到 C 手上会被拒绝）；
/// 4. ACK 发送者必须是该 transfer 的 recipient（update 不命中即拒绝）。
///
/// 幂等：重复 ACK 重复 UPDATE 同状态，无副作用、不报错。
async fn handle_group_file_complete_ack(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
    success: bool,
) {
    // 1+2. ACK 发送者必须与链路对端一致，且不能是自己
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    // 3. transfer 必须是本机发出的群文件，且 group_id 一致
    let Some(gf) = db::get_group_file(
        &state.db.lock().unwrap_or_else(|e| e.into_inner()),
        &transfer_id,
    ) else {
        return;
    };
    if gf.sender_id != state.device_id || gf.group_id != group_id {
        return;
    }
    // 幂等保护：completed 是终态。该 recipient 已 completed 时，
    // 后续 success=false ACK（重放/异常）不得把 completed 降级为 failed；
    // success=true ACK 重复到达则是无害的幂等更新。
    if !success {
        let already_completed = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::list_group_file_recipients(&dbc, &transfer_id)
                .unwrap_or_default()
                .into_iter()
                .any(|r| r.recipient_id == peer_id && r.status == "completed")
        };
        if already_completed {
            return;
        }
    }
    // 4. ACK 发送者必须是该 transfer 的 recipient，且状态按 success 迁移；
    //    只修改该 recipient，不影响其他成员。不命中（非 recipient）→ 拒绝。
    let (status, progress) = if success {
        ("completed", 1.0)
    } else {
        ("failed", 0.0)
    };
    // 发送端气泡 = 全体 recipient 结果的聚合（v0.12 最小语义），
    // 避免最后一个 ACK 直接覆盖之前更准确的总体状态：
    // - success ACK → delivered（有人收到即算；mixed 亦然，且不回退）
    // - failure ACK → 全部 recipient 都到终态（completed/failed）时：
    //     有人 completed → delivered；全部 failed → failed；
    //   仍有 pending/sending → 气泡保持当前状态（sending），等后续 ACK。
    let bubble;
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(&dbc, &transfer_id, peer_id, status, progress);
        if success {
            bubble = "delivered";
        } else {
            let recipients = db::list_group_file_recipients(&dbc, &transfer_id).unwrap_or_default();
            let all_terminal = recipients
                .iter()
                .all(|r| r.status == "completed" || r.status == "failed");
            if !all_terminal {
                return; // 仍有进行中的 recipient：气泡保持当前，等后续 ACK
            }
            bubble = if recipients.iter().any(|r| r.status == "completed") {
                "delivered"
            } else {
                "failed"
            };
        }
        let _ = db::set_message_status(&dbc, &format!("gfile-{transfer_id}"), bubble).ok();
        // sender 的 transfer 记录随聚合结果推进。
        // 状态：delivered → done，否则 failed（保留原有语义）。
        // 进度：按**在线成员**口径算，而不是「delivered 就写 1.0」——
        // 否则「在线成员全到了、但离线成员还 pending」时进度条会提前满格，
        // 与用户口径（离线不计入分母，在线全到才算完）相冲突。
        // ⚠️ `group_file_online_progress` 内部取 db 锁：必须先 drop 本段持有的锁，
        // 否则 std Mutex 同锁重入即死锁。
        let tf_status = if bubble == "delivered" {
            "done"
        } else {
            "failed"
        };
        let path = db::list_transfers(&dbc)
            .unwrap_or_default()
            .into_iter()
            .find(|t| t.id == transfer_id)
            .and_then(|t| t.path);
        drop(dbc);
        let tf_progress = crate::commands::group_file_online_progress(state, &transfer_id, 0.0);
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            &transfer_id,
            &gf.group_id,
            &gf.name,
            gf.size,
            "send",
            tf_status,
            path.as_deref(),
            tf_progress,
        )
        .ok();
        // 本 transfer 已到终态（delivered/failed）→ 进度条口径快照用完即弃，
        // 避免无界增长；注意**不能**只在 `tf_progress >= 1.0` 时清 ——
        // 「发送时无人在线」的 transfer 进度恒 0，那样就永远清不掉。
        if bubble == "delivered" || bubble == "failed" {
            state
                .group_file_online_targets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&transfer_id);
        }
    }
    if bubble == "delivered" {
        let _ = state
            .app
            .emit("message-acked", &format!("gfile-{transfer_id}"));
    }
}

/// 处理群文件分片（E2EE 解密 → seq/size 校验 → 增量哈希 → 写 `.part`）。
/// 无 GroupFileDone / 无 rename / 不标 completed——完成确认在下一阶段。
async fn handle_group_file_chunk(
    state: &Arc<AppState>,
    peer_id: &str,
    transfer_id: String,
    group_id: String,
    sender_id: String,
    seq: u32,
    data: String,
) {
    // 权限：链路 sender 与声明一致、不能是自己、群存在、sender 是群成员
    if sender_id != peer_id || sender_id == state.device_id {
        return;
    }
    let (group_exists, sender_is_member) = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => (true, g.members.contains(&sender_id)),
            None => (false, false),
        }
    };
    if !group_exists || !sender_is_member {
        return;
    }
    // 会话必须已经由合法 GroupFileOffer 建立；无 key 直接丢弃（不尝试其他密钥）
    let Some(file_key) = state
        .group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&transfer_id)
        .copied()
    else {
        return;
    };
    // 本地群文件记录（Offer 阶段建立）提供 name/size/sha256
    let Some(gf) = db::get_group_file(
        &state.db.lock().unwrap_or_else(|e| e.into_inner()),
        &transfer_id,
    ) else {
        return;
    };
    // 只有该 transfer 的原始 sender 发来的 chunk 才合法。
    // 其他群成员（无 file_key）发送的垃圾 chunk 不得终止合法接收：
    // 直接忽略，不删 .part、不清 session、不改 recipient 状态。
    if gf.sender_id != sender_id {
        return;
    }

    // 首个合法 chunk 到达时才创建 `.part`（安全路径，downloads 目录内）
    if !state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&transfer_id)
        && file::begin_group_receive(
            state,
            &transfer_id,
            &sender_id,
            &gf.name,
            gf.size,
            file_key,
            gf.sha256.clone(),
        )
        .is_err()
    {
        return;
    }

    // 解密（AEAD 失败 → 失败收尾：删 `.part`、置 failed，不写错误明文）
    let Ok(sealed) = STANDARD.decode(&data) else {
        fail_group_file_chunk(state, &transfer_id);
        return;
    };
    let Some(plaintext) = crypto::open_symmetric(&file_key, &sealed) else {
        fail_group_file_chunk(state, &transfer_id);
        return;
    };

    // seq / size 校验与写盘（复用一对一 FileReceiver 的严格语义）
    {
        use std::io::Write;
        let mut recv = state
            .group_file_receivers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(r) = recv.get_mut(&transfer_id) else {
            return;
        };
        // ⚠️ 群侧以前这里是 `if seq != r.next_seq { 整单失败 }`，注释写的理由是"TCP 有序"。
        // 那个前提在群里不成立：群文件经**中继扇出**，同一片可以被两条链路各送一次，
        // 而单聊那一族早就为同一件事付过学费（`file.rs` 里 `chunk_seq_decision` 的注释：
        // 把重复/迟到当致命 ⇒ 整单永远拼不齐）。规则只许有一份 ⇒ 这里直接调那个纯函数。
        let seq_call = file::chunk_seq_decision(seq, r.next_seq);
        if seq_call == file::ChunkSeq::Duplicate {
            // 重复/迟到片：与单聊同一处置（不写盘、不打死、不回错误），下一片照常进来。
            drop(recv);
            return;
        }
        if seq_call == file::ChunkSeq::Gap {
            // 真跳号：中间确实缺片，只能靠重传 ⇒ 终止这一单。
            drop(recv);
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
        if plaintext.len() as u64 > r.size.saturating_sub(r.received) {
            // 超出声明大小：防恶意 sender
            drop(recv);
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
        // 增量哈希（为下一阶段 FileDone 校验准备；本阶段不比对）
        use sha2::Digest;
        r.hasher.update(&plaintext);
        if r.file.write_all(&plaintext).is_err() {
            drop(recv);
            fail_group_file_chunk(state, &transfer_id);
            return;
        }
        r.received += plaintext.len() as u64;
        // 群接收器与单聊共用 `FileReceiver` ⇒ 同一个 idle 时钟，回收判据也只有一份
        r.fed_at_ms = db::now_ms();
        r.next_seq = r.next_seq.wrapping_add(1);
        let progress = if r.size == 0 {
            1.0
        } else {
            (r.received as f64 / r.size as f64).min(1.0)
        };
        drop(recv);
        // 进度落库：本阶段最高 sending，不标 completed
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db::update_group_file_recipient(
            &dbc,
            &transfer_id,
            &state.device_id,
            "sending",
            progress,
        );
    }
}
