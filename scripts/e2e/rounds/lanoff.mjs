#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **LANOFF** 轮次分册（一族一轮：run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "LANOFF";
import { INSTANCES, ROUND, S, bootBaseOf, check, launch, nowMs, procs, seed, sleep, step, stopOne, tcpOpen } from "../core.mjs";
import { BOOT_LINE, bootReady, countLog } from "../../e2e-logtail.mjs";
import { spawnSync } from "node:child_process";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
const LANOFF_LIE = ROUND === "lanoff-lie";

// ── #89：局域网开关的跨实例隔离轮 ───────────────────────────────────
// 三条腿：开着先学到 → 关掉之后学不到（核心）→ 翻回来又学得到（正向对照，证明中间那条不是空转）。
// 观察者 A **全程不重启**：如果 A 也被停掉，"计数不涨"就变成"没人再看"的同义反复（那条假判据的形状）。
export async function run() {
  step("局域网开关：关掉之后对端学不到我，翻回来必须重新学得到（env 不许替用户表态）", async () => {
    /// 不是抛错的 waitFor：这一轮的"等不到"必须是**断言红**，不是步骤崩。
    const within = async (fn, ms) => {
      const until = nowMs() + ms;
      for (;;) {
        if (fn()) return true;
        if (nowMs() >= until) return false;
        await sleep(1000);
      }
    };
    const B = INSTANCES[1];
    const learnedNeedle = (id) => `announce_verified: from=${id}`;
    const seedLan = (on) => seed(B.db, (db) => {
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('lan_enabled',?1)")
        .run(on ? "1" : "0");
    });
    const bootOf = async (inst) => {
      await within(() => bootReady(inst.log, bootBaseOf.get(inst.n), BOOT_LINE), 30_000);
    };

    // 上一轮（默认旅程）收尾时 A/B 已被 stopAll 停干净 ⇒ 这里自己起。
    launch(INSTANCES[0]);
    launch(B);
    await bootOf(INSTANCES[0]);
    await bootOf(B);
    // ① 前置：两边都开着（L-A 预置显式写了 lan_enabled='true'）时，A 要真学到过 B。
    const sawFirst = await within(() => countLog(INSTANCES[0].log, learnedNeedle(S.idB.runtimeId)) > 0, 45_000);
    check("前置：B 开着的时候 A 的日志里出现过它的 announce（否则下面那条「不涨」没有对照物）",
      sawFirst, ">0 次", countLog(INSTANCES[0].log, learnedNeedle(S.idB.runtimeId)));

    // ② 停机把 B 的键显式写成"关"，再带着 GOSSLAN_AUTOSTART=1 起它 —— 这一轮要的就是这一对。
    await stopOne(B);
    seedLan(false);
    launch(B);
    await bootOf(B);
    check("关掉后 B 自己没起网：它的日志里 discovery_started 计数为 0",
      countLog(B.log, "discovery_started") === 0, 0, countLog(B.log, "discovery_started"));

    // 关着的那段窗口里先把监听口的读数取走（**只打印**，理由见文件末尾那三行对照的注释）
    const listenerPids = () => {
      const r = spawnSync("lsof", ["-nP", "-tiTCP:" + B.port, "-sTCP:LISTEN"], { encoding: "utf8" });
      return (r.stdout || "").trim().split("\n").filter(Boolean);
    };
    const psOf = (pid) => pid
      ? (spawnSync("ps", ["-o", "pid=,comm=,lstart=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim()
         || "(ps 查不到这个 pid)")
      : "(没有 pid)";
    const openWhileOff = await tcpOpen(B.port);
    const whoHolds = listenerPids();
    check("★ 关掉之后连 TCP 监听口都不存在（别人拨不进来，不只是「我不再广播」）",
      openWhileOff === false, "连不上（false）",
      `连得上=${openWhileOff}；监听者=[${whoHolds.map(psOf).join(" | ")}]；`
      + `本轮 launch 起的 B 的 pid=${procs.get(B.n)?.pid}
       ⚠️ 这条 2026-09-27 凌晨先被判成"红"过一次，原因是**探针放错窗口**：那一刻量的是
       键翻回「开」之后重起的那个 B（日志尾巴上还留着它的 bc_directed 与 routed 建链），
       于是三个读数互相矛盾。挪进关着的那段窗口之后：监听者列表是空的。
       ⇒ 教训是「归因不清」和「探针读错了对象」长得一模一样，区别只在有没有把读数钉在事件上。`);


    // ③ 核心：A 活着且一直在听，两个广播周期内"学到 B"的次数一字不涨。
    const base = countLog(INSTANCES[0].log, learnedNeedle(S.idB.runtimeId));
    await sleep(26_000);
    const after = countLog(INSTANCES[0].log, learnedNeedle(S.idB.runtimeId));
    // 反向模式：注入、时序、读的东西全都一样，**只把这一条的期望翻成"该涨"**
    // ⇒ 产品没错时它必须红；报不出红就说明这条读的不是真日志行。
    check("★ 关掉之后 A 再也学不到 B（announce 计数一字不涨；env 没能把它偷偷打开）",
      LANOFF_LIE ? after > base : after === base,
      LANOFF_LIE ? "> 基线（这是反向模式的期望，正常应当红）" : base, after);

    // ④ 正向对照：翻回"开"并重启 B ⇒ 同一套读法必须立刻重新数得到 announce。
    //    没有这一条，第 ③ 条可以由"A 瞎了/日志格式变了/needle 拼错"来冒充成功。
    await stopOne(B);
    seedLan(true);
    launch(B);
    await bootOf(B);
    const grewBack = await within(
      () => countLog(INSTANCES[0].log, learnedNeedle(S.idB.runtimeId)) > after, 45_000);
    check("对照：把键翻回「开」并重启 B，A 的 announce 计数重新开始涨（证明第 ③ 条不是空转）",
      grewBack, "> 上一段读数", countLog(INSTANCES[0].log, learnedNeedle(S.idB.runtimeId)));
    check("对照：翻回「开」之后 B 自己的日志里 discovery_started 又出现（与上面那个 0 成对）",
      countLog(B.log, "discovery_started") > 0, ">0", countLog(B.log, "discovery_started"));
    // #95 的探针与它的两条对照（三条读数全设断言：①开=连得上、②闲口=连不上、③关=连不上）。
    // ②是①③的非空转证明；①是③的对照物 —— 缺任何一条，剩下的那条都可能是半个守卫。
    //   ① 开着的时候监听口在（正向，设断言）；
    //   ② 谁也没占的那个口连不上（探针自己的负对照，设断言 —— 没有它，①与③都是半个守卫：
    //      一个永远回 true 的 `tcpOpen` 能让①假绿、让③"看起来红在环境"）；
    //   ③ 键为关时监听口不在（这一条才是 #95 要的答案）。
    //   ⚠️ 它今晚**先被判错过一次**：探针最初放在"翻回开、重起 B"之后，量的是开着的那个进程，
    //   于是出现"③=true 但 discovery_started=0、日志里还有 bc_directed 与 routed 建链"这种
    //   三个读数互不相容的假矛盾；把读数挪回关着的那段窗口，监听者列表就是空的。
    //   ⇒ 「归因不清」与「探针读错了对象」在报告里长得一模一样，区别只有读数钉没钉在事件上。
    const openWhenOn = await tcpOpen(B.port);
    check("对照：翻回「开」之后 B 的 TCP 监听口连得上（这个键开着时端口确实在听）",
      openWhenOn === true, "连得上（true）", String(openWhenOn));
    // 探针的负对照：挑一个本轮任何实例都不用的口，必须连不上。
    const probeControl = await tcpOpen(65501);
    check("对照：`tcpOpen` 读得出不存在的口（探针自己不是恒真机 —— 上面两条读数因此才算数）",
      probeControl === false, "连不上（false）", String(probeControl));
  });
}
