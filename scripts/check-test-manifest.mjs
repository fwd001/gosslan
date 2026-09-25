#!/usr/bin/env node
/**
 * 测试清单守卫（test manifest guard）—— 挡住「测试静默不跑」。
 *
 * ## 为什么要有这个脚本
 *
 * 本项目的前端断言、Rust 用例、非空转护栏各有几百条（条数由各自的输出打印 —— 抄在这里
 * 一定腐烂）。但**没有任何东西
 * 保证它们真的被执行**。已经真实发生过一次（见 `scripts/verify-guards.py` 里
 * 「`cargo test --features bluetooth <名>` 一个测试都不会跑、退出码 0」那条记录）：
 *
 *   · `bluetooth` 是**非默认 feature**（`src-tauri/Cargo.toml` 的 `[features]`）。
 *     漏掉 `--features bluetooth` ⇒ BLE 相关模块根本不编译 ⇒ 那 20 条用例
 *     连同被测代码一起消失，而 `cargo test` **全绿**。
 *   · 前端 `npm test` 的脚本里是**手工枚举**的 48 条路径。新增测试文件若忘了
 *     加进那串字符串，新文件不会跑，而 `npm test` 依然**全绿**。
 *
 * 两种都是「退出码 0 的空转」——最危险的那类故障：没有任何信号。
 *
 * ## 为什么是「比对名字」而不是「比对数量」
 *
 * 数量阈值（比如 `>= 503`）会产生反向激励：为了凑数而保留已经没有价值的测试，
 * 而删除一个过时测试反而要改阈值。名字比对没有这个问题：
 *
 *   · 缺名（基线里有、实际没跑） → **FAIL** —— 这正是「静默跳过」，必须拦。
 *   · 多名（基线里没有、实际跑了）→ **WARN** —— 新测试跑得好好的，不是故障；
 *     打印出来提示跑 `--update` 把基线补齐即可（补齐后它才进入保护范围）。
 *
 * 判据方向刻意不对称：只对「少了」红脸，不对「多了」红脸。
 *
 * ## 基线必须按平台分开（否则会误报，把真失败淹掉）
 *
 * 有一部分用例是**平台门控**的 —— 本仓库实测：
 *
 *   · `transport/bluetooth_peripheral.rs`（macOS 外设）     5 条用例
 *   · `transport/bluetooth_peripheral_windows.rs`（Windows）2 条用例
 *
 * 这两个文件是 `#[cfg(all(feature = "bluetooth", target_os = "…"))]`。于是
 * **macOS 上的基线拿到 Windows 用，那 5 条会被判成「静默跳过」** —— 纯误报。
 * `scripts/verify-guards.py` 的 `platforms` 字段就是为同一个坑加的，它的注释写着：
 *
 *   > 正确做法是**显式跳过并说清楚**，而不是留一堆假失败把真失败淹掉。
 *
 * 所以基线按平台分文件：`test-baseline.<macos|windows|linux>.txt`。
 * 拿到本平台没有基线时**不猜、不退化**，直接报错让你用 `--update` 生成。
 *
 * ## 「多了只 WARN」留下的那只眼，由跨平台完整性核对补上
 *
 * 判据方向不对称是有代价的：一条用例只要**没进本平台基线**，它在本平台消失就不会红。
 * 于是 mac 侧每次 `--update` 自己跟上、win 侧越拉越远，Windows 基线烂到只剩并集的一部分，
 * 而那部分之外的用例从此"在 Windows 上被门控掉也没人知道"（mac 那条腿照跑照绿）。
 * 所以还有一条 **不依赖 cargo** 的判据：任一平台基线都不许比"各平台基线的并集"少一条
 * 说不出理由的用例 —— 理由只能是**源码里现算出来的 `target_os` 门控**
 * （模块声明与测试函数头上两类），解析不出平台约束的 cfg 一律要求"每个平台都得有"。
 * 本平台基线仍只认 `--update`（观测优先）；跨平台用 `--sync-baselines` 按同一套门控推。
 *
 * ## 用法
 *
 *     node scripts/check-test-manifest.mjs                  # 全部检查
 *     node scripts/check-test-manifest.mjs --only frontend  # 只查前端（秒级，无需编译）
 *     node scripts/check-test-manifest.mjs --only rust      # 只查 Rust（需编译）
 *     node scripts/check-test-manifest.mjs --update          # 用当前实际名单重写**本平台**基线
 *     node scripts/check-test-manifest.mjs --sync-baselines  # 按源码门控推出**其它平台**的基线
 *
 * 退出码：0 = 通过；1 = 有测试静默消失了。
 */

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const TAURI = path.join(ROOT, "src-tauri");

/** Node 的平台名 → Rust 的 `target_os`（基线文件名用它）。 */
const RUST_OS = { darwin: "macos", win32: "windows", linux: "linux" }[process.platform];
if (!RUST_OS) {
  console.error(`✗ 未知平台 ${process.platform}：本脚本只认 darwin / win32 / linux。`);
  console.error("  新平台请先确认基线该怎么分（见文件头「基线必须按平台分开」）。");
  process.exit(1);
}
const BASELINE = path.join(TAURI, `test-baseline.${RUST_OS}.txt`);

/** Rust 侧固定用这一组参数 —— 与 CI / verify 入口必须一致。 */
const RUST_ARGS = ["test", "--features", "bluetooth", "--lib", "--", "--list"];

const argv = process.argv.slice(2);
const only = (() => {
  const i = argv.indexOf("--only");
  return i >= 0 ? argv[i + 1] : null;
})();
const update = argv.includes("--update");
const syncBaselines = argv.includes("--sync-baselines");

if (only && only !== "frontend" && only !== "rust") {
  console.error(`✗ --only 只接受 frontend / rust，收到「${only}」`);
  process.exit(1);
}

const runFrontend = !only || only === "frontend";
const runRust = !only || only === "rust";

/** 路径统一成 posix 形式：本仓库的清单与基线一律用正斜杠（CI 在 macOS/Linux 上跑）。 */
const posix = (p) => p.split(path.sep).join("/");

/** 递归收集 `src` 下所有 `.test.ts`。 */
function collectFrontendTests(dir, acc = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) collectFrontendTests(p, acc);
    // ⚠️ 必须转成正斜杠再比：`package.json` 里登记的是 `src/utils/x.test.ts` 这种 posix 路径，
    // 而 Windows 上 `path.relative()` 给出的是 `src\utils\x.test.ts` ⇒ 直接比对会把**每一个**
    // 文件都判成"未登记"（现象：本机 `npm run verify` 第一步就红，而 CI 的 macOS 腿是绿的，
    // 因为那里的分隔符恰好一致）。2026-09-17 修。
    else if (e.name.endsWith(".test.ts")) acc.push(posix(path.relative(ROOT, p)));
  }
  return acc;
}

/**
 * 前端：`npm test` 脚本里手工枚举的路径 ⋈ 磁盘上真实存在的测试文件。
 *
 * 只报「磁盘有、脚本没列」（会被静默跳过）。反向（脚本列了但文件不存在）
 * `node --test` 自己会报错，不必在这里重复拦。
 */
function checkFrontend() {
  const pkg = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8"));
  const listed = new Set(
    pkg.scripts.test.split(/\s+/).filter((t) => t.endsWith(".test.ts")),
  );
  const onDisk = collectFrontendTests(path.join(ROOT, "src"));
  const skipped = onDisk.filter((f) => !listed.has(f));

  if (skipped.length === 0) {
    console.log(`✓ 前端测试清单：磁盘 ${onDisk.length} 个文件全部已登记（不会静默跳过）`);
    return true;
  }
  console.error(`✗ 前端有 ${skipped.length} 个测试文件存在但未登记进 package.json 的 test 脚本；`);
  console.error("  它们不会被执行，而 `npm test` 依然全绿：");
  for (const f of skipped) console.error(`    · ${f}`);
  console.error("  修法：把上面每个路径追加进 package.json 的 scripts.test。");
  return false;
}

/** 解析 `cargo test -- --list` 的输出：`<用例名>: test`。 */
function rustTestNames() {
  const out = execFileSync("cargo", RUST_ARGS, {
    cwd: TAURI,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
  });
  return out
    .split("\n")
    .map((l) => l.match(/^(.+): test$/)?.[1])
    .filter(Boolean)
    .sort();
}

/**
 * Rust：基线名单 ⋈ 实际 `--list`。
 *
 * 缺名必须红 —— 那意味着某个 feature 没开、某个 `mod` 没挂进 `mod.rs`、
 * 或某个 `#[cfg]` 没满足，被测代码连同测试一起消失了。
 */
/** 递归列出目录下所有 .rs 文件（跳过 target）。 */
function rustFiles(dir, acc = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.name === "target" || e.name === ".git") continue;
    const full = path.join(dir, e.name);
    if (e.isDirectory()) rustFiles(full, acc);
    else if (e.name.endsWith(".rs")) acc.push(full);
  }
  return acc;
}

let _fnNamesCache = null;
/** 全仓 Rust 源码里出现过的 `fn <名字>`（含 `pub fn` / `async fn` / `unsafe fn`）。 */
function declaredFnNames() {  if (_fnNamesCache) return _fnNamesCache;
  const set = new Set();
  for (const f of rustFiles(path.join(TAURI, "src"))) {
    const src = readFileSync(f, "utf8");
    // 标识符类放宽到「非空白、非括号、非逗号」：Rust 允许中文函数名
    // （本仓就有 `gossip_group不受好友检查影响`），用 `\w` 会把它误报成陈旧条目。
    for (const m of src.matchAll(/\bfn\s+([^\s(<,;]+)/g)) set.add(m[1]);
  }
  _fnNamesCache = set;
  return set;
}

/**
 * 平台门控扫描 —— 回答「这条用例在某个平台上**会不会**编译」。
 *
 * 只认两处的 cfg：① 模块声明（`mod X` / `include!("x.rs")`）② 测试函数头上。
 * 判据方向刻意保守：**解析不出平台约束的 cfg 一律当作"每个平台都要有"**，
 * 于是猜错的后果是"多要求一条"（响亮地红）而不是"少要求一条"（静默的洞）。
 */
const ALL_OS = ["macos", "windows", "linux", "android"];

/** cfg 表达式 → 允许的平台集合；返回 null 表示这条 cfg 与平台无关。 */
function platformsAllowed(expr) {
  const positive = new Set();
  const negative = new Set();
  for (const m of expr.matchAll(/target_os\s*=\s*"(\w+)"/g)) positive.add(m[1]);
  for (const m of expr.matchAll(/not\s*\(\s*target_os\s*=\s*"(\w+)"\s*\)/g)) {
    positive.delete(m[1]);
    negative.add(m[1]);
  }
  const bare = (tok) =>
    new RegExp(`(^|[^a-z_])${tok}([^a-z_]|$)`).test(expr.replace(/target_os\s*=\s*"\w+"/g, " "));
  if (bare("unix") && !/not\s*\(\s*unix/.test(expr)) ["macos", "linux", "android"].forEach((o) => positive.add(o));
  if (/not\s*\(\s*unix\s*\)/.test(expr)) ["macos", "linux", "android"].forEach((o) => negative.add(o));
  if (bare("windows") && !/not\s*\(\s*windows/.test(expr)) positive.add("windows");
  if (/not\s*\(\s*windows\s*\)/.test(expr)) negative.add("windows");
  if (positive.size === 0 && negative.size === 0) return null;
  let allowed = ALL_OS.filter((o) => !negative.has(o));
  if (positive.size > 0) allowed = allowed.filter((o) => positive.has(o));
  return new Set(allowed);
}

let _gateCache = null;
/**
 * 从源码现算的门控表：`{ kind: "module"|"fn", key, allowed, evidence }`。
 * 刻意不维护手写名单 —— 名单会漏、门控会改；判据必须自己去源码里找那几处。
 */
function platformGates() {
  if (_gateCache) return _gateCache;
  const gates = [];
  for (const f of rustFiles(path.join(TAURI, "src"))) {
    const lines = readFileSync(f, "utf8").split("\n");
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (!/^\s*#\[cfg\(/.test(line)) continue;
      const allowed = platformsAllowed(line);
      if (!allowed) continue;
      const rel = posix(path.relative(TAURI, f));
      // cfg 与声明之间还夹着别的属性（`#[test]` / `#[tokio::test]` / doc 注释）——
      // 不先把它们剥掉，`^\s*fn` 永远匹配不上，于是"函数级门控"这条腿是空的（我自己踩过）。
      const next = [lines[i + 1], lines[i + 2], lines[i + 3]]
        .filter(Boolean)
        .join("\n")
        .replace(/^\s*(?:#\[[^\n]*\]\s*|\/\/[/!]?[^\n]*\n)+/, "");
      const mod = next.match(/^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)/);
      const inc = next.match(/^\s*include!\(\s*"([^"]+)\.rs"/);
      const fn = next.match(/^\s*(?:async\s+|unsafe\s+|pub(?:\([^)]*\))?\s+)*fn\s+([^\s(<,;]+)/);
      if (mod) gates.push({ kind: "module", key: mod[1], allowed, evidence: `${rel}:${i + 1}` });
      else if (inc)
        gates.push({
          kind: "module",
          key: path.basename(inc[1]).replace(/\.rs$/, ""),
          allowed,
          evidence: `${rel}:${i + 1}`,
        });
      else if (fn) gates.push({ kind: "fn", key: fn[1], allowed, evidence: `${rel}:${i + 1}` });
    }
  }
  _gateCache = gates;
  return gates;
}

/** 这条用例名在 os 上是否被门控挡在外面（附带挡它的那条证据）。 */
function gateExcluding(name, os) {
  const segs = name.split("::");
  const fn = segs[segs.length - 1];
  for (const g of platformGates()) {
    if (g.allowed.has(os)) continue;
    if (g.kind === "fn" ? g.key === fn : segs.slice(0, -1).includes(g.key)) return g;
  }
  return null;
}

/**
 * 跨平台**完整性**核对：任何一份平台基线都不许比"其它平台基线的并集"少一条
 * 说不出理由的用例。
 *
 * 为什么必须有这条：`added`（本平台实际多跑）只能 WARN —— 新测试跑得好好的不该红。
 * 于是历史上 Windows 基线烂成了 mac 的 487/690：那 200 多条"实际会跑但没登记"的名字
 * 在 Windows 上永远只出 WARN，**有人把它们在 Windows 侧门控掉了也不会红**
 * （mac 那条腿照跑照绿）。缺名红脸只能管住"已登记的那部分"，
 * 没登记的这部分需要一个与平台无关的判据 —— 就是这条。
 *
 * 它不需要 cargo，所以在任何平台上都能查出另一条腿上的洞。
 */
function checkBaselinesAreComplete() {
  const files = readdirSync(TAURI).filter((x) => /^test-baseline\..+\.txt$/.test(x));
  if (files.length < 2) {
    console.log(`✓ 跨平台基线完整性：只有 ${files.length} 份基线，无可对比（新平台接进来即生效）`);
    return true;
  }
  const read = (f) =>
    readFileSync(path.join(TAURI, f), "utf8").split("\n").map((s) => s.trim()).filter(Boolean);
  const byOs = new Map(files.map((f) => [f.match(/^test-baseline\.(.+)\.txt$/)[1], read(f)]));
  const union = [...new Set([...byOs.values()].flat())];
  let ok = true;
  for (const [os, names] of byOs) {
    const have = new Set(names);
    const unjustified = union.filter((n) => !have.has(n) && !gateExcluding(n, os));
    if (unjustified.length === 0) continue;
    ok = false;
    console.error(`✗ 基线 ${path.join("src-tauri", `test-baseline.${os}.txt`)} 比并集少 ${unjustified.length} 条，且说不出平台理由：`);
    for (const n of unjustified.slice(0, 40)) console.error(`    · ${n}`);
    if (unjustified.length > 40) console.error(`    …（另 ${unjustified.length - 40} 条）`);
    console.error(`  这些用例在别的平台基线里存在、却没有一条 target_os 门控能解释它们为何不在 ${os} 上。`);
    console.error(`  成因：只 --update 了本平台基线，另一条腿留下空洞 ⇒ 它在本平台"多跑只 WARN"，`);
    console.error(`        将来真被门控掉时不会有任何人看见。`);
    console.error(`  修法：本平台跑 --update；跨平台跑 \`node scripts/check-test-manifest.mjs --sync-baselines\`。`);
  }
  if (ok) {
    const exempt = [...byOs.entries()].map(([os, names]) => {
      const have = new Set(names);
      return `${os}:${names.length}+${union.filter((n) => !have.has(n)).length}`;
    });
    console.log(`✓ 跨平台基线完整性：差额全部有源码门控背书（${exempt.join("，")}）`);
  }
  return ok;
}

/**
 * 用本平台实际名单 + 源码门控，把**其它平台**的基线推出来。
 * 本平台自己仍然只认 `--update`（观测优先于推断）。
 */
function syncOtherBaselines(actual) {
  const files = readdirSync(TAURI).filter((x) => /^test-baseline\..+\.txt$/.test(x));
  // 池子 = 本平台实际名单 ∪ 所有已有基线。只用实际名单会把"只在别的平台上才有的用例"
  // 顺手删掉（它们在本平台从来没被列出来过），那条腿反而更空。
  const pool = [
    ...new Set([
      ...actual,
      ...files
        .flatMap((f) => readFileSync(path.join(TAURI, f), "utf8").split("\n"))
        .map((s) => s.trim())
        .filter(Boolean),
    ]),
  ];
  for (const f of files) {
    const os = f.match(/^test-baseline\.(.+)\.txt$/)[1];
    if (os === RUST_OS) continue;
    const keep = pool.filter((n) => !gateExcluding(n, os)).sort();
    const dropped = pool.filter((n) => gateExcluding(n, os));
    writeFileSync(path.join(TAURI, f), keep.join("\n") + "\n");
    console.log(
      `✓ 已按源码门控推出 ${path.join("src-tauri", f)}：${keep.length} 条（排除 ${dropped.length} 条，池子 ${pool.length} 条）`,
    );
    for (const n of dropped) {
      const g = gateExcluding(n, os);
      console.log(`    · 排除 ${n} —— ${g.kind} 门控 [${[...g.allowed].join("|")}] 见 ${g.evidence}`);
    }
  }
}

/**
 * 每份平台基线里的每个名字，其函数名必须仍在源码里存在。
 *
 * 为什么这能与"平台差异"分开：平台门控的用例**函数还在**（只是本平台不编译）⇒ 不报；
 * 被删掉或改名的用例**函数已经没了** ⇒ 一定是基线烂了，删它没有争议。
 * 这条在任何平台上都会把所有平台的基线全查一遍，所以"只在 Windows 上才能发现的问题"
 * 第一次变得在 mac 上就能拦住。
 */
function checkNamesStillExist() {
  const declared = declaredFnNames();
  const stale = [];
  for (const f of readdirSync(TAURI).filter((x) => /^test-baseline\..+\.txt$/.test(x))) {
    for (const line of readFileSync(path.join(TAURI, f), "utf8").split("\n")) {
      const name = line.trim();
      if (!name) continue;
      const fn = name.split("::").pop();
      if (!declared.has(fn)) stale.push(`${f}: ${name}`);
    }
  }
  if (stale.length > 0) {
    console.error(`✗ 基线里有 ${stale.length} 个名字在源码中已不存在（陈旧条目，不是平台差异）：`);
    for (const n of stale) console.error(`    · ${n}`);
    console.error("  成因：删掉或改名的测试没同步所有平台的基线文件。");
    console.error("  修法：从对应文件里删掉这些行（本平台可直接 --update；跨平台要手工删）。");
    return false;
  }
  // 同一个函数名在不同平台基线里必须带**同一个模块路径**。
  // 现场（2026-09-25，我自己造的）：往 Windows 基线手工补两条用例时按记忆写成
  // `db::tests::…`，真路径是 `db::cascade_tests::…` —— 函数名存在所以上面那条查不出来，
  // 而 CI 上它变成"基线里有、本平台没跑"，又是那种"红得像代码坏了"的形状。
  const byPlatform = new Map();
  for (const f of readdirSync(TAURI).filter((x) => /^test-baseline\..+\.txt$/.test(x))) {
    for (const line of readFileSync(path.join(TAURI, f), "utf8").split("\n")) {
      const name = line.trim();
      if (!name) continue;
      const fn = name.split("::").pop();
      const pathPart = name.slice(0, name.length - fn.length);
      const seen = byPlatform.get(fn) || new Map();
      byPlatform.set(fn, seen);
      for (const prev of seen.keys()) {
        if (prev !== pathPart) {
          console.error(`✗ 用例 ${fn} 在不同平台基线里的模块路径不一致：${prev} vs ${pathPart}（${f}）`);
          console.error("  手工补基线时请按 `--update` 打出的**整条名字**复制，模块前缀不能凭记忆写。");
          return false;
        }
      }
      seen.set(pathPart, true);
    }
  }
  const files = readdirSync(TAURI).filter((x) => /^test-baseline\..+\.txt$/.test(x));
  console.log(`✓ 基线名字与模块路径跨平台一致（核对 ${files.length} 份基线）`);
  return true;
}


function checkRust() {
  // 这条不依赖 cargo（纯比对已有基线 + 源码门控），所以放在取实际名单之前：
  // cargo 编不过的时候，跨平台那只眼仍然要能睁开。
  const complete = checkBaselinesAreComplete();
  let actual;
  try {
    actual = rustTestNames();
  } catch (e) {
    console.error("✗ 无法取得 Rust 测试名单（cargo 失败）：");
    console.error(`  ${e.stderr || e.message}`);
    return false;
  }

  if (syncBaselines) {
    syncOtherBaselines(actual.sort());
    return complete;
  }

  if (update) {
    writeFileSync(BASELINE, actual.join("\n") + "\n");
    console.log(
      `✓ 已更新 ${path.relative(ROOT, BASELINE)}（${actual.length} 条用例，平台 ${RUST_OS}）`,
    );
    return true;
  }

  if (!existsSync(BASELINE)) {
    // ⚠️ 这里**故意不自动创建**：若在缺失时静默生成基线，等于"没有基线也算通过"，
    // 正是本脚本要消灭的那类空转（新平台第一次跑会假绿）。
    //
    // 但新平台（如 Windows 首次接入 CI）没法在别的机器上生成自己的基线 ——
    // 于是这里把与**已有基线**的差集打出来：差集通常只有几条（平台互斥的 #[cfg]），
    // 小到能塞进一条 CI 注解，照着它就能手工构造出本平台的基线。
    console.error(`✗ 本平台（${RUST_OS}）没有基线文件：${path.relative(ROOT, BASELINE)}`);
    console.error("  基线必须按平台分开（macOS 外设 / Windows 外设是互斥的 #[cfg]）——");
    console.error("  拿别的平台的基线来比会把平台门控的用例误判成「静默跳过」。");
    console.error("");
    console.error(`  引导：本次实际名单共 ${actual.length} 条。与已有基线的差集如下 ——`);
    const others = readdirSync(TAURI)
      .filter((f) => /^test-baseline\..+\.txt$/.test(f))
      .map((f) => ({
        file: f,
        names: readFileSync(path.join(TAURI, f), "utf8")
          .split("\n")
          .map((s) => s.trim())
          .filter(Boolean),
      }));
    if (others.length === 0) {
      console.error("  （没有任何已有基线可参照，直接跑 --update 生成）");
    }
    const actualSet2 = new Set(actual);
    for (const o of others) {
      const oSet = new Set(o.names);
      const onlyHere = actual.filter((n) => !oSet.has(n));
      const onlyThere = o.names.filter((n) => !actualSet2.has(n));
      console.error("");
      console.error(`  vs ${o.file}（${o.names.length} 条）：`);
      console.error(`    + 只在本平台实际名单里（${onlyHere.length}）—— 本平台专属用例：`);
      for (const n of onlyHere) console.error(`        ${n}`);
      console.error(`    - 只在对方基线里（${onlyThere.length}）—— 对方平台专属用例：`);
      for (const n of onlyThere) console.error(`        ${n}`);
      console.error(`    ⇒ 本平台基线 = 对方基线 ${onlyThere.length ? `去掉上面 ${onlyThere.length} 条` : ""}` +
        `${onlyThere.length && onlyHere.length ? "、" : ""}${onlyHere.length ? `加上上面 ${onlyHere.length} 条` : ""}` +
        `${!onlyThere.length && !onlyHere.length ? "（两者完全相同）" : ""}`);
    }
    console.error("");
    console.error("  构造好之后跑 `node scripts/check-test-manifest.mjs --only rust` 复核。");
    return false;
  }

  const baseline = readFileSync(BASELINE, "utf8").split("\n").map((l) => l.trim()).filter(Boolean).sort();
  const actualSet = new Set(actual);
  const baselineSet = new Set(baseline);

  // 跨平台基线的"名字还在不在"核对 —— 这条与平台无关，所以在任何平台上都能查出
  // 另一条腿上的**陈旧条目**（Windows 基线没人手工同步，历史上就是这么烂掉的：
  // 删掉/改名一个测试，mac 侧 --update 自动跟上，win 侧留着一堆不存在的名字，
  // 于是那条腿只能靠"缺名=静默跳过"报一堆误判，把真正的漏跑淹掉）。
  const missing = baseline.filter((n) => !actualSet.has(n));
  const added = actual.filter((n) => !baselineSet.has(n));

  let ok = complete;
  // 跨平台基线的"名字还在不在"核对 —— 这条与平台无关，所以在任何平台上都能查出
  // 另一条腿上的**陈旧条目**：删掉/改名一个测试时，mac 侧 `--update` 自动跟上，
  // 而 win 侧留着一堆不存在的名字，于是那条腿把"漏跑"与"基线烂了"混成同一种红，
  // 只能靠人猜（历史上正是来回跑了两轮 CI）。
  ok = checkNamesStillExist() && ok;

  if (missing.length > 0) {
    ok = false;
    console.error(`✗ 基线里的 ${missing.length} 条用例**没有跑**（静默跳过）：`);
    for (const n of missing) console.error(`    · ${n}`);
    console.error("  常见原因：漏了 `--features bluetooth`、模块没挂进 mod.rs、");
    console.error("           #[cfg] 条件不满足（例如平台门控）。");
  }

  if (added.length > 0) {
    console.warn(`⚠ 有 ${added.length} 条新用例不在基线里（它们**会跑**，只是尚未纳入保护）：`);
    for (const n of added) console.warn(`    · ${n}`);
    console.warn("  跑 `node scripts/check-test-manifest.mjs --update` 把它们纳入。");
  }

  // 「缺名」与「多名」同时出现，最常见的成因是**换了平台**（平台门控的 #[cfg] 变了）：
  // 一部分用例在本平台不编译、另一部分只在平台编译。这时直接给出可照做的结论，
  // 免得再猜一轮 —— 2026-09-16 引导 Windows 基线时就是这么来回跑了两轮 CI 的。
  if (missing.length > 0 && added.length > 0) {
    console.error("");
    console.error(
      `  ⇒ 若这是**平台差异**（而不是漏跑）：本平台基线 = 当前基线 ` +
        `去掉上面 ${missing.length} 条、加上上面 ${added.length} 条 ⇒ ` +
        `${baseline.length - missing.length + added.length} 条。`,
    );
  }

  if (ok) {
    console.log(
      `✓ Rust 测试清单：基线 ${baseline.length} 条全部在跑` +
        (added.length ? `（另有 ${added.length} 条新用例待纳入）` : ""),
    );
  }
  return ok;
}

const results = [];
if (runFrontend) results.push(checkFrontend());
if (runRust) results.push(checkRust());

if (results.every(Boolean)) {
  console.log("\n测试清单守卫通过。");
  process.exit(0);
}
console.error("\n测试清单守卫失败：有测试静默消失了。");
process.exit(1);
