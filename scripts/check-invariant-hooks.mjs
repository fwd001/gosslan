#!/usr/bin/env node
/**
 * 不变量钩子守卫（invariant hook guard）—— 挡住「契约文档里写着'必须'，下面没有任何东西在跑」。
 *
 * ## 为什么要有这个脚本
 *
 * `docs/protocol-invariants.md` 是总指令§三那份"物理定律"的唯一登记处，也是 AI 的必读清单
 * （`AI_ENGINEERING_INDEX.md` 直接指向它）。但**prose 的「验证」段不构成证明**：
 * 它写的是"应该测什么"的伪码，不是"今天哪条测试在跑"。
 * 结果就是"未来 AI 把某条不变量改坏 ⇒ CI 不响"的入口一直开着 —— 因为没有人能说出
 * 这条不变量坏掉时**哪条测试会变红**。
 *
 * 本脚本要求的只有一行机器可解析的绑定：
 *
 *     - 钩子：`db::tests::message_dedup_by_unique_msg_id` `scripts/check-lock-scope.mjs`
 *
 * 每个名字都必须**当场解析成真实存在的对象**，解析不出来就红。
 *
 * ## 为什么"能解析"就够了（以及哪里不够）
 *
 * 本脚本判的是**绑定是否失效**，不是**行为是否正确**。行为由各钩子自己判。
 * 两者合起来才挡住三种静默腐烂：
 *   ① 测试被删了/改名了，文档还在指着旧名字 ⇒ 本脚本红；
 *   ② 护栏脚本写完没接进任何门禁（"跑不到的绿"）⇒ `scripts/` 那条要求它出现在
 *      `scripts/verify.mjs` 或 `package.json` 里，否则红；
 *   ③ 基线里有名字但本平台没跑 ⇒ Rust 钩子只认**本平台基线**（macOS 认
 *      `test-baseline.macos.txt`），而"基线本身是否完整"由 `check-test-manifest.mjs` 判。
 *
 * ## 诚实的空白：NONE
 *
 * 没有自动化钩子的不变量**必须**写成 `- 钩子：NONE —— 理由…`。
 * 空白不写 = 红（防止"忘了"被读成"没有"）；写 NONE = 绿但**每轮都打印在末尾**
 * （防止它变成新的 prose 债）。总指令§十的规矩：不能把"没有测试"伪装成 PASS。
 *
 * ## 用法
 *
 *     node scripts/check-invariant-hooks.mjs          # 全部检查
 *     node scripts/check-invariant-hooks.mjs --quiet  # 只在有问题时输出（门禁用）
 *
 * 退出码：0 = 每条不变量的钩子都能解析；1 = 有解析不出来的钩子或有未登记缺口。
 */

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DOC = path.join(ROOT, "docs", "protocol-invariants.md");

const quiet = process.argv.slice(2).includes("--quiet");
const say = (...a) => {
  if (!quiet) console.log(...a);
};

/** Rust 钩子只认**本平台**基线（与 check-test-manifest.mjs 同一套映射）。 */
const RUST_OS = { darwin: "macos", win32: "windows", linux: "linux" }[process.platform];
const BASELINE = path.join(ROOT, "src-tauri", `test-baseline.${RUST_OS ?? "macos"}.txt`);

const read = (p) => (existsSync(p) ? readFileSync(p, "utf8") : null);

const baselineNames = new Set((read(BASELINE) ?? "").split(/\r?\n/).map((l) => l.trim()).filter(Boolean));
const guardNames = (() => {
  const src = read(path.join(ROOT, "scripts", "verify-guards.py")) ?? "";
  return [...src.matchAll(/name\s*=\s*"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]);
})();
const smokeDoc = read(path.join(ROOT, "docs", "acceptance", "stability-smoke-matrix.md")) ?? "";
const e2eSrc = read(path.join(ROOT, "scripts", "e2e-multi-instance.mjs")) ?? "";
const verifySrc = read(path.join(ROOT, "scripts", "verify.mjs")) ?? "";
const pkgJson = read(path.join(ROOT, "package.json")) ?? "";

/**
 * 解析一个钩子名。返回 { ok, kind, why }。
 * 支持的形态（刻意只这几种 —— 越少的形态越不容易出现"看着对其实没判"）：
 *   a::b::c                    Rust 用例，必须在本平台基线里
 *   src/…/x.test.ts[#片段]      前端测试文件（可选钉测试标题片段）
 *   scripts/xxx.mjs            护栏脚本：文件在 + 已接进门禁
 *   guards:名片段              verify-guards.py 里的 Case name
 *   e2e:判据片段               多实例 harness 里的真实判据
 *   smoke:行名片段             平台 Smoke 清单里的具名条目（=无自动化但有具名人工回归）
 *   NONE —— 理由               显式登记"这条今天没有自动化钩子"
 */
function resolveHook(tok) {
  if (tok.startsWith("guards:")) {
    const frag = tok.slice(7);
    return { ok: guardNames.some((n) => n.includes(frag)), kind: "guard-case",
      why: `verify-guards.py 里没有 name 含「${frag}」的用例` };
  }
  if (tok.startsWith("e2e:")) {
    const frag = tok.slice(4);
    return { ok: e2eSrc.includes(frag), kind: "e2e",
      why: `scripts/e2e-multi-instance.mjs 里找不到「${frag}」` };
  }
  if (tok.startsWith("smoke:")) {
    const frag = tok.slice(6);
    return { ok: smokeDoc.includes(frag), kind: "smoke",
      why: `平台 Smoke 清单里找不到「${frag}」这条具名条目` };
  }
  if (tok.startsWith("scripts/")) {
    const p = path.join(ROOT, tok);
    if (!existsSync(p)) return { ok: false, kind: "script", why: `${tok} 不存在` };
    const wired = verifySrc.includes(path.basename(tok)) || pkgJson.includes(path.basename(tok));
    return { ok: wired, kind: "script",
      why: `${tok} 存在但没接进任何门禁（verify.mjs / package.json 里搜不到这个名字）` };
  }
  if (/\.test\.ts$|\.spec\.ts$/.test(tok) || /\.test\.ts#/.test(tok) || /\.spec\.ts#/.test(tok)) {
    const [rel, frag] = tok.split("#");
    const p = path.join(ROOT, rel);
    if (!existsSync(p)) return { ok: false, kind: "frontend", why: `${rel} 不存在` };
    if (!frag) return { ok: true, kind: "frontend", why: "" };
    return { ok: read(p).includes(frag), kind: "frontend",
      why: `${rel} 里没有含「${frag}」的测试标题` };
  }
  if (tok.includes("::")) {
    return { ok: baselineNames.has(tok), kind: "rust",
      why: `${tok} 不在本平台 Rust 基线（${path.basename(BASELINE)}）里` };
  }
  return { ok: false, kind: "unknown", why: `不认识的钩子形态「${tok}」（只支持 ::/test.ts/scripts//guards:/e2e:/smoke:/NONE）` };
}

const src = read(DOC);
if (!src) {
  console.error(`✗ 读不到 ${path.relative(ROOT, DOC)}`);
  process.exit(1);
}

const lines = src.split(/\r?\n/);
const sections = [];
let cur = null;
for (let i = 0; i < lines.length; i += 1) {
  const m = lines[i].match(/^### (INV-P\d+)/);
  if (m) {
    cur = { id: m[1], title: lines[i].slice(4), start: i, hooks: [] };
    sections.push(cur);
    continue;
  }
  if (!cur) continue;
  if (/^## /.test(lines[i])) { cur = null; continue; }
  const h = lines[i].match(/^-\s*钩子：(.*)$/);
  if (h) cur.hooks.push({ line: i + 1, text: h[1].trim() });
}

const problems = [];
const counts = {};
let noneCount = 0;

for (const s of sections) {
  if (s.hooks.length === 0) {
    problems.push(`${s.id}（${s.title}）：整节没有「- 钩子：」这一行 —— `
      + "要么绑一个真实存在的钩子，要么显式写 `NONE —— 理由`（不写=判不出来是谁的锅）");
    continue;
  }
  for (const hk of s.hooks) {
    if (/^NONE\b/.test(hk.text)) {
      const reason = hk.text.replace(/^NONE\b\s*[—-]*\s*/, "").trim();
      if (reason.length < 8) {
        problems.push(`${s.id}:${hk.line} NONE 没写理由（"没有测试"必须说清为什么没有、靠什么兜）`);
        continue;
      }
      noneCount += 1;
      counts.NONE = (counts.NONE ?? 0) + 1;
      continue;
    }
    const toks = [...hk.text.matchAll(/`([^`]+)`/g)].map((m) => m[1]);
    if (toks.length === 0) {
      problems.push(`${s.id}:${hk.line} 钩子行里没有反引号包裹的名字`);
      continue;
    }
    for (const t of toks) {
      const r = resolveHook(t);
      counts[r.kind] = (counts[r.kind] ?? 0) + 1;
      if (!r.ok) problems.push(`${s.id}:${hk.line} 钩子解析失败：${r.why}`);
    }
  }
}

if (problems.length) {
  console.error(`✗ 不变量钩子：${problems.length} 处不成立`);
  for (const p of problems) console.error(`  · ${p}`);
  console.error("\n  修法：钩子名必须**当场能解析**。测试改名/删除 ⇒ 改这一行；");
  console.error("        确实没有自动化钩子 ⇒ 写 `- 钩子：NONE —— 理由`，并在");
  console.error("        docs/stability-roadmap.md 里留下对应的待办。");
  process.exit(1);
}

const bound = sections.length - noneCount;
say(`✓ 不变量钩子：${sections.length} 条全部绑定（具名钩子 ${bound} 条、登记为无自动化 ${noneCount} 条）`
  + ` · 形态 ${Object.entries(counts).map(([k, v]) => `${k}:${v}`).join(" ")}`);
if (noneCount > 0) {
  const ids = sections.filter((s) => s.hooks.some((h) => /^NONE\b/.test(h.text))).map((s) => s.id);
  say(`  ⚠️ 无自动化钩子（每轮都会打印，别让它变成新的 prose 债）：${ids.join(" / ")}`);
}
