/**
 * 图片预览的手势数学（纯函数，便于单测钉住）。
 *
 * ## 为什么抽出来
 * 触屏手势的判定全是**阈值与方向**问题：双指缩放要按"两指距离比"而不是各自位移；
 * 左右滑动切图必须**区分轴**（斜着划不该切图，那是想平移或干脆是误触）。
 * 这类判据在真机上很难逐条复现（要两只手精确控制距离/角度），但写成纯函数可以一次钉死。
 *
 * 组件（`ImageLightbox.vue`）只负责把指针事件喂进来、把结果贴到 `scale/translate`。
 */

/** 缩放下限（1 = 原始尺寸，不允许缩到比"适应屏幕"更小）。 */
export const MIN_SCALE = 1;
/** 缩放上限：再大只是马赛克，且手势精度跟不上。 */
export const MAX_SCALE = 5;

/**
 * 把缩放夹到合法区间。
 *
 * NaN 没有大小关系可比，回落到 1（不能让 `transform: scale(NaN)` 把界面搞坏）；
 * ±Infinity 仍按大小夹取（`+∞` ⇒ 上限、`-∞` ⇒ 下限），因为那是"极大/极小"的合法语义。
 */
export function clampScale(v: number): number {
  if (Number.isNaN(v)) return MIN_SCALE;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, v));
}

/**
 * 双指缩放的新倍率：**按两指距离的比例**缩放，而不是两指各自位移之和。
 *
 * @param startDistance 手势开始时两指距离（px）
 * @param currentDistance 当前两指距离（px）
 * @param startScale 手势开始时的倍率
 *
 * 起点距离过小（两指几乎重叠）时返回 `startScale`：此时比例会被放大成随机数，
 * 表现为"一碰就跳到最大/最小"。
 */
export function pinchScale(
  startDistance: number,
  currentDistance: number,
  startScale: number,
): number {
  if (!Number.isFinite(startDistance) || startDistance < 10) return clampScale(startScale);
  if (!Number.isFinite(currentDistance)) return clampScale(startScale);
  return clampScale((currentDistance / startDistance) * startScale);
}

/** 滑动切图的方向：`-1` 上一张 / `1` 下一张 / `null` 不动。 */
export type SwipeDirection = -1 | 1 | null;

/** 触发切图的最小水平位移（px）。太小会与"轻微抖动"混淆，太大则觉得划不动。 */
export const SWIPE_THRESHOLD = 60;

/**
 * 单指滑动是否要切图。
 *
 * 三条判据缺一不可：
 * 1. 位移超过阈值（`SWIPE_THRESHOLD`）；
 * 2. **水平位移占优**（`|dx| > |dy|`）—— 斜着划多半是想平移或误触，切图会很烦人；
 * 3. 未放大（`scale <= 1`）—— 放大后横向拖动是在**看局部**，不能切图。
 *
 * 到头（第一张继续右划 / 最后一张继续左划）返回 `null`：与 iOS 相册一致（不回绕）。
 */
export function swipeDirection(
  dx: number,
  dy: number,
  scale: number,
  index: number,
  count: number,
): SwipeDirection {
  if (!Number.isFinite(dx) || !Number.isFinite(dy)) return null;
  if (scale > 1) return null;
  if (Math.abs(dx) < SWIPE_THRESHOLD) return null;
  if (Math.abs(dx) <= Math.abs(dy)) return null;
  if (count <= 1) return null;
  // 手指向左划（dx < 0）⇒ 看下一张
  const want: SwipeDirection = dx < 0 ? 1 : -1;
  const target = index + want;
  if (target < 0 || target >= count) return null;
  return want;
}
