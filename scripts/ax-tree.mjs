#!/usr/bin/env node
/**
 * 读 macOS 系统无障碍控件树的**共用件**（编译 + 解析 + "重试到挂上为止"那圈循环）。
 *
 * ★ 为什么单独一份：同一套读数此前有两个消费者 —— `check-ui-runtime-ax.mjs`（自己起实例、判四条）
 *   与 E2E harness（在"对端刚收下那条 @"的同步点上顺手读一次）。
 *   各写一遍解析器就是造第二个事实源：上一轮"第一次读只有 2 个节点"那条机制，
 *   正是因为我只有自己那一份代码、别人按第一次读数下了"产品读不到"的结论才差点被埋掉。
 *
 * 用法（两个都返回**数据**，不判红绿 —— 判不判由调用方定）：
 *   import { probeTree, ensureHelper } from "./ax-tree.mjs";
 *   const r = probeTree(pid);            // r.parsed.webArea / r.parsed.total / r.names / r.tries
 */
import { execFileSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..");
const SRC = path.join(ROOT, "scripts", "axwalk.swift");

let cachedHelper = null;
/** 把仓里那份遍历器编到临时目录（按源码 mtime 复用；**不污染 target/**）。 */
export function ensureHelper() {
  if (cachedHelper && existsSync(cachedHelper)) return cachedHelper;
  const out = path.join(os.tmpdir(), `gosslan-axwalk-${statSync(SRC).mtimeMs.toString(36)}`);
  if (!existsSync(out)) {
    try {
      execFileSync("swiftc", ["-O", SRC, "-o", out], { stdio: "pipe" });
    } catch (e) {
      throw new Error(
        `编不出 AX 遍历器（需要 Xcode CLT 的 swiftc）：${(e.stderr?.toString?.() ?? e.message).split("\n")[0]}`,
      );
    }
  }
  cachedHelper = out;
  return out;
}

/** 解析遍历器那段人读输出 —— 只认它**打印出来的形状**，不猜。 */
export function parseTree(text) {
  const histLine = (text.match(/角色直方图：(.*)/) ?? [, ""])[1];
  const count = (role) => Number((histLine.match(new RegExp(`${role}=(\\d+)`)) ?? [, "0"])[1]);
  const total = Number((text.match(/遍历到的元素总数=(\d+)/) ?? [, "0"])[1]);
  const windowTitle = (text.match(/── window\[0\] title="([^"]*)"/) ?? [, ""])[1];
  const winRc = (text.match(/AXWindows rc=AXError\(rawValue: (-?\d+)\)/) ?? [, "?"])[1];
  // 可访问名字：读屏真正念出来的就是这一列（`desc` 空时退化到 `title`/`val`，与 #88 那条口径一致）
  const names = [];
  for (const m of text.matchAll(/\[([A-Za-z]+)\/[^\]]*\] desc="([^"]*)" val="([^"]*)" title="([^"]*)"/g)) {
    const label = m[2] || m[4] || m[3];
    if (label && label !== "Gosslan" && label !== "相闻") names.push({ role: m[1], label });
  }
  return {
    histLine, count, total, windowTitle, winRc, names,
    webArea: count("AXWebArea"),
    hung: count("AXWebArea") >= 1,
  };
}

function dumpWith(helper, pid) {
  try {
    return execFileSync(helper, [String(pid)], { encoding: "utf8", maxBuffer: 32 << 20, timeout: 60_000 });
  } catch (e) {
    return (e.stdout ?? "") + `\n(exec 失败: ${e.message})`;
  }
}

/**
 * ★ 关键形状：**读到 AXWebArea 为止**，而不是"读一次然后下结论"。
 * 实测：裸二进制启动后第一次读只有 `AXWindow + AXGroup`（WebKit 懒建 AX 树），
 * 同一进程上第二次读才拿到整棵树；而在别的进程生命周期里还可能遇到
 * `AXWindows rc=-25204`（窗口在屏、AX 却不响应）⇒ 两种空读数都要重试。
 */
export function probeTree(pid, { tries = 8, gapMs = 2000, onEmpty } = {}) {
  const helper = ensureHelper();
  let last = "";
  for (let i = 1; i <= tries; i += 1) {
    last = dumpWith(helper, pid);
    const parsed = parseTree(last);
    if (parsed.hung) return { parsed, tries: i, text: last, helper };
    if (i === 1 && onEmpty) onEmpty(parsed);
    try {
      execFileSync("/bin/sleep", [String(gapMs / 1000)]);
    } catch { /* 采样不该打断被测那一轮 */ }
  }
  return { parsed: parseTree(last), tries, text: last, helper };
}

/** 环境是否具备（调用方据此决定"跳过并明说"还是"判红"）。 */
export function axEnvironmentReady() {
  const problems = [];
  if (process.platform !== "darwin") problems.push(`本平台没有这条路（${process.platform}）`);
  try {
    const on = execFileSync("osascript", ["-e", 'tell application "System Events" to UI elements enabled'],
      { encoding: "utf8" }).trim();
    if (on !== "true") problems.push("跑测试的进程没有『辅助访问』权限");
  } catch {
    problems.push("取不到辅助访问授权状态");
  }
  if (!existsSync(SRC)) problems.push(`缺遍历器源码 ${path.relative(ROOT, SRC)}`);
  return { ready: problems.length === 0, problems };
}
