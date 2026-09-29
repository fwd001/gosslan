import { test } from "node:test";
import assert from "node:assert/strict";
import { batchIsUrgent, isUrgentNotice } from "./notifyUrgency.ts";
import type { MessageRecord, MsgKind } from "../types.ts";

/** 只填这条判据真正会读的字段；其余用不上（多填反而会把"读错了字段"伪装成通过）。 */
function rec(kind: MsgKind, content = ""): MessageRecord {
  return {
    id: 1,
    msg_id: `m-${kind}-${content.length}`,
    conv_id: "group:g1",
    sender_id: "peer",
    receiver_id: "me",
    kind,
    content,
    ts: 1,
    seq: 1,
    status: "sent",
  };
}
const todo = (priority?: string) =>
  rec("todo" as MsgKind, JSON.stringify({ todo_id: "t1", title: "x", ...(priority ? { priority } : {}) }));

test("紧急只有两个来源：群公告、紧急群任务", () => {
  assert.equal(isUrgentNotice(rec("announcement" as MsgKind)), true);
  assert.equal(isUrgentNotice(todo("high")), true);
  assert.equal(isUrgentNotice(rec("text" as MsgKind)), false);
  assert.equal(isUrgentNotice(rec("file" as MsgKind)), false);
  assert.equal(isUrgentNotice(rec("poll" as MsgKind)), false);
  // 「紧急」这一档必须与另两档不同，否则那个档位没有意义；反过来常规/不急不许点亮跳动
  assert.equal(isUrgentNotice(todo("normal")), false);
  assert.equal(isUrgentNotice(todo("low")), false);
});

/**
 * 反例那一面：`todo_update` 也要看（改状态时可以把一档改成"紧急"），
 * 而**其它 Card 类**（reaction / poll_vote / 表情回应）不许混进来 —— 判据写宽了就等于没写。
 */
test("task 的改动算，但只有 todo / todo_update 这两种；同族 Card 不算", () => {
  assert.equal(isUrgentNotice({ ...rec("todo_update" as MsgKind, todo("high").content) as MessageRecord, kind: "todo_update" }), true);
  for (const kind of ["reaction", "poll_vote", "recall", "pin", "todo_archive"]) {
    assert.equal(isUrgentNotice(rec(kind as MsgKind, JSON.stringify({ priority: "high" }))), false, `${kind} 不该算紧急`);
  }
});

/** 载荷解不开 / 缺 priority ⇒ 按不紧急处理：这条判据的失败方向必须是**少打扰**。 */
test("脏载荷不点亮跳动", () => {
  assert.equal(isUrgentNotice(rec("todo" as MsgKind, "{ 坏 JSON")), false);
  assert.equal(isUrgentNotice(rec("todo" as MsgKind, "")), false);
  assert.equal(isUrgentNotice(rec("todo" as MsgKind, JSON.stringify({ todo_id: "t1", title: "x" }))), false);
  assert.equal(isUrgentNotice(rec("todo" as MsgKind, JSON.stringify({ priority: "URGENT" }))), false, "大小写不算");
});

test("一批里有一条紧急就整批算紧急；空批不算", () => {
  assert.equal(batchIsUrgent([rec("text" as MsgKind), todo("high"), rec("text" as MsgKind)]), true);
  assert.equal(batchIsUrgent([rec("text" as MsgKind), todo("normal")]), false);
  assert.equal(batchIsUrgent([]), false, "空批必须 false：它是**每个**去抖窗口结束都会走的那条路");
});
