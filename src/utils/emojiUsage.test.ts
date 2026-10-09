import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  MAX_TRACKED,
  TOP_VISIBLE,
  pickerCells,
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
 * 常用那一行与固定矩阵是**同一个序列的前后两段**，而矩阵那一段不随常用变
 * （用户 2026-10-09：「下面表情不随上面常用变化而变化」）。
 *
 * 旧做法把常用的几格从原序里摘走 ⇒ 常用攒得越多、下面洞越多。这一组判据钉的就是
 * "摘走"这件事不再发生：矩阵段永远等于全表原样，代价是同一表情会出现两格。
 */
const TABLE = [
  { file: "a.webp", displayName: "[A]" },
  { file: "b.webp", displayName: "[B]" },
  { file: "c.webp", displayName: "[C]" },
  { file: "d.webp", displayName: "[D]" },
];

test("pickerCells：矩阵段永远是全表原样，与常用攒了多少无关", () => {
  for (const usage of [
    {},
    { "[D]": { n: 9, t: 1 } },
    { "[A]": { n: 5, t: 1 }, "[C]": { n: 4, t: 2 }, "[D]": { n: 3, t: 3 } },
  ]) {
    const g = pickerCells(TABLE, usage, 2);
    assert.deepEqual(
      g.items.slice(g.frequentCount),
      TABLE,
      `常用为 ${JSON.stringify(usage)} 时矩阵段被改动过（出现洞或重排都算）`,
    );
  }
});

test("pickerCells：常用那一行排在最前、按次数取，且同一表情会出现在两格", () => {
  const g = pickerCells(TABLE, { "[C]": { n: 7, t: 1 }, "[A]": { n: 2, t: 9 } }, 2);
  assert.equal(g.frequentCount, 2);
  assert.deepEqual(
    g.items.slice(0, 2).map((e) => e.displayName),
    ["[C]", "[A]"],
    "常用段没按次数排",
  );
  // 正面：C 既在常用段、也仍在原序第 3 格（旧做法会把它从原序摘走 ⇒ 这一条就红了）
  assert.equal(g.items.filter((e) => e.displayName === "[C]").length, 2, "常用那一格没在原序里保留");
  // :key 必须能分开这两格（只用 file 会撞 key，Vue 报重复键并复用错节点）
  const keys = g.items.map((e, i) => `${i < g.frequentCount ? "f" : "m"}:${e.file}`);
  assert.equal(new Set(keys).size, keys.length, "带段号的 key 仍不唯一 ⇒ 两段划分有问题");
});

test("pickerCells：一次都没用过时没有常用段，序列就是全表", () => {
  const g = pickerCells(TABLE, {}, 8);
  assert.equal(g.frequentCount, 0);
  assert.deepEqual(g.items, TABLE);
});

/** 账里的 token 指向已被删掉的表情（换过资源）：丢掉它，但**不许**因此扰动矩阵段。 */
test("pickerCells：账里的死 token 只影响常用段，矩阵段一格不少", () => {
  const g = pickerCells(TABLE, { "[gone]": { n: 99, t: 1 }, "[B]": { n: 5, t: 1 } }, 8);
  assert.equal(g.frequentCount, 1, "死 token 不该占掉一个常用格");
  assert.equal(g.items.length, 1 + TABLE.length);
  assert.deepEqual(g.items.slice(1), TABLE);
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
 * 面板**只有一块网格**：组件里 ↑↓ 的步长是写死的 `COLS`，而它只对"同一个 grid 容器"成立。
 * 一旦有人把常用做成第二块网格，键盘跳行的落点就错了 —— 界面上只表现为"有点不对"，
 * 所以这里按形状数（网格 class 只许出现一次），并把 `COLS` 与那个 class 绑成同一条判据。
 *
 * ⚠️ 2026-10-09 这一条的**理由**要连着读准：它守的是"不许出现第二块网格"，
 * 不是"常用必须是从原序里摘出来的几格"。后者是当年那一版的实现细节，已被用户改口
 * （「下面表情不随上面常用变化」）⇒ 现在常用与矩阵是同一序列的前后两段、同一表情会出现两格，
 * 那条"摘走"的老写法反而会让上面那四条 `pickerCells` 判据红。
 */
test("面板只有一块八列网格，常用与矩阵是同一个 cells 序列的前后两段", () => {
  // ⚠️ 分母只数**真挂到 class 上**的那个：组件里 `COLS` 那行注释也写着 `grid-cols-8`，
  // 拿裸字符串数会得到 2 —— 那条红量的是我自己的注释，不是布局。
  const grids = [...PICKER.matchAll(/class="[^"]*?grid-cols-(\d+)/g)];
  assert.equal(grids.length, 1, `出现 ${grids.length} 个网格 class ⇒ 键盘步长 COLS 会对不上真实布局`);
  const cols = PICKER.match(/const COLS = (\d+)/);
  assert.ok(cols, "找不到 COLS 常数 ⇒ 上面那条判据的落点没了，判据要跟着改");
  assert.equal(Number(cols[1]), Number(grids[0][1]), "COLS 与模板里的列数不同步 ⇒ 上下键跳错行");
  assert.match(PICKER, /v-for="\(e, i\) in cells"/, "格子不是从统一的 cells 序列渲染 ⇒ 两段各画一份");
  assert.equal(
    PICKER.includes(String.raw`v-for="e in EMOJIS"`),
    false,
    "模板另起一支遍历全量表 ⇒ 常用那一行成了第二份渲染（键盘出口与染色都会分叉）",
  );
});

/**
 * 两段各有一行小标题（用户 2026-10-09 拿参照图定的：「最常使用 / 全部表情」那种分块，
 * 并明确「在没有常用数据的时候，常用表情模块消失只有全部表情」）。
 *
 * 三条各守一侧：
 * - 「常用」的标题必须**有门槛**（一次都没用过时整节消失，不留空标题）；
 * - 「全部表情」的标题必须**没门槛**（它是恒在的那一节，被一起藏掉就是另一件事）；
 * - 两个标题都必须**不是 button** —— `buttons()` 收集的是按钮，标题一旦算进下标，
 *   键盘的整行步长就错一格（↑↓ 落到不该落的地方，界面上只表现为"有点不对"）。
 */
test("两段都有小标题：常用那一节可消失、全部表情那一节恒在", () => {
  assert.match(PICKER, /col-span-8/, "小标题没占满一整行 ⇒ 它会挤掉一个格子");
  assert.match(PICKER, /\{\{ t\("emoji\.frequent"\) \}\}/, "常用那一节上面没有小标题");
  assert.match(PICKER, /\{\{ t\("emoji\.all"\) \}\}/, "矩阵那一节上面没有「全部表情」小标题");
  assert.match(PICKER, /v-if="grid\.frequentCount"/, "没有常用数据时不该出现空的「常用」标题");
  // 「全部表情」插在两段交界处：条件只许是"到没到下标边界"，不许顺带挂上 frequentCount 的真假
  assert.match(
    PICKER,
    /v-if="i === grid\.frequentCount"/,
    "「全部表情」标题没插在交界处 ⇒ 常用为空时那一节会跟着消失",
  );
  assert.ok(
    !/v-if="grid\.frequentCount"[\s\S]{0,160}t\("emoji\.all"\)/.test(PICKER),
    "「全部表情」标题被 frequentCount 挡了 ⇒ 没有常用数据时面板一句标题都不剩",
  );
  // 两个标题元素都必须不是 button。⚠️ 锚必须是**模板插值**那种写法：
  // `t("emoji.frequent")` 在脚本里也出现一次（读屏标签「常用 · [微笑]」），
  // 拿裸串去 indexOf 会先撞上那一处，切出来的"元素"根本不是标题。
  for (const key of ["emoji.frequent", "emoji.all"]) {
    const at = PICKER.indexOf(`{{ t("${key}") }}`);
    assert.ok(at >= 0, `${key} 没有作为标题插值出现`);
    const start = PICKER.lastIndexOf("<", at);
    const tag = PICKER.slice(start, at);
    assert.ok(/^<div\b/.test(tag), `${key} 的标题元素不是 div ⇒ 它可能算进 buttons() 的下标`);
    assert.ok(/col-span-8/.test(tag), `${key} 的标题没占满一整行`);
  }
  // 反空转：坏形状必须被抓到
  assert.ok(
    /^<button\b/.test("<button class=\"col-span-8\">x"),
    "上面那条 !button 判据若抓不到 button 就是空转",
  );
  assert.ok(
    !/v-if="i === grid\.frequentCount"/.test('<div v-if="grid.frequentCount">全部表情</div>'),
    "把恒在的标题改成有条件渲染，必须被那条 v-if 判据抓到",
  );
});

/**
 * 同一表情会出现两格 ⇒ 划分与键只能按**位置**判。这三条是上一轮立的，
 * 而且已经登记成永久非空转用例（`verify-guards --only=emoji` 里两条的 expect_fail_hint
 * 就指在下面那两句报错文案上）⇒ **改样式时不许顺手删掉它们**。
 */
test("两段划分按位置判、渲染键带段号（同一表情两格带来的两条）", () => {
  assert.match(PICKER, /function isFrequent\(i: number\)/, "两段划分改成按 file 判了");
  assert.match(
    PICKER,
    /function cellKey\(e: EmojiDef, i: number\)/,
    "cellKey 不再是「段号 + file」的形状",
  );
  assert.match(PICKER, /:key="cellKey\(e, i\)"/, "渲染键没带段号 ⇒ 同一表情两格会撞 key");
  // 染色本身也必须吃位置：只留着 `isFrequent` 函数、模板里改成按表情判，第一条抓不到
  //（函数在不在与谁用它，是两件事）。运行时探针再量一次真实背景色。
  assert.match(
    PICKER,
    /:class="isFrequent\(i\)/,
    "格子的染色不再吃位置 ⇒ 矩阵里同一表情那一格会被连带染色",
  );
  assert.equal(
    PICKER.includes("headFiles"),
    false,
    "又回到「按 file 集合判常用」⇒ 矩阵里那一格会被连带染色",
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
