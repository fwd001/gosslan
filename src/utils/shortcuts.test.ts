import { test } from "node:test";
import assert from "node:assert/strict";
import { matchAppShortcut } from "./shortcuts.ts";
import { APP_ACTION } from "./appActions.ts";

const noMod = { mod: false, alt: false, shift: false, isComposing: false, key: "f" };

test("命中：主修饰键 + 单字符/符号 → 返回对应动作", () => {
  assert.equal(matchAppShortcut({ ...noMod, mod: true, key: "," }), APP_ACTION.openSettings);
  assert.equal(matchAppShortcut({ ...noMod, mod: true, key: "f" }), APP_ACTION.focusSearch);
  assert.equal(matchAppShortcut({ ...noMod, mod: true, key: "n" }), APP_ACTION.addFriend);
});

test("大写键也命中（CapsLock 打开时 e.key 为大写，调用方已 lowercase，这里再兜一层）", () => {
  assert.equal(matchAppShortcut({ ...noMod, mod: true, key: "F" }), APP_ACTION.focusSearch);
  assert.equal(matchAppShortcut({ ...noMod, mod: true, key: "N" }), APP_ACTION.addFriend);
});

test("无主修饰键 → 不处理（普通输入不受影响）", () => {
  assert.equal(matchAppShortcut({ ...noMod, mod: false, key: "f" }), null);
  assert.equal(matchAppShortcut({ ...noMod, mod: false, key: "n" }), null);
  assert.equal(matchAppShortcut({ ...noMod, mod: false, key: "," }), null);
});

test("alt / shift 任一按下 → 放行（不抢系统或应用内的组合）", () => {
  assert.equal(matchAppShortcut({ ...noMod, mod: true, alt: true, key: "f" }), null);
  assert.equal(matchAppShortcut({ ...noMod, mod: true, shift: true, key: "n" }), null);
  // shift+⌘+f 这类组合本应用不用，不该被吞
  assert.equal(matchAppShortcut({ ...noMod, mod: true, shift: true, key: "f" }), null);
});

test("输入法组合态 → 放行（中文候选时按 Enter/字母不应误触发）", () => {
  assert.equal(matchAppShortcut({ ...noMod, mod: true, isComposing: true, key: "f" }), null);
});

test("未映射的键 → 不处理", () => {
  for (const key of ["w", "q", "c", "v", "a", "x", "e", "1", " ", "Enter"]) {
    assert.equal(matchAppShortcut({ ...noMod, mod: true, key }), null, `key=${key}`);
  }
});

// ── N3（原生感走查）：⌘K / Ctrl+K 是桌面端"打开搜索"的事实标准，⌘F 是应用内已有的那条
test("⌘K / Ctrl+K 与 ⌘F 打开同一个搜索（原生感走查 N3）", () => {
  const base = { alt: false, shift: false, isComposing: false };
  assert.equal(matchAppShortcut({ ...base, mod: true, key: "k" }), APP_ACTION.focusSearch);
  assert.equal(matchAppShortcut({ ...base, mod: true, key: "K" }), APP_ACTION.focusSearch);
});

test("⌘⇧K / ⌘⌥K 不算打开搜索（组合键一律放行，与其余条目同一套规矩）", () => {
  const base = { mod: true, isComposing: false };
  assert.equal(matchAppShortcut({ ...base, alt: true, shift: false, key: "k" }), null);
  assert.equal(matchAppShortcut({ ...base, alt: false, shift: true, key: "k" }), null);
});
