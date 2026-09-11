import { test } from "node:test";
import assert from "node:assert/strict";
import { GROUP_LETTERS, OTHER_GROUP, groupByInitial, initialOf } from "./nameGroup.ts";

// ---------------- 首字母 ----------------

test("拉丁字母开头 → 大写首字母", () => {
  assert.equal(initialOf("alice"), "A");
  assert.equal(initialOf("Bob"), "B");
  assert.equal(initialOf("  zoe  "), "Z");
});

test("汉字开头 → 拼音首字母（内置表，确定性，不依赖平台 ICU）", () => {
  // ⚠️ 这里**不能**写 `if (不支持) return` —— 那是空转测试（本项目明令禁止）。
  // 首字母来自生成表 `src/data/hanInitials.ts`，与运行环境的 ICU 无关，必须硬断言。
  assert.equal(initialOf("张三"), "Z");
  assert.equal(initialOf("安琪"), "A");
  assert.equal(initialOf("波波"), "B");
  assert.equal(initialOf("陈晨"), "C");
  assert.equal(initialOf("李四"), "L");
  assert.equal(initialOf("王五"), "W");
});

test("姓氏多音字取**姓读音**（这是通讯录分组正确的关键）", () => {
  // 表生成时用了 pinyin-pro 的 `surname:'all'`：普通读音会把「单」分到 D、「曾」分到 C。
  assert.equal(initialOf("单雄信"), "S");
  assert.equal(initialOf("曾国藩"), "Z");
  assert.equal(initialOf("解缙"), "X");
  assert.equal(initialOf("仇英"), "Q");
  assert.equal(initialOf("区伯"), "O");
  assert.equal(initialOf("乐毅"), "Y");
});

test("数字 / 符号 / 空名字 → # 组", () => {
  assert.equal(initialOf("123"), OTHER_GROUP);
  assert.equal(initialOf("★star"), OTHER_GROUP);
  assert.equal(initialOf(""), OTHER_GROUP);
  assert.equal(initialOf("   "), OTHER_GROUP);
  // 表情开头同样归 #（Array.from 按码点取，不会把代理对切坏）
  assert.equal(initialOf("🐱猫"), OTHER_GROUP);
});

test("代理对（emoji）不会把首字符切坏", () => {
  // 用码点取首字符：若用 name[0] 会拿到半个代理对
  const initial = initialOf("😀abc");
  assert.equal(initial, OTHER_GROUP);
});

// ---------------- 分组 ----------------

test("分组：A–Z 顺序，`#` 垫底，空组不产出", () => {
  const items = [
    { name: "zoe" },
    { name: "alice" },
    { name: "123" },
    { name: "bob" },
  ];
  const groups = groupByInitial(items, (i) => i.name);
  assert.deepEqual(
    groups.map((g) => g.letter),
    ["A", "B", "Z", OTHER_GROUP],
  );
  assert.deepEqual(groups[0].items.map((i) => i.name), ["alice"]);
  assert.deepEqual(groups[3].items.map((i) => i.name), ["123"]);
  // 没有 C–Y 的空组
  assert.ok(!groups.some((g) => g.letter === "C"));
});

test("组内按同一套规则排序（中英混排稳定）", () => {
  const items = [{ name: "bob" }, { name: "Amy" }, { name: "alice" }];
  const groups = groupByInitial(items, (i) => i.name);
  const a = groups.find((g) => g.letter === "A");
  assert.ok(a);
  assert.deepEqual(a.items.map((i) => i.name), ["alice", "Amy"]);
});

test("空输入 → 空分组（不抛错、不产出空组）", () => {
  assert.deepEqual(groupByInitial([], (i: { name: string }) => i.name), []);
});

test("GROUP_LETTERS 是 26 个字母（供 UI 做字母索引条）", () => {
  assert.equal(GROUP_LETTERS.length, 26);
  assert.equal(GROUP_LETTERS[0], "A");
  assert.equal(GROUP_LETTERS[25], "Z");
});
