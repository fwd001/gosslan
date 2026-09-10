//! 链路路径类型：一条 Connection 走哪种网络路径。
//!
//! `PathKind` 与 `Endpoint` 正交：`Endpoint::Tcp` 既可能是 LAN 也可能是 Routed，
//! `Endpoint::Ble` 固定对应 Bluetooth。两者组合才完整描述「数据怎么走出去」。
//!
//! 与旧 `relay/mesh_router.rs` 的 `LinkKind`（只有 Lan / Bluetooth）不同，
//! 这里显式引入 `Routed`，为 Tailscale / VPN / 跨子网 TCP 路径预留。

/// 网络路径类型。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PathKind {
    /// 局域网直连（UDP 发现 + TCP 传输）。
    Lan,
    /// 跨子网 / VPN / Tailscale 等已路由 IP 的 TCP 路径。
    Routed,
    /// BLE（BitChat BLE Mesh）链路。
    Bluetooth,
}

impl PathKind {
    /// 稳定字符串标识（供日志 / 诊断 / 将来序列化用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            PathKind::Lan => "lan",
            PathKind::Routed => "routed",
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
        assert_eq!(PathKind::Bluetooth.as_str(), "bluetooth");
    }
}
