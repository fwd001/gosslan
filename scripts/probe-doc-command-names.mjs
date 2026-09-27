#!/usr/bin/env node
// 文档里引用的 npm 脚本名，有多少个在 package.json 里真的不存在？
//
// ★ **这一步有意不进任何门禁层**（判一次要不要接线的结论写在这里，不是忘了）：
//   命令名断掉是**响亮**的失败 —— 人或 AI 照着文档敲 `npm run x` 会当场被 npm 报错，
//   而本仓反复被咬的是**静默**那一类（`file:line` 漂了、证据名字改了，文档照样好看）。
//   给一个"必然立刻自曝"的形状加一步门禁 = §十四 反对的那种充气。
//   所以这条只当**现算工具**留着：改过脚本表之后手动跑一次，读数进文档。
//
// 复跑：`node scripts/probe-doc-command-names.mjs`
import { readFileSync, readdirSync, existsSync } from "node:fs";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..");
const pkg = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8"));
const scripts = new Set(Object.keys(pkg.scripts ?? {}));

// 扫描范围 = **约束文档**（今天读它的人会照着做的那几份），不含历史台账：
//   CHANGELOG.md / docs/notes/** / HANDOFF.md / 第三方 vendor 目录 —— 那些写的是"当时发生过什么"，
//   命令后来被改名并不是它们的错，把它们算进断链只会制造"必须去放宽守卫"的压力。
const SKIP_DIR = new Set(["node_modules", "test-results", "target", "vendor", "dist", ".git"]);
const SKIP_RE = /(^|\/)(CHANGELOG|README\.test|HANDOFF)\.md$|(^|\/)docs\/notes\//;
const FILES = [];
(function walk(dir) {
  for (const e of readdirSync(path.join(ROOT, dir), { withFileTypes: true })) {
    if (SKIP_DIR.has(e.name)) continue;
    const rel = path.posix.join(dir, e.name);
    if (e.isDirectory()) { walk(rel); continue; }
    if (!/\.(md|html)$/.test(e.name) || SKIP_RE.test(rel)) continue;
    FILES.push(rel);
  }
})(".");

const refs = new Map(); // script name -> [where]
for (const rel of FILES) {
  const text = readFileSync(path.join(ROOT, rel), "utf8");
  text.split("\n").forEach((line, idx) => {
    // 认 `npm run x`、`npm --silent run x`、`npm -s run x`；名字**本身带冒号**（`version:changelog`）
    // ⇒ 整段当一个名字，不许按冒号切（第一版就是切了冒号，把 50 个名字里的 18 个报成"不存在"）。
    for (const m of line.matchAll(/npm\s+(?:-\S+\s+)*run\s+([a-zA-Z0-9][a-zA-Z0-9:_-]*)/g)) {
      const name = m[1].replace(/[.,;:)\]}`'"]+$/, "");
      if (!refs.has(name)) refs.set(name, []);
      refs.get(name).push(`${rel}:${idx + 1}`);
    }
  });
}

const broken = [...refs].filter(([name]) => !scripts.has(name));
// 反空转闸：认不出东西 = 解析器坏了，不许把"什么都没判"当成通过。
if (!FILES.length) { console.error("✗ 扫描范围为空 ⇒ 这条现算没有判任何东西"); process.exit(1); }
if (refs.size < 20) {
  console.error(`✗ 只认出 ${refs.size} 个脚本引用（下限 20）⇒ 正则或范围坏了，别把"没认出来"读成"没有断链"`);
  process.exit(1);
}
console.log(`· 扫描 ${FILES.length} 份约束文档；认出 ${refs.size} 个不同的 \`npm run\` 名字（package.json 现有 ${scripts.size} 个脚本）`);
if (!broken.length) {
  console.log(`✓ 命令名断链 0 条 —— 文档里让读者跑的脚本，${refs.size} 个全都真的存在`);
  process.exit(0);
}
console.error(`✗ ${broken.length} 条断链（文档让读者跑一个不存在的脚本）：`);
for (const [name, where] of broken) {
  const near = [...scripts].filter((s) => s.startsWith(name.split(":")[0])).slice(0, 3);
  console.error(`  · npm run ${name} ← ${where.slice(0, 3).join(", ")}${where.length > 3 ? ` …共 ${where.length} 处` : ""}` +
    (near.length ? `　（同名前缀的现成脚本：${near.join(" ")}）` : ""));
}
process.exit(1);
