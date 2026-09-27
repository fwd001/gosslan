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
//   ② requiredLevel()+bumpVersion()：**一次发布取最高档**（SemVer 标准做法）—— 用于真正发版。
// 为什么不用 ① 定版本：184 个提交里有 23 个大功能，逐条累加会得到 25.1.2 这种数字，
// 它既不表达"这次发布有多大"，也和后端/前端/安装包的版本语义脱节。
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";

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

/**
 * 这条提交**是否算已声明**：
 * - 标题带 `[plan]`（本仓既有标记，Change Budget 也读它）⇒ 视为"零影响声明"，不动版本；
 * - 否则必须有 `Version-Bump: <级别>`，且与自己被定级的那一档一致。
 */
export function declaresBump(row) {
  // ★ 豁免只给**不动应用代码**的提交（`src/` 与 `src-tauri/src/` 之外的改动，如文档、门禁脚本）：
  // 否则一条 `[plan]` 就能把任何代码提交免掉，这个门禁就是装饰。
  if (!row.touchesCode && /\[plan\]/.test(row.subject ?? "")) return true;
  return parseBumpTrailer(row.message ?? "") === row.level;
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
  //
  // 真实后果：`scripts/verify-guards.py` 的「CHANGELOG 结构」用例拿 `check` 当命令，
  // 于是它永远无法进入"恢复即 PASS"，被判成护栏失效（2026-09-16 发现）。
  // **结构是结构、记账是记账** —— 拆成两个命令，各自说各自的话，不互相拖累。
  if (cmd === "changelog") {
    const problems = changelogProblems();
    if (problems.length) {
      console.error(`CHANGELOG 结构检查未通过：\n- ${problems.join("\n- ")}`);
      process.exit(1);
    }
    console.log("CHANGELOG 结构检查通过（唯一行首 `## [Unreleased]` 锚点 + 标题格式 + 新在前降序）");
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
    console.log("| # | commit | 日期 | 类型 | 级别 | 累计版本 | 判据 | 标题 |");
    console.log("|---|---|---|---|---|---|---|---|");
    let v = cur;
    rows.forEach((r, i) => {
      v = bumpVersion(v, r.level);
      const lvl = { patch: "小（patch）", minor: "中（minor）", major: "大（major）" }[r.level];
      console.log(`| ${i + 1} | \`${r.short}\` | ${r.date} | ${r.type} | ${lvl} | ${v} | ${r.reason} | ${r.subject.replace(/\|/g, "\\|")} |`);
    });
    return;
  }

  const top = requiredLevel(rows.map((r) => r.level));
  const target = top ? bumpVersion(cur, top) : cur;

  if (cmd === "level") {
    console.log(top ?? "patch");
    return;
  }
  if (cmd === "release") {
    if (!top) {
      console.log("没有需要发布的提交（版本不变）");
      return;
    }
    console.log(`按最高档 ${top} 提升：${cur} -> ${target}`);
    execSync(`node scripts/version.mjs ${top}`, { stdio: "inherit" });
    return;
  }

  if (cmd === "classify") {
    for (const r of rows) console.log(`${r.short}  ${r.level.padEnd(5)}  ${r.type.padEnd(8)}  ${r.subject}`);
    console.log(`\n最高档: ${top ?? "（无提交）"} ⇒ 本次发布应提升到 ${target}`);
    return;
  }

  // check：门禁。① 版本必须 ≥ 未发布提交要求的版本；② 每个提交都要声明 Version-Bump 并自洽。
  const problems = [];
  if (top && compareVersion(cur, target) < 0) {
    problems.push(`当前版本 ${cur} 落后于未发布提交要求的 ${target}（最高档 ${top}）⇒ 跑 \`npm run version:release\``);
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
  problems.push(...changelogProblems());
  if (problems.length) {
    console.error(`版本号规则检查未通过（自 ${since || "首个提交"}）：\n- ${problems.join("\n- ")}`);
    process.exit(1);
  }
  console.log(
    `版本号规则检查通过：自 ${since || "首个提交"} 共 ${rows.length} 个提交（最高档 ${top ?? "无"}，当前版本 ${cur} ≥ ${target}）` +
      `；声明检查只看未推送的 ${scoped.length} 个（已推送 ${rows.length - scoped.length} 个不再算噪音）`,
  );
}

if (process.argv[1] && process.argv[1].endsWith("semver.mjs")) main();
