// 职责边界：
// - `lib_tests.rs` 测试分册之2 —— 窗口面：capability 标签、辅助窗口单例/串行、Cmd-W、外链协议门
// 为什么拆：`src-tauri/src/lib_tests.rs` 原来 4,009 行、96 个顶层项挤在同一个 `mod tests` 里。
// 机制与 `network/transport/tests.rs` 那一刀同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::lib_tests` 里那同一个 `mod tests` ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ **本册必须与 `lib_tests.rs` 同级平铺**（不许挪进 `lib_tests/` 子目录）：册里到处是
// `include_str!("commands.rs")`、`include_str!("../gen/android/…")` 这类**相对本文件**的路径，
// 换目录会让它们整体偏移一位 —— 编译器会拒（不会静默），但那就不再是"逐字未变"的搬家，
// 恒等判据当场降级成"我读过觉得没变"。同目录先例：`protocol_tests.rs`。
// ⚠️ 册名以 `_tests.rs` 结尾也是被判据选定的：`transport_src_for_guards()` 那份"生产码全集"视图
// 只许装生产码，而登记对账守卫按 `_tests.rs` 后缀豁免测试分册（并进视图是最顺手却假绿的消红办法）。
    /// capability 的 `windows` 模式匹配。Tauri 内部用 glob；本项目只需要支持
    /// 全匹配 / `前缀*` / `*后缀` 三种写法（够用且不引入新依赖）。
    fn window_pattern_matches(pattern: &str, label: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return label.starts_with(prefix);
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            return label.ends_with(suffix);
        }
        pattern == label
    }

    /// 匹配器本身不许"永远为真"（否则下面的守卫会变成空转）。
    #[test]
    fn window_pattern_matcher_is_not_vacuous() {
        assert!(window_pattern_matches("main", "main"));
        assert!(!window_pattern_matches("main", "settings"));
        assert!(window_pattern_matches("*", "settings"));
        assert!(window_pattern_matches("set*", "settings"));
        assert!(window_pattern_matches("*ings", "settings"));
        assert!(!window_pattern_matches("settings", "settings-extra"));
    }

    /// 每一个会创建窗口的标签都必须被 capability 覆盖 —— 否则那个窗口的
    /// `plugin:*` / `core:*` 调用会被 ACL 拒绝（本项目没有 app ACL manifest，
    /// 自定义命令不校验，所以症状是"设��窗口里选目录/开链接/订阅事件静默失败"）。
    #[test]
    fn capability_covers_every_window_label() {
        let raw = include_str!("../capabilities/default.json");
        let caps: serde_json::Value =
            serde_json::from_str(raw).expect("capabilities/default.json 必须是合法 JSON");
        let patterns: Vec<String> = caps
            .get("windows")
            .and_then(|v| v.as_array())
            .expect("capabilities/default.json 缺少 windows 数组")
            .iter()
            .map(|v| v.as_str().expect("windows 数组项必须是字符串").to_string())
            .collect();
        // 防"清单被删空 ⇒ 循环空转"：固定窗口至少这三个。
        assert!(
            WINDOW_LABELS.len() >= 3,
            "WINDOW_LABELS 至少要有 main/settings/logs 三个"
        );
        // 群任务窗口现在是**固定** label（`tasks`），已经在 WINDOW_LABELS 里 ⇒ 不再有动态通配要验。
        for label in WINDOW_LABELS.iter().copied() {
            assert!(
                patterns.iter().any(|p| window_pattern_matches(p, label)),
                "窗口 `{label}` 没有被任何 capability 覆盖（windows = {patterns:?}）"
            );
        }
    }

    /// **反向**守卫：外链窗口**必须不被**任何 capability 覆盖。
    ///
    /// 它加载的是远端页面；一旦被 capability 覆盖，第三方内容就能调用本应用的
    /// `plugin:*` / `core:*`（对话框、打开器、事件……）—— 等于把命令面交出去。
    /// 与上一条互为镜像：上一条保证"该覆盖的都被覆盖"，这条保证"不该覆盖的没被覆盖"。
    #[test]
    fn link_window_is_not_capability_covered() {
        let raw = include_str!("../capabilities/default.json");
        let caps: serde_json::Value =
            serde_json::from_str(raw).expect("capabilities/default.json 必须是合法 JSON");
        let patterns: Vec<String> = caps
            .get("windows")
            .and_then(|v| v.as_array())
            .expect("capabilities/default.json 缺少 windows 数组")
            .iter()
            .map(|v| v.as_str().expect("windows 数组项必须是字符串").to_string())
            .collect();
        assert!(
            !patterns
                .iter()
                .any(|p| window_pattern_matches(p, WINDOW_LINK)),
            "外链窗口 `{WINDOW_LINK}` 不得被任何 capability 覆盖（远端页面会因此拿到命令面）：{patterns:?}"
        );
        // 同时确认它没被误加进固定窗口清单。
        assert!(
            !WINDOW_LABELS.contains(&WINDOW_LINK),
            "WINDOW_LINK 不该进 WINDOW_LABELS（那份清单是 capability 覆盖的正向清单）"
        );
    }

    /// **规则式**守卫：任何会碰重资源（数据库 / 文件系统 / 日志 / 剪贴板 / 网卡枚举 / 阻塞睡眠）
    /// 的命令，都必须声明 `#[tauri::command(async)]`。
    ///
    /// 依据（读过上游源码，不是猜的）：
    /// - `tauri-macros` 的 `body_blocking` 把命令函数**内联调用**在 IPC 处理器里；
    ///   只有 `ExecutionContext::Async` 才走 `respond_async_serialized`
    ///   → `crate::async_runtime::spawn(...)`（异步运行时线程）。
    /// - wry 的 `WKScriptMessageHandler::did_receive` 在 AppKit 消息循环里同步回调
    ///   （`wry-*/src/wkwebview/class/wry_web_view_delegate.rs`），即 **macOS 主线程**。
    ///
    /// 于是同步命令 = 在 UI 主线程上跑：**整个进程**（所有窗口）都会卡住，不只是发起调用的那个窗口。
    ///
    /// 为什么从"名字清单"改成"规则"：清单只能盯住写清单时想到的那几个。
    /// 真实事故（2026-09-12 用户反馈"点清除数据/恢复，设置窗口直接卡死；点添加好友主窗口卡死"）：
    /// 清单里 12 个命令是 async，但**另外 42 个**（`get_settings`/`get_friends`/`get_transfers`/
    /// `get_logs`/`list_interfaces`/`reset_settings`/`open_settings_window` …）仍是同步命令。
    /// 它们本身很快，但 `clear_all_data` 那种长事务会把 `db` 互斥锁握住数秒，
    /// 于是这些同步读**在主线程上等锁** ⇒ 两个窗口一起冻住。
    /// 规则式守卫能覆盖"以后新加的命令"，名字清单不能。
    #[test]
    fn blocking_commands_run_off_the_main_thread() {
        let src = all_commands_src();
        // ⚠️ 例外清单已清空（2026-09-20）：v4.22.2 曾把 4 个开窗命令登记成例外（理由是
        // "macOS 必须主线程调 AppKit"），结果在 Windows 上换来了更严重的故障 ——
        // 同步命令在 IPC 回调里内联跑主线程，而 Windows 建 WebView2 会在调用线程里泵消息
        // ⇒ 与 IPC 重入 ⇒ 持锁自锁 ⇒ 整个界面永久无响应。macOS 的约束改由
        // `commands::logs::decorate_aux_window` 把 AppKit 调用单独投回主线程解决，
        // 不再需要任何例外。

        // 重资源标记 → 人类可读的原因
        // `tray::` 标记（审计 2.2j）：托盘更新 = 整张图标逐像素混合 + 平台角标重绘，
        // 同步命令里内联跑等于在主线程 IPC 回调里做 CPU/平台活（macOS 主线程同时
        // 驱动整个 UI 事件循环）。set_unread_badge 曾因此漏网——字面量 marker
        // 匹配不到跨模块调用，本标记补上这个洞。
        let markers: [(&str, &str); 10] = [
            (".db", "访问 SQLite（可能等锁数秒）"),
            ("db::", "访问 SQLite"),
            ("std::fs", "文件系统 IO"),
            ("logger.", "日志（含整份快照）"),
            ("Clipboard", "系统剪贴板（可能被别的程序占着）"),
            ("list_interfaces", "枚举网卡"),
            ("if_addrs", "枚举网卡"),
            ("thread::sleep", "阻塞睡眠"),
            ("block_on", "阻塞等待异步任务"),
            ("tray::", "托盘图标重绘（逐像素混合 + set_icon/badge）"),
        ];

        // 建窗命令单独判：`open_*_window` 的函数体里没有上面那些重资源标记（db 访问都在
        // 已 try_lock 的辅助函数里），光靠 markers 抓不到它 —— 而它恰恰是最不能同步的一条。
        let window_markers = ["ensure_aux_window(", "WebviewWindowBuilder::new("];

        let mut offenders: Vec<String> = Vec::new();
        for (name, is_async, body) in command_bodies(src) {
            if is_async {
                continue;
            }
            let hit: Vec<&str> = markers
                .iter()
                .filter(|(m, _)| body.contains(m))
                .map(|(_, why)| *why)
                .collect();
            if !hit.is_empty() {
                offenders.push(format!("  {name}: {}", hit.join("、")));
            }
            if window_markers.iter().any(|m| body.contains(m)) {
                offenders.push(format!(
                    "  {name}: 创建窗口 —— Windows 上主线程内联建 WebView2 会泵消息、\
                     与 IPC 回调重入 ⇒ 永久挂死（见 commands/logs.rs 的 ensure_aux_window）"
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "以下命令会阻塞 macOS 主线程（同步命令在 wry 的 IPC 回调里内联执行），\
             必须加 `#[tauri::command(async)]` —— 否则长事务/读盘期间**所有窗口**一起卡死：\n{}",
            offenders.join("\n")
        );
    }

    /// 拆出 `commands.rs` 里每个 `#[tauri::command…]` 的 `(名字, 是否 async, 函数体)`。
    ///
    /// 手写扫描而不是上 syn：本测试只做文本判据，不引入新依赖；字符串/注释/生命周期都跳过，
    /// 否则函数体里的花括号（`format!("{}")` 之类）会让配对错位。
    fn command_bodies(src: &str) -> Vec<(String, bool, String)> {
        let bytes = src.as_bytes();
        let mut out = Vec::new();
        let mut i = 0usize;
        while let Some(pos) = src[i..].find("#[tauri::command") {
            let attr_start = i + pos;
            let attr_end = match src[attr_start..].find(']') {
                Some(e) => attr_start + e,
                None => break,
            };
            // 两条路都算"off main thread"：属性 `#[tauri::command(async)]`（同步函数被 spawn），
            // 或函数本身是 `async fn`（宏直接走 respond_async_serialized）。
            let attr_async = src[attr_start..attr_end].contains("async");
            // 从属性之后找 `fn 名字(`
            let after = &src[attr_end..];
            let fn_pos = match after.find("fn ") {
                Some(p) => attr_end + p,
                None => {
                    i = attr_end;
                    continue;
                }
            };
            let name: String = src[fn_pos + 3..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let is_async = attr_async || src[attr_end..fn_pos].contains("async");
            // 花括号配对取函数体（跳过字符串/字符/注释）
            let mut depth = 0i32;
            let mut j = fn_pos;
            let mut in_str = false;
            let mut in_line_comment = false;
            let mut in_block_comment = false;
            let mut body = String::new();
            while j < bytes.len() {
                let c = bytes[j] as char;
                let next = bytes.get(j + 1).map(|b| *b as char);
                if in_line_comment {
                    if c == '\n' {
                        in_line_comment = false;
                    }
                } else if in_block_comment {
                    if c == '*' && next == Some('/') {
                        in_block_comment = false;
                        j += 1;
                    }
                } else if in_str {
                    if c == '\\' {
                        j += 1;
                    } else if c == '"' {
                        in_str = false;
                    }
                } else if c == '/' && next == Some('/') {
                    in_line_comment = true;
                    j += 1;
                } else if c == '/' && next == Some('*') {
                    in_block_comment = true;
                    j += 1;
                } else if c == '"' {
                    in_str = true;
                } else if c == '\'' {
                    // 生命周期（`'_` / `'a`）不是字符字面量：只有 `'x'` 形式才算
                    if next == Some('\\') || bytes.get(j + 2).map(|b| *b as char) == Some('\'') {
                        j += 2;
                    }
                } else if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if depth > 0 {
                    body.push(c);
                }
                j += 1;
            }
            out.push((name, is_async, body));
            i = j.max(attr_end + 1);
        }
        out
    }

    /// 主窗口标签在 `tray` 里还有一份（那份是 `#[cfg(desktop)]`），两边不许漂移。
    #[cfg(desktop)]
    #[test]
    fn main_window_label_matches_tray_constant() {
        assert_eq!(tray::MAIN_WINDOW_LABEL, WINDOW_MAIN);
    }

    /// **⌘W 必须由我们自己的菜单项处理**（用户 2026-09-13 真机：主窗口 ⌘W 只会"滴滴滴"）。
    ///
    /// 系统预定义项 `PredefinedMenuItem::close_window` 的动作是 `performClose:`，AppKit 按
    /// 窗口的 `Closable` 样式位校验可用性；而本项目**所有应用窗口**都是 `decorations: false`
    /// ⇒ Borderless ⇒ 该项被判不可用 ⇒ 只有系统提示音，且**没有任何日志**。
    /// （2026-09-17 之前只有主窗口是无边框的、辅助窗口靠系统标题栏所以"正常"，更容易让人以为
    /// 是"主窗口特有的 bug"；现在辅助窗口也由 `decorate_aux_window` 补回 `Closable` 位。）
    ///
    /// 判据：窗口菜单不得再用预定义关闭项；必须有一个带 `CmdOrCtrl+W` 的自定义项，
    /// 且菜单事件处理里真的关掉"当前聚焦窗口"（回落到主窗口）。
    #[test]
    fn cmd_w_is_handled_by_our_own_menu_item() {
        let m = include_str!("menu.rs");
        // ⚠️ 先**去掉注释**再查"有没有用错项"：menu.rs 里那段解释恰恰写着这个名字，
        // 不剥注释就会把"解释这个坑"误判成"又踩了这个坑"（本项目已踩过一次这种假阳性）。
        let code: String = m
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("PredefinedMenuItem::close_window"),
            "不许用系统预定义的关闭项 —— 它会被 AppKit 按 `Closable` 样式位判为不可用，\
             表现就是主窗口 ⌘W 只响一声提示音（用户 2026-09-13 真机）"
        );
        assert!(
            m.contains("\"close-window\"") && m.contains("CmdOrCtrl+W"),
            "必须有一个 id=close-window、快捷键 CmdOrCtrl+W 的自定义菜单项"
        );
        let handler = m
            .find("\"close-window\" =>")
            .expect("菜单事件处理里必须接住 close-window（否则点了没反应）");
        let body = &m[handler..(handler + 700).min(m.len())];
        assert!(
            body.contains("is_focused"),
            "关的应当是**当前聚焦**的窗口（与 macOS 原生 ⌘W 语义一致）"
        );
        assert!(
            body.contains(".close()"),
            "走 `close()` 才会触发各窗口自己的 CloseRequested → 隐藏处理器（与「×」一致）"
        );
    }

    /// **独立窗口必须各自一个前端文档与入口**，不许再共用主窗口的 `index.html`。
    ///
    /// 为什么（用户 2026-09-12 实测）：「第二次打开设置，窗口会先刷成主聊天窗口、
    /// 然后立马变成设置界面」+「点一下要等很久」—— 根因就是设置/日志窗口加载的是主窗口的
    /// HTML，前端再按窗口 label 把聊天三栏挂起来换成设置页。窗口 URL 与前端文档的对应关系
    /// 只有这条护栏盯着（前端测试看得到 HTML，看不到 Rust 的 URL）。
    #[cfg(desktop)]
    #[test]
    fn aux_windows_open_their_own_document() {
        let commands = all_commands_src();

        let mut urls: Vec<String> = Vec::new();
        for part in commands.split("WebviewUrl::App(").skip(1) {
            let rest = part.trim_start().trim_start_matches('"');
            let end = rest
                .find('"')
                .unwrap_or_else(|| panic!("WebviewUrl::App 里的字符串没有闭合"));
            urls.push(rest[..end].to_string());
        }
        assert!(
            urls.len() >= 4,
            "应当能找到设置 / 日志 / 群任务 / 图片预览四个窗口的 URL，实际 {}",
            urls.len()
        );
        assert!(
            !urls.iter().any(|u| u == "index.html"),
            "独立窗口不得再共用主窗口的 index.html（那会让它先把聊天三栏挂起来）：{urls:?}"
        );
        // 外链窗口是唯一**不走 App 文档**的窗口（它加载远端 URL），必须真的是 External。
        assert!(
            commands.contains("WebviewUrl::External("),
            "外链窗口必须用 `WebviewUrl::External` 加载远端 URL"
        );

        // 每个 URL 都必须真的存在，且它引用的入口也必须存在 —— 这是"改了文件名忘改另一处"
        // 的唯一防线（前端构建不会因为 Rust 里的字符串写错而失败）。
        for (url, entry) in [
            ("settings.html", "src/entries/settings.ts"),
            ("logs.html", "src/entries/logs.ts"),
            ("todos.html", "src/entries/todos.ts"),
            ("preview.html", "src/entries/preview.ts"),
        ] {
            assert!(
                urls.iter().any(|u| u == url),
                "Rust 侧应当打开 {url}：{urls:?}"
            );
            // 每条 URL 都要显式列出来：`_ =>` 兜底会让新增的窗口"读到别人的 HTML"，
            // 于是下面那条 contains 断言替别人通过 —— 护栏变成空转。
            let html = match url {
                "settings.html" => include_str!("../../settings.html").to_string(),
                "logs.html" => include_str!("../../logs.html").to_string(),
                "todos.html" => include_str!("../../todos.html").to_string(),
                "preview.html" => include_str!("../../preview.html").to_string(),
                _ => unreachable!("新增 url 必须在这里补自己的 include_str!"),
            };
            assert!(
                html.contains(&format!("/{entry}")),
                "{url} 必须加载自己的入口 /{entry}（窗口差异由文档承担，而不是在一个入口里 if/else）"
            );
            // 入口文件真的存在（`include_str!` 让"文件被删/改名"在编译期就报错）
            match entry {
                "src/entries/settings.ts" => {
                    let _ = include_str!("../../src/entries/settings.ts");
                }
                "src/entries/logs.ts" => {
                    let _ = include_str!("../../src/entries/logs.ts");
                }
                "src/entries/todos.ts" => {
                    let _ = include_str!("../../src/entries/todos.ts");
                }
                "src/entries/preview.ts" => {
                    let _ = include_str!("../../src/entries/preview.ts");
                }
                _ => unreachable!("新增 entry 必须在这里补自己的 include_str!"),
            }
        }
    }

    /// **独立窗口的打开必须是"单例 + 串行创建 + 关闭即隐藏"**。
    ///
    /// 为什么（用户 2026-09-12 实测）：
    ///   - 连点两下会**开出第二个窗口**：`WebviewWindowBuilder::build()` 的重复 label 检查在
    ///     `prepare_window` 里做，而窗口被登记进 manager 是在主线程创建完成之后 ——
    ///     两个并发调用会双双通过检查（后者还会覆盖 manager 的记录）。
    ///   - 关闭即销毁 ⇒ 每次打开都要重建 WebView + 重新加载前端 + 重新 `app.init()`，
    ///     这正是"点一下要等很久"。
    #[cfg(desktop)]
    #[test]
    fn aux_window_open_is_singleton_serialized_and_resident() {
        let commands = all_commands_src();

        for signature in [
            "pub fn open_settings_window(",
            "pub fn open_log_window(",
            "pub fn open_link_window(",
            "fn ensure_preview_window(",
            "fn ensure_tasks_window(",
        ] {
            let body = rust_fn_body(commands, signature);
            assert!(
                body.contains("ensure_aux_window("),
                "{signature}…）必须走 `ensure_aux_window`（单例 + 串行创建）——                  自己写 `if let Some(win) = get_webview_window(..)` 会在并发时开出第二个窗口"
            );
            assert!(
                !body.contains("get_webview_window("),
                "{signature}…）不该自己查窗口存在性：那是 `ensure_aux_window` 的职责"
            );
        }

        let helper = rust_fn_body(commands, "fn ensure_aux_window<F>(");
        // 防"锚点过期 ⇒ 拿到空/残段 ⇒ 下面的断言空转"。
        assert!(
            !helper.is_empty(),
            "找不到 `ensure_aux_window` 的函数体（这条护栏会变成空转）"
        );
        assert!(
            helper.contains("AUX_WINDOW_CREATE_LOCK"),
            "`ensure_aux_window` 必须用创建锁串行化（否则并发会开出第二个窗口）"
        );
        assert!(
            // 签名在 2026-09-24 多了一个 `reveal`（预热那条路要"建好但不显示"），
            // 判据本身没变：**拿锁前后各查一次**，少一次就会在并发下开出第二个窗口。
            helper
                .matches("show_existing_aux_window(app, label, geo, reveal)")
                .count()
                >= 2,
            "`ensure_aux_window` 必须做双重检查（拿锁前后各查一次），实际只有一次"
        );
        assert!(
            // 2026-09-17：`resident` 改成参数（设置/日志常驻；群任务窗口每群一个 ⇒ 关闭即销毁）。
            // 判据是"常驻开关接上了 hide-on-close"，不再要求 helper 里出现某个具体常量。
            helper.contains("resident: bool") && helper.contains("install_hide_on_close(&win)"),
            "`ensure_aux_window` 里必须接上「关闭即隐藏」（常驻）——              否则每次打开都要重新加载 WebView + 前端，用户要等（`resident` 是开关）"
        );
        // 2026-09-24 策略翻转（用户："其他新窗口默认不销毁，第二次打开速度优化快点"）：
        // 常驻 = 群任务 / 预览 / 外链；销毁 = 设置 / 日志（这两个每次都要读最新的环境与日志尾部）。
        // 钉四件事，缺一条都是一个具体的回归：
        for (signature, want_resident) in [
            ("const AUX_TASKS_RESIDENT: bool = ", true),
            ("const AUX_PREVIEW_RESIDENT: bool = ", true),
            ("const AUX_LINK_RESIDENT: bool = ", true),
            ("const AUX_WINDOWS_RESIDENT: bool = ", false),
        ] {
            let want = format!("{signature}{want_resident}");
            assert!(
                commands.contains(&want),
                "常驻策略被改动了：{want} —— 改成销毁要连「为什么」一起写进这条注释，别只翻布尔值"
            );
        }
        // 常驻的群任务窗口必须只有**一扇**（固定 label）：动态 label 既需要淘汰逻辑，
        // 也让"不传参数预热"做不到。
        assert!(
            !code_flat(&commands).contains("WINDOW_GROUP_TODOS_PREFIX"),
            "群任务窗口又回到动态 label 了：那样既不累积不了（需要淘汰），也没法不传参数预热"
        );
        // 预热必须是"建好但不显示"：reveal 传 false，且它自己不许出现 show/focus。
        // 少了这条，预热就变成"启动时弹出一扇没内容的预览窗"（比慢更糟）。
        let prewarm = rust_fn_body(commands, "pub fn prewarm_aux_windows(");
        assert!(
            code_flat(&prewarm).contains("ensure_preview_window(&app,&state,false)")
                && code_flat(&prewarm).contains("ensure_tasks_window(&app,&state,\"\",false)"),
            "预热必须把**两扇**高频窗都备好，且都走共用的窗口构造、reveal 传 false"
        );
        assert!(
            !code_flat(&prewarm).contains(".show()")
                && !code_flat(&prewarm).contains(".set_focus()"),
            "预热路径里不许出现 show / set_focus —— 那会变成启动时弹一扇空窗"
        );
        let hide = rust_fn_body(commands, "fn install_hide_on_close(");
        assert!(
            hide.contains("prevent_close()") && hide.contains("hide()"),
            "`install_hide_on_close` 必须是 prevent_close + hide（关闭即隐藏）"
        );
        // ⚠️ 这条只判**函数体**：`rust_fn_body` 的起点是签名，所以上面那段解释"淘汰为什么不能
        // 放在这里"的注释（里面写着 `destroy()`）不会被算进来。
        assert!(
            !code_flat(&hide).contains(".destroy()"),
            "销毁不得发生在窗口事件回调里 —— 那是 wry 的主线程事件循环，\
             本文件已经为此死锁过一次（见 `log_window_body` 的说明）"
        );

        // 尺寸/位置必须走**物理像素**的 setter，不能走 builder 的逻辑坐标 `position()`：
        // `tao` 创建窗口时会把逻辑坐标**逐个显示器**按各自缩放换回物理、取第一个命中的显示器
        // （见 commands.rs 的 `fit_aux_window`），所以多屏不同缩放时子窗口会跑到另一块屏上
        // —— 用户 2026-09-16 实测报的正是这个，且它在单屏上完全看不出来。
        for signature in [
            "pub fn open_settings_window(",
            "pub fn open_log_window(",
            "pub fn open_link_window(",
            "fn ensure_preview_window(",
            "fn ensure_tasks_window(",
        ] {
            let body = rust_fn_body(commands, signature);
            assert!(
                body.contains("apply_aux_geometry("),
                "{signature}…）必须用 `apply_aux_geometry` 在 `build()` 之后按物理像素落地尺寸与位置"
            );
            assert!(
                !body.contains(".position("),
                "{signature}…）不得用 builder 的 `.position()`：它只有逻辑坐标，多屏不同缩放时\n\
                 会被 tao 换算到另一块显示器上（改用 apply_aux_geometry）"
            );
            assert!(
                body.contains(".visible(false)"),
                "{signature}…）必须**隐藏创建**：`ensure_aux_window` 随后才 show，\n\
                 中间这段用来摆位置/尺寸，否则窗口会先在默认位置闪一下再跳过去"
            );
        }
    }

    /// 装饰性数据**不得否决用户的动作**（2026-09-24 复查辅助窗口时找到的同形状缺陷）。
    ///
    /// `open_group_todos_window` 原先在"取群名填系统标题"那一步，拿不到 DB 锁时直接
    /// 拒掉整个开窗 —— 于是 DB 一忙（文件传输的进度写库、批量落库都算），这扇窗**根本
    /// 打不开**，而失败在前端只变成一句 toast，用户读到的是"点了没反应"。
    /// 群名只是任务栏那一点装饰（文档加载后前端会按同样口径再取一次并接管标题），
    /// 为它牺牲一次用户动作是彻头彻尾的错配。
    ///
    /// 判据取**正向形状**两条（必须是 match、拿不到锁那支必须给空串）。刻意不写负向断言：
    /// 本文件读的是原始源码（不剥注释），一句解释"以前错在哪"的注释就会把自己踩红 ——
    /// 同一课今天上午刚踩过一次（`windowEntries` 被"这里绝不调用 chat.init()"那句注释判红）。
    #[test]
    fn cosmetic_title_read_cannot_abort_the_window() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "pub fn open_group_todos_window(");
        let flat = code_flat(&body);
        assert!(
            flat.contains("matchstate.db.try_lock()"),
            "群名那一步必须是 try_lock 的 match（两支都往下走），不能是拿不到锁就中止"
        );
        assert!(
            flat.contains("Err(_)=>String::new()"),
            "拿不到锁的分支必须降级成空群名 —— 这是开窗路径上唯一允许「软失败」的地方"
        );
    }

    /// 外链 URL 的**协议白名单**（用户配置的网址会被 `WebviewUrl::External` 直接加载）。
    ///
    /// 为什么必须表驱动钉死：`javascript:` / `data:` / `file:` / `tauri:` 一旦漏过，
    /// 等于把"在应用 WebView 里执行脚本 / 读本机文件"的能力交给一段用户随手粘贴的字符串。
    #[test]
    fn external_link_rejects_non_http_schemes() {
        use crate::commands::validate_external_url;
        for ok in [
            "http://example.com",
            "https://example.com/a?b=c",
            "  https://a.b.c  ",
        ] {
            assert!(validate_external_url(ok).is_ok(), "应当放行：{ok}");
        }
        for bad in [
            "",
            "   ",
            "example.com",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "tauri://localhost",
            "https://",
        ] {
            assert!(
                validate_external_url(bad).is_err(),
                "必须拒绝非 http/https 或缺少主机名的输入：{bad}"
            );
        }
    }

    /// 群任务窗口是**固定 label** `tasks`，"看哪个群"改由后端那份当前上下文给。
    ///
    /// 为什么钉这个形状（用户 2026-09-24）："可不传参数、后台默默先把 WebView 建好，用的时候
    /// 瞬间激活" —— 动态 label（`todo-<groupId>`）在预热那一刻不知道该建哪一扇，"秒开"
    /// 只能建立在固定 label 上。两头各钉一次：Rust 常量与前端启动器的字面量必须同为 `tasks`
    /// （两份各写一遍必然漂移，这是本仓库反复踩过的一族）。
    #[cfg(desktop)]
    #[test]
    fn tasks_window_uses_one_fixed_label_cross_checked_with_frontend() {
        assert_eq!(WINDOW_TASKS, "tasks");
        assert!(
            WINDOW_LABELS.contains(&WINDOW_TASKS),
            "固定 label 必须进 WINDOW_LABELS（capability 覆盖那条守卫才不会漏看它）"
        );
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "fn ensure_tasks_window(");
        assert!(
            body.contains("crate::WINDOW_TASKS"),
            "窗口 label 必须取自常量，不许在 builder 里另写字面量"
        );
        // groupId 不再是 label 的一部分，但它仍然跨窗口当指令拿去查数据 ⇒ 校验不能省
        let open = rust_fn_body(commands, "pub fn open_group_todos_window(");
        assert!(
            open.contains("is_ascii_alphanumeric()") && open.contains("is_empty()"),
            "groupId 会被别的文档拿去查数据，必须先做非空/字符集校验"
        );
        let ts = include_str!("../../src/composables/useWindowLauncher.ts");
        assert!(
            ts.contains(r#""tasks""#),
            "前端启动器的 label 联合类型必须含 `tasks`（与 Rust WINDOW_TASKS 同一份字面量）"
        );
    }
