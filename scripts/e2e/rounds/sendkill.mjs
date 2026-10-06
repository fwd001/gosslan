#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **SENDKILL** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "SENDKILL";
import { INSTANCES, LIE, LIE_SHA, RUN_DIR, S, bootBaseOf, check, eid, launch, nowMs, openDb, procs, seed, sleep, step, tcpOpen, waitFor, waitSendTerminal } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { BOOT_LINE, bootReady } from "../../e2e-logtail.mjs";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

/// 和 ③ 同档：100 MB 在回环上传 ~0.8 s，50 ms 自旋才抓得到在飞窗口。
const SENDKILL_BYTES = Number(process.env.E2E_SENDKILL_MB || 100) * 1024 * 1024;

export async function run() {
  step("故障注入判据⑩：发送端在飞时被 SIGKILL ⇒ 崩溃不许抹掉「已入队」，重启后必须自己续完", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dl, { recursive: true });
    fs.mkdirSync(srcDir, { recursive: true });
    const idK = eid("k");
    const srcFileK = path.join(srcDir, `${idK}.bin`);
    fs.writeFileSync(srcFileK, Buffer.alloc(SENDKILL_BYTES));
    const srcShaK = createHash("sha256").update(fs.readFileSync(srcFileK)).digest("hex");
    const landedK = path.join(dl, `${idK}.bin`);
    const partK = path.join(dl, `${idK}.part`);
    const sizeK = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    // lie：注入完全相同，只换"补完之后该等于哪个摘要"⇒ 第 4 条必须红。
    const wantShaK = LIE ? LIE_SHA : srcShaK;
    const pA = procs.get(INSTANCES[0].n);
    if (!pA) throw new Error("拿不到 A 的子进程句柄 —— 这一格要杀的正是发送端");

    // 入队 = 复刻 `send_file` 命令在点击那一刻写的**三行**（气泡 / 传输台账 / 队列），照⑥ 的同形状。
    // 只写队列那一行就证不到"用户看得见的那一单"：崩溃后气泡没了 = 用户以为发过、其实没人再发，
    // 而台账与队列都在时用户界面上至少还有个入口 —— 这两件事在 UI 上是两个不同的结局。
    // ⚠️ 进程活着时写它的库是新用法：seed() 末尾的 wal_checkpoint(TRUNCATE) 撞上在写的连接会
    //    SQLITE_BUSY ⇒ 有限重试；真进不去就该换成"停机入队 + 更大文件"那条路。
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(idK);
          db.prepare("DELETE FROM file_transfers WHERE id=?1").run(idK);
          db.prepare("DELETE FROM messages WHERE msg_id=?1").run(`file-${idK}`);
          const ts = nowMs();
          const seq = db.prepare("SELECT COALESCE(MAX(seq),0)+1 s FROM messages WHERE conv_id=?1")
            .get(S.peerTo).s;
          db.prepare(
            `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
             VALUES(?1,?2,?3,?4,'file',?5,?6,?7,'sent')`,
          ).run(`file-${idK}`, S.peerTo, S.idA.runtimeId, S.peerTo,
            JSON.stringify({ name: `${idK}.bin`, path: srcFileK, size: SENDKILL_BYTES, sha256: "", subtype: "file" }),
            ts, seq);
          db.prepare(
            `INSERT INTO file_transfers(id,peer_id,name,size,direction,status,path,progress,created_at)
             VALUES(?1,?2,?3,?4,'send','pending',?5,0,?6)`,
          ).run(idK, S.peerTo, `${idK}.bin`, SENDKILL_BYTES, srcFileK, ts);
          db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          ).run(idK, S.peerTo, srcFileK, `${idK}.bin`, SENDKILL_BYTES, ts);
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }

    // ★ 世界前提（抛异常，不设判据）：必须抓到"字节正在飞"。抓不到就等于什么都没注入，
    //   而后面几条"不许成功"会因为链路根本没跑而集体假绿 —— 那是最像成功的一种失败。
    //   50 ms 自旋：100 MB 在回环上 ~0.8 s 就传完了，waitFor 的 500 ms 粒度抓不住。
    let atKill = -1;
    const spinT0 = nowMs();
    for (;;) {
      atKill = sizeK(partK);
      if (atKill > 0 && atKill < SENDKILL_BYTES) break;
      if (sizeK(landedK) >= 0) throw new Error(`还没杀就已收完（${(nowMs() - spinT0) / 1000}s）—— 在飞窗口没抓到`);
      if (nowMs() - spinT0 > 120_000) throw new Error(`等 120s 仍没抓到在飞 .part（实际 ${atKill} 字节）`);
      await sleep(50);
    }
    // ⚠️ 被信号杀死的子进程 `exitCode === null`，只有 `signalCode` 有值（③ 踩过同一个坑）。
    const deadA = () => pA.exitCode !== null || pA.signalCode !== null;
    pA.kill("SIGKILL");
    await waitFor(deadA, 15_000, "A 进程确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(20_000); // 一段"发送端根本不存在"的时间：这期间 B 不该收到任何东西

    const straysK = fs.existsSync(dl)
      ? fs.readdirSync(dl).filter((f) => f.includes(idK) && f !== `${idK}.part`)
      : [];
    check("发送端已死这 20s 内，接收目录不许出现终名文件（没人发 FileDone，完成无从谈起）",
      sizeK(landedK) < 0, "不存在", sizeK(landedK) >= 0 ? `已出现 ${sizeK(landedK)} 字节` : "不存在");
    const bMidDb = openDb(INSTANCES[1].db, true);
    const bMid = bMidDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idK) ?? null;
    bMidDb.close();
    check("发送端已死这 20s 内，接收侧台账不许被记成 done",
      bMid?.status !== "done", "非 done", bMid?.status ?? "无行");
    // ★ 这一条是整轮的重点：「先入队再投递」的另一半 —— 崩溃**不许**把入队事实抹掉。
    //   队列行没了 = 这一单永久没人再发，而用户界面上的气泡还在（最难发现的一种丢法）。
    const aDeadDb = openDb(INSTANCES[0].db, true);
    const aDeadQ = aDeadDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(idK).c;
    const aDeadRow = aDeadDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idK) ?? null;
    const aDeadBubble = aDeadDb
      .prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(`file-${idK}`).c;
    aDeadDb.close();
    check("发送端崩溃之后队列行必须还在（1 行，等待重启后重投）—— 崩溃不许当成已送达",
      aDeadQ === 1, 1, aDeadQ);
    // ★ 用户可见的那一半：崩溃不许把**会话里的那条气泡**一起带走。队列行没了 = 永久没人再发，
    //   气泡没了 = 用户连"曾经发过这一单"都看不见；两者在 UI 上是两个不同的结局，所以要各钉一条。
    check("发送端崩溃之后那条文件气泡必须还在（1 行）—— 崩溃不许把用户可见的这一单一起抹掉",
      aDeadBubble === 1, 1, aDeadBubble);
    check("发送端崩溃的这一刻自己不许记成 done（对端一个字节都没确认过）",
      aDeadRow?.status !== "done", "非 done", aDeadRow?.status ?? "无行");
    console.log(`  · 实测：在飞 .part=${atKill}B 时 SIGKILL A → 死透后等 20s`
      + `（B 台账=${bMid?.status ?? "无行"} · A 队列行=${aDeadQ} · A 台账=${aDeadRow?.status ?? "无行"}）`);

    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, `重启后的 A 的 TCP ${INSTANCES[0].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE), 30_000,
      "重启后的 A 打出 boot 完成行");
    const t0 = nowMs();
    await waitFor(() => sizeK(landedK) >= 0, 180_000, "重启后的 A 必须把这一单自己补完并 rename 成终名");
    const gotK = sizeK(landedK) >= 0
      ? createHash("sha256").update(fs.readFileSync(landedK)).digest("hex") : null;
    check("重启后补完的那份必须等于源文件（半途崩溃不许留下坏内容当成品）",
      gotK === wantShaK, wantShaK.slice(0, 12) + "…", gotK ? gotK.slice(0, 12) + "…" : "未落地");
    // 读序照⑨ 的教训：先等 A 自己走到终态（它的 done 由 B 的 ack 点亮，B 落地不是 A 的同步点），
    // 再读 B —— 接收侧的 done 由 rename 触发、ack 在其后 ⇒ B 一定不比 A 晚。
    const endK = await waitSendTerminal(idK);
    const aK = endK.row;
    const qK = endK.queued;
    const bEndDb = openDb(INSTANCES[1].db, true);
    const bK = bEndDb.prepare("SELECT status FROM file_transfers WHERE id=?1").all(idK);
    bEndDb.close();
    check("重启后的终局只有一个：两侧 done 且队列清零（不许停在中间态，也不许弃单）",
      bK.length === 1 && bK[0].status === "done" && aK?.status === "done" && qK === 0,
      `B=done A=done outbox=0`,
      `B=${bK.map((r) => r.status).join("/") || "无行"} A=${aK?.status ?? "无行"} outbox=${qK}`);
    const strays2K = fs.existsSync(dl)
      ? fs.readdirSync(dl).filter((f) => f.includes(idK) && f !== `${idK}.bin`) : [];
    check("补完之后接收目录不许留下半截 .part / 改名副本",
      strays2K.length === 0 && straysK.length === 0, "无残留",
      `崩溃窗口 ${straysK.length} 个 · 收尾 ${strays2K.length} 个`);
    console.log(`  · 实测：A 重启 → 终名落地 ${(nowMs() - t0) / 1000}s；`
      + `崩溃时 .part=${atKill}B → 落地 ${sizeK(landedK)}B（同一单从盘上真实进度续完）`);
  });
}
