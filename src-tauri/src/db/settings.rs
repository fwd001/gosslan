// 职责边界：
// - key-value 设置表（KVPairs）CRUD
// ---------------- 设置（key-value） ----------------

/// 配置布尔值的**唯一口径**（#125，2026-09-27）。
///
/// 为什么要收成一处：历史上每个读取方各写各的 —— 有的是 `v == "1"`，有的是 `v != "0"`。
/// 于是同一个字面量在两个键上含义相反：写 `"false"` 对 `!= "0"` 是**开**，对 `== "1"` 是**关**。
/// 手工写库、测试预置、将来的迁移脚本都会踩这颗雷（今天就在 E2E 预置上踩过一次，见 roadmap #125）。
///
/// 合法写法：`1/true/on/yes` 为真，`0/false/off/no` 为假（大小写与首尾空白不敏感）。
/// 其它一律 `None` = **明确的非法**，由调用方决定回落哪个默认值，并被 `get_config_bool` 打成可见日志。
pub fn parse_config_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// 与 `get_config_bool` 同口径，但保留**"键不存在 ⇒ `None`（用户从没设置过）"**这层语义 ——
/// `get_settings` 那份 `Option` 面会被前端当成"未设置"处理，直接塌成 bool 会改变功能。
/// 值存在但非法 ⇒ 留一条 `[config]` 日志并返回 `None`（= 按未设置走调用方的默认）。
pub fn get_config_bool_opt(conn: &Connection, key: &str) -> Option<bool> {
    let raw = get_setting(conn, key)?;
    match parse_config_bool(&raw) {
        Some(v) => Some(v),
        None => {
            eprintln!(
                "[config] 非法布尔值 key={key} value={raw:?} ⇒ 按未设置处理（写回合法值即可消除本行）"
            );
            None
        }
    }
}

/// 读一个配置布尔值。
/// - 键不存在 ⇒ `default`（调用方自己的那个默认，两族键的默认本来就不同）
/// - 值非法 ⇒ **先留一条 `[config]` 日志再**按 `default` 走；绝不静默当成开或关
pub fn get_config_bool(conn: &Connection, key: &str, default: bool) -> bool {
    let raw = match get_setting(conn, key) {
        Some(v) => v,
        None => return default,
    };
    match parse_config_bool(&raw) {
        Some(v) => v,
        None => {
            eprintln!(
                "[config] 非法布尔值 key={key} value={raw:?} ⇒ 回落默认 {default}（写回合法值即可消除本行）"
            );
            default
        }
    }
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// ★ 模块名不叫 `tests`：这份文件是被 `include!` 进 `db` 模块的，`mod tests` 会和同作用域里已有的那个撞名。
#[cfg(test)]
mod config_bool_tests {
    use super::*;
    use rusqlite::Connection;

    fn conn_with_settings() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '')",
        )
        .expect("建表");
        conn
    }

    /// #125 的正身：同一个"关"允许有四种写法，同一个"开"也是 —— 而且**两个键共用同一份口径**。
    #[test]
    fn config_bool_accepts_both_spelling_families() {
        for raw in ["1", "true", "TRUE", " On ", "yes"] {
            assert_eq!(parse_config_bool(raw), Some(true), "把 {raw:?} 判成了非真");
        }
        for raw in ["0", "false", "OFF", "no", " No "] {
            assert_eq!(parse_config_bool(raw), Some(false), "把 {raw:?} 判成了非假");
        }
    }

    /// 未知值必须是**明确的非法**（`None`），不许被"不等于 0 就算开"那种写法悄悄吞掉。
    #[test]
    fn config_bool_rejects_unknown_literals() {
        for raw in ["", "2", "-1", "maybe", "tru", "开启", "0.0"] {
            assert_eq!(parse_config_bool(raw), None, "{raw:?} 竟被判成合法布尔值");
        }
    }

    /// 键不存在 ⇒ 调用方给的默认值；值非法 ⇒ 也走默认值（而不是"看起来像开"）。
    #[test]
    fn get_config_bool_uses_the_given_default_for_missing_and_invalid() {
        let conn = conn_with_settings();
        assert!(get_config_bool(&conn, "lan_enabled", true));
        assert!(!get_config_bool(&conn, "relay_enabled", false));
        set_setting(&conn, "lan_enabled", "false").unwrap();
        assert!(!get_config_bool(&conn, "lan_enabled", true), "写 false 必须算关");
        set_setting(&conn, "lan_enabled", "0").unwrap();
        assert!(!get_config_bool(&conn, "lan_enabled", true));
        set_setting(&conn, "lan_enabled", "1").unwrap();
        assert!(get_config_bool(&conn, "lan_enabled", false));
        set_setting(&conn, "lan_enabled", "开启").unwrap();
        assert!(
            get_config_bool(&conn, "lan_enabled", true),
            "非法值要落回默认，且这一步必须走可见日志（下面那条用例钉日志）"
        );
    }

    /// 反向对照：非法值**不许**被当成"关"（今天 `== "1"` 那种写法就是这个行为）
    /// 与"开"（`!= "0"` 那种写法）—— 两族默认值不同 ⇒ 只有"按调用方给的默认"才说得清。
    #[test]
    fn invalid_literal_follows_default_not_the_literal_shape() {
        let conn = conn_with_settings();
        set_setting(&conn, "bt_enabled", "garbage").unwrap();
        assert!(
            get_config_bool(&conn, "bt_enabled", true),
            "默认开 ⇒ 非法值不能变成关"
        );
        assert!(
            !get_config_bool(&conn, "bt_enabled", false),
            "默认关 ⇒ 非法值不能变成开"
        );
    }
}
