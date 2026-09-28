#!/usr/bin/env node
/**
 * CI 巡检（只读）—— 补一段"推完没人能自己看"的缺口。
 *
 * 为什么需要它（前提实测过，不是猜）：
 *   ① `actions/jobs/{id}/logs` 匿名 **403**（"Must have admin rights"）；
 *   ② 浏览器打开 job 日志页要登录（公开仓库也要）；
 *   ③ **能匿名读**的是 `check-runs/{id}/annotations` —— 而 `scripts/ci-run.sh` 正是把失败诊断
 *      合成成注解写进去的那一步（它文件头把这三条渠道都记着）。
 * ⇒ 所以这个工具判"跑没跑、成没成"用 runs 列表，判"为什么红"用注解，**不去碰那两条读不到的日志**。
 *
 * ★ 本工具的立身原则：**看不见就明说看不见，绝不打印"CI 正常"**。
 *   匿名额度是 60 次/小时/IP（本机实测撞过 403 rate limit）；拿不到数据时退 2 并给恢复时间，
 *   而不是给一个空表让人读成"没有失败的运行"。
 *
 * 用法：
 *   node scripts/ci-status.mjs                    最近 3 小时内每个 workflow 的最新运行
 *   node scripts/ci-status.mjs --limit=30         看最近 30 条运行
 *   node scripts/ci-status.mjs --sha=<sha>        只判这一次提交有没有被 CI 覆盖（跑绿才算过）
 *   node scripts/ci-status.mjs --fail <runId>     把某次失败运行的逐 job 注解打出来
 *   GITHUB_TOKEN=ghp_… node scripts/ci-status.mjs  带令牌（额度 5000/小时，日志仍是 403）
 *
 * 退出码：0 = 看得到且没有失败；1 = 有失败（注解已打）；2 = 看不见（限流 / 网络 / 私有仓库没令牌）。
 */

import { execFileSync } from "node:child_process";
import process from "node:process";

const arg = (name, dflt) => {
  const hit = process.argv.find((a) => a.startsWith(`--${name}=`));
  if (hit) return hit.slice(name.length + 3);
  const flag = process.argv.indexOf(`--${name}`);
  return flag >= 0 && process.argv[flag + 1] && !process.argv[flag + 1].startsWith("--")
    ? process.argv[flag + 1]
    : dflt;
};

/** 远端地址 → `owner/repo`（SSH 与 HTTPS 两种写法都要认，本机用的是 SSH）。 */
function repoFromRemote() {
  const url = execFileSync("git", ["remote", "get-url", "origin"], { encoding: "utf8" }).trim();
  const m = /[:/]([^/:]+\/[^/]+?)(?:\.git)?$/.exec(url);
  if (!m) throw new Error(`认不出 owner/repo：${url}`);
  return m[1];
}

const repo = arg("repo", repoFromRemote());
const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN || "";
const base = `https://api.github.com/repos/${repo}`;
const headers = {
  Accept: "application/vnd.github+json",
  "X-GitHub-Api-Version": "2022-11-28",
  ...(token ? { Authorization: `Bearer ${token}` } : {}),
};

async function api(path) {
  const r = await fetch(base + path, { headers });
  const body = await r.json().catch(() => ({}));
  return { ok: r.ok, status: r.status, body, headers: Object.fromEntries(r.headers) };
}

/** 限流 / 需要权限 都要单独说清楚：这一族的"空结果"永远不等于"没有问题"。 */
function explainMiss(res, what) {
  const msg = res.body?.message ?? "";
  const reset = res.headers["x-ratelimit-reset"];
  const remaining = res.headers["x-ratelimit-remaining"];
  console.error(`✗ 看不见${what}：HTTP ${res.status} ${msg || "(无消息)"}`);
  if (res.status === 403 || res.status === 401) {
    if (reset && remaining === "0") {
      const mins = Math.max(0, Math.round((Number(reset) * 1000 - Date.now()) / 60000));
      console.error(`  ⇒ 匿名额度用完（60 次/小时/IP），约 ${mins} 分钟后恢复；`
        + `想立刻看就带令牌：GITHUB_TOKEN=<只读、fine-grained 就够> node scripts/ci-status.mjs`);
    } else {
      console.error("  ⇒ 403 但不是限流的形状：多半是**仓库私有**而这次没带令牌。"
        + "本工具读不到日志（那条渠道按设计就是 403），只能靠 check-run 注解。");
    }
  }
  process.exit(2);
}

if (process.argv.includes("--fail")) {
  const id = process.argv[process.argv.indexOf("--fail") + 1];
  if (!id || id.startsWith("--")) {
    console.error("用法：node scripts/ci-status.mjs --fail <runId>");
    process.exit(2);
  }
  const jobs = await api(`/actions/runs/${id}/jobs?per_page=100`);
  if (!jobs.ok) explainMiss(jobs, `运行 ${id} 的 job 列表`);
  let printed = 0;
  for (const job of jobs.body.jobs ?? []) {
    if (job.conclusion === "success" || job.conclusion === null) continue;
    console.log(`\n✗ job「${job.name}」${job.conclusion}  check-run id=${job.id}`);
    const ann = await api(`/check-runs/${job.id}/annotations`);
    if (!ann.ok) {
      console.error(`  （注解读不到：HTTP ${ann.status} —— 这一步是 ci-run.sh 写的，`
        + `读不到就说明那次失败发生在 ci-run.sh 之外）`);
      continue;
    }
    const list = ann.body.annotations ?? [];
    if (!list.length) console.log("  （这次没有注解 ⇒ 失败发生在 ci-run.sh 包的步骤之外，只能登录网页看）");
    for (const a of list) {
      console.log(`  · ${a.path}:${a.start_line} [${a.annotation_level}] ${a.message}`);
      printed += 1;
    }
  }
  if (!printed) console.log("\n没拿到任何注解。两种原因：那次失败不在 ci-run.sh 包的步骤里，或仓库私有又没带令牌。");
  process.exit(jobs.body.jobs?.some((j) => j.conclusion === "failure") ? 1 : 0);
}

const limit = Number(arg("limit", "50"));
const sha = arg("sha", "");
const withinHours = Number(arg("within", "3"));

const runs = await api(`/actions/runs?per_page=${limit}`);
if (!runs.ok) explainMiss(runs, " workflow 运行列表");
const all = runs.body.workflow_runs ?? [];
if (!all.length) {
  console.error("✗ 一条运行都没看到 —— 这不是「CI 干净」，这是**没判**（仓库没有 workflow？还是过滤器太窄？）");
  process.exit(2);
}

const since = Date.now() - withinHours * 3600_000;
const scope = sha ? all.filter((r) => r.head_sha.startsWith(sha) || sha.startsWith(r.head_sha)) : all.filter((r) => Date.parse(r.created_at) >= since);
if (!scope.length) {
  if (sha) {
    console.error(`✗ 提交 ${sha} 在 CI 里一条运行都没有 —— 那不等于"跑过了"，等于**没触发**`
      + `（触发条件不覆盖这次事件？还是刚推上去还没排队？稍后复跑本命令）`);
    process.exit(2);
  }
  console.error(`✗ 最近 ${withinHours} 小时没有任何运行 —— 同样不是好消息：默认分支这段时间没人推过东西？`);
  process.exit(2);
}

// 同一个 workflow + 同一个 ref 只留最新一条，避免把重跑读成"多次失败"
const latest = new Map();
for (const r of scope) {
  const key = `${r.name}::${r.head_branch}::${r.event}`;
  const prev = latest.get(key);
  if (!prev || Date.parse(r.created_at) > Date.parse(prev.created_at)) latest.set(key, r);
}
const rows = [...latest.values()].sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
const bad = rows.filter((r) => r.conclusion === "failure" || r.conclusion === "cancelled");
const busy = rows.filter((r) => r.status !== "completed");

console.log(`· ${repo} —— 看到 ${rows.length} 组（同 workflow+分支+事件 只留最新），`
  + `数据源：/actions/runs 匿名可读${token ? "（带令牌）" : ""}`);
for (const r of rows) {
  const mark = r.conclusion === "success" ? "✅" : r.conclusion === "failure" ? "❌" : r.conclusion ? "⚠️" : "⏳";
  console.log(`  ${mark} ${r.name.padEnd(46)} ${r.status}/${r.conclusion ?? "-"}  `
    + `${r.head_branch} ${r.event} ${r.created_at.slice(5, 16)}  run=${r.id}`);
}
if (busy.length) console.log(`\n还在跑 ${busy.length} 组 —— 「没红」不等于「绿」，稍后复跑本命令再判。`);
if (bad.length) {
  console.log(`\n✗ ${bad.length} 组失败。诊断用注解（ci-run.sh 写的），逐条拉：`);
  for (const r of bad) console.log(`    node scripts/ci-status.mjs --fail ${r.id}`);
  process.exit(1);
}
if (busy.length) process.exit(2);
console.log(`\n✓ 覆盖到的 ${rows.length} 组全部成功（这只说明**看到的那些**成功）`);
process.exit(0);
