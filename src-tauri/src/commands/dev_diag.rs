// 职责边界：
// - 开发诊断面板只读查询（状态快照、传输进度、peer 列表）
// - 只提供 get_* 查询，不做任何状态变更
// ---------------- 开发者诊断（隐藏面板用，只读不改网络行为） ----------------

/// 同步收集蓝牙通道事实（诊断面板用）。
///
/// 为什么不用 `network::ble::runtime_state`（那个是 async）：本函数在**同步命令**里跑，
/// 而 `state.links` 是 `tokio::sync::Mutex`。这里用 `try_lock` 尽力而为 —— 拿不到锁就把
/// 对端数留成通道给的值（下一轮刷新会补上），诊断面板绝不能因为统计而阻塞网络。
fn collect_ble_diag(s: &Arc<AppState>) -> crate::state::BleDiag {
    let mut d = crate::state::BleDiag {
        feature_compiled: cfg!(feature = "bluetooth"),
        activity: "idle".into(),
        ..Default::default()
    };
    // 通道 available / enabled：与 `RuntimeSnapshot` 完全同一口径（唯一真相源）。
    if let Some(bt) = TransportManager::new(s.clone())
        .status()
        .into_iter()
        .find(|c| c.channel == "bluetooth")
    {
        d.available = bt.available;
        d.enabled = bt.enabled;
        d.running = bt.running;
        d.peers = bt.peers;
    }
    #[cfg(feature = "bluetooth")]
    {
        // 链路表口径的"已建链对端数"：比通道计数更准，也不依赖那个占位 Transport 实现。
        d.peers = s
            .links
            .try_lock()
            .map(|links| {
                links
                    .values()
                    .filter(|ls| {
                        ls.iter()
                            .any(|l| l.path_kind == crate::mesh::PathKind::Bluetooth)
                    })
                    .count()
            })
            .unwrap_or(d.peers);
        d.running = s.ble.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        // 与通道同口径：起不来就是关（用户在设置里点的开，其实就是"真的在跑"）。
        d.enabled = d.running;
        d.available = d.available || d.running;
        d.no_dial = s
            .ble_no_dial
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        let now = crate::db::now_ms();
        let mut backoff: Vec<crate::state::BleBackoff> = s
            .ble_dial_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(id, (failures, next))| crate::state::BleBackoff {
                id: id.clone(),
                failures: *failures,
                remaining_ms: (*next - now).max(0),
            })
            .collect();
        backoff.sort_by_key(|a| std::cmp::Reverse(a.remaining_ms));
        d.backoff = backoff;
        let active = s.app_active.load(std::sync::atomic::Ordering::Relaxed);
        d.activity = if active {
            "active".into()
        } else {
            "idle".into()
        };
        d.scan_window_ms = crate::network::ble::scan_window_ms();
        d.scan_interval_ms = crate::network::ble::scan_interval_ms(active);
        let stats = *s.ble_scan.lock().unwrap_or_else(|e| e.into_inner());
        d.last_scan_ts = stats.last_ts;
        d.last_scan_total = stats.total;
        d.last_scan_matched = stats.matched;
    }
    d
}

/// 蓝牙候选行的一句话状态（诊断面板「候选链路」表里显示）。
fn ble_candidate_detail(d: &crate::state::BleDiag) -> String {
    if !d.feature_compiled {
        return "本次构建未编译蓝牙特性".into();
    }
    if !d.running {
        return if d.available {
            "未开启".into()
        } else {
            "不可用（无适配器或未授权）".into()
        };
    }
    let cadence = if d.activity == "active" {
        "前台"
    } else {
        "后台"
    };
    format!(
        "运行中 · {}节奏（{}s 扫描 / {}s 间隔）· {} 个对端",
        cadence,
        d.scan_window_ms / 1000,
        d.scan_interval_ms / 1000,
        d.peers
    )
}

/// 收集候选链路（网卡 + 蓝牙），供诊断面板展示自动选择逻辑的实际数据。
///
/// 用户 2026-09-13：「网卡-候选 也可以加上蓝牙」—— 于是蓝牙作为**一条候选**进同一张表，
/// 但它的字段语义与网卡不同（没有 IP / 广播 / RFC1918 / 虚拟网卡），
/// 所以用 `kind` 区分、用 `detail` 说人话，前端按 kind 渲染不同列。
///
/// `bt` 由调用方传入（一次采集、两处共用）：既省一次锁，也保证「候选表里的蓝牙行」
/// 与「蓝牙卡片」说的是**同一时刻**的状态。
fn collect_candidates(bt: &crate::state::BleDiag) -> Vec<crate::state::InterfaceCandidate> {
    use std::net::Ipv4Addr;

    fn is_virtual_ip(ip: &Ipv4Addr) -> bool {
        let o = ip.octets();
        (o[0] == 198 && (o[1] == 18 || o[1] == 19))
            || (o[0] == 100 && o[1] >= 64 && o[1] <= 127)
            || (o[0] == 169 && o[1] == 254)
    }
    fn is_rfc1918(ip: &Ipv4Addr) -> bool {
        let o = ip.octets();
        (o[0] == 10) || (o[0] == 172 && o[1] >= 16 && o[1] <= 31) || (o[0] == 192 && o[1] == 168)
    }
    fn is_virtual_name(name: &str) -> bool {
        let n = name.to_lowercase();
        [
            "utun",
            "tun",
            "tap",
            "wg",
            "docker",
            "br-",
            "veth",
            "virbr",
            "vmnet",
            "vboxnet",
            "hyper-v",
            "hv_",
            "vethernet",
            "cf-",
            "clash",
            "wintun",
            "tailscale",
            "ts-",
            "ham",
            "vpn",
        ]
        .iter()
        .any(|p| n.contains(p))
    }

    let mut out = Vec::new();
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in &ifs {
            if let if_addrs::IfAddr::V4(v4) = &i.addr {
                let ip = match i.ip() {
                    std::net::IpAddr::V4(v) => v,
                    _ => continue,
                };
                if ip.is_loopback() {
                    continue;
                }
                let has_bc = v4.broadcast.is_some();
                let rfc = is_rfc1918(&ip);
                let virt_ip = is_virtual_ip(&ip);
                let virt_name = is_virtual_name(&i.name);
                let mut score = 0i32;
                if has_bc {
                    score += 10;
                }
                if rfc {
                    score += 5;
                }
                if virt_ip {
                    score -= 50;
                }
                if virt_name {
                    score -= 30;
                }
                out.push(crate::state::InterfaceCandidate {
                    kind: "lan".into(),
                    name: i.name.clone(),
                    ip: ip.to_string(),
                    has_broadcast: has_bc,
                    broadcast: v4.broadcast.map(|b| b.to_string()),
                    is_rfc1918: rfc,
                    is_virtual: virt_ip || virt_name,
                    score,
                    selected: false, // 由调用方根据实际 bind_ip 设置
                    detail: String::new(),
                });
            }
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.ip.cmp(&b.ip)));

    // 蓝牙作为一条候选排在网卡后面（它不是"第 N 张网卡"，是另一条链路）。
    out.push(crate::state::InterfaceCandidate {
        kind: "bluetooth".into(),
        name: "蓝牙（BLE）".into(),
        ip: String::new(),
        has_broadcast: false,
        broadcast: None,
        is_rfc1918: false,
        is_virtual: false,
        score: 0,
        selected: bt.running,
        detail: ble_candidate_detail(bt),
    });
    out
}

/// 获取网络诊断状态（供隐藏开发者面板展示）。
///
/// 数据分两块，**互不冒充**：`mode`/`bound_ip`/… 只描述局域网；蓝牙在 `bluetooth` 里
/// 独立描述。纯蓝牙用户不会再看到"整机 offline"（用户 2026-09-13 的反馈）。
/// 「最近事件」已合并进运行日志，这里不再返回（见 `AppState::push_diag_event`）。
#[tauri::command(async)]
pub fn get_discovery_diag(state: State<'_, Arc<AppState>>) -> crate::state::DiscoveryDiag {
    let s = state.inner();
    let mut result = s.diag.lock().unwrap_or_else(|e| e.into_inner()).clone();
    {
        let net = s.network.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref h) = *net {
            result.mode = if h.bound_ip == "0.0.0.0" {
                "auto".into()
            } else {
                "manual".into()
            };
            // auto 模式下 Discovery 实际绑定的是真实 LAN IP，而不是 0.0.0.0。
            // tcp_listen 仍使用用户配置的地址（TCP 监听地址）。
            result.bound_ip = if h.actual_bound_ip.is_empty() {
                h.bound_ip.clone()
            } else {
                h.actual_bound_ip.clone()
            };
            result.tcp_listen = format!("{}:{}", h.bound_ip, h.tcp_port);
            result.udp_port = crate::protocol::UDP_PORT;
        } else {
            result.mode = "offline".into();
        }
    }
    // 候选链路（网卡 + 蓝牙）一次给全：面板只调一个命令，不会出现"两个命令数据不一致"。
    // 蓝牙事实只采集一次，候选表与蓝牙卡片共用同一份（保证同一时刻的状态）。
    let bt = collect_ble_diag(s);
    result.candidates = collect_candidates(&bt);
    result.bluetooth = bt;
    result
}

/// 获取候选链路列表（网卡 + 蓝牙，含评分）。
#[tauri::command(async)]
pub fn get_interface_candidates(
    state: State<'_, Arc<AppState>>,
) -> Vec<crate::state::InterfaceCandidate> {
    let bt = collect_ble_diag(state.inner());
    collect_candidates(&bt)
}
