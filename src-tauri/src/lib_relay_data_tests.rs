// 职责边界：
// - `lib_tests.rs` 测试分册之6 —— 公网中继数据面：定向策略、回收终态落库在锁外、口令不进日志、电路标 Relay
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
    /// 数据面转发必须吃中继策略（2026-09-19 审计 P0#5 回归护栏）。
    ///
    /// 为什么必须守：控制面（gossip）从 ADR-0016 起就走 `decide_forward`，而数据面
    /// （定向借道 / RelayChunk / OpaqueExternal）曾长期裸奔 —— 用户把中继设成
    /// 「关闭」，文件分片照样借他的带宽一跳一跳地跑，**设置项只有一半是真的**。
    /// 这类"开关只管一条路径"的分裂在 UI 上完全看不出来，只能源码钉死。
    ///
    /// ⚠️ 2026-10-07 改严：这条原来钉的是 `decide_relay_from_peer(` 出现 **≥3 次**，
    /// 而"三次"正是三段抄写的产物 —— 于是它把缺陷形状当成了要求：谁把闸收成一处 helper
    /// 让三个消费者共调，计数掉到 1，这条判据当场红，而代码其实变好了（最顺手的消红动作
    /// 是再抄第四遍）。现在钉的是**形状**：策略判定恰好一个家 + 三个转发点各自走它。
    /// 两条反证（本轮实测过，不是设计意图）：摘掉任意一个消费者 ⇒ `wired` 少一 ⇒ 红；
    /// 把闸内联回任一调用点 ⇒ `homes` 变 2 ⇒ 红。
    #[test]
    fn relay_data_plane_respects_policy() {
        let transport = crate::network::transport_src_for_guards();
        // ① 闸只有一个家：策略判定在整份生产码视图里恰好出现一次（就在 relay_denied 内部）。
        let homes = transport.matches("decide_relay_from_peer(").count();
        assert_eq!(
            homes, 1,
            "中继授权闸必须只有 relay_denied 一个家，实际 {homes} 处 —— \
             多于 1 处就是把同一段判断抄在多个转发点里（这条判据以前正替那个形状把关）"
        );
        // ② 那个家必须是**函数**，不是一处裸调用（防止有人把三处合并成一处后删掉 helper）。
        let defs = transport.matches("fn relay_denied(").count();
        assert_eq!(defs, 1, "relay_denied 必须是唯一的闸函数，实际 {defs} 处定义");
        // ③ 三个数据面转发点各自经过它：定向借道 / OpaqueExternal 转投 / RelayChunk 转投。
        let wired = transport.matches("if relay_denied(").count();
        assert_eq!(
            wired, 3,
            "数据面三个转发点（定向借道 / RelayChunk / OpaqueExternal）都要走 relay_denied，\
             实际 {wired} 处 —— 少一个就是那条路径又变成不经策略裸奔（P0#5 的原病灶）"
        );
        // gossip 控制面原有闸不得被拆掉
        assert!(
            transport.contains("decide_forward("),
            "gossip 转发的 relay 授权闸（decide_forward）被删了？"
        );
    }

    /// 中继态回收（审计 A2）：被回收的传输必须**落 DB 终态**并 emit，且 emit 在 db 锁**之外**。
    ///
    /// 两条各挡一种退化：① 只 `retain` 内存不写库 ⇒ DB 行永远停在 active/某个百分比，
    /// 接收端界面永久卡 X%（旧行为）；② 顺手在 `for id in &removed { 写库 + emit }` 里 emit
    /// ⇒ 把阶段 2 刚消灭的「锁内慢活」请回来（前端收到 file-failed 后下一次 IPC 要抢同一把锁）。
    #[test]
    fn relay_reclaim_finalizes_in_db_and_emits_outside_the_lock() {
        let src = crate::network::transport_src_for_guards();
        let body = rust_fn_body(&src, "pub fn sweep_stale_relay(");
        assert!(
            body.contains("mark_transfer_failed_if_active("),
            "回收中继态必须把仍 active 的传输行落 failed，否则接收端界面永久卡在百分比上"
        );
        let lock_at = body
            .find("state.db.lock()")
            .expect("回收必须写库（db 锁）才能落终态");
        // ⚠️ 探针刻意**不带左括号**：`src/api/events.test.ts` 扫 Rust 源码时不抹字符串字面量
        // （它必须看见 `emit("x")` 里那个串本身才认得出事件名），于是守卫里写全 `emit(` 会被
        // 当成一个真实发送点、把它后面那截文本当成"事件名" ⇒ 结构门禁无故变红。
        let emit_at = body
            .find("state.app.emit")
            .expect("回收必须 emit file-failed：只写库不发事件的话前端要等下一次刷新才知道");
        assert!(
            !body.contains(".db.lock().unwrap()"),
            "db 取锁必须抗中毒（`.unwrap_or_else(|e| e.into_inner())`）"
        );
        assert!(
            code_flat(&body[lock_at..emit_at]).contains("};"),
            "emit 必须在 db 锁作用域**之外**：锁内只收集要通知的 id，出锁再 emit"
        );
    }

    /// 中继发送不许宣称「未经证明的成功」（2026-09-23 审计 A1 的 L1 那一半）。
    ///
    /// 四处必须同时成立，拆掉任何一处就退回"写出 = 送达"那个假成功：
    /// ① `relay_send_to_neighbors` 交出**接住该帧的邻居数**（旧实现把每个 `try_send` 的结果
    ///    用 `let _ =` 丢掉，于是"没人接住"和"全都接住"在调用方看来一模一样）；
    /// ② `relay_push_file` 把它当**下限判据**用在两处：Offer 与每一片。注意方向 ——
    ///    `0 ⇒ 必然没送出` 才可用，`≥1` 不代表某个邻居与目标有直连，所以它不是送达证明
    ///    （真证明要等接收端回执，即尚未实施的 A1-L2）；
    /// ③ 失败必须落 DB 终态，且**只能经包装层这一个出口**：发送方向没有任何清扫器
    ///    （`sweep_stale_relay` 清的是接收侧那两张内存表），漏一条 Err 路径就是一行 active
    ///    永久挂在"进行中"，重启后又被捞出来显示百分比；
    /// ④ 推流之前插的那条系统消息必须是「请求下载」而不是完成时态 —— 写下它的那一刻，
    ///    这条链上还没有任何成功证据。
    /// 为什么是源码守卫：判定需要"邻居接住数为 0"的链路夹具 + 事件捕获，本仓没有这种夹具。
    #[test]
    fn relay_send_does_not_claim_unproven_success() {
        let tv = crate::network::transport_src_for_guards();
        let relay = rust_fn_body(&tv, "pub(crate) async fn relay_send_to_neighbors(");
        assert!(
            relay.contains("-> usize"),
            "relay_send_to_neighbors 必须返回接住该帧的邻居数 —— 返回 () 就等于把假成功留给调用方"
        );
        assert!(
            code_flat(&relay).contains("try_send(state,&p,msg).await.is_ok()"),
            "计数必须建立在 try_send 的成功判定上（`let _ =` 丢弃返回值就永远数不出来）"
        );

        // 视图读"一个家"`network::file_src_for_guards()`（2026-10-07 file.rs 按角色切成 include! 分册）：
            // 读单个文件只会看见主册 ⇒ 形状守卫对分册失明（假绿形状）。登记对账由 lib_source_view_tests 第四个用例钉两侧。
                    let file = &crate::network::file_src_for_guards();
        let push = rust_fn_body(file, "async fn relay_push_file(");
        let flat_push = code_flat(&push);
        assert_eq!(
            flat_push.matches(".await==0{returnErr(").count(),
            2,
            "Offer 与每一片各一处「0 个邻居接住 ⇒ 失败」的判据，少一处就是又退回无条件发送。\
             探针带 `.await` 与 `return Err` 两段形状：本函数里还有一处无害的 `if size == 0`，\
             只数 `==0{{` 会把它算进来（实测第一次就跑出 3）。刻意不再单列「调用处数」那条断言：\
             它被这条完全覆盖（少调用必然同时少判据），留着只会让变异用例打在它前面那条上、\
             得到一句弱确认（2026-09-23 真踩过）。"
        );

        // L2 那一半：写出 ≠ 送达。走完分片循环只证明"每一片被至少一个邻居接住"，
        // 所以终态只能是 `sent`，也不能广播"本机这条已完成"的事件。
        assert!(
            flat_push.contains("\"send\",\"sent\",") && !flat_push.contains("\"send\",\"done\","),
            "中继推送写完分片必须落 sent：直接写 done 就是无证据的成功\
             （验收红线「不要因为 TCP write 成功就认为消息已送达」）"
        );
        assert!(
            !flat_push.contains("\"file-done\""),
            "没有回执的推送不许广播完成事件：前端 onFileDone 会把内存里那行写成 done，\
             于是「库里 sent、界面上 ✓」。只准发 file-progress（那说的是本机写出进度）。\
             探针带引号，避开的正是本函数注释里那句反引号包裹的同名事件。"
        );

        let wrap = rust_fn_body(file, "pub async fn send_file_via_relay(");
        assert!(
            wrap.contains("mark_transfer_failed_if_active("),
            "失败必须落 DB 终态：发送方向没有清扫器，漏一条 Err 路径就是一行 active 永久挂着"
        );
        assert!(
            !wrap.contains("return Err("),
            "落终态依赖「唯一出口」这个形状：判据留在 relay_push_file 里，包装层只做 outcome→终态；\
             包装层里任何提前 return 都会绕过落终态，那就又是「界面说失败、库里说进行中」"
        );

        assert!(
            tv.contains("请求下载你的文件"),
            "共享下载的系统消息必须是「请求下载」：插它的时候还没有任何成功证据"
        );
        assert!(
            !tv.contains("下载了你的文件"),
            "「下载了你的文件」是在证据之前下的断言 —— 中继发送失败时它就成了一句假话"
        );
    }

    /// 中继收文件：① 完整性校验必须对**已按 seq 归位的字节一次性**算（审计 1.8）；
    /// ② 归位的字节必须在**磁盘上**，不在内存里（架构复审 P4）。
    ///
    /// 后果链（1.8）：旧实现逐片"到达即喂"增量哈希 —— 但中继链路分片天然**重复**
    /// （多邻居泛洪各送一份）且**乱序**（多路径时延不同），喂哈希发生在去重/排序
    /// 之前 ⇒ 分片收齐却必然校验失败：接收端报"文件完整性校验失败"、发送端却显示
    /// 成功（无回执），两端状态互相矛盾且无重试路径。
    ///
    /// 后果链（P4）：重组表原先是 `chunks: HashMap<u32, Vec<u8>>`，完成时再组装出第二份
    /// ⇒ 峰值 ≈ 2× 文件大小（600MB 文件 = 1.2GB 内存，移动端必被系统杀掉），
    /// 而当时的尺寸闸门只有一句 `size > i64::MAX`，等于没有。
    ///
    /// 判据分两组，都取**语义形状**：
    /// ① `handle_relay_chunk` 里不得出现 `.hasher`（增量喂哈希的旧写法），
    ///    校验点必须是对文件整体流式算的 `sha256_file_hex(`；
    /// ② `file_relay.rs` 的 `Reassembly` 不得再持有 `Vec<u8>` 载荷、`add_chunk`
    ///    必须真的 `seek + write_all` 落盘，且开档时预分配（`set_len`）——
    ///    少任何一件，"内存与文件大小无关"这个结论就不成立。
    #[test]
    fn relay_receive_hashes_assembled_plaintext_once() {
        let src = crate::network::transport_src_for_guards();
        let body = rust_fn_body(&src, "async fn handle_relay_chunk(");
        assert!(
            !body.contains(".hasher"),
            "handle_relay_chunk 里出现增量哈希：分片按到达顺序喂、在去重/排序之前，\
             重复与乱序都会算错 ⇒ 收齐了也报校验失败（审计 1.8）"
        );
        assert!(
            body.contains("sha256_file_hex("),
            "完整性校验必须对**已归位的整份字节**一次性算（流式 `sha256_file_hex`，\
             审计 1.8 + P4）：既不能逐片喂，也不能先把整份读回内存再 digest"
        );
        assert!(
            !body.contains("Sha256::digest(&"),
            "又出现了「把一份完整缓冲喂给 digest」的写法：中继接收的内存峰值必须与\
             文件大小无关（P4）"
        );

        let relay = include_str!("file_relay.rs");
        let begin = rust_fn_body(relay, "pub fn begin_reassemble(");
        let add = rust_fn_body(relay, "pub fn add_chunk(");
        assert!(
            begin.contains("set_len("),
            "开档必须预分配到声明的尺寸：既是「写到 offset 之外当场可判」的依据，\
             也让乱序落盘不必自己补零（P4）"
        );
        assert!(
            add.contains("SeekFrom::Start(") && add.contains("write_all("),
            "add_chunk 必须按 `seq × chunk_size` 直接写盘，而不是把分片存进内存表（P4）"
        );
        let struct_at = relay
            .find("pub struct Reassembly {")
            .expect("找不到 Reassembly —— 改名要同步这条守卫");
        let struct_body = &relay[struct_at..relay[struct_at..].find("\n}").unwrap() + struct_at];
        assert!(
            !struct_body.contains("Vec<u8>"),
            "Reassembly 又持有 `Vec<u8>` 载荷 = 退回整份驻内存，那正是 P4 要消灭的形状：{struct_body}"
        );
        assert!(
            struct_body.contains("HashSet<u32>") && struct_body.contains("file:"),
            "Reassembly 只许持有「收到过哪些 seq」+「已打开的文件句柄」两样：\
             多了任何一份字节缓冲都是 P4 复发"
        );
    }

    /// **解除好友关系必须同时解除内存里的身份绑定**（用户 2026-09-13 真机：不然"必须重启"）。
    ///
    /// 好友身份锚点的**绑定来源**必须问同一道闸（#32 第一片）。
    ///
    /// 后果链：`friends.ed25519_pubkey` 是 Hello 的验签锚点（INV-P21）与安全码的输入，
    /// 现在还是公网中继电路的准入判据（`list_bound_friend_identities` 只看它非空）；
    /// 而写入是 fill-only —— 首写者永久胜出。此前 `upsert_peer` 要求 `keys_verified`，
    /// 三条"成为好友"的路径读的却是**同一张 `peers` 表**且不过闸 ⇒ 一次伪造的 UDP announce
    /// 就能永久钉死锚点（E2EE 被击穿之外，还多了一条"我们主动跨公网给它建电路"）。
    ///
    /// 三件判据：① 三处都走同一个 helper（闸只有一份）；② 写钥匙的直调只许出现在
    /// "自己问过闸"的那几处；③ 三条"验签通过"的握手都必须打标 —— 漏一条就是
    /// "验过签却不标"，让收紧后的锚点永远补不上（安全改动做成可用性回退）。
    /// **口令值不许进日志、也不许进诊断事件**（`INTEGRATION.md` 要求 7 的硬部分）。
    ///
    /// 这条链路上"服务器口令"是唯一准入手段，而调试时最容易手滑写出的就是
    /// `format!("token={}", cfg.token)` 那一行 —— 日志会被导出、会被贴进求助帖。
    /// 判据按**语句**切（`;` 分隔）而不是按行：日志调用普遍跨行，逐行扫会漏掉真正插值的
    /// 那一行，那样这条守卫就成了摆设。
    ///
    /// 允许的唯一形态是"只报长度"（`token_len={}` + `chars().count()`），
    /// `save_relay_config` 与 `check_relay_server` 就是这么写的。
    #[test]
    fn relay_token_never_reaches_logs_or_diagnostics() {
        const VALUE_EXPRS: [&str; 2] = [".token", "token_norm"];
        const COUNT_ONLY: [&str; 2] = ["token_len", "chars().count()"];
        let sources = [
            ("transport", crate::network::transport_src_for_guards()),
            ("commands", all_commands_src().to_string()),
        ];
        // 先自证"这两份视图真的看得见中继"。`transport/relay.rs` 与 `commands/relay.rs` 都是
        // `include!` 进同一模块的分册，而守卫读的是**文件文本** —— 漏登记不会报错，
        // 只会让这条守卫扫描不到任何中继代码，于是**永远绿**（假绿比假红危险）。
        assert!(
            sources[0].1.contains("fn relay_negotiate("),
            "transport 视图里没有 `transport/relay.rs` 分册 ⇒ 这条守卫扫不到拨号器，会假绿。\
             新增分册时要在 `network::transport_src_for_guards()` 里同步登记一行。"
        );
        assert!(
            sources[1].1.contains("fn check_relay_server("),
            "commands 视图里没有 `commands/relay.rs` 分册 ⇒ 这条守卫扫不到中继命令，会假绿。\
             新增分册时要在 `all_commands_src()` 里同步登记一行。"
        );
        let mut offenders: Vec<String> = Vec::new();
        for (name, src) in sources {
            for stmt in src.split(';') {
                if !(stmt.contains("logger.") || stmt.contains("push_diag_event")) {
                    continue;
                }
                if !VALUE_EXPRS.iter().any(|p| stmt.contains(p)) {
                    continue;
                }
                if COUNT_ONLY.iter().any(|p| stmt.contains(p)) {
                    continue;
                }
                offenders.push(format!(
                    "[{name}] {}",
                    stmt.trim().replace('\n', " ").trim()
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "把口令**值**写进了日志或诊断事件（要求 7：不写日志、不进上报、不出现在诊断截图里）。\
             要留痕就只报长度：{}{}",
            "token_len={}",
            ", …chars().count()"
        );
    }

    /// 中继会合拨号必须把电路登记成 `PathKind::Relay`，**不能复用 `Routed`**。
    ///
    /// 为什么钉接线而不是只钉枚举：`path_rank` / `best_link_kind` / `path_kind_names_are_stable`
    /// 的单测只证明"枚举里有 Relay、优先级对、串名对"。把 relay.rs 那处拨号改回
    /// `PathKind::Routed`，它们**照样全绿** —— 而后果是用户能看见的：中转电路又会被界面标成
    /// 「跨网段 / VPN」、并和真正的 VPN 直达路径挤进同一个选路优先级（用户 2026-09-22 要求
    /// "局域网 / VPN / 蓝牙 / 公网中转四种通道全局统一"）。判据落在**唯一的那处拨号构造点**。
    #[test]
    fn relay_circuit_is_tagged_relay_not_routed() {
        let src = crate::network::transport_src_for_guards();
        // 自证视图里真的有中继分册（漏登记 ⇒ 扫不到拨号点 ⇒ 永远绿 = 假绿）。
        assert!(
            src.contains("fn relay_rendezvous_task("),
            "transport 视图里没有 `transport/relay.rs` 分册 ⇒ 这条守卫扫不到拨号点，会假绿。\
             新增分册时要在 `network::transport_src_for_guards()` 里同步登记一行。"
        );
        let body = rust_fn_body(&src, "pub async fn relay_rendezvous_task(");
        assert!(
            body.contains("PathKind::Relay"),
            "中继会合拨号必须用 PathKind::Relay 登记电路；改回 Routed 会让中转被标成「跨网段 / VPN」、\
             并与 VPN 直达路径同优先级（四种通道必须可区分）"
        );
    }
