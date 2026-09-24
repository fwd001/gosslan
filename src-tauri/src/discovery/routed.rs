//! Routed（已路由 IP）端点的**配置解析**：跨子网 / VPN / Tailscale 等场景下用户手填的地址。
//!
//! 设计 §8：不要为 Tailscale 单做一个机制——它只是 Routed IP 的一种实现，
//! WireGuard / ZeroTier / 企业 VPN / 普通跨子网路由都复用这一套。
//!
//! ⚠️ **本文件只负责"怎么把配置读出来"**，不负责发现与拨号。真正的 Routed 拨号在
//! `network/transport.rs` 的 routed_task（每 10s 一轮，读 `ROUTED_ENDPOINTS_KEY` 后
//! `connect_to_peer(.., PathKind::Routed, ..)`）。
//!
//! 这里曾有一个 `RoutedDiscovery` 实现 `Discovery` trait 的"新发现层"（Phase 3 目标结构：
//! `IP:PORT → TCP → Hello → Node ID → 产出 PeerCandidate`），但它**从未接进生产路径**
//! （零调用点，只有自身测试）。2026-09-24 架构复审 0-A2 连同 `discovery/{trait,manager,lan}.rs`
//! 一起删除 —— 留着它的后果不是冗余，是"地图上有两个家、改的人照着没在跑数据的那个改"
//! （见 `docs/migration-ledger.md` 与 `docs/domains.data.mjs` 的 `activeHome`）。
//! 它想立的规则仍然有效，写在下面这两条判据里：**身份只能来自握手，不能由端点推测**（P-A01）；
//! 未登记的地址不得注入候选。前者由 `handle_message` 的 Hello 验签落实。

use std::net::{IpAddr, SocketAddr};

use serde::{Deserialize, Serialize};

use crate::protocol::TCP_PORT;

/// 手动配置的 Routed 端点在 `settings` 表中的键。
pub const ROUTED_ENDPOINTS_KEY: &str = "routed_endpoints";

/// 一个手动配置的 Routed 端点。
///
/// `device_id` **可选**，两种语义：
/// - `Some(id)`：连接**已知**节点的跨子网 / VPN 路径。链路 key 直接用 `id`，
///   行为与历史版本完全一致（向后兼容旧配置）。
/// - `None`：连接**该地址上的** Gosslan 节点 —— 身份由 TCP 握手学来，正是 §8 的流程
///   `IP:PORT → TCP → Hello → Node ID → Identity → 建立 Peer`。
///   这是「少配置」的关键：用户没有理由被要求抄一串内部标识。
///
/// 历史说明：旧版注释曾写「必须携带 device_id」，理由是 `handle_message` 的 Hello
/// 分支要求 `device_id == peer_id`（身份绑定校验，见 INV-P21），主动方在收到 Hello
/// 之前无从得知对端身份。该限制已被「握手补全」（被动方回发 Hello）解除 ——
/// 主动方现在能在建链**之前**先完成一次握手、拿到真实身份，再登记链路。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutedEndpoint {
    /// 对端 device_id；省略表示「由握手学」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// `"ip:port"`（经命令层规范化后总是带端口，如 `100.64.0.1:59992`）
    pub address: String,
}

impl RoutedEndpoint {
    pub fn new(device_id: Option<String>, address: impl Into<String>) -> Self {
        Self {
            device_id,
            address: address.into(),
        }
    }

    /// 日志用：未指定 `device_id` 时给一个可读的占位。
    pub fn display_id(&self) -> &str {
        self.device_id.as_deref().unwrap_or("<握手学>")
    }

    /// 解析出可拨号的地址；格式非法返回 `None`（调用方负责记录并跳过，不要静默丢）。
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        parse_endpoint_addr(&self.address)
    }
}

/// 解析端点地址字符串（`ip` 或 `ip:port`）为 `SocketAddr`。
///
/// 省略端口时按标准 [`TCP_PORT`] 补全：端口是内部实现细节，用户没有理由必须知道它，
/// 更不该因为漏写而在拨号时被**静默跳过**。
///
/// **这是唯一实现** —— `RoutedEndpoint::socket_addr()`（拨号侧）与命令层的校验 /
/// 规范化都走它，避免「两处各解析一遍、行为还不一致」（曾真实发生：命令层接受
/// 裸 IP 并告诉用户「只填 IP 即可」，而拨号侧只认 `ip:port`，于是裸 IP 静默失效）。
pub fn parse_endpoint_addr(address: &str) -> Option<SocketAddr> {
    parse_endpoint_addr_on(address, TCP_PORT)
}

/// 公网中转服务器的默认端口。
///
/// **刻意不等于** [`TCP_PORT`]（局域网内 Gosslan 自己的 TCP 监听端口）：同一台机器上
/// 既跑着 LAN 服务、又恰好部署了中继时，两个默认端口相同会让"只填 IP"这条最自然的
/// 用法直接连到错的进程上，而那恰好是最难判断的故障形状。服务器侧默认也是这个数。
pub const RELAY_DEFAULT_PORT: u16 = 59993;

/// 同 [`parse_endpoint_addr`]，但由调用方给出"省略端口时补哪个端口"。
///
/// 解析逻辑只有这一份 —— 中继端点与跨网段端点必须用同一个语法（历史上两处各写一遍，
/// 结果命令层接受裸 IP、拨号侧只认 `ip:port`，用户看到"配成功了却永远连不上"）。
pub fn parse_endpoint_addr_on(address: &str, default_port: u16) -> Option<SocketAddr> {
    let address = address.trim();
    if let Ok(addr) = address.parse::<SocketAddr>() {
        return Some(addr);
    }
    address
        .parse::<IpAddr>()
        .ok()
        .map(|ip| SocketAddr::new(ip, default_port))
}

/// 解析存储的 JSON 数组。**逐条跳过非法条目**，绝不因一条坏数据导致整体失败。
pub fn parse_endpoints(json: &str) -> Vec<RoutedEndpoint> {
    serde_json::from_str::<Vec<RoutedEndpoint>>(json).unwrap_or_default()
}

/// 序列化待存储。
pub fn encode_endpoints(list: &[RoutedEndpoint]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 配置序列化往返
    #[test]
    fn endpoints_serialize_roundtrip() {
        let list = vec![
            RoutedEndpoint::new(Some("dev-a".into()), "100.64.0.1:59992"),
            RoutedEndpoint::new(Some("dev-b".into()), "10.0.0.5:60002"),
        ];
        let json = encode_endpoints(&list);
        assert_eq!(parse_endpoints(&json), list);
    }

    /// 坏数据 / 空输入不 panic，且解析结果为空
    #[test]
    fn malformed_endpoints_json_yields_empty() {
        assert!(parse_endpoints("").is_empty());
        assert!(parse_endpoints("not json").is_empty());
        assert!(parse_endpoints("{}").is_empty());
    }

    /// 端点地址解析：显式端口、**裸 IP（省略端口按 TCP_PORT 补全）**、空白容错；
    /// 非法输入返回 None。
    ///
    /// 裸 IP 这条是回归护栏：曾经命令层接受裸 IP 并提示「只填 IP 即可」，
    /// 而拨号侧只认 `ip:port` → 裸 IP 被静默跳过、永远不拨（用户无从察觉）。
    #[test]
    fn socket_addr_accepts_bare_ip_and_explicit_port() {
        // 显式端口
        assert_eq!(
            RoutedEndpoint::new(Some("a".into()), "100.64.0.1:60002").socket_addr(),
            Some(sa("100.64.0.1:60002"))
        );
        // 裸 IP → 补标准端口（手写 settings 时最常出现的形式）
        assert_eq!(
            RoutedEndpoint::new(Some("a".into()), "100.64.0.1").socket_addr(),
            Some(SocketAddr::new("100.64.0.1".parse().unwrap(), TCP_PORT))
        );
        // 前后空白容错（从聊天窗口复制地址常带空格）
        assert_eq!(
            RoutedEndpoint::new(Some("a".into()), "  100.64.0.1:60002  ").socket_addr(),
            Some(sa("100.64.0.1:60002"))
        );
        // 非法输入
        assert_eq!(
            RoutedEndpoint::new(Some("a".into()), "garbage").socket_addr(),
            None
        );
        assert_eq!(
            RoutedEndpoint::new(Some("a".into()), "").socket_addr(),
            None
        );
        assert_eq!(
            RoutedEndpoint::new(Some("a".into()), "100.64.0.1:").socket_addr(),
            None
        );
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// `device_id` 可省略：旧格式（带 id）与新格式（仅地址）都必须能解析。
    /// 省略时语义是「身份由握手学」—— 即 §8 的 `IP:PORT → TCP → Hello → Node ID`。
    ///
    /// 这是「少配置」的护栏：用户不该被要求抄一串内部标识；同时钉住**向后兼容**，
    /// 因为旧配置里是带着 `device_id` 的。
    #[test]
    fn device_id_is_optional_and_backward_compatible() {
        // 旧格式（历史配置）继续可解析、语义不变
        let legacy: Vec<RoutedEndpoint> =
            serde_json::from_str(r#"[{"device_id":"dev-a","address":"100.64.0.1:59992"}]"#)
                .unwrap();
        assert_eq!(legacy[0].device_id.as_deref(), Some("dev-a"));
        assert_eq!(legacy[0].display_id(), "dev-a");

        // 新格式：只填地址
        let minimal: Vec<RoutedEndpoint> =
            serde_json::from_str(r#"[{"address":"100.64.0.1"}]"#).unwrap();
        assert!(minimal[0].device_id.is_none(), "省略 device_id 应为 None");
        assert_eq!(minimal[0].display_id(), "<握手学>");
        assert_eq!(minimal[0].socket_addr(), Some(sa("100.64.0.1:59992")));

        // 序列化时 None 不写该键（配置保持干净），且往返一致
        let encoded = encode_endpoints(&minimal);
        assert!(
            !encoded.contains("device_id"),
            "未指定时不应写出该键: {encoded}"
        );
        assert_eq!(parse_endpoints(&encoded), minimal);
    }
}
