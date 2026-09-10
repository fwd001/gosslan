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
#[derive(Clone, Debug, Default)]
pub struct ConnectionHealth {
    /// 最近一次心跳往返时延（毫秒）。
    pub rtt_ms: Option<u64>,
    /// 最近一次成功收发的时间戳（Unix 毫秒）。
    pub last_seen_ms: Option<i64>,
    /// 连续失败次数（成功后清零）。
    pub consecutive_failures: u32,
}

impl ConnectionHealth {
    /// 记录一次成功收发。
    pub fn mark_seen(&mut self, now_ms: i64, rtt_ms: Option<u64>) {
        self.last_seen_ms = Some(now_ms);
        self.rtt_ms = rtt_ms;
        self.consecutive_failures = 0;
    }

    /// 记录一次失败（连续失败计数 +1，饱和加防溢出）。
    pub fn mark_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }

    /// 是否健康：最近 `timeout_ms` 内有过成功、且连续失败未超过阈值。
    ///
    /// 语义对齐设计 §34：只有「任一 Connection 健康」才 ONLINE，反之 offline。
    pub fn is_healthy(&self, now_ms: i64, timeout_ms: i64, max_failures: u32) -> bool {
        let seen_recently = self
            .last_seen_ms
            .is_some_and(|t| now_ms.saturating_sub(t) <= timeout_ms);
        seen_recently && self.consecutive_failures <= max_failures
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
        h.mark_seen(1000, Some(5));
        assert_eq!(h.consecutive_failures, 0);
        assert_eq!(h.rtt_ms, Some(5));
        assert_eq!(h.last_seen_ms, Some(1000));
    }

    #[test]
    fn health_timeout_threshold_is_inclusive() {
        let mut h = ConnectionHealth::default();
        h.mark_seen(1000, None);
        // 恰好在 timeout 边界内 → 健康
        assert!(h.is_healthy(11_000, 10_000, 3));
        // 超时 → 不健康
        assert!(!h.is_healthy(11_001, 10_000, 3));
        // 从未成功 → 不健康
        let fresh = ConnectionHealth::default();
        assert!(!fresh.is_healthy(1000, 10_000, 3));
    }

    #[test]
    fn health_failure_threshold() {
        let mut h = ConnectionHealth::default();
        h.mark_seen(1000, None);
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
