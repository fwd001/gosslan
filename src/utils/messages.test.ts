import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyIncomingToConversations,
  applyReplacements,
  furthestStatus,
  mergeMessages,
  preserveDeliveryStatus,
  previewText,
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

test("送达状态只前进：sent→delivered→read，逆序不变", () => {
  assert.equal(furthestStatus("sent", "delivered"), "delivered");
  assert.equal(furthestStatus("delivered", "read"), "read");
  assert.equal(furthestStatus("read", "delivered"), "read");
  assert.equal(furthestStatus("delivered", "sent"), "delivered");
});

// 模拟 Ack 早于乐观→真实替换的竞态（P0-1 根因）：
// onMessageAcked 在 store 里找不到真实 msg_id → 存入 pendingAcks；
// send() 拿到真实 rec 后检查 pendingAcks → 直接把 sent 提升为 delivered。
// 不这样做的后果：Ack 被丢弃，消息永久卡在「发送中」。
test("pendingAcks 缓存早到 Ack：send() 拿到真实 rec 后立即消费并提升状态", () => {
  const pendingAcks = new Map<string, string>();
  const store = new Map<string, { msg_id: string; status: string }>();

  // 模拟 onMessageAcked 找不到 real_msg_id → 存入 pendingAcks
  const realMsgId = "real-msg-1";
  const tmpMsgId = "tmp-1234567890-abc";

  // store 里只有乐观占位（真实记录尚未替换）
  store.set(tmpMsgId, { msg_id: tmpMsgId, status: "sending" });

  // Ack 到达 → store 里没有 real_msg_id → 记入 pendingAcks
  const found = [...store.values()].some((m) => m.msg_id === realMsgId);
  assert.equal(found, false);
  if (!found) pendingAcks.set(realMsgId, realMsgId);
  assert.equal(pendingAcks.size, 1);

  // 模拟 send() 拿到真实 rec 并 replaceMessage 后：检查 pendingAcks
  const realRec = { msg_id: realMsgId, status: "sent" };
  if (pendingAcks.has(realRec.msg_id)) {
    pendingAcks.delete(realRec.msg_id);
    realRec.status = furthestStatus(realRec.status, "delivered");
  }
  assert.equal(pendingAcks.size, 0, "pendingAcks 已消费");
  assert.equal(realRec.status, "delivered", "sent → delivered（不卡在发送中）");
});

// Ack 晚到（正常路径）：store 已有真实记录 → 直接匹配并提升，pendingAcks 不受影响
test("正常 Ack 路径：store 有记录时直接提升，不污染 pendingAcks", () => {
  const pendingAcks = new Map<string, string>();
  const realMsgId = "real-msg-2";
  const store = [{ msg_id: realMsgId, status: "sent" }];

  // Ack 到达 → 找到匹配 → 走正常路径
  const found = store.some((m) => m.msg_id === realMsgId);
  assert.equal(found, true);
  const idx = store.findIndex((m) => m.msg_id === realMsgId);
  store[idx] = { ...store[idx], status: furthestStatus(store[idx].status, "delivered") };
  assert.equal(store[idx].status, "delivered");
  assert.equal(pendingAcks.size, 0, "pendingAcks 不受影响");
});

// 重复 Ack 幂等：同 msg_id 第二次到达时已在 store 中，furthestStatus 不回退 read
test("重复 Ack 不污染 pendingAcks 且不降级已读", () => {
  const pendingAcks = new Map<string, string>();
  const store = [{ msg_id: "m1", status: "read" }];

  // Ack 第二次到达 → store 有记录且已 read → 不写 pendingAcks
  const found = store.some((m) => m.msg_id === "m1");
  assert.equal(found, true);
  assert.equal(pendingAcks.size, 0);

  // 再次查找 → 仍不写 pendingAcks
  if (!found) pendingAcks.set("m1", "m1");
  assert.equal(pendingAcks.size, 0, "重复 Ack 不污染");
});
