// Outbox 超时清扫与过期终态（spawn_outbox_sweeper / finalize_expired_*）
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第二批，判据与顺序见 docs/large-file-split-plan.md。

/// outbox 清扫间隔（毫秒）。
/// 30s 跑一次，每次扫描 created_at < now - OUTBOX_FAIL_DEADLINE_MS 的条目。
pub const OUTBOX_SWEEPER_INTERVAL_MS: u64 = 30_000;

/// 把一条过期的**消息** outbox 落终态：置 `messages.status = "failed"` + 删 outbox 行。
/// **两步都成功**才返回 `true` —— 调用方据此决定是否 `emit("message-failed")`。
///
/// 为什么必须返回这个 conjunction（审计 A3）：旧代码把 emit 写在写库**之外**、还用 `let _ =`
/// 丢掉写库返回值 ⇒ 锁中毒或写库失败时**界面报失败、而 outbox 行还在** ⇒ 下次 `flush_outbox`
/// 又把它发出去 = 重复投递（"我以为失败了的消息又发出去了"）。返回 `false` 时调用方**保留行、
/// 不 emit、留一条 warn**，下一 tick 重试。`group` 决定删哪张 outbox 表（单聊 / 群是两张表）。
fn finalize_expired_message(dbc: &rusqlite::Connection, msg_id: &str, group: bool) -> bool {
    let status_ok = db::set_message_status(dbc, msg_id, "failed").is_ok();
    let deleted = if group {
        db::delete_group_outbox_by_msg_id(dbc, msg_id).is_ok()
    } else {
        db::delete_outbox_by_msg_id(dbc, msg_id).is_ok()
    };
    status_ok && deleted
}

/// 把一条过期的**文件** outbox 落终态（审计 A3 同型）：两个消息前缀（`file-` / `gfile-`）
/// 置 failed + transfer 落 failed + 标记 file_outbox 失败。**全部成功**才返回 true（门控
/// `emit("file-failed")`）。
///
/// 写序、`done` 闸门、"这一路不碰群气泡"三条都在 `db::finalize_file_failure` 里 ——
/// 本函数只是把它的 `Result<bool>` 折成清扫器要的 bool（见那里的注释与
/// `expired_file_never_rewrites_a_completed_transfer` /
/// `expired_file_does_not_touch_the_group_bubble` 两条判据）。
fn finalize_expired_file(dbc: &rusqlite::Connection, transfer_id: &str) -> bool {
    // 实现已并入唯一的出口 `db::finalize_file_failure`（P7：三份收尾合成一份）。
    // 保留这一层是因为清扫器要的是 bool（emit 的门控条件），而顺序/闸门都在里面。
    db::finalize_file_failure(dbc, transfer_id, db::FileJobEnd::Expired).unwrap_or(false)
}

/// 启动 outbox 超时清扫后台任务。
///
/// 每 `OUTBOX_SWEEPER_INTERVAL_MS` 扫一次：单聊 outbox 和群 outbox 里
/// created_at + OUTBOX_FAIL_DEADLINE < now 的条目 → 删 outbox 行 + 置
/// messages.status = "failed" + emit("message-failed", msg_id) 通知前端。
///
/// 为什么必须有这个任务：
/// - outbox 的 flush_outbox 只在「建链 / Hello / 心跳」时触发，完全无链路的 peer
///   会让消息永远停在 outbox 里，前端永远看到 "sending"
/// - 即使有链路，writer_loop 可能被 bulk backpressure 挂起，ChatMessage 虽然已入
///   channel 但 writer_loop 还没写 → Ack 永远到不了 → outbox 永远不删
/// - 这个任务打破"无限等待"：对端**可达**却等不到 Ack 的条目 120s 判 failed；
///   对端**离线**的条目不是失败，保留到 `OUTBOX_OFFLINE_HOLD_MS`（上线补发承诺，
///   见 `db::should_fail_expired_outbox`）
pub fn spawn_outbox_sweeper(
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(OUTBOX_SWEEPER_INTERVAL_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                _ = tick.tick() => {}
            }

            let now = db::now_ms();
            let deadline_single = now - crate::db::OUTBOX_FAIL_DEADLINE_MS;
            let deadline_group = now - crate::db::GROUP_OUTBOX_FAIL_DEADLINE_MS;
            let deadline_file = now - crate::db::FILE_OUTBOX_FAIL_DEADLINE_MS;
            // 可达性快照：每 tick 取一次 links 键集，三条队列复用。
            // 此前每个候选行各抢一次 links 锁（500 离线行 = 500 次/tick，自审建议#4）；
            // 非空判定与 `has_link` 同口径（残留空 Vec 不算可达）。
            let reachable: std::collections::HashSet<String> = {
                let links = state.links.lock().await;
                links
                    .iter()
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            let is_reachable = |peer: &str| reachable.contains(peer);

            // --- 单聊 outbox ---
            // 候选行 = 过了 120s 仍未 Ack 的行；是否判 failed 由对端可达性决定：
            // 对端**离线**不是失败理由 —— 产品承诺「对方离线暂存、上线后自动补发」
            //（INV-P04），保留到 OUTBOX_OFFLINE_HOLD_MS 才当僵尸清理。
            // 2026-09-19 P0：此前离线 2 分钟即删行置 failed，离线补发被 sweeper 自己击穿。
            let candidates: Vec<(String, String, i64)> = {
                // 锁中毒也要可见地继续（into_inner）：这个清扫器是"无链路消息"唯一的终态出口，
                // `else { continue }` 静默跳过整个 tick = 消息永久停在 sending（审计 A3）。
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::list_expired_outbox(&dbc, deadline_single)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, mid, peer, created)| (mid, peer, created))
                    .collect()
            };
            for (msg_id, peer_id, created_at) in candidates {
                if !db::should_fail_expired_outbox(is_reachable(&peer_id), now - created_at) {
                    continue;
                }
                // 写库（置 failed + 删行）成功才 emit；锁只在这块作用域内持有，不跨 emit（审计 B1）。
                let finalized = {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    finalize_expired_message(&dbc, &msg_id, false)
                };
                if finalized {
                    let _ = state.app.emit("message-failed", &msg_id);
                } else {
                    // 不 emit、不删行 ⇒ 下一 tick 重试；留痕便于排障（绝不"界面说失败、行还在、又重发"）
                    state.logger.warn(
                        "outbox",
                        format!("过期单聊消息终态落库失败，保留 outbox 行下轮重试 msg={msg_id}"),
                    );
                }
            }

            // --- 群 outbox：行级分类 + msg 粒度放弃（2026-09-19 自审建议#5）---
            // 一条群消息按成员各一行；某成员离线 ⇒ 他的行保留（上线补发），
            // 只有**所有行都该放弃**时整条消息才置 failed。
            let group_rows: Vec<(String, String, String, i64)> = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::list_expired_group_outbox(&dbc, deadline_group).unwrap_or_default()
            };
            let mut per_msg: std::collections::HashMap<String, (String, Vec<(String, i64)>)> =
                std::collections::HashMap::new();
            for (msg_id, group_id, peer_id, created_at) in group_rows {
                per_msg
                    .entry(msg_id)
                    .or_insert_with(|| (group_id, Vec::new()))
                    .1
                    .push((peer_id, created_at));
            }
            for (msg_id, (_group_id, rows)) in per_msg {
                let all_give_up = rows.iter().all(|(peer, created)| {
                    // 群走**自己的**窗口（30min，见 `GROUP_OUTBOX_FAIL_DEADLINE_MS` 的理由）：
                    // 群 outbox 行是群消息唯一的补发载体，删早了就是永久丢。
                    db::should_fail_expired(
                        is_reachable(peer),
                        now - created,
                        crate::db::GROUP_OUTBOX_FAIL_DEADLINE_MS,
                        crate::db::OUTBOX_OFFLINE_HOLD_MS,
                    )
                });
                if !all_give_up {
                    continue;
                }
                let finalized = {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    finalize_expired_message(&dbc, &msg_id, true)
                };
                if finalized {
                    let _ = state.app.emit("message-failed", &msg_id);
                } else {
                    state.logger.warn(
                        "outbox",
                        format!("过期群消息终态落库失败，保留 outbox 行下轮重试 msg={msg_id}"),
                    );
                }
            }

            // --- 文件 outbox：与消息队列同一离线判据（窗口用文件自己的 30min）---
            // 自审建议#5：离线接收方的文件此前 30min 一律判 failed ——
            // 「关机一晚回来收不到大文件」与被修的 P0#2 同型。
            let expired_files: Vec<(String, String, Option<String>, i64)> = {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::list_expired_file_outbox(&dbc, deadline_file).unwrap_or_default()
            };
            for (transfer_id, peer_id, _group_id, created_at) in expired_files {
                if !db::should_fail_expired(
                    is_reachable(&peer_id),
                    now - created_at,
                    crate::db::FILE_OUTBOX_FAIL_DEADLINE_MS,
                    crate::db::OUTBOX_OFFLINE_HOLD_MS,
                ) {
                    continue;
                }
                let finalized = {
                    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    finalize_expired_file(&dbc, &transfer_id)
                };
                if finalized {
                    let _ = state.app.emit("file-failed", &transfer_id);
                } else {
                    state.logger.warn(
                        "outbox",
                        format!("过期文件终态落库失败，保留 file_outbox 行下轮重试 transfer={transfer_id}"),
                    );
                }
            }
        }
    })
}
