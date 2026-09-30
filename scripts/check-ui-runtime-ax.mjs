#!/usr/bin/env node
/**
 * 真 WebView（WKWebView）运行时验收入口 —— macOS 系统无障碍控件树那一层（#78 拍板走的那条"路由 B"）。
 *
 * 它判的是**别的所有层都判不到**的那一格：界面在**真实进程的真实 WebView** 里到底长出了什么、
 * 读屏用户实际能读到的**可访问名字**是什么。前几层的分工：
 *   · `*.test.ts` —— 纯函数与形状判据（没有界面）；
 *   · `check-ui-runtime.mjs`（CDP）—— 浏览器运行时里的渲染层（是真 DOM，但**不是发出去那个 WebView**）；
 *   · 双实例 E2E —— 线级/库级事实（日志与 SQLite，不看像素）。
 *
 * ★ 为什么**不进门禁**（结论，不是遗漏）：要辅助访问权限、要有人类 GUI 登录会话、只有 macOS 一条腿 ⇒
 *   进任何一层都会变成"换台机器就常红"。它按 `npm run verify:ax` 手动跑，验收矩阵里那一格标 MANUAL-LOCAL。
 *
 * ★ 编译 + 解析 + "重试到挂上为止"都在 `scripts/ax-tree.mjs`：**两个消费者共用一份解析器**
 *   （这里，以及 E2E harness 在"对端刚收下那条 @"的同步点上的可选采样 `GOSSLAN_AX=1`）。
 *   各写一遍就是造第二个事实源 —— 而上一轮"第一次读只有 2 个节点"这条机制，正因为量具只在我一个人手里，
 *   才被误读成"产品读不到内容树"。
 *
 * 用法：
 *   node scripts/check-ui-runtime-ax.mjs                # 起一个隔离实例、验完自己停掉并删掉它建的库
 *   node scripts/check-ui-runtime-ax.mjs --pid 12345    # 只读一个已在跑的进程（判据照判 ⇒ 这也是非空转的
 *                                                       #   反证入口：指到一个没有 WebView 的进程必须红）
 *   node scripts/check-ui-runtime-ax.mjs --instance 12  # 换多开编号（默认 41，避开 harness 用的 1/2/3）
 *   node scripts/check-ui-runtime-ax.mjs --fixture=todo  # 读数前置：先给这个隔离实例写一条生产形状的
 *                                                       #   群任务行，再读真窗口里那颗编号角标。
 *                                                       #   ★ 这是**读数**，不判红、不改退码判据
 *
 * 退出码：0 = 通过；1 = 判据红；2 = 环境不具备（**不是通过**，且会打印缺哪一条）。
 */
import { execFileSync, spawn } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";
import { DatabaseSync } from "node:sqlite";
import { FIXTURES, TODO_FIXTURE, seedTodoFixture } from "./axTodoFixture.mjs";
import os from "node:os";
import path from "node:path";
import { axEnvironmentReady, probeTree } from "./ax-tree.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const flag = (name, dflt) => {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 ? process.argv[i + 1] : dflt;
};
const INSTANCE = Number(flag("instance", "41"));
const TARGET_PID = flag("pid", null);
const FIXTURE = flag("fixture", null);
// ★ 只认 `--fixture todo` 这种空格写法：`--fixture=todo` 会被上面的 flag() 整个忽略 ⇒
//   使用者以为拿到了夹具读数、其实跑的是空库。这里宁可拒跑，也不给一份"看着像"的读数。
const eqForm = process.argv.find((a) => /^--(fixture|pid|instance)=/.test(a));
if (eqForm) {
  console.error(`✗ 参数写法 ${eqForm} 不被解析（本脚本只认空格形式）⇒ 改成 ${eqForm.replace("=", " ")}；`    + " 宁可拒跑，也不给一份被静默忽略的读数");
  process.exit(2);
}
const FIX = FIXTURE === null ? null : FIXTURES[FIXTURE];
if (FIXTURE !== null && !FIX) {
  console.error(`✗ 不认识的 --fixture=${FIXTURE}（可选：${Object.keys(FIXTURES).join(" / ")}）`);
  process.exit(2);
}

const fails = [];
const notes = [];
const ok = (m) => console.log(`  ✅ ${m}`);
const bad = (m) => { fails.push(m); console.log(`  ✗ ${m}`); };
const note = (m) => { notes.push(m); console.log(`  · ${m}`); };

const env = axEnvironmentReady();
if (!env.ready) {
  console.error(`✗ 环境不具备 ⇒ 不是通过：\n  - ${env.problems.join("\n  - ")}`);
  console.error("  辅助访问：系统设置 → 隐私与安全性 → 辅助功能，勾上跑测试的那个进程");
  process.exit(2);
}
const BIN = path.join(ROOT, "src-tauri", "target", "release", "gosslan");
if (!existsSync(BIN)) {
  console.error(`✗ 没有 release 二进制 ${BIN} ⇒ 先 (cd src-tauri && cargo build --release --features bluetooth)`);
  process.exit(2);
}

const appData = path.join(os.homedir(), "Library", "Application Support", "com.gosslan.app");
const dbPath = path.join(appData, `gosslan-${INSTANCE}.db`);
// ★ 只删**这次新建**的库：harness 用 1/2/3，别的编号也可能是用户自己开过的 ⇒ 先记在不在
const dbPreExisted = existsSync(dbPath);
if (TARGET_PID && dbPreExisted) note(`--pid 模式：不碰 ${path.relative(os.homedir(), dbPath)}`);

if (FIXTURE && TARGET_PID) {
  console.error("✗ --fixture 不能和 --pid 同时用：那会往一个不在本脚本管辖里的实例库写行");
  process.exit(2);
}
if (FIXTURE && dbPreExisted) {
  console.error(`✗ --fixture 只允许写**本次新建**的隔离库，而 gosslan-${INSTANCE}.db 本来就存在`    + " ⇒ 换个数（--instance 47）或先确认那不是你的库；这条限制就是为了不碰用户数据");
  process.exit(2);
}
let pid = Number(TARGET_PID);
let child = null;
if (FIX && !TARGET_PID) {
  // 库表是应用第一次启动时建的（迁移在 Rust 侧）⇒ 夹具只能"先起一次让它建表、停下、写行、再起"。
  const boot = spawn(BIN, [], {
    env: { ...process.env, GOSSLAN_INSTANCE: String(INSTANCE) }, stdio: "ignore", detached: true,
  });
  boot.unref();
  note(`夹具前置：先起一次让应用建表（pid ${boot.pid}）`);
  for (let i = 0; i < 30 && !existsSync(dbPath); i += 1) await sleep(1000);
  if (!existsSync(dbPath)) {
    console.error(`✗ 起了 30 s 还没出现 gosslan-${INSTANCE}.db ⇒ 应用没建库，夹具不做第二次猜测`);
    try { process.kill(-boot.pid, "SIGTERM"); } catch { /* 已经没了 */ }
    process.exit(2);
  }
  await sleep(3000);
  try { process.kill(-boot.pid, "SIGTERM"); } catch { /* 已经没了 */ }
  await sleep(2500);
  const db = new DatabaseSync(dbPath);
  try {
    const me = db.prepare("SELECT value FROM settings WHERE key='device_id'").get();
    if (!me) { console.error("✗ 库里没有 device_id ⇒ 这次启动没完成，夹具不写"); process.exit(2); }
    seedTodoFixture(db, me.value, FIX.over);
    note(`夹具已写入：conv=${TODO_FIXTURE.convId} 档=${FIXTURE} `
      + `number=${FIX.over.number ?? TODO_FIXTURE.number} ⇒ 期望：`
      + (FIX.expect
        ? `读到可见码 ${FIX.expect.visible} 与可访问名字 ${FIX.expect.named}`
        : "一条编号读数都不该出现（反面对照档）"));
  } finally { db.close(); }
}
if (!TARGET_PID) {
  child = spawn(BIN, [], {
    // 刻意**不带** GOSSLAN_AUTOSTART：#89 之后"预置说关就不该被 env 顶开"，这里连预置都没有 ⇒ 少一份网络噪音
    env: { ...process.env, GOSSLAN_INSTANCE: String(INSTANCE) },
    stdio: "ignore",
    detached: true,
  });
  child.unref();
  pid = child.pid;
  console.log(`· 起隔离实例 GOSSLAN_INSTANCE=${INSTANCE}（pid ${pid}），只读它的控件树`);
}

console.log(`\n=== 真 WebView 控件树验收（pid ${pid}）===`);
let probe;
try {
  probe = probeTree(pid, {
    onEmpty: (p) =>
      note(`第 1 次读只有 ${p.total} 个节点、零个 AXWebArea（AXWindows rc=${p.winRc}）`
        + " —— 这是**已知的懒建形状**（WebKit 到第二次读才建树），不是产品缺陷"),
  });
} catch (e) {
  console.error(`✗ 读树失败 ⇒ 不是通过：${e.message}`);
  process.exit(2);
}
const { parsed, tries, text } = probe;
// 原始读数留在 tmp 下（**不写进用户的应用数据目录**）：红了要能归因，得看见遍历器到底吐了什么
const dumpPath = path.join(os.tmpdir(), `gosslan-ax-dump-${pid}.txt`);
writeFileSync(dumpPath, text);

if (FIX) {
  // ★ 这一段是**读数**：只打印，不调 ok()/bad() ⇒ 夹具档不改变这个入口的四条判据与退码。
  //   为什么不做成判据：这一屏要的是"读屏用户实际念到什么"，而那条文案（「任务编号 …」）
  //   改了 i18n 就该跟着改期望 ⇒ 进任何一层都会变成"改文案要改门禁"。
  const want = FIX.expect;
  const hitVisible = want ? parsed.names.filter((n) => n.label === want.visible) : [];
  const hitNamed = want ? parsed.names.filter((n) => n.label.includes(want.named)) : [];
  const anyCode = parsed.names.filter((n) => /[RBT]\d{1,6}/.test(n.label));
  if (!want) {
    console.log(`  [fixture] 反面对照档：界面上带编号字样的名字应为 0，实到 ${anyCode.length} 个`
      + (anyCode.length ? ` ⇒ ${anyCode.map((n) => `${n.role}=${n.label}`).join(" ｜ ")}` : " ⇒ 一个都没有"));
  } else {
    console.log(`  [fixture] 读到可见码 ${want.visible}：${hitVisible.length} 个`
      + `（${hitVisible.map((n) => n.role).join("/") || "无"}）；`
      + `读到完整码「${want.named}」：${hitNamed.length} 个（${hitNamed.map((n) => n.role).join("/") || "无"}）`);
  }
  console.log(`  [fixture] 界面上全部带编号字样的名字：`
    + (anyCode.map((n) => `${n.role}=${n.label}`).join(" ｜ ")
      || "（一个都没有 ⇒ 那一格没渲染；带号档里这就是夹具/折叠没到位，无号档里这正是期望）"));
  if (want && (!hitVisible.length || !hitNamed.length)) {
    console.log("  [fixture] ⇒ 两种成因分不开时先看上面那行：整屏没有编号字样 = 夹具/折叠没到位；"
      + "有编号但不是这两个值 = 显示口径变了（那是产品事实，不是这条读数的失败）");
  }
  note("夹具档是读数，**不判红**：上面两行无论是什么，本入口的退码只由那四条判据决定");
}
// 判据 1：内容树真的挂上了（路由 B 的前提本身）
if (parsed.hung) ok(`内容树挂上了：AXWebArea=${parsed.webArea}（第 ${tries} 次读拿到，窗口标题 ${JSON.stringify(parsed.windowTitle)}）`);
else bad(`读了 ${tries} 次仍没有 AXWebArea ⇒ 真 WebView 的内容没暴露成控件树（或遍历器/时机坏了：AXWindows rc=${parsed.winRc}）`);

// 判据 2：不是"只读到窗口壳子"那种假绿（上一轮就是这么误判的）
if (parsed.total >= 15) ok(`节点总数 ${parsed.total} ≥ 15（不是只摸到窗口壳子）`);
else bad(`节点总数只有 ${parsed.total} ⇒ 内容树没展开，别当成「界面是空的」`);

// 判据 3：可访问名字真的到了控件上（读屏用户能读到的就是这一列；#88 那一族死在这条上）
if (parsed.names.length >= 5) {
  ok(`带可访问名字的元素 ${parsed.names.length} 个（例：${parsed.names.slice(0, 3).map((n) => n.label).join(" / ")}）`);
} else bad(`带名字的元素只有 ${parsed.names.length} 个 ⇒ 界面没有可读出来的名字，读屏用户等于空白`);

// 判据 4：至少有一批**可操作**控件被暴露，否则"能点"这件事仍没判到
if (parsed.count("AXButton") + parsed.count("AXTextField") >= 5) {
  ok(`可操作控件 ${parsed.count("AXButton")} 按钮 + ${parsed.count("AXTextField")} 输入框`);
} else bad(`可操作控件太少（按钮 ${parsed.count("AXButton")} / 输入框 ${parsed.count("AXTextField")}）`);

// 收尾：只停**自己起的**那个实例、只删**自己建的**那个库
if (child) {
  try { process.kill(-pid, "SIGTERM"); } catch { /* 已经退了 */ }
  execFileSync("/bin/sleep", ["2"]);
  note(`已停自己起的实例 ${pid}`);
  for (const suffix of ["", "-shm", "-wal"]) {
    const p = dbPath + suffix;
    if (!dbPreExisted && existsSync(p)) { execFileSync("/bin/rm", ["-f", p]); }
  }
  note(dbPreExisted ? `实例编号 ${INSTANCE} 的库本来就存在 ⇒ 一字未动` : `删掉本次新建的 gosslan-${INSTANCE}.db*`);
}

console.log(`\n原始 dump：${dumpPath}`);
console.log(`角色直方图：${parsed.histLine || "（空）"}`);
console.log("⚠️ 边界三条（别把这次绿读成「UI 全覆盖」）：① 只有 macOS 这一条腿，Windows/Linux/移动端仍归人工；"
  + "② 判据只看「有没有挂上树 + 有没有可访问名字」，**不判界面对不对**；"
  + "③ **不判键盘出口**（那一半仍归 Smoke-11 人工）⇒ 有意不进任何门禁层（要跑它：npm run verify:ax）。");
if (fails.length) {
  console.error(`\n✗ ${fails.length} 条判据红（备注 ${notes.length} 条）`);
  process.exit(1);
}
console.log(`\n✓ 真 WebView 内容树验收通过（读了 ${tries} 次挂上；备注 ${notes.length} 条）`);
