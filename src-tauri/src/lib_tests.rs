// 职责边界：
// - lib.rs 的源码守卫与启动期行为测试（搬出来用 `include!` 贴回同一模块 ⇒ 测试路径 lib::tests::* 不变）
// 为什么搬：搬之前 lib.rs 共 4511 行，这一段占 3952 行（约 87%），启动装配本身只有 559 行。
// 为什么这次搬动零路径风险：新文件与主文件**同目录**（src/），所以段内 `include_str!("notifications.rs")`
// 这类相对路径的含义一字不变。（对照 `network/transport/tests.rs` 那次：换了目录就必须把
// `include_str!("ble.rs")` 改成 `../ble.rs`，编译器会当场拒，不会静默。）
// 2026-10-06 再切：本文件原 4,009 行、96 个顶层项 ⇒ 按关注点搬进同目录的 `lib_<concern>_tests.rs`
// （`include!` 贴回同一个 `mod tests` ⇒ 测试路径 `lib::tests::*` 一字不变，理由与上面同一条：
// **同目录**才零路径风险）。留在壳里的只有三份守卫视图与它的解析器 —— 登记对账那条守卫读的是
// `include_str!("lib_tests.rs")` 并按函数名切体，把视图搬走就是弄丢那条守卫自己的锚点。
#[cfg(test)]
mod tests {
    use super::*;


    // ↓ 视图三件套与它的解析器**留在壳里**：`source_view_tests.rs` 里那条登记对账守卫读的是
    // `include_str!("lib_tests.rs")` 并按函数名切体 ⇒ 搬走它等于弄丢那条守卫自己的锚点。
    /// 把 commands.rs 头部 + 所有子模块文件拼接成一份完整源码。
    ///
    /// `include!` 只做编译期拼接，`include_str!` 看不到展开后的结果。
    /// 这条辅助让源码守卫测试拿到"等于原始单文件"的视图。
    fn all_commands_src() -> &'static str {
        concat!(
            include_str!("commands.rs"),
            "\n",
            include_str!("commands/system.rs"),
            "\n",
            include_str!("commands/network.rs"),
            "\n",
            include_str!("commands/dev_diag.rs"),
            "\n",
            include_str!("commands/channel.rs"),
            "\n",
            include_str!("commands/settings.rs"),
            "\n",
            include_str!("commands/friends.rs"),
            "\n",
            include_str!("commands/mobile_picker.rs"),
            "\n",
            include_str!("commands/chat.rs"),
            "\n",
            include_str!("commands/groups.rs"),
            "\n",
            include_str!("commands/window.rs"),
            "\n",
            include_str!("commands/group_files.rs"),
            "\n",
            include_str!("commands/files.rs"),
            "\n",
            include_str!("commands/share.rs"),
            "\n",
            include_str!("commands/helpers.rs"),
            "\n",
            include_str!("commands/favorites.rs"),
            "\n",
            include_str!("commands/routed.rs"),
            "\n",
            include_str!("commands/external_links.rs"),
            "\n",
            include_str!("commands/logs.rs"),
            "\n",
            include_str!("commands/chat_search.rs"),
            "\n",
            include_str!("commands/group_announcements.rs"),
            "\n",
            include_str!("commands/group_todo_media.rs"),
            "\n",
            include_str!("commands/group_file_dispatch.rs"),
            "\n",
            include_str!("commands/group_file_keys.rs"),
            "\n",
            // `commands/relay.rs` 之前漏在这里（4.25.0 接线时只登记了 `commands.rs` 的 `include!`
            // 与领域图），后果不是报错而是**假绿**：任何以"全部命令面"为判据的守卫都看不见
            // 中继那三个命令。补登记，让下面那条口令守卫能覆盖到它。
            include_str!("commands/relay.rs"),
        )
    }

    /// 把 db.rs 头部 + 所有子模块文件拼接成一份完整源码。
    fn all_db_src() -> &'static str {
        concat!(
            include_str!("db.rs"),
            "\n",
            include_str!("db/settings.rs"),
            "\n",
            include_str!("db/clocks.rs"),
            "\n",
            include_str!("db/group_delete_boundary.rs"),
            "\n",
            include_str!("db/friends.rs"),
            "\n",
            include_str!("db/groups.rs"),
            "\n",
            include_str!("db/messages.rs"),
            "\n",
            include_str!("db/conversations.rs"),
            "\n",
            include_str!("db/message_delete.rs"),
            "\n",
            include_str!("db/offline_queue.rs"),
            "\n",
            include_str!("db/group_offline_queue.rs"),
            "\n",
            include_str!("db/file_transfer.rs"),
            "\n",
            include_str!("db/file_offline.rs"),
            "\n",
            include_str!("db/group_files.rs"),
            "\n",
            include_str!("db/read_receipts.rs"),
            "\n",
            include_str!("db/favorites.rs"),
            "\n",
            include_str!("db/recalls.rs"),
        )
    }

    /// 按花括号配平切出一个函数（含签名到收尾 `}`）。
    ///
    /// 为什么不用现成的 `rust_fn_body`：它靠"找 `\n}\n`"定尾，而 `mod tests` 里的函数缩进四格，
    /// 收尾是 `\n    }\n` ⇒ 它找不到锚点、**返回剩余整个文件**（那是它刻意的兜底），
    /// 于是"登记清单"会被后面所有 `include_str!` 污染 —— 这条守卫要的是精确集合。
    fn slice_fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src
            .find(signature)
            .unwrap_or_else(|| panic!("源码里找不到 `{signature}` —— 这条守卫需要同步更新"));
        let open = src[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{signature}` 后面找不到函数体"));
        let mut depth = 0usize;
        for (i, ch) in src[start + open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &src[start..start + open + i + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("`{signature}` 的函数体花括号不配平");
    }

    /// 扫出一段源码里所有 `include_str!("…")` / `include!("…")` 的**相对路径**（跳过整行注释）。
    ///
    /// 为什么要跳过注释：这些文件里到处有人写"新增分册时要登记一行 `include!`"这类说明，
    /// 不跳过的话注释里那个举例路径会被当成真实登记项。
    fn scan_macro_paths(src: &str, macro_name: &str) -> Vec<String> {
        let needle = format!("{macro_name}!(\"");
        let mut out = Vec::new();
        for line in src.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut at = 0usize;
            while let Some(i) = line[at..].find(needle.as_str()) {
                let start = at + i + needle.len();
                let Some(j) = line[start..].find('"') else {
                    break;
                };
                out.push(line[start..start + j].to_string());
                at = start + j;
            }
        }
        out
    }

    /// 从 `entry` 出发**递归展开** `include!("…")`，返回除入口自身外的全部分册。
    ///
    /// 路径按 `include!` 的语义相对**当前文件**解析；解析不到文件的条目直接跳过
    /// （真正的 `include!` 路径写错根本编译不过，所以"文件不存在"只可能是注释里的举例）。
    fn include_closure(entry: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let text = std::fs::read_to_string(entry)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", entry.display()));
        let dir = entry.parent().unwrap();
        for rel in scan_macro_paths(&text, "include") {
            let child = dir.join(&rel);
            if !child.is_file() {
                continue;
            }
            let child = std::path::PathBuf::from(child.to_string_lossy().replace('\\', "/"));
            if out.contains(&child) {
                continue;
            }
            out.push(child.clone());
            include_closure(&child, out);
        }
    }


    include!("lib_source_view_tests.rs"); // 1/11 守卫视图自身的登记对账 + 按形状取源码的解析 helper
    include!("lib_window_tests.rs"); // 2/11 窗口面：capability 标签、辅助窗口单例/串行、Cmd-W、外链协议门
    include!("lib_android_jni_tests.rs"); // 3/11 安卓 JNI：Rust extern 与 Kotlin 签名互点、release 的 proguard keep 名单
    include!("lib_ble_tests.rs"); // 4/11 BLE 形状：指定拨号器、握手跳帧、外设节奏、载荷预算与运行态单一来源
    include!("lib_file_tests.rs"); // 5/11 文件传输形状：泡前不整档扫、进度按已写出字节、收尾在锁外、取消登记按收件人
    include!("lib_relay_data_tests.rs"); // 6/11 公网中继数据面：定向策略、回收终态落库在锁外、口令不进日志、电路标 Relay
    include!("lib_outbox_tests.rs"); // 7/11 Outbox 与过期终态：先落库再 emit、破坏性写延后、清扫覆盖半截与停滞接收端
    include!("lib_friend_identity_tests.rs"); // 8/11 好友与身份：申请/自动接受/pending 排除、三条成为好友的路径各绑一次公钥
    include!("lib_compat_gating_tests.rs"); // 9/11 跨版本与能力门：版本只声明不签名、新帧类型在发送口门控、受众三态
    include!("lib_startup_config_tests.rs"); // 10/11 启动与配置：降级拒绝早于写、锁内不 await、符号链不吃、开关读持久值
    include!("lib_delivery_shape_tests.rs"); // 11/11 投递形状：未知帧容忍、离线仍列出、群密钥先于群消息、发送成功后才落身份
}
