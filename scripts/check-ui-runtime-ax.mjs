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
 * ★ 唯一一条不能凭直觉写的机制（2026-09-28 实测，见 docs/stability-roadmap.md §12.6.3）：
 *   **第一次读永远只有两个节点**（`AXWindow` + 一个空 `AXGroup`，零个 `AXWebArea`），
 *   同一次进程上**第二次读**就拿到整棵树。⇒ 判据必须"读到出现 AXWebArea 为止"，
 *   而**不能**把"读不到"当成产品缺陷 —— 上一轮就是拿第一次的读数写下了"Tauri 能不能读到内容树仍未证实"。
 *
 * 用法：
 *   node scripts/check-ui-runtime-ax.mjs                # 起一个隔离实例、验完自己停掉并删掉它建的库
 *   node scripts/check-ui-runtime-ax.mjs --pid 12345    # 只读一个已在跑的进程（判据照判 ⇒ 这也是非空转的反证入口：
                                                           #   指到一个没有 WebView 的进程就必须红）
 *   node scripts/check-ui-runtime-ax.mjs --instance 12  # 换多开编号（默认 41，避开 harness 用的 1/2/3）
 *
 * 退出码：0 = 通过；1 = 判据红；2 = 环境不具备（**不是通过**，且会打印缺哪一条）。
 */
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

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

if (process.platform !== "darwin") {
  console.error(`✗ 本平台没有系统无障碍这条路可走（${process.platform}）⇒ 环境不具备，不是通过`);
  process.exit(2);
}
function have(cmd, args) {
  try { execFileSync(cmd, args, { stdio: "ignore" }); return true; } catch { return false; }
}
// 辅助访问授权探针：回 false 时**什么都读不到**，而这看起来完全像"产品没暴露树"。
let axOn = "";
try { axOn = execFileSync("osascript", ["-e", 'tell application "System Events" to UI elements enabled'], { encoding: "utf8" }).trim(); }
catch { axOn = ""; }
if (axOn !== "true") {
  console.error("✗ 跑测试的进程没有『辅助访问』权限（System Events 报 " + (axOn || "取不到") + "）⇒ 环境不具备，不是通过。\n"
    + "  系统设置 → 隐私与安全性 → 辅助功能：勾上终端/agent 再跑。");
  process.exit(2);
}
const BIN = path.join(ROOT, "src-tauri", "target", "release", "gosslan");
if (!existsSync(BIN)) {
  console.error(`✗ 没有 release 二进制 ${BIN} ⇒ 先 (cd src-tauri && cargo build --release --features bluetooth)`);
  process.exit(2);
}
if (!have("swiftc", ["--version"])) {
  console.error("✗ 没有 swiftc（Xcode CLT）⇒ 环境不具备，不是通过");
  process.exit(2);
}

// 遍历器：仓库里那份 axwalk.swift 编到临时目录（不污染 target/）
const tmp = mkdtempSync(path.join(os.tmpdir(), "gosslan-ax-"));
const helper = path.join(tmp, "axwalk");
try {
  execFileSync("swiftc", ["-O", path.join(ROOT, "scripts", "axwalk.swift"), "-o", helper], { stdio: "pipe" });
} catch (e) {
  console.error("✗ 编不出遍历器：\n" + (e.stderr?.toString?.() ?? e.message));
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

function dump() {
  try {
    return execFileSync(helper, [String(pid)], { encoding: "utf8", maxBuffer: 32 << 20, timeout: 60_000 });
  } catch (e) {
    return (e.stdout ?? "") + "\n(exec 失败: " + e.message + ")";
  }
}

/** ★ 关键形状：**读到 AXWebArea 为止**，而不是"读一次然后下结论"。 */
function readWebAreaTree(maxTries = 8, gapMs = 2000) {
  let last = "";
  for (let i = 1; i <= maxTries; i += 1) {
    last = dump();
    if (/AXWebArea=\d*[1-9]/.test(last) || last.includes("AXWebArea/")) {
      return { text: last, tries: i };
    }
    if (i === 1) {
      const t = last.match(/遍历到的元素总数=(\d+)/);
      note(`第 1 次读只有 ${t ? t[1] : "?"} 个节点、零个 AXWebArea —— 这是**已知的懒建形状**，不是产品缺陷`);
    }
    execFileSync("sleep", [String(gapMs / 1000)]);
  }
  return { text: last, tries: maxTries };
}

console.log(`\n=== 真 WebView 控件树验收（pid ${pid}）===`);
const { text, tries } = readWebAreaTree();
writeFileSync(path.join(tmp, "dump.txt"), text);
const histLine = (text.match(/角色直方图：(.*)/) || [, ""])[1];
const total = Number((text.match(/遍历到的元素总数=(\d+)/) || [, "0"])[1]);
const count = (role) => Number((histLine.match(new RegExp(`${role}=(\\d+)`)) || [, "0"])[1]);

// 判据 1：内容树真的挂上了（路由 B 的前提本身）
if (count("AXWebArea") >= 1) ok(`内容树挂上了：AXWebArea=${count("AXWebArea")}（第 ${tries} 次读拿到）`);
else bad(`读了 ${tries} 次仍没有 AXWebArea ⇒ 真 WebView 的内容没暴露成控件树（或遍历器坏了：窗口标题那一行是 ${JSON.stringify((text.match(/── window\[0\] title="([^"]*)"/) || [, "?"])[1])}）`);

// 判据 2：不是"只读到菜单栏"那种假绿（上一轮就是这么误判的）
if (total >= 15) ok(`节点总数 ${total} ≥ 15（不是只摸到窗口壳子）`);
else bad(`节点总数只有 ${total} ⇒ 内容树没展开，别当成"界面是空的"`);

// 判据 3：可访问名字真的到了控件上（读屏用户能读到的就是这一列；#88 那一族死在这条上）
const named = [...text.matchAll(/\[([A-Za-z]+)\/[^\]]*\] desc="([^"]{2,})"/g)].filter((m) => m[2] !== "Gosslan");
if (named.length >= 5) ok(`带可访问名字的元素 ${named.length} 个（例：${named.slice(0, 3).map((m) => m[2]).join(" / ")}）`);
else bad(`带 desc 的元素只有 ${named.length} 个 ⇒ 界面没有可读出来的名字，读屏用户等于空白`);

// 判据 4：至少有一个**可操作**控件被暴露（按钮/输入框），否则"能点"这件事仍没判到
if (count("AXButton") + count("AXTextField") >= 5) ok(`可操作控件 ${count("AXButton")} 按钮 + ${count("AXTextField")} 输入框`);
else bad(`可操作控件太少（按钮 ${count("AXButton")} / 输入框 ${count("AXTextField")}）`);

// 收尾：只停**自己起的**那个实例、只删**自己建的**那个库
if (child) {
  try { process.kill(-pid, "SIGTERM"); } catch { /* 已经退了 */ }
  execFileSync("sleep", ["2"]);
  note(`已停自己起的实例 ${pid}`);
  for (const suffix of ["", "-shm", "-wal"]) {
    const p = dbPath + suffix;
    if (!dbPreExisted && existsSync(p)) { rmSync(p, { force: true }); }
  }
  note(dbPreExisted ? `实例编号 ${INSTANCE} 的库本来就存在 ⇒ 一字未动` : `删掉本次新建的 gosslan-${INSTANCE}.db*`);
}
rmSync(tmp, { recursive: true, force: true });

console.log(`\n原始 dump 里的角色直方图：${histLine || "（空）"}`);
console.log(`⚠️ 边界三条（别把这次绿读成"UI 全覆盖"）：① 只有 macOS 这一条腿，Windows/Linux/移动端仍归人工；`
  + `② 判据只看"有没有挂上树 + 有没有可访问名字"，**不判界面对不对**；`
  + `③ 需要人类 GUI 会话与辅助访问权限 ⇒ 有意不进任何门禁层（要跑它：npm run verify:ax）。`);
if (fails.length) {
  console.error(`\n✗ ${fails.length} 条判据红（备注 ${notes.length} 条）`);
  process.exit(1);
}
console.log(`\n✓ 真 WebView 内容树验收通过（读了 ${tries} 次挂上；备注 ${notes.length} 条）`);
