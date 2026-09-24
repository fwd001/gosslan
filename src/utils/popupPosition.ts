/**
 * 浮层摆位的两个纯判据 —— **横向不许出屏**是这两个函数存在的全部理由。
 *
 * 为什么单独拎出来（2026-09-24 真机：群里"已读成员"弹层被屏幕边缘裁掉一半）：
 * 挂在消息行里的弹层会被消息列表的 `overflow-y: auto` 连带横向裁掉，
 * 而且它锚在气泡那一侧，宽度一大就顶出视口。表情面板早就改成 Teleport + fixed 坐标
 * （`MessageItem.positionReactionPicker` 的注释写着这条），已读弹层当时没跟着改 ⇒
 * 同一个坑两处各修一次。判据放成一份，两处共用，测试也只写一份。
 */

/** 面板宽度：视口太窄时让位给视口（左右各留 `pad`）。 */
export function popupWidth(prefers: number, viewportWidth: number, pad = 8): number {
  return Math.min(prefers, viewportWidth - pad * 2);
}

/**
 * 面板左边界（fixed 坐标）：把整个面板夹进视口。
 *
 * `anchor` 是入口按钮想对齐的那条边 —— `align: "start"` 对齐入口左缘（表情面板），
 * `align: "end"` 对齐入口右缘（已读成员列表：入口在气泡右侧，往左展开才不盖住自己）。
 */
export function popupLeft(
  anchor: { left: number; right: number },
  align: "start" | "end",
  width: number,
  viewportWidth: number,
  pad = 8,
): number {
  const raw = align === "end" ? anchor.right - width : anchor.left;
  // 先夹右再夹左：视口比面板还窄时（`width` 已被 popupWidth 收到视口内）不会来回打架
  return Math.max(pad, Math.min(raw, viewportWidth - pad - width));
}

/**
 * 往上弹还是往下弹：入口在视口**下半**就往上，上半就往下。
 *
 * 用视口中线而不是"剩余空间够不够"：面板高度是估算值（列表可滚动，真实高度未知），
 * 拿估算值去判"够不够"会在临界值来回翻 —— 表情面板历史上正是这样"一滚动就换方向"的。
 */
export function popupPlacement(anchorTop: number, viewportHeight: number): "above" | "below" {
  return anchorTop > viewportHeight / 2 ? "above" : "below";
}
