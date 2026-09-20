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
  isSilentKind,
  isTipKind,
  kindClass,
} from "./messageKinds.ts";
import { TODO_STATUSES } from "./todos.ts";

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
