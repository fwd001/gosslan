// 职责边界：
// - `lib_tests.rs` 测试分册之3 —— 安卓 JNI：Rust extern 与 Kotlin 签名互点、release 的 proguard keep 名单
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
    /// **Kotlin ↔ Rust 的 JNI 方法签名必须逐字对齐**。
    ///
    /// JNI 调用**不做任何编译期检查**：描述符写错只会在运行期抛 `NoSuchMethodError`，
    /// 而且只有真机上才现形。真实缺陷（本轮 code review 抓到）：
    /// Kotlin 的 `fun stop()` 是 Unit 方法（JNI `()V`），Rust 侧却用 `()Z` 调用 ⇒
    /// 用户关掉「蓝牙通道」后手机**仍在广播**（耗电 + 隐私），日志里一个字都没有。
    ///
    /// 这条护栏在**主机上**就能跑：解析 Kotlin 源码里 `fun` 的形参/返回类型推出 JNI 描述符，
    /// 与 Rust 侧 `kotlin_method!("名字", "描述符")` 的登记逐条比对；
    /// 并检查每个 `extern fn`（native 回调）在 Kotlin 里确有同名 `external fun`。
    #[test]
    fn android_jni_signatures_match_kotlin() {
        let kotlin =
            include_str!("../gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt");
        let rust = include_str!("transport/ble_android.rs");
        // 第二条 JNI 桥：「用系统里的其它应用打开文件」（FileProvider）。同一个坑，
        // 所以必须同一条护栏盯着 —— 新桥单独立一份检查只会漂移。
        let kotlin_open =
            include_str!("../gen/android/app/src/main/java/com/gosslan/app/OpenWith.kt");
        let rust_open = include_str!("android_open.rs");

        let kotlin_fns = parse_kotlin_funs(kotlin);
        let open_fns = parse_kotlin_funs(kotlin_open);
        for expected in [
            "start",
            "stop",
            "send",
            "isConnected",
            "payloadMtu",
            "nativeBootstrap",
            "nativeOnFrame",
            "nativeOnUnlinked",
            "nativeOnNotice",
            "nativeOnWarning",
        ] {
            assert!(
                kotlin_fns.iter().any(|(n, _, _)| n == expected),
                "没在 BlePeripheral.kt 里解析到 `{expected}` —— Kotlin 写法变了就要同步更新本护栏"
            );
        }
        // 打开/保存文件的桥：Rust 调 openWith / saveWith / writeBytesWith / convertHeicToJpeg
        // / isHevcVideo / isMotionPhoto，Kotlin 调 nativeAttachOpenWith
        for (file, fns, expected) in [
            ("OpenWith.kt", &open_fns, "openWith"),
            ("OpenWith.kt", &open_fns, "saveWith"),
            ("OpenWith.kt", &open_fns, "writeBytesWith"),
            ("OpenWith.kt", &open_fns, "convertHeicToJpeg"),
            ("OpenWith.kt", &open_fns, "isHevcVideo"),
            ("OpenWith.kt", &open_fns, "isMotionPhoto"),
            ("OpenWith.kt", &open_fns, "nativeAttachOpenWith"),
        ] {
            assert!(
                fns.iter().any(|(n, _, _)| n == expected),
                "没在 {file} 里解析到 `{expected}` —— Kotlin 写法变了就要同步更新本护栏"
            );
        }

        // ① Rust 登记的每个 Kotlin 方法，描述符必须与 Kotlin 源码推出的**逐字相同**
        let registered = parse_kotlin_method_registrations(rust);
        assert!(
            registered.len() >= 5,
            "Rust 侧至少应登记 5 个 Kotlin 方法，实际 {}",
            registered.len()
        );
        let open_registered = parse_kotlin_method_registrations(rust_open);
        assert_eq!(
            open_registered.len(),
            6,
            "android_open.rs 应恰好登记 6 个 Kotlin 方法（openWith + saveWith + writeBytesWith + convertHeicToJpeg + isHevcVideo + isMotionPhoto），实际 {} —— \
             解析器失效或有人漏登记",
            open_registered.len()
        );
        let mut static_checked = 0usize;
        for (name, desc) in registered.iter().chain(open_registered.iter()) {
            let (_, kotlin_desc, _) = kotlin_fns
                .iter()
                .chain(open_fns.iter())
                .find(|(n, _, _)| n == name)
                .unwrap_or_else(|| panic!("Rust 登记了 Kotlin 里不存在的 `{name}`"));
            assert_eq!(
                kotlin_desc, desc,
                "`{name}` 的 JNI 描述符不一致：Kotlin 是 `{kotlin_desc}`，Rust 却按 `{desc}` 调用 \
                 —— JNI 不做任何编译期检查，这只会在真机上抛 NoSuchMethodError"
            );

            // ③ Rust 用 `call_static_method` 调它 ⇒ Kotlin 侧必须有**静态桥**。
            //    顶层函数天然是 static；`object`/`class` 的成员**必须带 `@JvmStatic`** ——
            //    少了它真机日志是 `JNI 调用失败：Method not found: start ()Z`
            //    （蓝牙外设整条路径失效，而 central 扫描不受影响 ⇒ 症状极其隐蔽）。
            let (file, kt) = if kotlin_fns.iter().any(|(n, _, _)| n == name) {
                ("BlePeripheral.kt", kotlin)
            } else {
                ("OpenWith.kt", kotlin_open)
            };
            let (idx, decl) = kt
                .lines()
                .enumerate()
                .find(|(_, l)| l.contains(&format!("fun {name}(")))
                .unwrap_or_else(|| panic!("在 {file} 里找不到 `fun {name}(` 的声明行"));
            let indented = decl.starts_with(' ') || decl.starts_with('\t');
            if indented {
                let lines: Vec<&str> = kt.lines().collect();
                let from = idx.saturating_sub(12);
                let annotated = lines[from..idx].iter().any(|l| l.trim() == "@JvmStatic");
                assert!(
                    annotated,
                    "`{name}` 是 {file} 里 object/class 的成员，而 Rust 用 call_static_method 调它 \
                     ⇒ 必须在它上面加 `@JvmStatic`（否则没有静态桥：真机 `Method not found: {name}`）"
                );
            }
            static_checked += 1;
        }
        assert!(
            static_checked >= 9,
            "应检查 ≥9 个 Kotlin 方法（OpenWith 3 个 + BlePeripheral 6 个），实际 {static_checked} —— 护栏失效了"
        );

        // ② Rust 导出的 native 回调（snake_case → lowerCamelCase）必须在 Kotlin 里是 external fun
        for snake in parse_extern_fn_names(rust)
            .into_iter()
            .chain(parse_extern_fn_names(rust_open))
        {
            let camel = snake_to_lower_camel(&snake);
            let found = kotlin_fns
                .iter()
                .chain(open_fns.iter())
                .find(|(n, _, _)| n == &camel)
                .unwrap_or_else(|| {
                    panic!(
                        "Rust 导出了 native 方法 `{snake}`（Java 名 `{camel}`），但 Kotlin 里没有这个 \
                         `external fun` —— JVM 会 UnsatisfiedLinkError"
                    )
                });
            assert!(
                found.2,
                "Kotlin 的 `{camel}` 不是 `external` 声明，JVM 不会去查 native 实现"
            );
        }
    }

    /// **JNI 的 static / 实例形态必须与 Kotlin 声明的形态一致**。
    ///
    /// 真实缺陷（2026-09-12 真机实测的**启动闪退**，而且编译、单测、构建全绿）：
    /// `OpenWith.kt` 里 `nativeAttachOpenWith()` 是**文件级（顶层）函数** ⇒ 编译成
    /// `OpenWithKt` 的 **static** 方法；而 Rust 侧的 `native_method!` 少了 `static` 关键字
    /// ⇒ 宏把它按**实例方法**注册。ART 在第一次调用时判定不一致并**直接 abort 整个进程**：
    ///   `Native method '"nativeAttachOpenWith"' was registered as instance but called as static method`
    /// （崩溃栈落在 `MainActivity.onCreate` → `OpenWith.bootstrap`）。
    ///
    /// 判据（Kotlin 的一行声明就足够）：**顶格声明的 `external fun` = 顶层 = static**；
    /// 缩进在 `object`/`class` 里的 = 成员 = 实例。两者与 Rust 的 `static` 关键字必须一一对应。
    /// 对照：`BlePeripheral` 的 `nativeBootstrap()` 在 object 内 ⇒ 实例 ⇒ 宏不加 `static`。
    #[test]
    fn jni_static_matches_kotlin_toplevel() {
        let ble_kt =
            include_str!("../gen/android/app/src/main/java/com/gosslan/app/BlePeripheral.kt");
        let open_kt = include_str!("../gen/android/app/src/main/java/com/gosslan/app/OpenWith.kt");
        let cases = [
            (include_str!("transport/ble_android.rs"), ble_kt),
            (include_str!("android_open.rs"), open_kt),
        ];
        let mut checked = 0usize;
        for (rust, kotlin) in cases {
            for (snake, is_static) in rust_native_methods(rust) {
                let camel = snake_to_lower_camel(&snake);
                let toplevel = kotlin_fun_is_toplevel(kotlin, &camel).unwrap_or_else(|| {
                    panic!("Kotlin 里找不到 native 方法 `{camel}` —— 护栏需要同步更新")
                });
                assert_eq!(
                    is_static, toplevel,
                    "`{camel}` 的 static 形态与 Kotlin 不一致：Rust 注册为{}，Kotlin 却是{}函数 ——                      真机第一次调用时 ART 会直接 abort（进程消失、连日志都来不及写全）",
                    if is_static { "static" } else { "实例" },
                    if toplevel { "顶层（=static）" } else { "成员（=实例）" },
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 5,
            "应至少检查 5 个 native 方法（BlePeripheral 5 个 + OpenWith 1 个），实际 {checked} —— 解析器失效了"
        );
    }

    /// 解析 Rust 侧 `native_method! { … extern fn <名字> … }` → (snake 名, 是否 static)。
    fn rust_native_methods(src: &str) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("native_method! {") {
            rest = &rest[i + "native_method! {".len()..];
            let head = match rest.find("fn =") {
                Some(e) => &rest[..e],
                None => continue,
            };
            if let Some(p) = head.find("extern fn ") {
                let after = &head[p + "extern fn ".len()..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push((name, head[..p].contains("static")));
                }
            }
        }
        out
    }

    /// Kotlin 里 `fun <camel>(` 这一行是否**顶格**（顶格 = 顶层函数 = JNI static）。
    fn kotlin_fun_is_toplevel(src: &str, camel: &str) -> Option<bool> {
        let needle = format!("fun {camel}(");
        src.lines().find(|l| l.contains(&needle)).map(|l| {
            let mut chars = l.chars();
            !matches!(chars.next(), Some(' ') | Some('\t'))
        })
    }

    /// **release 包必须 keep 住 Rust 按名字调用的 Kotlin 方法**。
    ///
    /// R8 在 release 下会把它们改名（**实测**：`stop`/`start`/`send`/… 全变成 `a`/`b`/`c`/…），
    /// 而 JNI 只按「名字 + 签名」查找 ⇒ release 真机包上蓝牙外设整条路径 `NoSuchMethodError`。
    /// debug 包不做混淆，所以这个坑在开发期完全看不见（我是在打 release 包时才抓到的）。
    /// 这条护栏同时盯两种漂移：规则里**漏了**方法，以及规则里**多留了**已废弃的方法。
    #[test]
    fn release_keeps_every_kotlin_method_called_from_rust() {
        let rust = include_str!("transport/ble_android.rs");
        // 单一事实来源：`scripts/android/proguard-gosslan.pro`（`gen/android` 是生成物，
        // `tauri android init` 会重生它，所以注入脚本每次构建前都把这份搬进去）。
        let source = include_str!("../../scripts/android/proguard-gosslan.pro");
        let generated = include_str!("../gen/android/app/proguard-rules.pro");

        let source_block = proguard_jni_block(source)
            .expect("scripts/android/proguard-gosslan.pro 里没有 GOSSLAN_JNI 标记块");
        let generated_block = proguard_jni_block(generated).expect(
            "gen/android/app/proguard-rules.pro 里没有 GOSSLAN_JNI 标记块 —— \
             跑 `node scripts/inject-android-signing.mjs`（或任意一次 android 构建）就会补上；\
             缺了它，release 包的蓝牙在真机上会 NoSuchMethodError",
        );
        assert_eq!(
            source_block, generated_block,
            "两处 JNI keep 规则漂移了：事实来源是 scripts/android/proguard-gosslan.pro，\
             gen/android/app/proguard-rules.pro 只是构建时注入的副本"
        );

        let registered = parse_kotlin_method_registrations(rust);
        assert!(
            registered.len() >= 5,
            "Rust 侧至少应登记 5 个 Kotlin 方法，实际 {}",
            registered.len()
        );
        // 打开文件的桥（android_open.rs）同样只被 JNI 按名字调用 ⇒ 必须一起 keep。
        // 两条桥放在一起查，避免"新加的桥忘了写 keep 规则"这种只在 release 真机上现形的漏。
        let registered: Vec<(String, String)> = registered
            .into_iter()
            .chain(parse_kotlin_method_registrations(include_str!(
                "android_open.rs"
            )))
            .collect();
        assert!(
            registered.iter().any(|(n, _)| n == "openWith"),
            "没在 android_open.rs 里解析出 `openWith` 的 JNI 登记 —— 解析器失效了，\
             这条护栏会静默变成空转"
        );
        let kept = proguard_kept_method_names(&source_block);
        assert!(
            kept.len() >= 5,
            "从 keep 块里只解析出 {} 个方法名 —— 护栏解析器失效了（这才是真正的风险：\
             它一旦静默返回空，下面的检查就全是空转）",
            kept.len()
        );

        for (name, _) in &registered {
            assert!(
                kept.iter().any(|k| k == name),
                "keep 规则里缺少 `{name}` —— R8 会把它改名，release 真机上 JNI 找不到这个方法"
            );
        }
        for name in &kept {
            assert!(
                registered.iter().any(|(n, _)| n == name),
                "keep 规则里的 `{name}` 在 Rust 侧已经没有调用登记了（陈旧规则），删掉它"
            );
            // ⚠️ 还必须是 **static**：Rust 用 `env.call_static_method(...)` 调它们，而 Kotlin 的
            // `@JvmStatic fun x()` 在 object 里生成"实例方法 + 静态桥"两个条目 —— 只 keep 实例方法
            // 时 R8 会把静态桥当死代码删掉，真机日志：`JNI 调用失败：Method not found: start ()Z`
            // （蓝牙外设整条路径失效，central 角色不受影响，所以症状很隐蔽）。
            // 只看真正的规则行（`public …;`），别把说明注释里引用的同一句当成规则
            let line = source_block
                .lines()
                .map(str::trim)
                .find(|l| {
                    l.starts_with("public ") && l.ends_with(';') && l.contains(&format!(" {name}("))
                })
                .unwrap_or_else(|| panic!("keep 块里找不到 `{name}` 的规则行"));
            assert!(
                line.trim_start().starts_with("public static"),
                "keep 规则里的 `{name}` 必须写成 `public static …`（实际：`{}`）—— \
                 Rust 是按**静态**方法调它的，只 keep 实例方法会让真机报 `Method not found: {name}`",
                line.trim()
            );
        }
    }

    /// 取出 `# GOSSLAN_JNI_BEGIN … # GOSSLAN_JNI_END` 之间的正文（不含两端的标记行）。
    fn proguard_jni_block(src: &str) -> Option<String> {
        const BEGIN: &str = "# GOSSLAN_JNI_BEGIN";
        const END: &str = "# GOSSLAN_JNI_END";
        let start = src.find(BEGIN)? + BEGIN.len();
        let end = src[start..].find(END)? + start;
        Some(src[start..end].trim().to_string())
    }

    /// 从 keep 块里抽出被 keep 的**方法名**（跳过 `native <methods>;` 这类通配与注释）。
    fn proguard_kept_method_names(block: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            if !line.ends_with(';') || line.starts_with('#') || line.starts_with("-keep") {
                continue;
            }
            let Some(open) = line.find('(') else { continue };
            let Some(name) = line[..open].split_whitespace().last() else {
                continue;
            };
            if name.starts_with('<') {
                continue; // `native <methods>;`
            }
            out.push(name.to_string());
        }
        out
    }

    /// 解析 Kotlin 里**单行**的 `[modifiers] fun name(params): Ret` → `(名字, JNI 描述符, 是否 external)`。
    fn parse_kotlin_funs(src: &str) -> Vec<(String, String, bool)> {
        let mut out = Vec::new();
        for line in src.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            let Some(pos) = line.find("fun ") else {
                continue;
            };
            let after = &line[pos + 4..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let (Some(open), Some(close)) = (after.find('('), after.find(')')) else {
                continue;
            };
            if close < open {
                continue;
            }
            let params = &after[open + 1..close];
            let ret = after[close + 1..]
                .trim_start()
                .strip_prefix(':')
                .and_then(|r| r.split_whitespace().next())
                .map(|r| r.trim_end_matches(['{', '=']))
                .filter(|r| !r.is_empty())
                .unwrap_or("Unit");
            // 只解析"Rust 可能调用"的方法：形参/返回类型里有本项目没映射过的类型
            // （例如 `bootstrap(context: Context)`）就跳过 —— 护栏只关心被登记的那几个。
            let mut desc = String::from("(");
            let mut mapped = true;
            for p in params.split(',').filter(|p| !p.trim().is_empty()) {
                let ty = p.split(':').nth(1).map(str::trim).unwrap_or("Unit");
                match jni_type(ty) {
                    Some(t) => desc.push_str(&t),
                    None => {
                        mapped = false;
                        break;
                    }
                }
            }
            if !mapped {
                continue;
            }
            desc.push(')');
            match jni_type(ret) {
                Some(t) => desc.push_str(&t),
                None => continue,
            }
            out.push((name, desc, line.contains("external")));
        }
        out
    }

    /// 解析 Rust 侧 `kotlin_method!("名字", "描述符")` 的登记。
    fn parse_kotlin_method_registrations(src: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = src;
        const NEEDLE: &str = "kotlin_method!(";
        while let Some(i) = rest.find(NEEDLE) {
            let after = &rest[i + NEEDLE.len()..];
            // 结束括号要在**引号外**找：描述符形如 `"()V"`，里面也有 `)`
            let mut end = None;
            let mut in_quotes = false;
            for (idx, ch) in after.char_indices() {
                match ch {
                    '"' => in_quotes = !in_quotes,
                    ')' if !in_quotes => {
                        end = Some(idx);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            let parts: Vec<&str> = after[..end].split(',').map(str::trim).collect();
            if parts.len() == 2 {
                out.push((
                    parts[0].trim_matches('"').to_string(),
                    parts[1].trim_matches('"').to_string(),
                ));
            }
            rest = &after[end..];
        }
        out
    }

    /// 解析 Rust 侧 `extern fn name(` 的 native 回调名。
    fn parse_extern_fn_names(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = src;
        const NEEDLE: &str = "extern fn ";
        while let Some(i) = rest.find(NEEDLE) {
            let after = &rest[i + NEEDLE.len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
            rest = after;
        }
        out
    }

    fn snake_to_lower_camel(snake: &str) -> String {
        let mut parts = snake.split('_');
        let mut out = parts.next().unwrap_or_default().to_string();
        for part in parts {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.push_str(&first.to_uppercase().collect::<String>());
                out.push_str(chars.as_str());
            }
        }
        out
    }

    /// Kotlin 类型名 → JNI 描述符；本项目没映射过的类型返回 `None`（调用方跳过那个方法）。
    ///
    /// `Context`/`BluetoothGattServer` 这类只出现在 Kotlin 内部方法的形参里，
    /// Rust 从不调用它们，因此不需要（也不该）在这里维护映射。
    ///
    /// 可空标记 `?` 对 JNI 描述符**没有影响**（`String?` 与 `String` 都是
    /// `Ljava/lang/String;`）—— 这里显式去掉再匹配，否则 `OpenWith.openWith` 这种
    /// "返回 String? 表示成功/失败原因" 的方法会被整条跳过，护栏就静默失效了。
    fn jni_type(kotlin: &str) -> Option<String> {
        Some(match kotlin.trim().trim_end_matches('?') {
            "String" => "Ljava/lang/String;".to_string(),
            "ByteArray" => "[B".to_string(),
            "Boolean" => "Z".to_string(),
            "Int" => "I".to_string(),
            "Long" => "J".to_string(),
            "Float" => "F".to_string(),
            "Double" => "D".to_string(),
            "Unit" | "" => "V".to_string(),
            _ => return None,
        })
    }
