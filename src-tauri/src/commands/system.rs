// 职责边界：
// - 本机信息采集（设备 ID、系统版本、网卡）
// - 虚拟 IP 判断（is_virtual_ip，被 transport/discovery 引用）
// ---------------- 本机信息与配置 ----------------

#[tauri::command(async)]
pub fn get_device_info(state: State<'_, Arc<AppState>>) -> DeviceInfo {
    let s = state.inner();
    DeviceInfo {
        device_id: s.device_id.clone(),
        nickname: s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        avatar: s.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        device_type: crate::protocol::current_device_type().to_string(),
        tcp_port: s.tcp_port,
        online: s
            .network
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some(),
        x25519_pubkey: s.identity.x25519_public_b64(),
        ed25519_pubkey: s.identity.ed25519_public_b64(),
    }
}

/// 头像 data URL 解码后字节数；非法 base64 返回 usize::MAX（视为超限拒绝）。
fn avatar_decoded_len(data_url: &str) -> usize {
    let payload = data_url.split_once(',').map(|(_, p)| p).unwrap_or(data_url);
    STANDARD
        .decode(payload)
        .map(|b| b.len())
        .unwrap_or(usize::MAX)
}

#[tauri::command]
pub async fn update_profile(
    state: State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    nickname: String,
    avatar: Option<String>,
) -> Result<DeviceInfo, String> {
    let s = state.inner();
    // 昵称长度保护：按字符截断（UTF-8 安全）
    let nickname: String = nickname.chars().take(MAX_NICKNAME_LEN).collect();
    // 头像大小兜底：超限直接拒绝，防止超大 base64 落库 / 撑爆 UDP 广播
    if let Some(a) = &avatar {
        if avatar_decoded_len(a) > MAX_AVATAR_BYTES {
            return Err("头像过大，请压缩到 2MB 以内".to_string());
        }
    }
    {
        let dbc = s.db.lock().unwrap_or_else(|e| e.into_inner());
        db::set_setting(&dbc, "nickname", &nickname).map_err(|e| e.to_string())?;
        if let Some(a) = &avatar {
            db::set_setting(&dbc, "avatar", a).map_err(|e| e.to_string())?;
        }
    }
    *s.nickname.lock().unwrap_or_else(|e| e.into_inner()) = nickname.clone();
    *s.avatar.lock().unwrap_or_else(|e| e.into_inner()) = avatar.clone();

    let msg = Message::UserInfo {
        device_id: s.device_id.clone(),
        nickname,
        avatar,
        device_type: crate::protocol::current_device_type().to_string(),
    };
    // ⚠️ **锁内只做「取 + 克隆」，绝不 await**（与 `transport.rs` 心跳发送同一纪律）。
    //
    // 原先`let links = ...lock().await` 后就地 `tx.send().await`：这些都是**有界**队列
    // （1024），对端僵死（半开 TCP / 休眠 / 写缓冲满）时 `send().await` 会一直挂起，
    // 而它**握着全局 links 锁** ⇒ 所有 try_send、心跳、get_peers、mark_peer_offline、
    // teardown_link 以及看门狗全部阻塞。看门狗恰恰是唯一能发 cancel 拆掉那条卡死连接、
    // 让队列排空的机制 —— 它被同一把锁挡住，形成自锁死循环，只能靠用户手动重开局域网。
    let targets = {
        let links = s.links.lock().await;
        links
            .values()
            .flatten()
            // 队列选择唯一来源 = dispatch 分类表（大头像 → Low：2MB 头像在 BLE 上要分
            // 上千片，绝不能堵住聊天/好友请求的道；小头像 → Normal：资料变更要立刻可见）。
            .map(|link| {
                use crate::network::dispatch::MessagePriority::*;
                match crate::network::dispatch::message_priority(&msg) {
                    High => link.high.clone(),
                    Normal => link.normal.clone(),
                    Low => link.low.clone(),
                }
            })
            .collect::<Vec<_>>()
    };
    for tx in &targets {
        let _ = tx.send(msg.clone()).await;
    }

    // 昵称/头像变更：另一个窗口的资料区要跟着刷新。
    // 这两个键不在 `Settings` 形状里（它们是"资料"），所以 patch 里不放值 ——
    // 接收方看到键名会自己定向重拉一次 device_info（见前端 applySettingsPatch）。
    state.notify_settings_changed(&["nickname", "avatar"], Some(window.label()), json!({}));
    Ok(DeviceInfo {
        device_id: s.device_id.clone(),
        nickname: s.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        avatar: s.avatar.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        device_type: crate::protocol::current_device_type().to_string(),
        tcp_port: s.tcp_port,
        online: s
            .network
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some(),
        x25519_pubkey: s.identity.x25519_public_b64(),
        ed25519_pubkey: s.identity.ed25519_public_b64(),
    })
}

/// 判断 IPv4 是否为常见 VPN / Clash / 虚拟网卡地址段。
///
/// 保守策略：只过滤**几乎不可能出现在真实局域网**的地址段；
/// 10.x.x.x 等模糊段不纳入过滤（真实 LAN 广泛使用 10/8）。
pub fn is_virtual_ip(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    // 198.18.0.0/15 — Clash / sing-box / v2ray fake-ip 段
    (o[0] == 198 && (o[1] == 18 || o[1] == 19))
    // 100.64.0.0/10 — WireGuard / CGNAT / Tailscale 常用段
    || (o[0] == 100 && o[1] >= 64 && o[1] <= 127)
    // 169.254.0.0/16 — link-local
    || (o[0] == 169 && o[1] == 254)
}

#[tauri::command(async)]
pub fn list_interfaces() -> Vec<InterfaceInfo> {
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
                // is_lan：有广播地址（真实 LAN 的标志）+ 非 link-local + 非 VPN 地址段
                let has_broadcast = v4.broadcast.is_some();
                let is_link_local = ip.octets()[0] == 169 && ip.octets()[1] == 254;
                let not_vpn = !is_virtual_ip(&ip);
                let is_lan = has_broadcast && !is_link_local && not_vpn;
                out.push(InterfaceInfo {
                    name: i.name.clone(),
                    ip: ip.to_string(),
                    is_lan,
                });
            }
        }
    }
    out.sort_by(|a, b| a.ip.cmp(&b.ip));
    out
}
