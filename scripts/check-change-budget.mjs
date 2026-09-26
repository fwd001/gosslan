#!/usr/bin/env node
/**
 * Change Budget 守门 —— 把「改动半径」与「重复犯案」从口号变成机器判定。
 *
 * ## 为什么需要它
 *
 * CHANGELOG `4.18.7 → 4.18.10` 连着四个版本修同一个 BLE 分片问题,每一版都只有
 * 2~4 文件 / +24~+136 行 —— **小 diff 不等于安全**。如果门禁只用"改动小 = 放行",
 * 这一串会被全部放行。所以本守门有三个判据,而不是一个:
 *
 *   1. **规模**:超大 diff 必须显式声明(而不是默默合进去);
 *   2. **敏感文件**:碰核心协议/密码学/DB schema 的改动无论多小都要显式声明;
 *   3. **重复犯案**:同一领域连续被打补丁 ⇒ 说明缺不变量或单一事实来源,
 *      4.18.7→4.18.10 就是标准样本(四个"小修复"互相修)。
 *   4. **版本声明一致性**:`Version-Bump` 写了就必须真 bump 四个版本清单文件,反之亦然
 *      —— trailer 不只是仪式:判据 3 拿它判"这次是不是修补"。
 *
 * ## 判据(对范围内每个 commit)
 *
 * 先算**计入文件** = 改动文件 − 豁免文件(见下)。计入文件为空 ⇒ 整个 commit
 * 是纯工程/文档 commit ⇒ PASS。判据 4 **不看豁免**(版本清单文件本就在豁免之外),
 * 但 git 模式下读不到完整 message 时判据 3/4 一律**停用并计一条失败** ——
 * 停用与通过打印不同,不会悄悄变绿。
 *
 * | 级别 | 条件 | 要求 |
 * |---|---|---|
 * | L1 | ≤5 文件 && ≤200 LOC && ≤1 领域 && 不碰敏感文件 | 直接放行 |
 * | L2 | ≤10 文件 && ≤500 LOC && ≤2 领域 && 不碰敏感文件 | commit message 必须含 `[plan]` |
 * | L3 | 其余,或碰了敏感文件 | commit message 必须含 `[impact]` |
 *
 * **敏感文件**(碰了直接 L3,与规模无关):`protocol.rs`(线协议)、`crypto.rs`(E2EE)。
 * 这两个一错就是安全或全库数据问题,值得多一次显式声明。
 * ⚠️ 这里**曾经还有第三个** `schema.sql`,2026-09-26 随该文件一起退役
 * (表结构的唯一真源是 `db.rs` 的 SCHEMA 常量;那份手写 DDL 没有任何代码读它 ⇒
 * 把它当"敏感真源"反而是在给一份没人核对的假事实源发牌照)。
 * **别顺手把它加回来** —— 要防的是"手写第二份 DDL",不是"这个文件名"。
 *
 * **豁免文件**(不计入文件数/LOC):`*.md`、`*.txt`(含 test-baseline)、
 * `docs/**`、`.github/**`、`scripts/**`。理由:这些是工程仪式与文档,不是
 * 产品代码;不豁免的话,每加一个守门脚本/每写一次 CHANGELOG 都在吃预算。
 * 风险(在 docs 里藏坏内容)由 PR review 兜底。
 *
 * **版本白名单**(仅 `chore(release)` 生效):package.json / package-lock.json /
 * Cargo.toml / Cargo.lock / tauri.conf.json。每次发版固定动这 6 个文件,不豁免
 * 会每个版本撞门。
 *
 * **重复犯案**:取 `<merge-base>..HEAD` 内最近 5 个「**修补形状**」提交,各自映射领域
 * (豁免文件不计);任一领域出现 ≥3 次 ⇒ FAIL。窗口**只看本分支独有**的提交
 * —— main 上历史上已经有 BLE 连修的旧案,向前看,不审判历史。
 *
 * 「修补形状」**不看前缀怎么写**:前缀是手打的、可以写成 `feat` 躲窗口(真实形状:
 * `feat(ui): 主题色体系 + …… + 群任务编辑权限修复`)。改看 **`Version-Bump` 声明**:
 * 声明 `patch` = 作者断言"这次没有新能力" ⇒ 无论前缀是 fix 还是 feat 都进窗口;
 * 没有声明的旧提交退回按 `^fix` 前缀判(不缩小既有窗口)。
 *
 * **判据 4 —— 版本声明必须落地**(2026-09-21 加,PR #22/#23 的真实缺口):
 * message 里写了 `Version-Bump: patch|minor|major`,提交里就必须**同时**动这四个版本
 * 清单文件 —— `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、
 * `src-tauri/tauri.conf.json`(`scripts/version.mjs` 一次写这四个;`Cargo.lock` 归 cargo
 * 管、可能延后同步,所以不要求)。反方向同样:四个都动了却没声明 ⇒ 红
 * (`chore(release)` 例外:那条流的声明就在 subject 里)。
 * 为什么两个方向都要:判据 3 现在**消费这个 trailer**,一个不成立的 trailer 就等于
 * "窗口可以靠不写 trailer 躲开" —— 装饰性声明比没有声明更糟。
 * 近 200 个提交实测:命中红线的只有 PR #22/#23 那两条(声明了 bump、四个文件一个没动,
 * 版本最后是 v4.23.0 手工补账的),其余形状全部一致 ⇒ 不会误伤历史。
 *
 * ## 领域归属
 *
 * 文件路径按 `docs/domains.data.mjs` 的 `paths` 前缀映射;落进 `unmapped`
 * 的(commands.rs / state.rs / lib.rs 等装配层)**不计领域数**但**计文件数/LOC**
 * —— 改装配层不是免费的,只是它不属于哪个具体领域。
 *
 * ## 用法
 *
 *     node scripts/check-change-budget.mjs                     # 本地:检查 HEAD~1..HEAD
 *     node scripts/check-change-budget.mjs --range a1b2c3..HEAD
 *     node scripts/check-change-budget.mjs --from-json scripts/fixtures/change-budget.json
 *
 * CI(push event)自动用 `github.event.before..github.sha`;拿不到或 force push
 * 时退化为 HEAD~1..HEAD —— 并**打印实际用的范围**,不静默。
 *
 * `--from-json` 是**测试接缝**:verify-guards.py 的非空转用例喂 fixture,
 * 不碰 git。fixture 格式见 `scripts/fixtures/change-budget.json`。
 *
 * ## 本守门守不住的(诚实边界)
 *
 * · **语义**:+10 行可以把整条链路发不出消息(4.18.9 就是),规模判据管不了语义
 *   —— 那靠测试与不变量;
 * · **commit 切分 gaming**:把一个大改动拆成多个小 commit 就绕过了规模判据
 *   —— 但重复犯案判据仍会盯住"同一领域反复改";两道闸互为补充;
 * · **不写 trailer**:判据 4 管的是"写了就得真做",不管"该写却没写"。不声明
 *   `Version-Bump` 的提交会退回按 `^fix` 前缀进犯案窗口 ⇒ 把一条真修复写成
 *   `feat` 且**同时**删掉 trailer,仍能躲开窗口。这一步不做硬要求的原因:近 200 个提交里
 *   「无 trailer 无版本文件」有 56 条(旧的"多 commit 攒一版"流),现在强令每 commit 必
 *   bump 会把门禁变成对历史的审判。真发生了靠 review + `[plan]` 说明兜底。
 *
 * 退出码:0 = 全部通过;1 = 有 commit 超预算未声明、版本声明未落地、重复犯案,
 *         或（**仅当调用方声明了 `GOSSLAN_BUDGET_STRICT=1`**，目前只有 verify.yml）
 *         CI push→main 的范围不是事件里的 before..sha ⇒ 门禁在空转;
 *         2 = **零覆盖**(跑了但范围内没有 commit 可判,常见于本地"已 push 之后重跑")。
 *         ⇒ 2 不是 0:`verify.mjs` 会把它单列成"未覆盖项",而不是 ✅;
 *         ⇒ 出包脚本(build-android-releases.sh)把 2 当**警告**继续 —— 门禁判不到范围
 *           不该打断出包,那是另一件事。
 */

import { readFileSync, existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import domainMap from "../docs/domains.data.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// ---------------- 阈值与清单(全部显式,不做魔法) ----------------

/** L1:默认放行的"小改动"。依据:9/10 真实历史修复落在 ≤4 文件 / ≤+136 LOC。 */
const L1 = { files: 5, loc: 200, domains: 1 };
/** L2:中改动,需要 [plan] 标记(说明改了什么、为什么)。 */
const L2 = { files: 10, loc: 500, domains: 2 };
/** L3:以上之外,或碰敏感文件。需要 [impact] 标记(Impact Report 的最小形态)。 */

/** 碰了直接升 L3 的文件(仓库相对路径)。一错就是安全/全库数据问题的三处。 */
const SENSITIVE_FILES = new Set([
  "src-tauri/src/protocol.rs",
  "src-tauri/src/crypto.rs",
]);

/** 豁免文件(不计入预算):工程仪式与文档。 */
const EXEMPT_PREFIXES = [".github/", "scripts/", "docs/"];
const EXEMPT_SUFFIXES = [".md", ".txt"];

/** 版本白名单(仅 chore(release) 时豁免):一次发版固定动的五个文件。 */
const RELEASE_WHITELIST = new Set([
  "package.json",
  "package-lock.json",
  "src-tauri/Cargo.toml",
  "src-tauri/Cargo.lock",
  "src-tauri/tauri.conf.json",
]);

/** 重复犯案窗口:最近 N 个修补形状提交。 */
const OFFENDER_WINDOW = 5;
/** 同一领域在窗口内出现 ≥ N 次 ⇒ FAIL。依据:4.18.7→4.18.10 是 4 次;第 3 次就拦。 */
const OFFENDER_LIMIT = 3;

/**
 * 一次真 bump 必定同时出现的四个版本清单文件(`scripts/version.mjs` 直接写这四个)。
 * 不含 `Cargo.lock`:那个由 cargo 在构建时同步,可以合法地晚一版。
 */
const VERSION_FILES = [
  "package.json",
  "package-lock.json",
  "src-tauri/Cargo.toml",
  "src-tauri/tauri.conf.json",
];

/** `Version-Bump: patch|minor|major` trailer(整行,允许行首尾空白)。 */
const BUMP_DECL = /^Version-Bump:[ \t]*(patch|minor|major)[ \t]*$/im;

/** 取 commit 的完整 message(fixture 里就是 message 本身)。 */
function fullMessage(commit) {
  return commit.full ?? commit.message;
}

/** 声明的 bump 类型;没声明返回 null。 */
function declaredBump(commit) {
  const m = fullMessage(commit).match(BUMP_DECL);
  return m ? m[1].toLowerCase() : null;
}

/** 四个版本清单文件里这个 commit 没动的那些。 */
function missingVersionFiles(commit) {
  const have = new Set(commit.files.map((f) => f.path.replaceAll("\\", "/")));
  return VERSION_FILES.filter((p) => !have.has(p));
}

/** merge 提交不参与版本声明判定(bump 由被合并的那条 commit 或 release commit 承担)。 */
function isMerge(commit) {
  return /^Merge\b/.test(commit.message.split("\n")[0].trim());
}

/**
 * 「修补形状」= 这次没有新能力,只是在补已有的东西。
 * 判据优先看**声明**而不是前缀:前缀是手打的,`feat(ui): …… + 权限修复` 这种
 * 把修复塞进 feat 的形状会躲开只看 `^fix` 的窗口(判据 3 的犯案窗口曾因此漏判)。
 */
function isPatchShaped(commit) {
  if (isMerge(commit)) return false;
  const bump = declaredBump(commit);
  if (bump) return bump === "patch";
  return /^fix/.test(commit.message.split("\n")[0].trim());
}

// ---------------- 参数 ----------------

const argv = process.argv.slice(2);
function argOf(flag) {
  const i = argv.indexOf(flag);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : null;
}
const fromJson = argOf("--from-json");
const rangeArg = argOf("--range");

let ok = true;
const fail = (msg) => {
  ok = false;
  console.error(msg);
};

function git(...args) {
  return execFileSync("git", args, { cwd: ROOT, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

// ---------------- 领域归属 ----------------

/**
 * 仓库相对路径(POSIX 分隔)→ 领域 id | "assembly" | "unknown"。
 * assembly = unmapped 清单里的装配层;unknown = 谁都不认领(计入但不算领域数)。
 */
const unmappedPrefixes = (domainMap.unmapped ?? []).map(([p]) => p);
function domainOf(relPath) {
  const p = relPath.replaceAll("\\", "/");
  for (const d of domainMap.domains) {
    for (const entry of d.paths ?? []) {
      const e = entry.replaceAll("\\", "/");
      if (p === e || p.startsWith(e + "/")) return d.id;
    }
  }
  for (const u of unmappedPrefixes) {
    if (p === u || p.startsWith(u.replace(/\/$/, "") + "/")) return "assembly";
  }
  return "unknown";
}

/** 是否豁免文件(不计入预算)。 */
function isExempt(relPath) {
  const p = relPath.replaceAll("\\", "/");
  if (EXEMPT_PREFIXES.some((x) => p.startsWith(x))) return true;
  if (EXEMPT_SUFFIXES.some((x) => p.endsWith(x))) return true;
  return false;
}

/**
 * 对一个 commit 做分级判定。
 * @returns {{level: "EXEMPT"|"L1"|"L2"|"L3", counted: number, loc: number,
 *            domains: string[], sensitive: string[], problem: string|null}}
 */
function classify(commit) {
  const release = /^chore\(release\)/.test(commit.message);
  const counted = commit.files.filter(
    (f) =>
      !isExempt(f.path) &&
      !(release && RELEASE_WHITELIST.has(f.path.replaceAll("\\", "/"))),
  );
  const countedFiles = counted.filter((f) => !SENSITIVE_FILES.has(f.path.replaceAll("\\", "/")));
  const loc = counted.reduce((s, f) => s + f.add + f.del, 0);
  const domainSet = new Set(counted.map((f) => domainOf(f.path)).filter((x) => x !== "assembly" && x !== "unknown"));
  const domains = [...domainSet];
  const sensitive = counted.filter((f) => SENSITIVE_FILES.has(f.path.replaceAll("\\", "/"))).map((f) => f.path);
  const files = counted.length;

  if (files === 0) {
    return { level: "EXEMPT", counted: 0, loc, domains, sensitive, problem: null };
  }

  // [plan] 与 [plan: 说明] 都算 —— 冒号形式更自然(把计划直接写在标记里)
  const hasPlan = /\[plan[:\]]/i.test(commit.message);
  const hasImpact = /\[impact[:\]]/i.test(commit.message);

  const isL1 = files <= L1.files && loc <= L1.loc && domains.length <= L1.domains && sensitive.length === 0;
  if (isL1) return { level: "L1", counted: files, loc, domains, sensitive, problem: null };

  const isL2 =
    sensitive.length === 0 && files <= L2.files && loc <= L2.loc && domains.length <= L2.domains;
  if (isL2) {
    return hasPlan
      ? { level: "L2", counted: files, loc, domains, sensitive, problem: null }
      : {
          level: "L2",
          counted: files,
          loc,
          domains,
          sensitive,
          problem: `改动超出 L1(${files} 文件 / ${loc} 行 / ${domains.length} 领域)但 message 没有 [plan] 标记 —— 请在 commit message 里补 [plan] 并说明改动计划`,
        };
  }

  // L3
  return hasImpact
    ? { level: "L3", counted: files, loc, domains, sensitive, problem: null }
    : {
        level: "L3",
        counted: files,
        loc,
        domains,
        sensitive,
        problem:
          `改动达到 L3(${files} 文件 / ${loc} 行 / ${domains.length} 领域` +
          (sensitive.length ? `;碰敏感文件: ${sensitive.join(", ")}` : "") +
          `)但 message 没有 [impact] 标记 —— 请补 [impact] 与 Impact Report(动了什么、为什么安全、怎么验证)`,
      };
}

// ---------------- 数据源 ----------------

/**
 * 解析 `git log --format=%H%x00%s --numstat` 的输出。
 *
 * ⚠️ git 的输出形状(实测)是:
 *     sha\0subject
 *     (空行)          ← 空行跟在 format 头**之后**
 *     12  3  file.rs
 *     sha2\0subject2   ← 下一个头**直接**跟随,块之间没有空行
 * 所以**不能按空行切块** —— 要逐行扫:含 \0 的行是新 commit 头,其余是它的 numstat。
 */
function parseCommits(raw) {
  const commits = [];
  let current = null;
  for (const line of raw.split("\n")) {
    if (line.includes("\0")) {
      const [sha, ...msgParts] = line.split("\0");
      current = { sha: sha.slice(0, 7), message: msgParts.join("\0"), files: [] };
      commits.push(current);
      continue;
    }
    if (!current) continue;
    const m = line.match(/^(\d+|-)\t(\d+|-)\t(.+)$/);
    if (!m) continue;
    const [, add, del, file] = m;
    if (add === "-" || del === "-") continue; // 二进制:无法按行计,跳过(改动可见于 review)
    // rename(-M 开启)形如 `old => new` / `prefix{old => new}suffix`:取新路径
    const renamed = file.includes(" => ") ? file.split(" => ")[1].replace(/[}]/g, "") : file;
    const clean = renamed.replace(/^\{/, "").replace(/\}.*/, "");
    current.files.push({ path: clean, add: Number(add), del: Number(del) });
  }
  return commits;
}

/**
 * 重复犯案的观察窗口 = **本次受检范围内**的「修补形状」提交（= "本分支独有"，与文件头声明的语义一致）。
 *
 * ⚠️ 为什么不用 `merge-base(HEAD, origin/main)..HEAD`：在 fork + rebase 的工作流下，
 * merge-base 之后的提交里**包含 main 自己**的 fix ⇒ 只要某领域最近被 main 修过，
 * 任何人再提交一条**独立的新**修复都会被算成"第 3 次"而失败 —— 等于把该领域锁死。
 *
 * 2026-09-17 实测到这一点：`transport` 在窗口里已经有上游的 2 次（eca44cf、d6e5c82），
 * 我们再补一条**与那些缺陷毫无关系**的 Windows clippy 告警修复（`notify_payload_budget`
 * 在 Windows 上没有调用点）就凑满 3 次 ⇒ 被要求"停下来补不变量"。这与文件头那句
 * 「窗口**只看本分支独有**的提交 —— main 上历史上已经有 BLE 连修的旧案，向前看，不审判历史」
 * 正好相反：实现审判了历史。
 *
 * 改成按受检范围取窗口后，语义变成："**这一批提交里**同一领域反复修 ⇒ 说明该收敛了"，
 * 既能拦住 4.18.7→4.18.10 那种"一个分支里连打四个补丁"，又不会因为 main 的历史而误伤。
 *
 * ⚠️ 2026-09-21：窗口不再自己跑一次 `git log --grep=^fix`，而是从**已经拿到的**受检提交里
 * 用 `isPatchShaped` 选。两个原因：① 判据要靠 trailer，而 trailer 在 message 正文里，
 * `--grep=^fix` 那种 subject 锚定的过滤根本看不见它；② 同一批数据两处各取一遍 = 两条
 * 口径会漂移，而"窗口和受检范围不是同一批提交"正是上面那次误伤的成因。
 */
function buildOffenderWindow(candidates) {
  // git log 输出新→旧，取前 N 个 = 最近 N 个（与旧的 `-n 5` 语义一致）
  return candidates.filter(isPatchShaped).slice(0, OFFENDER_WINDOW);
}

/**
 * 取范围内每个 commit 的**完整** message（判据 4 要读 trailer，trailer 在正文里）。
 *
 * 单独一趟 `git log`、用 \x01 收尾：`parseCommits` 是逐行状态机（见其注释），
 * 把 %B 塞进同一趟输出会让正文里恰好长成 numstat 形状的行被误认成文件 —— 不为省一次
 * git 调用去冒"门禁悄悄读错数据"的险。
 */
function fullMessagesInRange(range) {
  const raw = git("log", range, "--format=%H%x00%B%x01");
  const out = new Map();
  for (const chunk of raw.split("\x01")) {
    if (!chunk.includes("\0")) continue;
    const [rawSha, ...msg] = chunk.split("\0");
    // ⚠️ 必须 trim：git 在每条记录后补的换行会留在**下一块**的 sha 前面（实测 —— 不 trim
    // 时只有第一条记录能对上 key，其余 commit 全被当成"没有 trailer"而误报）。
    const sha = rawSha.trim();
    if (sha.length < 7) continue;
    out.set(sha.slice(0, 7), msg.join("\0").replace(/^\n+|\n+$/g, ""));
  }
  return out;
}

/** @type {{commits: {sha: string, message: string, files: {path: string, add: number, del: number}[]}[], recentFixes: ?Array}} */
let data;
let usedRange = null;
/** 完整 message 是否读到了（判据 4 与犯案窗口的前置；读不到 = 停用，不是通过）。 */
let messagesReadable = true;
/** 本次是否**一个 commit 都没判到**（零覆盖）。零覆盖必须以退出码 2 单独说话，不能算 ✅。 */
let zeroCoverage = false;
/** 范围是否真来自 CI push event 的 before..sha（push→main 时这是"门禁有没有在跑"的唯一凭据）。 */
let rangeFromEventBefore = false;
/** 实际用的范围是从哪儿来的（写进输出，也写进"CI 空转"那条失败信息）。 */
let rangeSource = null;

if (fromJson) {
  data = JSON.parse(readFileSync(path.resolve(ROOT, fromJson), "utf8"));
  console.log(`数据源:fixture ${fromJson}(${data.commits.length} 个 commit)`);
  // fixture 没有 git range 可取，窗口由 `recentFixes` 显式喂；**同一个谓词**照跑，
  // 所以判据 3 的"看声明不看前缀"这条新口径在 fixture 模式下是真的被测到的。
  data.recentFixes = buildOffenderWindow(data.recentFixes ?? []);
} else {
  // 范围:显式 --range > CI push event(before..sha)> 未推送(origin/<branch>..HEAD)> HEAD~1..HEAD
  //
  // 为什么是"未推送"而不是 HEAD~1:HEAD~1..HEAD 会**永远重查最后一个 commit** ——
  // 它一旦被 push 过,再跑本地 verify 就红,门禁变成了对历史的审判而不是对未来的闸。
  // "未推送"与 CI 的 before..after 语义一致:门禁向前看。
  let range = rangeArg;
  rangeSource = rangeArg ? "--range 显式指定" : null;
  if (!range && process.env.GITHUB_EVENT_NAME === "push" && process.env.GITHUB_EVENT_BEFORE) {
    const before = process.env.GITHUB_EVENT_BEFORE;
    // force push / 新分支时 before 不在本仓库历史里,git 会报错 —— 用 try 探测
    try {
      execFileSync("git", ["cat-file", "-e", `${before}^{commit}`], { cwd: ROOT, stdio: "ignore" });
      range = `${before}..${process.env.GITHUB_SHA ?? "HEAD"}`;
      rangeSource = "CI push event 的 before..sha";
      rangeFromEventBefore = true;
    } catch {
      /* before 不可达,落到未推送范围 */
    }
  }
  if (!range) {
    // 候选按序取**第一个非空**的范围：`origin/<本分支>..HEAD` 在"刚把分支推上去、CI 正在
    // 跑"这种情形下是**空**的（远端 ref 已经指向 HEAD），这时退回与 main 的差集才判得到东西。
    let branch = "HEAD";
    try {
      branch = git("rev-parse", "--abbrev-ref", "HEAD").trim();
    } catch {
      /* 空仓库等极端情形,直接用 HEAD~1 */
    }
    const inCI = Boolean(process.env.GITHUB_EVENT_NAME);
    const candidates = [
      `origin/${branch}..HEAD`,
      ...(branch === "main" ? [] : ["origin/main..HEAD"]),
      // ⚠️ `HEAD~1..HEAD` 只在 CI 上兜底。本地正相反：全部已推送时它会把"最后一个已推送
      // commit"再判一遍 —— 那是审判历史而不是把关未来（文件头讲过这个坑），本地宁可报
      // **零覆盖**（退出码 2），也不要既误红又假装判过了。
      ...(inCI ? ["HEAD~1..HEAD"] : []),
    ];
    for (const c of candidates) {
      try {
        git("cat-file", "-e", c.split("..")[0] + "^{commit}");
        if (Number(git("rev-list", "--count", c).trim()) > 0) {
          range = c;
          rangeSource = `未推送范围（候选里第一个非空：${c}）`;
          break;
        }
      } catch {
        /* 该候选的端点不存在,试下一个 */
      }
    }
    if (!range) {
      range = candidates[0];
      rangeSource = "所有候选范围都为空（没有新 commit 可判 ⇒ 零覆盖）";
    }
  }
  usedRange = range;

  // 空范围(全部已推送)= 没有要检查的新 commit —— **零覆盖,不是通过**（见文末退出码）
  let commitsRaw = "";
  let rangeReadable = true;
  try {
    commitsRaw = git("log", range, "--numstat", "-M", "--format=%H%x00%s");
  } catch {
    rangeReadable = false;
    console.log(`检查范围:${range}(不可达)—— 判据 1/2/4 与窗口全部**未运行**`);
  }
  console.log(`检查范围:${usedRange}（来源：${rangeSource}）`);

  // 解析:逐行状态机(见 parseCommits 注释 —— 不能按空行切块)
  data = { commits: parseCommits(commitsRaw) };

  // 判据 4 与犯案窗口都要读 message 正文里的 trailer ⇒ 补一趟完整 message。
  // 拿不到就**显式失败并停用这两道**：静默退回"只有 subject"会让判据 4 把每一条真 bump
  // 误报成"动了版本文件却没声明" —— 一条读错数据的门禁比没有门禁更糟。
  if (rangeReadable && data.commits.length > 0) {
    try {
      const full = fullMessagesInRange(range);
      for (const c of data.commits) c.full = full.get(c.sha) ?? c.message;
    } catch (e) {
      messagesReadable = false;
      fail(
        `  ✗ 读取 commit 完整 message 失败 ⇒ 判据 4 与犯案窗口都判不了（已停用，不是通过）：` +
          `${e.message.split("\n")[0]}`,
      );
    }
  }

  // 重复犯案窗口:同一批受检提交里的最近 N 个「修补形状」
  data.recentFixes = rangeReadable && messagesReadable ? buildOffenderWindow(data.commits) : null;
}

// 零覆盖 = 一个 commit 都没判到（范围空 / 不可达 / fixture 空）。**不是通过**。
zeroCoverage = data.commits.length === 0;

// ---------------- 判据 1/2:逐 commit 分级 ----------------

console.log("\n判据 1/2:变更分级(规模 / 敏感文件 / 标记)");
for (const commit of data.commits) {
  const verdict = classify(commit);
  const tag = verdict.problem ? "✗" : "✓";
  const level =
    verdict.level === "EXEMPT" ? "豁免" : verdict.level;
  console.log(
    `  ${tag} ${commit.sha} ${level.padEnd(4)} ` +
      `${verdict.counted} 文件 / ${verdict.loc} 行` +
      (verdict.domains.length ? ` / 领域[${verdict.domains.join(",")}]` : " / 无领域文件") +
      (verdict.sensitive.length ? " / ⚠️敏感" : "") +
      `  ${commit.message.split("\n")[0].slice(0, 60)}`,
  );
  if (verdict.problem) fail(`      ${verdict.problem}`);
}
if (zeroCoverage) {
  console.log("  ⚠️ 零覆盖：受检范围里一个 commit 都没有 ⇒ 三道判据都没看过任何代码（**不等于通过**）");
} else if (ok) console.log("  ✓ 全部 commit 在预算内或已声明");

// ---------------- 判据 4:版本声明必须落地 ----------------
// trailer 现在是有消费者的（判据 3 拿它判"这次是不是修补"）。一个可以随便写、也可以
// 随便不写的声明 = 门禁的输入是装饰 ⇒ 两个方向一起判。
console.log("\n判据 4:版本声明与版本清单文件必须一致");
if (!messagesReadable) {
  console.log("  (完整 message 没读到 ⇒ 本判据**未运行**；上面已计一条失败)");
} else {
  let checked = 0;
  for (const commit of data.commits) {
    if (isMerge(commit)) continue;
    const bump = declaredBump(commit);
    const missing = missingVersionFiles(commit);
    const allPresent = missing.length === 0;
    const releaseSubject = /^chore\(release\)/.test(commit.message.split("\n")[0].trim());
    checked += 1;
    if (bump && !allPresent) {
      fail(
        `  ✗ ${commit.sha} 声明了 \`Version-Bump: ${bump}\`，但 ${missing.length}/${VERSION_FILES.length} ` +
          `个版本清单文件没动：${missing.join(", ")} —— 声明没落地就是没 bump。` +
          `跑 \`npm run version:${bump}\` 把这四个文件一起提上来；删掉 trailer 不是解法 —— ` +
          `真 bump 缺声明同样判红，而且犯案窗口会看不见这一版。`,
      );
    } else if (!bump && allPresent && !releaseSubject) {
      fail(
        `  ✗ ${commit.sha} 动了全部 ${VERSION_FILES.length} 个版本清单文件（一次真 bump）` +
          `却没声明 \`Version-Bump: patch|minor|major\` —— 缺声明 ⇒ 犯案窗口无法判断这次是不是修补。`,
      );
    } else {
      console.log(
        `  ✓ ${commit.sha} ${bump ? `Version-Bump: ${bump}` : "无声明"}${allPresent ? " / 版本文件 4/4" : ""}` +
          `${!bump && allPresent && releaseSubject ? "（chore(release) 豁免声明）" : ""}`,
      );
    }
  }
  if (checked === 0) console.log("  (范围内没有需要判定的 commit)");
}

// ---------------- 判据 3:重复犯案 ----------------

if (data.recentFixes) {
  console.log(
    `\n判据 3:重复犯案(受检范围内的修补形状提交最近 ${OFFENDER_WINDOW} 个里,同领域 ≥${OFFENDER_LIMIT} 次即红)` +
      `—— 本次窗口 ${data.recentFixes.length} 个`,
  );
  if (data.recentFixes.length === 0) console.log("  (窗口里一个修补形状提交都没有 —— 无可判定的重复)");
  /** @type {Map<string, string[]>} 领域 → [sha...] */
  const byDomain = new Map();
  for (const fix of data.recentFixes) {
    const domains = new Set(
      fix.files.filter((f) => !isExempt(f.path)).map((f) => domainOf(f.path)).filter((x) => x !== "assembly" && x !== "unknown"),
    );
    for (const d of domains) {
      if (!byDomain.has(d)) byDomain.set(d, []);
      byDomain.get(d).push(fix.sha);
    }
  }
  let offender = null;
  for (const [d, shas] of byDomain) {
    const mark = shas.length >= OFFENDER_LIMIT ? "✗" : "✓";
    console.log(`  ${mark} ${d}: ${shas.length} 次(${shas.join(", ")})`);
    if (shas.length >= OFFENDER_LIMIT && !offender) offender = { d, shas };
  }
  if (byDomain.size === 0) console.log("  (窗口里的修补都没落在有归属的领域上,无领域可计)");
  if (offender) {
    fail(
      `  ✗ 「${offender.d}」领域在最近 ${OFFENDER_WINDOW} 个修补形状提交里出现了 ${offender.shas.length} 次 —— ` +
        `这是 4.18.7→4.18.10 的标准犯案形态(每个补丁都很小,但它们在互相修)。\n` +
        `      请停下来:① 该领域的不变量补了吗(docs/protocol-invariants.md)?\n` +
        `      ② 单一事实来源收敛了吗(docs/migration-ledger.md 的台账行)?\n` +
        `      ③ 如果确属独立新问题,拆到独立分支分别提交,别在一个分支上连打补丁。`,
    );
  } else {
    console.log("  ✓ 无重复犯案");
  }
}

// ---------------- 汇总 ----------------

const ciPushMain =
  process.env.GITHUB_EVENT_NAME === "push" && process.env.GITHUB_REF_NAME === "main";
// 硬失败只对**门禁自己的 workflow** 开（verify.yml 显式声明 GOSSLAN_BUDGET_STRICT=1）。
// 其它消费方（build-android-releases.sh 的守卫清单、发布脚本）只是顺带跑一下这道门禁，
// 让它们因为"拿不到范围"而打断出包 = 将门禁的作用域扩到它管不着的地方。
const strict = process.env.GOSSLAN_BUDGET_STRICT === "1";

if (ok && strict && ciPushMain && !rangeFromEventBefore) {
  // CI 上 push 到 main，唯一**正确**的范围是 `github.event.before..github.sha`。走到这里
  // 说明它没拿到（env 没映射 / before 不可达 / 候选全空）⇒ 这一步看的不是本次推送的内容。
  // 必须红，而且要红得能被 `ci-run.sh` 转成注解（匿名可读渠道）—— 否则"CI 全绿"会被当成
  // "Change Budget 判过这次推送"，而它可能一个 commit 都没看过。
  console.error(
    `\n✗ CI push→main 的受检范围不是事件里的 before..sha` +
      `（范围 ${usedRange}，来源：${rangeSource ?? "未知"}）⇒ 门禁看的不是本次推送（空转）。\n` +
      `  先查 verify.yml 顶部那段 env：GITHUB_EVENT_BEFORE: \${{ github.event.before }}`,
  );
  process.exit(1);
}

if (ok && zeroCoverage) {
  console.log(`  受检范围：${usedRange ?? "（fixture）"} ｜ 判到的 commit：0`);
  console.log("\n⚠️ Change Budget **零覆盖** —— 跑了，但没有任何 commit 可判（退出码 2 ≠ 通过 0）。");
  console.log("  本地最常见的成因：这条提交已经 push 过（origin/main..HEAD 为空）。");
  console.log("  想真判一批：`node scripts/check-change-budget.mjs --range a1b2c3..HEAD`。");
  process.exit(2);
}

if (ok) {
  console.log("\n✓ Change Budget 通过。");
  console.log(
    "  (已知边界:管不了语义(+10 行能让链路发不出消息,靠测试)/拆 commit gaming 靠犯案判据兜底/" +
      "不写 Version-Bump trailer 且不动版本文件的提交,犯案窗口看不见。)",
  );
  process.exit(0);
}
console.error("\n✗ Change Budget 未通过 —— 请补声明([plan]/[impact])或收敛改动,别绕过。");
process.exit(1);
