// 消息流内预览的统一几何度量。
//
// VirtualList 只在数据变化时按 estimateHeight 排布，不会对真实 DOM 重新测量；
// 所以「MessageItem 实际渲染出来的高度」和「ChatWindow.estimateHeight 估出来的高度」
// 必须来自同一套常量与同一个判断函数，否则两者各自演化就会让相邻消息互相遮挡。
// 截断策略：消息流里固定只显示前 PREVIEW_LINES 行（overflow: hidden、无内部滚动条），
// 完整内容交给独立 Modal —— Modal 的 DOM 不在 VirtualList 内，不影响任何消息高度。

import { fontPx, type FontSizeKey } from "@/utils/chatStyle";
import { parseQuote, quoteBody } from "@/utils/quote";

/** 消息流内预览行数上限（文本 / 代码 / 附件代码一致），超出部分只能在 Modal 里看。 */
export const PREVIEW_LINES = 5;

/** 代码卡片底色：必须比聊天画布亮/暗一档，卡片与尖角才「浮起」有层次。
 *  暗色用 #161b22（比画布 #0f172a 亮一档）。
 *  亮色原为 #f6f8fa——与画布 #fafafa 仅差 2~4 个色阶，实测卡片边界只剩 1px 描边、
 *  尖角完全看不见（尖角填充的就是这个色，贴着画布等于没有尖角）。
 *  与聊天样式里「对方气泡 #eeeef0 才不与画布同色」是同一个坑，故亮色改为一档更深的冷灰。 */
export const CODE_SURFACE = { dark: "#161b22", light: "#eaeef3" } as const;

/** 文本气泡：`leading-normal` = 1.5 倍行距（原 1.625 即 `leading-relaxed`，
 *  2026-09-12 按用户反馈「气泡太高、不如微信和谐」收紧）。
 *  ⚠️ 与 `MessageTextBubble.vue` 的 `leading-normal` 成对，改一个必须改另一个。 */
const TEXT_LINE_RATIO = 1.5;
/** 文本气泡：`py-1.5` 纵向内边距合计 12px（原 `py-2` = 16px，同一次收紧）。
 *  ⚠️ 与 `MessageTextBubble.vue` / `MessageItem.vue` 兜底气泡的 `py-1.5` 成对。 */
const TEXT_BUBBLE_PADDING = 12;
/** MessageItem 给文本气泡统一保留 1px 描边，透明描边也会计入盒模型高度。 */
const TEXT_BUBBLE_BORDER = 2;
/** 文本气泡内长文本操作条：mt-1.5(6) + pt-1.5(6) + border-top(1) + text-xs 行高(16)。 */
const TEXT_ACTION_BAR = 29;

/** CodeBlock 在消息流里以「无描边的气泡卡片」呈现（描边会与气泡尖角露出拼接感，故去掉），
 *  因此高度里不含边框。独立的全文弹窗不走这里的估算。 */
/** CodeBlock：toolbar 固定 32px。 */
const CODE_TOOLBAR = 32;
/** CodeBlock：pre 上下各 12px 内边距。 */
const CODE_PADDING = 12;
/** CodeBlock：12.5px × 行距 1.6 = 20px / 行。 */
const CODE_LINE_HEIGHT = 20;
/** 代码块下方操作条：mt-1(4) + py-1(8) + leading-4(16)。 */
const CODE_ACTION_BAR = 28;
/** 一个视觉行的半角列数（气泡 max-w-[72%] 实测 ≈37 个半角字符 / 14px 正文）。
 *  改 MessageItem 的气泡宽度上限时，这里必须同步（偏小只会多留白，偏大会遮挡下一条）。 */
const COLUMNS_PER_LINE = 37;
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

/** 是否需要截断：渲染端与估算端共用它，保证「有没有第 6 行」两边判断一致。
 *  ⚠️ 只看**正文** —— 被 clamp 的就是正文那个 div，引用头不在其中。
 *  把引用头当成一行正文算进来，正文正好 5 行的引用消息会凭空多出一条「展开」操作条，
 *  点开弹出的全文和气泡里显示的一模一样。 */
export function textNeedsClamp(content: string, fontSize: FontSizeKey): boolean {
  return visualLineCount(quoteBody(content), textColumns(fontSize)) > PREVIEW_LINES;
}

/** 引用块排版：`text-[12px] leading-4`（行高 16px）+ `py-1`(8) + `mb-1.5`(6)。
 *  ⚠️ 与 MessageTextBubble 引用块的类名成对，改一类必须改另一类。 */
const QUOTE_LINE_HEIGHT = 16;
const QUOTE_CHROME = 8 + 6;
/** 引用头字号固定 12px，**不随正文字号档位变**，所以它单独一套列宽。 */
const QUOTE_FONT_PX = 12;

/** 引用块高度（无引用时为 0）。按 12px 的列宽折行，长片段会占多行。 */
function quoteHeaderHeight(header: string): number {
  if (!header) return 0;
  // 引用块左右 px-2 比正文 px-3 各少 4px，可用宽度略宽；这里不额外补偿，
  // 宁可多估一行（多估只是留白，少估会让相邻消息互相遮挡）。
  const cols = Math.round((COLUMNS_PER_LINE * BASE_FONT_PX) / QUOTE_FONT_PX);
  return visualLineCount(header, cols) * QUOTE_LINE_HEIGHT + QUOTE_CHROME;
}

/** 文本气泡高度（含引用块 + 截断态的操作条）；正文截断后恒为 5 行，不再随内容增高。 */
export function textBubbleHeight(content: string, fontSize: FontSizeKey): number {
  const { header, body } = parseQuote(content);
  const lineH = fontPx(fontSize) * TEXT_LINE_RATIO;
  const bodyH = textNeedsClamp(content, fontSize)
    ? PREVIEW_LINES * lineH + TEXT_ACTION_BAR
    : visualLineCount(body, textColumns(fontSize)) * lineH;
  return TEXT_BUBBLE_BORDER + TEXT_BUBBLE_PADDING + quoteHeaderHeight(header) + bodyH;
}

// ---------------- 代码 ----------------

/** 代码内容（inline code 消息 / 代码附件）是否需要截断。 */
export function codeNeedsClamp(content: string): boolean {
  return visualLineCount(content, COLUMNS_PER_LINE) > PREVIEW_LINES;
}

/** 完整代码块（未截断）自身高度：toolbar + 上下内边距 + 若干行（消息流里的卡片无描边）。 */
function codeBlockNaturalHeight(lines: number): number {
  return CODE_TOOLBAR + CODE_PADDING * 2 + lines * CODE_LINE_HEIGHT;
}

/**
 * 截断容器高度：从顶部往下裁，可见部分只有「toolbar + pre 上内边距 + 5 个整行」。
 * 不能再带上内边距和下边框，否则会在第 6 行上裁出一个笔尖。
 */
export const CODE_CLAMP_HEIGHT = CODE_TOOLBAR + CODE_PADDING + PREVIEW_LINES * CODE_LINE_HEIGHT;

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

// ---------------- 图片 ----------------

/**
 * 图片气泡的**高度上限**，同时是三个用途的同一个数：
 * ① `<img>` 的 `max-height`；② 加载完成前骨架预留的高度；③ 虚拟列表对图片行的估算。
 *
 * 为什么这三件事必须共用一个数（roadmap N22 那一格的落点）：图片容器**定宽** 208px
 * （`w-52`，用户 2026-09-12「图片发出来之后大小应该是固定的」），而消息载荷里
 * **没有原图尺寸**（`content` 的 JSON 只有 name/path/size/sha256/subtype；补这个字段要动
 * 消息协议，为视觉效果排除）⇒ 渲染高度只能在解码后知道：
 * `真实高 = min(本常量, 208 × 原图高 / 原图宽)`。既然解码前只能猜，就把它钉成
 * "预留 == 估算 == 上限"：骨架挂载那一次不再被实测纠正（少一整次整表前缀和重算，
 * 见 N10 那笔账），而竖长图（取到上限那一档）**加载完成零跳变**。
 * 代价（已知并由用户拍板）：横幅图（16:9 截图真高 ≈117px）完成时是**收缩**而不是原来的增长。
 *
 * ⚠️ 与 `MessageImageBubble.vue` 的骨架 / `<img>` **成对**：那边已从本常量取高度，
 * 所以这里改动会同时改到渲染；不要再在任何组件里写回 `h-*` / `max-h-*` 字面量
 * （由 designGuards ㉞ 与运行时探针 `--only=imgskel` 两段一起盯着）。
 */
export const IMAGE_BUBBLE_HEIGHT = 288;

/**
 * 图片**终态**占位块的高度（加载失败 / 本地媒体已被存储清理）：紧凑一档。
 *
 * 与上面的区别是"后面还有一张图要来"：终态不会，为一张永远等不来的图撑 288px 灰块
 * 是最差的选项。估算端仍按 IMAGE_BUBBLE_HEIGHT 给图片行 ⇒ 这里是**高估** ⇒ 只留白、
 * 不遮挡（本文件顶部那条契约说的就是方向）。
 */
export const IMAGE_PLACEHOLDER_HEIGHT = 128;
