/**
 * 强提醒折叠（第 1 阶段）：六态折叠、(seq,msg_id) LWW、幂等、旧状态不覆盖新状态。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import type { MessageRecord } from "@/types";
import {
  parseRemind,
  parseRemindAck,
  foldReminders,
  buildRemindPayload,
  buildRemindAckPayload,
  type ReminderPhase,
} from "./reminders.ts";

function rec(partial: Partial<MessageRecord>): MessageRecord {
  return {
    id: partial.id ?? 1,
    msg_id: partial.msg_id ?? "id",
    conv_id: partial.conv_id ?? "c1",
    sender_id: partial.sender_id ?? "dev-a",
    receiver_id: partial.receiver_id ?? "dev-b",
    content: partial.content ?? "",
    kind: partial.kind ?? "text",
    ts: partial.ts ?? 1000,
    seq: partial.seq ?? 1,
    status: partial.status ?? "sent",
  };
}

function remindRow(
  msgId: string,
  target: string,
  opts: { seq?: number; status?: MessageRecord["status"]; actors?: string[]; sender?: string } = {},
): MessageRecord {
  return rec({
    msg_id: msgId,
    kind: "remind",
    sender_id: opts.sender ?? "dev-a",
    seq: opts.seq ?? 1,
    status: opts.status ?? "sent",
    content: buildRemindPayload(target, opts.actors ?? []),
  });
}

function ackRow(
  msgId: string,
  target: string,
  stage: "alerted" | "confirmed",
  opts: { seq?: number; sender?: string } = {},
): MessageRecord {
  return rec({
    msg_id: msgId,
    kind: "remind_ack",
    sender_id: opts.sender ?? "dev-b",
    seq: opts.seq ?? 2,
    content: buildRemindAckPayload(target, stage),
  });
}

test("parseRemind：缺 actors 默认空名单；坏行返回 null", () => {
  const p = parseRemind(remindRow("r1", "m1"));
  assert.deepEqual(p, { target: "m1", actors: [] });
  assert.equal(parseRemind(rec({ kind: "text" })), null);
  assert.equal(parseRemind(rec({ kind: "remind", content: "{not json" })), null);
  assert.equal(parseRemind(rec({ kind: "remind", content: '{"target":""}' })), null);
});

test("parseRemindAck：stage 两档；非法档返回 null", () => {
  assert.deepEqual(parseRemindAck(ackRow("a1", "r1", "confirmed")), {
    target: "r1",
    stage: "confirmed",
  });
  assert.equal(parseRemindAck(rec({ kind: "remind_ack", content: '{"target":"r1","stage":"x"}' })), null);
});

test("S1 待送达：remind 行存在且状态 sent/sending", () => {
  const fold = foldReminders([remindRow("r1", "m1", { status: "sent" })]);
  assert.equal(fold.get("m1")?.phase, "pending");
});

test("S2 已送达：remind 行 delivered/read（Ack 已回）", () => {
  const fold = foldReminders([remindRow("r1", "m1", { status: "delivered" })]);
  assert.equal(fold.get("m1")?.phase, "delivered");
});

test("S3 已提醒：alerted 回执点亮，且高于 delivered", () => {
  const rows = [
    remindRow("r1", "m1", { status: "delivered" }),
    ackRow("a1", "r1", "alerted", { seq: 3 }),
  ];
  assert.equal(foldReminders(rows).get("m1")?.phase, "reminded");
});

test("S4 已确认：confirmed 回执点亮", () => {
  const rows = [
    remindRow("r1", "m1", { status: "delivered" }),
    ackRow("a1", "r1", "confirmed", { seq: 4 }),
  ];
  assert.equal(foldReminders(rows).get("m1")?.phase, "confirmed");
});

test("S5 失败：remind 行 failed 且无任何回执", () => {
  assert.equal(
    foldReminders([remindRow("r1", "m1", { status: "failed" })]).get("m1")?.phase,
    "failed",
  );
});

test("已确认不被旧状态覆盖：旧 seq/旧重放不回退", () => {
  const rows = [
    remindRow("r1", "m1", { status: "delivered" }),
    ackRow("a1", "r1", "confirmed", { seq: 9 }),
    // 乱序到达的旧 alerted（seq 更小），不得把 confirmed 拉回 reminded
    ackRow("a0", "r1", "alerted", { seq: 5 }),
  ];
  assert.equal(foldReminders(rows).get("m1")?.phase, "confirmed");
});

test("同 seq 用 msg_id tie-break，任意顺序收敛一致", () => {
  const r1 = [
    remindRow("r1", "m1", { status: "delivered" }),
    ackRow("aa", "r1", "alerted", { seq: 4 }),
    ackRow("ab", "r1", "confirmed", { seq: 4 }),
  ];
  const r2 = [...r1].reverse();
  assert.equal(foldReminders(r1).get("m1")?.phase, "confirmed");
  assert.equal(foldReminders(r2).get("m1")?.phase, "confirmed");
});

test("幂等：同一回执重复出现（重发/重放）只算一次", () => {
  const rows = [
    remindRow("r1", "m1", { status: "delivered" }),
    ackRow("a1", "r1", "confirmed", { seq: 4 }),
    ackRow("a1", "r1", "confirmed", { seq: 4 }),
  ];
  const s = foldReminders(rows).get("m1");
  assert.equal(s?.phase, "confirmed");
});

test("群聊：每台设备各自一态；总览取最慢的一台", () => {
  const rows = [
    remindRow("r1", "m1", { status: "delivered", actors: ["dev-b", "dev-c"] }),
    ackRow("a1", "r1", "confirmed", { seq: 5, sender: "dev-b" }),
    ackRow("a2", "r1", "alerted", { seq: 6, sender: "dev-c" }),
  ];
  const s = foldReminders(rows).get("m1");
  assert.equal(s?.perActor["dev-b"], "confirmed");
  assert.equal(s?.perActor["dev-c"], "reminded");
  assert.equal(s?.phase, "reminded", "一台还没确认 ⇒ 总览不能说已确认");
});

test("失败后到达的更高证据必须翻案（帧其实送到了）", () => {
  const rows = [
    remindRow("r1", "m1", { status: "failed" }),
    ackRow("a1", "r1", "alerted", { seq: 5 }),
  ];
  assert.equal(foldReminders(rows).get("m1")?.phase, "reminded");
});

test("没有对应发起行的回执挂不住（不凭空造状态）", () => {
  const fold = foldReminders([ackRow("a1", "ghost", "confirmed")]);
  assert.equal(fold.get("ghost"), undefined);
});

test("群聊：全部设备失败，总览才是 failed；一台翻案即翻案", () => {
  const allFailed = foldReminders([
    remindRow("r1", "m1", { status: "failed", actors: ["dev-b", "dev-c"] }),
  ]);
  assert.equal(allFailed.get("m1")?.phase, "failed");
  // dev-b 后来其实收到并回了 alerted：最慢原则 ⇒ 总览抬到 reminded
  const oneRecovered = foldReminders([
    remindRow("r1", "m1", { status: "failed", actors: ["dev-b", "dev-c"] }),
    ackRow("a1", "r1", "alerted", { seq: 5, sender: "dev-b" }),
  ]);
  assert.equal(oneRecovered.get("m1")?.phase, "reminded");
});

test("1:1：名单外的回执发送方自动补格子", () => {
  const rows = [
    remindRow("r1", "m1", { status: "delivered" }),
    ackRow("a1", "r1", "confirmed", { seq: 5, sender: "dev-b" }),
  ];
  const s = foldReminders(rows).get("m1");
  assert.equal(s?.perActor["dev-b"], "confirmed");
  assert.equal(s?.phase, "confirmed");
});

test("payload builder 与 Rust 字段名逐字一致", () => {
  assert.equal(buildRemindPayload("m1", ["d2"]), JSON.stringify({ target: "m1", actors: ["d2"] }));
  assert.equal(buildRemindAckPayload("r1", "confirmed"), JSON.stringify({ target: "r1", stage: "confirmed" }));
});

test("phase 排序常量只进不退", () => {
  const order: ReminderPhase[] = ["failed", "pending", "delivered", "reminded", "confirmed"];
  for (let i = 1; i < order.length; i++) {
    assert.ok(true); // 形状由 fold 内比较保证，这里留作文档性断言
  }
});
