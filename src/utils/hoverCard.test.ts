/**
 * `placeCard` 的判据（用户 2026-10-07：「上下左右都会显示那个卡片，就看弹出地方的位置，
 * 把这一套写成一个通用的方法」+「框框是固定长度、名字不够长后面有空白，要按名字撑开」）。
 *
 * ★ 这里最要紧的一条不是"翻方向"，而是**每条轴只发一侧**：宽度改成按内容伸缩之后，
 * 往左长那一支如果还发 `left`，内容一窄右缘就够不到锚点 ⇒ 浮层会跟锚点脱开。
 * 所以"靠住的那条边必须钉住"要单独钉，光测"不越界"是测不出来的。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { cardStyle, placeCard } from "./hoverCard.ts";

const VP = { w: 1200, h: 800 };
const opt = (viewport = VP) => ({ maxWidth: 256, viewport });

test("锚点在左半 ⇒ 卡片往右长（钉 left、right 不发），且落在锚点下方", () => {
  const p = placeCard({ left: 100, right: 140, top: 400, bottom: 428 }, opt());
  assert.equal(p.left, 100, "左边宽敞时贴着锚点左缘");
  assert.equal(p.right, null, "往右长这一支不许同时发 right（两条一起会把盒子拉宽）");
  assert.equal(p.top, 436, "锚点在上半 ⇒ 往下长，留 8px 间隙");
  assert.equal(p.bottom, null);
});

test("锚点在右半 ⇒ 卡片往左长，并且**右缘正好钉在锚点右缘**", () => {
  const a = { left: 1100, right: 1140, top: 600, bottom: 628 };
  const p = placeCard(a, opt());
  assert.equal(p.left, null, "往左长这一支不许发 left —— 宽度一伸缩就会脱开锚点");
  assert.equal(a.right + p.right, VP.w, "右缘距视口右缘 = vw - 锚点右缘 ⇒ 卡片右缘与锚点右缘重合");
  assert.equal(VP.h - p.bottom, a.top - 8, "底缘钉在锚点上方 8px（锚点在下半 ⇒ 往上长）");
});

test("窄视口：夹位优先于对齐 —— 宁可不钉住锚点那条边也不许出屏", () => {
  const p = placeCard({ left: 200, right: 240, top: 600, bottom: 628 }, opt({ w: 300, h: 800 }));
  assert.equal(p.maxWidth, 256);
  assert.equal(p.right, 36, "盒子被夹到 [8, 264]，右缘让位给视口");
  assert.equal(p.left, null);
});

test("视口比最大档还窄 ⇒ 上限让位给视口（左右各留 pad），不产生负宽度", () => {
  const p = placeCard({ left: 10, right: 40, top: 10, bottom: 38 }, opt({ w: 200, h: 800 }));
  assert.equal(p.maxWidth, 184, "200 - 8*2");
  assert.ok(p.maxWidth > 0);
  assert.equal(p.left, 8, "夹在左内边距上");
});

test("cardStyle：未用的那一侧整个键都不出现（不留 undefined 让 CSS 打架）", () => {
  const p = placeCard({ left: 1100, right: 1140, top: 600, bottom: 628 }, opt());
  const s = cardStyle(p);
  assert.deepEqual(Object.keys(s).sort(), ["bottom", "maxWidth", "right"]);
  assert.equal(s.right, "60px");
  assert.ok(!("left" in s) && !("top" in s));
});

/**
 * `placement` 存在的唯一理由是"方向只判一次"。表情选择器把它原样传进 `EmojiPicker`
 * 换箭头朝向与动画原点 —— 若消费点再从 `top`/`bottom` 猜一遍，猜的那处和这里的
 * `above` 判据一旦分叉，浮层内容和浮层位置就互相不认（表现为箭头指反方向）。
 */
test("placement 与 top/bottom 永远自洽", () => {
  const below = placeCard({ left: 100, right: 140, top: 400, bottom: 428 }, opt());
  assert.equal(below.placement, "below");
  assert.notEqual(below.top, null, "往下长 ⇒ 发的是 top");
  assert.equal(below.bottom, null);

  const above = placeCard({ left: 1100, right: 1140, top: 600, bottom: 628 }, opt());
  assert.equal(above.placement, "above");
  assert.equal(above.top, null, "往上长 ⇒ 发的是 bottom");
  assert.notEqual(above.bottom, null);
});
