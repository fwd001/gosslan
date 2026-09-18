// 职责边界：
// - 收藏表 CRUD（insert/get/list/delete_favorite）
// - favorite_from_row 行解析、favorite_content_with_path 路径替换
// ---------------- 收藏 ----------------

/// 收藏表的列清单（读与写共用，避免两处列顺序漂移）。
const FAVORITE_COLS: &str =
    "id, msg_id, conv_id, sender_id, kind, content, ts, favorited_at, media_path, media_size";

fn favorite_from_row(r: &rusqlite::Row<'_>) -> Result<Favorite> {
    Ok(Favorite {
        id: r.get(0)?,
        msg_id: r.get(1)?,
        conv_id: r.get(2)?,
        sender_id: r.get(3)?,
        kind: r.get(4)?,
        content: r.get(5)?,
        ts: r.get(6)?,
        favorited_at: r.get(7)?,
        media_path: r.get(8)?,
        media_size: r.get(9)?,
        // 文件系统的事归命令层：db 层只负责"这条记录在不在"，不 stat 磁盘。
        available: false,
    })
}

/// 写入一条收藏。返回 `true` = 这次真的插入了；`false` = **同一条消息早就收藏过**。
///
/// 幂等靠 `UNIQUE(msg_id)` + `INSERT OR IGNORE`：重复收藏不报错也不产生第二行。
/// 调用方据此决定要不要提示"已在收藏中"，以及**要不要删掉刚复制出来的副本文件**
/// （幂等命中时那份复制是多余的，留着就是磁盘垃圾）。
pub fn insert_favorite(conn: &Connection, f: &Favorite) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO favorites
           (id, msg_id, conv_id, sender_id, kind, content, ts, favorited_at, media_path, media_size)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            f.id,
            f.msg_id,
            f.conv_id,
            f.sender_id,
            f.kind,
            f.content,
            f.ts,
            f.favorited_at,
            f.media_path,
            f.media_size
        ],
    )?;
    Ok(n > 0)
}

pub fn get_favorite_by_msg(conn: &Connection, msg_id: &str) -> Result<Option<Favorite>> {
    conn.query_row(
        &format!("SELECT {FAVORITE_COLS} FROM favorites WHERE msg_id = ?1"),
        params![msg_id],
        favorite_from_row,
    )
    .optional()
}

/// 按收藏 id 取一行（预览/删除前的路径解析用）。
pub fn get_favorite(conn: &Connection, id: &str) -> Result<Option<Favorite>> {
    conn.query_row(
        &format!("SELECT {FAVORITE_COLS} FROM favorites WHERE id = ?1"),
        params![id],
        favorite_from_row,
    )
    .optional()
}

/// 全部收藏，**新的在前**（`favorited_at DESC`）。
pub fn list_favorites(conn: &Connection) -> Result<Vec<Favorite>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FAVORITE_COLS} FROM favorites ORDER BY favorited_at DESC, id DESC"
    ))?;
    let rows = stmt.query_map([], favorite_from_row)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 删除一条收藏，返回被删的那一行（调用方要据此删掉副本文件）。
pub fn delete_favorite(conn: &Connection, id: &str) -> Result<Option<Favorite>> {
    let row = conn
        .query_row(
            &format!("SELECT {FAVORITE_COLS} FROM favorites WHERE id = ?1"),
            params![id],
            favorite_from_row,
        )
        .optional()?;
    if row.is_some() {
        conn.execute("DELETE FROM favorites WHERE id = ?1", params![id])?;
    }
    Ok(row)
}

/// 把消息 content 里的本地路径换成收藏副本路径（其余字段原样保留）。
///
/// 为什么要改写而不是另存一个字段：前端的渲染与打开/另存全都只认 `content.path`
/// （见 `utils/localFile.ts` 的路径入参），改写这一处就让收藏**零改动复用**整套渲染。
///
/// 解析失败时**原样返回**：宁可让这条收藏指向旧路径（打不开，界面上显示「已清理」），
/// 也不能因为一段畸形 JSON 让"收藏"这个动作整个失败。
pub fn favorite_content_with_path(content: &str, new_path: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "path".to_string(),
                    serde_json::Value::String(new_path.to_string()),
                );
                return v.to_string();
            }
            content.to_string()
        }
        Err(_) => content.to_string(),
    }
}

// ---------------- 测试 ----------------
include!("favorites_tests.rs");
