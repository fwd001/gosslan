// 版本号规则（**单一事实来源**）：提交 → 级别（小/中/大）→ 版本累加。
//
// 规则（见 docs/VERSIONING.md，遵循 SemVer 2.0.0 + Conventional Commits）：
//   · patch：缺陷修复与非功能性改动（fix / docs / test / chore / build / ci / style / refactor）
//   · minor：**向后兼容**的新能力 / 用户可感知改进（feat、perf）
//   · major：**只有兼容性被破坏**才是 major（提交带 ! 或正文含 BREAKING CHANGE: footer）。
//     改动规模与线索词**不参与**定档 —— 一个大而向后兼容的功能仍然只是 minor。
//
// 两个"累加"口径，两者都实现、用途不同：
//   ① accumulate()：**逐提交累加**（字面执行"每次提交都进一位"）—— 只用于审计/台账；
//   ② owedBumpLevel()（= requiredLevel() 先排除零影响提交）+ bumpVersion()：**一次发布取最高档**
//      （SemVer 标准做法）—— 用于真正发版，也是 `check` 那半条「版本落后」的取档口径。
// 为什么不用 ① 定版本：184 个提交里有 23 个大功能，逐条累加会得到 25.1.2 这种数字，
// 它既不表达"这次发布有多大"，也和后端/前端/安装包的版本语义脱节。
import { execFileSync, execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

export const LEVEL_RANK = { patch: 1, minor: 2, major: 3 };
export const BUMP_TRAILER = "Version-Bump";

/** 提交类型 → 默认级别。 */
export const TYPE_LEVEL = {
  feat: "minor",
  perf: "minor",
  fix: "patch",
  refactor: "patch",
  docs: "patch",
  test: "patch",
  build: "patch",
  chore: "patch",
  ci: "patch",
  style: "patch",
};

/** 破坏性变更的正文标记（Conventional Commits：与 subject 的 ! 等价）。 */
export const BREAKING_FOOTER_RE = /^BREAKING[ -]CHANGE:/m;

/** 解析 `type(scope)!: summary`。 */
export function parseSubject(subject) {
  const m = /^([a-z]+)(\([^)]*\))?(!)?:\s*(.*)$/.exec(subject.trim());
  if (!m) return { type: "other", scope: null, breaking: false, summary: subject.trim() };
  return { type: m[1], scope: m[2] ? m[2].slice(1, -1) : null, breaking: !!m[3], summary: m[4] };
}

export function parseVersion(v) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(String(v).trim());
  if (!m) throw new Error(`非法版本号: ${v}`);
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

export function bumpVersion(version, level) {
  const [a, b, c] = parseVersion(version);
  if (level === "major") return `${a + 1}.0.0`;
  if (level === "minor") return `${a}.${b + 1}.0`;
  if (level === "patch") return `${a}.${b}.${c + 1}`;
  throw new Error(`未知级别: ${level}`);
}

export function compareVersion(x, y) {
  const p = parseVersion(x), q = parseVersion(y);
  for (let i = 0; i < 3; i++) if (p[i] !== q[i]) return p[i] < q[i] ? -1 : 1;
  return 0;
}

/**
 * 给一个提交定级。`churn` = 增删行数之和，`files` = 改动文件数。
 * 判据必须**确定性**（同样的输入永远同样的级别），否则台账与门禁都对不上。
 */
export function classifyCommit({ subject, message = "", churn = 0, files = 0 }) {
  const { type, breaking: bangBreaking } = parseSubject(subject);
  // SemVer 2.0.0：MAJOR 等价于「向后不兼容」。Conventional Commits 给了两种等价声明：
  // subject 里的 ! 与正文的 BREAKING CHANGE: footer。
  const breaking = bangBreaking || BREAKING_FOOTER_RE.test(message);
  if (breaking) {
    return { level: "major", type, reason: "显式破坏性变更（! 或 BREAKING CHANGE:）" };
  }
  if (type === "feat") return { level: "minor", type, reason: `向后兼容的新功能（${files} 文件 / ${churn} 行）` };
  if (type === "perf") return { level: "minor", type, reason: "用户可感知的性能改进（向后兼容）" };
  if (type === "fix") return { level: "patch", type, reason: "缺陷修复" };
  return { level: "patch", type, reason: "非功能性/内部改动（按规则进一位 patch）" };
}

/** 一次发布应取的级别 = 这批提交里的**最高档**（SemVer 标准做法）。 */
export function requiredLevel(levels) {
  let top = null;
  for (const l of levels) {
    if (!LEVEL_RANK[l]) throw new Error(`未知级别: ${l}`);
    if (top === null || LEVEL_RANK[l] > LEVEL_RANK[top]) top = l;
  }
  return top;
}

/** 逐提交累加（字面规则；仅用于台账，别拿来发版）。 */
export function accumulate(version, levels) {
  let v = version;
  for (const l of levels) v = bumpVersion(v, l);
  return v;
}

/** 提交信息里的 `Version-Bump: <level>` 声明（没有则返回 null）。 */
/**
 * 声明门禁的作用范围：**只看未推送的提交**（#123，用户 2026-09-27 拍板）。
 *
 * 为什么不看全范围：绝大多数缺声明的历史提交**已经推送**，改写它们要 rebase 已公开的历史；
 * 而门禁天天报上百条噪音的真实代价是「新漏的那一条被淹掉」。已推送的不再算当前噪音，
 * **新提交仍必须声明**（这条纪律不因静音而放松）。
 *
 * `pushedShorts` 为空 ⇒ 回退成全范围：拿不到远端引用时宁可继续严判，
 * 也不许把「什么都没判」当成静音通过（那是最像成功的一种失败）。
 */
/**
 * 这条路径是不是**会被打包发出去的应用码**。
 * 口径：前端 `src/**` 与 Rust `src-tauri/src/**`，**但测试文件不算**
 * （`*.test.ts` 与 Rust 侧的 `*_tests.rs` / `mod tests` 所在分册都是给门禁跑的，不进产物）。
 * 判错这一条的代价不对称：把测试算成应用码 ⇒ 每次补用例都被迫提版本号（版本号会通胀）；
 * 把应用码算成测试 ⇒ 才是真漏。所以两边都点名，用现算命令可核（见 VERSIONING.md）。
 */
export function isAppCodePath(p) {
  if (/\.(test|spec)\.tsx?$/.test(p)) return false;
  if (/^src-tauri\/src\/.+(_tests|tests)\.rs$/.test(p)) return false;
  return /^src\//.test(p) || /^src-tauri\/src\//.test(p);
}

/** 这条提交是否**声明零影响**：标题带 `[plan]` 且不动应用代码。两处用同一份口径。 */
export function isZeroImpact(row) {
  // ★ 豁免只给**不动应用代码**的提交（`src/` 与 `src-tauri/src/` 之外的改动，如文档、门禁脚本）：
  // 否则一条 `[plan]` 就能把任何代码提交免掉，这个门禁就是装饰。
  return !row.touchesCode && /\[plan\]/.test(row.subject ?? "");
}

/**
 * 这条提交**是否算已声明**：
 * - 标题带 `[plan]`（本仓既有标记，Change Budget 也读它）⇒ 视为"零影响声明"，不动版本；
 * - 否则必须有 `Version-Bump: <级别>`，且与自己被定级的那一档一致。
 */
export function declaresBump(row) {
  if (isZeroImpact(row)) return true;
  return parseBumpTrailer(row.message ?? "") === row.level;
}

/**
 * 这批提交**真正欠**的版本档位 = 非零影响提交里的最高档（零影响那条既不加也不稀释）。
 *
 * 为什么不能直接用整个范围的最高档：`classifyCommit` 对**任何**提交都至少给 patch，
 * 而 `since` 是"最后一次真改了版本号的提交" ⇒ 只要范围里还留着一条纯文档提交，
 * 旧写法就会报「当前版本落后于未发布提交要求的 X」并让 `version:release` 去升一位号。
 * 这既让"攒提交期间的 check 天天红"（红的门禁等于没有门禁），又与 `isAppCodePath` 那条
 * 注释里写明的意图相反（"每次补用例都被迫提版本号 ⇒ 版本号会通胀"）。
 *
 * ⚠️ 这一条**不放松**任何纪律：动应用代码的提交只写 `[plan]` 仍被 `declaresBump` 判红，
 * 于是它照样落进这里（见 `smuggled` 那格用例）。
 */
export function owedBumpLevel(rows) {
  return requiredLevel(rows.filter((r) => !isZeroImpact(r)).map((r) => r.level));
}

export function filterUnpushed(rows, pushedShorts) {
  if (!pushedShorts || pushedShorts.size === 0) return [...rows];
  return rows.filter((r) => !pushedShorts.has(r.short));
}

export function parseBumpTrailer(message) {
  const m = new RegExp(`^${BUMP_TRAILER}:\\s*(patch|minor|major)\\s*$`, "m").exec(message);
  return m ? m[1] : null;
}

// ---------------- CLI ----------------
function git(args) {
  return execSync(`git ${args}`, { encoding: "utf8", maxBuffer: 64 << 20 });
}

function currentVersion() {
  const pkg = JSON.parse(require$read("package.json"));
  return pkg.version;
}
function require$read(p) {
  return execSync(`cat ${p}`, { encoding: "utf8" });
}

/** 取 `since..HEAD` 的提交（含 subject / 改动规模 / 提交信息）。 */
export function collectCommits(since) {
  const range = since ? `${since}..HEAD` : "HEAD";
  const raw = git(`log --no-merges --reverse --pretty=format:%H%x1f%h%x1f%ad%x1f%s%x1f%B%x1e --date=short ${range}`);
  return raw
    .split("\x1e")
    .map((b) => b.trim())
    .filter(Boolean)
    .map((block) => {
      const [hash, short, date, subject, ...rest] = block.split("\x1f");
      const numstat = git(`show --numstat --format= ${hash}`);
      let churn = 0, files = 0, touchesCode = false;
      for (const line of numstat.split("\n")) {
        const parts = line.split("\t");
        if (parts.length < 3) continue;
        files += 1;
        churn += (Number(parts[0]) || 0) + (Number(parts[1]) || 0);
        // 口径：只看**将要发出去的应用码**（前端 src/ 与 Rust src-tauri/src/）；
        // 门禁脚本、文档、workflow 不算 ⇒ 它们仍可走 [plan] 零影响豁免。
        if (isAppCodePath(parts[2] ?? "")) touchesCode = true;
      }
      return { hash, short, date, subject, message: rest.join("\x1f"), churn, files, touchesCode };
    });
}

function latestTag() {
  try {
    return git("describe --tags --abbrev=0").trim();
  } catch {
    return "";
  }
}

/**
 * 上一次**版本提升**提交（改了 package.json 里 version 那一行的最新提交）。
 *
 * 门禁必须从它之后算起：如果从"最近的 tag"算起，那么刚发布完（版本已提到 3.0.0、
 * 但 tag 还停在 v2.1.2）时，范围里仍然含那批老提交 ⇒ 立刻误报"版本落后"。
 */
function lastVersionBump() {
  // 逐个看"改过 package.json"的最近提交，取第一个**真的改了 version 值**的。
  // 不用 `-S`（pickaxe 按字符串出现次数计数，`"2.1.2"`→`"3.0.0"` 次数不变、匹配不到，
  // 我第一版就踩了这个），也不用 `-G`（跨 shell 的转义很容易写错）。
  const hashes = git(`log -n 50 --format=%H -- package.json`).trim().split("\n").filter(Boolean);
  for (const h of hashes) {
    const diff = git(`show --format= --unified=0 ${h} -- package.json`);
    if (/^[+-]\s*"version":/m.test(diff)) return h;
  }
  return latestTag();
}

/**
 * `CHANGELOG.md` 的**结构**问题（返回空数组 = 结构正常）。
 *
 * 为什么需要这条检查：发布脚本按行首的 `## [Unreleased]` 锚点插入新小节。真实事故 ——
 * 它以前用 `includes("## [Unreleased]")` + 字符串 `replace` 找锚点，而某条更新日志的正文里
 * 恰好写了「补回 `## [Unreleased]` 小节」这句话，于是锚点被误命中：4.1.1~4.1.11 全被插进
 * 4.1.0 小节的半句话里，真正的 `## [Unreleased]` 标题被吞掉。这类破坏**不报错、不影响功能**，
 * 只有结构检查能拦住。
 */
export function changelogProblems(path = "CHANGELOG.md") {
  const problems = [];
  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    return [`读不到 ${path}`];
  }
  const headings = text
    .split("\n")
    .map((line, i) => ({ line, no: i + 1 }))
    .filter((h) => h.line.startsWith("## ["));
  const unreleased = headings.filter((h) => /^## \[Unreleased\]\s*$/.test(h.line));
  if (unreleased.length !== 1) {
    problems.push(
      `${path} 里应有且仅有一个**行首**的 \`## [Unreleased]\` 小节（发布脚本的插入锚点），实际 ${unreleased.length} 个`,
    );
  }
  const versions = [];
  for (const h of headings) {
    if (/^## \[Unreleased\]\s*$/.test(h.line)) continue;
    const m = h.line.match(/^## \[(\d+\.\d+\.\d+)\] - \d{4}-\d{2}-\d{2}$/);
    if (!m) {
      problems.push(`${path}:${h.no} 版本小节标题格式不对（应为 \`## [x.y.z] - YYYY-MM-DD\`）：${h.line}`);
      continue;
    }
    versions.push({ v: m[1], no: h.no });
  }
  for (let i = 1; i < versions.length; i += 1) {
    if (compareVersion(versions[i - 1].v, versions[i].v) < 0) {
      problems.push(
        `${path}:${versions[i].no} 版本小节顺序不对：${versions[i].v} 排在 ${versions[i - 1].v} 之后（应为"新在前"的降序）`,
      );
    }
  }
  return problems;
}

/**
 * **本次发版那一节**的归属问题（空数组 = 这一节里没有挂着别版说明的条目）。
 *
 * ## 结构检查为什么看不见这件事
 * `version.mjs` 把新节标题插在 `## [Unreleased]` 那一行的**下面**。于是"先跑提版脚本、
 * 再把条目写在锚点与新标题之间"这个顺序会让新节**看着有内容**，而那条内容其实是
 * **上一次**发版的说明 —— 2026-09-28 实测到：4.31.7/4.31.8/4.31.9 三节各挂着下一版的条目，
 * 而托盘那一条浮在 `[Unreleased]` 里。形状全合规（锚点唯一、标题格式、降序），
 * 只有归属判据拦得住，而读更新日志的人要的正是"哪一版改了什么"。
 *
 * ## 判据（只判能证的，判不了的显式打印）
 * 本节里每条 `### ` 标题：找到把它写进来的那次提交 A。
 * - A **自己就是提版提交**（A 的 `package.json` 版本 ≠ A 父提交的版本）⇒ 它只能待在**自己那一版**
 *   的小节里；挂在别处即红（就是上面那个事故的形状）。
 * - A 不提版（`[plan]` / 攒改动期间的提交）⇒ 不判：这类条目本来就归"下一次发版"。
 * - A 找不到（标题太通用、字面行在历史里重复）⇒ 计入 `判不了`，只打印不判。
 */
export function changelogAttributionProblems(changelogPath = "CHANGELOG.md", pkgPath = "package.json") {
  const problems = [];
  let cur;
  let lines;
  try {
    cur = JSON.parse(readFileSync(pkgPath, "utf8")).version;
    lines = readFileSync(changelogPath, "utf8").split("\n");
  } catch {
    return [`读不到 ${changelogPath} 或 ${pkgPath}`];
  }
  const from = lines.findIndex((l) => l.startsWith(`## [${cur}]`));
  if (from === -1) return [`${changelogPath} 里没有 \`## [${cur}]\` 这一节，而 ${pkgPath} 已是 ${cur}`];
  const items = [];
  for (let i = from + 1; i < lines.length && !/^## \[/.test(lines[i]); i += 1) {
    if (/^### /.test(lines[i])) items.push(lines[i].trim());
  }
  if (items.length === 0) return [`${changelogPath}:${from + 1} \`## [${cur}]\` 是空的（本次发版没有更新说明）`];
  // 谁把 package.json 提到 cur：该字面行最后一次"计数变化"的那次提交
  let bump = "";
  try {
    bump = execFileSync("git", ["log", "-1", "--format=%H", "-S", `"version": "${cur}"`, "--", pkgPath], {
      encoding: "utf8",
    }).trim();
  } catch {
    return [];
  }
  if (!bump) return []; // 提版还没落成提交（还在攒改动），这一节归谁还无从判起
  let undecided = 0;
  for (const it of items) {
    const ev = entryOwner(it, changelogPath, pkgPath);
    if (!ev) {
      undecided += 1;
      continue;
    }
    if (ev.isBumpCommit && ev.version !== cur) {
      problems.push(
        `${changelogPath}:${from + 1} 小节 \`## [${cur}]\` 里挂着「${it.slice(4, 40)}」，` +
          `而写它的那次提交 ${ev.writer.slice(0, 7)} 自己就是把版本提到 ${ev.version} 的那次 ⇒ 这条属于 ${ev.version}，不属于 ${cur}`,
      );
    }
  }
  if (undecided) {
    console.log(
      `（归属判据的量不到之处：本节 ${items.length} 条里有 ${undecided} 条的标题太通用、在历史里重复，字面行找不出唯一写入者 ⇒ 未判）`,
    );
  }
  return problems;
}

/**
 * 一条 `### ` 标题的**归属证据**：谁把它写进文件的，以及那次提交落笔时 `package.json` 是哪一版。
 * 返回 `null` = 判不了（标题在历史里重复，pickaxe 找不出唯一写入者）。
 * `isBumpCommit` = 那次提交自己把版本提了上去（自己的版本 ≠ 父提交的版本）⇒ 它说得出这条属于哪一版。
 * 不提版的提交（`[plan]` 那类）写下的条目本来就归下一次发版 ⇒ 不判。
 */
export function entryOwner(titleLine, changelogPath = "CHANGELOG.md", pkgPath = "package.json") {
  const cache = entryOwner._cache || (entryOwner._cache = new Map());
  const key = `${changelogPath}\u0000${pkgPath}\u0000${titleLine}`;
  if (cache.has(key)) return cache.get(key);
  // ⚠️ 必须 argv 传参，**不能**把标题拼进 shell 命令串：条目标题里有反引号（"搬成 `include!` 分册"），
  // 拼串会被 shell 当成命令替换真的执行一遍（本轮实测到 `/bin/sh: include!: command not found`）。
  const run = (args) => {
    try {
      return execFileSync("git", args, { encoding: "utf8", maxBuffer: 64 << 20 }).trim();
    } catch {
      return "";
    }
  };
  const writer = run(["log", "-1", "--format=%H", "-S", titleLine, "--", changelogPath]);
  if (!writer) {
    cache.set(key, null);
    return null;
  }
  const verOf = (ref) => {
    try {
      return JSON.parse(run(["show", `${ref}:${pkgPath}`])).version;
    } catch {
      return null;
    }
  };
  const parent = run(["log", "-1", "--format=%P", writer]).split(" ")[0];
  const version = verOf(writer);
  const ev = { writer, version, isBumpCommit: parent ? verOf(parent) !== version : true };
  cache.set(key, ev);
  return ev;
}

/**
 * 全历史归属审计（**量具**，一次要跑几百次 git ⇒ 不入门禁层，口径同 `selfproof:check`）。
 * 只统计"证据到得了"的那部分：写入者自己就是提版提交的条目。
 */
export function changelogAttributionAudit(changelogPath = "CHANGELOG.md", pkgPath = "package.json") {
  const lines = readFileSync(changelogPath, "utf8").split("\n");
  const secs = [];
  lines.forEach((l, i) => {
    const m = /^## \[(\d+\.\d+\.\d+)\]/.exec(l);
    if (m) secs.push({ v: m[1], i });
  });
  const rows = [];
  let judged = 0;
  let undecided = 0;
  let notBump = 0; // 写入者不提版的条目：本来就归"下一次发版"，判不了它挂在哪一节才对
  let entries = 0;
  for (const s of secs) {
    let j = s.i + 1;
    const items = [];
    while (j < lines.length && !/^## \[/.test(lines[j])) {
      if (/^### /.test(lines[j])) items.push(lines[j].trim());
      j += 1;
    }
    for (const it of items) {
      entries += 1;
      const ev = entryOwner(it, changelogPath, pkgPath);
      if (!ev) {
        undecided += 1;
        continue;
      }
      if (!ev.isBumpCommit) {
        notBump += 1; // 攒改动期间写的 ⇒ 归下一次发版，不判
        continue;
      }
      judged += 1;
      if (ev.version !== s.v) {
        rows.push(`  「${it.slice(4, 44)}」现挂 ${s.v} → 应归 ${ev.version}（写入者 ${ev.writer.slice(0, 7)} 就是那次提版）`);
      }
    }
  }
  return { sections: secs.length, entries, judged, notBump, misplaced: rows.length, undecided, rows };
}

function main() {
  const [cmd = "check", ...flags] = process.argv.slice(2);

  // `changelog`：**只**查 `CHANGELOG.md` 的结构（唯一行首锚点 / 标题格式 / 新在前降序），
  // 不碰版本记账。
  //
  // ## 为什么必须能单独跑
  //
  // 这条检查原先只作为 `check` 的 ③ 存在，而 `check` 的 ① ②（当前版本必须 ≥ 未发布提交
  // 要求的版本、每个提交都要有自洽的 `Version-Bump:` 声明）在**攒提交期间本来就该是红的** ——
  // 发版前就是那个状态。于是「CHANGELOG 结构」被迫跟着一起红。
  // ★ 2026-09-28 更新（#138 / #123）：那句"本来就该是红的"是**当时的状态**，今天已不成立 ——
  // ②只看未推送提交，①只看 `owedBumpLevel()`（零影响提交不再逼版本号）⇒ `check` 在攒提交期间可绿。
  // **但这条拆分仍然要**：结构检查不许被记账口径的另两半牵动（它判的是文件形状，不是账）。
  //
  // 真实后果：`scripts/verify-guards.py` 的「CHANGELOG 结构」用例拿 `check` 当命令，
  // 于是它永远无法进入"恢复即 PASS"，被判成护栏失效（2026-09-16 发现）。
  // **结构是结构、记账是记账** —— 拆成两个命令，各自说各自的话，不互相拖累。
  if (cmd === "changelog") {
    const problems = [...changelogProblems(), ...changelogAttributionProblems()];
    if (problems.length) {
      console.error(`CHANGELOG 检查未通过：\n- ${problems.join("\n- ")}`);
      process.exit(1);
    }
    console.log(
      "CHANGELOG 检查通过（唯一行首 `## [Unreleased]` 锚点 + 标题格式 + 新在前降序 + 本次发版那一节的条目归属可证）",
    );
    return;
  }

  // `audit-attribution`：全历史归属审计（**量具** —— 一次要跑几百次 git，所以不入门禁层，
  // 口径与 `selfproof:check` 那批一样：能复查，但不拦提交）。
  if (cmd === "audit-attribution") {
    const r = changelogAttributionAudit();
    console.log(
      `版本小节 ${r.sections} 个 · 条目总数 ${r.entries} 条 = 证据到得了 ${r.judged}` +
        ` ＋ 写入者不提版所以不判 ${r.notBump} ＋ 找不到唯一写入者 ${r.undecided}` +
        ` · 其中归属错位 ${r.misplaced} 条`,
    );
    for (const row of r.rows) console.log(row);
    return;
  }

  const sinceFlag = flags.indexOf("--since");
  const since = sinceFlag >= 0 ? flags[sinceFlag + 1] : lastVersionBump();
  const cur = currentVersion();
  const commits = collectCommits(since);
  const rows = commits.map((c) => ({ ...c, ...classifyCommit(c) }));

  // 作用范围（#123）：已推送 = 不在 `HEAD --not --remotes=origin` 那个集合里。
  // 没有任何远端引用时那个集合就是全部 ⇒ pushedShorts 为空 ⇒ 回退成全范围（见 filterUnpushed 文档）。
  // ★ 用 upstream 的两点范围，**不要**写成 `rev-list --not --remotes=origin HEAD`：
  // 那个写法会把 HEAD 也一起取反 ⇒ 输出空 ⇒ "未推送 0 个" ⇒ 声明检查静默变成什么都不判
  // （本仓最禁的一种失败：今天实测它真的返回空）。没有 upstream ⇒ pushedShorts 留空 ⇒ 回退全范围。
  let pushedShorts = new Set();
  let unpushedCount = 0;
  try {
    const upstream = git("rev-parse --abbrev-ref --symbolic-full-name @{u}").trim();
    if (upstream) {
      const unpushedHashes = new Set(
        git(`rev-list ${upstream}..HEAD`)
          .split("\n")
          .filter(Boolean),
      );
      unpushedCount = unpushedHashes.size;
      pushedShorts = new Set(rows.filter((r) => !unpushedHashes.has(r.hash)).map((r) => r.short));
    }
  } catch {
    // 没有 upstream（或远端引用还没取到）⇒ 走全范围，绝不静默放行
  }
  // 反空转闸：说"有 N 个未推送"却在本次范围里一个都没看到 ⇒ 判据自己坏了，当场报错而不是放行
  if (unpushedCount > 0 && rows.length > 0 && rows.length - pushedShorts.size === 0) {
    console.error(`声明检查的作用范围算空了（未推送 ${unpushedCount} 个，但都没落进本次范围）⇒ 拒绝放行，改判为全范围`);
    pushedShorts = new Set();
  }

  if (cmd === "ledger") {
    // ★ 两个洞一起堵（2026-09-28 我自己踩到：`npm run version:ledger` 把一份 271 行的账表
    //   覆盖成只剩表头两行，而**退出码 0**，靠 `git restore` 才救回来）：
    //   ① 旧 npm 脚本是 `… --since main > docs/version-ledger.md`，在 main 分支上 `main..HEAD`
    //     恒为 0 条 ⇒ 每次都产出一张空表；
    //   ② shell 的 `>` 在进程还没判之前就把文件截成 0 字节 ⇒ "拒绝写"必须由拿句柄的这一方做，
    //     所以改成由本命令自己写（`--out`，默认就是原来那个路径）。
    //   **不改范围语义**：默认仍是"上一次改版本号的提交之后"（`check`/`classify` 用的同一个窗口），
    //   累计列也照旧是"从当前版本往后逐条累加"（台账用，别拿来发版 —— 见 accumulate 的注释）。
    const outIdx = flags.findIndex((f) => f === "--out" || f.startsWith("--out="));
    const out =
      outIdx < 0
        ? "docs/version-ledger.md"
        : flags[outIdx] === "--out"
          ? flags[outIdx + 1]
          : flags[outIdx].slice("--out=".length);
    if (!out) {
      console.error("✗ `--out` 后面没有路径");
      process.exit(1);
    }
    if (rows.length === 0) {
      const msg = `范围（${since || "整个历史"}..HEAD）里没有新提交 ⇒ 没有要入账的东西，${out} 一字未动`;
      if (sinceFlag >= 0) {
        // 显式给了范围却一条都没有 ⇒ 十有八九是范围写错（`--since main` 在 main 上就是这种写法），
        // 必须响亮地红；而不显式给范围时的"没有新提交"是正常状态，只如实打印。
        console.error(`✗ ${msg} ⇒ 你给的 --since 框不住任何提交，别让它把账表覆盖成空表`);
        process.exit(1);
      }
      console.log(`· ${msg}`);
      return;
    }
    const lines = [
      "| # | commit | 日期 | 类型 | 级别 | 累计版本 | 判据 | 标题 |",
      "|---|---|---|---|---|---|---|---|",
    ];
    let v = cur;
    rows.forEach((r, i) => {
      v = bumpVersion(v, r.level);
      const lvl = { patch: "小（patch）", minor: "中（minor）", major: "大（major）" }[r.level];
      lines.push(`| ${i + 1} | \`${r.short}\` | ${r.date} | ${r.type} | ${lvl} | ${v} | ${r.reason} | ${r.subject.replace(/\|/g, "\\|")} |`);
    });
    writeFileSync(out, lines.join("\n") + "\n");
    console.log(`✓ 已写 ${out}：${rows.length} 条（末行累计版本 ${v}，范围 ${since}..HEAD）`);
    return;
  }

  const top = requiredLevel(rows.map((r) => r.level));
  // ★ "欠不欠一次版本号"看 `owed`（零影响提交不算），`top` 只是整个范围的字面最高档。
  // 两者在正常节奏下相等（代码提交的标题不会带 [plan]），差集恰好就是"纯文档/判据那一类"。
  const owed = owedBumpLevel(rows);
  const owedTarget = owed ? bumpVersion(cur, owed) : cur;

  if (cmd === "level") {
    console.log(owed ?? "patch");
    return;
  }
  if (cmd === "release") {
    if (!owed) {
      console.log(
        rows.length
          ? `范围内 ${rows.length} 条提交全是零影响（标题 [plan] 且不动 src/ 与 src-tauri/src/）⇒ 版本不变（仍 ${cur}）`
          : "没有需要发布的提交（版本不变）",
      );
      return;
    }
    console.log(`按最高档 ${owed} 提升：${cur} -> ${owedTarget}`);
    execSync(`node scripts/version.mjs ${owed}`, { stdio: "inherit" });
    return;
  }

  if (cmd === "classify") {
    for (const r of rows) console.log(`${r.short}  ${r.level.padEnd(5)}  ${r.type.padEnd(8)}  ${r.subject}`);
    const zero = rows.filter(isZeroImpact);
    console.log(`\n字面最高档: ${top ?? "（无提交）"}　**真欠的档位**: ${owed ?? "（无 —— 全是零影响提交）"}`);
    console.log(`⇒ 本次发布应提升到 ${owedTarget}（排除 ${zero.length} 条零影响提交：${zero.map((r) => r.short).join(" ") || "无"}）`);
    return;
  }

  // check：门禁。① 版本必须 ≥ **真欠**的那一档；② 每个提交都要声明 Version-Bump 并自洽。
  const problems = [];
  if (owed && compareVersion(cur, owedTarget) < 0) {
    problems.push(`当前版本 ${cur} 落后于未发布提交要求的 ${owedTarget}（最高档 ${owed}）⇒ 跑 \`npm run version:release\``);
  }
  // ★ 声明检查只看**未推送**的提交；`--all-commits` 显式扩回全范围（审计/自查用）。
  const scoped = flags.includes("--all-commits") ? rows : filterUnpushed(rows, pushedShorts);
  const wrong = scoped.filter((r) => !declaresBump(r));
  if (wrong.length) {
    problems.push(
      `${wrong.length} 个**未推送**提交缺少/写错了 \`${BUMP_TRAILER}:\` 声明（应为该提交的级别；已推送的历史不算噪音，审计请加「--all-commits」）：\n` +
        wrong.slice(0, 8).map((r) => `  ${r.short} 期望 ${r.level} 实际 ${parseBumpTrailer(r.message) ?? "(无)"}  ${r.subject}`).join("\n"),
    );
  }
  // ③ CHANGELOG 结构（锚点存在 + 标题格式 + 新在前的降序）。
  problems.push(...changelogProblems(), ...changelogAttributionProblems());
  if (problems.length) {
    console.error(`版本号规则检查未通过（自 ${since || "首个提交"}）：\n- ${problems.join("\n- ")}`);
    process.exit(1);
  }
  const zeroCount = rows.filter(isZeroImpact).length;
  console.log(
    `版本号规则检查通过：自 ${since || "首个提交"} 共 ${rows.length} 个提交` +
      `（真欠的档位 ${owed ?? "无 —— 范围内全是零影响提交"}，字面最高档 ${top ?? "无"}，其中 ${zeroCount} 条按 [plan] 零影响豁免` +
      `；动应用代码的提交写 [plan] 不算豁免），当前版本 ${cur} ≥ ${owedTarget}` +
      `；声明检查只看未推送的 ${scoped.length} 个（已推送 ${rows.length - scoped.length} 个不再算噪音）`,
  );
}

if (process.argv[1] && process.argv[1].endsWith("semver.mjs")) main();
