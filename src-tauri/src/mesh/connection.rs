//! Connection：到达某个 Peer 的一条具体链路。
//!
//! 一个 Peer 可拥有多条 Connection（LAN + Tailscale + BLE 各一条），
//! 每条独立记录端点与健康度。这与旧模型「Peer = 单 IP = 单 TCP」相反，
//! 是本次架构演进（P-A02）的核心拆分点。

use super::endpoint::Endpoint;
use super::path::PathKind;

/// 通道类型 —— congestion 标记的粒度。
///
/// Connection 有两个独立的 mpsc channel：priority（Chat/Ack/Gossip）和 bulk
/// （FileChunk/GroupFileChunk）。writer 层已经 priority-first 物理隔离，
/// congestion 信号也必须按 channel 隔离 —— 否则 bulk Full 会污染 priority 选路。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Priority,
    Bulk,
}

/// 连接健康度（纯运行时信息，不持久化）。
///
/// 设计 §35：每条 Connection 独立记录 RTT / 成功 / 失败 / 连续失败；
/// Peer 的在线状态由这些 Connection 聚合得出（Phase 2 的 `online_state`）。
///
/// ## 为什么读写活性必须分开记（M3-0b）
///
/// M3-0 最初只有一个 `last_seen_ms`，**写成功与读成功写的是同一个字段**，
/// 而心跳每 5s 会给每条连接写成功一次 ⇒ 一条**半开 TCP**（对端已消失、
/// 但本机内核仍接受写入）会**永久保持「健康」**。选路若据此过滤，
/// 就会一直选中这条死路 —— 正是 ADR-0014 §7 要解决的失效场景。
///
/// 拆开之后：`last_read_seen_ms`（真的收到了对端的帧）才是「对端活着」的证据，
/// `is_healthy` 只看它。写成功只更新 `last_write_seen_ms`（诊断用，不参与判定）。
///
/// ## 为什么 congestion 必须按 channel 隔离（M3-c 修复）
///
/// Connection 有 priority / bulk 两个 mpsc channel，writer 层 priority-first
/// 物理隔离。但原先只有一个 `last_congestion_ms` —— bulk Full 会把整个 Connection
/// 标记为 congested → pick_link 把 LAN 从**所有**消息类型的 preferred 候选中排除，
/// 包括 priority 的 ChatMessage / Gossip / Ack。
///
/// 现在拆成独立的 `last_prio_congestion_ms` + `last_bulk_congestion_ms`：
/// pick_link 只检查 priority congestion，bulk congestion 不影响 Chat/Gossip 选路。
#[derive(Clone, Debug, Default)]
pub struct ConnectionHealth {
    /// 最近一次心跳往返时延（毫秒）。
    pub rtt_ms: Option<u64>,
    /// 最近一次**写出成功**的时间戳（Unix 毫秒）。诊断用，不参与 `is_healthy`。
    pub last_write_seen_ms: Option<i64>,
    /// 最近一次**读入成功**的时间戳（Unix 毫秒）：唯一「对端活着」的证据。
    pub last_read_seen_ms: Option<i64>,
    /// 连续失败次数（成功后清零）。
    pub consecutive_failures: u32,
    /// priority channel 最近一次**发送侧拥塞**被观察到的时间戳。
    /// priority 用于 Chat/Ack/Gossip 等控制消息。
    pub last_prio_congestion_ms: Option<i64>,
    /// bulk channel 最近一次**发送侧拥塞**被观察到的时间戳。
    /// bulk 用于 FileChunk/GroupFileChunk/大头像等大 payload。
    pub last_bulk_congestion_ms: Option<i64>,
}

impl ConnectionHealth {
    /// 记录一次成功**写出**（只刷新出站活性，**不**影响健康判定）。
    ///
    /// 半开 TCP 上写成功会持续发生，因此这里绝不能顺手刷新 `last_read_seen_ms`
    /// —— 那就退回 M3-0 的缺陷了。
    pub fn mark_write_seen(&mut self, now_ms: i64) {
        self.last_write_seen_ms = Some(now_ms);
        self.consecutive_failures = 0;
    }

    /// 记录一次成功**读入**（真正的「对端活着」）。
    pub fn mark_read_seen(&mut self, now_ms: i64, rtt_ms: Option<u64>) {
        self.last_read_seen_ms = Some(now_ms);
        self.rtt_ms = rtt_ms;
        self.consecutive_failures = 0;
    }

    /// 建链时**播种**一次读入活性。
    ///
    /// 唯一允许在 `reader_loop` 之外写入读活性的地方：刚建好的连接还没收到过帧，
    /// 若不播种就会被判不健康，`should_dial` 反复重拨（见 ADR-0014 §3.1 注意 1）。
    pub fn seed_read_seen(&mut self, now_ms: i64) {
        self.last_read_seen_ms = Some(now_ms);
        self.consecutive_failures = 0;
    }

    /// 记录一次失败（连续失败计数 +1，饱和加防溢出）。
    pub fn mark_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }

    /// 是否健康：最近 `timeout_ms` 内**读到过**对端的帧、且连续失败未超过阈值。
    ///
    /// ⚠️ 关于 `max_failures`：在当前设计里**这个分支不会触发** —— `writer_loop` 首次写失败
    /// 即 `break`（连接报废，不累积计数），而每次成功读写都会清零 `consecutive_failures`，
    /// 所以它只可能是 0 或 1。真正需要防的是**半开链路**（对端消失、内核仍收写：既不写失败
    /// 也读不到帧），它由**读活性超时拆除**处理（见 `network::transport` 的 watchdog 与
    /// ADR-0014 §3.3 的 M3-c 说明）。保留该参数只为将来引入显式失败信号时复用，
    /// **不要**在它上面继续加逻辑。
    ///
    /// 只认读活性（`last_read_seen_ms`）—— 写成功不构成「对端活着」的证据（半开 TCP）。
    /// 语义对齐设计 §34：只有「任一 Connection 健康」才 ONLINE，反之 offline。
    pub fn is_healthy(&self, now_ms: i64, timeout_ms: i64, max_failures: u32) -> bool {
        let read_recently = self
            .last_read_seen_ms
            .is_some_and(|t| now_ms.saturating_sub(t) <= timeout_ms);
        read_recently && self.consecutive_failures <= max_failures
    }

    /// 记录一次**发送侧拥塞**被观察到（queue Full / writer 阻塞等）。
    ///
    /// 只写时间戳，**不影响 `is_healthy`** —— 拥塞与 liveness 是独立维度。
    /// 允许：`healthy = true` 且 `congested = true`。
    ///
    /// 必须指定 channel：bulk congestion 不影响 priority 选路（writer 层
    /// 已经 priority-first 物理隔离，选路层必须尊重同样的隔离）。
    pub fn mark_congested(&mut self, now_ms: i64, channel: ChannelKind) {
        match channel {
            ChannelKind::Priority => self.last_prio_congestion_ms = Some(now_ms),
            ChannelKind::Bulk => self.last_bulk_congestion_ms = Some(now_ms),
        }
    }

    /// 拥塞已解除（writer 恢复消费 / TCP 窗口恢复）。
    ///
    /// writer_loop 里双通道都 empty 时调用 —— 同时清除两个 channel 的 congestion。
    /// 因为 writer 是两个 channel 共用一个 TCP 连接，writer 恢复就意味着两个
    /// channel 都不再被 writer 消费速度阻塞。
    pub fn mark_congestion_recovered(&mut self) {
        self.last_prio_congestion_ms = None;
        self.last_bulk_congestion_ms = None;
    }

    /// priority channel 是否判定为"当前拥塞"。
    /// pick_link 用于选路：只检查 priority congestion，bulk 拥塞不影响 Chat/Gossip。
    pub fn is_prio_congested(&self, now_ms: i64, window_ms: i64) -> bool {
        self.last_prio_congestion_ms
            .is_some_and(|t| now_ms.saturating_sub(t) <= window_ms)
    }

    /// bulk channel 是否判定为"当前拥塞"。
    /// 诊断用（将来可扩展 bulk 特定的选路逻辑）。
    pub fn is_bulk_congested(&self, now_ms: i64, window_ms: i64) -> bool {
        self.last_bulk_congestion_ms
            .is_some_and(|t| now_ms.saturating_sub(t) <= window_ms)
    }

    /// 是否判定为"任一 channel 当前拥塞"。
    /// 保留用于完整诊断场景；pick_link 不使用此方法。
    pub fn is_any_congested(&self, now_ms: i64, window_ms: i64) -> bool {
        self.is_prio_congested(now_ms, window_ms) || self.is_bulk_congested(now_ms, window_ms)
    }
}

/// 到达某 Peer 的一条链路。
#[derive(Clone, Debug)]
pub struct Connection {
    /// 全局唯一连接 id（本进程内，用于路由表 / 日志）。
    pub id: String,
    /// 所属 Peer 的 device_id。
    pub peer_id: String,
    /// 可达端点。
    pub endpoint: Endpoint,
    /// 路径类型。
    pub path_kind: PathKind,
    /// 健康度。
    pub health: ConnectionHealth,
}

impl Connection {
    /// 新建一条 Connection，自动生成唯一 id。
    pub fn new(peer_id: impl Into<String>, endpoint: Endpoint, path_kind: PathKind) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            peer_id: peer_id.into(),
            endpoint,
            path_kind,
            health: ConnectionHealth::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_marks_seen_and_resets_failures() {
        let mut h = ConnectionHealth::default();
        h.mark_failure();
        h.mark_failure();
        assert_eq!(h.consecutive_failures, 2);
        h.mark_read_seen(1000, Some(5));
        assert_eq!(h.consecutive_failures, 0);
        assert_eq!(h.rtt_ms, Some(5));
        assert_eq!(h.last_read_seen_ms, Some(1000));
    }

    /// 核心不变量（M3-0b）：**写成功不算健康证据**。
    /// 半开 TCP 上写会持续「成功」，若写也刷新读活性，死链路会永久健康。
    #[test]
    fn write_success_alone_never_makes_a_connection_healthy() {
        let mut h = ConnectionHealth::default();
        // 只有写成功（对端已死、内核仍收写）
        h.mark_write_seen(1000);
        assert_eq!(h.last_write_seen_ms, Some(1000));
        assert!(
            !h.is_healthy(1000, 10_000, 3),
            "只有写成功不得判健康 —— 否则半开链路会被选路一直选中"
        );
        // 之后真的读到了一帧 → 才算健康
        h.mark_read_seen(2000, None);
        assert!(h.is_healthy(2000, 10_000, 3));
    }

    /// 读活性会**过期**：最后一次读入超过阈值 ⇒ 不健康（这正是半开链路的收敛路径）。
    #[test]
    fn read_liveness_expires_for_half_open_link() {
        let mut h = ConnectionHealth::default();
        h.mark_read_seen(1000, None);
        // 半开期间写一直成功（心跳），但读一直没发生
        for t in 2..=20 {
            h.mark_write_seen(t * 1000);
        }
        assert!(h.is_healthy(11_000, 10_000, 3), "边界内仍健康");
        assert!(
            !h.is_healthy(11_001, 10_000, 3),
            "写成功刷了 19 次也必须因读活性过期而判不健康"
        );
    }

    /// 建链播种：刚建好（尚未收到任何帧）必须算健康，否则会触发反复重拨。
    #[test]
    fn seed_makes_fresh_connection_healthy_without_any_read() {
        let mut h = ConnectionHealth::default();
        h.seed_read_seen(1000);
        assert!(h.is_healthy(1000, 10_000, 3));
        // 超过阈值仍会过期 —— 播种只是一次性，不是永久豁免
        assert!(!h.is_healthy(12_000, 10_000, 3));
    }

    #[test]
    fn health_timeout_threshold_is_inclusive() {
        let mut h = ConnectionHealth::default();
        h.mark_read_seen(1000, None);
        // 恰好在 timeout 边界内 → 健康
        assert!(h.is_healthy(11_000, 10_000, 3));
        // 超时 → 不健康
        assert!(!h.is_healthy(11_001, 10_000, 3));
        // 从未读到过 → 不健康
        let fresh = ConnectionHealth::default();
        assert!(!fresh.is_healthy(1000, 10_000, 3));
    }

    #[test]
    fn health_failure_threshold() {
        let mut h = ConnectionHealth::default();
        h.mark_read_seen(1000, None);
        for _ in 0..3 {
            h.mark_failure();
        }
        // 连续失败 == 阈值 → 仍健康
        assert!(h.is_healthy(1000, 10_000, 3));
        h.mark_failure();
        // 连续失败 > 阈值 → 不健康
        assert!(!h.is_healthy(1000, 10_000, 3));
    }

    // ===== Congestion 新维度测试 =====

    /// Test 1: 默认状态 → healthy 由 read_seen 决定，congested=false
    #[test]
    fn default_connection_is_not_congested() {
        let h = ConnectionHealth::default();
        // 从未 read → 不健康
        assert!(!h.is_healthy(1000, 10_000, 3));
        // 从未拥塞 → 不拥塞
        assert!(!h.is_any_congested(1000, 5_000));
    }

    /// Test 1b: 正常 connection → healthy=true, congested=false
    #[test]
    fn healthy_connection_is_not_congested() {
        let mut h = ConnectionHealth::default();
        h.mark_read_seen(1000, None);
        assert!(h.is_healthy(1000, 10_000, 3));
        assert!(!h.is_any_congested(1000, 5_000));
    }

    /// Test 2: mark_congested → congested=true，同时 healthy 仍 true
    /// 这是"healthy 但不可及时发送"的核心状态
    #[test]
    fn congestion_is_independent_of_healthy() {
        let mut h = ConnectionHealth::default();
        h.mark_read_seen(1000, None);
        assert!(h.is_healthy(1000, 10_000, 3), "先建立 healthy 基线");

        h.mark_congested(2000, ChannelKind::Priority);
        assert!(
            h.is_any_congested(2000, 5_000),
            "mark_congested 后应为 congested"
        );
        assert!(
            h.is_healthy(2000, 10_000, 3),
            "拥塞不应该破坏 healthy —— 两个维度必须独立"
        );
    }

    /// Test 3: 拥塞恢复 → 再判不拥塞
    #[test]
    fn congestion_can_be_recovered() {
        let mut h = ConnectionHealth::default();
        h.mark_congested(1000, ChannelKind::Priority);
        assert!(h.is_any_congested(1000, 5_000));

        h.mark_congestion_recovered();
        assert!(!h.is_any_congested(2000, 5_000), "恢复后不应再判拥塞");
    }

    /// Test 3b: 时间窗过期 → 自动降级为不拥塞（不需要显式恢复）
    #[test]
    fn congestion_expires_after_window() {
        let mut h = ConnectionHealth::default();
        h.mark_congested(1000, ChannelKind::Priority);
        assert!(
            h.is_any_congested(6_000, 5_000),
            "窗口内（边界 inclusive）仍拥塞"
        );
        assert!(!h.is_any_congested(6_001, 5_000), "窗口过期自动不拥塞");
    }

    /// Test 4: is_healthy 现有语义绝对不能被 congestion 破坏
    /// 即使 congested=true，只要 read_seen 正常 → is_healthy 必须仍为 true
    #[test]
    fn congestion_does_not_affect_is_healthy_at_all() {
        let mut h = ConnectionHealth::default();
        h.mark_read_seen(1000, None);
        h.mark_congested(5000, ChannelKind::Priority);

        // read_seen 过期前 → healthy，congestion 还在窗口内 → 两者同时成立
        assert!(h.is_healthy(5_000, 10_000, 3));
        assert!(h.is_any_congested(5_000, 5_000));

        // read_seen 过期后 → unhealthy（这是 read_seen 自己过期的结果）
        // 此时 congestion 窗口还没过期（5000+5000=10000 < 11001 其实也过期了...）
        // 换一组数字让 congestion 还在：mark_congested 在 read_seen 过期前刚发生
        let mut h2 = ConnectionHealth::default();
        h2.mark_read_seen(1000, None);
        h2.mark_congested(9_900, ChannelKind::Priority); // 离 read_seen 9s，离 is_healthy timeout 还有 100ms
        assert!(
            h2.is_healthy(10_050, 10_000, 3),
            "read_seen 还没过期 → healthy"
        );
        assert!(
            h2.is_any_congested(10_050, 5_000),
            "congestion 窗口还没过期"
        );
        // 两者同时成立 — 这就是 Router 下阶段要识别的 "healthy 但 congested"
    }

    /// 综合场景：模拟完整周期
    #[test]
    fn congestion_lifecycle_ready_congested_recover() {
        let mut h = ConnectionHealth::default();
        // Ready: 正常收发
        h.mark_read_seen(1000, None);
        h.mark_write_seen(1010);
        assert!(h.is_healthy(1010, 10_000, 3));
        assert!(!h.is_any_congested(1010, 5_000));

        // queue Full → Congested
        h.mark_congested(1020, ChannelKind::Priority);
        assert!(h.is_any_congested(1020, 5_000));
        assert!(h.is_healthy(1020, 10_000, 3), "拥塞不影响健康");

        // writer 恢复消费 → 新的 write_seen 同时恢复拥塞
        h.mark_write_seen(1030);
        h.mark_congestion_recovered();
        assert!(!h.is_any_congested(1030, 5_000));
        assert!(h.is_healthy(1030, 10_000, 3));
    }
}
