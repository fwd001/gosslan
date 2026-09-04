import { test } from "node:test";
import assert from "node:assert/strict";
import { cn } from "./cn.ts";

test("合并多个类名", () => {
  assert.equal(cn("a", "b", "c"), "a b c");
});

test("过滤 falsy 值", () => {
  assert.equal(cn("a", false, null, undefined, "", "b"), "a b");
});

test("tailwind-merge 去重冲突类（后者覆盖前者）", () => {
  assert.equal(cn("px-2", "px-4"), "px-4");
  assert.equal(cn("text-red-500", "text-blue-500"), "text-blue-500");
});

test("条件类名", () => {
  assert.equal(cn("base", true && "on", false && "off"), "base on");
});
