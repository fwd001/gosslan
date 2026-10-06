#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **RESUME** 轮次分册（一族一轮：preset + run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "RESUME";
import { FILE_BYTES, INSTANCES, LIE, LIE_SHA, RUN_DIR, S, check, eid, nowMs, openDb, seed, sleep, step, tailLog, waitFor, waitSendTerminal } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
let xferId3, srcFile3, srcSha3;

// ── 注入②：--fault=resume-prefix ────────────────────────────────────
// 与"脏前缀"只差一件事：这里预置的 64 KiB 是**源文件自己的开头**。
// 于是接收端报出的已收字节是真值 ⇒ 发送端必须从 65536 续发；hasher 用真前缀播种后，
// 最终 sha256 必须仍然等于源。它钉的是 decide_offer 注释里那次 160MB 真机事故
// （"有活跃接收器时一律 Accept" ⇒ 每轮从 0 重灌 ⇒ 界面恒 0% ⇒ 最后判"分片失败"）：
// **对有效前缀不许重灌**是行为契约，不是性能偏好。
export async function preset() {
  step("预置（注入②）：B 侧已有真前缀 64 KiB + A 侧待发一个 1 MB 文件", () => {
    xferId3 = eid("r");
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile3 = path.join(dir, `${xferId3}.bin`);
    const buf = Buffer.alloc(FILE_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile3, buf);
    srcSha3 = createHash("sha256").update(buf).digest("hex");
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId3);
      db.prepare(
        `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
         VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
      ).run(xferId3, S.peerTo, srcFile3, `${xferId3}.bin`, buf.length, nowMs());
    });
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    fs.writeFileSync(path.join(dl, `${xferId3}.part`), buf.subarray(0, 64 * 1024));
    console.log(`      预置真前缀 65536 字节 / 全文件 ${buf.length} 字节（sha256=${srcSha3.slice(0, 12)}…）`);
  });
}

export async function run() {
  step("故障注入判据②：真前缀必须被续传复用，拼出来的字节要等于源文件", async () => {
    const recvB = path.join(RUN_DIR, "recv", "B");
    const landed3 = path.join(recvB, `${xferId3}.bin`);
    const PRE = 64 * 1024;
    // lie 模式同时换掉"期望已收字节数"和"期望摘要"两个输入 ⇒ 前两条必须报红。
    const wantPre = LIE ? PRE / 2 : PRE;
    const wantSha3 = LIE ? LIE_SHA : srcSha3;
    await waitFor(() => (tailLog(INSTANCES[0].log, 60000) || "").includes(xferId3),
      90_000, `A 侧出现对 ${xferId3} 的处理痕迹（先证明这一单真被投递过，再谈续传）`);
    await sleep(12_000); // 让续传 / rename / 可能的重试都落定
    const aLog = tailLog(INSTANCES[0].log, 60000) || "";
    const line = aLog.split("\n").filter((l) => l.includes(xferId3) && l.includes("接收端已有")).pop() || "";
    const m = line.match(/接收端已有 (\d+) 字节/);
    check("必须按对端已收的 65536 字节续传，不许从 0 重灌整份（160MB 事故那一格）",
      !!m && Number(m[1]) === wantPre, String(wantPre), m ? m[1] : "A 日志里没有针对这一单的续发行");
    const exists3 = fs.existsSync(landed3);
    const got3 = exists3 ? createHash("sha256").update(fs.readFileSync(landed3)).digest("hex") : null;
    check("续传拼出来的文件 sha256 必须等于源文件（前缀 + 尾段字节级正确）",
      exists3 && got3 === wantSha3, wantSha3.slice(0, 12) + "…",
      exists3 ? got3.slice(0, 12) + "…" : "未落地");
    const bDb = openDb(INSTANCES[1].db, true);
    const b3 = bDb.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId3);
    bDb.close();
    // ⚠️ 上面那句 sleep 只是让**日志**落定，不等于 A 的终态到了：下面这条是无条件断"两侧 done"，
    //   所以必须等 A 自己被 ack 点亮（见 waitSendTerminal），否则第 12 秒读早了照样假红。
    const t3 = await waitSendTerminal(xferId3);
    const a3 = t3.row;
    const q3 = t3.queued;
    const both3 = `B=${b3.map((r) => r.status).join("/") || "无行"} A=${a3?.status ?? "无行"} outbox=${q3}`;
    check("同一条续传在 B 侧只记一次（不许一次传输落多行）", b3.length === 1, 1, b3.length);
    check("续传完成就是完成：两侧终态 done 且 outbox 已清（不许停在中间态）",
      b3.length === 1 && b3[0].status === "done" && a3?.status === "done" && q3 === 0,
      "1 行 + 双侧 done + outbox=0", both3);
    const kept3 = fs.existsSync(recvB) ? fs.readdirSync(recvB).filter((f) => f.includes(xferId3)) : [];
    check("前缀用完即弃：接收目录只剩 1 个终名文件，无 .part 残留、无副本",
      kept3.length === 1 && kept3[0] === `${xferId3}.bin`, `${xferId3}.bin`, kept3.join(", ") || "空");
  });
}
