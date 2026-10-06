#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **POISON** 轮次分册（一族一轮：preset + run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "POISON";
import { FILE_BYTES, INSTANCES, LIE, LIE_SHA, RUN_DIR, S, check, eid, nowMs, openDb, seed, step, tailLog, waitFor, waitSendTerminal } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
let xferId2, srcFile2, srcSha2;

// ── 故障注入（总指令§八 / roadmap A-2）：--fault=poison-part ─────────
// 为什么选「给接收端预置一段脏 .part」，而不是「改 A 库里的 sha256」：
//   发送侧的 hash 是 `sha256_file_hex()` **发送时从磁盘现算**的（network/file.rs:375/665），
//   DB 里那一份改了就等于没改 —— 那条注入只会红得莫名其妙（我差点就写成那样）。
//   而接收侧 `resume_receive` 明写「不再 truncate，用已有前缀播种 hasher」（file.rs:1317-1388），
//   所以一个**严格短于文件**的脏 .part = 确定性地让最终 sha256 不匹配，零生产码改动、无竞态。
export async function preset() {
  step("故障注入：A 再排一个 1 MB 文件，同时给 B 预置 4 KB 脏 .part 前缀", () => {
    xferId2 = eid("p");
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile2 = path.join(dir, `${xferId2}.bin`);
    const buf = Buffer.alloc(FILE_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile2, buf);
    srcSha2 = createHash("sha256").update(buf).digest("hex");
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId2);
      db.prepare(
        `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
         VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
      ).run(xferId2, S.peerTo, srcFile2, `${xferId2}.bin`, buf.length, nowMs());
    });
    // 脏前缀必须严格短于文件：等长或更长会让接收端回 received >= size，那走的是
    // AlreadyHave 分支（合法地宣布"我早收完了"），就不是在测 hash 拒收这一格。
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const junk = Buffer.alloc(4096);
    for (let i = 0; i < junk.length; i += 32) junk.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(path.join(dl, `${xferId2}.part`), junk);
    console.log(`      预置脏 .part = ${junk.length} 字节 / 真实文件 = ${buf.length} 字节`);
  });
}

export async function run() {
  step("故障注入判据：脏 .part 前缀不许污染结局（拒收 或 补齐，二选一，不许交叉）", async () => {
    const recvB = path.join(RUN_DIR, "recv", "B");
    const landed2 = path.join(recvB, `${xferId2}.bin`);
    // 前提断言：B 真的动过这一单。没有它，下面几条会因为"链路根本没跑"而集体假绿 ——
    // 那正是最像成功的一种失败。
    let how = "";
    await waitFor(() => {
      const b = openDb(INSTANCES[1].db, true);
      const r = b.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId2);
      b.close();
      if (r) { how = `B 侧有行 status=${r.status}`; return true; }
      if ((tailLog(INSTANCES[1].log, 20000) || "").includes(xferId2)) { how = "B 日志提到过这个 transfer_id"; return true; }
      return false;
    }, 90_000, `B 侧出现对 ${xferId2} 的处理痕迹（先证明这一单真被投递过，再谈拒收）`);
    console.log(`      观察：${how}`);
    await new Promise((r) => setTimeout(r, 10_000)); // 让 hash 校验 / rename / 重试落定

    const exists = fs.existsSync(landed2);
    const gotSha = exists
      ? createHash("sha256").update(fs.readFileSync(landed2)).digest("hex")
      : null;
    // 脏前缀被丢掉、整份重新收齐并改名 ⇒ 这是"自愈完成"，此时 done 才是**正确**终态。
    const wantSha2 = LIE ? LIE_SHA : srcSha2;
    const clean = !!exists && gotSha === wantSha2;
    // ⚠️ 这一轮的"两侧 done"是**蕴含式**的后件 ⇒ 只在补齐分支才需要等 A 到终态
    //   （不补齐那一支本来就允许停在 pending/重试中，等满 60s 只会白烧门禁时间）。
    //   lie 模式下 clean 必为假 ⇒ 走的还是今天这条不等待的路，反向自证那 2 条红不受影响。
    if (clean) await waitSendTerminal(xferId2);

    const bDb = openDb(INSTANCES[1].db, true);
    const b2 = bDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId2);
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const a2 = aDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId2);
    const queued = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId2).c;
    aDb.close();
    const strays2 = fs.existsSync(recvB)
      ? fs.readdirSync(recvB).filter((f) => f.includes(xferId2) && f !== `${xferId2}.bin`)
      : [];
    const both = `B=${b2?.status ?? "无行"} A=${a2?.status ?? "无行"} outbox=${queued}`;

    // 四条合起来 = "结局只允许两种，且不许交叉"：
    //   A) 补齐了 ⇒ 终名 sha256 == 源 + 两侧 done + outbox 已清（自愈完成）
    //   B) 没补齐 ⇒ 没有正确终名 + 两侧都不许 done（明确没成功、还能重试）
    // 交叉态才是真 bug：done 却没有正确文件 = 假成功；文件已正确落地却仍 failed = 恢复失败、界面永久转圈。
    // ⚠️ 上一版这里写的是"必须非 done"——被实跑证伪了：它写的是"我以为失败长什么样"，不是产品契约。
    check("坏内容不许冒充成功：终名要么不出现，出现则 sha256 必须等于源文件",
      !exists || clean,
      "不出现 或 " + wantSha2.slice(0, 12) + "…",
      exists ? gotSha.slice(0, 12) + "…" : "未出现");
    check("若脏前缀最终被补齐（字节正确）：两侧必须 done 且 outbox 已清 —— 不许停在中间态",
      !clean || (b2?.status === "done" && a2?.status === "done" && queued === 0),
      clean ? "双侧 done + outbox=0" : "不适用（未落地）", both);
    check("若终名未落地或字节不对：两侧都不许 done —— 绝不许对坏内容宣布完成",
      clean || (b2?.status !== "done" && a2?.status !== "done"),
      clean ? "不适用（本轮走补齐分支）" : "双侧非 done", both);
    check("污染过的前缀不许留在盘上：接收目录不得残留该 transfer 的 .part / 副本",
      strays2.length === 0, 0, strays2.join(", ") || 0);
  });
}
