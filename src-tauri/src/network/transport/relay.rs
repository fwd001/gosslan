// ---- 公网中继（blind circuit relay）：会合循环 / 协商接线 / 撤链 ----
//
// 职责边界：只负责"如何经服务器多拿一条链路"。不碰帧语义、不碰选路优先级、不碰 Gossip
// 传播 —— 链路一旦建立，上面跑的就是既有的 `connect_to_peer` 全流程。
// 设计与取舍见 `docs/adr/0020-blind-circuit-relay.md`；服务器代码在独立仓库
// fwd001/gosslan-relay-server。
//
// ⚠️ 本文件被 `network/transport.rs` 用 `include!` 并进**同一个模块**（与
// `transport/outbound.rs`、`transport/gossip.rs` 同一先例）：因此这里**不得重复 import
// 父模块已有的名字**（`HashSet`/`SocketAddr`/`Arc`/`watch`/`Duration`/`AsyncRead`/
// `AsyncWrite`/`AppState`/`TcpSender`/`TcpReceiver`/`db`/`PathKind`/`MeshEndpoint`…），
// 否则 E0252。只 import 父模块没有的东西。
//
// 为什么这一大段可以这么薄（ADR-0020 的支点）：`connect_to_peer` 的链路 key 来自
// **握手验签学到的 device_id**，不来自 socket 地址（`discovery/routed.rs` 的 P-A01）。
// 所以对上层而言"穿过一台哑管道的字节流"与一条直连 TCP 没有区别 —— 这里只需要
// 把首行写给服务器、把两端协商做完、然后把已经密封好的读写半交回原来那套代码。

use crate::transport::relay_seal;

/// 会合循环周期。与 `routed_endpoints` 的拨号周期同值、同一个理由：每轮重读配置 ⇒
/// 运行时改配置免重启即生效；换天时两端各算一天靠"下一轮再来"自愈，不需要边界协商代码。
const RELAY_RENDEZVOUS_SECS: u64 = 10;
/// 协商线交换的超时。已配对的两端通常毫秒级；10s 覆盖跨境链路的一轮 RTT。
const RELAY_NEGOTIATE_TIMEOUT_SECS: u64 = 10;
/// 同时持有的中继电路上限。
///
/// 为什么必须有上限：一条电路 = 一条 TCP 连接，而"当前没有 LAN 链路的好友"在一个 30 人群里
/// 可以是 29。全开既不必要（聊天与群消息本来就靠 Gossip 洪泛跨跳送达，**一条电路进网即可
/// 触达全网**）也不经济。超出的好友走既有的多跳路径。
const MAX_RELAY_CIRCUITS: usize = 8;

/// 一次"经服务器"的拨号所需信息。`None` = 直连，此时 `connect_to_peer` 行为与今天逐字节一致。
pub struct RelayDial {
    /// 服务器地址（也记在这条链路的端点上 —— 代价见 ADR-0020 §4）。
    pub server: SocketAddr,
    /// 准入口令。只发给用户自己部署的服务器。
    pub token: String,
    /// 这一对好友今天的中继通道（64 位十六进制摘要）。
    pub channel_hex: String,
    /// 期望的对端 device_id：进协商签名材料，串线的电路会验签失败。
    pub peer_id: String,
    /// 对端的 Ed25519 公钥，**必须来自 friends 表的绑定值**。
    /// 拿协商线自报的公钥来验等于没验 —— 那是整个盲管道唯一的身份支点。
    pub peer_ed25519: String,
}

/// UTC 日序号（通道哈希的 epoch）。抽成函数是为了能被测试钉住。
pub fn relay_epoch_day(now_ms: i64) -> u64 {
    // 负数（时钟未同步）按 0：两侧都落到同一个"第 0 天"。宁可暂时可关联，
    // 也不要各算一个不相干的值导致永远配不上。
    if now_ms <= 0 {
        0
    } else {
        (now_ms / 86_400_000) as u64
    }
}

/// 换天窗口内**本轮该试哪一天**的哈希（INTEGRATION.md 要求 5）。
///
/// UTC 00:00 前后两端可能各算"今天"和"昨天"，只拨一个哈希就配不上，而且现场是"两端都显示
/// 已连接却始终配不成" —— 最难查的那种。指南给的主动方案是窗口内同时拨两个哈希；**这里做不到
/// "同时"**：在途守卫键是 `peer:{id}`（`transport.rs:2241`），同一个对端的第二条拨号会被它挡掉。
/// 所以按轮**交替** —— 每轮 10 秒，两端各自交替 ⇒ 最多两轮必然落在同一天上。
/// 这不违反要求 1（那条禁的是"同一个 ch 同时存在两条自己的连接"；任一时刻只有一个 ch 在拨）。
///
/// 抽成纯函数（`now_ms` + `round`）是为了能被测试钉住：窗口边界与交替序都不依赖真实时钟。
pub fn relay_day_to_try(now_ms: i64, round: u64) -> u64 {
    let day = relay_epoch_day(now_ms);
    if !relay_in_day_rollover_window(now_ms) {
        return day;
    }
    // 偶数轮今天、奇数轮昨天。`day` 为 0 时 saturating 停在 0 —— 那本来就是"时钟没同步、
    // 两侧都按第 0 天"的退化情形，两端仍然一致。
    match round % 2 {
        0 => day,
        _ => day.saturating_sub(1),
    }
}

/// 是否落在 UTC 换天前后各一小时的窗口里（窗口外不必多此一举）。
fn relay_in_day_rollover_window(now_ms: i64) -> bool {
    if now_ms <= 0 {
        return false;
    }
    let sec_of_day = (now_ms % 86_400_000) / 1_000;
    // 窗口 = 一天里首尾各一小时；写成"不在中间那段"而不是 `a || b`，
    // 是因为后者会被 clippy 判成手写 range contains（而且前者更少写错边界）。
    !(3_600..86_400 - 3_600).contains(&sec_of_day)
}

/// 挑出这一轮该建中继电路的好友。三条排除规则都是**判据**，不是偏好：
///
/// - `connected`：已有任意链路（LAN / 手动跨网段 / 别的电路）⇒ 不必绕公网。
///   这条是"局域网能连上时中转自然闲置"在**建立侧**的保证；选路侧另有 `pick_link` 的
///   LAN 恒优先兜着（`mesh/selection.rs`）。两边都成立才叫"闲置"。
/// - `via_relay`：已在本服务器上有电路 ⇒ 不重复开，且计入上限（否则每轮突破上限越开越多）。
/// - 没有绑定 Ed25519 公钥的好友根本不进候选（见 `db::list_bound_friend_identities`）：
///   不给陌生人走公网是 ADR-0020 D4 —— 今天"TOFU 未验证也能拿链路"在局域网可接受，公网不可。
pub fn pick_relay_targets(
    identities: &[(String, String)],
    my_id: &str,
    connected: &HashSet<String>,
    via_relay: &HashSet<String>,
    cap: usize,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (id, pk) in identities {
        if id == my_id || connected.contains(id) {
            continue;
        }
        // 已有电路也算进配额：本函数只决定"本轮还想新增几条"。
        if out.len() + via_relay.len() >= cap {
            break;
        }
        out.push((id.clone(), pk.clone()));
    }
    out
}

/// 当前链路快照：谁已有链路、谁已有到 `server` 的中继电路。
///
/// 一次锁内快照，而不是对每个好友各调一次 `has_link`：后者会在 N 次加解锁之间看到
/// 不一致的世界，并据此把同一对多开一条电路。
async fn relay_link_snapshot(
    state: &AppState,
    server: SocketAddr,
) -> (HashSet<String>, HashSet<String>) {
    let links = state.links.lock().await;
    let mut connected = HashSet::new();
    let mut via_relay = HashSet::new();
    for (peer, list) in links.iter() {
        if !list.is_empty() {
            connected.insert(peer.clone());
        }
        if list.iter().any(|l| l.endpoint == MeshEndpoint::Tcp(server)) {
            via_relay.insert(peer.clone());
        }
    }
    (connected, via_relay)
}

/// 关掉开关 / 换了服务器地址时，把**上一台服务器**上的电路撤掉。返回撤了几条。
///
/// 为什么不能等它自己空闲退出：`writer_loop` 会一直持有这条字节流 —— 用户点了"关闭公网
/// 中转"之后仍有一条活链路在往外发流量，那是"关不掉"，比连不上更不可接受。
///
/// 判据是"端点等于旧服务器地址"。极端情况下用户把同一个 `ip:port` 同时配成跨网段端点与
/// 中继服务器，那种撞车这里**宁可错撤**：被撤的链路会在下一轮 LAN/Routed 里自然重建。
async fn drop_relay_circuits(state: &Arc<AppState>, server: SocketAddr) -> usize {
    let victims: Vec<_> = {
        let links = state.links.lock().await;
        links
            .iter()
            .flat_map(|(peer, list)| {
                list.iter()
                    .filter(|l| l.endpoint == MeshEndpoint::Tcp(server))
                    .map(move |l| (peer.clone(), l.cancel.clone()))
            })
            .collect()
    };
    for (peer, cancel) in &victims {
        let _ = cancel.send(true);
        state
            .logger
            .info("relay", format!("撤销电路 peer={} server={}", peer, server));
    }
    victims.len()
}

/// 会合循环。仿 `routed_task`：每轮重读配置 ⇒ 运行时改配置免重启即生效。
pub async fn relay_rendezvous_task(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(RELAY_RENDEZVOUS_SECS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // 上一轮生效的服务器地址：只有它变化（含变回"未配置"）才需要撤旧电路。
    let mut prev_server: Option<SocketAddr> = None;
    // 轮次计数：只用来决定换天窗口内这一轮试哪一天的哈希（见 `relay_day_to_try`）。
    let mut round: u64 = 0;

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = tick.tick() => {}
        }
        round = round.wrapping_add(1);

        let cfg = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            crate::commands::load_relay_runtime(&dbc)
        };
        let Some(cfg) = cfg else {
            if let Some(old) = prev_server.take() {
                let n = drop_relay_circuits(&state, old).await;
                state
                    .logger
                    .info("relay", format!("公网中转已停用，已撤销 {} 条电路", n));
            }
            continue;
        };
        if let Some(old) = prev_server.filter(|old| *old != cfg.addr) {
            let n = drop_relay_circuits(&state, old).await;
            state.logger.info(
                "relay",
                format!("服务器已变更 {} → {}，先撤销 {} 条旧电路", old, cfg.addr, n),
            );
        }
        prev_server = Some(cfg.addr);

        let identities = {
            let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
            match db::list_bound_friend_identities(&dbc) {
                Ok(v) => v,
                Err(e) => {
                    // DB 错误**不折叠成"没有好友"**：那会让这一轮静默无事可做、下一轮还是
                    // 一样，用户只看到"配了却没连上"。
                    state
                        .logger
                        .warn("relay", format!("读好友公钥失败，本轮跳过: {}", e));
                    continue;
                }
            }
        };
        let (connected, via_relay) = relay_link_snapshot(&state, cfg.addr).await;
        let targets = pick_relay_targets(
            &identities,
            &state.device_id,
            &connected,
            &via_relay,
            MAX_RELAY_CIRCUITS,
        );
        if targets.is_empty() {
            continue;
        }
        // 换天窗口内按轮交替试今日/昨日（要求 5；为什么不能"同时"见 `relay_day_to_try`）。
        let day = relay_day_to_try(db::now_ms(), round);
        let my_pk = state.identity.ed25519_public_b64();
        let my_id = state.device_id.clone();
        let my_signing = state.identity.ed25519_signing.clone();

        // 并发拨号：一条卡住不能把同一轮里其他好友一起拖住（同 routed_task 的理由）。
        let mut dials = tokio::task::JoinSet::new();
        for (peer_id, peer_pk) in targets {
            let channel =
                relay_seal::channel_hash(&my_pk, &peer_pk, &my_id, &peer_id, day);
            let dial = RelayDial {
                server: cfg.addr,
                token: cfg.token.clone(),
                channel_hex: relay_seal::channel_hex(&channel),
                peer_id,
                peer_ed25519: peer_pk,
            };
            let state = state.clone();
            let shutdown = shutdown.clone();
            let (my_id, my_signing) = (my_id.clone(), my_signing.clone());
            dials.spawn(async move {
                let peer = dial.peer_id.clone();
                let outcome = connect_to_peer(
                    &state,
                    Some(&peer),
                    dial.server,
                    PathKind::Routed,
                    shutdown,
                    Some(RelayCtx {
                        dial: &dial,
                        device_id: &my_id,
                        signing: &my_signing,
                    }),
                )
                .await;
                match outcome {
                    DialOutcome::Connected => state.logger.info(
                        "relay",
                        format!("电路已建立 peer={} server={}", peer, dial.server),
                    ),
                    // 每 10s 一轮的常态：已有链路 / 在途 / 并发满 / 停机，静默。
                    DialOutcome::AlreadyConnected
                    | DialOutcome::AlreadyDialing
                    | DialOutcome::DialBusy
                    | DialOutcome::Stopped => {}
                    DialOutcome::Failed(e) => {
                        // 对端还没上线时这就是常态，所以 warn 不是 error；但必须留痕 ——
                        // "永远连不上"要靠这一行定位（同 routed 那次的教训）。
                        state
                            .logger
                            .warn("relay", format!("建链未成功 peer={}：{}", peer, e));
                    }
                }
            });
        }
        // 必须排空：JoinSet 被 drop 会立刻 abort 掉未完成任务（routed_task 的同一条教训）。
        while dials.join_next().await.is_some() {}
    }
}

/// `connect_to_peer` 走中继时要多知道的事：拨号参数之外，还需要本机身份来签协商线。
/// 打成一个小结构体是为了让 `connect_to_peer` 的参数只多**一个** `Option`，
/// 而不是多三个（那会把所有直连调用点都改一遍）。
pub struct RelayCtx<'a> {
    pub dial: &'a RelayDial,
    pub device_id: &'a str,
    pub signing: &'a ed25519_dalek::SigningKey,
}

/// 在 `Hello` 之前完成：服务器准入首行 + 两端协商 + 开启记录层密封。
///
/// 参数刻意**不含 `AppState`**（只要读写半 + 本机 device_id/签名私钥）：协商本身不需要
/// 别的运行时状态，而 `AppState` 要 `AppHandle` 才能构造 —— 换成现在这个签名，它就能被
/// duplex 上的真实端到端测试驱动（见下面的用例）。
///
/// 任一步失败一律 `Err`，调用方直接断开这条 socket。**没有"退回明文继续"这条路**
/// （`AI_RULES.md` §19：不得静默绕过/降级加密；失败要么明确报错要么明确重试）。
pub async fn relay_negotiate(
    w: &mut TcpSender,
    r: &mut TcpReceiver,
    my_device_id: &str,
    my_signing: &ed25519_dalek::SigningKey,
    dial: &RelayDial,
) -> Result<(), String> {
    // 只写的一侧需要这个 trait；读在 `read_bounded_line` 里，它自己 import。
    use tokio::io::AsyncWriteExt;

    // ① 服务器准入。首行之后服务器不再解析任何字节，所以写完就往下走 —— 它没有回复协议。
    let preamble = format!("GSRL1 {} {}\n", dial.token, dial.channel_hex);
    w.write_all(preamble.as_bytes())
        .await
        .map_err(|e| format!("中继首行写入失败: {e}"))?;

    // ② 协商线：**双向同时发**，因此不存在"谁先谁后"的死锁，也不依赖谁先连上服务器。
    let kp = relay_seal::generate_eph_keypair();
    let offer = relay_seal::build_wrap_offer(
        my_device_id,
        &dial.peer_id,
        &dial.channel_hex,
        my_signing,
        &kp,
    );
    w.write_all(offer.to_line().as_bytes())
        .await
        .map_err(|e| format!("协商线写入失败: {e}"))?;

    // ③ 读对端协商线（超时 + 长度闸门）。
    let line = tokio::time::timeout(
        Duration::from_secs(RELAY_NEGOTIATE_TIMEOUT_SECS),
        read_bounded_line(r),
    )
    .await
    .map_err(|_| format!("等待对端协商线超时（{}s）", RELAY_NEGOTIATE_TIMEOUT_SECS))?
    .map_err(|e| format!("读取协商线失败: {e}"))?;

    let peer_offer = relay_seal::WrapOffer::parse_line(&line).ok_or("对端协商线格式非法")?;
    let session = relay_seal::accept_wrap_offer(
        &peer_offer,
        my_device_id,
        &dial.peer_id,
        &dial.peer_ed25519,
        &dial.channel_hex,
        kp.secret,
    )
    .ok_or_else(|| {
        // 整条链路上最值得看的一行日志：它意味着"服务器上配对我的不是那位好友"——
        // 要么口令被外人共用（同一台服务器上串了线），要么对方重装过（公钥已变）。
        format!(
            "协商验签失败：对端不是 {}（或通道不符）。核对服务器口令是否只在这批人之间共享；若对方重装过 Gosslan，需删除后重新添加好友",
            dial.peer_id
        )
    })?;

    // ④ 从这里起 `Hello` 与之后所有帧都在密文里。就地换掉读写半的密封态 ——
    //    不新增循环、不改 `writer_loop`/`reader_loop` 的签名（见 `TcpSender::enable_seal`）。
    w.enable_seal(session.out_key);
    r.enable_seal(session.in_key);
    Ok(())
}

/// 读一行（`\n` 结束），**逐字节读**。
///
/// ⚠️ 为什么不用 `read_until` 或 `read(&mut buf[64])`：对端发完协商线之后**立刻**发第一条
/// 密封帧（`Hello`），两者很可能落在同一个 TCP 段里。按块读会把换行之后的字节一起吃进临时
/// 缓冲并丢掉 —— 表现是"协商成功但握手永远超时"，而且只在跨网段链路上出现，是本功能
/// 最难查的一种坏法。逐字节读的代价只有协商这一条线的百来次 `poll_read`，每条电路一次。
async fn read_bounded_line(r: &mut TcpReceiver) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    // 上限直接复用协商线的定义，不在两处各写一个数。
    const LINE_MAX: usize = relay_seal::WRAP_LINE_MAX;
    let mut buf = Vec::with_capacity(192);
    let mut byte = [0u8; 1];
    loop {
        let n = r.read(&mut byte).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "对端在协商线结束前断开",
            ));
        }
        if byte[0] == b'\n' {
            return String::from_utf8(buf)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "协商线不是 UTF-8"));
        }
        buf.push(byte[0]);
        if buf.len() > LINE_MAX {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "协商线超长",
            ));
        }
    }
}

#[cfg(test)]
mod relay_wiring_tests {
    use super::*;
    use crate::transport::tcp::{read_bytes, write_bytes, TcpReceiver, TcpSender};
    use ed25519_dalek::SigningKey;
    use tokio::io::{copy_bidirectional, duplex, split, AsyncReadExt, DuplexStream};

    /// 换天窗口（INTEGRATION.md 要求 5）：窗口外永远今日；窗口内按轮交替，
    /// 且**相邻两轮必然覆盖到另一天** —— 两端各自交替时，最多两轮就必然落在同一天上。
    #[test]
    fn rollover_window_alternates_the_day_per_round() {
        const DAY: u64 = 7_000;
        const MS: i64 = (DAY as i64) * 86_400_000;

        // 窗口外（UTC 正午）：奇偶轮都试今天，不该多此一举
        let noon = MS + 43_200_000;
        assert_eq!(relay_day_to_try(noon, 1), DAY);
        assert_eq!(relay_day_to_try(noon, 2), DAY);

        // 窗口内（刚过 00:00）：交替，并且第二轮必须回到今天，不能一路往前退
        let just_after = MS + 30 * 60_000;
        assert_eq!(relay_day_to_try(just_after, 0), DAY);
        assert_eq!(relay_day_to_try(just_after, 1), DAY - 1);
        assert_eq!(relay_day_to_try(just_after, 2), DAY, "交替必须回到今天");

        // 窗口内（差 1 秒到 00:00）：同样交替
        let just_before = MS + 86_399_000;
        assert_eq!(relay_day_to_try(just_before, 0), DAY);
        assert_eq!(relay_day_to_try(just_before, 1), DAY - 1);

        // 边界两侧对同一判据必须一致（否则一端算窗口内、另一端算外，等于没修）
        assert!(relay_in_day_rollover_window(MS + 3_599_000));
        assert!(!relay_in_day_rollover_window(MS + 3_600_000));

        // 时钟未同步：两侧都停在第 0 天且**不参与交替** —— 各算一个不相干的值才是真配不上
        assert_eq!(relay_day_to_try(-5, 1), 0);
        assert!(!relay_in_day_rollover_window(0));
    }

    fn id_set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn ident() -> (SigningKey, String) {
        let sk = SigningKey::generate(&mut rand_core::OsRng);
        (sk.clone(), STANDARD.encode(sk.verifying_key().to_bytes()))
    }

    // ─────────────────────── 纯判据 ───────────────────────

    /// 已有任何链路的好友**不得**再开中继电路：这是"局域网能连时自然闲置"的建立侧保证。
    #[test]
    fn picks_only_friends_without_any_link() {
        let identities = vec![
            ("gosslan-a".to_string(), "pkA".to_string()),
            ("gosslan-b".to_string(), "pkB".to_string()),
            ("gosslan-c".to_string(), "pkC".to_string()),
        ];
        let out = pick_relay_targets(
            &identities,
            "gosslan-a",
            &id_set(&["gosslan-b"]),
            &HashSet::new(),
            8,
        );
        assert_eq!(
            out.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["gosslan-c"],
            "自己与已有链路者都不该进来"
        );
    }

    /// 上限按"已有电路 + 本轮新增"一起算，否则每轮突破上限越开越多。
    #[test]
    fn cap_counts_existing_relay_circuits() {
        let identities: Vec<(String, String)> = (0..10)
            .map(|i| (format!("gosslan-i{}", i), format!("pk{}", i)))
            .collect();
        let empty = HashSet::new();
        assert_eq!(pick_relay_targets(&identities, "me", &empty, &empty, 8).len(), 8);
        assert_eq!(
            pick_relay_targets(
                &identities,
                "me",
                &empty,
                &id_set(&["gosslan-i0", "gosslan-i1", "gosslan-i2"]),
                8
            )
            .len(),
            5,
            "已有 3 条时本轮最多再补 5 条"
        );
        assert!(pick_relay_targets(&identities, "me", &empty, &empty, 0).is_empty());
    }

    /// epoch 必须按天稳定：写成任何带毫秒抖动的形式，两端就会各算各的通道，
    /// 表现是"永远配不上对而两边日志都正常"。
    #[test]
    fn epoch_day_is_day_granular() {
        let d = 86_400_000i64;
        assert_eq!(relay_epoch_day(0), 0);
        assert_eq!(relay_epoch_day(-5), 0, "时钟未同步时两侧都要落到同一天");
        assert_eq!(relay_epoch_day(d - 1), 0);
        assert_eq!(relay_epoch_day(d), 1);
        assert_eq!(relay_epoch_day(d * 2 + 12345), 2);
        assert_eq!(relay_epoch_day(d * 7 + 10), relay_epoch_day(d * 7 + 900));
    }

    // ─────────────────── 经"假中继"的端到端 ───────────────────

    /// 最小假中继：**照文档实现**（读掉两条 `GSRL1` 首行，然后把两边拼成双向字节管道，
    /// 之后一个字节都不看）。用它而不是真 Node 服务器，是为了让这条链路能在 `cargo test`
    /// 里跑；协议契约本身由服务器仓库自己的 8 条自测钉住。
    async fn dumb_relay(mut a: DuplexStream, mut b: DuplexStream) {
        for s in [&mut a, &mut b] {
            let mut line = Vec::new();
            loop {
                let byte = s.read_u8().await.unwrap_or(b'\n');
                if byte == b'\n' || line.len() > 512 {
                    break;
                }
                line.push(byte);
            }
        }
        // 首行之后：只搬字节。
        let _ = copy_bidirectional(&mut a, &mut b).await;
    }

    struct Side {
        w: TcpSender,
        r: TcpReceiver,
    }

    /// 两个客户端各拿一条管道，中间夹一个假中继。
    fn wire() -> (Side, Side, tokio::task::JoinHandle<()>) {
        let (cli_a, srv_a) = duplex(1024);
        let (cli_b, srv_b) = duplex(1024);
        let relay = tokio::spawn(dumb_relay(srv_a, srv_b));
        let (ar, aw) = split(cli_a);
        let (br, bw) = split(cli_b);
        (
            Side {
                w: TcpSender::new(aw),
                r: TcpReceiver::new(ar),
            },
            Side {
                w: TcpSender::new(bw),
                r: TcpReceiver::new(br),
            },
            relay,
        )
    }

    fn dial_for(channel_hex: &str, peer_id: &str, peer_ed25519: &str) -> RelayDial {
        RelayDial {
            server: "127.0.0.1:59993".parse().unwrap(),
            token: "shared-token".to_string(),
            channel_hex: channel_hex.to_string(),
            peer_id: peer_id.to_string(),
            peer_ed25519: peer_ed25519.to_string(),
        }
    }

    /// **本文件最重要的一条测试**：两端经"只会组网"的中继完成协商，然后
    /// 用既有的 `write_bytes`/`read_bytes` 互换帧。它同时钉住四件事 ——
    /// ① 首行/协商线的序列与服务器契约兼容；② 密封后上层帧格式逐字节不变；
    /// ③ 逐字节读确实避开了"吞掉第一条帧"；④ 双向同时发不会死锁。
    #[tokio::test]
    async fn two_peers_negotiate_and_exchange_frames_through_the_pipe() {
        let (sk_a, pk_a) = ident();
        let (sk_b, pk_b) = ident();
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        let ch = relay_seal::channel_hex(&relay_seal::channel_hash(
            &pk_a, &pk_b, id_a, id_b, 1,
        ));
        let (mut sa, mut sb, relay) = wire();
        let da = dial_for(&ch, id_b, &pk_b);
        let db = dial_for(&ch, id_a, &pk_a);

        let ha = tokio::spawn(async move {
            relay_negotiate(&mut sa.w, &mut sa.r, id_a, &sk_a, &da).await.unwrap();
            (sa.w, sa.r)
        });
        let hb = tokio::spawn(async move {
            relay_negotiate(&mut sb.w, &mut sb.r, id_b, &sk_b, &db).await.unwrap();
            (sb.w, sb.r)
        });
        let (mut aw, mut ar) = ha.await.unwrap();
        let (mut bw, mut br) = hb.await.unwrap();

        // 每一趟都必须**边写边读**：假中继与两端串起来的有效缓冲只有 1024 字节，
        // 先写完再读会让写侧填满管道后永久等待一个还没开始的读者 —— 那是测试的
        // 写法错，不是链路错（生产上 writer_loop 与 reader_loop 本来就是两个任务）。
        let (wt, rd) = tokio::join!(
            write_bytes(&mut aw, b"{\"type\":\"heartbeat\"}"),
            read_bytes(&mut br)
        );
        wt.expect("A 写入必须成功");
        assert_eq!(rd.expect("B 读取必须成功"), b"{\"type\":\"heartbeat\"}");

        // B → A：一条比链路缓冲大得多的帧（走"一条记录横跨多次 socket 读"的密文路径）
        let big = vec![0x33u8; 40_000];
        let (wt, rd) = tokio::join!(write_bytes(&mut bw, &big), read_bytes(&mut ar));
        wt.unwrap();
        assert_eq!(rd.unwrap(), big);

        // A → B：再来一条小的，证明两个方向的计数器没有互相打断
        let (wt, rd) = tokio::join!(write_bytes(&mut aw, b"tail"), read_bytes(&mut br));
        wt.unwrap();
        assert_eq!(rd.unwrap(), b"tail");

        relay.abort();
    }

    /// fail-closed：绑定值与对端真实身份不符（口令被外人共用 / 服务器串线 / 对方重装）
    /// ⇒ 两端都必须 `Err`，不得有一端"成功"而另一端卡住，也不得降级成明文。
    #[tokio::test]
    async fn negotiation_fails_closed_when_identity_does_not_match() {
        let (sk_a, pk_a) = ident();
        let (sk_b, _pk_b) = ident();
        let (_sk_evil, pk_evil) = ident(); // 谁也不是的第三把钥
        let (id_a, id_b) = ("gosslan-aaa", "gosslan-bbb");
        // 通道哈希按"pk_a ↔ pk_evil"算：B 的真实公钥与 A 绑定的 evil 值都不符
        let ch = relay_seal::channel_hex(&relay_seal::channel_hash(
            &pk_a, &pk_evil, id_a, id_b, 1,
        ));
        let (mut sa, mut sb, relay) = wire();
        let da = dial_for(&ch, id_b, &pk_evil);
        let db = dial_for(&ch, id_a, &pk_evil);

        let ha = tokio::spawn(async move {
            relay_negotiate(&mut sa.w, &mut sa.r, id_a, &sk_a, &da).await
        });
        let hb = tokio::spawn(async move {
            relay_negotiate(&mut sb.w, &mut sb.r, id_b, &sk_b, &db).await
        });
        let ra = ha.await.unwrap();
        let rb = hb.await.unwrap();
        assert!(ra.is_err(), "A 侧必须失败：{:?}", ra.ok());
        assert!(rb.is_err(), "B 侧必须失败：{:?}", rb.ok());
        // 两边的失败理由要能各自读出来（用户要按这个决定"改口令"还是"重新加好友"）
        assert!(ra.unwrap_err().contains("协商"));
        relay.abort();
    }

    /// 对端只发首行、从不发协商线 ⇒ 必须超时失败，而不是永久挂住。
    /// （永久挂住的代价是那条电路占着一个 `DialGuard` 与一条 socket。）
    #[tokio::test]
    async fn negotiate_times_out_when_peer_never_speaks() {
        let (sk_a, pk_a) = ident();
        let (_sk_b, pk_b) = ident();
        let ch = relay_seal::channel_hex(&relay_seal::channel_hash(
            &pk_a, &pk_b, "gosslan-aaa", "gosslan-bbb", 1,
        ));
        let (cli_a, mut srv_a) = duplex(1024);
        let (ar, aw) = split(cli_a);
        let mut w = TcpSender::new(aw);
        let mut r = TcpReceiver::new(ar);
        // 假中继只吃掉 A 的首行，然后**停在那里什么都不转发**（对端始终不出现）。
        // 必须让 `srv_a` 一直活着：任务一结束就把这一半边 drop 掉的话，cli_a 会立刻
        // 收到 broken pipe，A 在写协商线那步就失败了 —— 那就变成在测另一件事。
        let sink = tokio::spawn(async move {
            let mut discard = Vec::new();
            let mut byte = [0u8; 1];
            while let Ok(1) = srv_a.read(&mut byte).await {
                if byte[0] == b'\n' {
                    break;
                }
                discard.push(byte[0]);
            }
            // 读完首行后不再读、也不再写，但**不退出**：把 socket 留住。
            std::future::pending::<()>().await;
        });
        let dial = dial_for(&ch, "gosslan-bbb", &pk_b);
        let started = std::time::Instant::now();
        // 真实超时是 10s，测试里用 timeout 提前判失败：只要证明它**不是**立刻 Ok，
        // 且卡在读取协商线那一步（首行与协商线都已写出）。
        let res = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            relay_negotiate(&mut w, &mut r, "gosslan-aaa", &sk_a, &dial),
        )
        .await;
        assert!(res.is_err(), "对端不说话时必须还没返回");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        sink.abort();
    }
}
