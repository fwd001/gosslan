import { test } from "node:test";
import assert from "node:assert/strict";
import { applyIncomingToConversations, mergeMessages, previewText } from "./messages.ts";
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

test("按时间戳升序排序", () => {
  const existing = [msg({ msg_id: "m3", ts: 30 })];
  const incoming = [msg({ msg_id: "m1", ts: 10 }), msg({ msg_id: "m2", ts: 20 })];
  const out = mergeMessages(existing, incoming);
  assert.deepEqual(out.map((m) => m.msg_id), ["m1", "m2", "m3"]);
});

test("同时间戳按 id 升序（稳定排序）", () => {
  const a = msg({ msg_id: "a", ts: 5, id: 3 });
  const b = msg({ msg_id: "b", ts: 5, id: 1 });
  const c = msg({ msg_id: "c", ts: 5, id: 2 });
  const out = mergeMessages([], [a, b, c]);
  assert.deepEqual(out.map((m) => m.msg_id), ["b", "c", "a"]);
});

test("空 incoming 返回现有消息（不丢失）", () => {
  const existing = [msg({ msg_id: "m1", ts: 1 })];
  assert.deepEqual(mergeMessages(existing, []), existing);
});

test("空 existing 时对 incoming 排序", () => {
  const incoming = [msg({ msg_id: "b", ts: 2 }), msg({ msg_id: "a", ts: 1 })];
  assert.deepEqual(mergeMessages([], incoming).map((m) => m.msg_id), ["a", "b"]);
});

test("模拟密集 Gossip 广播：1000 条 + 500 条重复 → 去重后仍 1000 条", () => {
  const incoming: MessageRecord[] = [];
  for (let i = 0; i < 1000; i++) {
    incoming.push(msg({ msg_id: `g${i}`, ts: i }));
    if (i < 500) incoming.push(msg({ msg_id: `g${i}`, ts: i })); // 多节点转发导致重复
  }
  const out = mergeMessages([], incoming);
  assert.equal(out.length, 1000);
  assert.equal(out[0].ts, 0);
  assert.equal(out[999].ts, 999);
});

test("不改动输入的 existing 数组（无副作用）", () => {
  const existing = [msg({ msg_id: "m1", ts: 1 })];
  const snapshot = [...existing];
  mergeMessages(existing, [msg({ msg_id: "m2", ts: 2 })]);
  assert.deepEqual(existing, snapshot);
});

test("previewText：文件/图片/代码显示特殊标记", () => {
  assert.equal(previewText(msg({ msg_id: "x", kind: "file", content: "{}" })), "[文件]");
  assert.equal(previewText(msg({ msg_id: "x", kind: "image", content: "data:" })), "[图片]");
  assert.equal(previewText(msg({ msg_id: "x", kind: "code", content: "fn()" })), "[代码]");
});

test("previewText：长文本截断 30 字符、短文本原样", () => {
  assert.equal(previewText(msg({ msg_id: "x", content: "a".repeat(100) })).length, 30);
  assert.equal(previewText(msg({ msg_id: "x", content: "hi" })), "hi");
});

// ---------------- 会话更新（applyIncomingToConversations） ----------------

function conv(id: string, lastTs: number | null = null, unread = 0): Conversation {
  return { id, kind: "single", name: id, avatar: null, last_msg: null, last_ts: lastTs, unread };
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
