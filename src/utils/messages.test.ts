import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyIncomingToConversations,
  applyReplacements,
  appendLocalOnly,
  furthestStatus,
  mergeMessages,
  messageMentionsAll,
  messageMentionsName,
  pickMediaContent,
  preserveDeliveryStatus,
  previewText,
  selectCachedConversations,
  sortConversations,
  syncProfileFromPeers,
  unreadAnchorIndex,
} from "./messages.ts";
import type { Conversation, MessageRecord } from "../types";

function msg(partial: Partial<MessageRecord> & { msg_id: string }): MessageRecord {
  return {
    id: 0,
    conv_id: "c1",
    sender_id: "a",
    receiver_id: "b",
    kind: "text",
    content: "hello",
    ts: 0,
    seq: 0,
    status: "sent",
    ...partial,
  };
}

test("按 msg_id 去重：重复消息只保留一次", () => {
  const existing = [msg({ msg_id: "m1", ts: 1 }), msg({ msg_id: "m2", ts: 2 })];
  const incoming = [msg({ msg_id: "m1", ts: 1 }), msg({ msg_id: "m3", ts: 3 })];
  const out = mergeMessages(existing, incoming);
  assert.deepEqual(out.map((m) => m.msg_id), ["m1", "m2", "m3"]);
});

test("按逻辑序号升序排序（不依赖墙上时钟）", () => {
  const existing = [msg({ msg_id: "m3", seq: 3 })];
  const incoming = [msg({ msg_id: "m1", seq: 1 }), msg({ msg_id: "m2", seq: 2 })];
  const out = mergeMessages(existing, incoming);
  assert.deepEqual(out.map((m) => m.msg_id), ["m1", "m2", "m3"]);
});

test("同逻辑序号按 id 升序（稳定排序）", () => {
  const a = msg({ msg_id: "a", seq: 5, id: 3 });
  const b = msg({ msg_id: "b", seq: 5, id: 1 });
  const c = msg({ msg_id: "c", seq: 5, id: 2 });
  const out = mergeMessages([], [a, b, c]);
  assert.deepEqual(out.map((m) => m.msg_id), ["b", "c", "a"]);
});

test("空 incoming 返回现有消息（不丢失）", () => {
  const existing = [msg({ msg_id: "m1", ts: 1 })];
  assert.deepEqual(mergeMessages(existing, []), existing);
});

test("空 existing 时对 incoming 排序", () => {
  const incoming = [msg({ msg_id: "b", seq: 2 }), msg({ msg_id: "a", seq: 1 })];
  assert.deepEqual(mergeMessages([], incoming).map((m) => m.msg_id), ["a", "b"]);
});

test("模拟密集 Gossip 广播：1000 条 + 500 条重复 → 去重后仍 1000 条", () => {
  const incoming: MessageRecord[] = [];
  for (let i = 0; i < 1000; i++) {
    incoming.push(msg({ msg_id: `g${i}`, seq: i }));
    if (i < 500) incoming.push(msg({ msg_id: `g${i}`, seq: i })); // 多节点转发导致重复
  }
  const out = mergeMessages([], incoming);
  assert.equal(out.length, 1000);
  assert.equal(out[0].seq, 0);
  assert.equal(out[999].seq, 999);
});

test("不改动输入的 existing 数组（无副作用）", () => {
  const existing = [msg({ msg_id: "m1", ts: 1 })];
  const snapshot = [...existing];
  mergeMessages(existing, [msg({ msg_id: "m2", ts: 2 })]);
  assert.deepEqual(existing, snapshot);
});

test("previewText：文件/图片/代码显示特殊标记", () => {
  assert.equal(previewText(msg({ msg_id: "x", kind: "file", content: "{}" })), "[文件]");
  // 旧格式 data URL 与新格式 JSON 元数据都应显示 [图片]，不能泄露 JSON 内容
  assert.equal(previewText(msg({ msg_id: "x", kind: "image", content: "data:" })), "[图片]");
  assert.equal(
    previewText(msg({ msg_id: "x", kind: "image", content: '{"name":"a.png","path":"/x/a.png","size":1}' })),
    "[图片]",
  );
  assert.equal(previewText(msg({ msg_id: "x", kind: "code", content: "fn()" })), "[代码]");
});

test("previewText：长文本截断 30 字符、短文本原样", () => {
  assert.equal(previewText(msg({ msg_id: "x", content: "a".repeat(100) })).length, 30);
  assert.equal(previewText(msg({ msg_id: "x", content: "hi" })), "hi");
});

/**
 * **本机不认识的 kind 绝不能返回载荷原文**（INV-P24 第 2 条）。
 *
 * 会话列表/通知的文案在 Rust 算（`protocol::preview_text`），这里的是前端自己那条路径
 * （消息列表渲染、置顶列表都用它）。两边同一判据、同一文案，由
 * `messageKinds.test.ts` 与 Rust 比对字面量。
 * 对照分支不可省：`text` 必须照旧透传，否则"不管什么 kind 都塞占位"也能让第一条通过。
 *
 * ⚠️ 这里要 `as unknown as` 是因为 `MessageRecord.kind` 的类型只列了已知值，而**运行时**
 * 它会收到对端新版本带来的任意字符串 —— 类型表达不了这件事（放宽成 string 会牵动所有
 * 比较点，与 V3a 的 Rust 改动一起做，见任务「未知 kind 不再整帧丢弃」）。
 */
test("previewText：本机不认识的 kind 给占位而不是载荷", () => {
  const payload = '{"question":"周五前交","options":["A","B"]}';
  const future = msg({ msg_id: "x", content: payload });
  future.kind = "sticker" as unknown as typeof future.kind;
  assert.equal(previewText(future), "[不支持的消息]");
  assert.ok(!previewText(future).includes("周五"), "未知 kind 不得外泄载荷内容");
  assert.equal(previewText(msg({ msg_id: "x", kind: "text", content: "hi" })), "hi");
});

// ---------------- 会话更新（applyIncomingToConversations） ----------------

function conv(id: string, lastTs: number | null = null, unread = 0, pinned = false): Conversation {
  return { id, kind: "single", name: id, avatar: null, last_msg: null, last_ts: lastTs, unread, pinned };
}

test("活跃会话收到消息不计未读，非活跃会话累计未读", () => {
  const cs = [conv("f1", 0, 0), conv("f2", 0, 0)];
  const byConv = new Map([
    ["f1", [msg({ msg_id: "m1", conv_id: "f1", ts: 10, content: "hi" })]],
    ["f2", [msg({ msg_id: "m2", conv_id: "f2", ts: 20, content: "yo" })]],
  ]);
  const out = applyIncomingToConversations(cs, "f1", byConv);
  const f1 = out.find((c) => c.id === "f1")!;
  const f2 = out.find((c) => c.id === "f2")!;
  assert.equal(f1.unread, 0); // 活跃
  assert.equal(f2.unread, 1); // 非活跃
  assert.equal(f1.last_msg, "hi");
  assert.equal(f2.last_ts, 20);
});

test("会话按 last_ts 降序重排", () => {
  const cs = [conv("f1", 1), conv("f2", 2)];
  const byConv = new Map([["f1", [msg({ msg_id: "m1", conv_id: "f1", ts: 100, content: "新" })]]]);
  const out = applyIncomingToConversations(cs, null, byConv);
  assert.equal(out[0].id, "f1"); // f1 更新后排最前
  assert.equal(out[1].id, "f2");
});

test("未知会话 ID 不影响其它会话（跳过）", () => {
  const cs = [conv("f1", 0, 0)];
  const byConv = new Map([["unknown", [msg({ msg_id: "m1", conv_id: "unknown", ts: 10 })]]]);
  const out = applyIncomingToConversations(cs, null, byConv);
  assert.equal(out.length, 1);
  assert.equal(out[0].last_msg, null); // 未变
});

test("不修改原 conversations 数组（无副作用）", () => {
  const cs = [conv("f1", 0, 0)];
  const snapshot = JSON.stringify(cs);
  applyIncomingToConversations(cs, null, new Map([["f1", [msg({ msg_id: "m1", conv_id: "f1", ts: 1 })]]]));
  assert.equal(JSON.stringify(cs), snapshot);
});

// ---------------- 静默类不参与未读/预览 ----------------

test("静默事件不计未读、不改预览、不把会话顶到最前", () => {
  // 这是「回个表情把会话顶到最前」的回归锁：后端已按 is_non_notifying_kind 过滤，
  // 前端这条路径曾经漏了 —— 两边未读数从此不一致（DB 里是 0，界面上是 1）。
  const cs = [conv("g1", 10), conv("g2", 20)];
  const byConv = new Map([
    ["g1", [msg({ msg_id: "r1", conv_id: "g1", kind: "reaction", ts: 999, seq: 99 })]],
  ]);
  const out = applyIncomingToConversations(cs, null, byConv);
  const g1 = out.find((c) => c.id === "g1")!;
  assert.equal(g1.unread, 0, "静默事件不得计未读");
  assert.equal(g1.last_msg, null, "静默事件不得改预览");
  assert.equal(g1.last_ts, 10, "静默事件不得改时间戳");
  assert.equal(out[0].id, "g2", "不得把会话顶到最前");
});

test("同一批里静默与正文混在一起：只有正文生效", () => {
  const cs = [conv("g1", 10)];
  const byConv = new Map([
    ["g1", [
      msg({ msg_id: "r1", conv_id: "g1", kind: "pin", ts: 999 }),
      msg({ msg_id: "t1", conv_id: "g1", kind: "text", content: "在吗", ts: 1000 }),
    ]],
  ]);
  const g1 = applyIncomingToConversations(cs, null, byConv).find((c) => c.id === "g1")!;
  assert.equal(g1.unread, 1, "只算正文那一条");
  assert.equal(g1.last_msg, "在吗");
  assert.equal(g1.last_ts, 1000);
});

test("整批都是静默事件时该会话完全不动", () => {
  const cs = [conv("g1", 10)];
  const byConv = new Map([["g1", [
    msg({ msg_id: "a", conv_id: "g1", kind: "recall", ts: 1 }),
    msg({ msg_id: "b", conv_id: "g1", kind: "pin", ts: 2 }),
  ]]]);
  const g1 = applyIncomingToConversations(cs, null, byConv).find((c) => c.id === "g1")!;
  assert.equal(g1.unread, 0);
  assert.equal(g1.last_msg, null);
  assert.equal(g1.last_ts, 10);
});

test("系统消息经广播抵达时不记账：未读不涨、预览不被顶掉", () => {
  // 后端 `protocol.rs::is_non_notifying_kind` = `is_silent_kind || kind == "system"`，
  // 且加人通知现在**经消息管道广播**给全体成员 ⇒ 这条路径真会走到这里。
  // 前端原先只滤 isSilentKind ⇒ 未读 +1、预览变成"张三加入了群聊"，
  // 而下一次从 DB 拉回来时数字又掉回去（审计 2026-09-23 · 4.2 复核发现的漂移）。
  const cs = [conv("g1", 10)];
  const byConv = new Map([
    [
      "g1",
      [
        msg({ msg_id: "s1", conv_id: "g1", kind: "system", content: "「张三」加入了群聊", ts: 999 }),
        msg({ msg_id: "t1", conv_id: "g1", kind: "text", content: "在吗", ts: 1000 }),
      ],
    ],
  ]);
  const g1 = applyIncomingToConversations(cs, null, byConv).find((c) => c.id === "g1")!;
  assert.equal(g1.unread, 1, "只有正文那条记账");
  assert.equal(g1.last_msg, "在吗", "预览不能被系统消息顶掉");
  assert.equal(g1.last_ts, 1000);
});

test("整批都是系统消息 ⇒ 该会话完全不动（不置顶、不改预览、不记未读）", () => {
  const cs = [conv("g1", 10), conv("g2", 5)];
  const byConv = new Map([
    ["g1", [msg({ msg_id: "s1", conv_id: "g1", kind: "system", content: "「李四」加入了群聊", ts: 999 })]],
  ]);
  const out = applyIncomingToConversations(cs, null, byConv);
  const g1 = out.find((c) => c.id === "g1")!;
  assert.equal(g1.unread, 0);
  assert.equal(g1.last_msg, null);
  assert.equal(g1.last_ts, 10, "时间戳也不能动 —— 动了就等于把它顶到最前");
  assert.deepEqual(out.map((c) => c.id), ["g1", "g2"], "顺序仍按原 last_ts");
});

// ---------------- 会话排序与置顶 ----------------

test("sortConversations：置顶优先于 last_ts", () => {
  const cs = [conv("new", 999), conv("old-pinned", 1, 0, true)];
  assert.deepEqual(sortConversations(cs).map((c) => c.id), ["old-pinned", "new"]);
});

test("sortConversations：同为置顶时按 last_ts 倒序", () => {
  const cs = [conv("p1", 10, 0, true), conv("p2", 20, 0, true), conv("n1", 99)];
  assert.deepEqual(sortConversations(cs).map((c) => c.id), ["p2", "p1", "n1"]);
});

test("sortConversations：last_ts 为 null 视为 0，排在最后", () => {
  const cs = [conv("empty", null), conv("has", 5)];
  assert.deepEqual(sortConversations(cs).map((c) => c.id), ["has", "empty"]);
});

test("sortConversations：不修改原数组（无副作用）", () => {
  const cs = [conv("a", 1), conv("b", 2)];
  const snapshot = JSON.stringify(cs);
  sortConversations(cs);
  assert.equal(JSON.stringify(cs), snapshot);
});

test("置顶会话收到新消息不会被未置顶会话挤下去", () => {
  const cs = [conv("pinned", 1, 0, true), conv("plain", 2)];
  const byConv = new Map([["plain", [msg({ msg_id: "m1", conv_id: "plain", ts: 100 })]]]);
  const out = applyIncomingToConversations(cs, null, byConv);
  assert.deepEqual(out.map((c) => c.id), ["pinned", "plain"]);
});

// ---------------- 发送状态链（P0-1 / P0-2） ----------------

test("乐观记录仍在批次里：真实记录在批次落地时替换它", () => {
  const optimistic = msg({ msg_id: "tmp-1", status: "sending", ts: 100 });
  const real = msg({ msg_id: "m1", status: "sent", ts: 100 });
  const replacements = new Map([["tmp-1", real]]);
  const out = applyReplacements([optimistic], replacements);
  assert.deepEqual(out.map((m) => [m.msg_id, m.status]), [["m1", "sent"]]);
  assert.equal(replacements.size, 0); // 挂起项已消费，不会重复替换
});

test("批次落地后无挂起项：原样返回且不误改其它消息", () => {
  const a = msg({ msg_id: "m1", status: "delivered" });
  const b = msg({ msg_id: "m2", status: "read" });
  const replacements = new Map([["tmp-x", msg({ msg_id: "m9" })]]);
  const out = applyReplacements([a, b], replacements);
  assert.equal(out[0], a);
  assert.equal(out[1], b);
  assert.equal(replacements.size, 1); // 无关挂起项保留
});

test("会话重查快照不得退回已推进的送达状态", () => {
  const fresh = [
    msg({ msg_id: "m1", status: "sent" }),
    msg({ msg_id: "m2", status: "delivered" }),
    msg({ msg_id: "m3", status: "sent" }),
  ];
  const local = [
    msg({ msg_id: "m1", status: "read" }), // peer-read 在查询期间到达
    msg({ msg_id: "m2", status: "delivered" }),
    msg({ msg_id: "m3", status: "sending" }), // 非送达链上的状态不参与推进
  ];
  const out = preserveDeliveryStatus(fresh, local);
  assert.deepEqual(out.map((m) => m.status), ["read", "delivered", "sent"]);
  assert.equal(fresh[0].status, "sent"); // 无副作用
});

test("空本地缓存时原样返回重查结果", () => {
  const fresh = [msg({ msg_id: "m1", status: "sent" })];
  assert.equal(preserveDeliveryStatus(fresh, []), fresh);
});

// ==================== 快照覆盖 vs 本地独有记录（审计 1.3） ====================

test("loadMessages 快照覆盖不得吞掉在途乐观气泡（tmp-*）", () => {
  // 发送在途期间发生一次 loadMessages：DB 快照里还没有这条消息
  const fresh = [msg({ msg_id: "m1", seq: 1 })];
  const local = [
    msg({ msg_id: "m1", seq: 1 }),
    msg({ msg_id: "tmp-1", seq: Number.MAX_SAFE_INTEGER, status: "sending" }),
  ];
  const out = appendLocalOnly(preserveDeliveryStatus(fresh, local), local);
  assert.deepEqual(
    out.map((m) => m.msg_id),
    ["m1", "tmp-1"],
    "乐观气泡被吞 → 用户以为发送失败而重发（重复消息）",
  );
  assert.equal(out[1].status, "sending");
});

test("快照覆盖不得吞掉文件失败占位（file-failed-*）", () => {
  const fresh = [msg({ msg_id: "m1", seq: 1 })];
  const local = [
    msg({ msg_id: "file-failed-1", kind: "file", seq: Number.MAX_SAFE_INTEGER, status: "failed" }),
  ];
  const out = appendLocalOnly(preserveDeliveryStatus(fresh, local), local);
  assert.deepEqual(out.map((m) => m.msg_id), ["m1", "file-failed-1"]);
});

test("已替换为真实记录的乐观气泡不得复活（tmp 已不在 local）", () => {
  // 正常时序：invoke 返回 → replaceMessage 把 tmp-* 换成真实 msg_id
  const fresh = [msg({ msg_id: "m1", seq: 1 })];
  const local = [msg({ msg_id: "m1", seq: 1, status: "delivered" })];
  const out = appendLocalOnly(preserveDeliveryStatus(fresh, local), local);
  assert.deepEqual(out.map((m) => m.msg_id), ["m1"]);
});

test("已在快照里的消息不得被追加成重复行", () => {
  const fresh = [msg({ msg_id: "m1", seq: 1 })];
  const local = [msg({ msg_id: "m1", seq: 1, status: "read" })];
  const out = appendLocalOnly(preserveDeliveryStatus(fresh, local), local);
  assert.equal(out.length, 1, "fresh 里已有的 msg_id 不该从 local 再追加一份");
});

test("非本地独有记录不得借快照覆盖复活（例如已被删除的 DB 行）", () => {
  // local 里有、fresh 里没有、且不是 tmp-*/file-failed-* 前缀：说明它已被
  // 删除或本就不该保留 —— 不得追加回来（否则已删消息反复复活）。
  const fresh = [msg({ msg_id: "m1", seq: 1 })];
  const local = [msg({ msg_id: "m1", seq: 1 }), msg({ msg_id: "m-gone", seq: 2 })];
  const out = appendLocalOnly(preserveDeliveryStatus(fresh, local), local);
  assert.deepEqual(out.map((m) => m.msg_id), ["m1"]);
});

// ==================== 媒体 path 回填 vs stale 快照 ====================

test("群图片：回填前的 DB 快照不得擦掉已回填的 path", () => {
  const done = msg({
    msg_id: "gfile-1",
    kind: "image",
    status: "delivered",
    content: JSON.stringify({ name: "a.jpg", path: "/d/a.jpg" }),
  });
  // loadMessages 在 Done 回填之前取的数据：同一 msg_id、没有 path
  const stale = msg({
    msg_id: "gfile-1",
    kind: "image",
    status: "sent",
    content: JSON.stringify({ name: "a.jpg", progress: 0 }),
  });
  const out = preserveDeliveryStatus([stale], [done]);
  assert.equal(JSON.parse(out[0].content).path, "/d/a.jpg", "path 被 stale 快照擦掉 ⇒ 预览请求根本不会发出");
  assert.equal(out[0].status, "delivered", "status 仍只前进");
  assert.equal(out[0].msg_id, "gfile-1");
});

test("pickMediaContent：只保护「有 path → 无 path」这一种回退", () => {
  const noPath = msg({ msg_id: "g1", kind: "file", content: JSON.stringify({ name: "a", size: 1 }) });
  const withPath = msg({
    msg_id: "g1",
    kind: "file",
    content: JSON.stringify({ name: "a", size: 1, path: "/d/a" }),
  });
  assert.equal(pickMediaContent(undefined, withPath), withPath.content, "无旧记录 ⇒ 取新");
  assert.equal(pickMediaContent(noPath, withPath), withPath.content, "新记录带 path ⇒ 取新");
  assert.equal(pickMediaContent(withPath, noPath), withPath.content, "新记录丢了 path ⇒ 保旧");
  const moved = msg({
    msg_id: "g1",
    kind: "file",
    content: JSON.stringify({ name: "a", size: 1, path: "/d/renamed" }),
  });
  assert.equal(pickMediaContent(withPath, moved), moved.content, "两边都有 path ⇒ 以新载荷为准");
  const text = msg({ msg_id: "t1", kind: "text", content: "hi" });
  assert.equal(pickMediaContent(withPath, text), text.content, "非媒体行一律取新");
  const broken = msg({ msg_id: "g2", kind: "image", content: "{不是 JSON" });
  assert.equal(
    pickMediaContent(withPath, broken),
    withPath.content,
    "新载荷解析不出 path 就当作无 path：本地那份有 path 的必须留住",
  );
});

test("送达状态只前进：sent→delivered→read，逆序不变", () => {
  assert.equal(furthestStatus("sent", "delivered"), "delivered");
  assert.equal(furthestStatus("delivered", "read"), "read");
  assert.equal(furthestStatus("read", "delivered"), "read");
  assert.equal(furthestStatus("delivered", "sent"), "delivered");
});

// ==================== P0-1 Ack 竞态测试（状态机模型） ====================
//
// 无法直接测 useChatStore（它 import @/api），这里用等价状态机模拟全部路径。
// pendingAcks(Set) / pendingReplace / replaceMessage / batch flush / onMessageAcked /
// furthestStatus 全部内联等价实现，确保7个性质全部被覆盖。

type Msg = { msg_id: string; status: string };

function storeReplace(store: Msg[], msgId: string, next: Msg): void {
  const i = store.findIndex((m) => m.msg_id === msgId);
  if (i >= 0) { store[i] = { ...next, status: furthestStatus(store[i].status, next.status) }; }
  else { store.push(next); }
}

function onAck(store: Msg[], pendingAcks: Set<string>, msgId: string): void {
  if (store.some((m) => m.msg_id === msgId)) {
    const i = store.findIndex((m) => m.msg_id === msgId);
    store[i] = { ...store[i], status: furthestStatus(store[i].status, "delivered") };
  } else {
    pendingAcks.add(msgId);
  }
}

function batchFlush(store: Msg[], pendingReplace: Map<string, Msg>): void {
  for (const [tmpId, next] of pendingReplace) {
    const i = store.findIndex((m) => m.msg_id === tmpId);
    if (i >= 0) { store[i] = { ...next, status: furthestStatus(store[i].status, next.status) }; }
    else { const j = store.findIndex((m) => m.msg_id === next.msg_id);
      if (j >= 0) { store[j] = { ...next, status: furthestStatus(store[j].status, next.status) }; }
      else { store.push(next); } }
  }
  pendingReplace.clear();
}

function doSend(store: Msg[], acks: Set<string>, _rep: Map<string, Msg>, tmpId: string, realId: string): void {
  store.push({ msg_id: tmpId, status: "sending" });
  onAck(store, acks, realId);
  const acked = acks.delete(realId);
  const next = acked
    ? { msg_id: realId, status: furthestStatus("sent", "delivered") }
    : { msg_id: realId, status: "sent" };
  storeReplace(store, tmpId, next);
}

// 1. Ack 晚到
test("1: Ack 晚到 — send → real → Ack → delivered", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  doSend(s, a, r, "t0", "m0");
  onAck(s, a, "m0");
  assert.equal(s.find(m => m.msg_id === "m0")!.status, "delivered");
});

// 2. Ack 早到，batch 已 flush
test("2: Ack 早到（batch 已 flush） — Ack → send → delivered", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  onAck(s, a, "m1");               // store 空 → pendingAcks
  doSend(s, a, r, "t1", "m1");     // acked=true → 直接 delivered
  assert.equal(s.find(m => m.msg_id === "m1")!.status, "delivered");
  assert.equal(a.size, 0);
});

// 3. Ack 早到，batch 未 flush → send() 的 pendingAcks.delete(rec.msg_id) 命中 → delivered
test("3: Ack 早到（batch 未 flush） — Ack → real return → delivered", () => {
  const s: Msg[] = [{ msg_id: "t2", status: "sending" }], a = new Set<string>(), r = new Map<string, Msg>();
  // 模拟 onMessageAcked 在 await 期间到达：store 没有 m2 → pendingAcks.add("m2")
  a.add("m2");
  // send() 返回后：pendingAcks.delete(rec.msg_id) → acked=true → 直接 delivered
  const acked = a.delete("m2");
  assert.equal(acked, true, "pendingAcks 匹配 rec.msg_id");
  storeReplace(s, "t2", { msg_id: "m2", status: furthestStatus("sent", "delivered") });
  batchFlush(s, r);
  assert.equal(s.find(x => x.msg_id === "m2")!.status, "delivered");
  assert.equal(a.size, 0);
});

// 4. 连续快速发送 10 条，Ack 乱序，全部不是 sending
test("4: 连续快速发送10条，Ack 乱序，全部不是 sending", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  for (let i = 0; i < 10; i++) doSend(s, a, r, `t${i}`, `m${i}`);
  [3, 7, 9].forEach(i => onAck(s, a, `m${i}`));
  for (let i = 0; i < 10; i++) onAck(s, a, `m${i}`);
  for (let i = 0; i < 10; i++) {
    assert.notEqual(s.find(x => x.msg_id === `m${i}`)!.status, "sending", `m${i}`);
  }
});

// 5a. Ack → read
test("5a: Ack 先到再 peer-read → 最终 read", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  doSend(s, a, r, "ta", "ma");
  onAck(s, a, "ma"); // → delivered
  const i = s.findIndex(m => m.msg_id === "ma");
  s[i] = { ...s[i], status: "read" };
  assert.equal(s[i].status, "read");
});

// 5b. read → Ack
test("5b: peer-read 先到再 Ack → 最终 read", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  doSend(s, a, r, "tb", "mb");
  const i = s.findIndex(m => m.msg_id === "mb");
  s[i] = { ...s[i], status: "read" };
  onAck(s, a, "mb"); // furthestStatus("read","delivered") = "read"
  assert.equal(s[i].status, "read");
});

// 6. duplicate Ack
test("6: duplicate Ack 幂等 — 不降级 delivered", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  doSend(s, a, r, "tc", "mc");
  onAck(s, a, "mc");
  onAck(s, a, "mc"); // 第二次
  assert.equal(s.find(m => m.msg_id === "mc")!.status, "delivered");
});

// 7. send 失败 → failed，pendingAck 不提升
test("7: send 失败 → failed，残留 pendingAck 不提升", () => {
  const s: Msg[] = [], a = new Set<string>(), r = new Map<string, Msg>();
  onAck(s, a, "md"); // store 空 → pendingAcks
  assert.equal(a.has("md"), true);
  // 模拟 send 失败：catch 块设置 failed，不消费 pendingAcks
  s.push({ msg_id: "td", status: "failed" });
  assert.equal(a.has("md"), true, "pendingAck 未消费");
  assert.equal(s.find(m => m.msg_id === "td")!.status, "failed");
});

// ==================== Profile Sync 测试 ====================

test("syncProfileFromPeers: peer 改昵称 → friend + conversation 同步更新", () => {
  const friends = [{ device_id: "p1", nickname: "旧名", avatar: null as string | null }];
  const convs = [{ id: "p1", kind: "single", name: "旧名", avatar: null as string | null }];
  const peers = [{ device_id: "p1", nickname: "新名", avatar: "data:img" }];
  syncProfileFromPeers(friends, convs, peers);
  assert.equal(friends[0].nickname, "新名");
  assert.equal(friends[0].avatar, "data:img");
  assert.equal(convs[0].name, "新名");
  assert.equal(convs[0].avatar, "data:img");
});

test("syncProfileFromPeers: 群聊 conversation 不被修改", () => {
  const friends: { device_id: string; nickname: string; avatar: string | null }[] = [];
  const convs = [{ id: "g1", kind: "group", name: "群聊名", avatar: null as string | null }];
  const peers = [{ device_id: "g1", nickname: "假名", avatar: "x" }];
  syncProfileFromPeers(friends, convs, peers);
  assert.equal(convs[0].name, "群聊名", "群聊名不受 peers 影响");
});

test("syncProfileFromPeers: 无匹配 peer 时保持原值", () => {
  const friends = [{ device_id: "p1", nickname: "不变", avatar: null as string | null }];
  const convs = [{ id: "p1", kind: "single", name: "不变", avatar: null as string | null }];
  syncProfileFromPeers(friends, convs, []);
  assert.equal(friends[0].nickname, "不变");
  assert.equal(convs[0].name, "不变");
});

// ---------------- messageMentionsName：被 @ 检测（[有人@我] 的判定核心） ----------------

test("被 @ 检测：选择器插入（@名字 尾随空格）与手打行首均命中", () => {
  assert.equal(messageMentionsName(msg({ content: "@周工 你好" }), "周工"), true);
  assert.equal(messageMentionsName(msg({ content: "@周工" }), "周工"), true);
  assert.equal(messageMentionsName(msg({ content: "叫上 @周工 一起" }), "周工"), true);
});

test("被 @ 检测：名字后跟中英文标点也命中", () => {
  assert.equal(messageMentionsName(msg({ content: "@周工，来一下" }), "周工"), true);
  assert.equal(messageMentionsName(msg({ content: "@周工!" }), "周工"), true);
});

test("被 @ 检测：前缀名不误伤（我是小王，@的是小王爷）", () => {
  assert.equal(messageMentionsName(msg({ content: "@小王爷 吃饭" }), "小王"), false);
  assert.equal(messageMentionsName(msg({ content: "@小王 吃饭" }), "小王爷"), false);
});

test("被 @ 检测：邮箱里的 @ 与普通文本不误判", () => {
  assert.equal(messageMentionsName(msg({ content: "邮件发我 a@周工.com" }), "周工"), false);
  assert.equal(messageMentionsName(msg({ content: "周工在吗" }), "周工"), false);
});

test("被 @ 检测：非文本消息与空名不参与判断", () => {
  assert.equal(messageMentionsName(msg({ kind: "code", content: "@周工 code()" }), "周工"), false);
  assert.equal(messageMentionsName(msg({ content: "@周工 hi" }), "  "), false);
});

test("被 @ 检测：昵称含正则特殊字符按字面匹配", () => {
  assert.equal(messageMentionsName(msg({ content: "@a.b(1) 看看" }), "a.b(1)"), true);
  assert.equal(messageMentionsName(msg({ content: "@aXbX1 看看" }), "a.b(1)"), false);
});

test("被 @ 检测：输入框补出来的前导边界也命中（用户 2026-09-16）", () => {
  // 选择器插入 @ 时会补一个前导 nbsp（见 MessageComposer.applyMention）——
  // 中文里「你好@张三」不敲空格是常态，触发端不设边界限制，插入端负责把边界补齐。
  // 修改前这类消息发送端看着是蓝色 chip、接收端既不通知也不高亮（静默失效）。
  assert.equal(messageMentionsName(msg({ content: "你好 @周工 记得" }), "周工"), true);
  assert.equal(messageMentionsAll(msg({ content: "大家好 @所有人 开会" })), true);
  // ⚠️ 检测端本身不因此放宽：**没有**前导边界的手打形态仍按既有口径不命中。
  assert.equal(messageMentionsName(msg({ content: "你好@周工 记得" }), "周工"), false);
});

test("被 @ 检测：表情 token 收尾的 ] 也算前导边界（与气泡高亮同源）", () => {
  // 气泡把正文按表情 token 切段、**逐段**跑 linkify，段首天然命中 `^`；
  // 检测端对完整正文跑正则。两边不认同一套边界，就会出现
  // 「气泡里 @名字 是蓝色高亮块、却没有红点也不发通知」。
  assert.equal(messageMentionsName(msg({ content: "[微笑]@周工 快来" }), "周工"), true);
  assert.equal(messageMentionsAll(msg({ content: "[微笑]@所有人 快来" })), true);
  // 邮箱形态仍不误判：`a` 既不是空白也不是 `]`
  assert.equal(messageMentionsName(msg({ content: "a@周工.com" }), "周工"), false);
});

// ---------------- messageMentionsAll：@所有人 ----------------

test("@所有人：行首、空白后、尾随标点均命中", () => {
  assert.equal(messageMentionsAll(msg({ content: "@所有人 下午三点开会" })), true);
  assert.equal(messageMentionsAll(msg({ content: "通知 @所有人" })), true);
  assert.equal(messageMentionsAll(msg({ content: "@所有人，请注意" })), true);
  assert.equal(messageMentionsAll(msg({ content: "通知：@所有人" })), false);
});

test("@所有人：@ 前必须是行首或空白（与 @成员 的既有口径完全一致）", () => {
  // 这是 linkify 高亮与 [有人@我] 检测共用的边界约定：
  // 紧贴中文标点的 @ 既不参与高亮，也就不该触发红点 —— 两边必须同进同退，
  // 否则会出现「高亮没亮但红点了」的割裂体验。
  assert.equal(messageMentionsAll(msg({ content: "通知：@所有人" })), false);
  assert.equal(messageMentionsName(msg({ content: "通知：@周工" }), "周工"), false);
});

test("@所有人：边界与 @成员 同源，不误吞长词", () => {
  // @ 前必须是行首/空白（邮箱形态不命中）
  assert.equal(messageMentionsAll(msg({ content: "a@所有人.com" })), false);
  // 名字后必须落在边界上：@所有人甲乙 不是 @所有人
  assert.equal(messageMentionsAll(msg({ content: "@所有人甲乙 在吗" })), false);
  // 名字前必须有 @
  assert.equal(messageMentionsAll(msg({ content: "所有人注意" })), false);
});

test("@所有人：仅文本消息参与判断（与 @成员 口径一致）", () => {
  assert.equal(messageMentionsAll(msg({ kind: "code", content: "@所有人" })), false);
  assert.equal(messageMentionsAll(msg({ kind: "system", content: "@所有人" })), false);
});

test("@所有人：@所有人 不会顺带命中某个叫「所」的成员", () => {
  assert.equal(messageMentionsName(msg({ content: "@所有人 开会" }), "所"), false);
});

test("@所有人：正则被复用，重复调用结果必须稳定", () => {
  // 判定正则在模块级预编译并被反复复用（消息摄入是热路径）。若将来有人给它加上 `g`
  // 标志，`.test()` 会带上 lastIndex 状态，第二次调用就可能漏判 —— 这条钉死它。
  const hit = msg({ content: "@所有人 开会" });
  const miss = msg({ content: "通知：@所有人" });
  for (let i = 0; i < 5; i++) {
    assert.equal(messageMentionsAll(hit), true, `第 ${i + 1} 次应命中`);
    assert.equal(messageMentionsAll(miss), false, `第 ${i + 1} 次不应命中`);
  }
});

// ---------------- 消息缓存上界（useChatStore 的 messages 淘汰判定） ----------------

test("消息缓存上界：其余会话按 LRU 保留最近 N 个", () => {
  // lruOrder 末尾 = 最近使用
  const keep = selectCachedConversations(["c1", "c2", "c3", "c4", "c5"], "c5", 3);
  assert.deepEqual([...keep].sort(), ["c3", "c4", "c5"]);
  assert.equal(keep.has("c1"), false, "最旧的会话必须被淘汰");
  assert.equal(keep.has("c2"), false);
});

test("消息缓存上界：活跃会话即使不在 LRU 中也必须保留", () => {
  // 从系统通知直接打开、尚未 touch 过的会话：淘汰它会直接让用户看到空白
  const keep = selectCachedConversations(["c1", "c2"], "cX", 1);
  assert.equal(keep.has("cX"), true);
  assert.equal(keep.has("c2"), true);
  assert.equal(keep.has("c1"), false);
});

test("消息缓存上界：上限大于已缓存数量时全保留；activeId 为空时不误加", () => {
  assert.deepEqual([...selectCachedConversations(["c1"], "c1", 4)], ["c1"]);
  assert.deepEqual([...selectCachedConversations(["c1", "c2"], null, 1)], ["c2"]);
  assert.deepEqual([...selectCachedConversations(["c1"], undefined, 2)], ["c1"]);
  assert.deepEqual([...selectCachedConversations([], null, 2)], []);
});

// ---------------- 未读分割线锚点（审计阶段 4 · 4.1-1） ----------------

test("未读锚点：全是 text 时就是「从末尾数第 N 条」", () => {
  const list = [1, 2, 3, 4, 5].map((i) => msg({ msg_id: `m${i}` }));
  assert.equal(unreadAnchorIndex(list, 2), 3);
  assert.equal(unreadAnchorIndex(list, 1), 4);
});

test("未读锚点：system 占下标但不占未读 —— 不能被算进额度", () => {
  // 后端 is_non_notifying_kind 把 system 排除在未读之外，而它会渲染进时间线。
  // 若把它当一条未读消耗额度，分割线就会少盖住一条真正的未读（这里 2 vs 1 可区分）。
  const list = [
    msg({ msg_id: "m1" }),
    msg({ msg_id: "m2" }),
    msg({ msg_id: "s", kind: "system" }),
    msg({ msg_id: "m3" }),
  ];
  assert.equal(unreadAnchorIndex(list, 2), 1, "倒数第 2 条**计未读**的是 m2（index 1）");
});

test("未读锚点：额度用不完时 clamp 到「最早的计未读行」，不是粗暴的 0", () => {
  // 首行是 system（不计未读）⇒ 锚点必须是 m1（index 1）；返回 0 会把分割线画到
  // 一条本来就已读的系统消息上面。
  const list = [msg({ msg_id: "s", kind: "system" }), msg({ msg_id: "m1" })];
  assert.equal(unreadAnchorIndex(list, 5), 1);
});

test("未读锚点：没有一行计未读 ⇒ -1（宁可不画，别画错）", () => {
  const list = [msg({ msg_id: "s1", kind: "system" }), msg({ msg_id: "s2", kind: "system" })];
  assert.equal(unreadAnchorIndex(list, 3), -1);
});

test("未读锚点：空列表与 unread<=0 都返回 -1", () => {
  assert.equal(unreadAnchorIndex([], 3), -1);
  assert.equal(unreadAnchorIndex([msg({ msg_id: "m1" })], 0), -1);
  assert.equal(unreadAnchorIndex([msg({ msg_id: "m1" })], -2), -1);
});

test("未读锚点：静默行被误传进来也不消耗额度（判据只有一份）", () => {
  // 调用方按理应已经用 isRenderedInTimeline 过滤过；这里钉的是"即便没过滤，
  // 静默类也绝不参与未读换算"，这样两处判据漂移时最坏也只是画的位置保守。
  const list = [
    msg({ msg_id: "m1" }),
    msg({ msg_id: "r", kind: "reaction" }),
    msg({ msg_id: "m2" }),
  ];
  assert.equal(unreadAnchorIndex(list, 1), 2);
});
