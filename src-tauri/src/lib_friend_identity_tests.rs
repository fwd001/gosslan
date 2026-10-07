// 职责边界：
// - `lib_tests.rs` 测试分册之8 —— 好友与身份：申请/自动接受/pending 排除、三条成为好友的路径各绑一次公钥
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
    /// **好友申请必须"发出即登记、建链补发"**（用户真机："已发送"但对方什么都没收到）。
    ///
    /// 好友申请是**没有回执**的定向帧：链路正好在那一刻抖动（BLE 镜像互拨打断链路）时
    /// 它会静默丢失，而发送方界面依然显示「已发送，等待对方确认」。
    /// 现在：命令**先登记再发**；任何传输建链/Hello 补全时补发一次；
    /// 收到同意或拒绝（`forget_pending_request`）后清除 —— 所以不会无限重发。
    #[test]
    fn friend_request_survives_a_dropped_link() {
        let commands = all_commands_src();
        let i = commands
            .find("pub async fn send_friend_request(")
            .expect("send_friend_request 必须在");
        let body = &commands[i..i + 1200];
        assert!(
            body.contains("pending_out_requests"),
            "命令必须**先登记**再发（丢了才知道要补发）"
        );
        assert!(
            body.contains("send_friend_request_via_link"),
            "发送逻辑要复用同一个实现（补发走的是同一条路径）"
        );
        let transport = crate::network::transport_src_for_guards();
        assert!(
            transport.contains("pub async fn flush_pending_friend_request("),
            "必须有补发入口"
        );
        assert!(
            transport.contains("flush_pending_friend_request(state, &device_id).await;"),
            "建链/Hello 补全时必须调用补发（所有传输的建链都会走到那里）"
        );
        let forget = rust_fn_body(&transport, "pub fn forget_pending_request(");
        assert!(
            forget.contains("pending_out_requests"),
            "收到同意/拒绝后必须清掉登记，否则会一直补发"
        );
    }

    /// **每一条"同意好友"的路径都必须清掉那条申请**。
    ///
    /// 真实缺陷（用户 2026-09-12 真机实测）：双方互发过申请时，A 点了同意，B 的「新朋友」里
    /// 那条申请**还在** —— 直连路径（`Message::FriendAccept`）只加了好友、忘了清 pending，
    /// 而跨跳路径（`GossipKind::FriendAccept`）清了。同一件事两条路径行为不一致，
    /// 表现成"有时候会清、有时候不清"。这里把"两条路径都要清"钉死。
    #[test]
    fn every_friend_accept_path_forgets_the_pending_request() {
        let transport = crate::network::transport_src_for_guards();
        let transport_f = code_flat(&transport);
        // 2026-09-28 复审改的形状：原先两条链路各写一遍"清哪几张表 + 要不要通知"，
        // 而**直连那一条既漏了出站登记、也漏了去重门控** ⇒ 收成一个 `apply_friend_accept`。
        // 判据因此从"两处各数一遍调用"改成"两条链路都走这个家 + 这个家自己做那两件事"
        // —— 更严：少一条消费者当场红，家里少一件事也当场红。
        assert_eq!(
            transport_f.matches("apply_friend_accept(state,&from,").count(),
            2,
            "两条 FriendAccept 路径（直连 `Message::FriendAccept` + 跨跳 `GossipKind::FriendAccept`）\
             都必须走同一个家 —— 各写一遍正是这次抓到的缺陷形状"
        );
        let home = rust_fn_body(&transport, "pub(crate) fn apply_friend_accept(");
        let home_f = code_flat(&home);
        assert!(
            home_f.contains("forget_pending_request(state,from)"),
            "这个家必须清那条申请（入站与出站两张表都在 `forget_pending_request` 里）"
        );
        assert!(
            home_f.contains("ifwas_friend{") && home_f.contains("show_if_enabled(state"),
            "通知要由 `was_friend` 门控 —— 重复投递不许刷屏（直连那条原先没有这一格）"
        );
        let commands = all_commands_src();
        let commands_f = code_flat(commands);
        assert_eq!(
            commands_f
                .matches("forget_pending_request(s,peer_id)")
                .count(),
            1,
            "`respond_friend_request` 的同意路径也要走同一个助手（别各写一遍）"
        );
        let helper = rust_fn_body(&transport, "pub fn forget_pending_request(");
        assert!(
            helper.contains("remove(peer_id)"),
            "助手必须真的把内存态的 pending 删掉"
        );
    }

    /// **"申请人已经是我的好友"时，这条申请必须被自动同意**（双方关系必须收敛）。
    ///
    /// 真实缺陷（用户 2026-09-12 真机实测）：B 的好友列表里已经有 A，而 A 是**重置过的账号**、
    /// 列表里没有 B。A 发申请 → 旧实现只在 B 侧插一条 pending，而 `get_pending_requests`
    /// 又会把「申请人已是好友」的条目过滤掉（那是为了修「已经是好友了、申请还挂着」）
    /// ⇒ **两边都看不到、谁也加不上**；用户只能先把 B 里的 A 删掉再加回来。
    ///
    /// 用户给的规则：既然 B 那边已经把 A 当好友，就等于 B 已经同意了 —— 直接走完整的同意路径。
    #[test]
    fn friend_request_from_existing_friend_auto_accepts() {
        let transport = crate::network::transport_src_for_guards();
        let helper = rust_fn_body(&transport, "async fn auto_accept_if_already_friend(");
        assert!(
            helper.contains("db::get_friend") && helper.contains("accept_friend_request"),
            "自动同意必须：① 真的判『他是不是已经是我的好友』；② 走**同一个** accept 实现（别各写一遍）"
        );
        let calls = transport
            .matches("auto_accept_if_already_friend(state, ")
            .count();
        assert_eq!(
            calls, 2,
            "两条 FriendRequest 路径（直连 `Message::FriendRequest` + 跨跳 `GossipKind::FriendRequest`）\
             都必须先做自动同意 —— 只修一条就会『同一件事两种行为』（这正是上一条缺陷的成因）"
        );
        let commands = all_commands_src();
        assert_eq!(
            commands
                .matches("pub(crate) async fn accept_friend_request(")
                .count(),
            1,
            "『同意好友』只能有一份实现：两套路径不一致正是『单边好友关系』这类缺陷的温床"
        );
    }

    /// **`get_pending_requests` 必须按好友关系过滤**（用户明确要求的兜底规则）。
    #[test]
    fn pending_requests_exclude_existing_friends() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "pub fn get_pending_requests(");
        assert!(
            body.contains("is_actionable_request"),
            "列表必须按好友关系过滤 —— 否则「已经是好友了申请还挂着」只能靠每条路径都记得清"
        );
        let pred = rust_fn_body(commands, "pub(crate) fn is_actionable_request(");
        assert!(
            pred.contains("!friend_ids.contains"),
            "判据必须是「不是好友才算待处理」"
        );
    }

    /// 好友身份锚点的**绑定来源**必须问同一道闸（#32 第一片）。
    ///
    /// 后果链：`friends.ed25519_pubkey` 是 Hello 的验签锚点（INV-P21）与安全码的输入，
    /// 现在还是公网中继电路的准入判据（`list_bound_friend_identities` 只看它非空）；
    /// 而写入是 fill-only —— 首写者永久胜出。此前 `upsert_peer` 要求 `keys_verified`，
    /// 三条"成为好友"的路径读的却是**同一张 `peers` 表**且不过闸 ⇒ 一次伪造的 UDP announce
    /// 就能永久钉死锚点（E2EE 被击穿之外，还多了一条"我们主动跨公网给它建电路"）。
    ///
    /// 三件判据：① 三处都走同一个 helper（闸只有一份）；② 写钥匙的直调只许出现在
    /// "自己问过闸"的那几处；③ **验签通过的握手都要打标，且打标要排在 `upsert_peer` 之后**
    /// —— 漏一条或排错序就是"验过签却不标"，让收紧后的锚点永远补不上
    /// （4.25.7 那次修的就是这个：三条握手打标不够，第四条在 `handle_message` 的 Hello 分支）。
    #[test]
    fn friend_identity_anchor_has_one_binding_rule() {
        let src = crate::network::transport_src_for_guards();
        assert_eq!(
            src.matches("bind_friend_keys_on_accept(").count(),
            3,
            "helper 定义 1 + 三条成为好友的路径各 1 = 3（少一处就是有人又写了一遍绑定逻辑）。\
             2026-09-28：直连与跨跳那两条已收进 `apply_friend_accept` 一个家 ⇒ 视图里\
             从 4 降到 3（定义 1 + 这个家 1 + 第三条路 1）。这一格**只许因为又有人绕开家而变大**"
        );
        assert_eq!(
            src.matches("peer_keys_trusted(").count(),
            3,
            "这道闸只许一份定义，被 `upsert_peer` 与 accept helper 各复用一次\
             （名字刻意不含 `mark_peer_keys_verified` 的子串，否则计数会被骗）"
        );
        // "哪一列能绑"这条判据只许一份定义、且只被 accept helper 用一次。
        // （不能用全文出现次数：新加的单测也会调用它，那会把计数骗成"重复定义"。）
        assert_eq!(
            src.matches("fn acceptable_friend_keys(").count(),
            1,
            "两列两套规矩的判据只许一处定义"
        );
        let helper = rust_fn_body(&src, "fn bind_friend_keys_on_accept(");
        assert_eq!(
            code_flat(&helper)
                .matches("acceptable_friend_keys(")
                .count(),
            1,
            "accept helper 必须把判据委托给那一份定义，而不是自己再算一遍"
        );
        assert!(
            code_flat(&helper).contains("peer_keys_trusted(state,friend_id)"),
            "accept helper 必须现问这道闸（不缓存 verified 结果，也不绕过）"
        );
        assert_eq!(
            src.matches("mark_peer_keys_verified(").count(),
            5,
            "1 处定义 + 四处打标：入站首帧 / 出站握手 / BLE 包装 / **Hello 落进 handle_message \
             时 upsert_peer 之后那一次**。最后这条才是全 transport 通用的锚点升级点，\
             删掉它 = 第一次连上的好友永远绑不上身份（安全码算不出、中继永不准入且无报错）"
        );
        // 顺序判据：打标必须在 `upsert_peer` **之后**。`mark_peer_keys_verified` 在
        // `peers` 条目不存在时是空操作，而 `upsert_peer` 新建条目恒标 `keys_verified: false`，
        // 所以"先 upsert 再 mark"是唯一成立的次序 —— 反过来写不会报错，只会静默绑不上。
        // 2026-10-07：Hello 臂随身份族搬进 transport/handle_identity.rs ⇒ 开窗锚点跟着搬家。
        // 不跟着改会怎样：下面两条 .expect 直接红（不是静默变弱），所以这条红是搬家自己报出来的。
        let hello_branch = rust_fn_body(&src, "async fn handle_identity_and_friend_messages(");
        let upsert_at = hello_branch
            .find("upsert_peer(")
            .expect("handle_message 的 Hello 分支必须经 upsert_peer 登记 peers");
        let mark_at = hello_branch
            .find("mark_peer_keys_verified(")
            .expect("handle_message 的 Hello 分支必须给验签通过的 Hello 打标");
        assert!(
            upsert_at < mark_at,
            "打标必须排在 upsert_peer 之后：条目还不存在时 mark 是空操作（见其 ⚠️ 注释）"
        );
        // 写 friends 钥匙的**生产**调用点全集：accept helper、`upsert_peer`（自带闸）、
        // Gossip 那处以"这一封能解密"为持有证明的补齐（只准绑 x25519）。
        // 刻意不用全文计数 —— 单测也会调它，那会把"测试多了"误判成"逻辑重复了"。
        assert!(
            code_flat(&helper).contains("db::update_friend_pubkeys(conn,friend_id,"),
            "accept helper 必须经唯一的写入函数落库，不许自己拼 SQL"
        );
        let upsert = rust_fn_body(&src, "fn upsert_peer(");
        assert_eq!(
            code_flat(&upsert)
                .matches("db::update_friend_pubkeys(")
                .count(),
            1,
            "`upsert_peer` 只许一处写钥匙"
        );
        assert!(
            code_flat(&upsert).contains("peer_keys_trusted(state,device_id)"),
            "`upsert_peer` 那道闸不许拆 —— 它和 accept helper 问的是同一个判据"
        );
        let gossip = code_flat(include_str!("network/transport/gossip.rs"));
        assert!(
            gossip.contains(
                "db::update_friend_pubkeys(&dbc,&env.sender_id,Some(&env.sender_pubkey),None)"
            ),
            "Gossip 那处补齐只准绑 x25519；哪天改成也写 ed25519，就等于给未验签的来源开锚点"
        );
    }

    /// 真机链路：对方重装后公钥变了 → 我们的身份表只补空、不覆盖（INV-P11）→
    /// 用户按提示删好友重新加，`friends` 表那行没了，但 `verify_hello` 还会回落到
    /// **内存 `peers` 表**里广播学来的旧公钥 ⇒ Hello 一直被硬拒 ⇒ 消息与好友申请都进不来，
    /// **只有重启**（内存清空）才回落到 TOFU。
    ///
    /// 判据：两条解除关系的路径（本地删好友 / 对方发来 FriendRemove）都必须调用
    /// `forget_peer_identity`；并且给用户的提示必须写出**可行动的下一步**
    /// （否则用户只能自己猜，或者干脆重启）。
    #[test]
    fn removing_a_friend_also_drops_the_in_memory_identity_binding() {
        let cmd = all_commands_src();
        let body = rust_fn_body(cmd, "pub async fn remove_friend(");
        assert!(
            body.contains("forget_peer_identity"),
            "`remove_friend` 必须调 `forget_peer_identity` —— 只删 friends 表那一行，\
             内存里的旧公钥会继续当信任根用（症状：删了好友重新加也没用，必须重启）"
        );
        let tr = crate::network::transport_src_for_guards();
        // 对方解除关系那条路径（Message::FriendRemove）同样要清。
        // ★ 窗口开在**处理体**上（2026-10-07 该臂随身份族搬进 transport/handle_identity.rs）：
        //   直接在全集视图里 find("Message::FriendRemove {") 会先撞上 handle_message 的那条分发臂
        //   （`m @ (… | Message::FriendRemove { .. })`），往后 1,500 字符里没有 forget_peer_identity
        //   ⇒ 判据会红得像是"分支被删了"。注意**不覆盖 tr**：本测试后面还要用全集视图找别的锚点。
        let fam_body = rust_fn_body(&tr, "async fn handle_identity_and_friend_messages(");
        let start = fam_body
            .find("Message::FriendRemove {")
            .expect("必须还有 FriendRemove 分支（本护栏锚点）");
        let tail = &fam_body[start..];
        // 取一个足够覆盖该分支的窗口（分支实现变了也不会假绿：下面断言的锚点就在分支里）
        let branch = &tail[..tail.len().min(1500)];
        assert!(
            branch.contains("forget_peer_identity"),
            "收到 FriendRemove（对方删了我）时也要解除身份绑定，与 `remove_friend` 对称"
        );
        // 提示必须可行动：说清"删掉好友重新添加"且"不用重启"
        let warn = tr
            .find("fn warn_key_conflict_once")
            .expect("必须还有密钥冲突提示（本护栏锚点）");
        let warn_body = &tr[warn..warn + 2000];
        assert!(
            warn_body.contains("重新添加") && warn_body.contains("不用重启"),
            "密钥变化的安全提示必须写清可行动的下一步（重新添加 + 不用重启）—— \
             只写「建议当面核对」等于把用户扔在原地"
        );
    }
