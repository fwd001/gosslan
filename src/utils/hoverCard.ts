/**
 * 通用「智能浮层卡片」摆位 —— 把"贴着锚点弹、**四个方向都会翻**、不许出屏"收成一个家。
 *
 * 为什么要抽出来（用户 2026-10-07：「上下左右都会显示那个卡片，就看这个卡片弹出地方的位置，
 * 我们可以把这一套东西写成一个通用的方法，将来可能还有好玩的地方就可以复用」）：
 * 表情名单、已读成员列表、表情选择器已经各写了一份"别顶出屏幕"，第三处再来一遍必然漂移。
 *
 * 两条轴**各按锚点离哪条边近**独立翻，互不牵连：
 *
 * - **横向**：锚点中心在视口左半 ⇒ 卡片往右长（钉 `left`）；右半 ⇒ 往左长（钉 `right`）。
 *   ★ 必须钉住"会靠住的那条边"：卡片宽度按内容伸缩（只给 `max-width`），
 *   往左长那一支若仍写 `left`，内容一窄右缘就够不到锚点右缘 —— 对齐判据当场红。
 * - **纵向**：锚点在视口下半 ⇒ 往上长（钉 `bottom`）；上半 ⇒ 往下长（钉 `top`）。
 *   用视口中线而不是"剩余空间够不够"：卡片高度是内容给的估算值，拿估算值判方向
 *   会在临界值来回翻（表情面板历史上正是这样"一滚动就换方向"的）。
 *
 * 夹位始终按**最宽那一档**算 ⇒ 内容只会更窄，因此更往里，不可能外溢。
 */
import { popupLeft, popupPlacement, popupWidth } from "./popupPosition.ts";

/** 锚点的四条边（`DOMRect` 天然满足这个形状）。 */
export interface CardAnchor {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/**
 * 卡片的落点：**每条轴只发一侧**（另一侧为 null），这样宽度按内容伸缩时
 * 靠住的那条边不会跑。`maxWidth` 是这一档的上限，不是写死的宽度。
 *
 * `placement` 把"这次到底往上还是往下弹"这个**语义**也带出去：
 * 光有 `top`/`bottom` 两个数不够 —— 消费方常常还要按方向换箭头朝向、换动画原点
 * （表情选择器就把这个值原样传进 `EmojiPicker` 的 `placement` prop）。
 * 让它由摆位函数一次算定，比在每个消费点再 `top != null ? … : …` 猜一遍强：
 * 猜的那一处和这里的 `above` 判据一旦分叉，浮层内容和浮层位置会互相不认。
 */
export interface CardPlacement {
  left: number | null;
  right: number | null;
  top: number | null;
  bottom: number | null;
  maxWidth: number;
  placement: "above" | "below";
}

export interface CardOptions {
  /** 期望的最大宽度（视口太窄时会让位给视口）。默认 256。 */
  maxWidth?: number;
  /** 离视口边缘与离锚点的最小间距。默认 8。 */
  pad?: number;
  /** 注入视口尺寸，便于单测不依赖真实 window。 */
  viewport?: { w: number; h: number };
}

export function placeCard(anchor: CardAnchor, opts: CardOptions = {}): CardPlacement {
  const pad = opts.pad ?? 8;
  const vw = opts.viewport?.w ?? window.innerWidth;
  const vh = opts.viewport?.h ?? window.innerHeight;
  const maxW = popupWidth(opts.maxWidth ?? 256, vw, pad);
  const endAligned = (anchor.left + anchor.right) / 2 >= vw / 2;
  const boxLeft = popupLeft(
    { left: anchor.left, right: anchor.right },
    endAligned ? "end" : "start",
    maxW,
    vw,
    pad,
  );
  const above = popupPlacement(anchor.top, vh) === "above";
  return {
    left: endAligned ? null : boxLeft,
    right: endAligned ? vw - boxLeft - maxW : null,
    top: above ? null : anchor.bottom + pad,
    bottom: above ? vh - anchor.top + pad : null,
    maxWidth: maxW,
    placement: above ? "above" : "below",
  };
}

/** 摆位 → `:style` 对象。未用的那一侧**整个键都不出现**，不留 `undefined` 让 CSS 打架。 */
export function cardStyle(p: CardPlacement): Record<string, string> {
  const s: Record<string, string> = { maxWidth: `${p.maxWidth}px` };
  if (p.left != null) s.left = `${p.left}px`;
  if (p.right != null) s.right = `${p.right}px`;
  if (p.top != null) s.top = `${p.top}px`;
  if (p.bottom != null) s.bottom = `${p.bottom}px`;
  return s;
}
