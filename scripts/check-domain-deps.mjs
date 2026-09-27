#!/usr/bin/env node
/**
 * 跨领域依赖守门（domain-deps guard）—— 不让"跨域"边界变成口头约定。
 *
 * ## 为什么需要它
 *
 * `docs/domains.data.mjs` 的每个领域除了 `paths`（地盘的边界）还有一个
 * `consumes: [domainId]` 列表 —— **声明本领域合法依赖的其他领域**。
 * 这一层只写不守等于零：
 *
 *   · AI 可能新增一条"方便"的跨域引用 ⇒ 边界慢慢被改写；
 *   · 维护者合并 PR 时肉眼看不出 `network/transport.rs` 加一行 use xxx
 *     已经触到第 9 个领域；
 *   · 编译照常通过（rust 对跨 crate 引用没有域的概念），CI 永远全绿。
 *
 * 本守门就是把那层"诚实声明"机器化。
 *
 * ## ⚠️ 跟 `check-domain-map.mjs` 的分工（不重复）
 *
 * | 守门 | 守的是什么 |
 * |---|---|
 * | `check-domain-map.mjs` 判据 A-F | **图的形式**：路径存在 / 不重叠 / 无孤儿 / `enforce` 只能开在单家 |
 * | `check-domain-deps.mjs`（本脚本） | **图的依赖方向**：`use crate::xxx` 落到别的领域，是否在 `consumes` 里 |
 *
 * 两层都过了 ⇒ 领域图是自洽的；只过一层 ⇒ 另一层要么补要么删。
 *
 * ## 判据
 *
 *  G. **跨领域引用受 `consumes` 约束**：扫描每个领域 `paths` 下所有 `.rs`
 *     文件的 `use crate::xxx`（排除 `#[cfg(test)] mod tests` 内），对
 *     每条引用解析归属：
 *       · 落到本域（self）→ 放行；
 *       · 落到装配层（`commands` / `state` / `network` / `lib` / `main`）
 *         → 放行（这些是 crosscutting 装配，不该被域管）；
 *       · 落到 `paths` 中**没**在任何领域里的模块 → 放行（unmapped）；
 *       · 落到另一个领域 → 那个领域的 `id` 必须在 self 的 `consumes` 中，
 *         否则 FAIL，并指出"加进 consumes 还是删掉这条 use"的修法。
 *
 * H. **`consumes` 不能引用不存在的领域 id**：写错的 id 第一天就该红，
 *     不该等到新增文件撞上空指针才暴露。
 *
 * I. **`consumes` 不能引自己**：显而易见。
 *
 * ## ⚠️ 本脚本**守不住**的事
 *
 * · **测试代码**（`#[cfg(test)] mod tests` 内）的引用 —— 它们用 `crate::xxx`
 *   是为了 mock / 直接访问内部状态，把它们圈进域约束反而会把测试改写得很难看。
 *   所以一律排除。状态机在 `findTestModuleRanges` 里（粗略分大括号，够用）。
 * · **前端**（`src/`）：本脚本**只扫** `src-tauri/src` 下 `.rs`。`presentation`
 *   域没有独立的 `consumes`，这条在域图里写明。
 * · **`activeHome` 的是否属实**：依然是 `check-domain-map.mjs` 的边界。
 * · **汇编字符串 / proc-macro 生成的代码**：不扫，rust parser 也分不清。
 *
 * ## 用法
 *
 *     node scripts/check-domain-deps.mjs
 *
 * 退出码：0 = 通过；1 = 有跨域引用不合法。
 */

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import domainMap from "../docs/domains.data.mjs";
import { isAppCodePath } from "./semver.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/**
 * 装配/crosscutting 模块，被各域随意引用是合理的，不该被守门拦。
 * 它们之所以留在 unmapped（而不是被某个域认领），是因为认领它们会让
 * "那个域"变成什么都管的杂物筐。
 */
const ASSEMBLY_MODULES = new Set([
  "commands", // 6422 行的命令注册表，前后端边界（Phase 7 拆）
  "state", // 1494 行 / 跨领域共享状态（耦合热点）
  "network", // network/mod.rs：起 TCP 监听 + UDP 发现（注：crate::network::file/ble/transport/discovery 会落到具体域）
  "lib", // 入口（不应该被 use，但万一出现）
  "main", // 入口（不应该被 use，但万一出现）
]);

let ok = true;
const fail = (msg) => {
  ok = false;
  console.error(msg);
};

/** 递归收集 `src-tauri/src` 下所有 .rs 文件。 */
function collectRustFiles(absDir, acc = []) {
  for (const e of readdirSync(absDir, { withFileTypes: true })) {
    if (e.name === "target" || e.name === "node_modules" || e.name.startsWith(".")) continue;
    const abs = path.join(absDir, e.name);
    if (e.isDirectory()) collectRustFiles(abs, acc);
    else if (e.name.endsWith(".rs")) acc.push(abs);
  }
  return acc;
}

/** 展开一条 `paths` 项：目录 → 目录下所有 .rs，文件 → 自身。 */
function expandPath(entry) {
  const abs = path.join(ROOT, entry);
  if (!existsSync(abs)) return null;
  if (statSync(abs).isDirectory()) return collectRustFiles(abs);
  return [abs];
}

/**
 * 把 `src-tauri/src/network/file.rs` 这样的物理路径翻译成 rust 模块路径
 * `network::file`（去前缀 src-tauri/src、去后缀 .rs、斜杠换 ::）。
 *
 * 关键：每个文件**独立一份** module path —— 否则不同域的同名首段
 * （如 transport 域与 files 域都用到 `network/`）会互相覆盖。
 */
function fileToModulePath(absPath) {
  const norm = path.normalize(absPath);
  const marker = `${path.sep}src-tauri${path.sep}src${path.sep}`;
  const idx = norm.indexOf(marker);
  const rel = idx >= 0 ? norm.slice(idx + marker.length) : norm;
  const noExt = rel.endsWith(".rs") ? rel.slice(0, -3) : rel;
  return noExt.split(path.sep).join("::");
}

/**
 * 找出文件中所有 `#[cfg(test)] mod NAME { ... }` 块的行范围。
 *
 * 状态机做法：
 *   1. 看到 `#[cfg(test)]` 注解开始；
 *   2. 跳过紧跟的 `#[allow(...)]` / `#[cfg(...)]` 之类注解直到看到 `mod NAME {`；
 *   3. 数 `{}` 找匹配的闭合（剥注释后逐字符）。
 *
 * 粗略处理：在 rust 源码的常见形态下足够用 —— 真正的 parser 我们不需要。
 *
 * @returns {{start: number, end: number}[]}
 */
function findTestModuleRanges(content) {
  const lines = content.split("\n");
  const ranges = [];

  for (let i = 0; i < lines.length; i++) {
    if (!/^\s*#\[cfg\(test\)\]/.test(lines[i])) continue;

    let j = i + 1;
    while (j < lines.length && /^\s*#\[[a-zA-Z_]/.test(lines[j])) j++;
    if (j >= lines.length) break;
    const m = lines[j].match(/^(\s*)mod\s+(\w+)\s*(?:\{|;)/);
    if (!m || lines[j].includes(";")) {
      i = j;
      continue;
    }
    const startLine = j;

    let depth = 1;
    let k = j + 1;
    for (; k < lines.length; k++) {
      const stripped = lines[k].replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/, "");
      for (const ch of stripped) {
        if (ch === "{") depth++;
        else if (ch === "}") depth--;
        if (depth === 0) break;
      }
      if (depth === 0) break;
    }
    if (depth !== 0) {
      // 配对失败 —— 保守处理：不排除（宁可漏检不可误排）
      ranges.push({ start: i, end: startLine });
    } else {
      ranges.push({ start: i, end: k });
    }
    i = k;
  }
  return ranges;
}

/** 给定行号，判断它是否在任何一个 #[cfg(test)] mod 块的范围内。 */
function isInTestRange(idx, ranges) {
  for (const r of ranges) if (idx >= r.start && idx <= r.end) return true;
  return false;
}

/**
 * 提取文件中**生产代码**的 `use crate::xxx` 引用的 module path。
 * 返回 `Array<{line, modulePath}>`，modulePath 是 `crate::` 之后的整段，
 * 如 `mesh::router`、`network::transport`。
 */
function scanUseCrate(content) {
  const lines = content.split("\n");
  const testRanges = findTestModuleRanges(content);
  const refs = [];

  for (let i = 0; i < lines.length; i++) {
    if (isInTestRange(i, testRanges)) continue;
    const line = lines[i];
    const m = line.match(/^\s*use\s+crate::([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z_][a-zA-Z0-9_]*)*)/);
    if (!m) continue;
    refs.push({ line: i + 1, modulePath: m[1] });
  }
  return refs;
}

/**
 * ★ #133-A：**只报告、不判红**的那一半 —— 数本文件里"内联写法"的跨域引用。
 *
 * 为什么要数它：本守门原先只看 `use crate::xxx` 行，而 `db/messages.rs` 里那条
 * **地图自己注释成"真生产依赖"**的耦合（`crate::protocol::MentionTarget` /
 * `parse_mention_targets_column`，`docs/domains.data.mjs` 里 db 的 consumes 注释写着它）
 * 一行 `use` 都没有 ⇒ 判据对着一条已声明的生产依赖完全看不见。
 * 这条洞不是"理论上的绕法"，是实测到的。
 *
 * 为什么先不判红：把内联也纳入判据会把历史上按 `use` 口径建起来的 `consumes` 名单一次性判红，
 * 而"补声明"还是"改代码"属于要人拍板的面 ⇒ 现在先把分母打印出来，让洞变成能看见的数。
 *
 * 计数是**上界**：注释与字符串里出现的 `crate::` 也会被数进去（不解析语法树）。
 */
const INLINE_RE = /crate::([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z_][a-zA-Z0-9_]*)*)/g;
function scanInlineCrate(content) {
  const lines = content.split("\n");
  const testRanges = findTestModuleRanges(content);
  const out = [];
  for (let i = 0; i < lines.length; i++) {
    if (isInTestRange(i, testRanges)) continue;
    const line = lines[i];
    if (/^\s*(pub\s+)?use\s/.test(line)) continue; // use 行由 scanUseCrate 负责
    if (/^\s*(\/\/|\/\*)/.test(line)) continue; // 整行注释
    let m;
    INLINE_RE.lastIndex = 0;
    while ((m = INLINE_RE.exec(line)) !== null) out.push({ line: i + 1, modulePath: m[1] });
  }
  return out;
}

/// #133 的两个计数器（由主循环填，最后一起打印）
let skippedTestFiles = 0;
let skippedTestRefs = 0;
/** @type {Map<string, {count: number, sample: string}>} `源域→目标域` 的内联耦合（判据看不见的哪些） */
const inlineOnlyPairs = new Map();

/**
 * 建立 `rust module path → 领域 id` 的精确索引（基于所有领域的 `paths`）。
 *
 * 关键设计：每个 `.rs` 文件**独立一条**索引（rust 模块路径如
 * `network::transport` / `transport::tcp` / `mesh::router`），不共用
 * "首段" —— 因为 `network/` 下同时有 transport 域（`network::transport`）、
 * presence 域（`network::discovery`）、files 域（`network::file`），按首段
 * 会让后注册的覆盖先注册的，整张地图就跑偏了。
 */
function buildDomainIndex() {
  /** @type {Map<string, string>} rust module path → 领域 id */
  const index = new Map();

  for (const d of domainMap.domains) {
    for (const entry of d.paths ?? []) {
      const expanded = expandPath(entry);
      if (!expanded) continue;
      for (const f of expanded) {
        const modulePath = fileToModulePath(f);
        // 同一 modulePath 理论上不会跨域（domains.data.mjs 的判据 C 保证
        // `paths` 不重叠），但保险起见：发现冲突就大声报错，避免静默吞掉。
        const prev = index.get(modulePath);
        if (prev && prev !== d.id) {
          console.error(`  ⚠️ 模块路径冲突：${modulePath} 既归「${prev}」又归「${d.id}」`);
        }
        index.set(modulePath, d.id);
      }
    }
  }
  return index;
}

/** 把 modulePath 字符串（如 `network::transport` 或 `crypto`）拆成段数组。 */
function splitMod(p) {
  return p.split("::");
}

/**
 * 给定 `use crate::xxx::yyy::zzz` 的 `xxx::yyy::zzz` 部分解析出它落到
 * 哪个领域（id）/"assembly"/"unmapped"。
 *
 * 优先按**最长前缀**匹配：先试整段、失败剥最后一段、直到只剩首段。
 * 这样 `crate::mesh::router::MeshRouter` 既能精确命中 `mesh::router`，
 * 也能退到 `mesh::router` 再退到 `mesh` —— 永远不会张冠李戴。
 */
function resolveDomain(modulePath, index) {
  const parts = splitMod(modulePath);
  for (let len = parts.length; len >= 1; len--) {
    const key = parts.slice(0, len).join("::");
    const hit = index.get(key);
    if (hit) return hit;
  }
  if (ASSEMBLY_MODULES.has(parts[0])) return "assembly";
  return "unmapped";
}

const idToDomain = new Map(domainMap.domains.map((d) => [d.id, d]));
const index = buildDomainIndex();

const violations = [];

// ---------------- H/I. consumes 写错或自引 ----------------
console.log("判据 H/I：consumes 字段本身合法（id 必须存在、不能自引）");
for (const d of domainMap.domains) {
  if (!Array.isArray(d.consumes)) {
    fail(`  ✗ 领域 ${d.id} 缺 consumes（数组）。守门不敢默认放过 —— 写个 [] 也行。`);
    continue;
  }
  for (const target of d.consumes) {
    if (target === d.id) {
      fail(`  ✗ 领域 ${d.id} 的 consumes 引了自己`);
    } else if (!idToDomain.has(target)) {
      fail(`  ✗ 领域 ${d.id} 的 consumes 引用了不存在的领域：${target}`);
    }
  }
}
if (ok) console.log("  ✓ consumes 字段全部合法");

// ---------------- G. 跨领域引用受 consumes 约束 ----------------
console.log("\n判据 G：每个领域的 use crate::xxx 受 consumes 约束");
for (const d of domainMap.domains) {
  const allowed = new Set(d.consumes ?? []);
  const files = [];
  for (const entry of d.paths ?? []) {
    const expanded = expandPath(entry);
    if (!expanded) continue; // 路径不存在是 check-domain-map.mjs 的事
    files.push(...expanded);
  }
  if (files.length === 0) continue; // friendship 没文件

  for (const f of files) {
    const rel = path.relative(ROOT, f);
    let content;
    try {
      content = readFileSync(f, "utf8");
    } catch {
      continue;
    }
    // ★ #133-B：独立测试文件按仓里**已有那一份**口径处理 —— `semver.mjs` 的 `isAppCodePath`
    //   已把 `src-tauri/src/**_tests.rs` 判成"不是应用码"（`check-change-budget.mjs` 也复用它），
    //   而本脚本原先只跳过 `#[cfg(test)] mod` **块内** ⇒ 整份测试文件被当生产扫，
    //   与本文件自己那句"提取**生产代码**的 use crate::"对不上。不复用就会长出第二个家。
    if (!isAppCodePath(rel.replaceAll("\\", "/"))) {
      skippedTestFiles += 1;
      skippedTestRefs += scanUseCrate(content).length;
      continue;
    }
    const refs = scanUseCrate(content);
    // ★ #133-A：把"只有内联写法、判据完全看不见"的跨域对数出来（只报告，不判红）
    const seenTargets = new Set();
    for (const ref of refs) {
      const t = resolveDomain(ref.modulePath, index);
      if (t !== d.id && t !== "assembly" && t !== "unmapped") seenTargets.add(t);
    }
    for (const im of scanInlineCrate(content)) {
      const t = resolveDomain(im.modulePath, index);
      if (t === d.id || t === "assembly" || t === "unmapped" || seenTargets.has(t)) continue;
      const key = `${d.id}→${t}`;
      const cur = inlineOnlyPairs.get(key);
      if (cur) cur.count += 1;
      else inlineOnlyPairs.set(key, { count: 1, sample: `${rel}:${im.line} crate::${im.modulePath}` });
    }
    for (const ref of refs) {
      const target = resolveDomain(ref.modulePath, index);
      if (target === d.id) continue; // self —— 域内引用,不算跨域
      if (target === "assembly" || target === "unmapped") continue;
      if (!allowed.has(target)) {
        violations.push({
          sourceDomain: d.id,
          sourceFile: rel,
          sourceLine: ref.line,
          refPath: `crate::${ref.modulePath}`,
          target,
        });
      }
    }
  }
}

if (violations.length === 0) {
  console.log("  ✓ 全部跨领域引用都在消耗方声明的 consumes 中");
} else {
  fail(`  ✗ ${violations.length} 条跨领域引用不在消耗方域的 consumes 列表里：`);
  for (const v of violations) {
    console.error(
      `      ${v.sourceFile}:${v.sourceLine}  在「${v.sourceDomain}」域内 ${v.refPath}\n` +
        `        → 想依赖「${v.target}」域，但「${v.sourceDomain}」的 consumes 列表里没有它。\n` +
        `        修法（请谨慎二选一）：① 把「${v.target}」加进「${v.sourceDomain}」的 consumes（依赖是合理的）；\n` +
        `                              ② 删掉这条 use 或挪到装配层（state/commands）`,
    );
  }
}

// ---------------- 汇总 ----------------
if (ok) {
  console.log("\n✓ 领域依赖方向与 consumes 声明一致。");
  console.log("  （边界：独立测试文件不算生产耦合（下面打印忽略了多少）/ 前端不扫 / activeHome 由 check-domain-map.mjs 守。）");
  // ★ #133 的两条分母，**每次跑都自己打印**，免得"已知边界"又变回一句没人核对的手抄话
  console.log(
    `  · 按 isAppCodePath 忽略独立测试文件 ${skippedTestFiles} 个（其中 use crate:: ${skippedTestRefs} 条）`,
  );
  const pairs = [...inlineOnlyPairs.entries()].sort((a, b) => b[1].count - a[1].count);
  const inlineTotal = pairs.reduce((s, [, v]) => s + v.count, 0);
  console.log(
    `  · 判据看不见的内联跨域引用：**${inlineTotal} 处**（上界，含注释与字符串里的 crate::），`
      + `去重后 ${pairs.length} 对 源域→目标域`,
  );
  for (const [k, v] of pairs.slice(0, 8)) {
    console.log(`      ${k}  ${v.count} 处  首例 ${v.sample}`);
  }
  if (pairs.length > 8) console.log(`      …其余 ${pairs.length - 8} 对未列`);
  console.log("      ⚠️ 这些**不判红**：把它们纳入判据会把按 use 口径建起来的 consumes 名单整片判红，"
    + "\n         而「补声明还是改代码」是要人拍板的面（#133-A）。先让洞有数。");
  // ★ 但"有数"不等于"随便涨"：这里上一把**只许变小**的棘轮 —— 冻的不是处数（那是上界，
  //   会被注释里的 `crate::` 之类的噪声牵着抖），冻的是**去重后的源域→目标域对数**。
  //   为什么这样收：内联耦合每多一对，就是又有一条形同"已声明的生产依赖"（db→messaging 那种）
  //   走到判据看不见的地方去；存量不判红是要人拍板，**新增**没有这个理由。
  //   反证入口（不用改文件就能证明它会红）：`GOSSLAN_DOMAIN_INLINE_PAIR_MAX=12 node scripts/check-domain-deps.mjs`
  //   ⚠️ 上限必须钉成**存量数字**而不是"当天的条数" —— 取后者等于恒真，棘轮永远不会咬。
  //      存量 13 对是 2026-09-28 现算（复跑同一条命令看上面那行打印）；**只许调小**，
  //      调大等于把洞重新变沉默 ⇒ 要调必须先在这行注释里写下谁同意、为什么。
  const pairMax = Number(process.env.GOSSLAN_DOMAIN_INLINE_PAIR_MAX ?? "13");
  if (pairs.length > pairMax) {
    console.error(
      `  ✗ 内联跨域引用对数 ${pairs.length} 超过棘轮上限 ${pairMax}`
        + "\n      ⇒ 判据看不见的耦合**又长了一对**。修法只有两条：① 把这次新增的那条改成 use 并加进 consumes"
        + "\n         （让它回到判据看得见的地方）；② 别新增。"
        + "\n      ⚠️ 不许直接把上限调大：那等于把洞又变成沉默的。",
    );
    process.exit(1);
  }
  process.exit(0);
}
console.error("\n✗ 跨领域依赖与 consumes 声明不一致 —— 先决定是修 consumes 还是删 use,别打补丁。");
process.exit(1);
