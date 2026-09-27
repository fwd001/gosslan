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
 *     node scripts/verify.mjs --group <组名>      # 组名只认下面 GROUPS 那份，非法值会自报全部合法值
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
 * 退出码：0 = 全部通过；1 = 有步骤失败。
 * 默认 fail-fast（第一步红就停），**例外**是 `--group local` / `--group release`：
 * 那两层的每一步都是独立的一轮 E2E ⇒ 默认跑完再汇总（见下面 `KEEP_GOING` 那段，红照样 exit 1）。
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
    name: "不变量钩子守卫",
    why: "挡住「文档写着'必须'、下面没有任何东西在跑」——每条 INV-P 的 `- 钩子：` 必须当场解析成真实存在的"
      + "用例/护栏/夹具；测试改名或被删而文档没回来改，这里就红（条数由该步自己打印）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-invariant-hooks.mjs"],
  },
  {
    group: "frontend",
    name: "私钥边界守卫",
    why: "钉住 INV-P18：可序列化类型（=命令返回值与 emit 载荷的共同要求）不许带密钥字段，"
      + "命令也不许把密钥放在交出值的位置；破掉时**界面完全无症状**，所以只能靠机器判（条数由该步打印）",
    cwd: ROOT,
    cmd: NODE_EXE,
    args: ["scripts/check-key-boundary.mjs"],
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
const GROUPS = ["frontend", "rust", "android", "local", "release"];

/**
 * `--group <组名>`：组名的唯一真源是下面这份 `GROUPS`（别在注释或文档里再抄一份枚举 ——
 * 抄过一次就会漂：这里曾长期写着 "frontend|rust|android" 三个，而 `local`/`release` 落地后没人回头改）。
 * CI 的三个 job 各取 frontend / rust / android；local 与 release 是本地专项层。
 */
const groupFlag = (() => {
  const i = argv.indexOf("--group");
  return i >= 0 ? argv[i + 1] : null;
})();
if (groupFlag && !GROUPS.includes(groupFlag)) {
  console.error(`✗ --group 只接受 ${GROUPS.join(" / ")}，收到 "${groupFlag}"`);
  process.exit(1);
}

/**
 * `local` / `release` 这两层的每一步都是**独立的一轮 E2E**，步骤之间没有"前一步不成就不能跑"的依赖
 * ⇒ 默认**跑完再汇总**（要回旧行为：加 `--fail-fast`）。
 * 为什么只有这一族反过来：fail-fast 省的只是时间，而**红的那一刻恰恰是最需要证据的时候**。
 * 现场（2026-09-27 实测）：屏幕锁着 ⇒ 「报告带全屏帧」那一格按环境红 ⇒ 旧默认让本地层
 * 第 1 轮 0.8s 判负、**后面 13 轮全部标"未跑"**，一次环境问题吃掉整层证据。
 * ⚠️ 快速层/全量层**不改**：那里的步骤真有"编译 → 测试"的依赖，继续跑只会堆出一屏级联红。
 * 退出码语义不变：**任何一步红 ⇒ 仍然 exit 1**，跑更多只是多拿证据，不是把红洗白。
 */
const KEEP_GOING =
  !argv.includes("--fail-fast") && (groupFlag === "local" || groupFlag === "release");

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
      name: "UI 运行时探针：键盘出口 + 任务卡片真渲染（浏览器内真按键）",
      why:
        `§19/§20 那一格唯一能自动化的形状：焦点落点、方向键走格子、Esc 关完之后焦点去哪、` +
        `回车打开的是不是高亮那一条 —— 这些只有走过**真浏览器输入管线**才看得见，` +
        `而仓里没有 DOM 测试地基（没有 jsdom / vitest / @vue/test-utils / playwright 依赖）。` +
        `这一条用「vite dev + 本机 playwright 缓存里的 Chrome for Testing + Node 内置 WebSocket 讲 CDP」，` +
        `**不给 package.json 加任何依赖**（加一层测试框架属于"为稳定性任务改动项目外的东西"）。` +
        `★ 四把非空转都是当场跑过的：① 把组件里的 COLS 从 8 改成 7（布局仍是 8 列）⇒ ` +
        `恰好"↓ 走整行"与"↓ 之后 x 不变"两条红、其余 11 条照常绿；` +
        `② 把 #92 修掉的那行改回旧行为（回车固定开第一条）⇒ 恰好"emit 的必须是高亮那条"红，` +
        `报错里直接印出 msgId=m0 而高亮在 index 2。另外 Enter/空格不带 text 时零次激活，` +
        `这一条被固定成正向跑里的一条**反向对照判据**（不是装饰：没有它，"回车能选中"可能是探针自造的假绿）。` +
        `③ 摘掉任务卡缩略图的 clickable（= #112 修复前的真形状）⇒ 恰好「键盘可达」与「Enter 开统一预览」两条红；` +
        `④ 把卡片「查实时状态表」改回「只读快照」（= #23 修复前的形状）⇒ 恰好那条状态换档的判据红。` +
        `★ 同批还判上 #97 那半句「那张卡真出现在时间线里」：标题在 DOM 里、载荷内部字段名不外露、` +
        `同一句 @ 在两种视角下换标签、底部入口 emit 的是这条的 todo_id。` +
        `⚠️ 顺手记一条夹具自己的坑：props 对象必须建在渲染函数里面 —— 提到外面求值一次就把 open 冻住，` +
        `表情面板整块不渲染；是隔壁那两段把它照出来的 ⇒ **改夹具要跑**全部段**（段数由 `grep -c "^async function run" scripts/check-ui-runtime.mjs` 现算），别只跑新写的那一段**。` +
        `⚠️ 两条口径边界：① 这是**浏览器内**，WKWebView / WebView2 一律未证 ⇒ 那一半仍是人工（Smoke-11）；` +
        `② 缺 Chrome for Testing 时这一条是**红**，不是跳过（§十 不许把没跑写成 PASS），` +
        `CI 不跑这一层所以不会因此变红。${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/check-ui-runtime.mjs"],
    },
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
      name: "双实例 E2E：尺寸阶梯 · 1 KB（极小档）",
      why: `同一趟旅程（文本 + 文件 + 重启）换 **1 KB** 重跑，证明默认轮不是"只在 1 MB 上成立"。` +
        `极小档专打两件事：单片文件（不足一片）不许在收尾前被当成完成、` +
        `以及"零头即整份"时进度与终态仍然自洽。断言条数与默认轮同源（阶梯不加新断言，只换尺寸）；` +
        `${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--size=0.001"],
    },
    {
      group: "local",
      name: "双实例 E2E：尺寸阶梯 · 10 MB（中段多片）",
      why: `同一趟旅程换 **10 MB** 重跑：这是"多片 + 最后一片是零头"那一格 —— ` +
        `阶梯里最容易藏 off-by-one 的位置（片数 = ⌈size/片长⌉，收尾只认最后一次 rename）。` +
        `⚠️ 边界：100 MB 及以上不进本地层（一次几分钟、CI 与本地都会烂），` +
        `那一档按 §十 记 SIMULATED/真机，由 `+"`E2E_KILL_MB`"+` 那条杀进程轮代偿覆盖大文件窗口；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--size=10"],
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
    {
      group: "local",
      name: "双实例 E2E：接收目录写不进去（磁盘满/只读）必须明确失败并止步",
      why: `把接收端目录改成只读 ⇒ 对端在 offer 期就建不出临时文件、只能回 FileReject；` +
        `钉"写不进去不等于传输成功"：发送侧必须在重试上限内落到明确终态（GiveUp→failed）、` +
        `次数不许超过 MAX_FILE_OUTBOX_RETRIES、接收目录不许留下这个 transfer 的任何东西；` +
        `与冻结轮互为对照（那一轮对端不回话⇒根本没尝试，这一轮对端活着⇒真跑到 GiveUp）；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=recv-readonly"],
    },
    {
      group: "local",
      name: "双实例 E2E：入队后源文件被改小 ⇒ 按磁盘真值收发、两侧终态一致",
      why: `先冻住接收端、入队（气泡 + 台账 + 队列三行一起写，复刻点击那一刻的产物）、把原件截断、再解冻 ⇒ ` +
        `发送端 offer 的 size 必然来自截断后的盘（A 只在收到入站帧时才读盘，A-9）。钉的是：` +
        `落地字节数与新 size 一致、内容 hash 与截断后的源一致（不是半截也不是多给旧字节）、` +
        `两侧同时 done 且队列行已关、接收目录只留终名那一份。` +
        `★ 这一轮同时照出 A-11（实测：发送侧气泡与台账的 size 仍是入队时那份，与真正发出去的字节不等）——` +
        `修之前不许把那个分歧写成断言；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=src-shrunk"],
    },
    {
      group: "local",
      name: "双实例 E2E：三个文件一起排队（含两个同名）⇒ 一张都不许丢、内容不许串味",
      why: `一次入队 3 个 transfer，其中两张**文件名相同、内容不同**（真机形状：一次选两张同名截图）。` +
        `钉的是：三单各自落 done 且队列清空（串行投递不许把后面的挤死）、同名两单必须落在**两个不同路径**` +
        `（少一次就是后一次 rename 覆盖前一次 = 静默丢数据）、落地内容多重集合 == 源内容多重集合` +
        `（交错/串味/被顶掉都会露出来）、每单两侧台账各只一行、无 .part 残留。` +
        `⚠️ 边界要写清：这一格证明的是「同 peer 串行 flush ⇒ 后一单的 offer 一定看得见前一单已 rename 的文件」，` +
        `**没有**覆盖"两个 offer 都在任一次 rename 之前到达"那个真会撞 final_path 的交错（那需要三个实例：两个发送者 → 一个接收者）；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=multi-file"],
    },
    {
      group: "local",
      name: "双实例 E2E：预置可写 .part 后目录改只读 ⇒ 只有收尾 rename 塌（半路失败不冒充完成）",
      why: `先给接收端放**真前缀** .part（⇒ offer 不被拒、字节照流进来），再把接收目录 chmod 成只读：` +
        `往已存在的 inode 里写不需要目录写权限，唯一会 EACCES 的就是收尾那次 rename —— 与⑤（offer 期就写不进、` +
        `接收侧一行都不写）互为两半。钉的是：.part 必须真被续写过（证明这一轮打的不是⑤那条路）、` +
        `A 侧终态明确（行被收尾删除 / failed / cancelled / done 都算，停在 pending/sending = 界面永远转圈）、` +
        `attempts ≤ MAX_FILE_OUTBOX_RETRIES、接收侧台账不许假 done。` +
        `★ 这一轮实测照出 **A-12**（A {status:gone,aT:done} / B=failed / .part 整份 / final 不存在 —— ` +
        `AlreadyHave 按字节数短路，绕过了「rename 才算完成」）：交叉自洽那条判据今天不成立，` +
        `按 A-11 的先例**只打印不设断言**，修完再补；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=recv-dir-rotted"],
    },
    {
      group: "local",
      name: "双实例 E2E：100 MB 在飞时冻住接收端 ⇒ 盘上进度一字不涨，解冻后补到源内容",
      why: `§七「发送方提前放弃 / 接收方仍在线」与 §19「文件·断线」的交叉格，此前零跨实例判据。` +
        `与冻结轮④的区别是**时序**：④ 先冻再入队，实测那一轮 A 一次都没尝试（A-9：待传队列只被入站帧带动）` +
        `⇒ ④ 里根本没有在飞字节；这一轮先用 50 ms 自旋抓到 .part 真的在长，再 SIGSTOP。` +
        `钉的是不变量「接收端真实进度」：对端停止写盘之后盘上进度必须**一字不涨**（A 灌进 socket 与内核缓冲的` +
        `字节都不许算进度），两侧都不许在失联期间记 done；解冻之后必须补到源内容、两侧 done、队列清零、` +
        `半截 .part 不残留。★ 实测（2026-09-26）：在飞窗口抓得住（动手时刻的 .part 在 1.8MB / 7.1MB 之间摆，` +
        `随机器负载变 ⇒ 只打印不设判据），冻结 70s 期间一字不涨；` +
        `而先兑现的出口是 chunk loop 那句 [FILE] COMPLETED … ok=false（attempts 涨到 1、队列退回 pending），` +
        `60s 的 [STALL] **一行都没出现** ⇒ 所以这一轮不断言"走的哪一条放弃出口"（钉死任一条都会假红），只打印；` +
        `解冻→终名落地 5.5s / 28.6s / 51.6s 都实测到过（原链路续完 vs 先退避再重拨）。` +
        `★ 另一条被本地层跑红才找到的判据教训：**A 的终态天然晚于 B 的 rename**（A 的 done + 删队列行由对端 ` +
        `FileCompleteAck 点亮，而 ack 在 rename 之后才发；实测这段滞后不是常数——0.0s 与 14.6s 都出现过 ⇒ ` +
        `"立刻读"和"睡固定秒数再读"都会红，只能有界等）⇒ 终局那条走 waitSendTerminal()，` +
        `有界等 60s（心跳 5s + 队列退避 5s），到不了才判红；同一形状在默认轮/杀进程轮/冻结轮一并收口，` +
        `其中杀进程轮原本用 sleep(15s) 把这个竞态藏住而不是解决。` +
        `另⚠️：这**不是**「用户点取消」（那条要 IPC 调 cancel_file_transfer，harness 零改产码调不到）。` +
        `每轮条数由 check-doc-numbers 现算，只登记在验收矩阵顶部「轮次账」一处；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=stall-mid"],
    },
    {
      group: "local",
      name: "双实例 E2E：100 MB 在飞时 SIGKILL 发送端 ⇒ 崩溃不许抹掉「已入队」，重启后自己续完",
      why: `§七「发送过程中杀进程」×「重启后继续」+ §八「本端重启」+ §九「数据生命周期：` +
        `运行→退出→重新启动」的交叉格，此前**零跨实例判据** —— 注入③ 杀的是接收端，` +
        `全仓没有任何一轮从"发送端自己崩了"这一侧看过。钉的核心不是"能不能续传"` +
        `（②/③ 已证），而是**一次崩溃不许把「已入队」这个事实抹掉**：队列行是` +
        `「先入队再投递」的载体，它若因崩溃变成 done 或消失，这一单就永久没人再发，` +
        `而用户界面上的气泡还在 —— 最难发现的一种丢法。时序：判据自己入队 → 50 ms 自旋` +
        `抓到 .part 真的在长 → SIGKILL A → 死透后再等 20 s（这段是"发送端不存在"的时间）→` +
        `重启 A → 等它自己补完。★ 实测（2026-09-26，26 断言全绿）：在飞 .part=5,242,880 B 时杀，` +
        `那 20 s 里 B 无终名、B 台账 failed、A 台账 active、**A 队列行仍是 1**；` +
        `A 重启后 12.6 s 从盘上那点字节续到整份 104,857,600 B、sha 等于源、两侧 done、` +
        `队列清零、不留 .part。终局那条照⑨ 的教训走 waitSendTerminal()（A 的 done 由对端 ack` +
        `点亮，B 落地不是 A 的同步点）。反证轮（入口在 package.json 的 sendside-selfproof）只换` +
        `"补完该等于哪个摘要"这一份输入 —— 反向模式**不许**写进这一层：它预期红，进门禁就是把整层静音。` +
        `⚠️ 每轮条数由 check-doc-numbers 现算，只登记在验收矩阵顶部「轮次账」` +
        `一处；${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--fault=sender-kill-mid"],
    },
    {
      group: "local",
      name: "双实例 E2E：群聊这一族跨实例真跑（发送 / 离线成员补发 / 撤回 G-Set / Ack 清队列）",
      why: `§九「群聊」此前在跨实例层面是**零判据**（harness 里 grep group 只命中 file_outbox 的列名，` +
        `从没建过群），而群消息走的是一条与 1:1 不同的管道：group_outbox 按成员一行 + Gossip 信封 + ` +
        `GroupAck 删行 + G-Set 撤回。这一轮停机预置两端群记录（形状照 e2e_peer.rs 的 ensure_test_group 先例）、` +
        `A 排四条群消息（正文 / 撤回 / 第二条正文 / **一句 @**），**入队时对端进程还没起** ⇒ 只能靠建链后的 ` +
        `flush_group_outbox 送达，顺带就是 §五 点名的「群聊 + 离线成员重新上线」与「聊天 + 群聊 + 文件」两格组合。` +
        `★ 那句 @ 钉的是 §8 那条存储不变量：**库里那串字节一个字都不许动** —— 逐字等于打出去的原文，` +
        `且既没被换成呈现层的「@你」（那等于把我的视角烧进公共数据）、也没被"规范化"成 device id（那会把可读那份弄没）。` +
        `群聊轮全绿（每轮条数由 check-doc-numbers 现算，只登记在验收矩阵顶部「轮次账」一处）；`+"`--round=group-lie`"+"` 只翻判据读的那个 id ⇒ 9 条红、" +
        `其余与 id 无关的保持绿（证明这些条读的是真落库行，不是同义反复）。` +
        `⚠️ 边界两条：① 群 payload **不做 re-seal**（transport.rs:6689-6693），所以信封由 harness 自己签名加密 —— ` +
        `这一轮的绿只证明"接收端能解出来并落库"，不证明"发送内核自己会怎么组信封"；` +
        `② 发送侧本地的撤回物化**不设判据**（harness 写的是入队形状、没执行产品的撤回命令，` +
        `按"我以为应该"去断言就是替产品许愿）；群文件端到端也不在这一格，` +
        `群任务另起 `+"`--round=task`"+`，而「@你」在真界面上长什么样属运行时层（Smoke-10/11 那一类）。${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--round=group"],
    },
    {
      group: "local",
      name: "双实例 E2E：局域网开关的隔离判据（关掉 ⇒ 对端学不到我，翻回来 ⇒ 重新学得到）",
      why: `#89 剩下的那一半：**全程带着 GOSSLAN_AUTOSTART=1 跑**，证的就是"预置说关"连强制联网的环境变量都不许越过。` +
        `三条腿——开着先学到 → 关掉之后 A 的 announce 计数一字不涨 → 把键翻回开又涨回来；` +
        `中间那条的期望值在它的 -lie 反向轮里翻面（只翻判据、不动注入）。` +
        `⚠️ 观察者 A 全程不重启：不然"不涨"就退化成"没人再看"的同义反复。` + LOCAL_ONLY_WHY,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--round=lanoff"],
    },
    {
      group: "local",
      name: "双实例 E2E：正在建群时被 SIGKILL ⇒ 对端仍必须自己学到这个群（重递不许依赖内存登记）",
      why: `§28「链路失效」那一族里今天做得成的那一格（另一格"断所有链路"要 root 或真设备）。` +
        `⚠️ 它**刻意不是**"重启后不许留下半个群"那种形状：真实建群路径把 groups / group_members /` +
        `settings(gk:) / conversations 四张写放在同一个事务里（` + "`commands/groups.rs:7`" + `）⇒ ` +
        `那句断言永远绿，是本仓反复判过的"半个守卫"。有作用点的一半在**投递**：` +
        `提交之后才逐成员推 GroupKey，而"没推出去"的重试登记 ` + "`pending_group_keys`" + ` 是**进程内**的一张表 ` +
        `⇒ 一次 SIGKILL 必然抹掉它。所以这轮钉的是那句设计注释的另一半：**群名册才是事实源，` +
        `链路活着就重递**（` + "`requeue_group_keys_for_peer`" + `，建链 / Hello / 心跳三处）。` +
        `时序：群只写在 A 的盘上（= 命令层已 commit、GroupKey 一次都没送到）→ 只起 A、对端缺席 15s →` +
        `真 SIGKILL A → 死透后再等 10s（这段是"A 根本不存在"的时间）→ 再起 A+B → 有界等 B 自己长出这个群。` +
        `八条断言：预置侧 B 零行（正向轮那条"学到了"的对照物）+ 缺席期 A 名册 2 位 + A 那份密钥逐字节在盘上` +
        `（重递的原料）+ 崩溃后名册仍是 2 位 + B 学到群行（creator/name 对）+ 成员恰好 2 位（重复 requeue 要幂等）+` +
        `B 手里那把对称密钥与 A 那份逐字节相同 + B 不许凭空多出一行会话（关系同步 ≠ 聊天同步）。` +
        `反证 ` + "`--round=groupcrash-lie`" + ` 照 group-lie 的先例：**等待用真 id、判据读翻过的 id** ⇒ ` +
        `红的只能来自"读的不是真落库行"。` +
        `⚠️ 边界三条：① 这轮造的是**磁盘能表达的那份等价坏状态**（那条内存队列从进程外写不进去），` +
        `所以它不判"内存登记本身对不对"；② 群 payload 不做 re-seal，信封与密钥都由 harness 预置 ⇒ ` +
        `绿只证明"接收端能学到并落库"，不证明发送内核自己怎么组 GroupKey 帧；` +
        `③ 收敛靠的是"下一次建链/Hello/心跳带动重递"，所以这轮判的是**会收敛**，不判"多快"。` +
        `每轮条数由 check-doc-numbers 现算，只登记在验收矩阵顶部「轮次账」一处。${LOCAL_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--round=groupcrash"],
    },
  );
} else if (!groupFlag) {
  // 不跑也要说出来 —— 一片绿暗示"全跑过了"正是这套门禁最反对的样子。
  // 不写轮数：这一层的条目由上面的步骤表自己点名，写死数字就是下一个漂移点（同 CHANGELOG 的口径）。
  console.log(`· 本地专项层（多实例 E2E，条目见 npm run verify:e2e）本轮没跑：${LOCAL_ONLY_WHY}`);
  console.log("  要跑它：npm run verify:e2e（或 --group local）");
}

/**
 * 「发版前专项层」：三实例链式轮（用户 2026-09-26 拍板＝**建，但只挂在发版前**，不进日常本地门禁）。
 *
 * 为什么单开一层而不是塞进 `local`：这一轮除了基础旅程那七步，还要**同时开三个真进程**、
 * 并且为第三个实例多付一次"先单独起来自建身份、再停机写库、再三端同场"的重启。
 * 塞进日常层会让每次 `verify:e2e` 都多等一节，而"太慢于是被人跳过"的门禁等于没有门禁。
 * 所以它只在发版检查单里被点名，CI 也不认领（理由同 `local`：要 release 产物 + 桌面 GUI 会话）。
 */
const RELEASE_ONLY_WHY =
  "发版前专项：第三个真进程 + 为它多重启一轮 ⇒ 日常层不收，发版检查单点名跑（npm run verify:release）";
if (groupFlag === "release") {
  steps.push(
    {
      group: "release",
      name: "三实例链式 E2E：A 从没直发给 C 的那条群消息，C 仍收敛到了",
      why:
        `§五「群聊 + gossip」里唯一两实例测不到的那一半：**成员不在发送者的逐成员直发队列里，` +
        `只能靠中间人把 gossip 扇给它**。判据钉两侧 —— A 的直发队列里没有任何面向 C 的行（这才是` +
        `"不是直发"的正身，不依赖拓扑），以及 C 侧的落库：一行、明文、发送者仍是 A、` +
        `conv_id/seq 与信封一致、且 B 自己不重复落库。` +
        `⚠️ 两条已实测的边界（别把这一轮读成它们已被证明）：同机造不出"A-C 无链路"` +
        `（关掉局域网发现仍在广播与验收），以及"C 晚到就收不到"（中间人只对当时可达的邻居扇出，` +
        `不补推）—— 前者仍在待拍板，后者已由 #77 落地、并另起下面那一格「round=gossip-late」判。` +
        `非空转由 ` +
        "`--round=gossip3-lie`" +
        ` 反向轮证：只翻判据读的那个 id ⇒ 读 C 库的那四条必须报红，而拓扑那两条照常绿。${RELEASE_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--round=gossip3"],
    },
    {
      group: "release",
      name: "三实例补递 E2E：A 在 C 上线之前就发完，C 与中间人建链后仍拿到了那条",
      why:
        `#77 那一格的判据。与链式轮同一套拓扑（A—B—C，A 与 C 互不为好友、互不给端点），` +
        `只是**启动时序反过来**：链式轮先建好 B↔C 再让 A 发（证明"当时能扇到"），这一轮让 A 在` +
        `C 还不存在时就发完（证明"以后还能补到"）。判据钉三侧 —— B 在 C 上线之前就已落库` +
        `（手里有过它才谈得上补）、B 的日志里出现指向 C 的补递行（归因：这条是补的不是扇的）、` +
        `C 侧落库一行且明文/发送者/conv_id/seq 全对；另有两条前置挡住"其实直连也能拿到"的假绿：` +
        `C 侧没有任何与 A 的建链行、A 的逐成员直发队列里没有面向 C 的行。` +
        `⚠️ 边界（不许读成群消息最终一定一致）：补递缓存是**进程内**的、每组只留最近 16 条、` +
        `只留 10 分钟窗口 —— 中间人重启过或 C 掉线太久，补到的就是部分历史；这一轮只证` +
        `"窗口内、同一次运行里"这一格。非空转由 ` +
        "`--round=gossip-late-lie`" +
        ` 反向轮证：只翻判据读的那个 id ⇒ 读 C 库的那三条必须报红，拓扑与归因那几条照常绿。` +
        `${RELEASE_ONLY_WHY}`,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--round=gossip-late"],
    },
    {
      group: "release",
      name: "任务专项 E2E：任务在两个真实进程之间**双向**收敛，且扛得住重启与发送端崩溃",
      why:
        `第二阶段 §22 那一格。载荷形状逐字对齐 protocol.rs::TodoPayload（snake_case 无 rename），` +
        `群消息按 seq 定序：A 发起 todo(1) → todo_update(2,done) → todo_update(3,done+archived) → ` +
        `todo_update(4,doing)，B 发起 todo(5) → todo_update(6,done)。判据钉在**两侧的数据事实**上：` +
        `收的那一侧每条各恰好一行（重复投递/丢件都当场红）、必须是解密后的明文 JSON（拿到密文或空串即红）、` +
        `同一条 todo_id、创建那条的 assignees 必须含**对端**的 device_id（「与我相关」的输入就是它）、` +
        `status 由 todo 变 done 且带 done_at、归档那条只允许 done+archived=true、重开回 doing 且清掉 done_at、` +
        `seq 不许乱（LWW 靠它）、creator 与 sender_id 都不许被改写成接收者自己、` +
        `不许串进 1:1 会话、**两侧**的 group_outbox 都要被 GroupAck 清成 0；另有一步专判重启：` +
        `六条状态行的 kind:seq 一字不变、A 侧对端发起的两条不许变成第二行（已 Ack 的队列不许被点亮成二次投递）。` +
        `§28「正在任务同步时退出」那一格也落在这一轮里：对端整个缺席时把两条任务入队 ⇒ 只起发送端 ⇒ 缺席期队列行必须是 2 行、` +
        `对端侧 0 行 ⇒ **SIGKILL 发送端** ⇒ 崩溃后队列行与那两条气泡都必须还在（「已入队」这个事实不许被一次崩溃抹掉）⇒ ` +
        `再起两端，建链带动 flush ⇒ 对端必须各恰好一行且明文解得开、seq 7→8、todo→done，发送侧队列最终被 GroupAck 清成 0。` +
        `文件族（注入⑩）早就证过同一件事，群任务这一族以前没有判据。` +
        `§27「任务 + 图片」那一格也在这轮：对端发起的那条任务带两张图片引用（一张 image 一张 file），` +
        `判的是**五个字段逐字段全等且顺序不变**（元数据过线、真实字节走群文件管线 —— SQLite 不存 BLOB 这条原则继续保持）；` +
        `每个字段掉了都是静默的：subtype 错就渲染成文件块、sha256/id 错就本地取不到字节、size 错就进度对不上。` +
        `★ 这一腿还顺带照出一个只有**反向跑**才能发现的空转：写成「(p?.images ?? []).every(...)」时，` +
        `"一条都没收到"恒为真 ⇒ 加判据先问"一条都没收到时它是真还是假"。` +
        ` 在"一条都没收到"时恒为真 —— 加判据时先问"一条都没收到时它是真还是假"。` +
        `★ 为什么"B→A"那一腿是真判据而不是自我循环：A 侧那两行 harness **没有**预置，` +
        `唯一可能的来源是 B 建链后 flush_group_outbox 真投递 + A 自己解密落库。` +
        `⚠️ 两条边界（别把这一轮读成已证明的东西）：` +
        `①**徽标那个数字不在判据里** —— 它是前端 store 现算的，SQLite 里没有这一格，` +
        `本轮判的是它的输入（状态行 + 指派关系）；读数本身归 utils/todos 的纯函数用例与运行时层。` +
        `②**"发起方自己发完立刻在自己时间线看到"也不在判据里** —— harness 没有任何"让应用自己执行一次动作"` +
        `的入口（全仓只认 GOSSLAN_INSTANCE / GOSSLAN_AUTOSTART），两侧那一行都是预置写进去的，` +
        `判它等于循环论证；那一半要靠内核自 emit 之后留的那行痕迹（017a433）在能驱动 UI 的轮次里补（Smoke-10）。` +
        `非空转两条路：` +
        "`--round=task-lie`" +
        ` 只翻判据读的 msg_id ⇒ 读对端库的那几条必须报红，而发送侧预置那条照常绿；` +
        `重启那三条、"对端队列清成 0"那条、以及崩溃那一整腿（缺席 / 队列行 / 气泡 / 最终清成 0）lie 都判不到` +
        `（它们读的是真 id，拿翻过的 id 去读只会得到"0 行"，分不清"没送达"与"没留住"），` +
        `改由**只换期望值**的变异跑分两次证：一次把重启那几条换成假期望、一次把崩溃腿五条换成假期望 ⇒ ` +
        `各自"恰好被改的那几条红、其余照常绿"，跑完还原并核对 diff 只剩纯新增。` +
        RELEASE_ONLY_WHY,
      cwd: ROOT,
      cmd: NODE_EXE,
      args: ["scripts/e2e-multi-instance.mjs", "--round=task"],
    },
  );
} else if (!groupFlag) {
  console.log(`· 发版前专项层（三实例链式 E2E）本轮没跑：${RELEASE_ONLY_WHY}`);
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
    if (KEEP_GOING) {
      // 独立轮次：继续跑后面的，最后一起汇总。红**不会**被这样洗白 —— 见结尾的退出码。
      console.error("   这一层各步是独立的轮次 ⇒ 继续跑后面的（要旧行为加 `--fail-fast`）。");
      continue;
    }
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
