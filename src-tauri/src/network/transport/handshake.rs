// Hello 认证与好友接受：判据、密钥绑定、有界补发、签名 Hello 构造
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第四批；守卫视图与领域图的分册清单一同登记。

/// Hello 认证的**纯判定**：给定「已绑定公钥」（`None` = 首次接触 TOFU），
/// 判断本次 Hello 是否可信。抽出来是为了可单测（不依赖 AppState）。
fn hello_auth_decision(
    bound: Option<&str>,
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
    sig_b64: &str,
) -> Result<(), String> {
    if nonce.is_empty() || sig_b64.is_empty() {
        return Err(format!("Hello 缺少 nonce/sig（device_id={device_id}）"));
    }
    let data = hello_signing_bytes(device_id, tcp_port, nonce, x25519_pubkey, ed25519_pubkey);
    match bound {
        // 已知身份：自报公钥必须与绑定公钥一致，且签名必须由该公钥验证通过。
        // 攻击者即便拿到真实公钥也签不出来；用自己公钥签名则与绑定值不符。
        Some(expected) => {
            if expected != ed25519_pubkey {
                return Err(format!(
                    "Hello 公钥与已绑定身份不符（device_id={device_id}）：对方可能重装了应用。\
                     若确认是本人重装，删掉该好友后重新添加即可（聊天记录保留、无需重启）"
                ));
            }
            if !crypto::verify_signature(expected, &data, sig_b64) {
                return Err(format!("Hello 签名校验失败（device_id={device_id}）"));
            }
            Ok(())
        }
        // 首次接触（TOFU）：仅要求自洽签名；密钥绑定在后续 announce/upsert 中固化。
        // 注意：TOFU 分支无法冒充「已建立信任的身份」——那是上面 Some 分支的事。
        None => {
            if !crypto::verify_signature(ed25519_pubkey, &data, sig_b64) {
                return Err(format!("Hello 自签名校验失败（device_id={device_id}）"));
            }
            Ok(())
        }
    }
}

/// 校验 Hello 握手，确认 TCP 对端确实持有 `device_id` 绑定的 Ed25519 私钥。
///
/// 信任根：`friends`（持久、权威）→ `peers`（运行时）中该 device_id 已绑定的
/// Ed25519 公钥。两者都没有时才走 TOFU（首次接触），用 Hello 自带的公钥验签。
///
/// 这封堵的是：任意局域网节点在 Hello 里自报好友/群主的 device_id 即可建立链路，
/// 随后利用 `from == peer_id` 的绑定关系伪造 GroupMemberRemoved / GroupRename 等
/// 明文控制消息（把群从受害者本地删掉、改名）。已建立信任的身份必须签名匹配。
///
/// 返回 `Err(原因)` 表示必须拒绝该连接。
pub(crate) fn verify_hello(
    state: &AppState,
    device_id: &str,
    tcp_port: u16,
    nonce: &str,
    x25519_pubkey: &str,
    ed25519_pubkey: &str,
    sig_b64: &str,
) -> Result<(), String> {
    if nonce.is_empty() || sig_b64.is_empty() {
        return Err(format!("Hello 缺少 nonce/sig（device_id={device_id}）"));
    }
    // 已绑定身份：**好友表（持久，权威）优先**；在线节点表只作回落，
    // 且**回落项必须是已验签的**（`keys_verified`）。
    //
    // ⚠️ 为什么回落必须过滤：`peers` 里的公钥可能来自**未签名**的 UDP announce。
    // 若把广播来的公钥当成绑定，攻击者只要抢先广播（真实节点 5s 才播一次，他 100ms 一次，
    // 必赢这个竞态）就能让真实好友的 Hello 被判「公钥与已绑定身份不符」而永远连不上，
    // 同时攻击者自己的 Hello（用他自报的那把公钥验签）却能顺利通过 —— 完成身份冒充。
    let bound = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend_ed25519(&dbc, device_id)
    }
    .or_else(|| {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        bound_ed25519_from_peer(peers.get(device_id))
    });
    hello_auth_decision(
        bound.as_deref(),
        device_id,
        tcp_port,
        nonce,
        x25519_pubkey,
        ed25519_pubkey,
        sig_b64,
    )?;
    // ⚠️ nonce 的消费必须放在**验签通过之后**。
    //
    // 它是一条有界 FIFO（512 条）：先消费等于给任何**未通过验签**的连接发了一张
    // 污染缓存的入场券 —— 洪泛者可以持续占用/挤出槽位，把合法对端的 nonce 顶掉，
    // 或在窗口内让合法 Hello 被误判成「重放」而拒掉（表现为"好友时连时断"）。
    // 顺序调换不改变任何安全性质：重放的 Hello 签名本来就有效，
    // 依旧会被下面这一判拦下 —— 只是它不再有机会占用槽位。
    if !state.accept_hello_nonce(nonce) {
        return Err(format!("Hello nonce 重放（device_id={device_id}）"));
    }
    Ok(())
}

/// 在线节点表里的公钥**能否作为 Hello 的身份绑定** —— 唯一判定点。
///
/// 只有 `keys_verified`（Hello 验签通过后由 `mark_peer_keys_verified` 置位）的条目才算数。
/// 未验签的条目只可能来自**未签名**的 UDP announce：任何人拿到 device_id（announce 里
/// 明文广播）就能以它广播自己的公钥。若这种公钥被当成绑定，攻击者只需抢先广播
/// （真实节点 5s 播一次、他 100ms 一次，必赢竞态），就能：
///   ① 让真实好友的 Hello 被判「公钥与已绑定身份不符」而永远连不上；
///   ② 用自己的私钥签 Hello 冒充该好友 —— 绑定值就是他自己的公钥，验签必然通过。
/// 抽成独立函数是为了让这条规则有名字、有单测，而不是散在 `or_else` 闭包里。
fn bound_ed25519_from_peer(peer: Option<&Peer>) -> Option<String> {
    peer.filter(|p| p.keys_verified)
        .and_then(|p| p.ed25519_pubkey.clone())
}

/// `peers` 里这对钥匙是不是**被证明过**的 —— 只有验签通过的 Hello 能打上 `keys_verified`
/// （见 `mark_peer_keys_verified`），未签名的 UDP announce 带来的不算。
///
/// 这是"能不能拿它当身份锚点"的**唯一**判据。此前只有 `upsert_peer` 问它，而三条
/// 成为好友的路径（直连 FriendAccept / 跨跳 Gossip FriendAccept / 本机点同意）读的是
/// **同一张 `peers` 表**却不过这道闸 —— 同一个概念两处规矩，就是本仓库反复付钱的那类缺陷。
/// 收紧的理由：`friends.ed25519_pubkey` 此后既是 Hello 的验签锚点（INV-P21），又是
/// **公网中继电路的准入判据**（`db/friends.rs` 的 `list_bound_friend_identities` 只看它
/// 非空），一次伪造广播把它抢先填上，后果从"消息被加密给攻击者"扩到"我们主动跨公网
/// 给攻击者建电路、并把协商验签锚在它的钥匙上"（ADR-0020 自己称那把钥匙为"整个设计的支点"）。
///
/// 留 NULL 不是死路：验签通过的 Hello 会经 `handle_message` 的 Hello 分支打上
/// `keys_verified`（见 `mark_peer_keys_verified` 那条 ⚠️），下一条 announce 就补得进来 ——
/// 而 `update_friend_pubkeys` 只填空、不覆盖，所以"晚一点绑"安全，"绑错"永久。
fn peer_keys_trusted(state: &AppState, device_id: &str) -> bool {
    state
        .peers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .map(|p| p.keys_verified)
        .unwrap_or(false)
}

/// 这一刻允许绑哪些钥匙（纯函数 —— 把"两列两套规矩"这条判据单独钉住，不必造 AppState）。
///
/// `verified` 来自 `peer_keys_trusted`：**加密钥匙照绑，身份锚点只认被证明过的来源**。
/// 未 verified 时把 `ed25519` 收成 `None` 不是"漏了一步"，而是刻意留 NULL 等一次验签通过的
/// Hello 来补（`upsert_peer` 那条路本来就做这件事）—— 因为 `update_friend_pubkeys` 只填空、
/// 首写者永久胜出，"晚一点绑"安全，"绑错"永久。
fn acceptable_friend_keys(
    verified: bool,
    x25519: Option<String>,
    ed25519: Option<String>,
) -> (Option<String>, Option<String>) {
    (x25519, if verified { ed25519 } else { None })
}

/// 成为好友那一刻把公钥补进 `friends`：**加密钥匙照旧早绑，身份锚点只认被证明过的来源**。
///
/// 为什么两列区别对待（这是 #32 这一片的全部要点）：
///   · `x25519` 是功能性钥匙 —— 不绑就是"首次加密发送失败"（三条调用点原本各写一遍的注释
///     说的都是这件事），而 Gossip 那条补齐路径以"这一封能解密"作持有证明，收紧它只会把
///     可靠性修回去；
///   · `ed25519` 是**身份锚点** —— Hello 验签（INV-P21）、安全码、以及公网中继的准入判据
///     （`list_bound_friend_identities` 只看它非空）全都读它，而 `update_friend_pubkeys`
///     只填空、首写者永久胜出 ⇒ 一次伪造广播就能把它永久钉死。
///
/// 未 verified 时留 NULL 不是死路：任何一次验签通过的 Hello 都会经 `upsert_peer` 补上。
/// 调用方必须**已经持有** `state.db` 的锁（本函数不再取锁 —— 同锁重入会当场死锁）。
pub(crate) fn bind_friend_keys_on_accept(
    state: &AppState,
    conn: &rusqlite::Connection,
    friend_id: &str,
) {
    let (x, e) = {
        let peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
        peers
            .get(friend_id)
            .map(|p| (p.x25519_pubkey.clone(), p.ed25519_pubkey.clone()))
            .unwrap_or((None, None))
    };
    let (x, e) = acceptable_friend_keys(peer_keys_trusted(state, friend_id), x, e);
    if x.is_some() || e.is_some() {
        db::update_friend_pubkeys(conn, friend_id, x.as_deref(), e.as_deref()).ok();
    }
}

/// 把某节点的公钥标记为**已验证**（Hello 验签通过后调用）。
///
/// 为什么必须有这个显式升级点：`peers` 表由两条信任级别完全不同的路径共同维护 ——
/// 未签名的 UDP announce（可伪造）与验签通过的 Hello（可信）。只靠「表里有值」无法区分
/// 二者，于是未验证的公钥会被当成身份绑定用（详见 `verify_hello` 与 `upsert_peer` 的注释）。
/// 这里用一对公钥的**实际值**再核对一次：只有与 Hello 自报值一致时才升级，
/// 避免在验签与本次写入之间被另一条 announce 插空改动。
///
/// ⚠️ **条目不存在时它是空操作**（下面的 `let Some(p) = ... else { return }`），所以"在握手
/// 验签通过后立刻调一次"**不够**：出站拨号与 BLE 那两条路径的 `peers` 条目是稍后才由
/// `handle_message` 的 Hello 分支经 `upsert_peer` 建的（`upsert_peer` 新建时恒标
/// `keys_verified: false`）。只在握手处调，则"第一次连接的对方"整个会话都是未验证 ⇒
/// `friends.ed25519` 补不上 ⇒ 安全码算不出、公网中继永不准入。所以 Hello 分支在
/// `upsert_peer` **之后**再调一次（那里条目必然已带着这对刚验过的钥匙）。
fn mark_peer_keys_verified(state: &AppState, device_id: &str, x25519: &str, ed25519: &str) {
    let mut peers = state.peers.lock().unwrap_or_else(|e| e.into_inner());
    // ⚠️ **只给已存在的记录打标，绝不凭空造记录**。
    //
    // 曾经的写法是「Hello 早于 announce ⇒ 先插一条占位记录」，那是错的：
    // `peers` 的条目还要承载 **ip / tcp_port / nickname**（「添加好友」列表直接读它、
    // 拨号也用它），凭空造出来的条目这些字段全是空的 —— 表现为「搜得到这个节点、
    // 但加不上好友」，而且会被当成"在线"参与 UI 判定。
    //
    // 对端若还没 announce，这里就什么都不做：`bound` 回落为空 ⇒ 走 TOFU 分支
    // （与本次改动之前的行为完全一致），下一条 announce 会把它正常登记进来。
    let Some(p) = peers.get_mut(device_id) else {
        return;
    };
    if p.x25519_pubkey.as_deref() != Some(x25519) || p.ed25519_pubkey.as_deref() != Some(ed25519) {
        return; // 与自报值不一致：不动（交由既有 key_conflict 路径处理）
    }
    p.keys_verified = true;
}

/// 同意好友之后**忘掉这条申请**（内存态 `pending_requests` 里的那一行）。
///
/// 真实缺陷（用户 2026-09-12 真机实测）：双方互发过申请时，A 点了同意，B 的「新朋友」里
/// 那条申请**还在** —— 因为直连路径（`Message::FriendAccept`）只加了好友、没有清 pending，
/// 而跨跳路径（`GossipKind::FriendAccept`）清了。同一件事两条路径行为不一致，
/// 于是"有时候会清、有时候不清"。现在两条路径 + `respond_friend_request` 都走这一个助手，
/// 前端再用 `pendingRequests`（按好友列表过滤）兜一层，不会再出现"已经是好友还挂在申请里"。
/// 收到好友申请时的统一前置判断：**对方已经是我的好友就直接同意**。
///
/// 返回 `true` 表示"已经自动处理掉了，不要再往 pending 里插"。
///
/// ## 为什么必须有（用户 2026-09-12 真机实测的 bug）
/// B 的好友列表里已经有 A，而 A 是**重置过的账号**、列表里没有 B：
///   · A 发申请 → 旧实现只在 B 侧插一条 pending；
///   · 而 `get_pending_requests` 又会把"申请人是已是好友"的条目**过滤掉**
///     （那是为了修"已经是好友了、申请还挂着"）；
///   ⇒ 两边都看不到、谁也加不上。用户只能先**删掉** B 里的 A、再加回来。
///
/// 正确语义（用户给的规则）：**"他已经是我的好友"就等于我已经同意了这件事** ——
/// 收到这种申请时直接走完整的"同意"路径（落库 + 回执 + 清 pending + 通知 UI），
/// 双方关系立刻收敛，不需要任何人工动作。
///
/// 为什么放在两条 FriendRequest 路径**都**调：直连（`Message::FriendRequest`）与跨跳
/// （`GossipKind::FriendRequest`）是两套独立的入口，只在一条上修就会"同一件事两种行为"。
async fn auto_accept_if_already_friend(state: &Arc<AppState>, peer_id: &str) -> bool {
    let already_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_friend(&dbc, peer_id).is_some()
    };
    if !already_friend {
        return false;
    }
    state.logger.info(
        "friend",
        format!("收到已是好友的申请：自动同意 peer={peer_id}（双方关系收敛）"),
    );
    if let Err(e) = crate::commands::accept_friend_request(state, peer_id).await {
        state
            .logger
            .warn("friend", format!("自动同意失败 peer={peer_id}：{e}"));
    }
    true
}

/// 收到「好友申请已通过」后的**全部**本地动作 —— 直连与跨跳共用这一个家。
///
/// ## 为什么要收成一家（2026-09-28 架构复审抓到）
/// "重复投递不许刷屏"这条判据原先**只写在跨跳那一份里**（`gossip.rs`，注释里还记着
/// 真机日志同一秒三次那个症状），直连这一份没有 ⇒ 同一条链路上同一个缺陷照旧存在。
/// "同一句话在两处各写一遍、其中一处缺条件"就是这类缺陷的形状，所以连留痕文字
/// 都收进来（只有 `trace` 按来源不同）。
///
/// ## 幂等判据
/// `FriendAccept` 没有 ACK 机制，发送方会持续补发（见 `flush_pending_friend_accept`）⇒
/// 只按"这一次是否真的从不是好友变成好友"决定要不要通知；`add_friend` 本身是幂等的。
pub(crate) fn apply_friend_accept(state: &Arc<AppState>, from: &str, trace: &str) {
    let name = resolve_nickname(state, from);
    let was_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let seen = db::get_friend(&dbc, from).is_some();
        db::add_friend(&dbc, from, &name, None).ok();
        // 同步公钥：x25519 照旧早绑，ed25519 只认 verified 来源
        // （跨跳到达的那一份尤其要紧 —— 它允许 TOFU，`peers` 里那对钥匙
        //  可能来自未签名广播，判据在 `bind_friend_keys_on_accept` 里）。
        bind_friend_keys_on_accept(state, &dbc, from);
        seen
    };
    // 已经是好友了 ⇒ 这条申请必须消失（否则「新朋友」里会留着一条永远处理不掉的申请）。
    forget_pending_request(state, from);
    // emit 每次都发：前端 store 只是据此重拉好友列表（幂等），
    // 而漏发会让「首次那个 emit 恰好没被界面收到」时界面永远不刷新。
    let _ = state.app.emit("friend-accepted", from);
    if was_friend {
        // 重复投递：只留一行便于排查的痕迹，**不通知**。
        state
            .logger
            .info("friend", format!("重复的好友同意（已忽略）peer={from}"));
    } else {
        state.logger.info("friend", format!("{trace} peer={from}"));
        let _ = crate::notifications::show_if_enabled(
            state,
            "好友申请已通过",
            &format!("{name} 已成为你的好友"),
        );
    }
}

pub fn forget_pending_request(state: &AppState, peer_id: &str) {
    state
        .pending_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(peer_id);
    // 同一条路径也清掉"我方已发出、等对方确认"的登记：对方既然回执了（同意/拒绝），
    // 就不该再补发（`flush_pending_friend_request` 只对仍未处理的申请生效）。
    state
        .pending_out_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(peer_id);
}

/// **补发"我方已发出、还未被处理"的好友申请**（建链 / Hello 补全时调用）。
///
/// 为什么需要（用户 2026-09-12 真机）：「好友已发送，等待对方确认」，但对方**什么都没收到**
/// —— 好友申请是**没有回执**的定向帧，链路正好在那一刻抖动（BLE 镜像互拨打断链路）时
/// 它就静默丢了，而发送方界面依然显示"已发送"。现在发出即登记，这里补发一次；
/// 收到同意/拒绝（走 `forget_pending_request`）后清除，所以不会无限重发。
pub async fn flush_pending_friend_request(state: &Arc<AppState>, peer_id: &str) {
    let pending = state
        .pending_out_requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(peer_id);
    if !pending {
        return;
    }
    // ⚠️ **已经是好友就不再补发**。
    //
    // 这条登记原本只由 `forget_pending_request` 在「收到对方的同意/拒绝」时清除。
    // 但同意回执本身是**没有 ACK 的定向帧**，可能一直送不到（真机日志：
    // Mac 侧持续 `补发好友申请` 而全程没有 `收到跨跳好友同意`）——
    // 于是登记永不解除，链路每建立一次就重发一次，**无限循环**。
    //
    // 判据用「本地好友表里有没有他」而不是「有没有收到那个回执」：
    // 无论友谊是通过哪条路径建立的（对方同意、我方同意、自动同意），
    // 只要已经是好友，这条待发申请就失去了意义。
    let already_friend = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::db::get_friend(&dbc, peer_id).is_some()
    };
    if already_friend {
        state
            .pending_out_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer_id);
        state
            .logger
            .info("friend", format!("已是好友，停止补发申请 peer={peer_id}"));
        return;
    }
    // 复用同一条发送路径（含目标定向 + 重签），失败也不清登记 —— 下次建链再试
    if crate::commands::send_friend_request_via_link(state, peer_id)
        .await
        .is_ok()
    {
        state.logger.info(
            "friend",
            format!("补发好友申请 peer={peer_id}（此前链路抖动丢过）"),
        );
    }
}

/// 好友同意回执「补发一次 / 收尾 / 什么都不做」的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendAcceptFlush {
    /// 补发，且这是第几次（1-based）。
    Flush(u32),
    /// 窗口或次数用尽 ⇒ 清掉登记并留一条 warn。
    GiveUp,
    /// 还没到间隔，或根本没有登记。
    Nothing,
}

/// 好友同意回执补发策略的**纯函数内核**：
/// `entry` = `(issed_at, attempts, last_at)`，`None` = 没有待补发登记。
///
/// 策略：`window_ms` 窗口内最多 `max_attempts` 次、两次之间至少隔 `min_interval_ms`。
/// 为什么是"有界补发"而不是无限重试：对方收到重复的 `FriendAccept` 是幂等的（多一次
/// `add_friend` + 一个 UI 事件），但没必要一直打扰；3 次足以覆盖"一次建链 + 两次心跳"。
pub fn friend_accept_flush_decision(
    entry: Option<(i64, u32, i64)>,
    now: i64,
    max_attempts: u32,
    min_interval_ms: i64,
    window_ms: i64,
) -> FriendAcceptFlush {
    let Some((issued, attempts, last)) = entry else {
        return FriendAcceptFlush::Nothing;
    };
    if now - issued > window_ms || attempts >= max_attempts {
        return FriendAcceptFlush::GiveUp;
    }
    if now - last < min_interval_ms {
        return FriendAcceptFlush::Nothing;
    }
    FriendAcceptFlush::Flush(attempts + 1)
}

/// **补发好友同意回执**（`FriendAccept`）。
///
/// 与 `flush_pending_friend_request` 的关键差别：申请是"等对方动作"（收到同意/拒绝才清），
/// 而同意回执**没有回执** —— 发送方无从得知对方是否收到。所以这里用**有界补发**：
/// 2 分钟窗口内最多 3 次、两次之间至少隔 5s（心跳周期），窗口/次数用尽就打一条 warn 收尾。
///
/// 为什么必须做（用户 2026-09-13 真机）：Android 点「接受」后 Android 侧好友列表已经有对方，
/// 但 **Mac 端状态一直没同步** —— 那一帧在 BLE 链路抖动/尚未建好时静默丢了，且永不重发。
pub async fn flush_pending_friend_accept(state: &Arc<AppState>, peer_id: &str) {
    const MAX_ATTEMPTS: u32 = 3;
    const MIN_INTERVAL_MS: i64 = 5_000;
    const WINDOW_MS: i64 = 120_000;
    let now = crate::db::now_ms();

    // 锁内只做判定（**绝不跨 await 持锁**）；判定本身是纯函数
    // （`friend_accept_flush_decision`）—— "窗口 + 次数 + 间隔"这类策略最容易写反，
    // 写在 async + 锁里就没法单测。
    let decision = {
        let entry = state
            .pending_out_accepts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer_id)
            .copied();
        friend_accept_flush_decision(entry, now, MAX_ATTEMPTS, MIN_INTERVAL_MS, WINDOW_MS)
    };
    let attempt = match decision {
        FriendAcceptFlush::Nothing => return,
        FriendAcceptFlush::GiveUp => {
            let removed = state
                .pending_out_accepts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(peer_id);
            state.logger.warn(
                "friend",
                format!(
                    "好友同意回执补发结束 peer={peer_id}（窗口/次数用尽：now={now} 登记={removed:?}）"
                ),
            );
            return;
        }
        FriendAcceptFlush::Flush(attempt) => attempt,
    };
    // 判定是纯的 ⇒ 次数/时刻的写入在执行侧完成
    {
        let mut map = state
            .pending_out_accepts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(peer_id) {
            entry.1 = attempt;
            entry.2 = now;
        }
    }
    match crate::commands::send_friend_accept_via_link(state, peer_id).await {
        Ok(()) => state.logger.info(
            "friend",
            format!("[FRIEND] 补发好友同意回执 peer={peer_id}（第 {attempt} 次）"),
        ),
        Err(e) => state.logger.warn(
            "friend",
            format!("[FRIEND] 补发好友同意回执失败 peer={peer_id}（第 {attempt} 次）：{e}"),
        ),
    }
}

/// 构造带签名的 Hello（nonce 每次新生成，签名覆盖连接身份的全部字段）。
/// Hello 里能带的头像上限（字节）。
///
/// ## 为什么必须限制（真机 2026-09-14 三端日志，本批最严重的一条）
///
/// `Hello` 是**握手帧**：链路刚建好就要发出去，对端等它的时间就是 `HANDSHAKE_TIMEOUT`（10s）。
/// 而旧实现把 `state.avatar` **原样**塞进 Hello —— 用户设的是一张 base64 图片时，
/// 这个帧可以到 **几百 KB**。BLE 上后果是双重的：
///   · central 侧：424303 字节 ÷ 514 字节/片 ≈ **826 片 × 12ms ≈ 10s** ⇒ 正好撞上握手超时，
///     对端日志是「握手超时：对端未回 Hello」；
///   · 外设侧：CoreBluetooth/Android 在某些时序下报的 `maximumUpdateValueLength` 还是默认值
///     （MTU 23 ⇒ 每片 20 字节）⇒ 需要 **3 万多片** > `MAX_BLE_CHUNKS_PER_MESSAGE`(8192)
///     ⇒ `fragment()` 直接返回 `None`，日志是「回 Hello 失败：帧无法分片（过大或 MTU 非法：
///     len=424303 mtu=20）」—— 真机上就是"搜得到、连得上、永远握手不成、发不出消息"。
///
/// 头像属于**展示信息**，晚一点、走别的路径同步都可以；握手帧必须小到能秒过。
/// 取 2 KiB：正常的小图标/首字母头像远小于它，而任何"图片级"头像都会被挡在握手之外。
pub const HELLO_AVATAR_MAX_BYTES: usize = 2048;

/// 交给 Hello 携带的头像：**过大就返回 `None`**（并且只留一次 warn 让真机可查）。
///
/// 纯函数：`Option<&str>` 便于单测。
pub fn hello_avatar_for_wire(avatar: Option<&str>) -> Option<&str> {
    match avatar {
        Some(a) if !a.is_empty() && a.len() <= HELLO_AVATAR_MAX_BYTES => Some(a),
        _ => None,
    }
}

pub fn build_signed_hello(state: &AppState, conv_clock: i64) -> Message {
    let device_id = state.device_id.clone();
    let tcp_port = state.tcp_port;
    let x25519_pubkey = state.identity.x25519_public_b64();
    let ed25519_pubkey = state.identity.ed25519_public_b64();
    // ⚠️ 头像**必须先过尺寸闸门**再进握手帧（见 `HELLO_AVATAR_MAX_BYTES` 的说明）：
    // 一张 base64 头像能把 Hello 撑到几百 KB，BLE 上直接导致握手超时或"帧无法分片"。
    let raw_avatar = state
        .avatar
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let avatar = hello_avatar_for_wire(raw_avatar.as_deref()).map(str::to_string);
    if avatar.is_none() && raw_avatar.as_deref().is_some_and(|a| !a.is_empty()) {
        state.logger.warn(
            "transport",
            format!(
                "Hello 不携带头像：本机头像 {} 字节 > 上限 {} 字节（握手帧必须小；头像由 UserInfo 同步）",
                raw_avatar.as_deref().map(str::len).unwrap_or(0),
                HELLO_AVATAR_MAX_BYTES
            ),
        );
    }
    let nonce = STANDARD.encode(crypto::random_key());
    let sig = state.identity.sign_b64(&hello_signing_bytes(
        &device_id,
        tcp_port,
        &nonce,
        &x25519_pubkey,
        &ed25519_pubkey,
    ));
    Message::Hello {
        device_id,
        nickname: state
            .nickname
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        avatar,
        device_type: crate::protocol::current_device_type().to_string(),
        content_features: crate::protocol::content_features(),
        protocol_version: Some(crate::protocol::PROTOCOL_VERSION),
        app_version: Some(crate::protocol::current_app_version().to_string()),
        tcp_port,
        x25519_pubkey,
        ed25519_pubkey,
        conv_clock,
        nonce,
        sig,
    }
}

/// 把本机完整资料（昵称/头像/设备类型）**定向**同步给一个对端。
///
/// 为什么需要它：Hello 只带不超过 2KiB 的头像、Presence 已不再内联大头像，所以
/// 「大头像」只剩这一条正式路径 —— 链路建好后同步**一次**。超过
/// CONTROL_AVATAR_MAX_BYTES 的帧会被通道分类降到 Low 队列，
/// 不再和聊天/好友请求抢优先道。
pub async fn send_user_info_to(state: &Arc<AppState>, peer_id: &str) {
    let msg = Message::UserInfo {
        device_id: state.device_id.clone(),
        nickname: state
            .nickname
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        avatar: state
            .avatar
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        device_type: crate::protocol::current_device_type().to_string(),
    };
    let _ = try_send(state, peer_id, &msg).await;
}
