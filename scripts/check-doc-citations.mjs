#!/usr/bin/env node
/**
 * 文档 `file:line` 引用守卫。
 *
 * ## 为什么要有这个脚本
 * 今天一次只读审计（2026-10-03）在**纯文档**里查出 9 处断裂，全部通过了现有全部门禁：
 *
 * ① `docs/migration-ledger.md` §5「维护规则」是写给接手者的**操作指令**，其中 5 处让读者去
 *    更新 `docs/domains.yml` —— 那个文件**从不存在**（`.yml`→`.mjs` 是有意改名，工具链没有
 *    YAML 解析器），而门禁 `check-domain-map.mjs` 读的是 `.data.mjs` ⇒ **门禁照亮绿灯**。
 * ② 同一份台账里 `transport/mod.rs:122` —— 那个文件**只有 118 行**，引用指着不存在的行。
 * ③ `src-tauri/src/transport/tcp.rs` 模块头两行「接线状态」表的 `file:line` 全部腐烂
 *    （`network/transport.rs:1231,1232,…` 逐个核对**全指向无关代码**）—— 注释说"已接线"是对的、
 *    **行号是烂的**，这比"注释撒谎"更隐蔽：顺行号核对的人会看到无关代码，从而误判"注释在骗人"
 *    而把好的接线拆掉。这是本仓「待接线声明腐烂」族的第三次。
 *
 * 共同点：**这些引用都有唯一事实源（那个文件），只是行号被人手抄了一遍。**
 * 与 `check-doc-numbers.mjs` 同一立场（能被现算的东西不许留第二份手抄），但管的是另一面：
 * 它管"数字"，本脚本管"**指向**"。
 *
 * ## 三条判据（刻意保守：宁可漏报，也绝不误报）
 * **A. 文件必须存在。** 引用的路径按 `SOURCE_ROOTS` 里的每个根 + 文档写出的相对路径解析；
 *    解析不到就报。裸文件名（`state.rs:873`，省略了目录）按"全仓唯一同名文件"解析。
 * **B. 行号必须落在文件范围内。** 这是本脚本的核心，也是唯一能低误报地判"引用腐烂"的那条。
 *    ⚠️ **刻意不判"行内容是否匹配文档说的那个符号"** —— 那会大量误报：文档常写
 *    "见 `foo.rs:12` 的 `bar` 函数"，而那一行可能是 `impl` 块、可能是多行签名的一部分、
 *    可能中间插了行。要判符号得先解析 Rust/TS 语法，那是另一个量级的工具（且解析错了
 *    门禁会静默失效 —— 比没有门禁更危险）。**先把"范围"这条低误报的守住，符号级校验留作
 *    后续可选项。**
 * **C. 扫到 0 处引用时判 INCONCLUSIVE 而非绿。** 防止正则失配 / 目录搬迁让门禁悄悄空转。
 *
 * ## 故意不管的（不是漏了，是判据本身做不到）
 * · **带日期的历史快照**：`docs/ARCHITECTURE-REVIEW-*.md` / `docs/final-architecture-review.md` /
 *   `CHANGELOG.md` —— 它们的职责就是把"当时读到的是什么"原样记下来（含当时的行号），
 *   拿今天的事实源去判它们**必然**误报。`docs/stability-roadmap.md` 同理（漂移事故台账）。
 * · **外部 crate / 本仓之外的文件**（如 `tauri-2.11.5/src/app.rs:386`）：`EXTERNAL` 前缀表显式列出。
 * · **裸文件名有多个候选**（如 `mod.rs`、`tests.rs`）：判不准就跳过并计数，见 `ambiguous`。
 *   **静默漏报比误报危险**，但"猜一个"比漏报更危险 —— 所以只跳过、并在结尾报出数量。
 *
 * 退出码：0 = 通过；1 = 有断裂引用。
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/**
 * 被管的活文档：句子里的 `file:line` 是在**声明当前事实**的那几份。
 *
 * 沿用 `check-doc-numbers.mjs` 的 `LIVE_DOCS` 取舍（那份的注释解释了为什么不管
 * stability-roadmap 与 CHANGELOG），只多管两份：
 * · `docs/migration-ledger.md` —— §5 是给接手者的**操作指令**，引用腐烂会直接让人做错事；
 * · `AI_RULES.md` / `AI_PROJECT_HANDOFF.md` —— 每次开工都会被读，且它们是"约束"不是"记录"。
 */
const LIVE_DOCS = [
  "README.md",
  "AI_RULES.md",
  "AI_PROJECT_HANDOFF.md",
  "docs/migration-ledger.md",
  "docs/protocol-invariants.md",
];

/**
 * 路径解析根：文档里的 `network/transport.rs` 按 `src-tauri/src/` 下的相对路径理解。
 *
 * 依据（本项目的事实，不是约定）：活文档里 23 处引用**没有一处**带 `src-tauri/src/` 前缀，
 * 而它们全部指向 Rust 源文件 ⇒ 约定就是"相对 `src-tauri/src/`"。
 * `src/` 放前端（`.ts`/`.vue`），`scripts/` 放门禁脚本 —— 三根都试，谁先命中算谁。
 */
const SOURCE_ROOTS = ["src-tauri/src", "src", "scripts", "docs", "."];

/** 外部 / 本仓之外：显式列出，不靠"解析不到就当外部"（那会让判据 A 形同虚设）。 */
const EXTERNAL = [
  /^tauri-\d[\w.-]*\//, // 外部 crate 的源码引用
  /^\.\.\//, // 明确的父目录引用
];

// 形如 `path/to/file.rs:12`、`:12-34`、`:15,19,24`（反引号或裸文本都吃）
const CITE = /`?([A-Za-z0-9_][\w./-]*\.(?:rs|ts|vue|mjs|js|py|toml|json|yml|yaml|html|css|md))((?::\d+)+(?:[-,]\d+)*)`?/g;

const fails = [];
const skippedExternal = [];
let ambiguous = 0;
let total = 0;

/** 全仓文件名索引：裸名（`state.rs:873`）靠它解析。 */
function buildIndex() {
  const byBase = new Map();
  const skip = new Set(["node_modules", "target", "dist", ".git", ".workbuddy", "test-results"]);
  const walk = (dir) => {
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      if (skip.has(e.name)) continue;
      const abs = path.join(dir, e.name);
      if (e.isDirectory()) walk(abs);
      else {
        if (!byBase.has(e.name)) byBase.set(e.name, []);
        byBase.get(e.name).push(path.relative(ROOT, abs));
      }
    }
  };
  walk(ROOT);
  return byBase;
}

const INDEX = buildIndex();

/** 解析引用里的路径 → 仓库内绝对路径；解析不到返回 null。 */
function resolveRef(ref) {
  if (path.isAbsolute(ref)) return fs.existsSync(ref) ? ref : null;
  for (const root of SOURCE_ROOTS) {
    const p = path.join(ROOT, root, ref);
    if (fs.existsSync(p) && fs.statSync(p).isFile()) return p;
  }
  // 裸名：全仓唯一同名才认
  const hits = INDEX.get(path.basename(ref)) ?? [];
  if (hits.length === 1) return path.join(ROOT, hits[0]);
  if (hits.length > 1) ambiguous++;
  return null;
}

/** `:15,19,24` / `:12-34` → [12, 15, 19, 24]（区间展开，端点都取） */
function parseLineSpec(spec) {
  const out = [];
  for (const part of spec.split(":").filter(Boolean)) {
    for (const seg of part.split(",")) {
      const m = seg.match(/^(\d+)(?:-(\d+))?$/);
      if (!m) continue;
      const a = Number(m[1]);
      const b = m[2] ? Number(m[2]) : a;
      // 区间取端点即可（判"是否在范围内"不需要枚举中间每一行）
      out.push(a);
      if (b !== a) out.push(b);
    }
  }
  return out;
}

for (const doc of LIVE_DOCS) {
  const abs = path.join(ROOT, doc);
  if (!fs.existsSync(abs)) {
    fails.push(`${doc}：被管的活文档不存在（清单与仓库不一致，先确认是不是被移走了）`);
    continue;
  }
  const text = fs.readFileSync(abs, "utf8");
  const lines = text.split("\n");
  for (const m of text.matchAll(CITE)) {
    const [, ref, spec] = m;
    total++;
    // 行号必须落在文件范围内
    const nums = parseLineSpec(spec);
    if (nums.length === 0) continue;
    if (EXTERNAL.some((re) => re.test(ref))) {
      skippedExternal.push(`${doc}: ${ref}${spec}`);
      continue;
    }
    const target = resolveRef(ref);
    if (!target) {
      // 解析不到：只在"它看起来确实像个仓库内路径"时报，避免把散文里的
      // `README.md`（相对文档自身）之类算成断裂。
      if (ref.includes("/")) fails.push(`${doc}:${line(text, m.index)} 引用 \`${ref}\` —— 本仓找不到这个文件`);
      continue;
    }
    const totalLines = fs.readFileSync(target, "utf8").split("\n").length;
    for (const n of nums) {
      if (n < 1 || n > totalLines) {
        fails.push(
          `${doc}:${line(text, m.index)} 引用 \`${ref}${spec}\` —— ` +
            `超出文件范围（${path.relative(ROOT, target)} 共 ${totalLines} 行）`,
        );
      }
    }
  }
}

function line(text, idx) {
  return text.slice(0, idx).split("\n").length;
}

// ---------- 判据 C：覆盖面自证 ----------
if (total < 20) {
  console.error(
    `✗ 只从活文档里认出 ${total} 处 \`file:line\` 引用（今天至少 20 处）\n` +
      `  ⇒ 解析前提变了（正则失配 / 文档改名 / 全部清空），先修这条守卫再谈别的。`,
  );
  process.exit(1);
}

if (fails.length) {
  console.error("✗ 文档引用守卫：以下 `file:line` 已断裂（指向不存在的文件或超出文件范围）\n");
  for (const f of fails) console.error(`  - ${f}`);
  console.error(
    `\n修法：把行号重钉到真符号上（\`grep -n '<符号名>' <文件>\` 现算，别抄）；` +
      `若那是外部文件或本仓确实没有该文件，加进 EXTERNAL 前缀表并写明理由。`,
  );
  process.exit(1);
}

console.log(
  `✓ 文档引用守卫：${LIVE_DOCS.length} 份活文档、${total} 处 \`file:line\` 引用全部落在真文件真行范围内` +
    (ambiguous ? `（${ambiguous} 处裸名有多个同名文件、判不准已跳过）` : "") +
    (skippedExternal.length ? `（${skippedExternal.length} 处外部引用已豁免）` : ""),
);
