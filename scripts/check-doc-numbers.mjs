#!/usr/bin/env node
/**
 * 文档硬数字守卫（总指令§十三 / roadmap B-3）
 *
 * ## 为什么要有这个脚本
 *
 * 一个通宵里抓到四处同类漂移，全是**手写数字**：
 *   ① `docs/acceptance/1.0-release.md` 写全量门禁「15 步」——实为 16，手抄的清单还漏了一步；
 *   ② `README.md` 的 IPC 条数 138 → 实际 129；
 *   ③ **门禁自己的文案也漂**：`verify.mjs` 的 `why:` 写「292 个取锁点」，
 *      而同一步在同一次运行里打印 284 guard 绑定 + 11 语句临时量 = **295**；
 *   ④ 同一个「取锁点条数」在仓库里同时存在 **292 / 295 / 296** 三种写法。
 * 共同点：**这些数字都有唯一事实源，只是被人手抄了一遍。**
 *
 * ## 两类判据
 *
 * **A. 对账**：文档里凡出现「全量 N 步 / 快速 N 步」，N 必须等于 `verify.mjs --list` 现算出来的条数。
 *    （`--list` 无副作用、退出 0，所以现算是便宜的。）
 * **B. 禁令**：指定文件里不许再手写「取锁点」的条数 —— 真相是 `check-lock-scope.mjs`
 *    每次实算并打印的，抄一份进文档就等于制造第二个事实源。
 *
 * **C. 双实例 E2E 的断言数由 harness 现算**：`scripts/e2e-multi-instance.mjs` 里按模式归堆数 `check(`
 *    调用，文档凡声明「默认轮 N 断言 / 脏前缀轮 N 断言 / 续传轮 N 断言」必须等于现算值，
 *    且三种标注**一种都不许消失**（否则「删掉标签」就是绕过这条守卫的最短路径）。
 *
 * 退出码：0 = 通过；1 = 有漂移 / 有违规手写。
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** 被管的活文档：每次开工都会被 AI 读一遍、且**句子里的数字是在声明当前事实**的那几份。
 *
 * ⚠️ 已知边界（故意不管，不是漏了）：
 *   · `docs/stability-roadmap.md` 是**漂移事故台账**，它的职责就是把"曾经写错过什么"原样记下来，
 *     所以允许出现旧数字；它自己的状态表由人手维护，本脚本判不出"这句是历史还是这句是现状"。
 *   · `CHANGELOG.md` 是带日期的历史发布记录，同理不管。
 *   ⇒ 也就是说：**这条守卫只保证"契约面"没有第二个事实源**，不保证全仓库无手写数字。
 */
const LIVE_DOCS = [
  "README.md",
  "docs/acceptance/1.0-release.md",
  "docs/acceptance/stability-smoke-matrix.md",
  "docs/protocol-invariants.md",
  "docs/ARCHITECTURE-MAP.html",
  "scripts/verify.mjs",
];

const fails = [];

// ---------- 判据 A：门禁步数必须现算对账 ----------
function stepCounts() {
  const run = (args) => {
    const r = spawnSync("node", ["scripts/verify.mjs", ...args, "--list"], {
      cwd: ROOT,
      encoding: "utf8",
    });
    if (r.status !== 0) {
      throw new Error(`verify.mjs ${args.join(" ")} --list 退出 ${r.status}：${r.stderr || r.stdout}`);
    }
    // --list 用「  N. 步骤名」编号快速层步骤；重门禁层在分隔线之后、不编号。
    const numbered = [...r.stdout.matchAll(/^\s+(\d+)\.\s/gm)].map((m) => Number(m[1]));
    return numbered.length;
  };
  return { quick: run([]), full: run(["--full-gate"]) };
}

let counts;
try {
  counts = stepCounts();
} catch (e) {
  console.error(`✗ 读不到门禁步数：${e.message}`);
  process.exit(1);
}
console.log(`· 现算门禁步数：快速层 ${counts.quick} 步 / 全量 ${counts.full} 步`);

// 只在"谈门禁"的句子里抓数字，避免把"三步走"这类散文误判成违约。
const GATE_WORD = /(全量|完整门禁|重门禁|快速层|快速|full-gate|verify)/;
const STEP_CLAIM = /([^\n]{0,28}?)(\d{1,3})\s*步/g;

for (const rel of LIVE_DOCS) {
  const abs = path.join(ROOT, rel);
  if (!fs.existsSync(abs)) {
    fails.push(`${rel} 不存在（守卫名单里却有它）`);
    continue;
  }
  const text = fs.readFileSync(abs, "utf8");
  for (const m of text.matchAll(STEP_CLAIM)) {
    const [, before, nStr] = m;
    if (!GATE_WORD.test(before)) continue;
    const n = Number(nStr);
    if (n === counts.quick || n === counts.full) continue;
    fails.push(
      `${rel}：手写「${before.trim().slice(-20) || "…"}${nStr} 步」= ${n}，` +
        `现算只有 快速 ${counts.quick} / 全量 ${counts.full}`,
    );
  }
}

// ---------- 判据 B：手写「取锁点」条数一律禁止 ----------
const LOCK_COUNT_BANNED = [
  "README.md",
  "docs/acceptance/1.0-release.md",
  "docs/acceptance/stability-smoke-matrix.md",
  "docs/protocol-invariants.md",
  "docs/ARCHITECTURE-MAP.html",
  "scripts/verify.mjs",
  "scripts/check-lock-scope.mjs",
];
const LOCK_RE = /(\d{2,4})\s*(?:处|个)?\s*取锁点/g;

for (const rel of LOCK_COUNT_BANNED) {
  const abs = path.join(ROOT, rel);
  if (!fs.existsSync(abs)) continue;
  const text = fs.readFileSync(abs, "utf8");
  for (const m of text.matchAll(LOCK_RE)) {
    fails.push(
      `${rel}：手写「${m[0].trim()}」——取锁点条数由 check-lock-scope.mjs 每次实算打印，` +
        `抄进文档就是第二个事实源（已漂过一次：292/295/296 三种写法并存）`,
    );
  }
}


// ---------- 判据 C：双实例 E2E 的断言数由 harness 现算 ----------
const HARNESS = "scripts/e2e-multi-instance.mjs";
/** 按「顶层 if (MODE) { … } 顶格 } 收尾」把 check( 调用归堆。
 *  只认顶格的 `}` 收块 —— 与这个文件的写法一致；缩进的 } 一律不算闭合。 */
/// harness 里的注入模式 → 活文档里必须出现的轮次名。**加一条注入就得在这里登记一行**：
/// 没登记的表现是"文档永远不会要求它 ⇒ 这一轮的断言数没人对账"，正是要拦的那种静默漏。
const MODE_LABEL = { POISON: "脏前缀轮", RESUME: "续传轮", KILL: "杀进程轮" };
function harnessAsserts() {
  const src = fs.readFileSync(path.join(ROOT, HARNESS), "utf8");
  const per = {};
  let mode = null;
  let common = 0;
  for (const line of src.split("\n")) {
    const open = line.match(/^\s*if \(([A-Z][A-Z_]*)\) \{/);
    if (open) { mode = open[1]; if (mode !== "NEGATIVE") per[mode] = per[mode] || 0; continue; }
    if (/^}/.test(line)) { mode = null; continue; }
    // 只数「check("字符串名"」这种真断言调用点：`function check(` 与 `check(s.name, …)`
    //    （失败记录器）都不是断言，混进来会把数算大。
    const n = (line.match(/\bcheck\(\s*"/g) || []).length;
    if (!n) continue;
    if (mode === "NEGATIVE") continue; // 反向模式复用同一条旅程，不新增断言数
    if (mode) per[mode] += n; else common += n;
  }
  const unknown = Object.keys(per).filter((m) => !(m in MODE_LABEL));
  if (unknown.length) {
    throw new Error(
      `harness 里有没登记的注入模式块：${unknown.join(" / ")} —— 加一条注入要在 MODE_LABEL 登记轮次名，` +
        `否则这一轮的断言数永远不会被对账`,
    );
  }
  const out = { 默认轮: common };
  for (const [m, label] of Object.entries(MODE_LABEL)) out[label] = common + (per[m] || 0);
  return out;
}

let e2e;
try {
  e2e = harnessAsserts();
} catch (e) {
  console.error(`✗ 读不到 harness 断言数：${e.message}`);
  process.exit(1);
}
console.log(
  `· 现算 E2E 断言数：${Object.entries(e2e).map(([k, v]) => `${k} ${v}`).join(" / ")}`,
);
const E2E_CLAIM = /(默认轮|脏前缀轮|故障轮|续传轮|杀进程轮)([^。\n]{0,16}?)(\d{1,3})\s*条?\s*断言/g;
const seenLabel = new Set();
for (const rel of LIVE_DOCS) {
  const abs = path.join(ROOT, rel);
  if (!fs.existsSync(abs)) continue;
  const text = fs.readFileSync(abs, "utf8");
  for (const m of text.matchAll(E2E_CLAIM)) {
    const label = m[1] === "故障轮" ? "脏前缀轮" : m[1];
    seenLabel.add(label);
    const n = Number(m[3]);
    if (n === e2e[label]) continue;
    fails.push(
      `${rel}：手写「${label} ${m[3]} 条断言」= ${n}，` +
        `现算 ${HARNESS} 是 ${e2e[label]} —— 加/删断言后要改的是 harness 的注释口径，不是文档里的数字`,
    );
  }
}
for (const label of Object.keys(e2e)) {
  if (!seenLabel.has(label)) {
    fails.push(`活文档里再没有「${label} N 断言」这种声明了 —— 删标签等于绕过判据 C，不许`);
  }
}

if (fails.length) {
  console.error(`\n✗ 文档硬数字漂移 ${fails.length} 处：`);
  for (const f of fails) console.error(`  · ${f}`);
  console.error(
    `\n  改法：**把数字从文档里删掉**，让唯一事实源自己打印它。` +
      `不要「改成对的」——那只是把下一次漂移排上队。`,
  );
  process.exit(1);
}
console.log(
  `✓ 文档硬数字对账通过（${LIVE_DOCS.length} 份活文档；取锁点条数无手写；E2E 断言数现算对账）`,
);
