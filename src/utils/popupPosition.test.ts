import { test } from "node:test";
import assert from "node:assert/strict";
import { popupLeft, popupPlacement, popupWidth } from "./popupPosition.ts";

test("popupWidth：视口够宽用面板宽，窄视口让位给视口（左右各留 pad）", () => {
  assert.equal(popupWidth(360, 1200), 360);
  assert.equal(popupWidth(360, 320), 304, "320 - 8*2");
  assert.equal(popupWidth(144, 1200), 144);
});

test("popupLeft：入口在右缘时也不许把面板顶出屏幕（真机「已读列表被裁一半」）", () => {
  // 已读弹层：入口贴着右缘（1200 视口、入口右缘 1190），面板宽 144 ⇒ 右对齐 ⇒ 向左展开
  assert.equal(popupLeft({ left: 1170, right: 1190 }, "end", 144, 1200), 1046);
  // 视口很窄（300）而面板已被收到 284：右对齐后必须被夹在 pad 上，不能是负数
  assert.equal(popupLeft({ left: 250, right: 300 }, "end", 284, 300), 8);
  // 表情面板：左对齐入口左缘，且入口贴近右缘时要整体左移
  assert.equal(popupLeft({ left: 1100, right: 1140 }, "start", 360, 1200), 832);
  assert.equal(popupLeft({ left: 20, right: 60 }, "start", 360, 1200), 20, "左边宽敞时贴着入口左缘");
  // 极端：入口左缘为负（横向滚出视口）也不能为负
  assert.equal(popupLeft({ left: -50, right: -10 }, "start", 360, 1200), 8);
});

test("popupPlacement：以下半视口为界翻向，不依赖面板高度估算", () => {
  assert.equal(popupPlacement(100, 800), "below");
  assert.equal(popupPlacement(400, 800), "below", "正好在中线上归下半（保守：往下弹）");
  assert.equal(popupPlacement(401, 800), "above");
  assert.equal(popupPlacement(799, 800), "above");
});
