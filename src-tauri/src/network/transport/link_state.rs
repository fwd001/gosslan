// 链路状态与 conv_link 快照（mesh 层同步 6b-3、连接登记表、按节流打日志）
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第二批，判据与顺序见 docs/large-file-split-plan.md。

// 依据端点地址判断路径类型。
//
// 私有 / 环回 / 链路本地地址视为 LAN；其余（含 Tailscale 的 100.64/10 CGNAT 段，
// 它**不是** RFC1918 私有地址）视为 Routed —— 正好符合「跨子网走 Routed」的预期。
//
// 注意 IPv6 的 ULA（`fc00::/7`，含 Tailscale 的 `fd7a:115c:a1e0::/48`）**故意**留在
// Routed：它虽然叫「唯一本地地址」，但实践中主要出现在跨子网隧道里。判定只依赖
// 地址属性，不针对任何具体软件（§36：不要把 Clash / Tailscale 写死进网络核心）。

/// 某 peer 当前第一条连接（入站视角）的路径类型字符串。
///
/// 直连时这就是「对方 ↔ 我」的真实路径；桥接时是「中继 ↔ 我」的最后一段。
pub(crate) async fn inbound_path_kind(state: &AppState, peer_id: &str) -> String {
    // ① 锁作用域内只取快照（与 `try_send` 同规矩：不在锁里 await 别的锁）
    let links: Vec<crate::state::Link> = {
        let g = state.links.lock().await;
        match g.get(peer_id) {
            Some(l) if !l.is_empty() => l.clone(),
            _ => return PathKind::Lan.as_str().to_string(),
        }
    };
    // ② 取健康阈值与 mesh 连接，跑**与发送完全相同**的选路
    let (health_timeout_ms, max_failures) = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        (pm.health_timeout_ms(), pm.max_failures())
    };
    let conns: Vec<crate::mesh::Connection> = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        pm.get(peer_id)
            .map(|p| p.connections().to_vec())
            .unwrap_or_default()
    };
    let order = route_order(
        &links,
        peer_id,
        &conns,
        db::now_ms(),
        health_timeout_ms,
        max_failures,
    );
    // ③ 徽标显示「实际会走的那条」= 选路结果的第一条（见 `badge_path_kind`）。
    //
    // 为什么不能用 `first()`（旧实现）：一个 peer 可能同时有 LAN + Routed(+Relay/BLE) 多条连接，
    // `first()` 是**插入顺序**，与 `pick_link`（LAN > Routed > Relay > Bluetooth + 活性过滤）
    // 可能不一致 ⇒ 界面显示"桥接 N"，消息实际走的是 LAN（用户 2026-09-12 反馈过徽标不符）。
    // 选路函数返回空只可能发生在"全部候选都不可用"，此时退回首条（与发送时的兜底一致）。
    badge_path_kind(&links, &order).as_str().to_string()
}

/// 「当前链路」徽标该显示哪条路径：**选路结果的第一条**。
///
/// 抽成纯函数的原因：徽标是用户唯一能直接看见的链路信息，而它的正确性判据是
/// 「与实际发送选的同一条」—— 那是个下标对应关系，端到端很难复现
/// （要先制造 LAN + Routed 双路径、再对比徽标与日志），但纯函数可以一次钉死。
/// `order` 为空（全部候选不可用）时退回首条，与发送路径的兜底一致。
fn badge_path_kind(links: &[crate::state::Link], order: &[usize]) -> PathKind {
    let idx = order.first().copied().unwrap_or(0);
    links.get(idx).map(|l| l.path_kind).unwrap_or(PathKind::Lan)
}

/// 更新会话的「当前链路」快照（最近一条消息的链路 + 中间节点数）。
///
/// 只在链路**变化**时写并留一行日志——链路状态是内存态，日志是唯一可观测手段
/// （真机排障看连接实际走了哪条路）。收发消息频繁，不做无谓的重复写。
pub(crate) fn update_conv_link(state: &AppState, conv_id: &str, path: &str, hop: u8) {
    let mut links = state.conv_link.lock().unwrap_or_else(|e| e.into_inner());
    let changed = match links.get(conv_id) {
        Some(old) => old.path != path || old.hop != hop,
        None => true,
    };
    if changed {
        links.insert(
            conv_id.to_string(),
            LinkState {
                path: path.to_string(),
                hop,
            },
        );
        state
            .logger
            .info("link", format!("conv={conv_id} path={path} hop={hop}"));
    }
}

/// 忘掉某个 device_id 的**内存态身份绑定**（公钥 / 首见时间），链路不动。
///
/// ## 为什么必须有（用户 2026-09-13 真机：「必须重启才能重新加好友」）
///
/// 对方重装应用后公钥变了，而我们的身份表**只补空、不覆盖**（INV-P11：公钥冲突不静默覆盖）。
/// 用户按提示"删掉好友重新加"时，`friends` 表那一行确实没了 —— 但 `verify_hello` 的绑定
/// 还有**第二条腿**：内存里的 `peers` 表（广播里学来的、**未经验签**的旧公钥）。
/// 于是 Hello 继续被硬拒 ⇒ 消息与好友申请全都进不来 ⇒ **只有重启**（内存清空）才回落到 TOFU。
///
/// 删除好友 = 用户**显式**解除了这层信任 ⇒ 内存绑定必须一起失效。
/// 只清身份（公钥 / 首见时间），**不动链路、昵称、IP** —— 正在连着的会话不该被这一下打断。
pub(crate) fn forget_peer_identity(state: &AppState, device_id: &str) {
    {
        let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peers.get_mut(device_id) {
            p.x25519_pubkey = None;
            p.ed25519_pubkey = None;
            p.first_seen = None;
        }
    }
    state
        .peer_manager
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .forget_identity(device_id);
    // 允许下次**再提示一次**（否则"删了又加、对方又变了"时用户永远不再被告知）
    state
        .key_conflict_warned
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
    state.logger.info(
        "identity",
        format!(
            "已解除设备身份绑定 device_id={device_id}\
             （删好友 / 对方解除关系；下次连接以验签结果重新绑定，无需重启）"
        ),
    );
}

/// 直连链路刚建立时，把该会话的「当前链路」快照从"桥接"纠正为直连。
///
/// 为什么需要（真机 2026-09-14 全 Windows 局域网）：conv_link 是**上一条消息**的快照，
/// 发送方在无直连时乐观写 hop=1；之后即使直连建好了，聊天头也一直显示「桥接 · 1」，
/// 直到再发一条消息。这里在链路登记时主动纠正，避免界面长期误导。
///
/// 只在**已经存在该会话快照**时改，不新造条目。
fn note_direct_link(state: &AppState, peer_id: &str, path_kind: PathKind) {
    let mut links = state.conv_link.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = links.get_mut(peer_id) {
        let path = path_kind.as_str();
        if s.hop != 0 || s.path != path {
            s.path = path.to_string();
            s.hop = 0;
            state.logger.info(
                "link",
                format!("conv={peer_id} path={path} hop=0（直连已建立，纠正桥接快照）"),
            );
        }
    }
}

/// 连接建立后：把这条连接登记到 mesh 层的 `PeerManager`。
///
/// 这样 mesh 层的 Peer/Connection 才与传输层的 `Link` 一一对应，
/// Phase 2 建立的「任一 Connection 健康 ⇒ Online」才有真实连接数据支撑。
/// 公钥在此刻可能尚未学到（拨号侧），留空即可 —— 收到 Hello / announce 后由
/// `PeerIdentity::merge_missing` 补齐（只补空、不覆盖）。
/// 日志限频：同一个 key 每 `min_interval_ms` 最多放行一次。
///
/// 用**模块级静态**而不是 `AppState` 字段：它只服务日志，不值得为一个诊断辅助
/// 引入新的、需要清理的可增长状态。key 全是编译期字面量 ⇒ 表的规模天然有界。
fn log_throttled(key: &'static str, min_interval_ms: i64) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static LAST: OnceLock<Mutex<HashMap<&'static str, i64>>> = OnceLock::new();
    let now = crate::db::now_ms();
    let mut m = LAST
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match m.get(key) {
        Some(&t) if now - t < min_interval_ms => false,
        _ => {
            m.insert(key, now);
            true
        }
    }
}

pub(crate) fn register_connection(
    state: &AppState,
    peer_id: &str,
    endpoint: MeshEndpoint,
    path_kind: PathKind,
) {
    // 建链成功：排查真机连接问题（"什么时候连上的、走的哪条通道"）的第一手信息。
    // 在此之前网络层**完全没有**建链日志 —— 用户报「一会儿在线一会儿不在线」时，
    // 无从判断是哪条通道在反复建立/断开。
    state.logger.info(
        "link",
        format!(
            "建链 peer={peer_id} path={} ep={endpoint:?}",
            path_kind.as_str()
        ),
    );
    let identity = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers
            .get(peer_id)
            .map(|p| PeerIdentity {
                x25519_public_key: p.x25519_pubkey.clone(),
                ed25519_public_key: p.ed25519_pubkey.clone(),
            })
            .unwrap_or_default()
    };

    // 路径类型来自调用方（见 `Link::path_kind` 注释：从 IP 反推会把用户配置的
    // 私有段 Routed 端点误判成 LAN）
    let path = path_kind;
    let candidate = PeerCandidate::new(peer_id, identity, endpoint.clone(), path);
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    let (_, outcome) = pm.merge(candidate);
    // 建链即算一次「成功收发」—— 否则「已建立但还没收发」的连接会被健康判据算作不健康
    // （ADR-0014 §3.1 的硬性注意 ①：漏掉这一步，M3 的选路会把刚建好的连接判为不可用，
    // 进而退化成「按固定顺序挑」，甚至触发反复重拨）。
    let now = db::now_ms();
    // M3-0b：建链播种的是**读**活性（"刚建好就算活"），此后只由 `reader_loop` 刷新。
    // 若这里改成写活性，半开链路会重新变成永久健康。
    //
    // ⚠️ **只在真的是新连接时播种**（`is_new_connection`）。
    // 复核发现的原实现缺陷：无条件播种 ⇒ 同一个端点在握手/重连路径上被再次
    // `register_connection` 时，一条**已经死掉**（读活性过期）的 Connection 会被
    // 重新"续命"一个完整超时窗口；更糟的是刚播种的 LAN 链路（可能已是半开）
    // 会在该窗口内**压过一条真正健康的 Routed 链路**（选路按 LAN > Routed 排序）。
    if outcome.is_new_connection {
        pm.seed_connection_read_seen(peer_id, &endpoint, now);
    }
    // `online` 是 mesh 健康信号**在生产路径**唯一的外部可观测点：`ConnectionHealth` 是内存态，
    // 没有它就只能靠读代码相信「信号接上了」（这正是 M3-0 之前的状态）。
    let online = pm.online_state(peer_id, now) == PeerOnlineState::Online;
    state.logger.info(
        "mesh",
        format!(
            "+conn peer={peer_id} ep={endpoint} path={path:?} \
             new_peer={} new_conn={} conns={} online={}",
            outcome.is_new_peer,
            outcome.is_new_connection,
            pm.get(peer_id).map(|p| p.connection_count()).unwrap_or(0),
            u8::from(online),
        ),
    );
    // 直连建好了：把可能残留的"桥接"快照纠正过来（见 note_direct_link 的说明）。
    note_direct_link(state, peer_id, path_kind);
}

/// 连接断开后：从 mesh 层移除**这一条** Connection（同一 peer 的其他连接保留）。
pub(crate) fn unregister_connection(state: &AppState, peer_id: &str, endpoint: &MeshEndpoint) {
    let mut pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
    // ⚠️ `remove_connection` 会告诉我们**是否真的移除了**，不能不看就记日志。
    // 同一条链路有两条拆除路径都会走到这里（BLE 侧的 `teardown_link` 与传输层的统一拆除），
    // 无条件记日志就会打出两条一模一样的 `-conn … conns=0`，读起来像"同时断了两条链路"
    // —— 真机日志里出现过（用户 2026-09-16 的记录），排查掉线时会直接把人带偏。
    if !pm.remove_connection(peer_id, endpoint) {
        return;
    }
    state.logger.info(
        "mesh",
        format!(
            "-conn peer={peer_id} ep={endpoint} conns={}",
            pm.get(peer_id).map(|p| p.connection_count()).unwrap_or(0)
        ),
    );
}

#[cfg(test)]
mod mesh_sync_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn sa(a: u8, b: u8, c: u8, d: u8) -> std::net::SocketAddr {
        std::net::SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), 59992)
    }

    fn sa6(s: &str) -> std::net::SocketAddr {
        std::net::SocketAddr::new(s.parse::<IpAddr>().unwrap(), 59992)
    }

    /// 地址构造不依赖「拼字符串再解析」，因此 IPv6 **不需要方括号**。
    ///
    /// 旧实现 `format!("{ip}:{port}").parse()` 在 IPv6 上会得到 `fd7a::1:59992`
    /// 这种非法地址 → 解析失败 → 静默丢掉连接。这正是「IPv6 端点配了却不拨号」的根因。
    #[test]
    fn socket_addr_from_accepts_v4_and_bare_v6() {
        assert_eq!(
            socket_addr_from("192.168.1.20", 59992),
            Some(sa(192, 168, 1, 20))
        );
        assert_eq!(
            socket_addr_from("fd7a:115c:a1e0::1", 59992),
            Some(sa6("fd7a:115c:a1e0::1"))
        );
        assert_eq!(socket_addr_from("::1", 59992), Some(sa6("::1")));
        // 非法输入返回 None（调用方跳过本轮，不 panic）
        assert_eq!(socket_addr_from("not-an-ip", 1), None);
        assert_eq!(socket_addr_from("", 1), None);
        // 方括号写法是 `"host:port"` 整体的语法，不是裸 IP —— 传到这里应当被拒绝
        assert_eq!(socket_addr_from("[fd7a::1]", 1), None);
    }
}
