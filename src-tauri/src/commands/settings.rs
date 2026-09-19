// 职责边界：
// - 设置持久化（get/set_settings）
// - 语言切换、蓝牙权限、恢复默认
// ---------------- 应用偏好设置（本地持久化） ----------------

/// 应用偏好：外观、网卡选择等。持久化到本地 SQLite，重启后恢复。
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme_color: Option<String>,
    pub font_family: Option<String>,
    pub dark_mode: Option<bool>,
    /// 外观模式："system" | "light" | "dark"。缺省视为 "system"（跟随系统）。
    /// 与 `dark_mode` 的关系：`appearance_mode` 是**用户意图**，`dark_mode` 是**解析后的结果**
    /// （跟随系统时由前端按系统偏好解析后回写），二者同时持久化，互不冲突。
    pub appearance_mode: Option<String>,
    /// 桌面通知开关（缺省视为开启——否则用户会漏消息且不知道有开关）。
    pub notify_enabled: Option<bool>,
    /// 通知是否显示消息正文（隐私：关掉后只显示"收到新消息"，锁屏/通知中心不泄内容）。
    pub notify_show_content: Option<bool>,
    /// 界面语言："zh-CN" | "en-US"。缺省视为 "zh-CN"。
    pub language: Option<String>,
    pub bind_ip: Option<String>,
    /// 聊天显示样式 JSON：{"preset":"classic","fontSize":"md","compact":true}
    pub chat_style: Option<String>,
    /// 对端样式表 JSON（device_id -> style JSON）。仅由后端在收到 ChatStyle 消息时写入，
    /// 前端只读；save_settings 忽略该字段。
    pub peer_styles: Option<String>,
    /// 中继授权策略："off" | "friends" | "allowlist" | "all"。
    ///
    /// 缺省/脏值 = `all` —— **与今天的行为完全一致**（多跳转发一直是无条件的）。
    /// 为什么默认不是更"安全"的 off：跨跳投递（A—B—C 且 A/C 无直连）依赖中间节点转发，
    /// 默认关掉会让已有拓扑静默丢消息（红线 §8.1 #3：不得在重构里顺手改变传播语义）。
    /// 想限制中继的用户在设置里显式选择。见 `mesh/relay_policy.rs` 与 ADR-0016。
    pub relay_policy: Option<String>,
    /// 中继白名单（JSON 字符串数组，`allowlist` 策略用）。
    pub relay_allowlist: Option<String>,
}

/// e2ee_enabled 键保留在 reset 链中仅为清理 v0.10.0 及更早版本的残留值；
/// v0.11.0 起 E2EE 恒开、不可关闭，该键不再被读写。
const SETTINGS_KEYS: [&str; 13] = [
    "theme_color",
    "font_family",
    "dark_mode",
    "appearance_mode",
    "notify_enabled",
    "notify_show_content",
    "language",
    "bind_ip",
    "chat_style",
    "e2ee_enabled",
    "lan_enabled",
    "relay_policy",
    "relay_allowlist",
];

/// 把「变了的键」读成前端可以直接应用的一小块快照（键名与 `Settings` 的 camelCase 一致）。
///
/// 为什么要有它：`settings-changed` 以前是无载荷事件，接收方只能整份重拉
/// （`get_settings` + `get_device_info` + `get_share_dir`）。带上这一小块之后，
/// 另一个窗口**零 IPC** 就能把界面改对 —— 用户要求"界面响应速度高于一切"，
/// 跨窗口这条路径同样适用。
///
/// **调用约定**：请在**持有 db 锁时**调用，把结果交给
/// `AppState::notify_settings_changed(changed, origin, values)` —— 那里刻意不再自己加锁
/// （std Mutex 不可重入，否则与持锁调用点死锁）。
///
/// 只处理"值能放进 `Settings` 形状里"的键；`nickname`/`avatar`/`shareDir`/`downloadsDir`
/// 不在 `Settings` 里（它们是资料/目录），前端看到这些键会各自做一次**定向**重拉。
pub fn settings_patch_values(db: &rusqlite::Connection, changed: &[&str]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for key in changed {
        match *key {
            "themeColor" => {
                map.insert(key.to_string(), json!(db::get_setting(db, "theme_color")));
            }
            "fontFamily" => {
                map.insert(key.to_string(), json!(db::get_setting(db, "font_family")));
            }
            "darkMode" => {
                map.insert(
                    key.to_string(),
                    json!(db::get_setting(db, "dark_mode").map(|v| v == "1")),
                );
            }
            "appearanceMode" => {
                map.insert(
                    key.to_string(),
                    json!(db::get_setting(db, "appearance_mode")),
                );
            }
            // 通知两项的缺省是**开**（与 `get_settings` 同口径），否则"没设置过"会被应用成关闭
            "notifyEnabled" => {
                map.insert(
                    key.to_string(),
                    json!(db::get_setting(db, "notify_enabled")
                        .map(|v| v != "0")
                        .unwrap_or(true)),
                );
            }
            "notifyShowContent" => {
                map.insert(
                    key.to_string(),
                    json!(db::get_setting(db, "notify_show_content")
                        .map(|v| v != "0")
                        .unwrap_or(true)),
                );
            }
            "language" => {
                map.insert(key.to_string(), json!(db::get_setting(db, "language")));
            }
            "bindIp" => {
                map.insert(key.to_string(), json!(db::get_setting(db, "bind_ip")));
            }
            "chatStyle" => {
                map.insert(key.to_string(), json!(db::get_setting(db, "chat_style")));
            }
            "peerStyles" => {
                map.insert(
                    key.to_string(),
                    json!(db::get_setting(db, "chat_peer_styles")),
                );
            }
            "relayPolicy" => {
                map.insert(key.to_string(), json!(db::get_setting(db, "relay_policy")));
            }
            "relayAllowlist" => {
                map.insert(
                    key.to_string(),
                    json!(db::get_setting(db, "relay_allowlist")),
                );
            }
            "retentionDays" => {
                map.insert(key.to_string(), json!(db::get_setting(db, RETENTION_KEY)));
            }
            "maxBytes" => {
                map.insert(key.to_string(), json!(db::get_setting(db, MAX_BYTES_KEY)));
            }
            // nickname/avatar/shareDir/downloadsDir：不在 `Settings` 形状里，前端定向重拉。
            _ => {}
        }
    }
    serde_json::Value::Object(map)
}

/// appearance_mode 的合法取值：脏值一律忽略（宁可回落"跟随系统"，也不要写进库）。
const APPEARANCE_MODES: [&str; 3] = ["system", "light", "dark"];

/// language 的合法取值：脏值一律忽略（回落"跟随系统"）。
/// "system" = 前端按系统语言决定（zh* → 中文，其余 → 英文）。
const LANGUAGES: [&str; 3] = ["system", "zh-CN", "en-US"];

/// relay_policy 的合法取值（与 `mesh::relay_policy::RelayPolicy::as_str` 一一对应）。
const RELAY_POLICIES: [&str; 4] = ["off", "friends", "allowlist", "all"];

/// 把前端**解析后**的界面语言推给后端。
///
/// 两个用途：
/// 1. 重建 macOS 菜单栏 —— 原生控件的文案不归 WebView 管；
/// 2. **后端自己生成的文案**（群成员变更 / 文件下载 / 托盘提示 / 窗口标题）按它选语言
///    （见 `AppState::is_zh`）。「跟随系统」的解析规则只在前端有一份，后端的兜底判断在
///    Windows 上恒为「否」—— 用户 2026-09-16 实测的「加群提示是英文」就是这个缺口。
///
/// 前端在**启动完成**与每次切换语言时各推一次（启动那次在 `app.init()` 里，不能被
/// `if (has("language"))` 挡住：从没改过语言的用户库里根本没这个键）。
///
/// 非 macOS 平台下菜单模块整体不编译，所以这里必须 cfg 掉那部分（保留命令本身，
/// 让前端调用在其它平台也能拿到 Ok —— 前端不需要按平台分支）。
#[tauri::command(async)]
pub fn set_ui_language(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    lang: String,
) -> Result<(), String> {
    // 先记下解析结果：它同时是 macOS 菜单与后端文案的语言依据。
    state.set_ui_language_hint(&lang);
    #[cfg(target_os = "macos")]
    {
        crate::menu::apply(&app, crate::menu::UiLang::parse(&lang)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &lang;
    }
    // ⚠️ 这里**刻意不发** `settings-changed`。
    //
    // 以前它发（无载荷、广播），于是形成过一个**事件乒乓**：另一个窗口收到 → 重拉设置 →
    // `applySettingsSnapshot` 结尾无条件 `pushUiLanguage()` → 又调回这条命令 → 再发一次
    // ⇒ 两个窗口互相触发，高频 IPC 环（"界面响应速度高于一切"最怕这个）。
    // 语言变更的**事实来源**是 `save_settings`（前端每次切语言都会调它，patch 里带
    // language 的值），所以这条命令不负责广播，只做上面两件事。
    let _ = &app;
    Ok(())
}

/// 申请 Android 的运行时权限（「附近的设备」：蓝牙扫描/连接/广播 + 附近的 WiFi 设备）。
///
/// 为什么要有这条命令：Android 12+ 把这些权限都拆成**运行时**权限，不申请的话
/// 局域网发现收不到组播、蓝牙通道也打不开 —— 用户第一次装完必须手动去系统设置里开，
/// 体验很差。现在 App 启动时前端调一次（系统弹框），被拒时提示"去系统设置打开"。
///
/// 非 Android 平台是**空操作**（这些权限在 macOS/iOS/Windows 上不存在或安装即授予）。
#[tauri::command(async)]
pub fn request_ble_permissions() -> Result<(), String> {
    #[cfg(all(target_os = "android", feature = "bluetooth"))]
    {
        return crate::transport::ble_android::request_permissions();
    }
    #[cfg(not(all(target_os = "android", feature = "bluetooth")))]
    {
        Ok(())
    }
}

/// 前端把 JS 异常 / 未处理的 Promise 拒绝送到后端日志。
///
/// 为什么需要它：界面上"点了没反应"最常见的原因就是**一次 JS 异常**
/// （在渲染或事件处理里抛出后，整个交互看起来就死了），而前端异常此前**不留任何痕迹** ——
/// 用户只能描述成"卡住了"，我们无从下手。现在它会出现在「运行日志」里，可以复制给我们。
#[tauri::command(async)]
pub fn log_frontend_error(state: State<'_, Arc<AppState>>, kind: String, text: String) {
    let text: String = text.chars().take(2000).collect();
    state
        .inner()
        .logger
        .warn("ui", format!("[前端 {kind}] {text}"));
}

#[tauri::command(async)]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    Settings {
        theme_color: db::get_setting(&dbc, "theme_color"),
        font_family: db::get_setting(&dbc, "font_family"),
        dark_mode: db::get_setting(&dbc, "dark_mode").map(|v| v == "1"),
        appearance_mode: db::get_setting(&dbc, "appearance_mode"),
        // 通知默认开启、默认显示正文：缺省时按 `Some(true)`，旧记录与未设置都能有合理行为。
        notify_enabled: db::get_setting(&dbc, "notify_enabled")
            .map(|v| v != "0")
            .or(Some(true)),
        notify_show_content: db::get_setting(&dbc, "notify_show_content")
            .map(|v| v != "0")
            .or(Some(true)),
        language: db::get_setting(&dbc, "language"),
        relay_policy: db::get_setting(&dbc, "relay_policy"),
        relay_allowlist: db::get_setting(&dbc, "relay_allowlist"),
        bind_ip: db::get_setting(&dbc, "bind_ip"),
        chat_style: db::get_setting(&dbc, "chat_style"),
        peer_styles: db::get_setting(&dbc, "chat_peer_styles"),
    }
}

#[tauri::command(async)]
pub fn save_settings(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    settings: Settings,
) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    // 一边写一边记"哪些键真的被这次调用写了" —— 事件只带这一小块（见 `SettingsPatch`）。
    let mut changed: Vec<&str> = Vec::new();
    if let Some(v) = settings.theme_color {
        db::set_setting(&dbc, "theme_color", &v).map_err(|e| e.to_string())?;
        changed.push("themeColor");
    }
    if let Some(v) = settings.font_family {
        db::set_setting(&dbc, "font_family", &v).map_err(|e| e.to_string())?;
        changed.push("fontFamily");
    }
    if let Some(v) = settings.dark_mode {
        db::set_setting(&dbc, "dark_mode", if v { "1" } else { "0" }).map_err(|e| e.to_string())?;
        changed.push("darkMode");
    }
    if let Some(v) = settings.appearance_mode {
        if APPEARANCE_MODES.contains(&v.as_str()) {
            db::set_setting(&dbc, "appearance_mode", &v).map_err(|e| e.to_string())?;
            changed.push("appearanceMode");
        }
    }
    if let Some(v) = settings.notify_enabled {
        db::set_setting(&dbc, "notify_enabled", if v { "1" } else { "0" })
            .map_err(|e| e.to_string())?;
        changed.push("notifyEnabled");
    }
    if let Some(v) = settings.notify_show_content {
        db::set_setting(&dbc, "notify_show_content", if v { "1" } else { "0" })
            .map_err(|e| e.to_string())?;
        changed.push("notifyShowContent");
    }
    if let Some(v) = settings.language {
        if LANGUAGES.contains(&v.as_str()) {
            db::set_setting(&dbc, "language", &v).map_err(|e| e.to_string())?;
            changed.push("language");
        }
    }
    if let Some(v) = settings.bind_ip {
        db::set_setting(&dbc, "bind_ip", &v).map_err(|e| e.to_string())?;
        changed.push("bindIp");
    }
    if let Some(v) = settings.chat_style {
        db::set_setting(&dbc, "chat_style", &v).map_err(|e| e.to_string())?;
        changed.push("chatStyle");
    }
    // 中继授权：脏值一律忽略（宁可维持现状，也不要写进库让传播语义变得不可预期）
    if let Some(v) = settings.relay_policy.as_deref() {
        if RELAY_POLICIES.contains(&v) {
            db::set_setting(&dbc, "relay_policy", v).map_err(|e| e.to_string())?;
            changed.push("relayPolicy");
        }
    }
    if let Some(v) = settings.relay_allowlist.as_deref() {
        // 只接受合法 JSON 数组：写进脏值会让 RelayConfig::parse 静默退化成空表，
        // 用户会看到"白名单明明填了却不生效"。
        if serde_json::from_str::<Vec<String>>(v).is_ok() {
            db::set_setting(&dbc, "relay_allowlist", v).map_err(|e| e.to_string())?;
            changed.push("relayAllowlist");
        }
    }
    // patch 在**持锁时**读好（见 settings_patch_values 的调用约定：那边不再自己加锁，
    // 否则与这里仍持有的锁死锁）。
    let patch = settings_patch_values(&dbc, &changed);
    // 回读一遍写进内存缓存（转发路径热读，不能每次去锁 SQLite）。
    // ⚠️ 先放掉 DB 锁再更新缓存：避免与转发路径形成锁顺序纠缠。
    let relay = crate::mesh::relay_policy::RelayConfig::parse(
        db::get_setting(&dbc, "relay_policy").as_deref(),
        db::get_setting(&dbc, "relay_allowlist").as_deref(),
    );
    drop(dbc);
    state.set_relay_policy_config(relay);
    // 只把**变了的键**发给**另一个窗口**（发起窗口自己已经应用过了，不回发）。
    state.notify_settings_changed(&changed, Some(window.label()), patch);
    Ok(())
}

/// 恢复默认设置：清除所有用户可配置设置（外观、昵称、头像、网卡、缓存策略等）。
/// 保留 device_id、x25519_secret、ed25519_secret、好友列表、聊天记录。
#[tauri::command(async)]
pub fn reset_settings(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    let dbc = state.inner().db.lock().unwrap_or_else(|e| e.into_inner());
    for key in SETTINGS_KEYS.iter().chain([
        &RETENTION_KEY,
        &MAX_BYTES_KEY,
        &"bt_enabled",
        &"chat_peer_styles",
        &"nickname",
        &"avatar",
        // 左栏「外部链接」也属于用户配置：恢复默认一并清掉。
        &EXTERNAL_LINKS_KEY,
    ]) {
        db::delete_setting(&dbc, key).map_err(|e| e.to_string())?;
    }
    // 「恢复默认」也清掉了 relay_policy / relay_allowlist ⇒ 内存缓存必须回到默认（All），
    // 否则用户点了恢复默认、行为却还是旧的限制策略（要重启才生效）。
    drop(dbc);
    state.set_relay_policy_config(crate::mesh::relay_policy::RelayConfig::default());
    // 「恢复默认」把大部分键**删掉**了（不是写成某个值），逐一送 patch 反而容易漏；
    // 用 `"*"` 明确表示"全量都变了" —— 接收方做一次完整重拉（一次性动作，不心疼）。
    state.notify_settings_changed(&["*"], Some(window.label()), json!({}));
    Ok(())
}

/// 广播本机聊天样式到所有已连接节点（样式变更即调用，对方设备与好友同步收到）。
#[tauri::command]
pub async fn broadcast_chat_style(
    state: State<'_, Arc<AppState>>,
    style: String,
) -> Result<(), String> {
    let s = state.inner();
    let msg = Message::ChatStyle {
        from: s.device_id.clone(),
        to: None,
        style,
    };
    // 同 update_profile：锁内只克隆发送端，发送在锁外做 —— 否则一条拥塞链路
    // 就能握着全局 links 锁把整个网络层（含自愈用的看门狗）拖死。
    let targets = {
        let links = s.links.lock().await;
        links
            .values()
            .flatten()
            .map(|link| link.normal.clone())
            .collect::<Vec<_>>()
    };
    for tx in &targets {
        let _ = tx.send(msg.clone()).await;
    }
    Ok(())
}
