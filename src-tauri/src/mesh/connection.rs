//! Connection：到达某个 Peer 的一条具体链路。
//!
//! 一个 Peer 可拥有多条 Connection（LAN + Tailscale + BLE 各一条），
//! 每条独立记录端点与健康度。这与旧模型「Peer = 单 IP = 单 TCP」相反，
//! 是本次架构演进（P-A02）的核心拆分点。

use super::endpoint::Endpoint;
use super::path::PathKind;

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
}
