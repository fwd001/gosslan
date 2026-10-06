// 主动建链：拨号裁决、链路快照、握手超时与连接建立
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第四批；守卫视图与领域图的分册清单一同登记。

/// 拨号连接的超时上限。
///
/// 存在的理由：`TcpStream::connect` 在「SYN 被静默丢弃」时（对端防火墙 DROP、
/// VPN / 虚拟网卡路由黑洞）要等操作系统把 SYN 重传耗尽才返回 —— Linux/macOS
/// 可达 75s 以上，Windows 约 21s。
///
/// 而 `ensure_link` 是在 **UDP announce 接收循环里 `.await`** 的，没有上限就意味着
/// 一个收得到广播、TCP 却被丢弃的对端会把**整个发现循环堵死**（表现为发现假死、
/// 其他节点迟迟不出现）。Routed 拨号同样受影响。
///
/// 5s 远大于正常握手（同链路 <1ms；Tailscale 直连或经中继通常 <2s），只用于截断黑洞。
/// 心跳周期（秒）。**健康超时必须 ≥ 3 个周期**，见 `state.rs` 里
/// `PeerManager::new(15_000, 3)` 附近的说明与不变量测试；改这里要同步那个值。
pub const HEARTBEAT_INTERVAL_SECS: u64 = 5;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 主动拨号时等待对端回发 Hello 的上限 —— **只有「身份未知」的 Routed 端点会等**
/// （已知身份的路径不等，行为与历史一致）。
///
/// 远大于正常握手（同链路 <1ms、Tailscale 直连或中继 <2s），只用来兜住
/// 「对端是未升级的旧版本、不会回发 Hello」——否则该轮拨号会一直挂着。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 由字符串 IP + 端口构造 `SocketAddr`。
///
/// **刻意不用 `format!("{ip}:{port}").parse()`**：那种写法把地址与端口先拼成字符串，
/// 而 IPv6 只有写成 `[fd7a::1]:59992` 才是合法 SocketAddr，直接拼会得到
/// `fd7a::1:59992` → 解析失败。调用方拿到的地址在配置层已校验通过，重建失败就等于
/// **把一条合法配置静默丢掉**（曾真实发生：IPv6 端点「配了却永远不拨号」）。
/// 分开解析 IP 与端口，v4 / v6 都成立，也不需要方括号。
fn socket_addr_from(ip: &str, port: u16) -> Option<SocketAddr> {
    ip.parse::<IpAddr>()
        .ok()
        .map(|ip| SocketAddr::new(ip, port))
}

/// 拨号结果。
///
/// `Stopped` 与 `Failed` 分开，是为了在正常停机时不产生误导性的「拨号失败」日志；
/// LAN 路径的正常失败（对端离线、或该由对端拨号）则完全不打日志，避免刷屏。
enum DialOutcome {
    /// 本次真正建链成功（含首次握手学身份）。
    Connected,
    /// 端点已有一条连接（去重命中）。拨号任务每 10s 一轮，这是**常态**，不打日志
    /// —— 否则「已连上」会每 10s 重复刷屏，把真正的新连接淹掉。
    AlreadyConnected,
    /// 同一目标已有拨号在途（D6 在途去重命中）⇒ 本次不拨。同样是常态，不打日志。
    AlreadyDialing,
    /// 并发拨号已达上限（`MAX_CONCURRENT_DIALS`）⇒ 本轮放弃，下个周期再试。常态，不打日志。
    DialBusy,
    /// 拨号被停机信号中断（应用正在退出 / 切换网络）。
    Stopped,
    Failed(String),
}

/// 世代是否仍然有效（D8-4）。抽成纯函数是为了让"跨世代必须否决"这条**安全属性**
/// 有一个能被检索到、能被单测钉住的落点（真实路径要构造 AppState，单测造不出来）。
/// 某 peer 现有链路的快照 `(端点, 路径类型, 该连接是否健康)`。
///
/// 抽成 `pub(crate)` 的唯一目的是**让 BLE 走同一套入站/出站去重判据**
/// （`should_accept_inbound`），而不是在第三种传输里复制一份"有没有同路径连接"的判断 ——
/// 复核报告点名过"同一判断两处实现、行为还不一致"是这个项目踩过的坑。
#[cfg(feature = "bluetooth")]
pub(crate) async fn link_snapshot(
    state: &AppState,
    peer_id: &str,
) -> Vec<(MeshEndpoint, PathKind, bool)> {
    let list = {
        let links = state.links.lock().await;
        links.get(peer_id).cloned().unwrap_or_default()
    };
    let (timeout_ms, max_failures, conns) = {
        let pm = state.peer_manager.lock().unwrap_or_else(|e| e.into_inner());
        (
            pm.health_timeout_ms(),
            pm.max_failures(),
            pm.get(peer_id)
                .map(|p| p.connections().to_vec())
                .unwrap_or_default(),
        )
    };
    let now = db::now_ms();
    list.iter()
        .map(|l| {
            let healthy = conns
                .iter()
                .find(|c| c.endpoint == l.endpoint)
                .map(|c| c.health.is_healthy(now, timeout_ms, max_failures))
                .unwrap_or(true);
            (l.endpoint.clone(), l.path_kind, healthy)
        })
        .collect()
}

/// Hello 验签的 `pub(crate)` 包装：BLE 运行时（`network/ble.rs`）复用同一份验签逻辑，
/// **不允许**任何传输自己实现一遍（身份认证只应有一个实现）。
#[cfg(feature = "bluetooth")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_hello_for_ble(
    state: &AppState,
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
    sig_b64: &str,
) -> Result<(), String> {
    let r = verify_hello(
        state,
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig_b64,
    );
    // 蓝牙这条也一样：验签通过才算"被证明过"（理由见出站握手那处的注释）。
    if r.is_ok() {
        mark_peer_keys_verified(state, device_id, x25519_pubkey, ed25519_pubkey);
    }
    r
}

/// 读循环收尾时，「这个 peer 是否真的离线」的唯一判据（真值表由 `transport/tests.rs` 钉）。
///
/// 旧写法是 `removed && empty`：本端这一次**摘到了东西**才算离线。可半开看门狗
/// （`transport.rs` 里 `if v.is_empty() { links.remove(&peer); }` 那段）会先把整条 key 摘掉，
/// 于是读循环收尾时 `removed=false` 而 `empty=true` ⇒ 判成"还没离线"，
/// 而它后面那一串（清链路快照、标记离线、失败回收在途接收）全都不会跑：
/// 聊天头部的链路徽标继续显示在线、在途的那单文件要等 5 分钟空闲超时才失败、也没有掉线日志。
/// ⇒ 离线与否只取决于**这个 peer 还剩不剩链路**，与"是谁摘掉最后一条"无关；
/// 还剩别的链路（LAN + Tailscale + BLE 并存）时不许点亮，那正是 failover 的语义。
pub(crate) fn peer_offline_after_tail(removed: bool, empty: bool) -> bool {
    let _ = removed;
    empty
}

/// 入站去重判据（BLE 侧复用；TCP 侧在 `handle_incoming` 内联调用同一个函数）。
#[cfg(feature = "bluetooth")]
pub(crate) fn should_accept_inbound_public(
    my_id: &str,
    peer_id: &str,
    incoming: PathKind,
    existing: &[(MeshEndpoint, PathKind, bool)],
) -> bool {
    should_accept_inbound(my_id, peer_id, incoming, existing)
}

/// 世代是否仍然有效（D8-4）。
fn generation_is_current(captured: u64, current: u64) -> bool {
    captured == current
}

/// 单个 peer 允许并存的最大链路数（防御"同 peer 反复建链"的无界增长）。
///
/// 正常拓扑一个 peer 最多 4 条（LAN + Routed + Relay + BLE），取 6 留余量（例如换网瞬间新旧并存）。
pub(crate) const MAX_LINKS_PER_PEER: usize = 6;

/// **入站去重判据**（D6-2/D6-3 的核心，纯函数便于钉住）。
///
/// 背景：接受侧原先**无条件**把新连接 append 进 `links`，于是——
/// * 任意已验签对端可以反复建链，链条无界增长（每条 2 个 1024 容量信道 + 2 个任务）；
/// * 两侧都配了对方地址（或 LAN announce 时序不对称）时，同一对等关系会稳定停在
///   2 条镜像 TCP，`route_order`/心跳/候选都翻倍，还污染 M3 的多路径验收。
///
/// 判据设计（必须**确定性且对称**，否则会两边互拒导致谁也连不上）：
/// 1. 已有同**路径类型**的连接，且「本机是指定拨号方」（`my_id > peer_id`，与
///    `should_dial` 同一规则）⇒ 拒收这条入站：镜像里保留**我方拨出的**那条
///    （我方连接由我方健康判据管理，语义最清楚）。对端（小 ID）在同一条件下
///    会接受我们的拨入 ⇒ 双方算出同一个赢家，不会互拒。
/// 2. 链路数已达 `MAX_LINKS_PER_PEER` ⇒ 拒收（防无界增长）。
/// 3. 其余一律接受 —— 尤其**一条都没有时必须接受**，否则直接断掉连通性。
fn should_accept_inbound(
    my_id: &str,
    peer_id: &str,
    incoming: PathKind,
    // (端点, 路径类型, 该连接**当前是否健康**)
    existing: &[(MeshEndpoint, PathKind, bool)],
) -> bool {
    if existing.len() >= MAX_LINKS_PER_PEER {
        return false;
    }
    // 只有"指定拨号方"才拒绝镜像；小 ID 方始终接受（它本来就不主动拨）。
    //
    // ⚠️ 必须再加"那条已有连接**仍然健康**"：若它已经半开/僵死（还没被 watchdog 拆），
    // 按路径存在就拒收会把对端**刚拨进来的新鲜连接**也挡掉 —— 而本机因为
    // `has_lan_path` 仍为真也不会重拨（`ensure_link` 以为 LAN 已连通），于是双方
    // 要等 watchdog（最长 45s）拆掉死链路才能恢复。加了这个条件，新鲜连接立刻接管，
    // 恢复时间从"最长 45s"变成"这一次握手"。
    if my_id > peer_id
        && existing
            .iter()
            .any(|(_, k, healthy)| *k == incoming && *healthy)
    {
        return false;
    }
    true
}

/// 小 ID 兜底拨号的触发阈值：对端在线（announce 首次学到）却在本机无连接超过该时长，
/// 说明大 ID 一方拨不过来（单向可达 / 大 ID 长期离线），小 ID 兜底主动拨号。
/// 10s = 2 个 announce 周期（announce 5s 一轮），给大 ID 足够时间先拨通。
const BACKUP_DIAL_AFTER_MS: i64 = 10_000;

/// `ensure_link` 判据的**纯函数内核**（便于非空转单测）：
/// 该 peer 现有的这些端点里，是否已有**走 LAN 路径**的连接。
///
/// 单独抽出来的理由：D5 的回归点正是「把任意连接当成 LAN 已连通」——
/// 那是**一行布尔表达式**的错误，端到端很难复现（要先 Routed 连上、再等 announce），
/// 而这里可以逐条钉死：只有 Routed 端点 ⇒ `false`（要继续拨 LAN）。
fn has_lan_path(links: &[(MeshEndpoint, PathKind)]) -> bool {
    links.iter().any(|(_, kind)| *kind == PathKind::Lan)
}

/// 拨号决策的**可测入口**：把「现有连接 → 是否还要拨 LAN」这一步也收进函数里。
///
/// 为什么不直接在 `ensure_link` 里算 `has_lan_link`：那样「把任意连接当成 LAN 已连通」
/// 这个回归（D5）只会体现在一行布尔表达式上，测试无从钉住（helper 单独测是空的 ——
/// 只要调用点写错，helper 再对也没用）。收进来后，测试直接喂「只有 Routed 端点」，
/// 回归时该断言必然 FAIL。
fn should_dial_for_peer(
    my_id: &str,
    peer_id: &str,
    has_endpoint: bool,
    existing: &[(MeshEndpoint, PathKind)],
    first_seen: Option<i64>,
    now_ms: i64,
) -> bool {
    // 判据是**同路径（LAN）已连通**，不是「有任意连接」：后者会让先经 Routed 连上的
    // 对等关系永远拿不到 LAN 链路（D5）。
    let has_lan_link = has_endpoint || has_lan_path(existing);
    should_dial(
        my_id,
        peer_id,
        has_endpoint,
        has_lan_link,
        first_seen,
        now_ms,
    )
}

/// 是否该主动拨这个端点（纯函数，便于单测 + 护栏非空转）。
///
/// 决策顺序（越靠前越确定、越便宜，命中即短路）：
/// 1. `has_endpoint`：**这个端点**已经连上了 → 无事可做。
/// 2. `has_lan_link`：**LAN 这条路径**已经连通 → 不拨。
///    这一条源自 P1-2 的修正（原为「任意链路」），但 2026-09-12 复核发现原判据过宽：
///    只要 peer 有任何一条连接（例如先经 Routed/Tailscale 连上），LAN 路径就**永远拿不到**
///    ⇒ M3 的「LAN > Routed」优先级在这些拓扑里**永不生效**，多路径退化成单路径。
///    现在只在「LAN 已连通」时短路，Routed-first 的对等关系仍会补一条 LAN。
///
///    为什么不能按「这个端点」判（`has_endpoint` 单独判不行）：接受侧 `handle_incoming`
///    记录的 `Link.endpoint` 是 TCP **源地址（临时端口）**，而这里拿到的是 announce 自报的
///    **监听地址**，两者永不相等 ⇒ 被动方（小 ID）的第 1 条永远不命中，10s 后兜底拨号会
///    反向再拨一条，同一对节点稳定停留 **2 条镜像 TCP**。所以「同路径是否已连通」必须按
///    **路径类型**判（LAN 链路无论端点记的是监听地址还是临时端口，路径都是 LAN）。
/// 3. 本机是大 ID（`my_id > peer_id`）：恒拨（对称场景的确定性拨号方）。
/// 4. 本机是小 ID：仅当对端在线（`first_seen` 有值）且「首次发现」已超过
///    `BACKUP_DIAL_AFTER_MS` 才兜底拨 —— 给大 ID 足够时间先拨通；单侧不可达
///    （不对称 NAT / 防火墙）时由小 ID 补齐连通性。
fn should_dial(
    my_id: &str,
    peer_id: &str,
    has_endpoint: bool,
    has_lan_link: bool,
    first_seen: Option<i64>,
    now_ms: i64,
) -> bool {
    if has_endpoint || has_lan_link {
        return false;
    }
    if my_id > peer_id {
        return true;
    }
    first_seen.is_some_and(|since| now_ms - since >= BACKUP_DIAL_AFTER_MS)
}

pub async fn ensure_link(
    state: &Arc<AppState>,
    peer_id: &str,
    ip: &str,
    tcp_port: u16,
    shutdown: watch::Receiver<bool>,
) {
    // 端点解析失败则放弃本轮（下一轮 announce 会再试）。
    let Some(endpoint) = socket_addr_from(ip, tcp_port) else {
        return;
    };
    // 虚拟/隧道源地址（Clash fake-ip、Tailscale/CGNAT、link-local）不当作 LAN 直连去拨：
    // 真机 2026-09-14 全 Windows 局域网——announce 的源地址未经过滤，若来自 TUN，会拨出
    // 一条假的「LAN」链路并让 has_lan_path 永真，反而堵死真实 LAN 直连。真实 LAN 的
    // announce 会用真实地址再来一轮；隧道场景仍由用户配置的 Routed 端点负责。
    if let Ok(v4) = ip.parse::<Ipv4Addr>() {
        if is_virtual_ip(&v4) {
            return;
        }
    }
    // ① 这个端点已经连上了（典型是「自己拨出去的那条」）→ 本轮无事可做。
    let has_endpoint = state
        .has_endpoint(peer_id, &MeshEndpoint::Tcp(endpoint))
        .await;
    // ② **LAN 这条路径**已经连通 → 不再拨。
    //
    // 判据是「同路径是否已连通」，不是「有没有任意连接」也不是「有没有连到这个端点」：
    //   · 按端点判：接受侧记的是临时端口、这里比的是监听地址，永不相等 ⇒ 镜像重拨
    //     （见 `should_dial` 注释）；
    //   · 按「任意连接」判：先经 Routed/Tailscale/BLE 连上的对等关系**永远拿不到 LAN 链路**
    //     ⇒ M3 的「LAN > Routed」优先级永不生效（2026-09-12 复核抓到的多路径硬阻塞）。
    // 本函数只负责**给 LAN 路径补连通性**；Routed 由配置驱动、BLE 由发现驱动，都不经过这里。
    // 现有连接的端点快照（锁内只取数据，决策在锁外做）。
    // 快照里带上**路径类型**：判"LAN 是否已连通"必须看 Link 自己记的路径，
    // 不能按端点 IP 段反推（用户配置的私有段 Routed 端点会被误判成 LAN）。
    let existing_links: Vec<(MeshEndpoint, PathKind)> = {
        let links = state.links.lock().await;
        links
            .get(peer_id)
            .map(|v| {
                v.iter()
                    .map(|l| (l.endpoint.clone(), l.path_kind))
                    .collect()
            })
            .unwrap_or_default()
    };
    // 首次建链：大 ID 立即拨号，小 ID 等大 ID 拨；小 ID 在「对端在线却迟迟连不上」时兜底。
    let should = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        let first_seen = peers.get(peer_id).and_then(|p| p.first_seen);
        should_dial_for_peer(
            &state.device_id,
            peer_id,
            has_endpoint,
            &existing_links,
            first_seen,
            db::now_ms(),
        )
    };
    if !should {
        return;
    }
    // LAN 发现路径的拨号失败是常态（对端离线、或本轮该由对端拨），刻意不打日志；
    // 但**握手验签失败/身份不符**会在 `connect_to_peer` 内以 warn + 诊断事件留痕
    // （那是「有人冒充」或「配置写错」的信号，不能静默）。
    let _ = connect_to_peer(
        state,
        Some(peer_id),
        endpoint,
        PathKind::Lan,
        shutdown,
        None,
    )
    .await;
}

/// 建立一条到 `endpoint` 的连接。
///
/// 调用方传 **已解析好的 `SocketAddr`**：地址的解析与校验在配置/announce 层各做一次，
/// 这里不再「拼字符串再解析」（那是 IPv6 丢方括号的根源）。
///
/// `known_id` 决定握手方式：
/// - `Some(id)`：身份已知（LAN announce 学到 / 用户显式配置了 `device_id`）。
///   ⚠️ **仍然要握手验签**：`id` 只说明「对方自称/我们以为它是谁」，
///   未认证的 announce 不能充当身份（否则任意进程可冒用好友 id 接链并伪造 Ack /
///   FriendRemove）。且要求对端自称的 device_id 与 `id` 一致，不一致即失败留痕 ——
///   这样「配置写错」不再表现为静默单向黑洞。
/// - `None`：身份未知（Routed 端点只填了地址）。此时**必须先握手**：发自己的 Hello →
///   等对端回发的 Hello → 验签 → 得到真实 `device_id` 与双公钥，再登记链路。
///   这正是 §8 的 `IP:PORT → TCP → Hello → Node ID → Identity`，
///   也是「不用手填 device_id」的实现方式。
///
/// 为什么必须「先握手、再登记」：链路 key（`links` 的 HashMap key，以及 `writer_loop`
/// 持有的 `peer_id`）必须在 spawn 之前确定，而 `writer_loop` 要用它回写
/// `pending_reads`，事后无法改名。
#[allow(clippy::too_many_arguments)]
async fn connect_to_peer(
    state: &Arc<AppState>,
    known_id: Option<&str>,
    endpoint: SocketAddr,
    path_kind: PathKind,
    mut shutdown: watch::Receiver<bool>,
    // 经中继时才传（`None` = 直连，行为与今天逐字节一致，ADR-0020 D1/D3）：这条 socket
    // 其实通向一台公网中继服务器，先完成准入与两端协商、把读写半换成密封态，之后
    // **照原来的流程一字不改**地握手、登记、收发。
    relay: Option<RelayCtx<'_>>,
) -> DialOutcome {
    // 传输无关的端点表示（拨号本身仍是 TCP：BLE 走自己的拨号路径，见 ADR-0015）
    let ep = MeshEndpoint::Tcp(endpoint);
    // ① **在途去重（D6）**：`has_endpoint` 与"登记链路"之间隔着 connect + 握手（最长 10s），
    //    两条并发路径会同时看到"还没连上"从而各拨一条 ⇒ `links[peer]` 出现两条同端点链路。
    //    这里用 RAII 守卫登记"我正在拨"，任何提前返回/panic 都会自动释放。
    //    键：身份已知用 `peer:`（同一 peer 的不同地址不该同时拨），否则用 `ep:`。
    let dial_key = match known_id {
        Some(id) => format!("peer:{id}"),
        None => format!("ep:{endpoint}"),
    };
    let _in_flight = match crate::state::DialGuard::try_acquire(state, dial_key) {
        Some(g) => g,
        None => return DialOutcome::AlreadyDialing,
    };
    // ② 并发上限：挡"大量**不同**目标"的拨号洪泛（伪造 announce 可批量制造）。
    //    拿不到许可就本轮放弃 —— announce 5s 一轮、Routed 10s 一轮，都会再来。
    let Ok(_permit) = state.dial_permits.clone().try_acquire_owned() else {
        return DialOutcome::DialBusy;
    };
    // ③ 按端点去重：与 `ensure_link` 的检查构成双重保险（announce 与 Routed 拨号会并发触发）。
    // 身份未知时只能按端点判 —— 否则 10s 重试的每一轮都会重复建链。
    let already = match known_id {
        Some(id) => state.has_endpoint(id, &ep).await,
        None => state.has_endpoint_addr(&ep).await,
    };
    if already {
        return DialOutcome::AlreadyConnected;
    }

    // connect 与停机信号赛跑：`stop()` 只等后台任务 2s，若 connect 正在等超时，
    // 不中断就会拖慢退出 / `app.restart()`（后者还会与端口释放抢时间）。
    let stream = tokio::select! {
        biased;
        _ = shutdown.changed() => return DialOutcome::Stopped,
        res = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(endpoint)) => match res {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return DialOutcome::Failed(format!("连接失败: {e}")),
            Err(_) => {
                return DialOutcome::Failed(format!(
                    "连接超时（{}s 内未建立）",
                    CONNECT_TIMEOUT.as_secs()
                ))
            }
        },
    };

    let (raw_r, raw_w) = stream.into_split();
    // 握手阶段要直接读写 socket（此时还没有 writer / reader 循环），故声明为 mut。
    let mut r = TcpReceiver::new(raw_r);
    let mut w = TcpSender::new(raw_w);

    // 经中继时：先与服务器完成准入、再与对端完成协商密封，然后才走下面那段一模一样的
    // 握手。**协商失败直接断链，不降级为明文**（AI_RULES §19：不得静默绕过加密）。
    if let Some(ctx) = relay {
        if let Err((kind, e)) =
            relay_negotiate(&mut w, &mut r, ctx.device_id, ctx.signing, ctx.dial).await
        {
            // 档位进诊断事件：三类失败的处置动作完全不同（改地址 / 核口令 / 等对方开开关），
            // 混成一句"建链未成功"就会让人去查错的那一头（INTEGRATION.md §1.1）。
            state.push_diag_event(
                "relay_negotiate",
                &format!("[{}] {}; server={}", kind.as_str(), e, ctx.dial.server),
            );
            state.logger.warn(
                "relay",
                format!(
                    "协商失败[{}] peer={} server={}：{}",
                    kind.as_str(),
                    ctx.dial.peer_id,
                    ctx.dial.server,
                    e
                ),
            );
            return DialOutcome::Failed(e);
        }
    }

    // ---- 握手：**两条路径都必须验签**（§8 / ADR-0011）----
    //
    // 这里刻意**不做**「已知 id 就跳过握手」的捷径。复核抓到的 High 缺陷正是这个捷径：
    // `known_id` 来自**未认证的 UDP announce**（`pkt.device_id` + `src.ip()`）或本地配置，
    // 它只是「对方自称是谁 / 我们以为它是谁」，**不是身份**。跳过握手 ⇒ 任意进程只要
    // 广播一个好友的 device_id，就会被拨号并**以此身份**接链；随后它能伪造
    // FriendRemove（静默删好友）/ UserInfo / ReadReceipt / **Ack** —— 其中 Ack 会让
    // 发送方删掉 outbox 行，等于对**真实**好友的消息静默永久丢失。
    // 同理，配置里写错或过期的 device_id 会变成「能连上、但对方所有帧都被丢弃」的
    // 单向黑洞，而且原先**一行日志都没有**。
    //
    // 现在：先发自己的 Hello → 读对端 Hello → **必须验签**；`Some(id)` 还要求
    // 对端自称的 device_id 与预期一致，不一致直接失败并留日志。
    let conv_clock = match known_id {
        // 已知身份：可以带上真实会话时钟（未知身份只能发 0，对端 observe_clock 取 max 不会倒退）。
        Some(id) => {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            db::get_clock(&dbc, id)
        }
        None => 0,
    };
    let hello = build_signed_hello(state, conv_clock);
    if let Err(e) = write_frame(&mut w, &hello).await {
        // 握手帧的失败一律算"这次拨号没成"：此刻链路还没建立，没有"保留链路"可言。
        return DialOutcome::Failed(format!("握手发送失败: {}", e.reason()));
    }
    // 读对端回发的 Hello（对端收到我们的 Hello 后会回发，见 `handle_incoming`）。
    let first = tokio::select! {
        biased;
        _ = shutdown.changed() => return DialOutcome::Stopped,
        res = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut r)) => match res {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => return DialOutcome::Failed(format!("握手读取失败: {e}")),
            Err(_) => {
                return DialOutcome::Failed(format!(
                    "握手超时（{}s 内未收到对端 Hello —— 对端可能不是 Gosslan 节点）",
                    HANDSHAKE_TIMEOUT.as_secs()
                ))
            }
        },
    };
    let Message::Hello {
        device_id,
        tcp_port,
        nonce,
        sig,
        x25519_pubkey,
        ed25519_pubkey,
        ..
    } = &first
    else {
        return DialOutcome::Failed("握手失败: 对端首帧不是 Hello".to_string());
    };
    // 身份一致性：拨号目标是我们**以为**的 id 时，对端必须就是它。
    // 不匹配就断开并留日志 —— 既堵住冒充，也让「配置写错」不再表现为静默黑洞。
    if let Some(expected) = known_id {
        if device_id != expected {
            let reason =
                format!("握手身份不符：预期 {expected}，对端自称 {device_id}（端点 {endpoint}）");
            state.push_diag_event("hello_mismatch", &reason);
            state.logger.warn("transport", reason.clone());
            return DialOutcome::Failed(reason);
        }
    }
    if let Err(reason) = verify_hello(
        state,
        device_id,
        *tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig,
    ) {
        state.push_diag_event("hello_rejected", &format!("{reason}; from={endpoint}"));
        state.logger.warn(
            "transport",
            format!("握手验签失败：{reason}（端点 {endpoint}）"),
        );
        return DialOutcome::Failed(format!("握手失败: {reason}"));
    }
    // 验签通过 ⇒ 这对钥匙已被证明由该 device_id 的持有者使用 —— 与入站首帧同一个升级点。
    // 此前只有入站那条打标：出站与 BLE 两条**同样验过签**的路径不打标，`peer_keys_trusted`
    // 就永远为假 ⇒ 只靠拨号或蓝牙连上的好友，`friends.ed25519` 再也补不上（`upsert_peer`
    // 与 accept 两侧都要求 verified）⇒ 安全码算不出、公网中继永不准入。收紧绑定来源
    // 必须同时把这三条补齐，否则就是把安全改动做成可用性回退。
    mark_peer_keys_verified(state, device_id, x25519_pubkey, ed25519_pubkey);
    let peer_id: String = device_id.clone();
    let learned_hello: Option<Message> = Some(first);

    let ((high_tx, high_rx), (normal_tx, normal_rx), (low_tx, low_rx)) =
        link_channels(crate::protocol::FILE_CHUNK);
    // 本连接独立的取消信号（M3#6），语义同 `handle_incoming`。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: MeshEndpoint::Tcp(endpoint),
            path_kind,
            high: high_tx.clone(),
            normal: normal_tx.clone(),
            low: low_tx.clone(),
            cancel: cancel_tx,
        });
    // 同步到 mesh 层（拨号侧同样登记，路径类型由调用方携带）
    register_connection(state, &peer_id, ep.clone(), path_kind);
    replay_group_frames_to(state, &peer_id); // #77：出站新链路 ⇒ 补递窗口内的群历史
    tokio::spawn(writer_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        w,
        high_rx,
        normal_rx,
        low_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));

    // 握手已在建链前完成（两条路径都发过自己的 Hello，且都验过对端的 Hello），
    // 这里就地处理对端首帧 —— 走的正是 `handle_incoming` 那条路径
    // （写身份 + 双公钥、对齐会话时钟、冲刷待发队列）。
    if known_id.is_none() {
        // 留痕：配置里没写 device_id 时，这行日志是用户/开发者**唯一**能确认
        // 「到底连上了谁」的地方。
        state.logger.info(
            "transport",
            format!("握手学到对端身份 peer={peer_id} ep={endpoint}"),
        );
    }
    if let Some(first) = learned_hello {
        handle_message(state, &peer_id, first).await;
    }

    tokio::spawn(reader_loop(
        state.clone(),
        r,
        peer_id.clone(),
        ep.clone(),
        low_tx,
        shutdown,
        cancel_rx,
    ));
    flush_outbox(state, &peer_id).await;
    // 群密钥必须**先于**群消息补发（2026-09-24 RC2）：`handle_gossip` 在解密之前就把 msg_id
    // 登进去重表 ⇒ 密钥后到的那一条已经被"见过"挡掉，之后再重发多少次都没人消费。
    requeue_group_keys_for_peer(state, &peer_id);
    flush_pending_group_keys(state, &peer_id).await;
    flush_group_outbox(state, &peer_id).await;
    flush_pending_reads(state, &peer_id).await;
    flush_pending_group_reads(state, &peer_id).await;
    crate::commands::flush_pending_files(state, &peer_id).await;
    // 群文件离线投递：该 peer 的 pending GroupFile 顺序发送
    crate::commands::flush_pending_group_files(state, &peer_id).await;
    DialOutcome::Connected
}
