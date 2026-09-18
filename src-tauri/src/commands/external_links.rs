// 职责边界：
// - 外链管理（add/remove/list_link、validate_external_url）
// ---------------- 外部链接（左栏「链接」视图 → 点开在独立窗口加载） ----------------

/// 一条用户配置的外部链接。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLink {
    /// 稳定主键：改名字/改网址都按它匹配，不会跟丢。
    pub id: String,
    pub name: String,
    pub url: String,
}

const EXTERNAL_LINKS_KEY: &str = "external_links";
/// 数量上限（防呆：这是一排给人点的入口，不是书签管理器）。
const MAX_EXTERNAL_LINKS: usize = 20;
/// 显示名长度上限（字符数）。
const MAX_LINK_NAME_CHARS: usize = 32;

/// 校验并规范化一个外部网址 —— **只允许 http/https 且 host 非空**。
///
/// 为什么必须做协议白名单：这个网址会被 [`open_link_window`] 交给 `WebviewUrl::External`
/// 直接加载，`javascript:`/`data:`/`file:`/`tauri:` 之类要么能执行脚本、要么能读本机文件 ——
/// 等于把一条本机攻击面交给用户随手粘贴的字符串。与 `utils/linkify.ts` 只认 `https?://`
/// 是同一条口径（消息里的链接也受同一限制）。
pub fn validate_external_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("网址不能为空".to_string());
    }
    let parsed = url::Url::parse(trimmed).map_err(|_| "网址格式不正确".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("网址必须以 http:// 或 https:// 开头".to_string());
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err("网址缺少主机名".to_string());
    }
    Ok(parsed.to_string())
}

/// 容错解析：逐条跳过坏数据，绝不因一条脏数据让整张表消失（与 `parse_endpoints` 同口径）。
fn parse_external_links(raw: &str) -> Vec<ExternalLink> {
    serde_json::from_str::<Vec<ExternalLink>>(raw)
        .map(|list| {
            list.into_iter()
                .filter(|l| {
                    !l.id.is_empty() && !l.name.is_empty() && validate_external_url(&l.url).is_ok()
                })
                .take(MAX_EXTERNAL_LINKS)
                .collect()
        })
        .unwrap_or_default()
}

fn encode_external_links(list: &[ExternalLink]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".to_string())
}

/// 规范化编辑输入并做重复校验（`except_id` = 编辑时排除自己那条）。
fn normalize_link_input(
    list: &[ExternalLink],
    name: &str,
    url: &str,
    except_id: Option<&str>,
) -> Result<(String, String), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("请填写名称".to_string());
    }
    if name.chars().count() > MAX_LINK_NAME_CHARS {
        return Err(format!("名称最长 {MAX_LINK_NAME_CHARS} 个字符"));
    }
    let url = validate_external_url(url)?;
    if list
        .iter()
        .any(|l| l.url == url && Some(l.id.as_str()) != except_id)
    {
        return Err("该网址已存在".to_string());
    }
    Ok((name, url))
}

/// 列出全部外部链接。
#[tauri::command(async)]
pub fn list_external_links(state: tauri::State<'_, Arc<AppState>>) -> Vec<ExternalLink> {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    parse_external_links(&db::get_setting(&dbc, EXTERNAL_LINKS_KEY).unwrap_or_default())
}

/// 添加一条外部链接，返回更新后的完整列表。
#[tauri::command(async)]
pub fn add_external_link(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    url: String,
) -> Result<Vec<ExternalLink>, String> {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list =
        parse_external_links(&db::get_setting(&dbc, EXTERNAL_LINKS_KEY).unwrap_or_default());
    if list.len() >= MAX_EXTERNAL_LINKS {
        return Err(format!("最多只能添加 {MAX_EXTERNAL_LINKS} 个链接"));
    }
    let (name, url) = normalize_link_input(&list, &name, &url, None)?;
    list.push(ExternalLink {
        id: Uuid::new_v4().to_string(),
        name,
        url,
    });
    db::set_setting(&dbc, EXTERNAL_LINKS_KEY, &encode_external_links(&list))
        .map_err(|e| format!("保存失败: {e}"))?;
    Ok(list)
}

/// 修改一条外部链接（按 `id` 匹配），返回更新后的完整列表。
#[tauri::command(async)]
pub fn update_external_link(
    state: tauri::State<'_, Arc<AppState>>,
    id: String,
    name: String,
    url: String,
) -> Result<Vec<ExternalLink>, String> {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list =
        parse_external_links(&db::get_setting(&dbc, EXTERNAL_LINKS_KEY).unwrap_or_default());
    // 先算规范化值（不可变借用），再取可变引用落值 —— 否则 `list` 同时被借两次。
    let (name, url) = normalize_link_input(&list, &name, &url, Some(&id))?;
    let Some(target) = list.iter_mut().find(|l| l.id == id) else {
        return Err("链接不存在".to_string());
    };
    target.name = name;
    target.url = url;
    db::set_setting(&dbc, EXTERNAL_LINKS_KEY, &encode_external_links(&list))
        .map_err(|e| format!("保存失败: {e}"))?;
    Ok(list)
}

/// 删除一条外部链接（按 `id` 匹配），返回更新后的完整列表。
#[tauri::command(async)]
pub fn remove_external_link(
    state: tauri::State<'_, Arc<AppState>>,
    id: String,
) -> Result<Vec<ExternalLink>, String> {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list =
        parse_external_links(&db::get_setting(&dbc, EXTERNAL_LINKS_KEY).unwrap_or_default());
    let before = list.len();
    list.retain(|l| l.id != id);
    if list.len() != before {
        db::set_setting(&dbc, EXTERNAL_LINKS_KEY, &encode_external_links(&list))
            .map_err(|e| format!("保存失败: {e}"))?;
    }
    Ok(list)
}
