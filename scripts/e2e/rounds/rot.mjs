#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **ROT** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "ROT";
import { DISK_MAX_ATTEMPTS, INSTANCES, LIE, RUN_DIR, S, check, eid, nowMs, openDb, seed, sleep, step, tailLog, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──

let xferId8, srcFile8, srcSha8, term8 = null;

const ROT_BYTES = Number(process.env.E2E_ROT_MB || 1) * 1024 * 1024;

const ROT_PREFIX = 64 * 1024;

export async function run() {
  step("故障注入判据⑧：预置可写 .part 后把接收目录改成只读 ⇒ 半路失败不许被当成完成（rename 才算完成）", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId8 = eid("i");
    const name8 = `${xferId8}.bin`;
    const part8 = path.join(dl, `${xferId8}.part`);
    const final8 = path.join(dl, name8);
    srcFile8 = path.join(srcDir, name8);
    const buf8 = Buffer.alloc(ROT_BYTES);
    for (let i = 0; i < buf8.length; i += 32) buf8.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile8, buf8);
    srcSha8 = createHash("sha256").update(buf8).digest("hex");
    // 注入的形状刻意选成「前缀已经在那儿了，之后的每一步都不许改口」：
    //   1) 预置**真前缀** `.part` ⇒ 接收端走的是 `resume_receive`（播种 hasher、不 truncate），
    //      于是 offer 期不会 EACCES，A 一定把剩下的字节发过来 —— 与⑤（offer 期就写不进）分道。
    //   2) 再把**目录**改成只读 ⇒ 往已存在的文件里写仍然合法（写权限看的是 inode），
    //      但 create / rename / unlink 全部 EACCES ⇒ 唯一会塌下来的动作就是收尾那次 rename。
    //      这正是⑤的注释里点名"要另开一条"的 A-5 形状。
    fs.writeFileSync(part8, buf8.subarray(0, ROT_PREFIX));
    const t0 = nowMs();
    fs.chmodSync(dl, 0o500);
    let bTerminal = null;
    let aGrew = false;
    let partGrew = 0;
    try {
      for (let i = 0; ; i++) {
        try {
          seed(INSTANCES[0].db, (db) => {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId8);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId8, S.peerTo, srcFile8, name8, ROT_BYTES, nowMs());
          });
          break;
        } catch (e) {
          if (i >= 5) throw e;
          await sleep(300);
        }
      }
      const sizeOf = (p) => {
        try {
          return fs.statSync(p).size;
        } catch {
          return -1;
        }
      };
      term8 = null;
      // ⚠️ 「A 侧 outbox 行被删掉」**就是终态**（成功收尾的判据，见 J2 那条
      //    「A 侧 file_outbox 行已被收尾删除」）。第一版我把它当成"还没到终态"继续等 ⇒
      //    跑满 180 s 超时，把"A 已经宣布完成"这件事实读成了"卡住"。行没了必须立刻收，
      //    否则这条判据会把**成功**判成**超时**，而超时恰恰是这条判据最不该混淆的信号。
      let sawRow = false;
      await waitFor(() => {
        const g = sizeOf(part8);
        if (g > ROT_PREFIX) {
          partGrew = g;
          aGrew = true;
        }
        const ad = openDb(INSTANCES[0].db, true);
        const r = ad.prepare("SELECT status,attempts FROM file_outbox WHERE transfer_id=?1").get(xferId8);
        const aT = ad.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId8);
        ad.close();
        const bd = openDb(INSTANCES[1].db, true);
        const bT = bd.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId8);
        bd.close();
        bTerminal = bT?.status ?? null;
        if (r) sawRow = true;
        if (!r && sawRow) term8 = { status: "gone", attempts: 0, aT: aT?.status ?? null };
        else if (r && (r.status === "failed" || r.status === "cancelled" || r.status === "done"))
          term8 = { ...r, aT: aT?.status ?? null };
        return !!term8;
      }, 180_000, "A 侧队列要落到明确终态（行被收尾删除 / failed / cancelled / done 都算，不许静静挂着）");
      console.log(
        `  · 实测：入队 → A 终态 ${((nowMs() - t0) / 1000).toFixed(1)}s ${JSON.stringify(term8)} B=${bTerminal ?? "无行"} .part 峰值=${partGrew}`,
      );
      for (const l of (tailLog(INSTANCES[1].log, 200000) || "").split("\n")
        .filter((x) => x.includes(xferId8) || /rename|重命名|写失败|finalize|终态/i.test(x)).slice(-8)) console.log("      B│ " + l.slice(0, 220));
    } finally {
      fs.chmodSync(dl, 0o700);
    }
    // ── 反空转前提（⑤的教训：先量"我以为已经成立的前提"，再判结论）──
    // 前缀真的被续写 ⇒ 这一轮走的是"收到一半才失败"，而不是⑤那条"offer 期就被拒"。
    // 这条判红不表示产品坏了，表示**注入没落地**，必须分开说，否则后面每一条都是空转。
    check("注入真的落地：.part 必须被续写过（超过预置前缀）", aGrew, `>${ROT_PREFIX}`, partGrew);
    check("A 侧队列必须落到明确终态（行被收尾删除/failed/cancelled/done 都算，停在 pending/sending=界面永远转圈）",
      !!term8, "终态", "180s 内没到终态");
    check("重试不许失控：A 侧 attempts 有界", !!term8 && term8.attempts <= DISK_MAX_ATTEMPTS,
      `≤${DISK_MAX_ATTEMPTS}`, term8 ? term8.attempts : "无终态");
    const existsFinal = fs.existsSync(final8);
    const finalSha = existsFinal ? createHash("sha256").update(fs.readFileSync(final8)).digest("hex") : null;
    const landedWhole = existsFinal && finalSha === srcSha8;
    const claimedDone = !!term8 && (term8.status === "gone" || term8.status === "done" || term8.aT === "done");
    // ★ A-12 修完之后加回来的那条交叉自洽判据（原文照抄 roadmap A-12 那格，一字未改）
    check("发送侧宣布完成（队列行被收尾删除或台账 done）⇒ 接收侧必须有整份且 sha256 相等的 final 文件",
      !claimedDone || landedWhole,
      claimedDone ? "整份 final" : "不声称完成（前件不成立）",
      `A 队列=${term8?.status ?? "无"} / A 台账=${term8?.aT ?? "无"} / final=${existsFinal ? (landedWhole ? "整份" : "内容不符") : "不存在"} / .part=${partGrew}`);
    // 上面那条是**蕴含式**，前件不成立时它自己永远红不了 ⇒ 必须再钉一条"本轮 rename 恒失败 ⇒
    // 发送侧只能落到明确失败"。没有这条，上一条就会退化成 A-9 那次救过我的"永远为真的空转"；
    // lie 轮翻的也正是这一条的期望值。
    const wantA = LIE ? "done" : "failed";
    check("改名永远做不成时，发送侧只能落到明确失败（不许 done，更不许把队列行删掉当收尾）",
      !!term8 && term8.status === wantA, wantA, term8 ? term8.status : "180s 内没到终态");
    // 接收侧台账不许假装成功：B 侧有行时只能停在非 done。
    check("接收侧台账不许假装成功（唯一的失败出口）",
      landedWhole || bTerminal !== "done", "非 done", bTerminal ?? "B 侧无行");
    console.log(`      注：解除只读后 B=${bTerminal ?? "无行"} / .part=${fs.existsSync(part8) ? fs.statSync(part8).size : "已清"} —— 自愈与否只打印，不设判据`);
  });
}
