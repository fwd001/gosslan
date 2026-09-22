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

// ---------------- 保存前"真拨一次"（INTEGRATION.md 要求 2）----------------

/// 只填 IP 时依次尝试的候选端口。
///
/// 为什么要有这一条：用户手上的信息常常只有"服务器那个 IP"，而端口是部署者用 `PORT`
/// 环境变量自己定的。逐个试的成本是一次 TCP 握手（毫秒级），换来的正是"少配置"；
/// 而**用户显式写了端口就只试那一个** —— 他已经给了答案，再去猜只会把"端口不通"
/// 这件本该立刻看见的事藏起来。
pub const RELAY_PROBE_PORTS: [u16; 3] = [RELAY_DEFAULT_PORT, 443, 59994];

/// 探测时"等多久算首行被接受"。
///
/// 必须**同时远小于**服务器 `WAIT_TIMEOUT_MS`（30s）与客户端协商看门狗
/// （`RELAY_NEGOTIATE_TIMEOUT_SECS = 10`）：`INTEGRATION.md` §1.1 说得很清楚，服务器的
/// 两种命运差两个数量级（毫秒级 RST vs 被挂在等待表里），所以判"活着"只需盖过公网 RTT；
/// 拖到 10s 以上会撞自己的看门狗，拖到 30s 以上则是白占服务器一个 pending 槽
/// （每槽 `PENDING_MAX` = 1 MiB）。
const RELAY_PROBE_HOLD: Duration = Duration::from_millis(2_500);

/// 建立 TCP 连接的预算（单个候选端口）。
const RELAY_PROBE_CONNECT: Duration = Duration::from_millis(3_000);

/// 一次探测的结论。`kind` 是**机器可读**串（界面按它取文案，不拿中文串去匹配），
/// 取值与 `RelayProbeKind::as_str` 同一套 —— 拨号侧与探测侧共用一份分类。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RelayProbeReport {
    /// `"unreachable"` | `"rejected"` | `"held"`（探测不会产生 `peer_identity`：没有对端）。
    pub kind: String,
    /// 探到的那个 `ip:port`。三档都有值：全不可达时是**第一个**候选。
    pub server: String,
    /// 试过哪些端口（`"203.0.113.9:59993, 203.0.113.9:443"`），供诊断抄给人看。
    pub tried: String,
    /// 人可读的补充。**绝不含口令**（要求 7：口令不进日志、不进诊断面板的截图字段）。
    pub detail: String,
}

/// 探测用的"假通道哈希"：形状合法（64 位小写 hex），值是随机的 ⇒ 服务器上不会有人
/// 登记同一个 ch，于是"被拒 / 被挂"这两档的判据完全不受第三方影响。
fn probe_channel_hex() -> String {
    use rand_core::RngCore;
    let mut b = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 单个地址的一次探测。
///
/// ⚠️ 这里**不经过** `connect_to_peer`：那条路会把链路登记进 `state.links`、占住在途守卫、
/// 真的去发 `Hello`。探测只想问服务器"我这行首字你收不收"，所以自己开一条裸 socket，
/// 拿到结论就关。副产品是它在服务器上最多占一个 pending 槽 `RELAY_PROBE_HOLD` 那么久。
async fn probe_relay_addr(
    addr: std::net::SocketAddr,
    token: &str,
) -> (crate::network::transport::RelayProbeKind, String) {
    use crate::network::transport::RelayProbeKind;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let stream = match tokio::time::timeout(
        RELAY_PROBE_CONNECT,
        tokio::net::TcpStream::connect(addr),
    )
    .await
    {
        Err(_) => return (RelayProbeKind::Unreachable, format!("{addr} 连接超时")),
        Ok(Err(e)) => return (RelayProbeKind::Unreachable, format!("{addr}: {e}")),
        Ok(Ok(s)) => s,
    };
    // 首行格式与真拨一模一样（`GSRL1 <token> <64hex>\n`），否则测的就不是同一条判据。
    let preamble = crate::network::transport::relay_preamble(token, &probe_channel_hex());
    let mut w = stream;
    if let Err(e) = w.write_all(preamble.as_bytes()).await {
        // 连上了却写不出去 = 对方在我们写之前就把写半关了 —— 那正是"首行被拒"的形状。
        return (RelayProbeKind::Rejected, format!("首行写不出去: {e}"));
    }
    match tokio::time::timeout(RELAY_PROBE_HOLD, w.read(&mut [0u8; 1])).await {
        // 到点还没被关 ⇒ 首行已被接受、被挂进等待表。这就是要的"服务器可达 + 口令正确"。
        Err(_) => (
            RelayProbeKind::Held,
            "首行已被服务器接受（探测窗口内未被关闭）".to_string(),
        ),
        // 立刻 EOF / 报错 ⇒ 走了那个 `reject()` 的 destroy()。
        Ok(Err(e)) => (RelayProbeKind::Rejected, format!("首行写出后立刻断开: {e}")),
        Ok(Ok(0)) => (
            RelayProbeKind::Rejected,
            "首行写出后服务器立刻关闭连接".to_string(),
        ),
        // 这台服务器不该回任何字节（它没有回复协议）。真回了就如实报并归到"被拒"，
        // 而不是假装看懂了一个新协议。
        Ok(Ok(n)) => (
            RelayProbeKind::Rejected,
            format!("收到意外的 {n} 字节应答（本版服务器不该回复）"),
        ),
    }
}

/// 这次探测该试哪些端口（纯函数 —— 判据要能被测试钉住，而"试哪几个端口"恰恰是
/// 唯一**不该**依赖网络事实的部分：候选表里那个 443 在开发机上可能被本地代理占着）。
///
/// 返回 `None` = 地址形状本身不可用（缺端口 / 端口不是数字）。
pub fn relay_probe_ports(server_norm: &str, explicit_port: bool) -> Option<Vec<u16>> {
    let (_, port_s) = server_norm.rsplit_once(':')?;
    if explicit_port {
        Some(vec![port_s.parse::<u16>().ok()?])
    } else {
        Some(RELAY_PROBE_PORTS.to_vec())
    }
}

/// 探测 = 按候选端口逐个试，第一个**不是**"连不上"的结论就是答案。
///
/// 为什么"被拒"也停止试下一个端口：被拒说明这个地址上确实有一台 Gosslan 中转
/// （它按我们的首行格式做了判定），换端口只会拿到同一个口令错；继续试反而会把
/// "口令不对"稀释成"三个端口都连不上"。
pub async fn probe_relay_server(
    server_norm: &str,
    token: &str,
    explicit_port: bool,
) -> RelayProbeReport {
    use crate::network::transport::RelayProbeKind;
    let unreachable = |detail: String| RelayProbeReport {
        kind: RelayProbeKind::Unreachable.as_str().to_string(),
        server: server_norm.to_string(),
        tried: String::new(),
        detail,
    };
    let ip = match server_norm.rsplit_once(':') {
        Some((ip, _)) => ip,
        None => return unreachable("地址缺少端口（规范化后应为 ip:port）".to_string()),
    };
    let Ok(base) = ip.parse::<std::net::Ipv4Addr>() else {
        return unreachable("地址不是 IPv4 字面量".to_string());
    };
    let Some(ports) = relay_probe_ports(server_norm, explicit_port) else {
        return unreachable(format!("{server_norm} 的端口不是合法数字"));
    };

    let mut tried: Vec<String> = Vec::new();
    let mut last_detail = String::new();
    for port in ports {
        let addr = std::net::SocketAddr::from((base, port));
        let (kind, detail) = probe_relay_addr(addr, token).await;
        let label = addr.to_string();
        tried.push(label.clone());
        if kind != RelayProbeKind::Unreachable {
            return RelayProbeReport {
                kind: kind.as_str().to_string(),
                server: label,
                tried: tried.join(", "),
                detail,
            };
        }
        last_detail = detail;
    }
    RelayProbeReport {
        kind: RelayProbeKind::Unreachable.as_str().to_string(),
        server: tried.first().cloned().unwrap_or_else(|| server_norm.to_string()),
        tried: tried.join(", "),
        // 三个端口都连不上时只报最后一个错就够了：根因通常在网络侧（安全组 / IP 写错），
        // 不在端口侧 —— 全列出来反而让人以为"再换个端口就行"。
        detail: last_detail,
    }
}

/// 保存前先真拨一次，告诉用户是哪一类失败（要求 2 的三档）。
///
/// 它**不改任何配置**：判"这地址能不能用"是瞬时事，写库由 `save_relay_config` 负责。
/// 界面上顺序是先存后测（探测失败不该让用户填的东西消失），所以这里只读参数。
#[tauri::command(async)]
pub async fn check_relay_server(
    state: State<'_, Arc<AppState>>,
    server: String,
    token: String,
) -> Result<RelayProbeReport, String> {
    let explicit_port = server.rsplit_once(':').is_some_and(|(_, p)| {
        !p.is_empty() && server.trim().contains(':') && p.chars().all(|c| c.is_ascii_digit())
    });
    let server_norm = normalize_relay_server(&server)?;
    let token_norm = validate_relay_token(&token)?;
    let report = probe_relay_server(&server_norm, &token_norm, explicit_port).await;
    // 口令只记长度（与 `save_relay_config` 同一口径：日志会被导出、也可能被截图）。
    state.logger.info(
        "relay",
        format!(
            "探测 kind={} server={} tried={} token_len={}",
            report.kind, report.server, report.tried, token_norm.chars().count()
        ),
    );
    Ok(report)
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

    /// 探测必须把服务器的**两种命运**分开的两档：立刻被关 ⇒ `rejected`；
    /// 被挂在等待表里 ⇒ `held`。这两个用例是 `INTEGRATION.md` §1.1 那纸契约的可执行版本。
    ///
    /// 用真 loopback 监听而不是 mock：判据本身就是"对端关没关 socket"，
    /// 拿 mock 替掉它就成了"我测了我编的接口"。
    #[tokio::test]
    async fn probe_splits_the_two_server_fates() {
        use tokio::io::AsyncReadExt;

        // ① 读完首行立刻关 ⇒ 那就是服务器侧 `reject()` 的 destroy()
        let reject = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_a = reject.local_addr().unwrap();
        let h1 = tokio::spawn(async move {
            let (mut s, _) = reject.accept().await.unwrap();
            let mut buf = [0u8; 128];
            let _ = s.read(&mut buf).await;
            drop(s); // 立刻关
        });
        let ra = probe_relay_server(&addr_a.to_string(), "hunter2", true).await;
        assert_eq!(ra.kind, "rejected", "首行后被立刻关必须判 rejected：{:?}", ra.detail);
        assert_eq!(ra.server, addr_a.to_string());
        h1.abort();

        // ② 接受并**留着**连接（真服务器把它挂在等待表里 30s）⇒ held
        let hold = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_b = hold.local_addr().unwrap();
        let h2 = tokio::spawn(async move {
            let (s, _) = hold.accept().await.unwrap();
            // 不读也不关：留着。探测自己的 2.5s 窗口到点就该判"首行已被接受"。
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(s);
        });
        let rb = probe_relay_server(&addr_b.to_string(), "hunter2", true).await;
        assert_eq!(rb.kind, "held", "被挂着必须判 held：{:?}", rb.detail);
        assert_eq!(rb.server, addr_b.to_string(), "held 时要把这个地址回给界面回填");
        h2.abort();

        // ③ 没人监听 ⇒ unreachable（loopback 上会立刻 RST，不依赖超时）
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_c = dead.local_addr().unwrap();
        drop(dead);
        let rc = probe_relay_server(&addr_c.to_string(), "hunter2", true).await;
        assert_eq!(rc.kind, "unreachable");
    }

    /// 口令**绝不能**出现在探测报告里（要求 7：不进日志、不进诊断面板的截图字段）。
    #[tokio::test]
    async fn probe_report_never_carries_the_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            // 读完再关，制造 rejected 那一档（它的 detail 最长，最容易漏字）
            let mut buf = [0u8; 256];
            let _ = tokio::io::AsyncReadExt::read(&mut s, &mut buf).await;
            drop(s);
        });
        let secret = "SUPE-R-SECRET-9f2c";
        let r = probe_relay_server(&addr.to_string(), secret, true).await;
        for field in [&r.kind, &r.server, &r.tried, &r.detail] {
            assert!(!field.contains(secret), "报告字段里出现了口令：{field}");
        }
        h.abort();
    }

    /// 首行必须留得下**最长合法口令**：服务器对整行有 256 字节闸门，超了就是
    /// `[reject] 首行超长` 那种"两边都看不出根因"的失败。这条把上限算死在测试里，
    /// 免得哪天有人把 `RELAY_TOKEN_MAX_LEN` 调大而没人注意。
    #[test]
    fn longest_allowed_token_still_fits_the_server_line_limit() {
        let token = "x".repeat(RELAY_TOKEN_MAX_LEN);
        let line = format!("GSRL1 {token} {}", "ab".repeat(32));
        assert!(
            line.len() <= 256,
            "首行 {} 字节，超过服务器 256 字节闸门：上限该压到 {} 才对",
            line.len(),
            256 - "GSRL1  \n".len() - 64,
        );
    }

    /// 端口没显式给时才试候选表；给了就**只试那一个**（用户已经给了答案，再去猜会把
    /// "端口不通"这件本该立刻看见的事稀释成三个端口的失败）。
    ///
    /// 这条刻意做成**纯函数**用例：候选表里有 443，开发机上它可能被本地代理占着，
    /// 拿真连接测"试了哪几个端口"会得到一房过一房不过的结果。
    #[test]
    fn explicit_port_is_not_second_guessed() {
        assert_eq!(
            relay_probe_ports("203.0.113.9:60500", true).unwrap(),
            vec![60500],
            "显式端口不许被候选表稀释"
        );
        assert_eq!(
            relay_probe_ports("203.0.113.9:59993", false).unwrap(),
            RELAY_PROBE_PORTS.to_vec(),
            "只填 IP 时才把候选表试完"
        );
        // 默认端口必须在候选表第一位：绝大多数部署没改过端口，先试它最省一轮
        assert_eq!(RELAY_PROBE_PORTS[0], RELAY_DEFAULT_PORT);
        // 形状坏的值一律判"没法试"，而不是悄悄退回默认端口（那会把配置错误变成"连不上"）
        assert_eq!(relay_probe_ports("203.0.113.9:notaport", true), None);
        assert_eq!(relay_probe_ports("203.0.113.9", true), None);
    }
}
