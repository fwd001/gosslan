// BLE **central 角色**那一半：扫描 → 判该不该拨 → 拨号 → 握手验签 → 登记链路。
// 
// 为什么单独一册：这一册的失败模式全是"连不上/连错/连上又拆"（真机 2026-09-12 那一批都在这里），
// 而外设角色的失败模式是"广播没起来、握手被投给旧链路"。两半各自要读的不变量不重叠。
// 
// 恒等判据（与 transport / file 两刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 本模块整体在 `#[cfg(feature = "bluetooth")]` 后面（`network/mod.rs:10`）⇒ 这些册跟着根文件一起门控，
//   不需要各自再写 cfg。搬家同批必须做的三件事：`network/mod.rs::ble_src_for_guards()` 登记本册、
//   `docs/domains.data.mjs` 认领、`scripts/check-ble-constants.mjs` 的 BLE_DOMAIN_FILES 覆盖本册
//   （那份名单是**硬编码文件清单** —— 漏了不会红，会让那条守卫对新册里的匿名常量永远失明）。

/// **让扫描循环立刻扫一轮**（用户打开「添加好友」/点「扫描」时调用）。
///
/// 返回 `true` = 已通知到（通道在跑）；`false` = 蓝牙通道没开，什么也没做。
///
/// 与 LAN 的 `search_nearby_peers` 同一个思路：**周期扫描负责"保持发现"，
/// 用户动作负责"立刻发现"** —— 后者才是用户感知到"快"的地方。
///
/// 与 [`wake_scan`] 的分工（两条链路的语义不同，不要合并）：
///   · 这一个 = **用户主动触发** ⇒ 立刻扫，并给拨号退避打折（`USER_TRIGGER_BACKOFF_FACTOR`）；
///   · `wake_scan` = **内部信号**（断链立刻重拨 / 从后台切回前台）⇒ 立刻扫，但不打折。
pub fn trigger_scan_now(state: &Arc<AppState>) -> bool {
    let slot = state.ble_scan_now.lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_ref() {
        Some(tx) => {
            let next = tx.borrow().wrapping_add(1);
            let _ = tx.send(next);
            state
                .logger
                .info("ble", "[DISCOVERY] 用户触发：立刻再扫一轮 BLE");
            true
        }
        None => false,
    }
}

/// 请求扫描循环**立刻扫一轮**（从后台切回前台 / 断链后立刻重拨时调用）。
///
/// 用 `Notify` 而不是重启循环：不打断正在进行的扫描，只是把"下一轮"的等待清零 ——
/// 否则后台节奏下用户点开「添加好友」最多要干等 30s，体感就是"搜不到人"。
pub fn wake_scan(state: &AppState) {
    state.ble_wake.notify_one();
}

async fn scan_loop(
    state: Arc<AppState>,
    adapter: Adapter,
    mut shutdown: watch::Receiver<bool>,
    mut scan_now: watch::Receiver<u64>,
) {
    // 第一轮**不要等**：用户打开「添加好友」/刚开蓝牙时，最不想等的就是那 2 秒
    let mut skip_initial_wait = true;
    // 本轮是不是用户主动触发的（决定拨号退避要不要打折，见下）
    let mut user_triggered = false;
    loop {
        match driver::scan_peers(&adapter, SCAN_WINDOW).await {
            Ok((peers, total)) => {
                // 每次扫描都要留痕（**包括 0 个**）：真机上这两个数字是排查的关键 ——
                // "收到 0 个广播"= 扫描/权限/硬件问题；"收到 N 个但 0 个是本服务"= 对端没在广播
                // 或广播里没有我们的服务 UUID。用户 2026-09-12 的"互相搜不到"当时日志里
                // 什么都没有，只能靠猜。
                state.logger.info(
                    "ble",
                    format!(
                        "BLE 扫描：收到 {total} 个广播，其中 {} 个是本应用服务{}",
                        peers.len(),
                        if user_triggered {
                            "（用户主动触发）"
                        } else {
                            ""
                        }
                    ),
                );
                // 记进诊断状态（面板要在不重新扫的情况下知道最近一轮看到了什么）
                *state.ble_scan.lock().unwrap_or_else(|e| e.into_inner()) =
                    crate::state::BleScanStats {
                        last_ts: crate::db::now_ms(),
                        total: total as u32,
                        matched: peers.len() as u32,
                    };
                for peripheral in peers {
                    if *shutdown.borrow() {
                        return;
                    }
                    // **扫到 = 对端在广播**（这条扫描结果已经把"服务 UUID 对得上"过滤过了，
                    // 见 `driver::scan_peers`）。记下来供 `should_dial_ble` 判断：
                    // 对端不广播时必须由我们无条件拨（只做 central 的平台唯一能建链的方式）。
                    let adv_id = peripheral.id().to_string();
                    state
                        .ble_peer_advertises
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(adv_id);
                    // 对端是这条链路上的**指定拨号方**（它比我大）⇒ 别去拨它：
                    // 我们拨过去只会被它按镜像规则拒掉，而每次连接都会打断它拨过来的那条
                    // 好链路（真机症状：45s 收不到帧 → 看门狗拆链 → "加好友时连接已关闭"）。
                    if state
                        .ble_no_dial
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .contains(&peripheral.id().to_string())
                    {
                        continue;
                    }
                    // 失败退避：刚连不上的候选先别急着再试（指数退避 5s→60s，见
                    // `ble_dial_backoff_ms` 的注释：旧上限 10 分钟会把暂时性失败变成
                    // 用户可见的"好友申请等了几分钟"）。
                    //
                    // ⚠️ 跳过时**必须留痕**（含剩余毫秒）：否则真机上只能看到
                    // "候选 X 未建立链路"，完全看不出"其实是被退避锁住了"。
                    {
                        let now = crate::db::now_ms();
                        let skip = ble_dial_backoff(&state, &peripheral.id().to_string(), now);
                        if skip {
                            // 用户主动触发 ⇒ 退避窗口打折。理由是实测体感：
                            // 用户点了「扫描」，而退避还剩 40s ⇒ **什么都不发生**，
                            // 只能被理解成"搜不到"。打折后仍留一点间隔，不连打。
                            let raw_left = state
                                .ble_dial_failures
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .get(&peripheral.id().to_string())
                                .map(|(_, next)| (*next - now).max(0))
                                .unwrap_or(0);
                            let left = if user_triggered {
                                raw_left / USER_TRIGGER_BACKOFF_FACTOR
                            } else {
                                raw_left
                            };
                            if left > 0 {
                                state.logger.info(
                                    "ble",
                                    format!(
                                        "[DISCOVERY] 跳过候选 id={} 原因=退避中 剩余={left}ms{}",
                                        peripheral.id(),
                                        if user_triggered {
                                            "（已按用户触发打折）"
                                        } else {
                                            ""
                                        }
                                    ),
                                );
                                continue;
                            }
                            // 打折后已经可以试了：把退避记录清掉，让下面的拨号正常进行
                            clear_ble_dial_failure(&state, &peripheral.id().to_string());
                        }
                    }
                    let st = state.clone();
                    let sd = shutdown.clone();
                    let dial_id = peripheral.id().to_string();
                    // ⚠️ 「已建链」这一判定必须在**打「开始连接」之前**做。
                    // 原先只有 `dial_and_register` 里那条静默跳过，于是日志每轮都写一句
                    // 「候选可拨 ⇒ 开始连接（GATT central）」，后面却什么都没有 ——
                    // 真机日志里连着 18 轮这么写，读起来像"应用在反复拨一个已经连上的对端"，
                    // 而真相是这一轮**什么都没发生**（用户 2026-09-16 排查时被它带偏）。
                    // 放在这里还顺带省掉了下面读广播属性的那次 await。
                    if state
                        .has_endpoint_addr(&MeshEndpoint::Ble(BleEndpoint::new(dial_id.clone())))
                        .await
                    {
                        // ⚠️ 这里必须**补上**原来由 `dial_and_register` 的 `Ok(())` 分支
                        // 顺带做掉的那件事：清掉该地址的拨号退避。否则"已建链"的地址会一直
                        // 留着一条过期退避，等这条链路断掉、下一轮扫描要重拨时被它拖住
                        // （最长 20s）—— 用户看到的是"刚断开却半天连不回来"。
                        clear_ble_dial_failure(&state, &dial_id);
                        state.logger.info(
                            "ble",
                            format!("[DISCOVERY] 跳过候选 id={dial_id} 原因=已建链（不重复拨号）"),
                        );
                        continue;
                    }
                    // 把**广播里能拿到的事实**一起打出来（真机 2026-09-13 第二轮补）：
                    // 之前只打地址，于是"信号多强、是不是随机地址、对端有没有报名字、
                    // 它自报的服务列表是什么"这些一眼能定性的信息全丢了，
                    // 只剩一句 `Not connected` 无从判断。
                    // 这些字段都在 `PeripheralProperties` 里（btleplug 从广播/扫描响应解析）。
                    //
                    // ⚠️ 2026-09-13 第七轮：把**命中的那个服务 UUID**也打出来。
                    // 真机上出现过"Windows 搜得到安卓和 Mac，但安卓的扫描里只有 1 个本应用服务"
                    // —— 到底是对端没广播、还是广播里带的是**另一个** UUID（旧版本 / 另一份构建），
                    // 只有把 UUID 打出来才能区分。这是"三方都能扫到、偏偏有一方扫不到"的决定性证据。
                    let facts = match peripheral.properties().await {
                        Ok(Some(p)) => {
                            // 直接现算 UUID（`driver::uuid` 是私有的，不为了这条日志去放开它）
                            let svc = uuid::Uuid::parse_str(SERVICE_UUID)
                                .expect("BLE 服务 UUID 常量必须合法");
                            let hit = p
                                .services
                                .iter()
                                .find(|u| **u == svc)
                                .map(|u| u.to_string())
                                .unwrap_or_else(|| "(未在本设备广播里看到我们的 UUID)".to_string());
                            format!(
                                "rssi={:?} 地址类型={:?} 名字={:?} 广播服务数={} 命中={hit} 发射功率={:?}",
                                p.rssi,
                                p.address_type,
                                p.local_name.as_deref().or(p.advertisement_name.as_deref()),
                                p.services.len(),
                                p.tx_power_level
                            )
                        }
                        Ok(None) => "广播属性暂不可用".to_string(),
                        Err(e) => format!("读广播属性失败：{e}"),
                    };
                    state.logger.info(
                        "ble",
                        format!(
                            "[DISCOVERY] 候选可拨 id={dial_id} ⇒ 开始连接（GATT central）｜{facts}"
                        ),
                    );
                    // 每个候选一个任务：连接 + 握手最长 10s，串行会把扫描周期拖垮
                    tokio::spawn(async move {
                        let id = peripheral.id().to_string();
                        match dial_and_register(st.clone(), peripheral, sd).await {
                            // 成功 ⇒ 清掉这个候选的失败计数（下次断了还能正常重拨）
                            Ok(()) => clear_ble_dial_failure(&st, &id),
                            // 连接失败是常态（对方正在忙、走远了、不是 Gosslan 端），
                            // 记 info 不记 warn —— 否则日志会被邻居设备刷满
                            Err(e) => {
                                note_ble_dial_failure(&st, &id);
                                st.logger.info(
                                    "ble",
                                    format!("[DISCONNECT] 候选 {id} 未建立链路：{e}（已进入退避）"),
                                );
                            }
                        }
                    });
                }
            }
            Err(e) => state.logger.warn("ble", format!("扫描失败：{e}")),
        }
        // 本轮结束：决定等多久再开下一轮。
        // 节奏由**应用是否在前台/聚焦**决定（用户 2026-09-13 的功耗策略）：前台 5s、后台 30s。
        // 两个"立刻扫"的入口都要认（语义不同，见 helper 的注释）：
        //   · `ble_scan_now` = 用户主动触发（打开「添加好友」/点扫描）⇒ 立刻扫 + 退避打折；
        //   · `ble_wake`     = 内部信号（断链立刻重拨 / 从后台切回前台）⇒ 立刻扫，不打折。
        user_triggered = false;
        if skip_initial_wait {
            skip_initial_wait = false;
            continue;
        }
        let active = state.app_active.load(std::sync::atomic::Ordering::Relaxed);
        let interval = scan_interval(active);
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            // 用户主动触发：收到新值 ⇒ 跳过等待，马上扫一轮（并给退避打折）
            res = scan_now.changed() => {
                if res.is_err() {
                    // 发送端被丢掉（通道停止）：继续按周期跑，不要退出扫描循环
                } else {
                    user_triggered = true;
                }
            }
            // 内部唤醒：断链立刻重拨 / 从后台切回前台 ⇒ 也立刻扫，但**不打折**退避
            _ = state.ble_wake.notified() => {
                state.logger.info("ble", "[SCAN] 收到唤醒信号 ⇒ 立刻扫描一轮");
            }
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

/// BLE 候选失败退避的**纯函数内核**：连续失败 `failures` 次后，要等多久才允许再试。
///
/// ## 为什么几乎不退了（2026-09-13 第三轮真机）
///
/// 真机日志暴露出退避**把重试饿死了**：扫描周期已经是 2s，而退避是 5s→10s→20s→40s，
/// 于是日志里一半的行是 `跳过候选 … 原因=退避中 剩余=7849ms` ——
/// 用户看到的"搜不出来"，很大程度上是**我们自己不去连**。
///
/// 关键认识：**退避的初衷（别打扰对端）已经被 `DialGuard` 在途去重实现了** ——
/// 同一个对端不会叠起多条连接（真机 2026-09-13 第一轮就是那个 bug 的教训）。
/// 既然如此，"每轮扫描都试一次"就是安全的：一轮 4s，对端每 4s 被尝试一次，
/// 而 BLE 上连不上的尝试本身是廉价且无副作用的。
///
/// ## 为什么形状是"前几次不退 + 缓增到 20s"（合并评审 2026-09-13）
///
/// 一开始写成"前 3 次不退、之后**固定 5s**"。评审指出：那是**激进**的那一端 ——
/// 失败很多次（对端长期不在、或根本不是 Gosslan 端）时会一直每轮都敲。
/// 退避**慢**的代价用户可以忍（多等几秒），但射频/功耗被打**没有用户可见的反馈**，
/// 只会在电量上体现。所以改成缓增并把上限放在 20s：
///
/// | 连续失败 | 1–3 | 4 | 5 | 6 | 7+ |
/// |---|---|---|---|---|---|
/// | 冷却 | **0**（不退） | 5s | 10s | 20s | 20s（封顶） |
///
/// 上限 20s 与扫描周期（约 4s）同量级：最多跳过 5 轮就一定会再试一次，
/// 不会重演"被退避锁到分钟级"那次的故障形态。
fn ble_dial_backoff_ms(failures: u32) -> i64 {
    /// 前几次失败不退避 —— 这是"搜不出来"最直接的解药。
    const FREE_ATTEMPTS: u32 = 3;
    /// 缓增的起点与上限。
    const BASE_MS: i64 = 5_000;
    const MAX_MS: i64 = 20_000;
    if failures <= FREE_ATTEMPTS {
        return 0;
    }
    // 第 4 次 ⇒ 5s，第 5 次 ⇒ 10s，第 6 次 ⇒ 20s，之后封顶
    let step = failures - FREE_ATTEMPTS - 1; // 0,1,2,…
    (BASE_MS << step.min(2)).min(MAX_MS)
}

/// 该候选现在是否处于退避期（true = 跳过）。
fn ble_dial_backoff(state: &Arc<AppState>, id: &str, now: i64) -> bool {
    let map = state
        .ble_dial_failures
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.get(id).is_some_and(|(_, next)| now < *next)
}

/// 记一次失败（连续失败次数 +1，并按指数退避设下次允许时间）。
fn note_ble_dial_failure(state: &Arc<AppState>, id: &str) {
    let mut map = state
        .ble_dial_failures
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = map.entry(id.to_string()).or_insert((0, 0));
    entry.0 = entry.0.saturating_add(1);
    entry.1 = crate::db::now_ms() + ble_dial_backoff_ms(entry.0);
}

/// 连上了就清掉失败计数（下次断了还能正常重拨）。
fn clear_ble_dial_failure(state: &Arc<AppState>, id: &str) {
    state
        .ble_dial_failures
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

/// **BLE 链路谁拨号**（与 TCP 的 `should_dial` 同一条规则：**大 id 拨、小 id 只接受**）。
///
/// 抽成纯函数的理由：它是"两端都跑 central+peripheral 时不互相拨号"的**唯一判据**，
/// 而这类缺陷在真机上表现成"链路时好时坏、点加好友说连接已关闭"（镜像链路互相打断），
/// 极难复现；纯函数可以一次钉死，并让护栏在有人把它改成"总是拨"时立刻 FAIL。
///
/// ## `peer_advertises`：为「只做 central 的平台」留的活口（Windows / ADR-0015 §7-f）
///
/// 「大 id 拨」这条规则**只在两端都能广播时才成立** —— 它的前提是"对方也会拨我"。
/// Windows 这一轮只做 central（不能广播、不能被连），如果照搬这条规则：
/// 只要 Windows 的 id 比手机小，就**没有任何一侧会拨号**，链路永远建不起来。
///
/// 所以判据改成：**只要对端不广播（我们扫不到它的外围广播），就必须由我们拨**；
/// 只有确认对端在广播时才回到 id 比较。这样：
///   · Windows（小 id）↔ 手机（广播）⇒ Windows 无条件拨，链路能建；
///   · Mac ↔ 手机（两侧都广播）⇒ 行为与今天**逐字节一致**（id 比较）。
///
/// 注意：这个参数只影响"要不要主动拨"，**不影响身份** —— 身份永远只由双向 Hello 验签建立。
fn should_dial_ble(my_id: &str, peer_id: &str, peer_advertises: bool) -> bool {
    !peer_advertises || my_id > peer_id
}

/// 判断一帧**是不是 Hello** 的成本上限（字节）：Hello 只有设备 id + 两个公钥 + 签名，
/// 几百字节量级；超过这个长度的帧不可能是握手首帧，直接跳过解析。
///
/// 为什么要设上限：外设侧会对**每一个**到达的帧做这个判断（见
/// `peripheral_accept_loop`），而大文件分片是 256 KiB —— 对它们做一次
/// `serde_json::from_slice::<Message>` 就是白烧一倍解析成本。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
const HELLO_PEEK_MAX_BYTES: usize = 1024;

/// 轻量判断：这帧是不是 `Message::Hello`（用于上面的重连判据）。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "android"))]
fn frame_is_hello(bytes: &[u8]) -> bool {
    bytes.len() <= HELLO_PEEK_MAX_BYTES
        && matches!(
            serde_json::from_slice::<Message>(bytes),
            Ok(Message::Hello { .. })
        )
}

/// 连接一个候选 → 双向 Hello 验签 → 登记链路 → 起收发循环。
async fn dial_and_register(
    state: Arc<AppState>,
    peripheral: btleplug::platform::Peripheral,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ble_id = peripheral.id().to_string();
    // ── 在途去重（与 TCP 同款 `DialGuard`）────────────────────────────────
    // 真机证据（2026-09-13）：扫描每 10s 一轮，而握手最长 10s ⇒ 同一个外设上会**叠起
    // 2~3 个拨号任务**，每个都建一条 CoreBluetooth 连接并各自订阅一次通知流。
    // 而 Android 的 GATT server 对同一地址**只保留最后一条连接**，于是通知很可能被投给
    // "已经没人读的那条" ⇒ Mac 侧一个字节都收不到，而安卓侧 `notify` 全部返回成功。
    let Some(_dial_guard) = crate::state::DialGuard::try_acquire(&state, format!("ble:{ble_id}"))
    else {
        state.logger.info(
            "ble",
            format!("[CONNECT] 跳过 {ble_id}：已有在途拨号（避免在同一对端上叠连接）"),
        );
        return Ok(());
    };
    let ep = MeshEndpoint::Ble(BleEndpoint::new(ble_id.clone()));
    // 这个端点已经连着 ⇒ 跳过（`connect_to_peer` 的同款去重）。
    // 常规路径上扫描侧已经拦掉了（那时的日志是「跳过候选 id=… 原因=已建链」），
    // 这里兜的是"扫描判定完、任务真正跑起来之前"那一小段窗口 —— 概率低但确实会发生，
    // 所以也得留痕，不能像原来那样静默 return（静默正是"日志说开始连接却没了下文"的成因）。
    if state.has_endpoint_addr(&ep).await {
        state.logger.info(
            "ble",
            format!("[CONNECT] 跳过 {ble_id}：判定到拨号之间已建链（不重复拨号）"),
        );
        return Ok(());
    }
    // 上一次失败可能留了一条**已经没人读**的连接：先断开再重连。
    // 不这么做的话，`connect()` 会直接复用那条旧连接，而新订阅的通知流收不到任何东西。
    //
    // 同时把**系统报告的链路状态**记下来（真机 2026-09-13 第二轮需要它）：
    // "Windows 能不能连上手机"这件事有两个完全不同的失败面 ——
    //   · `is_connected=false` 且 connect 一直 Not connected ⇒ 射频层根本没连上
    //     （对端没在监听 / 不在范围 / 系统未授权 / 适配器问题）；
    //   · `is_connected=true` 但服务读不到 ⇒ 连上了、GATT 数据库还没就绪（重试才有意义）。
    // 没有这一行，日志里两者长得一模一样，只能靠猜。
    let before_connected = peripheral.is_connected().await;
    if matches!(before_connected, Ok(true)) {
        state.logger.info(
            "ble",
            format!("[CONNECT] {ble_id} 仍处于已连接状态 ⇒ 先断开，避免复用幽灵连接"),
        );
        let _ = peripheral.disconnect().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let conn = match tokio::time::timeout(BLE_CONNECT_TIMEOUT, driver::connect(&peripheral)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            // 失败时把"系统此刻怎么看这条链路"一并打出来 —— 这是下一轮排查的**唯一**线索
            let after = peripheral
                .is_connected()
                .await
                .map(|v| v.to_string())
                .unwrap_or_else(|err| format!("查询失败({err})"));
            return Err(format!(
                "{e}｜系统链路状态：连接前={:?} 失败后={after}",
                before_connected
            ));
        }
        Err(_) => {
            // 超时（见 BLE_CONNECT_TIMEOUT）：显式断开，DialGuard 随函数返回释放，
            // 该对端随即可被下一轮扫描重新拨号 —— 不再"永远连不上"。
            let _ = peripheral.disconnect().await;
            state.logger.warn(
                "ble",
                format!(
                    "[CONNECT] 连接超时（{}s 内未完成 connect/service discovery）ep={ble_id}",
                    BLE_CONNECT_TIMEOUT.as_secs()
                ),
            );
            return Err(format!(
                "连接超时（{}s 内未完成 connect/service discovery）",
                BLE_CONNECT_TIMEOUT.as_secs()
            ));
        }
    };
    state.logger.info(
        "ble",
        format!("[GATT] 已就绪 ep={ble_id}（连接 + 服务发现 + 通知订阅都成功）"),
    );
    // **每次建链必须打 MTU** —— 排查 BLE 大文件卡死的最关键观测点。
    // WinRT 上 MTU 是异步协商的（connect 返回时可能还是默认 23），
    // 但先记一次 baseline；后续 adapter events / 写循环失败时再交叉验证。
    let mtu = conn.mtu();
    state.logger.info(
        "ble",
        format!(
            "[MTU] ep={ble_id} 协商 MTU={mtu}B （ATT 有效载荷 ≈ {}B）",
            driver::payload_mtu(mtu)
        ),
    );
    // 从这里开始，任何失败都必须**显式断开** —— drop 一个 btleplug `Peripheral`
    // 不会断开 CoreBluetooth 连接，残留会累积成"幽灵连接"（真机症状见上面的注释）。
    match finish_dial(state.clone(), &peripheral, conn, &mut shutdown).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = peripheral.disconnect().await;
            Err(e)
        }
    }
}

/// `dial_and_register` 的握手与登记阶段（拆出来只为让失败路径能统一断开连接）。
async fn finish_dial(
    state: Arc<AppState>,
    peripheral: &btleplug::platform::Peripheral,
    conn: driver::BleConnection,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let ble_id = peripheral.id().to_string();
    let ep = MeshEndpoint::Ble(BleEndpoint::new(ble_id.clone()));
    let (mut writer, mut reader) = conn.into_split();

    // **协商到的 MTU 必须留痕**（2026-09-13 审计）：它是 BLE 吞吐的**唯一**决定因素
    // （每片有效载荷 = MTU-3-6；速率 ≈ 载荷 / 每片间隔）。
    // 此前全仓库没有这一行，于是文档里的"MTU=23 ⇒ 1KB/s"一直是**猜测**，
    // 而代码其实会协商到 182~514 字节载荷（btleplug：macOS `maximumWriteValueLength+3`、
    // Android `requestMtu(517)`）—— 差一个数量级。没有这条日志就没法判断"慢"到底慢在哪。
    // 注意：这里的"每片有效载荷"是**含 6 字节分片头**的 ATT 预算；真正上去的数据是
    // 预算减 6。以前括号里写"MTU=载荷+3+6"是错的（多了 6），会让真机排查算错一个量级。
    let mtu_budget = writer.payload_mtu();
    let (net_bytes, kbps) = ble_throughput_estimate(mtu_budget);
    state.logger.info(
        "ble",
        format!(
            "[GATT] MTU 协商结果 ep={ble_id} 每片有效载荷={mtu_budget} 字节（净数据={net_bytes}，分片头 6；按 12ms/片估算 ≈ {kbps:.1} KB/s）"
        ),
    );

    // ---- 握手：先发自己的 Hello，再读对端的、并**必须验签**（§8 / ADR-0011）----
    // BLE 地址不是身份，所以这里身份一定是"未知"（conv_clock 传 0 即可：
    // 对端 observe_clock 取 max，不会因此倒退）。
    let hello = build_signed_hello(&state, 0);
    let bytes = serde_json::to_vec(&hello).map_err(|e| format!("Hello 序列化失败：{e}"))?;
    writer.send_frame(&bytes).await?;

    // ⚠️ **允许跳过握手前导帧**（2026-09-13 真机抓到的真因）：
    //    日志里反复出现 `对端首帧不是 Hello（收到 chat_message）` ⇒ 链路**永久建不起来**。
    //    原因是 Android 的 notify 按**central 地址**投递：上一条链路的待发帧（outbox flush）
    //    会落在**新连接**上，于是新连接的"第一帧"是先前的业务帧，而不是 Hello。
    //    旧行为直接放弃 ⇒ 双方各自重拨、互相打断，好友申请/消息全部过期。
    //    新行为：窗口内继续读，丢掉非 Hello 的前导帧（**不处理**——身份还没验签），
    //    读到 Hello 就正常握手；窗口耗尽仍只报错（并说明收到了什么）。
    let first = read_hello_frame(
        &mut reader,
        HANDSHAKE_TIMEOUT,
        &mut *shutdown,
        &state,
        &ble_id,
    )
    .await?;
    let Message::Hello {
        device_id,
        nickname,
        device_type,
        tcp_port,
        nonce,
        sig,
        x25519_pubkey,
        ed25519_pubkey,
        ..
    } = &first
    else {
        unreachable!("read_hello_frame 只返回 Hello");
    };
    crate::network::transport::verify_hello_for_ble(
        &state,
        device_id,
        *tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig,
    )?;
    let peer_id = device_id.clone();

    // ---- 指定拨号方判据（与 TCP 的 `should_dial` 同一条规则：**大 id 拨，小 id 只接受**）----
    //
    // 两端都同时跑 central + peripheral ⇒ 会互相拨号。若对端 id 比我大，说明它也会拨我：
    // 我拨过去建成的是一条**镜像链路**，它会把这条拒掉（不回 Hello），而这条连接的建立
    // 过程会打断它拨给我的那条好链路 —— 于是好链路 45s 收不到帧被看门狗拆掉、再重来
    // （用户 2026-09-12 实测：「点加好友：发送失败，连接已关闭」）。
    // 所以：记进"不要再拨"，并主动放弃这一条。
    //
    // `peer_advertises`：我们是在**扫描结果**里看到这个端点的（扫到 = 它在广播），
    // 但只有 `scan_loop` 真的把它记进 `ble_peer_advertises` 才算"确认能广播"。
    // 传 false（Windows 这类只做 central 的平台，或对端不广播）⇒ 无条件拨，见 `should_dial_ble`。
    let peer_advertises = state
        .ble_peer_advertises
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&ble_id);
    if !should_dial_ble(&state.device_id, &peer_id, peer_advertises) {
        state
            .ble_no_dial
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(ble_id.clone());
        state.logger.info(
            "ble",
            format!(
                "对端 {peer_id} 是这条链路的指定拨号方（id 更大）⇒ 记下不再主动拨它，避免镜像链路互扰"
            ),
        );
        // 显式断开：只 return 的话这条连接会挂着，继续占着对端 GATT server 的那个连接槽
        //（对端每次收到我们的新连接都会替换旧连接 ⇒ 正好打断它拨过来的好链路）。
        let _ = peripheral.disconnect().await;
        return Ok(());
    }

    // ---- 去重：与 TCP 入站**同一个判据**（不要在这里复制第二份"有没有同路径连接"）----
    let existing = link_snapshot(&state, &peer_id).await;
    if !should_accept_inbound_public(&state.device_id, &peer_id, PathKind::Bluetooth, &existing) {
        return Err("已有蓝牙链路（或该 peer 链路数已满），不重复建链".to_string());
    }

    // ---- 登记链路（端点 = BLE 标识，路径 = Bluetooth）----
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
    register_connection(&state, &peer_id, ep.clone(), PathKind::Bluetooth);
    crate::network::transport::replay_group_frames_to(&state, &peer_id); // #77：蓝牙出站
    state.logger.info(
        "ble",
        format!(
            "[SESSION] 已就绪 peer={peer_id} 昵称={nickname:?} 类型={device_type} ep={ep}\
             （双向 Hello 已验签，transport 可用；ep 是**本机这一侧的链路标识**，\
              central 侧=对端外设标识 / 外设侧=对端 central 标识，两者不一定同串）"
        ),
    );
    state.logger.info(
        "ble",
        format!("+ble-link peer={peer_id} ep={ep}（双向 Hello 已验签）"),
    );

    // 首帧（对端 Hello）交给统一的处理路径：写身份 + 双公钥、对齐会话时钟、冲刷待发队列
    handle_message(&state, &peer_id, first).await;

    tokio::spawn(ble_writer_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        writer,
        high_rx,
        normal_rx,
        low_rx,
        shutdown.clone(),
        cancel_rx.clone(),
    ));
    // ↑ 写循环泛型化：central 写 GATT 特征、外设发通知，逻辑同一份（见 `FrameSink`）
    tokio::spawn(ble_reader_loop(
        state.clone(),
        peer_id.clone(),
        ep.clone(),
        reader,
        shutdown.clone(),
        cancel_rx,
    ));

    // 建链即冲一次待发队列（与 TCP 拨号成功后的序列一致）
    flush_outbox(&state, &peer_id).await;
    flush_group_outbox(&state, &peer_id).await;
    flush_pending_reads(&state, &peer_id).await;
    flush_pending_group_reads(&state, &peer_id).await;
    flush_pending_group_keys(&state, &peer_id).await;
    crate::commands::flush_pending_files(&state, &peer_id).await;
    crate::commands::flush_pending_group_files(&state, &peer_id).await;
    Ok(())
}

/// 在读循环之外（握手阶段）读一条完整消息：窗口内没有分片就继续等，直到超时。
async fn read_one(
    reader: &mut BleReader,
    overall: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Option<Message>, String> {
    let deadline = tokio::time::Instant::now() + overall;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        let frame = tokio::select! {
            biased;
            _ = shutdown.changed() => return Ok(None),
            res = reader.next_frame(READ_IDLE) => res?,
        };
        let Some(frame) = frame else {
            let _ = reader.gc();
            continue;
        };
        return serde_json::from_slice::<Message>(&frame)
            .map(Some)
            .map_err(|e| format!("握手帧无法解析：{e}"));
    }
}

/// 握手期间最多跳过多少个"非 Hello 的前导帧"。
///
/// 为什么要上限：既能让"上一条链路的残留帧"过去，又不能让对端无限灌帧把握手拖住
/// （每一帧都要过一遍 JSON 解析）。
const MAX_HANDSHAKE_PREAMBLE_FRAMES: u32 = 32;

/// 读**首个 Hello**：跳过并丢弃握手前导帧（见调用点的说明）。
///
/// 安全性：被丢掉的帧**绝不进入业务处理**（`handle_message` 那一层）—— 身份来自 Hello
/// 的签名验证，验签之前任何帧都只是字节。
///
/// 注：护栏 `ble_handshake_skips_leading_frames_without_processing_them` 会检查本函数体里
/// **不出现**业务入口的调用，所以这里的说明用文字描述、不写成那句调用本身。
async fn read_hello_frame(
    reader: &mut BleReader,
    overall: Duration,
    shutdown: &mut watch::Receiver<bool>,
    state: &AppState,
    ep_for_log: &str,
) -> Result<Message, String> {
    let deadline = tokio::time::Instant::now() + overall;
    let mut dropped = 0u32;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(format!(
                "握手超时：窗口内没等到 Hello（丢掉了 {dropped} 个前导帧）"
            ));
        }
        let Some(frame) = read_one(reader, left, shutdown).await? else {
            return Err(format!(
                "握手超时：对端未回 Hello（丢掉了 {dropped} 个前导帧）"
            ));
        };
        match preamble_action(dropped, matches!(frame, Message::Hello { .. })) {
            PreambleAction::Hello => {
                if dropped > 0 {
                    state.logger.info(
                        "ble",
                        format!(
                            "[SESSION] 跳过 {dropped} 个握手前导帧后收到 Hello ep={ep_for_log}"
                        ),
                    );
                }
                return Ok(frame);
            }
            PreambleAction::GiveUp => {
                return Err(format!(
                    "对端首帧不是 Hello（连续 {dropped} 帧都不是，最后一帧 type={}）",
                    frame.wire_kind()
                ));
            }
            PreambleAction::Drop => {}
        }
        dropped += 1;
        state.logger.info(
            "ble",
            format!(
                "[SESSION] 丢弃握手前导帧 type={} ep={ep_for_log}（等 Hello，已丢 {dropped}）",
                frame.wire_kind()
            ),
        );
    }
}

/// 握手前导帧的处理决定（纯函数内核，见 `read_hello_frame`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreambleAction {
    /// 收到 Hello ⇒ 进入握手。
    Hello,
    /// 还不是 Hello，但额度没用完 ⇒ 丢掉它继续等。
    Drop,
    /// 额度用尽 ⇒ 明确报错（带上最后一帧的类型，便于真机定位）。
    GiveUp,
}

/// 纯函数：`dropped` = 已经丢掉了多少个前导帧。
fn preamble_action(dropped: u32, is_hello: bool) -> PreambleAction {
    if is_hello {
        PreambleAction::Hello
    } else if dropped >= MAX_HANDSHAKE_PREAMBLE_FRAMES {
        PreambleAction::GiveUp
    } else {
        PreambleAction::Drop
    }
}
