// 群治理事件：改名、移除成员、群主转让、成员退群
// 从 transport.rs 搬出：`include!` 回同一模块 ⇒ 模块路径、可见性、测试全名一字不变。
// 大文件拆分第三批；守卫视图与领域图的分册清单一同登记。

/// 处理群名变更广播：仅群创建者可发起，成员端校验后同步本地群名与会话标题。
async fn handle_group_rename(state: &Arc<AppState>, group_id: String, from: String, name: String) {
    if name.is_empty() || from == state.device_id {
        return;
    }
    let name: String = name.chars().take(MAX_GROUP_NAME_LEN).collect();
    // 只接受群创建者的改名
    let is_creator = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.creator == from)
            .unwrap_or(false)
    };
    if !is_creator {
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::rename_group(&dbc, &group_id, &name).ok();
    }
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理「成员被移出群」。
///
/// 两个分支，**此前只有第一个**：
/// 1. `to == 本机`：我本人被移出 → 清理本地群 + 会话 + 群密钥；
/// 2. `to != 本机`：别人被移出 → 同步本地成员表 + 清掉指向他的待补发群消息 +
///    落一条群内系统消息，让群里的人都知道。
async fn handle_group_member_removed(
    state: &Arc<AppState>,
    group_id: String,
    from: String,
    to: String,
) {
    if from == state.device_id {
        return; // 本机发起的移人，本地已处理（含系统消息）
    }
    // 只接受**群创建者**发起的移人（防成员互踢）
    let is_creator = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::get_group(&dbc, &group_id)
            .map(|g| g.creator == from)
            .unwrap_or(false)
    };
    match member_removed_action(false, is_creator, to == state.device_id) {
        MemberRemovedAction::Ignore => return,
        // ---- ① 我本人被移出 ----
        MemberRemovedAction::RemoveSelf => {
            {
                let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
                db::delete_group(&dbc, &group_id).ok();
                let _ = dbc.execute(
                    "DELETE FROM settings WHERE key = ?1",
                    params![format!("gk:{group_id}")],
                );
            }
            state
                .group_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&group_id);
            let _ = state.app.emit("group-member-removed", &group_id);
            let _ = state.app.emit("groups-updated", &group_id);
            return;
        }
        // ---- ② 别人被移出：同步成员表 + 群内系统消息 ----
        MemberRemovedAction::RemoveOther => {}
    }

    let changed = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) if g.members.contains(&to) => {
                // 他已经不是成员了：指向他的待补发群消息也不该再投递
                db::delete_group_outbox_for_peer_in_group(&dbc, &group_id, &to).ok();
                db::remove_group_member(&dbc, &group_id, &to).is_ok()
            }
            _ => false,
        }
    };
    if !changed {
        return; // 幂等：已经不在成员表里就不重复插系统消息
    }
    let name = resolve_nickname(state, &to);
    insert_group_system_message(state, &group_id, &group_member_removed_text(state, &name));
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理「群主转让」：只接受**当前创建者**发起、且新群主确实是群成员的转让。
/// 广播给全体成员，因此新任群主自己也会收到并更新本地记录。
async fn handle_group_creator_changed(
    state: &Arc<AppState>,
    group_id: String,
    from: String,
    to: String,
) {
    if from == state.device_id {
        return; // 本机发起的转让，本地已更新
    }
    let ok = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) => g.creator == from && g.members.contains(&to),
            None => false,
        }
    };
    if !ok {
        return;
    }
    {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_group_creator(&dbc, &group_id, &to).ok();
    }
    // 群内提示：转让成功了但群里没人知道，等于"群主静默换人"（见 group_creator_changed_text）。
    // 幂等由上面的 `ok` 判据保证：第二次收到同一条转让时 `g.creator == from` 已不成立。
    let name = resolve_nickname(state, &to);
    insert_group_system_message(state, &group_id, &group_creator_changed_text(state, &name));
    let _ = state.app.emit("groups-updated", &group_id);
}

/// 处理「成员主动退群」：把 `from` 从本地成员表移除（幂等）。
/// 群主不允许直接退群（须先转让），因此忽略「群主退出」这类异常/伪造消息。
async fn handle_group_member_left(state: &Arc<AppState>, group_id: String, from: String) {
    if from == state.device_id {
        return; // 本机发起的退群，本地已处理
    }
    let changed = {
        let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db::get_group(&dbc, &group_id) {
            Some(g) if g.creator != from && g.members.contains(&from) => {
                // 他已经退了：指向他的待补发群消息不该再投递
                db::delete_group_outbox_for_peer_in_group(&dbc, &group_id, &from).ok();
                db::remove_group_member(&dbc, &group_id, &from).is_ok()
            }
            _ => false,
        }
    };
    if changed {
        // 群内系统消息 —— 此前成员表会同步，但群里看不到任何提示
        let name = resolve_nickname(state, &from);
        insert_group_system_message(state, &group_id, &group_member_left_text(state, &name));
        let _ = state.app.emit("groups-updated", &group_id);
    }
}
