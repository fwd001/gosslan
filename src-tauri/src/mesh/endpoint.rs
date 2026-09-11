//! 网络端点：一条 Connection 的可达地址。
//!
//! 端点是与身份无关的**临时信息**（架构原则 P-A01 / 设计 §32）：
//! IP / BLE 句柄会变，而 `device_id` 不变。因此端点绝不能作为 Peer 的身份。

use std::net::SocketAddr;

/// BLE 端点：以字符串标识对端（BitChat node id / BLE 地址）。
///
/// Phase 7 接入真实 BLE 后端后再补充更结构化的字段（MTU、服务 / 特征 UUID 等），
/// 当前先以最小可测试形态存在。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BleEndpoint {
    /// 对端在 BLE / BitChat 域内的标识。
    pub address: String,
}

impl BleEndpoint {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }
}

/// 网络端点。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Endpoint {
    /// TCP 端点（LAN 与 Routed 路径共用，区别由 `PathKind` 表达）。
    Tcp(SocketAddr),
    /// BLE 端点。
    Ble(BleEndpoint),
}

impl std::fmt::Display for Endpoint {
    /// 日志/诊断用的一行文本。TCP 保持与旧日志完全一致的 `ip:port`
    /// （运维习惯与既有日志检索都依赖它），BLE 用 `ble:<地址>`。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Endpoint::Tcp(addr) => write!(f, "{addr}"),
            Endpoint::Ble(b) => write!(f, "ble:{}", b.address),
        }
    }
}

impl From<SocketAddr> for Endpoint {
    fn from(addr: SocketAddr) -> Self {
        Endpoint::Tcp(addr)
    }
}

impl Endpoint {
    /// TCP 地址（非 TCP 端点返回 `None`）。
    ///
    /// 存在的意义：**只有少数几处真正需要 IP**（TCP 拨号、回填 `peers.ip`、Windows 的
    /// SO_LINGER）。用它把这些地方显式标记出来，而不是让调用方到处 `match` ——
    /// 也避免将来有人拿 BLE 端点去 `unwrap`。
    pub fn as_tcp(&self) -> Option<SocketAddr> {
        match self {
            Endpoint::Tcp(addr) => Some(*addr),
            Endpoint::Ble(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_endpoint_equality() {
        let a = Endpoint::Tcp(SocketAddr::from(([192, 168, 1, 20], 59992)));
        let b = Endpoint::Tcp(SocketAddr::from(([192, 168, 1, 20], 59992)));
        let c = Endpoint::Tcp(SocketAddr::from(([192, 168, 1, 21], 59992)));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn display_keeps_tcp_format_and_marks_ble() {
        // TCP 的日志文本必须与改造前逐字一致（既有日志检索/排查习惯依赖它）
        assert_eq!(Endpoint::Tcp(SocketAddr::from(([10, 0, 0, 5], 60001))).to_string(), "10.0.0.5:60001");
        assert_eq!(Endpoint::Ble(BleEndpoint::new("node-1")).to_string(), "ble:node-1");
        assert_eq!(
            Endpoint::Tcp(SocketAddr::from(([10, 0, 0, 5], 60001))).as_tcp(),
            Some(SocketAddr::from(([10, 0, 0, 5], 60001)))
        );
        assert_eq!(Endpoint::Ble(BleEndpoint::new("node-1")).as_tcp(), None);
    }

    #[test]
    fn ble_endpoint_equality() {
        let a = Endpoint::Ble(BleEndpoint::new("node-1"));
        let b = Endpoint::Ble(BleEndpoint::new("node-1"));
        let c = Endpoint::Ble(BleEndpoint::new("node-2"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
