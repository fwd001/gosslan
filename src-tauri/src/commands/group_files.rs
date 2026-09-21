// 职责边界：
// - 群消息发送内核（send_group_payload，文本/代码/合并共用）
// - 群 Todo/投票/公告 命令入口
// - ⚠️ 只放 command entry + 通用 helper，投递/密钥在独立子模块
// ---------------- 群文件（Offer / session-key 阶段） ----------------

// 发起群文件（本阶段只建立 Offer 与 file session key，不含分片传输）。
//
// 流程：校验发起者是群成员 → 实时读取当前成员快照 → 事务内创建
// group_files + 全部 recipient 行（避免半完成状态）→ 生成随机 file_key
// 存内存 → 对可达成员发送 GroupFileOffer（群密钥封装 file_key）→
// 流式读取文件、逐 256KB 分片 AEAD 加密后向全部可达 recipient 发送
// GroupFileChunk（seq 从 0 严格递增）。不可达成员保持 pending。

/// 发群消息（文本 / 代码）。校验与长度限制留在这一层，内核只管发送。
#[tauri::command(async)]
pub async fn send_group_message(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    content: String,
    kind: String,
) -> Result<MessageRecord, String> {
    let wire_kind = match kind.as_str() {
        "text" => "text",
        "code" => "code",
        // 群聊里的合并转发（与单聊同一套载荷与校验）
        "merge" => {
            crate::protocol::parse_merge_payload(&content)?;
            "merge"
        }
        _ => return Err("群聊不支持该消息类型".to_string()),
    };
    let content = check_message_content(content)?;
    send_group_payload(state.inner(), &group_id, wire_kind, content).await
}

/// 群任务标题上限：它是卡片上的一行标题，不是长文。
const MAX_TODO_TITLE_LEN: usize = 200;
/// 群任务描述上限：长文本说明，但仍要受控（避免单条任务撑爆事件日志）。
const MAX_TODO_DESC_LEN: usize = 4000;
/// 投票选项数上限（下标要能塞进 u32 且 UI 排得下）。
const MAX_POLL_OPTIONS: usize = 10;

/// 创建一条群任务（任意成员）。
///
/// `todo_id` 用随机 id（`todo-<uuid>`）而不是"创建事件的 msg_id"：后续每一次改状态/改标题
/// 都是**重新发一份定义**，它们必须引用同一个键，而 msg_id 每次都会变。
#[tauri::command(async)]
pub async fn send_group_todo(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    title: String,
    assignees: Vec<String>,
    description: Option<String>,
    images: Option<Vec<crate::protocol::TodoImage>>,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("任务标题不能为空".to_string());
    }
    if title.chars().count() > MAX_TODO_TITLE_LEN {
        return Err(format!("任务标题不能超过 {MAX_TODO_TITLE_LEN} 字"));
    }
    let description = description.unwrap_or_default().trim().to_string();
    if description.chars().count() > MAX_TODO_DESC_LEN {
        return Err(format!("任务描述不能超过 {MAX_TODO_DESC_LEN} 字"));
    }
    let images = images.unwrap_or_default();
    check_todo_assignees(s, &group_id, &assignees)?;
    let payload = crate::protocol::TodoPayload {
        todo_id: format!("todo-{}", Uuid::new_v4()),
        title,
        assignees,
        status: crate::protocol::default_todo_status(),
        creator: s.device_id.clone(),
        deleted: false,
        description,
        images,
        archived: false,
        done_at: None,
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    send_group_payload(s, &group_id, "todo", content).await
}

/// 被指派人必须**至少一个且都是群成员**（用户 2026-09-16：「每个任务可以给一个或多个人」）。
///
/// 为什么在命令层拦：指派人同时是**改状态的鉴权依据**（见 `may_update_todo`）——
/// 放进一个非成员会让这条任务对谁都"改不了状态"（谁都不是被指派人），而群里也没人认识它。
fn check_todo_assignees(s: &AppState, group_id: &str, assignees: &[String]) -> Result<(), String> {
    if assignees.is_empty() {
        return Err("请至少指派一名成员".to_string());
    }
    let members = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, group_id)
            .map(|g| g.members)
            .ok_or_else(|| "群不存在".to_string())?
    };
    // ⚠️ `resolve_nickname` 内部会再锁一次 db，必须在放锁之后调用（见文件里那条同名注释）
    if let Some(bad) = assignees.iter().find(|a| !members.contains(a)) {
        return Err(format!("{} 不是群成员", resolve_nickname(s, bad)));
    }
    Ok(())
}

/// 取某个任务在**本机消息库**里的最新定义（LWW：`(seq desc, msg_id desc)`）。
///
/// 为什么后端也要做这一步：命令层要判权就得知道这条任务的 `creator` 与 `assignees`，
/// 而它们只存在于事件日志里（这些特性没有专表，见 ADR-0018）。这里只取"最新一条定义"
/// 这一件事、不做完整折叠；**LWW 规则必须与前端 `src/utils/todos.ts` 的 `newer()` 一致**
/// （`(seq, msg_id)` 元组比较，同 seq 时按 msg_id 字符串比）。
///
/// 已删除（墓碑）的定义照样返回：改/删的鉴权同样需要它的 creator。
fn latest_todo_def(
    conn: &rusqlite::Connection,
    conv_id: &str,
    todo_id: &str,
) -> Option<crate::protocol::TodoPayload> {
    let mut stmt = conn
        .prepare(
            // 两种 kind 都要看：创建是 `todo`、后续每次改动是 `todo_update`，
            // 它们同属一条 LWW 序列（载荷同构）。
            "SELECT content FROM messages WHERE conv_id = ?1 AND kind IN ('todo', 'todo_update') \
             ORDER BY seq DESC, msg_id DESC",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![conv_id], |r| r.get::<_, String>(0))
        .ok()?;
    for row in rows.flatten() {
        if let Ok(p) = serde_json::from_str::<crate::protocol::TodoPayload>(&row) {
            if p.todo_id == todo_id {
                return Some(p);
            }
        }
    }
    None
}

/// 谁能改一条任务 —— **纯函数**，便于单测。
///
/// 档位只有两档，判据是"这次改动**是不是**结构改动"（结构 = 改标题 / 删除，
/// 见调用点的 `edits_structure`）：
///
/// | 改动 | 允许谁 |
/// |---|---|
/// | 改标题 / 删除（结构） | 创建者 **或** 群主 |
/// | 其余（描述 / 图片 / 指派人 / 状态 / 归档） | 创建者 **或** 群主 **或** 当前被指派人 |
///
/// 只需这一个输入 ⇒ 参数里没有 `edits_assignees`（v4.23.1 删的）：放宽群主权限之后，
/// "改指派人"与"只改状态"落在同一档，那个入参一次都没被读过，而表上还在单独讲它
/// ⇒ 说明与实现各说一份。
///
/// 为什么被指派人能改指派人（用户 2026-09-17）：被 @ 的人也该能把自己手上的活转派 / 加人 /
/// 补描述与截图，否则「创建者请假了、指派的人干不了」就成了死结。但标题与删除是
/// **任务归属**，仍只归创建者或群主 —— 所以判定必须**先问是不是结构改动**，否则
/// 「同时改指派人 + 标题」会被被指派人那一档一并放行。
///
/// 为什么群主「什么都能改」（用户 2026-09-20：「群主不能编辑群任务」）：此前群主只被允许
/// 改标题 / 删除 / 改指派人，而**描述 / 图片 / 状态**落进了「被指派人」那一档 ——
/// 群主在编辑表单里改了描述或状态、点保存，就会收到
/// 「只有创建者或被指派人可以修改任务状态」。群主是群的管理者，成员创建的任务它也该能编辑。
fn may_update_todo(
    def: &crate::protocol::TodoPayload,
    actor: &str,
    group_creator: &str,
    edits_structure: bool,
) -> bool {
    if def.creator == actor || group_creator == actor {
        return true;
    }
    // 被指派人：能改指派人 / 状态（两者判定相同），但不能改结构（标题 / 描述 / 图片 / 删除）。
    if edits_structure {
        return false;
    }
    def.assignees.iter().any(|a| a == actor)
}

/// 完成态与归档字段的**权威推导**（纯函数，便于单测）。
///
/// 口径（用户 2026-09-17：「完成以后手动归档」）：
/// - **非完成态** ⇒ 一律 `(archived=false, done_at=None)` —— "重新打开"就等于把它从归档里拿回来；
/// - **完成态** ⇒ `done_at` **沿用原值**（只在首次完成时记 `now`），否则用户改个标题就把
///   7 天自动归档的计时重置了；`archived` 只由**显式请求**决定（`None` = 保留原值）。
///
/// 两处都刻意**不接受客户端自报 `done_at`** ⇒ 无法伪造"完成时间"骗过自动归档；
/// 也刻意不允许"非完成却归档" ⇒ 否则一条进行中的任务会从活动列表里消失。
fn resolve_done_archive(
    is_done: bool,
    requested_archived: Option<bool>,
    prev_archived: bool,
    prev_done_at: Option<i64>,
    now: i64,
) -> (bool, Option<i64>) {
    if !is_done {
        return (false, None);
    }
    (
        requested_archived.unwrap_or(prev_archived),
        prev_done_at.or(Some(now)),
    )
}

/// 更新一条群任务：改状态 / 改标题与指派人 / 改描述与图片 / 归档 / 删除。
///
/// 为什么多件事合成一个命令：它们都是"重新发一份定义"（LWW per `todo_id`），
/// 构造与校验几乎相同，只有鉴权口径不同（见 [`may_update_todo`]）——
/// 拆成多个命令就是多份重复的构造/校验代码。
///
/// ⚠️ `creator` **不从参数来**：由服务端从库里最新定义回填。否则任何人传一个别人的
/// creator 就能改别人的任务（而 creator 正是鉴权依据）。
/// ⚠️ `done_at` **不从参数来**：`status=="done"` 时由服务端在**首次**完成时记权威时间戳
/// （重复保存沿用原值，免得改个标题就把 7 天计时重置）；否则客户端伪造「完成时间」就能骗过自动归档。
/// `archived` 是**显式意图**（`Some(true)` = 手动归档）：完成不再自动归档（用户 2026-09-17）。
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub async fn update_group_todo(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    todo_id: String,
    title: String,
    assignees: Vec<String>,
    status: String,
    deleted: bool,
    description: Option<String>,
    images: Option<Vec<crate::protocol::TodoImage>>,
    archived: Option<bool>,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let title = title.trim().to_string();
    if !deleted {
        if title.is_empty() {
            return Err("任务标题不能为空".to_string());
        }
        if title.chars().count() > MAX_TODO_TITLE_LEN {
            return Err(format!("任务标题不能超过 {MAX_TODO_TITLE_LEN} 字"));
        }
        if !crate::protocol::todo_status_is_valid(&status) {
            return Err("任务状态不合法".to_string());
        }
        check_todo_assignees(s, &group_id, &assignees)?;
    }
    let (def, group_creator) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let def = latest_todo_def(&dbc, &format!("group:{group_id}"), &todo_id)
            .ok_or_else(|| "任务不存在".to_string())?;
        let creator = db::get_group(&dbc, &group_id)
            .map(|g| g.creator)
            .ok_or_else(|| "群不存在".to_string())?;
        (def, creator)
    };
    // `None` ⇒ 保留库里原值（不让"只改状态"的请求把描述/图片清空）；
    // `Some(x)` ⇒ 用传来的（显式传空串即表示清空）。
    let description = match description {
        Some(d) => d.trim().to_string(),
        None => def.description.clone(),
    };
    let images = match images {
        Some(imgs) => imgs,
        None => def.images.clone(),
    };
    if !deleted && description.chars().count() > MAX_TODO_DESC_LEN {
        return Err(format!("任务描述不能超过 {MAX_TODO_DESC_LEN} 字"));
    }
    // 两档判权（见 `may_update_todo` 上方那张表）：
    //   · 结构（改标题 / 删除）：仅创建者或群主
    //   · 其余（描述 / 图片 / 指派人 / 状态 / 归档）：创建者 / 群主 / 当前被指派人
    // `edits_assignees` 在这里**只用来挑错误文案**（同一档里三种角色各自的提示不同），
    // 不参与判权 —— 判权只需要"是不是结构改动"这一个输入。
    let edits_assignees = assignees != def.assignees;
    let edits_structure = deleted || title != def.title;
    if !may_update_todo(&def, &s.device_id, &group_creator, edits_structure) {
        return Err(if edits_assignees {
            "只有任务创建者、群主或被指派人可以修改指派人".to_string()
        } else if edits_structure {
            "只有任务创建者或群主可以修改任务".to_string()
        } else {
            "只有创建者或被指派人可以修改任务状态".to_string()
        });
    }
    let (archived, done_at) = resolve_done_archive(
        status == "done",
        archived,
        def.archived,
        def.done_at,
        crate::db::now_ms(),
    );
    let payload = crate::protocol::TodoPayload {
        todo_id,
        title,
        assignees,
        status,
        creator: def.creator,
        deleted,
        description,
        images,
        archived,
        done_at,
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    // ⚠️ 发 `todo_update`（Silent）而不是 `todo`（Card）：改状态/改标题/删除是**状态微调**，
    // 不该给全群记未读、弹通知（创建才该）。两者载荷同构、折叠也是同一条 LWW 规则，
    // 区别只在通知口径 —— 详见 `protocol.rs` 的 `WIRE_KINDS` 注释。
    send_group_payload(s, &group_id, "todo_update", content).await
}

/// 发起投票（任意成员）。
#[tauri::command(async)]
pub async fn send_group_poll(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    question: String,
    options: Vec<String>,
    multi: bool,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let question = question.trim().to_string();
    let options: Vec<String> = options
        .into_iter()
        .map(|o| o.trim().to_string())
        .filter(|o| !o.is_empty())
        .collect();
    if question.is_empty() {
        return Err("投票主题不能为空".to_string());
    }
    if options.len() < 2 {
        return Err("至少需要两个选项".to_string());
    }
    if options.len() > MAX_POLL_OPTIONS {
        return Err(format!("最多 {MAX_POLL_OPTIONS} 个选项"));
    }
    let payload = crate::protocol::PollPayload {
        poll_id: format!("poll-{}", Uuid::new_v4()),
        question,
        options,
        multi,
        closed: false,
        creator: s.device_id.clone(),
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    send_group_payload(s, &group_id, "poll", content).await
}

/// 投票 / 改票 / 撤票（任意成员；每人只写自己那一格）。
/// 撤票就是传空的 `choices`。
#[tauri::command(async)]
pub async fn cast_group_poll_vote(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    poll_id: String,
    choices: Vec<u32>,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    if poll_id.is_empty() {
        return Err("缺少投票标识".to_string());
    }
    let content = serde_json::to_string(&crate::protocol::PollVotePayload { poll_id, choices })
        .map_err(|e| e.to_string())?;
    send_group_payload(s, &group_id, "poll_vote", content).await
}

/// 发布群公告（**仅群主**）。
///
/// 权限口径与 `handle_group_rename` 逐字同构（`group.creator == 我`）：
/// 公告是发给全群的**权威信息**，人人可发就失去了"公告"的意义。
/// 群主离线时发不了 —— 无中心即无中心授权，不做"降级为任何人可发"。
#[tauri::command(async)]
pub async fn send_group_announcement(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    text: String,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("公告内容不能为空".to_string());
    }
    if text.chars().count() > MAX_ANNOUNCEMENT_LEN {
        return Err(format!("公告不能超过 {MAX_ANNOUNCEMENT_LEN} 字"));
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let g = db::get_group(&dbc, &group_id).ok_or("群不存在")?;
        if g.creator != s.device_id {
            return Err("只有群主可以发布公告".to_string());
        }
    }
    let content = serde_json::to_string(&crate::protocol::AnnouncementPayload { text })
        .map_err(|e| e.to_string())?;
    send_group_payload(s, &group_id, "announcement", content).await
}

/// 删除一条群公告（仅群主）：发 `announcement_delete` **墓碑**（Silent），全端据此把横幅折掉。
///
/// 为什么是墓碑而不是删行：公告与其它群消息同走一条 LWW/折叠管线 —— 接收端
/// （`transport.rs` 对该 kind 强制 owner-only）按 `ann_id == 公告的 msg_id` 折叠，
/// 与 `ChatWindow.vue` 的折叠规则一致；本端由前端自己的折叠逻辑收敛。
#[tauri::command(async)]
pub async fn delete_group_announcement(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    ann_id: String,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    let ann_id = ann_id.trim().to_string();
    if ann_id.is_empty() {
        return Err("公告标识缺失".to_string());
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let g = db::get_group(&dbc, &group_id).ok_or("群不存在")?;
        if g.creator != s.device_id {
            return Err("只有群主可以删除公告".to_string());
        }
    }
    let content = serde_json::to_string(&crate::protocol::AnnouncementDeletePayload { ann_id })
        .map_err(|e| e.to_string())?;
    send_group_payload(s, &group_id, "announcement_delete", content).await
}

// ---------------- 群公告 ----------------
include!("group_announcements.rs");

// ---------------- Todo 图片 & 进度 ----------------
include!("group_todo_media.rs");

// ---------------- 群文件投递核心 ----------------
include!("group_file_dispatch.rs");

// ---------------- 密钥恢复 ----------------
include!("group_file_keys.rs");
