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
///
/// `mentions` = 这条**文本** @ 了哪些人（设备 id），随载荷带给全部成员，接收端据此判
/// 「有人@我」。它是三态的：缺省（旧前端 / 非选择器入口）= 不发这个键 = 接收端按昵称兜底；
/// 空数组 = 明确回答"谁都没 @"。为什么这一份**不落库**（而 #122 的落点那一份落），
/// 见 `state::MessageRecord` 的 `mention_targets` 字段文档；为什么上限 64，见 `protocol::gossip_plaintext`。

#[tauri::command(async)]
pub async fn send_group_message(
    state: State<'_, Arc<AppState>>,
    group_id: String,
    content: String,
    kind: String,
    mentions: Option<Vec<String>>,
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
    // 只有文本带 @ 名单：code / merge 里的 `@名字` 不算点名，与前端 `messageMentionsMe`
    // 的 kind 判据同口径 —— 这一层多带一份，接收端也只是丢掉。
    let mentions = if wire_kind == "text" {
        mentions.as_deref()
    } else {
        None
    };
    send_group_payload(state.inner(), &group_id, wire_kind, content, mentions).await
}

/// 群任务标题上限：它是卡片上的一行标题，不是长文。
const MAX_TODO_TITLE_LEN: usize = 200;
/// 群任务描述上限：长文本说明，但仍要受控（避免单条任务撑爆事件日志）。
const MAX_TODO_DESC_LEN: usize = 4000;

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
    category: Option<String>,
) -> Result<MessageRecord, String> {
    let s = state.inner();
    // 类型只在**给了值**时校验；没给就是缺省那一档（旧前端不传这一格也要能建任务）。
    // 非法值明确拒绝、不静默回落：静默回落会让"界面给出一个后端不认的值"变成一条悄悄存成
    // 普通任务的需求，而用户看不出自己选错了。
    let category = match category {
        Some(c) => {
            if !crate::protocol::todo_category_is_valid(&c) {
                return Err("任务类型不合法".to_string());
            }
            c
        }
        None => crate::protocol::default_todo_category(),
    };
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
    // 编号在**建这一条**时分配一次（锁只在取号期间持有：发出去的那条路径还要再锁一次 db）。
    let number = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::next_todo_number(&dbc, &group_id).map_err(|e| e.to_string())?
    };
    let payload = crate::protocol::TodoPayload {
        todo_id: format!("todo-{}", Uuid::new_v4()),
        number,
        title,
        assignees,
        status: crate::protocol::default_todo_status(),
        creator: s.device_id.clone(),
        priority: crate::protocol::default_todo_priority(),
        category,
        deleted: false,
        description,
        images,
        archived: false,
        done_at: None,
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let todo_id = payload.todo_id.clone();
    let rec = send_group_payload(s, &group_id, "todo", content, None).await?;
    // §29 可观测性：任务这条链以前**一行日志都不打**（`commands` 目录 29 处 logger 无一与任务有关），
    // 于是"任务没同步过去"只能靠比对两侧的库来定位。这里补上起点：只记 id 与 kind，
    // **不记标题/描述**（那是用户内容，与"token 值不得入日志"同一族约束）。
    s.logger.info(
        "group",
        format!("群任务已建 task={todo_id} group={group_id} msg={}", rec.msg_id),
    );
    Ok(rec)
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
/// ★ 还有一条必须与前端 `carryAbsentTodoFields` 一致的规矩（两份实现、改动要同批改两边）：
/// `priority` / `category` / `number` 这三格，**最新那一份载荷没说**（键不存在，或值本端
/// 读不出合法档位）时，往更旧的几份里找最近说过的值**沿用**；`number` 再优先认创建那一条
/// （`kind = 'todo'`）—— 它是创建那一刻定死的绝对坐标。
/// 不这么做的真实后果：`4.31.14` 之前的对端结构体里根本没有这三格，它改一次状态就把
/// 类型抹回「普通任务」、优先级抹回「常规」、编号抹成 0；而这里返回的 def 还是**判权的输入**
/// （三条窄道逐字比对类型与优先级）⇒ 抹平之后连"谁能改这条任务"都跟着错。
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
            "SELECT kind, content FROM messages WHERE conv_id = ?1 AND kind IN ('todo', 'todo_update') \
             ORDER BY seq DESC, msg_id DESC",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![conv_id], |r| {
            Ok::<_, rusqlite::Error>((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .ok()?;
    let mut latest: Option<crate::protocol::TodoPayload> = None;
    // 这三格的"最近说过的值"：从新到旧扫，第一个说到的（可能就是最新那一份）胜出。
    let mut carry_priority: Option<String> = None;
    let mut carry_category: Option<String> = None;
    let mut carry_number: Option<i64> = None;
    // 创建那一条说过的号：绝对坐标，压过任何一份更新里的号。DESC 扫到最后一刻 = 最旧那条创建。
    let mut created_number: Option<i64> = None;
    for row in rows.flatten() {
        let (kind, content) = row;
        // 先按 Value 读**键在不在**：`TodoPayload` 的 `serde(default)` 会把"没带"抹成默认值，
        // 而那正是这条规矩要区分的两种情况。
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        let stated_priority = v
            .get("priority")
            .and_then(|x| x.as_str())
            .filter(|s| crate::protocol::todo_priority_is_valid(s))
            .map(str::to_string);
        let stated_category = v
            .get("category")
            .and_then(|x| x.as_str())
            .filter(|s| crate::protocol::todo_category_is_valid(s))
            .map(str::to_string);
        let stated_number = v.get("number").and_then(|x| x.as_i64()).filter(|n| *n > 0);
        let Ok(p) = serde_json::from_value::<crate::protocol::TodoPayload>(v) else {
            continue;
        };
        if p.todo_id != todo_id {
            continue;
        }
        if latest.is_none() {
            latest = Some(p);
        }
        if carry_priority.is_none() {
            carry_priority = stated_priority;
        }
        if carry_category.is_none() {
            carry_category = stated_category;
        }
        if carry_number.is_none() {
            carry_number = stated_number;
        }
        if kind == "todo" && stated_number.is_some() {
            created_number = stated_number;
        }
    }
    let mut p = latest?;
    // "没说"沿用已知的；说了的已经在最新那份里了，这里赋回同一个值是恒等。
    if let Some(x) = carry_priority {
        p.priority = x;
    }
    if let Some(x) = carry_category {
        p.category = x;
    }
    if let Some(n) = created_number.or(carry_number) {
        p.number = n;
    }
    Some(p)
}

/// 谁能改一条任务 —— **纯函数**，便于单测。
///
/// 档位只有两档，判据是"这次改动**是不是**结构改动"（结构 = 改标题 / 删除，
/// 见调用点的 `edits_structure`）：
///
/// | 改动 | 允许谁 |
/// |---|---|
/// | 改标题 / 删除（结构） | 创建者 **或** 群主 |
/// | 其余（描述 / 图片 / 指派人 / 状态） | 创建者 **或** 群主 **或** 当前被指派人 |
///
/// ⚠️ 「归档」与「还原」不在这两档里 —— 它们各有一条只对**群成员**开放的窄档
/// （[`may_change_todo`]），因为"把干完的活收起来 / 再拎回来"是每个成员都在做的整理动作。
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

/// 两条图片元数据列表是否**逐字段相同**（`TodoImage` 没有 `PartialEq`，在此就地比）。
fn same_images(
    a: &[crate::protocol::TodoImage],
    b: &[crate::protocol::TodoImage],
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.id == y.id
                && x.name == y.name
                && x.size == y.size
                && x.sha256 == y.sha256
                && x.subtype == y.subtype
        })
}

/// 一次改动请求里需要**与库里最新定义逐字段比对**的那些字段。
///
/// 为什么打包成一个入参：这两条窄档的判据都是"除了一位别的都不许动"，
/// 比对的字段一共八个；按位置参数摊开就是十个入参（读调用点时看不出谁是谁），
/// 抽成结构体之后"窄"这件事本身就是类型说的。
struct TodoEdit<'a> {
    deleted: bool,
    title: &'a str,
    status: &'a str,
    assignees: &'a [String],
    description: &'a str,
    images: &'a [crate::protocol::TodoImage],
    archived: Option<bool>,
    priority: &'a str,
    category: &'a str,
}

/// 本次请求是否**只动归档位**：其余字段与库里最新定义逐字相同，且归档值确实翻转了。
///
/// 这条判据是成员窄档的入口之一，所以宁可写得死板：「改标题顺带归档」「改状态顺带归档」
/// 都不算 —— 否则任何人拿一条归档请求就能顺着这条口子改到不该改的字段。
/// `Some(!def.archived)` 同时钉住了"确实是一次改动"（传了相同值 = 没改，不进这一档）。
fn archive_only_change(def: &crate::protocol::TodoPayload, e: &TodoEdit<'_>) -> bool {
    !e.deleted
        && e.archived == Some(!def.archived)
        && e.title == def.title
        && e.status == def.status
        && e.assignees == def.assignees
        && e.description == def.description
        && same_images(e.images, &def.images)
        // 只改优先级不算"只动归档位"：那条窄档对整个群都放行，优先级不该顺着它进去
        && e.priority == def.priority
        // 类型同理：需求改成缺陷、缺陷改成普通任务，都是**内容改动**不是归档
        && e.category == def.category
}

/// 本次请求是否**只是"还原"**：库里状态是「完成」，请求把它退回「待办」，其余字段逐字相同。
///
/// 为什么"还原"要看状态而不是看归档位：`resolve_done_archive` 对**非完成态**一律写回
/// `archived=false, done_at=None` ⇒ 客户端只要把状态退回待办，归档位与完成时间自动就被清了。
/// 所以"还原"这件事在后端只有**一个**入参可变，正好能像归档那样判得死板。
/// ⚠️ 方向是单向的（只允许 `done → todo`）：反方向「把待办直接改成完成」是真正的
/// 完成动作，仍归创建者 / 群主 / 被指派人 —— 否则任何人都能替别人宣布干完了。
fn reopen_only_change(def: &crate::protocol::TodoPayload, e: &TodoEdit<'_>) -> bool {
    !e.deleted
        && def.status == "done"
        && e.status == "todo"
        && e.title == def.title
        && e.assignees == def.assignees
        && e.description == def.description
        && same_images(e.images, &def.images)
        // ⚠️ 这两格以前**没比**：那条写归档窄档的理由（"对整个群放行的口子不该让人顺着改内容"）
        // 对还原这一档同样成立 —— 漏比的后果是任何成员都能拿一次 `done → todo` 顺带把优先级
        // 或类型改掉，而界面上只看得到"有人把它还原了"。补成与归档那一档同一口径。
        && e.priority == def.priority
        && e.category == def.category
}

/// 请求的指派人名单是否正好等于「原名单 + 末尾追加 actor」这一个改动。
///
/// 写成形状判定而不是"包含 actor 就行"：这条窄档对整个群放行，任何多出来的位
/// （踢掉别人、加进别人、换顺序）都必须落回创建者/群主那一档。
fn assignees_are_def_plus_actor(def: &[String], req: &[String], actor: &str) -> bool {
    req.len() == def.len() + 1
        && req.last().map(|x| x == actor).unwrap_or(false)
        && !def.iter().any(|a| a == actor)
        && req[..req.len() - 1] == *def
}

/// 本次请求是否**只是"认领一条需求"**：库里类型是 `requirement`、actor 原本不在名单里，
/// 请求恰好把他自己追加到末尾，其余字段逐字相同。
///
/// 为什么不新增 `claimed` 字段、也不新增权限位（用户那句「认领后他要有关联人的全部权限」）：
/// `assignees` **本来就是**改状态的判权依据 —— 把自己加进去，那些权限自动跟着来。
/// 多一个字段只会多一个可以和 `assignees` 吵架的状态。
/// ⚠️ 只开给「需求」：普通任务与缺陷仍要"被指派才有权限"，否则任何人都能把自己塞进
/// 别人的缺陷里再宣布它完成。
fn claim_only_change(def: &crate::protocol::TodoPayload, actor: &str, e: &TodoEdit<'_>) -> bool {
    !e.deleted
        && !def.archived
        && def.category == "requirement"
        && e.category == def.category
        && e.title == def.title
        && e.status == def.status
        && e.description == def.description
        && e.priority == def.priority
        // 认领不许顺带归档（那是归档那一档的事）
        && e.archived.is_none()
        && same_images(e.images, &def.images)
        && assignees_are_def_plus_actor(&def.assignees, e.assignees, actor)
}

/// 「成员窄档」：只服务**本群成员**的三条窄动作（归档 / 还原 / 认领一条需求）。
#[derive(Clone, Copy, Default)]
struct MemberLane {
    archive_only: bool,
    reopen_only: bool,
    claim_only: bool,
    is_member: bool,
}

impl MemberLane {
    fn allows(self) -> bool {
        self.is_member && (self.archive_only || self.reopen_only || self.claim_only)
    }
}

/// 命令层的总判权：[`may_update_todo`] 的两档，外加 [`MemberLane`] 那两条放宽档。
///
/// 归档与还原放宽给**全体群成员**（用户 2026-09-24：「群里所有人都可以归档」，
/// 同日追加：「归档和被归档的数据还原，任何人都可以操作，其他权限不变」）。
/// 三条前提：① 改动必须真的只落在那一位上（[`archive_only_change`] / [`reopen_only_change`]）；
/// ② 必须是本群成员（归档/还原是群内协作动作，不给外人）；③ 归档只在「完成」态成立，
/// 由调用点显式拒绝"归档一条未完成的任务"（见 `update_group_todo`）。
///
/// **其余一切**（改标题 / 描述 / 图片 / 指派人 / 状态 / 删除）仍然只走前两档 ——
/// 尤其"完成"这个动作没有放宽：成员能把干完的活收进归档、也能把它拎回来，
/// 但不能替别人宣布干完了。
fn may_change_todo(
    def: &crate::protocol::TodoPayload,
    actor: &str,
    group_creator: &str,
    edits_structure: bool,
    lane: MemberLane,
) -> bool {
    may_update_todo(def, actor, group_creator, edits_structure) || lane.allows()
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
    priority: Option<String>,
    category: Option<String>,
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
    // 优先级只在**给了值**时校验并覆盖；没给就沿用库里那份（编辑描述不该顺手把号改掉，
    // 同理编辑状态也不该把优先级重置成"常规"）。非法值明确拒绝，不静默吞。
    if let Some(p) = priority.as_deref() {
        if !crate::protocol::todo_priority_is_valid(p) {
            return Err("任务优先级不合法".to_string());
        }
    }
    // 类型与优先级同口径：只在给值时校验并覆盖，没给就沿用库里那份（改状态不该顺手改类型）。
    if let Some(c) = category.as_deref() {
        if !crate::protocol::todo_category_is_valid(c) {
            return Err("任务类型不合法".to_string());
        }
    }
    let (def, group_creator, members) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let def = latest_todo_def(&dbc, &format!("group:{group_id}"), &todo_id)
            .ok_or_else(|| "任务不存在".to_string())?;
        let g = db::get_group(&dbc, &group_id).ok_or_else(|| "群不存在".to_string())?;
        (def, g.creator, g.members)
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
    // 判权：`may_update_todo` 的两档，外加"只动归档位 / 只还原"这两条**成员窄档**
    // （用户 2026-09-24 与同日追加）。见 [`may_change_todo`]。
    // `edits_assignees` 在这里**只用来挑错误文案**（同一档里三种角色各自的提示不同），
    // 不参与判权 —— 判权只需要"是不是结构改动"与"这次改动落在哪条窄档"这两个输入。
    let priority = match priority {
        Some(p) => p,
        None => def.priority.clone(),
    };
    let category = match category {
        Some(c) => c,
        None => def.category.clone(),
    };
    let edits_assignees = assignees != def.assignees;
    let edits_structure = deleted || title != def.title;
    let edit = TodoEdit {
        deleted,
        title: &title,
        status: &status,
        assignees: &assignees,
        description: &description,
        images: &images,
        archived,
        priority: &priority,
        category: &category,
    };
    let lane = MemberLane {
        archive_only: archive_only_change(&def, &edit),
        reopen_only: reopen_only_change(&def, &edit),
        claim_only: claim_only_change(&def, &s.device_id, &edit),
        is_member: members.iter().any(|m| m == &s.device_id),
    };
    if !may_change_todo(
        &def,
        &s.device_id,
        &group_creator,
        edits_structure,
        lane,
    ) {
        return Err(if edits_assignees {
            "只有任务创建者、群主或被指派人可以修改指派人".to_string()
        } else if edits_structure {
            "只有任务创建者或群主可以修改任务".to_string()
        } else if lane.archive_only || lane.reopen_only || lane.claim_only {
            "只有群成员可以归档、还原或认领这条任务".to_string()
        } else {
            "只有创建者或被指派人可以修改任务状态".to_string()
        });
    }
    // 「归档」只在完成态成立（`resolve_done_archive` 对非完成态一律写回 archived=false）。
    // 原来这里是**静默丢弃**：调用方拿到一条成功的记录、任务却没进归档，正是
    // "显示成功但没真成功"。放宽到全体成员之后更容易撞上（成员没资格改状态、
    // 却可能挑一条没完成的任务点归档），所以改成明确拒绝。
    if archived == Some(true) && status != "done" {
        return Err("只有完成的任务可以归档".to_string());
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
        // 编号是创建那一刻定死的**绝对坐标**：编辑 / 归档 / 还原 / 删除都从原定义带过来，
        // 不接受客户端自报（与 `creator` 同一口径），否则"改一次换个号"就回来了。
        number: def.number,
        priority,
        category,
    };
    let content = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let (todo_id, status) = (payload.todo_id.clone(), payload.status.clone());
    let (deleted, archived) = (payload.deleted, payload.archived);
    // ⚠️ 发 `todo_update`（Silent）而不是 `todo`（Card）：改状态/改标题/删除是**状态微调**，
    // 不该给全群记未读、弹通知（创建才该）。两者载荷同构、折叠也是同一条 LWW 规则，
    // 区别只在通知口径 —— 详见 `protocol.rs` 的 `WIRE_KINDS` 注释。
    let rec = send_group_payload(s, &group_id, "todo_update", content, None).await?;
    // §29：状态迁移是本端与对端最容易各说一套的一段（徽标数字就靠它减），
    // 所以这里记的是**状态本身**而不是标题。
    s.logger.info(
        "group",
        format!(
            "群任务已改 task={todo_id} group={group_id} msg={} status={status} deleted={deleted} archived={archived:?}",
            rec.msg_id
        ),
    );
    Ok(rec)
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
    send_group_payload(s, &group_id, "announcement", content, None).await
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
    send_group_payload(s, &group_id, "announcement_delete", content, None).await
}

// ---------------- 群公告 ----------------
include!("group_announcements.rs");

// ---------------- Todo 图片 & 进度 ----------------
include!("group_todo_media.rs");

// ---------------- 群文件投递核心 ----------------
include!("group_file_dispatch.rs");

// ---------------- 密钥恢复 ----------------
include!("group_file_keys.rs");
