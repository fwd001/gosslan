#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **DMREACT** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "DMREACT";
import { INSTANCES, ROUND, S, check, eid, nowMs, openDb, seed, sleep, step, waitFor } from "../core.mjs";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
const DMREACT_LIE = ROUND === "dmreaction-lie";

// §六 点名的「1:1 也要表情回应」跨实例那一半。这一轮判的是静默事件唯一"看得见"的那两面：
// 它必须落到对端时间线上（送达），又必须**不碰**那条会话的未读与摘要（不打扰）。
// 反证只翻判据读的那两份期望值（未读数 + 摘要哨兵），预置与时序一字不动 ⇒ 恰好 2 条红；
// 而「恰好落一条」「kind 仍是 reaction」「A 侧队列被对端 Ack 回收」三条读的是真 msg_id ⇒
// 反证档里它们照绿，这就是「红来自断言本身、不是链路没跑」的对照。
// ⚠️ 判不到的两半按 §十五 记未验证、不写 PASS：① 发送侧门控（dm_allowed_by_features 在
// send_message 里面，harness 没有"让应用执行一条命令"的入口）；② "折叠成一枚胶囊"（那是
// 前端折叠，DB 里没有那一格）。
export async function run() {
  step("J6 1:1 的一条静默事件跨实例：对端必须收到，且不许顶会话、不许改摘要", async () => {
    const SENT = "安静的那句摘要";
    const reactMsgId = eid();
    const ts = nowMs();
    // 被回应的"原消息"用 J1 那条真落库的 id：静默事件必须指向一条真实存在的消息。
    const plain = JSON.stringify({ target: S.msgId, emoji: "[赞]", add: true });
    const payload = JSON.stringify({
      type: "chat_message", msg_id: reactMsgId, from: S.idA.runtimeId, to: S.idB.runtimeId,
      kind: "reaction", content: "enc1:harness-placeholder", ts, seq: 9,
    });
    // 把 B 侧那条会话摆成一个可分辨的静止态。钉的是 last_msg / last_ts 两列 —— 它们**只有**
    // `touch_conversation` 会写，而同一句写把 unread+1 与摘要一起写进去；静默分支走的是
    // `ensure_conversation`（INSERT OR IGNORE，一个字节都不碰）⇒ 钉住这两列就等于钉住
    // "没走那条会顶会话的路"。不这么做的话"摘要没变"只是 NULL 在绿（这几轮的 seed 从不写
    // last_msg，那一格本来没有对照物）。
    // ⚠️ 为什么**不**拿 unread 当判据：B 是带界面的活进程，界面对打开着的会话本来就会清未读
    // ⇒ unread 是产品自己拥有的列，判它量到的是 UI 的动作而不是这条消息的分支。
    // 本轮第一次跑就是被这一点判红的（实测 unread=0、同一句 UPDATE 写的摘要哨兵却保住了）。
    const QUIET_TS = 1_700_000_000_000;
    seed(INSTANCES[1].db, (db) => {
      db.prepare("UPDATE conversations SET last_msg=?1,last_ts=?2 WHERE id=?3")
        .run(SENT, QUIET_TS, S.idA.runtimeId);
    });
    // 与 J1/J3 同一个配方：两行必须成对，只插 outbox 则 re-seal 没有明文，只插 messages 则 Ack 找不到人。
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM messages WHERE msg_id=?1").run(reactMsgId);
      db.prepare("DELETE FROM outbox WHERE msg_id=?1").run(reactMsgId);
      db.prepare(
        `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
         VALUES(?1,?2,?3,?4,'reaction',?5,?6,9,'sending')`,
      ).run(reactMsgId, S.idB.runtimeId, S.idA.runtimeId, S.idB.runtimeId, plain, ts);
      db.prepare("INSERT INTO outbox(msg_id,peer_id,payload,created_at) VALUES(?1,?2,?3,?4)")
        .run(reactMsgId, S.idB.runtimeId, payload, ts);
    });
    const readB = () => {
      const d = openDb(INSTANCES[1].db, true);
      try {
        return d.prepare("SELECT kind,content,sender_id FROM messages WHERE msg_id=?1").all(reactMsgId);
      } finally { d.close(); }
    };
    const convB = () => {
      const d = openDb(INSTANCES[1].db, true);
      try {
        return d.prepare("SELECT last_msg,last_ts FROM conversations WHERE id=?1")
          .get(S.idA.runtimeId);
      } finally { d.close(); }
    };
    const queued = () => {
      const d = openDb(INSTANCES[0].db, true);
      try {
        return d.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(reactMsgId).c;
      } finally { d.close(); }
    };
    // 这条是在链路已经建好、应用已经在跑之后才塞进队列的 ⇒ 没有"下一次建链"来冲它，
    // 只能等它自己那趟心跳/重试 ⇒ 等待窗口沿用续发轮那一档，不拿 J1 的硬套。
    await waitFor(() => readB().length > 0, 150_000, "B 侧出现这条表情回应");
    await waitFor(() => queued() === 0, 90_000, "A 侧这条的 outbox 被 Ack 删除");
    await sleep(1500); // 多留一点窗口，让「重复投递」这种退化有机会显现

    const rows = readB();
    const pre = convB();
    const wantTs = DMREACT_LIE ? QUIET_TS + 1 : QUIET_TS;
    const wantPreview = DMREACT_LIE ? `${SENT}（被改写）` : SENT;
    check("表情回应跨实例恰好落一条（1:1 收得到静默事件，也没重复投）",
      rows.length === 1, 1, rows.length);
    check("B 侧这一行的 kind 仍是 reaction（新枚举个变体真的被对端解析并落库）",
      rows[0]?.kind === "reaction", "reaction", rows[0]?.kind ?? "无行");
    check("预置成立：那条会话确实被摆成静止态（不成立则下面两条是在判一个不存在的行）",
      pre?.last_msg === SENT && pre?.last_ts === QUIET_TS, true,
      `${pre?.last_ts ?? "无行"}/${pre?.last_msg ?? "无行"}`);
    check("这条静默事件没把会话的时间戳顶上去（顶上去=走了 touch_conversation，顺带就 +1 未读）",
      pre?.last_ts === wantTs, wantTs, pre?.last_ts ?? "无行");
    check("那条会话的摘要仍是那句哨兵（没被载荷那段 JSON 覆写）",
      pre?.last_msg === wantPreview, wantPreview, pre?.last_msg ?? "无行");
    check("A 侧这一行的 outbox 被对端 Ack 回收（读真 id ⇒ 反证档里它照绿）",
      queued() === 0, 0, queued());
  });
}
