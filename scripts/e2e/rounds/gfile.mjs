#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **GFILE** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "GFILE";
import { GROUP_ID, GROUP_KEY_STR, GROUP_NAME, INSTANCES, LIE_SHA, ROOT, ROUND, RUN_DIR, S, bootBaseOf, check, eid, launch, nowMs, openDb, seed, sleep, step, stopAll, tcpOpen, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { BOOT_LINE, bootReady } from "../../e2e-logtail.mjs";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
const GFILE_LIE = ROUND === "gfile-lie";

// ── 主流程 ─────────────────────────────────────────────────────────
export async function run() {
  step("群文件跨实例判据：A 把货备在盘上、B 上线后必须收到逐字节相同的文件（§七-4「文件 + 群聊」）", async () => {
    await stopAll();
    // 密封 file_key 这一步必须过生产 crypto ⇒ 先确保协议对端的构建产物在位（dev 档，实测十几秒）。
    // 为什么不在 JS 里自己封：nonce 与标签的摆法是协议的一部分，复刻就是拿测试自己的实现去验实现自己。
    const taui = path.join(ROOT, "src-tauri");
    const peerBin = path.join(taui, "target", "debug", "examples", "e2e_peer");
    const built = spawnSync("cargo", ["build", "--example", "e2e_peer"], {
      cwd: taui, encoding: "utf8", timeout: 900_000,
    });
    check("能拿到协议对端的构建产物（会话密钥只由生产 crypto 封装，JS 不复刻线格式）",
      built.status === 0 && fs.existsSync(peerBin),
      "cargo build --example e2e_peer 退 0 且二进制在位",
      `退 ${built.status} · 二进制 ${fs.existsSync(peerBin) ? "在位" : "缺失"}`
      + `${built.status === 0 ? "" : " · " + String(built.stderr || "").slice(-160)}`);
    if (built.status !== 0 || !fs.existsSync(peerBin)) return;

    const ts = nowMs();
    const convId = `group:${GROUP_ID}`;
    const tid = eid("gf");
    const name = `${tid}.bin`;
    // 体积走环境变量（默认 2MB ≈ 8 个分片，够把 Chunk 流走完又不把门禁拖长）
    const bytes = Math.round((Number(process.env.E2E_GFILE_MB) || 2) * 1024 * 1024);
    const srcDir = path.join(RUN_DIR, "src", "gfile");
    fs.mkdirSync(srcDir, { recursive: true });
    const src = path.join(srcDir, name);
    fs.writeFileSync(src, Buffer.alloc(bytes, 0x5a));
    const sha = createHash("sha256").update(fs.readFileSync(src)).digest("hex");
    // 反向轮照 groupcrash 的先例：预置、时序、拓扑一字不动，只把**判据读的那份摘要**换掉
    const wantSha = GFILE_LIE ? LIE_SHA : sha;

    const sealed = (() => {
      const r = spawnSync(peerBin, ["--gfk", GROUP_KEY_STR], { encoding: "utf8", timeout: 60_000 });
      if (r.status !== 0) return null;
      const line = String(r.stdout).split("\n").find((l) => l.startsWith("GFK\t"));
      return line ? (line.split("\t")[2] || null) : null;
    })();
    check("生产 crypto 交出了一份密封好的 file_key（拿不到就没有任何一条后续判据有意义）",
      typeof sealed === "string" && sealed.length > 0,
      "GFK 行第三列非空", sealed ? `有（${sealed.length}B base64）` : "对端非 0 退出或没打出 GFK 行");
    if (!sealed) return;

    const recv = (file, gid, t) => {
      const db = openDb(file, true);
      try {
        // 群文件气泡的**连接键是 msg_id = `gfile-{transfer_id}`**（transport.rs:5166），
        // content 那份 JSON 只有 name/size/sha256/subtype —— 按正文找 tid 会读到 0 行。
        const bubble = db.prepare("SELECT conv_id,kind,content FROM messages WHERE msg_id=?1").get(`gfile-${t}`) ?? null;
        return {
          gf: db.prepare("SELECT transfer_id,sender_id,name,size,sha256,status FROM group_files WHERE transfer_id=?1").get(t) ?? null,
          me: db.prepare("SELECT status FROM group_file_recipients WHERE transfer_id=?1").all(t),
          bubble,
          misplaced: db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1 AND conv_id<>?2")
            .get(`gfile-${t}`, `group:${gid}`).c,
        };
      } finally { db.close(); }
    };
    // 落地文件按本轮 id 找（收尾会改名，所以扫 RUN_DIR 而不是钉一个固定目录名）
    const landed = () => {
      const out = [];
      const walk = (d, depth) => {
        if (depth > 4) return;
        for (const e of fs.readdirSync(d, { withFileTypes: true })) {
          const p = path.join(d, e.name);
          if (e.isDirectory()) walk(p, depth + 1);
          else if (e.name.includes(tid) && !p.startsWith(srcDir)) out.push(p);
        }
      };
      walk(RUN_DIR, 0);
      return out;
    };

    // 停机预置：群在**两端**都存在（接收侧的权限判据要求 sender ∈ 本地群成员，
    // 而那份"群关系同步"本身就是另一轮判的事 —— 这一轮不许偷偷依赖它），
    // 货只在 A 的盘上：B 此刻对这条 transfer 一无所知。
    for (const inst of INSTANCES) {
      seed(inst.db, (db) => {
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        db.prepare("DELETE FROM groups WHERE id=?1").run(GROUP_ID);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, S.idA.runtimeId, ts);
        for (const m of [S.idA.runtimeId, S.idB.runtimeId]) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)").run(GROUP_ID, m);
        }
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)").run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at) VALUES(?1,'group',?2,NULL,0,?3)")
          .run(convId, GROUP_NAME, ts);
      });
    }
    seed(INSTANCES[0].db, (db) => {
      db.prepare("INSERT OR REPLACE INTO group_files(transfer_id,group_id,sender_id,name,size,sha256,status,created_at,scope,todo_id) VALUES(?1,?2,?3,?4,?5,?6,'pending',?7,'chat','')")
        .run(tid, GROUP_ID, S.idA.runtimeId, name, bytes, sha, ts);
      db.prepare("INSERT OR REPLACE INTO group_file_recipients(transfer_id,recipient_id,status,progress,updated_at) VALUES(?1,?2,'pending',0,?3)")
        .run(tid, S.idB.runtimeId, ts);
      db.prepare("INSERT OR REPLACE INTO file_transfers(id,peer_id,name,size,direction,status,path,progress,created_at) VALUES(?1,?2,?3,?4,'send','pending',?5,0,?6)")
        .run(tid, S.idB.runtimeId, name, bytes, src, ts);
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)").run(`gfk:${tid}`, sealed);
    });

    const before = recv(INSTANCES[1].db, GROUP_ID, tid);
    check("预置成立：B 此刻对这条 transfer 零行可知、盘上也没有（下面「收到了」才有对照物）",
      before.gf === null && before.me.length === 0 && landed().length === 0,
      "群文件行 0 / 成员行 0 / 无落地文件",
      `群文件行 ${before.gf ? 1 : 0} / 成员行 ${before.me.length} / 文件 ${landed().length}`);

    for (const i of INSTANCES) {
      launch(i);
      await waitFor(() => tcpOpen(i.port), 60_000, `群文件跨实例判据：实例 ${i.label} 的 TCP 可连`);
      await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 30_000,
        `群文件跨实例判据：实例 ${i.label} 打出 boot 完成行`);
    }

    const t0 = nowMs();
    let snap = null;
    // 有界地等 B 自己把台账推到终态；到点把**最后一次读数**交给判据判红（不 throw，
    // 那会把一次真缺陷渲染成"基础设施超时"）
    await waitFor(() => {
      const r = recv(INSTANCES[1].db, GROUP_ID, tid);
      const files = landed();
      if (r.me.some((x) => x.status === "completed") && files.length >= 1) {
        snap = { ...r, files, ms: nowMs() - t0 };
      }
      return !!snap;
    }, 180_000, "群文件跨实例判据：B 侧这条 transfer 要落到 completed 并且文件真在盘上");
    if (!snap) {
      const r = recv(INSTANCES[1].db, GROUP_ID, tid);
      snap = { ...r, files: landed(), ms: nowMs() - t0 };
    }
    check("B 上线后这条群文件必须自己投出去：接收侧成员行落到 completed（180s 内）",
      snap.me.some((x) => x.status === "completed"),
      "至少一行 completed",
      `等 ${snap.ms}ms · ${JSON.stringify(snap.me.map((x) => x.status))}`);
    check("B 收到的那条群文件元数据必须与源逐字段相同（名字 / 字节数 / 摘要）",
      snap.gf !== null && snap.gf.name === name && snap.gf.size === bytes && snap.gf.sha256 === wantSha,
      `name=${name.slice(-6)} size=${bytes} sha=${wantSha.slice(0, 8)}…`,
      snap.gf === null ? "0 行" : `name=${snap.gf.name} size=${snap.gf.size} sha=${snap.gf.sha256.slice(0, 8)}…`);
    const gotShas = snap.files.map((f) => createHash("sha256").update(fs.readFileSync(f)).digest("hex"));
    check("「台账说收到了」不等于「收下了」：B 盘上那份内容的 sha256 必须等于源",
      gotShas.length >= 1 && gotShas.some((h) => h === wantSha),
      wantSha.slice(0, 12) + "…", gotShas.map((h) => h.slice(0, 12)).join(",") || "无文件");
    check("这条群文件的气泡只落群会话（连接键 msg_id=gfile-{tid}；串味就是 §五 那句「消息层与文件层互相隔离」破了）",
      snap.bubble !== null && snap.bubble.conv_id === convId && snap.misplaced === 0,
      `1 行、conv_id=${convId}、错位 0`,
      snap.bubble === null ? "0 行（气泡根本没落库）"
        : `conv=${snap.bubble.conv_id} kind=${snap.bubble.kind} 错位 ${snap.misplaced}`);
    check("不许留 .part 半成品（收尾改名之前必须已经落全）",
      snap.files.every((f) => !f.endsWith(".part")), "无 .part",
      snap.files.map((f) => path.basename(f)).join(", ") || "无");
    // 发送侧这一行是由**对端那一帧成功 ACK**点亮的（transport.rs 的 ACK 处理：收到
    // success=true 才 update_group_file_recipient(..., "completed", 1.0)）—— 它与"对面盘上
    // 收下了"是**两件事**，且必然更晚。原来这里在观察到 B 的 completed 之后**单点读一次**，
    // 读到什么全看那一帧回没回来 ⇒ 同一条判据可以一次绿一次红（本仓记过的形状：
    // 「某一瞬间没发生」不是判据；终局判据要等发送侧自己被 ack 点亮）。
    // 判据强度一字未改：仍然要求"必须到终态"，只是给它一个有界窗口；到点仍不亮就照旧报红，
    // 那时报的才是"ACK 这条路真断了"，而不是"探针放错了窗口"。
    let aRows = [];
    const aDeadline = nowMs() + 60_000;
    for (;;) {
      const aDb = openDb(INSTANCES[0].db, true);
      try {
        aRows = aDb
          .prepare("SELECT recipient_id,status,progress FROM group_file_recipients WHERE transfer_id=?1")
          .all(tid);
      } finally {
        aDb.close();
      }
      if (aRows.length === 1 && !["pending", "sending"].includes(aRows[0].status)) break;
      if (nowMs() >= aDeadline) break;
      await sleep(1_000);
    }
    check("发送侧那一行不许停在 pending/sending（#154-4 那个洞就是这个形状：补发失败只 log 不写终态 ⇒ 永远在发、重启也不重试）",
      aRows.length === 1 && !["pending", "sending"].includes(aRows[0].status),
      "1 行、状态不是 pending/sending", JSON.stringify(aRows));
    const gkRow = (() => {
      const db = openDb(INSTANCES[0].db, true);
      try {
        return db.prepare("SELECT value FROM settings WHERE key=?1").get(`gfk:${tid}`) ?? null;
      } finally { db.close(); }
    })();
    check("A 侧那份密封密钥必须留在 settings 里（它是重启后 ensure_group_file_key 唯一的原料）",
      gkRow !== null && gkRow.value === sealed, "gfk:{tid} 行还在、内容与交出去那份一致",
      gkRow === null ? "读不到这一行" : `在位（${String(gkRow.value).length}B）`);
  });
}
