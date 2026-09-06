// 消息流内预览的统一几何度量。
//
// VirtualList 只在数据变化时按 estimateHeight 排布，不会对真实 DOM 重新测量；
// 所以「MessageItem 实际渲染出来的高度」和「ChatWindow.estimateHeight 估出来的高度」
// 必须来自同一套常量与同一个判断函数，否则两者各自演化就会让相邻消息互相遮挡。
// 截断策略：消息流里固定只显示前 PREVIEW_LINES 行（overflow: hidden、无内部滚动条），
// 完整内容交给独立 Modal —— Modal 的 DOM 不在 VirtualList 内，不影响任何消息高度。

import { fontPx, type FontSizeKey } from "@/utils/chatStyle";

/** 消息流内预览行数上限（文本 / 代码 / 附件代码一致），超出部分只能在 Modal 里看。 */
export const PREVIEW_LINES = 5;

/** 文本气泡：leading-relaxed = 1.625 倍行距。 */
const TEXT_LINE_RATIO = 1.625;
/** 文本气泡：py-2 纵向内边距。 */
const TEXT_BUBBLE_PADDING = 16;
/** 文本气泡内长文本操作条：mt-1.5(6) + pt-1.5(6) + border-top(1) + text-xs 行高(16)。 */
const TEXT_ACTION_BAR = 29;

/** CodeBlock：单侧 1px 边框（上下各一条）。 */
const CODE_BORDER = 1;
/** CodeBlock：toolbar 固定 32px。 */
const CODE_TOOLBAR = 32;
/** CodeBlock：pre 上下各 12px 内边距。 */
const CODE_PADDING = 12;
/** CodeBlock：12.5px × 行距 1.6 = 20px / 行。 */
const CODE_LINE_HEIGHT = 20;
/** 代码块下方操作条：mt-1(4) + py-1(8) + leading-4(16)。 */
const CODE_ACTION_BAR = 28;
/** 一个视觉行的半角列数（气泡 max-w-[78%] 实测 ≈40 个半角字符 / 14px 正文）。 */
const COLUMNS_PER_LINE = 40;
/** COLUMNS_PER_LINE 对应的基准字号；字号变大时每行放不下的字符按比例减少。 */
const BASE_FONT_PX = 14;

/** 全角字符（CJK 及常用全角标点、emoji）按 2 个半角列计，其余按 1 列。 */
function columnWidth(s: string): number {
  let n = 0;
  for (const ch of s) n += (ch.codePointAt(0) ?? 0) > 0x2e80 ? 2 : 1;
  return n;
}

/** pre-wrap / whitespace-pre-wrap 下的视觉行数：每个逻辑行按列数折行后累加。 */
export function visualLineCount(content: string, columnsPerLine: number): number {
  let lines = 0;
  for (const line of content.split("\n")) {
    lines += Math.max(1, Math.ceil(columnWidth(line) / columnsPerLine));
  }
  return lines;
}

// ---------------- 文本 ----------------

function textColumns(fontSize: FontSizeKey): number {
  return Math.max(12, Math.round((COLUMNS_PER_LINE * BASE_FONT_PX) / fontPx(fontSize)));
}

/** 是否需要截断：渲染端与估算端共用它，保证「有没有第 6 行」两边判断一致。 */
export function textNeedsClamp(content: string, fontSize: FontSizeKey): boolean {
  return visualLineCount(content, textColumns(fontSize)) > PREVIEW_LINES;
}

/** 文本气泡高度（含截断态的操作条）；截断后恒为 5 行，不再随内容增高。 */
export function textBubbleHeight(content: string, fontSize: FontSizeKey): number {
  const lineH = fontPx(fontSize) * TEXT_LINE_RATIO;
  if (textNeedsClamp(content, fontSize)) {
    return TEXT_BUBBLE_PADDING + PREVIEW_LINES * lineH + TEXT_ACTION_BAR;
  }
  return TEXT_BUBBLE_PADDING + visualLineCount(content, textColumns(fontSize)) * lineH;
}

// ---------------- 代码 ----------------

/** 代码内容（inline code 消息 / 代码附件）是否需要截断。 */
export function codeNeedsClamp(content: string): boolean {
  return visualLineCount(content, COLUMNS_PER_LINE) > PREVIEW_LINES;
}

/** 完整代码块（未截断）自身高度：上下边框 + toolbar + 上下内边距 + 若干行。 */
function codeBlockNaturalHeight(lines: number): number {
  return (
    CODE_BORDER * 2 +
    CODE_TOOLBAR +
    CODE_PADDING * 2 +
    lines * CODE_LINE_HEIGHT
  );
}

/**
 * 截断容器高度：从顶部往下裁，可见部分只有「上边框 + toolbar + pre 上内边距 + 5 个整行」。
 * 不能再带上内边距和下边框，否则会在第 6 行上裁出一个笔尖。
 */
export const CODE_CLAMP_HEIGHT =
  CODE_BORDER + CODE_TOOLBAR + CODE_PADDING + PREVIEW_LINES * CODE_LINE_HEIGHT;

/** 代码块占位高度 = 预览区（截断时为固定 5 行）+ 常驻操作条（复制 / 展开显示）。 */
export function codeBlockHeight(content: string): number {
  const preview = codeNeedsClamp(content)
    ? CODE_CLAMP_HEIGHT
    : codeBlockNaturalHeight(visualLineCount(content, COLUMNS_PER_LINE));
  return preview + CODE_ACTION_BAR;
}

/**
 * 截断态代码块的整体占位高度。
 * 附件代码在文件内容读出前无法预知行数，只能按截断态估——宁可少几行留白，也不让消息重叠。
 */
export const CLAMPED_CODE_BLOCK_HEIGHT = CODE_CLAMP_HEIGHT + CODE_ACTION_BAR;
