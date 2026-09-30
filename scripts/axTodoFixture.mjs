/**
 * `npm run verify:ax` 的**读数夹具**：往一个隔离实例的库里写一条"生产侧真会写出来的形状"的群任务行，
 * 好让真 WKWebView 的无障碍树里真的长出那颗编号角标可读。
 *
 * ⚠️ 它是**读数的前置**，不是判据：`check-ui-runtime-ax.mjs` 带 `--fixture=todo` 时打印
 * "读到了什么 / 没读到什么"，**不 `check()`** ⇒ 这条档不进任何门禁层、不影响那个入口的退码判据。
 * 为什么必须留在库里而不是探针里手写：探针自己造一行 = 判的是"自造路径"，产品哪天改了载荷形状，
 * 读数照样绿，而它已经和真实渲染路径无关了（本仓为这类事翻过车）。
 *
 * 载荷的 13 个键与 `src-tauri/src/protocol.rs` 的 `TodoPayload` 逐字段对齐，
 * 由 `scripts/axTodoFixture.test.ts` 现读源码钉住（生产加字段而这里没跟上 ⇒ 红）。
 */
export const TODO_FIXTURE = {
  groupId: "gprobe",
  groupName: "E2E-Probe",
  convId: "group:gprobe",
  msgId: "todo-probe-1",
  kind: "todo",
  /** 群消息的 `receiver_id` 就是群 id（与 `e2e-multi-instance.mjs` 任务轮那几条 INSERT 同形）。 */
  receiverId: "gprobe",
  category: "requirement",
  number: 12345,
  /** 期望在真窗口控件树里读到的两条：可见的是缩短码，可访问名字里带完整码。 */
  expect: { visible: "R2345", named: "任务编号 R12345" },
  defaultTodo: { category: "requirement", number: 12345, status: "doing" },
};

/** 一条 `todo` 定义行的载荷（键集合 == Rust 侧 `TodoPayload` 的字段）。 */
export function todoFixturePayload(deviceId, over = {}) {
  return {
    todo_id: todoFixtureId(over.number ?? TODO_FIXTURE.number),
    title: "probe 任务",
    assignees: [deviceId],
    status: "doing",
    creator: deviceId,
    deleted: false,
    description: "",
    images: [],
    archived: false,
    priority: "normal",
    category: TODO_FIXTURE.category,
    number: TODO_FIXTURE.number,
    done_at: null,
    ...over,
  };
}

/**
 * 写库：群 + 成员 + 会话 + 那条 todo 行。`db` 由调用方给（`node:sqlite` 的 DatabaseSync）。
 * 只在**隔离实例**的库上调 ⇒ 探针自己负责确认这个实例编号不是用户正在用的那个。
 */
/** 每档一个独立 id：同一库里放两档（带号 / 无号）时，后写的那条不许把前一条顶掉。 */
export function todoFixtureId(number) { return `${TODO_FIXTURE.msgId}-${number}`; }

/** 两档夹具：带号那档期望读到两条，无号那档期望**一条都没有**（反面对照）。 */
export const FIXTURES = {
  todo: { over: {}, expect: TODO_FIXTURE.expect },
  "todo-nonumber": { over: { number: 0 }, expect: null },
};

export function seedTodoFixture(db, deviceId, over = {}) {
  const now = Date.now();
  db.exec("BEGIN");
  try {
    db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
      .run(TODO_FIXTURE.groupId, TODO_FIXTURE.groupName, deviceId, now);
    db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
      .run(TODO_FIXTURE.groupId, deviceId);
    db.prepare(
      "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at,last_ts)"
      + " VALUES(?1,'group',?2,NULL,0,?3,?4)",
    ).run(TODO_FIXTURE.convId, TODO_FIXTURE.groupName, now, now);
    const msgId = todoFixtureId(over.number ?? TODO_FIXTURE.number);
    db.prepare("DELETE FROM messages WHERE msg_id=?1").run(msgId);
    db.prepare(
      "INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)"
      + " VALUES(?1,?2,?3,?4,'todo',?5,?6,1,'sent')",
    ).run(msgId, TODO_FIXTURE.convId, deviceId, TODO_FIXTURE.receiverId,
      JSON.stringify(todoFixturePayload(deviceId, over)), now);
    db.exec("COMMIT");
  } catch (e) {
    db.exec("ROLLBACK");
    throw e;
  }
  return { deviceId, ts: now };
}
