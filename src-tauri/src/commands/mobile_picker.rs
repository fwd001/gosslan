// 职责边界：
// - 移动端 content:// URI 落地到应用私有目录
// - Android 相册/文件选择器回调
// ---------------- 移动端文件选择：content:// 必须"落地"成真实文件 ----------------

/// 从 `content://` URI 里尽力取出**真实文件名**（含扩展名）。
///
/// - external-storage / documents 提供者会把相对路径编码进 URI
///   （`…/document/primary%3ADownload%2Freport.pdf`）⇒ 解出来就是 `report.pdf`；
/// - MediaStore（相册）只给数字 id（`…/images/media/1000000033`）⇒ 拿不到名字，
///   返回 `None`，由调用方按**内容嗅探**补一个（见 [`sniff_media_ext`]）。
///
/// 纯函数：主机上可直接单测（真机行为无法在无设备环境验证，规则必须先钉住）。
pub fn name_from_content_uri(uri: &str) -> Option<String> {
    let after_scheme = uri.split_once("://").map(|(_, rest)| rest).unwrap_or(uri);
    let last = after_scheme.rsplit('/').next()?;
    // 百分号解码（`%2F` → `/`、`%3A` → `:`），只处理这两种 + 空格，够用且不做过度解码
    let decoded = last
        .replace("%2F", "/")
        .replace("%2f", "/")
        .replace("%3A", ":")
        .replace("%3a", ":")
        .replace("%20", " ");
    // `primary:Download/report.pdf` ⇒ 取最后一段
    let candidate = decoded
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(&decoded)
        .to_string();
    // 必须是"像文件名"的东西：有点、有扩展名、长度合理、不含危险字符
    let ok = candidate.len() > 3
        && candidate.len() <= 180
        && candidate.contains('.')
        && !candidate.starts_with('.')
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_. ()[]（）【】".contains(c));
    if ok {
        Some(candidate)
    } else {
        None
    }
}

/// 按**文件头**判断媒体类型（拿不到扩展名时用）。
///
/// 为什么按内容而不是扩展名：Android 相册给的 URI 没有文件名，
/// 而"从相册选图片"必须发成**图片**消息（否则用户看到的是一个附件）。
pub fn sniff_media_ext(head: &[u8]) -> &'static str {
    match head {
        [0xFF, 0xD8, 0xFF, ..] => "jpg",
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'B', b'M', ..] => "bmp",
        [0x25, b'P', b'D', b'F', ..] => "pdf",
        [b'P', b'K', 0x03, 0x04, ..] => "zip",
        _ => "bin",
    }
}

/// 文件名消毒：去掉目录分隔符与控制字符，避免写到缓存目录之外（路径穿越）。
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').to_string();
    if trimmed.is_empty() {
        "file.bin".to_string()
    } else {
        trimmed.chars().take(180).collect()
    }
}

/// 把「文件选择器」交给我们的东西**落地**成一个真实可读的文件路径。
///
/// 为什么必须有这一步（用户 2026-09-12 安卓真机实测：「文字能发、代码能发，
/// 但发附件/图片总是失败」）：Android 的系统文件选择器返回的是 **`content://` URI**，
/// 不是文件路径 —— `tauri-plugin-dialog` 的 Kotlin 侧直接把 `uri.toString()` 交给前端
/// （插件里那个 `FilePickerUtils.getPathFromUri` 是**没人调用**的死代码）。
/// 于是这个 URI 被原样传给 Rust 的文件发送逻辑，`std::fs::metadata("content://…")`
/// 必然失败 ⇒「文件不存在或不可读」/「发送失败」。桌面端返回的本来就是真实路径，
/// 所以**只有安卓会这样**。
///
/// 修法：URI 走 `tauri-plugin-fs` 的跨平台 API（Android 侧会经 ContentResolver 解析）
/// 复制到应用缓存目录，再把真实路径交给原本的发送链路 —— 后面的图片预览、缩略图、
/// 断点重传全都不用改。
#[tauri::command(async)]
pub async fn import_picked_file(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
    suggested_name: Option<String>,
) -> Result<String, String> {
    use tauri_plugin_fs::FsExt;
    if !path.starts_with("content://") {
        // 桌面 / 已经是真实路径（`file://` 或普通路径）：原样返回
        return Ok(path);
    }
    let dir = state.cache_dir.join("imports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导入目录失败：{e}"))?;
    let tmp = dir.join(format!(".incoming-{}", uuid::Uuid::new_v4()));
    // `content://` 必须走 `FilePath::Url`（fs 插件在安卓侧据此走 ContentResolver 解析）
    let uri = url::Url::parse(&path).map_err(|e| format!("无法解析所选文件的 URI：{e}"))?;
    let mut src = app
        .fs()
        .open(uri, tauri_plugin_fs::OpenOptions::new().read(true).clone())
        .map_err(|e| format!("打不开所选文件（{e}）"))?;
    let mut out = std::fs::File::create(&tmp).map_err(|e| format!("写入临时文件失败：{e}"))?;
    std::io::copy(&mut src, &mut out).map_err(|e| format!("复制所选文件失败：{e}"))?;
    drop(out);
    // 名字优先级：URI 里能解出来的 > 前端给的显示名 > 按内容嗅探补一个
    let head = {
        use std::io::Read as _;
        let mut head = [0u8; 16];
        let mut f = std::fs::File::open(&tmp).map_err(|e| e.to_string())?;
        let n = f.read(&mut head).unwrap_or(0);
        head[..n].to_vec()
    };
    let ext = sniff_media_ext(&head);
    let name = name_from_content_uri(&path)
        .or_else(|| suggested_name.map(|n| sanitize_file_name(&n)))
        .unwrap_or_else(|| format!("gosslan-{}.{ext}", db::now_ms()));
    let dest = dir.join(sanitize_file_name(&name));
    std::fs::rename(&tmp, &dest).map_err(|e| format!("落地所选文件失败：{e}"))?;
    state.logger.info(
        "file",
        format!(
            "已把所选文件落地到缓存：{}（{} 字节）",
            dest.display(),
            std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0)
        ),
    );
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command(async)]
pub fn get_pending_requests(state: State<'_, Arc<AppState>>) -> Vec<PendingRequest> {
    let s = state.inner();
    let friend_ids: std::collections::HashSet<String> = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_friends(&dbc)
            .unwrap_or_default()
            .into_iter()
            .map(|f| f.device_id)
            .collect()
    };
    let mut map = s.pending_requests.lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, req| is_actionable_request(req, &friend_ids));
    map.values().cloned().collect()
}

/// 这条好友申请该不该出现在「新朋友」里？
///
/// 判据只有一条：**人已经是好友了 ⇒ 申请不该再出现**（用户 2026-09-12 明确要求）。
/// 抽成纯函数是为了能在主机上直接单测这条规则 —— 它原先散落在"同意"的各条路径里，
/// 直连路径漏了清、跨跳路径清了，表现成"有时候会清、有时候不清，全看对方怎么被加上"。
pub(crate) fn is_actionable_request(
    req: &PendingRequest,
    friend_ids: &std::collections::HashSet<String>,
) -> bool {
    !friend_ids.contains(&req.from)
}

/// **把好友申请真正发出去**（构造定向 Gossip 信封 → 直连就精确发、否则洪泛）。
///
/// 抽出来的原因：它有两个调用方 —— 用户点「加好友」（命令），以及**建链后补发**
/// （`flush_pending_friend_request`，用户真机遇到"已发送但对方没收到"之后加的）。
pub(crate) async fn send_friend_request_via_link(
    s: &Arc<AppState>,
    peer_id: &str,
) -> Result<(), String> {
    // 目标必须在 peers 表（announce / Presence 学到），且需有 X25519 公钥才能 E2EE 加密。
    let target_pubkey = {
        let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers.get(peer_id).and_then(|p| p.x25519_pubkey.clone())
    };
    let Some(target_pubkey) = target_pubkey else {
        return Err("未找到该节点或缺少其公钥，请先重新扫描".to_string());
    };
    let nickname = s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let raw_avatar = s.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // 好友申请是**最需要秒到**的控制帧，且走优先通道：绝不能内联大头像
    // （几百 KB 会分上千片、还可能超过 BLE 单帧上限被整帧丢弃 ⇒ 对方永远收不到，
    // 而界面仍显示"已发送"）。超限就不带头像；建链后由 UserInfo 定向同步大头像。
    let avatar = crate::network::transport::hello_avatar_for_wire(raw_avatar.as_deref());
    // E2EE 加密好友申请内容（昵称/可选头像）；from/to 已在信封 sender_id / target 里。
    let payload =
        serde_json::json!({ "from_nickname": nickname, "from_avatar": avatar }).to_string();
    let shared =
        crypto::shared_secret(&s.identity.x25519_secret, &target_pubkey).ok_or("密钥交换失败")?;
    let sealed = crypto::seal(&shared, payload.as_bytes()).ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let mut env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::FriendRequest,
            None,
            None,
            &payload_b64,
            db::now_ms(),
            0,
        )
    };
    // 定向目标 + 重签（target 参与 signing_bytes）。
    env.target = Some(peer_id.to_string());
    env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
    // 目标直连 → 只发它（精确）；否则广播，靠中间节点按 target 定向转发（跨跳）。
    if s.has_link(peer_id).await {
        try_send(s, peer_id, &Message::Gossip { envelope: env }).await?;
    } else {
        broadcast_gossip(s, env).await;
    }
    Ok(())
}

#[tauri::command(async)]
pub async fn send_friend_request(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
) -> Result<(), String> {
    let s = state.inner();
    if peer_id.is_empty() || peer_id == s.device_id {
        return Err("不能向自己发送好友申请".to_string());
    }
    // **先登记再发**：好友申请没有回执，链路抖动时它会静默丢失，而界面照样显示"已发送"
    //（用户 2026-09-12 真机：对方什么都没收到）。登记后由建链/Hello 补全时重发，
    // 收到同意/拒绝再清除（见 `forget_pending_request`）。
    s.pending_out_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(peer_id.clone());
    send_friend_request_via_link(s, &peer_id).await
}

/// **同意好友申请**这条路径的**唯一**实现：落库（好友 + 会话 + 公钥）→ 回执（Gossip 定向加密）
/// → 清 pending → 通知 UI。
///
/// 为什么必须抽出来：现在有**两个**入口会"同意"——
///   ① 用户在「新朋友」里点同意（`respond_friend_request`）；
///   ② 收到一个**已经是我的好友**的人发来的申请时自动同意（`transport::auto_accept_if_already_friend`）。
/// 两者只要有一处漏了落库或漏了回执，就会造出"我这儿有他、他那儿没我"的**单边好友关系**，
/// 而那种状态在界面上表现为"对方加不上我"（用户 2026-09-12 实测的那个 bug）。
/// **好友同意回执（`FriendAccept`）**这条路径的**唯一**发送实现。
///
/// 为什么抽出来：`accept_friend_request` 发一次之后**没有任何回执**能证明对端收到了，
/// 所以链路抖动时必须能**用同一条路径重发**（`transport::flush_pending_friend_accept`
/// 在建链/心跳时调用）。两处各写一份的话，"重发的那份"迟早会漏掉定向/重签/加密。
pub(crate) async fn send_friend_accept_via_link(
    s: &Arc<AppState>,
    peer_id: &str,
) -> Result<(), String> {
    // 对方公钥优先从 peers 表读（接收 FriendRequest 时已 upsert_peer 记录），
    // friends 表兜底（maybe_update_friend 可能已持久化）。
    let target_pubkey = {
        let from_peers = {
            let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
            peers.get(peer_id).and_then(|p| p.x25519_pubkey.clone())
        };
        let from_friends = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_friend_x25519(&dbc, peer_id)
        };
        from_peers.or(from_friends)
    };
    let Some(target_pubkey) = target_pubkey else {
        // 缺对端公钥时**绝不静默**：本地已加好友，但回执发不出去会导致好友关系
        // 单边成立。打日志留痕（对方 Presence 尚未到达 / 已被 sweep 清理）。
        s.logger.warn(
            "friend",
            format!("同意好友但缺对端公钥，FriendAccept 未发送 peer={peer_id}"),
        );
        return Err("缺对端公钥".to_string());
    };
    let shared =
        crypto::shared_secret(&s.identity.x25519_secret, &target_pubkey).ok_or("密钥交换失败")?;
    let sealed = crypto::seal(&shared, b"{}").ok_or("加密失败")?;
    let payload_b64 = STANDARD.encode(&sealed);
    let mut env = {
        let gossip = s.gossip.lock().unwrap_or_else(|e| e.into_inner());
        gossip.build_envelope(
            &s.identity,
            &s.device_id,
            GossipKind::FriendAccept,
            None,
            None,
            &payload_b64,
            db::now_ms(),
            0,
        )
    };
    env.target = Some(peer_id.to_string());
    env.sender_sig = s.identity.sign_b64(&env.signing_bytes());
    if s.has_link(peer_id).await {
        try_send(s, peer_id, &Message::Gossip { envelope: env }).await?;
    } else {
        broadcast_gossip(s, env).await;
    }
    Ok(())
}

pub(crate) async fn accept_friend_request(s: &Arc<AppState>, peer_id: &str) -> Result<(), String> {
    let name = resolve_nickname(s, peer_id);
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::add_friend(&dbc, peer_id, &name, None).ok();
        db::ensure_conversation(&dbc, peer_id, "single", &name, None).ok();
    }
    // 补写 peers 表已有的公钥到 friends 表：accept 路径此前不写公钥，
    // 而建链（Hello）早于加好友、公钥不变时 key_changed 不触发补写，
    // 导致 friends 公钥永久缺失 → 群密钥分发被静默跳过。
    // 与 transport.rs 中 FriendAccept 接收路径的补写行为一致。
    maybe_update_friend(s, peer_id, &name, None);
    // **先登记再发**（与好友申请同一条纪律）：同意回执没有回执，链路抖动时它会静默丢失，
    // 而发送方界面已显示"已同意" ⇒ 另一端永远停在"等待对方确认"（真机 2026-09-13）。
    {
        let now = db::now_ms();
        s.pending_out_accepts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer_id.to_string(), (now, 0, 0));
    }
    if let Err(e) = send_friend_accept_via_link(s, peer_id).await {
        s.logger.warn(
            "friend",
            format!("好友同意回执发送失败（已登记待补发）peer={peer_id}: {e}"),
        );
    }
    crate::network::transport::forget_pending_request(s, peer_id);
    let _ = s.app.emit("friend-accepted", &peer_id);
    Ok(())
}

#[tauri::command(async)]
pub async fn respond_friend_request(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    accept: bool,
) -> Result<(), String> {
    let s = state.inner();
    if !s
        .pending_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&peer_id)
    {
        return Err("好友申请不存在或已处理".to_string());
    }
    if accept {
        return accept_friend_request(s, &peer_id).await;
    } else {
        // 拒绝回执：跨跳（无直连）时 try_send 会失败，但**绝不因此阻塞本地清理**——
        // 否则「拒绝」发不出去会导致 pending 不被删除、申请「清掉又冒出来」。
        let msg = Message::FriendReject {
            from: s.device_id.clone(),
            to: peer_id.clone(),
        };
        let _ = try_send(s, &peer_id, &msg).await;
        s.pending_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer_id);
        let _ = s.app.emit("friend-rejected", &peer_id);
    }
    Ok(())
}
