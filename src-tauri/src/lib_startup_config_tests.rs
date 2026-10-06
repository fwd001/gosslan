// 职责边界：
// - `lib_tests.rs` 测试分册之10 —— 启动与配置：降级拒绝早于写、锁内不 await、符号链不吃、开关读持久值
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
    /// 启动期"数据比本机新 ⇒ 拒绝打开"这条路径的三个形状都必须钉住（AI_RULES §13）。
    ///
    /// ① **判定早于任何写操作**：`execute_batch(SCHEMA)` 是 `CREATE TABLE IF NOT EXISTS`，
    ///    看着无害，但它确实写文件；先写再判，`downgrade_message()` 里"数据没有被修改"就成了
    ///    谎话，而用户正是凭这句话决定"可以放心装新版本"。（回归过的位置：v4.22.36 之前
    ///    判定在 `run_migrations` 里 = SCHEMA 之后。）
    /// ② **调用方按类型分支**，不许 `to_string().contains("user_version")` —— 协议层已经
    ///    因为按错误字符串分类吞过一整条消息（INV-P24 第 2 条，v4.22.34）。
    /// ③ **弹窗只能非阻塞**：`blocking_show()` 的桌面实现是 `run_on_main_thread`
    ///    （tauri-plugin-dialog-2.7.3/src/desktop.rs:222），而 `setup` 正跑在主线程上 ⇒
    ///    排在队列里的弹窗永远执行不到 = 开机自锁，与 v4.22.30 修掉的 Windows 开窗卡死
    ///    同一个形状。写错成 blocking 版本不会报错，只会**永远白屏**，所以必须机器拦。
    #[test]
    fn boot_downgrade_refusal_is_typed_precedes_writes_and_non_blocking() {
        /// 只留**代码行**：这条守卫判的是"有没有真的调用"，注释里提到 API 名字是
        /// 常事（本条第一次跑就被自己的注释判红了），所以先把行注释剥掉 ——
        /// 既不让散文误伤守卫，也不让散文冒充成实现。
        fn code_only(src: &str) -> String {
            src.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        }
        let db = code_only(include_str!("db.rs"));
        // ⚠️ 只取**测试模块之前**那段代码：守卫自己就在 lib.rs 里，全文一起扫的话
        // 下面这几个字面串会先命中守卫自己的源码 —— 正判据永远为真、负判据永远为假，
        // 这条守卫会变成看着严密实际空转的那种。
        let lib = code_only(
            include_str!("lib.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default(),
        );

        let check = "if current > DB_VERSION";
        let write = "conn.execute_batch(SCHEMA)";
        assert_eq!(
            db.matches(check).count(),
            1,
            "降级判定全仓只许一处（第二处迟早与第一处口径不同）"
        );
        assert_eq!(
            db.matches(write).count(),
            1,
            "本机 schema 只许在一处写入，否则这条顺序断言无法判定位置"
        );
        assert!(
            db.find(check).unwrap() < db.find(write).unwrap(),
            "降级判定必须早于 execute_batch(SCHEMA)：先写再判 = 「数据未被修改」是谎话"
        );

        assert!(
            lib.contains("downcast_ref::<db::InitError>()"),
            "启动期必须按 db::InitError 类型分支，而不是按错误字符串猜"
        );
        assert!(
            !lib.contains("blocking_show"),
            "主线程上 blocking_show() 会自锁（弹窗任务排在被钉住的主线程队列里）"
        );
        assert!(
            lib.contains(".show(move |_|"),
            "降级提示必须非阻塞排队 + 提前 return，让主线程回到事件循环"
        );
    }

    /// 不得「先绑定 `links` 守卫、再在循环里 await 发送」。
    ///
    /// `links` 是 `tokio::sync::Mutex`，跨 await 持锁**编译器不拦**，而发送目标都是有界队列
    /// （1024）：对端僵死（半开 TCP / 休眠 / 写缓冲满）时 `send().await` 会一直挂起却握着
    /// 全局 links 锁 ⇒ try_send、心跳、get_peers、mark_peer_offline、teardown_link 以及
    /// 看门狗全部阻塞。看门狗恰恰是唯一能发 cancel 拆掉那条卡死连接、让队列排空的机制，
    /// 它被同一把锁挡住就是自锁死循环，只能靠用户手动重开局域网。
    /// 正确写法：锁内只 `clone` 发送端快照，发送放到锁外（与心跳发送同一纪律）。
    #[test]
    fn never_awaits_while_holding_the_links_lock() {
        let cmds = all_commands_src();
        for f in [
            "pub async fn update_profile(",
            "pub async fn broadcast_chat_style(",
        ] {
            let body = rust_fn_body(cmds, f);
            assert!(
                !body.contains("for link in links"),
                "{f} 又回到「持有 links 守卫时 await 发送」的写法：\
                 队列有界，对端僵死会让 send().await 永久挂起并握着全局 links 锁，\
                 连看门狗都拿不到锁 ⇒ 网络层自锁死。必须先 collect 发送端快照、再在锁外发送。"
            );
        }
    }

    /// transport 生产代码的 std Mutex 一律**抗中毒**取锁（审计 A8）。
    ///
    /// 全仓主导写法是 `.lock().unwrap_or_else(|e| e.into_inner())`（`state.rs` 的注释也明令"不要写
    /// `.lock().unwrap()`"），但 `network/transport.rs` 曾留了 11 处多行链式 `.lock().unwrap()`。
    /// 后果：全仓没有 `catch_unwind` ⇒ 一次锁中毒 panic 会带走 `reader_loop` 并**跳过它紧接着的收尾**
    /// （`links.remove` / `mark_peer_offline` / 接收器清理），于是那条连接的对端在界面上永久显示"在线"。
    ///
    /// 判据：**去掉所有空白后**扫 transport 全集视图，不得出现 `.lock().unwrap()` —— 一并覆盖单行与
    /// 多行链式两种写法。为什么能整文件扫而不误伤测试：transport.rs 与各 `transport/*.rs` 分册的
    /// `#[cfg(test)]` 模块本来就不用这个写法（实测 0 处），test 代码里的 4 处都在 `file.rs`/`logs_tests.rs`
    /// （不在这份视图里），测试可以合理地对中毒 panic。
    #[test]
    fn transport_locks_tolerate_poison() {
        let src = crate::network::transport_src_for_guards();
        let flat = src.split_whitespace().collect::<String>();
        assert!(
            !flat.contains(".lock().unwrap()"),
            "transport 生产代码里出现 `.lock().unwrap()`：一次中毒 panic 会带走 reader_loop、跳过收尾，\
             对端永久\"在线\"（审计 A8）。改用 `.lock().unwrap_or_else(|e| e.into_inner())`。"
        );
    }

    /// 缓存清理器的目录遍历**绝不跟随符号链接**（2026-09-23 审计 1.1）。
    ///
    /// 后果链：缓存目录是远端输入可达面（收到的文件名/目录名不受信）。旧实现用
    /// `e.path().metadata()` 判类型 —— 它**跟随软链**：缓存里一个指向任意位置的软链
    /// （文件或目录）会让其目标被收集进清理列表并 `remove_file` **永久删除**，
    /// 报告里只算"清理了多少缓存"。这是全仓唯一确认的数据丢失点。
    ///
    /// 判据：`walk_files` 体内必须用 `DirEntry::file_type()`（不跟随链接）判真身，
    /// 且不得出现 `e.path().metadata()`（跟随链接的旧写法）。
    #[test]
    fn cache_cleaner_walk_never_follows_symlinks() {
        let src = include_str!("storage/cache_cleaner.rs");
        let body = rust_fn_body(src, "pub fn walk_files(");
        assert!(
            body.contains("e.file_type()"),
            "walk_files 必须用 DirEntry::file_type()（不跟随软链）判文件真身（审计 1.1）"
        );
        assert!(
            !body.contains("e.path().metadata()"),
            "walk_files 里出现 e.path().metadata()：它跟随符号链接，软链目标会被当缓存删除（审计 1.1）\
             —— 数据丢失，不是清理"
        );
    }

    /// **headless 开关不许覆盖用户显式关掉的局域网**（2026-09-27，根因级修法）。
    ///
    /// 现场：2026-09-26 有一条产品结论"关掉局域网发现还能被别人学到"，复跑发现**那个实例从来没真的
    /// 关掉过** —— 启动路径写的是 `if forced || enabled`，`GOSSLAN_AUTOSTART=1` 把库里的"关"覆盖成"开"。
    /// 现在这个 env 只剩一件事可做：**选绑定地址**（0.0.0.0），"开不开"只由持久化偏好决定。
    ///
    /// ⚠️ 这条钉的是源码形状（半个守卫）：换一种拼法（比如把 `forced` 并进 `enabled` 再判）它看不见。
    ///    真判据是"预置 lan_enabled='0' 的那个实例，对端日志里不该出现它的 announce" —— 那需要
    ///    单独一轮（第 4 处登记），已记在 roadmap #89，不在这里顺手接线。
    #[test]
    fn autostart_env_never_overrides_an_explicit_lan_off() {
        let src = include_str!("lib.rs");
        let at = src
            .find("GOSSLAN_AUTOSTART")
            .expect("启动路径必须还认这个 headless 开关");
        // 只看**测试模块之前**那一段：本条判据自己的字符串里就写着被禁的那种拼法，
        // 拿整份文件去 contains 会匹配到自己身上（第一次跑正是这么红的）。
        // 范围 = 整份 lib.rs。2026-09-28 之后成立的方式变了：测试整段搬进
        // `include!("lib_tests.rs")` 的那个文件，**lib.rs 里不该再有 `#[cfg(test)]`** ⇒ 判据不可能匹配到自己。
        // 这一格同时把"lib.rs 只许生产码"钉住：谁再往启动装配文件里塞测试模块，这里当场红。
        assert!(
            !src.contains("#[cfg(test)]"),
            "lib.rs 现在只该是生产码（测试在 lib_tests.rs）；出现测试模块意味着本条判据的范围要重新界定"
        );
        let test_at = src.len();
        assert!(
            at < test_at,
            "这个开关必须出现在启动路径里，而不是只活在测试里"
        );
        // 先抹掉行注释再匹配：本文件**启动块自己的注释**就写着"旧写法是 `if forced || enabled`"，
        // 不抹的话这条禁令会因为一段说明文字而永远红（同一族坑见 `cmd_w_is_handled_by_our_own_menu_item`）。
        let block: String = src[at..test_at]
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !block.contains("if forced || enabled"),
            "GOSSLAN_AUTOSTART 不许再把用户显式关掉的局域网判成开（那条假产品结论就是这么来的）"
        );
        assert!(
            block.contains("\n                    if enabled {"),
            "开不开网络只能由 `enabled`（= db 里的 lan_enabled）决定，形状必须留在启动块里"
        );
        assert!(
            block.contains("let started = if forced {") && block.contains("\"0.0.0.0\""),
            "forced 剩下的职责必须是**选绑定地址**；它一旦无事可做就该整个删掉，而不是留着覆盖偏好"
        );
    }

    /// **通道偏好必须与运行状态分开表达**（用户 2026-09-14 桌面实测：关掉蓝牙、退出重进又被打开）。
    ///
    /// 根因：快照里 channels[bluetooth].enabled 用的是"运行时是否在跑"，应用刚启动、BLE 还没
    /// 拉起时必然是 false ⇒ 前端 ensureBluetoothOn 无法区分"用户明确关掉"与"还没启动"，
    /// 于是把偏好覆盖成开。判据：通道状态必须有独立的 preferred 字段，且快照从持久化键
    /// （lan_enabled / bt_enabled）填充。
    #[test]
    fn channel_status_exposes_persisted_preference() {
        let tm = include_str!("transport/mod.rs");
        assert!(
            tm.contains("pub preferred: bool"),
            "ChannelStatus 必须有独立的 preferred 字段（与 running 分开），否则前端只能拿运行状态猜偏好"
        );
        let cmds = all_commands_src();
        let body = rust_fn_body(cmds, "pub async fn build_runtime_snapshot(");
        assert!(
            body.contains("get_lan_enabled") && body.contains("get_bt_enabled"),
            "快照必须从持久化键填充 preferred（开机后偏好不能丢）"
        );
        assert!(
            body.contains("c.preferred"),
            "必须把 db 里的偏好写回通道状态"
        );
    }

    /// **系统通知必须真的发得出去、且能被观察**（用户 2026-09-14：Windows 同事收不到任何通知）。
    ///
    /// 三个必须同时成立的判据：
    /// 1. Rust 侧通知统一走 crate::notifications（能返回错误），不再用插件那个把错误 spawn
    ///    掉丢掉的 show()；
    /// 2. **不经前端**的好友申请/好友通过通知必须尊重 notify_enabled（否则关了通知还会被弹）；
    /// 3. 设置页要有能如实报告失败的“发送测试通知”入口，否则 Windows 上（未安装 / 勿扰）
    ///    永远只能靠猜。
    #[test]
    fn notifications_are_observable_and_respect_the_switch() {
        let transport = crate::network::transport_src_for_guards();
        assert!(
            !transport.contains("tauri_plugin_notification::NotificationExt"),
            "network 层不得再直接用插件的 show()（它把错误 spawn 掉丢了）—— 统一走 crate::notifications"
        );
        assert_eq!(
            transport.matches("crate::notifications::show").count(),
            3,
            "三处 Rust 侧通知（好友申请×2 + 好友通过×1）都必须走 notifications（含开关与错误）。\
             原先是 4（通过那件事在直连与跨跳各写一遍），2026-09-28 收成一个\
             `apply_friend_accept` 之后只剩一家 —— 这个数字**只许因为又漏了一条路而变大**"
        );
        let notif = include_str!("notifications.rs");
        assert!(
            notif.contains("pub fn show_if_enabled") && notif.contains("notify_enabled"),
            "notifications 必须提供“尊重总开关”的入口"
        );
        assert!(
            notif.contains("map_err(|e| e.to_string())"),
            "notify-rust 的错误必须返回出来，不能吞"
        );
        let commands = all_commands_src();
        assert!(
            commands.contains("pub fn send_test_notification("),
            "必须有设置页可调用的测试通知命令"
        );
    }
