// 职责边界：
// - transport 行为测试分册之4 —— 多链路选路次序（LAN / routed / BLE 优先级、健康度过滤、陈旧代次）
// 为什么拆：`transport/tests.rs` 原来 3,227 行、11 个顶层项全挤在同一个 `mod tests` 里。
// 机制与 `file_tests.rs` / `logs_tests.rs` 同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::network::transport::tests` 这同一个模块 ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ 本册**不进** `network::transport_src_for_guards()` 那份"生产码全集"视图：视图掺测试字面量会把
// 按窗口取段的守卫飘到测试文本上（2026-10-06 实测 4 条假红）。`lib_tests.rs` 里那条登记对账守卫
// 按 `_tests.rs` 结尾豁免本册 —— 别为了消红把测试文本并进去（那正是假绿的形状）。
    fn make_link(
        addr: &str,
        kind: PathKind,
    ) -> (
        crate::state::Link,
        mpsc::Receiver<Message>,
        mpsc::Receiver<Message>,
    ) {
        let (b_tx, b_rx) = mpsc::channel(4);
        let (p_tx, p_rx) = mpsc::channel(4);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint: MeshEndpoint::Tcp(addr.parse().unwrap()),
                path_kind: kind,
                high: b_tx.clone(),
                normal: p_tx,
                low: b_tx,
                cancel,
            },
            b_rx,
            p_rx,
        )
    }

    /// 按任意 `Endpoint` 造一条链路（`make_link` 只接受 TCP 地址字符串，BLE 用这个）。
    fn make_link_endpoint(
        endpoint: MeshEndpoint,
        kind: PathKind,
    ) -> (
        crate::state::Link,
        mpsc::Receiver<Message>,
        mpsc::Receiver<Message>,
    ) {
        let (b_tx, b_rx) = mpsc::channel(4);
        let (p_tx, p_rx) = mpsc::channel(4);
        let (cancel, _cancel_rx) = watch::channel(false);
        (
            crate::state::Link {
                endpoint,
                path_kind: kind,
                high: b_tx.clone(),
                normal: p_tx,
                low: b_tx,
                cancel,
            },
            b_rx,
            p_rx,
        )
    }

    /// 按任意 `Endpoint` 造一个 mesh 层 `Connection`（同上）。
    fn mesh_conn_endpoint(
        peer: &str,
        endpoint: MeshEndpoint,
        healthy_at: Option<i64>,
        kind: PathKind,
    ) -> crate::mesh::Connection {
        let mut c = crate::mesh::Connection::new(peer, endpoint, kind);
        if let Some(t) = healthy_at {
            c.health.seed_read_seen(t);
        }
        c
    }

    fn mesh_conn(
        peer: &str,
        addr: &str,
        healthy_at: Option<i64>,
        kind: PathKind,
    ) -> crate::mesh::Connection {
        let mut c = crate::mesh::Connection::new(
            peer,
            crate::mesh::endpoint::Endpoint::Tcp(addr.parse().unwrap()),
            kind,
        );
        if let Some(t) = healthy_at {
            c.health.seed_read_seen(t);
        }
        c
    }

    /// 单链路：顺序无变化（**行为零变化**，M3-b 的前提）。
    #[test]
    fn route_order_single_link_is_unchanged() {
        let (l0, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![l0];
        let conns = vec![mesh_conn(
            "peer",
            "192.168.1.20:59992",
            Some(1000),
            PathKind::Lan,
        )];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order, vec![0]);
    }

    /// 核心（M3-b 的收益）：两条都健康时**LAN 优先**，与插入顺序无关。
    #[test]
    fn route_order_prefers_lan_over_routed_regardless_of_insertion() {
        // 故意把 Routed 放在下标 0（插入在前），LAN 在下标 1
        let (routed, _b0, _p0) = make_link("100.70.10.20:59992", PathKind::Routed);
        let (lan, _b1, _p1) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![routed, lan];
        let conns = vec![
            mesh_conn("peer", "100.70.10.20:59992", Some(1000), PathKind::Routed),
            mesh_conn("peer", "192.168.1.20:59992", Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order[0], 1, "应优先 LAN（下标 1），而不是插入在前的 Routed");
        // 不变量：其余链路仍排在后面做 failover，**一条都不能丢**
        assert_eq!(order.len(), 2);
        assert!(order.contains(&0) && order.contains(&1));
    }

    /// BLE 链路的两个"不该被当成 LAN"判据（ADR-0015 的 7-c）：
    /// ① 选路：TCP（LAN/Routed）必须排在 BLE 前面 —— 蓝牙带宽/功耗都差一个量级；
    /// ② 拨号：只有 BLE 连上**不算**「LAN 已连通」，否则 `ensure_link` 不再补 LAN 链路
    ///    （用户明明在同一局域网，却一直走蓝牙 —— 电量与速度都吃亏）。
    #[test]
    fn ble_link_is_neither_lan_nor_preferred_over_tcp() {
        let ble = MeshEndpoint::Ble(crate::mesh::BleEndpoint::new("node-1"));
        let lan: MeshEndpoint = "192.168.1.20:59992"
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .into();

        // ① 选路：BLE 插在前面也不该被优先选
        let (ble_link, _b0, _p0) = make_link_endpoint(ble.clone(), PathKind::Bluetooth);
        let (lan_link, _b1, _p1) = make_link_endpoint(lan.clone(), PathKind::Lan);
        let links = vec![ble_link, lan_link];
        let conns = vec![
            mesh_conn_endpoint("peer", ble.clone(), Some(1000), PathKind::Bluetooth),
            mesh_conn_endpoint("peer", lan.clone(), Some(1000), PathKind::Lan),
        ];
        let order = route_order(&links, "peer", &conns, 1000, 15_000, 3);
        assert_eq!(order.len(), 2, "BLE 链路同样是 failover 候选，不能丢");
        assert_eq!(order[0], 1, "LAN 必须优先于 BLE");

        // ② 拨号判据：只有 BLE ⇒ LAN 路径尚未连通 ⇒ 仍要去补一条 LAN
        assert!(!has_lan_path(&[(ble.clone(), PathKind::Bluetooth)]));
        let now = 1_000_000;
        assert!(
            should_dial_for_peer(
                "b",
                "a",
                false,
                &[(ble, PathKind::Bluetooth)],
                Some(now - 60_000),
                now
            ),
            "只有 BLE 连接时仍应补 LAN"
        );
    }

    /// 跨世代的入站连接必须被否决（D8-4）。
    #[test]
    fn stale_generation_is_rejected() {
        assert!(generation_is_current(3, 3), "同一世代允许登记");
        assert!(
            !generation_is_current(3, 4),
            "stop/start 之后的旧世代不得登记"
        );
        // 世代只增不减，但"捕获值比当前大"同样视为无效（防御性：不做大小比较）
        assert!(!generation_is_current(4, 3));
    }

    /// failover 核心：LAN 的读活性过期（半开）而 Routed 健康 → 选 Routed。
    #[test]
    fn route_order_skips_unhealthy_lan_when_routed_is_healthy() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            // LAN：只有很早的读活性（已过期）
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),
            // Routed：刚刚读到过帧
            mesh_conn("peer", "100.70.10.20:59992", Some(60_000), PathKind::Routed),
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(order[0], 1, "LAN 不健康时必须降级到 Routed（真 failover）");
        assert_eq!(order.len(), 2, "不健康链路仍保留在后面（可作最后手段）");
    }

    /// 登记窗口：传输链路存在但 mesh 侧还没登记 → 合成「刚播种」候选，不能因此被判不可用。
    #[test]
    fn route_order_tolerates_missing_mesh_candidate() {
        let (only, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let links = vec![only];
        let order = route_order(&links, "peer", &[], 1000, 15_000, 3);
        assert_eq!(order, vec![0], "缺候选时不得丢链路（登记窗口是常态）");
    }

    /// 全部不健康：`pick_link` 退回首条（保持可用），且顺序仍是全量排列。
    #[test]
    fn route_order_keeps_all_links_when_none_healthy() {
        let (lan, _b0, _p0) = make_link("192.168.1.20:59992", PathKind::Lan);
        let (routed, _b1, _p1) = make_link("100.70.10.20:59992", PathKind::Routed);
        let links = vec![lan, routed];
        let conns = vec![
            mesh_conn("peer", "192.168.1.20:59992", Some(0), PathKind::Lan),
            mesh_conn("peer", "100.70.10.20:59992", Some(0), PathKind::Routed),
        ];
        let order = route_order(&links, "peer", &conns, 60_000, 15_000, 3);
        assert_eq!(
            order.len(),
            2,
            "全不健康也要把链路交出去（可用性优先于择优）"
        );
    }

    // ---- M3-b：真实信道上的 failover（ADR-0014 §8「切断被选中那条 → 消息仍送达」的单元版）----
