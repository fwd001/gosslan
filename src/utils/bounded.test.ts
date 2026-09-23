import { test } from "node:test";
import assert from "node:assert/strict";
import { trimOldest } from "./bounded.ts";

test("trimOldest：没超容量时不动它、返回 0", () => {
  const m = new Map<string, number>([["a", 1], ["b", 2]]);
  assert.equal(trimOldest(m, 5), 0);
  assert.deepEqual([...m.keys()], ["a", "b"]);
  const s = new Set([1, 2, 3]);
  assert.equal(trimOldest(s, 3), 0);
  assert.equal(s.size, 3);
});

test("trimOldest：超容量时按插入序淘汰最旧的，留下的必须是最新的几条", () => {
  const m = new Map<string, number>([
    ["old", 1],
    ["mid", 2],
    ["new", 3],
  ]);
  assert.equal(trimOldest(m, 2), 1);
  assert.deepEqual([...m.keys()], ["mid", "new"]);
  assert.equal(trimOldest(m, 1), 1);
  assert.deepEqual([...m.keys()], ["new"]);
});

test("trimOldest：Map 删的是**键**而不是迭代出来的 [k,v] 对", () => {
  // 这条钉的是一个很容易写出来的错实现：`for (const x of coll) coll.delete(x)`。
  // Map 迭代给的是 `[key, value]` 数组，拿它当键 delete 什么都删不掉 ⇒
  // 循环条件永远不满足 ⇒ 要么白转一圈、要么（配 while 的版本）死循环。
  const m = new Map<number, string>([
    [1, "a"],
    [2, "b"],
    [3, "c"],
  ]);
  assert.equal(trimOldest(m, 1), 2);
  assert.deepEqual([...m.entries()], [[3, "c"]]);
});

test("trimOldest：Set 走的是值（Set 的键就是值）", () => {
  const s = new Set(["a", "b", "c", "d"]);
  assert.equal(trimOldest(s, 2), 2);
  assert.deepEqual([...s], ["c", "d"]);
});

test("trimOldest：max<=0 清空而不是静默放过（传 0 多半是配置写错）", () => {
  const m = new Map<string, number>([["a", 1]]);
  assert.equal(trimOldest(m, 0), 1);
  assert.equal(m.size, 0);
  const s = new Set([1, 2]);
  assert.equal(trimOldest(s, -5), 2);
  assert.equal(s.size, 0);
});

test("trimOldest：等值不删（size === max 是允许的稳态，不是每次都动手）", () => {
  const s = new Set([1, 2]);
  assert.equal(trimOldest(s, 2), 0);
  s.add(3);
  assert.equal(trimOldest(s, 2), 1);
  assert.deepEqual([...s], [2, 3]);
});
