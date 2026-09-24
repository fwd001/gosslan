// 职责边界：
// - logs.rs 的源码守卫测试
// - 辅助窗口几何不变式（fit_inside_main / centered）
#[cfg(test)]
mod tests {
    use super::{friend_is_online, FRIEND_ONLINE_GRACE_MS};

    /// 独立窗口的几何不变式：**永远不比主窗口大**、在主窗口内**居中**、
    /// 且最小尺寸不会把窗口顶回比目标更大。
    ///
    /// 为什么必须钉住：用户 2026-09-16 报的正是这个（「新窗口没居中，而且比主窗口还大很多」），
    /// 而这几条都是**几何量**—— 肉眼量不准，也只能靠算。
    #[cfg(desktop)]
    #[test]
    fn aux_window_fits_inside_the_main_window_and_is_centered() {
        use super::{fit_aux_window, AUX_WINDOW_MARGIN};
        // 四组真实参数：设置 780×600（最小 560×420）、日志 760×560（最小 420×320）、
        // 群任务 780×620（最小 360×420）、外链 1000×720（最小 420×320）。
        let cases = [
            ((780.0, 600.0), (560.0, 420.0)),
            ((760.0, 560.0), (420.0, 320.0)),
            ((780.0, 620.0), (360.0, 420.0)),
            ((1000.0, 720.0), (420.0, 320.0)),
        ];
        // 主窗口尺寸（物理像素）：默认 1000×680@100%、拉小、很小、放大、以及 125%/150% 缩放。
        let mains = [
            ((1000u32, 680u32), 1.0),
            ((900, 700), 1.0),
            ((700, 500), 1.0),
            ((400, 300), 1.0),
            ((1600, 1000), 1.0),
            ((1500, 1020), 1.25),
            ((1700, 1200), 1.5),
        ];
        // 主窗口左上角：主屏 (0,0)、右侧副屏 (1920,0)、**左侧**副屏（负坐标）——多屏必须都对。
        let origins = [(0i32, 0i32), (1920, 0), (-1920, 100), (100, 50)];
        for (ideal, min) in cases {
            for (size, scale) in mains {
                for main_pos in origins {
                    let g = fit_aux_window(size, main_pos, scale, ideal, min);
                    assert!(
                        g.size.0 <= size.0 && g.size.1 <= size.1,
                        "子窗口 {:?} 不能比主窗口 {:?} 大（ideal={ideal:?} scale={scale}）",
                        g.size,
                        size
                    );
                    assert!(
                        g.min.0 <= g.size.0 && g.min.1 <= g.size.1,
                        "最小尺寸 {:?} 不能大于实际尺寸 {:?} —— 否则系统会把窗口顶回去，缩小等于白做",
                        g.min,
                        g.size
                    );
                    // 居中：按真实外框算，左右/上下留边相等，且整体在主窗口内
                    let outer = (g.size.0 + 16, g.size.1 + 39); // 假装有 16/39 的边框与标题栏
                    let (x, y) = g.centered_pos(outer);
                    let left = x - main_pos.0;
                    let right = main_pos.0 + size.0 as i32 - (x + outer.0 as i32);
                    assert!(
                        (left - right).abs() <= 1,
                        "水平未居中：左 {left} 右 {right}"
                    );
                    let top = y - main_pos.1;
                    let bottom = main_pos.1 + size.1 as i32 - (y + outer.1 as i32);
                    assert!(
                        (top - bottom).abs() <= 1,
                        "垂直未居中：上 {top} 下 {bottom}"
                    );
                    let (ex, ey) = g.centered_pos(g.size);
                    assert!(
                        ex >= main_pos.0 && ey >= main_pos.1,
                        "不能跑到主窗口左上角之外"
                    );
                }
            }
        }
        // 装得下时必须**保持设计尺寸 × 主窗口缩放**（不能因为主窗口大就无限放大）
        let g = fit_aux_window((1600, 1000), (0, 0), 1.0, (780.0, 600.0), (560.0, 420.0));
        assert_eq!(g.size, (780, 600), "主窗口够大时应保持设计尺寸");
        let g = fit_aux_window((3000, 2000), (0, 0), 1.5, (780.0, 600.0), (560.0, 420.0));
        assert_eq!(
            g.size,
            (1170, 900),
            "150% 屏上 780×600 逻辑 = 1170×900 物理"
        );
        assert_eq!(g.min, (840, 630), "最小尺寸也要按同一缩放换成物理值");
        // 装不下时把边距留够（24 逻辑像素 × 缩放）
        let g = fit_aux_window((700, 500), (0, 0), 1.0, (780.0, 600.0), (560.0, 420.0));
        assert_eq!(
            g.size,
            (
                700 - 2 * AUX_WINDOW_MARGIN as u32,
                500 - 2 * AUX_WINDOW_MARGIN as u32
            )
        );
        let g = fit_aux_window((800, 600), (0, 0), 2.0, (780.0, 600.0), (560.0, 420.0));
        assert_eq!(
            g.size,
            (
                800 - 2 * (AUX_WINDOW_MARGIN * 2.0) as u32,
                600 - 2 * (AUX_WINDOW_MARGIN * 2.0) as u32
            )
        );
    }

    /// 还原"用户上次拉的尺寸"时的钳制（配合 `restore_aux_window_size`）。
    ///
    /// 用户 2026-09-21：「任务新窗口也太大了吧，另外，这个窗口都没记住用户的尺寸吗？」
    /// 这两句是同一处逻辑的两面：还原得太宽松 ⇒ 状态文件里的历史大尺寸把窗口顶回去；
    /// 不还原 ⇒ 用户拉过的大小白拉。所以判据是**双向**的，缺一条都会退化成用户报过的毛病。
    #[cfg(desktop)]
    #[test]
    fn restored_aux_window_size_is_clamped_between_min_and_design() {
        use super::clamp_aux_size;
        let min = (360, 420);
        let design = (780, 620);
        // 用户拉小过 ⇒ 记住（这是"记住尺寸"的正面用例；之前的行为是把它丢回设计尺寸）
        assert_eq!(clamp_aux_size((600, 500), min, design), (600, 500));
        // 用户拉大过 / 老版本落过更大的盘 ⇒ 夹回设计尺寸
        assert_eq!(clamp_aux_size((1080, 780), min, design), design);
        // 只一边越界 ⇒ 只夹那一边（不要把没越界的一边也拖走）
        assert_eq!(clamp_aux_size((1080, 500), min, design), (780, 500));
        // 比最小还小 ⇒ 抬到最小（否则系统会把窗口顶回去，实际尺寸与状态对不上）
        assert_eq!(clamp_aux_size((100, 100), min, design), min);
        // 防御：min > max（几何不变式被违反）时不能 panic —— 窗口打不开比尺寸不合意严重得多
        assert_eq!(clamp_aux_size((500, 500), (900, 900), (780, 620)), (900, 900));
    }

    /// `generate_handler!` 里列出的命令在**移动端也必须存在**。
    ///
    /// 为什么必须守：`lib.rs` 的 `generate_handler!` 是**无条件**列出命令名的，而
    /// `#[tauri::command]` 生成的包装宏跟着函数一起被 `#[cfg(desktop)]` 裁掉 ——
    /// 于是"桌面专属命令 + 无条件列出"就等于 **Android/iOS 目标编译失败（E0433）**。
    /// 2026-09-12 本轮就是这样踩到的：四个设置/日志窗口命令让移动端整包编不出来，
    /// 而当时**没有任何守门**（`cargo test --lib` 只按桌面口径检查）。
    ///
    /// 判据：凡是被列出的、且定义处带 `#[cfg(desktop)]` 的命令，必须同时有
    /// `#[cfg(mobile)]` 的桩（沿用 `focus_window` 的范式）。
    #[test]
    fn every_handler_command_exists_for_mobile() {
        // 本文件自身的源码（用于查 `#[cfg(desktop)]` / `#[cfg(mobile)]` 成对性）
        let src = include_str!("../commands.rs");
        let lib_src = include_str!("../lib.rs");
        let start = lib_src
            .find("generate_handler![")
            .expect("lib.rs 应有 generate_handler!");
        let rest = &lib_src[start..];
        let end = rest.find(']').expect("generate_handler! 应有收尾方括号");
        let listed: Vec<&str> = rest[..end]
            .lines()
            .filter_map(|l| l.trim().strip_prefix("commands::"))
            .map(|s| s.trim().trim_end_matches(','))
            .filter(|s| !s.is_empty())
            .collect();
        assert!(
            listed.len() > 50,
            "命令列表解析异常（只解析出 {} 条）—— 守卫会变成空转",
            listed.len()
        );
        for name in listed {
            let desktop_only = src.contains(&format!(
                "#[cfg(desktop)]\n#[tauri::command]\npub fn {name}("
            ));
            if !desktop_only {
                continue; // 非桌面专属 ⇒ 移动端本来就有
            }
            let has_mobile = src.contains(&format!(
                "#[cfg(mobile)]\n#[tauri::command]\npub fn {name}("
            ));
            assert!(
                has_mobile,
                "命令 `{name}` 是桌面专属（#[cfg(desktop)]）却被 generate_handler! 无条件列出，\n\
                 且没有 #[cfg(mobile)] 桩 ⇒ Android/iOS 目标会编译失败（E0433）。\n\
                 修法：照着 `focus_window` 加一个移动端桩（打开类返回明确 Err、关闭类 Ok(())）。"
            );
        }
    }

    use super::{
        check_message_content, decode_outgoing_image, group_file_progress_from, image_extension,
        normalize_routed_address, MAX_MESSAGE_LEN, MAX_OUTGOING_IMAGE_BYTES,
    };
    use crate::state::GroupFileRecipient;
    use std::collections::HashSet;

    fn recipient(id: &str, progress: f64) -> GroupFileRecipient {
        GroupFileRecipient {
            recipient_id: id.to_string(),
            status: if progress >= 1.0 {
                "completed"
            } else {
                "sending"
            }
            .to_string(),
            progress,
            updated_at: 0,
        }
    }

    fn online(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// 好友在线判据：**最近见过 或 有活链路**；只"在节点表里"不算在线。
    ///
    /// 两条真机结论一起钉住：① 节点条目在掉线后会**保留**（`mark_peer_offline` 不再删，
    /// 否则 Mac 的「添加好友」列表里对端只闪一下）；② 因此"在表里"绝不能等价于"在线"，
    /// 否则就是复核抓到过的"连过又掉线 ⇒ 永久在线"。
    #[test]
    fn friend_online_needs_freshness_or_an_active_link() {
        let now = 1_000_000_000_000_i64;
        // 刚见过（节点条目保留着）⇒ 在线
        assert!(friend_is_online(now - 1_000, now, false));
        // 15s 边界内 ⇒ 在线（announce 5s 一轮 + 抖动，容忍丢一两轮）
        assert!(friend_is_online(now - FRIEND_ONLINE_GRACE_MS, now, false));
        // 超过窗口且没有链路 ⇒ 离线（就是"连过又掉线"的那个场景）
        assert!(!friend_is_online(
            now - FRIEND_ONLINE_GRACE_MS - 1,
            now,
            false
        ));
        assert!(!friend_is_online(0, now, false));
        // 有活链路 ⇒ 恒在线（哪怕很久没有 announce：跨子网中继场景）
        assert!(friend_is_online(0, now, true));
    }

    /// 用户口径：进度条只按**发送时在线的成员**算。
    /// 复现场景：群 3 人，1 人离线；两个在线成员都收完 ⇒ 进度必须是 **100%**，
    /// 而不是被离线成员的 0 拖成 50%（这正是 `6e9b96e` 那轮用户反馈的「卡在 50%」）。
    #[test]
    fn group_file_progress_ignores_offline_members() {
        let rs = vec![
            recipient("a", 1.0),
            recipient("b", 1.0),
            recipient("offline", 0.0),
        ];
        let snap = online(&["a", "b"]);
        assert_eq!(group_file_progress_from(&rs, Some(&snap), 0.0), 1.0);
        // 对照组：不传快照（全体口径）时，同一组数据只有 2/3 —— 证明差异来自分母而非巧合。
        let all = group_file_progress_from(&rs, None, 0.0);
        assert!(
            (all - 2.0 / 3.0).abs() < 1e-9,
            "全体口径应为 2/3，实际 {all}"
        );
    }

    /// 离线成员之后上线补发**不得回退**进度条：分母是冻结快照，与他的进度无关。
    #[test]
    fn late_online_member_does_not_regress_progress() {
        let snap = online(&["a", "b"]);
        let done = vec![
            recipient("a", 1.0),
            recipient("b", 1.0),
            recipient("offline", 0.0),
        ];
        assert_eq!(group_file_progress_from(&done, Some(&snap), 0.0), 1.0);
        // 离线者开始补发（进度 0.5）——仍在快照外，不影响结果
        let catching_up = vec![
            recipient("a", 1.0),
            recipient("b", 1.0),
            recipient("offline", 0.5),
        ];
        assert_eq!(
            group_file_progress_from(&catching_up, Some(&snap), 0.0),
            1.0
        );
    }

    /// 在线成员未全部完成时，进度是在线成员的平均值（不是 max，也不是全体）。
    #[test]
    fn group_file_progress_averages_online_members() {
        let rs = vec![
            recipient("a", 1.0),
            recipient("b", 0.0),
            recipient("offline", 1.0),
        ];
        let snap = online(&["a", "b"]);
        assert_eq!(group_file_progress_from(&rs, Some(&snap), 0.0), 0.5);
    }

    /// 发送时无人在线 ⇒ 进度恒 0（没有「在线成员都收到了」这件事）。
    #[test]
    fn group_file_progress_is_zero_when_nobody_online_at_send() {
        let rs = vec![recipient("a", 0.0), recipient("b", 0.0)];
        let empty = HashSet::new();
        assert_eq!(group_file_progress_from(&rs, Some(&empty), 0.0), 0.0);
        // 即便调用方传了 fallback（本连接字节进度），空快照也必须压到 0 ——
        // 否则「发给一个刚好在线的成员」会看起来像全群都完成了。
        assert_eq!(group_file_progress_from(&rs, Some(&empty), 0.7), 0.0);
    }

    /// 快照丢失（重启后内存态清空）时退回全体口径；空集合/越界 fallback 都要夹紧。
    #[test]
    fn group_file_progress_fallback_and_clamp() {
        let rs = vec![recipient("a", 0.5), recipient("b", 0.5)];
        assert_eq!(group_file_progress_from(&rs, None, 0.0), 0.5);
        assert_eq!(
            group_file_progress_from(&[], None, 0.3),
            0.3,
            "无 recipient 时用 fallback"
        );
        assert_eq!(
            group_file_progress_from(&rs, None, 2.0),
            1.0,
            "fallback 超界要夹到 1"
        );
        assert_eq!(
            group_file_progress_from(&rs, None, -1.0),
            0.5,
            "负 fallback 不得把进度拉成负"
        );
    }

    /// Routed 端点地址：`ip` 与 `ip:port` 两种写法都收（省略端口补标准 `TCP_PORT`），
    /// 并在**存储前规范化**——这样 add 与 remove 比较的是同一个字符串，
    /// 不会出现「加进去了却删不掉」。IPv6 当场明确拒绝。
    #[test]
    fn routed_endpoint_address_is_normalized_to_ipv4_socket() {
        // 省略端口 → 补标准端口（端口是内部细节，用户不必知道）
        assert_eq!(
            normalize_routed_address("100.64.0.1").unwrap(),
            format!("100.64.0.1:{}", crate::protocol::TCP_PORT)
        );
        // 显式端口 → 原样保留（对端用了 --instance 的场景）
        assert_eq!(
            normalize_routed_address("100.64.0.1:60002").unwrap(),
            "100.64.0.1:60002"
        );
        // 前后空白容错（从聊天窗口复制地址常带空格）
        assert_eq!(
            normalize_routed_address("  192.168.1.5  ").unwrap(),
            format!("192.168.1.5:{}", crate::protocol::TCP_PORT)
        );

        // 格式错误
        assert!(normalize_routed_address("100.64.0.1:").is_err());
        assert!(normalize_routed_address("garbage").is_err());
        assert!(normalize_routed_address("").is_err());

        // IPv6：拒绝，且错误信息要能给出可操作的指引
        let err = normalize_routed_address("[fd7a:115c:a1e0::1]:59992").unwrap_err();
        assert!(err.contains("IPv6"), "错误信息应点明 IPv6：{err}");
        assert!(normalize_routed_address("fd7a:115c:a1e0::1").is_err());
    }
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    // ---------- 单条消息长度上限：超限报错，绝不静默截断 ----------

    #[test]
    fn message_content_within_limit_is_returned_unchanged() {
        let ok = "a".repeat(MAX_MESSAGE_LEN);
        let got = check_message_content(ok.clone()).unwrap();
        assert_eq!(got, ok, "恰好到上限必须原样通过，不能被改动");
    }

    #[test]
    fn message_content_over_limit_is_rejected_not_truncated() {
        let over = "b".repeat(MAX_MESSAGE_LEN + 1);
        let err = check_message_content(over).unwrap_err();
        assert!(err.contains("过长"), "错误文案应说明「过长」：{err}");
        assert!(
            err.contains(&(MAX_MESSAGE_LEN + 1).to_string()),
            "应告知实际长度，便于用户判断如何分段：{err}"
        );
    }

    #[test]
    fn message_length_is_counted_in_chars_not_bytes() {
        // 按字符计数：5 万汉字（15 万字节）合法，5 万零 1 个汉字才拒绝。
        // 若误按字节计数，5 万汉字会被判超限，正常长文就发不出去了。
        let cjk = "中".repeat(MAX_MESSAGE_LEN);
        assert!(check_message_content(cjk).is_ok());

        let cjk_over = "中".repeat(MAX_MESSAGE_LEN + 1);
        assert!(check_message_content(cjk_over).is_err());
    }

    #[test]
    fn empty_message_content_is_allowed_at_this_layer() {
        // 空内容的拦截属于上层（发送键置灰），这里不做业务判断，避免产生第二处规则。
        assert!(check_message_content(String::new()).is_ok());
    }

    #[test]
    fn image_extension_maps_common_mimes() {
        assert_eq!(image_extension("image/png"), Some("png"));
        assert_eq!(image_extension("image/jpeg"), Some("jpg"));
        assert_eq!(image_extension("image/gif"), Some("gif"));
        assert_eq!(image_extension("image/webp"), Some("webp"));
        assert_eq!(image_extension("IMAGE/PNG"), Some("png"));
        assert_eq!(image_extension("image/bmp"), None);
        assert_eq!(image_extension("text/plain"), None);
    }

    fn data_url(mime: &str, bytes: &[u8]) -> String {
        format!("data:{};base64,{}", mime, STANDARD.encode(bytes))
    }

    #[test]
    fn decode_accepts_png_jpeg_gif_webp() {
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            let expected_ext = image_extension(mime).unwrap();
            let url = data_url(mime, b"fake-image-body");
            let (ext, bytes) = decode_outgoing_image(&url).unwrap();
            assert_eq!(ext, expected_ext);
            assert_eq!(bytes, b"fake-image-body");
        }
    }

    #[test]
    fn decode_rejects_non_image_mime() {
        let url = data_url("text/plain", b"hello");
        assert!(decode_outgoing_image(&url).unwrap_err().contains("图片"));
    }

    #[test]
    fn decode_rejects_unsupported_image_mime() {
        let url = data_url("image/bmp", b"hello");
        assert!(decode_outgoing_image(&url).unwrap_err().contains("不支持"));
    }

    #[test]
    fn decode_rejects_invalid_base64() {
        let url = "data:image/png;base64,!!!";
        assert!(decode_outgoing_image(url).unwrap_err().contains("解码失败"));
    }

    #[test]
    fn decode_rejects_malformed_data_url() {
        assert!(decode_outgoing_image("not-a-data-url").is_err());
        assert!(decode_outgoing_image("data:image/png").is_err());
        assert!(decode_outgoing_image("data:image/png,raw").is_err());
    }

    #[test]
    fn decode_rejects_empty_image() {
        let url = data_url("image/png", b"");
        assert!(decode_outgoing_image(&url).unwrap_err().contains("为空"));
    }

    #[test]
    fn decode_rejects_over_byte_limit() {
        let big = vec![0u8; (MAX_OUTGOING_IMAGE_BYTES + 1) as usize];
        let url = data_url("image/png", &big);
        let err = decode_outgoing_image(&url).unwrap_err();
        assert!(err.contains("过大"));
        assert!(err.contains(&MAX_OUTGOING_IMAGE_BYTES.to_string()));
    }

    /// **设置变更的补丁必须"只带变了的键、且键名与前端 camelCase 一致"**。
    ///
    /// 这是 `settings-changed` 从"无载荷 + 全量重拉"改成"带补丁 + 零 IPC"的地基：
    /// 键名写错（例如 `theme_color` 而不是 `themeColor`）不会报错，只会让另一个窗口
    /// **静默地不更新**（用户看到的就是"设置里改了、主界面没变"那类幽灵问题）。
    #[test]
    fn settings_patch_carries_only_changed_keys_with_camel_case_names() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        crate::db::set_setting(&conn, "theme_color", "#123456").unwrap();
        crate::db::set_setting(&conn, "language", "en-US").unwrap();
        crate::db::set_setting(&conn, "dark_mode", "1").unwrap();

        let patch = super::settings_patch_values(&conn, &["themeColor", "language", "darkMode"]);
        let obj = patch.as_object().expect("补丁必须是 JSON 对象");
        assert_eq!(obj.len(), 3, "只应包含被点名的键");
        assert_eq!(obj["themeColor"], serde_json::json!("#123456"));
        assert_eq!(obj["language"], serde_json::json!("en-US"));
        assert_eq!(
            obj["darkMode"],
            serde_json::json!(true),
            "dark_mode 要转成布尔"
        );
        assert!(obj.get("theme_color").is_none(), "键名必须是 camelCase");
        assert!(obj.get("fontFamily").is_none(), "没变的键不得出现");

        // 通知两项的缺省是"开"（与 get_settings 同口径）：没设置过也必须给 true，
        // 否则另一个窗口会把"通知已开启"应用成关闭。
        let patch = super::settings_patch_values(&conn, &["notifyEnabled", "notifyShowContent"]);
        assert_eq!(patch["notifyEnabled"], serde_json::json!(true));
        assert_eq!(patch["notifyShowContent"], serde_json::json!(true));

        // 资料/目录不在 Settings 形状里 ⇒ 不放进补丁（接收方看到键名会定向重拉）
        let patch = super::settings_patch_values(&conn, &["nickname", "avatar", "shareDir"]);
        assert_eq!(patch.as_object().unwrap().len(), 0);
    }

    /// **清空必须是"分批 + 批间放锁"** —— 这是"点清除数据不再卡死"的机制本身。
    ///
    /// 用户实测：点「清除数据」时设置窗口整个卡死、点不动。根因是原实现用一个横跨所有表的
    /// 大事务，把 `db` 互斥锁握住数秒；期间每个读命令都在等锁，等锁的 async 任务占住工作线程，
    /// 新 IPC 排不上队。修法是分批 —— 本用例**确定性地**证明"单次只删一批"：
    /// 若有人把它改回"一个大事务"，第一次调用就会把表删光，下面的断言立刻失败。
    #[test]
    fn clear_is_batched_so_the_db_lock_is_held_only_briefly() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        let rows = super::CLEAR_BATCH_ROWS * 2 + 10;
        {
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..rows {
                tx.execute(
                    "INSERT INTO messages(msg_id, conv_id, sender_id, receiver_id, kind, content, ts, status)
                     VALUES(?1, 'c1', 'me', 'peer', 'text', 'x', ?2, 'sent')",
                    rusqlite::params![format!("m{i}"), i as i64],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let db = std::sync::Mutex::new(conn);

        // ① 单次调用只允许删一批
        let first = super::clear_one_batch(&db, "messages").unwrap();
        assert_eq!(
            first,
            super::CLEAR_BATCH_ROWS as u64,
            "单次调用必须只删一批（一次 2000 行）；删更多说明事务又变大了"
        );
        let left_after_first: i64 = db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            left_after_first,
            rows as i64 - super::CLEAR_BATCH_ROWS as i64,
            "第一次调用只该删掉一批（剩 {} 行）—— 若这里是 0，说明又回到\"一个大事务握住锁\"了",
            rows - super::CLEAR_BATCH_ROWS
        );

        // ② 循环删干净
        let mut total = first;
        loop {
            let n = super::clear_one_batch(&db, "messages").unwrap();
            total += n;
            if n == 0 {
                break;
            }
        }
        assert_eq!(total, rows as u64, "必须把行删干净");
        let left: i64 = db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "表必须清空");
    }

    #[test]
    fn decode_respects_exact_byte_limit() {
        let exact = vec![0u8; MAX_OUTGOING_IMAGE_BYTES as usize];
        let url = data_url("image/png", &exact);
        let (_, bytes) = decode_outgoing_image(&url).unwrap();
        assert_eq!(bytes.len() as u64, MAX_OUTGOING_IMAGE_BYTES);
    }

    /// **`content://` URI → 真实文件名**（安卓选择器的返回值解析规则）。
    #[test]
    fn content_uri_yields_a_real_file_name_when_it_encodes_one() {
        // external-storage / documents 提供者会把相对路径编码进 URI
        assert_eq!(
            super::name_from_content_uri(
                "content://com.android.externalstorage.documents/document/primary%3ADownload%2Freport.pdf"
            )
            .as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            super::name_from_content_uri(
                "content://x/document/primary%3APictures%2FIMG%202024.jpg"
            )
            .as_deref(),
            Some("IMG 2024.jpg")
        );
        // MediaStore（相册）只有数字 id ⇒ 取不到名字，交给内容嗅探
        assert_eq!(
            super::name_from_content_uri("content://media/external/images/media/1000000033"),
            None
        );
        // 危险/异常形状一律拒绝（不能让它决定落盘路径）
        assert_eq!(
            super::name_from_content_uri("content://x/document/..%2F..%2Fetc%2Fpasswd"),
            None
        );
        assert_eq!(
            super::name_from_content_uri("content://x/document/noext"),
            None
        );
    }

    /// **按文件头嗅探媒体类型**：相册 URI 没有扩展名，靠内容才能把图片发成"图片"。
    #[test]
    fn sniff_media_ext_recognises_the_common_types() {
        assert_eq!(super::sniff_media_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(super::sniff_media_ext(b"\x89PNG\r\n\x1a\n"), "png");
        assert_eq!(super::sniff_media_ext(b"GIF89a"), "gif");
        assert_eq!(
            super::sniff_media_ext(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            "webp"
        );
        assert_eq!(super::sniff_media_ext(b"%PDF-1.7"), "pdf");
        assert_eq!(super::sniff_media_ext(b"PK\x03\x04"), "zip");
        // 未知内容 → bin（当成普通文件，绝不猜成图片）
        assert_eq!(super::sniff_media_ext(b"\x00\x01\x02\x03"), "bin");
        assert_eq!(super::sniff_media_ext(&[]), "bin");

        // ===== HEIC/HEIF family（ISO BMFF ftyp box at offset 4）=====
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypheic"), "heic");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypheix"), "heic");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypmif1"), "heic");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypmsf1"), "heic");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftyphevc"), "heic");

        // ===== MP4 / MOV / 视频格式（ISO BMFF ftyp box）=====
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypisom"), "mp4");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypmp42"), "mp4");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypM4V "), "mp4");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftypqt  "), "mov");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftyp3gpp5"), "3gp");
        assert_eq!(super::sniff_media_ext(b"\x00\x00\x00\x20ftyp3gp"), "3gp");

        // ===== WebM（EBML 容器）=====
        assert_eq!(super::sniff_media_ext(b"\x1A\x45\xDF\xA3\x01\x00\x00\x00"), "webm");

        // ===== AVI（RIFF....AVI）=====
        assert_eq!(
            super::sniff_media_ext(b"RIFF\x00\x00\x00\x00AVI LIST"),
            "avi"
        );

        // ===== FLV =====
        assert_eq!(super::sniff_media_ext(b"FLV\x01\x00\x00\x00\x09"), "flv");
    }

    /// 文件名消毒：不允许写出缓存目录之外，也不允许空名字。
    #[test]
    fn sanitize_file_name_blocks_path_traversal() {
        let escaped = super::sanitize_file_name("../../etc/passwd");
        assert!(
            !escaped.contains('/') && !escaped.contains('\\'),
            "消毒后不能含路径分隔符：{escaped}"
        );
        assert_eq!(super::sanitize_file_name("a/b\\c.txt"), "a_b_c.txt");
        assert_eq!(super::sanitize_file_name("   "), "file.bin");
        assert_eq!(super::sanitize_file_name("...hidden"), "hidden");
        assert_eq!(super::sanitize_file_name("正常名字.pdf"), "正常名字.pdf");
    }

    /// **已经是好友的人，其好友申请不该再出现在「新朋友」里**（用户 2026-09-12 真机实测要求）。
    fn pending(from: &str) -> super::PendingRequest {
        super::PendingRequest {
            from: from.to_string(),
            from_nickname: from.to_string(),
            from_avatar: None,
            ts: 1,
        }
    }

    fn ids(list: &[&str]) -> std::collections::HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pending_request_from_an_existing_friend_is_not_actionable() {
        // 不是好友 → 该显示
        assert!(super::is_actionable_request(&pending("A"), &ids(&[])));
        assert!(super::is_actionable_request(&pending("A"), &ids(&["B"])));
        // 已是好友 → 不显示（这正是用户报的"点了同意，对方那边申请还挂着"）
        assert!(!super::is_actionable_request(&pending("A"), &ids(&["A"])));
        assert!(!super::is_actionable_request(
            &pending("A"),
            &ids(&["A", "B"])
        ));
    }

    /// 蓝牙开关的决策规则：**最后一次意图胜出**，冷却只排队、不丢弃。
    ///
    /// 为什么必须守（用户 2026-09-13）：
    /// - 旧实现"冷却期内直接忽略请求"会把用户"关一下马上又开"的第二次点击**静默吞掉**
    ///   ⇒ 开关看起来点了没反应；
    /// - 反过来，抖动的调用方（每秒十几次开/关）必须仍然被挡住 —— 每次真实启停都要等满冷却，
    ///   这是 2026-09-12"整个应用顿卡"那个事故的护栏。
    /// 两条都在下面钉住。
    #[cfg(feature = "bluetooth")]
    #[test]
    fn bt_switch_plan_coalesces_intent_and_keeps_the_cooldown() {
        use super::bt_switch_plan;
        const CD: u64 = 3_000;

        // ① 常规：从未启停过 + 运行状态与目标不同 ⇒ 立刻动手
        let p = bt_switch_plan(false, true, Some(true), 0, 10_000, CD);
        assert!(p.apply && p.wait_ms == 0, "第一次开启必须立刻执行：{p:?}");

        // ② 幂等：运行状态已经是目标状态 ⇒ 不碰蓝牙栈（抖动护栏）
        let p = bt_switch_plan(true, true, Some(true), 10_000, 10_100, CD);
        assert!(!p.apply, "已经是目标状态时不许再启停：{p:?}");

        // ③ 冷却期内**改主意** ⇒ 不是丢弃，而是排队等够冷却再执行
        let p = bt_switch_plan(false, true, Some(true), 10_000, 11_000, CD);
        assert!(p.apply, "冷却期内的新意图必须被执行（不能丢）：{p:?}");
        assert_eq!(p.wait_ms, 2_000, "应等到 3s 冷却结束：{p:?}");

        // ④ 冷却已过 ⇒ 立刻执行
        let p = bt_switch_plan(false, true, Some(true), 10_000, 14_000, CD);
        assert!(p.apply && p.wait_ms == 0, "冷却结束后应立刻执行：{p:?}");

        // ⑤ 意图已被更晚的请求改写 ⇒ 本请求什么都不做（让那次去做）
        let p = bt_switch_plan(false, true, Some(false), 10_000, 20_000, CD);
        assert!(!p.apply, "被更晚的意图取代的请求不该动蓝牙栈：{p:?}");
        assert!(p.reason.contains("取代"), "原因要说清楚：{p:?}");

        // ⑥ 时钟回拨 / 毫秒溢出：不允许 panic，也不允许负等待
        let p = bt_switch_plan(false, true, Some(true), 10_000, 5_000, CD);
        assert!(p.apply && p.wait_ms == CD, "时钟回拨时按满冷却等待：{p:?}");
    }

    /// `latest_todo_def`：从消息日志里取"最新一条定义"，规则必须与前端 `newer()` 一致。
    ///
    /// 为什么这条必须有：它是**服务端鉴权的唯一依据**（谁创建、指派了谁）——
    /// 取错一条（例如按随机 id 排序而没按 `seq`）就会让"谁能改"判错，
    /// 而这类错误在界面上完全看不出来（只是某些人少了几个按钮、或多了不该有的权限）。
    #[test]
    fn latest_todo_def_follows_the_same_lww_rule_as_the_frontend() {
        use crate::protocol::TodoPayload;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        let conv = "group:g1";
        let payload = |title: &str| TodoPayload {
            todo_id: "todo-1".into(),
            title: title.into(),
            assignees: vec!["alice".into()],
            status: "todo".into(),
            creator: "alice".into(),
            deleted: false,
            description: String::new(),
            images: vec![],
            archived: false,
            done_at: None,
        };
        let insert = |msg_id: &str, seq: i64, p: &TodoPayload| {
            conn.execute(
                "INSERT INTO messages (msg_id, conv_id, sender_id, receiver_id, kind, content, ts, seq, status) \
                 VALUES (?1, ?2, 'alice', 'bob', 'todo', ?3, 0, ?4, 'sent')",
                rusqlite::params![msg_id, conv, serde_json::to_string(p).unwrap(), seq],
            )
            .unwrap();
        };
        insert("m1", 5, &payload("旧标题"));
        insert("m2", 6, &payload("新标题"));
        insert("m3", 6, &payload("同 seq 但 msg_id 更大"));
        assert_eq!(
            super::latest_todo_def(&conn, conv, "todo-1").unwrap().title,
            "同 seq 但 msg_id 更大",
            "同 seq 时必须按 msg_id 取更大者（与前端 newer() 同规则）"
        );
        assert!(
            super::latest_todo_def(&conn, conv, "todo-2").is_none(),
            "别的 todo_id 不许串台"
        );
        assert!(
            super::latest_todo_def(&conn, "group:g2", "todo-1").is_none(),
            "别的群不许串台"
        );
    }

    /// 任务改动的鉴权判据 —— 两档 + 「只动归档位」的放宽档（口径来自用户 2026-09-17、
    /// 2026-09-20 与 2026-09-24）：
    /// · 结构（改标题 / 删除）：创建者 **或** 群主
    /// · 其余（描述 / 图片 / 指派人 / 状态）：创建者 **或** 群主 **或** 当前被指派人
    /// · 归档：任何**群成员**都可以（用户 2026-09-24「群里所有人都可以归档」）
    /// 外加一条自洽检查：无关成员两档都不行（放宽群主权限不该顺带放进第三人）。
    #[test]
    fn todo_update_permission_matrix() {
        use crate::protocol::TodoPayload;
        let def = TodoPayload {
            todo_id: "t".into(),
            title: "x".into(),
            assignees: vec!["alice".into(), "bob".into()],
            status: "todo".into(),
            creator: "alice".into(),
            deleted: false,
            description: String::new(),
            images: vec![],
            archived: false,
            done_at: None,
        };
        // 参数顺序：(def, actor, group_creator, edits_structure)
        // 档位只有两档（结构 = 改标题/删除；其余 = 描述/图片/指派人/状态/归档），
        // 所以每条断言都是**不同的行为** —— 旧矩阵里那些只换 `edits_assignees` 的行
        // 判的是同一件事（那个入参根本不参与判权，v4.23.1 已删）。
        // 创建者 alice：两档都可以
        assert!(super::may_update_todo(&def, "alice", "owner", false));
        assert!(super::may_update_todo(&def, "alice", "owner", true));
        // 被指派人 bob：能改状态 / 指派人 / 描述；不能改标题 / 删除。
        // 后一条就是"同时改指派人 + 标题"必须被拒的形状（旧实现先判指派人会放行它）。
        assert!(super::may_update_todo(&def, "bob", "owner", false));
        assert!(
            !super::may_update_todo(&def, "bob", "owner", true),
            "被指派人不得改标题 / 删除任务"
        );
        // 群主 owner：两档都可以（用户 2026-09-20「群主不能编辑群任务」）
        assert!(super::may_update_todo(&def, "owner", "owner", false));
        assert!(super::may_update_todo(&def, "owner", "owner", true));
        // 无关成员 carol：两档都不行（放宽群主权限**没有**顺带放进第三人）
        assert!(!super::may_update_todo(&def, "carol", "owner", false));
        assert!(!super::may_update_todo(&def, "carol", "owner", true));
        // 自洽检查：actor 恰好等于群主时才算群主，别人冒充群主无效
        assert!(super::may_update_todo(&def, "dave", "dave", true));
        assert!(!super::may_update_todo(&def, "carol", "dave", true));
    }

    /// 「只动归档位」这一档（用户 2026-09-24：群里所有人都可以归档）。
    ///
    /// 判权本身只有一行（`两档 || (archive_only && 是成员)`），值得钉的是 **`archive_only`
    /// 的入口**：这条口子一旦被"顺带改一点别的"挤进来，等于把鉴权前两档全部作废 ——
    /// 「改标题 + 归档」「改状态 + 归档」「删除 + 归档」三种形状都必须判否，
    /// 传了**相同**的归档值（没真的改）也必须判否。
    #[test]
    fn todo_archive_only_lane_is_narrow_and_member_only() {
        use crate::protocol::TodoPayload;
        let def = TodoPayload {
            todo_id: "t".into(),
            title: "x".into(),
            assignees: vec!["alice".into()],
            status: "done".into(),
            creator: "alice".into(),
            deleted: false,
            description: "d".into(),
            images: vec![],
            archived: false,
            done_at: Some(1),
        };
        // 只动归档位：请求 = 库里原值 + archived 翻转。
        let archive = |title: &str, status: &str, deleted: bool, archived: Option<bool>| {
            super::archive_only_change(
                &def,
                deleted,
                title,
                status,
                &def.assignees,
                &def.description,
                &def.images,
                archived,
            )
        };
        assert!(archive("x", "done", false, Some(true)), "纯归档要放行");
        assert!(
            super::archive_only_change(
                &TodoPayload { archived: true, ..def.clone() },
                false,
                "x",
                "done",
                &def.assignees,
                &def.description,
                &def.images,
                Some(false),
            ),
            "取消归档走同一条口子（库里 archived=true ⇒ 翻动就是 false）"
        );
        assert!(
            !archive("改过的标题", "done", false, Some(true)),
            "夹带改标题不许走这条口子"
        );
        assert!(
            !archive("x", "todo", false, Some(true)),
            "夹带改状态不许走这条口子"
        );
        assert!(
            !archive("x", "done", true, Some(true)),
            "夹带删除不许走这条口子"
        );
        assert!(
            !archive("x", "done", false, None),
            "没传 archived = 不是归档改动"
        );
        assert!(
            !archive("x", "done", false, Some(false)),
            "库里本来就没归档 ⇒ 翻动后相同 = 什么都没改"
        );
        // 判权：无关成员 carol 只有在这一档 + 是本群成员时才被放行。
        let may = |archive_only: bool, is_member: bool| {
            super::may_change_todo(&def, "carol", "owner", false, archive_only, is_member)
        };
        assert!(may(true, true), "群成员只动归档位 ⇒ 可以");
        assert!(!may(true, false), "不是本群成员 ⇒ 同一条改动也不给");
        assert!(!may(false, true), "普通成员改状态仍然不行（放宽只覆盖归档这一位）");
        // 结构改动即使"看起来只动归档"也不放宽（archive_only 已排除 deleted/改标题，
        // 这里再钉住判权那一行不会自己把它放进来）。
        assert!(
            !super::may_change_todo(&def, "carol", "owner", true, false, true),
            "结构改动不会因为『反正是成员』被放行"
        );

        // 判权对了但**没接上命令**，这条放宽就等于没做（而且没有任何测试会红）：
        // `update_group_todo` 必须用 `may_change_todo` 那个总入口，并把 `members`
        // 真读出来算 `is_member`。只能钉接线（与本仓其余"判据有单测、接线有守卫"的
        // 分工一致），而误接回 `may_update_todo` 的编译错误**挡不住** —— 那个函数还在，
        // 签名也对得上，表现就是"所有人都归档不了"。
        // 就地 flatten（`lib.rs` 测试模块里那个 `code_flat` 是它的私有函数，跨不到这里）：
        // 守卫搜的是"调用形状"，而 `cargo fmt` 会把多参调用拆成一行一个 ⇒ 不拆就搜不到。
        let cmd: String = include_str!("group_files.rs")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let at = cmd
            .find("if!may_change_todo(")
            .expect("鉴权调用点找不到（被改写了？守卫要同步）");
        // 窗口取到闭合的 `){` 为止，**不数固定字节**：数长度会随 fmt 漂移，
        // 极端时还会切进多字节字符里 panic（`code_flat` 之后仍按 char 计）。
        let win_end = cmd[at..]
            .find("){")
            .map(|k| at + k + 2)
            .expect("找不到鉴权 if 的闭合");
        let window = &cmd[at..win_end];
        assert!(
            window.contains("archive_only,") && window.contains("is_member,"),
            "放宽档的两个入参必须传进总判权，否则这条口子永远不生效"
        );
        assert!(
            !cmd.contains("if!may_update_todo("),
            "命令层不得再直接调两档判据 —— 那样归档放宽会被绕开"
        );
        assert!(
            cmd.contains("letis_member=members.iter()"),
            "is_member 必须来自群的 members，不能是常数或别的表"
        );
    }

    /// 完成 / 归档字段的权威推导（用户 2026-09-17：「完成以后手动归档」）。
    ///
    /// 为什么必须钉死：这两条都**只体现在行为里**，看代码很容易漏 ——
    /// ① 完成不再自动归档（否则用户刚点完成，任务就从列表里消失了）；
    /// ② 重复保存不改 `done_at`（否则改个标题就把 7 天自动归档的计时重置了）。
    #[test]
    fn todo_done_and_archive_are_resolved_server_side() {
        let now = 1_800_000_000_000;
        // 首次完成：不归档（要手动），记完成时间
        assert_eq!(
            super::resolve_done_archive(true, None, false, None, now),
            (false, Some(now))
        );
        // 完成态重复保存（改标题/描述）：沿用原完成时间，不重置计时；归档状态不动
        assert_eq!(
            super::resolve_done_archive(true, None, false, Some(now - 1234), now),
            (false, Some(now - 1234)),
            "重复保存不得把 7 天计时重置"
        );
        // 手动归档：显式请求才归档
        assert_eq!(
            super::resolve_done_archive(true, Some(true), false, Some(now - 1234), now),
            (true, Some(now - 1234))
        );
        // 取消归档（已归档的完成任务被取消）
        assert_eq!(
            super::resolve_done_archive(true, Some(false), true, Some(now - 1234), now),
            (false, Some(now - 1234))
        );
        // 非完成态（进行中 / 重新打开）：一律取消归档 + 清空完成时间
        assert_eq!(
            super::resolve_done_archive(false, None, true, Some(now - 1234), now),
            (false, None)
        );
        assert_eq!(
            super::resolve_done_archive(false, Some(true), true, Some(now - 1234), now),
            (false, None),
            "非完成态不得被归档（否则进行中的任务会从活动列表消失）"
        );
    }
}
