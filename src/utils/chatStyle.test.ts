import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { contrastRatio, luma, toHsl } from "./color.ts";
import { CHAT_PRESETS, formatTimeDivider, mentionHighlightColor, resolveChatColors } from "./chatStyle.ts";

/** 覆盖冷暖、高/低饱和的代表性主题色：任何人选的自定义色都应安全。 */
const THEME_COLORS = [
  "#3b82f6", // 默认蓝
  "#22c55e", // 绿
  "#ef4444", // 红
  "#a855f7", // 紫
  "#f59e0b", // 琥珀
  "#64748b", // 低饱和石板灰
  "#0ea5e9", // 亮天蓝
];

/** 聊天区画布（亮色 --gosslan-chat，近白）。改 style.css 里这个值必须同步这里。 */
const CHAT_BG = "#ffffff";

test("亮色：自己的气泡按 luma 定标统一到微信重量档（207±11），文字对比达 AA", () => {
  for (const theme of THEME_COLORS) {
    const c = resolveChatColors("theme", theme, false);
    // 微信绿气泡 #9df29f luma 207；各主题色经 luma 定标后都应落在同一视觉重量档。
    // 218 上限兼顾暖色（琥珀）天然高 luma；低于 195 会明显"重"于对方气泡（周工反馈"怪"）。
    const l = luma(c.mineBubble);
    assert.ok(
      l >= 195 && l <= 218,
      `${theme} 的底色重量跑偏：${c.mineBubble}（luma ${l.toFixed(0)}，目标 207±11）`,
    );
    assert.ok(
      luma(c.mineText) < 110,
      `${theme} 的字色不够深：${c.mineText}（luma ${luma(c.mineText).toFixed(0)}）`,
    );
    const ratio = contrastRatio(c.mineText, c.mineBubble);
    assert.ok(ratio >= 4.5, `${theme} 亮色对比度不足：${ratio.toFixed(2)}`);
  }
});

test("亮色：气泡底色保留主题色的色相（不会被混白洗成灰）", () => {
  const theme = "#3b82f6";
  const c = resolveChatColors("theme", theme, false);
  // 色相偏差应小于 1 度
  assert.ok(Math.abs(toHsl(c.mineBubble)[0] - toHsl(theme)[0]) < 1);
  // 底色仍要有可见的色彩倾向，不能退化成纯灰
  assert.ok(toHsl(c.mineBubble)[1] > 0.4, `底色饱和度掉太多：${toHsl(c.mineBubble)[1]}`);
});

test("亮色：微信式结构——画布近白，两个气泡都暗于画布，自己远深于对方", () => {
  const c = resolveChatColors("theme", "#3b82f6", false);
  assert.ok(
    luma(c.mineBubble) < luma(c.otherBubble),
    `自己的气泡应深于对方：${luma(c.mineBubble).toFixed(0)} vs ${luma(c.otherBubble).toFixed(0)}`,
  );
  assert.ok(
    luma(c.otherBubble) < luma(CHAT_BG),
    `对方的气泡应比画布深（浅灰）：${luma(c.otherBubble).toFixed(0)} vs ${luma(CHAT_BG).toFixed(0)}`,
  );
  assert.notEqual(toHsl(c.mineBubble)[0], toHsl(c.otherBubble)[0]);
});

test("亮色：气泡与画布拉得开，不会糊在一起", () => {
  // 早前画布 #f8fafc 与纯白对方气泡对比仅 1.02，等于没有气泡（周工反馈"区分度不高"）
  const c = resolveChatColors("theme", "#3b82f6", false);
  assert.ok(contrastRatio(c.otherBubble, CHAT_BG) >= 1.08, "对方气泡与画布对比过低");
  assert.ok(contrastRatio(c.mineBubble, CHAT_BG) >= 1.3, "自己的气泡与画布对比过低");
  // 两侧气泡之间也要能分辨
  assert.ok(contrastRatio(c.mineBubble, c.otherBubble) >= 1.3, "两侧气泡分不开");
});

test("亮色：全部预设的自己气泡与画布对比 ≥ 1.25（防再次糊底）", () => {
  for (const preset of CHAT_PRESETS) {
    const c = resolveChatColors(preset.key, "#3b82f6", false);
    const ratio = contrastRatio(c.mineBubble, CHAT_BG);
    assert.ok(ratio >= 1.25, `${preset.label} 的气泡与画布对比不足：${ratio.toFixed(2)}`);
  }
});

test("暗色：自己的气泡不过亮，文字对比度达 AA", () => {
  for (const theme of THEME_COLORS) {
    const c = resolveChatColors("theme", theme, true);
    assert.ok(luma(c.mineBubble) <= 130, `${theme} 暗色气泡过亮：${luma(c.mineBubble).toFixed(0)}`);
    const ratio = contrastRatio(c.mineText, c.mineBubble);
    assert.ok(ratio >= 4.5, `${theme} 暗色对比度不足：${ratio.toFixed(2)}`);
  }
});

test("全部预设：明暗两套配色的正文对比度都达 AA", () => {
  for (const preset of CHAT_PRESETS) {
    for (const dark of [false, true]) {
      const c = resolveChatColors(preset.key, "#3b82f6", dark);
      const mine = contrastRatio(c.mineText, c.mineBubble);
      const other = contrastRatio(c.otherText, c.otherBubble);
      assert.ok(mine >= 4.5, `${preset.key}/${dark ? "暗" : "亮"}/自己：${mine.toFixed(2)}`);
      assert.ok(other >= 4.5, `${preset.key}/${dark ? "暗" : "亮"}/对方：${other.toFixed(2)}`);
    }
  }
});

test("时间分割线：微信式智能格式（今天/昨天/一周内/今年/跨年）", () => {
  // now = 2026-09-09（周三）16:00
  const now = new Date(2026, 8, 9, 16, 0, 0).getTime();
  const t = (month: number, day: number, h: number, m: number, year = 2026) =>
    new Date(year, month - 1, day, h, m, 0).getTime();

  assert.equal(formatTimeDivider(t(9, 9, 10, 5), now), "10:05", "今天只报时分");
  assert.equal(formatTimeDivider(t(9, 8, 23, 5), now), "昨天 23:05", "昨天带「昨天」前缀");
  assert.equal(formatTimeDivider(t(9, 5, 8, 0), now), "周六 08:00", "一周内报星期");
  assert.equal(formatTimeDivider(t(8, 30, 12, 0), now), "8月30日 12:00", "今年更早报月日");
  assert.equal(
    formatTimeDivider(t(12, 31, 18, 30, 2025), now),
    "2025年12月31日 18:30",
    "跨年带年份",
  );
});

// ---------------- mention 高亮色：@提及 蓝字在任意气泡底上都得能看清 ----------------

test("mention 高亮色：明暗 × 全主题 × 自己/对方气泡底，对比 ≥ 4.5 且与气泡文字有区分", () => {
  for (const theme of THEME_COLORS) {
    for (const dark of [false, true]) {
      const c = resolveChatColors("theme", theme, dark);
      for (const bg of [c.mineBubble, c.otherBubble]) {
        const fg = mentionHighlightColor(theme, dark, bg);
        assert.ok(fg, `${theme} ${dark ? "暗" : "亮"}色（底 ${bg}）无解：高亮色回退了 inherit`);
        const ratio = contrastRatio(fg, bg);
        assert.ok(
          ratio >= 4.5,
          `${theme} ${dark ? "暗" : "亮"}色 mention 高亮对比不足：fg=${fg} bg=${bg} → ${ratio.toFixed(2)}`,
        );
        // 与气泡正文文字色不同，否则等于没区分
        const mine = bg === c.mineBubble;
        assert.notEqual(fg, mine ? c.mineText : c.otherText);
      }
    }
  }
});

test("mention 高亮色：底色读不到时不猜色，取梯度末端且不低于主题内任一底", () => {
  for (const theme of THEME_COLORS) {
    for (const dark of [false, true]) {
      const c = resolveChatColors("theme", theme, dark);
      for (const unknown of [undefined, ""]) {
        const fg = mentionHighlightColor(theme, dark, unknown);
        assert.ok(fg, `${theme} ${dark ? "暗" : "亮"}色底色未知时回退了 inherit`);
        // 梯度末端 = 暗色最浅 / 亮色最深：对主题内的中性底必须仍是最高对比那一端
        for (const bg of [c.mineBubble, c.otherBubble]) {
          const ratio = contrastRatio(fg, bg);
          assert.ok(
            ratio >= 4.5,
            `${theme} ${dark ? "暗" : "亮"}色未知底 → 对 ${bg} 只有 ${ratio.toFixed(2)}`,
          );
          // 且不得比试算结果更浅/更亮，否则说明末端选反了
          const solved = mentionHighlightColor(theme, dark, bg);
          const [, , lf] = toHsl(fg);
          const [, , ls] = toHsl(solved);
          assert.ok(dark ? lf >= ls : lf <= ls, `${theme} ${dark ? "暗" : "亮"}末端档选反了`);
        }
      }
    }
  }
});

// 底色兜底只能有一份事实：组件里再写一个 hex 字面量 = 抄走主题 token 的值，
// token 一改这边静默失准（历史上气泡与全文弹窗两处抄的还是不同的值）。
test("组件调 mentionHighlightColor 不得自带兜底色", () => {
  for (const rel of [
    "components/message/MessageTextBubble.vue",
    "components/message/MessageContentModal.vue",
    "components/chat/MessageComposer.vue",
  ]) {
    const src = readFileSync(join(import.meta.dirname, "..", rel), "utf8");
    const calls = src
      .split("\n")
      .filter((l) => l.includes("mentionHighlightColor(") && !l.trim().startsWith("//"));
    assert.ok(calls.length > 0, `${rel} 不再调用 mentionHighlightColor？守卫需重定位`);
    for (const line of calls) {
      assert.ok(
        !line.includes("#"),
        `${rel} 的调用里出现了硬编码兜底色（底色未知应把空串传下去）：${line.trim()}`,
      );
    }
  }
});
