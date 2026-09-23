import { test } from "node:test";
import assert from "node:assert/strict";
import { createInitScope } from "./initScope.ts";

test("initScope：dispose 按逆序执行全部卸载器", () => {
  const scope = createInitScope();
  const order: string[] = [];
  scope.onDispose(() => order.push("a"));
  scope.onDispose(() => order.push("b"));
  scope.dispose();
  assert.deepEqual(order, ["b", "a"], "后注册的先拆（配对卸载 LIFO）");
});

test("initScope：dispose 之后注册的东西必须立刻执行", () => {
  // 真实形状：listen() 的卸载函数要等 IPC 往返才拿到，那时本轮可能已被下一轮拆掉。
  // 攒进已废弃的列表 = 永远没人调用 = 监听真的留下了。
  const scope = createInitScope();
  scope.dispose();
  let ran = 0;
  scope.onDispose(() => ran++);
  assert.equal(ran, 1, "迟到的注册必须就地注销");
});

test("initScope：重复 dispose 不会把卸载器跑两遍", () => {
  const scope = createInitScope();
  let n = 0;
  scope.onDispose(() => n++);
  scope.dispose();
  scope.dispose();
  assert.equal(n, 1);
});

test("initScope：一个卸载器抛错不得跳过剩下的", () => {
  const scope = createInitScope();
  const ran: string[] = [];
  scope.onDispose(() => ran.push("first"));
  scope.onDispose(() => {
    throw new Error("IPC 抖动");
  });
  scope.onDispose(() => ran.push("last"));
  assert.doesNotThrow(() => scope.dispose());
  assert.deepEqual(ran, ["last", "first"]);
});

test("initScope：实战 —— 空 scope 的 dispose 不报错", () => {
  const scope = createInitScope();
  assert.doesNotThrow(() => scope.dispose());
});
