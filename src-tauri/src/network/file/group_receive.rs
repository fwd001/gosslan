// 群接收：一个群文件对**多个成员**各建一个接收器。
//
// 为什么单独一册：单聊那份状态按 `transfer_id` 一格，群那份是 `(transfer_id, member_id)` 一格，
// 取用/失败/接管三个动作都要按成员展开 —— 这是"同一条链的第二个受众维度"，最容易和单聊那册
// 互相漏改，所以自成一本。
//

// 恒等判据（与 transport 那五刀同一套）：`cargo test --features bluetooth --lib -- --list` 用例名差集 0 行、
//   `verify-guards.py --list` 每条锚点恰好命中一次（本模块的锚点由 runner 沿 include! 树自动解析 ⇒ 不动 Case 的 file=）、
//   clippy `-D warnings`、`cargo fmt --check`。
// ⚠️ 搬家同批必须做的两件事：`network/mod.rs::file_src_for_guards()` 登记本册（漏了=形状守卫看不见这段生产码，假绿），
//   以及 `docs/domains.data.mjs` 的 transport/file 领域 paths（漏了=判据 D 报无主文件）。

/// 群文件接收：创建 `.part` 接收状态。
/// 路径安全（safe_file_name + downloads 目录内 unique_path）与一对一相同；
/// 不写 file_transfers——群文件的进度/状态记录在 group_files /
/// group_file_recipients 表中。
pub fn begin_group_receive(
    state: &AppState,
    transfer_id: &str,
    peer_id: &str,
    name: &str,
    size: u64,
    file_key: [u8; 32],
    expected_sha256: String,
) -> Result<PathBuf, String> {
    let final_path = make_receiver(
        state,
        transfer_id,
        peer_id,
        name,
        size,
        file_key,
        expected_sha256.clone(),
        &state.group_file_receivers,
    )?;
    // 持久化本地路径到 file_transfers（复用现有表，无 schema 变更）：
    // 群文件气泡的打开/另存/历史加载经 transfer_id 关联到该真实本地路径。
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::upsert_transfer(
            &dbc,
            transfer_id,
            peer_id,
            name,
            size,
            "receive",
            "active",
            Some(final_path.to_string_lossy().as_ref()),
            0.0,
        )
        .ok();
        // 统一状态：群文件接收一开始就登记 Active（cid → 暂无 path）。
        // 中途失败由 fail_group_receive 标 Incomplete ⇒ 建链自动重取
        // （群成员也能做种，原发送方不在也能从别人取）。
        let now = db::now_ms();
        let rec = crate::content::model::TransferRecord {
            cid: expected_sha256.clone(),
            transfer_id: Some(transfer_id.to_string()),
            peer_id: peer_id.to_string(),
            group_id: None,
            name: name.to_string(),
            size,
            direction: crate::content::model::Direction::Receive,
            status: crate::content::model::TransferStatus::Active,
            received: 0,
            attempts: 0,
            next_attempt_at: 0,
            last_error: None,
            path: None,
            created_at: now,
            updated_at: now,
        };
        let _ = crate::content::store::upsert(&dbc, &rec);
    }
    Ok(final_path)
}

/// 群文件接收失败：删除 `.part` 并移除接收状态（不 rename、不标 done）。
pub fn fail_group_receive(state: &AppState, transfer_id: &str) {
    if let Some(r) = state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(transfer_id)
    {
        fail_taken_group_receive(state, &r);
    }
}

/// 对端确认一条链路都不剩时，**原子摘取**它名下全部群接收（判据与摘表在同一次持锁里完成）。
///
/// 为什么必须有这一份：`handle_peer_unlinked` 以前是"锁内 `collect()` 出 id、锁外逐个收尾"，
/// 而同一个文件里 `take_stalled_receive` / `take_stalled_group_receive` 的注释早就把那个形状判死过
/// —— 那两步之间完全可以挤进一个新 FileOffer（同一个 transfer_id 重建接收器），
/// 按 id 收尾就会把**正在正常收**的那一单判死。返回 (id, FileReceiver) 与那条家族同形状。
pub fn take_group_receives_for_peer(
    state: &AppState,
    peer_id: &str,
) -> Vec<(String, FileReceiver)> {
    let mut recv = state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = recv
        .iter()
        .filter(|(_, r)| r.peer_id == peer_id)
        .map(|(k, _)| k.clone())
        .collect();
    ids.into_iter()
        .filter_map(|k| recv.remove(&k).map(|r| (k, r)))
        .collect()
}

/// 静默群接收器的**原子**回收单位（理由同 `take_stalled_receive`：判据与摘表必须同一次持锁）。
pub fn take_stalled_group_receive(state: &AppState, now: i64) -> Option<(String, FileReceiver)> {
    let mut recv = state
        .group_file_receivers
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = recv
        .iter()
        .find(|(_, r)| receive_is_stale(now - r.fed_at_ms))
        .map(|(k, _)| k.clone())?;
    recv.remove(&key).map(|r| (key, r))
}

/// 摘表之后的群接收收尾（与单聊同一份口径：`.part` 保留、状态记 Incomplete 可重取）。
pub fn fail_taken_group_receive(state: &AppState, r: &FileReceiver) {
    // **保留 .part**（不删）：断点续传的前缀（群友从种子拉取时也走 resume_receive）。
    let _ = &r.tmp_path;
    // 统一状态：群文件中途失败/断链 ⇒ Incomplete（可恢复）⇒ 建链时按退避自动重取。
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let _ = crate::content::store::record_failure(
        &dbc,
        &r.expected_sha256,
        &r.peer_id,
        crate::content::model::Direction::Receive,
        crate::content::model::FailReason::Partial,
        db::now_ms(),
    );
}
