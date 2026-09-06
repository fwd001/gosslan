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

// ==================== P0-1 Ack 竞态测试 ====================
//
// 模拟 store 级别的 Ack 流程（无法直接测 useChatStore，因为它 import @/api），
// 但核心逻辑完全由 replaceMessage + furthestStatus + pendingAcks 三者驱动，
// 这里用等价状态机验证每条性质。

/** 模拟一次完整的 send→ack 生命周期。 */
function simulateSendAcks(
  pendingAcks: Map<string, string>,
  replaceMessage: (convId: string, msgId: string, next: { msg_id: string; status: string }) => void,
  store: { msg_id: string; status: string }[],
  convId: string,
  tmpId: string,
  realId: string,
) {
  // 1. 乐观上屏
  store.push({ msg_id: tmpId, status: "sending" });
  // 2. 模拟 onMessageAcked —— 找得到就走正常路径，找不到就存 pendingAcks
  const found = store.some((m) => m.msg_id === realId);
  if (!found) pendingAcks.set(realId, realId);
  // 3. send() 返回真实 rec → replaceMessage(tmp→real)
  replaceMessage(convId, tmpId, { msg_id: realId, status: "sent" });
  // 4. 消费 pendingAcks
  if (pendingAcks.has(realId)) {
    pendingAcks.delete(realId);
    replaceMessage(convId, realId, { msg_id: realId, status: "delivered" });
  }
}

function makeReplaceMessage(store: { msg_id: string; status: string }[]) {
  return (_convId: string, msgId: string, next: { msg_id: string; status: string }) => {
    const i = store.findIndex((m) => m.msg_id === msgId);
    if (i >= 0) {
      store[i] = { ...next, status: furthestStatus(store[i].status, next.status) };
    } else {
      store.push(next);
    }
  };
}

test("Ack 晚到：send() 返回后 store 有真实记录，onMessageAcked 直接匹配并提升", () => {
  const pendingAcks = new Map<string, string>();
  const store: { msg_id: string; status: string }[] = [];
  const replaceMessage = makeReplaceMessage(store);
  const tmpId = "tmp-1", realId = "m1";

  store.push({ msg_id: tmpId, status: "sending" });
  replaceMessage("f1", tmpId, { msg_id: realId, status: "sent" });
  // Ack 到达时 store 已有 realId → 不写 pendingAcks
  assert.equal(store.some((m) => m.msg_id === realId), true);
  assert.equal(pendingAcks.size, 0);
  assert.equal(store.find((m) => m.msg_id === realId)!.status, "sent");
});

test("Ack 早到：onMessageAcked 先存入 pendingAcks，send() 拿到 rec 后消费并提升", () => {
  const pendingAcks = new Map<string, string>();
  const store: { msg_id: string; status: string }[] = [];
  const replaceMessage = makeReplaceMessage(store);

  simulateSendAcks(pendingAcks, replaceMessage, store, "f1", "tmp-2", "m2");
  assert.equal(pendingAcks.size, 0, "pendingAcks 已消费");
  assert.equal(store.find((m) => m.msg_id === "m2")!.status, "delivered", "sent → delivered");
});

test("重复 Ack 幂等：store 已有 delivered 的消息再收到 Ack 不回退不污染", () => {
  const pendingAcks = new Map<string, string>();
  const store: { msg_id: string; status: string }[] = [];
  const replaceMessage = makeReplaceMessage(store);

  store.push({ msg_id: "m3", status: "delivered" });
  // 第一次 Ack：store 有记录 → 不写 pendingAcks，furthestStatus 不降级
  assert.equal(store.some((m) => m.msg_id === "m3"), true);
  const idx1 = store.findIndex((m) => m.msg_id === "m3");
  store[idx1] = { ...store[idx1], status: furthestStatus(store[idx1].status, "delivered") };
  assert.equal(store[idx1].status, "delivered");
  assert.equal(pendingAcks.size, 0);
  // 推进到 read 后再 Ack
  store[idx1] = { ...store[idx1], status: "read" };
  store[idx1] = { ...store[idx1], status: furthestStatus(store[idx1].status, "delivered") };
  assert.equal(store[idx1].status, "read", "read 不得退回 delivered");
});

test("连续快速发送 10 条，10 条全部进入 delivered", () => {
  const pendingAcks = new Map<string, string>();
  const store: { msg_id: string; status: string }[] = [];
  const replaceMessage = makeReplaceMessage(store);

  for (let i = 0; i < 10; i++) {
    simulateSendAcks(pendingAcks, replaceMessage, store, "f1", `tmp-${i}`, `m-${i}`);
  }
  for (let i = 0; i < 10; i++) {
    assert.equal(store.find((m) => m.msg_id === `m-${i}`)!.status, "delivered", `m-${i} 未 delivered`);
  }
  assert.equal(pendingAcks.size, 0, "pendingAcks 全部消费");
});

test("read 状态不降级：replaceMessage 的 furthestStatus 守卫防止 read → delivered", () => {
  const store: { msg_id: string; status: string }[] = [];
  const replaceMessage = makeReplaceMessage(store);

  // send() → sent
  replaceMessage("f1", "tmp-x", { msg_id: "m5", status: "sent" });
  // peer-read 到达 → 提升为 read
  replaceMessage("f1", "m5", { msg_id: "m5", status: "read" });
  // 迟到的 Ack → furthestStatus 保护：read 不得退回 delivered
  replaceMessage("f1", "m5", { msg_id: "m5", status: "delivered" });
  assert.equal(store.find((m) => m.msg_id === "m5")!.status, "read", "Ack 不得把 read 降为 delivered");
});
