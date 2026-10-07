// BLE **外设角色**那一半：起广播 → 收 central → 首帧判路由 → 握手 → 建链与顶替旧链。
// 
// 为什么单独一册：这一半带三张表（routes / handshaking / RouteCtl 通道）和一个"同一 central 地址
// 被复用"的顶替问题 —— 与 central 侧那半的登记表完全不同，混在一起最容易把事件循环灌成洪水。
// 
// 恒等判据（与 transport / file 两刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 本模块整体在 `#[cfg(feature = "bluetooth")]` 后面（`network/mod.rs:10`）⇒ 这些册跟着根文件一起门控，
//   不需要各自再写 cfg。搬家同批必须做的三件事：`network/mod.rs::ble_src_for_guards()` 登记本册、
//   `docs/domains.data.mjs` 认领、`scripts/check-ble-constants.mjs` 的 BLE_DOMAIN_FILES 覆盖本册
//   （那份名单是**硬编码文件清单** —— 漏了不会红，会让那条守卫对新册里的匿名常量永远失明）。

/// 往已有链路投递，还是当作**新连接**重新握手（外设侧收到一帧时的唯一判据）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeripheralRouteAction {
    /// 投给该 central 已登记的链路（正常数据帧）。
    ToExistingLink,
    /// 走握手路径（没有链路，或这是**重连**发来的新 Hello）。
    ToHandshake,
}

/// 判定外设侧收到的一帧该投给旧链路还是重新握手。
///
/// ## 为什么必须有这条判据（2026-09-12 真机）
///
/// BLE 上同一个 central 的地址在**重连**时会被复用（macOS 侧是 CoreBluetooth 给同一台
/// 手机分配的 UUID，Android 侧是同一个 MAC）。旧连接的链路任务可能还没被清理，
/// 于是新连接发来的 **Hello 会被投给旧链路的管道**：
///   · 旧链路的写句柄指向**旧连接** ⇒ 新连接永远收不到 Hello 回应
///     ⇒ 对端报「握手超时：对端未回 Hello」；
///   · 旧链路把这条 Hello 当普通帧消费掉 ⇒ 对端报「对端首帧不是 Hello」。
/// 两种报错在用户侧都是"蓝牙时好时坏、加好友没反应"。
///
/// 所以：**有活路由 + 收到 Hello ⇒ 一定是重连**，必须换路由并重新握手。
/// 其余情况（普通数据帧、或本来就没有路由）都按原来的投递/握手走。
///
/// 抽成纯函数的理由同 `should_dial_ble`：这类判据写反了在真机上极难复现，
/// 而这里可以把它一次钉死，并让护栏在有人改成"永远投旧链路"时立刻 FAIL。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn peripheral_route_action(has_route: bool, frame_is_hello: bool) -> PeripheralRouteAction {
    // 没有活路由 ⇒ 只能握手；有路由且这帧是 Hello ⇒ 一定是重连 ⇒ 也必须握手。
    // 只有「有路由 + 不是 Hello」才是正常的"在已有链路上收数据"。
    if !has_route || frame_is_hello {
        PeripheralRouteAction::ToHandshake
    } else {
        PeripheralRouteAction::ToExistingLink
    }
}

/// 外设事件循环收到的路由控制消息（握手成功后把"往这个 central 投帧"的管道交给循环）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
enum RouteCtl {
    Add {
        central: String,
        tx: mpsc::Sender<Vec<u8>>,
    },
    /// 握手**失败**收尾 ⇒ 从 `handshaking` 里摘掉这个 central，允许它再次触发握手。
    ///
    /// 为什么必须有（2026-09-13 审计抓到的"加入不了 mesh"缺陷）：旧实现只在
    /// `Add`（握手成功）与 `Unlinked`（对端退订）时清理 `handshaking`，
    /// 而**握手失败**（对端根本不是 Gosslan 端、Hello 验签不过、首帧异常…）时**不清理**
    /// ⇒ 那个 central 之后发来的**真 Hello 会被「已在握手」静默丢弃** ⇒ 设备再也进不来。
    /// macOS 外设没有断连回调（`Unlinked` 不一定到），这个条目可能**永久残留**。
    ///
    /// ⚠️ 只在失败时发：成功路径由 `Add` 清理；若成功也发，会与"刚起来的第二次握手"
    /// 抢同一个标记（把新握手的 `handshaking` 误清 ⇒ 同一 central 叠起多条握手）。
    HandshakeFailed { central: String },
}

/// 把外设事件队列里**已经入队**的 `Notice`/`Warning` 逐条落日志。
///
/// 为什么需要它：外设角色**为什么起不来**（权限缺失 / 蓝牙没开 / 本机不支持广播 /
/// GATT server 打不开……）是 Kotlin / CoreBluetooth 侧经 `Notice`/`Warning` 事件上报的，
/// 它们落在 `server.events` 这条队列里。而这条队列**唯一**的消费点是外设接收循环
/// （`peripheral_accept_loop` 里的 `server.events.recv()`），启动失败时那个循环根本不会起来
/// —— 直接 `stop()` 会把队列连同原因一起丢掉，用户最终只看到一句自指的
/// 「Android BLE 外设未能启动（详见日志中的具体原因）」，而日志里并没有那个原因。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn drain_peripheral_events(server: &mut peripheral::PeripheralServer, state: &AppState) {
    while let Ok(ev) = server.events.try_recv() {
        match ev {
            PeripheralEvent::Warning(text) => state.logger.warn("ble", text),
            PeripheralEvent::Notice(text) => state.logger.info("ble", text),
            // 启动都没成功，不可能有帧/断链事件；真出现也只说明状态机不对，不值得为它编文案
            PeripheralEvent::Frame { .. } | PeripheralEvent::Unlinked { .. } => {}
        }
    }
}

/// 停外设并把队列排空落日志。
///
/// ⚠️ `stop()` **前后各排一次**，缺一不可：原因来自两处 ——
/// 启动期间上报的 `Notice`/`Warning`（先入队），以及 `stop()` 自身失败时塞进来的
/// Warning（见 `PeripheralServer::stop`：停不掉意味着可能还在广播，必须留痕）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn stop_peripheral_and_drain(server: &mut peripheral::PeripheralServer, state: &AppState) {
    drain_peripheral_events(server, state);
    server.stop();
    drain_peripheral_events(server, state);
}

/// 启动外设角色。失败只记日志：能扫别人但别人连不上我们，属于**降级**而不是故障，
/// 不该把整个蓝牙开关判为不可用（LAN 更不受影响）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn start_peripheral(state: Arc<AppState>, shutdown: watch::Receiver<bool>) {
    let mut startup = match peripheral::start() {
        Ok(startup) => startup,
        Err(e) => {
            state.logger.warn(
                "ble",
                format!("蓝牙外设角色未启动（central 角色不受影响，仍可主动连别人）：{e}"),
            );
            return;
        }
    };
    // 等 CoreBluetooth 上报状态：把"未授权 / 蓝牙关着 / 广播失败"变成一条**说得清**的错误。
    // 超时不致命（系统可能只是还没上报），此时按"已启动"继续。
    match tokio::time::timeout(peripheral::STATE_WAIT, startup.state).await {
        Ok(Ok(Ok(()))) => state.logger.info(
            "ble",
            "蓝牙外设角色已启动（广播服务 UUID，等待手机/PC 连入）",
        ),
        Ok(Ok(Err(e))) => {
            state.logger.warn(
                "ble",
                format!("蓝牙外设角色不可用（central 角色不受影响）：{e}"),
            );
            stop_peripheral_and_drain(&mut startup.server, &state);
            return;
        }
        Ok(Err(_)) => {
            state
                .logger
                .warn("ble", "蓝牙外设角色的状态回调通道被关闭，放弃启动");
            stop_peripheral_and_drain(&mut startup.server, &state);
            return;
        }
        Err(_) => state.logger.info(
            "ble",
            "蓝牙外设角色已启动（未在 3s 内收到状态回调，继续广播）",
        ),
    }
    tokio::spawn(peripheral_accept_loop(state, startup.server, shutdown));
}

/// 外设侧的总循环：把每个 central 的帧分派给它的链路任务，首帧走握手。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn peripheral_accept_loop(
    state: Arc<AppState>,
    mut server: peripheral::PeripheralServer,
    mut shutdown: watch::Receiver<bool>,
) {
    // central 标识 → 该链路的帧管道；`handshaking` 防止同一个 central 触发多次握手
    let mut routes: HashMap<String, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut handshaking: HashSet<String> = HashSet::new();
    let (route_tx, mut route_rx) = mpsc::channel::<RouteCtl>(16);

    loop {
        let ev = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            Some(ctl) = route_rx.recv() => {
                match ctl {
                    RouteCtl::Add { central, tx } => {
                        handshaking.remove(&central);
                        routes.insert(central, tx);
                    }
                    // 握手失败 ⇒ 解除"握手中"标记（否则这个 central 的真 Hello 永远被丢）
                    RouteCtl::HandshakeFailed { central } => {
                        handshaking.remove(&central);
                    }
                }
                continue;
            }
            maybe = server.events.recv() => match maybe {
                Some(ev) => ev,
                None => break,
            },
        };

        match ev {
            PeripheralEvent::Frame { central, bytes } => {
                // 先判「投旧链路 还是 重新握手」（判据与理由见 `peripheral_route_action`）：
                // 同一个 central 地址的**重连**发来的 Hello 绝不能被投给旧链路的管道,
                // 否则新连接永远收不到 Hello 回应（对端表现为"握手超时"/"首帧不是 Hello"）。
                let has_route = routes.contains_key(&central);
                let action =
                    peripheral_route_action(has_route, has_route && frame_is_hello(&bytes));
                let mut pending = Some(bytes);
                if action == PeripheralRouteAction::ToHandshake && has_route {
                    routes.remove(&central);
                    state.logger.info(
                        "ble",
                        format!("外设侧收到新连接的 Hello（central={central}）⇒ 换路由并重新握手"),
                    );
                }
                if action == PeripheralRouteAction::ToExistingLink {
                    if let Some(tx) = routes.get(&central).cloned() {
                        match tx.send(pending.take().expect("pending 刚被设置")).await {
                            Ok(()) => continue,
                            Err(e) => {
                                pending = Some(e.0);
                                routes.remove(&central);
                                state.logger.info(
                                    "ble",
                                    format!("外设侧旧链路已失效，按重连处理 central={central}"),
                                );
                            }
                        }
                    }
                }
                let bytes = pending.expect("未投递的帧必须还在");
                // 外设侧的同一类问题：新连接的"第一帧"可能仍是上一条链路的残留业务帧
                // （Android 的 notify 按 central 地址投递）。这里**丢掉**它并等 Hello ——
                // 既不能投给旧路由（那条链路已死），也不能当握手首帧（会立刻失败）。
                if action == PeripheralRouteAction::ToHandshake && !frame_is_hello(&bytes) {
                    let kind = serde_json::from_slice::<Message>(&bytes)
                        .map(|m| m.wire_kind())
                        .unwrap_or_else(|_| "无法解析".to_string());
                    state.logger.info(
                        "ble",
                        format!(
                            "[SESSION] 丢弃外设侧握手前导帧 type={kind} central={central}（等 Hello）"
                        ),
                    );
                    continue;
                }
                if handshaking.insert(central.clone()) {
                    let st = state.clone();
                    let wrt = server.writer.clone();
                    let ctl_tx = route_tx.clone();
                    let failed_central = central.clone();
                    // ⚠️ `shutdown` 必须在**进入 async move 之前**克隆：它是循环外的
                    // `watch::Receiver`，循环顶部的 `select!` 每轮都要用；
                    // 让 `async move` 直接捕获它会把它搬出循环（E0382）。
                    let sd = shutdown.clone();
                    tokio::spawn(async move {
                        let ok =
                            accept_handshake(st, wrt, central, bytes, ctl_tx.clone(), sd).await;
                        // ⚠️ **只在失败时**解除"握手中"标记（成功路径由 `RouteCtl::Add` 解除）。
                        // 失败不解标记 ⇒ 这个 central 的真 Hello 永远被丢 ⇒ 设备再也加入不进来
                        // （macOS 外设没有断连回调，条目可能永久残留）。
                        if !ok {
                            let _ = ctl_tx
                                .send(RouteCtl::HandshakeFailed {
                                    central: failed_central,
                                })
                                .await;
                        }
                    });
                } else {
                    // 2026-09-23 审计 B3：该 central 已在握手中，这帧 Hello 被丢。
                    // 旧实现静默掉出 —— 重连方在 10s 握手窗口内报"握手超时"时，
                    // 这条输入路径在日志里完全不可见，无从排查。
                    state.logger.info(
                        "ble",
                        format!(
                            "[SESSION] 丢弃握手中 central 的重复 Hello central={central}（上一次握手仍在进行）"
                        ),
                    );
                }
            }
            PeripheralEvent::Unlinked { central } => {
                // ⚠️ 真机 2026-09-13 第五轮：**"取消订阅"不能当成"断开"立刻摘链路**。
                //
                // 现象（用户三台设备：手机 + Mac + Windows，Android 侧日志）：
                //   `外设侧对端取消订阅（视为断开）central=34:13:E8:90:51:B3`
                //   每 1~2 秒一条，**14 分钟刷了几百次** —— 而那个地址是 Mac。
                // 旧行为：每一条都立刻 `handshaking.remove` + `routes.remove` +
                // `detach_by_endpoint` ⇒ 事件循环被这条洪水灌满，**同一时刻正在握手的
                // 另一台设备（Windows，17:50:16 已连上、MTU=517 都协商完了）被挤掉**，
                // 永远走不到 `[SESSION] 已就绪（外设侧）`。用户看到的仍是"互相搜不到"。
                //
                // GATT 语义本来就允许"取消订阅"与"断开"是两件事（两者都会走这个回调），
                // 所以这里按**有没有待给的链路**分流：
                //   · 没有已登记链路（还在握手 / 刚连上）⇒ 只清握手标记、**绝不动路由**，
                //     那条链路让握手自己去完成或超时收尾；
                //   · 已有链路 ⇒ 才是真的断开，按原逻辑摘掉。
                let established = {
                    let links = state.links.lock().await;
                    links.values().flatten().any(|l| {
                        l.path_kind == PathKind::Bluetooth
                            && matches!(
                                &l.endpoint,
                                MeshEndpoint::Ble(b)
                                    if b.address.eq_ignore_ascii_case(&central)
                            )
                    })
                };
                handshaking.remove(&central);
                if !established {
                    // 只留痕、**不摘路由**：拒绝把"握手中的链路"误伤掉
                    state.logger.info(
                        "ble",
                        format!(
                            "外设侧取消订阅 central={central}（尚无已登记链路 ⇒ 视为重连前的噪声，保留路由与握手）"
                        ),
                    );
                    continue;
                }
                routes.remove(&central);
                state
                    .logger
                    .info("ble", format!("外设侧已登记链路断开 central={central}"));
                let ep = MeshEndpoint::Ble(BleEndpoint::new(central));
                detach_by_endpoint(&state, &ep, "peripheral_unlinked").await;
            }
            // 驱动侧的诊断/告警：**必须**记进日志 —— 蓝牙在真机上出问题时，
            // 这是用户唯一能贴给我们的线索（"开着蓝牙却没人能发现我们"就是这类）。
            PeripheralEvent::Notice(text) => state.logger.info("ble", text),
            PeripheralEvent::Warning(text) => state.logger.warn("ble", text),
        }
    }

    server.stop();
    state.logger.info("ble", "蓝牙外设角色已停止广播");
}

/// 按 BLE 端点摘链路（外设侧只知道 central 标识，peer_id 要反查）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn detach_by_endpoint(state: &Arc<AppState>, ep: &MeshEndpoint, reason: &'static str) {
    let peer = {
        let links = state.links.lock().await;
        links
            .iter()
            .find(|(_, v)| v.iter().any(|l| &l.endpoint == ep))
            .map(|(p, _)| p.clone())
    };
    if let Some(peer) = peer {
        teardown_link(state, &peer, ep, reason).await;
    }
}

/// 外设侧握手的外壳：失败一律**只记日志**（对端可能只是路过、或者根本不是 Gosslan 端）。
///
/// 返回值 = 是否**真的建链成功**（`try_accept_handshake` 已发出 `RouteCtl::Add`）。
/// 调用方据此决定要不要解除 `handshaking` 标记 —— 见 `RouteCtl::HandshakeFailed` 的注释。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn accept_handshake(
    state: Arc<AppState>,
    writer: PeripheralWriter,
    central: String,
    first_bytes: Vec<u8>,
    route_tx: mpsc::Sender<RouteCtl>,
    shutdown: watch::Receiver<bool>,
) -> bool {
    match try_accept_handshake(&state, writer, &central, first_bytes, route_tx, shutdown).await {
        Ok(()) => true,
        Err(e) => {
            state
                .logger
                .info("ble", format!("外设侧未建链 central={central}：{e}"));
            false
        }
    }
}

/// 真身：验签对端 Hello → 回我们的 Hello → 登记链路 → 起收发。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
async fn try_accept_handshake(
    state: &Arc<AppState>,
    writer: PeripheralWriter,
    central: &str,
    first_bytes: Vec<u8>,
    route_tx: mpsc::Sender<RouteCtl>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ep = MeshEndpoint::Ble(BleEndpoint::new(central.to_string()));

    // **外设侧的 MTU 同样必须留痕**（2026-09-13 审计）：它决定"我们发通知时每片能塞多少字节"，
    // 与 central 侧的写方向是两个独立的值（对端可能协商出不同结果）。
    // 真机"手机→电脑传得慢/传不完"时，第一件事就是比这两条日志。
    let mtu_budget = writer.payload_mtu(central);
    let (net_bytes, kbps) = ble_throughput_estimate(mtu_budget);
    state.logger.info(
        "ble",
        format!(
            "[GATT] 外设侧 MTU 协商结果 central={central} 每片有效载荷={mtu_budget} 字节（净数据={net_bytes}；按 12ms/片估算 ≈ {kbps:.1} KB/s）"
        ),
    );

    // ---- 1. 首帧必须是 Hello，且签名必须验过（BLE 地址不是身份）----
    let first: Message =
        serde_json::from_slice(&first_bytes).map_err(|e| format!("对端首帧无法解析：{e}"))?;
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
        return Err(format!(
            "外设侧首帧不是 Hello（收到 {}，central={central}）",
            first.wire_kind()
        ));
    };
    crate::network::transport::verify_hello_for_ble(
        state,
        device_id,
        *tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig,
    )?;
    let peer_id = device_id.clone();

    // ---- 2. 同一个 BLE 端点的旧链路让位 ----
    // CoreBluetooth 的外设角色**没有**"central 断开"回调（只有取消订阅），
    // 所以旧链路可能早就死了而我们还留着它；对端重新连上来时必须由新链路取代，
    // 否则这个 central 会永远撞在 `should_accept_inbound_public` 上、彻底连不进来。
    detach_by_endpoint(state, &ep, "new_connection_displace").await;

    // ---- 3. 链路数上限仍然要守（防无界增长），但**同路径的镜像链路要放行** ----
    //
    // 为什么不能像 TCP 那样"按同路径去重直接拒"（4.2.6 的教训，用户真机日志）：
    // BLE 上两端都跑 central+peripheral，小 id 那一侧（Mac）会不停来拨我们；
    // 如果我们**在回 Hello 之前**就拒掉，它就**永远学不到对端 device_id** ⇒
    // 也就永远进不了它自己的"不要再拨"名单 ⇒ 每 13s 重拨一次，
    // 而**每次连接都会打断我们拨过去的那条好链路**（Android GATT server 对同一 central
    // 的新连接会替换旧的）⇒ 好链路 45s 收不到帧被看门狗拆掉 ⇒ 加好友时"连接已关闭"。
    // 所以：**让它握手成功**，它拿到 device_id 后会自己判"我比你小 ⇒ 该你拨我"并把这条
    // 镜像链路收掉（`dial_and_register` 里的 `should_dial_ble`）。一次性打扰，换来永久安静。
    let existing = link_snapshot(state, &peer_id).await;
    if !should_accept_inbound_public(&state.device_id, &peer_id, PathKind::Bluetooth, &existing)
        && existing.len() >= crate::network::transport::MAX_LINKS_PER_PEER
    {
        return Err("该 peer 链路数已满，不重复建链".to_string());
    }
    if existing
        .iter()
        .any(|(_, k, healthy)| *k == PathKind::Bluetooth && *healthy)
    {
        state.logger.info(
            "ble",
            format!("对端 {peer_id} 已有蓝牙链路，这条是镜像入站 —— 仍然完成握手，好让它自己退让"),
        );
    }

    // ---- 4. 路由先就位，再回 Hello ----
    // 对端收到我们的 Hello 后会**立刻**开始冲刷待发队列；路由早一步挂上，
    // 那批帧才不会被"注册还没完成"的缝隙吞掉（通道有缓冲，读者随后就来）。
    let (frame_tx, frame_rx) = mpsc::channel::<Vec<u8>>(1024);
    route_tx
        .send(RouteCtl::Add {
            central: central.to_string(),
            tx: frame_tx,
        })
        .await
        .map_err(|_| "外设事件循环已退出".to_string())?;

    // ---- 5. 回我们的 Hello（对端正卡在 10s 超时里等它）----
    if shutdown.borrow().to_owned() {
        return Err("蓝牙通道正在停止".to_string());
    }
    let hello = build_signed_hello(state, 0);
    let bytes = serde_json::to_vec(&hello).map_err(|e| format!("Hello 序列化失败：{e}"))?;
    writer
        .send_frame(central, &bytes)
        .await
        .map_err(|e| format!("回 Hello 失败：{e}"))?;

    // ---- 6. 登记链路（端点 = BLE central 标识，路径 = Bluetooth）----
    let ((high_tx, high_rx), (normal_tx, normal_rx), (low_tx, low_rx)) =
        crate::network::transport::link_channels(crate::network::file::BLE_FILE_CHUNK);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state
        .links
        .lock()
        .await
        .entry(peer_id.clone())
        .or_default()
        .push(Link {
            endpoint: ep.clone(),
            path_kind: PathKind::Bluetooth,
            high: high_tx.clone(),
            normal: normal_tx.clone(),
            low: low_tx.clone(),
            cancel: cancel_tx,
        });
    register_connection(state, &peer_id, ep.clone(), PathKind::Bluetooth);
    crate::network::transport::replay_group_frames_to(state, &peer_id); // #77：蓝牙入站
                                                                        // 对端能连上我们 ⇒ 之前"我拨不上它"的失败计数已经过期，必须清掉。
                                                                        // 不清的话：唯一的拨号方（大 id/不能被拨入的那侧）会被自己的退避锁住，
                                                                        // 而它恰恰是断线后唯一会重连的一方（真机表现：好友申请等几分钟）。
    clear_ble_dial_failure(state, central);
    state.logger.info(
        "ble",
        format!("[SESSION] 已就绪（外设侧）peer={peer_id} ep={ep}（双向 Hello 已验签）"),
    );
    state.logger.info(
        "ble",
        format!("+ble-link(外设) peer={peer_id} ep={ep}（双向 Hello 已验签）"),
    );

    // 对端 Hello 交给统一处理路径：写身份 + 双公钥、对齐会话时钟、冲刷待发队列
    handle_message(state, &peer_id, first).await;

    tokio::spawn(ble_writer_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        PeripheralSink {
            writer,
            central: central.to_string(),
        },
        high_rx,
        normal_rx,
        low_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));
    tokio::spawn(ble_reader_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        ChannelSource { rx: frame_rx },
        shutdown,
        cancel_rx,
    ));

    // 建链即冲一次待发队列（与 central / TCP 拨号成功后的序列完全一致）
    flush_outbox(state, &peer_id).await;
    flush_group_outbox(state, &peer_id).await;
    flush_pending_reads(state, &peer_id).await;
    flush_pending_group_reads(state, &peer_id).await;
    flush_pending_group_keys(state, &peer_id).await;
    crate::commands::flush_pending_files(state, &peer_id).await;
    crate::commands::flush_pending_group_files(state, &peer_id).await;
    Ok(())
}
