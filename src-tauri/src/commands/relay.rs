// 职责边界：
// - 公网中转（blind circuit relay）的配置读写与校验
//   （服务器代码在独立仓库 fwd001/gosslan-relay-server；本文件只管"客户端怎么记住它"）
//   见 ADR-0020。
// ---------------- 公网中转配置 ----------------

/// 中继配置在前端与命令层之间的形状。
///
/// ⚠️ 这是**独立命令**，不是 `Settings` 的三个字段。理由是真实故障形状，不是审美：
/// 前端的 `persistSettings()` 会把所有偏好打包成**一次** `save_settings` 全量写。
/// 若把中继地址塞进 `Settings` 并在其中做校验，一个非法地址就会让**整次保存失败**
/// —— 用户改的主题、昵称、缓存策略全都存不下去，而提示只说"地址格式不对"。
/// 跨网段端点（`routed_endpoints`）早就是这个形状，这里沿用它。
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RelayConfigView {
    /// 是否启用。关掉之后服务器地址与口令仍然保留（用户常常是先试再关）。
    pub enabled: bool,
    /// 规范化后的 `ip:port`；空串 = 未配置。
    pub server: String,
    /// 准入口令。原样回显给用户：这是**用户自己填的**口令，本机 SQLite 里也是明文
    /// （身份私钥同样如此），不额外做"只存哈希"的假加密。
    pub token: String,
}

/// `settings` 表里的三个键。定义成常量，避免"拨号侧读的字面量和写入侧的字面量不一致"。
pub const RELAY_ENABLED_KEY: &str = "relay_enabled";
pub const RELAY_SERVER_KEY: &str = "relay_server";
pub const RELAY_TOKEN_KEY: &str = "relay_token";
/// 口令长度上限：中继首行整体 ≤ 256 字节（`GSRL1 <token> <64hex>\n` ≈ 80 + len(token)）。
/// 超了会被服务器判"首行超长"直接断开 —— 那是**最难查的一类**表现：
/// 客户端一路"拨号未成功"，服务器一行 `[reject] 首行超长`，两边都看不出根因。
pub const RELAY_TOKEN_MAX_LEN: usize = 128;

/// 规范化用户填的中继地址，返回 `ip:port`。
///
/// 省略端口时补 [`RELAY_DEFAULT_PORT`]（不是局域网那个 59992 —— 同一台机器上两者要能共存）。
/// 解析与默认端口补全复用 `discovery::routed` 的**同一个实现**：那边曾经因为
/// "命令层接受裸 IP、拨号侧只认 ip:port"而造成配置静默失效（见 `routed.rs` 注释），
/// 同一个错误不该在第二条路径上再犯一次。
pub fn normalize_relay_server(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("请填写中转服务器地址，例如 203.0.113.10:59993".to_string());
    }
    let Some(addr) = parse_endpoint_addr_on(trimmed, RELAY_DEFAULT_PORT) else {
        return Err(format!("地址格式应为 ip 或 ip:port，收到：{trimmed}"));
    };
    if addr.is_ipv6() {
        // 与 routed 同一判据：TCP 监听侧只绑 IPv4，IPv6 拨出去也连不上。
        // 当场拒绝好过"存下来了但永远连不上"。
        return Err("暂不支持 IPv6 地址（当前 TCP 监听仅 IPv4）。请填写 IPv4，例如 203.0.113.10:59993".to_string());
    }
    Ok(addr.to_string())
}

/// 校验口令。**空白字符必须拒绝**：中继首行是 `GSRL1 <token> <ch>` 这种空格分隔的一行文本，
/// 口令里带空格会让服务器解析出错误的字段数 ⇒ 永远被拒，而客户端只会反复报"拨号未成功"。
pub fn validate_relay_token(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("请填写服务器上的访问口令（部署时设置的 TOKEN）".to_string());
    }
    if trimmed.chars().any(|c| c.is_whitespace()) {
        return Err("访问口令不能包含空格或换行".to_string());
    }
    if trimmed.len() > RELAY_TOKEN_MAX_LEN {
        return Err(format!(
            "访问口令过长（{} 字符，上限 {} 字符）",
            trimmed.len(),
            RELAY_TOKEN_MAX_LEN
        ));
    }
    Ok(trimmed.to_string())
}

/// 运行时读到的中继配置（拨号循环每轮调一次；**唯一**的读取实现）。
///
/// 为什么每次读库而不是在 `AppState` 里缓存一份：`routed_endpoints` 就是这个做法，
/// 好处是"改了立即生效、不需要重启、也没有第二份真相要维护"；代价是每 10s 一次
/// SQLite 读，在这个量级上可以忽略。缓存会额外要求"每条写路径都记得更新缓存"
/// —— 那是本项目反复付钱的地方（`relay_policy` 的缓存就是靠 save/reset 两处手工同步）。
///
/// 返回 `None` = 不该建任何中继电路（未启用 / 未配置 / 历史脏数据解析失败）。
/// **脏数据退化成"不启用"而不是报错**：这条路径在拨号循环里，没有用户界面能承接错误。
pub fn load_relay_runtime(db: &rusqlite::Connection) -> Option<RelayRuntime> {
    let enabled = db::get_setting(db, RELAY_ENABLED_KEY).is_some_and(|v| v == "1");
    if !enabled {
        return None;
    }
    let server = db::get_setting(db, RELAY_SERVER_KEY).unwrap_or_default();
    let token = db::get_setting(db, RELAY_TOKEN_KEY).unwrap_or_default();
    let addr = parse_endpoint_addr_on(&server, RELAY_DEFAULT_PORT)?;
    if token.is_empty() || token.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some(RelayRuntime { addr, token })
}

/// 拨号侧要的已解析配置。
pub struct RelayRuntime {
    pub addr: std::net::SocketAddr,
    pub token: String,
}

/// 读取当前中继配置（供设置页显示）。
#[tauri::command(async)]
pub fn get_relay_config(state: State<'_, Arc<AppState>>) -> RelayConfigView {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    RelayConfigView {
        // 缺省 = 关：这个功能必须用户显式开启（不填服务器就没有这条链路，行为与今天一致）。
        enabled: db::get_setting(&dbc, RELAY_ENABLED_KEY).is_some_and(|v| v == "1"),
        server: db::get_setting(&dbc, RELAY_SERVER_KEY).unwrap_or_default(),
        token: db::get_setting(&dbc, RELAY_TOKEN_KEY).unwrap_or_default(),
    }
}

/// 保存中继配置。
///
/// **校验失败一律返回 `Err`，不静默丢**：填了地址却因脏值被丢弃是本项目已记录过的
/// 最难排查的一类故障（routed 端点的注释里就写着那次）。开启状态下地址与口令都必须有效；
/// 关闭状态下允许留空（用户可能只想暂时停用）。
#[tauri::command(async)]
pub fn save_relay_config(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    enabled: bool,
    server: String,
    token: String,
) -> Result<RelayConfigView, String> {
    let server_in = server.trim().to_string();
    let token_in = token.trim().to_string();
    let server_norm = if enabled || !server_in.is_empty() {
        normalize_relay_server(&server_in)?
    } else {
        String::new()
    };
    let token_norm = if enabled || !token_in.is_empty() {
        validate_relay_token(&token_in)?
    } else {
        String::new()
    };

    let view = RelayConfigView {
        enabled,
        server: server_norm.clone(),
        token: token_norm.clone(),
    };
    {
        let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, RELAY_ENABLED_KEY, if enabled { "1" } else { "0" })
            .map_err(|e| format!("保存失败: {e}"))?;
        db::set_setting(&dbc, RELAY_SERVER_KEY, &server_norm)
            .map_err(|e| format!("保存失败: {e}"))?;
        db::set_setting(&dbc, RELAY_TOKEN_KEY, &token_norm)
            .map_err(|e| format!("保存失败: {e}"))?;
    }
    // 留痕：这条链路的故障绝大多数出在"配错了"，日志里必须看得见配了什么。
    // 口令**只记长度**，不记内容（日志会被导出、也可能被截图给别人看）。
    state.logger.info(
        "relay",
        format!(
            "配置已保存 enabled={} server={} token_len={}",
            enabled,
            server_norm,
            token_norm.chars().count()
        ),
    );
    // 刻意**不发** `settings-changed`：与 `routed_endpoints` 同一形状 —— 拨号循环每 10s
    // 重读一次库，运行时改配置免重启即生效；而事件会让另一个窗口去做一次全量重拉，
    // 换来的只是"界面上那一行早出现 10 秒"。
    let _ = window;
    Ok(view)
}

#[cfg(test)]
// 名字必须唯一：`commands.rs` 用 `include!` 把所有命令文件并进**同一个模块**，
// 而那里已经有一个由 `logs_tests.rs` 提供的 `mod tests` —— 再叫 `tests` 就是 E0428。
mod relay_config_tests {
    use super::*;

    #[test]
    fn relay_server_default_port_is_not_the_lan_port() {
        // 同一台机器上既跑 LAN TCP 又跑中继时，两个默认端口必须不同，
        // 否则用户"只填 IP"会得到一个连不上的组合且无从判断。
        assert_ne!(RELAY_DEFAULT_PORT, crate::protocol::TCP_PORT);
        assert_eq!(
            normalize_relay_server("203.0.113.10").unwrap(),
            format!("203.0.113.10:{}", RELAY_DEFAULT_PORT)
        );
    }

    #[test]
    fn relay_server_accepts_ip_and_ip_port_and_rejects_junk() {
        assert_eq!(normalize_relay_server(" 198.51.100.7:6000 ").unwrap(), "198.51.100.7:6000");
        assert_eq!(normalize_relay_server("10.0.0.5").unwrap(), "10.0.0.5:59993");
        for bad in ["", "   ", "relay.example.com", "203.0.113", "256.1.1.1:80", "abc"] {
            assert!(
                normalize_relay_server(bad).is_err(),
                "非法地址必须当场拒绝，不能存下来让用户以为配好了：{bad:?}"
            );
        }
    }

    #[test]
    fn relay_token_rejects_whitespace_because_the_preamble_is_space_separated() {
        assert_eq!(validate_relay_token("  hunter2  ").unwrap(), "hunter2");
        for bad in ["", "  ", "has space", "has\ttab", "有 中文空格"] {
            assert!(validate_relay_token(bad).is_err(), "含空白的口令会破坏首行：{bad:?}");
        }
        let long = "x".repeat(RELAY_TOKEN_MAX_LEN + 1);
        assert!(validate_relay_token(&long).is_err(), "超长口令会被服务器判首行超长");
        assert!(validate_relay_token(&"x".repeat(RELAY_TOKEN_MAX_LEN)).is_ok());
    }

    /// 运行时读取必须**fail-closed**：未启用 / 缺地址 / 缺口令 / 脏数据 ⇒ `None`，
    /// 而不是拿着半套配置去拨号。
    #[test]
    fn load_relay_runtime_is_fail_closed() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(crate::db::SCHEMA).unwrap();
        let put = |k: &str, v: &str| db::set_setting(&db, k, v).unwrap();

        // 什么都没有 ⇒ None
        assert!(load_relay_runtime(&db).is_none());
        // 开了但没地址 ⇒ None
        put(RELAY_ENABLED_KEY, "1");
        assert!(load_relay_runtime(&db).is_none());
        // 地址有了、口令空 ⇒ None
        put(RELAY_SERVER_KEY, "203.0.113.9:59993");
        assert!(load_relay_runtime(&db).is_none());
        // 齐了 ⇒ Some，且裸 IP 也补默认端口
        put(RELAY_TOKEN_KEY, "s3cret");
        let rt = load_relay_runtime(&db).expect("齐了就该读到");
        assert_eq!(rt.addr.port(), RELAY_DEFAULT_PORT);
        assert_eq!(rt.addr.ip().to_string(), "203.0.113.9");
        assert_eq!(rt.token, "s3cret");
        // 关掉 ⇒ None（配置保留但不建链路）
        put(RELAY_ENABLED_KEY, "0");
        assert!(load_relay_runtime(&db).is_none());
        // 脏值（不是 "1"）一律按未启用处理，不当成"开启且值为真"
        put(RELAY_ENABLED_KEY, "true");
        assert!(load_relay_runtime(&db).is_none());
        // 地址被写成脏字符串 ⇒ None（解析失败不 panic、不兜底成某个默认值）
        put(RELAY_ENABLED_KEY, "1");
        put(RELAY_SERVER_KEY, "not-an-ip");
        assert!(load_relay_runtime(&db).is_none());
        // 口令里混进空白（历史数据/手工写库）⇒ None
        put(RELAY_SERVER_KEY, "203.0.113.9");
        put(RELAY_TOKEN_KEY, "bad token");
        assert!(load_relay_runtime(&db).is_none());
    }
}
