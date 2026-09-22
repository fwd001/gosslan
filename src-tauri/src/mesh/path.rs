//! 链路路径类型：一条 Connection 走哪种网络路径。
//!
//! `PathKind` 与 `Endpoint` 正交：`Endpoint::Tcp` 既可能是 LAN 也可能是 Routed，
//! `Endpoint::Ble` 固定对应 Bluetooth。两者组合才完整描述「数据怎么走出去」。
//!
//! 与旧 `relay/mesh_router.rs` 的 `LinkKind`（只有 Lan / Bluetooth）不同，
//! 这里显式引入 `Routed`，为 Tailscale / VPN / 跨子网 TCP 路径预留。
//!
//! `Relay` 与 `Routed` 都是「跨网 TCP」，但语义不同，必须分开：
//! `Routed` 是**对端真实 IP 可路由直达**（VPN / 跨子网），`Relay` 是经**公网中转服务器**
//! 配对的密封电路 —— 它的 `Endpoint` 是服务器地址而不是对端地址。混为一谈会让界面把
//! 中转电路标成「跨网段 / VPN」，并让"按地址区分连接"的逻辑踩到同一服务器地址（ADR-0020）。

/// 网络路径类型。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PathKind {
    /// 局域网直连（UDP 发现 + TCP 传输）。
    Lan,
    /// 跨子网 / VPN / Tailscale 等已路由 IP 的 TCP 路径（对端真实 IP 直达）。
    Routed,
    /// 经公网中转服务器配对的密封电路（`Endpoint` 是服务器地址，不是对端地址）。
    Relay,
    /// BLE（BitChat BLE Mesh）链路。
    Bluetooth,
}

impl PathKind {
    /// 稳定字符串标识（供日志 / 诊断 / 将来序列化用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            PathKind::Lan => "lan",
            PathKind::Routed => "routed",
            PathKind::Relay => "relay",
            PathKind::Bluetooth => "bluetooth",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_kind_names_are_stable() {
        assert_eq!(PathKind::Lan.as_str(), "lan");
        assert_eq!(PathKind::Routed.as_str(), "routed");
        assert_eq!(PathKind::Relay.as_str(), "relay");
        assert_eq!(PathKind::Bluetooth.as_str(), "bluetooth");
    }
}
