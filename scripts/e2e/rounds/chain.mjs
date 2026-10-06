#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **CHAIN** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "CHAIN";
import { ALL_INST, GROUP_ID, GROUP_KEY_B64, GROUP_KEY_STR, GROUP_NAME, INSTANCES, INST_C, ROUND, RUN_DIR, S, bootBaseOf, buildGroupEnvelope, check, ed25519Priv, launch, noteId, nowMs, openDb, readIdentity, seed, sleep, step, stopAll, tcpOpen, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { BOOT_LINE, bootReady, countLog } from "../../e2e-logtail.mjs";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
/// 转发轮的正文：判据里既要看 C 解出的明文等于它，也要把它写进 A 的 messages（明文列）。
const CHAIN_TEXT = "two-hop group message via B";
let chainMsgId;
/// 反向模式：拓扑、预置、投递全都一样，只把**判据要找的那条 msg_id** 换成必定不存在的值。
const CHAIN_LIE = ROUND === "gossip3-lie";

// §五「群聊 + gossip」这一族里最后一格：成员**不是 A 的直发对象**，只能靠中间人把 gossip 带给它。
// 两实例的群轮（`--round=group`）里 A→B 是直发（`group_outbox` 一发就中），
// 「收到 gossip 之后再扇给自己当时可达的邻居」这条路径从头到尾没被走过（**晚到的那一半现在由
// 下面的 `--round=gossip-late` 判**：中间人把窗口内转发过的信封在建链时重递一次） —— 那一半只有第三个实例能测。
//
// ⚠️ 边界一：这一格**证不了**"A 与 C 之间没有链路"（实测两次，都是红的）：
//   run-2026-09-26T09-20-42-139Z 与 run-2026-09-26T09-24-08-894Z 里，C 库内 `lan_enabled='false'`、
//   好友只有 B、端点只有 B，A 的日志仍然出现 `diag/announce_verified: from=<C 的 id>`
//   ⇒ **关掉局域网发现并没有停止广播，也没有停止接收侧的 announce 验证**，同机三实例必然互相建链。
//   所以拿"拓扑隔离"当前提会让这一轮常红。发现本身另立条目待拍板，不在测试任务里顺手改产品码。
//
// ★ 于是判据换成一条**机器可判定、且不依赖拓扑**的陈述：**A 的逐成员直发队列里从来没有面向 C 的行**
//   （见下面那条 `group_outbox ... peer_id=C` 必须 0 行）。这才是"C 收到的不是直发"的正身。
//
// ⚠️ 边界二：这一格判的是**多跳收敛**，不是"晚到成员补拉"（补拉那一半另有 `--round=gossip-late`）。
//   历史读数留档：实测（run-2026-09-26T09-24-08-894Z 与
//   同形的一次晚到构造）里让 C 在 A 发完之后才第一次上线 ⇒ 90 s 内 C 库里 0 行 ——
//   **那是 #77 落地之前的产品行为**，现在的形状是 B 除了"当时那一瞬间的扇出"之外，
//   还会在为新成员登记链路时把窗口内转发过的信封重递一次（有界：每组 16 条 / 10 分钟 / 30s 间隔）：
//   递不到补推。
//   那是产品行为，已按实测记进 roadmap 待拍板 —— 在这一轮里写成绿就是替产品许愿。
//   ⇒ 所以 **C 必须先于 A 起、且 B↔C 链路先建好再起 A**（2026-09-27：三端同时起会让这一格偶发红，
//     同一份二进制一次 91.2s 四条红、一次 1.1s 全绿 ⇒ 判据当时不可复现，改法见下面启动那一段）。
export async function run() {
  step("链式三实例：A 从没直发给 C 的那条群消息，C 仍收敛到了（中间人在收到的一瞬间扇出）", async () => {
    // 上一步（L-B）收尾时 A/B 已被 stopAll 停干净 ⇒ 下面写的都是**停机库**。
    // C 第一次拉起只为自建身份与库，而且单独拉（新库会广播 announce，别让它在这个窗口里被别人学到）。
    launch(INST_C);
    await waitFor(() => tcpOpen(INST_C.port), 60_000, "C 首启（只为建身份与库）：TCP 可连");
    await waitFor(() => bootReady(INST_C.log, bootBaseOf.get(INST_C.n), BOOT_LINE),
      30_000, "C 首启：打出 boot 完成行");
    await stopAll(); // 此刻 procs 里只有 C —— stopAll 顺带保证"没清理干净"当场炸

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
      // 陷阱：macOS 上 load() 优先信书签 ⇒ 只写路径（与 seedPair 同口径）
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recvC);
    });
    // B：把 C 追加进好友与端点。⚠️ 端点必须**先读回再追加** —— 直接覆盖会把 A 的端点清没，
    // 那样连 A-B 都断，整轮退化成「B 谁也没连上」的假红。
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
    // 三端各写一份群 + 同一份群密钥（逐列形状照群轮：content 存明文、receiver_id 是裸 group_id、
    // 初始 status='sent'、时钟一起推进否则撞 seq）。
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
      kind: "text", content: CHAIN_TEXT, ts, seq: 1,
    });
    chainMsgId = env.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
      db.prepare(
        `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
         VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sent')`,
      ).run(env.messageId, convId, S.idA.runtimeId, GROUP_ID, CHAIN_TEXT, ts);
      // 只给 B 一行：C 从始至终不是 A 的直发对象（这一条本身就是判据，见下面 aDb 那格）。
      db.prepare(
        `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
         VALUES(?1,?2,?3,?4,?5)`,
      ).run(env.messageId, GROUP_ID, S.idB.runtimeId, env.wire, ts);
      db.prepare("INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq").run(convId, 1);
    });

    // 三端同场起，但**时序有讲究**（2026-09-27 被一次实测推翻后改的）：
    // 先起 B 与 C、等 B↔C 链路真的建成，**最后**才起 A。
    // 为什么：中间人只在"收到的那一瞬间"把 gossip 扇给**当时可达**的邻居（这正是 #77 那一格的产品行为）。
    // 旧写法三端同时起 ⇒ A 的排队群消息可能在 C 的链路建好之前就到 B ⇒ B 无处可扇，C 永远收不到。
    // 现场：run-2026-09-26T18-33-35-042Z 里 C 侧四条断言全红、白等 91.2s，
    //      而同一份二进制换个启动时序 7/7 只花 1.1s（run-2026-09-26T18-37-54-706Z）
    //      ⇒ 这条判据当时**不可复现**，而"偶尔绿的门禁"比"没跑"更坏。
    // ⚠️ 改的只是**测试的同步点**，产品行为一字未动；晚到的邻居那一半现在由 #77 补递判（ #77。
    for (const inst of [INSTANCES[1], INST_C]) launch(inst);
    for (const inst of [INSTANCES[1], INST_C]) {
      await waitFor(() => tcpOpen(inst.port), 60_000, `链式轮：实例 ${inst.label} 的 TCP ${inst.port} 可连`);
      await waitFor(() => bootReady(inst.log, bootBaseOf.get(inst.n), BOOT_LINE), 30_000,
        `链式轮：实例 ${inst.label} 打出 boot 完成行`);
    }
    // 投递的同步点：C 必须先与 B 建成链路，否则"C 没收到"只是链路没建起来，判不到产品头上。
    await waitFor(() => countLog(INST_C.log, `建链 peer=${S.idB.runtimeId}`) > 0,
      60_000, "前置：C 与 B 先建成链路（这一步不过就不起 A）");
    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, "链式轮：A 的 TCP 可连");
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE),
      30_000, "链式轮：A 打出 boot 完成行");
    const bDb = openDb(INSTANCES[1].db, true);
    let bRows = [];
    try {
      const q = bDb.prepare("SELECT content,sender_id,conv_id FROM messages WHERE msg_id=?1");
      const until = nowMs() + 60_000;
      for (;;) {
        bRows = q.all(chainMsgId);
        if (bRows.length || nowMs() >= until) break;
        await sleep(1000);
      }
    } finally { bDb.close(); }
    check("前置：A 排的那条群消息先真到了 B（B 手里有过它，后面才谈得上转发）",
      bRows.length === 1 && bRows[0]?.content === CHAIN_TEXT,
      `1 行 / ${CHAIN_TEXT}`, `${bRows.length} 行 / ${bRows[0]?.content}`);

    // （B↔C 链路这一前置已经上移到"起 A 之前"，这里不再等第二次。）
    // 反向模式（§十四「错误行为测试」）：上面全部照跑，只把判据要去找的那个 msg_id 换成必定不存在的值
    // ⇒ 报不出红就说明下面几条读的不是真落库行。
    const judgedId = CHAIN_LIE ? noteId(`${chainMsgId}-lie`) : chainMsgId;
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

    check("★ C 的库里落了那条消息，且只有一行（A 从没直发给 C ⇒ 只能是中间人的 gossip 收敛）",
      rows.length === 1, 1, rows.length);
    check("C 侧解出的是明文正文（解密发生在 C 自己身上，不是谁代解后送明文）",
      rows[0]?.content === CHAIN_TEXT, CHAIN_TEXT, rows[0]?.content);
    check("C 侧记的发送者仍是 A（经手不改归属）",
      rows[0]?.sender_id === S.idA.runtimeId, S.idA.runtimeId, rows[0]?.sender_id);
    check("C 侧落在群会话、seq 与信封一致",
      rows[0]?.conv_id === convId && rows[0]?.seq === 1, `${convId}/seq=1`,
      `${rows[0]?.conv_id}/seq=${rows[0]?.seq}`);
    const aDb = openDb(INSTANCES[0].db, true);
    let outPeers = [];
    try {
      outPeers = aDb.prepare("SELECT peer_id FROM group_outbox WHERE msg_id=?1 AND peer_id=?2")
        .all(chainMsgId, idC.runtimeId).map((r) => r.peer_id);
    } finally { aDb.close(); }
    check("A 的逐成员直发队列里**没有任何面向 C 的行**（C 从来不是 A 的直发对象）",
      outPeers.length === 0, 0, JSON.stringify(outPeers));
    const bAgain = openDb(INSTANCES[1].db, true);
    let bCount = -1;
    try {
      bCount = bAgain.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(chainMsgId).c;
    } finally { bAgain.close(); }
    check("B 自己仍只有一行（把消息递给下游不会让自己重复落库）", bCount, 1, bCount);

    // C 的日志与库进产物：报告按 label 走（#74 之后判据是形状匹配），第三实例的现场不能只留在 appdata。
    try {
      fs.copyFileSync(INST_C.log, path.join(RUN_DIR, `instance-${INST_C.label}.app.log`));
      fs.mkdirSync(path.join(RUN_DIR, `sqlite-${INST_C.label}`), { recursive: true });
      for (const s of ["", "-wal", "-shm"]) {
        if (fs.existsSync(INST_C.db + s)) {
          fs.copyFileSync(INST_C.db + s, path.join(RUN_DIR, `sqlite-${INST_C.label}`, path.basename(INST_C.db + s)));
        }
      }
    } catch { /* 产物复制失败不改判定（判定只看库与日志里的真事实） */ }
  });
}
