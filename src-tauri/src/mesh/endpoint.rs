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
    fn ble_endpoint_equality() {
        let a = Endpoint::Ble(BleEndpoint::new("node-1"));
        let b = Endpoint::Ble(BleEndpoint::new("node-1"));
        let c = Endpoint::Ble(BleEndpoint::new("node-2"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
