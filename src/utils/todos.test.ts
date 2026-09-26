import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TODO_AUTO_ARCHIVE_DAYS,
  TODO_STATUS_BAR,
  TODO_STATUS_CLASS,
  TODO_STATUS_DEFAULT,
  TODO_STATUS_PILL,
  TODO_STATUSES,
  canArchiveOrReopenTodo,
  canEditAssignees,
  canUpdateTodo,
  foldTodos,
  isEffectivelyArchived,
  parseTodo,
  todoCompletedForCreator,
  todoMentionsMe,
  openTodosForMe,
} from "./todos.ts";
import type { TodoItem } from "./todos.ts";
import type { MessageRecord } from "../types";

function rec(kind: string, msg_id: string, sender: string, content: unknown, seq: number): MessageRecord {
  return {
    id: 0, msg_id, conv_id: "group:g1", sender_id: sender, receiver_id: "g1",
    kind: kind as MessageRecord["kind"], content: JSON.stringify(content), ts: 0, seq, status: "delivered",
  };
}
const def = (id: string, seq: number, extra: Record<string, unknown> = {}) =>
  rec("todo", `m-${id}-${seq}`, "a", { todo_id: id, title: "写周报", assignees: ["a"], status: "todo", creator: "a", deleted: false, ...extra }, seq);

test("parseTodo：畸形载荷返回 null，不抛错", () => {
  const bad = (c: string, k = "todo"): MessageRecord =>
    ({ ...def("x", 1), kind: k as MessageRecord["kind"], content: c });
  assert.equal(parseTodo(bad("{")), null);
  assert.equal(parseTodo(bad("{}")), null);                       // 缺 todo_id
  assert.equal(parseTodo(bad('{"todo_id":""}')), null);
  assert.equal(parseTodo({ ...def("x", 1), kind: "text" }), null);
});

test("parseTodo：未知或缺失的状态回落「待办」（任务不会从列表里消失）", () => {
  const noStatus = parseTodo(rec("todo", "m1", "a", { todo_id: "t1", title: "x" }, 1));
  assert.equal(noStatus?.status, TODO_STATUS_DEFAULT);
  const junk = parseTodo(rec("todo", "m2", "a", { todo_id: "t2", status: "fly" }, 1));
  assert.equal(junk?.status, TODO_STATUS_DEFAULT, "脏状态按待办处理，不整条丢掉");
});

test("折叠：创建后可列出，墓碑（deleted）不列", () => {
  assert.equal(foldTodos([def("t1", 1)]).length, 1);
  assert.equal(foldTodos([def("t1", 1, { deleted: true })]).length, 0);
});

/** 创建时间来自**创建那条记录**（`kind === "todo"`）的 ts；后续更新不改写它。 */
test("createdAt：取创建记录的 ts，且不被后续 todo_update 改写", () => {
  const T0 = 1_700_000_000_000;
  const creation = { ...def("t1", 1, { title: "周报" }), ts: T0 };
  const update = {
    ...def("t1", 5, { title: "周报" }),
    ts: T0 + 5_000,
    kind: "todo_update" as MessageRecord["kind"],
  };
  const [t] = foldTodos([creation, update]);
  assert.equal(t.createdAt, T0, "创建时间 = 创建记录的 ts（不受更新影响）");
  // 顺序无关
  assert.equal(foldTodos([update, creation])[0].createdAt, T0);
  // 没有创建记录（历史脏数据）→ null，详情里不回显这一行
  const [u] = foldTodos([
    { ...def("t2", 2, { title: "x" }), kind: "todo_update" as MessageRecord["kind"] },
  ]);
  assert.equal(u.createdAt, null);
});

test("状态改动走同一条定义（LWW）：取版本最大的那一份", () => {
  const recs = [def("t1", 1), def("t1", 9, { status: "doing" })];
  assert.equal(foldTodos(recs)[0].status, "doing");
  // 到达顺序无关（离线两端各改一次，合并结果一致）
  assert.equal(foldTodos([def("t1", 9, { status: "doing" }), def("t1", 1)])[0].status, "doing");
});

/**
 * 改动事件走的是 **`todo_update`（Silent）**，创建走 `todo`（Card）——
 * 两种 kind 载荷同构、同属一条 LWW 序列，折叠必须一视同仁。
 *
 * 为什么要拆成两个 kind（只是通知口径，不是合并语义）：若改动也用 Card，
 * 用户每改一次状态，全群就多一条未读 + 一条通知。
 */
test("折叠同时接受 todo 与 todo_update（拆分只为通知口径）", () => {
  const created = def("t1", 1, { title: "旧", status: "todo" });
  const updated = { ...def("t1", 5, { title: "新", status: "doing" }), kind: "todo_update" as MessageRecord["kind"] };
  const [t] = foldTodos([created, updated]);
  assert.equal(t.title, "新", "改动的定义必须参与折叠");
  assert.equal(t.status, "doing");
  // 反向到达也一致
  assert.equal(foldTodos([updated, created])[0].status, "doing");
  assert.equal(foldTodos([updated, created]).length, 1, "两个 kind 是同一条任务，不是两条");
});

test("同 seq 时按 msg_id 取更大者（与 Rust 的 ORDER BY seq DESC, msg_id DESC 同规则）", () => {
  const a = def("t1", 5, { status: "doing" });
  const b = { ...def("t1", 5, { status: "done" }), msg_id: "zzz" };
  assert.equal(foldTodos([a, b])[0].status, "done");
  assert.equal(foldTodos([b, a])[0].status, "done", "换顺序结果必须一致");
});

test("改标题 / 换指派人 / 删除都是同一层的 LWW", () => {
  const recs = [
    def("t1", 1, { title: "旧", assignees: ["a"] }),
    def("t1", 5, { title: "新", assignees: ["alice", "bob"] }),
  ];
  const [t] = foldTodos(recs);
  assert.equal(t.title, "新");
  assert.deepEqual(t.assignees, ["alice", "bob"]);
  assert.equal(foldTodos([...recs, def("t1", 6, { deleted: true })]).length, 0, "删除是最高版本的墓碑");
});

test("排序：按创建版本从新到旧（此前按随机 todo_id 字符串，等于没排序）", () => {
  const out = foldTodos([def("t1", 1), def("t2", 7), def("t3", 3)]);
  assert.deepEqual(out.map((t) => t.todoId), ["t2", "t3", "t1"]);
});

test("多个任务互不干扰；非任务消息被忽略", () => {
  const out = foldTodos([def("t1", 1), def("t2", 2)]);
  assert.equal(out.length, 2);
  const text: MessageRecord = { ...def("t3", 1), kind: "text", content: "hi" };
  assert.equal(foldTodos([text]).length, 0);
});

test("状态取值表：四态且缺省是待办（与 protocol.rs 的 TODO_STATUSES 对齐）", () => {
  assert.deepEqual([...TODO_STATUSES], ["todo", "doing", "overdue", "done"]);
  assert.equal(TODO_STATUS_DEFAULT, "todo");
});

/**
 * 状态配色**只有一处口径**（用户 2026-09-17 视觉走查：同一个「延期」在看板里是橙、
 * 在成员面板里是红 —— 各抄一份必然漂移）。断言「延期」落在 warning 档且不得再用 danger，
 * 并断言三张表覆盖同一组状态（漏一个就会在界面上"没有样式"）。
 */
test("状态配色口径统一：延期用 warning，不得再出现 danger", () => {
  const maps = [
    ["TODO_STATUS_CLASS", TODO_STATUS_CLASS],
    ["TODO_STATUS_PILL", TODO_STATUS_PILL],
    ["TODO_STATUS_BAR", TODO_STATUS_BAR],
  ] as const;
  for (const [name, map] of maps) {
    assert.ok(map.overdue.includes("warning"), `${name}.overdue 必须是 warning 档，实际 ${map.overdue}`);
    assert.ok(!map.overdue.includes("danger"), `${name}.overdue 不得用 danger（延期是待关注，不是错误）`);
    assert.deepEqual(
      Object.keys(map).sort(),
      [...TODO_STATUSES].sort(),
      `${name} 的状态集合必须与 TODO_STATUSES 一致`,
    );
  }
});

/**
 * 列表色条必须**只引用 token**（用户 2026-09-24 #37 的配色要求"美观合适"，
 * 而这条是它能同时成立在深色模式下的唯一方式）：写死 `#xxxxxx` 的话，
 * 浅色下好看的那支色在深色下会糊进面板底。
 */
test("状态色条只用 token，不写死色值", () => {
  for (const [status, cls] of Object.entries(TODO_STATUS_BAR)) {
    assert.match(cls, /^bg-\[var\(--gosslan-[a-z0-9-]+\)\]$/, `${status} 的色条必须是单个 token 背景`);
  }
});

/**
 * 授权矩阵 —— **必须与 Rust `commands::may_update_todo` 的用例表逐项一致**。
 * 后端是权威（真正的拦截在命令层），这份是界面显示用的镜像；两边都各自有一条用例表，
 * 改口径时两处一起改（`src-tauri/src/commands.rs` 的 `todo_update_permission_matrix`）。
 */
test("授权矩阵：创建者与群主全权 / 被指派人不能改结构 / 无关成员什么都不行", () => {
  const item = { creator: "alice", assignees: ["bob", "carol"] };
  // 创建者：改状态、改结构都行
  assert.ok(canUpdateTodo(item, "alice", "owner", false));
  assert.ok(canUpdateTodo(item, "alice", "owner", true));
  // 被指派人：能改状态，不能改结构（标题 / 删除）
  assert.ok(canUpdateTodo(item, "bob", "owner", false));
  assert.equal(canUpdateTodo(item, "bob", "owner", true), false, "被指派人不得改标题 / 删除");
  // 群主：**改什么都行**（用户 2026-09-20「群主不能编辑群任务」）
  assert.ok(canUpdateTodo(item, "owner", "owner", true));
  assert.ok(canUpdateTodo(item, "owner", "owner", false));
  // 无关成员：什么都不行
  assert.equal(canUpdateTodo(item, "dave", "owner", false), false);
  assert.equal(canUpdateTodo(item, "dave", "owner", true), false);
});

/**
 * 成员窄档（用户 2026-09-24 #37 + 同日追加：「归档和被归档的数据还原，任何人都可以操作，
 * 其他权限不变」）—— **必须与 Rust `commands::may_change_todo` 的用例表逐项一致**
 * （`todo_member_lanes_are_narrow_and_member_only`）。
 *
 * 界面按这份决定"给不给按钮"，真正的拦截在命令层；两边都收口意味着：
 * 无关成员 **只**多得到归档与还原这两个动作，改状态 / 改标题 / 删除仍然一律被拒
 * —— 所以这里同时断言"放宽没有外溢"。
 */
test("成员窄档：归档与还原对全体群成员开放，改状态/改结构仍然不行", () => {
  const item = { creator: "alice", assignees: ["bob"] };
  const members = ["alice", "bob", "carol", "dave"];
  // 无关成员 dave：归档/还原可以（就是这条放宽），改状态/改结构仍然不行。
  assert.ok(canArchiveOrReopenTodo(item, "dave", "owner", members));
  assert.equal(canUpdateTodo(item, "dave", "owner", false), false, "放宽不得外溢到改状态");
  assert.equal(canUpdateTodo(item, "dave", "owner", true), false, "放宽不得外溢到改结构");
  // 不是本群成员的第三人：连归档都不给。
  assert.equal(
    canArchiveOrReopenTodo(item, "erin", "owner", members),
    false,
    "归档/还原是群内动作，不给外人",
  );
  // 群主与被指派人本来就在前两档里（放宽与它们不冲突）。
  assert.ok(canArchiveOrReopenTodo(item, "owner", "owner", members));
  assert.ok(canArchiveOrReopenTodo(item, "bob", "owner", members));
  // device_id 还没拿到的那一帧（启动早期）不得误判成"是成员"。
  assert.equal(canArchiveOrReopenTodo(item, "", "owner", members), false, "空 actor 不算群成员");
});

/**
 * 新字段解析（2026-09-17 优化）：描述 / 图片 / 归档 / 完成时间。
 * 旧载荷（没有这些字段）必须仍能解析 —— 缺省为空串 / 空数组 / 未归档 / null。
 */
test("parseTodo：旧载荷缺新字段时回落安全默认值", () => {
  const [t] = foldTodos([def("t1", 1)]);
  assert.equal(t.description, "");
  assert.deepEqual(t.images, []);
  assert.equal(t.archived, false);
  assert.equal(t.doneAt, null);
});

test("parseTodo：新字段（描述 / 图片 / 归档 / done_at）被读取", () => {
  const img = { id: "sha1", name: "a.png", size: 10, sha256: "sha1", subtype: "png" };
  const recs = [
    def("t1", 1, { description: "第一行\n第二行", images: [img], archived: true, done_at: 1700000000000 }),
  ];
  const [t] = foldTodos(recs);
  assert.equal(t.description, "第一行\n第二行");
  assert.deepEqual(t.images, [img]);
  assert.equal(t.archived, true);
  assert.equal(t.doneAt, 1700000000000);
});

test("parseTodo：畸形图片项被过滤（没有 sha256 的丢掉）", () => {
  const recs = [def("t1", 1, { images: [{ name: "无指纹" }, { sha256: "ok" }, "junk"] })];
  const [t] = foldTodos(recs);
  assert.equal(t.images.length, 1);
  assert.equal(t.images[0].sha256, "ok");
});

/**
 * 归档口径：显式 archived 立即归档；完成（done）后超过 7 天才自动归档；
 * 未完成的旧任务永不自动归档（没有截止时间，不做"过期即归档"）。
 */
test("isEffectivelyArchived：显式归档 / 完成满 7 天自动归档 / 未完成不归档", () => {
  const DAY = 86400000;
  const now = 1_800_000_000_000;
  // 显式归档：不论状态、不论时间
  assert.equal(isEffectivelyArchived({ status: "doing", archived: true, doneAt: null }, now), true);
  // 完成但未满 7 天：不归档
  assert.equal(
    isEffectivelyArchived({ status: "done", archived: false, doneAt: now - 6 * DAY }, now),
    false,
  );
  // 完成满 7 天：自动归档
  assert.equal(
    isEffectivelyArchived(
      { status: "done", archived: false, doneAt: now - TODO_AUTO_ARCHIVE_DAYS * DAY },
      now,
    ),
    true,
  );
  // 未完成即使 doneAt 是很久以前也不归档（防御脏数据）
  assert.equal(isEffectivelyArchived({ status: "todo", archived: false, doneAt: now - 99 * DAY }, now), false);
  // 完成但没有 doneAt（旧载荷）：不自动归档，需要显式 archived
  assert.equal(isEffectivelyArchived({ status: "done", archived: false, doneAt: null }, now), false);
});

/**
 * 改指派人口径（用户 2026-09-17）：创建者 / 群主 / 当前被指派人 都能改指派人；
 * 无关成员不行。与后端 `may_update_todo` 的 `edits_assignees` 分支一致。
 */
test("canEditAssignees：创建者 / 群主 / 被指派人可改，他人不可", () => {
  const item = { creator: "alice", assignees: ["bob"] };
  assert.ok(canEditAssignees(item, "alice", "owner"));
  assert.ok(canEditAssignees(item, "owner", "owner"));
  assert.ok(canEditAssignees(item, "bob", "owner"));
  assert.equal(canEditAssignees(item, "dave", "owner"), false);
});

/** 显式 @ 指派 = 被 @：按 device id 精确匹配，昵称不参与。 */
test("todoMentionsMe：命中/未命中指派人；非任务消息永不为真", () => {
  const t = def("t1", 1, { assignees: ["bob", "carol"] });
  assert.equal(todoMentionsMe(t, "bob"), true);
  assert.equal(todoMentionsMe(t, "alice"), false);
  assert.equal(todoMentionsMe(t, ""), false, "没有本机 id 时不为真");
  // `todo_update` 同构，也要能识别（改指派人把我加进去 = 点名我）
  const upd = { ...t, kind: "todo_update" as MessageRecord["kind"] };
  assert.equal(todoMentionsMe(upd, "carol"), true);
  const text: MessageRecord = { ...t, kind: "text", content: "@bob hi" };
  assert.equal(todoMentionsMe(text, "bob"), false, "文本消息不走任务提及");
});

/**
 * 「我创建的任务被完成」判定（用户 2026-09-17：任务完成要给创建人提示）。
 * 只认 `todo_update` + `status==done` + `creator==我` + 未删除；其余一律 null（不打扰）。
 */
test("todoCompletedForCreator：只认「我创建的任务被完成」", () => {
  const done = { ...def("t1", 5, { status: "done", creator: "me" }), kind: "todo_update" as MessageRecord["kind"] };
  assert.equal(todoCompletedForCreator(done, "me"), "写周报", "命中并回传标题");

  // 创建不是我 → 不提示（即使完成了）
  const notMine = { ...def("t2", 5, { status: "done", creator: "alice" }), kind: "todo_update" as MessageRecord["kind"] };
  assert.equal(todoCompletedForCreator(notMine, "me"), null);

  // 不是完成（改状态到 doing / 改标题） → 不提示
  const doing = { ...def("t3", 5, { status: "doing", creator: "me" }), kind: "todo_update" as MessageRecord["kind"] };
  assert.equal(todoCompletedForCreator(doing, "me"), null);

  // 墓碑（删除） → 不提示
  const removed = { ...def("t4", 5, { status: "done", creator: "me", deleted: true }), kind: "todo_update" as MessageRecord["kind"] };
  assert.equal(todoCompletedForCreator(removed, "me"), null);

  // 创建消息（`todo`）不算"完成" → 不提示
  assert.equal(todoCompletedForCreator(def("t5", 1, { status: "done", creator: "me" }), "me"), null);

  // 没有本机 id → 不提示
  assert.equal(todoCompletedForCreator(done, ""), null);
});

// ---------------- 与我相关的未完成任务（蓝色徽标的唯一判据，用户 2026-09-26） ----------------
// 口径原话："除了完成和归档的其他数据都统计成一个蓝色的数值" ⇒ 排除的只有**完成**与**归档**两态，
// 其余状态（待办/进行中/延期）都要算进来。判据只写这一份，会话列表与任务图标共用。

const todoFixture = (
  over: Partial<TodoItem> & Pick<TodoItem, "todoId">,
): TodoItem => ({
  title: "",
  assignees: [],
  status: TODO_STATUS_DEFAULT,
  creator: "other",
  description: "",
  images: [],
  archived: false,
  doneAt: null,
  createdAt: null,
  ...over,
});

test("徽标只数「与我相关且还活着」：完成与归档都排掉，待办/进行中/延期都算", () => {
  const items: TodoItem[] = [
    todoFixture({ todoId: "a", assignees: ["me"] }),
    todoFixture({ todoId: "b", assignees: ["me"], status: "doing" }),
    todoFixture({ todoId: "c", assignees: ["me"], status: "overdue" }),
    todoFixture({ todoId: "d", assignees: ["me"], status: "done" }), // 完成 → 不计
    todoFixture({ todoId: "e", assignees: ["me"], archived: true }), // 手动归档 → 不计
    todoFixture({ todoId: "f", creator: "me" }), // 我创建的 → 算相关
    todoFixture({ todoId: "g", assignees: ["x"], creator: "x" }), // 与我无关 → 不计
  ];
  assert.deepEqual(
    openTodosForMe(items, "me").map((t) => t.todoId),
    ["a", "b", "c", "f"],
  );
});

test("归档优先于状态：未完成但已归档的任务也不进徽标（用户说的「除了归档」）", () => {
  const items = [todoFixture({ todoId: "z", assignees: ["me"], status: "doing", archived: true })];
  assert.deepEqual(openTodosForMe(items, "me"), []);
});

test("本机 id 还没拿到时返回空 —— 身份未就绪不许点亮一个假数字", () => {
  const items = [todoFixture({ todoId: "a", assignees: [""], creator: "" })];
  assert.deepEqual(openTodosForMe(items, ""), []);
});
