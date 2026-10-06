// 职责边界：
// - `lib_tests.rs` 测试分册之1 —— 守卫视图自身的登记对账 + 按形状取源码的解析 helper
// 为什么拆：`src-tauri/src/lib_tests.rs` 原来 4,009 行、96 个顶层项挤在同一个 `mod tests` 里。
// 机制与 `network/transport/tests.rs` 那一刀同一条：`include!` 是文本粘贴 ⇒ 这些项仍属于
// `crate::lib_tests` 里那同一个 `mod tests` ⇒ 模块路径、`use`、可见性、**测试全名一字不变**
// （恒等判据＝`cargo test --features bluetooth --lib -- --list` 的 789 条用例名差集 0 行）。
// ⚠️ **本册必须与 `lib_tests.rs` 同级平铺**（不许挪进 `lib_tests/` 子目录）：册里到处是
// `include_str!("commands.rs")`、`include_str!("../gen/android/…")` 这类**相对本文件**的路径，
// 换目录会让它们整体偏移一位 —— 编译器会拒（不会静默），但那就不再是"逐字未变"的搬家，
// 恒等判据当场降级成"我读过觉得没变"。同目录先例：`protocol_tests.rs`。
// ⚠️ 册名以 `_tests.rs` 结尾也是被判据选定的：`transport_src_for_guards()` 那份"生产码全集"视图
// 只许装生产码，而登记对账守卫按 `_tests.rs` 后缀豁免测试分册（并进视图是最顺手却假绿的消红办法）。
    /// **0-A1：三份"守卫用的源码全集"必须等于编译器实际 `include!` 进来的分册集合。**
    ///
    /// 为什么要这条（架构复审 2026-09-24 P10）：`include!` 只做编译期拼接，`include_str!`
    /// 看不见展开后的结果 ⇒ 守卫用的视图是**手工登记的第二份清单**。它已经漂移过两次：
    /// `commands/relay.rs` 与 `transport/relay.rs` 在 4.25.0 接线时都只登记了 `include!` 与领域图、
    /// 漏了这里（现场注释见 `all_commands_src()` 内与 `network/mod.rs:229`）。
    /// **漏登记的后果不是报错而是假绿** —— 以"全部命令面"为判据的守卫扫不到那个分册，
    /// 于是永远通过。假红至少逼人来看，假绿会一直骗下去。
    ///
    /// 两个方向都钉住：
    ///   · 编译器有、视图没有 ⇒ 假绿（最危险），红；
    ///   · 视图有、编译器没有 ⇒ 守卫会去扫根本不在这个模块里的代码（假红的来源），也红。
    ///
    /// `*_tests.rs` 分册是**故意**不进视图的（测试文本会把生产模式扫描带偏），
    /// 所以判据是"闭包减去测试分册"，并把这件事写在断言消息里而不是靠沉默。
    #[test]
    fn guard_source_views_register_every_include_subfile() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // 承载 `fn all_commands_src()` / `fn all_db_src()` 的文件：2026-09-28 起测试整段在
        // `lib_tests.rs`（lib.rs 只留生产码）⇒ 要找的是**本文件**，不是 lib.rs。
        // 刻意不把测试文本并进那两个视图：视图是"生产码全集"，掺进测试字面量会让
        // 所有以它为判据的守卫多看见一堆字符串常量（那是假绿的形状）。
        let this_file = include_str!("lib_tests.rs");
        let mod_file = include_str!("network/mod.rs");
        // 第五列是**每个用例自己的 canary**：一个"必须出现在闭包里"的分册。
        // 为什么不能只写一条"闭包 ≥ N"：三个视图的分册数差一个量级
        // （commands 24 / db 16 / transport 3），同一个阈值套下去要么松到没有意义、
        // 要么把 transport 直接判成"解析器失效"（第一版就是这么红的）。
        // canary 挑的都是历史上真漂移过的那批（4.25.0 接线时 relay 漏登记过两次）。
        let cases: [(&str, &str, &str, &str, &str); 3] = [
            (
                "commands.rs",
                "fn all_commands_src()",
                this_file,
                "all_commands_src()",
                "commands/relay.rs",
            ),
            (
                "db.rs",
                "fn all_db_src()",
                this_file,
                "all_db_src()",
                "db/recalls.rs",
            ),
            (
                "network/transport.rs",
                "fn transport_src_for_guards()",
                mod_file,
                "network::transport_src_for_guards()",
                "transport/relay.rs",
            ),
        ];
        for (entry_name, signature, holder_src, label, canary) in cases {
            let entry = manifest.join("src").join(entry_name);
            let mut closure = Vec::new();
            include_closure(&entry, &mut closure);
            let expected: Vec<String> = closure
                .iter()
                .filter(|p| {
                    !p.file_name()
                        // `_tests.rs` 与本仓唯一一份恰好叫 `tests.rs` 的 transport 分册都不算生产码：
                        // 视图里没有它们，这里若还要求登记就会逼着别人把测试文本并进去（假绿形状）。
                        .map_or(false, |n| {
                            let n = n.to_string_lossy();
                            n.ends_with("_tests.rs") || n == "tests.rs"
                        })
                })
                .map(|p| p.to_string_lossy().into_owned())
                .collect();

            // 登记项是相对"持有该函数的文件"的，按同一规则解析成绝对路径再比集合。
            let holder_dir = entry.parent().unwrap();
            let registered: Vec<String> =
                scan_macro_paths(slice_fn_body(holder_src, signature), "include_str")
                    .into_iter()
                    .map(|rel| {
                        std::path::PathBuf::from(
                            holder_dir.join(rel).to_string_lossy().replace('\\', "/"),
                        )
                        .to_string_lossy()
                        .into_owned()
                    })
                    .filter(|p| !p.ends_with(entry_name))
                    .collect();

            let missing: Vec<&String> = expected
                .iter()
                .filter(|p| !registered.contains(p))
                .collect();
            let extra: Vec<&String> = registered
                .iter()
                .filter(|p| !expected.contains(p))
                .collect();
            assert!(
                missing.is_empty(),
                "`{label}` 少登记了 {} 个分册（编译器 include! 了它，守卫却看不见 ⇒ **假绿**）：{missing:?}\n\
                 新增分册时要在 `{label}` 里同步登记一行 include_str!。",
                missing.len()
            );
            assert!(
                extra.is_empty(),
                "`{label}` 多登记了 {} 个不在 include! 闭包里的文件（守卫会扫根本不在这个模块里的代码 ⇒ 假红）：{extra:?}",
                extra.len()
            );
            // 反向自检：解析器如果整体失效（两边都空 / 都少），上面两条会同时通过 ⇒ 空转。
            // 所以每个用例点一枚 canary：它必须**同时在闭包与登记清单里**。
            let in_closure = expected.iter().any(|p| p.ends_with(canary));
            let in_registered = registered.iter().any(|p| p.ends_with(canary));
            assert!(
                in_closure && in_registered,
                "`{label}` 的 canary `{canary}` 没同时出现在两侧（闭包 {in_closure} / 登记 {in_registered}）\
                 ⇒ 解析器或清单坏了，这条守卫正在空转"
            );
        }
    }

    /// 把源码切成"顶层条目"，返回 `(条目名, 条目体)`；`#[cfg(test)]` 的 `mod` 整块跳过。
    ///
    /// 只做顶层：本仓 rustfmt 下 `fn` / `mod` 的声明都在第 0 列，函数体内的嵌套函数有缩进，
    /// 因此"以 `fn `/`mod ` 开头且无缩进"就是唯一的条目边界。不数大括号 ⇒ 不受字符串干扰。
    fn top_level_fns(src: &str) -> Vec<(String, String)> {
        let lines: Vec<&str> = src.split('\n').collect();
        // 每个条目的起始行 + 名字 + 是不是测试块
        let mut items: Vec<(usize, String, bool)> = Vec::new();
        let mut i = 0usize;
        while i < lines.len() {
            let l = lines[i];
            let is_cfg_test = l.trim_end() == "#[cfg(test)]";
            if let Some(name) = item_name(l) {
                // 往前看一眼属性行，判断是否测试专用
                let mut test = false;
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    let prev = lines[k].trim_end();
                    if prev == "#[cfg(test)]" {
                        test = true;
                        break;
                    }
                    if !prev.starts_with('#') && !prev.is_empty() {
                        break;
                    }
                }
                items.push((i, name, test || is_cfg_test));
            }
            i += 1;
        }
        let mut out = Vec::new();
        for (n, (start, name, is_test)) in items.iter().enumerate() {
            if *is_test {
                continue;
            }
            let end = items.get(n + 1).map(|x| x.0).unwrap_or(lines.len());
            let body = lines[*start..end].join("\n");
            out.push((format!("{name}（第 {} 行）", start + 1), body));
        }
        out
    }

    /// 这一行是不是一个顶层 `fn` / `mod` 声明？是的话给出它的名字。
    fn item_name(line: &str) -> Option<String> {
        let re = regex_for_item(line)?;
        Some(re)
    }

    fn regex_for_item(line: &str) -> Option<String> {
        // 手写判定，避免为一条守卫引入 regex 依赖
        if line.starts_with(' ') || line.starts_with('\t') || line.is_empty() {
            return None;
        }
        let rest = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub(super) "))
            .or_else(|| line.strip_prefix("pub "))
            .or_else(|| line.strip_prefix("async "))
            .unwrap_or(line);
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        let (kw, tail) = rest.split_once(' ')?;
        if kw != "fn" && kw != "mod" {
            return None;
        }
        let name: String = tail
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            return None;
        }
        Some(format!("{kw} {name}"))
    }

    /// 取一个顶层函数的函数体（从签名起到第 0 列的 `}` 为止）。
    ///
    /// 用它做"接线守卫"：这类性质（窗口走单例 helper、URL 指向自己的入口）
    /// 编译器管不着，而退化后**功能看起来仍然正常**，只有连点/开窗慢才暴露。
    ///
    /// ⚠️ **必须先归一化行尾**（2026-09-13）：本仓库在 Windows 上会被 git
    /// （`core.autocrlf`）检出成 **CRLF**，此时函数结尾的字节是 `\n}\r\n`，
    /// **不含**锚点 `"\n}\n"`。旧实现直接在原文上 `find` ⇒ 永远找不到锚点 ⇒
    /// 静默退化成 `&rest[..]`（**整个文件剩余部分**），于是：
    ///   · `assert!(body.contains(..))` 全部"通过"（假绿）；
    ///   · `assert!(!body.contains(..))` 全部**误报失败**（真缺陷在别处也会报到这里）。
    /// 这正是本项目最该防的那类问题：护栏还在跑，却既盯不住真缺陷、又误报无关代码。
    ///
    /// 返回 `Cow`：LF 检出（macOS/Linux）零拷贝借用原文，CRLF 检出才复制一份。
    fn rust_fn_body<'a>(src: &'a str, signature: &str) -> std::borrow::Cow<'a, str> {
        let normalized = if src.contains('\r') {
            std::borrow::Cow::Owned(src.replace("\r\n", "\n"))
        } else {
            std::borrow::Cow::Borrowed(src)
        };
        let start = normalized
            .find(signature)
            .unwrap_or_else(|| panic!("源码里找不到 `{signature}` —— 护栏需要同步更新"));
        let rest = &normalized[start..];
        // 找不到收尾锚点时返回剩余全部：**这是刻意的**（宁可多看一点，也不要 panic
        // 让护栏本身变成构建阻塞），但上面那段注释说明了它为什么会掩盖问题。
        let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
        match normalized {
            // 借用原文时可以直接切原文（偏移一致）
            std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Borrowed(&src[start..start + end]),
            std::borrow::Cow::Owned(s) => {
                std::borrow::Cow::Owned(s[start..start + end].to_string())
            }
        }
    }

    /// 把源码字符串里的**所有空白**（空格 / 换行 / 制表 / CR）都去掉。
    ///
    /// 护栏用 `include_str!` 读源码然后 `.contains()` 搜特定调用格式；
    /// `cargo fmt` 会把单行调用拆成多行（`fn(a, b, c)` → 每行一个参数），
    /// 带空格的 `.contains("fn(a, b")` 就会误报。
    /// 先 flatten 再搜，让 fmt 怎么拆都不怕。
    fn code_flat(src: &str) -> String {
        src.chars().filter(|c| !c.is_whitespace()).collect()
    }
