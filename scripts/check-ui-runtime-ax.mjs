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
 *
 * 退出码：0 = 通过；1 = 判据红；2 = 环境不具备（**不是通过**，且会打印缺哪一条）。
 */
import { execFileSync, spawn } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
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

let pid = Number(TARGET_PID);
let child = null;
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
