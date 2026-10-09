import { test } from "node:test";
import assert from "node:assert/strict";
import { EMPTY_REACTION_CHIPS, EMPTY_STRING_LIST } from "./emptyList.ts";

test("空列表 prop 常量：取到的一直是同一个对象（这处优化存在的全部理由）", () => {
  assert.equal(EMPTY_STRING_LIST.length, 0);
  assert.equal(EMPTY_REACTION_CHIPS.length, 0);
  // 两条常量互不相同：把它们合成同一条会让「已读列表」与「回应 chip」共用一个数组
  assert.notEqual(EMPTY_STRING_LIST as unknown, EMPTY_REACTION_CHIPS as unknown);
});

test("空列表 prop 常量：冻住 —— 原地改当场抛，而不是让一条脏数组挂在所有消息上", () => {
  assert.equal(Object.isFrozen(EMPTY_STRING_LIST), true);
  assert.equal(Object.isFrozen(EMPTY_REACTION_CHIPS), true);
  assert.throws(() => {
    EMPTY_STRING_LIST.push("device-id");
  }, TypeError);
  assert.throws(() => {
    EMPTY_REACTION_CHIPS.length = 1;
  }, TypeError);
  assert.equal(EMPTY_STRING_LIST.length, 0);
  assert.equal(EMPTY_REACTION_CHIPS.length, 0);
});

test("空列表 prop 常量：`[] !== []` —— 这就是调用点写回字面量时会被白 patch 的那件事", () => {
  // 这条钉的是改动依赖的语言事实。有人把调用点改回 `?? []` 时它**不会**红（它判的是语言，
  // 不是那一行代码）；能照出那次回退的是 perf 量具的 ?row= 那一档 —— 边界已写进 CHANGELOG。
  assert.notEqual(EMPTY_STRING_LIST, []);
  assert.equal(EMPTY_STRING_LIST === EMPTY_STRING_LIST, true);
});
