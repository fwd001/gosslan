// 职责边界：
// - 中继端点配置（RoutedEndpoint 相关命令）
// ---------------- 跨子网（Routed）端点配置 ----------------

/// 校验并**规范化**手动配置的 Routed 端点地址，返回 `ip:port`。
///
/// 两种输入都接受：
/// - `100.64.0.1:60002`（显式端口，对端用了 `--instance` 时需要）
/// - `100.64.0.1`（省略端口 → 用标准 [`TCP_PORT`]）
///
/// 允许省略端口是「少配置」的一部分：端口是内部实现细节，默认单实例场景下用户
/// 没有理由需要知道它，更不该因为漏写端口而被拒绝。
///
/// 只接受 **IPv4**：TCP 监听侧绑的是 `Ipv4Addr`（`network::transport::spawn`），
/// IPv6 端点即使拨出去也连不上。在这里当场拒绝，好过「存下来了但永远连不上」
/// —— 后者对用户完全不可见（配置成功、日志无错、就是没反应）。
fn normalize_routed_address(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    // 解析（含「裸 IP 补标准端口」）由 `parse_endpoint_addr` 单点负责 ——
    // 拨号侧走的是同一个实现，避免两条路径行为不一致。
    let Some(addr) = parse_endpoint_addr(trimmed) else {
        return Err(format!("地址格式应为 ip 或 ip:port，收到：{trimmed}"));
    };
    if addr.is_ipv6() {
        return Err(
            "暂不支持 IPv6 地址（当前 TCP 监听仅 IPv4）。请填写 IPv4，例如 100.64.0.1".to_string(),
        );
    }
    // 规范化后再存储：add / remove 比较的是同一个字符串，避免「加进去了却删不掉」
    Ok(addr.to_string())
}

/// 列出手动配置的跨子网端点（Tailscale / VPN / 跨网段）。
#[tauri::command(async)]
pub fn list_routed_endpoints(state: tauri::State<'_, Arc<AppState>>) -> Vec<RoutedEndpoint> {
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    parse_endpoints(&db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default())
}

/// 添加一个跨子网端点。
///
/// `device_id` **可省略**（传 `null` 或空串都当作未指定）：
/// - 提供时：语义是「连接这个**已知**节点」，链路 key 直接用它，行为与历史一致；
/// - 省略时：身份由 TCP 握手学来（§8：`IP:PORT → TCP → Hello → Node ID → Identity`），
///   用户只需要知道对方地址 —— 这才是「少配置」。
///
/// 地址接受 `ip` 或 `ip:port`（省略端口按标准 [`TCP_PORT`] 补全），目前仅 IPv4：
/// TCP 监听侧绑的是 `Ipv4Addr`，IPv6 端点拨出去也连不上。
#[tauri::command(async)]
pub fn add_routed_endpoint(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: Option<String>,
    address: String,
) -> Result<Vec<RoutedEndpoint>, String> {
    // 空串等同「未指定」：UI 上的输入框没填时通常会传空串，不该存下一个没意义的值。
    let device_id = device_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let address = normalize_routed_address(&address)?;
    let candidate = RoutedEndpoint::new(device_id, address);

    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list =
        parse_endpoints(&db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default());
    // 同一个**地址**不重复添加，无论是否带 device_id —— 一个地址只对应一个端点。
    if !list.iter().any(|e| e.address == candidate.address) {
        list.push(candidate);
    }
    db::set_setting(&dbc, ROUTED_ENDPOINTS_KEY, &encode_endpoints(&list))
        .map_err(|e| format!("保存失败: {e}"))?;
    Ok(list)
}

/// 移除一个跨子网端点（只按**地址**匹配）。
///
/// 地址是端点的唯一标识：`device_id` 可以省略，用它当判据会让「只填地址添加、
/// 带指纹删除」匹配不上。地址先规范化（与 `add` 存进去的形式一致）。
///
/// **比对时也把库里已存的那条再规范化一次**，兜住「历史脏数据」（比如老版本直接
/// 写 SQLite 没经过 `add` 的裸 IP 无端口），否则会出现「列表里看得见但删不掉」——
/// 用户的真实反馈：UI 看着有 `100.101.221.60`，删的时候 normalize 成
/// `100.101.221.60:59992`，而库里存的就是裸 `100.101.221.60`，字符串不相等。
#[tauri::command(async)]
pub fn remove_routed_endpoint(
    state: tauri::State<'_, Arc<AppState>>,
    address: String,
) -> Result<Vec<RoutedEndpoint>, String> {
    let address = normalize_routed_address(&address)?;
    let dbc = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut list =
        parse_endpoints(&db::get_setting(&dbc, ROUTED_ENDPOINTS_KEY).unwrap_or_default());
    let before = list.len();
    list.retain(|e| {
        // 库里的历史脏数据可能未归一化（裸 IP / 裸 IP 带非标准端口），按当前规则再过一遍
        // 归一化后比较。归一化失败的条目（语法错乱）保守地按字符串相等判，免得误删。
        let stored_norm =
            normalize_routed_address(&e.address).unwrap_or_else(|_| e.address.clone());
        stored_norm != address
    });
    if list.len() != before {
        db::set_setting(&dbc, ROUTED_ENDPOINTS_KEY, &encode_endpoints(&list))
            .map_err(|e| format!("保存失败: {e}"))?;
    }
    Ok(list)
}
