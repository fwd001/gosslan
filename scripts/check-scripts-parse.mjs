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

const refs = new Set();
const re = /scripts\/[\w.-]+\.(?:mjs|py)/g;
for (const f of ["scripts/verify.mjs", "package.json"]) {
  const p = path.join(ROOT, f);
  if (!fs.existsSync(p)) continue;
  for (const m of fs.readFileSync(p, "utf8").matchAll(re)) refs.add(m[0]);
}

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
console.log(`✓ ${refs.size} 个被门禁调用的脚本全部解析通过`);
