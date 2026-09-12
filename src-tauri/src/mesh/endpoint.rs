//! 网络端点：一条 Connection 的可达地址。
//!
//! 端点是与身份无关的**临时信息**（架构原则 P-A01 / 设计 §32）：
//! IP / BLE 句柄会变，而 `device_id` 不变。因此端点绝不能作为 Peer 的身份。

use std::net::SocketAddr;

/// BLE 端点：以字符串标识对端（BitChat node id / BLE 地址）。
///
/// Phase 7 接入真实 BLE 后端后再补充更结构化的字段（MTU、服务 / 特征 UUID 等），
/// 当前先以最小可测试形态存在。
#[derive(Clone, Debug)]
pub struct BleEndpoint {
    /// 对端在 BLE / BitChat 域内的标识（**保留原始大小写**：Android 侧要拿它去查
    /// Kotlin 维护的连接表，那里的键是系统给的 `device.address`，不能改）。
    pub address: String,
}

impl BleEndpoint {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }
}

/// **BLE 地址的比较与哈希一律忽略大小写**（真机踩过，症状极隐蔽）。
///
/// 同一台对端在不同角色下拿到的地址**大小写不一样**：
/// * macOS 外设角色（CoreBluetooth 回调）给的是**大写** UUID（`8474C5DD-…`）；
/// * macOS central 角色（btleplug 的 `PeripheralId::to_string()`）给的是**小写**（`8474c5dd-…`）。
///
/// 于是"这个端点是不是已经连上了"的去重比较**永远匹配不上** ⇒ 每轮扫描都重新拨号
/// ⇒ 每次连接都替换对端 GATT server 上的旧连接 ⇒ 把它拨过来的那条好链路反复打断
/// （用户 2026-09-12 真机：Mac 每 13s 重拨一次、手机那条链路 45s 无帧被看门狗拆掉、
/// 点「加好友」时正好落在死链窗口里报「连接已发送，对面没反应」）。
///
/// 保留原始字符串（Android 要拿它查表），但**相等性与哈希按小写算** —— 一行修掉所有比较点
/// （去重、拆链、端点快照），不需要在每个调用处各写一次 `eq_ignore_ascii_case`。
impl PartialEq for BleEndpoint {
    fn eq(&self, other: &Self) -> bool {
        self.address.eq_ignore_ascii_case(&other.address)
    }
}

impl Eq for BleEndpoint {}

impl std::hash::Hash for BleEndpoint {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.address.to_ascii_lowercase().hash(state);
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

#[cfg(test)]
mod ble_identity_tests {
    use super::*;
    use std::collections::HashSet;

    /// **同一台对端在不同角色下大小写不同，必须视为同一个端点**（真机踩过）。
    ///
    /// macOS 外设角色给大写 UUID、btleplug central 给小写 —— 不忽略大小写时
    /// "这个端点已经连上了"的去重永远不命中 ⇒ 每轮扫描重拨 ⇒ 反复打断对端那条好链路。
    #[test]
    fn ble_endpoint_equality_ignores_case() {
        let upper = BleEndpoint::new("8474C5DD-65D8-4847-4C4A-1C3C39E0CDBD");
        let lower = BleEndpoint::new("8474c5dd-65d8-4847-4c4a-1c3c39e0cdbd");
        assert_eq!(upper, lower, "大小写不同的同一地址必须相等");
        assert_eq!(
            Endpoint::Ble(upper.clone()),
            Endpoint::Ble(lower.clone()),
            "包装成 MeshEndpoint 后同样成立"
        );
        // 哈希也必须一致（HashSet/HashMap 的键才能命中）
        let mut set = HashSet::new();
        set.insert(Endpoint::Ble(upper));
        assert!(set.contains(&Endpoint::Ble(lower)), "哈希必须按小写算");
        // 原始大小写要保留（Android 侧拿它去查 Kotlin 的连接表）
        assert_eq!(
            self_address(&BleEndpoint::new("AA:BB")),
            "AA:BB",
            "不得把原始字符串改掉"
        );
    }

    fn self_address(e: &BleEndpoint) -> String {
        e.address.clone()
    }
}
