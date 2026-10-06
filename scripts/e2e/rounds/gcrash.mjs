#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **GCRASH** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "GCRASH";
import { GROUP_ID, GROUP_KEY_STR, GROUP_NAME, INSTANCES, ROUND, S, bootBaseOf, check, launch, nowMs, openDb, procs, seed, sleep, step, stopAll, tcpOpen, waitFor } from "../core.mjs";
import { BOOT_LINE, bootReady } from "../../e2e-logtail.mjs";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
const GCRASH_LIE = ROUND === "groupcrash-lie";

/// #121：§28「链路失效」那一族里唯一今天做得成的格子 —— **正在建群时被 SIGKILL**。
///
/// 这一格钉的**不是**"重启后不许留下半个群"：真实建群路径（`commands/groups.rs:7`）把
/// groups / group_members / settings(gk:) / conversations 四张写放在同一个事务里，
/// SQLite 本来就保证要么全有要么全无 ⇒ 那句断言永远绿，是这里反复判过的"半个守卫"。
///
/// 有作用点的那一半是**投递**：`create_group` 提交完之后才逐成员推 `GroupKey`，
/// 而那份"没推出去"的重试登记（`pending_group_keys`）是**进程内**的一张表 ——
/// 一次 SIGKILL 必然把它抹掉。所以这一格真正要钉的是那句设计注释：
/// **群名册才是事实源，链路活着就重递**（`requeue_group_keys_for_peer`，建链 / Hello / 心跳三处）。
/// 它一旦被改回"发出去过就算了"或"只认内存登记"，用户看到的就是"我建的群对方永远不见"，
/// 而 2026-09-24 的真机群不同步正是这个形状。
///
/// ⚠️ 为什么不"预置邀请队列行"：那条队列在内存里，从进程外写不进去（能写的只有 SQLite）。
/// 所以这里用**磁盘能表达的那份等价坏状态** —— 群只长在 A 的盘上、B 一行都不知道，
/// 这正是"命令层已 commit、GroupKey 一次都没送到"的唯一盘上事实；再叠一次真 SIGKILL，
/// 于是"内存登记丢了"这一半是**构造出来的**，不是我假设的。
export async function run() {
  step("建群崩溃判据：群只在 A 的盘上、密钥从没送到时 A 被 SIGKILL ⇒ 重启后 B 必须自己学到这个群", async () => {
    await stopAll();
    const ts = nowMs();
    const convId = `group:${GROUP_ID}`;
    const members = [S.idA.runtimeId, S.idB.runtimeId];
    // 反向模式照 `group-lie` 的先例：**等待用真 id，判据读翻过的 id**。
    // 于是红只能来自"读的不是真落库行"这一件事，不来自基础设施噪声（预置、投递、时序全一样）。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const wantGid = (id) => (GCRASH_LIE ? flip(id) : id);
    /** 一个实例盘上关于某个群的三份事实：群行、成员数、密钥串。 */
    const groupFacts = (file, gid) => {
      const db = openDb(file, true);
      try {
        const g = db.prepare("SELECT id,name,creator FROM groups WHERE id=?1").get(gid) ?? null;
        return {
          group: g,
          memberCount: db.prepare("SELECT COUNT(*) c FROM group_members WHERE group_id=?1").get(gid).c,
          members: db.prepare("SELECT device_id FROM group_members WHERE group_id=?1 ORDER BY device_id")
            .all(gid).map((r) => r.device_id).join(","),
          key: db.prepare("SELECT value FROM settings WHERE key=?1").get(`gk:${gid}`)?.value ?? null,
          conv: db.prepare("SELECT COUNT(*) c FROM conversations WHERE id=?1").get(`group:${gid}`).c,
        };
      } finally { db.close(); }
    };

    // 预置 = 建群命令跑到那一刻的样子：四张写在 A 的盘上，B 侧一字未知。
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
      db.prepare("DELETE FROM groups WHERE id=?1").run(GROUP_ID);
      db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
        .run(GROUP_ID, GROUP_NAME, S.idA.runtimeId, ts);
      for (const m of members) {
        db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)").run(GROUP_ID, m);
      }
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
        .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
      db.prepare(
        "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
        + " VALUES(?1,'group',?2,NULL,0,?3)",
      ).run(convId, GROUP_NAME, ts);
    });

    const bEmpty = groupFacts(INSTANCES[1].db, GROUP_ID);
    check("预置成立：B 侧对这个群零行可知（群此刻只长在 A 的盘上 —— 下面那条「学到了」才有对照物）",
      bEmpty.group === null && bEmpty.memberCount === 0 && bEmpty.key === null && bEmpty.conv === 0,
      "群行 0 / 成员 0 / 无密钥 / 无会话行",
      `群行 ${bEmpty.group ? "1" : "0"} / 成员 ${bEmpty.memberCount} / 密钥 ${bEmpty.key === null ? "无" : "有"} / 会话 ${bEmpty.conv}`);

    // 只起 A：对端整个不存在 ⇒ 密钥一条也送不出去，而"送不出去"的那份登记只在内存里。
    launch(INSTANCES[0]);
    await waitFor(() => tcpOpen(INSTANCES[0].port), 60_000, "建群崩溃判据：A 的 TCP 可连");
    await waitFor(() => bootReady(INSTANCES[0].log, bootBaseOf.get(INSTANCES[0].n), BOOT_LINE), 30_000,
      "建群崩溃判据：A 打出 boot 完成行");
    await sleep(15_000); // 一段"对端完全缺席"的时间（跨好几个心跳周期，不是刚起来没来得及）

    const a1 = groupFacts(INSTANCES[0].db, GROUP_ID);
    check("对端缺席 15s 之后 A 侧名册仍是 2 位 —— 送不出去不许让创建者把自己这边未送达的成员摘掉",
      a1.memberCount === 2, 2, a1.memberCount);
    check("A 侧那份群密钥仍在盘上（它就是重启后重递的原料：丢了就没有任何东西能自收敛）",
      a1.key === GROUP_KEY_STR, "与预置逐字节相同",
      a1.key === null ? "null（行没了）" : `${a1.key.slice(0, 8)}…(${a1.key.length}B)`);

    const pA = procs.get(INSTANCES[0].n);
    // ⚠️ 被信号杀死的子进程 `exitCode === null`、只有 `signalCode` 有值（③⑩ 踩过同一个坑）。
    const deadA = () => pA.exitCode !== null || pA.signalCode !== null;
    pA.kill("SIGKILL");
    await waitFor(deadA, 15_000, "建群崩溃判据：A 确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(10_000); // 一段"A 根本不存在"的时间

    const a2 = groupFacts(INSTANCES[0].db, GROUP_ID);
    check("SIGKILL 之后 A 侧名册仍是 2 位 —— 崩溃不许把群改成半截（这一条判的不是事务，是没有清理路径）",
      a2.memberCount === 2 && a2.group !== null, "2 位成员 + 群行还在",
      `${a2.memberCount} 位 / 群行 ${a2.group ? "在" : "没"}`);

    launch(INSTANCES[0]);
    launch(INSTANCES[1]);
    for (const i of INSTANCES) {
      await waitFor(() => tcpOpen(i.port), 60_000, `建群崩溃判据：重启后实例 ${i.label} 的 TCP 可连`);
      await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 30_000,
        `建群崩溃判据：重启后实例 ${i.label} 打出 boot 完成行`);
    }
    // ★ 有界地**等**它自己收敛，到点就把最后一次读数交给判据判红 —— 不 throw（那会把一次
    //   真缺陷渲染成"基础设施超时"），也不写"兜底断言"（到不了就是死代码）。
    //   等待用的是真 id：这一格要证的是"它到底发没发"，拿翻过的 id 去等只会得到永远超时。
    const t0 = nowMs();
    for (;;) {
      if (groupFacts(INSTANCES[1].db, GROUP_ID).group !== null) break;
      if (nowMs() - t0 >= 120_000) break;
      await sleep(1_000);
    }
    const convergedMs = nowMs() - t0;
    const b = groupFacts(INSTANCES[1].db, wantGid(GROUP_ID));
    check("发送端崩过一次之后，B 仍必须自己学到这个群（120s 内）—— 重递不许依赖那份已被杀掉的内存登记",
      b.group !== null && b.group.creator === S.idA.runtimeId && b.group.name === GROUP_NAME,
      `1 行、creator=A、name=${GROUP_NAME}`,
      b.group === null ? `0 行（等 ${convergedMs}ms 没学到）`
        : `creator=${b.group.creator} name=${b.group.name}`);
    check("B 学到的成员表恰好 2 位且是这两台（重复 requeue 是设计内的，所以 upsert 必须幂等）",
      b.memberCount === 2 && b.members === [S.idA.runtimeId, S.idB.runtimeId].sort().join(","),
      `2 位 = ${[S.idA.runtimeId, S.idB.runtimeId].sort().join(",")}`, `${b.memberCount} 位 = ${b.members}`);
    check("B 手里那把对称密钥要与 A 盘上那份逐字节相同（解不开群消息的就是这一格）",
      b.key === a2.key && b.key === GROUP_KEY_STR, "逐字节等于 A 那份",
      b.key === null ? "null" : `${b.key.slice(0, 8)}…(${b.key.length}B)`);
    // 反向模式里这一条**照例保持绿**（ absence 判据读谁都是 0 行）：它的价值在正向轮 ——
    // 群关系同步一旦又去顺手建会话行，用户清库重装后群聊会凭空回到聊天列表。
    check("B 不许因为这个群凭空多出一行会话（关系同步 ≠ 聊天同步，清库重装后群不该自己冒出来）",
      b.conv === 0, 0, b.conv);
  });
}
