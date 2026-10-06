#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **POSTTEXT** 轮次分册（一族一轮：preset + run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "POSTTEXT";
import { GROUP_ID, GROUP_KEY_B64, GROUP_KEY_STR, GROUP_NAME, INSTANCES, ROUND, S, bootBaseOf, buildGroupEnvelope, check, ed25519Priv, eid, launch, nowMs, openDb, procs, seed, sleep, step, tcpOpen, waitFor } from "../core.mjs";
import { BOOT_LINE, bootReady } from "../../e2e-logtail.mjs";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
const POSTTEXT_LIE = ROUND === "posttext-lie";

// 续发轮的群预置：**只写群本身，一条消息都不排**（消息由下面 J4 在文件传完之后才排）。
// 为什么必须放在实例启动之前：接收端解密用的是开机时从 `settings(gk:…)` 读进内存的那份群密钥，
// 启动之后再往盘上写那一行，进程读不到 ⇒ 判据会红在「预置时序」上，而不是红在产品。
// 这一条是 §三 点名的「大文件之后群同步是否仍然正常」那一半的起点。
export async function preset() {
  step("群预置（续发轮）：两端各写同一份群 + 同一份群密钥，不排任何群消息", () => {
    const members = [S.idA.runtimeId, S.idB.runtimeId];
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
        ).run(`group:${GROUP_ID}`, GROUP_NAME, ts);
      });
    }
  });
}

// §三 点名那一半 + §五 那张"文件层不许拖垮消息层"。默认轮的顺序是**先文本（上面那条 J1）
// 再文件（J2）** ⇒ 它只判过"文件之前聊天能用"，从来没有一条判据把那条文本放到一份文件
// **走完之后再投**。这一轮补的就是那一半：同一条已建立的链路、同一对进程。
// 反向只翻判据自己读的那两份值（期望的明文 + 台账行的 id），产品码一字不动 ⇒ 红必须来自断言本身。
export async function run() {
  step("J3 传完之后续发一条普通文本：必须照常落库、被对端 Ack 回收，且不许把刚完成那份的终态带回去", async () => {
  const postMsgId = eid();
  const ts = nowMs();
  // 反证档翻的是**判据读的两份值**：期望的明文（内容那条）与读台账行用的 transfer_id（终态那条）。
  const expectContent = POSTTEXT_LIE ? "after the large file (tampered)" : "after the large file";
  const wantXfer = POSTTEXT_LIE ? `${S.xferId}-ghost` : S.xferId;
  const payload = JSON.stringify({
    type: "chat_message",
    msg_id: postMsgId,
    from: S.idA.runtimeId,
    to: S.idB.runtimeId,
    kind: "text",
    content: "enc1:harness-placeholder", // 占位；应用会 re-seal 成真密文
    ts,
    seq: 2,
  });
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM messages WHERE msg_id=?1").run(postMsgId);
    db.prepare("DELETE FROM outbox WHERE msg_id=?1").run(postMsgId);
    // 与 J1 同一个配方：两行必须成对，只插 outbox 则 re-seal 没有明文，只插 messages 则 Ack 找不到人。
    db.prepare(
      `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
       VALUES(?1,?2,?3,?4,'text',?5,?6,2,'sending')`,
    ).run(postMsgId, S.idB.runtimeId, S.idA.runtimeId, S.idB.runtimeId, "after the large file", ts);
    db.prepare("INSERT INTO outbox(msg_id,peer_id,payload,created_at) VALUES(?1,?2,?3,?4)")
      .run(postMsgId, S.idB.runtimeId, payload, ts);
  });
  // 这一行是在**应用已经跑着、链路已经建好**之后才塞进队列的 ⇒ 没有"下一次建链"来冲它，
  // 只能等它自己那趟心跳/重试。所以这里的等待窗口按那一趟的节奏放宽，不拿 J1 的 60s 硬套。
  const readB = () => {
    const d = openDb(INSTANCES[1].db, true);
    try {
      return d.prepare("SELECT content,sender_id FROM messages WHERE msg_id=?1").all(postMsgId);
    } finally {
      d.close();
    }
  };
  const outboxLeft = () => {
    const d = openDb(INSTANCES[0].db, true);
    try {
      return d.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(postMsgId).c;
    } finally {
      d.close();
    }
  };
  await waitFor(() => readB().length > 0, 150_000, "B 侧出现这条「传完之后」的文本");
  await waitFor(() => outboxLeft() === 0, 90_000, "A 侧这条的 outbox 被 Ack 删除");
  await sleep(1500); // 多留一点窗口，让「重复投递」这种退化有机会显现

  const rowsB = readB();
  check("传完一份文件之后，续发的文本在 B 侧恰好落一条（文件层没把消息层占死，也没重复投）",
    rowsB.length === 1, 1, rowsB.length);
  check("B 侧内容与明文一致（走完大文件那条链之后 re-seal/解密这条路径照常）",
    rowsB[0]?.content === expectContent, expectContent, rowsB[0]?.content);
  check("B 侧方向正确（sender 还是 A）", rowsB[0]?.sender_id === S.idA.runtimeId, S.idA.runtimeId, rowsB[0]?.sender_id);
  check("A 侧这条的 outbox 行被对端 Ack 删除（不是本端写完就回收）", outboxLeft() === 0, 0, outboxLeft());
  const st = (() => {
    const d = openDb(INSTANCES[0].db, true);
    try {
      return d.prepare("SELECT status FROM messages WHERE msg_id=?1").get(postMsgId)?.status;
    } finally {
      d.close();
    }
  })();
  check("A 侧这条的状态前进过 sending（由对端确认点亮）",
    !["sending", "failed"].includes(st), "sent/delivered/read", st);
  // §五 的隔离形状 + P7：续发这条文本不许把**刚完成那份文件**的终态改回去。
  const tt = (() => {
    const d = openDb(INSTANCES[0].db, true);
    try {
      return d.prepare("SELECT status FROM file_transfers WHERE id=?1").get(wantXfer)?.status;
    } finally {
      d.close();
    }
  })();
  check("续发文本之后，刚完成那份的发送侧终态仍是 done（终态不许被后来的消息覆盖）",
    tt === "done", "done", tt ?? "无行");
  });

// §三 点名的另一半 ——「大文件之后**群同步**是否仍然正常」。J3 只管住 1:1 那条消息层；
// 群消息走的是**另一条队列**（`group_outbox`，靠心跳/建链时的 flush_group_outbox 送出，
// `network/transport.rs` 那三个调用点）⇒ 不单独排一条，"文件层把群同步层拖死"这种形状
// 在这一轮里是**绿**的（这正是 §五 那张隔离表要求证的东西）。
// 反向只翻判据读的那两份值（期望明文 + 查那行用的会话 id），预置与投递一字不动 ⇒
// 三条群判据一起红，而「队列被回收」那条读的是真 msg_id ⇒ 它照绿，
// 于是这次红能证"红来自断言本身、不是链路没跑"。
  step("J4 传完之后排一条群消息：群队列在大文件之后仍要送达、解密、落群会话，队列行还要被回收", async () => {
  const convId = `group:${GROUP_ID}`;
  const ts = nowMs();
  const env = buildGroupEnvelope({
    groupKey: GROUP_KEY_B64, senderId: S.idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
    x25519Pub: S.idA.x25519Pub, ed25519Pub: S.idA.ed25519Pub,
    groupId: GROUP_ID, groupName: GROUP_NAME, creator: S.idA.runtimeId,
    members: [S.idA.runtimeId, S.idB.runtimeId],
    kind: "text", content: "group msg after the large file", ts, seq: 1,
  });
  const wantConv = POSTTEXT_LIE ? `${convId}-ghost` : convId;
  const gPlain = "group msg after the large file";
  const expectG = POSTTEXT_LIE ? `${gPlain} (tampered)` : gPlain;
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
    db.prepare("DELETE FROM group_outbox WHERE msg_id=?1").run(env.messageId);
    // 与群聊轮同一个配方：发送方自己那条 + 群队列那一行；队列行是**唯一**被产品码读走的东西。
    db.prepare(
      `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
       VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sent')`,
    ).run(env.messageId, convId, S.idA.runtimeId, GROUP_ID, gPlain, ts);
    db.prepare(
      `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
       VALUES(?1,?2,?3,?4,?5)`,
    ).run(env.messageId, GROUP_ID, S.idB.runtimeId, env.wire, ts);
  });
  const readG = () => {
    const d = openDb(INSTANCES[1].db, true);
    try {
      return d.prepare("SELECT conv_id,content FROM messages WHERE msg_id=?1").all(env.messageId);
    } finally { d.close(); }
  };
  const queued = () => {
    const d = openDb(INSTANCES[0].db, true);
    try {
      return d.prepare("SELECT COUNT(*) c FROM group_outbox WHERE msg_id=?1").get(env.messageId).c;
    } finally { d.close(); }
  };
  // 等待用的是**真 id**（反向翻的是判据读的值，不是读键）⇒ 反证档里这一步照样能等到，
  // 红只会落在下面四条比较上。
  await waitFor(() => readG().length > 0, 150_000, "B 侧出现这条「传完之后」的群消息");
  await waitFor(() => queued() === 0, 90_000, "A 侧群队列那行被送达回收");
  await sleep(1500); // 多留一点窗口，让"重复投递"这种退化有机会显现

  const rowsG = readG();
  const hit = rowsG.find((r) => r.conv_id === wantConv);
  check("大文件传完之后排进群队列的那条，那个群会话里恰好落一条（群同步层没被文件层占死，也没重复投）",
    rowsG.filter((r) => r.conv_id === wantConv).length === 1, 1,
    rowsG.filter((r) => r.conv_id === wantConv).length);
  check("B 侧那条群消息的明文解得回来（群密钥解封 + 解密这条路径在大文件之后照常）",
    hit?.content === expectG, expectG, hit?.content);
  // 「没串进 1:1」不需要第二条查询：同一行只能有一个 conv_id，钉住它等于群会话就够了。
  check("它落在群会话里（同一行不可能既属群又属 1:1，所以这条同时排除了串会话）",
    hit?.conv_id === convId, convId, hit?.conv_id);
  check("A 侧 group_outbox 那行被回收（送达才算收尾，不是入队即删）", queued() === 0, 0, queued());
  });

// §三 点名的「快速连续发送」：以前只有 L2 那条 store 层用例（`src/utils/messages.test.ts:558`
// 「连续快速发送10条，Ack 乱序，全部不是 sending」），它判的是**乐观态与 Ack 回收**；
// 从来没有一次跨进程跑证明"N 条真过线、一条不多一条不少、各自内容不串味"。
// 这一格同时是 §七-1（dedup）与 §七-3（历史回归）要的那一半，所以它排在**同一条已经被大文件
// 与群同步压过的链路**上：前面几步已经把这条连接用满，这里再看连发还成不成立。
// ⚠️ **刻意不判到达顺序**：产品口径是"Ack 可以乱序"（就是那条 L2 用例），把 FIFO 写成判据
// 会把一条允许的行为判成缺陷；这里只判"每条各一次 + 内容逐条对上 + 队列回收干净"。
// 反向只多翻判据读的那份期望明文（第 3 条那格），预置与时序一字不动。
  step("J5 连发五条普通文本：各恰好落一行、明文逐条对上不许串味、outbox 全被对端 Ack 回收", async () => {
    const t0 = nowMs();
    const burst = [];
    for (let k = 0; k < 5; k++) {
      burst.push({ id: eid(), plain: `burst ${k + 1} of 5`, ts: t0 + k, seq: 3 + k });
    }
    const ids = burst.map((m) => m.id);
    // 五条一次性写进去：这才叫"连发"。逐条 step 会变成五次串行往返，测不到同一批里的相互影响。
    seed(INSTANCES[0].db, (db) => {
      for (const m of burst) {
        const payload = JSON.stringify({
          type: "chat_message", msg_id: m.id, from: S.idA.runtimeId, to: S.idB.runtimeId,
          kind: "text", content: "enc1:harness-placeholder", ts: m.ts, seq: m.seq,
        });
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(m.id);
        db.prepare("DELETE FROM outbox WHERE msg_id=?1").run(m.id);
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,'text',?5,?6,?7,'sending')`,
        ).run(m.id, S.idB.runtimeId, S.idA.runtimeId, S.idB.runtimeId, m.plain, m.ts, m.seq);
        db.prepare("INSERT INTO outbox(msg_id,peer_id,payload,created_at) VALUES(?1,?2,?3,?4)")
          .run(m.id, S.idB.runtimeId, payload, m.ts);
      }
    });
    const readB = (id) => {
      const d = openDb(INSTANCES[1].db, true);
      try {
        return d.prepare("SELECT conv_id,content FROM messages WHERE msg_id=?1").all(id);
      } finally { d.close(); }
    };
    const stillQueued = () => {
      const d = openDb(INSTANCES[0].db, true);
      try {
        return d.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id IN (?1,?2,?3,?4,?5)")
          .get(...ids).c;
      } finally { d.close(); }
    };
    const statuses = () => {
      const d = openDb(INSTANCES[0].db, true);
      try {
        return d.prepare("SELECT msg_id,status FROM messages WHERE msg_id IN (?1,?2,?3,?4,?5)")
          .all(...ids);
      } finally { d.close(); }
    };
    await waitFor(() => burst.every((m) => readB(m.id).length > 0), 180_000, "B 侧五条连发的文本都到齐");
    await waitFor(() => stillQueued() === 0, 90_000, "A 侧五条 outbox 全被对端 Ack 删除");
    await sleep(1500); // 给"重复投递"这种退化留出显形窗口

    const perId = burst.map((m) => ({ m, rows: readB(m.id) }));
    check("连发五条在 B 侧各恰好一行（不丢、不重复投 —— 被文件与群消息压过的链路上 dedup 照常）",
      perId.every(({ rows }) => rows.length === 1), "五条各 1 行",
      perId.map(({ m, rows }) => `${m.plain}=${rows.length}`).join(" "));
    const expectOf = (k) => (POSTTEXT_LIE && k === 2 ? `${burst[k].plain} (tampered)` : burst[k].plain);
    check("每条落库的明文就是它自己那一条（同批连发最贵的破坏是串味，必须逐条对上）",
      perId.every(({ rows }, k) => rows[0]?.content === expectOf(k)),
      burst.map((_m, k) => expectOf(k)).join(" / "),
      perId.map(({ rows }) => rows[0]?.content ?? "无行").join(" / "));
    // ⚠️ 这一条的期望值**当场被正向跑纠正过一次**（红在判据、不在产品）：1:1 会话的行在每一侧
    // 都以"对端那个 id"为键 —— B 库里这五条的 conv_id 是 **A 的** runtimeId，不是 B 自己的。
    // 我第一版写成 idB.runtimeId，正向立刻红：预期 …-i2 / 实际 …-i1。判据要判的是
    // "连发有没有把会话归属写散"，所以钉"五条同一个 conv_id"+"那一个就是以对端为键的这条 1:1"。
    const convs = [...new Set(perId.flatMap(({ rows }) => rows.map((r) => r.conv_id)))];
    check("五条都落在同一条 1:1 会话里（连发不许把归属写散；B 侧这条会话以**对端 A** 的 id 为键）",
      convs.length === 1 && convs[0] === S.idA.runtimeId, S.idA.runtimeId, convs.join(","));
    check("A 侧五条 outbox 行全部被对端 Ack 回收（不是本端写完 socket 就删）",
      stillQueued() === 0, 0, stillQueued());
    const st = statuses();
    check("A 侧五条状态都前进过 sending（每条由对端确认点亮，没有一条停在原地或被打回 failed）",
      st.length === 5 && st.every((r) => !["sending", "failed"].includes(r.status)),
      "五条都是 sent/delivered/read",
      st.map((r) => `${r.msg_id.slice(0, 6)}=${r.status}`).join(" "));
    const fileNow = (() => {
      const d = openDb(INSTANCES[0].db, true);
      try {
        return d.prepare("SELECT status FROM file_transfers WHERE id=?1").get(S.xferId)?.status;
      } finally { d.close(); }
    })();
    check("连发这五条不许把前面那份文件的终态带回去（P7：已完成的终态不是后来消息能改的）",
      fileNow === "done", "done", fileNow ?? "无行");
  });
  // ── J6：§三 点名的「大文件之后新建群」────────────────────────────────
  // 这一格以前在跨实例层是**零判据**：J1..J5 用的那个群在开机前就同时播种进了两端的盘，
  // 于是"新建的群能不能送达对方"从来没被这条链路判过；`--round=groupcrash` 判的是
  // "A 自己被 SIGKILL 之后还能不能重递"，那是崩溃恢复，不是"传完大文件之后建了个新群"。
  // ⚠️ 前置状态必须是生产到得了的那一份：**建群那一刻对方不在**。
  //   两端都在线时 create_group 提交完就即时逐成员推 GroupKey，而 harness 从进程外只写得出
  //   SQLite —— "对方在线却一次都没送到"这种状态生产里不存在，硬造出来判的是自造路径。
  //   所以这里先把 B 正常停掉（不是杀 A），在 B 缺席期间把群只长在 A 的盘上，再让 B 上线。
  // 于是这一格钉的是：链路被大文件用满之后，**群名册那份事实源仍然能在对端重新出现时把群补上**
  //   （`requeue_group_keys_for_peer`），而不是只在建链那一次起作用。
  step("J6 传完之后新建一个 B 从未见过的群（建群时对方离线）：B 重新上线后必须自己学到这个群", async () => {
    const NEW_GID = "g-e2e-afterfile";
    const NEW_NAME = "E2E-AfterBigFile";
    const newConv = `group:${NEW_GID}`;
    const members = [S.idA.runtimeId, S.idB.runtimeId];
    // 反向轮照 groupcrash / gfile 的先例：预置、时序、拓扑一字不动，只翻**判据读的那份 id**
    // ⇒ 红只能来自"读的不是真落库那行"，不来自基础设施（等待用的仍是真 id）。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const wantGid = POSTTEXT_LIE ? flip(NEW_GID) : NEW_GID;
    const onDisk = (file, gid) => {
      const db = openDb(file, true);
      try {
        return {
          rows: db.prepare("SELECT COUNT(*) c FROM groups WHERE id=?1").get(gid).c,
          group: db.prepare("SELECT id,name,creator FROM groups WHERE id=?1").get(gid) ?? null,
          memberCount: db.prepare("SELECT COUNT(*) c FROM group_members WHERE group_id=?1").get(gid).c,
          key: db.prepare("SELECT value FROM settings WHERE key=?1").get(`gk:${gid}`)?.value ?? null,
          conv: db.prepare("SELECT COUNT(*) c FROM conversations WHERE id=?1").get(`group:${gid}`).c,
        };
      } finally { db.close(); }
    };

    // B 正常下线（SIGTERM，给它收尾的机会 —— 这一格判的不是崩溃）。
    const pB = procs.get(INSTANCES[1].n);
    if (pB && pB.exitCode === null) {
      pB.kill("SIGTERM");
      await Promise.race([
        new Promise((r) => pB.once("exit", r)),
        sleep(8000).then(() => { if (pB.exitCode === null) pB.kill("SIGKILL"); }),
      ]);
    }
    if (pB) procs.delete(INSTANCES[1].n);
    const bDead = async () => !procs.has(INSTANCES[1].n) && !(await tcpOpen(INSTANCES[1].port));
    await waitFor(bDead, 20_000, "J6：B 确实下线（TCP 口不再有人听）");

    // B 缺席期间，群只长在 A 的盘上（四张写 = 建群命令跑到那一刻的形状）。
    const ts = nowMs();
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_members WHERE group_id=?1").run(NEW_GID);
      db.prepare("DELETE FROM groups WHERE id=?1").run(NEW_GID);
      db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
        .run(NEW_GID, NEW_NAME, S.idA.runtimeId, ts);
      for (const m of members) {
        db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)").run(NEW_GID, m);
      }
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)").run(`gk:${NEW_GID}`, GROUP_KEY_STR);
      db.prepare(
        "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
        + " VALUES(?1,'group',?2,NULL,0,?3)",
      ).run(newConv, NEW_NAME, ts);
    });
    // 清掉 B 盘上任何残留（上一轮跑过的话），否则"一无所知"那条前置就只是运气。
    seed(INSTANCES[1].db, (db) => {
      db.prepare("DELETE FROM group_members WHERE group_id=?1").run(NEW_GID);
      db.prepare("DELETE FROM groups WHERE id=?1").run(NEW_GID);
      db.prepare("DELETE FROM conversations WHERE id=?1").run(newConv);
      db.prepare("DELETE FROM settings WHERE key=?1").run(`gk:${NEW_GID}`);
    });

    const b0 = onDisk(INSTANCES[1].db, NEW_GID);
    check("前置成立：B 上线前对这个群零行可知（于是下面那条「学到了」有对照物，不是恒真）",
      b0.rows === 0 && b0.memberCount === 0 && b0.key === null && b0.conv === 0,
      "群行 0 / 成员 0 / 无密钥 / 无会话行",
      `群行 ${b0.rows} / 成员 ${b0.memberCount} / 密钥 ${b0.key === null ? "无" : "有"} / 会话 ${b0.conv}`);

    await sleep(10_000); // 一段"B 根本不在"的时间：这期间任何即时推送都发不出去
    launch(INSTANCES[1]);
    await waitFor(() => tcpOpen(INSTANCES[1].port), 60_000, "J6：B 重新上线（TCP 可连）");
    await waitFor(() => bootReady(INSTANCES[1].log, bootBaseOf.get(INSTANCES[1].n), BOOT_LINE),
      30_000, "J6：B 重新上线并打出 boot 完成行");

    // 有界地等它自己收敛：到点就把最后一次读数交给判据判红 —— 不 throw
    //（那会把一次真缺陷渲染成"基础设施超时"），也不写兜底断言（到不了就是死代码）。
    const t0 = nowMs();
    let b = onDisk(INSTANCES[1].db, NEW_GID);
    for (;;) {
      b = onDisk(INSTANCES[1].db, NEW_GID);
      if (b.rows > 0) break;
      if (nowMs() - t0 >= 150_000) break;
      await sleep(1_000);
    }
    const convergedMs = nowMs() - t0;
    const learned = onDisk(INSTANCES[1].db, wantGid);
    check("B 重新上线后必须自己学到这个新群（150s 内）—— 大文件把链路用满之后，名册仍是事实源",
      learned.group !== null && learned.group.creator === S.idA.runtimeId && learned.group.name === NEW_NAME,
      `1 行、creator=A、name=${NEW_NAME}`,
      learned.group === null
        ? `无行（等了 ${(convergedMs / 1000).toFixed(1)}s）`
        : `creator=${learned.group.creator} name=${learned.group.name} · ${(convergedMs / 1000).toFixed(1)}s`);
    check("学到的那份名册是 2 位（对方给的成员名单要一起落，不能只落一个光群名）",
      learned.memberCount === 2, 2, learned.memberCount);
    check("学到的那份群密钥逐字节等于 A 盘上那份（解不开群消息的话，落个群名是没用的）",
      learned.key === GROUP_KEY_STR, "与 A 那份逐字节相同",
      learned.key === null ? "null（行没了）" : `${learned.key.slice(0, 8)}…(${learned.key.length}B)`);
    // ⚠️ 这里原本还有一条「B 侧要同时出现这个群的会话行」—— 正向跑当场把它判成红，而红在判据不在产品：
    //   会话行由消息那条路点亮（`db/conversations.rs:30` 的 upsert、:66 的 ensure），群密钥落库这一路
    //   只写 groups / group_members / settings(gk:)；而界面对"有没有这个群"读的是另一条腿
    //   （`src/api/index.ts:149` 的 getGroups → `useChatStore.ts:682` 那一段），且 `list_conversations`
    //   只 SELECT FROM conversations、不与 groups 求并 ⇒ 把"会话行必须同时出现"写成判据，
    //   判的是产品从没承诺过的形状。"送达"由上面四条钉住（群行、creator、名册 2 位、密钥逐字节相同），
    //   "会话行随后由第一条群消息点亮"那一半已由 J4 在同一条被大文件用满的链上判过。
    check("这个群在 B 盘上恰好一行（重递不许把同一个群落成两份）",
      learned.rows === 1, 1, learned.rows);
  });
}
