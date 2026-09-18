// 职责边界：
// - 自绘标题栏窗口控制（最小化/最大化/全屏/关闭）
// - window_close 里主窗口隐藏 vs 辅助窗口销毁的语义
// ---------------- 自绘标题栏：窗口控制 ----------------
// ⚠️ 这些命令作用于**调用它们的那个窗口**（Tauri 把发起 IPC 的窗口注入 `WebviewWindow` 参数）。
// 主窗口与各辅助窗口共用同一份自绘标题栏（`src/components/TitleBar.vue`），所以**不能写死 "main"**
// —— 否则辅助窗口的最小化/关闭会作用到主窗口上（用户 2026-09-17：辅助窗口要脱离系统标题栏）。

/// 最小化**当前窗口**。
#[cfg(desktop)]
#[tauri::command]
pub fn window_minimize(window: tauri::WebviewWindow) {
    let _ = window.minimize();
}

/// 移动端没有独立窗口概念，最小化由系统接管。
#[cfg(mobile)]
#[tauri::command]
pub fn window_minimize(_app: tauri::AppHandle) {}

/// 切换**当前窗口**最大化，返回切换后的状态。
///
/// 先判 `is_maximizable`：设置/日志/群任务窗口在 builder 上设了 `.maximizable(false)`
/// （小窗口不给最大化），这里再兜一道，免得 UI 层的按钮判断漏了。
#[cfg(desktop)]
#[tauri::command]
pub fn window_toggle_maximize(window: tauri::WebviewWindow) -> bool {
    if !window.is_maximizable().unwrap_or(true) {
        return false;
    }
    match window.is_maximized() {
        Ok(true) => {
            let _ = window.unmaximize();
            false
        }
        _ => {
            let _ = window.maximize();
            true
        }
    }
}

/// 移动端窗口始终铺满屏幕，等价于「不可再最大化」。
#[cfg(mobile)]
#[tauri::command]
pub fn window_toggle_maximize(_app: tauri::AppHandle) -> bool {
    false
}

/// 返回**当前窗口**是否最大化。
#[cfg(desktop)]
#[tauri::command]
pub fn window_is_maximized(window: tauri::WebviewWindow) -> bool {
    window.is_maximized().unwrap_or(false)
}

/// 移动端没有"最大化"概念。
#[cfg(mobile)]
#[tauri::command]
pub fn window_is_maximized(_app: tauri::AppHandle) -> bool {
    false
}

/// 切换**当前窗口**全屏，返回切换后的状态。
/// 用于 macOS 绿灯的 option-click（HIG：缩放按钮按住 Option 即进入/退出全屏）。
#[cfg(desktop)]
#[tauri::command]
pub fn window_toggle_fullscreen(window: tauri::WebviewWindow) -> bool {
    match window.is_fullscreen() {
        Ok(true) => {
            let _ = window.set_fullscreen(false);
            false
        }
        _ => {
            let _ = window.set_fullscreen(true);
            true
        }
    }
}

/// 移动端无"全屏"概念（窗口本就铺满屏幕），返回 false。
#[cfg(mobile)]
#[tauri::command]
pub fn window_toggle_fullscreen(_app: tauri::AppHandle) -> bool {
    false
}

/// 关闭**当前窗口**。
///
/// **主窗口 = 隐藏**（托盘语义：关掉窗口 ≠ 退出应用，与 `tray.rs` 的 CloseRequested 一致）；
/// **辅助窗口 = `close()`** —— 外链窗口（常驻）由 `install_hide_on_close` 转成隐藏，
/// 其余（设置/日志/群任务，均不常驻）直接销毁。写死 "main" 会让辅助窗口的关闭键把**主窗口**藏起来。
#[cfg(desktop)]
#[tauri::command]
pub fn window_close(window: tauri::WebviewWindow) {
    if window.label() == crate::WINDOW_MAIN {
        let _ = window.hide();
    } else {
        let _ = window.close();
    }
}

/// 移动端：保持既有行为（隐藏主窗口）。
#[cfg(mobile)]
#[tauri::command]
pub fn window_close(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(crate::WINDOW_MAIN) {
        let _ = w.hide();
    }
}

#[tauri::command(async)]
/// 群消息发送的**唯一内核**：群密钥加密 → Gossip 信封 → 落库（消息 + 每个成员的 outbox）
/// → 广播。文本、代码、表情回应等全部走这一条路。
///
/// 为什么必须只有一条：它们都要 E2EE、都要 outbox 兜底、都要 GroupAck、都要被四层幂等
/// 去重覆盖。若各写一份，任何一处修 bug（历史上最典型的是「填完 group_creator/members
/// 后忘了重算重签 → 群消息被静默丢弃」）都只会修到其中一条路径。
async fn send_group_payload(
    s: &Arc<AppState>,
    group_id: &str,
    kind: &str,
    content: String,
) -> Result<MessageRecord, String> {
    let ts = db::now_ms();
    let conv_id = format!("group:{group_id}");
    let seq = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_clock(&dbc, &conv_id).map_err(|e| format!("逻辑时钟推进失败：{e}"))?
    };
    // 把群名 + 创建者 + 当前成员一并带上：跨端成员即便从未收到 GroupKey、
    // 只凭这条群消息也能在本地正确建群（含成员表），成员面板因此不为空。
    let group_meta = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, group_id).map(|g| (g.name, g.creator, g.members))
    };
    let (group_name, group_creator, group_members) = match group_meta {
        Some((n, c, m)) => (n, Some(c), m),
        None => return Err("群不存在".to_string()),
    };
    if !group_members.contains(&s.device_id) {
        return Err("你已不在该群中".to_string());
    }
    let key = get_group_key(s, group_id).await.ok_or("群密钥缺失")?;
    let preview = crate::protocol::preview_text(kind, &content);

    // 群密钥加密 + Gossip 信封（E2EE 恒开：载荷用群密钥 ChaCha20-Poly1305 加密）
    let plaintext = serde_json::json!({ "kind": kind, "content": content }).to_string();
    let sealed = crypto::seal_symmetric(&key, plaintext.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        let mut env = gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::Group,
            Some(group_id.to_string()),
            Some(group_name.clone()),
            &payload_b64,
            ts,
            seq,
        );
        env.group_creator = group_creator;
        env.group_members = group_members.clone();
        // group_creator / group_members 属于签名材料（GossipEnvelope::signing_bytes），
        // 而 build_envelope 内部已按「尚未填值」的状态算过 message_id 与 sender_sig。
        // 若此处不重算重签，接收端 verify_envelope 会用最终字段重新计算签名材料，
        // 与旧签名不一致 → 验签失败 → handle_gossip 静默丢弃群消息（群聊收不到的根因）。
        // compute_message_id 只依赖 sender_id + ts + payload，重算后 message_id 不变，
        // 与既有协议语义保持一致。
        env.compute_message_id();
        env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
        env
    };
    // 信封 encrypted 默认 true（build_envelope 内置），无需改写

    // 本地落库：msg_id 统一用 envelope.message_id（与单聊发送路径一致），
    // 保证同一条群消息在本地记录 / Gossip 投递 / 接收端落库三处身份一致。
    let rec = MessageRecord {
        id: 0,
        msg_id: env.message_id.clone(),
        conv_id: conv_id.clone(),
        sender_id: s.device_id.clone(),
        receiver_id: group_id.to_string(),
        kind: kind.to_string(),
        content: content.clone(),
        ts,
        seq,
        status: "sent".to_string(),
    };
    // 群消息与单聊一样需要可靠投递：本地落库 + 每个成员的 outbox 在同一事务里完成，
    // 再由建链 / Hello / 心跳触发 flush_group_outbox 补发，收到 GroupAck 才删行。
    let gossip_msg = Message::Gossip {
        envelope: env.clone(),
    };
    let payload = serde_json::to_string(&gossip_msg).map_err(|e| e.to_string())?;
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        db::insert_message(&tx, &rec).map_err(|e| format!("消息写入失败：{e}"))?;
        if crate::protocol::is_non_notifying_kind(kind) {
            // 静默事件与系统提示不改会话预览 —— 否则「自己回了个表情」会把会话列表摘要
            // 变成一段 JSON。会话行仍要确保存在。
            db::ensure_conversation(&tx, &conv_id, "group", &group_name, None)
                .map_err(|e| format!("会话写入失败：{e}"))?;
        } else {
            db::touch_conversation(&tx, &conv_id, "group", &group_name, None, &preview, 0)
                .map_err(|e| format!("会话写入失败：{e}"))?;
        }
        for member in &group_members {
            if member == &s.device_id {
                continue;
            }
            db::insert_group_outbox(&tx, &rec.msg_id, group_id, member, &payload)
                .map_err(|e| format!("群消息入队失败：{e}"))?;
        }
        tx.commit().map_err(|e| format!("群消息写入失败：{e}"))?;
    }

    broadcast_gossip(s, env).await;

    Ok(rec)
}
