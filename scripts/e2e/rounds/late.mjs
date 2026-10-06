#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **LATE** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "LATE";
import { ALL_INST, GROUP_ID, GROUP_KEY_B64, GROUP_KEY_STR, GROUP_NAME, INSTANCES, INST_C, ROUND, RUN_DIR, S, bootBaseOf, buildGroupEnvelope, check, ed25519Priv, launch, noteId, nowMs, openDb, readIdentity, seed, sleep, step, stopAll, tcpOpen, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { BOOT_LINE, bootReady, countLog } from "../../e2e-logtail.mjs";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
// 补递轮用另一段正文：判据里的文本比对就只可能命中这一轮的落库行
const LATE_TEXT = "late joiner replayed by B";
/// 反向模式：拓扑、预置、时序全都一样，只把判据要找的那条 msg_id 换成必定不存在的值。
const LATE_LIE = ROUND === "gossip-late-lie";

// ── #77：晚到成员的补递轮 ────────────────────────────────────────────
// 这一轮证明的是"以后还能补到"，与链式轮的"当时能扇到"是两条独立的判据（拓扑相同、时序相反）。
export async function run() {
  step("晚到成员补递：A 在 C 上线之前就发完，C 与中间人建链之后仍拿到了那条（#77）", async () => {
    // C 第一次拉起只为自建身份与库，而且单独拉（新库会广播 announce，别让它在这个窗口里被别人学到）。
    launch(INST_C);
    await waitFor(() => tcpOpen(INST_C.port), 60_000, "C 首启（只为建身份与库）：TCP 可连");
    await waitFor(() => bootReady(INST_C.log, bootBaseOf.get(INST_C.n), BOOT_LINE),
      30_000, "C 首启：打出 boot 完成行");
    await stopAll();

    const idC = readIdentity(INST_C);
    const recvC = path.join(RUN_DIR, "recv", INST_C.label);
    fs.mkdirSync(recvC, { recursive: true });
    seed(INST_C.db, (db) => {
      db.prepare(
        `INSERT INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(S.idB.runtimeId, `e2e-${INSTANCES[1].label}`, S.idB.x25519Pub, Date.now());
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify([{ address: `127.0.0.1:${INSTANCES[1].port}` }]));
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recvC);
    });
    // B：把 C 追加进好友与端点。⚠️ 端点必须**先读回再追加**（同链式轮）—— 直接覆盖会把 A 的端点清没。
    seed(INSTANCES[1].db, (db) => {
      db.prepare(
        `INSERT OR IGNORE INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(idC.runtimeId, `e2e-${INST_C.label}`, idC.x25519Pub, Date.now());
      const cur = db.prepare("SELECT value FROM settings WHERE key='routed_endpoints'").get();
      const eps = JSON.parse(cur?.value || "[]");
      const addr = `127.0.0.1:${INST_C.port}`;
      if (!eps.some((e) => e.address === addr)) eps.push({ address: addr });
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify(eps));
    });
    const members = [S.idA.runtimeId, S.idB.runtimeId, idC.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of ALL_INST) {
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
    const env = buildGroupEnvelope({
      groupKey: GROUP_KEY_B64, senderId: S.idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: S.idA.x25519Pub, ed25519Pub: S.idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: S.idA.runtimeId, members,
      kind: "text", content: LATE_TEXT, ts, seq: 1,
    });
    const lateMsgId = env.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
      db.prepare(
        `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
         VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sent')`,
      ).run(env.messageId, convId, S.idA.runtimeId, GROUP_ID, LATE_TEXT, ts);
      // 只给 B 一行：C 从始至终不是 A 的直发对象（下面 A 侧那一格钉的就是这个）。
      db.prepare(
        `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
         VALUES(?1,?2,?3,?4,?5)`,
      ).run(env.messageId, GROUP_ID, S.idB.runtimeId, env.wire, ts);
      db.prepare("INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq").run(convId, 1);
    });

    // ★ 时序：先起 A 与 B、让那条**走完 A→B**，最后才起 C。
    // 反过来（三端同场）就退化成链式轮 —— 那一刻 C 的链路已经在了，"补递"根本没有发生的必要。
    for (const inst of [INSTANCES[0], INSTANCES[1]]) launch(inst);
    for (const inst of [INSTANCES[0], INSTANCES[1]]) {
      await waitFor(() => tcpOpen(inst.port), 60_000, `补递轮：实例 ${inst.label} 的 TCP ${inst.port} 可连`);
      await waitFor(() => bootReady(inst.log, bootBaseOf.get(inst.n), BOOT_LINE), 30_000,
        `补递轮：实例 ${inst.label} 打出 boot 完成行`);
    }
    const bDb = openDb(INSTANCES[1].db, true);
    let bRows = [];
    try {
      const q = bDb.prepare("SELECT content,sender_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 60_000;
      for (;;) {
        bRows = q.all(lateMsgId);
        if (bRows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { bDb.close(); }
    check("前置：C 还没上线，那条群消息就已经落在 B 的库里（B 手里有过它，才谈得上'以后补'）",
      bRows.length === 1 && bRows[0]?.content === LATE_TEXT,
      `1 行 / ${LATE_TEXT}`, `${bRows.length} 行 / ${bRows[0]?.content}`);

    launch(INST_C);
    await waitFor(() => tcpOpen(INST_C.port), 60_000, "补递轮：C 上线后 TCP 可连");
    await waitFor(() => bootReady(INST_C.log, bootBaseOf.get(INST_C.n), BOOT_LINE),
      30_000, "补递轮：C 打出 boot 完成行");
    // 补递的触发点 = B 为 C 登记链路的那一刻（C 拨 B ⇒ B 侧走入站 accept）。
    await waitFor(() => countLog(INST_C.log, `建链 peer=${S.idB.runtimeId}`) > 0,
      60_000, "前置：C 与 B 建成链路（这一步不过就没有补递的触发点）");
    // ⚠️ 这里**不设断言**（2026-09-27 自己抓到的一条 flaky）：曾经写成
    //   "C 侧从来没有与 A 的建链行" ⇒ 判 `countLog(...) === 0`。同一台机器上三实例是**能**经局域网
    //   互相发现的（这条边界早就写在链式轮那一格的 why 里：同机造不出"A-C 无链路"），
    //   所以那个 0 只是"A 还没轮到拨 C"的瞬时读数 —— 采到的那一刻是 0、下一次跑就是 1
    //   （实测：run-…20-53 里 1.7s 报红"实际 1"，而 run-…20-16 与 20-39 两次都是 0）。
    //   ⇒ 拿"某一瞬间没发生"当判据 = 竞态判据。归因不靠它也能立：
    //     ① A 的逐成员直发队列里没有面向 C 的行（下面那条断言）⇒ 这一帧不是 A 直发的；
    //     ② B 的日志里有指向 C 的补递行 ⇒ 这一帧是 B 补的。
    //   读数继续打印，进报告产物，只是不当判据。
    console.log(`     · [只记录，不判] C 侧与 A 的建链行数=${countLog(INST_C.log, `建链 peer=${S.idA.runtimeId}`)}`
      + "（同机局域网能互达 ⇒ 这个数不是判据，见上面注释）");

    // 归因这一格：C 收到这一条**只能**来自补递 —— B 的日志里那一行是本机自己打的，
    // 没有它就没有"哪条路径递的"这个问题的答案（链式轮那格证明不了这一轮，反之亦然）。
    let replayLines = 0;
    {
      const until = nowMs() + 30_000;
      for (;;) {
        replayLines = countLog(INSTANCES[1].log, `补递群消息 peer=${idC.runtimeId}`);
        if (replayLines > 0 || nowMs() >= until) break;
        await sleep(1000);
      }
    }
    check("★ B 的日志里出现补递那一行，且指向的正是刚上线的 C（证明这一条是「以后补的」而不是「当时扇的」）",
      replayLines > 0, "至少 1 行 peer=C 的补递", `${replayLines} 行`);

    const judgedId = LATE_LIE ? noteId(`${lateMsgId}-lie`) : lateMsgId;
    const cDb = openDb(INST_C.db, true);
    let rows = [];
    try {
      const q = cDb.prepare("SELECT content,seq,sender_id,conv_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 90_000;
      for (;;) {
        rows = q.all(judgedId);
        if (rows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { cDb.close(); } // 句柄只开一次：这条循环最多读 90 遍，每遍重开会放大 BUSY 概率
    check("★ C 的库里落了那条消息，且只有一行（它上线时 A 早发完了 ⇒ 只可能是中间人补的）",
      rows.length === 1, 1, rows.length);
    check("C 侧解出明文正文、发送者仍是 A、落在群会话且 seq 与信封一致",
      rows[0]?.content === LATE_TEXT && rows[0]?.sender_id === S.idA.runtimeId
      && rows[0]?.conv_id === convId && rows[0]?.seq === 1,
      `${LATE_TEXT} / A / ${convId} / seq=1`,
      `${rows[0]?.content} / ${rows[0]?.sender_id} / ${rows[0]?.conv_id} / seq=${rows[0]?.seq}`);
    const aDb = openDb(INSTANCES[0].db, true);
    let outPeers = [];
    try {
      outPeers = aDb.prepare("SELECT peer_id FROM group_outbox WHERE msg_id=?1 AND peer_id=?2")
        .all(lateMsgId, idC.runtimeId).map((r) => r.peer_id);
    } finally { aDb.close(); }
    check("A 的逐成员直发队列里**没有任何面向 C 的行**（C 从来不是 A 的直发对象）",
      outPeers.length === 0, 0, JSON.stringify(outPeers));

    try {
      fs.copyFileSync(INST_C.log, path.join(RUN_DIR, `instance-${INST_C.label}.app.log`));
      fs.mkdirSync(path.join(RUN_DIR, `sqlite-${INST_C.label}`), { recursive: true });
      for (const suffix of ["", "-wal", "-shm"]) {
        if (fs.existsSync(INST_C.db + suffix)) {
          fs.copyFileSync(INST_C.db + suffix,
            path.join(RUN_DIR, `sqlite-${INST_C.label}`, path.basename(INST_C.db + suffix)));
        }
      }
    } catch { /* 产物复制失败不改判定（判定只看库与日志里的真事实） */ }
  });
}
