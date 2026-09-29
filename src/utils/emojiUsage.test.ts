import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  MAX_TRACKED,
  TOP_VISIBLE,
  readEmojiUsage,
  topEmojiTokens,
  withEmojiUse,
  writeEmojiUsage,
  EMOJI_USAGE_KEY,
} from "./emojiUsage.ts";

/** 最小心智模型：一个不抛的内存 KV，`throwOn` 用来模拟 localStorage 被禁用/写满。 */
function memStore(seed: Record<string, string> = {}, throwOn = false) {
  const m = new Map(Object.entries(seed));
  return {
    map: m,
    getItem(k: string) {
      if (throwOn) throw new Error("SecurityError");
      return m.get(k) ?? null;
    },
    setItem(k: string, v: string) {
      if (throwOn) throw new Error("QuotaExceededError");
      m.set(k, v);
    },
  };
}

test("计数排序：次数优先，同次数比最近，都相同按 token 定序", () => {
  const usage = {
    "[赞]": { n: 3, t: 100 },
    "[微笑]": { n: 5, t: 50 },
    "[狗头]": { n: 3, t: 900 },
    "[心]": { n: 3, t: 100 },
  };
  assert.deepEqual(topEmojiTokens(usage, 10), [
    "[微笑]", // 5 次，最多
    "[狗头]", // 3 次但最近
    "[心]", // 3 次同刻 → 字典序（顺序不许随机飘）
    "[赞]",
  ]);
});

test("上限就是上限：常用行只取 TOP_VISIBLE 个，且不重复", () => {
  const usage: Record<string, { n: number; t: number }> = {};
  for (let i = 0; i < 40; i++) usage[`[e${i}]`] = { n: i + 1, t: 1 };
  const top = topEmojiTokens(usage);
  assert.equal(top.length, TOP_VISIBLE, "条数必须正好一行的格子数");
  assert.equal(new Set(top).size, top.length, "同一个表情不许出现两次（重复渲染就是两格）");
});

/**
 * 记一次 = 次数 +1 且**不动别的条目**。
 * 这条同时是"调用点是纯函数"的正面判据：改成 `usage[token].n++` 会当场红
 * （就地改会让调用点那个 ref 的旧值一起变）。
 */
test("withEmojiUse 返回新表，不改入参", () => {
  const before = { "[赞]": { n: 2, t: 7 }, "[微笑]": { n: 1, t: 9 } };
  const after = withEmojiUse(before, "[赞]", 123);
  assert.deepEqual(after["[赞]"], { n: 3, t: 123 });
  assert.deepEqual(before["[赞]"], { n: 2, t: 7 }, "入参被就地改了");
  assert.equal(after["[微笑]"]!.n, 1, "别的表情不该被这一次带动");
});

/** 剪枝用的排序必须与面板那次是同一个比较器 ⇒ 被剪掉的只会是"排在下方的"。 */
test("超过 MAX_TRACKED 才剪，且剪掉的是最不常用的那一头", () => {
  let usage: Record<string, { n: number; t: number }> = {};
  for (let i = 0; i < MAX_TRACKED; i++) usage = withEmojiUse(usage, `[old${i}]`, 1_000 + i);
  assert.equal(Object.keys(usage).length, MAX_TRACKED);
  const next = withEmojiUse(usage, "[fresh]", 5_000);
  assert.equal(Object.keys(next).length, MAX_TRACKED, "只该维持上限，不该越剪越多");
  assert.ok(next["[fresh]"], "刚点过的必须留下");
  assert.equal(next["[old0]"], undefined, "次数并列时最久没用的那条先出局");
  assert.ok(next["[old63]"], "次数并列时最近点过的那条不该出局");
});

/** 失败方向 = 「没有常用行」，而不是白屏（面板挂在输入框上，抛一次错整块没了）。 */
test("脏数据 / 存储不可用一律退回空表，绝不抛", () => {
  const bad = [
    "not json",
    "[]",
    "null",
    String.fromCharCode(34) + "a string" + String.fromCharCode(34),
    `{"[赞]":3}`,
    '{"[a]":{"n":0,"t":1}}', // 次数 0：从没真用过
    '{"[b]":{"n":"3","t":1}}', // 次数不是数字
    '{"[c]":{"n":1,"t":"x"}}', // 时刻不是数字
    '{"[d]":{"n":null,"t":1}}', // null 会让比较器返回 NaN ⇒ 整个顺序不可预期
  ];
  for (const raw of bad) {
    assert.deepEqual(readEmojiUsage(memStore({ [EMOJI_USAGE_KEY]: raw })), {}, `输入 ${raw}`);
  }
  assert.deepEqual(readEmojiUsage(null), {});
  assert.deepEqual(readEmojiUsage(memStore({}, true)), {}, "getItem 抛错也不能连累调用方");
});

test("写失败静默丢掉，不抛（配额满 / 隐私模式）", () => {
  const boom = memStore({}, true);
  assert.doesNotThrow(() => writeEmojiUsage(boom, { "[赞]": { n: 1, t: 1 } }));
  const ok = memStore();
  writeEmojiUsage(ok, { "[赞]": { n: 4, t: 9 } });
  assert.deepEqual(readEmojiUsage(ok), { "[赞]": { n: 4, t: 9 } }, "写进去的要能原样读回来");
});

/**
 * 两处入口都必须走同一个 `EmojiPicker`（用户 2026-09-29：「全局表情选择统一组件都做」）。
 *
 * 判源码形状而不是跑 UI：这几条落在组件与调用点的接线，而单测环境没有 WebView。
 * 挡的是「以后有人在别处直接遍历 EMOJIS 自己拼一排格子」—— 那种写法编译过、类型过，
 * 只是从此那个入口既不进常用计数、也拿不到键盘出口（Esc / 方向键都在组件里）。
 */
const PICKER = readFileSync(join(import.meta.dirname, "..", "components", "EmojiPicker.vue"), "utf8");

test("常用计数挂在面板自己那一次 select 上（不是某个调用点各写一遍）", () => {
  assert.match(PICKER, /function pick\(/, "面板没有单一的选中出口 ⇒ 计数会被调用点各自复制");
  assert.match(PICKER, /withEmojiUse\(/, "选中没记进账 ⇒ 常用行永远是空的");
  assert.match(PICKER, /writeEmojiUsage\(/, "只在内存记 ⇒ 重启就丢");
  assert.match(PICKER, /emit\("select", e\.displayName\)/, "输出的还是那个 token（插入与回应共用）");
});

/**
 * 常用那一格是**重排**，不是第二块网格：组件里 ↑↓ 的步长是写死的 `COLS`，
 * 而它只对"同一个 grid 容器"成立。一旦有人把常用做成第二个网格，键盘跳行的落点就错了
 * —— 界面上只表现为"有点不对"，所以这里按形状数（网格 class 只许出现一次），
 * 并把 `COLS` 与那个 class 绑成同一条判据。
 */
test("面板只有一个八列网格，常用只是它前面的几格", () => {
  // ⚠️ 分母只数**真挂到 class 上**的那个：组件里 `COLS` 那行注释也写着 `grid-cols-8`，
  // 拿裸字符串数会得到 2 —— 那条红量的是我自己的注释，不是布局。
  const grids = [...PICKER.matchAll(/class="[^"]*?grid-cols-(\d+)/g)];
  assert.equal(grids.length, 1, `出现 ${grids.length} 个网格 class ⇒ 键盘步长 COLS 会对不上真实布局`);
  const cols = PICKER.match(/const COLS = (\d+)/);
  assert.ok(cols, "找不到 COLS 常数 ⇒ 上面那条判据的落点没了，判据要跟着改");
  assert.equal(Number(cols[1]), Number(grids[0][1]), "COLS 与模板里的列数不同步 ⇒ 上下键跳错行");
  assert.match(PICKER, /v-for="e in cells"/, "格子不是从统一的 cells 序列渲染 ⇒ 常用行没并进同一网格");
  assert.equal(
    PICKER.includes(String.raw`v-for="e in EMOJIS"`),
    false,
    "模板仍在遍历全量表 ⇒ 常用行成了第二份渲染（同一表情会出现两格）",
  );
});

/**
 * 「最常使用」必须两处入口都读得到同一份账（用户要的是"全局"，不是只给输入框加）。
 * 反面判据配正面判据：只写"不许各写一遍"的话，把两个调用点都删掉就"通过"了。
 */
test("两处表情入口都还在同一个组件上（输入框插入 / 消息回应）", () => {
  const composer = readFileSync(
    join(import.meta.dirname, "..", "components", "chat", "MessageComposer.vue"),
    "utf8",
  );
  const item = readFileSync(
    join(import.meta.dirname, "..", "components", "MessageItem.vue"),
    "utf8",
  );
  for (const [name, src] of [
    ["MessageComposer", composer],
    ["MessageItem", item],
  ] as const) {
    assert.match(src, /<EmojiPicker/, `${name} 不再用统一面板 ⇒ 那一侧没有常用行、也没有键盘出口`);
    assert.match(src, /@select=/, `${name} 的面板没有选中出口`);
  }
  // 正面：两处入口的落点各自存在（插入走 insertEmoji、回应走 onReactionPick），不是空壳
  assert.match(composer, /function insertEmoji\(/);
  assert.match(item, /function onReactionPick\(/);
});
