#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **DISK** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "DISK";
import { DISK_MAX_ATTEMPTS, INSTANCES, LIE, RUN_DIR, S, check, eid, nowMs, openDb, seed, sleep, step, tailLog, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

let xferId6, term6 = null;

const DISK_BYTES = Number(process.env.E2E_DISK_MB || 1) * 1024 * 1024;

export async function run() {
  step("故障注入判据⑤：接收目录写不进去 ⇒ 必须明确失败并止步，不许假 done、不许无限重试", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId6 = eid("g");
    const srcFile6 = path.join(srcDir, `${xferId6}.bin`);
    fs.writeFileSync(srcFile6, Buffer.alloc(DISK_BYTES));
    const t0 = nowMs();
    // 注入：接收目录整个改成只读 —— 建 `.part` 与最终 rename 都需要目录写权限。
    fs.chmodSync(dl, 0o500);
    try {
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId6);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId6, S.peerTo, srcFile6, `${xferId6}.bin`, DISK_BYTES, nowMs());
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      term6 = null;
      await waitFor(() => {
        const db = openDb(INSTANCES[0].db, true);
        const r = db
          .prepare("SELECT status,attempts FROM file_outbox WHERE transfer_id=?1")
          .get(xferId6);
        db.close();
        // 终态 = 队列行落到 failed/cancelled。
        // ⚠️ 不要把 `sending` 当终态：它是"正在投递"的中间态（mark_file_outbox_sending 顺手 +1 attempts），
        //    第一版就是这么判的，结果第 4 次尝试的 33.2 s 处抓到 `{status:'sending',attempts:4}` 判红。
        //    （停在 sending 会不会永久卡住？不会 —— AppState 初始化有 reset_sending_to_pending，
        //    file_offline.rs:135-146 的注释正是为这件事写的。）
        if (r && (r.status === "failed" || r.status === "cancelled")) term6 = r;
        return !!term6;
      }, 180_000, "A 侧这一单要在重试上限内落到明确终态（不许静静挂着）");
      console.log(
        `  · 实测：入队 → 明确终态 ${((nowMs() - t0) / 1000).toFixed(1)}s ${JSON.stringify(term6)}`,
      );
      for (const l of (tailLog(INSTANCES[0].log, 60000) || "").split("\n")
        .filter((x) => x.includes(xferId6)).slice(-6)) console.log("      A│ " + l.slice(0, 220));
      for (const l of (tailLog(INSTANCES[1].log, 60000) || "").split("\n")
        .filter((x) => x.includes("初始化失败")).slice(-3)) console.log("      B│ " + l.slice(0, 220));
    } finally {
      // ⚠️ 必须还原：否则这一轮的接收目录连同后续清理都带着只读位，
      //    而且下一个模式会被这条注入的残留状态污染。
      fs.chmodSync(dl, 0o700);
    }
    const seen = fs.readdirSync(dl).filter((f) => f.includes(xferId6));
    // 反空转前提（冻结轮的教训：先量"我以为已经成立的前提"）：
    // B 的日志必须真说过"初始化失败" ⇒ 注入确实生效、A 确实试过，而不是"这一单没跑"带来的假绿。
    const bLog = tailLog(INSTANCES[1].log, 200000) || "";
    check("注入真的生效：接收端日志必须出现「接收文件初始化失败」",
      bLog.includes("接收文件初始化失败"), "≥1 次", (bLog.match(/接收文件初始化失败/g) || []).length);
    check("重试真的发生过（与冻结轮的分水岭：这里对端活着、有入站帧）",
      !!term6 && term6.attempts >= 2, "≥2", term6 ? term6.attempts : "无终态");
    check("重试不许失控：次数不得超过 MAX_FILE_OUTBOX_RETRIES",
      !!term6 && term6.attempts <= DISK_MAX_ATTEMPTS, `≤${DISK_MAX_ATTEMPTS}`,
      term6 ? term6.attempts : "无终态");
    // lie 模式：注入完全一样，只把"该落到哪个终态"换成 done ⇒ 这一条必须红。
    const wantTerm = LIE ? "done" : "failed";
    check("接收端写不进去时，发送侧必须落到明确终态（不许停在 pending/active，也不许假 done）",
      !!term6 && term6.status === wantTerm, wantTerm, term6 ? term6.status : "始终没到终态");
    const aDb = openDb(INSTANCES[0].db, true);
    const a6 = aDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId6);
    aDb.close();
    check("发送侧台账不许是 done（唯一出口的判定）",
      a6?.status !== "done", "非 done", a6?.status ?? "无行");
    check("写不进去就不许在接收目录留下这个 transfer 的任何东西",
      seen.length === 0, "无文件", seen.join(", ") || "无");
  });
}
