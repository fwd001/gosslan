// 职责边界：
// - `lib_tests.rs` 测试分册之9 —— 跨版本与能力门：版本只声明不签名、新帧类型在发送口门控、受众三态
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
    /// **对端版本必须「声明但不签名、记录但会回收」**（ADR-0007 决策 1 / INV-P24）。
    ///
    /// 为什么只能钉源码：这三个环节各自的行为都是"什么都没发生"，
    /// 没有哪个测试会因为**漏了**其中一条而失败，而每条漏掉的后果都不轻：
    /// - 进了签名材料 ⇒ 老端验签失败 ⇒ "报版本"本身变成破坏性变更（老设备直接连不上）；
    /// - 不记录 ⇒ 未知帧日志与诊断面板都无从解释"到底谁版本高"；
    /// - 不回收 ⇒ 节点进出比删好友频繁得多，长跑后这张表无界增长。
    #[test]
    fn peer_version_is_declared_not_signed_and_reclaimed() {
        let proto = include_str!("protocol.rs");
        let sig = rust_fn_body(proto, "pub fn hello_signing_bytes(");
        for f in ["protocol_version", "app_version"] {
            assert!(
                !sig.contains(f),
                "{f} 不得进入 Hello 签名材料（否则老端验签失败，加字段=断兼容）"
            );
        }
        let transport = crate::network::transport_src_for_guards();
        let built = rust_fn_body(&transport, "pub fn build_signed_hello(");
        assert!(
            code_flat(&built).contains("protocol_version:Some(crate::protocol::PROTOCOL_VERSION)"),
            "本机 Hello 必须声明 PROTOCOL_VERSION，否则对端永远看不到我们的版本"
        );
        assert!(
            code_flat(&built).contains("app_version:Some(crate::protocol::current_app_version("),
            "本机 Hello 必须声明 app_version（只给人看，但诊断面板要靠它认人）"
        );
        let hello_arm = rust_fn_body(&transport, "pub async fn handle_message(");
        assert!(
            hello_arm.contains("peer_versions"),
            "Hello 到达时必须记录对端声明的版本（TCP 与 BLE 共用这一个写入点）"
        );
        let cmds = all_commands_src();
        assert!(
            cmds.contains("result.peer_versions = collect_peer_versions("),
            "诊断面板必须真的把对端版本取出来 —— 只存不读等于没有"
        );
        let disc = include_str!("network/discovery.rs");
        assert!(
            disc.contains("peer_versions"),
            "sweep_peers 必须回收 peer_versions（与 peer_content_features 同一回收点）"
        );
    }

    /// INV-P24 第 4 条的**接线**：门控判据存在、唯一，1:1 发送路径真的问它，
    /// 而且挡下时走的是三态文案入口（判据唯一 + 文案唯一是同一件事的两半）。
    ///
    /// 为什么不能只测判据函数：判据写得再对，发送点不调用 = 没有门控。这条守卫
    /// 就是为了让"忘了接"这件事变成编译期之后立刻能看到的红。
    /// 背景（本轮 RCA 查出来的真实现场，不是假想需求）：`MsgKind::Merge` 是 V1 期间
    /// （`9b26006`）才加的，v4.8.2 / v4.18.10 / v4.20.0 三个已发布版本里没有这个变体 ——
    /// 它们的 `ChatMessage.kind` 仍是枚举，收到 `kind:"merge"` 会整帧丢掉。
    ///
    /// 4.31.23 起 1:1 问的是 `dm_allowed_by_features`（那张表额外要求对端声明
    /// "能吃任意 wire kind"），所以这里的锚点跟着换成那一个 —— **但只换锚点不算数**：
    /// 下面同时钉住"1:1 不许绕过它去问全局那一位"，否则这条守卫会被"改回旧函数"
    /// 这一种改法静默通过，而那条路会让 1:1 少掉一半判据。
    #[test]
    fn new_message_kinds_are_gated_at_the_send_path() {
        let proto = include_str!("protocol.rs");
        for f in [
            "pub fn kind_required_feature(",
            "pub fn kind_allowed_by_features(",
            "pub fn dm_required_features(",
            "pub fn dm_allowed_by_features(",
            "pub fn kind_blocked_hint(",
        ] {
            assert_eq!(
                proto.matches(f).count(),
                1,
                "{f} 全仓只许一处，第二处迟早口径不同"
            );
        }
        let table = rust_fn_body(proto, "pub fn kind_required_feature(");
        assert!(
            table.contains("\"merge\""),
            "merge 必须仍在门控表里：v4.20.0 及更早的 MsgKind 没有这个变体"
        );

        let chat = include_str!("commands/chat.rs");
        let body = rust_fn_body(chat, "pub async fn send_message(");
        let gate = body
            .find("dm_allowed_by_features(")
            .expect("1:1 发送路径必须问门控判据，否则老对端会静默丢帧");
        assert!(
            body.contains("dm_required_features(&kind)"),
            "1:1 的『这个 kind 要不要门控』也必须问那一个家（只问放行、不问要求 = 判据半接）"
        );
        assert!(
            !body.contains("kind_allowed_by_features("),
            "1:1 不许绕过 dm_allowed_by_features 去问全局那一位：它不含『对端能不能吃任意 wire kind』，\
             绕过去的后果正是这条守卫要挡的那次事故（老端丢帧 + 断链）"
        );
        // 挡下时走的是**三态**文案入口。绕过它直接引用旧那句的后果不是排版，是假指控：
        // `peer_content_features` 是内存表，对方一离线就被 sweep 掉 ⇒ 缺条目通常只代表
        // "此刻不知道"，而旧那句说的是"它版本较旧"。
        assert!(
            body.contains("kind_blocked_hint("),
            "门控挡下时要按\"对端声明过缺位\"与\"能力未知\"分两句说"
        );
        assert!(
            !body.contains("kind_unsupported_hint("),
            "1:1 发送口不许绕过 kind_blocked_hint 自己去挑文案"
        );
        // 给自己发的那条分支在前 —— 自聊不经过网络，不该被判"对方版本不支持"。
        let self_branch = body
            .find("insert_self_message(")
            .expect("自发消息分支不见了，顺序判据失去锚点");
        assert!(
            self_branch < gate,
            "门控必须排在自发消息分支之后，否则给自己发合并转发也会被挡"
        );
        // 挡在公钥查找之前：这条根本不会发出去，不该再触发 who_has 探测白等 1.2s。
        let probe = body
            .find("let pubkey = {")
            .expect("公钥查找块不见了，顺序判据失去锚点");
        assert!(
            gate < probe,
            "门控要早于公钥探测，白探测一次会卡用户 1.2 秒"
        );
    }

    /// 群聊侧的「受众预告」必须接线，而且必须与 1:1 的门控**不共用默认值**。
    ///
    /// 为什么单独一条：本轮考古查明群 kind 走 Gossip 载荷、老成员不丢帧只看成原始文本，
    /// 所以群口刻意**不拦发送**（1:1 那边才是"不门控不许发"）。这个区别全靠
    /// `kind_audience` 把"未知"与"确知不支持"分开数 —— 一旦有人图省事把它并进
    /// `kind_allowed_by_features` 的"传 0"口径，提示就会在离线成员在场时永远响，
    /// 而一条永远在响的提示等于没有提示。这里钉四件事：判据唯一、群口真的问它、
    /// 命令注册在 generate_handler 里（漏注册的历史事故是安卓端 8 个 E0433）、
    /// 以及"未知算 unknown"这条分支还在。
    #[test]
    fn group_audience_is_wired_and_keeps_three_states() {
        let proto = include_str!("protocol.rs");
        for f in ["pub fn kind_audience(", "pub fn kind_audience_hint("] {
            assert_eq!(
                proto.matches(f).count(),
                1,
                "{f} 全仓只许一处，第二处迟早口径不同"
            );
        }
        let body = rust_fn_body(proto, "pub fn kind_audience(");
        assert!(
            body.contains("=> unknown += 1"),
            "没交换过 Hello / 不在线的成员必须记成 unknown，不许并进 unsupported"
        );
        assert!(
            !body.contains("unwrap_or(0)"),
            "群受众判据不得借用 1:1 那个「不知道就当不支持」的默认值"
        );

        let group = include_str!("commands/window.rs");
        let body = rust_fn_body(group, "async fn send_group_payload(");
        assert!(
            body.contains("kind_audience(") && body.contains("kind_audience_hint("),
            "群内核必须复用同一份判据与文案；自己拼一份第二处就会与第一处漂移"
        );
        assert!(
            body.contains("let _ = s.app.emit") && body.contains("\"content-audience\""),
            "群口的提示必须是 fire-and-forget（`let _ =`），且发在前端听得见的那个事件上"
        );
        // ⚠️ 上面两条刻意**不写成带左括号的那种形态**：`src/api/events.test.ts` 是按文本扫
        // Rust 源码里的调用点找事件名的，它分不清"代码里的调用"与"测试字符串/注释里的片段"
        // —— 这条断言一旦把那三个字符写全，就会被扫成一个孤儿事件（同类坑本仓库已记过一次：
        // 守卫注释里出现被扫描的关键字，于是守卫把自己判红）。
        // "不拦发送"要判的是**受众那一段**，不是整个内核 —— 内核里"群不存在 / 你已不在该群
        // / 群密钥缺失"这三条本来就该拒发，拿它们当违反者会把守卫判成假红（第一次就红了）。
        let audience_at = body
            .find("kind_audience(")
            .expect("内核没调用 kind_audience");
        let after_audience = &body[audience_at..];
        assert!(
            !after_audience.contains("return Err(") && !after_audience.contains("?"),
            "受众判定之后必须只发提示、不改发送结果 —— 群老成员不丢帧，拦整群消息没有收益"
        );
        // 事件名与监听者的两端都由 `src/api/events.test.ts` 双向核对（那条守卫本来就是
        // "Rust 发的事件必须有人听"）—— 这里不再抄一份，免得两处规则各说一遍。
    }

    /// **内容拉取必须走能力协商**（ADR-0019 Phase 3）：旧端不发新帧、新端才拉；
    /// 且能力位**不能进 Hello 签名材料**，否则老端验签会失败（向后兼容的硬前提）。
    #[test]
    fn content_pull_requires_capability_negotiation() {
        let proto = include_str!("protocol.rs");
        assert!(proto.contains("CONTENT_FEATURE_PULL"));
        let sig = rust_fn_body(proto, "pub fn hello_signing_bytes(");
        assert!(
            !sig.contains("content_features"),
            "content_features 不得进入签名材料（否则老端验签失败）"
        );
        let cmds = all_commands_src();
        let body = rust_fn_body(cmds, "pub async fn request_content(");
        assert!(
            body.contains("CONTENT_FEATURE_PULL"),
            "拉取必须按对端能力位协商，旧端不发新帧"
        );
        // 按裸 cid 的变体（合并转发卡片的读侧）同样必须协商 —— 卡片载荷里的 cid
        // 来自对端声明，不协商就发帧会让旧端收到理解不了的帧。
        let body_by_cid = rust_fn_body(cmds, "pub async fn request_content_by_cid(");
        assert!(
            body_by_cid.contains("CONTENT_FEATURE_PULL"),
            "request_content_by_cid 也必须按对端能力位协商"
        );
        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("find_source"),
            "服务端必须按 cid 找本地内容"
        );
        assert!(
            transport.contains("db::get_group"),
            "群成员也应能作为拉取请求方（A→B 成功后，C 可从已收完的 B 拉）"
        );
    }

    /// **未完成的内容要能自动重试，且同样受能力协商约束**（ADR-0019 Phase 1）。
    #[test]
    fn incomplete_content_is_auto_retried_behind_capability_gate() {
        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("async fn retry_incomplete_content("),
            "必须实现建链自动重试"
        );
        assert!(
            transport.contains("CONTENT_FEATURE_PULL"),
            "自动重试也必须走能力协商（旧端不发新帧）"
        );
        let cmds = all_commands_src();
        assert!(
            cmds.contains("pub fn get_content_transfers("),
            "必须有统一状态查询命令（前端气泡据此显示）"
        );
        let model = include_str!("content/model.rs");
        assert!(
            model.contains("Serialize, Deserialize, Clone, Debug, PartialEq"),
            "TransferRecord 必须可序列化给前端"
        );
        // 视图读"一个家"`network::file_src_for_guards()`（2026-10-07 file.rs 按角色切成 include! 分册）：
            // 读单个文件只会看见主册 ⇒ 形状守卫对分册失明（假绿形状）。登记对账由 lib_source_view_tests 第四个用例钉两侧。
                    let file = &crate::network::file_src_for_guards();
        assert!(
            file.contains("record_failure"),
            "中途失败/断链必须在 fail_receive 里记 Incomplete，否则记录永远停在 Active、自动重试不触发"
        );
        assert!(
            file.contains("pub fn resume_receive("),
            "必须有断点续传接收（从 .part 前缀继续）"
        );
        assert!(
            transport.contains("send_file_from_path_at"),
            "服务端必须支持从偏移续发（from_bytes）"
        );
        // 审计 §7 风险 1：接收端必须把"我已有多少字节"回给发送端，发送端据此续发（不重头覆盖）。
        // 判据改成"经过那一份 `decide_offer`" —— 变量名允许变，**把真实位置回出去**这件事不许变。
        assert!(
            transport.contains("file::decide_offer(")
                && transport.contains("file::OfferDecision::ResumeFrom(held)")
                && transport.contains("received: held"),
            "接收端必须按真实已收字节回 FileReject.received，发送端据此续发"
        );
        assert!(
            transport.contains("file::receiver_progress(state, &transfer_id)")
                && transport.contains("file::retained_part_len(state, &transfer_id)"),
            "`held` 的两个来源都要在场：有活跃接收器时用内存计数，没有时才是磁盘前缀"
        );
        // 审计 §7 风险 2：必须有过期 .part 的定期清扫。
        assert!(
            file.contains("pub fn sweep_stale_parts("),
            "必须有 .part 定期清扫（可恢复失败会保留前缀，不能让它们无限堆积）"
        );
    }
