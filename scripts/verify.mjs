#!/usr/bin/env node
/**
 * Gosslan 统一验证入口（`npm run verify`）。
 *
 * ## 为什么要有它
 *
 * 这个仓库的验证手段散在很多地方，且此前**没有任何一个地方把它们串起来**：
 *
 *   · `npm test`                        前端断言（457）
 *   · `cargo test --features bluetooth` Rust 用例（505）
 *   · `check-test-manifest`             挡住"测试静默不跑"
 *   · `check-invariant-exceptions`      挡住"照文档误修"
 *   · `check-ble-constants`             挡住"BLE 载荷预算多处各算一遍"
 *   · `check-domain-map`                挡住"领域图变成虚构"
 *   · `version:changelog`               CHANGELOG 结构（发布脚本的插入锚点）
 *   · `verify-guards.py`                非空转验证（100 条护栏）
 *   · `npm run build`                   前端构建
 *   · `check-mobile.sh`                 Android 编译门禁（Android 专属代码只有它看得见）
 *
 * ⚠️ **Android 那一步在本地可能被跳过**（缺 NDK / 缺 rust target）：跳过会显式打印原因并
 * 记进汇总，**不会**当成失败 —— 但 CI 上它是独立 job、无条件跑，所以本地跳过不影响覆盖。
 * 别把本地的 ⏭ 当成通过。
 *
 * 后果是每个入口各记一部分：CI 里记一份、`build-windows-release.ps1` 里手串一份、
 * 开发者脑子里再记一份。**"验证"没有单一入口，就等于没有统一的验证标准** ——
 * 谁记得跑什么，就跑什么。
 *
 * 这个脚本把顺序固定下来（顺序本身有讲究，见下面每一步的注释），并让
 * CI、发布脚本、本地开发**跑同一套**。
 *
 * ## 用法
 *
 *     npm run verify              # 默认：全部步骤，护栏只跑前端子集（约 1~2 分钟）
 *     npm run verify -- --full    # 再加完整护栏（Rust 用例要重编译，10 分钟以上）
 *     npm run verify -- --no-guards  # 跳过护栏（不推荐；仅用于护栏工具缺失时的临时绕过）
 *     npm run verify -- --list    # 只列出会跑哪些步骤
 *
 * ## 顺序为什么是这样（不要随手调换）
 *
 *   1. 两个**静态守卫**放最前：它们不编译、秒级，且失败原因最明确 —— 先跑最便宜、
 *      信息量最大的检查。
 *   2. `npm test` 在构建之前：断言挂了就没必要再花时间构建。
 *   3. **`npm run build` 必须在 `cargo test` 之前**。这不是优化，是硬依赖：
 *      `dist/` 在 .gitignore 里、不进版本库，而 Tauri 的 `build.rs` 要读它。
 *      全新 clone 上顺序错了会得到：
 *          error: proc macro panicked
 *            = help: message: The `frontendDist` configuration is set to "../dist"
 *                            but this path doesn't exist
 *      即 `cargo test` **根本编不过**。
 *   4. Rust 单测之后紧跟 Rust 清单守卫：后者用 `--list` 取名单，此时编译缓存是热的。
 *   5. 护栏放最后：全量跑要重编译 Rust，最慢。
 *
 * ## 有意**没有**包含 `npm run version:check`
 *
 * 它当前是**红的**（`29ee060` 与 `7c03341` 两个历史提交缺 `Version-Bump:` 声明）。
 * 把它塞进来会让这个入口一出生就是红的，而红的入口等于没有入口 ——
 * 大家会立刻开始绕过它。等那两个提交在后续发布流程里被消化掉之后再加。
 * 同理，`cargo fmt --check`（517 处差异）与 `clippy`（未安装）也不在这里。
 *
 * ⚠️ 但 `version:check` 里**混着**一件与记账无关的事：CHANGELOG 结构检查
 * （唯一行首 `## [Unreleased]` 锚点 / 标题格式 / 新在前降序）。它现在有独立入口
 * `npm run version:changelog`，**已纳入本入口** —— 结构是结构、记账是记账，
 * 不该因为"版本还没发"就连结构检查一起红掉（这正是它此前被判成护栏失效的原因）。
 *
 * 退出码：0 = 全部通过；1 = 有步骤失败（默认 fail-fast，不继续跑后面的）。
 */

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const TAURI = path.join(ROOT, "src-tauri");
const WIN = process.platform === "win32";

// Windows 的 `shell:true` 会按空格截断命令：可执行路径必须自己带引号（Program Files）。
// ⚠️ 只在 Windows 加引号：macOS/Linux 走 `shell:false`，引号会被当成文件名的**一部分**
// —— 结果是第 1 步直接 `ENOENT`，整条 verify 在 unix 上从第一步就红（真踩到 2026-09-20）。
const NODE_EXE = WIN ? '"' + process.execPath + '"' : process.execPath;

/** npm 在 Windows 上是 npm.cmd；不处理会 spawn 失败。 */
const NPM = WIN ? "npm.cmd" : "npm";

/**
 * 找一个可用的 python 解释器。
 *
 * Windows 上通常只有 `python`（没有 `python3`），macOS/Linux 上反过来。
 * `verify-guards.py` 是本项目的既有护栏工具，Windows 上一直在用（见该文件里
 * 2026-09-13 的注释），所以这里只需要把名字找对。
 */
function findPython() {
  for (const c of WIN ? ["python", "python3"] : ["python3", "python"]) {
    const r = spawnSync(c, ["--version"], { stdio: "ignore" });
    if (r.status === 0) return c;
  }
  return null;
}

const argv = process.argv.slice(2);
const full = argv.includes("--full");
const noGuards = argv.includes("--no-guards");
const listOnly = argv.includes("--list");

const python = noGuards ? null : findPython();

/** @type {{name: string, cwd: string, cmd: string, args: string[], why: string}[]} */
const steps = [
  {
    name: "测试清单守卫（前端）",
    why: "挡住「新增 .test.ts 忘了登记进 package.json」这类静默不跑",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-test-manifest.mjs", "--only", "frontend"],
  },
  {
    name: "不变量例外守卫",
    why: "挡住「例外只写在实现旁边、没写进 protocol-invariants.md」导致的照文档误修",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-invariant-exceptions.mjs"],
  },
  {
    name: "BLE 常量单一事实来源",
    why: "挡住「同一个概念多处各算一遍」——CHANGELOG 4.18.7→4.18.10 连着四版修的就是它",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-ble-constants.mjs"],
  },
  {
    name: "领域图守门",
    why: "挡住「地图变成虚构」——路径存在 / 一文件不属两域 / 无文件漏归属 / enforce 只能开在单家",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-domain-map.mjs"],
  },
  {
    name: "领域依赖方向守门",
    why: "挡住「跨域 use 不声不响」——每个域的 use crate::xxx 必须落在 self 或 consumes 里（生产代码，不扫测试块）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-domain-deps.mjs"],
  },
  {
    name: "Change Budget 守门",
    why: "挡住「改动半径不声明 + 同领域反复打补丁」——L2 需 [plan]、L3/敏感文件需 [impact]、同领域 3 次 fix 即红",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-change-budget.mjs"],
  },
  {
    name: "CHANGELOG 结构",
    why: "发布脚本按行首 `## [Unreleased]` 插小节；锚点被吞/顺序错乱不报错，只有结构检查能拦",
    cwd: ROOT,
    cmd: NPM,
    args: ["run", "version:changelog"],
  },
  {
    name: "前端测试（npm test）",
    why: "457 条前端断言",
    cwd: ROOT,
    cmd: NPM,
    args: ["test"],
  },
  {
    name: "前端构建（npm run build）",
    why: "不只是构建产物：cargo test 依赖 dist/ 存在（见文件头第 3 条）",
    cwd: ROOT,
    cmd: NPM,
    args: ["run", "build"],
  },
  {
    name: "cargo fmt --check --all",
    why: "格式统一。2026-09-17 一次性全量重排后零差异，作为门禁防漂移",
    cwd: TAURI,
    cmd: "cargo",
    args: ["fmt", "--check", "--all"],
  },
  {
    name: "cargo clippy（-D warnings，--features bluetooth）",
    why: "2026-09-17 清理 26 条 warning 后零红。--features bluetooth 不能省，否则 BLE 模块不编译",
    cwd: TAURI,
    cmd: "cargo",
    args: ["clippy", "--features", "bluetooth", "--", "-D", "warnings"],
  },
  {
    name: "Rust 单测（--features bluetooth）",
    why: "505 条用例。--features bluetooth 不能省：漏了会让 16 条 BLE 用例静默消失",
    cwd: TAURI,
    cmd: "cargo",
    args: ["test", "--features", "bluetooth"],
  },
  {
    name: "测试清单守卫（Rust）",
    why: "比对基线名单与实际 --list，缺名即红（正是上一步那个风险的兜底）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-test-manifest.mjs", "--only", "rust"],
  },
];

if (!noGuards) {
  steps.push({
    name: full ? "护栏非空转（全量，慢）" : "护栏非空转（前端子集）",
    why: full
      ? "97 条护栏全部「改坏必须 FAIL、恢复必须 PASS」；Rust 用例要重编译，10 分钟以上"
      : "前端子集十几秒；全量护栏请用 --full（发版前跑一次）",
    cwd: ROOT,
    cmd: python ?? "python3",
    args: ["scripts/verify-guards.py", ...(full ? [] : ["--only", "frontend"])],
  });
}

// Android 编译门禁：**Android 专属代码只有这条腿看得见**（macOS/Windows 的构建都跳过它）。
// CI 里是独立 job、无条件跑；本地缺 NDK / rust target 时**显式跳过并说明**，
// 而不是失败 —— 但也不静默（跳过会记进汇总，且说清 CI 上仍然覆盖）。
steps.push({
  name: "移动端编译门禁（Android）",
  why: "cargo check --target aarch64-linux-android（0 warning）—— 拦住「移动端整包编不出来」",
  cwd: ROOT,
  cmd: "bash",
  args: ["scripts/check-mobile.sh", "--bluetooth"],
  skipReason: mobileSkipReason(),
});

/** 本地跑不了 Android 门禁时给出原因（CI 上无条件跑，所以本地跳过不影响覆盖）。 */
function mobileSkipReason() {
  const target = "aarch64-linux-android";
  const installed = spawnSync("rustup", ["target", "list", "--installed"], { encoding: "utf8" });
  if (installed.status !== 0 || !installed.stdout?.includes(target)) {
    return `未装 rust target ${target}（rustup target add ${target}）`;
  }
  const bases = [
    process.env.ANDROID_NDK_ROOT,
    process.env.ANDROID_HOME && path.join(process.env.ANDROID_HOME, "ndk"),
    path.join(process.env.HOME ?? "", "Library", "Android", "sdk", "ndk"),
  ].filter(Boolean);
  if (!bases.some((b) => existsSync(b))) {
    return "未找到 Android NDK（装 NDK 或设 ANDROID_NDK_ROOT）";
  }
  return null;
}

if (listOnly) {
  console.log("npm run verify 会按顺序跑：");
  steps.forEach((s, i) =>
    console.log(
      `  ${i + 1}. ${s.name} —— ${s.why}` +
        (s.skipReason ? `\n      ⏭ 本机将跳过：${s.skipReason}` : ""),
    ),
  );
  process.exit(0);
}

if (!noGuards && !python) {
  // 不静默跳过：本项目最忌讳的就是"没有守到却当作守到了"。
  console.error("✗ 找不到 python（试过 python3 / python），无法跑护栏非空转验证。");
  console.error("  verify-guards.py 是本项目的既有护栏工具，Windows 上一直在用 ——");
  console.error("  请装 python3，或明确用 `npm run verify -- --no-guards` 跳过（会在输出里留痕）。");
  process.exit(1);
}

// dist/ 缺失时提前说清楚，避免让人对着 cargo 的 proc-macro panic 发愣。
if (!existsSync(path.join(ROOT, "dist")) && steps.some((s) => s.cwd === TAURI)) {
  console.log("提示：dist/ 不存在，第 4 步会先构建前端（cargo test 依赖它）。\n");
}

console.log("=== Gosslan verify ===\n");

const results = [];
const t0 = Date.now();

for (const [i, s] of steps.entries()) {
  const label = `[${i + 1}/${steps.length}] ${s.name}`;
  console.log(`${label}`);
  console.log(`      ${s.why}`);

  if (s.skipReason) {
    // 显式跳过（不是静默）：原因会进汇总。CI 上这一步无条件跑，所以本地跳过不影响覆盖。
    console.log(`      ⏭  跳过：${s.skipReason}\n`);
    results.push({ name: s.name, ok: true, secs: "—", skipped: s.skipReason });
    continue;
  }

  const start = Date.now();
  // ⚠️ Windows 上必须 `shell: true`：自 Node 18.20 / 20.12 起，`spawnSync` **不能**直接
  // 拉起 `.cmd`（npm 在 Windows 上就是 `npm.cmd`），否则报 `EINVAL` —— 本脚本的
  // 第 7/8 步（`npm run version:changelog`、`npm test`、`npm run build`）会全部"未能执行"。
  // 只对 Windows 打开：POSIX 上 npm 是普通可执行文件，多一层 shell 只会引入额外的引号语义。
  const r = spawnSync(s.cmd, s.args, { cwd: s.cwd, stdio: "inherit", shell: WIN });
  const secs = ((Date.now() - start) / 1000).toFixed(1);

  // status 为 null 表示被信号杀掉（或命令没跑起来）。
  const ok = r.status === 0;
  results.push({ name: s.name, ok, secs });

  if (!ok) {
    console.error(
      `\n❌ 第 ${i + 1} 步失败：${s.name}（${secs}s）` +
        (r.status === null ? `（未能执行：${r.error?.message ?? "未知原因"}）` : ""),
    );
    console.error("   后面的步骤没有跑（fail-fast）。");
    break;
  }
  console.log(`      ✅ ${secs}s\n`);
}

const total = ((Date.now() - t0) / 1000).toFixed(1);
const failed = results.filter((r) => !r.ok);
const notRun = steps.length - results.length;

console.log("--- 汇总 ---");
for (const r of results) {
  const mark = r.skipped ? "⏭" : r.ok ? "✅" : "❌";
  const tail = r.skipped ? `跳过（${r.skipped}）` : `${r.secs}s`;
  console.log(`  ${mark} ${r.name}  ${tail}`);
}
for (const s of steps.slice(results.length)) {
  console.log(`  ⏭   ${s.name}（未跑）`);
}

const skipped = results.filter((r) => r.skipped).length;
const ran = results.filter((r) => !r.skipped).length;

if (failed.length === 0) {
  console.log(
    `\n✅ 全部通过（${ran} 步${skipped ? `，跳过 ${skipped} 步` : ""}，共 ${total}s）`,
  );
  if (skipped) console.log("   ⚠️ 被跳过的步骤在 CI 上仍会跑 —— 别把本地的 ⏭ 当成通过。");
  process.exit(0);
}
console.error(`\n❌ 失败 ${failed.length} 步${notRun ? `，未跑 ${notRun} 步` : ""}（共 ${total}s）`);
process.exit(1);
