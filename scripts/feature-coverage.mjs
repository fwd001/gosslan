#!/usr/bin/env node
/**
 * 功能面 → 判据 的覆盖量具（#149）。
 *
 * ## 它回答什么
 * 「所有能自动测的功能都划进自动测试」这句话要能复查，先得有一张"哪些后端能力从没在任何
 * 判据语料里出现过"的清单。本脚本就是那张清单的**现算版**：分母由它自己打印，不做基线、
 * 不进任何门禁层（和 `check-domain-deps` 的"只打印、不纳判"同一口径）。
 *
 * ## 为什么不写成一张表
 * 写进文档的"N 条已覆盖"当天就会漂，而且会变成第二份需要人回头维护的事实源
 * （本仓为这类账吃过多次教训）。所以这里只留量具：任何时刻重跑同一条命令就是当下的真相。
 *
 * ## 为什么不纳判（不做成红/绿）
 * 命中的判据是**线索不是证明**：
 * - 命中可能只是测试里顺手写了这个字符串（不代表那条命令被断过言）；
 * - 未命中也可能是"被间接判到了"——例如一批命令都只是 `get_settings` 的读法，
 *   而 E2E 判的是"界面拿到快照后显示了什么"。
 * 所以判红会产出一堆为了绿而写的假测试。真要用法：看"一处都没命中"的那批，逐条问
 * "它有没有用户可感知的行为？没有 -> 记进 MANUAL；有 -> 补最便宜的那层判据"。
 *
 * 复跑：`node scripts/feature-coverage.mjs` 或 `npm run coverage:features`
 *   看全量清单加 `--all`；要机器读加 `--json`。
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname.replace(/\/$/, "");
const ALL = process.argv.includes("--all");
const JSON_OUT = process.argv.includes("--json");

function walk(dir, out = []) {
  for (const e of readdirSync(join(ROOT, dir), { withFileTypes: true })) {
    if (e.name === "node_modules" || e.name === "target" || e.name.startsWith(".")) continue;
    const rel = join(dir, e.name);
    if (e.isDirectory()) walk(rel, out);
    else out.push(rel);
  }
  return out;
}

/** ① 功能面：api 门面向后端要的每一个名字（invoke 的命令名 + listen 的事件名）。 */
const apiSrc = readFileSync(join(ROOT, "src/api/index.ts"), "utf8");
const surface = new Map(); // name -> Set<"invoke"|"listen">
for (const m of apiSrc.matchAll(/\binvoke(?:<[^>]*>)?\s*\(\s*"([a-z0-9_]+)"/g)) {
  if (!surface.has(m[1])) surface.set(m[1], new Set());
  surface.get(m[1]).add("invoke");
}
for (const m of apiSrc.matchAll(/\blisten(?:<[^>]*>)?\s*\(\s*"([a-z0-9:_-]+)"/g)) {
  if (!surface.has(m[1])) surface.set(m[1], new Set());
  surface.get(m[1]).add("listen");
}

/** ② 判据语料：只有"测试/守卫/E2E"这一侧算，生产码自己不算（否则永远命中自己）。 */
const CORPORA = [
  ["rust 测试", (f) => /(^|\/)[^/]*_tests\.rs$/.test(f) || /(^|\/)tests\.rs$/.test(f)],
  ["rust 内联测试", (f) => f.startsWith("src-tauri/src/") && f.endsWith(".rs")],
  ["前端测试", (f) => f.endsWith(".test.ts")],
  ["跨实例 E2E", (f) => f === "scripts/e2e-multi-instance.mjs"],
  ["护栏注入", (f) => f === "scripts/verify-guards.py"],
  ["UI 运行时探针", (f) => f.startsWith("scripts/ui-runtime")],
];
const allFiles = [...walk("src-tauri/src"), ...walk("src"), ...walk("scripts")].filter((f) =>
  /\.(rs|ts|py|mjs|vue)$/.test(f),
);
// rust 内联测试要只看带 #[cfg(test)] 的那些文件，否则等于把生产码整个算进语料
const rustInline = new Set(
  allFiles
    .filter((f) => f.startsWith("src-tauri/src/") && f.endsWith(".rs"))
    .filter((f) => readFileSync(join(ROOT, f), "utf8").includes("#[cfg(test)]")),
);
const texts = new Map();
for (const [label, pred] of CORPORA) {
  const files = allFiles.filter((f) => {
    if (label === "rust 内联测试") return rustInline.has(f);
    if (label === "rust 测试") return pred(f) && !rustInline.has(f);
    return pred(f);
  });
  texts.set(
    label,
    files.map((f) => ({ f, body: readFileSync(join(ROOT, f), "utf8") })),
  );
}

const hits = new Map(); // name -> [corpus labels]
for (const name of surface.keys()) {
  const found = [];
  for (const [label, entries] of texts) {
    if (entries.some(({ body }) => body.includes(name))) {
      found.push(label);
    }
  }
  hits.set(name, found);
}

/**
 * ③ 第三个态：**经委托判到**。
 *
 * 命令体大多是 `db::xxx(&dbc, ..)` 的薄包装，而判据往往写在 `db::xxx` 那一层 ——
 * 只按命令名搜语料会把这些一律报成"没测"，那是**误导下一个人的假缺口**
 * （本仓对"账表只覆盖作者想到的那一半"已经吃过教训）。所以这里再解一次包装：
 * 从命令体里抓它调用的 `db::fn` 与 `crate::module::fn`，去语料里搜那个名字。
 */
const cmdFiles = allFiles.filter(
  (f) => f.startsWith("src-tauri/src/commands/") && f.endsWith(".rs") && !/_tests\.rs$/.test(f),
);
const cmdText = cmdFiles.map((f) => readFileSync(join(ROOT, f), "utf8")).join("\n");
const delegated = new Map(); // name -> [被委托的函数名]
for (const name of surface.keys()) {
  if (hits.get(name).length > 0) continue;
  const at = cmdText.indexOf(`fn ${name}(`);
  if (at < 0) continue;
  // **按花括号配平**取本命令的函数体。第一版这里是"切到下一个 `pub fn` 之前"，
  // 结果把邻居命令的委托一起算进来（实测：`get_group_reads` 被报成调 12 个 db 函数，
  // `pin_group_message` 16 个 —— 那显然是假命中）。命令之间隔着 `#[tauri::command]`，
  // 只靠"下一个 pub fn"根本切不开。
  const open = cmdText.indexOf("{", at);
  if (open < 0) continue;
  let depth = 0;
  let end = -1;
  for (let i = open; i < cmdText.length; i++) {
    const c = cmdText[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) { end = i; break; }
    }
  }
  if (end < 0) continue;
  const body = cmdText.slice(at, end + 1);
  const callees = [...body.matchAll(/\b(?:db|crate::db)::([a-z0-9_]+)\(/g)].map((m) => m[1]);
  const judged = [...new Set(callees)].filter((fn) =>
    [...texts.values()].some((entries) => entries.some(({ body: b }) => b.includes(fn))),
  );
  if (judged.length) delegated.set(name, judged);
}
const indirect = [...delegated.keys()];
const missed = [...hits.entries()]
  .filter(([k, v]) => v.length === 0 && !delegated.has(k))
  .map(([k]) => k);

if (JSON_OUT) {
  console.log(JSON.stringify({ denominator: hits.size, missed, indirect }, null, 2));
} else {
  const covered = hits.size - missed.length - indirect.length;
  console.log(`功能面分母：${hits.size} 个后端名字（api 门面里的 invoke 命令名 + listen 事件名，现算）`);
  for (const [label] of CORPORA) {
    const n = [...hits.values()].filter((v) => v.includes(label)).length;
    console.log(`  · ${label.padEnd(16)} 语料 ${String(texts.get(label).length).padStart(4)} 个文件，命中 ${String(n).padStart(3)} / ${hits.size}`);
  }
  console.log(`**直接**在任何判据语料里出现的：${covered}；经命令体委托到 db 函数而被判到的：${indirect.length}；三样都不沾的：${missed.length}`);
  console.log(`⚠️ 两个方向都要记住：命中是**线索不是证明**（语料里可能只是顺手写了这个字符串）；`);
  console.log(`   "经委托"也只说明被委托的那个函数出现在语料里，不保证断言覆盖了这条命令的语义。`);
  if (ALL && indirect.length) {
    console.log(`\n经委托判到的（加 --all 才列，为了可抽查）：`);
    for (const [k, v] of delegated) console.log(`  · ${k} -> db::${v.join(", db::")}`);
  }
  if (missed.length) {
    console.log(`\n三样都不沾的（逐条问：有用户可感知的行为吗？有 -> 补最便宜那层的判据；纯 OS/窗口 pass-through -> 记进 MANUAL）：`);
    const list = ALL ? missed : missed.slice(0, 40);
    for (const m of list) console.log(`  · ${m}`);
    if (!ALL && missed.length > list.length) console.log(`  … 共 ${missed.length} 条，加 --all 看全量`);
  }
}
// 量具不是判据：永远退 0，避免它哪天变成"为了绿而写测试"的压力来源。
process.exit(0);
