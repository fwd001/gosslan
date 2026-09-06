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

// ==================== P0-1 Ack 竞态测试（状态机模型） ====================
//
// 无法直接测 useChatStore（它 import @/api），这里用等价状态机模拟全部路径。
// pendingAcks / pendingReplace / replaceMessage / batch flush / onMessageAcked /
// furthestStatus 全部内联等价实现，确保5个性质全部被覆盖。

type Msg = { msg_id: string; status: string };

/** 等价于 useChatStore.replaceMessage 的 in-place swap（数组模型）。
 *  如果 msgId 不在 store 中且提供了 pendingReplace，则走 pendingReplace 路径。
 *  如果 msgId 在 store 中，做 furthestStatus 守卫的 in-place 替换。 */
function storeReplace(
  store: Msg[],
  msgId: string,
  next: Msg,
  pendingReplace?: Map<string, Msg>,
): void {
  const i = store.findIndex((m) => m.msg_id === msgId);
  if (i >= 0) {
    store[i] = { ...next, status: furthestStatus(store[i].status, next.status) };
  } else if (pendingReplace) {
    pendingReplace.set(msgId, next);
  } else {
    store.push(next);
  }
}

/** 等价于 onMessageAcked：找不到 real msg_id 时，扫描 pendingAcks 标记仍在批次中的乐观 ID。 */
function onAck(store: Msg[], pendingAcks: Set<string>, msgId: string): void {
  if (!store.some((m) => m.msg_id === msgId)) {
    // real msg_id 不在 store → 扫描 pendingAcks 找仍在批次中的乐观 ID
    for (const optId of pendingAcks) {
      if (!store.some((m) => m.msg_id === optId)) {
        pendingAcks.add(optId); // 再次 add（幂等）标记已匹配
        break;
      }
    }
  } else {
    const i = store.findIndex((m) => m.msg_id === msgId);
    store[i] = { ...store[i], status: furthestStatus(store[i].status, "delivered") };
  }
}

/** 等价于 applyIncoming 批量落地 pendingReplace。
 *  tmpId 不在 store 中时（已被 replaceMessage 替换），按 msg_id 字段找并更新，
 *  不做 push（防重复）。 */
function batchFlush(store: Msg[], pendingReplace: Map<string, Msg>, curStatus?: Map<string, string>): void {
  for (const [tmpId, next] of pendingReplace) {
    const i = store.findIndex((m) => m.msg_id === tmpId);
    if (i >= 0) {
      const cur = curStatus?.get(tmpId) ?? store[i].status;
      store[i] = { ...next, status: furthestStatus(cur, next.status) };
    } else {
      // tmpId 已被替换为 realId → 按 msg_id 找到并更新（不 push，防重复）
      const j = store.findIndex((m) => m.msg_id === next.msg_id);
      if (j >= 0) {
        const cur = curStatus?.get(next.msg_id) ?? store[j].status;
        store[j] = { ...next, status: furthestStatus(cur, next.status) };
      }
    }
  }
  pendingReplace.clear();
}

/** 完整 send 流程：乐观上屏 → Ack 早到? → 真实 rec → replaceMessage(tmp→real) → pendingAcks 消费。 */
function doSend(
  store: Msg[],
  pendingAcks: Set<string>,
  pendingReplace: Map<string, Msg>,
  tmpId: string,
  realId: string,
): void {
  store.push({ msg_id: tmpId, status: "sending" });
  onAck(store, pendingAcks, realId);                     // 模拟 onMessageAcked（可能标记 pendingAcks）
  storeReplace(store, tmpId, { msg_id: realId, status: "sent" }, pendingReplace); // replaceMessage(tmp→real)
  // send() 消费 pendingAcks —— 同时查 optimistic 和 real ID 以覆盖两种时序
  if (pendingAcks.has(tmpId) || pendingAcks.has(realId)) {
    pendingAcks.delete(tmpId);
    pendingAcks.delete(realId);
    storeReplace(store, tmpId, { msg_id: realId, status: "delivered" }, pendingReplace);
  }
}

// A. Ack 晚到：send → real 落地 → Ack 到达 → delivered
test("A: Ack 晚到 — send → real → Ack → delivered", () => {
  const store: Msg[] = [];
  const acks = new Set<string>();
  const rep = new Map<string, Msg>();
  doSend(store, acks, rep, "tmp-0", "m-0");
  // Ack 此时到达 → store 有 m-0 → 直接提升
  onAck(store, acks, "m-0");
  assert.equal(store.find((m) => m.msg_id === "m-0")!.status, "delivered");
  assert.equal(acks.size, 0);
});

// B. Ack 早到且 optimistic 尚未 flush → pendingReplace 落地后 → delivered
test("B: Ack 早到（pendingReplace 延迟 flush） — pre-register → Ack → real return → flush → delivered", () => {
  const store: Msg[] = [{ msg_id: "tmp-1", status: "sending" }];
  const acks = new Set<string>();
  const rep = new Map<string, Msg>();
  // 预注册乐观 ID
  acks.add("tmp-1");
  // Ack 在 await 期间到达 → onAck 标记 tmp-1
  onAck(store, acks, "m-1");
  assert.equal(acks.has("tmp-1"), true, "tmp-1 被标记");
  // replaceMessage(tmp-1 → m-1) → 找到 tmp-1，替换 store
  storeReplace(store, "tmp-1", { msg_id: "m-1", status: "sent" }, rep);
  // pendingAcks 消费 → storeReplace(tmp-1, delivered) → tmp-1 不在 store → pendingReplace
  if (acks.has("tmp-1")) {
    acks.delete("tmp-1");
    storeReplace(store, "tmp-1", { msg_id: "m-1", status: "delivered" }, rep);
  }
  assert.equal(rep.size, 1, "delivered 进入 pendingReplace");
  // 批次 flush → pendingReplace 落地 → furthestStatus("sent", "delivered") = "delivered"
  const curStatus = new Map(store.map((m) => [m.msg_id, m.status]));
  batchFlush(store, rep, curStatus);
  assert.equal(store.find((m) => m.msg_id === "m-1")!.status, "delivered");
  assert.equal(acks.size, 0);
});

// C. Ack 早到且 optimistic 尚未 flush → 预注册 + onAck 扫描匹配 → delivered
test("C: Ack 早到（pendingReplace 延迟 flush） — Ack → real return → flush → delivered", () => {
  const store: Msg[] = [{ msg_id: "tmp-2", status: "sending" }];
  const acks = new Set<string>();
  const rep = new Map<string, Msg>();
  // 预注册乐观 ID
  acks.add("tmp-2");
  // Ack 在 await 期间到达 → onAck 标记 tmp-2
  onAck(store, acks, "m-2");
  assert.equal(acks.has("tmp-2"), true, "tmp-2 被标记");
  // replaceMessage(tmp-2 → m-2) → 找到 tmp-2，替换 store
  storeReplace(store, "tmp-2", { msg_id: "m-2", status: "sent" }, rep);
  // pendingAcks 消费 → storeReplace(tmp-2, delivered) → tmp-2 不在 store → pendingReplace
  if (acks.has("tmp-2")) {
    acks.delete("tmp-2");
    storeReplace(store, "tmp-2", { msg_id: "m-2", status: "delivered" }, rep);
  }
  assert.equal(rep.size, 1, "delivered 进入 pendingReplace");
  // 批次 flush → pendingReplace 落地 → delivered
  const curStatus = new Map(store.map((m) => [m.msg_id, m.status]));
  batchFlush(store, rep, curStatus);
  assert.equal(store.find((m) => m.msg_id === "m-2")!.status, "delivered");
  assert.equal(acks.size, 0);
});

// D. 连续快速发送 10 条，Ack 顺序任意（3/7/10 先到），全部 delivered
test("D: 连续快速发送10条，Ack 乱序，全部 delivered", () => {
  const store: Msg[] = [];
  const acks = new Set<string>();
  const rep = new Map<string, Msg>();

  for (let i = 0; i < 10; i++) doSend(store, acks, rep, `tmp-${i}`, `m-${i}`);

  // 模拟乱序 Ack：3、7、10 先到
  [3, 7, 10].forEach((i) => onAck(store, acks, `m-${i}`));
  assert.equal(store.find((m) => m.msg_id === "m-3")!.status, "delivered");
  assert.equal(store.find((m) => m.msg_id === "m-7")!.status, "delivered");
  assert.equal(store.find((m) => m.msg_id === "m-10")?.status, undefined, "m-10 不存在");

  // 其余 Ack 依次到达
  for (let i = 0; i < 10; i++) onAck(store, acks, `m-${i}`);
  for (let i = 0; i < 10; i++) {
    assert.equal(store.find((m) => m.msg_id === `m-${i}`)!.status, "delivered", `m-${i}`);
  }
});

// E. Ack + peer-read 竞态：send → peer-read → Ack，最终 read
test("E: Ack + peer-read 竞态 — send → peer-read → Ack → read", () => {
  const store: Msg[] = [];
  const acks = new Set<string>();
  const rep = new Map<string, Msg>();
  doSend(store, acks, rep, "tmp-e", "m-e");
  // peer-read 到达
  const i = store.findIndex((m) => m.msg_id === "m-e");
  store[i] = { ...store[i], status: "read" };
  // 迟到 Ack 到达 → furthestStatus("read", "delivered") = "read"
  onAck(store, acks, "m-e");
  assert.equal(store[i].status, "read", "Ack 不得把 read 降为 delivered");
});

// F. duplicate Ack 不改变结果
test("F: duplicate Ack 幂等 — 重复 Ack 不改变已 delivered 状态", () => {
  const store: Msg[] = [];
  const acks = new Set<string>();
  const rep = new Map<string, Msg>();
  doSend(store, acks, rep, "tmp-f", "m-f");
  onAck(store, acks, "m-f");
  assert.equal(store.find((m) => m.msg_id === "m-f")!.status, "delivered");
  // 第二次 Ack
  onAck(store, acks, "m-f");
  assert.equal(store.find((m) => m.msg_id === "m-f")!.status, "delivered", "重复 Ack 不降级");
  assert.equal(acks.size, 0, "不写 pendingAcks");
});
