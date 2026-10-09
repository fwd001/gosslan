#!/usr/bin/env python3
"""护栏非空转用例分册：桌面装配与安全边界（commands* / lib.rs / db / crypto / capability / storage / 菜单通知）。

本册 26 条 / 464 行，2026-10-07 从 `scripts/verify-guards.py`（原 4,176 行、202 条挤在一份
`CASES` 字面量里）按**锚定的被守物**切出来。块文本逐字未搬动过一字 ⇒
恒等判据＝`verify-guards.py --list` 的输出排序后与拆前**逐字节相同**（条数与用例名都不是"我觉得一样"）。

⚠️ 三条硬规矩（都是这仓自己踩出来的形状）：
1. 加一条护栏就加进**对应这一域**的本册；域由 `file=` 锚点决定，不按"谁方便找"。
2. 分册必须被 `guard_cases/__init__.py` 的 `MODULES` 点名 —— 那里有一条**起跑前就会炸**的对账：
   目录里的模块集合 != 名单 ⇒ `ImportError`。漏点名的后果不是报错而是**那几条护栏从此不跑**（假绿）。
3. `check-doc-numbers.mjs` 的判据 E 现在按 `scripts/guard_cases/*.py` 里的 Case 构造行现算条数，
   与契约图上那一格对账；数到 0 它自己 throw（尺子坏了要比被测物先响）。

路由规则按**路径段**匹配，不许用裸 substring：`TodoCardBubble.vue` 里含 "ble"、`FriendProfile.vue`
里含 "file" —— 第一版就是这么被错分进蓝牙册与文件册的（各 2 条），改成段/主干相等才干净。
"""

from __future__ import annotations

from .base import ROOT, TAURI, Case, cargo, npm

CASES: list[Case] = [
    # ---------------- Rust：主线程阻塞 ----------------
    Case(
        name="主线程守卫（同步命令碰数据库必须报出）",
        why="用户反馈的三个卡死现象就来自这条：同步命令在 macOS 主线程执行，长事务持锁时全部窗口冻住",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "#[tauri::command(async)]\npub fn get_settings",
            "#[tauri::command]\npub fn get_settings",
        )],
        cmd=cargo("test", "--lib", "blocking_commands_run_off_the_main_thread"),
        cwd=TAURI,
        expect_fail_hint="get_settings",
        tags=["rust", "perf"],
    ),
    Case(
        name="开窗命令必须留在工作线程（Windows 同步建 WebView2 = 永久卡死）",
        why="v4.22.2 为修 macOS 的 AppKit 线程问题，把 open_*_window 改成同步命令 —— "
        "Win 端点「设置」立刻整个界面无响应、只能杀进程（用户 2026-09-20 真机）。"
        "机制：同步命令在 wry 的 IPC 回调里**内联跑主线程**，而 Windows 建 WebView2 会在"
        "调用线程里泵消息（tauri-runtime-wry：must be called from a separate thread,"
        " otherwise the channel will introduce a deadlock）⇒ 与正在处理的 IPC 重入 ⇒ "
        "AUX_WINDOW_CREATE_LOCK 同线程二次 lock（std Mutex 不可重入）= 永久自锁。"
        "这条用例证明「把开窗命令改回同步」一定会被护栏拦下",
        file=TAURI / "src" / "commands" / "logs.rs",
        injections=[(
            "#[tauri::command(async)]\npub fn open_settings_window(",
            "#[tauri::command]\npub fn open_settings_window(",
        )],
        cmd=cargo("test", "--lib", "blocking_commands_run_off_the_main_thread"),
        cwd=TAURI,
        expect_fail_hint="创建窗口",
        tags=["rust", "window", "deadlock"],
    ),
    # ---------------- 启动期：数据比本机新（v4.22.36）----------------
    Case(
        name="降级拒绝必须早于任何写操作（「数据未被修改」不许是谎话）",
        why="拒绝降级这件事本身是对的，但判定曾经写在 run_migrations 里 = execute_batch(SCHEMA) "
        "之后：那次 batch 虽是 CREATE TABLE IF NOT EXISTS，却确实写文件 ⇒ 弹窗里"
        "「你的聊天记录没有被修改（迁移在写入任何数据之前就已中止）」成了假话，"
        "而用户正是凭这句话判断「可以放心去装新版本」",
        file=TAURI / "src" / "db.rs",
        injections=[(
            # ⚠️ 锚点只钉"降级判定那两行本身"。原先钉的是
            # `return Err(err);\n }\n conn.execute_batch(SCHEMA)?;` 这一整块，而 #46 把
            # "数表"那一段插到了 `}` 与 `execute_batch` 之间 ⇒ 锚点变成 0 次命中，
            # 这条用例**静默失效**了两周（它在 `--only rust` 的子集里，日常门禁跑不到）。
            '        eprintln!("[gosslan-db] FATAL: {err}");\n        return Err(err);',
            '        eprintln!("[gosslan-db] FATAL: {err}");\n'
            "        conn.execute_batch(SCHEMA).ok();\n"
            "        return Err(err);",
        )],
        cmd=cargo("test", "--lib", "downgrade_refusal_writes_nothing"),
        cwd=TAURI,
        expect_fail_hint="不许建任何表",
        tags=["rust", "boot"],
    ),
    Case(
        name="开机降级弹窗必须非阻塞（blocking_show 在主线程自锁）",
        why="插件桌面实现是 app_handle.run_on_main_thread(...)（desktop.rs:222），而 setup "
        "就跑在主线程上 ⇒ blocking_show 的 rx.recv() 钉住主线程、弹窗任务永远排不到 = "
        "开机白屏死锁，与 v4.22.30 的 Windows 开窗卡死同一个形状。写错不会编译失败，"
        "只会永远看不见那句提示，所以只能机器拦",
        file=TAURI / "src" / "lib.rs",
        injections=[(
            "                            handle\n"
            "                                .dialog()\n"
            "                                .message(msg)\n"
            "                                .title(\"Gosslan\")\n"
            "                                .show(move |_| {\n"
            "                                    // 用户点掉提示之后才退出（exit 走事件循环代理，跨线程安全）\n"
            "                                    handle.exit(1);\n"
            "                                });",
            "                            if handle\n"
            "                                .dialog()\n"
            "                                .message(msg)\n"
            "                                .title(\"Gosslan\")\n"
            "                                .blocking_show()\n"
            "                            {\n"
            "                                std::process::exit(1);\n"
            "                            }",
        )],
        cmd=cargo(
            "test", "--lib", "boot_downgrade_refusal_is_typed_precedes_writes_and_non_blocking"
        ),
        cwd=TAURI,
        expect_fail_hint="blocking_show",
        tags=["rust", "boot", "deadlock"],
    ),
    Case(
        name="Windows 专用分支的编译错误必须本地可拦（ends_with 少 as_str）",
        why="2026-09-14 真实事故：notifications.rs 的 Windows 分支写成 ends_with(format!(...))，"
            "String 未实现 Pattern ⇒ 两个 Windows CI job 全挂；macOS 上该分支被 cfg 掉、本地跑不出来。",
        file=TAURI / "src" / "notifications.rs",
        injections=[(
            'let in_dev = curr_dir.ends_with(format!("{SEP}target{SEP}debug").as_str())',
            'let in_dev = curr_dir.ends_with(format!("{SEP}target{SEP}debug"))',
        )],
        cmd=cargo("test", "--lib", "windows_only_branch_is_source_checkable_for_pattern_bounds"),
        cwd=TAURI,
        expect_fail_hint="as_str",
        tags=["rust", "new-guards"],
    ),
    # ---------------- Rust：capability 覆盖 ----------------
    Case(
        name="capability 覆盖每个窗口（漏一个窗口 ACL 会静默拒绝）",
        why="设置窗口曾经不在 capability 的 windows 里，表现为「选目录/订阅事件静默失败」",
        file=TAURI / "capabilities" / "default.json",
        # 2026-09-24：群任务窗口从动态 label `todo-*` 改成固定的 `tasks`，预览窗口新增 ⇒
        # 锚点跟着换成现行那一行（锚点过期会被判成"注入 0 次"，等于这条护栏不再被证明）。
        injections=[
            (
                '"windows": ["main", "settings", "logs", "preview", "tasks"]',
                '"windows": ["main", "logs", "preview", "tasks"]',
            )
        ],
        cmd=cargo("test", "--lib", "capability_covers_every_window_label"),
        cwd=TAURI,
        expect_fail_hint="settings",
        tags=["rust", "packaging"],
    ),
    # ---------------- Rust：沙盒目录书签 ----------------
    Case(
        name="书签优先于路径（沙盒重启后唯一带权限的来源）",
        why="书签不优先 ⇒ 用户在 Finder 里移动过的目录会指回旧路径；沙盒里权限也丢了",
        file=TAURI / "src" / "user_dirs.rs",
        injections=[(
            """    if let Some(Ok(path)) = from_bookmark {
        if !path.is_empty() {
            return Some(path);
        }
    }
    stored""",
            """    if stored.is_some() {
        return stored;
    }
    if let Some(Ok(path)) = from_bookmark {
        if !path.is_empty() {
            return Some(path);
        }
    }
    None""",
        )],
        cmd=cargo("test", "--lib", "user_dirs"),
        cwd=TAURI,
        expect_fail_hint="bookmark_wins_over_the_stored_path",
        tags=["rust", "macos"],
    ),
    Case(
        name="⌘W 必须由自定义菜单项处理（系统预定义项在无边框窗口上会被判不可用）",
        why="用户 2026-09-13 真机：Mac 上主窗口 ⌘W 只会「滴滴滴」，而设置/日志窗口正常，"
        "⌘Q 也正常。系统预定义关闭项的动作是 performClose:，AppKit 按窗口的 Closable "
        "样式位校验可用性，而本项目 decorations:false ⇒ Borderless ⇒ 该项被判不可用，"
        "**而且没有任何日志**；自定义项不经这套校验，行为与「×」一致",
        file=TAURI / "src" / "menu.rs",
        injections=[(
            '        .item(&MenuItem::with_id(\n            app,\n            "close-window",\n            l.close_window,\n            true,\n            Some("CmdOrCtrl+W"),\n        )?)\n',
            '        .item(&PredefinedMenuItem::close_window(app, None)?)\n',
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "cmd_w_is_handled_by_our_own_menu_item",
        ),
        cwd=TAURI,
        expect_fail_hint="不许用系统预定义的关闭项",
        tags=["rust", "macos", "window"],
    ),
    Case(
        name="解除好友关系必须同时解除内存身份绑定（否则重装后只能重启）",
        why="用户 2026-09-13 真机：对方重装换过公钥后，删好友重新加也收不到任何东西，"
        "**必须重启**。根因是 `verify_hello` 的绑定有两条腿：friends 表 + 内存 peers 表"
        "（广播学来、未验签的公钥）。删好友只断了第一条腿，内存那条旧公钥继续当信任根 ⇒ "
        "Hello 一直被硬拒。这条退化的形态是「功能看着都在、就是连不上」，只能靠源码护栏盯住",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "    crate::network::transport::forget_peer_identity(s, &peer_id);\n",
            "",
        )],
        cmd=cargo(
            "test",
            "--offline",
            "--lib",
            "--features",
            "bluetooth",
            "removing_a_friend_also_drops_the_in_memory_identity_binding",
        ),
        cwd=TAURI,
        expect_fail_hint="forget_peer_identity",
        tags=["rust", "identity", "friend"],
    ),
    # ---------------- Rust：好友申请丢了要能补发 ----------------
    Case(
        name="好友申请丢了要能补发（『已发送』但对方没收到）",
        why="用户 2026-09-12 真机：点加好友后对方什么都没收到，而发送方显示「已发送，等待对方确认」"
        "—— 好友申请是没有回执的定向帧，链路抖动时会静默丢失。现在发出即登记、建链补发、"
        "收到同意/拒绝后清除",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "    s.pending_out_requests\n        .lock()\n        .unwrap_or_else(|e| e.into_inner())\n        .insert(peer_id.clone());\n",
            "",
        )],
        cmd=cargo("test", "--lib", "friend_request_survives_a_dropped_link"),
        cwd=TAURI,
        expect_fail_hint="先登记",
        tags=["rust", "friend"],
    ),
    # ---------------- Rust：分批清空（点清除数据不卡死的机制） ----------------
    Case(
        name="清空数据必须分批（单次只删一批，批间放锁）",
        why="用户实测：点「清除数据」设置窗口卡死 —— 原实现一个大事务握住 db 锁数秒，"
        "所有读命令都在等锁。改回大事务就会静默退化",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT {CLEAR_BATCH_ROWS})",
            "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT 999999999)",
        )],
        cmd=cargo("test", "--lib", "clear_is_batched"),
        cwd=TAURI,
        expect_fail_hint="单次调用",
        tags=["rust", "perf"],
    ),
    # ---------------- 窗口架构：三个窗口各自一个文档 + 一个入口 ----------------
    Case(
        name="窗口入口（独立窗口不得再共用主窗口的 HTML）",
        why="用户实测：「第二次打开设置，窗口先刷成主聊天窗口、又立马变成设置界面」「点一下要等很久」"
        "—— 根因就是设置/日志窗口加载的是主窗口的 index.html，前端再把聊天三栏挂起来换成设置页",
        file=TAURI / "src" / "commands.rs",
        injections=[
            ('WebviewUrl::App("settings.html".into())', 'WebviewUrl::App("index.html".into())'),
            ('WebviewUrl::App("todos.html".into())', 'WebviewUrl::App("index.html".into())'),
        ],
        cmd=cargo("test", "--lib", "aux_windows_open_their_own_document"),
        cwd=TAURI,
        expect_fail_hint="index.html",
        tags=["rust", "window"],
    ),
    Case(
        name="自聊消息必须留在本地（不进 outbox / 不发 gossip）",
        why="「和自己聊天」的消息收发双方都是本机：一旦写进 outbox，那一行**永远等不到 Ack**"
        "（没有对端），会被每次心跳/建链的 flush_outbox 重发 ⇒ 「outbox 必然排空」这条不变量失效。"
        "而这在界面上完全看不出来（消息照样显示、列表照样刷新），只有库里悄悄长出一条永不消失的行。",
        file=TAURI / "src" / "commands.rs",
        injections=[
            (
                '    db::insert_message(&dbc, &rec).map_err(|e| format!("消息写入失败：{e}"))?;',
                '    db::insert_message_and_outbox(&dbc, &rec, &me, "x").map_err(|e| format!("消息写入失败：{e}"))?;',
            )
        ],
        cmd=cargo("test", "--lib", "self_chat_stays_local"),
        cwd=TAURI,
        expect_fail_hint="不得出现",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="窗口单例（打开命令不得自己查窗口存在性）",
        why="连点两下会开出第二个窗口：`build()` 的重复 label 检查在 prepare 阶段，而窗口登记进 manager "
        "是主线程创建完成之后 —— 并发调用会双双通过。必须统一走 ensure_aux_window（单例 + 串行）。"
        "2026-09-19 随 4.22.2 的 async→同步改造，锚点从 commands.rs 搬到 commands/logs.rs",
        file=TAURI / "src" / "commands" / "logs.rs",
        injections=[
            (
                "    ensure_aux_window(\n        &app,\n        crate::WINDOW_SETTINGS,",
                "    if app.get_webview_window(crate::WINDOW_SETTINGS).is_some() {\n        return Ok(());\n    }\n    ensure_aux_window(\n        &app,\n        crate::WINDOW_SETTINGS,",
            )
        ],
        cmd=cargo("test", "--lib", "aux_window_open_is_singleton_serialized_and_resident"),
        cwd=TAURI,
        expect_fail_hint="不该自己查窗口存在性",
        tags=["rust", "window"],
    ),
    Case(
        name="外链窗口隔离（远端页面不得拿到任何 capability）",
        why="外链窗口加载的是**远端页面**；一旦被 capability 覆盖，第三方内容就能调用本应用的"
        "dialog/opener/event 等命令面 —— 等于把本机能力交给用户随手配置的网址",
        file=TAURI / "capabilities" / "default.json",
        # 锚点=现行 windows 数组的收尾（把外链 label 追加进去就是那条被禁止的回归）。
        # ⚠️ 别用 `"todo-*"` 当锚点：动态 label 已在 2026-09-24 换成固定的 `tasks`。
        injections=[('"preview", "tasks"]', '"preview", "tasks", "link"]')],
        cmd=cargo("test", "--lib", "link_window_is_not_capability_covered"),
        cwd=TAURI,
        expect_fail_hint="link",
        tags=["rust", "window", "new-guards"],
    ),
    Case(
        name="外链 URL 协议白名单（只放行 http/https）",
        why="`javascript:` / `data:` / `file:` / `tauri:` 一旦漏过，等于把『在应用 WebView 里执行脚本 / "
        "读本机文件』的能力交给一段用户粘贴的字符串",
        file=TAURI / "src" / "commands.rs",
        injections=[('!matches!(parsed.scheme(), "http" | "https")', "false")],
        cmd=cargo("test", "--lib", "external_link_rejects_non_http_schemes"),
        cwd=TAURI,
        expect_fail_hint="必须拒绝",
        tags=["rust", "new-guards"],
    ),
    Case(
        name="外链窗口用 WebviewUrl::External 加载远端 URL",
        why="外链窗口是唯一不走本地 App 文档的窗口；退回 App(\"index.html\") 会把聊天三栏挂起来"
        "（回到一窗一入口之前的老问题），且根本加载不了外部网址",
        file=TAURI / "src" / "commands.rs",
        injections=[
            (
                "            WebviewUrl::External(build_url),",
                '            WebviewUrl::App("index.html".into()),',
            )
        ],
        cmd=cargo("test", "--lib", "aux_windows_open_their_own_document"),
        cwd=TAURI,
        # 注入后失败的是"不得再共用 index.html"那条判据（它先于 External 断言触发）。
        expect_fail_hint="index.html",
        tags=["rust", "window"],
    ),
    Case(
        name="群任务窗口的 label 取自常量（固定一扇，不用动态 todo-*）",
        why="预热/复用都建立在「这一扇窗的 label 固定」上（用户 2026-09-24：「可不传参数、后台默默"
        "先把 WebView 建好，用的时候瞬间激活」）；builder 里另写字面量就会与 `crate::WINDOW_TASKS`"
        "以及前端启动器那份 `\"tasks\"` 漂移 —— 三方不一致表现为「点了没反应」或「预热的那扇永远等不到」。"
        "⚠️ 必须同时改坏两处（ensure_aux_window + builder）：护栏看的是**整个函数体**里还有没有常量，"
        "只换一处仍然绿。",
        file=TAURI / "src" / "commands" / "logs.rs",
        injections=[
            (
                '    ensure_aux_window(\n        app,\n        crate::WINDOW_TASKS,',
                '    ensure_aux_window(\n        app,\n        "todo-fixed",',
            ),
            (
                '                &build_app,\n                crate::WINDOW_TASKS,',
                '                &build_app,\n                "todo-fixed",',
            ),
        ],
        cmd=cargo("test", "--lib", "tasks_window_uses_one_fixed_label_cross_checked_with_frontend"),
        cwd=TAURI,
        expect_fail_hint="取自常量",
        tags=["rust", "window", "new-guards"],
    ),
    Case(
        name="守卫的源码全集必须登记齐 include! 分册（漏登记=假绿）",
        why="`include!` 只做编译期拼接，守卫用的 `include_str!` 清单是**手工登记的第二份**。"
        "它已经漂移过两次：4.25.0 接线中继时 `commands/relay.rs` 与 `transport/relay.rs` 都只登记了"
        "`include!` 与领域图、漏了这里（现场注释还在）。而漏登记的后果**不是报错是假绿** —— "
        "以「全部命令面」为判据的守卫扫不到那个分册，于是永远通过。"
        "注入=把 relay 那一行登记删掉（正是当年真实发生过的那个形状）。",
        file=TAURI / "src" / "lib.rs",
        injections=[('            include_str!("commands/relay.rs"),\n', "")],
        cmd=cargo("test", "--lib", "guard_source_views_register_every_include_subfile"),
        cwd=TAURI,
        expect_fail_hint="少登记",
        tags=["rust", "guards"],
    ),
    Case(
        name="好友申请兜底（get_pending_requests 必须按好友关系过滤）",
        why="用户明确要求：已在好友列表的人，其申请应当自动清除。主修在各条同意路径，"
        "这里是不依赖「哪条消息到了」的兜底判据",
        file=TAURI / "src" / "commands.rs",
        injections=[("map.retain(|_, req| is_actionable_request(req, &friend_ids));", "map.retain(|_, _| true);")],
        cmd=cargo("test", "--lib", "pending_requests_exclude_existing_friends"),
        cwd=TAURI,
        expect_fail_hint="必须按好友关系过滤",
        tags=["rust", "friend"],
    ),
    Case(
        name="掉线节点：留在发现列表里，但不得算「在线」",
        why="真机 2026-09-12（只开蓝牙）：手机能看到 Mac（已发现未建联），Mac 里安卓什么都不显示 —— "
        "根因是链路一断就删节点条目，而 BLE 上「连上→被对端退让→断开」是常态，"
        "「添加好友」列表里只闪一下、用户点不到。反向的坑是复核抓到过的 High 缺陷："
        "若保留条目却仍按「在 peers 表里 = 在线」判定，就变成「连过又掉线 ⇒ 永久在线」。"
        "两件事必须一起成立：条目保留 + 在线看 last_seen 新鲜度",
        file=TAURI / "src" / "commands.rs",
        injections=[(
            "        f.online = friend_is_online(last_seen, now, active_links.contains(&f.device_id));",
            "        f.online = peers.contains_key(&f.device_id) || active_links.contains(&f.device_id);",
        )],
        cmd=cargo("test", "--lib", "offline_peer_stays_listed_but_is_not_online"),
        cwd=TAURI,
        expect_fail_hint="friend_is_online",
        tags=["rust", "presence", "network"],
    ),
    Case(
        name="打包配置（release 前端必须压缩）",
        why="TAURI_ENV_DEBUG 是字符串（release 为 \"false\"），`!process.env.TAURI_ENV_DEBUG` "
        "把 release 当成 debug ⇒ 前端不压缩还带 sourcemap（实测 310KB → 500KB + 735KB .map）。"
        "这类退化不报错、不影响功能，只会让所有 release 包悄悄变慢",
        file=ROOT / "vite.config.ts",
        injections=[("minify: isDebugBuild ? false : \"esbuild\"", "minify: !process.env.TAURI_ENV_DEBUG ? \"esbuild\" : false")],
        cmd=npm("test"),
        cwd=ROOT,
        expect_fail_hint="TAURI_ENV_DEBUG",
        tags=["frontend", "build"],
    ),
    Case(
        name="测试清单：平台基线之间的差额说不出平台理由必须报出来（Windows 那条腿不能是瞎的）",
        why="`added`（本平台多跑）只能 warn —— 新测试跑得好好的不该红。代价是**一条用例只要不在某平台的基线里，"
            "它在那个平台上消失就不会红**，而 mac 那条腿照跑照绿。Windows 基线曾烂到只有并集的一部分"
            "（487 vs 690，其中 194 条没有任何平台门控能解释）。新判据不依赖 cargo：任一平台基线都不许比"
            "各平台基线的并集少一条**有源码门控背书**的用例，背书从 src-tauri/src 现扫（模块声明级 + 测试函数级"
            "的 target_os/unix/windows cfg），解析不出平台约束的 cfg 一律要求每个平台都得有 ⇒ 猜错的方向是"
            "'响亮地红'而不是'静默的洞'。故意从 Windows 基线删掉一条跨平台用例必须红。",
        file=TAURI / "test-baseline.windows.txt",
        injections=[("db::migration_tests::downgrade_refusal_writes_nothing\n", "")],
        cmd=["node", "scripts/check-test-manifest.mjs", "--only", "frontend"],
        cwd=ROOT,
        expect_fail_hint="说不出平台理由",
        # 走 --only frontend 这一份**是有意的**：这条判据不需要 cargo，所以它必须在不编译 Rust 的
        # 那条路径上也被跑到 —— 否则"cargo 编不过"会把跨平台这只眼一起关掉（放进 checkRust 里就是这个后果）。
        tags=["manifest", "frontend", "ci"],
    ),
    Case(
        name="私钥边界：给 Identity 补一行 derive(Serialize) 必须报出来（INV-P18 靠构造，不靠人记得）",
        why="私钥今天不外泄，只是因为 `Identity`/`EphKeypair` 没 derive `Serialize` —— "
            "序列化不了就到不了前端（命令返回值与 emit 载荷都要求 Serialize）。"
            "但这行防御是**一行改动就能破、破掉之后界面完全无症状**的安全边界："
            "有人为了「顺手打印一下身份」给结构加一行 derive，整条 E2EE 就归零了。"
            "`scripts/check-key-boundary.mjs` 判两条：A 任何可序列化类型都不许带密钥字段"
            "（按名字 + 按类型双词表，所以 `ed25519_signing` 这种不含 secret 的名字也抓得住）；"
            "B 命令不许把密钥放在「交出值」的位置。本用例注入的正是最可能的那次误改。",
        file=TAURI / "src" / "crypto.rs",
        injections=[(
            "pub struct Identity {",
            "#[derive(Clone, Debug, serde::Serialize)]\npub struct Identity {",
        )],
        cmd=["node", "scripts/check-key-boundary.mjs"],
        cwd=ROOT,
        expect_fail_hint="可序列化类型 Identity 带密钥字段",
        tags=["rust", "security", "new-guards"],
    ),
    Case(
        name="数表必须早于 execute_batch(SCHEMA)（否则 is_fresh 恒假 ⇒ 全新库重放整条迁移链）",
        why="`is_fresh = current == 0 && pre_table_count == 0` 里只有前半个条件是真话：SCHEMA 会建出\n"
        "     全部 19 张表，所以数表一旦跑到 SCHEMA 之后，`is_fresh` 就恒为假 —— 全新库于是把 v1→v9\n"
        "     整条链重放一遍（2026-09-25 之前正是这样）：首次启动多打 9 行假的「running v1→v2…」\n"
        "     日志、v7 两句「孤儿清理跳过一条语句」告警、还先建 `idx_outbox_msg_id` 再在 v8→v9 删掉它。\n"
        "     ⚠️ 这个退化在**形状上完全看不出来**（迁移全是幂等的，`user_version` 两种走法都停在\n"
        "     DB_VERSION）⇒ 只有源码顺序能钉住它，所以本用例的注入就是「把两句话调换顺序」：\n"
        "     编译照过、测试套照绿，只有这条守卫会红。",
        file=TAURI / "src" / "db.rs",
        injections=[(
            "    let pre_table_count: i64 = conn\n        .query_row(\n            \"SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'\",\n            [],\n            |r| r.get(0),\n        )\n        .unwrap_or(0);\n    conn.execute_batch(SCHEMA)?;\n",
            "    conn.execute_batch(SCHEMA)?;\n    let pre_table_count: i64 = conn\n        .query_row(\n            \"SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'\",\n            [],\n            |r| r.get(0),\n        )\n        .unwrap_or(0);\n",
        )],
        cmd=cargo("test", "--lib", "tables_are_counted_before_the_schema_is_applied"),
        cwd=TAURI,
        expect_fail_hint="恒为假",
        tags=["rust", "db", "stability", "startup", "new-guards"],
    ),
    Case(
        name="缓存清理器不得跟随符号链接（审计 1.1：软链目标会被当缓存永久删除）",
        why="缓存目录是远端输入可达面（收到的文件名/目录名不受信）。旧实现用 e.path().metadata()\n"
        "     判类型——它**跟随软链**：缓存里一个指向任意位置的软链会让其目标被收集进清理列表并\n"
        "     remove_file 永久删除，报告里只算「清理了多少缓存」—— 全仓唯一确认的数据丢失点。\n"
        "     注入方式：把 DirEntry::file_type()（不跟随链接）换回「path().metadata().map(|m|\n"
        "     m.file_type())」（类型对得上、仍可编译，语义回到跟随链接），\n"
        "     守卫 cache_cleaner_walk_never_follows_symlinks 扫到 e.path().metadata() 必须红。",
        file=TAURI / "src" / "storage" / "cache_cleaner.rs",
        injections=[(
            "            let Ok(ft) = e.file_type() else {",
            "            let Ok(ft) = e.path().metadata().map(|m| m.file_type()) else {",
        )],
        cmd=cargo("test", "--lib", "cache_cleaner_walk_never_follows_symlinks"),
        cwd=TAURI,
        expect_fail_hint="不跟随软链",
        tags=["rust", "storage", "stability", "data-loss", "new-guards"],
    ),
    Case(
        name="预览摆位的钳制被摘掉必须被抓住（接线判据，不是纯计算那条）",
        why="用户 2026-10-09：主窗口拖到屏幕靠上后「打开预览图片 窗口会有一部分展示在屏幕外面」。\n"
        "     ⚠️ 这条用例本身就是本轮变异抓出来的：把 `clamp_pos_into_rect(base, outer, r)` 换成 `base`，\n"
        "     那两条**纯计算**判据全绿 —— 它们测的是新写的那两个函数，坏掉的是「谁调它们」。\n"
        "     所以补了读源码切片的 recenter_aux_window_actually_uses_the_placement_rules；\n"
        "     这里证明的正是那一条会红（不是证明数学对）。",
        file=TAURI / "src" / "commands" / "logs.rs",
        injections=[(
            "        Some(r) => clamp_pos_into_rect(base, outer, r),",
            "        Some(r) => base,",
        )],
        cmd=cargo("test", "--lib", "recenter_aux_window_actually_uses"),
        cwd=TAURI,
        expect_fail_hint="不再调用 clamp_pos_into_rect(",
        tags=["rust", "window", "new-guards"],
    ),]
