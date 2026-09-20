// 消息行高度估算：VirtualList 按它排布，MessageItem 按 previewMetrics 渲染，
// 两边必须共用同一套常量（见 previewMetrics.ts 顶部说明），否则相邻消息会互相遮挡。

import {
  CLAMPED_CODE_BLOCK_HEIGHT,
  codeBlockHeight,
  textBubbleHeight,
} from "@/utils/previewMetrics";
import type { FontSizeKey } from "@/utils/chatStyle";
import { isKnownKind, isTipKind } from "@/utils/messageKinds";
import type { MessageRecord } from "@/types";

/** 时间分割线阈值（≥5 分钟）。 */
const TIME_DIVIDER_GAP = 5 * 60 * 1000;

/** MessageItem: py-1.5（同发方连续消息间距 12px，学微信的呼吸感） */
const ROW_PADDING = 12;
/** 群聊昵称行：leading-none(11) + mb-[7px](7) = 18px。
 *  字号/行高改动必须同步这里与 MessageItem 的昵称行（两者是同一份高度的两处表达）。
 *  行高取 leading-none 是为了让墨迹贴住行盒顶、与头像顶边齐平（行高 1.5 会往下推 ~3.7px，看着"名字偏低"）。 */
const NICKNAME_ROW = 18;
/** 时间分割线（含 py-2） */
const TIME_DIVIDER = 32;
/** 图片气泡：max-h-72 */
const IMAGE_BUBBLE = 288;
/** 普通文件卡片 */
const FILE_CARD = 92;
/** 提示行（系统消息 / 已撤回）：text-xs 行盒 16 + ROW_PADDING 12。 */
const SYSTEM_ROW = 28;
/**
 * 合并转发卡片：固定高度（标题 1 行 + 最多 3 行预览 + 页脚 1 行 + 内外边距）。
 *
 * 卡片**刻意做成定高**：它的内容（N 条摘要）随条数变化，若高度随之变化，
 * 虚拟列表的估算就得跟着算一遍"3 行里每行会不会换行"，而卡片本来就有"最多 3 行预览"
 * 的截断设计 —— 让渲染与估算都锚在这个常量上最省事也最稳。
 * 改卡片内边距/行数时必须同步这里（`MergeCard.vue` 里有注释指向本常量）。
 */
const MERGE_CARD = 96;
/** 群任务卡片（TodoCardBubble）：头部(30) + 指派人行(24) + 底部按钮(38) + 余量。 */
const TODO_CARD_BASE = 96;
/** 任务卡片有描述时追加（描述最多 ~2.6 行 ≈ 50，留余量）。 */
const TODO_CARD_DESC = 58;
/** 任务卡片有图片时追加（一行 80px 缩略图 + 间距，留余量）。 */
const TODO_CARD_IMAGES = 88;
/**
 * 未知 kind 的占位气泡（`UnsupportedKindBubble`）：说明行 + 展开按钮 + 上下边距，留余量。
 *
 * 它**与载荷长度无关** —— 未知消息的载荷往往是 JSON，按文本估会高出一大片空白。
 * 改占位气泡的行数时要同步这里（与 MERGE_CARD 同一类契约）。
 */
const UNSUPPORTED_BUBBLE = 92;
export interface EstimateContext {
  messages: MessageRecord[];
  isGroup: boolean;
  /** 本机 device_id：用于判断「非本人」及昵称行。 */
  selfId?: string;
  fontSize: FontSizeKey;
}

/**
 * 气泡高度缓存：键 = 字号 + msg_id。
 *
 * 为什么必须有：VirtualList 的 offsets 是**全表前缀和**（O(n)），任何时候有行的实测高度与估算
 * 不一致，就会触发整表重算 → 重算里对每条消息都要调一次估算。50 万条的会话一重算就是
 * 50 万次估算，而估算对 file 类消息还要 `JSON.parse(content)`（10% 的消息命中）。
 *
 * 为什么只缓存「气泡」这一段：整体的高度还包含昵称行与时间分割线，而**分割线取决于上一条消息
 * 的时间**（`prev.ts`），所以整函数的结果随下标变化、不能按 msg_id 缓存。气泡段只依赖
 * (kind, content, 字号)，这三者对同一条消息是不变的（本应用没有「编辑消息」功能）。
 *
 * 为什么把字号放进键里：唯一会变的就是设置页的字号——放进去之后改字号自然全部失效，
 * 不需要任何手动清理逻辑。
 */
const bubbleCache = new Map<string, number>();
/** 缓存上限：超出后按插入顺序淘汰最早的一条（Map 保序，O(1)）。 */
const BUBBLE_CACHE_MAX = 50_000;

function bubbleHeightCached(m: MessageRecord, fontSize: FontSizeKey): number {
  const key = `${fontSize}|${m.msg_id}`;
  const hit = bubbleCache.get(key);
  if (hit !== undefined) return hit;
  const v = computeBubbleHeight(m, fontSize);
  if (bubbleCache.size >= BUBBLE_CACHE_MAX) {
    const oldest = bubbleCache.keys().next().value;
    if (oldest !== undefined) bubbleCache.delete(oldest);
  }
  bubbleCache.set(key, v);
  return v;
}

/** 气泡本身的高度（不含昵称行 / 时间分割线）。 */
function computeBubbleHeight(m: MessageRecord, fontSize: FontSizeKey): number {
  switch (m.kind) {
    case "code":
      return codeBlockHeight(m.content);
    case "image":
      return IMAGE_BUBBLE;
    case "file": {
      // 普通文件卡片 92；附件图片 ≤288；附件代码按截断态占位（读文件前预知不了行数）。
      let sub = "file";
      let hasPath = false;
      try {
        const o = JSON.parse(m.content) as { subtype?: string; path?: string };
        sub = o?.subtype ?? "file";
        hasPath = !!o?.path;
      } catch {
        /* 历史 / 异常内容按普通 file 卡片估 */
      }
      if (hasPath && sub === "image") return IMAGE_BUBBLE;
      if (hasPath && sub === "code") return CLAMPED_CODE_BLOCK_HEIGHT;
      return FILE_CARD;
    }
    case "system":
    // 「已撤回」与系统消息是**同一形态**（`messageKinds.TIP_KINDS`）——此前它落到 default
    // 按空文本气泡估 35px，比实际渲染的 28px 高 7px，虚拟列表就会把它下面那条推偏。
    case "recalled":
      return SYSTEM_ROW;
    // 合并转发卡片：定高（见 MERGE_CARD 的说明）
    case "merge":
      return MERGE_CARD;
    case "todo": {
      // 卡片高度随描述/图片存在与否变化；宁可多估留白，也不让相邻消息互相遮挡。
      let desc = false;
      let imgs = false;
      try {
        const o = JSON.parse(m.content) as { description?: string; images?: unknown[] };
        desc = typeof o?.description === "string" && o.description.length > 0;
        imgs = Array.isArray(o?.images) && o.images.length > 0;
      } catch {
        /* 异常内容按最小卡片估 */
      }
      return TODO_CARD_BASE + (desc ? TODO_CARD_DESC : 0) + (imgs ? TODO_CARD_IMAGES : 0);
    }    // 本机不认识的 kind（对端版本比本机新）⇒ 渲染占位气泡，高度与载荷长度无关
    // （详见 `UNSUPPORTED_BUBBLE` 的说明：按文本估会留出一大片空白）。
    default:
      return isKnownKind(m.kind) ? textBubbleHeight(m.content, fontSize) : UNSUPPORTED_BUBBLE;
  }
}

/** 单条消息的占位高度（气泡 + 时间/昵称/分割线，头像与气泡同行不计入）。 */
export function estimateMessageHeight(
  m: MessageRecord,
  index: number | undefined,
  ctx: EstimateContext,
): number {
  const prev = index != null && index > 0 ? ctx.messages[index - 1] : null;

  const bubble = bubbleHeightCached(m, ctx.fontSize);

  // 每条消息独立完整渲染（无合并）：时间行恒有；群聊非本人显示昵称
  const showDivider = !prev || m.ts - prev.ts >= TIME_DIVIDER_GAP;
  // 提示行没有昵称行（`MessageItem` 里提示行整支都在头像行之外）—— 这里必须同步，
  // 否则群聊里一条"对方撤回"会多估 18px。
  const showNickname =
    ctx.isGroup && m.sender_id !== ctx.selfId && !isTipKind(m.kind);

  return (
    bubble +
    ROW_PADDING +
    (showNickname ? NICKNAME_ROW : 0) +
    (showDivider ? TIME_DIVIDER : 0)
  );
}
