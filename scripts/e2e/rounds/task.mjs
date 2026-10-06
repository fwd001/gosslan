#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **TASK** 轮次分册（一族一轮：preset + run + recheck）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "TASK";
import { GROUP_ID, GROUP_KEY_B64, GROUP_KEY_STR, GROUP_NAME, INSTANCES, ROUND, S, bootBaseOf, buildGroupEnvelope, check, ed25519Priv, launch, nowMs, openDb, procs, seed, sleep, step, stopAll, tcpOpen, waitFor } from "../core.mjs";
import { BOOT_LINE, bootReady } from "../../e2e-logtail.mjs";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
let taskCreateId = "";
let taskUpdateId = "";
let taskArchId = "";
let taskReopenId = "";
/// 任务描述里带的那两张图片**引用**（真实字节走群文件管线，SQLite 不存 BLOB）。
/// 五个字段各有各的用途：`subtype` 决定卡片里是缩略图还是文件块、`sha256`/`id` 是接收方
/// 在本地目录里找字节的键、`size` 用于进度与完整性 —— 少一个都是**静默**的（不会报错，只会显示不对）。
let taskImages = [];
/// 对端发起的那两条（创建 / 改成完成）。以前这一轮只有 A→B 一个方向 ⇒
/// "任务只能由本端发起"这一半从头到尾没被判过，B 签名/对端 creator 这条授权输入也没人核。
let taskBCreateId = "";
let taskBDoneId = "";
const TASK_LIE = ROUND === "task-lie";
/// 崩溃那一腿的两条（发送端在"还没送达"的时刻被 SIGKILL）。判的不是"能不能续传"
/// （②③⑩ 已证），而是**群任务这一族的「已入队」这个事实许不许被一次崩溃抹掉**。
let taskCrashCreateId = "";
let taskCrashDoneId = "";

/// 任务轮预置：两端同一份群 + 群密钥（与群聊轮同形），A 排两条群载荷 ——
/// `todo`（创建，seq=1，指派给 B）与 `todo_update`（B 视角下的完成，seq=2）。
/// 载荷字段逐字对齐 `protocol.rs::TodoPayload`（todo_id/title/assignees/status/creator/
/// deleted/description/images/archived/done_at），**snake_case 无 rename**。
export async function preset() {
  step("任务预置：两端各写一份群 + 同一份群密钥，A 排「创建」与「完成」两条任务载荷", () => {
    const members = [S.idA.runtimeId, S.idB.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of INSTANCES) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, S.idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
      });
    }
    const todoId = "todo-e2e-1";
    const mk = (over) => JSON.stringify({
      todo_id: todoId, title: "e2e task", assignees: [S.idB.runtimeId], status: "todo",
      creator: S.idA.runtimeId, deleted: false, description: "", images: [], archived: false,
      done_at: null, ...over,
    });
    const base = {
      groupKey: GROUP_KEY_B64, senderId: S.idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: S.idA.x25519Pub, ed25519Pub: S.idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: S.idA.runtimeId, members,
    };
    const c = buildGroupEnvelope({ ...base, kind: "todo", content: mk({}), ts, seq: 1 });
    const u = buildGroupEnvelope({
      ...base, kind: "todo_update", content: mk({ status: "done", done_at: ts + 5 }), ts: ts + 1, seq: 2,
    });
    // §7 那两条迁移（完成后归档 ⇒ 与我相关的数从 1 掉到 0；再重开 ⇒ 又回到 1）。
    // 载荷形状与命令层一致：`resolve_done_archive` 只允许 done 带 archived，
    // 重开则 status 回 doing、archived=false、done_at 清空 —— 照抄这个口径，不自创一套。
    const ar = buildGroupEnvelope({
      ...base, kind: "todo_update",
      content: mk({ status: "done", done_at: ts + 5, archived: true }), ts: ts + 2, seq: 3,
    });
    const re = buildGroupEnvelope({
      ...base, kind: "todo_update",
      content: mk({ status: "doing", done_at: null }), ts: ts + 3, seq: 4,
    });
    taskCreateId = c.messageId;
    taskUpdateId = u.messageId;
    taskArchId = ar.messageId;
    taskReopenId = re.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [c, 1, "todo", mk({}), ts],
        [u, 2, "todo_update", mk({ status: "done", done_at: ts + 5 }), ts + 1],
        [ar, 3, "todo_update", mk({ status: "done", done_at: ts + 5, archived: true }), ts + 2],
        [re, 4, "todo_update", mk({ status: "doing", done_at: null }), ts + 3],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, S.idA.runtimeId, GROUP_ID, kind, content, at, seq);
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, S.idB.runtimeId, env.wire, at);
      }
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 4);
    });

    // ★ 方向反过来再排一次：B 自己建一条任务、自己改成完成，A 只是收的一方。
    // 为什么这是**合法**形状而不是伪造：命令层的改/删授权是
    // `def.creator == actor || group_creator == actor`（commands/group_files.rs:181），
    // B 是这条任务自己的 creator ⇒ 走第一支。拿"非创建者改任务"来排这一腿会判到一条
    // 产品本来就不允许的输入上，红得没有意义。
    // 时钟这边预置到 6 不会挡住 A 的 1..4：接收侧走 `observe_clock`=`max(local,observed)`
    // （db/clocks.rs:41-48），只有发送侧的 `next_clock` 会分配新号 —— 已读源码确认，不是猜的。
    const todoId2 = "todo-e2e-2";
    taskImages = [
      {
        id: createHash("sha256").update("e2e-task-image-1").digest("hex"),
        name: "白板照片.jpg", size: 20480,
        sha256: createHash("sha256").update("e2e-task-image-1").digest("hex"),
        subtype: "image",
      },
      {
        id: createHash("sha256").update("e2e-task-file-1").digest("hex"),
        name: "合同.pdf", size: 98304,
        sha256: createHash("sha256").update("e2e-task-file-1").digest("hex"),
        subtype: "file",
      },
    ];
    const mk2 = (over) => JSON.stringify({
      todo_id: todoId2, title: "e2e task by B", assignees: [S.idA.runtimeId], status: "todo",
      creator: S.idB.runtimeId, deleted: false, description: "", images: taskImages,
      archived: false, done_at: null, ...over,
    });
    const baseB = {
      groupKey: GROUP_KEY_B64, senderId: S.idB.runtimeId, priv: ed25519Priv(INSTANCES[1]),
      x25519Pub: S.idB.x25519Pub, ed25519Pub: S.idB.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: S.idA.runtimeId, members,
    };
    const b1 = buildGroupEnvelope({ ...baseB, kind: "todo", content: mk2({}), ts: ts + 4, seq: 5 });
    const b2 = buildGroupEnvelope({
      ...baseB, kind: "todo_update",
      content: mk2({ status: "done", done_at: ts + 9 }), ts: ts + 5, seq: 6,
    });
    taskBCreateId = b1.messageId;
    taskBDoneId = b2.messageId;
    seed(INSTANCES[1].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [b1, 5, "todo", mk2({}), ts + 4],
        [b2, 6, "todo_update", mk2({ status: "done", done_at: ts + 9 }), ts + 5],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, S.idB.runtimeId, GROUP_ID, kind, content, at, seq);
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, S.idA.runtimeId, env.wire, at);
      }
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 6);
    });
  });
}

// §十四要的「错误行为测试」+ §七/§八的「文件 hash 不一致 / .part 已存在」：
// 坏内容必须要么被拒收、要么被补齐成正确字节 —— 但绝不允许"报成功却没有正确文件"。

export async function run() {
  step("任务判据：B 收到创建/完成/归档/重开四条、明文解得开、指派就是我、seq 决定 LWW 终态", async () => {
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (TASK_LIE ? flip(id) : id);
    const convId = `group:${GROUP_ID}`;
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [taskCreateId, taskUpdateId, taskArchId, taskReopenId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "B 侧四条任务载荷到齐（建链后 flush_group_outbox 送达）");

    const bDb = openDb(INSTANCES[1].db, true);
    const q = (id) => bDb.prepare(
      "SELECT m.conv_id,m.sender_id,m.kind,m.content,m.seq,m.status FROM messages m WHERE m.msg_id=?1",
    ).all(id);
    const rowC = q(want(taskCreateId));
    const rowU = q(want(taskUpdateId));
    const rowA = q(want(taskArchId));
    const rowR = q(want(taskReopenId));
    const leak1to1 = bDb.prepare(
      "SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (?2,?3,?4,?5)",
    ).get(convId, taskCreateId, taskUpdateId, taskArchId, taskReopenId).c;
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const queued = aDb.prepare(
      "SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2,?3,?4)",
    ).get(taskCreateId, taskUpdateId, taskArchId, taskReopenId).c;
    const aKinds = aDb.prepare(
      "SELECT kind FROM messages WHERE msg_id IN (?1,?2) ORDER BY seq",
    ).all(taskCreateId, taskUpdateId).map((r) => r.kind);
    aDb.close();

    const parse = (rows) => {
      if (rows.length !== 1) return null;
      try { return JSON.parse(rows[0].content); } catch { return null; }
    };
    const pc = parse(rowC);
    const pu = parse(rowU);
    const pa = parse(rowA);
    const pr = parse(rowR);
    const uniq = new Set([taskCreateId, taskUpdateId, taskArchId, taskReopenId]).size;

    // 这条读的是**发送侧**（A 的库），lie 模式翻的是判据读的 id ⇒ 它在正向与反向两跑里都该绿：
    // 它是这一轮的"预置/投递没坏"控制项，不是被钉的那件事本身。
    check("A 侧两条载荷按 seq 排的 kind 依次是 todo / todo_update（发送侧预置控制项）",
      aKinds.join(",") === "todo,todo_update", "todo,todo_update", aKinds.join(","));
    check("B 侧四条各恰好一行（多行=重复投递，0 行=没送达）",
      [rowC, rowU, rowA, rowR].every((r) => r.length === 1), "1/1/1/1",
      [rowC, rowU, rowA, rowR].map((r) => r.length).join("/"));
    check("B 侧落库的必须是群会话行（不许串进 1:1）",
      rowC.length === 1 && rowC[0].conv_id === convId && leak1to1 === 0,
      `${convId} 且 1:1 里 0 条`, `${rowC[0]?.conv_id} / leak=${leak1to1}`);
    check("载荷必须是**解密后的明文 JSON**（拿到密文或空串都说明没真解密）",
      !!pc && !!pu, "两条都能 JSON.parse", `c=${pc === null ? "解析失败" : "ok"} u=${pu === null ? "解析失败" : "ok"}`);
    check("两条指的是**同一个任务**（todo_id 相同，不是两条无关消息）",
      !!pc && !!pu && pc.todo_id === pu.todo_id && pc.todo_id === "todo-e2e-1",
      "todo-e2e-1", `${pc?.todo_id} / ${pu?.todo_id}`);
    check("创建那条的指派里必须有 B 的 device_id（「与我相关」的输入就是这个）",
      Array.isArray(pc?.assignees) && pc.assignees.includes(S.idB.runtimeId),
      `含 ${S.idB.runtimeId}`, JSON.stringify(pc?.assignees));
    check("创建那条的 status 是 todo、完成那条是 done 且带 done_at",
      pc?.status === "todo" && pu?.status === "done" && !!pu?.done_at,
      "todo → done(+done_at)", `${pc?.status} → ${pu?.status} done_at=${pu?.done_at}`);
    check("seq 必须是创建 1 / 完成 2（LWW 靠 seq 定序，乱了折叠出的终态就不对）",
      rowC[0]?.seq === 1 && rowU[0]?.seq === 2, "1 / 2",
      `${rowC[0]?.seq} / ${rowU[0]?.seq}`);
    check("creator 由载荷带着，且必须是 A（改/删授权判据靠它）",
      pc?.creator === S.idA.runtimeId && pu?.creator === S.idA.runtimeId,
      S.idA.runtimeId, `${pc?.creator} / ${pu?.creator}`);
    check("发送方在 B 侧记为 A 的 runtimeId（不许被改写成接收者自己）",
      rowC[0]?.sender_id === S.idA.runtimeId, S.idA.runtimeId, rowC[0]?.sender_id);
    check("A 侧这两条的 group_outbox 必须被 GroupAck 清干净（队列残留=还会重发）",
      queued === 0, 0, queued);
    check("四条载荷的 msg_id 互不相同且各唯一（INV-P01 幂等的前提）",
      uniq === 4, 4, uniq);
    // §7 要的完整迁移：创建 → 完成 →（完成态才允许）归档 → 重开。
    // 徽标那条数在真实应用里就是靠这一串状态行的**先后**算出来的（done/archived 不算，
    // 重开回 doing 又要算回来），所以这里判的是四行的状态字段与 seq 顺序，不是像素。
    check("归档那条必须 archived=true 且 status=done（命令层 resolve_done_archive 只允许这个组合）",
      pa?.archived === true && pa?.status === "done", "done + archived=true",
      `${pa?.status} + archived=${pa?.archived}`);
    check("重开那条回到 doing 且清掉 archived/done_at（「与我相关」的数要从 0 又变回 1）",
      pr?.status === "doing" && pr?.archived === false && !pr?.done_at,
      "doing + archived=false + done_at=null",
      `${pr?.status} + archived=${pr?.archived} + done_at=${JSON.stringify(pr?.done_at)}`);
    check("四条的 seq 必须严格 1/2/3/4（到货顺序不等于因果顺序，LWW 全靠 seq）",
      [rowC[0]?.seq, rowU[0]?.seq, rowA[0]?.seq, rowR[0]?.seq].join(",") === "1,2,3,4",
      "1,2,3,4",
      [rowC[0]?.seq, rowU[0]?.seq, rowA[0]?.seq, rowR[0]?.seq].join(","));

    // ── 对端发起的那一半（B 建 → B 完成 → A 同步）──
    // 预置时 A/B 都停着，靠建链后的 flush_group_outbox 送达；这条 waitFor 用**真 id**，
    // 与上面同形：lie 只翻判据读的那份，不翻"等不等得到"，否则红会落在超时上、看不出是判据坏。
    await waitFor(() => {
      const db = openDb(INSTANCES[0].db, true);
      try {
        return [taskBCreateId, taskBDoneId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "A 侧收到 B 发起的创建与完成两条（发起方换向的这一半以前没有判据）");

    const aDb2 = openDb(INSTANCES[0].db, true);
    const q2 = (id) => aDb2.prepare(
      "SELECT conv_id,sender_id,kind,content,seq FROM messages WHERE msg_id=?1",
    ).all(id);
    const bRowC = q2(want(taskBCreateId));
    const bRowU = q2(want(taskBDoneId));
    const bLeak = aDb2.prepare(
      "SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (?2,?3)",
    ).get(convId, taskBCreateId, taskBDoneId).c;
    aDb2.close();
    const bDb2 = openDb(INSTANCES[1].db, true);
    const bQueued = bDb2.prepare(
      "SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2)",
    ).get(taskBCreateId, taskBDoneId).c;
    bDb2.close();
    const pbc = parse(bRowC);
    const pbu = parse(bRowU);

    check("B 发起的两条在 A 侧各恰好一行、且落在群会话里（换向这一腿真投递了）",
      bRowC.length === 1 && bRowU.length === 1 && bRowC[0]?.conv_id === convId && bLeak === 0,
      "1/1 行且 1:1 里 0 条",
      `${bRowC.length}/${bRowU.length} conv=${bRowC[0]?.conv_id} leak=${bLeak}`);
    check("A 侧解出来的明文指向 B 建的那条任务，creator 就是发送者 B（授权读的是这个字段）",
      !!pbc && !!pbu && pbc.todo_id === "todo-e2e-2" && pbu.todo_id === "todo-e2e-2"
      && pbc.creator === S.idB.runtimeId && pbu.creator === S.idB.runtimeId,
      `todo-e2e-2 + creator=${S.idB.runtimeId}`,
      `${pbc?.todo_id}/${pbu?.todo_id} creator=${pbc?.creator}/${pbu?.creator}`);
    check("创建那条指派的是 A、seq 5→6 且 todo→done 带 done_at（对端视角的「与我相关」输入）",
      Array.isArray(pbc?.assignees) && pbc.assignees.includes(S.idA.runtimeId)
      && pbc?.status === "todo" && pbu?.status === "done" && !!pbu?.done_at
      && bRowC[0]?.seq === 5 && bRowU[0]?.seq === 6,
      "assignees 含 A + 5/6 + todo→done(+done_at)",
      `${JSON.stringify(pbc?.assignees)} seq=${bRowC[0]?.seq}/${bRowU[0]?.seq} ${pbc?.status}→${pbu?.status} done_at=${pbu?.done_at}`);
    check("发送方在 A 侧记为 B 的 runtimeId（对端发起的不得被写成接收者自己）",
      bRowC[0]?.sender_id === S.idB.runtimeId, S.idB.runtimeId, bRowC[0]?.sender_id);
    check("B 侧这一单的 group_outbox 也被 Ack 清干净（发起方的队列残留=下次建链还会重发）",
      bQueued === 0, 0, bQueued);

    // §27「任务 + 图片」这一格：任务描述里带的图片**只有元数据过线**（真实字节走群文件管线，
    // SQLite 不存 BLOB —— 这条原则必须继续保持）。五个字段每一个掉了都是静默的：
    // `subtype` 错 ⇒ 卡片把图片渲染成文件块；`sha256`/`id` 错 ⇒ 接收方在本地目录里找不到字节；
    // `size` 错 ⇒ 进度与完整性对不上。所以逐字段比，不按字符串比（序列化顺序不是契约）。
    const imgSame = (a, b) => !!a && !!b && a.id === b.id && a.name === b.name
      && a.size === b.size && a.sha256 === b.sha256 && a.subtype === b.subtype;
    const imgsOk = (p) => {
      const got = p?.images;
      return Array.isArray(got) && got.length === taskImages.length
        && taskImages.every((im, k) => imgSame(im, got[k]));
    };
    check("任务带的两张图片引用跨进程**逐字段**原样到齐、顺序不变（创建那条与改成完成那条都要带住）",
      imgsOk(pbc) && imgsOk(pbu), `${taskImages.length} 条 × 5 字段全等`,
      `c=${JSON.stringify(pbc?.images)} u.ok=${imgsOk(pbu)}`);
    // 这一条钉的是产品侧写在注释里的一句等式："id 与 sha256 同值，接收方按它在本地解析"。
    // 它一旦分叉，接收方就永远取不到那张图的字节 —— 而线上看着完全正常。
    // ★ `every` 对**空数组恒为真** ⇒ 必须先钉"有一条以上"，否则对端一条都没到时这条照样绿
    //   （实测：lie 模式下只加 `.every` 那半边是 18 红，补上前半句才是 19 红）。
    const idsMatch = (p) => Array.isArray(p?.images) && p.images.length > 0
      && p.images.every((im) => im.id === im.sha256 && im.sha256.length === 64);
    check("每条图片引用的 id 必须等于 sha256（=64 位 hex）—— 接收方就是拿它在本地目录里找字节的",
      idsMatch(pbc) && idsMatch(pbu), "id==sha256 且 64 位",
      JSON.stringify((pbc?.images ?? []).map((im) => `${im.id === im.sha256}/${im.sha256?.length}`)));
  });

  // ── §28「正在任务同步时退出」这一格：群任务这一族的崩溃恢复（以前只有文件族有判据）──
  // 钉的核心**不是**"能不能续传"（②③⑩ 已证），而是**一次崩溃不许把「已入队」这个事实抹掉**：
  // `group_outbox` 行是"先入队再投递"这条不变量的载体。它若因发送端崩溃变成 0 行，
  // 这两条任务就永久没人再发，而 A 的聊天气泡还在（§30 说的"最难发现的一种丢法"）。
  // ★ 时序由判据自己造：停机入队 → **只起 A**（对端不存在 ⇒ 一条也送不出去）→ SIGKILL A →
  //   再起 A+B（建链事件带动 flush，这是 A-9 那条缺口的机制面，这里当机制用，不等于认可那个缺口）。
  step("任务崩溃判据：群任务还没送达时发送端被 SIGKILL ⇒ 队列行与气泡都不许消失，重启后必须自己送到", async () => {
    await stopAll();
    const ts = nowMs();
    const convId = `group:${GROUP_ID}`;
    const members = [S.idA.runtimeId, S.idB.runtimeId];
    const todoId = "todo-e2e-3";
    const mk = (over) => JSON.stringify({
      todo_id: todoId, title: "e2e task crash", assignees: [S.idB.runtimeId], status: "todo",
      creator: S.idA.runtimeId, deleted: false, description: "", images: [], archived: false,
      done_at: null, ...over,
    });
    const base = {
      groupKey: GROUP_KEY_B64, senderId: S.idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: S.idA.x25519Pub, ed25519Pub: S.idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: S.idA.runtimeId, members,
    };
    const c1 = buildGroupEnvelope({ ...base, kind: "todo", content: mk({}), ts, seq: 7 });
    const c2 = buildGroupEnvelope({
      ...base, kind: "todo_update", content: mk({ status: "done", done_at: ts + 3 }), ts: ts + 1, seq: 8,
    });
    taskCrashCreateId = c1.messageId;
    taskCrashDoneId = c2.messageId;
    seed(INSTANCES[0].db, (db) => {
      for (const [env, seq, kind, content, at] of [
        [c1, 7, "todo", mk({}), ts],
        [c2, 8, "todo_update", mk({ status: "done", done_at: ts + 3 }), ts + 1],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, S.idA.runtimeId, GROUP_ID, kind, content, at, seq);
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, S.idB.runtimeId, env.wire, at);
      }
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 8);
    });

    // 只起 A：这一格的坏状态是"对端整个不存在"，所以这两条**必须**只能待在队列里。
    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, `崩溃判据：A 的 TCP ${INSTANCES[0].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE), 30_000,
      "崩溃判据：A 打出 boot 完成行");
    await sleep(15_000); // 一段"对端完全缺席"的时间

    const readA = () => {
      const db = openDb(INSTANCES[0].db, true);
      try {
        return {
          queued: db.prepare("SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2)")
            .get(taskCrashCreateId, taskCrashDoneId).c,
          bubbles: db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id IN (?1,?2)")
            .get(taskCrashCreateId, taskCrashDoneId).c,
        };
      } finally { db.close(); }
    };
    const readB = () => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id IN (?1,?2)")
          .get(taskCrashCreateId, taskCrashDoneId).c;
      } finally { db.close(); }
    };
    // 缺席那一条读的是**真 id**（翻过的 id 必然查不到 ⇒ 那条断言就成了永远为真的空转）。
    // 它的正向对照是同一步里后面那条"重启后 B 侧真收到" —— 缺席判据与到齐判据配对，才不是"没跑起来"。
    check("对端缺席 15s 期间 B 侧不许有这两条的任何一行（还没送达就是没送达）",
      readB() === 0, 0, readB());
    const q1 = readA();
    check("对端缺席期间 A 侧两条队列行都还在（「先入队再投递」的载体不许自己消失）",
      q1.queued === 2, 2, q1.queued);

    const pA = procs.get(INSTANCES[0].n);
    // ⚠️ 被信号杀死的子进程 `exitCode === null`、只有 `signalCode` 有值（③⑩ 踩过同一个坑）。
    const deadA = () => pA.exitCode !== null || pA.signalCode !== null;
    pA.kill("SIGKILL");
    await waitFor(deadA, 15_000, "崩溃判据：A 确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(10_000); // 一段"发送端根本不存在"的时间

    const q2 = readA();
    check("发送端崩溃后队列行必须还是 2 行 —— 崩溃不许把它当成已送达、更不许抹掉「已入队」",
      q2.queued === 2, 2, q2.queued);
    check("发送端崩溃后 A 侧那两条气泡必须还在 —— 队列没了是永久没人再发，气泡没了是用户连「发过」都看不见",
      q2.bubbles === 2, 2, q2.bubbles);

    launch(INSTANCES[0]);
    launch(INSTANCES[1]);
    for (const i of INSTANCES) {
      await waitFor(() => tcpOpen(i.port), 60_000, `重启后实例 ${i.label} 的 TCP ${i.port} 可连`);
      await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 30_000,
        `重启后实例 ${i.label} 打出 boot 完成行`);
    }
    // 同步点选在**被断言那一侧自己到齐**：B 落库靠 A 建链后的 flush，A 的队列清空靠 B 的 GroupAck
    // （⑨ 的教训：拿另一侧的产物当这一侧的同步点会天然晚一步）。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (TASK_LIE ? flip(id) : id);
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [taskCrashCreateId, taskCrashDoneId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 90_000, "重启后这两条必须自己送到 B（建链带动 flush）");

    const bDb = openDb(INSTANCES[1].db, true);
    const rowsCrash = [want(taskCrashCreateId), want(taskCrashDoneId)].map((id) => bDb.prepare(
      "SELECT conv_id,sender_id,kind,content,seq FROM messages WHERE msg_id=?1",
    ).all(id));
    bDb.close();
    const safeParse = (rows) => {
      if (rows.length !== 1) return null;
      try { return JSON.parse(rows[0].content); } catch { return null; }
    };
    const pc = safeParse(rowsCrash[0]);
    const pd = safeParse(rowsCrash[1]);
    check("重启补送到的这两条在 B 侧各恰好一行、明文解得开、seq 7→8 且 todo→done（重放不许送坏内容）",
      rowsCrash.every((r) => r.length === 1) && rowsCrash[0][0]?.conv_id === convId
      && rowsCrash[0][0]?.sender_id === S.idA.runtimeId
      && pc?.todo_id === todoId && pd?.todo_id === todoId
      && pc?.status === "todo" && pd?.status === "done" && !!pd?.done_at
      && rowsCrash[0][0]?.seq === 7 && rowsCrash[1][0]?.seq === 8,
      "各 1 行 + 明文 todo→done + seq 7/8 + sender=A",
      `${rowsCrash.map((r) => r.length).join("/")} ${pc?.status}→${pd?.status}`
      + ` seq=${rowsCrash[0][0]?.seq}/${rowsCrash[1][0]?.seq} sender=${rowsCrash[0][0]?.sender_id}`);

    // A 的队列清空只能**有界地等**它自己发生（GroupAck 在 B 落库之后才发）；
    // 到不了终态就把最后一次读数交给 check 判红 —— 不写"兜底断言"（那种断言到不了就是死代码）。
    let qEnd = -1;
    const tEnd = nowMs();
    for (;;) {
      qEnd = readA().queued;
      if (qEnd === 0 || nowMs() - tEnd >= 60_000) break;
      await sleep(500);
    }
    check("重启后这两条的队列行最终被 GroupAck 清成 0（残留=下次建链还会重发一遍）",
      qEnd === 0, 0, qEnd);
  });
}

/// §22「A 重启后 badge 仍然正确」里**这一层能判的那一半**：徽标数字是前端算的（runtime 层，
/// 见 roadmap §13.3 的层次修正），但它是从这几行状态折叠出来的 —— 所以真正要钉的是
/// 「重启之后这些行还在、没被改写成旧状态、也没被再投一遍」。判据读的是**真 id**：
/// 这一格要证的是持久性，拿翻过的 id 去读只会得到"0 行"，那种红分不清"没送达"和"没留住"。
/// 独立成一个 `if (TASK)` 块而不是塞进上面那一步：上面那一步的判据全按 `check-doc-numbers`
/// 归到「默认轮」，往里塞 TASK 专属断言会让默认轮少算几条、任务轮多算几条（现算守卫会当场判红）。
export async function recheck() {
  step("任务重启判据：六条状态行两端都活得过重启，且已 Ack 的队列不被点亮成二次投递", async () => {
    // 上一步（L-B）已经 stopAll + bootAndStop 走完一轮真实重启，此刻两端都是停机库 ⇒ 直接读。
    const bDb = openDb(INSTANCES[1].db, true);
    const bRows = bDb.prepare(
      "SELECT msg_id,kind,seq,status FROM messages WHERE msg_id IN (?1,?2,?3,?4)"
      + " ORDER BY seq",
    ).all(taskCreateId, taskUpdateId, taskArchId, taskReopenId)
      .map((r) => `${r.kind}:${r.seq}`);
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const aRows = aDb.prepare(
      "SELECT msg_id,kind,seq FROM messages WHERE msg_id IN (?1,?2) ORDER BY seq",
    ).all(taskBCreateId, taskBDoneId).map((r) => `${r.kind}:${r.seq}`);
    const aLeft = aDb.prepare(
      "SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2,?3,?4)",
    ).get(taskCreateId, taskUpdateId, taskArchId, taskReopenId).c;
    aDb.close();
    const bLeft = (() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return db.prepare("SELECT COUNT(*) c FROM group_outbox WHERE msg_id IN (?1,?2)")
          .get(taskBCreateId, taskBDoneId).c;
      } finally { db.close(); }
    })();

    check("重启后 B 侧四条状态行仍各一行、kind:seq 一字不变（折叠出的终态不许退）",
      bRows.join(",") === "todo:1,todo_update:2,todo_update:3,todo_update:4",
      "todo:1,todo_update:2,todo_update:3,todo_update:4", bRows.join(","));
    check("重启后 A 侧对端发起的两条仍各一行（不二次投递 = 已 Ack 的队列没被点亮）",
      aRows.join(",") === "todo:5,todo_update:6", "todo:5,todo_update:6", aRows.join(","));
    check("重启后两侧的 group_outbox 对这六条都是 0 行（残留=下次建链会再发一遍）",
      aLeft === 0 && bLeft === 0, "0 / 0", `${aLeft} / ${bLeft}`);
  });
}
