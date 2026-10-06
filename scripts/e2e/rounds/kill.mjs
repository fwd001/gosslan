#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **KILL** 轮次分册（一族一轮：preset + run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "KILL";
import { INSTANCES, LIE, LIE_SHA, RUN_DIR, S, bootBaseOf, check, eid, launch, nowMs, openDb, procs, seed, sleep, step, tailLog, tcpOpen, waitFor, waitSendTerminal } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { BOOT_LINE, bootReady } from "../../e2e-logtail.mjs";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

/// 这一条注入专用的尺寸（与 J2 的 1 MB 分开，免得把默认轮也拖慢）。
const KILL_BYTES = Number(process.env.E2E_KILL_MB || 100) * 1024 * 1024;

let xferId4, srcFile4, srcSha4, partAtKill = 0;

export async function preset() {
  step(`预置（注入③）：先生成一个 ${KILL_BYTES / 1024 / 1024} MB 源文件（行进库留到判据里，见下面那段注释）`, () => {
    xferId4 = eid("k");
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile4 = path.join(dir, `${xferId4}.bin`);
    const buf = Buffer.alloc(KILL_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile4, buf);
    srcSha4 = createHash("sha256").update(buf).digest("hex");
    console.log(`      源文件 ${buf.length / 1024 / 1024} MB（sha256=${srcSha4.slice(0, 12)}…）`);
  });
}

export async function run() {
  step("故障注入判据③：接收中被 SIGKILL ⇒ 不许假成功，重启后按盘上真实字节续完", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const partPath = path.join(dl, `${xferId4}.part`);
    const landed = path.join(dl, `${xferId4}.bin`);
    const partSize = () => {
      try {
        return fs.statSync(partPath).size;
      } catch {
        return 0;
      }
    };
    // 这一单**由我在判据里才入队**：前两版都在停机时预置，于是传输发生在"起 A/B 等链路"
    // 那一步里，等判据去看时早传完了 —— 打空的两轮报红全是我的时序问题，不是产品的。
    // ⚠️ 进程活着时写它的库是新用法：seed() 末尾的 wal_checkpoint(TRUNCATE) 撞上在写的
    //    连接会 SQLITE_BUSY ⇒ 重试几次；真进不去就该换成"停机入队 + 大文件"那条路。
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId4);
          db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          ).run(xferId4, S.peerTo, srcFile4, `${xferId4}.bin`, KILL_BYTES, nowMs());
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }
    // waitFor 是 500ms 粒度，而实测一次 100 MB 传输只有 ~0.78s ⇒ 会打空。这里用 50ms 自旋。
    // 打没打中窗口是**这条注入自己**的成败，必须红给看，不许悄悄当成"已通过"。
    const inFlight = () => {
      const n = partSize();
      return n > 0 && n < KILL_BYTES;
    };
    let miss = "";
    const until = nowMs() + 120_000;
    while (nowMs() < until && !inFlight()) await sleep(50);
    if (!inFlight()) {
      miss =
        `120s 内没出现"在飞"的 .part（实际 ${partSize()} 字节）—— ` +
        `要么 A 没有在飞行中把这单捡起来，要么传得太快/太大没抓着`;
    }
    partAtKill = partSize();
    const pB = procs.get(INSTANCES[1].n);
    // ⚠️ 被信号杀死的子进程：`exitCode === null` + `signalCode === "SIGKILL"`。
    //    拿 exitCode !== null 判"死了没有"永远等不到（第一版就在这里超时）。
    const dead = () => !!pB && (pB.exitCode !== null || pB.signalCode !== null);
    if (!dead()) pB.kill("SIGKILL");
    await waitFor(dead, 15_000, "B 进程确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(2_000); // 让 A 把"写失败了"变成状态
    const landedAtKill = fs.existsSync(landed);
    // ★ 参考量取在**确认已死之后**，不取在按下 SIGKILL 之前：`.part` 只有 B 自己会写，
    //   所以此刻起它永久冻结 —— 这才是"死的那一刻盘上有多少字节"。
    //   原先拿 kill 前的快照当期望值，本地层实测把它判红了：快照 4,194,304，而重启后的 B 自己读到
    //   4,456,448 并要求从这里续（差恰好一个 256 KiB 片）⇒ **产品服从的是盘上真值，红的是判据自己的读数窗口**
    //   （从快照到真死 B 还在收片，且写入也要一会儿才在 stat 上显现；两种成因指向同一个修法）。
    //   与 waitSendTerminal 同一族：先问"这个数是靠谁定格的"。下面那行打印就是这扇窗的探针。
    const partAtDeath = partSize();
    console.log(`  · 实测：按下 SIGKILL 前读到 ${partAtKill} 字节，确认已死后冻结在 ${partAtDeath} 字节`
      + `（差 ${partAtDeath - partAtKill}，非零就是快照打早了 —— 期望值以冻结那个为准）`);

    check("窗口必须真打中：杀的那一刻 .part 在 0~全量之间",
      !miss && partAtKill > 0 && partAtKill < KILL_BYTES,
      `0 < .part < ${KILL_BYTES}`, miss || `${partAtKill} 字节`);
    check("rename 才算完成：B 死在半路时接收目录不许出现终名文件",
      !landedAtKill, "不存在", landedAtKill ? "已出现" : "不存在");
    const aMid = openDb(INSTANCES[0].db, true);
    const a4mid = aMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId4);
    aMid.close();
    check("发送端不许在对端没确认时宣布完成：对端被杀的那一刻 A 不能是 done",
      a4mid?.status !== "done", "非 done", a4mid?.status ?? "无行");

    // 重启 B：链路该自己回来、outbox 该自己重投，且必须**接着盘上那点字节**发
    launch(INSTANCES[1]);
    await waitFor(() => tcpOpen(INSTANCES[1].port), 60_000, `重启后的 B 的 TCP ${INSTANCES[1].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[1].log, bootBaseOf.get(INSTANCES[1].n), BOOT_LINE), 30_000,
      "重启后的 B 打出 boot 完成行");
    await waitFor(() => fs.existsSync(landed) || partSize() > partAtDeath, 120_000,
      "重启后这一单被重新拾起（.part 比死时更长，或终名文件出现）");
    await sleep(15_000); // 让续传 / rename / 多轮重试都落定

    // 期望值 = **确认已死后冻结的那个字节数**（不是按下 SIGKILL 之前的快照，见上面那段注释）。
    const wantFrom = LIE ? partAtDeath + 1 : partAtDeath;
    const aLog = tailLog(INSTANCES[0].log, 60000) || "";
    const line = aLog.split("\n").filter((l) => l.includes(xferId4) && l.includes("接收端已有")).pop() || "";
    const m = line.match(/接收端已有 (\d+) 字节/);
    check("重启后必须从**盘上真实字节数**续发，不许从 0 重灌（接收端真实进度优先）",
      !!m && Number(m[1]) === wantFrom, String(wantFrom),
      m ? m[1] : "A 日志里没有针对这一单的续发行");
    const exists4 = fs.existsSync(landed);
    const got4 = exists4 ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    const wantSha4 = LIE ? LIE_SHA : srcSha4;
    check("死前写的前缀 + 重启后续发的尾段 = 源文件（sha256 逐字节对得上）",
      exists4 && got4 === wantSha4, wantSha4.slice(0, 12) + "…",
      exists4 ? got4.slice(0, 12) + "…" : "未落地");
    const bDb4 = openDb(INSTANCES[1].db, true);
    const b4 = bDb4.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId4);
    bDb4.close();
    // ⚠️ 上面那句 `sleep(15_000)` 只是把 ack 竞态**藏住**，不是解决它（读早了照样红）。
    //   改成有界等 A 自己到终态：既不再靠运气，也不用白等 15s。
    const t4 = await waitSendTerminal(xferId4);
    const a4 = t4.row;
    const q4 = t4.queued;
    const both4 = `B=${b4.map((r) => r.status).join("/") || "无行"} A=${a4?.status ?? "无行"} outbox=${q4}`;
    check("恢复的终局只有一个：两侧 done 且 outbox 已清（不许停在中间态，也不许弃单）",
      b4.length === 1 && b4[0].status === "done" && a4?.status === "done" && q4 === 0,
      "1 行 + 双侧 done + outbox=0", both4);
    const kept4 = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(xferId4)) : [];
    check("续完之后接收目录只剩 1 个终名文件：无 .part 残留、无半截副本",
      kept4.length === 1 && kept4[0] === `${xferId4}.bin`, `${xferId4}.bin`, kept4.join(", ") || "空");
  });
}
