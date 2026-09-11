//! LanDiscovery：UDP 广播 / 组播 / who_has → PeerCandidate。
//!
//! Phase 3 这一步只落地**候选解析**（纯函数、完全可测）：把收到的 UDP `announce`
//! 翻译成 `PeerCandidate`。socket 绑定 / 收发循环仍留在 `network::discovery`，
//! 待下一步由 Adapter 接入——先让「发现」与「连接」的边界在类型上成立（P-A04）。

use std::net::SocketAddr;

use crate::mesh::candidate::PeerCandidate;
use crate::mesh::endpoint::Endpoint;
use crate::mesh::path::PathKind;
use crate::mesh::peer::PeerIdentity;
use crate::protocol::UdpPacket;

/// 把一条 UDP `announce` 解析成候选。
///
/// 关键（P-A01）：身份取包内的 `device_id`，**绝不**用来源 IP 当身份。
/// - IP（与 UDP 源端口）只参与构造 endpoint——IP 会变，身份不变；
/// - TCP 端口取包内自报的 `tcp_port`，与 UDP 源端口无关。
///
/// 返回 `None`：非 announce 包、空 `device_id`、或就是本节点自己（回环广播）。
pub fn announce_to_candidate(
    pkt: &UdpPacket,
    src: SocketAddr,
    my_device_id: &str,
) -> Option<PeerCandidate> {
    if pkt.kind != "announce" {
        return None;
    }
    if pkt.device_id.is_empty() || pkt.device_id == my_device_id {
        return None;
    }
    Some(PeerCandidate::new(
        pkt.device_id.clone(),
        PeerIdentity {
            x25519_public_key: pkt.x25519_pubkey.clone(),
            ed25519_public_key: pkt.ed25519_pubkey.clone(),
        },
        Endpoint::Tcp(SocketAddr::new(src.ip(), pkt.tcp_port)),
        PathKind::Lan,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn pkt(kind: &str, id: &str, port: u16) -> UdpPacket {
        UdpPacket {
            kind: kind.into(),
            device_id: id.into(),
            nickname: "nick".into(),
            tcp_port: port,
            x25519_pubkey: None,
            ed25519_pubkey: None,
        }
    }

    /// 源端口是 UDP 的，与 TCP endpoint 端口无关。
    fn src(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), 41_234)
    }

    fn tcp(a: u8, b: u8, c: u8, d: u8, port: u16) -> Endpoint {
        Endpoint::Tcp(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(a, b, c, d)),
            port,
        ))
    }

    /// 端点 = 来源 IP + 包内自报的 tcp_port（不是 UDP 源端口）。
    #[test]
    fn endpoint_uses_source_ip_and_declared_tcp_port() {
        let p = pkt("announce", "ABC123", 59992);
        let c = announce_to_candidate(&p, src(192, 168, 1, 20), "me").unwrap();
        assert_eq!(c.device_id, "ABC123");
        assert_eq!(c.endpoint, tcp(192, 168, 1, 20, 59992));
        assert_eq!(c.path_kind, PathKind::Lan);
    }

    /// P-A01：IP 不是身份。同一来源 IP 报出不同 device_id → 两个不同身份。
    #[test]
    fn identity_comes_from_packet_not_source_ip() {
        let c1 = announce_to_candidate(&pkt("announce", "A", 59992), src(192, 168, 1, 20), "me")
            .unwrap();
        let c2 = announce_to_candidate(&pkt("announce", "B", 59992), src(192, 168, 1, 20), "me")
            .unwrap();
        assert_ne!(c1.device_id, c2.device_id);
        assert_eq!(c1.endpoint, c2.endpoint);
    }

    /// P-A01 另一面：IP 变了身份不变。同一 device_id 从不同 IP 来 → 同一身份、
    /// 不同端点，交给 PeerManager 后合并成同一 Peer 的多条 Connection。
    #[test]
    fn same_identity_from_different_ips_yields_two_endpoints() {
        let p = pkt("announce", "ABC123", 59992);
        let c1 = announce_to_candidate(&p, src(192, 168, 1, 20), "me").unwrap();
        let c2 = announce_to_candidate(&p, src(100, 80, 20, 30), "me").unwrap();
        assert_eq!(c1.device_id, c2.device_id);
        assert_ne!(c1.endpoint, c2.endpoint);
    }

    #[test]
    fn non_announce_kind_is_ignored() {
        // who_has 是探测请求，不是「谁存在」的答复 → 不产候选
        let p = pkt("who_has", "ABC123", 59992);
        assert!(announce_to_candidate(&p, src(192, 168, 1, 20), "me").is_none());
    }

    /// 自己广播的包（多网卡 / 组播回环）不能把自己当成 peer。
    #[test]
    fn own_announce_is_ignored() {
        let p = pkt("announce", "me", 59992);
        assert!(announce_to_candidate(&p, src(192, 168, 1, 20), "me").is_none());
    }

    #[test]
    fn empty_device_id_is_ignored() {
        let p = pkt("announce", "", 59992);
        assert!(announce_to_candidate(&p, src(192, 168, 1, 20), "me").is_none());
    }

    /// 公钥随 announce 携带进入候选身份（PeerManager 侧只补空、不覆盖）。
    #[test]
    fn public_keys_are_carried_into_candidate() {
        let mut p = pkt("announce", "ABC123", 59992);
        p.x25519_pubkey = Some("x1".into());
        p.ed25519_pubkey = Some("e1".into());

        let c = announce_to_candidate(&p, src(192, 168, 1, 20), "me").unwrap();
        assert_eq!(c.identity.x25519_public_key.as_deref(), Some("x1"));
        assert_eq!(c.identity.ed25519_public_key.as_deref(), Some("e1"));
    }
}
