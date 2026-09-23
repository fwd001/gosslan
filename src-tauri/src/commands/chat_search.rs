// 职责边界：
// - 聊天历史搜索（search_messages / search_chat_history）
// - 搜索结果结构体（ChatSearchMessage/ChatSearchGroup/SearchResult）
// - ⚠️ 原混在 favorites.rs 里，因功能域独立而拆出

/// 搜索消息：返回匹配关键词的会话列表及其最新匹配消息。
#[tauri::command(async)]
pub fn search_messages(
    state: State<'_, Arc<AppState>>,
    keyword: String,
) -> Result<Vec<SearchResult>, String> {
    let s = state.inner();
    // 搜索关键词长度保护：按字符截断（UTF-8 安全）
    let keyword: String = keyword.chars().take(MAX_SEARCH_LEN).collect();
    let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
    let conv_ids = db::search_messages(&dbc, &keyword, 20).map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for conv_id in conv_ids {
        // 获取会话名称
        let name = db::get_friend(&dbc, &conv_id)
            .map(|(n, _)| n)
            .unwrap_or_else(|| conv_id.clone());
        // 获取最新匹配消息
        let msgs = db::search_messages_in_conv(&dbc, &conv_id, &keyword, 1).unwrap_or_default();
        if let Some(m) = msgs.into_iter().next() {
            results.push(SearchResult {
                conv_id,
                name,
                match_content: m.content,
                match_ts: m.ts,
                match_msg_id: m.msg_id,
            });
        }
    }
    Ok(results)
}

/// 「搜索聊天记录」结果页的一条命中。
#[derive(Serialize)]
pub struct ChatSearchMessage {
    pub msg_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub kind: String,
    pub content: String,
    pub ts: i64,
}

/// 按会话分组的命中（结果页左栏一个会话一行，右栏是它的命中消息）。
#[derive(Serialize)]
pub struct ChatSearchGroup {
    pub conv_id: String,
    pub name: String,
    /// single | group（前端据此决定是否显示发送者昵称）
    pub kind: String,
    pub avatar: Option<String>,
    /// 该会话在**当前筛选条件**下的命中总数（微信式「共 N 条相关聊天记录」）。
    pub total: i64,
    pub latest_ts: i64,
    /// 命中消息（时间倒序；受 `SEARCH_HISTORY_PER_CONV` 限制）。
    pub messages: Vec<ChatSearchMessage>,
}

/// 每个会话在结果页里最多展开多少条命中。用户真正要的是"扫一眼找到那条"，
/// 单个会话几百条既没人看也吃内存；点「进入聊天」才是继续翻的地方。
const SEARCH_HISTORY_PER_CONV: usize = 60;
/// 结果页最多返回多少个会话（左栏列表长度）。
const SEARCH_HISTORY_MAX_CONVS: i64 = 50;
/// 一次检索从库里取回的最大命中数（分组前的硬上限，防全表大结果）。
const SEARCH_HISTORY_MAX_HITS: i64 = 2000;

/// 搜索聊天记录（跨会话，按会话分组）。
///
/// 与 `search_messages`（会话列表里"内容命中的会话"摘要，只取每会话最新一条）的区别：
/// 这里要的是**结果页**的数据形态 —— 每个会话的命中总数 + 命中消息列表（含发送者），
/// 并支持微信搜索页那两个筛选：发送人、日期区间。
///
/// 为什么标 `(async)`：这是一次带 LIKE 的全表扫描（可能几千行），
/// 同步命令会在 macOS 主线程上跑（见 `heavy_commands_run_off_the_main_thread` 守卫）。
#[tauri::command(async)]
pub fn search_chat_history(
    state: State<'_, Arc<AppState>>,
    keyword: String,
    sender_id: Option<String>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
) -> Result<Vec<ChatSearchGroup>, String> {
    let s = state.inner();
    // 关键词长度保护（UTF-8 安全截断）
    let keyword: String = keyword.chars().take(MAX_SEARCH_LEN).collect();
    if keyword.trim().is_empty() {
        return Ok(Vec::new());
    }
    let sender = sender_id.filter(|v| !v.is_empty());

    let hits = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::search_history(
            &dbc,
            &keyword,
            sender.as_deref(),
            since_ms,
            until_ms,
            SEARCH_HISTORY_MAX_HITS,
        )
        .map_err(|e| e.to_string())?
    };

    // 分组（保持查询的 ts DESC 顺序 ⇒ 组内消息与左栏顺序都是"最新在前"），
    // 同时收集唯一发送者集合，供元数据批量预取。
    let mut order: Vec<String> = Vec::new();
    let mut grouped: std::collections::HashMap<String, Vec<db::ChatSearchHit>> =
        std::collections::HashMap::new();
    let mut sender_ids: Vec<String> = Vec::new();
    for h in hits {
        if !grouped.contains_key(&h.conv_id) {
            order.push(h.conv_id.clone());
        }
        if !sender_ids.contains(&h.sender_id) {
            sender_ids.push(h.sender_id.clone());
        }
        grouped.entry(h.conv_id.clone()).or_default().push(h);
    }

    // 元数据批量预取（审计 2.1c）：旧形状每个会话 conversation_meta()、每条命中
    // sender_display_name() 各抢一次 db 锁 —— 50 会话 × 60 条命中 ≈ 3000 次锁往返，
    // 库越大越卡。现在 db 一把锁查齐全部点查，peers 一把锁补非好友昵称（两锁不叠加）。
    let conv_list: Vec<String> = order
        .iter()
        .take(SEARCH_HISTORY_MAX_CONVS as usize)
        .cloned()
        .collect();
    let meta = SearchMeta::prefetch(s, &conv_list, &sender_ids);

    let mut out = Vec::new();
    for conv_id in order.into_iter().take(SEARCH_HISTORY_MAX_CONVS as usize) {
        let Some(list) = grouped.remove(&conv_id) else {
            continue;
        };
        let total = list.first().map(|h| h.total).unwrap_or(0);
        let latest_ts = list.first().map(|h| h.ts).unwrap_or(0);
        let (name, kind, avatar) = meta.conv_meta(&conv_id);
        let messages = list
            .into_iter()
            .take(SEARCH_HISTORY_PER_CONV)
            .map(|h| ChatSearchMessage {
                sender_name: meta.sender_name(&h.sender_id),
                msg_id: h.msg_id,
                sender_id: h.sender_id,
                kind: h.kind,
                content: h.content,
                ts: h.ts,
            })
            .collect();
        out.push(ChatSearchGroup {
            conv_id,
            name,
            kind,
            avatar,
            total,
            latest_ts,
            messages,
        });
    }
    Ok(out)
}

/// 会话/发送者元数据的批量预取缓存。
///
/// 语义与原 `conversation_meta` / `sender_display_name` 逐条版本完全一致：
/// 会话名/头像/类型（群读 groups、单聊读好友、自聊读本机）；
/// 发送者名（自己→昵称，好友→好友昵称，其它→peers 昵称，都 miss→设备 id 前 8 位）。
struct SearchMeta {
    conv: std::collections::HashMap<String, (String, String, Option<String>)>,
    sender: std::collections::HashMap<String, String>,
}

impl SearchMeta {
    fn prefetch(s: &AppState, conv_ids: &[String], sender_ids: &[String]) -> Self {
        let mut conv = std::collections::HashMap::new();
        let mut sender = std::collections::HashMap::new();
        {
            let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
            for c in conv_ids {
                conv.insert(c.clone(), conversation_meta_locked(&dbc, s, c));
            }
            for sid in sender_ids {
                if sid == &s.device_id {
                    sender.insert(
                        sid.clone(),
                        s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                    );
                } else if let Some((name, _)) = db::get_friend(&dbc, sid) {
                    sender.insert(sid.clone(), name);
                }
            }
        }
        // 非好友（群内陌生人 / 已删好友）回落 peers 昵称 —— db 锁已释放，两锁不叠加。
        {
            let peers = s.peers.lock().unwrap_or_else(|e| e.into_inner());
            for sid in sender_ids {
                if sender.contains_key(sid) {
                    continue;
                }
                if let Some(p) = peers.get(sid) {
                    if !p.nickname.is_empty() {
                        sender.insert(sid.clone(), p.nickname.clone());
                    }
                }
            }
        }
        Self { conv, sender }
    }

    fn conv_meta(&self, conv_id: &str) -> (String, String, Option<String>) {
        self.conv
            .get(conv_id)
            .cloned()
            .unwrap_or_else(|| (conv_id.to_string(), "single".to_string(), None))
    }

    /// 发送者的显示名；预取也 miss（预取列表不该漏，防御）→ 设备 id 前 8 位。
    fn sender_name(&self, sender_id: &str) -> String {
        self.sender
            .get(sender_id)
            .cloned()
            .unwrap_or_else(|| sender_id.chars().take(8).collect())
    }
}

/// 会话的显示名 / 类型 / 头像（群聊与单聊各取一处，与其它列表口径一致）。
/// 锁内版：Connection 由调用方（批量预取）提供，本函数不再自己抢锁。
fn conversation_meta_locked(
    dbc: &rusqlite::Connection,
    s: &AppState,
    conv_id: &str,
) -> (String, String, Option<String>) {
    // 「和自己聊天」的会话 id 就是本机 device_id（见 `insert_self_message`）
    if conv_id == s.device_id {
        return (s.self_display_name(), "single".to_string(), s.self_avatar());
    }
    if let Some(group_id) = conv_id.strip_prefix("group:") {
        if let Some(g) = db::get_group(dbc, group_id) {
            return (g.name, "group".to_string(), None);
        }
        return (conv_id.to_string(), "group".to_string(), None);
    }
    match db::get_friend(dbc, conv_id) {
        Some((name, avatar)) => (name, "single".to_string(), avatar),
        None => (conv_id.to_string(), "single".to_string(), None),
    }
}

#[derive(Serialize)]
pub struct SearchResult {
    conv_id: String,
    name: String,
    match_content: String,
    match_ts: i64,
    /// 命中消息的 msg_id：前端据此"跳到那一条"（只给 conv_id 的话，
    /// 用户点进去还要自己在会话里翻，搜索就只完成了一半）。
    match_msg_id: String,
}
