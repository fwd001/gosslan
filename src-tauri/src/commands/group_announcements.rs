// 职责边界：
// - 群公告查询（list_active_group_announcements）
// - ActiveAnnouncement 结构体与墓碑折叠逻辑

/// 当前生效的群公告（每群一条）：会话列表 📢 标记的数据源。
///
/// 前端不能从 `chat.messages` 折叠 —— 那是 ~4 个会话的 LRU 缓存，列表要覆盖**全部**群。
/// 这里一条 SQL 捞出全部公告与墓碑，按到达序折叠出每群当前生效的一条
/// （墓碑按 `ann_id == 公告的 msg_id` 命中，与 `ChatWindow.vue` 的折叠规则一致）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAnnouncement {
    pub group_id: String,
    /// 当前生效公告的 msg_id（= 删除时的 ann_id）。
    pub msg_id: String,
    pub text: String,
}

#[tauri::command(async)]
pub fn list_active_group_announcements(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ActiveAnnouncement>, String> {
    use std::collections::{HashMap, HashSet};
    let s = state.inner();
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = dbc
        .prepare(
            "SELECT conv_id, msg_id, seq, content FROM messages \
             WHERE kind IN ('announcement', 'announcement_delete') AND conv_id LIKE 'group:%' \
             ORDER BY seq ASC, msg_id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    // 按到达序处理：每群保留最后一条公告（= (seq, msg_id) 最大），墓碑进集合。
    let mut best: HashMap<String, (i64, String, String)> = HashMap::new();
    let mut tombstones: HashSet<(String, String)> = HashSet::new();
    for row in rows.flatten() {
        let (conv_id, msg_id, seq, content) = row;
        let group_id = conv_id.trim_start_matches("group:").to_string();
        if let Ok(p) = serde_json::from_str::<crate::protocol::AnnouncementPayload>(&content) {
            best.insert(group_id.clone(), (seq, msg_id, p.text));
        } else if let Ok(p) =
            serde_json::from_str::<crate::protocol::AnnouncementDeletePayload>(&content)
        {
            tombstones.insert((group_id, p.ann_id));
        }
    }
    let mut out: Vec<ActiveAnnouncement> = Vec::new();
    for (group_id, (_seq, msg_id, text)) in best {
        if tombstones.contains(&(group_id.clone(), msg_id.clone())) {
            continue;
        }
        out.push(ActiveAnnouncement {
            group_id,
            msg_id,
            text,
        });
    }
    Ok(out)
}

/// 公告长度上限：与群名（40）同档量级 —— 公告是置顶横幅里的一段短文本，
/// 不是长文（长文该发消息）。同时也是对广播体积的限制。
const MAX_ANNOUNCEMENT_LEN: usize = 500;

/// 置顶 / 取消置顶一条群消息。
///
/// 权限：任意群成员（可逆、低风险）。与「仅群主可改名」那类不可逆操作不同 ——
/// 置顶错了再取消即可，不必引入管理员角色。
#[tauri::command(async)]
pub async fn pin_group_message(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    target: String,
    pinned: bool,
) -> Result<MessageRecord, String> {
    if target.is_empty() {
        return Err("缺少目标消息".to_string());
    }
    let payload = crate::protocol::PinPayload {
        target: target.clone(),
        pinned,
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    // ⚠️ **必须把事件记录返回给前端**（与 `send_group_reaction` 同口径）。
    // 置顶在界面上的呈现是 `foldPinned(该会话全部消息)` 折叠出来的 ——
    // 事件不进前端 store，折叠就看不到它，界面要等重进会话重新拉全量才刷新。
    // 此前这里返回 `()`，前端拿到了也无从 enqueue。
    send_group_payload(state.inner(), &group_id, "pin", content).await
}

/// 撤回窗口：超过它就不再允许撤回。
///
/// **只在发送端强制**。接收端无法验证发送方的墙上时钟（`env.ts` 不参与排序也不可信），
/// 所以接收端接受任何来自作者本人的撤回 —— 这是产品规则，不是安全边界。
/// 真正不可伪造的是**作者身份**：信封被 Ed25519 签名，只有原作者能撤回自己的消息。
const RECALL_WINDOW_MS: i64 = 120_000;

/// 撤回一条自己发的群消息。
///
/// 权限：**仅原作者**。不做"群主撤他人" —— 那需要引入管理员角色，
/// 而没有中心权威就没有中心授权（本项目无服务器）。
#[tauri::command(async)]
pub async fn recall_group_message(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    target: String,
) -> Result<(), String> {
    let s = state.inner();
    let conv_id = format!("group:{group_id}");
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let Some((sender_id, _)) = db::get_message_preview_source(&dbc, &target) else {
            return Err("消息不存在".to_string());
        };
        if sender_id != s.device_id {
            return Err("只能撤回自己发送的消息".to_string());
        }
        // 时间窗**只在发送端强制**（见 RECALL_WINDOW_MS 的说明：接收端无法验证对方的时钟）。
        // 用本地记录的 ts 判断：这条消息是本机发出的，本地时钟对它有意义。
        let ts: i64 = dbc
            .query_row(
                "SELECT ts FROM messages WHERE msg_id = ?1",
                rusqlite::params![target],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if ts > 0 && db::now_ms() - ts > RECALL_WINDOW_MS {
            return Err("超过可撤回时间（2 分钟）".to_string());
        }
        if db::is_recalled(&dbc, &target) {
            return Ok(()); // 幂等：已撤回过就直接成功
        }
    }
    let payload = crate::protocol::RecallPayload {
        target: target.clone(),
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    // 先发事件（走与普通消息同一条可靠管道），成功后再物化本地 ——
    // 顺序反了会出现「本地显示已撤回、但对端根本没收到」。
    send_group_payload(s, &group_id, crate::protocol::KIND_RECALL, content).await?;
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let seq = db::get_clock(&dbc, &conv_id);
        db::insert_recall(&dbc, &conv_id, &target, &s.device_id, seq).ok();
        db::materialize_recall(&dbc, &target).ok();
    }
    let _ = s.app.emit("message-recalled", &target);
    Ok(())
}

/// 表情回应：对某条群消息添加/取消一个表情。
///
/// 它是一条**静默事件**（`kind = "reaction"`）：走与普通群消息完全相同的可靠管道
/// （E2EE + outbox + GroupAck + 幂等去重 + 离线补发），但接收端不计未读、不改预览、
/// 不弹通知 —— 否则「回个表情」会和发一条消息一样吵闹，正是这个功能要消除的噪音。
#[tauri::command(async)]
pub async fn send_group_reaction(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    target: String,
    emoji: String,
    add: bool,
) -> Result<MessageRecord, String> {
    if target.is_empty() {
        return Err("回应缺少目标消息".to_string());
    }
    // 只接受本应用已知的表情 token 形态（`[名字]`），避免把任意字符串当表情写进库里、
    // 也避免超长内容进入广播。
    if !crate::protocol::is_valid_emoji_token(&emoji) {
        return Err("不认识的表情".to_string());
    }
    let payload = crate::protocol::ReactionPayload { target, emoji, add };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    send_group_payload(state.inner(), &group_id, "reaction", content).await
}

#[tauri::command(async)]
pub async fn send_group_file(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    path: String,
    scope: Option<String>,
    todo_id: Option<String>,
) -> Result<String, String> {
    let s = state.inner();
    // `scope == "todo"` 表示这是待办描述图片：复用群文件传输管线把字节投递给全员，
    // 但不进聊天时间线、不弹气泡（见下方气泡块的条件跳过）。缺省为普通群文件。
    let scope = scope.unwrap_or_else(|| "chat".to_string());
    let todo_id = todo_id.unwrap_or_default();

    // 文件校验：存在 + 普通文件；size/name 取自本地 metadata，不进入协议
    let p = std::path::PathBuf::from(&path);
    let meta = std::fs::metadata(&p).map_err(|e| format!("文件不存在或不可读：{e}"))?;
    if !meta.is_file() {
        return Err("只能发送普通文件".to_string());
    }
    let size = meta.len();
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    // 文件级 SHA-256：256KB 分块流式计算放阻塞线程池（不整读内存、不卡 async runtime）
    let p_sha = p.clone();
    let sha256 = tokio::task::spawn_blocking(move || file::sha256_file_hex(&p_sha))
        .await
        .map_err(|e| e.to_string())??;

    // 发起者必须是群成员（本地群存在）
    let members: Vec<String> = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let group = db::get_group(&dbc, &group_id).ok_or("群不存在")?;
        if !group.members.contains(&s.device_id) {
            return Err("你不是该群成员".to_string());
        }
        // 成员快照：创建时当前群成员（不含自己），作为 recipient 集合
        group
            .members
            .into_iter()
            .filter(|m| m != &s.device_id)
            .collect()
    };
    if members.is_empty() {
        // 待办描述图片：群里只有自己时**没有要投递的人**，但这不是错误 —— 本机的字节已经
        // 在原路径上，把内容登记进 content store（缩略图按 sha256 找回）就够了。
        // 普通群文件维持报错（用户发文件本来就是为了给别人）。
        if scope == "todo" {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = crate::content::store::record_local(
                &dbc,
                &sha256,
                &s.device_id,
                Some(&group_id),
                &name,
                size,
                crate::content::model::Direction::Send,
                &path,
                db::now_ms(),
            );
            return Ok(Uuid::new_v4().to_string());
        }
        return Err("群内没有其他成员".to_string());
    }

    let transfer_id = Uuid::new_v4().to_string();

    // 事务：group_files + 全部 recipient 行一次写入，避免半完成状态
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let gf = GroupFile {
            transfer_id: transfer_id.clone(),
            group_id: group_id.clone(),
            sender_id: s.device_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            status: "pending".to_string(),
            created_at: db::now_ms(),
            scope: scope.clone(),
            todo_id: todo_id.clone(),
        };
        let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
        db::insert_group_file(&tx, &gf).map_err(|e| e.to_string())?;
        for m in &members {
            db::insert_group_file_recipient(&tx, &transfer_id, m).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        // 待办图片：发送方把自己也登记为内容种子（peer_id = 自己 + **原文件路径**），
        // 这样本机按 sha256 就能解析到原文件渲染缩略图（与接收方口径一致），
        // `read_content_preview` 也据此放行“自己的内容”。仅 todo —— 不改动普通群文件既有行为。
        if scope == "todo" {
            let _ = crate::content::store::record_local(
                &dbc,
                &sha256,
                &s.device_id,
                Some(&group_id),
                &name,
                size,
                crate::content::model::Direction::Send,
                &path,
                db::now_ms(),
            );
        }
    }

    // 群密钥获取与 file_key 封装先于运行态写入：任何失败都不残留内存状态
    let group_key = get_group_key(s, &group_id).await.ok_or("群密钥缺失")?;

    // 随机 file session key：一个 transfer 只生成一次（CSPRNG，仅内存）
    let file_key = crypto::random_key();
    let sealed_file_key =
        STANDARD.encode(crypto::seal_symmetric(&group_key, &file_key).ok_or("封装文件密钥失败")?);
    s.group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(transfer_id.clone(), file_key);
    // 密封 file_key 持久化（群密钥封装，非明文）：重启后离线 pending
    // 群文件的投递仍能恢复 file_key（群密钥 gk:% 本身保留）
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, &format!("gfk:{transfer_id}"), &sealed_file_key).ok();
    }

    // 发送者本地气泡先落库：无论成员当前是否在线，用户看到的都是「发送中/待投递」，
    // 而不是一个报错后又偷偷排队的隐藏任务。
    // 图片文件保持 kind="image"，预览摘要为 [图片]，其余走 kind="file"。
    // `scope == "todo"` 时跳过：待办图片是任务的一部分，不该在聊天时间线里另起一条文件消息。
    if scope != "todo" {
        let subtype = file::classify_file_subtype(&name);
        let kind = if subtype == "image" { "image" } else { "file" };
        let content =
        serde_json::json!({ "name": name, "path": path, "size": size, "sha256": sha256, "subtype": subtype })
            .to_string();
        let conv_id = format!("group:{group_id}");
        let seq = {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::next_clock(&dbc, &conv_id).unwrap_or(1)
        };
        let rec = MessageRecord {
            id: 0,
            msg_id: format!("gfile-{transfer_id}"),
            conv_id,
            sender_id: s.device_id.clone(),
            receiver_id: group_id.clone(),
            kind: kind.to_string(),
            content,
            ts: db::now_ms(),
            seq,
            status: "sending".to_string(),
        };
        {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            db::insert_message(&dbc, &rec).ok();
            db::upsert_transfer(
                &dbc,
                &transfer_id,
                &group_id,
                &name,
                size,
                "send",
                "active",
                Some(p.to_string_lossy().as_ref()),
                0.0,
            )
            .ok();
            let group_name = db::get_group(&dbc, &group_id)
                .map(|g| g.name)
                .unwrap_or_default();
            let preview = if kind == "image" {
                "[图片]".to_string()
            } else {
                format!("[群文件] {name}")
            };
            db::touch_conversation(
                &dbc,
                &format!("group:{group_id}"),
                "group",
                &group_name,
                None,
                &preview,
                0,
            )
            .ok();
        }
        let _ = s.app.emit("message-received", &rec);
    }

    // 可达成员：有 TCP link 且 peers 信息完整；其余保持 pending，由上线事件自动投递。
    let mut reachable: Vec<String> = Vec::new();
    for m in &members {
        if s.has_link(m).await && resolve_member_x25519(s, m).is_some() {
            reachable.push(m.clone());
        }
    }

    // 进度条分母快照：发送那一刻在线的成员（用户口径见 `AppState::group_file_online_targets`）。
    // 冻结在这里的理由：离线成员之后上线补发时**不得**回退进度条（用户明确要求），
    // 动态算分母会让他一上线就把进度条往回拉。
    {
        let mut snap = s
            .group_file_online_targets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        snap.insert(transfer_id.clone(), reachable.iter().cloned().collect());
    }

    // 逐可达成员发送 Offer
    for m in &reachable {
        let msg = Message::GroupFileOffer {
            transfer_id: transfer_id.clone(),
            group_id: group_id.clone(),
            sender_id: s.device_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            sealed_file_key: sealed_file_key.clone(),
            scope: scope.clone(),
            todo_id: todo_id.clone(),
        };
        if try_send(s, m, &msg).await.is_ok() {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db::update_group_file_recipient(&dbc, &transfer_id, m, "sending", 0.0);
        }
    }

    // 逐可达成员并行投递（每个 recipient 独立任务：Offer → Chunk → Done）。
    // 单个 recipient 失败只标它 failed，不阻塞其他；离线成员保持 pending，
    // 由上线事件（flush_pending_group_files）自动投递。
    let s2 = s.clone();
    let tid = transfer_id.clone();
    let gid = group_id.clone();
    let src = p.clone();
    for m in &reachable {
        let s3 = s2.clone();
        let tid3 = tid.clone();
        let gid3 = gid.clone();
        let src3 = src.clone();
        let m3 = m.clone();
        tokio::spawn(async move {
            if let Err(e) =
                dispatch_group_file_to_peer(&s3, &tid3, &gid3, &m3, src3.to_string_lossy().as_ref())
                    .await
            {
                app_handle_log(
                    &s3,
                    &format!("group-file dispatch {tid3} -> {m3} failed: {e}"),
                );
            }
        });
    }

    Ok(transfer_id)
}

/// 发送一张**待办描述图片**：复用群文件传输管线把字节投递给全体群成员，
/// 但 `scope="todo"` ⇒ 不进聊天时间线、不弹气泡、不改会话预览（见 `send_group_file`）。
///
/// 真实字节落盘到本机下载目录并经 content store 按 `sha256` 登记；待办载荷只带图片**元数据**
/// （`TodoImage{id=sha256, name, size, sha256, subtype}`），渲染时按 `sha256` 解析本地路径，
/// 缺失则成员上线后自动补取。这样既全端同步、又不让描述图片污染聊天流。
#[tauri::command(async)]
pub async fn send_todo_image(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    todo_id: String,
    path: String,
) -> Result<String, String> {
    send_group_file(
        state,
        group_id,
        path,
        Some("todo".to_string()),
        Some(todo_id),
    )
    .await
}
