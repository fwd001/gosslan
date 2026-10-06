// 职责边界：
// - transport 行为测试分册之6 —— 链路队列的字节预算与「只有一个创建点」（queue_policy）
// 为什么拆：`transport/tests.rs` 原来 3,227 行、2 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    /// 链路的 low 队列必须按**字节**封顶，而不是按帧数（第 2 步 · P3）。
    ///
    /// 旧形状是四个建链点各写死三条 1024 深的 `mpsc::channel`：一片 LAN 分块上线是
    /// `base64(chunk + 28) ≈ 341 KB`，1024 槽 ⇒ **单链路最坏 ~350 MB**
    /// （多链路、群文件多收件人按连接翻倍）。背压是有的，但**位置错了**：
    /// 缓冲先分配完才开始排队。
    ///
    /// 这条断言故意只说"预算"与"不许为 0"，不说槽数是多少 —— 槽数是推导量，
    /// 把 24 写进测试就等于每次调预算都要改一次测试。
    #[test]
    fn link_low_queue_is_bounded_by_bytes_not_frame_count() {
        // `mpsc::channel(0)` 会 panic ⇒ 折算下限必须 ≥1，这里连极小与极大分片一起试
        for plain in [crate::network::file::BLE_FILE_CHUNK, 1, 64] {
            let slots = low_queue_slots(plain);
            assert!(
                slots >= 1,
                "chunk={plain} 折算出 {slots} 槽，channel(0) 会直接 panic"
            );
        }
        // 分片越大 ⇒ 槽越少（单调不增），否则"按字节封顶"这句话是空的
        let mut prev = usize::MAX;
        for plain in [
            1usize,
            4096,
            65536,
            256 * 1024,
            1024 * 1024,
            crate::protocol::MAX_FRAME,
        ] {
            let slots = low_queue_slots(plain);
            assert!(
                slots <= prev,
                "chunk={plain} 反而比更小的分片排得更深（{slots} > {prev}）"
            );
            prev = slots;
        }
        // 真正的 P3 判据：两种生产分片尺寸下「槽数 × 单帧线上字节」都不许越过预算
        for plain in [
            crate::network::file::BLE_FILE_CHUNK,
            crate::protocol::FILE_CHUNK,
        ] {
            let held = low_queue_slots(plain) * crate::network::file::chunk_wire_bytes(plain);
            assert!(
                held <= LINK_QUEUE_BYTE_BUDGET,
                "chunk={plain} ⇒ 队列最坏装 {held} 字节，超过预算 {LINK_QUEUE_BYTE_BUDGET}"
            );
        }
        // 旧形状必须真的被治好：256KB 分片下不可能再排到 1024 深
        assert!(
            low_queue_slots(crate::protocol::FILE_CHUNK) < 1024,
            "LAN 分片的队列还是 1024 深 = 那条 ~350 MB 的老路没堵住"
        );
    }

    /// 四条链路的三条队列必须由**同一个策略**开出来（P3 的防回归位置）。
    ///
    /// 为什么还要源码钉一层，纯函数已经测过了：`1024` 这个字面量散在 4 个建链点里
    /// （入站 / 出站拨号 / BLE 两处）。只要还剩一份字面量，下一个加链路的人就会照抄那一份
    /// ⇒ "字节预算"退化成"其中三处有预算"，而且这种事**只在真机大文件时才看得见**。
    #[test]
    fn link_queues_are_created_from_one_place() {
        let mut src = crate::network::transport_src_for_guards();
        // 路径相对**本文件**：这段测试原来在 `network/transport.rs` 里，`ble.rs` 指的就是
        // 同目录之上的 `network/ble.rs`；搬进 `network/transport/` 之后必须写 `../ble.rs`，
        // 否则读到的是不存在的路径（编译器直接拒，不会静默）。
        src.push_str(include_str!("../ble.rs"));
        // 两个探针都**拼起来写**：本文件就是被扫的源码之一，直接写字面量会数进自己
        // （今天已经在别处被这个坑咬过两次：一次假过、一次假红）。
        let literal = "mpsc::channel(10".to_string() + "24)";
        assert_eq!(
            src.matches(literal.as_str()).count(),
            0,
            "还有写死的 1024 槽建链点：字节预算会被那一份绕过去"
        );
        let call = "link_chan".to_string() + "nels(";
        assert_eq!(
            src.matches(call.as_str()).count(),
            5,
            "应为「定义 1 处 + 四个建链点各 1 处」；少了就是有条链路还在自己开队列"
        );
        // 光"都走 link_channels"还不够：函数本身也得真的按预算折算开 low，
        // 否则预算名存实亡（这一条是被变异测试逼出来的）。
        let wired = "mpsc::channel(low_queue".to_string() + "_slots(";
        assert_eq!(
            src.matches(wired.as_str()).count(),
            1,
            "low 队列必须由 `low_queue_slots` 折算出来，不能直接给常量深度"
        );
    }
