/**
 * 「打开独立窗口」的单飞 + 防抖判据 + 接线守卫。
 *
 * 为什么这两层都要守（用户 2026-09-12 实测）：
 *   - 连点设置会**开出第二个窗口**、还会让窗口先闪成主聊天界面 —— 后端单例/串行是主修，
 *     前端这一层负责"别把请求打爆"，两层都要在；
 *   - 这类退化**看起来完全正常**（点一下照样能开），只有连点时才会暴露，
 *     所以除了单测，还要静态盯住"两个开窗函数确实走了 `launchAuxWindow`"。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";
import { shouldLaunchWindow, WINDOW_LAUNCH_MIN_INTERVAL_MS } from "./windowLaunch.ts";

const srcDir = join(import.meta.dirname, "..");

// ---------------- 判据本身 ----------------

test("第一次点击（从未发起过、也没有在飞）应当发起", () => {
  assert.equal(shouldLaunchWindow(1_000, false, 0), true);
});

test("正在打开时再点：合并进那一次，不再发起", () => {
  // 距上次发起已经过了很久，但仍然不应该再发一次 —— 合并优先于时间判据
  assert.equal(shouldLaunchWindow(999_999, true, 1_000), false);
});

test("刚点过（不足最小间隔）视为连点，忽略", () => {
  const now = 10_000;
  assert.equal(shouldLaunchWindow(now, false, now - 1), false, "1ms 后连点应被吃掉");
  assert.equal(
    shouldLaunchWindow(now, false, now - (WINDOW_LAUNCH_MIN_INTERVAL_MS - 1)),
    false,
    "差 1ms 到间隔也应被吃掉",
  );
});

test("超过最小间隔后（关掉再开）正常放行", () => {
  const now = 10_000;
  assert.equal(shouldLaunchWindow(now, false, now - WINDOW_LAUNCH_MIN_INTERVAL_MS), true);
  assert.equal(shouldLaunchWindow(now, false, now - 5_000), true);
});

test("间隔设为 0 时只剩'单飞'在起作用（证明时间判据不是唯一防线）", () => {
  assert.equal(shouldLaunchWindow(1_000, false, 999, 0), true);
  assert.equal(shouldLaunchWindow(1_000, true, 0, 0), false);
});

// ---------------- 接线守卫：两个开窗按钮必须走统一入口 ----------------

test("桌面端开窗必须走 launchAuxWindow（单飞 + 防抖 + pending 状态）", () => {
  const layout = readFileSync(join(srcDir, "layouts", "ResponsiveLayout.vue"), "utf8");

  for (const [label, invokeName] of [
    ["settings", "openSettingsWindow"],
    ["logs", "openLogWindow"],
  ] as const) {
    assert.match(
      layout,
      new RegExp(`launchAuxWindow\\("${label}",\\s*\\(\\)\\s*=>\\s*api\\.${invokeName}\\(\\)\\)`),
      `ResponsiveLayout 打开 ${label} 窗口必须走 launchAuxWindow("${label}", …) —— ` +
        `直接在按钮里 invoke 会退化成"连点就发多次 IPC"，也就是用户报的连点问题`,
    );
  }

  // 反向：不能再有绕过 launcher 的裸调用（两条命令都必须出现在 launchAuxWindow 的参数里）
  const rawCalls = layout.match(/api\.open(SettingsWindow|LogWindow)\(\)/g) ?? [];
  assert.equal(rawCalls.length, 2, `裸调用 open*Window 应恰好 2 处（都在 launcher 里），实际 ${rawCalls.length}`);
});

test("两个窗口的 pending 状态接到了按钮上（冷启动那一下用户能看到「点到了」）", () => {
  const layout = readFileSync(join(srcDir, "layouts", "ResponsiveLayout.vue"), "utf8");
  const rail = readFileSync(join(srcDir, "components", "NavRail.vue"), "utf8");

  assert.match(layout, /useWindowOpening\("settings"\)/, "ResponsiveLayout 要订阅 settings 的打开状态");
  assert.match(layout, /useWindowOpening\("logs"\)/, "ResponsiveLayout 要订阅 logs 的打开状态");
  assert.match(layout, /:settings-opening="settingsOpening"/, "窄导航要拿到 settings 的 pending 状态");
  assert.match(layout, /:logs-opening="logsOpening"/, "窄导航要拿到 logs 的 pending 状态");
  assert.match(rail, /:aria-busy="settingsOpening"/, "设置按钮要有 aria-busy 反馈");
  assert.match(rail, /:aria-busy="logsOpening"/, "日志按钮要有 aria-busy 反馈");
});
