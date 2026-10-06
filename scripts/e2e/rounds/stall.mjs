#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **STALL** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "STALL";
import { INSTANCES, LIE, LIE_SHA, RUN_DIR, S, check, eid, nowMs, openDb, procs, seed, sleep, step, tailLog, waitFor, waitSendTerminal } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

const STALL_BYTES = Number(process.env.E2E_STALL_MB || 100) * 1024 * 1024;

/// 冻结时长：必须 **>60 s** 才越过 `FILE_STALL_ABORT_MS`；留 10 s 余量给 5 s 一跳的停滞检查。
const STALL_HOLD_MS = Number(process.env.E2E_STALL_S || 70) * 1000;

export async function run() {
  step("故障注入判据⑨：字节在飞时冻住对端 ⇒ 冻结期间盘上进度一字不涨，解冻后必须补到源内容", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dl, { recursive: true });
    fs.mkdirSync(srcDir, { recursive: true });
    const idS = eid("s");
    const srcFileS = path.join(srcDir, `${idS}.bin`);
    fs.writeFileSync(srcFileS, Buffer.alloc(STALL_BYTES));
    const srcShaS = createHash("sha256").update(fs.readFileSync(srcFileS)).digest("hex");
    const landedS = path.join(dl, `${idS}.bin`);
    const partS = path.join(dl, `${idS}.part`);
    const sizeOfS = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    // lie：注入完全相同，只换"补完之后该等于哪个摘要"⇒ 第 7 条必须红。
    const wantShaS = LIE ? LIE_SHA : srcShaS;
    const pB = procs.get(INSTANCES[1].n);
    if (!pB) throw new Error("拿不到 B 的子进程句柄 —— 这条注入没有可冻结的对象");

    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(idS);
      db.prepare(
        `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
         VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
      ).run(idS, S.peerTo, srcFileS, `${idS}.bin`, STALL_BYTES, nowMs());
    });

    let frozen = false;
    let thawed = false;
    let atGrowth = -1;
    let atFreeze = -1;
    let mid = null;
    try {
      // ⚠️ 自旋等 `.part` 真的开始长 —— 这一条是整轮的**世界前提**：
      // 抓不到在飞字节，后面"不许假成功"那几条会因为链路根本没跑而集体假绿。
      // 50ms 粒度（不是 waitFor 的 500ms）：100 MB 在回环上 ~0.78s 就走完了。
      const spinT0 = nowMs();
      for (;;) {
        atGrowth = sizeOfS(partS);
        if (atGrowth > 0) break;
        if (sizeOfS(landedS) >= 0) throw new Error(`还没冻住就已经收完（${(nowMs() - spinT0) / 1000}s）—— 在飞窗口没抓到`);
        if (nowMs() - spinT0 > 20_000) throw new Error("等 20s 仍没有 .part：这一单根本没被投递");
        await sleep(50);
      }
      const freezeT0 = nowMs();
      pB.kill("SIGSTOP");
      frozen = true;
      // 快照取在**冻结之后**：冻结前那几毫秒 B 还在写，拿 atGrowth 当基准会虚涨。
      // ⚠️ 还要再等一下：`kill("SIGSTOP")` 是**异步生效**的，信号排到队上之后 B 仍可能写完一片。
      //   这一格的基准要是取早了，就会把"B 在信号生效前最后写的那片"算成"冻结期间涨了" ——
      //   与注入③ 同族的读数竞态（那里是 SIGKILL 前快照，已改成确认已死后再读）。
      await sleep(300);
      atFreeze = sizeOfS(partS);
      await sleep(STALL_HOLD_MS);

      const midDb = openDb(INSTANCES[0].db, true);
      // ⚠️ `attempts` 在 `file_outbox` 上，不在 `file_transfers` 上（第一版在这里写了
      //   `SELECT status,attempts FROM file_transfers` ⇒ no such column，红的是判据不是产品）。
      const midA = midDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idS) ?? null;
      const midOut = midDb.prepare("SELECT status, attempts FROM file_outbox WHERE transfer_id=?1").get(idS) ?? null;
      midDb.close();
      const midBDb = openDb(INSTANCES[1].db, true);
      const midB = midBDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(idS) ?? null;
      midBDb.close();
      mid = {
        part: sizeOfS(partS), landed: sizeOfS(landedS),
        a: midA?.status ?? null, b: midB?.status ?? null,
        out: midOut ? `${midOut.status}/${midOut.attempts}` : "无行",
      };
      check("冻结期间接收目录不许出现终名文件（没写完就没有完成可言）",
        mid.landed < 0, "不存在", mid.landed >= 0 ? `已出现 ${mid.landed} 字节` : "不存在");
      check("冻结期间发送侧不许记成 done", mid.a?.status !== "done", "非 done", mid.a?.status ?? "无行");
      check("冻结期间接收侧不许记成 done（它一次回执都没发出去）",
        mid.b?.status !== "done", "非 done", mid.b?.status ?? "无行");
      // ★ 这一条钉的是不变量「接收端真实进度」的跨实例形状：对端停止写盘之后，
      //   盘上进度必须**一字不涨**。A 往 socket 里灌的字节、内核缓冲的字节都不算进度。
      check("对端被冻住期间，接收端盘上进度不许继续涨（进度只能来自真实写入）",
        mid.part === atFreeze, `冻结时刻的 ${atFreeze} 字节`,
        mid.part === atFreeze ? `${atFreeze} 字节（一字未涨）` : `涨到 ${mid.part} 字节`);

      // 走的是哪条分支只打印、不判：**2026-09-26 已量清** —— 先到的是 45 s 链路 watchdog（15 s × 3），
      // 60 s 的 `FILE_STALL_ABORT_MS` 在"对端完全冻死"下够不到（156 份归档 run 里 `[STALL]` 命中 0 行），
      // 所以把"看到 [STALL]"写成判据必然是永远不成立的空转 ⇒ 只打印。机制与那 0 行的账记在 roadmap A-13。
      const abandon = (tailLog(INSTANCES[0].log, 200000) || "").split("\n")
        .filter((x) => x.includes(idS) && /STALL|放弃|ok=false|失败|拒绝|error/i.test(x)).slice(-6);
      console.log(`  · 实测：抓到在飞 .part=${atGrowth}B → 冻结时 ${atFreeze}B → 冻后 ${mid.part}B`
        + `（冻结 ${(nowMs() - freezeT0) / 1000}s）`);
      console.log(`  · 实测：A 侧这一单 ${mid.a ?? "无行"} / B 侧 ${mid.b ?? "无行"}`
        + ` / 队列行 status/attempts=${mid.out}`);
      for (const l of abandon) console.log("      A│ " + l.slice(0, 220));
      if (!abandon.length) console.log("      · A 侧本轮没留下'这一单已结束'的日志痕迹（记下，待判是否 A-13）");

      pB.kill("SIGCONT");
      thawed = true;
      const t0 = nowMs();
      await waitFor(() => sizeOfS(landedS) >= 0, 180_000, "解冻后必须把这一单补完并 rename 成终名");
      console.log(`  · 实测：解冻 → 终名落地 ${(nowMs() - t0) / 1000}s`);
    } finally {
      // ⚠️ 被 SIGSTOP 停住的进程收不到 SIGTERM ⇒ 不解冻会把整条 harness 挂死（冻结轮的教训）。
      if (frozen && !thawed) { try { pB.kill("SIGCONT"); } catch { /* 已经退了 */ } }
    }

    const gotS = sizeOfS(landedS) >= 0
      ? createHash("sha256").update(fs.readFileSync(landedS)).digest("hex") : null;
    check("补完之后的字节内容必须等于源文件（半途放弃不许留下坏内容当成功）",
      gotS === wantShaS, wantShaS.slice(0, 12) + "…", gotS ? gotS.slice(0, 12) + "…" : "未落地");
    // ⚠️ 判终局之前必须**等 A 自己走到终态**，不能拿"B 的终名文件出现"当同步点
    //   （理由与窗口取值都写在 `waitSendTerminal` 上；这一轮就是它被本地层判红的现场）。
    //   读序：先等 A，再读 B —— 接收侧的 done 由 rename 触发、ack 在其后 ⇒ B 一定不比 A 晚。
    const endS = await waitSendTerminal(idS);
    const aS = endS.row;
    const qS = endS.queued;
    const bDbS = openDb(INSTANCES[1].db, true);
    const bS = bDbS.prepare("SELECT status FROM file_transfers WHERE id=?1").all(idS);
    bDbS.close();
    console.log(`  · 实测：B 落地 → A 终态 ${(endS.waitedMs / 1000).toFixed(1)}s`
      + `（A=${aS?.status ?? "无行"} outbox=${qS}）`);
    check("终局只有一个：两侧 done 且 outbox 已清（不许停在中间态，也不许悄悄弃单）",
      bS.length === 1 && bS[0].status === "done" && aS?.status === "done" && qS === 0,
      "1 行 + 双侧 done + outbox=0",
      `B=${bS.map((r) => r.status).join("/") || "无行"} A=${aS?.status ?? "无行"} outbox=${qS}`);
    const keptS = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(idS)) : [];
    check("补完之后接收目录只剩终名文件：那份半截 .part 不许残留",
      keptS.length === 1 && keptS[0] === `${idS}.bin`, `${idS}.bin`, keptS.join(", ") || "空");
  });
}
