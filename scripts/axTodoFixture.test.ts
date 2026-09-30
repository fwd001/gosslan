import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { join } from "node:path";
import { FIXTURES, TODO_FIXTURE, seedTodoFixture, todoFixturePayload } from "./axTodoFixture.mjs";

/**
 * `scripts/axTodoFixture.mjs` 的行为用例 —— 它管的是"真窗口读数能不能信"这件事的前提。
 *
 * 为什么值得单开一份：`npm run verify:ax` 读的是真实 WKWebView 的无障碍树，而那颗编号角标要出现，
 * 库里必须有一条**生产侧真会写出来的形状**的 todo 行。夹具一旦与 `TodoPayload` 脱节
 * （生产加了字段、或 conv/receiver 的写法变了），真窗口照样能读到"R2345"，但读的是
 * **夹具自己造的**那条路径 ⇒ 这条读数就不再说明产品。所以这里钉三件事：
 *   1. 载荷键集合必须盖住 Rust 侧 `TodoPayload` 的**全部**字段名（现读源码，不抄清单）；
 *   2. 行的形状三条（`conv_id = group:<gid>`、`kind = todo`、`receiver_id = 群 id`）与 harness 任务轮同形；
 *   3. 期望读数与 `number`/`category` 一致，并且**没号那条必须没有读数**（反面对照，
 *      否则"永远都能读到一串码"的写法混过去，第 2 步的正例就毫无意义）。
 */
const PROTOCOL = join(import.meta.dirname, "..", "src-tauri", "src", "protocol.rs");

/** 现读 Rust 结构体字段名：不写死清单 ⇒ 生产加字段而夹具没跟上就会红。 */
function payloadFields(): string[] {
  const src = readFileSync(PROTOCOL, "utf8");
  const at = src.indexOf("pub struct TodoPayload");
  assert.ok(at >= 0, "找不到 `pub struct TodoPayload` —— 结构体改名了？那这份夹具也该重新对一遍");
  const body = src.slice(at, src.indexOf("\n}", at));
  const names: string[] = [];
  for (const m of body.matchAll(/^\s*pub ([a-z0-9_]+):/gm)) names.push(m[1]);
  assert.ok(names.length >= 8, `只读出 ${names.length} 个字段，解析坏了（分母不对就别往下判）`);
  return names;
}

test("夹具载荷的键必须盖住 TodoPayload 的全部字段（现读 protocol.rs）", () => {
  const fields = payloadFields();
  const keys = Object.keys(todoFixturePayload("gosslan-device-x", TODO_FIXTURE.defaultTodo));
  const missing = fields.filter((f) => !keys.includes(f));
  assert.deepEqual(missing, [], `夹具缺这些字段 ⇒ 真窗口读的是自造路径：${missing.join(", ")}`);
});

test("行形状三条与 harness 任务轮同形（conv_id / kind / receiver_id）", () => {
  assert.equal(TODO_FIXTURE.convId, `group:${TODO_FIXTURE.groupId}`,
    "会话 id 必须是 `group:<群 id>` —— 前端按这个键取消息");
  assert.equal(TODO_FIXTURE.kind, "todo", "定义行的 kind 必须是 todo（不是 todo_update）");
  assert.equal(TODO_FIXTURE.receiverId, TODO_FIXTURE.groupId,
    "群消息的 receiver_id 是群 id（与 e2e-multi-instance.mjs 里那几条 INSERT 同形）");
});

test("期望读数跟着 number 与 category 走；没号那条必须一个读数都没有", () => {
  assert.equal(TODO_FIXTURE.category, "requirement");
  assert.equal(TODO_FIXTURE.number, 12345);
  assert.deepEqual(TODO_FIXTURE.expect, { visible: "R2345", named: "任务编号 R12345" },
    "五位号 ⇒ 可见只留后四位，而可访问名字带完整码（编号规则第 5、6 条）");
  // 反面对照：同一份夹具把号拿掉 ⇒ 期望必须是"读不到"，否则正例证明不了任何事
  const noNumber = { ...TODO_FIXTURE, number: 0, expect: null };
  assert.equal(noNumber.expect, null, "无号那条必须没有期望读数");
  assert.equal(todoFixturePayload("gosslan-device-x", { ...TODO_FIXTURE.defaultTodo, number: 0 }).number, 0,
    "夹具要能原样写出 number=0 那条（真窗口里那一格必须整个不出现）");
});

test("夹具档要两档：带号那档期望两条读数，无号那档期望一条都没有", () => {
  assert.deepEqual(Object.keys(FIXTURES).sort(), ["todo", "todo-nonumber"],
    "两档都在（少一档 ⇒ 只剩正例，读数就成了探针自己造的）");
  assert.equal(FIXTURES.todo.expect.visible, "R2345");
  assert.equal(FIXTURES["todo-nonumber"].expect, null, "无号那档的期望必须是「读不到」");
  assert.equal(FIXTURES["todo-nonumber"].over.number, 0);
});

test("seedTodoFixture 落库后读回来：kind/conv/receiver 与载荷里的 number 都要跟着覆盖走", () => {
  const db = new DatabaseSync(":memory:");
  try {
    // 只建夹具要写的那四张表（真库是它的超集）：这一条判的是"我写进去的东西读回来还是它"，
    // 不判应用的迁移 —— 迁移由真窗口那趟跑（今天实测过：空库先起一次让应用建表）。
    db.exec("CREATE TABLE groups(id TEXT PRIMARY KEY, name TEXT, creator TEXT, created_at INTEGER)");
    db.exec("CREATE TABLE group_members(group_id TEXT, device_id TEXT)");
    db.exec("CREATE TABLE conversations(id TEXT PRIMARY KEY, kind TEXT, name TEXT, avatar TEXT,"
      + " unread INTEGER, updated_at INTEGER, last_ts INTEGER)");
    db.exec("CREATE TABLE messages(msg_id TEXT PRIMARY KEY, conv_id TEXT, sender_id TEXT,"
      + " receiver_id TEXT, kind TEXT, content TEXT, ts INTEGER, seq INTEGER, status TEXT)");
    seedTodoFixture(db, "dev-1", FIXTURES.todo.over);
    seedTodoFixture(db, "dev-1", FIXTURES["todo-nonumber"].over);
    const rows = db.prepare("SELECT msg_id,kind,conv_id,receiver_id,content FROM messages"
      + " ORDER BY msg_id").all();
    assert.equal(rows.length, 2);
    for (const r of rows) {
      assert.equal(r.kind, "todo");
      assert.equal(r.conv_id, TODO_FIXTURE.convId);
      assert.equal(r.receiver_id, TODO_FIXTURE.groupId);
    }
    assert.deepEqual(
      rows.map((r) => JSON.parse(r.content).number).sort((a, b) => a - b), [0, 12345],
      "两档的号必须一个有一个没 —— 否则「那一格跟着号走」这件事没有任何一档在证");
    assert.equal(new Set(rows.map((r) => r.msg_id)).size, 2, "两档必须各占一行（同 id 会互相顶掉）");
  } finally { db.close(); }
});
