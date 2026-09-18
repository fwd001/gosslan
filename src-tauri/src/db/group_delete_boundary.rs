// 职责边界：
// - 群聊删除边界（clear_chat 后防旧消息回灌）
// ---------------- 群聊删除边界（清除聊天数据后旧消息防回灌） ----------------

/// 本地删除边界键：记录本机清除该群聊时的逻辑序号（Lamport seq）。
pub fn clear_boundary_key(group_id: &str) -> String {
    format!("clear_boundary:group:{group_id}")
}

/// 写入群聊删除边界（清除/删除群会话时调用）。
pub fn set_clear_boundary(conn: &Connection, group_id: &str, seq: i64) -> Result<()> {
    set_setting(conn, &clear_boundary_key(group_id), &seq.to_string())
}

/// 群消息落库前的边界判定：逻辑序号 <= 清除边界 → 视为旧历史，不得重新写入本机。
/// 不再使用墙上时钟，也不猜测发送方时钟。
/// 该消息是否被「清空聊天记录」的水位挡住（不该落库）。
///
/// ⚠️ **只对 `Bubble` 生效**。水位是**聊天历史**的水位，不该管群级沉淀物：
/// 一个离线成员的公告（Card）如果 seq ≤ 本机 boundary 就被丢弃，
/// 会导致**各成员看到的公告不一致** —— 而公告恰恰是要求"所有人都看到同一份"的东西。
/// 静默事件同理：它们不进时间线，与"清空历史"无关。
pub fn group_message_blocked_by_boundary(
    conn: &Connection,
    group_id: &str,
    seq: i64,
    kind: &str,
) -> bool {
    if crate::protocol::kind_class(kind) != crate::protocol::KindClass::Bubble {
        return false;
    }
    get_setting(conn, &clear_boundary_key(group_id))
        .and_then(|v| v.parse::<i64>().ok())
        .map(|boundary| seq <= boundary)
        .unwrap_or(false)
}

/// 删除一条设置（「恢复默认」时清除偏好键，让上层回落到默认值）。
pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}

/// 局域网通道开关偏好：只有显式写入 "0"（用户在设置页关闭）才为关。
/// 键不存在 ⇒ 开启 ⇒ 首次安装与「尚无该键」的旧版本升级都会自动联网，
/// 并立即将此默认值持久化，之后每次启动读到明确的 "1" 而非依赖隐式默认。
/// 读取方仅存在于桌面端启动路径（`lib.rs` 的 `#[cfg(desktop)]` 块），
/// 移动端仍由设置页手动开启，故在该目标下为死代码。
#[cfg_attr(not(desktop), allow(dead_code))]
pub fn get_lan_enabled(conn: &Connection) -> bool {
    match get_setting(conn, "lan_enabled") {
        Some(v) => v != "0",
        None => {
            set_lan_enabled(conn, true).ok();
            true
        }
    }
}

/// 写入局域网通道开关偏好（沿用 settings 表，不引入新的配置存储）。
pub fn set_lan_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    set_setting(conn, "lan_enabled", if enabled { "1" } else { "0" })
}

/// 蓝牙通道开关偏好。
///
/// **移动端默认开启**（用户 2026-09-12 安卓实测要求：「如果测到蓝牙是手机的话，蓝牙通道
/// 应该是默认打开的，并且不用设置」—— 参考 BitChat：进去就能连，不用配对、不用配置、
/// 不用先去设置里打开开关）。桌面端维持默认关闭：局域网是有线/同网段的快路径，
/// 蓝牙是可选的低带宽通道，不该在用户没要求时悄悄开射频。
/// 键不存在时才套用默认值并**立刻持久化**（与 `get_lan_enabled` 同一套语义：
/// 之后每次启动读到的是明确的 "0"/"1"，而不是依赖隐式默认）。
pub fn get_bt_enabled(conn: &Connection) -> bool {
    match get_setting(conn, "bt_enabled") {
        Some(v) => v == "1",
        None => {
            // 用户 2026-09-12 规则：**有蓝牙就默认开**（三端一致）——
            // 之前桌面默认关、手机默认开，结果"手机上默认有通道、Mac 上还要手动点一次"，
            // 而且（更糟）会让"偏好=关"与"运行时=开"互相回灌，触发启停抖动（见 `set_channel_enabled`）。
            let default_on = true;
            set_bt_enabled(conn, default_on).ok();
            default_on
        }
    }
}

/// 写入蓝牙通道开关偏好（沿用 settings 表，不引入新的配置存储）。
pub fn set_bt_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    set_setting(conn, "bt_enabled", if enabled { "1" } else { "0" })
}
