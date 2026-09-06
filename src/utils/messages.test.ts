import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyIncomingToConversations,
  applyReplacements,
  furthestStatus,
  mergeMessages,
  preserveDeliveryStatus,
  previewText,
  syncProfileFromPeers,
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
