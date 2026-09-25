#!/usr/bin/env node
/**
 * Gosslan 统一验证入口（`npm run verify`）。
 *
 * ## 为什么要有它
 *
 * 这个仓库的验证手段散在很多地方，且此前**没有任何一个地方把它们串起来**：
 *
 *   · `npm test`                        前端断言（条数由它自己打印，写在这里一定会腐烂）
 *   · `cargo test --features bluetooth` Rust 用例（同上）
 *   · `check-test-manifest`             挡住"测试静默不跑"
 *   · `check-invariant-exceptions`      挡住"照文档误修"
 *   · `check-ble-constants`             挡住"BLE 载荷预算多处各算一遍"
 *   · `check-domain-map`                挡住"领域图变成虚构"
 *   · `version:changelog`               CHANGELOG 结构（发布脚本的插入锚点）
 *   · `verify-guards.py`                护栏非空转验证（条数由该工具自己打印）
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
 * ## 用法（2026-09-20 起分两层）
 *
 *     npm run verify              # 快速层：不碰 cargo，秒级~20s（日常迭代就跑这个）
 *     npm run verify:full         # 全量层：加 cargo fmt/clippy/test + Rust 清单 + 护栏扫描 + Android
 *     npm run verify:full -- --full   # 再加**全量**护栏（含 Rust 用例，每条都要重编译，10 分钟以上）
 *                                     #   （`--full` 也会自动拉起重门禁层，不会静默跑成快速层）
 *     npm run verify:full -- --no-guards  # 全量层但跳过护栏扫描（护栏工具缺失时的临时绕过，会留痕）
 *                                     #   （快速层本来就不含护栏扫描，那里加这个参数是无操作）
 *     npm run verify -- --list    # 只列出会跑哪些步骤（加 -- --full-gate 看全量层）
 *     node scripts/verify.mjs --group frontend|rust|android
 *                                 # 只跑某一个 CI job 的步骤（CI 用这个，见下面「CI 归属」）
 *
 * ### 为什么分两层（实测数据，不是猜的）
 *
 * 一次全量 verify = **412s**，但里面**只有 5.1s 真的在执行测试**：
 *   · 177s 冷编译测试产物（583 条用例本身只跑 5s）
 *   · 43s clippy 把同一个 crate 按 check profile **再编一遍**（与 test profile 不共享产物）
 *   · 168s 护栏非空转扫描（每条改坏→跑→还原，顺带把源文件 mtime 弄脏 ⇒ 下一轮又多编 60s）
 *   · 5.1s 真的跑测试
 * 也就是说慢的从来不是"检查"，是"编译"——**改一行注释不该付 400s 的编译税**。
 *
 * ### 为什么这样拆仍然安全（关键前提，别当成"检查变松了"）
 *
 *   1. CI（`verify.yml`）在**任意分支每次 push** 跑全套：前端 job + Rust job（mac/win 矩阵）
 *      + Android job ⇒ 只要推上去，编译一定被验证；
 *   2. 快速层结束时**显式列出**它没跑哪些重门禁（见 `--full-gate` 与汇总段），
 *      绿色只代表"这一层过了"，不代表"编得过" —— 静默省略是本项目最忌讳的事；
 *   3. 该跑的时机写进了 `AI_RULES.md`：**动过 Rust/依赖/构建配置 ⇒ 提交前必须
 *      `npm run verify:full`**；纯文档、纯前端文案/样式 ⇒ 快速层 + CI 兜底。
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
/**
 * 是否跑「重门禁层」（cargo 编译 / 护栏非空转 / Android 交叉编译）。
 *
 * **默认不跑**（2026-09-20 分层）。理由不是"省时间"这么轻：一次全量 verify 实测 412s，
 * 而其中**只有 5.1s 真的在执行测试** —— 见文件头「为什么分两层」的实测拆分。
 *
 * 安全网（分层成立的前提，不是省略检查）：
 * - `.github/workflows/verify.yml` 在**任意分支每次 push** 跑全套
 *   （前端 job + Rust job × mac/win 矩阵 + Android job）⇒ 推上去必然被编译检验；
 * - 快速层结束时**显式列出**没跑哪些重门禁（绝不静默），别把本地绿当成"编得过"；
 * - 动过 Rust/依赖/构建配置 ⇒ 提交前跑 `npm run verify:full`（口径写进 AI_RULES.md）。
 *
 * ⚠️ `--full`（全量护栏）**自动蕴含**本开关：护栏扫描属于重门禁层，若只给 `--full` 又把它
 * 过滤掉，旧写法 `npm run verify -- --full` 会安静地一条护栏都不跑、还打印满屏绿 ——
 * 那是"没守却当作守到了"，比慢得多严重。
 */
const fullGate = argv.includes("--full-gate") || full;

// ⚠️ 快速层不跑护栏 ⇒ 连"找 python"都不做：否则没装 python 的机器上，一个与护栏无关的
// 快速检查会因为找不到解释器直接红（分层时很容易顺手漏掉这一条）。
const python = noGuards || !fullGate ? null : findPython();

/** @type {{name: string, why: string, cwd: string, cmd: string, args: string[],
 *          group: "frontend"|"rust"|"android", skipReason?: string|null}[]} */
const steps = [
  {
    group: "frontend",
    name: "测试清单守卫（前端）",
    why: "挡住「新增 .test.ts 忘了登记进 package.json」这类静默不跑",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-test-manifest.mjs", "--only", "frontend"],
  },
  {
    group: "frontend",
    name: "不变量例外守卫",
    why: "挡住「例外只写在实现旁边、没写进 protocol-invariants.md」导致的照文档误修",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-invariant-exceptions.mjs"],
  },
  {
    group: "frontend",
    name: "db 锁作用域守卫",
    why: "挡住「锁还活着时 emit」——只有一条 SQLite 连接，前端收到事件后的第一次 IPC 抢同一把锁 ⇒ 那一刻界面冻一下；全部取锁点逐处判（条数由该步自己打印，别处不抄），判据自带 7 段夹具自证",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-lock-scope.mjs"],
  },
  {
    group: "frontend",
    name: "文档硬数字对账",
    why: "挡住「文档抄了一份数字、下次改代码没人回来改它」——门禁步数由 verify --list 现算对账；取锁点条数一律禁止手写（曾在四处共存三种写法）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-doc-numbers.mjs"],
  },
  {
    group: "frontend",
    name: "BLE 常量单一事实来源",
    why: "挡住「同一个概念多处各算一遍」——CHANGELOG 4.18.7→4.18.10 连着四版修的就是它",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-ble-constants.mjs"],
  },
  {
    group: "frontend",
    name: "领域图守门",
    why: "挡住「地图变成虚构」——路径存在 / 一文件不属两域 / 无文件漏归属 / enforce 只能开在单家",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-domain-map.mjs"],
  },
  {
    group: "frontend",
    name: "领域依赖方向守门",
    why: "挡住「跨域 use 不声不响」——每个域的 use crate::xxx 必须落在 self 或 consumes 里（生产代码，不扫测试块）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-domain-deps.mjs"],
  },
  {
    group: "frontend",
    name: "Change Budget 守门",
    why: "挡住「改动半径不声明 + 同领域反复打补丁 + 版本声明不成立」——L2 需 [plan]、L3/敏感文件需 [impact]、同领域 3 次修补即红（按 Version-Bump 声明判「修补」，不看 feat/fix 前缀）、声明与四个版本清单文件必须双向一致",
    // 退出码 2 = 受检范围为空（一个 commit 都没判到）。**不能算 ✅**：它既不是通过也不是失败，
    // 而是"这一步没看过任何代码" ⇒ 汇总里单列成未覆盖项（跟重门禁层同一套说法）。
    zeroCoverageExit: 2,
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-change-budget.mjs"],
  },
  {
    group: "frontend",
    name: "CHANGELOG 结构",
    why: "发布脚本按行首 `## [Unreleased]` 插小节；锚点被吞/顺序错乱不报错，只有结构检查能拦",
    cwd: ROOT,
    cmd: NPM,
    args: ["run", "version:changelog"],
  },
  {
    group: "frontend",
    name: "前端测试（npm test）",
    why: "前端全部断言。条数交给 npm test 自己打印 —— 写死在这里一定会腐烂",
    cwd: ROOT,
    cmd: NPM,
    args: ["test"],
  },
  {
    group: "frontend",
    name: "前端静态检查 + 构建（npm run build）",
    why:
      "vue-tsc 类型检查 + vite 产物。留在快速层：它**不碰 cargo**，而且是前端唯一的类型门禁" +
      "（漏了它，写坏 TS 只能等下一次真编译才炸）。dist/ 同时是 cargo 的硬依赖（见文件头第 3 条）。",
    cwd: ROOT,
    cmd: NPM,
    args: ["run", "build"],
  },
  {
    group: "rust",
    name: "cargo fmt --check --all",
    why: "格式统一。2026-09-17 一次性全量重排后零差异，作为门禁防漂移",
    cwd: TAURI,
    cmd: "cargo",
    args: ["fmt", "--check", "--all"],
  },
  {
    group: "rust",
    name: "cargo clippy（-D warnings，--features bluetooth）",
    why: "2026-09-17 清理 26 条 warning 后零红。--features bluetooth 不能省，否则 BLE 模块不编译",
    cwd: TAURI,
    cmd: "cargo",
    args: ["clippy", "--features", "bluetooth", "--", "-D", "warnings"],
  },
  {
    group: "rust",
    name: "Rust 单测（--features bluetooth）",
    why: "--features bluetooth 不能省：漏了会让整批 BLE 用例连同被测代码一起消失（清单守卫兜底）。条数交给 cargo test 与下一步的清单守卫打印",
    cwd: TAURI,
    cmd: "cargo",
    args: ["test", "--features", "bluetooth"],
  },
  {
    group: "rust",
    name: "测试清单守卫（Rust）",
    why: "比对基线名单与实际 --list，缺名即红（正是上一步那个风险的兜底）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-test-manifest.mjs", "--only", "rust"],
  },
];

if (!noGuards) {
  // 条数与耗时都交给该步自己打印（`verify-guards.py` 有 `[n/m]` 进度）——
  // 写死在这里的数字一定会腐烂：这里曾经同时错过"97 条"和"十几秒"两个旧值。
  steps.push({
    group: "frontend",
    name: full ? "护栏非空转（全量，慢）" : "护栏非空转（前端子集）",
    why: full
      ? "全部护栏逐条「改坏必须 FAIL、恢复必须 PASS」；含 Rust 用例 ⇒ 每条都要重编译，比子集慢得多"
      : "只扫打了 frontend 标签的护栏；全量请用 --full（发版/出包前跑一次）",
    cwd: ROOT,
    cmd: python ?? "python3",
    args: ["scripts/verify-guards.py", ...(full ? [] : ["--only", "frontend"])],
  });
}

// Android 编译门禁：**Android 专属代码只有这条腿看得见**（macOS/Windows 的构建都跳过它）。
// CI 里是独立 job、无条件跑；本地缺 NDK / rust target 时**显式跳过并说明**，
// 而不是失败 —— 但也不静默（跳过会记进汇总，且说清 CI 上仍然覆盖）。
steps.push({
  group: "android",
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

/**
 * 「重门禁层」= 需要 cargo 编译、或本身耗时在分钟级的步骤。
 *
 * 判据按**名字与命令**认，不给步骤各加 tier 字段（那会演化成第二份清单，加一步就得记得
 * 同步、漏一个就静默不跑）。新增步骤时如果它要编译，名字里带上 `cargo` / `（Rust）` /
 * `移动端编译门禁` / `护栏非空转` 即可自动归层。
 *
 * ⚠️ 别把 `group`（下一步要讲的 CI 归属）当成本函数的替代品 —— 两者不是一回事：
 * 「护栏非空转（前端子集）」按这里是**重门禁层**（全量子集要重编译 Rust 用例），
 * 但 CI 把它放在 **frontend job**（子集只扫前端护栏，秒级到一分多钟，不碰 cargo）。
 * 合成一个字段就会两头都判错，所以分层继续派生、归属显式声明。
 */
function isHeavyStep(s) {
  return (
    s.cmd === "cargo" ||
    s.name.includes("（Rust）") ||
    s.name.includes("移动端编译门禁") ||
    s.name.includes("护栏非空转")
  );
}

/**
 * CI 归属：这一步由哪个 job 跑（`frontend` / `rust` / `android`）。
 *
 * 为什么要显式写在这一列上：以前"跑哪些检查"这件事在**两处各写一遍**
 * （`scripts/verify.mjs` 的步骤表 + `.github/workflows/verify.yml` 的 `run:` 列表），
 * 于是必然腐烂 —— 现成的证据：verify.yml 头部那句"457 条前端断言 + 503 条 Rust 用例 +
 * 93 条非空转护栏"早就与实际不符（今天 Rust 用例是 500 多条起步、护栏条数也在长）。
 * 单源化之后 CI 只说"跑哪个组"，清单只在这里有一份。
 *
 * 归属**不派生**自名字：它表达的是"该由哪台机器、带着什么工具链去跑"，
 * 与"快不快/要不要编译"是两条正交的轴（见上面 isHeavyStep 的 ⚠️）。
 *
 * 强制声明：漏写 `group` 的步骤会在下面当场红 —— 因为静默漏掉的后果正是
 * "本地有这条门禁、CI 永远不跑"，那比没有更危险。
 *
 * ⚠️ 但 `local` **不是第四个 CI job**，它是显式反过来的那一格："这一层 CI 就是跑不动，
 * 并且要让所有人看见它没跑"。真把它并进 frontend/rust，CI 不会变强，只会把一个需要
 * release 产物 + GUI 会话 + 百 MB 磁盘的层变成一条永远红的 job。
 */
const GROUPS = ["frontend", "rust", "android", "local"];

/** `--group <frontend|rust|android>`：CI 的三个 job 各自只说"跑哪个组"，清单不再抄一份。 */
const groupFlag = (() => {
  const i = argv.indexOf("--group");
  return i >= 0 ? argv[i + 1] : null;
})();
if (groupFlag && !GROUPS.includes(groupFlag)) {
  console.error(`✗ --group 只接受 ${GROUPS.join(" / ")}，收到 "${groupFlag}"`);
  process.exit(1);
}

/**
 * 「本地专项层」（§十五要的 test:multi-instance / test:fault-injection）。
 *
 * 之前这四轮正向 E2E 只活在 package.json 的脚本里 —— `grep e2e-multi-instance scripts/verify.mjs`
 * 零命中 ⇒ 没有任何门禁会跑它，等于"某人记得跑"才算跑过。现在接进同一个入口。
 *
 * 但**不假装 CI 覆盖了它**：归属列写 frontend/rust 会造出"CI 有这条 job"的假象，而 CI 现在真跑不动
 * （要 release 产物 + 桌面 GUI 会话 + 百 MB 磁盘，Windows 那条腿还卡在 #47）。所以用一个显式的
 * `local` 组声明"这一层 CI 不跑"，并且**默认层与全量层都不收它** ⇒ 快速层/全量层的计数一位没动
 * （那两个数有 `check-doc-numbers.mjs` 对着 `--list` 现算对账）。
 */
const LOCAL_ONLY_WHY =
  "本地专项：要 release 产物 + 桌面 GUI 会话 + 百 MB 级磁盘，CI 现在跑不动（Windows 腿卡在 #47）⇒ 宁可不跑，不许把没跑写成绿";
if (groupFlag === "local") {
  steps.push(
    {
      group: "local",
      name: "双实例 E2E：默认轮（文本 + 文件 + 重启）",
      why: `两个真实 release 进程对发，断言两侧 DB/日志/磁盘收敛一致；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs"],
    },
    {
      group: "local",
      name: "双实例 E2E：脏 .part 前缀注入",
      why: `接收侧预置脏前缀 ⇒ 整体校验必须拦下、重试补齐；产品要么自愈要么明确失败，不许假 done；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=poison-part"],
    },
    {
      group: "local",
      name: "双实例 E2E：真前缀必须被续传复用",
      why: `接收侧已有源文件自己的前缀 ⇒ 发送端不许从 0 重灌整份（160 MB 真机事故那一格）；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=resume-prefix"],
    },
    {
      group: "local",
      name: "双实例 E2E：接收中 SIGKILL 后重启续完",
      why: `百 MB 文件在飞时杀掉接收端 ⇒ 半路不许出现终名、发送侧不许报 done，重启后按盘上真实字节续完；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=kill-mid"],
    },
    {
      group: "local",
      name: "双实例 E2E：对端失联（进程被冻住）后解冻续完",
      why: `SIGSTOP 冻住接收端 ⇒ 失联期间两侧都不许 done、接收目录不许出现终名；解冻后对端自己补齐且只成功一次。` +
        `⚠️ 这一轮**没有**覆盖"到点重投"：实测失联期间发送侧一次都没尝试（file_outbox 只由链路事件驱动 flush，没有到点定时器），` +
        `所以也**不能**声称钉住了"write 成功 ≠ 已送达"；该缺口按 A 类风险登记在 roadmap，修好前别改口；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=peer-freeze"],
    },
  );
} else if (!groupFlag) {
  // 不跑也要说出来 —— 一片绿暗示"全跑过了"正是这套门禁最反对的样子。
  // 不写轮数：这一层的条目由上面的步骤表自己点名，写死数字就是下一个漂移点（同 CHANGELOG 的口径）。
  console.log(`· 本地专项层（多实例 E2E，条目见 npm run verify:e2e）本轮没跑：${LOCAL_ONLY_WHY}`);
  console.log("  要跑它：npm run verify:e2e（或 --group local）");
}

/** 列出"不属于本组"的步骤：组模式下也必须点名未跑项，不许用一片绿暗示"全跑过了"。 */
function printNotRun(title) {
  const others = steps.filter((s) => s.group !== groupFlag);
  console.log(`  ── ${title}，共 ${others.length} 项：`);
  for (const s of others) console.log(`     · [${s.group}] ${s.name}`);
}

const active = groupFlag
  ? steps.filter((s) => s.group === groupFlag)
  : fullGate
    ? steps
    : steps.filter((s) => !isHeavyStep(s));
const heldOut = steps.length - active.length;

/**
 * 每个步骤都必须声明 CI 归属。
 *
 * 漏声明的后果不是"少了一行配置"，而是**这条门禁本地跑、CI 永远不跑** —— 而且没人会
 * 注意到，因为两边的清单看起来都是绿的。归属与分层是两条正交的轴（见 isHeavyStep 的 ⚠️），
 * 所以这一列不能从名字派生，只能显式写、并强制检查。
 */
{
  const missing = steps.filter((s) => !GROUPS.includes(s.group));
  if (missing.length > 0) {
    console.error(`✗ 以下步骤没声明 CI 归属（group ∈ ${GROUPS.join("/")}）：`);
    for (const s of missing) console.error(`   · ${s.name}`);
    console.error("  后果：CI 的 `run:` 清单与这里各写一份 ⇒ 必然腐烂（verify.yml 头部那串");
    console.error("        “457/503/93 条”就是烂掉的样子）；漏声明 = 这条门禁 CI 从不执行。");
    process.exit(1);
  }
}

/**
 * 会不会拉起 Rust 工具链（`bash` 与 `python` 也算：`check-mobile.sh` 与 `verify-guards.py`
 * 内部都在跑 cargo）。
 */
function mayTouchToolchain(s) {
  return (
    s.cmd === "cargo" ||
    s.cmd === "rustup" ||
    s.cmd === "bash" ||
    s.cmd === "python3" ||
    s.cmd === "python" ||
    s.args.some((a) => typeof a === "string" && a.startsWith("cargo"))
  );
}

/**
 * 分层**自证**：快速层里不许出现"会被识别为碰工具链、却没归进重门禁层"的步骤。
 *
 * 为什么要有它：`isHeavyStep` 是按名字/命令认的启发式。将来有人加一条
 * `{cmd:"bash", args:["scripts/x.sh"]}` 名叫「某项检查」——它内部可能跑 cargo，
 * 却会因为名字没带关键词而**悄悄落进快速层**，表现是"快速层怎么突然三分钟了"，
 * 没人会去查原因（本项目铁律：会让人想绕过的门禁等于没有门禁）。这里当场红并给出修法。
 *
 * ⚠️ 覆盖面与盲区（别把它当成"分层已被机器守住"）：
 *   · 守得住：`cmd` 是 cargo / rustup / bash / python*，或 args 里出现 `cargo*` 却没归层；
 *   · 守不住：经由 npm/npx 间接拉起工具链的步骤（如 `npm run tauri build`）—— 文本判据
 *     看不出它会编译。这类只能靠命名约定，好在漏判的后果是"快速层变慢"（可见），
 *     不是"门禁被跳过"（安全方向）；
 *   · **本条自证已做过非空转验证**（v4.22.40）：`verify-guards.py` 里
 *     「门禁分层的自证不是装饰」那条 —— 注入方式是把 `移动端编译门禁（Android）`
 *     改名去掉关键词，于是它 `cmd:"bash"` 会碰工具链却不再被 `isHeavyStep` 归走 ⇒
 *     快速层必须以退出码 1 红掉。
 *     （顺带修正一段历史自证：v4.22.31 当时用"注入一条 `cmd:"cargo"` 的步骤"来证，
 *     那是**无效注入** —— cargo 步骤本来就被归走，永远不会漏，那次"通过"什么也没证明。）
 */
{
  // 组模式跳过这条：它判的是"快速层有没有混进碰工具链的步骤"，而 `--group` 是按 CI job
  // 选步骤（frontend 组**故意**含护栏非空转那条重门禁），拿分层尺子去量会假红。
  const leaked =
    !fullGate && !groupFlag
      ? active.filter((s) => mayTouchToolchain(s) && !isHeavyStep(s))
      : [];
  if (leaked && leaked.length > 0) {
    console.error("✗ 门禁分层自检失败：以下步骤会碰 Rust 工具链，却没被归进重门禁层：");
    for (const s of leaked) console.error(`   · ${s.name}（cmd=${s.cmd}）`);
    console.error("  后果：`npm run verify` 从秒级退化成几分钟，且没人看得出为什么。");
    console.error("  修法：让步骤名命中 isHeavyStep 的关键词之一（cargo / （Rust） /");
    console.error("        移动端编译门禁 / 护栏非空转），或把 cmd 换成不碰工具链的入口。");
    process.exit(1);
  }
}

if (listOnly) {
  console.log(
    `npm run verify${groupFlag ? ` -- --group ${groupFlag}` : fullGate ? " -- --full-gate" : ""} 会按顺序跑：`,
  );
  active.forEach((s, i) =>
    console.log(
      `  ${i + 1}. ${s.name} —— ${s.why}` +
        (s.skipReason ? `\n      ⏭ 本机将跳过：${s.skipReason}` : ""),
    ),
  );
  if (groupFlag) {
    printNotRun(`其它 CI job 的组（--group ${groupFlag} 不跑）`);
  } else if (!fullGate) {
    console.log("  ── 以下重门禁层默认不跑（`npm run verify:full` 全跑，CI 每次 push 全跑）：");
    steps.filter(isHeavyStep).forEach((s) => console.log(`     · ${s.name}`));
  }
  process.exit(0);
}

// ⚠️ 只在护栏**真的要跑**时才要求 python：快速层不跑护栏，没装 python 也必须能过。
// 但也不能静默：全量层里找不到解释器就是硬失败（本项目最忌讳"没守却当作守到了"）。
if (!noGuards && fullGate && !python) {
  console.error("✗ 找不到 python（试过 python3 / python），无法跑护栏非空转验证。");
  console.error("  verify-guards.py 是本项目的既有护栏工具，Windows 上一直在用 ——");
  console.error("  请装 python3，或明确用 `npm run verify:full -- --no-guards` 跳过（会在输出里留痕）。");
  process.exit(1);
}

// dist/ 缺失时提前说清楚，避免让人对着 cargo 的 proc-macro panic 发愣。
if (!existsSync(path.join(ROOT, "dist")) && active.some((s) => s.cwd === TAURI)) {
  console.log("提示：dist/ 不存在 —— 前端构建步骤会先产出它（cargo 的 build.rs 依赖）。\n");
}

console.log(
  `=== Gosslan verify —— ${fullGate ? "全量层（含 cargo 编译 / 护栏扫描 / Android）" : "快速层（不编译）"} ===\n`,
);

const results = [];
const t0 = Date.now();

for (const [i, s] of active.entries()) {
  const label = `[${i + 1}/${active.length}] ${s.name}`;
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
  // 退出码 `zeroCoverageExit` 单独一类：命令跑了、但范围内没有任何东西可判 ⇒ 不红，
  // 但也**绝不记成 ✅**（本项目最忌讳"没守却当作守了"）。
  const zero = s.zeroCoverageExit != null && r.status === s.zeroCoverageExit;
  const ok = r.status === 0 || zero;
  results.push({ name: s.name, ok, secs, zeroCoverage: zero });

  if (!ok) {
    console.error(
      `\n❌ 第 ${i + 1} 步失败：${s.name}（${secs}s）` +
        (r.status === null ? `（未能执行：${r.error?.message ?? "未知原因"}）` : ""),
    );
    console.error("   后面的步骤没有跑（fail-fast）。");
    break;
  }
  console.log(
    zero ? `      ⚠️ 零覆盖 ${secs}s（跑了，但范围内没有可判定的对象）\n` : `      ✅ ${secs}s\n`,
  );
}

const total = ((Date.now() - t0) / 1000).toFixed(1);
const failed = results.filter((r) => !r.ok);
const notRun = active.length - results.length;

console.log("--- 汇总 ---");
for (const r of results) {
  const mark = r.skipped ? "⏭" : r.zeroCoverage ? "⚠️" : r.ok ? "✅" : "❌";
  const tail = r.skipped
    ? `跳过（${r.skipped}）`
    : r.zeroCoverage
      ? `零覆盖 ${r.secs}s（没判到任何对象）`
      : `${r.secs}s`;
  console.log(`  ${mark} ${r.name}  ${tail}`);
}
for (const s of active.slice(results.length)) {
  console.log(`  ⏭   ${s.name}（未跑）`);
}
// ⚠️ 快速层/组模式都必须**点名**它没跑什么：本项目最忌讳的就是"没守却当作守了"。
// 绿色只表示"这一层的门禁过了"，不表示"代码编得过"，也不表示"别的 job 那组过了"。
if (groupFlag && failed.length === 0) {
  printNotRun(`不属于 --group ${groupFlag}`);
  console.log(
    "     → 本组绿 ≠ 全套绿：CI 三个 job 各跑一组，本地一把跑全用 `npm run verify:full`。",
  );
} else if (!groupFlag && !fullGate && failed.length === 0) {
  console.log(`  ── ${heldOut} 项重门禁**本次未跑**：`);
  for (const s of steps.filter(isHeavyStep)) console.log(`     · ${s.name}`);
  console.log(
    "     → 提交/打包前跑 `npm run verify:full`；push 到任意分支时 CI 会全跑\n" +
      "       （verify.yml：前端 job + Rust job（mac/win）+ Android job）。",
  );
}

const skipped = results.filter((r) => r.skipped).length;
const zeroCov = results.filter((r) => r.zeroCoverage);
const ran = results.filter((r) => !r.skipped).length;

if (failed.length === 0) {
  console.log(
    `\n✅ ${
      groupFlag ? `--group ${groupFlag} 那一组` : fullGate ? "全部门禁" : "快速层门禁"
    }通过（${ran} 步${skipped ? `，跳过 ${skipped} 步` : ""}，共 ${total}s）`,
  );
  if (skipped) console.log("   ⚠️ 被跳过的步骤在 CI 上仍会跑 —— 别把本地的 ⏭ 当成通过。");
  if (zeroCov.length) {
    console.log(`   ⚠️ ${zeroCov.length} 步**零覆盖**（跑了但没判到东西）：${zeroCov.map((r) => r.name).join("、")}`);
    console.log("      最常见成因：本地在 push 之后重跑（`origin/main..HEAD` 已为空）。");
    console.log("      零覆盖 ≠ 守住 —— 要看它真判了什么，用 `node scripts/check-change-budget.mjs --range a..b`。");
  }
  process.exit(0);
}
console.error(`\n❌ 失败 ${failed.length} 步${notRun ? `，未跑 ${notRun} 步` : ""}（共 ${total}s）`);
process.exit(1);
