import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MAX_SCALE,
  MIN_SCALE,
  clampScale,
  pinchScale,
  swipeDirection,
} from "./lightboxGestures.ts";

test("缩放夹取：上下限与非法值", () => {
  assert.equal(clampScale(1), 1);
  assert.equal(clampScale(0.2), MIN_SCALE, "不允许缩到比适应屏幕更小");
  assert.equal(clampScale(99), MAX_SCALE);
  assert.equal(clampScale(Number.NaN), MIN_SCALE, "NaN 不能让界面炸掉");
  assert.equal(clampScale(Number.POSITIVE_INFINITY), MAX_SCALE);
});

test("双指缩放按距离比例（不是位移之和）", () => {
  // 距离翻倍 ⇒ 倍率翻倍
  assert.equal(pinchScale(100, 200, 1), 2);
  // 距离减半 ⇒ 倍率减半（不小于下限）
  assert.equal(pinchScale(100, 50, 2), 1);
  // 从已有倍率继续放大
  assert.equal(pinchScale(100, 100, 3), 3);
  // 超过上限被夹住
  assert.equal(pinchScale(50, 500, 4), MAX_SCALE);
  // 起点两指几乎重叠 ⇒ 保持原倍率（否则会随机跳到极值）
  assert.equal(pinchScale(3, 300, 1.5), 1.5);
  assert.equal(pinchScale(Number.NaN, 300, 1.5), 1.5);
});

test("滑动切图：阈值、主轴与边界", () => {
  // 正常左划 ⇒ 下一张
  assert.equal(swipeDirection(-120, 10, 1, 0, 3), 1);
  // 正常右划 ⇒ 上一张
  assert.equal(swipeDirection(120, 10, 1, 2, 3), -1);
  // 位移不够 ⇒ 不动
  assert.equal(swipeDirection(-30, 0, 1, 0, 3), null);
  // 斜着划（垂直占优）⇒ 不动
  assert.equal(swipeDirection(-100, -140, 1, 0, 3), null);
  // 放大状态下横向拖动是"看局部"，不能切图
  assert.equal(swipeDirection(-200, 0, 2, 0, 3), null);
  // 边界：第一张右划 / 最后一张左划 都不回绕
  assert.equal(swipeDirection(200, 0, 1, 0, 3), null);
  assert.equal(swipeDirection(-200, 0, 1, 2, 3), null);
  // 只有一张图 ⇒ 怎么划都不切
  assert.equal(swipeDirection(-200, 0, 1, 0, 1), null);
  // 非有限值不处理
  assert.equal(swipeDirection(Number.NaN, 0, 1, 0, 3), null);
});
