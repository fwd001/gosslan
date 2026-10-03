#!/usr/bin/env node
/**
 * shell 变量紧跟中文字符的守卫。
 *
 * ## 为什么要有这个脚本
 * bash 把 `$name` 之后**紧邻**的字节当成变量名的一部分去解析。中文/全角字符的
 * 首字节落在 `[A-Za-z0-9_]` 之外的某些区间上，但 bash 在这个 locale 下把它们
 * **算进了解析**，于是变量名被撑大、值整段丢失：
 *
 * ```console
 * $ bash -c 'SRC=/tmp/app; echo "A: $SRC（后续）"'
 * A: ��后续）            ← SRC 的值 /tmp/app 整个没了
 * $ bash -c 'B=x; echo "C=$B）"'
 * C=��                   ← 同上
 * ```
 *
 * 代价不是"报错"（不崩），而是**错误消息里最该看清的那个值消失了** ——
 * 而这些行几乎都在错误分支上（"[错误] 未找到 $BIN，请先…"、"缺少 Rust 目标 $TARGET"），
 * 恰好是排障时唯一要看的那一行。今天 2026-10-03 在本仓**一天内两处**踩到：
 * `scripts/ci-run.sh` 的注释记着它（"实测 `$status（` 输出成乱码"），
 * 而我在 `publish-release-assets.sh` 写新守卫时又犯了一次（`--label=$label（` ⇒
 * 直接 `unbound variable`，因为那个变量当时还没赋值）。
 *
 * ## 判据（一条）
 * 扫 `scripts/**` 与 `.github/workflows/**` 里的 `.sh` / `.yml` / `.yaml`，
 * 凡是 `$` + 变量名 + **紧邻** CJK / 全角标点的写法 ⇒ 报出行号与片段。
 *
 * **必须排除的三类误报**（不然这条守卫会立刻变成噪音）：
 * ① **注释里引用问题案例**（`ci-run.sh:23` 写着 `` `$status（` `` 并解释它为什么坏）
 *    —— 它是**文档**，不是代码；
 * ② 已经写成 `${name}` 的（`$B_CONN_C` 后面跟的是 `）` 而 `$` 后面紧邻的字符是字母，
 *    真的危险形状是 `$name` 后面直接跟全角）；
 * ③ PowerShell 段（`.yml` 里的 `run: |` 块是 pwsh，`$dir（` 在那里由 pwsh 自己定规则）。
 *
 * ⚠️ ③ 的处理是**只跳过 pwsh 代码块**，不是跳过整个 `.yml` —— `build-windows-webview2.yml`
 * 里的 pwsh 那处实测是安全的（pwsh 不做这种解析），但白名单只写具体位置，
 * 新增 pwsh 代码块会重新被扫到并按"bash 规则"判 ⇒ 那时需要重新评估。
 *
 * 退出码：0 = 通过；1 = 有裸写隐患。
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** 被扫的目录与后缀。刻意**不含** `.mjs`/`.ts` —— JS 模板串里 `${}` 是常态，
 *  而裸 `$name` 在 JS 里不是这个坑（那是 zsh 的坑，见 AI_RULES 提交信息规范）。 */
const SCAN = [
  { dir: "scripts", exts: [".sh", ".yml", ".yaml"] },
  { dir: ".github/workflows", exts: [".yml", ".yaml"] },
];

/** `$` + 变量名 + 紧邻 CJK/全角。变量名本身是 `[A-Za-z_][A-Za-z0-9_]*`，
 *  所以能匹配上就说明 bash 会把后面的中文一起吞进变量名。 */
const RISKY = /\$[A-Za-z_][A-Za-z0-9_]*[　-〿一-鿿＀-￯]/;

/** 显式豁免：`路径:行号` → 为什么它是安全的/是文档。
 *  **只写具体位置**，不写通配 —— 通配会让新代码悄悄落在豁免范围内。 */
const EXEMPT = new Map([
  // 注释里**引用**这个坑当反面案例（第一行就是文档：解释为什么要写 ${var}）
  ["scripts/ci-run.sh:23", "注释：引用 `$status（` 作为「为什么要写 ${var}」的反面案例"],
]);

/** `.yml` 里 pwsh 代码块的行号（1-based）—— 那里的 `$var` 由 pwsh 解析，不适用本规则。 */
const PWSH_BLOCKS = new Map([
  [".github/workflows/build-windows-webview2.yml", null], // 运行时按标记判定
]);

/** `.yml` 里 pwsh 代码块的标记 cmdlet —— 块内出现任一个即认为整块是 PowerShell。
 *  ⚠️ 列表要覆盖常见 cmdlet：少一个（比如一开始漏了 `Test-Path` / `Write-Error`）
 *  就会让那一块整体被判成 bash 而误报。**判不准不如判宽**：本规则对 pwsh 本来就不适用。 */
const PWSH_MARKERS =
  /\$env:|Write-Host|Write-Error|Write-Warning|Write-Output|Test-Path|Get-ChildItem|Get-Content|Set-Location|\$LASTEXITCODE|\$PSScriptRoot|New-Item|Remove-Item|Copy-Item|Move-Item|Start-Process|Invoke-WebRequest/;

/** 找出文件里所有 PowerShell 代码块的行号集合（0-based）。
 *
 *  ⚠️ 必须**预扫描整块**而不是"从当前行往上找最近的 `run: |`"：
 *  逐行往上找的写法在 `.yml` 里会漏掉"块内没有 marker 的那几行"——
 *  实测 `build-windows-webview2.yml:96` 就因此被判成 bash（第一次跑没排除掉）。
 *  正确做法：先确定哪些区间是 pwsh 块，再对块内每一行打标。 */
function pwshLineSet(lines) {
  const set = new Set();
  for (let i = 0; i < lines.length; i += 1) {
    const m = lines[i].match(/^(\s*)run:\s*\|\s*$/);
    if (!m) continue;
    const base = m[1].length;
    // 先收集块内全部行，再判这个块是不是 pwsh（有 marker 才是）
    const body = [];
    for (let j = i + 1; j < lines.length; j += 1) {
      const l = lines[j];
      if (l.trim() && l.match(/^\s*/)[0].length <= base) break; // 出了块
      body.push(j);
    }
    if (body.some((j) => PWSH_MARKERS.test(lines[j]))) body.forEach((j) => set.add(j));
  }
  return set;
}

function walk(dir, exts, acc = []) {
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return acc;
  }
  for (const e of entries) {
    if (e.name === "node_modules" || e.name === "target" || e.name === ".git") continue;
    const abs = path.join(dir, e.name);
    if (e.isDirectory()) walk(abs, exts, acc);
    else if (exts.some((x) => e.name.endsWith(x))) acc.push(abs);
  }
  return acc;
}

const files = SCAN.flatMap(({ dir, exts }) => walk(path.join(ROOT, dir), exts));
const findings = [];
let exempted = 0;
let pwshSkipped = 0;

for (const abs of files) {
  const rel = path.relative(ROOT, abs);
  const lines = fs.readFileSync(abs, "utf8").split("\n");
  const pwshLines = pwshLineSet(lines);
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const m = line.match(RISKY);
    if (!m) continue;
    // ① 注释（整行或行内）里引用反面案例 ⇒ 豁免，但要显式登记过
    const key = `${rel}:${i + 1}`;
    if (EXEMPT.has(key)) {
      exempted += 1;
      continue;
    }
    // ③ pwsh 代码块
    if (pwshLines.has(i)) {
      pwshSkipped += 1;
      continue;
    }
    findings.push({ where: key, snippet: line.trim().slice(0, 90), hit: m[0] });
  }
}

// 覆盖面自证：一条都没扫到 ⇒ 解析前提变了（目录挪了 / 正则失配），直接红。
if (files.length < 5) {
  console.error(
    `✗ 只扫到 ${files.length} 个文件（今天至少 5 个）⇒ 路径或后缀写错了，先修守卫再谈别的。\n` +
      `  扫到的：${files.map((f) => path.relative(ROOT, f)).join(", ")}`,
  );
  process.exit(1);
}

if (findings.length) {
  console.error("✗ shell 变量紧跟中文字符：bash 会把中文一起吞进变量名，导致该变量的值**整段丢失**\n");
  for (const f of findings) {
    console.error(`  - ${f.where}  ${JSON.stringify(f.hit)}`);
    console.error(`      ${f.snippet}`);
  }
  console.error(
    `\n修法：一律写 \${var}（例：\${SRC}（…））。这些行几乎都在**错误分支**上，\n` +
      `    而"错误消息里最该看清的那个值消失了"恰好是排障时最难受的一种坏。\n` +
      `  若某处确实是注释里引用反面案例，加进本脚本的 EXEMPT 并写明理由（不接受通配）。`,
  );
  process.exit(1);
}

console.log(
  `✓ shell 变量护栏：${files.length} 个文件全部通过（裸写 0 处` +
    (exempted ? `；显式豁免 ${exempted} 处（注释引用反面案例）` : "") +
    (pwshSkipped ? `；pwsh 代码块 ${pwshSkipped} 处不适用本规则` : "") +
    `）`,
);
