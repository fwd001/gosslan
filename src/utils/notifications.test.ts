import { test } from "node:test";
import assert from "node:assert/strict";
import { notificationBody } from "./notifications.ts";

test("显示正文 + 单条 → 返回正文预览", () => {
  assert.equal(
    notificationBody({ showContent: true, count: 1, sender: "张三", preview: "你好" }),
    "你好",
  );
});

test("显示正文 + 多条 → 合并提示（带昵称与条数）", () => {
  assert.equal(
    notificationBody({ showContent: true, count: 3, sender: "张三", preview: "" }),
    "张三 等 3 条新消息",
  );
});

test("隐藏正文 + 单条 → 只提示收到，不泄内容", () => {
  assert.equal(
    notificationBody({ showContent: false, count: 1, sender: "张三", preview: "机密内容" }),
    "你收到一条新消息",
  );
});

test("隐藏正文 + 多条 → 只提示收到，不泄内容（回归：隐私开关必须同时挡单条与多条）", () => {
  assert.equal(
    notificationBody({ showContent: false, count: 5, sender: "张三", preview: "机密内容" }),
    "你收到 5 条新消息",
  );
});

test("隐藏正文时正文内容完全不出现（锁屏隐私）", () => {
  const body = notificationBody({ showContent: false, count: 1, sender: "张三", preview: "银行卡号 6222" });
  assert.ok(!body.includes("6222"), `正文不得泄露，实际=${body}`);
});

// ---------------- 通知回队合并（审计阶段 4 · 4.2，flush 失败不再吞掉整批） ----------------

import { mergeNoticesInto, type QueuedNotice } from "./notifications.ts";
import type { MessageRecord } from "../types";

/** 只关心 conv_id / ts / sender_id / kind / content，其余字段填默认。 */
function rec(convId: string, ts: number, over: Partial<MessageRecord> = {}): MessageRecord {
  return {
    id: 0,
    msg_id: `m-${convId}-${ts}`,
    conv_id: convId,
    sender_id: "a",
    receiver_id: "b",
    kind: "text",
    content: "hi",
    ts,
    seq: ts,
    status: "sent",
    ...over,
  } as MessageRecord;
}
const notice = (convId: string, ts: number, count: number): QueuedNotice => ({
  count,
  last: rec(convId, ts),
});

test("回队：队列是空的 ⇒ 原样放回", () => {
  const q = new Map<string, QueuedNotice>();
  mergeNoticesInto(q, [notice("c1", 10, 2)]);
  assert.equal(q.get("c1")?.count, 2);
  assert.equal(q.get("c1")?.last.ts, 10);
});

test("回队：窗口期内该会话又来了新消息 ⇒ 条数累加、last 取更新的那条", () => {
  const q = new Map<string, QueuedNotice>([["c1", notice("c1", 30, 1)]]);
  mergeNoticesInto(q, [notice("c1", 10, 2)]); // 放回的是更早的一批
  assert.equal(q.get("c1")?.count, 3, "两段条数必须合并");
  assert.equal(q.get("c1")?.last.ts, 30, "旧批次不许把更新的 last 覆盖回去");
});

test("回队：last 更新时按新的走（通知正文与点击跳转都读它）", () => {
  const q = new Map<string, QueuedNotice>([["c1", notice("c1", 10, 1)]]);
  mergeNoticesInto(q, [notice("c1", 99, 1, )]);
  assert.equal(q.get("c1")?.last.ts, 99);
  assert.equal(q.get("c1")?.count, 2);
});

test("回队：多个会话各自成项，不互相吞", () => {
  const q = new Map<string, QueuedNotice>();
  mergeNoticesInto(q, [notice("c1", 1, 1), notice("c2", 2, 3)]);
  assert.deepEqual([...q.keys()], ["c1", "c2"]);
  assert.equal(q.get("c2")?.count, 3);
});

test("回队：空批次不动队列", () => {
  const q = new Map<string, QueuedNotice>([["c1", notice("c1", 5, 1)]]);
  mergeNoticesInto(q, []);
  assert.equal(q.size, 1);
});
