// 职责边界：
// - 收藏 CRUD（add/remove/list_favorite）
// - 收藏项的 media path 解析与复制重命名
// - 消息删除命令（delete_messages，收藏相关）
// - 聊天历史导出（export_chat_text）
// ---------------- 收藏 ----------------

/// 校验并解析一条收藏的**媒体副本**路径。
///
/// 安全边界比原消息更严（对照 [`resolve_media_path`]）：收藏副本只允许落在收藏目录内，
/// **没有"本机发出的消息"那种豁免** —— 副本是我们自己复制进去的，路径必须是我们给的。
/// 库里若被塞进 `/etc/passwd` 这类路径，这里必须拒绝，否则就成了任意文件读取。
fn resolve_favorite_media_path(s: &AppState, favorite_id: &str) -> MediaPath {
    let stored = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_favorite(&dbc, favorite_id)
    };
    let Ok(Some(fav)) = stored else {
        return MediaPath::Unknown("收藏不存在".to_string());
    };
    let Some(path) = fav.media_path else {
        return MediaPath::Unknown("这条收藏没有本地副本".to_string());
    };
    let Ok(file) = std::fs::canonicalize(&path) else {
        // 副本被人手删了（收藏目录不在存储清理范围内，走到这里只有这一种可能）。
        return MediaPath::Gone;
    };
    let under_favorites = std::fs::canonicalize(&s.favorites_dir)
        .map(|dir| file.starts_with(dir))
        .unwrap_or(false);
    if !under_favorites {
        return MediaPath::Unknown("路径越权".to_string());
    }
    match std::fs::metadata(&file) {
        Ok(meta) if meta.is_file() => MediaPath::Present(Box::new(file)),
        Ok(_) => MediaPath::Unknown("非普通文件".to_string()),
        Err(_) => MediaPath::Gone,
    }
}

/// 副本文件名：`{收藏 id}.{安全扩展名}`。
///
/// 只用 id 做文件名（不用原名）：原名可能带路径分隔符、超长、重名或非 ASCII，
/// 拿它拼路径是自找麻烦；界面上显示的名字从 `content.name` 来，与文件名无关。
/// 扩展名保留是为了「用系统里的其它程序打开」时还能认出文件类型。
fn favorite_copy_name(id: &str, content: &str, src: &std::path::Path) -> String {
    let ext_of = |p: &str| {
        std::path::Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_string())
    };
    let from_name = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| {
            v.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.to_string())
        })
        .and_then(|n| ext_of(&n));
    let ext = ext_of(&src.to_string_lossy())
        .or(from_name)
        .unwrap_or_default();
    // 只留 ASCII 字母数字并限长：扩展名会被拼进文件名，不能带 `.`/`/` 之类
    let safe: String = ext
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(10)
        .collect();
    if safe.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{safe}")
    }
}

/// 列出全部收藏（新的在前）。
///
/// `available` 在这里 stat 一次副本填好：前端据此把"副本不在了"渲染成「已清理」占位，
/// 而不是让用户点开才发现打不开。纯文本/代码没有副本，按"可用"处理（它没有可丢的东西）。
#[tauri::command(async)]
pub fn list_favorites(state: State<'_, Arc<AppState>>) -> Result<Vec<Favorite>, String> {
    let s = state.inner();
    let mut rows = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::list_favorites(&dbc).map_err(|e| e.to_string())?
    };
    for f in &mut rows {
        f.available = match f.media_path.as_deref() {
            Some(p) => std::path::Path::new(p).is_file(),
            None => true,
        };
    }
    Ok(rows)
}

/// 收藏一条消息（幂等：同一条消息重复收藏不会产生第二条）。
///
/// 图片/文件会把源文件**复制**到收藏目录，并把副本路径改写进 `content.path`
/// —— 前端整套渲染/打开/另存都只认 `content.path`，改写这一处就让收藏零改动复用它们。
/// 副本是"独立存储"的物理保证：之后删除会话、清理缓存都不再影响这条收藏。
#[tauri::command(async)]
pub fn add_favorite(
    state: State<'_, Arc<AppState>>,
    msg_id: String,
    conv_id: String,
) -> Result<Favorite, String> {
    let s = state.inner();
    let (sender_id, kind, content, ts) = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_favorite_source(&dbc, &msg_id)
    }
    .ok_or_else(|| "要收藏的消息不在本机".to_string())?;

    // 幂等：已收藏过就直接返回原条目 —— 不重复复制文件（那会给磁盘留下无人引用的垃圾）。
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = db::get_favorite_by_msg(&dbc, &msg_id).map_err(|e| e.to_string())? {
            return Ok(Favorite {
                available: true,
                ..existing
            });
        }
    }

    let id = Uuid::new_v4().to_string();
    let mut final_content = content.clone();
    let mut media_path: Option<String> = None;
    let mut media_size: i64 = 0;

    if kind == "image" || kind == "file" {
        let src = match resolve_media_path(s, &msg_id) {
            MediaPath::Present(p) => *p,
            MediaPath::Gone => {
                return Err("文件已不在本机（可能已被「存储清理」删除），无法收藏".to_string())
            }
            MediaPath::Unknown(e) => return Err(e),
        };
        let dst = s
            .favorites_dir
            .join(favorite_copy_name(&id, &content, &src));
        std::fs::copy(&src, &dst).map_err(|e| format!("复制到收藏目录失败：{e}"))?;
        media_size = std::fs::metadata(&dst).map(|m| m.len() as i64).unwrap_or(0);
        let dst_str = dst.to_string_lossy().to_string();
        final_content = db::favorite_content_with_path(&content, &dst_str);
        media_path = Some(dst_str);
    }

    let row = Favorite {
        id: id.clone(),
        msg_id: msg_id.clone(),
        conv_id,
        sender_id,
        kind,
        content: final_content,
        ts,
        favorited_at: db::now_ms(),
        media_path: media_path.clone(),
        media_size,
        available: true,
    };
    let inserted = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::insert_favorite(&dbc, &row).map_err(|e| e.to_string())?
    };
    if !inserted {
        // 竞态：两次收藏请求交错，另一条先落库。把这次多复制出来的副本删掉再返回已有条目，
        // 否则那份拷贝永远不会有人引用（收藏删的是另一条的 media_path）。
        if let Some(p) = media_path {
            let _ = std::fs::remove_file(p);
        }
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        let existing = db::get_favorite_by_msg(&dbc, &msg_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "收藏写入失败".to_string())?;
        return Ok(Favorite {
            available: true,
            ..existing
        });
    }
    s.logger.info(
        "favorite",
        format!("已收藏消息 kind={} msg_id={msg_id}", row.kind),
    );
    Ok(row)
}

/// 取消收藏：先删记录、再删副本文件。
///
/// 顺序不能反：先删文件而删记录失败，会留下一条**打不开**的收藏（比"删了记录但文件没删掉"
/// 差得多 —— 后者只占磁盘，用户看不见）。文件删除失败只记日志，不回滚记录。
#[tauri::command(async)]
pub fn remove_favorite(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    let s = state.inner();
    let removed = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_favorite(&dbc, &id).map_err(|e| e.to_string())?
    };
    let Some(fav) = removed else {
        return Ok(()); // 已经不在收藏里了：幂等返回，不报错
    };
    if let Some(path) = fav.media_path {
        let p = std::path::Path::new(&path);
        // 只删收藏目录内的文件：同一套边界判断，防止脏行把删除操作引到库外
        let under_favorites = std::fs::canonicalize(&s.favorites_dir)
            .ok()
            .zip(std::fs::canonicalize(p).ok())
            .map(|(dir, f)| f.starts_with(dir))
            .unwrap_or(false);
        if under_favorites {
            if let Err(e) = std::fs::remove_file(p) {
                s.logger
                    .warn("favorite", format!("删除收藏副本失败 {path}: {e}"));
            }
        } else {
            s.logger
                .warn("favorite", format!("跳过越权路径的副本删除：{path}"));
        }
    }
    Ok(())
}

/// 读取收藏副本的预览字节（图片）。契约与 [`read_file_preview`] 一致：
/// 超限返回 "TOO_LARGE"，副本不在返回 "文件不存在"。
#[tauri::command(async)]
pub fn read_favorite_preview(
    state: State<'_, Arc<AppState>>,
    id: String,
    max_bytes: u64,
) -> Result<tauri::ipc::Response, String> {
    let s = state.inner();
    let max_bytes = max_bytes.min(15 * 1024 * 1024);
    let file = match resolve_favorite_media_path(s, &id) {
        MediaPath::Present(p) => *p,
        MediaPath::Unknown(e) => return Err(e),
        MediaPath::Gone => return Err("文件不存在".to_string()),
    };
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    if meta.len() > max_bytes {
        return Err("TOO_LARGE".to_string());
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 一次批量删除的条数上限。
///
/// 为什么要有：整批在**一个事务**里删（见 `db::delete_messages`），条数过大就会把
/// `db` 互斥锁握住很久，而界面上的会话列表/搜索等读命令都在等这把锁。UI 侧的多选
/// 本来也不会一次选上千条。
const MAX_DELETE_BATCH: usize = 500;

/// 本地删除若干条消息（微信语义：**只删本机**，对方那边照常保留）。返回实际删除条数。
///
/// 为什么不复用"逐条删"的接口（假设前端循环调）：删 N 条要重算 N 次会话摘要、
/// 开 N 个事务；批量接口在一个事务里按会话去重算一次就够。
#[tauri::command(async)]
pub fn delete_messages(
    state: State<'_, Arc<AppState>>,
    msg_ids: Vec<String>,
) -> Result<usize, String> {
    let s = state.inner();
    if msg_ids.len() > MAX_DELETE_BATCH {
        return Err(format!(
            "一次最多删除 {MAX_DELETE_BATCH} 条，当前 {} 条",
            msg_ids.len()
        ));
    }
    if msg_ids.is_empty() {
        return Ok(0);
    }
    let removed = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::delete_messages(&dbc, &msg_ids).map_err(|e| e.to_string())?
    };
    s.logger
        .info("chat", format!("本地删除消息 {removed} 条（不影响对方）"));
    Ok(removed)
}

/**
 * 按 content store 的 cid 读取已落盘内容的字节（合并转发卡片图片 / 待办描述图片共用）。
 *
 * cid 即 sha256（content store 主键）。两条取回路径：① `find_local_path` —— 已登记的
 * 本机副本（接收到的群文件/待办图片、本机快照）；② 回落 `find_source` + 安全校验
 * （downloads 下或本机自发内容）—— 覆盖“发送侧待办图片还在用户原始路径”的场景。
 * 安全边界与 `read_file_preview`/`resolve_media_path` 同口径。
 */
#[tauri::command(async)]
pub fn read_content_preview(
    state: State<'_, Arc<AppState>>,
    cid: String,
    max_bytes: u64,
) -> Result<tauri::ipc::Response, String> {
    let s = state.inner();
    let max_bytes = max_bytes.min(15 * 1024 * 1024);
    let file = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        match crate::content::store::find_local_path(&dbc, &cid) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let (path, owner) = match crate::content::store::find_source(&dbc, &cid) {
                    Ok(Some((peer, _group, p))) => (p, peer),
                    Ok(None) => return Err("内容不存在".to_string()),
                    Err(e) => return Err(e.to_string()),
                };
                let f = std::fs::canonicalize(&path).map_err(|_| "文件不存在".to_string())?;
                let under_downloads = std::fs::canonicalize(
                    s.downloads_dir
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_path(),
                )
                .map(|dir| f.starts_with(dir))
                .unwrap_or(false);
                // 只允许读 downloads 下的文件（接收侧），或本机自己发出的内容（发送侧
                // 待办图片是用户自选的原始路径）—— 与 resolve_media_path 同一口径。
                if !under_downloads && owner != s.device_id {
                    return Err("路径越权".to_string());
                }
                f
            }
        }
    };
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("文件不存在".to_string());
    }
    if meta.len() > max_bytes {
        return Err("TOO_LARGE".to_string());
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 导出全部聊天文字到用户指定文件（Markdown 单文件）。
///
/// 定位：磁盘满 / 换机时的**自救手段**——存储清理只删媒体、不动文字，但一旦库损坏
/// 或要迁机，没有导出入口就只能看着数据丢。只导出文字，媒体仅保留文件名
/// （见 `export` 模块说明：刻意不产出 HTML，避免对端消息在本机浏览器里执行）。
///
/// `utc_offset_minutes` 由前端给出（`-new Date().getTimezoneOffset()`）：Rust 侧不引入
/// 时区库（`AI_RULES §25`），跨夏令时切换的历史消息可能有 1 小时偏差，已在模块注释说明。
#[tauri::command(async)]
pub fn export_chat_text(
    state: State<'_, Arc<AppState>>,
    destination: String,
    utc_offset_minutes: i64,
) -> Result<export::ExportSummary, String> {
    let s = state.inner();
    if destination.trim().is_empty() {
        return Err("导出路径为空".to_string());
    }
    let device_name = s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let my_id = s.device_id.clone();

    // 只把「读库」放进锁里：渲染几十万条消息 + 写盘可能耗时较长，
    // 不能让一次导出把消息落库卡住。
    let sections = {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        export::collect_sections(&dbc, &my_id)?
    };

    let messages: usize = sections.iter().map(|(_, m)| m.len()).sum();
    let conversations = sections.len();
    let generated_at = export::format_local_time(db::now_ms(), utc_offset_minutes);
    let text = export::render_markdown(&device_name, &generated_at, &sections, utc_offset_minutes);

    std::fs::write(&destination, text.as_bytes()).map_err(|e| format!("写入导出文件失败：{e}"))?;
    Ok(export::ExportSummary {
        conversations,
        messages,
        path: destination,
    })
}

/// 清除所有聊天数据（保留好友、身份、设置）。
/// SQLite 删除使用 transaction，任一失败则 rollback。
/// 文件系统清理在 DB commit 成功后执行；文件删除失败不影响 DB 结果。
/// 每批删除的行数。
///
/// 为什么是 2000：要把 `db` 互斥锁的**单次持有时长**压到毫秒级。用户实测的
/// "点「清除数据」设置窗口卡死"就是长事务握锁数秒导致的 —— 期间每个读命令都要等锁，
/// 等锁的 async 任务会占住工作线程，新的 IPC 排不上队，界面看起来就是死的。
const CLEAR_BATCH_ROWS: usize = 2000;

/// 删**一批**（同步核心，便于单测）：一批一个短事务，返回删除行数。
///
/// 表名只来自本文件里的字面量列表，不存在注入面。
fn clear_one_batch(
    db: &std::sync::Mutex<rusqlite::Connection>,
    table: &str,
) -> Result<u64, String> {
    let dbc = db.lock().unwrap_or_else(|e| e.into_inner());
    let tx = dbc.unchecked_transaction().map_err(|e| e.to_string())?;
    let n = tx
        .execute(
            &format!(
                "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT {CLEAR_BATCH_ROWS})"
            ),
            [],
        )
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n as u64)
}

/// 分批清空一张表：**每批一个短事务**，批与批之间释放 `db` 锁并让出调度，
/// 让同一进程里的读命令（会话列表、设置回读…）能插进来。
///
/// ⚠️ 「批间放锁」这件事有单测盯着（`clear_is_batched_so_the_db_lock_is_held_only_briefly`）：
/// 它是"点清除数据不再卡死"的**唯一**机制，改回一个大事务会静默退化。
async fn clear_table_batched(s: &Arc<AppState>, table: &str) -> Result<u64, String> {
    let mut total: u64 = 0;
    loop {
        let deleted = clear_one_batch(&s.db, table)?; // 锁在函数返回时释放
        total += deleted;
        if deleted == 0 {
            return Ok(total);
        }
        // 让出调度：给等锁的命令一个真正拿到锁的机会
        tokio::task::yield_now().await;
    }
}

#[tauri::command]
pub async fn clear_all_data(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    let s = state.inner();

    // 1. SQLite 删除（**分批**，见 `clear_table_batched`）。
    //    ⚠️ 这里不再用一个横跨所有表的大事务：那会把 `db` 互斥锁握住数秒，
    //    期间所有读命令都在等锁 —— 用户实测"点「清除数据」→ 设置窗口卡死、点不动"。
    //    代价：中途失败会留下**部分删除**（"清除数据"本身是破坏性操作，可接受），
    //    换来的是界面全程可用。
    let mut cleared: u64 = 0;
    for table in [
        "messages",
        "conversations",
        "outbox",
        "group_outbox",
        "file_outbox",
        "file_transfers",
        "pending_reads",
        "pending_group_reads",
        "group_reads",
        "group_files",
        "group_file_recipients",
        "group_members",
        "groups",
        // 收藏也是"聊天数据"：用户点「清除数据」就是要清干净，留下记录只会变成打不开的条目。
        "favorites",
    ] {
        cleared += clear_table_batched(s, table).await?;
    }
    {
        // 这两条量级很小（键值 + 群时钟），一次删完即可
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        dbc.execute("DELETE FROM settings WHERE key LIKE 'gk:%'", [])
            .map_err(|e| e.to_string())?;
        dbc.execute(
            "DELETE FROM conversation_clocks WHERE conv_id LIKE 'group:%'",
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    s.logger.info(
        "db",
        format!("清除聊天数据：共删除 {cleared} 行（分批执行，界面全程可响应）"),
    );

    // 2. Runtime state 清理：群密钥内存缓存一并清空（彻底退出群聊）。
    s.pending_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.pending_reads
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.pending_file_accept
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.pending_file_complete
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.pending_share_tree
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    // 群文件/文件投递运行态与待发群密钥同属聊天数据运行态（不清会残留
    // 已删群的 file_key，且 pending 群密钥可能在重连时复活已删群记录）
    s.group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.group_file_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.pending_group_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.group_file_sending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    s.file_sending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    *s.relay.lock().unwrap_or_else(|e| e.into_inner()) = crate::file_relay::RelayManager::new();
    // 先关闭未完成接收的文件句柄，再清理 downloads 目录中的 .part 临时文件。
    s.file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();

    // 3. 文件系统清理（DB commit 成功后执行）
    //    收集错误而非立即返回，避免文件清理失败伪装成"整个操作失败"
    let mut fs_errors: Vec<String> = Vec::new();

    // 清空 cache_dir 内容（保留目录本身）
    if s.cache_dir.exists() {
        for entry in std::fs::read_dir(&s.cache_dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            let result = if p.is_file() {
                std::fs::remove_file(&p)
            } else if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                fs_errors.push(format!("cache_dir: {} ({})", p.display(), e));
            }
        }
    }
    // 清空 downloads_dir 内容（保留目录本身）
    let dl = s
        .downloads_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if dl.exists() {
        for entry in std::fs::read_dir(&dl)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            let result = if p.is_file() {
                std::fs::remove_file(&p)
            } else if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                fs_errors.push(format!("downloads_dir: {} ({})", p.display(), e));
            }
        }
    }
    // 清空收藏副本目录（保留目录本身）。
    // 收藏目录不在 media_dirs 里、也不受存储清理影响（那正是它存在的意义），
    // 所以必须在这里**显式**清 —— 否则收藏记录删了、副本文件会永远留在磁盘上。
    if s.favorites_dir.exists() {
        for entry in std::fs::read_dir(&s.favorites_dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            let result = if p.is_file() {
                std::fs::remove_file(&p)
            } else if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                fs_errors.push(format!("favorites_dir: {} ({})", p.display(), e));
            }
        }
    }

    // 破坏性操作必须**广播**：用户实测（Mac 4.1.10）"在设置里清了缓存、目录和聊天记录，
    // 但主界面没反应" —— 因为清除只发生在设置窗口自己的 store 里，主窗口是另一个 WebView，
    // 它手里的会话列表/消息一条都没变（看起来像"没清掉"）。
    // 注意：即使有文件没删掉，**数据库记录已经清了** ⇒ 也要广播（否则界面同样显示旧数据）。
    s.notify_data_cleared(Some(window.label()));

    if fs_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "数据记录已清除，但部分缓存文件未能删除：{}",
            fs_errors.join("；")
        ))
    }
}

// ---------------- 聊天历史搜索 ----------------
include!("chat_search.rs");
