#!/usr/bin/env node
/**
 * `*-selfproof` 档的**红数基线**核对 —— 补一段谁都没守的沉默失败。
 *
 * 为什么需要它（前提核实过，不是重复造轮子）：每个反证档"该红几条"只写在验收矩阵的文字里。
 * `check-doc-numbers` 的判据 F 只做**集合互点**（`-lie` 档 ↔ npm 入口，双向），它不跑任何东西、
 * 也从不看红数 ⇒ "这一档今天还会不会红"没有机器守卫。而判据 F 自己的注释正好写着它管不到的
 * 那个形状：死入口"下一次跑反证会跑成一趟正向轮并打印全绿" —— 那就是本工具要抓的静默变空。
 *
 * ★ 档位分母由本文件**自己现数并打印**（口径：`package.json` 里脚本名以 `selfproof` 结尾的入口）。
 *   写这份工具的第一版用的过滤是 `endsWith("-selfproof")`，漏掉了 `test:fault-injection:selfproof`
 *   （冒号而不是连字符）—— 一个真反证档就这样不在基线里而没人报红。**别再收窄检索条件。**
 *
 * 它判什么（三件一起比，缺一件都可能被糊过去）：
 *   ① 退出码（0 / 非 0 反了就是坏）
 *   ② 报红条数（文档里那句"恰好 X 条红"的机器版 —— 本工具的核心）
 *   ③ 该轮断言/自查格总数（悄悄少跑一条也会被抓住）
 *
 * 每档"该红还是该绿"**不写进基线**，由 npm 命令串现推（只有两个来源，就不会互相不一致）：
 *   `--round=X-lie` / `--fault=X-lie` → 反证档，**必须红**（red > 0）
 *   `--*-selfcheck`                   → 判据自查档，**必须绿**（它注入的是假夹具，
 *                                       绿 = "判据真的把假的判成了假"）
 *   两种都不像 → 直接红着退出：口径认不出来的档不许被默认判成任何一种。
 *
 * 非空转口径：基线文件里没有任何"人工保证"字段 ⇒ 新加一档不需要记得改口径，
 * 但**必须**被 `--sync` 真跑过一次才能进基线；`--sync` 若发现某个反证档本轮红 0 条
 * （或自查档反而红了），**拒绝写基线**并退 1 —— 把洞烘进基线比没有基线更坏。
 *
 * 为什么不接进门禁层（写清楚，别当偷懒）：跑一遍 = 把 20 档各起一次真进程 E2E，约 30–45 分钟；
 * 而 `verify:release` 现在只有 3 步 / 约 70 秒。塞进发版前层会让每次发版多等半小时，
 * 那反而会让这一步被跳过 —— 所以它是**量具**：改了判据 / 夹具 / harness 之后手动跑一次。
 *
 * 用法：
 *   `node scripts/check-selfproof-baseline.mjs`            核对（默认）
 *   `node scripts/check-selfproof-baseline.mjs --sync`     重新生成基线（全档跑齐才写）
 *   `node scripts/check-selfproof-baseline.mjs --only=k`   只跑一档（配 `--sync` 是**合并**，
 *                                                          合并后仍要求基线覆盖全部档 ⇒ 不许残缺）
 */

import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const BASELINE = path.join(ROOT, "scripts", "fixtures", "selfproof-baseline.json");
const pkg = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8"));

const argv = process.argv.slice(2);
const SYNC = argv.includes("--sync");
const only = (argv.find((a) => a.startsWith("--only=")) || "").slice(7);
/// 递归闸：本工具用 `npm run <档>` 起子进程，而子进程继承环境变量。
/// 将来谁把一档的名字写成 `xxx-selfproof` 而命令又指回本工具，就会 20 档 × 自身体积地炸开 ——
/// 那种形状宁可当场报错，也不要它跑到机器没响应才发现。
if (process.env.GOSSLAN_SELFPROPFORK === "1") {
  console.error("✗ 检测到自我嵌套调用（GOSSLAN_SELFPROPFORK=1 已在环境里）"
    + " —— 说明某个 `*selfproof` 入口又指回了本工具；命名请避开这个后缀（现成的 `selfproof:check` 就是这么来的）。");
  process.exit(1);
}
process.env.GOSSLAN_SELFPROPFORK = "1";

/** 所有 selfproof 档。**后缀判据用 `selfproof` 而不是 `-selfproof`**（见文件头那条实测教训）。 */
const ALL = Object.keys(pkg.scripts)
  .filter((k) => k.endsWith("selfproof"))
  .sort();
const ENTRIES = ALL.filter((k) => !only || k === only || k.endsWith(only));

if (ALL.length === 0) {
  console.error("✗ `package.json` 里一个 `*selfproof` 入口都没匹配到 —— 这个工具此刻什么都没判");
  process.exit(1);
}
if (ENTRIES.length === 0) {
  console.error(`✗ --only=${only} 没匹配到任何档（全量共 ${ALL.length} 档）—— 先怀疑过滤条件`);
  process.exit(1);
}

/** 由命令串推口径：这一档按设计必须红还是必须绿。认不出来就判工具坏。 */
function classify(name) {
  const cmd = pkg.scripts[name];
  const isLie = /-lie\b/.test(cmd);
  const isSelfCheck = /--\S*selfcheck\b/.test(cmd);
  if (isLie === isSelfCheck) {
    return {
      error: `${name} 的口径推不出来（命令：${cmd}）—— 反证档的命令要含 \`-lie\`、`
        + `判据自查档要含 \`--*-selfcheck\`，两者都不像就不许默认按任何一种记账`,
    };
  }
  return { kind: isLie ? "lie" : "selfcheck", mustRed: isLie };
}

const classes = {};
{
  const errs = [];
  for (const k of ALL) {
    const c = classify(k);
    if (c.error) errs.push(`  ✗ ${c.error}`);
    else classes[k] = c;
  }
  const nLie = Object.values(classes).filter((c) => c.kind === "lie").length;
  const nChk = Object.values(classes).filter((c) => c.kind === "selfcheck").length;
  console.log(`· 档位分母：${ALL.length} 档 = 反证 ${nLie} + 判据自查 ${nChk}（现数自 package.json）`);
  if (only) console.log(`· --only=${only} → 本趟只跑 ${ENTRIES.length} 档`);
  if (errs.length) {
    for (const e of errs) console.error(e);
    process.exit(1);
  }
}

// ★ 起跑前的拦条：每一步都要起真进程 E2E，放在跑完之后等于让人先等 40 分钟再告他"这次不该开始"。
if (!SYNC && !existsSync(BASELINE)) {
  console.error("✗ 缺基线文件 scripts/fixtures/selfproof-baseline.json —— 生成："
    + "node scripts/check-selfproof-baseline.mjs --sync");
  process.exit(1);
}
/// 基线的**覆盖集合**必须和 package.json 的档位集合一模一样：
/// 少一档 ⇒ 那档从此没人核对；多一档 ⇒ 那入口已被删而基线还替它记账。两种都是静默失效。
let prevEntries = {};
if (existsSync(BASELINE)) prevEntries = JSON.parse(readFileSync(BASELINE, "utf8")).entries ?? {};
if (!SYNC) {
  const missing = ALL.filter((k) => !prevEntries[k]);
  const stale = Object.keys(prevEntries).filter((k) => !ALL.includes(k));
  if (missing.length || stale.length) {
    if (missing.length) console.error(`  ✗ 基线缺这 ${missing.length} 档：${missing.join("，")}`);
    if (stale.length) console.error(`  ✗ 基线多这 ${stale.length} 档（入口已不在 package.json）：${stale.join("，")}`);
    console.error("✗ 核对模式什么都不跑 —— 先补齐/删掉再跑（补：--only=<档> --sync 单档合并）");
    process.exit(1);
  }
}

/**
 * 从一次运行的输出里读「报红条数 / 总数 / 单位」。读不出来就是 bug，绝不当 0 处理。
 * 四种结论行（都是各档**自己打印的原文**，不是本工具造的）：
 *   `✗ N/M 条断言报红` / `✅ …全绿（M 条断言）`   → 一轮 E2E 的断言级结论
 *   `✗ …自证红 N 条` / `✅ …自证…T 格…`          → 判据自查档的格级结论
 */
function parseVerdict(out) {
  const red = /✗ (\d+)\/(\d+) 条断言报红/.exec(out);
  if (red) return { red: Number(red[1]), total: Number(red[2]), unit: "条断言", green: false };
  const green = /全绿（(\d+) 条断言）/.exec(out);
  if (green) return { red: 0, total: Number(green[1]), unit: "条断言", green: true };
  const chkRed = /✗ [^\n]*?自证红 (\d+) 条/.exec(out);
  if (chkRed) return { red: Number(chkRed[1]), total: null, unit: "格", green: false };
  const chkGreen = /✅ [^\n]*?自证[^\n]*/.exec(out);
  if (chkGreen) {
    const cells = /(\d+) 格/.exec(chkGreen[0]);
    // `--shot-selfcheck` 的结论行里不印格数（它印的是"能判假 / 能判真"两态，且随环境变），
    // 所以 total 允许 null —— 但**退码仍被核对**，判据真坏了照样红。
    return { red: 0, total: cells ? Number(cells[1]) : null, unit: "格", green: true };
  }
  return null;
}

function runEntry(name) {
  const r = spawnSync("npm", ["run", name], { cwd: ROOT, encoding: "utf8", maxBuffer: 64 << 20 });
  const out = `${r.stdout || ""}\n${r.stderr || ""}`;
  const v = parseVerdict(out);
  if (!v) {
    console.error(`  ✗ ${name}：读不出结论行（四种已知形状都没命中）—— 退码 ${r.status}。`
      + `不猜，直接判工具坏。`);
    return { name, exit: r.status ?? 1, red: null, total: null, unit: null, parseFail: true };
  }
  return { name, exit: r.status ?? 1, red: v.red, total: v.total, unit: v.unit, green: v.green };
}

const observed = [];
for (const name of ENTRIES) {
  process.stderr.write(`· 跑 ${name} …\n`);
  const o = runEntry(name);
  observed.push(o);
  if (!o.parseFail) {
    const cls = classes[name];
    console.log(`  ${name}  [${cls.kind === "lie" ? "反证：必须红" : "自查：必须绿"}]`
      + `  退码 ${o.exit}  红 ${o.red} 条 / 共 ${o.total ?? "未印"} ${o.unit}`);
  }
}

/** 该档的观测值是否**符合它自己的口径**（与基线无关，只看这一轮跑出来的事实）。 */
function consistency(o) {
  const cls = classes[o.name];
  if (o.parseFail) return "读不出结论行";
  if (cls.kind === "lie" && o.red === 0) return "反证档红 0 条 —— 那个坏样子已经不生效了";
  if (cls.kind === "selfcheck" && (o.red !== 0 || o.exit !== 0)) {
    return `判据自查档不绿（红 ${o.red} 条 / 退码 ${o.exit}）—— 判据本身坏了`;
  }
  return null;
}

if (SYNC) {
  const broken = observed.map(consistency).map((c, i) => (c ? { o: observed[i], c } : null)).filter(Boolean);
  if (broken.length) {
    console.error(`\n✗ 有 ${broken.length} 档与本趟跑出来的事实不一致 —— **基线不写**（把洞烘进基线比没有基线更坏）：`);
    for (const b of broken) console.error(`  · ${b.o.name}：${b.c}`);
    console.error("  先查它为什么变成这样，再决定是**改判据**还是**改口径**。别 --sync 覆盖掉这条信息。");
    process.exit(1);
  }
  const entries = { ...prevEntries };
  for (const o of observed) entries[o.name] = { exit: o.exit, red: o.red, total: o.total, unit: o.unit };
  const ordered = {};
  for (const k of ALL) if (entries[k]) ordered[k] = entries[k];
  if (Object.keys(ordered).length !== ALL.length) {
    console.error(`\n✗ 合并后基线只覆盖 ${Object.keys(ordered).length}/${ALL.length} 档`
      + `（本趟跑的是 --only 选出的 ${ENTRIES.length} 档）—— 基线必须**全档齐**才写，`
      + `残缺基线等于给剩下的档开了"没人核对"的口子。补齐缺的那些再 --sync。`);
    process.exit(1);
  }
  const body = {
    _comment: "check-selfproof-baseline.mjs 的基线：每个 *selfproof 档的「退出码 / 报红条数 / 结论总数 / 单位」。"
      + "跑法：`node scripts/check-selfproof-baseline.mjs`（核对）；改了判据/夹具/harness 后重新生成用 `--sync`。"
      + "★ 这里刻意**不存** expect_red：每档该红还是该绿由 package.json 的命令串现推"
      + "（`-lie` = 必须红，`--*-selfcheck` = 必须绿），两个来源就会互相不一致。",
    generated_from_tip: spawnSync("git", ["rev-parse", "--short", "HEAD"], { cwd: ROOT, encoding: "utf8" }).stdout.trim(),
    entries: ordered,
  };
  writeFileSync(BASELINE, `${JSON.stringify(body, null, 2)}\n`);
  console.log(`\n✓ 已写基线 ${path.relative(ROOT, BASELINE)}（${ALL.length} 档，本趟更新 ${observed.length} 档）`);
  process.exit(0);
}

// ── 核对模式 ────────────────────────────────────────────────
let bad = 0;
for (const o of observed) {
  const b = prevEntries[o.name];
  if (o.parseFail) { bad += 1; continue; }
  const c = consistency(o);
  if (c) { console.error(`  ✗ ${o.name}：${c}`); bad += 1; continue; }
  const diffs = [];
  if (b.exit !== o.exit) diffs.push(`退码 ${b.exit}→${o.exit}`);
  if (b.total !== o.total) diffs.push(`结论总数 ${b.total}→${o.total}`);
  if (b.red !== o.red) diffs.push(`红数 ${b.red}→${o.red}`);
  if (diffs.length) {
    console.error(`  ✗ ${o.name}：${diffs.join("，")} —— 基线是 ${b.red}/${b.total} ${b.unit ?? ""}`
      + `；红数变 0 ⇒ 那个坏样子已不再生效，红数变多或总数变化 ⇒ 覆盖面动了（要连文档一起改）`);
    bad += 1;
  } else {
    console.log(`  ✓ ${o.name}：退码 ${o.exit}、红 ${o.red}/${o.total ?? "未印"} 与基线一致`);
  }
}
if (bad) {
  console.error(`\n✗ ${bad}/${observed.length} 档与基线不一致 —— "该红几条"是契约，不是观感。`);
  process.exit(1);
}
console.log(`\n✓ ${observed.length}/${ALL.length} 档的红数 / 退码 / 结论总数全部与基线一致`);
process.exit(0);
