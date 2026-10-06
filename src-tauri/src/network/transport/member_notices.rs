// 群成员变动（加人 / 踢人 / 退群 / 转让群主）的群内系统消息：动作判定 + 两侧共用文案 + 名字解析。
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。

/// 群成员变更的**群内系统消息**文案。
///
/// 后端产生的系统消息同样要跟随语言设置 —— 前端 i18n 覆盖不到后端直接写库的行。
/// 两条文案各自只有一处实现，供「发起方」与「接收方」共用，避免措辞漂移。
/// 收到 `GroupMemberRemoved` 后本机应做的动作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberRemovedAction {
    /// 忽略：非创建者发起（防成员互踢），或本消息就来自本机（本地已处理）
    Ignore,
    /// 我本人被移出：清理本地群 + 会话 + 群密钥
    RemoveSelf,
    /// 别人被移出：同步本地成员表 + 落群内系统消息
    RemoveOther,
}

/// 分支选择（纯函数，便于单测）。
///
/// 单独抽出来的理由：这次 bug 的根因就是**少了一个分支** —— 接收端只处理「我本人被
/// 移出」，其余成员直接 return，于是群里其他人的成员表不变小、也看不到任何提示。
/// 把三分支的选择做成纯函数，「必须有 RemoveOther」这件事就被测试钉住了。
pub fn member_removed_action(
    sender_is_me: bool,
    sender_is_creator: bool,
    to_is_me: bool,
) -> MemberRemovedAction {
    if sender_is_me || !sender_is_creator {
        return MemberRemovedAction::Ignore;
    }
    if to_is_me {
        MemberRemovedAction::RemoveSelf
    } else {
        MemberRemovedAction::RemoveOther
    }
}

/// 往指定群的会话插一条本地系统消息。
///
/// 群的 conv_id 约定是 `group:{group_id}` —— 这个约定只有一处实现，避免各处手拼前缀。
pub fn insert_group_system_message(state: &AppState, group_id: &str, text: &str) {
    crate::commands::insert_system_message(state, &format!("group:{group_id}"), text);
}

/// 加人通知的文案。此前**加人完全没有通知**（只靠 GroupKey 重发 + 群消息自愈），
/// 群里其他人根本不知道多了一个成员 —— 而踢人/退群都是有系统消息的，
/// 同一类事件两种待遇。语言跟随本机设置，与既有两条同口径。
pub fn group_member_added_text(state: &AppState, name: &str) -> String {
    if state.is_zh() {
        format!("「{name}」加入了群聊")
    } else {
        format!("\"{name}\" joined the group")
    }
}

pub fn group_member_removed_text(state: &AppState, name: &str) -> String {
    if state.is_zh() {
        format!("「{name}」已被移出群聊")
    } else {
        format!("“{name}” has been removed from the group")
    }
}

/// 成员**主动退群**的群内系统消息文案（详见 `group_member_removed_text`）。
pub fn group_member_left_text(state: &AppState, name: &str) -> String {
    if state.is_zh() {
        format!("「{name}」退出了群聊")
    } else {
        format!("“{name}” left the group")
    }
}

/// **群主转让**的群内系统消息文案（详见 `group_member_removed_text`）。
///
/// 此前转让群主**没有任何群内提示**：成员表里的「群主」标记悄悄换了人，群里一声不响 ——
/// 而加人 / 踢人 / 退群三种成员变更都是有系统消息的，同类事件三种待遇。
/// 说话人视角对两侧都成立（旧群主发起、其余成员接收），所以两边共用这一句。
pub fn group_creator_changed_text(state: &AppState, name: &str) -> String {
    if state.is_zh() {
        format!("「{name}」成为新群主")
    } else {
        format!("“{name}” is now the group owner")
    }
}

pub fn resolve_nickname(state: &AppState, id: &str) -> String {
    // 自己：好友表/节点表里都没有"我"，不特判就会回落到 device_id 原文
    // （「和自己聊天」的会话名、把自己当发送者时的文案都会变成一串 gosslan-xxxx）。
    if id == state.device_id {
        return state.self_display_name();
    }
    if let Some(p) = state
        .peers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
    {
        if !p.nickname.is_empty() {
            return p.nickname.clone();
        }
    }
    if let Some(r) = state
        .pending_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
    {
        return r.from_nickname.clone();
    }
    if let Some((n, _)) = db::get_friend(&state.db.lock().unwrap_or_else(|e| e.into_inner()), id) {
        return n;
    }
    id.to_string()
}

fn resolve_group_name(state: &AppState, group_id: &str) -> String {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(groups) = db::list_groups(&dbc) {
        if let Some(g) = groups.into_iter().find(|g| g.id == group_id) {
            return g.name;
        }
    }
    format!("群聊 {group_id}")
}

fn preview_content(kind: &str, content: &str) -> String {
    // 预览文案的唯一事实源在 `protocol::preview_text`（会话摘要 + 通知正文共用；
    // 前端 `utils/messages.ts` 的 `previewText` 与之逐项一致）。
    crate::protocol::preview_text(kind, content)
}
