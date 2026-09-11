import { test } from "node:test";
import assert from "node:assert/strict";
import { IME_COMMIT_WINDOW_MS, isImeKey } from "./ime.ts";

const NOW = 1_000_000;

test("自跟踪的组合态：compositionend 之前一律算 IME 键", () => {
  // 这是最可靠的一道：即使事件的 isComposing 是 false（WKWebView 的怪癖），也必须拦下
  assert.equal(isImeKey({ key: "Enter", isComposing: false }, true, 0, NOW), true);
  assert.equal(isImeKey({ key: "a" }, true, 0, NOW), true);
});

test("标准 isComposing / 旧式 keyCode 229 都算 IME 键", () => {
  assert.equal(isImeKey({ key: "Enter", isComposing: true }, false, 0, NOW), true);
  assert.equal(isImeKey({ key: "Enter", keyCode: 229 }, false, 0, NOW), true);
});

test("复现真实缺陷：compositionend 先于 keydown ⇒ 提交用的 Enter 不得触发发送", () => {
  // 用户实际场景：拼音候选框开着按回车选字。WKWebView 的事件顺序是
  // compositionend → keydown(Enter, isComposing=false) —— 只看 isComposing 会误发。
  const justEnded = NOW - 10; // 10ms 前刚结束组合
  assert.equal(
    isImeKey({ key: "Enter", isComposing: false }, false, justEnded, NOW),
    true,
    "提交键必须被识别为 IME 键（否则会把半成品中文发出去）",
  );
});

test("窗口边界：恰好等于窗口仍算提交，超出则视为正常回车", () => {
  assert.equal(isImeKey({ key: "Enter" }, false, NOW - IME_COMMIT_WINDOW_MS, NOW), true);
  assert.equal(isImeKey({ key: "Enter" }, false, NOW - IME_COMMIT_WINDOW_MS - 1, NOW), false);
  // 自定义窗口
  assert.equal(isImeKey({ key: "Enter" }, false, NOW - 200, NOW, 300), true);
});

test("正常回车不受影响：从未组合过 / 组合结束很久之后", () => {
  assert.equal(isImeKey({ key: "Enter", isComposing: false }, false, 0, NOW), false);
  assert.equal(isImeKey({ key: "Enter", isComposing: false }, false, NOW - 5_000, NOW), false);
});

test("短窗口只对 Enter 生效：组合结束后的其它键正常处理", () => {
  const justEnded = NOW - 5;
  // 提交后立刻按退格删字、按 Shift+Enter 换行等，都不该被这条规则吞掉
  assert.equal(isImeKey({ key: "Backspace" }, false, justEnded, NOW), false);
  assert.equal(isImeKey({ key: "a" }, false, justEnded, NOW), false);
  assert.equal(isImeKey({ key: "ArrowUp" }, false, justEnded, NOW), false);
});

test("空格不是我们的发送键，因此即便紧跟组合结束也不拦（避免影响输入）", () => {
  assert.equal(isImeKey({ key: " " }, false, NOW - 1, NOW), false);
});
