// 职责边界：`handle_message` 的「身份握手与好友关系」分支组 —— 只处理 `Hello`、`Heartbeat`、`UserInfo`、`FriendRequest`、`FriendAccept`、`FriendReject`、`FriendRemove`、`FriendMessageBlocked` 这些变体；不做路由、不起新连接，落库/发送都在各自臂体里（与原函数逐字相同）。
// 为什么单独一册：`handle_message` 原来 1,540 行 / 38 个臂挤在一个函数里。已用 AST 现量过
//   **match 之前没有裸 let 绑定、分支之间不共享局部量** ⇒ 按消息族拆出去是机械搬家而不是改控制流，
//   依据与量法见 docs/large-file-split-plan.md §5.1。
// 恒等判据（每族都跑同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点仍恰好命中一次、clippy `-D warnings`、`cargo fmt --check`。
// 为什么并成一族：这 8 个变体都在回答同一个问题——「对端是谁、能不能信」。身份锚点打标（mark_peer_keys_verified）、
//   换号后不覆盖已绑定密钥、好友同意的两条路径走同一个家，全都住在这里；留在 handle_message 里时
//   「打标点」与「它必须排在 upsert_peer 之后」这两条守卫隔着几百行互相看不见。

async fn handle_identity_and_friend_messages(state: &Arc<AppState>, peer_id: &str, msg: Message) {
    // 这层 match 与原函数里的同一层（臂仍是 8 格缩进 ⇒ 逐字未改）。
    // `_ => {}` 不是吞消息：调用方只在命中本族变体时才把 msg 交进来。
    match msg {
        Message::Hello {
            device_id,
            nickname,
            avatar,
            device_type,
            content_features,
            protocol_version,
            app_version,
            tcp_port,
            x25519_pubkey,
            ed25519_pubkey,
            conv_clock,
            ..
        } => {
            if device_id != peer_id {
                return;
            }
            // 记录对端的内容能力位（不签名，仅用于"是否发拉取帧"）。
            state
                .peer_content_features
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(device_id.clone(), content_features);
            // 记录对端声明的版本（同样不签名）。TCP 与 BLE 都走这一个写入点：
            // 两条 transport 建链后都会把这个已验签的 Hello 交回 `handle_message`。
            state
                .peer_versions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    device_id.clone(),
                    crate::state::PeerVersion {
                        protocol_version,
                        app_version,
                    },
                );
            let ip = state
                .peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&device_id)
                .map(|p| p.ip.clone())
                .unwrap_or_default();
            // 打标要留一份钥匙：下面 `upsert_peer` 会把这两个绑定 move 进去。
            let (hello_x, hello_e) = (x25519_pubkey.clone(), ed25519_pubkey.clone());
            upsert_peer(
                state,
                &device_id,
                &nickname,
                avatar.clone(),
                &device_type,
                &ip,
                tcp_port,
                Some(x25519_pubkey),
                Some(ed25519_pubkey),
                None,
            )
            .await;
            // ⚠️ 这一句必须排在 `upsert_peer` **之后**，而且是**唯一**一条对所有 transport
            // 都成立的打标点（TCP 入站 / 出站拨号 / BLE 建链后都把已验签的 Hello 交回这里）。
            // 握手处那三次打标只覆盖"`peers` 条目已经由 announce 建好"的情形：条目不存在时
            // `mark_peer_keys_verified` 是空操作，而 `upsert_peer` 新建条目恒标
            // `keys_verified: false` ⇒ 只靠握手处打标，第一次连上的好友整个会话都绑不上
            // 身份锚点（表现为安全码算不出、公网中继永不准入 —— 且没有任何报错）。
            mark_peer_keys_verified(state, &device_id, &hello_x, &hello_e);
            // 对齐单聊逻辑时钟：避免离线期间的时钟落差让后续新消息序号偏小。
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::observe_clock(&dbc, &device_id, conv_clock).ok();
            }
            maybe_update_friend(state, &device_id, &nickname, avatar);
            flush_outbox(state, &device_id).await;
            // 同建链路径：群密钥先于群消息，且每次 Hello 都重新登记一遍（幂等、帧很小）。
            requeue_group_keys_for_peer(state, &device_id);
            flush_pending_group_keys(state, &device_id).await;
            flush_group_outbox(state, &device_id).await;
            flush_pending_reads(state, &device_id).await;
            flush_pending_group_reads(state, &device_id).await;
            crate::commands::flush_pending_files(state, &device_id).await;
            // 群文件离线投递：该 peer 的 pending GroupFile 顺序发送
            crate::commands::flush_pending_group_files(state, &device_id).await;
            // 好友申请没有回执：建链补全时补发一次（用户真机：链路抖动丢过一次，
            // 对方什么都没收到，而我方界面显示"已发送"）。
            flush_pending_friend_request(state, &device_id).await;
            // 好友同意回执同样没有回执：建链后补发（真机：Mac 端好友状态一直没同步）。
            flush_pending_friend_accept(state, &device_id).await;
            // Phase 1：建链即**自动重试**该 peer 名下未完成的可恢复内容
            // （只对声明了拉取能力的对端发 ContentRequest；退避未到点的跳过）。
            retry_incomplete_content(state, &device_id).await;
            // 建链后把**我的完整资料**（含大头像）定向发给这一个对端：
            // Hello 只带小头像、Presence 不再内联大头像，这里是「大头像只同步一次」
            // 的正式路径。LAN 上瞬间完成；BLE 上走 bulk，慢但不会堵住聊天。
            send_user_info_to(state, &device_id).await;
        }
        // ---- Phase 8（ADR-0017）：外部 mesh（BitChat）的不透明帧，Gosslan 只当中继 ----
        //
        // 三个行为，别的什么都不做：**收得到 · 去得掉重 · TTL 递减后转发**。
        // 不解密、不落库、不建 BitChat 用户/channel；载荷对 Gosslan 永远是不透明字节。
        Message::Heartbeat { device_id } => {
            if device_id != peer_id {
                return;
            }
            touch_peer(state, &device_id).await;
            flush_outbox(state, &device_id).await;
            // 心跳也是一次"链路确实活着"的重发机会（见 `requeue_group_keys_for_peer` 的注释）。
            requeue_group_keys_for_peer(state, &device_id);
            flush_pending_group_keys(state, &device_id).await;
            flush_group_outbox(state, &device_id).await;
            flush_pending_reads(state, &device_id).await;
            flush_pending_group_reads(state, &device_id).await;
            crate::commands::flush_pending_files(state, &device_id).await;
            crate::commands::flush_pending_group_files(state, &device_id).await;
            // 心跳 = 链路确实活着。这一帧丢了的好友申请/同意回执在这里补发，
            // 不必等到链路再断一次、重新建链（真机：点了加好友要等几分钟才有反应）。
            flush_pending_friend_request(state, &device_id).await;
            flush_pending_friend_accept(state, &device_id).await;
        }
        Message::UserInfo {
            device_id,
            nickname,
            avatar,
            device_type,
        } => {
            if device_id != peer_id {
                return;
            }
            let ip = state
                .peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&device_id)
                .map(|p| p.ip.clone())
                .unwrap_or_default();
            upsert_peer(
                state,
                &device_id,
                &nickname,
                avatar.clone(),
                &device_type,
                &ip,
                0,
                None,
                None,
                None,
            )
            .await;
            maybe_update_friend(state, &device_id, &nickname, avatar.clone());
            // 同步更新 single 会话的昵称/头像（conversations DB）
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::update_conversation_profile(&dbc, &device_id, &nickname, avatar.as_deref()).ok();
        }
        Message::FriendRequest {
            from,
            from_nickname,
            from_avatar,
            to,
            ts,
        } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 他已经是我的好友 ⇒ 直接同意（别插 pending，见 auto_accept_if_already_friend）
            if auto_accept_if_already_friend(state, &from).await {
                return;
            }
            let req = PendingRequest {
                from: from.clone(),
                from_nickname: from_nickname.clone(),
                from_avatar: from_avatar.clone(),
                ts,
            };
            state
                .pending_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(from.clone(), req.clone());
            let _ = state.app.emit("friend-request", &req);
            let mut extra = std::collections::HashMap::new();
            extra.insert("type".to_string(), "friend_request".to_string());
            // macOS 点击捕获必须带 action 按钮（见 crate::notifications）；
            // 这条通知不经前端，文案与标题同为固定中文。
            extra.insert("action_label".to_string(), "查看".to_string());
            // 好友申请是**不经前端**的通知：必须走后端开关 + 错误可见的统一入口。
            // 点击（Windows）后唤起窗口并跳到「新的朋友」；移动端点击由插件送回前端。
            let click_app = state.app.clone();
            let _ = crate::notifications::show_click_if_enabled(
                state,
                "好友申请",
                &format!("{from_nickname} 请求添加你为好友"),
                extra,
                false,
                move || {
                    crate::notifications::on_notification_clicked(
                        &click_app,
                        "friend_request",
                        None,
                        None,
                    )
                },
            );
        }
        Message::FriendAccept { from, to } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 直连这一份原先**没有**去重 ⇒ "被『好友申请已通过』反复刷屏"在直连链路上
            // 照旧存在（跨跳那一份早就有这条判据，两份各写一遍正是缺陷的形状）。
            // 现在两条链路都走 `apply_friend_accept` 这一个家。
            apply_friend_accept(state, &from, "收到好友同意");
        }
        Message::FriendReject { from, to } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 回执（同意或拒绝）走同一个清队列入口。这里原先只清**入站**那一张，
            // 而 `forget_pending_request` 的注释写的就是"同意/拒绝都要清两张" ——
            // 漏掉出站登记 ⇒ `flush_pending_friend_request` 每次建链都把已被拒绝的
            // 申请再发一遍，对方那边的「新朋友」里那条申请永远删不掉。
            forget_pending_request(state, &from);
            let _ = state.app.emit("friend-rejected", &from);
        }
        Message::FriendRemove { from, to } => {
            if from != peer_id {
                return;
            }
            if to != state.device_id {
                return;
            }
            // 对方删除了好友关系：移除本地好友行（不删除聊天记录）
            // ⚠️ 与 `remove_friend` 对称：**关系解除就解除身份绑定**，否则对方重装换过公钥后
            // 这条内存里的旧公钥会一直当信任根用（症状同样是"只能重启"）。
            forget_peer_identity(state, &from);
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::remove_friend(&dbc, &from).ok();
            drop(dbc);
            let _ = state.app.emit("friend-removed", &from);
        }
        Message::FriendMessageBlocked {
            ref from,
            ref to,
            ref original_sender,
        } => {
            if from != peer_id {
                return;
            }
            if to == &state.device_id {
                // 目标是本机：通知前端
                let _ = state.app.emit("friend-message-blocked", from);
            } else if original_sender != &state.device_id {
                // 中继节点：转发给原始发送方（与 Ack relay 同逻辑）
                let _ = try_send(
                    state,
                    original_sender,
                    &Message::FriendMessageBlocked {
                        from: from.clone(),
                        to: to.clone(),
                        original_sender: original_sender.clone(),
                    },
                )
                .await;
            }
        }
        _ => {}
    }
}
