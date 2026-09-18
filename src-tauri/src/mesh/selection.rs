//! 多路径选路（ADR-0014）。
//!
//! `pick_link` 是**纯函数**：输入一组候选连接 + 当前时间 + 健康阈值，输出该用哪一条。
//!
//! ## 为什么抽成纯函数
//!
//! 选路最终要落在 `try_send` —— 网络核心热路径。写错会整体影响收发，所以 ADR-0014
//! 把「策略」与「接线」拆成两个 commit（M3-a / M3-b），**M3-a 行为零变化**：
//! 函数先就位、只被单测与日志调用，接线时再改 `try_send`。这样出问题能二分定位。
//!
//! ## 策略（ADR-0014 §3.2）
//!
//! 1. 先按**活性**过滤：不健康的连接不参与（`ConnectionHealth::is_healthy`）；
//! 2. 再按**路径优先级**：LAN > Routed > Bluetooth；
//! 3. 同优先级用**建链顺序**打破平局 —— 稳定、可复现，不引入随机性（便于复现问题）；
//! 4. 全部不健康 → **退回第一条**，而不是返回 `None` 让调用方报错。
//!    保持可用优于报错，且与改造前「首个成功即返回」的兜底行为一致。
//!
//! ## 为什么本阶段不做 RTT 排序
//!
//! `Message::Heartbeat` 是**单向**的（收到只 `touch_peer` + flush，不回包），没有可靠的
//! 往返测量来源；而 LAN 与 Tailscale 的延迟差几个数量级，**路径优先级已经能正确区分**。
//! 按 YAGNI 不引入 RTT（要引入需给 Heartbeat 加回包 = 新 wire 变体）。详见 ADR-0014 §2。

use super::connection::Connection;
use super::path::PathKind;

/// congestion 信号的时间窗（毫秒）。
///
/// 这个窗口决定「最近多久内出现过 queue Full / writer 堵塞」算当前拥塞。
/// 3s = 比心跳周期（5s）短但足够覆盖一次瞬时拥塞，
/// 避免一个 3 秒前的 Full 永久污染路由决策。
pub const CONGESTION_WINDOW_MS: i64 = 3_000;

/// 路径优先级：数值越小越优先。
///
/// 语义排序（LAN 直连最快 > 已路由 IP > 蓝牙）**不依赖 `PathKind` 的声明顺序** ——
/// 枚举顺序是巧合，不能当语义用（那样以后往中间插一个变体就会静默改变选路优先级）。
fn path_rank(kind: PathKind) -> u8 {
    match kind {
        PathKind::Lan => 0,
        PathKind::Routed => 1,
        PathKind::Bluetooth => 2,
    }
}

/// 从候选连接里挑一条，返回其在 `candidates` 中的下标；空集合返回 `None`。
///
/// `health_timeout_ms` / `max_failures` 由调用方从 `PeerManager` 取
/// （`health_timeout_ms()` / `max_failures()`），避免阈值散落两处。
///
/// # 拥塞感知（M3-c）
///
/// 与"不健康"不同，"拥塞"是发送侧的独立维度：一条连接可以
/// `healthy=true`（心跳还在入站）但 `congested=true`（队列 Full / writer 被 TCP 窗口 0 卡住）。
/// 本函数在"选最优 healthy"之前，会先跳过**最近**拥塞的链路 ——
/// 但仅当"存在至少一条不拥塞的 healthy 候选"时才跳过。
/// 这保证了：拥塞信号**只做让路，不做封杀**。
///
/// 策略：
/// 1. 过滤 `is_healthy` → healthy 候选
/// 2. 从 healthy 里再过滤 `!is_congested` → 首选候选
/// 3. 首选非空 → 从首选里按 path_rank 选最优
/// 4. 首选为空（所有 healthy 都拥塞）→ 退回到从所有 healthy 里选（保持可用）
/// 5. healthy 全部为空 → 退回第一条（兜底不变）
pub fn pick_link(
    candidates: &[Connection],
    now_ms: i64,
    health_timeout_ms: i64,
    max_failures: u32,
    congestion_window_ms: i64,
) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }

    // Step 1: 过滤出所有 healthy 候选（liveness 维度）
    let healthy_mask: Vec<bool> = candidates
        .iter()
        .map(|c| c.health.is_healthy(now_ms, health_timeout_ms, max_failures))
        .collect();

    // Step 2: 当前版本暂不把 Priority congestion 纳入选路排除条件。
    //
    // 原因（2026-09-18 传输回归审计）：
    // Priority congestion 信号的唯一来源是 prio_tx.try_send() 发现 queue Full。
    // 但 prio channel 与 bulk channel 共享同一个 writer_loop —— writer_loop 被 bulk 的
    // write_frame（TCP backpressure）挂起时，prio_rx 同样停止消费 → prio queue 满 →
    // 被记录为 Priority congestion。这是「共享 writer_loop 被 bulk 阻塞」的结果，
    // 不是「Priority 网络路径真的拥塞」。把它喂给 pick_link 会让健康 LAN 被从 preferred
    // 候选中排除，错误切到 BLE/Routed，反而放大延迟。
    //
    // 因此：congestion 信号**仍记录**（mark_conn_congested 不动），但**不影响选路**，
    // 恢复到 v4.18.10 的"只按 healthy + path_rank 选"语义。
    //
    // congestion_window_ms 参数保留：暂不改签名以免级联 route_order / try_send；
    // 将来 Writer/Priority 解耦后可重新启用。
    let preferred_mask = healthy_mask.clone();

    // Step 3: 从 preferred 里挑最优；如果 preferred 全空，从 healthy 里挑（保持可用）
    let search_mask = if preferred_mask.iter().any(|p| *p) {
        preferred_mask.as_slice()
    } else {
        healthy_mask.as_slice()
    };

    let mut best: Option<(u8, usize)> = None;
    for (i, c) in candidates.iter().enumerate() {
        if !search_mask[i] {
            continue;
        }
        let rank = path_rank(c.path_kind);
        match best {
            // 严格更优才替换 ⇒ 同优先级保留**先出现**的那条（建链顺序打破平局）
            Some((best_rank, _)) if rank >= best_rank => {}
            _ => best = Some((rank, i)),
        }
    }

    // 全部不健康 → 退回第一条：保持可用，不把「无健康连接」变成调用方的错误分支。
    Some(best.map_or(0, |(_, i)| i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::endpoint::Endpoint;
    use crate::mesh::ChannelKind;
    use std::net::SocketAddr;

    const NOW: i64 = 1_000_000;
    const TIMEOUT: i64 = 10_000;
    const MAX_FAIL: u32 = 3;

    fn conn(peer: &str, port: u16, kind: PathKind) -> Connection {
        Connection::new(
            peer,
            Endpoint::Tcp(SocketAddr::from(([192, 168, 1, 20], port))),
            kind,
        )
    }

    /// 造一条「健康」连接（最近成功收发过）。
    fn healthy(peer: &str, port: u16, kind: PathKind) -> Connection {
        let mut c = conn(peer, port, kind);
        c.health.mark_read_seen(NOW, None);
        c
    }

    /// 造一条「从未成功」的连接（不健康）。
    fn never_seen(peer: &str, port: u16, kind: PathKind) -> Connection {
        conn(peer, port, kind)
    }

    #[test]
    fn empty_candidates_returns_none() {
        assert_eq!(
            pick_link(&[], NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            None
        );
    }

    #[test]
    fn single_healthy_connection_is_picked() {
        let c = vec![healthy("p", 1, PathKind::Routed)];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0)
        );
    }

    #[test]
    fn all_unhealthy_falls_back_to_first_keeps_usable() {
        // 全部不健康：不能返回 None（那会让调用方多一个错误分支），退回第一条保持可用。
        let c = vec![
            never_seen("p", 1, PathKind::Routed),
            never_seen("p", 2, PathKind::Lan),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0)
        );
    }

    #[test]
    fn path_priority_lan_beats_routed_and_bluetooth() {
        // 顺序打乱也不影响：LAN 排在最后也仍然被选中
        let c = vec![
            healthy("p", 1, PathKind::Bluetooth),
            healthy("p", 2, PathKind::Routed),
            healthy("p", 3, PathKind::Lan),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(2)
        );
    }

    #[test]
    fn path_priority_routed_beats_bluetooth() {
        let c = vec![
            healthy("p", 1, PathKind::Bluetooth),
            healthy("p", 2, PathKind::Routed),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(1)
        );
    }

    #[test]
    fn unhealthy_lan_does_not_block_healthy_routed() {
        // 这是「多路径 failover」的核心：LAN 断了，Routed 接管。
        let c = vec![
            never_seen("p", 1, PathKind::Lan),
            healthy("p", 2, PathKind::Routed),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(1)
        );
    }

    #[test]
    fn same_priority_keeps_earliest_for_stability() {
        // 两条 LAN 都健康 ⇒ 取先出现的（建链顺序），保证可复现、不抖动
        let c = vec![
            healthy("p", 1, PathKind::Lan),
            healthy("p", 2, PathKind::Lan),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0)
        );
    }

    #[test]
    fn stale_connection_is_not_healthy() {
        // 最后一次成功远早于阈值 ⇒ 不健康 ⇒ 让位给健康的 Routed
        let mut stale = conn("p", 1, PathKind::Lan);
        stale.health.mark_read_seen(NOW - TIMEOUT - 1, None);
        let c = vec![stale, healthy("p", 2, PathKind::Routed)];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(1)
        );
    }

    #[test]
    fn too_many_consecutive_failures_is_not_healthy() {
        // 连续失败超过阈值 ⇒ 不健康（MAX_FAIL=3，打 4 次）
        let mut flaky = conn("p", 1, PathKind::Lan);
        flaky.health.mark_read_seen(NOW, None);
        for _ in 0..=MAX_FAIL {
            flaky.health.mark_failure();
        }
        let c = vec![flaky, healthy("p", 2, PathKind::Bluetooth)];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(1)
        );
    }

    #[test]
    fn failures_within_threshold_still_healthy() {
        // 连续失败 == 阈值仍算健康（与 ConnectionHealth::is_healthy 的语义一致，
        // 边界不能在这里被悄悄改严）
        let mut flaky = conn("p", 1, PathKind::Lan);
        flaky.health.mark_read_seen(NOW, None);
        for _ in 0..MAX_FAIL {
            flaky.health.mark_failure();
        }
        let c = vec![flaky, healthy("p", 2, PathKind::Routed)];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0)
        );
    }

    #[test]
    fn path_rank_order_is_explicit() {
        // 直接钉住语义顺序，防止有人把枚举声明顺序当语义用
        assert!(path_rank(PathKind::Lan) < path_rank(PathKind::Routed));
        assert!(path_rank(PathKind::Routed) < path_rank(PathKind::Bluetooth));
    }

    // ===== Congestion 感知选路测试（M3-c）=====

    /// 造一条「健康 + 拥塞」的连接。
    fn congested(peer: &str, port: u16, kind: PathKind) -> Connection {
        let mut c = healthy(peer, port, kind);
        c.health.mark_congested(NOW, ChannelKind::Priority);
        c
    }

    /// Test A: LAN healthy + 不拥塞，Routed 也 healthy + 不拥塞
    /// → 保持 LAN 优先（拥塞信号不影响正常优先级）
    #[test]
    fn a_lan_not_congested_still_beats_routed() {
        let c = vec![
            healthy("p", 1, PathKind::Routed),
            healthy("p", 2, PathKind::Lan),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(1)
        );
    }

    /// Test B: LAN healthy + 拥塞，Routed healthy + 不拥塞
    /// → **congestion 不影响选路**，仍按 path_rank 选 LAN（恢复 v4.18.10 语义）
    ///
    /// 原因：Priority congestion 信号可能由"共享 writer_loop 被 bulk backpressure 挂起"
    /// 产生，不是 Priority 网络路径真拥塞的可靠测量。因此拥塞记录保留但选路忽略。
    #[test]
    fn b_lan_congested_routed_not_congested_still_picks_lan() {
        let c = vec![
            congested("p", 1, PathKind::Lan),
            healthy("p", 2, PathKind::Routed),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0) // LAN (rank 0) > Routed (rank 1)，不受 congestion 影响
        );
    }

    /// Test C: LAN healthy + 拥塞，BLE healthy + 不拥塞
    /// → **congestion 不影响选路**，仍按 path_rank 选 LAN（恢复 v4.18.10 语义）
    #[test]
    fn c_lan_congested_ble_not_congested_still_picks_lan() {
        let c = vec![
            congested("p", 1, PathKind::Lan),
            healthy("p", 2, PathKind::Bluetooth),
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0) // LAN (rank 0) > BLE (rank 2)，不受 congestion 影响
        );
    }

    /// Test D: LAN healthy + 拥塞，没有其他 healthy 非拥塞链路
    /// → 仍然选 LAN！不能返回 None（保持可用，拥塞只做让路不做封杀）
    #[test]
    fn d_lan_congested_no_alternative_still_picks_lan() {
        let c = vec![
            congested("p", 1, PathKind::Lan),
            never_seen("p", 2, PathKind::Routed), // 不健康
        ];
        // LAN 虽然拥塞但 healthy=true（read_seen 还在），Routed 直接不健康
        // → preferred 空（healthy 都拥塞）→ 退回到 healthy 里挑 → LAN 唯一 healthy → 选 LAN
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(0)
        );
    }

    /// Test E: LAN 曾经拥塞但已恢复（时间窗过期）
    /// → LAN 恢复正常优先级
    #[test]
    fn e_lan_congestion_recovered_resumes_priority() {
        let mut lan = healthy("p", 1, PathKind::Lan);
        lan.health
            .mark_congested(NOW - CONGESTION_WINDOW_MS - 1, ChannelKind::Priority); // 拥塞在窗口外 → 过期
        let c = vec![
            healthy("p", 2, PathKind::Routed),
            lan, // LAN 拥塞信号已过期，等价于不拥塞
        ];
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(1)
        );
    }

    /// Test F: 所有候选都拥塞（但都 healthy）
    /// → 退回到正常 PathKind 优先级（不拥塞让路不触发，因为 preferred 全空）
    #[test]
    fn f_all_candidates_congested_falls_back_to_normal_priority() {
        let c = vec![
            congested("p", 1, PathKind::Bluetooth),
            congested("p", 2, PathKind::Routed),
            congested("p", 3, PathKind::Lan),
        ];
        // 全拥塞 → preferred 全空 → 从 healthy 里选 → LAN 优先级最高 → index 2
        assert_eq!(
            pick_link(&c, NOW, TIMEOUT, MAX_FAIL, CONGESTION_WINDOW_MS),
            Some(2)
        );
    }
}
