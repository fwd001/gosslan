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

if (fails.length) {
  console.error(`\n✗ 文档硬数字漂移 ${fails.length} 处：`);
  for (const f of fails) console.error(`  · ${f}`);
  console.error(
    `\n  改法：**把数字从文档里删掉**，让唯一事实源自己打印它。` +
      `不要「改成对的」——那只是把下一次漂移排上队。`,
  );
  process.exit(1);
}
console.log(`✓ 文档硬数字对账通过（${LIVE_DOCS.length} 份活文档；取锁点条数无手写）`);
