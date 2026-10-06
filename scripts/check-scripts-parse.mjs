#!/usr/bin/env node
// 门禁自己的判据脚本，坏了没有便宜层看得见。
//
// 起因（2026-09-27，#126 的提交里）：往 harness 的一条提示语后面接了第二行，
// 但那一行落在已经闭合的模板字符串**外面** ⇒ `scripts/e2e-multi-instance.mjs` 整份语法错。
// 没有任何一层能便宜地发现它：能发现它的只有 E2E 层本身，而那一层一次 500s+，
// 改一句提示语不会有人去跑它。结果就是"这份 harness 从提交那一刻起根本跑不了"。
//
// 所以这一层只做一件事：把**门禁真的会调用**的那些脚本各解析一遍（node --check / py_compile），
// 秒级。名单不手写 —— 手写名单就是下一处漂移。
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(process.argv.find((a) => a.startsWith("--root="))?.slice(7) ?? ".");

let refs = new Set();
const re = /scripts\/[\w.-]+\.(?:mjs|py)/g;
for (const f of ["scripts/verify.mjs", "package.json"]) {
  const p = path.join(ROOT, f);
  if (!fs.existsSync(p)) continue;
  for (const m of fs.readFileSync(p, "utf8").matchAll(re)) refs.add(m[0]);
}

/**
 * ★ 闭包再走一层：**被这些脚本 import 的共用件**也要查。
 * 不这么做就有一个静默洞 —— `node --check` 只解析传入的那一份文件，**不解析它的 import**，
 * 所以共用件里的语法错在"被门禁调用的那个脚本"身上完全看不出来（实测：把共用件改坏，
 * 调用方的 `node --check` 仍退 0，只有真跑起来那一刻才炸）。2026-09-28 把 AX 解析抽成
 * `ax-tree.mjs` 之后，这个洞第一次有了真实的被守对象。
 */
const importRe = /(?:from|import\()\s*["'](\.\.?\/?[^"']+\.mjs)["']/g;
/**
 * ★ Python 侧的同一条：2026-10-07 把 `verify-guards.py` 的 202 条 Case 切进 `scripts/guard_cases/` 之后，
 * 只查主文件等于什么都没查 —— 那个包是 `importlib.import_module(f".{m}")` **动态**加载的，
 * 静态正则看不见模块名。所以规则是：`from <pkg> import` 指到一个包目录 ⇒ 把目录里的 `.py` 全收
 * （Python 自己也是这么加载的），`from .x import` 这种相对导入同理。
 */
const pyImportRe = /^from\s+([.\w]+)\s+import/gm;
function pyClosure(rel) {
  const abs = path.join(ROOT, rel);
  if (!rel.endsWith(".py") || !fs.existsSync(abs)) return [];
  const dir = path.dirname(abs);
  const out = [];
  for (const m of fs.readFileSync(abs, "utf8").matchAll(pyImportRe)) {
    const dotted = m[1];
    const rel2 = dotted.startsWith(".")
      ? path.relative(ROOT, path.resolve(dir, ...dotted.slice(1).split("/").filter(Boolean) || ".")).split(path.sep).join("/")
      : `scripts/${dotted.split(".").join("/")}`;
    if (!rel2) continue;
    const asFile = `${rel2}.py`;
    if (fs.existsSync(path.join(ROOT, asFile))) out.push(asFile);
    const pkgInit = path.join(ROOT, rel2, "__init__.py");
    if (fs.existsSync(pkgInit)) {
      for (const f of fs.readdirSync(path.join(ROOT, rel2))) {
        if (f.endsWith(".py")) out.push(`${rel2}/${f}`.split(path.sep).join("/"));
      }
    }
  }
  return out;
}
let frontier = [...refs];
const all = new Set(refs);
while (frontier.length) {
  const next = [];
  for (const rel of frontier) {
    const abs = path.join(ROOT, rel);
    if (!fs.existsSync(abs)) continue;
    const kids = rel.endsWith(".py")
      ? pyClosure(rel)
      : [...fs.readFileSync(abs, "utf8").matchAll(importRe)].map((m) =>
          path.relative(ROOT, path.resolve(path.dirname(abs), m[1])).split(path.sep).join("/")
        );
    for (const child of kids) {
      if (!all.has(child)) { all.add(child); next.push(child); }
    }
  }
  frontier = next;
}
const importedCount = all.size - refs.size;
refs = all;

if (refs.size === 0) {
  console.error(`✗ 在 ${ROOT} 里一条脚本引用都没找到 —— 名单为空 ⇒ 这一层什么都没判，按红处理`);
  process.exit(1);
}

const bad = [];
for (const rel of [...refs].sort()) {
  const abs = path.join(ROOT, rel);
  if (!fs.existsSync(abs)) {
    bad.push(`${rel}：被引用但文件不存在`);
    continue;
  }
  // 只用 `compile()` 在内存里判语法：`python3 -m py_compile` 会把 .pyc 写进
  // `scripts/__pycache__/`，那等于每跑一次门禁就往仓库里撒一个未跟踪目录。
  const [cmd, args] = rel.endsWith(".py")
    ? ["python3", ["-c", "import sys; compile(open(sys.argv[1], encoding='utf-8').read(), sys.argv[1], 'exec')", abs]]
    : [process.execPath, ["--check", abs]];
  const r = spawnSync(cmd, args, { encoding: "utf8" });
  if (r.status !== 0) {
    // 取**末尾**几行：node 与 python 都把"哪一行、为什么"放在最后，
    // 开头几行是内部调用栈（`-c` 那条尤其明显：开头是 Traceback 头，出错原因在最后）。
    const detail = `${r.stderr ?? ""}\n${r.stdout ?? ""}`
      .split("\n").filter((l) => l.trim() !== "").slice(-4).join("\n    ");
    bad.push(`${rel}：解析失败\n    ${detail}`);
  }
}

if (bad.length) {
  console.error(`✗ ${bad.length} 个判据脚本没通过解析（共查 ${refs.size} 个）：`);
  for (const b of bad) console.error(`  · ${b}`);
  console.error(`  这些脚本是门禁的判据本身：解析不过 ⇒ 它们一条都没在判，不是"暂时没跑到"`);
  process.exit(1);
}
console.log(
  `✓ ${refs.size} 个脚本全部解析通过（门禁/脚本表直接调用 ${refs.size - importedCount} 个 + 它们 import 的共用件 ${importedCount} 个）`,
);
