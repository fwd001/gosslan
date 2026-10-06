#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **FREEZE** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "FREEZE";
import { INSTANCES, LIE, LIE_SHA, RUN_DIR, S, check, eid, nowMs, openDb, procs, seed, sleep, step, tailLog, waitFor, waitSendTerminal } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

let xferId5, srcFile5, srcSha5;

const FREEZE_BYTES = Number(process.env.E2E_FREEZE_MB || 1) * 1024 * 1024;

/// 调结长度。**<45s**：链路还活着，只是对端不回话；
/// **>45s**：越过 watchdog ⇒ 拆链 + 重拨（解冻后由重拨/心跳重新触发 flush）。
/// 两种 regime 用同一组"结局空间"判据，不需要分叉。
const FREEZE_MS = Number(process.env.E2E_FREEZE_S || 30) * 1000;

export async function run() {
  step("故障注入判据④：对端失联（进程被冻住）⇒ 失联期间不许假成功，对端回来必须自己补齐", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId5 = eid("f");
    srcFile5 = path.join(srcDir, `${xferId5}.bin`);
    fs.writeFileSync(srcFile5, Buffer.alloc(FREEZE_BYTES));
    srcSha5 = createHash("sha256").update(fs.readFileSync(srcFile5)).digest("hex");
    const landed = path.join(dl, `${xferId5}.bin`);
    const partPath = path.join(dl, `${xferId5}.part`);
    const sizeOf = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    // lie 模式：注入一模一样，只把"终局该等于哪个摘要"换掉 ⇒ 摘要那条必须红。
    const wantSha5 = LIE ? LIE_SHA : srcSha5;
    const pB = procs.get(INSTANCES[1].n);
    if (!pB) throw new Error("拿不到 B 的子进程句柄 —— 这条注入没有可冻结的对象");
    let thawed = false;
    // ⚠️ 解冻必须放进 finally：**被 SIGSTOP 停住的进程收不到 SIGTERM 的处理**（信号挂起），
    //    收尾的 stopAll() 会永久等一个不会来的 exit ⇒ 整条 harness 挂死、留一个僵尸。
    pB.kill("SIGSTOP");
    let mid = null;
    try {
      // 入队在冻结之后：与 kill 轮同一个教训 —— 注入时机必须在判据自己手里。
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId5);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId5, S.peerTo, srcFile5, `${xferId5}.bin`, FREEZE_BYTES, nowMs());
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      await sleep(FREEZE_MS);
      const aMid = openDb(INSTANCES[0].db, true);
      const st = aMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId5);
      const q = aMid.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId5).c;
      aMid.close();
      const midLanded = sizeOf(landed);
      const bMid = openDb(INSTANCES[1].db, true);
      const b5mid = bMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId5);
      bMid.close();
      mid = {
        status: st?.status ?? "无行", queued: q, b: b5mid?.status ?? "无行",
        part: sizeOf(partPath), landed: midLanded,
      };
      // ⚠️ 实测到的**产品现状**（30s 与 60s 各跑一遍，结论相同；写在这里是防止下一个 AI 把这一轮
      //   当成它看起来像在测的东西）：失联期间 A 侧 `attempts` 一次没涨、`file_transfers` 连行都没有
      //   ⇒ **A 根本没有尝试过**。根因：`flush_pending_files` 只被建链 / Hello / 心跳 / BLE 这类
      //   **入站事件**触发（transport.rs:2580/2902/3015、ble.rs:1102/2110），没有任何定时器去兑现
      //   `file_outbox.next_attempt_at` 与那个 5s backoff；对端"活着但一句不回"时不会有入站事件。
      //   ⇒ 这一轮证明的是：失联期间两侧都不许假成功 + 对端回来自己补齐。
      //   它**没有证明**"write 成功 ≠ 已送达"（那需要一个真在飞的写），也**没覆盖**"失联期间到点重投"。
      //   后者已按 A 类风险登记在 roadmap；修好之前不许把下面三条改名成"已覆盖重试"。
      check("失联期间接收目录不许出现终名文件（没收下就没有完成可言）",
        midLanded < 0, "不存在", midLanded >= 0 ? `已出现 ${midLanded} 字节` : "不存在");
      check("失联期间发送侧不许记成 done", mid.status !== "done", "非 done", mid.status);
      check("失联期间接收侧也不许记成 done（它一次回执都没发过）",
        mid.b !== "done", "非 done", mid.b);
      console.log(`  · 实测（失联 ${FREEZE_MS / 1000}s）：${JSON.stringify(mid)}`);
      for (const l of (tailLog(INSTANCES[0].log, 60000) || "").split("\n")
        .filter((x) => x.includes(xferId5)).slice(-8)) console.log("      A│ " + l.slice(0, 220));
      pB.kill("SIGCONT");
      thawed = true;
      const t0 = nowMs();
      await waitFor(() => sizeOf(landed) >= 0, 120_000, "解冻后 B 该把这一单收完并 rename 成终名");
      console.log(`  · 实测：解冻 → 终名落地 ${(nowMs() - t0) / 1000}s`);
    } finally {
      if (!thawed) { try { pB.kill("SIGCONT"); } catch { /* 已经退了 */ } }
    }
    const got = sizeOf(landed) >= 0
      ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    check("补齐之后的字节内容必须等于源文件（终局不许是坏内容）",
      got === wantSha5, wantSha5.slice(0, 12) + "…", got ? got.slice(0, 12) + "…" : "未落地");
    const bDb = openDb(INSTANCES[1].db, true);
    const b5 = bDb.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId5);
    bDb.close();
    // ⚠️ 同 J2 /  kill 轮：解冻→落地 0.5s 太快，读 A 必须等它自己被 ack 点亮，不能拿 B 的 rename 当同步点。
    const t5 = await waitSendTerminal(xferId5);
    const a5 = t5.row;
    const q5 = t5.queued;
    const both5 = `B=${b5.map((r) => r.status).join("/") || "无行"} A=${a5?.status ?? "无行"} outbox=${q5}`;
    check("对端解冻后必须自己补到终态：两侧 done 且 outbox 已清（不许停在中间态、不许弃单）",
      b5.length === 1 && b5[0].status === "done" && a5?.status === "done" && q5 === 0,
      "1 行 + 双侧 done + outbox=0", both5);
    const kept5 = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(xferId5)) : [];
    check("解冻之后不许留下第二次成功的痕迹",
      kept5.length === 1 && kept5[0] === `${xferId5}.bin`, `${xferId5}.bin`, kept5.join(", ") || "空");
  });
}
