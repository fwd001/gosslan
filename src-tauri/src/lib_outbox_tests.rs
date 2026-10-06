// 职责边界：
// - `lib_tests.rs` 测试分册之7 —— Outbox 与过期终态：先落库再 emit、破坏性写延后、清扫覆盖半截与停滞接收端
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
    /// 出站清扫器的「失败终态」必须**由写库成功门控**（审计 A3）。
    ///
    /// 旧缺陷：三处 `emit("message-failed"/"file-failed")` 都写在 `if let Ok(dbc){ 写库 }` **之外**，
    /// 且写库返回值被 `let _ =` 丢掉 ⇒ 锁中毒或写库失败时**界面照样报失败、而 outbox 行还在**
    /// ⇒ 下一次 `flush_outbox` 把它再发一遍 = 用户看到「我以为失败了的消息又发出去了」（重复投递）。
    /// 判据：① 三处终态各由 finalize 助手落库，emit 在 `if finalized {` 分支里（写库成功才发）；
    ///      ② db 锁中毒不许 `else { continue }` 静默跳过整个 tick（消息会永久停在 sending）。
    /// 为什么是源码守卫：本仓没有能驱动这个清扫循环 + 捕获 emit 的异步夹具（同 4.25.7 打标顺序那条）。
    #[test]
    fn outbox_sweeper_emits_only_after_db_write_succeeds() {
        let src = crate::network::transport_src_for_guards();
        let body = rust_fn_body(&src, "pub fn spawn_outbox_sweeper(");
        assert_eq!(
            body.matches("finalize_expired_message(").count(),
            2,
            "单聊 + 群两处终态必须各调一次 finalize_expired_message（两步写库都成功才返回 true）"
        );
        assert_eq!(
            body.matches("finalize_expired_file(").count(),
            1,
            "文件终态必须调 finalize_expired_file"
        );
        assert_eq!(
            body.matches("if finalized {").count(),
            3,
            "三处 emit 必须各自被 `if finalized {{ … }}` 门控 —— 写库没成功就不许 emit，否则重复投递"
        );
        assert_eq!(
            body.matches("emit(\"message-failed\"").count(),
            2,
            "message-failed 仍恰好两处（单聊 + 群）"
        );
        assert_eq!(
            body.matches("emit(\"file-failed\"").count(),
            1,
            "file-failed 仍恰好一处"
        );
        assert!(
            !body.contains(".db.lock() else { continue }"),
            "db 锁中毒必须可见（poison-tolerant + 日志），不许 `else {{ continue }}` 静默跳过整个清扫 tick"
        );
    }

    /// 落终态的**写序**：把行踢出重试集合的那一次写必须排最后（2026-09-23 审计 A3 的自身缺陷）。
    ///
    /// `finalize_*` 返回 `false` 时对不 emit、"下一 tick 重试" —— 这个承诺只有在**行还在重试集合里**
    /// 时才成立。文件那趟里 `mark_file_outbox_failed` 是唯一会让 `list_expired_file_outbox`
    /// 再也扫不到该行的写（它只选 pending/sending），一旦排到最前面，后面任何一步失败就变成
    /// "界面报失败、行再也扫不到"，或者更糟：永久停在"发送中"。消息那趟同型（删行必须最后）。
    /// 判据用位置而不是数量：把破坏性写挪回前面时数量不变，只有顺序变。
    #[test]
    fn terminal_finalize_defers_the_destructive_write() {
        let src = crate::network::transport_src_for_guards();

        // P7 之后实现并进了唯一出口 `db::finalize_file_failure`（`finalize_expired_file`
        // 只剩一层 bool 包装）⇒ 这条逐函数守卫跟着挪到新家；迁移本身由
        // `every_finalize_path_defers_the_destructive_write`（自动扫）与
        // `expired_file_*` 那两条行为测试兜着。
        let dbc = all_db_src();
        let file_body = rust_fn_body(&dbc, "pub fn finalize_file_failure(");
        let destructive = file_body
            .find("mark_file_outbox_failed(")
            .expect("文件终态必须仍由 mark_file_outbox_failed 关队列行");
        for anchor in ["set_message_status(", "upsert_transfer("] {
            let at = file_body
                .rfind(anchor)
                .unwrap_or_else(|| panic!("finalize_file_failure 少了 {anchor} 这一步"));
            assert!(
                at < destructive,
                "破坏性写（把行踢出重试集合）必须排最后：它先跑 ⇒ 后续步骤失败时行已是 failed，\
                 下一轮再也扫不到 ⇒ {anchor} 的失败永久无人重试，界面卡在「发送中」"
            );
        }

        let msg_body = rust_fn_body(&src, "fn finalize_expired_message(");
        let deleted = msg_body
            .find("delete_outbox_by_msg_id(")
            .expect("单聊终态必须仍删 outbox 行");
        assert!(
            msg_body
                .find("set_message_status(")
                .expect("少了置 failed 那一步")
                < deleted,
            "删行是消息终态里唯一不可重放的写，必须排在置 failed 之后"
        );
    }

    /// 把上面那条**点名式**写序守卫升级成**自动扫全部收尾路径**（第 4 步 P7 第 4 条）。
    ///
    /// 为什么必须升级：`fail_file_job` 与 `cancel_file_transfer` 各自也做"落终态 + 把行踢出
    /// 重试集合"这件事，形状与 `finalize_expired_file` 一模一样，但它们是**另写的两份**
    /// （§9 那族平行实现）。点名式守卫看不见没被点名的那个 —— 于是那里明令禁止的顺序
    /// （先踢出集合、后置气泡状态）在这里照样成立：outbox 行先变 failed/cancelled，
    /// 后面任何一步失败，清扫器再也扫不到这一行 ⇒ 那条气泡**永久停在「发送中」**，
    /// 而且再没有人会去修它（它已经不在任何重试集合里）。
    ///
    /// 判据是**结构**的，不是清单：凡调用了"把行踢出重试集合"那一类写的函数，
    /// 该调用必须排在同函数内所有"面向用户的写"之后。新增第三个收尾函数会**自动**被扫到，
    /// 不必记得来这张表里登记 —— 这正是点名式做不到的那一点。
    ///
    /// ⚠️ 两个刻意的保守性，都是为了**不静默放过**：
    ///  · 注释里提到的函数名也算命中（保守方向 = 多报）。所以收尾函数不许在注释里写
    ///    `mark_file_outbox_failed(` 这种带左括号的形式，要写就用别的措辞 ——
    ///    多报会立刻被人看见并改掉，漏报不会。
    ///  · 分两个分支各调一次破坏性写会被判红（文本顺序看不出分支）。合并成
    ///    "分支只决定要不要做面向用户的写，破坏性写在分支之后统一做一次" ——
    ///    本来就是更少的重复，不是为迁就判据而绕路。
    #[test]
    fn every_finalize_path_defers_the_destructive_write() {
        /// 会让这一行**从此扫不到**的写。
        const DESTRUCTIVE: &[&str] = &[
            "mark_file_outbox_failed(",
            "mark_file_outbox_cancelled(",
            "delete_file_outbox(",
            "delete_outbox_by_msg_id(",
            "delete_group_outbox_by_msg_id(",
        ];
        /// 终态在界面上的投影：气泡状态、传输台账。
        const USER_FACING: &[&str] = &[
            "set_message_status(",
            "upsert_transfer(",
            "mark_transfer_failed_if_active(",
        ];
        let mut scanned = 0usize;
        let mut bad: Vec<String> = Vec::new();
        let transport = crate::network::transport_src_for_guards();
        for src in [transport.as_str(), all_commands_src(), all_db_src()] {
            for (name, body) in top_level_fns(src) {
                // 最早的破坏性写；没有就不是收尾函数
                let Some(destroy_at) = DESTRUCTIVE.iter().filter_map(|d| body.find(d)).min() else {
                    continue;
                };
                scanned += 1;
                // 最晚的面向用户的写；没有就无从比较（如 flush 的成功分支只删行）
                let Some(last_user_write) = USER_FACING.iter().filter_map(|u| body.rfind(u)).max()
                else {
                    continue;
                };
                if last_user_write > destroy_at {
                    bad.push(format!(
                        "{name}：把行踢出重试集合的那一步在前，面向用户的写在后"
                    ));
                }
            }
        }
        assert!(
            scanned >= 4,
            "只扫到 {scanned} 个收尾函数 ⇒ 这条判据已经空转（至少该有 finalize_expired_* 两条 +              fail_file_job + cancel_file_transfer）。通常是锚点或聚合清单变了，先去核对聚合源"
        );
        assert!(
            bad.is_empty(),
            "有 {} 个收尾路径的写序反了：{bad:?}\n             破坏性写（failed/cancelled/删行）必须排最后 —— 它先跑，后续步骤失败时行已扫不到，             那条气泡就永久停在「发送中」且无人再修",
            bad.len()
        );
    }

    /// 投递失败的重试裁决必须**只有一个判据点**（#25 第 2 段）。
    ///
    /// 三条新单测证的是纯函数本身；这条守的是**调用点有没有真的在用它** —— 那正是这类
    /// "抽成纯函数"最容易漏的半边：判据搬走了、测试全绿，循环里却还留着旧的
    /// `if !retryable || over_limit`，两份判据不一致时没人发现（界面上就是"有的文件永远转圈、
    /// 有的一试就判死"）。退避毫秒数同理：调用点再写死一个 5000 就是第二份事实来源。
    #[test]
    fn the_outbox_retry_decision_has_exactly_one_judge_and_one_caller() {
        let flat = code_flat(include_str!("commands/files.rs"));
        assert_eq!(
            flat.matches("file::send_retry_verdict(&e,attempts)")
                .count(),
            1,
            "投递失败必须走那一份纯函数裁决；0 处 = 调用点被换回内联判据，三条单测集体失效"
        );
        assert!(
            !flat.contains(">=MAX_FILE_OUTBOX_RETRIES")
                && !flat.contains(">=crate::network::file::MAX_FILE_OUTBOX_RETRIES"),
            "commands 里再比一次 attempts 就是第二份判据"
        );
        // 两个分支各自必须做对那一件事。切片两端都锚在 ASCII 起始处（GiveUp / Retry），
        // 不会因为切进中文字符里 panic —— 这条写法本身是被踩过一次才定下来的。
        let give = flat
            .find("RetryVerdict::GiveUp(reason)=>{")
            .expect("找不到 GiveUp 分支");
        let retry = flat
            .find("RetryVerdict::Retry{backoff_ms}=>")
            .expect("找不到 Retry 分支");
        assert!(
            retry > give,
            "两个分支的先后锚点看不懂了（护栏需要同步更新）"
        );
        let give_arm = &flat[give..retry];
        assert!(
            give_arm.contains("fail_file_job("),
            "GiveUp 必须真的落终态，否则\"放弃\"只存在于日志里，行还留在 sending"
        );
        assert!(
            give_arm.contains("logger.warn("),
            "放弃必须留痕：只写库不记日志，事后无从查清是哪条链路、哪一次判的死"
        );
        assert!(
            flat[retry..].contains("mark_file_outbox_pending(&dbc,&transfer_id,backoff_ms)"),
            "退避时长必须由裁决给出，不是调用点写死的第二个数"
        );
    }

    /// 每小时那一趟必须**三件事都做**（P1：断链清理改成"只在 peer 全掉线时清"之后，
    /// 静默接收器的回收是唯一的兜底）。
    ///
    /// 为什么钉在这里而不是写成行为测试：这三件事的共同点是"对端不会再来敲这一单了，
    /// 得有人替它收尾"。漏掉任何一件的**表现都是永不回收**（`.part` + 文件句柄 / 中继内存表 /
    /// 静默接收器），既不报错也不影响别的功能可见性 —— 只有把它当成**一个集合**来钉才有意义。
    ///
    /// 顺序也有讲究：`sweep_stale_parts` 是"只删不在表里的"，所以接收器回收**必须在这一趟里**，
    /// 否则它摘掉表项之后要再等一小时才会轮到 `.part`。
    #[test]
    fn hourly_sweep_covers_parts_relay_and_stalled_receivers() {
        let src = include_str!("lib.rs");
        let at = src
            .find(".part 清扫任务失败")
            .expect("找不到每小时清扫任务（这条护栏会空转）");
        // 窗口必须**止于测试模块**：本文件末尾就有 `mod tests`，而它自己写着这些字面量 ——
        // 取到文件末尾等于"护栏在自己的源码上找自己的名字"，永远命中、永远绿。
        // （同一个自参照坑已经坑过两次：`!tail.contains(FILE_RECEIVE…)` 那半截也是这么假过的。）
        // 窗口止于**文件末尾**：测试整段已在 lib_tests.rs，lib.rs 里不会再有自参照的名字。
        // 同时钉住"lib.rs 只许生产码"这条搬家后的新边界。
        assert!(
            !src.contains("#[cfg(test)]"),
            "lib.rs 现在只该是生产码（测试在 lib_tests.rs）；这条护栏的窗口要重新界定"
        );
        let end = src.len();
        let tail = &src[at..end];
        for call in ["sweep_stale_relay(", "sweep_stalled_receives("] {
            assert!(
                tail.contains(call),
                "每小时清扫少了 {call} —— 内存态就没人回收了"
            );
        }
        // 超时判据必须只有一份，且在 network 侧（这里不许长出第二份"多久算静默"）
        assert!(
            !tail.contains("FILE_RECEIVE_IDLE"),
            "lib.rs 不该自己算接收超时——判据在 file::receive_is_stale"
        );
    }
