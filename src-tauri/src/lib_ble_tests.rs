// 职责边界：
// - `lib_tests.rs` 测试分册之4 —— BLE 形状：指定拨号器、握手跳帧、外设节奏、载荷预算与运行态单一来源
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
    /// **BLE 链路上必须只有一个"指定拨号方"**（大 id 拨、小 id 只接受）。
    ///
    /// 真实缺陷（用户 2026-09-12 真机："点加好友：发送失败，连接已关闭"）：
    /// 两端都同时跑 central + peripheral ⇒ **互相拨号**，形成两条镜像 BLE 链路；
    /// 小 id 那一侧拨过去的连接会被对端按镜像规则拒掉（不回 Hello），但**每次连接本身
    /// 都会打断对端拨过来的那条好链路** ⇒ 好链路 45s 收不到帧被看门狗拆掉 ⇒ 再重来。
    /// 修法：小 id 在握手验签后**记住"不要再拨这个外设"**并主动放弃该镜像链路。
    ///
    /// ⚠️ 2026-09-13（Windows Phase 1）本条判据多了 `peer_advertises` 前缀：
    /// "大 id 拨"只在**两端都能广播**时成立。Windows 只做 central，若照搬会让
    /// "Windows id 更小"变成两侧都不拨的死局（真机现象：搜到了但永远连不上）。
    /// 所以既要**保留**镜像护栏（对端在广播时仍按 id 比较），又要
    /// **保留**"对端不广播 ⇒ 必须我们拨"这个活口 —— 两条都不能被删掉。
    #[test]
    fn ble_link_has_a_designated_dialer() {
        let ble = &crate::network::ble_src_for_guards();
        let body = rust_fn_body(ble, "fn should_dial_ble(");
        assert!(
            body.contains("my_id > peer_id"),
            "判据必须保留 TCP 的 `should_dial` 同款：两端都能广播时**大 id 拨、小 id 只接受**"
        );
        assert!(
            body.contains("peer_advertises"),
            "对端不广播时必须由我们拨（`peer_advertises` 前缀）—— \
             少了它，只做 central 的平台（Windows）在 id 更小时会两侧都不拨，链路永远建不起来"
        );
        assert!(
            ble.contains("if !should_dial_ble(&state.device_id, &peer_id, peer_advertises)"),
            "central 侧握手后必须用它决定要不要放弃镜像链路"
        );
        assert!(
            ble.contains("ble_no_dial"),
            "必须记住『不要再拨』的名单 —— 否则每轮扫描还会去拨它、反复打断好的那条链路"
        );
        assert!(
            ble.contains("ble_peer_advertises"),
            "『对端是否在广播』必须有事实来源（扫描结果登记），不能靠平台常量猜"
        );
        assert!(
            ble.contains("contains(&peripheral.id().to_string())"),
            "扫描循环必须跳过名单里的外设"
        );
    }

    /// **BLE 握手必须容忍前导帧，且前导帧绝不进业务处理**（真机 2026-09-13）。
    ///
    /// 日志证据：`[GATT] 已就绪 → [DISCONNECT] 对端首帧不是 Hello（收到 chat_message）`。
    /// Android 的 notify 按 **central 地址**投递 ⇒ 上一条链路的待发帧会落在新连接上，
    /// "首帧必须是 Hello"这条旧判据于是把**本来能建起来的链路**全部打死。
    ///
    /// 这条护栏盯两件事：① central 侧走"读到 Hello 为止"的辅助函数（不是裸 read_one）；
    /// ② 被丢掉的前导帧**不能**进 `handle_message`（身份未验签，那是一条安全边界）。
    #[test]
    fn ble_handshake_skips_leading_frames_without_processing_them() {
        let ble = &crate::network::ble_src_for_guards();
        let ble_f = code_flat(ble);
        assert!(
            ble_f.contains("read_hello_frame(&mutreader,HANDSHAKE_TIMEOUT"),
            "central 侧必须用 read_hello_frame（容忍前导帧），不能退回裸 read_one + 首帧断言"
        );
        let helper = rust_fn_body(ble, "async fn read_hello_frame(");
        assert!(
            !helper.contains("handle_message"),
            "前导帧绝不能进 handle_message —— 身份来自 Hello 验签，验签之前它只是字节"
        );
        assert!(
            helper.contains("preamble_action("),
            "额度判定必须走纯函数 preamble_action（可单测）"
        );
        // 外设侧同理；但只有**有外设角色**的平台才存在那段代码
        //（Windows Phase 1 只做 central —— ADR-0015 §7-f，外设分支不编译）。
        #[cfg(any(target_os = "macos", target_os = "android"))]
        assert!(
            ble.contains("丢弃外设侧握手前导帧"),
            "外设侧同样要丢前导帧（同一条 Android notify 语义）"
        );
    }

    /// **BLE 拨号必须去重 + 失败必须断开**（真机 2026-09-13：Mac 一个字节都收不到）。
    ///
    /// 真机证据：Mac 侧 `[GATT] 已就绪` 之后 `握手超时：对端未回 Hello` 反复出现，
    /// 而安卓侧 `[SEND] type=gossip kind=FriendRequest` 全部"成功"。根因是**同一对端上叠了
    /// 多条连接**：扫描每 10s 一轮、握手最长 10s ⇒ 每轮都新起一个拨号任务；
    /// 失败后又从不 `disconnect()`（drop 一个 btleplug `Peripheral` 不会断开 CoreBluetooth），
    /// 于是留下"已经没人读"的幽灵连接 + 多个通知流订阅；Android 的 GATT server 对同一地址
    /// 只保留最后一条连接，通知于是被投给没人读的那条。
    ///
    /// 这条护栏盯四件事（少一件就会退回原状）：
    /// ① 拨号前用 `DialGuard` 去重；② 复用旧连接前先断开；③ 握手失败必须显式断开；
    /// ④ 分片级统计必须存在（否则"没发出去"与"没收到"永远分不清）。
    #[test]
    fn ble_dial_is_deduplicated_and_disconnects_on_failure() {
        let ble = &crate::network::ble_src_for_guards();
        let dial = rust_fn_body(ble, "async fn dial_and_register(");
        assert!(
            dial.contains("DialGuard::try_acquire") && dial.contains("ble:{ble_id}"),
            "拨号前必须按外设 id 做在途去重（否则每轮扫描叠一条连接）"
        );
        assert!(
            dial.contains("is_connected()"),
            "复用旧连接前必须先断开（否则 connect() 会复用幽灵连接、新订阅收不到任何通知）"
        );
        assert!(
            dial.contains("peripheral.disconnect().await"),
            "失败路径必须显式断开（drop 不会断开 CoreBluetooth 连接）"
        );
        assert!(
            ble.contains("[FRAG] 收到通知"),
            "读循环必须打**分片级**日志：否则『对端没发』与『发了我没收到』无法区分"
        );
        let bt = include_str!("transport/bluetooth.rs");
        assert!(
            bt.contains("fn stats(") && bt.contains("seen_notifications"),
            "BleReader 必须统计收到的通知条数/字节数（分片级可见性的来源）"
        );
    }

    /// **Android 外设的通知必须节流**（真机 2026-09-13：742B 的帧只到了 38/53 片）。
    ///
    /// 算术证据：Mac 侧 `[FRAG] 收到通知 38 条 / 747 字节`；而 742 字节的帧在 MTU=23
    /// （每片 14 字节载荷 + 6 字节分片头）下需要 **53 片** ⇒ 丢了 15 片 ⇒ 永远拼不出完整帧
    /// ⇒ 表现就是"安卓收到了好友申请并且加上了，Mac 什么都收不到"。
    /// `notifyCharacteristicChanged` 连发会被 Android 协议栈丢包，`send()` 返回 true 只代表
    /// 调用被接受；必须每片之间留一个连接间隔。
    #[test]
    fn android_peripheral_paces_its_notifications() {
        let android = include_str!("transport/ble_android.rs");
        assert!(
            android.contains("const NOTIFY_CHUNK_INTERVAL: Duration"),
            "必须显式定义通知间隔常量（可调、可测），而不是散落的 magic number"
        );
        let send = rust_fn_body(
            android,
            "    pub async fn send_frame(&self, central: &str, payload: &[u8])",
        );
        assert!(
            send.contains("sleep(NOTIFY_CHUNK_INTERVAL).await"),
            "逐片发送的循环里必须真的 sleep 这个间隔 —— 否则连发丢片会重现"
        );
        assert!(
            send.contains("idx + 1 < total"),
            "最后一片之后不该再等（否则每帧白等一个间隔）"
        );
        // 发送侧的分片数必须留痕：与对端的 [FRAG] 数字对照才能判"发少了 / 收丢了"
        let ble = &crate::network::ble_src_for_guards();
        assert!(
            ble.contains("分片={n}"),
            "写循环必须打出发出的分片数（与对端 [FRAG] 对照）"
        );
    }

    /// **BLE 文件分块必须小，且不能让一帧打死链路**（真机 2026-09-13：大图"两边都成功"）。
    ///
    /// 日志证据：`[SEND] 写失败 ⇒ 结束该链路写循环 … type=file_chunk`；而接收侧反复
    /// `接收文件初始化失败: 重复的文件传输` + `file_reject`。根因：一对一文件流每块默认
    /// 256 KiB，在 MTU=23 的 BLE 上需要 18725 个分片 > `MAX_BLE_CHUNKS_PER_MESSAGE`(8192)
    /// ⇒ `fragment()` 返回 None ⇒ 写循环把**整条链路**拆掉；同时重复的 offer 被 reject
    /// ⇒ 对端停止重试 ⇒ 文件永远到不了，而发送方界面显示"已发送/已读"。
    ///
    /// 这条护栏盯三件事：① 分块按链路选路结果决定；② "帧无法分片"只丢这一帧、不拆链路；
    /// ③ 重复的 offer 必须幂等回 accept。
    #[test]
    fn ble_file_transfer_respects_link_limits() {
        // 视图读"一个家"`network::file_src_for_guards()`（2026-10-07 file.rs 按角色切成 include! 分册）：
            // 读单个文件只会看见主册 ⇒ 形状守卫对分册失明（假绿形状）。登记对账由 lib_source_view_tests 第四个用例钉两侧。
                    let file = &crate::network::file_src_for_guards();
        let stream = rust_fn_body(file, "async fn stream_file(");
        assert!(
            stream.contains("chunk_size_for_path("),
            "文件分块大小必须按链路能力决定（BLE 上 256 KiB 分不出片）"
        );
        assert!(
            stream.contains("resolve_stream_link("),
            "分块大小必须取自**实际选路结果**，不能按平台写死"
        );
        // 保序不变量：一条分片流只能待在同一条连接上（真机：多文件并发时 600MB
        // 大文件跑到 100% 报"文件分片顺序错误"，单发同一文件必成功）。
        // 一律用 code_flat：裸子串会被 rustfmt 拆行而静默空转。
        let stream_f = code_flat(&stream);
        assert!(
            stream_f.contains("send_on_link_with_tick(&link,&chunk,FILE_STALL_TICK,")
                // ⚠️ 第二段判据刻意**不写到右括号**：参数一多 rustfmt 会拆行并补尾随逗号，
                // 把 `)` 写进判据等于把断言绑在排版上（本轮实测踩过：拆行后判据永不匹配，
                // 红的是护栏自己，不是它保护的代码）。
                && stream_f.contains("stall_tick(state,transfer_id,peer_id,stream_started_ms,&mutstalled_shown"),
            "分片必须投到钉住的那条链路，且等待期间必须做停滞检查 —— 对端不收时发送就挂在\
             背压上，这里是唯一的观测点（摘掉检查 = 界面冻在同一个百分比直到 deadline）"
        );
        assert!(
            stream_f.contains("send_on_link(&link,&done)"),
            "FileDone 必须排在**自己那串分片之后**走同一条链路：走 try_send 时队列满会换到\
             空闲连接，完成帧超过在途分片先到 ⇒ 接收端判「文件传输未完成」"
        );
        assert!(
            !stream_f.contains("try_send(state,peer_id,&chunk)"),
            "文件分片不得逐条选路（跨连接乱序）"
        );

        let ble = &crate::network::ble_src_for_guards();
        assert!(
            ble.contains("丢弃无法分片的帧（链路保留）"),
            "「帧无法分片」只该丢这一帧：拆链路会让同连接上其它传输一起失败"
        );

        let transport = crate::network::transport_src_for_guards();
        // 重复 offer 必须幂等回 accept（旧行为 reject ⇒ 对端停止重试、文件永远收不到）。
        // ⚠️ 但幂等**不等于**无条件 Accept：判据只许有一份（`file::decide_offer`），
        // 而位置对得上时还必须把段号归零 —— 少这一步，续传段（按 seq 0 重编）会被
        // 当成"迟到的重复片"整段丢掉。2026-09-22 那次 160MB 永不收敛就是这么来的。
        assert!(
            transport.contains("file::decide_offer(")
                && transport.contains("file::OfferDecision::AcceptResumeSegment")
                && transport.contains("file::restart_segment(state, &transfer_id)"),
            "重复 FileOffer 的答复必须走那一份判据，且续传段的「段号归零」不许在处理器里另写一遍"
        );
        assert!(
            !transport.contains("retained != from_bytes"),
            "位置比对不许在处理器里再写一遍（两处规矩必然漂移，判据只许 `decide_offer` 一份）"
        );

        // 群路径同理：Offer / Chunk / Done 三类帧必须全在同一条链路上。
        // 只钉分片不钉 Offer 会造出更隐蔽的分裂：Offer 是 Normal、分片是 Low，Normal 满掉
        // 时 failover 到另一条连接 ⇒ 分片先到、密钥后到，而接收端没有该 transfer 的密钥时
        // 是**静默 return**（不报错、不回执），这批分片就永久丢了。
        let g = code_flat(include_str!("commands/group_file_dispatch.rs"));
        assert!(
            g.contains("send_on_link(&link,&offer)")
                && g.contains("send_on_link_with_tick(&link,&chunk,file::FILE_STALL_TICK,"),
            "群文件的 Offer 与分片必须走同一条钉住的链路（分片还要带停滞检查）"
        );
        assert!(
            !g.contains("try_send(state,recipient,"),
            "群文件三类帧都不得再走逐条选路的 try_send（跨连接乱序）"
        );
    }

    /// **扫描结果不得再用 `Peripheral::services()` 二次过滤**（真机踩过，症状极隐蔽）。
    ///
    /// `start_scan(ScanFilter{services})` 已在平台层过滤；而 `services()` 在 **Android 上
    /// 只有连接并 `discover_services()` 之后才有值**，未连接时恒为空 ⇒ 拿它过滤会把所有候选
    /// 丢掉。真机症状（用户 2026-09-12）：两台设备蓝牙都开着、都在广播、系统层扫描也命中，
    /// 但**一个候选都不去连**，双方永远发现不了彼此，而日志里一个字都没有。
    #[test]
    fn scan_results_are_not_filtered_by_unconnected_services() {
        let bt = include_str!("transport/bluetooth.rs");
        let body = rust_fn_body(bt, "pub async fn scan_peers(");
        assert!(
            !body.contains("p.services()") && !body.contains(".services().iter()"),
            "扫描结果必须**原样返回**：按 `services()` 过滤在 Android 上会把候选全丢掉 \
             （未连接时它恒为空集合）"
        );
        assert!(
            body.contains("ScanFilter::default()"),
            "不得在**平台层**用服务 UUID 过滤：macOS 把 128 位 UUID 放进扫描响应，\
             Android 的硬件过滤只匹配主广播包 ⇒ 会永远收不到 Mac 的广播（真机实测）"
        );
        assert!(
            body.contains("properties()"),
            "必须按**广播内容**（`properties().services`）判定，而不是按连接后才发现的服务"
        );
        assert!(
            body.contains("discover_services()") || body.contains("连接"),
            "必须在注释里写清为什么不能过滤 —— 否则下一个人很容易『顺手补一个校验』"
        );
        // 扫到候选必须留痕（否则这类缺陷在日志里完全不可见）
        let ble = &crate::network::ble_src_for_guards();
        assert!(
            ble.contains("BLE 扫描：收到"),
            "每次扫描都要打日志（收到的广播总数 + 其中本服务的个数）：真机上这是区分\
             『扫描收不到广播』与『收到了但都不是本服务』的唯一线索"
        );
    }

    /// **外设侧收到的 Hello 必须换路由重新握手**（重连时的经典坑，真机踩过）。
    ///
    /// 2026-09-12 真机：Mac（central）反复报 `对端首帧不是 Hello` / `握手超时：对端未回 Hello`。
    /// BLE 上同一个 central 的地址在**重连**时复用，旧连接的链路任务可能还没清理：
    /// 新连接的 Hello 一旦被投给**旧链路的管道**，旧链路的写句柄写的是旧连接
    /// ⇒ 新连接永远收不到 Hello 回应。用户侧表现就是"蓝牙时好时坏、加好友没反应"。
    ///
    /// 这条护栏盯三件事：① 判据函数存在且语义正确（真值表由 `ble::tests` 钉住）；
    /// ② 接收循环**真的调用**它（判据再好，调用点写错也白搭）；③ 诊断信息必须带上
    /// 收到的类型名 —— 否则下次真机日志里还是只有一句"不是 Hello"，无从下手。
    #[test]
    fn peripheral_reconnect_hello_replaces_the_stale_route() {
        let ble = &crate::network::ble_src_for_guards();
        let action = rust_fn_body(ble, "fn peripheral_route_action(");
        assert!(
            action.contains("!has_route || frame_is_hello"),
            "判据必须是『没有活路由 或 这帧是 Hello ⇒ 走握手』，其余才投已有链路"
        );
        assert!(
            ble.contains("peripheral_route_action(has_route, has_route && frame_is_hello(&bytes))"),
            "接收循环必须**真的调用**这个判据（只在有路由时才解析首帧，避免给大分片白烧一次解析）"
        );
        assert!(
            ble.contains("外设侧收到新连接的 Hello"),
            "换路由必须留痕：真机上这是区分『重连接管』与『链路抖动』的唯一日志"
        );
        // 诊断信息必须带类型名（两处：central 侧拨号 + 外设侧接收入站）
        assert!(
            ble.contains("对端首帧不是 Hello（连续 {dropped} 帧都不是，最后一帧 type="),
            "central 侧失败时必须带**最后一帧的类型**（`wire_kind()`）"
        );
        assert!(
            ble.contains("外设侧首帧不是 Hello（收到 {") && ble.contains("first.wire_kind()"),
            "外设侧失败时同样必须带**收到的类型**（`wire_kind()`）"
        );
        assert!(
            ble.contains("wire_kind()"),
            "类型名要走 `Message::wire_kind()`（与 serde tag 同一份事实来源）"
        );
    }

    /// 外设侧**每次订阅都必须清掉该 central 的重组器**（用户优先级 ①：加入 mesh 的稳定性）。
    ///
    /// 为什么（2026-09-13 框架审计）：对端的 `msg_id` **每条连接都从 1 重新开始**，而 macOS
    /// 外设角色**没有** didDisconnect 回调、`didUnsubscribe` 也不保证在断连时到达 ⇒ 上一轮
    /// 残留的半截消息会和重连后的第一帧撞在同一个 `msg_id` 上（分片数/序号对不上）⇒ 那条帧
    /// 被当坏片丢掉（真机体感："重连之后第一条消息丢了"）。只靠 `BleReassembler` 的 30s TTL
    /// 兜底太慢 —— 真机重连通常就在几秒内发生。
    ///
    /// 判据：`did_subscribe` 里必须**按 central id `remove`**（不能整体 `clear`：会误伤其它
    /// 正在线的对端）。
    #[test]
    fn peripheral_subscribe_resets_that_centrals_reassembler() {
        let src = include_str!("transport/bluetooth_peripheral.rs");
        let start = src
            .find("fn did_subscribe(")
            .expect("源码里找不到 `fn did_subscribe(` —— 护栏需要同步更新");
        let end = src[start..]
            .find("fn did_unsubscribe(")
            .map(|i| start + i)
            .expect("源码里找不到 `fn did_unsubscribe(` —— 护栏需要同步更新");
        let body = &src[start..end];
        assert!(
            body.contains("reassemblers"),
            "`did_subscribe` 必须清掉该 central 的分片重组器 —— 否则重连后第一条帧会和上一轮的半截消息撞车"
        );
        assert!(
            body.contains(".remove(&id)"),
            "必须**按 central id** remove（整体 clear 会误伤其它在线对端）"
        );
    }

    /// **BLE 写失败必须"退避重试 → 拆链路"，绝不能只结束写循环**（真机 2026-09-13 安卓）。
    ///
    /// 真机链路：Android 与 Mac/Windows 的 BLE 会话都 `[SESSION] 已就绪`，随后一阵群 gossip
    /// 洪水把链路写满 ⇒ `[SEND] 写失败 ⇒ 结束该链路写循环`（两条链路各一次）⇒ 从此**发不出去**，
    /// 界面报「发送失败，连接已关闭」，而**读**还在正常收 ⇒ 看门狗按读活性判健康、45s 也不拆
    /// ⇒ 只能重启应用。根因是"只结束写循环、把链路留成能收不能发的僵尸"。
    ///
    /// 判据：写循环必须有重试上限常量、最终失败要走链路表的 `cancel.send(true)`（让读循环
    /// 收尾时 `teardown_link` 清链路+清退避+wake_scan），并且失败日志要带上**原因**（旧实现
    /// 只打 `type=?`，真机上完全看不出为什么写失败）。
    #[test]
    fn ble_write_failure_retries_then_tears_the_link_down() {
        let ble = &crate::network::ble_src_for_guards();
        let body = rust_fn_body(ble, "async fn ble_writer_loop<S: FrameSink + 'static>(");
        assert!(
            body.contains("WRITE_RETRY_ATTEMPTS"),
            "写失败必须**退避重试**（瞬态失败一次性判死会把链路变成僵尸）"
        );
        assert!(
            body.contains("l.cancel.send(true)"),
            "写循环最终失败必须去链路表里取消这一条（否则读循环还活着 ⇒ \
             能收不能发的僵尸链路，看门狗按读活性判健康、永远不拆，只能重启应用）"
        );
        assert!(
            body.contains("原因={e}"),
            "失败日志必须带上真实原因（旧实现只打 `type=?`，真机上无从判断）"
        );
        assert!(
            ble.contains("WRITE_RETRY_WAIT"),
            "重试间隔必须是常量（可读、可调）"
        );
    }

    /// **`driver::connect` 的"便宜路径"必须先判 `is_connected()`**（2026-09-13 合并评审）。
    ///
    /// 判据：12 × 250ms = 3 秒的 `discover_services()` 重试只在**已经连着**时才有意义。
    /// 无条件先跑它，等于给 macOS/Android 的每次拨号凭空加 3 秒（它们本来第一次
    /// `connect()` 就成功），既拖慢握手又可能撞上 10s 的握手超时 —— 而这种退化
    /// **功能看起来还在**（最终连得上，只是慢/偶尔超时），只能靠结构护栏盯住。
    #[test]
    fn ble_connect_cheap_path_requires_an_existing_connection() {
        let src = include_str!("transport/bluetooth.rs");
        let body = rust_fn_body(src, "pub async fn connect(peripheral: &Peripheral)");
        assert!(
            body.contains("is_connected"),
            "`connect()` 的便宜路径必须先用 `is_connected()` 判一下 —— 否则未连接时白等 3 秒"
        );
        let gate = body.find("is_connected").expect("上面刚断言过");
        let cheap = body
            .find("LINK_READY_ATTEMPTS")
            .expect("必须还有便宜路径（LINK_READY_ATTEMPTS）");
        assert!(
            gate < cheap,
            "`is_connected()` 的判断必须在便宜路径的循环**之前**（先判再跑，否则等于没判）"
        );
    }

    /// 蓝牙开关**不许卡在"等 CoreBluetooth 回报状态"上**（用户 2026-09-13）。
    ///
    /// `ble::start` 里 `start_peripheral` 要等 `peripheral::STATE_WAIT = 3s`，而它在
    /// `set_channel_enabled` 的关键路径上 —— 一旦 `await` 它，用户点开关就要干等 3 秒
    /// （用户原话："点了一下，过了好一会儿才会开"）。外设角色本来就是**独立失败**的
    /// （起不来只影响"别人连我们"），所以必须丢到后台任务里。
    ///
    /// 同时**句柄必须先写进 `state.ble` 再 spawn 外设**：反过来会出现"刚开就关"时
    /// `stop()` 拿不到 handle ⇒ 发不出停机信号 ⇒ 那个外设任务永远活着（蓝牙关不掉）。
    #[test]
    fn ble_start_does_not_block_on_the_peripheral_state_wait() {
        let ble = &crate::network::ble_src_for_guards();
        let body = rust_fn_body(ble, "pub async fn start(state: Arc<AppState>)");
        assert!(
            !body.contains("start_peripheral(state.clone(), shutdown_tx.subscribe()).await"),
            "不许 await 外设启动（要等最多 3s 的 CoreBluetooth 状态回调）—— 必须 tokio::spawn"
        );
        let spawn = body
            .find("tokio::spawn(start_peripheral(")
            .expect("外设启动必须在后台任务里跑（`tokio::spawn(start_peripheral(...))`）");
        let store = body
            .find("*state.ble.lock()")
            .expect("必须把 ble 句柄写进 state.ble");
        assert!(
            store < spawn,
            "句柄必须先写进 state.ble、再 spawn 外设，否则「刚开就关」时 stop() 发不出停机信号"
        );
    }

    /// **蓝牙通道缺省就是开**（用户 2026-09-12 规则：「有蓝牙就默认开，不用手动开关」）。
    ///
    /// 之前是"手机默认开、桌面默认关"，结果：Mac 上还要手动点一次；更糟的是
    /// "偏好=关 而运行时=开"会互相回灌，触发启停抖动（Mac 日志里那种每秒一次的
    /// `外设角色已启动 → 已停止广播` 循环，会把蓝牙栈和 CPU 打满、整个应用顿卡）。
    #[test]
    fn bt_defaults_on_everywhere() {
        let db = all_db_src();
        let body = rust_fn_body(db, "pub fn get_bt_enabled(");
        assert!(
            body.contains("let default_on = true;"),
            "缺省必须是**开**（三端一致）；写成按平台分支会重新引入「偏好/运行时互相回灌」的抖动"
        );
        assert!(
            body.contains("set_bt_enabled(conn, default_on)"),
            "缺省值必须立刻持久化（与 `get_lan_enabled` 同一套语义）"
        );
        let tm = include_str!("transport/mod.rs");
        assert!(
            tm.contains("crate::db::get_bt_enabled(&dbc)"),
            "`TransportManager::new` 必须用 `db::get_bt_enabled`（缺省值才不会在各处漂移）"
        );
    }

    /// 通道状态里的蓝牙必须是**真实运行时**状态。
    ///
    /// `TransportManager` 里的 `BluetoothTransport` 是"尚未接线"的占位实现：它的
    /// `running` 恒为 `false`、`peers` 恒为 0。界面直接采信它就会永远显示"蓝牙未运行"，
    /// 用户点了开关也看不出变化（用户 2026-09-12 安卓实测「蓝牙通道打不开」里，
    /// 有一部分就是这个假状态造成的误导）。
    #[test]
    fn runtime_snapshot_reports_real_bluetooth_runtime() {
        let commands = all_commands_src();
        let body = rust_fn_body(commands, "pub async fn build_runtime_snapshot(");
        assert!(
            body.contains("runtime_state"),
            "蓝牙 running/peers 必须取自 `network::ble::runtime_state`"
        );
        assert!(
            body.contains("bt.enabled = bt_running"),
            "起不来就必须显示为关（否则界面假装已开，用户只会觉得「点了没用」）"
        );
    }

    /// **运行状态只能有"一个快照 + 一个事件"**（用户要求的 ②）。
    ///
    /// 真实缺陷：`channels[lan].enabled`（`get_channel_status`）与 `online`（`get_network_status`）
    /// 是同一件事的两份前端状态，各自被不同事件更新 ⇒ 必然"外面开了、里面还是关的"。
    /// 现在旧的"半份状态"命令必须**不存在**，且事件必须**带载荷**（`RuntimeSnapshot`）。
    #[test]
    fn runtime_state_has_a_single_source() {
        let commands = all_commands_src();
        for gone in [
            "pub async fn get_channel_status(",
            "pub fn get_network_status(",
        ] {
            assert!(
                !commands.contains(gone),
                "`{gone}` 应当已经被 `get_runtime_snapshot` + `build_runtime_snapshot` 取代 —— \
                 留着它就等于给同一件事留了第二份状态"
            );
        }
        assert!(
            commands.contains("pub async fn build_runtime_snapshot("),
            "必须有唯一的采集点"
        );
        let state = include_str!("state.rs");
        let body = rust_fn_body(state, "pub fn notify_runtime_changed(");
        assert!(
            body.contains("snapshot: RuntimeSnapshot"),
            "`runtime-changed` 必须**带快照**（无载荷的话接收方只能再全量重拉一遍）"
        );
        assert!(
            body.contains("emit_filter(EVENT_RUNTIME_CHANGED"),
            "必须用 emit_filter 排除发起窗口（发起窗口从命令返回值里已经拿到了）"
        );
        assert!(
            body.contains("event_target_label(target) != origin.as_deref()"),
            "过滤条件必须是『目标窗口 ≠ 发起窗口』"
        );
    }
