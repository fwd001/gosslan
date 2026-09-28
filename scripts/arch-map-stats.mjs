#!/usr/bin/env node
/**
 * 架构图取数量具（只读）：把 `docs/ARCHITECTURE-MAP.html` 里那些"现状戳"一次算齐。
 *
 * 为什么要它（实测前提）：图上的数是手抄的，而图**天生滞后** —— 今天现算就撞到三处：
 * 注册命令 130（图上写 129）、`db.rs` 建表 20 张（图上写 19）、`db::` 公开函数 118（图上写 114）。
 * 之前每刷一次就要临时现推一把正则，而临时正则本身会写窄（我这次就错过两次：
 * `#[tauri::command]` 漏了 `#[tauri::command(async)]`；按"第一个 `]`"切 `generate_handler!` 数出 221 条假数）。
 * ⇒ 把这些口径**固化成一份代码**，图上只写"复跑本命令"，不再抄数（§13 的判法不变：**不加相等判据**，
 *   这张图是给人看的地图，不是门禁；但它必须随时能被一条命令刷成真值）。
 *
 * 用法：`node scripts/arch-map-stats.mjs`（无参数、无写入、无网络）
 */

import { execFileSync } from "node:child_process";
import { stripRustComments } from "./rustSrc.ts";
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const ROOT = path.resolve(import.meta.dirname, "..");
const R = (p) => readFileSync(path.join(ROOT, p), "utf8");

/** 递归收 .rs（跳过 target 与 .git）。 */
function rustFiles(dir) {
  const out = [];
  for (const e of readdirSync(path.join(ROOT, dir), { withFileTypes: true })) {
    if (e.name === "target" || e.name === ".git" || e.name === "node_modules") continue;
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...rustFiles(p));
    else if (e.name.endsWith(".rs")) out.push(p);
  }
  return out;
}

const row = (name, value, how) => console.log(`${String(name).padEnd(26)} = ${String(value).padEnd(9)} 口径：${how}`);
/// ⚠️ 任何「按形态数」的计数**必须先剥注释**：第一版没剥，`db.rs` 注释里那两处 `CREATE TABLE IF NOT EXISTS`
/// 被当成建表，18 张被数成 20 —— 差点把假数刷进图。Rust 的剥法用共用件（`scripts/rustSrc.ts`），不在此重写。
const code = (text) => stripRustComments(text);
const line = (t) => console.log(`\n── ${t}`);

const pkg = JSON.parse(R("package.json"));
const head = execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: ROOT, encoding: "utf8" }).trim();
row("版本 / HEAD", `${pkg.version} / ${head}`, "`node -e 'console.log(require(`./package.json`).version)'` + `git rev-parse --short HEAD`");

line("存储（SQLite）");
const dbSrc = R("src-tauri/src/db.rs");
const dbVersion = /pub const DB_VERSION: u32 = (\d+)/.exec(dbSrc)?.[1];
row("DB_VERSION", dbVersion, "`grep -n 'pub const DB_VERSION' src-tauri/src/db.rs`");
const tables = [...code(dbSrc).matchAll(/CREATE TABLE(?: IF NOT EXISTS)?\s+(?:\w+\.)?(\w+)/g)].map((m) => m[1]);
row("建表数", tables.length, "`db.rs` 里 `CREATE TABLE` 出现次数（不含测试文件）");
row("索引数", (code(dbSrc).match(/CREATE (UNIQUE )?INDEX/g) || []).length, "`db.rs` 里 `CREATE [UNIQUE] INDEX` 次数");
row("新增列的迁移", [...code(dbSrc).matchAll(/ALTER TABLE/g)].length, "`db.rs` 里 `ALTER TABLE` 次数（v10→v11 的 mention_targets 在这一类）");

line("命令面（IPC 入口）");
const lib = R("src-tauri/src/lib.rs");
/**
 * `generate_handler![…]` 的条目**按括号结构**数：从那一行往后走到与它同缩进的 `])`，
 * 中间每行一条。⚠️ 别拿"第一个 `]`"当结尾（列表里有嵌套 `vec![…]` / 多行注释时会截错，
 * 实测这样数出过 221 这种假数）。
 */
function handlerEntries() {
  const lines = lib.split("\n");
  const start = lines.findIndex((l) => l.includes("generate_handler!["));
  if (start < 0) throw new Error("找不到 generate_handler!");
  const baseIndent = lines[start].search(/\S/);
  const out = [];
  for (let i = start + 1; i < lines.length; i++) {
    const t = lines[i].trim();
    const ind = lines[i].search(/\S/);
    if (t.startsWith("])") || (t === "]" && ind <= baseIndent)) return { out, end: i + 1 };
    if (!t || t.startsWith("//")) continue;
    out.push(t.replace(/,$/, "").trim());
  }
  throw new Error("generate_handler 列表没找到闭合行");
}
const handlers = handlerEntries();
row("注册命令", handlers.out.length, `lib.rs 第 ${start_()}–${handlers.end} 行的 \`generate_handler![…]\` 条目数`);
const attrs = rustFiles("src-tauri/src").reduce((s, p) => {
  const t = R(p);
  return s + (code(t).match(/#\[tauri::command(\(|\])/g) || []).length;
}, 0);
row("命令属性总数", attrs, "`#[tauri::command]` 与 `#[tauri::command(…)]` 两种写法都算（只数裸写法会少）");
const prod = rustFiles("src-tauri/src").filter((p) => !/_tests\.rs$/.test(p));
const dbFns = prod.filter((p) => p.includes("/db/"))
  .reduce((s, p) => s + (code(R(p)).match(/pub(\(crate\))? fn /g) || []).length, 0);
row("db:: 公开函数", dbFns, "`src-tauri/src/db/**.rs`（不含 `*_tests.rs`）里 `pub fn` / `pub(crate) fn` 数");

line("事件面（后端 → 前端）");
const emits = new Map();
for (const p of prod) {
  for (const m of code(R(p)).matchAll(/\.emit(?:_sync)?\(\s*"([^"{]+)"/g)) emits.set(m[1], p);
}
row("字面量事件名", emits.size, "生产 .rs 里 `.emit(「…」)` / `.emit_sync(「…」)` 的字面量（`format!` 那种动态名不在内）");
let dynEmit = 0;
for (const p of prod) dynEmit += (code(R(p)).match(/\.emit(?:_sync)?\(\s*format!/g) || []).length;
row("动态拼的事件名", dynEmit, "`.emit(format!(…))` 出现次数 —— 这些名字**机器点不齐**，别当成全覆盖");
const api = R("src/api/index.ts");
const listens = new Set([...api.matchAll(/listen(?:<[^>]*>)?\(\s*["']([^"']+)["']/g)].map((m) => m[1]));
row("门面 listen 名", listens.size, "`src/api/index.ts` 里字面量事件名（去重）");
const invokes = new Set([...api.matchAll(/invoke(?:<[^>]*>)?\(\s*["']([^"']+)["']/g)].map((m) => m[1]));
row("门面 invoke 名", invokes.size, "`src/api/index.ts` 里字面量命令名（去重）");
const unbound = [...emits.keys()].filter((n) => !listens.has(n));
const evtTest = R("src/api/events.test.ts");
const allowBlock = /const ALLOWED_EMIT_WITHOUT_LISTENER: Record<string, string> = \{([\s\S]*?)\n\};/.exec(evtTest)?.[1] ?? "";
const allowed = new Set([...allowBlock.matchAll(/"(?:([^"\\]+)|\n\s*"([^"\\]+))":/g)].map((m) => m[1] ?? m[2]).filter(Boolean));
const unexcused = unbound.filter((n) => !allowed.has(n));
row("发了但门面没听", unbound.length,
  `其中 ${unbound.filter((n) => allowed.has(n)).length} 个在 \`events.test.ts\` 的显式豁免表里（各写理由）；`
  + (unexcused.length ? `**${unexcused.length} 个没理由 = 真漏接：${unexcused.join(", ")}**` : "没有无理由的"));
if (unexcused.length) process.exitCode = 1;

line("窗口与领域");
const labels = /pub const WINDOW_LABELS: &\[&str\] = &\[(.*?)\];/s.exec(lib)?.[1] ?? "";
const wins = labels.split(",").map((x) => x.trim()).filter(Boolean);
/// ⚠️ `matchAll` 的每一项是**整个匹配 + 各捕获组**的数组：解构 `[x]` 拿到的是 `m[0]`（整条匹配文本），
/// 不是捕获组。第一版就因此把"整条匹配"当常量名去比 `wins`，结果 6 个全部"不在清单里"（假报）。
const winConsts = [...code(lib).matchAll(/pub const (WINDOW_[A-Z_]+): &str = "(.+?)"/g)]
  .map((m) => ({ name: m[1], lit: m[2] }));
const notInList = winConsts.filter((w) => w.name !== "WINDOW_LABELS" && !wins.includes(w.name))
  .map((w) => `${w.name}="${w.lit}"`);
row("窗口常量", winConsts.length, "`lib.rs` 里 `pub const WINDOW_*: &str = \"…\"` 的条数（一扇窗一个）");
row("capability 清单", wins.length, "`WINDOW_LABELS` 数组；"
  + (notInList.length ? `**刻意不在里面**的：${notInList.join(" ")}（`
    + "外链窗由 lib.rs 的断言钉着『不许进正向清单』，远端页面不该拿到 IPC 能力 ⇒ 别当漏项去『补上』）" : "全都在"));
row("常驻标记", /const AUX_WINDOWS_RESIDENT: bool = (\w+)/.exec(R("src-tauri/src/commands/logs.rs"))?.[1] ?? "?",
  "`commands/logs.rs` 的 `AUX_WINDOWS_RESIDENT`（决定「常驻窗口要不要重拉兜底」）");
/// ⚠️ 顶层键不是领域清单：那份文件导出的形状是 `{ version, coverageRoots, unmapped, domains }`，
/// 领域在 `domains` 里。第一版直接 `Object.keys(默认导出)` 数出 4 —— 那是"文件里有几个顶层键"，
/// 不是"有几个领域"。**口径要指到数据本身那一层，不能指到容器。**
const domainData = (await import("../docs/domains.data.mjs")).default;
/// `domains` 是**数组**（元素 `{id,name,tier,paths,invariants}`），不是以领域为键的对象
/// ⇒ 直接 `Object.keys` 会数出 0/1/2… 这种索引名。取 `id`，条数按数组算。
const domList = Array.isArray(domainData.domains) ? domainData.domains : Object.values(domainData.domains ?? {});
const domNames = domList.map((d) => d.id ?? d.name);
row("领域数", domNames.length, "`import docs/domains.data.mjs` 后取 `domains`（数组，元素含 id/name/tier/paths/invariants）：" + domNames.join("/"));
row("领域数据版本", `${domainData.version} / 未映射 ${Object.keys(domainData.unmapped ?? {}).length} / 覆盖根 ${Array.isArray(domainData.coverageRoots) ? domainData.coverageRoots.length : typeof domainData.coverageRoots}`,
  "同一份文件的 `version` / `unmapped` / `coverageRoots`（判据读的就是这份）");

line("不变量与验证体系");
const inv = [...new Set((R("docs/protocol-invariants.md").match(/INV-P\d{2}/g) || []))];
row("不变量条数", inv.length, "`docs/protocol-invariants.md` 里出现过的 INV-P## 去重（绑定由 `check-invariant-hooks` 判）");
row("门禁步数", "见下行", "`node scripts/verify.mjs --list` 自己打印各层步数（判据 A 同源，这里不抄）");

line("大文件（>3000 行，本轮要按「零风险」拆的那批）");
const big = [];
for (const dir of ["src-tauri/src", "src"]) {
  const walk = (d) => {
    for (const e of readdirSync(path.join(ROOT, d), { withFileTypes: true })) {
      if (e.name === "node_modules" || e.name.startsWith(".")) continue;
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (/\.(rs|vue|ts)$/.test(e.name)) {
        const n = R(p).split("\n").length;
        if (n > 3000) big.push([p, n]);
      }
    }
  };
  walk(dir);
}
big.sort((a, b) => b[1] - a[1]);
for (const [p, n] of big) {
  const t = R(p);
  const inTests = /(^|\n)\s*(#\[cfg\(test\)\]|mod tests)\b/.test(t)
    ? `  ← 含 tests 段（净实现行数要单独算）` : "";
  console.log(`  ${String(n).padStart(5)} 行  ${p}${inTests}`);
}
if (!big.length) console.log("  （没有 >3000 行的文件）");

function start_() {
  return lib.split("\n").findIndex((l) => l.includes("generate_handler![")) + 1;
}
/// 前提守卫：这几项按定义不可能为 0；真出 0 就是**口径漂了或解析坏了**，
/// 而"0 条 ⇒ 于是没东西可判 ⇒ 看着像干净"是本仓判过最坏的一种形状。
const premises = { 注册命令: handlers.out.length, 建表数: tables.length, 字面量事件名: emits.size, 领域数: domNames.length, 不变量条数: inv.length };
const dead = Object.entries(premises).filter(([, v]) => !v);
if (dead.length) {
  console.error("\n✗ 量具自己的前提塌了：" + dead.map(([k, v]) => `${k}=${v}`).join("，")
    + " —— 按定义不可能为 0，先修这里的解析，别把 0 读成「没有问题」");
  process.exitCode = 1;
}

console.log("\n· 这份输出就是「地图现状」的唯一算数口径；把它贴进图里的复跑列，别把上面的数抄进图正文当承诺。");
