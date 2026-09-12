# btleplug 的 Android Java 实现（随仓库入库）

btleplug 在 Android 上是 **Rust + Java 混合**实现：Java 侧只被 native 代码**按类名**调用
（`find_class("io/github/gedgygedgy/rust/future/Future")` 这类），所以这些 `.java`
**必须编译进 App**，并且 release 包必须用 proguard keep 住（见 `scripts/android/proguard-gosslan.pro`）。

## 为什么要入库（而不是从 CARGO_HOME 里读）

1. **crates.io 的 `btleplug-0.13.0` 包只带 `com/nonpolynomial/**`（14 个 .java）**：
   `tar tzf btleplug-0.13.0.crate | grep gedgy` = 0 —— 另一半
   `io/github/gedgygedgy/**`（18 个 .java）**只在它的 git 仓库里**。
2. 之前我们把那 18 个文件手工塞进 `CARGO_HOME/registry/src/...` 的**提取目录**，
   而提取目录是**易失**的（换一个 `CARGO_HOME`、或 cargo 重新解包就没了）
   ⇒ 打出来的包里时有时无。
3. 更致命的是 keep 规则曾把包名拼错（`gedgygeddy`），R8 于是把这 18 个类当死代码删掉，
   而 `-dontwarn` 又把警告吞掉 ⇒ **构建期毫无提示**，只在真机 logcat 现形：
   `failed to resolve Java class 'io/github/gedgygedgy/rust/future/Future'` → 蓝牙不可用 + 闪退。

**结论**：两个包一起入库，构建只依赖本目录（不再依赖 CARGO_HOME 的提取状态）。

## 来源与升级

- `com/nonpolynomial/**`、`io/github/gedgygedgy/**`：btleplug **0.13.0**
  （上游：`github.com/deviceplug/btleplug`，目录 `src/droidplug/java/src/main/java`）。
- 许可：btleplug 为 `MIT/Apache-2.0/BSD-3-Clause`（见其 `Cargo.toml` 的 `license` 字段）。
- **升级 btleplug 时必须重新 vendor**：把新版本这两个目录整体覆盖过来，并核对
  `src/droidplug/jni/*.rs` 里 `find_class` / `jni_sig!` 用到的类名是否变化
  （改了就要同步 proguard keep 规则与注入脚本里的文件清单）。
