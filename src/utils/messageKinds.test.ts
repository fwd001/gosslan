import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import {
  BUBBLE_KINDS,
  CARD_KINDS,
  SILENT_KINDS,
  TIP_KINDS,
  UNSUPPORTED_KIND_LABEL,
  isKnownKind,
  isRemindableKind,
  isSilentKind,
  isTipKind,
  isRenderedInTimeline,
  countsTowardUnread,
  kindClass,
} from "./messageKinds.ts";
import {
  TODO_CATEGORIES,
  TODO_CATEGORY_DEFAULT,
  TODO_PRIORITIES,
  TODO_PRIORITY_DEFAULT,
  TODO_STATUSES,
  TODO_STATUS_DEFAULT,
} from "./todos.ts";

const here = dirname(fileURLToPath(import.meta.url));
const protocolRs = readFileSync(join(here, "../../src-tauri/src/protocol.rs"), "utf8");

test("未知 kind 回退 bubble（宁可多显示，不静默吞）", () => {
  assert.equal(kindClass("未来才有的新类型"), "bubble");
  assert.equal(kindClass(""), "bubble");
});

test("已知 kind 的分类", () => {
  assert.equal(kindClass("text"), "bubble");
  assert.equal(kindClass("system"), "bubble");
  assert.equal(kindClass("reaction"), "silent");
  assert.ok(isSilentKind("reaction"));
  assert.ok(!isSilentKind("text"));
});

/**
 * **跨语言契约**：Rust 的 `WIRE_KINDS` 与 TS 的 `SILENT_KINDS` 必须一致。
 *
 * 两侧判定的后果不同（Rust 决定落库/未读，TS 决定渲染/通知），一旦漂移就会出现
 * 「服务端算静默、前端照常弹通知」这类分裂行为 —— 而且只在真机跑起来才看得见。
 * 直接读源码比对，让它变成编译/测试期就能发现的问题。
 */
test("跨语言契约：kind 分类与 protocol.rs 的 WIRE_KINDS 一致", () => {
  const table = protocolRs.slice(
    protocolRs.indexOf("pub const WIRE_KINDS"),
    protocolRs.indexOf("];", protocolRs.indexOf("pub const WIRE_KINDS")),
  );
  assert.ok(table.includes("WIRE_KINDS"), "应能在 protocol.rs 找到 WIRE_KINDS 表");

  const entries = [...table.matchAll(/\("([^"]+)",\s*KindClass::(\w+)\)/g)].map(
    (m) => [m[1], m[2]] as const,
  );
  assert.ok(entries.length >= 6, `解析出的 kind 太少（${entries.length}），表格式可能变了`);

  const rustSilent = entries.filter(([, c]) => c === "Silent").map(([k]) => k).sort();
  assert.deepEqual([...SILENT_KINDS].sort(), rustSilent, "TS 与 Rust 的静默种类清单必须一致");

  const rustCard = entries.filter(([, c]) => c === "Card").map(([k]) => k).sort();
  assert.deepEqual([...CARD_KINDS].sort(), rustCard, "TS 与 Rust 的 Card 清单必须一致");

  // 内容类清单同样要一致 —— 它是 `isKnownKind` 的三条来源之一，漏一项就会把
  // 一条本机其实会渲染的消息判成「不支持」。
  const rustBubble = entries.filter(([, c]) => c === "Bubble").map(([k]) => k).sort();
  assert.deepEqual([...BUBBLE_KINDS].sort(), rustBubble, "TS 与 Rust 的内容类清单必须一致");

  // 逐个 kind 的分类也要对得上（不只是静默那一列）
  for (const [kind, cls] of entries) {
    const expected = cls === "Silent" ? "silent" : cls === "Card" ? "card" : "bubble";
    assert.equal(kindClass(kind), expected, `${kind} 的分类两侧不一致`);
  }
});

/**
 * **未知 kind 的判据与占位文案必须跨语言一致**（INV-P24 第 2 条）。
 *
 * 为什么必须机器判：会话列表与通知的文案由 Rust 算、气泡由前端算 —— 同一句"不支持"
 * 漂成两个词，用户会在两个位置看到两种说法。更危险的是一种看起来完全合理的写法：
 * 把 `isKnownKind` 实现成 `kindClass(kind) === "bubble"` —— 未知值**也**回落到 bubble，
 * 于是兜底路径永远不触发、载荷原文继续上屏，而页面上只是"少了一个占位"，没人会报 bug。
 */
test("跨语言契约：未知 kind 判据与占位文案与 protocol.rs 一致", () => {
  const label = protocolRs.match(/pub const UNSUPPORTED_PREVIEW_LABEL: &str = "([^"]+)"/)?.[1];
  assert.ok(label, "应在 protocol.rs 找到 UNSUPPORTED_PREVIEW_LABEL");
  assert.equal(UNSUPPORTED_KIND_LABEL, label, "两侧占位文案必须一字不差");

  const table = protocolRs.slice(
    protocolRs.indexOf("pub const WIRE_KINDS"),
    protocolRs.indexOf("];", protocolRs.indexOf("pub const WIRE_KINDS")),
  );
  for (const m of table.matchAll(/\("([^"]+)",\s*KindClass::\w+\)/g)) {
    assert.ok(isKnownKind(m[1]), `Rust 表里的 ${m[1]} 被前端判成未知`);
  }
  // 表外的必须判未知 —— 整条兜底路径的开关
  assert.ok(!isKnownKind("sticker"), "表里没有的 kind 必须判为未知");
  assert.ok(!isKnownKind(""), "空 kind 必须判为未知");
  // 反向对照：kindClass 对未知仍返回 bubble ⇒ 两个判据不可互相替代
  assert.equal(kindClass("sticker"), "bubble");
  assert.ok(!isKnownKind("sticker"));
});

/**
 * **跨语言契约**：任务状态取值表两侧必须一致。
 *
 * 状态用字符串在线上传（`TodoPayload.status`），后端还会用它做命令层校验
 * （`todo_status_is_valid`）。两侧漂移的后果是**静默**的：前端给一个后端不认的值时，
 * 用户会看到"改状态失败"，而代码里两边看起来都写对了 —— 所以直接读源码比对。
 */
test("跨语言契约：任务状态表与 protocol.rs 的 TODO_STATUSES 一致", () => {
  const table = protocolRs.slice(
    protocolRs.indexOf("pub const TODO_STATUSES"),
    protocolRs.indexOf("];", protocolRs.indexOf("pub const TODO_STATUSES")),
  );
  const rust = [...table.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
  assert.ok(rust.length >= 4, `解析出的状态太少（${rust.length}），表格式可能变了`);
  assert.deepEqual([...TODO_STATUSES], rust, "TS 与 Rust 的任务状态表必须一致");
});

/** 从 Rust 源码里取一张 `pub const NAME: [&str; N] = ["a", "b"];` 的表。 */
function rustTableOf(src: string, name: string): string[] {
  const at = src.indexOf(`pub const ${name}`);
  assert.ok(at > 0, `protocol.rs 里找不到 ${name} ⇒ 这张表搬家或改名，判据要跟着改`);
  const end = src.indexOf("];", at);
  assert.ok(end > at, `${name} 的表体没闭合 ⇒ 解析落点会一路读到文件末尾`);
  return [...src.slice(at, end).matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
}

/** 取 `pub fn NAME() -> String { "x".to_string() }` 里那个字面量。 */
function rustDefaultOf(src: string, fn: string): string {
  const m = src.match(new RegExp(`pub fn ${fn}\\(\\) -> String \\{\\s*"([a-z_]+)"\\.to_string\\(\\)`));
  assert.ok(m, `找不到 ${fn}() 的缺省字面量 ⇒ 回落值这条判据失去落点`);
  return m[1];
}

/**
 * 跨语言契约第二格：任务**优先级**（三档，2026-09-29 落地）。
 *
 * 与状态那条同一条理由：优先级是**字符串在线上传**的，后端命令层还会拿它做校验
 * （`todo_priority_is_valid`）。两侧漂移的表现是静默的 —— 前端给出一个后端不认的值，
 * 用户只看到"保存失败"，而两边代码各自看起来都对。
 *
 * 顺带钉**缺省值**：旧载荷没有 `priority` 时，后端解析成一份、前端 `foldTodos` 回落成另一份 ⇒
 * 同一条任务在两侧显示成不同档位（那是比"表不一致"更难发现的一种飘）。
 */
test("跨语言契约：优先级表与缺省值两侧一致", () => {
  const rust = rustTableOf(protocolRs, "TODO_PRIORITIES");
  assert.ok(rust.length >= 3, `解析出的档位太少（${rust.length}），表格式可能变了`);
  assert.deepEqual([...TODO_PRIORITIES], rust, "TS 与 Rust 的优先级表必须同序同值");
  assert.equal(
    TODO_PRIORITY_DEFAULT,
    rustDefaultOf(protocolRs, "default_todo_priority"),
    "缺省档位必须同一个",
  );
  assert.equal(
    TODO_STATUS_DEFAULT,
    rustDefaultOf(protocolRs, "default_todo_status"),
    "缺省状态必须同一个",
  );
  // 缺省值必须真的落在自己那张表里（写成一个表外的字符串会"合法地"显示不出来）
  assert.ok(rust.includes(TODO_PRIORITY_DEFAULT), "缺省档位不在表内 ⇒ 界面拿不到它的 label/class");
});

/**
 * 跨语言契约第三格：任务**类型**（三档，2026-09-29 落地）。
 *
 * 同优先级那条的理由，但这一格的漂移面更大：类型既进命令层校验（`todo_category_is_valid`），
 * 又决定前端那张 label/pill 两张表的键 —— 少一个值就是"存得进去、显示不出来"。
 * ⚠️ 缺省值必须一起钉：旧载荷没有 `category` 时两侧各回落一份 ⇒ 同一条任务在两个成员界面上
 * 一个显示「任务」角标、另一个显示空白。
 */
test("跨语言契约：任务类型表与缺省值两侧一致", () => {
  const rust = rustTableOf(protocolRs, "TODO_CATEGORIES");
  assert.ok(rust.length >= 3, `解析出的类型太少（${rust.length}），表格式可能变了`);
  assert.deepEqual([...TODO_CATEGORIES], rust, "TS 与 Rust 的类型表必须同序同值");
  assert.equal(
    TODO_CATEGORY_DEFAULT,
    rustDefaultOf(protocolRs, "default_todo_category"),
    "缺省类型必须同一个",
  );
  assert.ok(rust.includes(TODO_CATEGORY_DEFAULT), "缺省类型不在表内 ⇒ 界面拿不到它的 label/pill");
});

/**
 * 阳性对照（这几条判据不是恒过的证明）：把 Rust 那张表改一个值，比对必须变红。
 *
 * 为什么单独钉一条：跨语言比对最容易写成"两侧都从同一处读"从而永远相等；
 * 这里喂给同一个解析器一份**手动漂移过**的源码，要求它看得见差异。
 */
test("对照：解析器看得见人为造的漂移（判据不空转）", () => {
  const mutated = protocolRs.replace('["high", "normal", "low"]', '["high", "normal", "urgent"]');
  assert.notEqual(mutated, protocolRs, "替换没生效 ⇒ 这条对照什么都没测");
  assert.notDeepEqual(
    rustTableOf(mutated, "TODO_PRIORITIES"),
    [...TODO_PRIORITIES],
    "改了 Rust 那侧的值，判据必须报不一致",
  );
  const defMutated = protocolRs.replace(
    'pub fn default_todo_priority() -> String {\n    "normal"',
    'pub fn default_todo_priority() -> String {\n    "low"',
  );
  assert.notEqual(defMutated, protocolRs, "缺省值那条替换没生效 ⇒ 对照空转");
  assert.equal(rustDefaultOf(defMutated, "default_todo_priority"), "low");
  // 类型那一格同理：只改 Rust 侧的值 / 只改 Rust 侧的缺省，比对都必须看得见
  const catMutated = protocolRs.replace('["task", "requirement", "bug"]', '["task", "requirement", "defect"]');
  assert.notEqual(catMutated, protocolRs, "类型表那条替换没生效 ⇒ 对照空转");
  assert.notDeepEqual(
    rustTableOf(catMutated, "TODO_CATEGORIES"),
    [...TODO_CATEGORIES],
    "改了 Rust 那侧的类型值，判据必须报不一致",
  );
  const catDefMutated = protocolRs.replace(
    'pub fn default_todo_category() -> String {\n    "task"',
    'pub fn default_todo_category() -> String {\n    "bug"',
  );
  assert.notEqual(catDefMutated, protocolRs, "缺省类型那条替换没生效 ⇒ 对照空转");
  assert.equal(rustDefaultOf(catDefMutated, "default_todo_category"), "bug");
});

/**
 * 提示行（微信式居中灰字）的判定**只能有一个来源**。
 *
 * 为什么必须守：`isTipKind` 直接决定两件事 —— 渲染分支（有没有头像/气泡）与
 * **高度估算**（`messageHeight` 还要据此决定要不要留昵称行）。两边各写一份
 * `kind === "system"` 时，`recalled` 就被漏掉：群聊里一条"对方撤回"会多估 18px，
 * 虚拟列表把下面那条推偏、两条消息互相遮挡 —— 而这只在群里、有对方撤回时现形。
 */
test("提示行的判定只有一个来源（MessageItem 与 messageHeight 都走 isTipKind）", () => {
  const read = (p: string) => readFileSync(join(here, p), "utf8");
  const messageItem = read("../../src/components/MessageItem.vue");
  const messageHeight = read("./messageHeight.ts");

  assert.ok(TIP_KINDS.includes("system") && TIP_KINDS.includes("recalled"), "系统消息与撤回都算提示行");
  assert.ok(isTipKind("system") && isTipKind("recalled") && !isTipKind("text"));

  for (const [name, src] of [["MessageItem.vue", messageItem], ["messageHeight.ts", messageHeight]] as const) {
    assert.match(src, /isTipKind\(/, `${name} 必须用 isTipKind 判定提示行（不要自己比 kind 字面量）`);
    assert.ok(
      !/kind === "system"/.test(src),
      `${name} 不得再自己写 kind === "system" —— 那样会漏掉 recalled（两边漂移即高度估算出错）`,
    );
  }
  // 提示行没有昵称行 ⇒ 高度估算必须跟着排除（渲染侧整支都在头像行之外）
  assert.match(
    messageHeight,
    /showNickname =[\s\S]{0,120}isTipKind\(m\.kind\)/,
    "提示行不计昵称行 —— 不排除就与渲染对不上",
  );
});

// ── §30 那条回归：「自己创建的群任务在聊天里看不见」（2026-09-26 用户实测报出，#82 已修）──
// 根因不在渲染组件，而在**同一件事有两份口径**：`messageKinds.ts` 的注释当时写着"Card 不进时间线"，
// 与 `isRenderedInTimeline("todo") === true` 相反 ⇒ "看不见"被当成设计如此，没人去查。
// 所以这三条钉的不是"todo 恰好可见"这一个点，而是**逼着每一次改动都必须做一次决定**：
// ① 逐项判决表要覆盖全部已知 kind（新增一种而这里没登记 ⇒ 当场红）；
// ② 时间线过滤只有一个来源（ChatWindow 不许再写一份 kind 名单，否则未读分割线会画错消息）；
// ③ 过滤出来了还得有人渲染它（摘掉专门卡片就退回原始 JSON，那是这条 bug 的另一半）。
const VISIBLE_IN_TIMELINE = [
  "text", "code", "image", "file", "system", "recalled", "merge", "todo",
];
const NOT_IN_TIMELINE = [...SILENT_KINDS, "announcement", "poll"];

test("时间线可见性逐项判决表：新增一种 kind 必须在这里决定可不可见", () => {
  assert.deepEqual(
    [...new Set([...VISIBLE_IN_TIMELINE, ...NOT_IN_TIMELINE])].sort(),
    [...new Set([...BUBBLE_KINDS, ...SILENT_KINDS, ...CARD_KINDS])].sort(),
    "判决表与已知 kind 全集对不上 —— 新增长出来的那一种必须被显式判一次，不许默认不可见",
  );
  for (const k of VISIBLE_IN_TIMELINE) {
    assert.equal(isRenderedInTimeline(k), true, `${k} 必须在时间线里可见`);
  }
  for (const k of NOT_IN_TIMELINE) {
    assert.equal(isRenderedInTimeline(k), false, `${k} 不该出现在时间线里（各有各的去处）`);
  }
  // 未知 kind 走 bubble 兜底 ⇒ 默认可见：宁可多显示一条，也不静默吞掉对端的新内容
  assert.equal(isRenderedInTimeline("a_kind_from_a_newer_version"), true);
});

test("「不进时间线」的那两种 Card 各自有具名去处，不是没人显示", () => {
  const read = (p: string) => readFileSync(join(here, p), "utf8");
  // announcement → 聊天窗顶部的公告条（与时间线是两条路，所以它不进过滤结果）
  const chat = read("../../src/components/ChatWindow.vue");
  assert.match(
    chat,
    /m\.kind !== "announcement"/,
    "公告条的挑选判据变了 —— 那意味着 announcement 没有了具名去处，先补呈现再改这条",
  );
  // poll → **本客户端今天发不出投票**：api 层没有任何发投票的包装。
  // 这条钉的是"发不出去"这个前提本身 —— 哪天真加上入口，它会红，逼着做一次决定，
  // 而不是让新种出来的 poll 静默落在时间线之外（= 复现 #82 的形状）。
  const api = read("../../src/api/index.ts");
  assert.doesNotMatch(
    api,
    /invoke<[^>]*>\(\s*"send_group_poll"/,
    "出现发投票的入口了 ⇒ 先决定 poll 到底在不在时间线渲染，再改 VISIBLE_IN_TIMELINE",
  );
});

test("时间线过滤只有一份判据，且 todo 卡片真的在渲染链里", () => {
  const read = (p: string) => readFileSync(join(here, p), "utf8");
  const chat = read("../../src/components/ChatWindow.vue");
  assert.match(
    chat,
    /filter\(\(m\) => isRenderedInTimeline\(m\.kind\)\)/,
    "ChatWindow 的时间线过滤不再走 isRenderedInTimeline ⇒ 与 store 侧算未读分割线的那份会漂移",
  );
  const item = read("../../src/components/MessageItem.vue");
  assert.match(item, /TodoCardBubble/, "todo 的专门卡片被摘掉了 —— 时间线里会退回原始 JSON");
});

test("强提醒：发起/回执是静默 kind（不计未读、不进时间线、不走普通通知）", () => {
  for (const k of ["remind", "remind_ack"]) {
    assert.ok(SILENT_KINDS.includes(k), `${k} 必须在静默清单里`);
    assert.equal(isSilentKind(k), true);
    assert.equal(isRenderedInTimeline(k), false, `${k} 不进时间线（标识挂在原消息上）`);
    assert.equal(countsTowardUnread(k), false, `${k} 不计未读`);
    assert.ok(isKnownKind(k), `${k} 必须是本机认识的 kind（不能走未知兜底）`);
  }
});

test("isRemindableKind：只有有实质内容的消息可被强提醒；提示行/静默事件不行", () => {
  for (const k of ["text", "code", "image", "file", "merge", "poll", "todo", "announcement"]) {
    assert.equal(isRemindableKind(k), true, `${k} 应当可提醒`);
  }
  for (const k of ["system", "recalled", "remind", "remind_ack", "reaction", "pin", ""]) {
    assert.equal(isRemindableKind(k), false, `${k} 不应当可提醒`);
  }
});
