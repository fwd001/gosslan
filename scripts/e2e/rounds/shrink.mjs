#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **SHRINK** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "SHRINK";
import { INSTANCES, LIE, LIE_SHA, RUN_DIR, S, check, eid, nowMs, openDb, procs, seed, sleep, step, tailLog, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

let xferId7, srcFile7;

const SHRINK_BYTES = Number(process.env.E2E_SHRINK_MB || 1) * 1024 * 1024;

const SHRINK_TO = Number(process.env.E2E_SHRINK_TO_BYTES || 4096);

export async function run() {
  step("故障注入判据⑥：入队后源文件被改小 ⇒ 只许按磁盘上那份真值收发，两侧终态一致", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId7 = eid("h");
    const name7 = `${xferId7}.bin`;
    srcFile7 = path.join(srcDir, name7);
    fs.writeFileSync(srcFile7, Buffer.alloc(SHRINK_BYTES));
    const landed = path.join(dl, name7);
    const sizeOf = (p) => { try { return fs.statSync(p).size; } catch { return -1; } };
    const pB = procs.get(INSTANCES[1].n);
    if (!pB) throw new Error("拿不到 B 的子进程句柄 —— 这一轮的窗口要靠冻结 B 来保证");
    let thawed = false;
    let sent = null;
    try {
      // 顺序不能换：先冻住 B，A 才有"不读盘"的确定窗口（A 只在收到入站帧时才 flush，见 A-9）。
      pB.kill("SIGSTOP");
      // 入队 = 复刻 `send_file` 命令在点击那一刻写的三行（气泡 / 传输台账 / 队列）。
      // 少写一行就测不到这一格的疑点：气泡与台账的 size 都是**按当时磁盘**算出来的。
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId7);
            db.prepare("DELETE FROM file_transfers WHERE id=?1").run(xferId7);
            db.prepare("DELETE FROM messages WHERE msg_id=?1").run(`file-${xferId7}`);
            const ts = nowMs();
            const seq = db.prepare("SELECT COALESCE(MAX(seq),0)+1 s FROM messages WHERE conv_id=?1")
              .get(S.peerTo).s;
            db.prepare(
              `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
               VALUES(?1,?2,?3,?4,'file',?5,?6,?7,'sent')`,
            ).run(`file-${xferId7}`, S.peerTo, S.idA.runtimeId, S.peerTo,
              JSON.stringify({ name: name7, path: srcFile7, size: SHRINK_BYTES, sha256: "", subtype: "file" }),
              ts, seq);
            db.prepare(
              `INSERT INTO file_transfers(id,peer_id,name,size,direction,status,path,progress,created_at)
               VALUES(?1,?2,?3,?4,'send','pending',?5,0,?6)`,
            ).run(xferId7, S.peerTo, name7, SHRINK_BYTES, srcFile7, ts);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId7, S.peerTo, srcFile7, name7, SHRINK_BYTES, ts);
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      // 注入：入队之后把原件改小（用户在同一批"等着对方上线"的单子还没发出去时改了那个文件）。
      fs.truncateSync(srcFile7, SHRINK_TO);
      const shrunkSha = createHash("sha256").update(fs.readFileSync(srcFile7)).digest("hex");
      pB.kill("SIGCONT");
      thawed = true;
      const t0 = nowMs();
      await waitFor(() => {
        const aDb = openDb(INSTANCES[0].db, true);
        const a = aDb.prepare("SELECT status,size FROM file_transfers WHERE id=?1").get(xferId7);
        const q = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId7).c;
        const bubble = aDb.prepare("SELECT content FROM messages WHERE msg_id=?1").get(`file-${xferId7}`);
        aDb.close();
        const bDb = openDb(INSTANCES[1].db, true);
        const b = bDb.prepare("SELECT status,size FROM file_transfers WHERE id=?1").get(xferId7);
        bDb.close();
        // 收完的判据用"队列行已关 + 两侧 done"，不用 landed 存在 —— rename 之后 A 还要等回执才落 done。
        if (a && b && a.status === "done" && b.status === "done" && q === 0) {
          sent = {
            a, q, b, landed: sizeOf(landed),
            bubbleSize: bubble ? JSON.parse(bubble.content).size : null,
            bubbleSha: bubble ? JSON.parse(bubble.content).sha256 : null,
            shrunkSha,
          };
        }
        return !!sent;
      }, 120_000, "改小的原件要按新 size 走完 offer→chunk→rename→done");
      console.log(`  · 实测：解冻 → 两侧 done ${(nowMs() - t0) / 1000}s ${JSON.stringify(sent)}`);
      for (const l of (tailLog(INSTANCES[0].log, 60000) || "").split("\n")
        .filter((x) => x.includes(xferId7)).slice(-6)) console.log("      A│ " + l.slice(0, 220));
    } finally {
      // 被 SIGSTOP 停住的进程收不到 SIGTERM ⇒ 不解冻会把整条 harness 挂死（冻结轮的教训）。
      if (!thawed) { try { pB.kill("SIGCONT"); } catch { /* 已经退了 */ } }
    }
    const landedBytes = sizeOf(landed);
    const got = landedBytes >= 0
      ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    check("落地字节数必须等于截断后的磁盘大小（说明这一单按真值重算，不是按入队那份）",
      sent.landed === SHRINK_TO && landedBytes === SHRINK_TO, SHRINK_TO,
      `offer时队列=${sent?.landed} 盘上=${landedBytes}`);
    check("落地内容必须等于截断后的源文件（不许把半截当完成，也不许多给旧字节）",
      got === (LIE ? LIE_SHA : sent.shrunkSha), (LIE ? LIE_SHA : sent.shrunkSha).slice(0, 12) + "…",
      got ? got.slice(0, 12) + "…" : "未落地");
    check("两侧台账必须同时 done 且队列行已关（跨设备终态不许分叉）",
      sent.a.status === "done" && sent.b.status === "done" && sent.q === 0,
      "A=done B=done outbox=0", `A=${sent.a.status} B=${sent.b.status} outbox=${sent.q}`);
    check("接收目录只许有终名那一个文件（无 .part 残留、无第二份）",
      fs.readdirSync(dl).filter((f) => f.includes(xferId7)).join(",") === name7, name7,
      fs.readdirSync(dl).filter((f) => f.includes(xferId7)).join(", ") || "空");
    check("接收端台账的 size 必须等于盘上真实字节数（接收端说真话）",
      sent.b.size === landedBytes, landedBytes, sent.b.size);
    check("发送端气泡回填的 sha256 必须等于落地文件摘要（内容寻址 cid 不许撒谎）",
      sent.bubbleSha === got, got?.slice(0, 12) + "…", sent.bubbleSha?.slice(0, 12) ?? "空");
    // ⚠️ 这一行**不是断言**，是这一轮照出来的**分歧证据**（已按 A-11 登记 roadmap）：
    //   气泡与发送台账的 size 来自点击那一刻的磁盘，`upsert_transfer` 的 ON CONFLICT 只改
    //   status/path/progress、不改 size ⇒ 原件事后变小时，A 自己看到的"多大"和真正发出去、
    //   B 收到的那份就不是一个数。修（回填 size）之前不许把它写成断言，也不许删这条打印。
    console.log(
      `  · 实测 size 分歧：入队 ${SHRINK_BYTES} → 实发 ${landedBytes}` +
      ` | A 气泡 ${sent.bubbleSize} · A 台账 ${sent.a.size} · B 台账 ${sent.b.size}`,
    );
  });
}
