#!/usr/bin/env node
// 职责边界：双实例 E2E 的 **GROUP** 轮次分册（一族一轮：preset + run）。
// 为什么按族切而不是按行数切：这一族的判据只读自己那几份族私有的量
//   （AST 现算：89 个顶层名字只在某一个块里出现，跨块的只有 9 个），所以一个族 = 一个文件是**作用点的边界**。
// 「只搬不改」在这里是可证的：块体本来就是 2 格缩进（顶层 if 之内）⇒ 去掉旗标行与它顶格的 } 之后
//   正好是函数体缩进，逐字即所得、一行没重排。旗标与分发留在驱动脚本 e2e-multi-instance.mjs
//   （判据 D 与契约图那条 ROUND 现读命令因此一字不用改）。
// ⚠️ 判据 C（scripts/check-doc-numbers.mjs）按下面这行 export const MODE 把本文件的 check( 归到这一轮：
//    改这行、或把断言挪去别的分册，都会让活文档里那句「N 条断言」当场对不上。
export const MODE = "GROUP";
import { GROUP_ID, GROUP_KEY_B64, GROUP_KEY_STR, GROUP_NAME, INSTANCES, ROUND, RUN_DIR, S, buildGroupEnvelope, buildMentionTargets, check, ed25519Priv, noteId, nowMs, openDb, procs, seed, sleep, step, waitFor } from "../core.mjs";
import fs from "node:fs";
import path from "node:path";
import { countLog } from "../../e2e-logtail.mjs";

// ── 族私有的量（只有这一族读；随块一起搬过来）──
let gMentionText = "";
/// #122 那一格的**夹具自查**基准：落点是 harness 侧按规则算出来的（见 `buildMentionTargets`），
/// 留一份是为了后面那条跨进程判据能先判"夹具到底有没有数出来"——
/// 否则镜像函数哪天数成 0 条，红会挂在产品名下（本项目挂过好几次的形状：红在判据、不在被测物）。
let gMentionTargets = [];
/// #122 落点那三条的**专用反证**。为什么不能复用 `group-lie`：那一档翻的是 `msg_id`，
/// 而落点那三条读的是**真 id**（刻意不走 `want()`，见矩阵「轮次账」那句"反向红数仍是 9"）
/// ⇒ 翻 id 永远碰不到它们，`group-lie` 对这一格等于没判。
/// 这一档什么都不动拓扑与时序，**只把线上明文里的 `mention_targets` 键摘掉**
/// （= 对端退回"只带名单"的老形状）⇒ 期望恰好正向那一条红、对照与夹具自查两条照旧绿。
/// ⚠️ 它**不进任何门禁层**（与所有 `-lie` 档同规矩）：那是"把能红变成常红"的静音通道。
const TARGETS_LIE = ROUND === "group-targets-lie";
let gTextId, gRecallId, gText2Id;
/// §8「存储永远保存真实身份」那一格：A 打出来的 @ 正文（用的是 A 自己给 B 存的昵称）。
/// 呈现层可以把它换成「@你」，**库里那串字节一个字都不许动** —— 所以文本与 id 都要留着当比对基准。
let gMentionId = "";
/// `--round=group` 里 #103（@ 绑身份）那两条线级判据读的 id：一条明文带 `mentions`，一条不带这个键。
let gMentionOnlyId = "";
let gLegacyShapeId = "";
/// #143 正面判据用的"就绪之后才送"的那条 @。
/// **在这里构造、但不入库**：采样点（群聊轮中段）没有 `base`/`ts`/`convId` 这些闭包变量，
/// 所以把 seed 需要的整份纯数据一次性带出来；真正写进 A 的 `group_outbox` 发生在
/// "B 的界面已被证明渲染过"之后 —— 那正是这条判据与开机那次的唯一区别。
let gLateSeed = null;
/// 反向模式：注入与预置完全不动，只把**判据要去找的那个 msg_id** 换成一个必定不存在的值。
/// 报不出红 ⇒ 那几条断言读的不是真落库行。
const GROUP_LIE = ROUND === "group-lie";

/// 群聊这一族（`--round=group`）的预置。形状照 `e2e_peer.rs:300-338` 的 `ensure_test_group`
/// （仓内既有先例：停机给真实例写群记录），两端各写三行：
/// `settings['gk:{gid}']`（对称密钥，transport.rs:5981-5986 要求 base64 解出正好 32 字节）、
/// `groups` + `group_members`（发送侧 window.rs:133-143 两者缺一就 `Err`）、
/// `conversations`（e2e_peer 也写了；不写也能跑，但搜索谓词会漏掉无会话行的历史）。
/// A 侧再排两条群消息（正文 seq=1、撤回 seq=2）：**入队时对端进程还没起** ⇒
/// 这一轮只能靠建链后的 `flush_group_outbox` 送达，正是 §五「群聊 + 离线成员重新上线」那一格。
export async function preset() {
  step("群聊预置：两端各写一份群 + 同一份群密钥，A 再排两条群消息（正文 + 撤回）", () => {
    const members = [S.idA.runtimeId, S.idB.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of INSTANCES) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, S.idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
        // ★ 只有 `GOSSLAN_AX=1` 才写的**读数前置**（#140）：给 B 多写一行闲会话，保证被 @ 的那个群
        // **不是**界面自动打开的那条。
        // 为什么必须有它：桌面端启动会**自动打开 convs[0]**（`ResponsiveLayout.vue` 那条 watch，
        // 对齐微信桌面版），而这一轮 B 的列表里原本只有那一个群 ⇒ 那条 @ 永远落在"被自动打开"
        // 的会话上 ⇒ 按规则（`4.31.5`：开着**且真在看**才抑制）本来就不该亮红点。之前两次读数
        // 判不出结论的根因是这个，不是无障碍树读不到。
        // ⚠️ 生效方式**与我原先以为的不同**（下方 ① 已按现读改口）：决定 convs[0] 的是前端
        // `sortConversations`（只看 `last_ts`），不是 SQL 那条带 `updated_at` 兜底的排序
        // ⇒ 这行没有消息的闲会话在前端序里排**最后**；这一轮真正抢到 convs[0] 的是那条 1:1
        //   （「Wise Seal L45」，它有新到的文件消息）。保留它的理由改成：若某轮 B 的列表里只有群，
        //   它至少不会被自动打开 ⇒ 这条前置仍然有意义，但它不是这次读数能判的原因。
        // ⚠️ 必须写在这同一个 seed 回调里：先做成一条独立 step 时，它被排到整轮**最后**执行
        //    （日志里是 `[10/10] 读数前置`），那一刻 @ 早就被摄入并按规则抑制掉了 —— 读数是"没有"，
        //    但成因是采样前置没生效，不是产品。这种错不会报错，只会让一次真投递白做。
        // ★ 2026-09-28 第一次带这层前置跑到的读数（run-2026-09-27T22-25-52-307Z/ax-B-tree.txt）：
        //    B 的会话行是「E2E-Group，5 条未读」，全树 `有人` 命中 0（提示文案就是 `[有人@我]` ⇒
        //    不是"名字里没有@"那种误读）⇒ 红点没亮。
        // ★★ 那天稍后把剩下的解释逐条用现读排掉了，结论收窄成一条结构性事实（工单 #143 还留着最后一测）：
        //    ① 排序键**查错过一次、现已按现读改口**：`db/conversations.rs:90` 的 SQL 序
        //       (`COALESCE(last_ts, updated_at, 0) DESC`) **不是**决定 convs[0] 的那一个 ——
        //       自动打开走 `ResponsiveLayout.vue` 那条 watch 的 `convs[0]`，而 `chat.conversations`
        //       是前端 `sortConversations`（`src/utils/messages.ts:383`）排过的
        //       = `pinned` 优先、其余**只看 `last_ts ?? 0`、没有消息就是 0、不看 `updated_at`**
        //       ⇒ 实测界面上这一行的位置：**E2E-Idle 排最后**，convs[0] = 那条 1:1（「Wise Seal L45」，
        //       它有新到的文件消息）⇒ 被 @ 的群没被自动打开这条**结论不变**，但成立的理由是"列表里还有
        //       一条更新得更晚的 1:1"，不是"decoy 抢到了 convs[0]"。
        //       ⚠️ 教训（记在这里免得下次再犯）：判"界面第一条是谁"要去读**渲染后的序**，
        //       读 SQL 的 ORDER BY 会给我一个看着自洽、实际另一套的排序键 —— 同一个键名 `last_ts` 骗人。
        //    ② "emit 丢字段"排除：全仓只有 `network/transport/gossip.rs:930` 一处生产代码写
        //       `mention_ids`，而这次投递走的正是那条 Gossip 路径、载荷是 flatten 的 `IncomingMessage`；
        //    ③ "id 对不上"排除：判"不是自己发的"用的是同一个 `myDeviceId`，两者若不同本轮多条断言当场就红。
        //    ⇒ 剩下的结构性事实：`mentionedConvs` 全仓只有一个 `.add()`（摄入那一刻）、没有任何重算路径
        //       ⇒ 错过那一次 `message-received` 就永久没有这条提醒（未读仍涨，那是后端 `touch_conversation` 算的）。
        //    ★ 同日再补一条现读：**"错过 emit"从"未证实"升成"当前最可能的成因"** ——
        //       同一份日志文件（`instance-B-6.stashed-gosslan-2.log`）里第 1 行 `[boot] AppState::init 完成`
        //       与第 15–16 行 `群消息@输入 … mentions=1` **都是 22:25:53，相隔 0 秒**
        //       ⇒ @ 就落在后端刚起、WebView 前端很可能还没注册 `Tauri listen` 的那一瞬里。
        //       ⚠️ 这个数一度被我读成"相隔 71 秒"从而把这条解释判成削弱：那是**跨文件跨进程**的比较 ——
        //       同一轮里 `instance-B-2/-4/-6` 是 B 的**三次不同起停**（22:24:42 / 22:25:52 / 22:25:53），
        //       拿 -2 的 boot 去比 -6 的摄入，量出来的是两轮之间的间隔，不是一个进程内的时序。
        //       ⇒ 纪律：读旋转日志的时间戳**必须先确认是同一个文件（同一个进程）**，跨文件比时刻=废数。
        //    ★ 前端"当时在不在"另有直接证据：ax-B-tree.txt 里 AXWebArea 存在、会话行与正文名字都读得到
        //       ⇒ 采样那一刻界面确实渲染了（这不能反推"摄入那一刻"也渲染了 —— 那正是待判的那件事）。
        //    ★ 最后一测**已做完并出结论**（同日几分钟后，就是下面 `[ax-late]` 那一段）：
        //      就绪门 = 重试到树里出现 `AXWebArea`，然后才把一条带 `mentions=[B]` 的群 @ 写进 A 的
        //      `group_outbox`（靠心跳冲队列送达，`transport.rs:3011-3014`），等 B 自己打日志确认摄入后
        //      再采一次 ⇒ 读到 `AXButton=E2E-Group，7 条未读，[有人@我]`
        //      （产物 `run-2026-09-27T22-50-07-508Z/ax-B-tree-late.txt`）。
        //      ⇒ 同一轮、同一进程、同一扇窗的对照成立：**开机那一瞬投的不亮、就绪之后投的亮**
        //      ⇒ 未亮的成因 = 启动窗口错过那一次 `message-received`，而这枚红点只有一个写入点、无重算路径；
        //      "就绪后也点不亮"（更糟的那种形状）被排除。
        //    ⚠️ 这条读数证的是"能亮"，**不是**"启动窗口的丢法已关闭"。要不要在加载时用已落库的
        //      `mention_targets` 重算 = 会推翻"重启不该再亮一次红点"的既定口径 ⇒ 语义决定仍摆给用户拍板。
        if (process.env.GOSSLAN_AX === "1" && inst === INSTANCES[1]) {
          db.prepare(
            "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
            + " VALUES(?1,'group',?2,NULL,0,?3)",
          ).run("group:e2e-idle-window", "E2E-Idle", ts + 10 * 60 * 1000);
        }
      });
    }
    const base = {
      groupKey: GROUP_KEY_B64, senderId: S.idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: S.idA.x25519Pub, ed25519Pub: S.idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: S.idA.runtimeId, members,
    };
    const t = buildGroupEnvelope({ ...base, kind: "text", content: "hello from group harness", ts, seq: 1 });
    const r = buildGroupEnvelope({
      ...base, kind: "recall", content: JSON.stringify({ target: t.messageId }), ts: ts + 1, seq: 2,
    });
    // ⚠️ 第三条**不被撤回**的正文是必需的，不是为了凑数：第一轮实测（26/27 绿）只有 1 条正文
    // 时，「内容是解密后的明文」与「撤回把正文清空成 ""」这两格**读同一行的同一列**，
    // 于是先落的那格必红 —— 红在判据、不是产品（本项目第三次撞同一形状）。
    // 「真解密成功」这一格只能由一条**永远不会被物化覆盖**的行来证。
    const t2 = buildGroupEnvelope({ ...base, kind: "text", content: "second group message", ts: ts + 2, seq: 3 });
    // §8 那一格：A 打的一句 @B（用的是 A 库里给 B 存的昵称 —— `seedPair` 写的就是 `e2e-<label>`）。
    // 为什么这条值得排：把「@你」做成"存的时候替换"是这条规则最自然的破坏方式，
    // 而它的表现是**另一个人的屏幕上被烧进了我的视角**（同一条消息只能有一个正确存储形态）。
    gMentionText = `@e2e-${INSTANCES[1].label} 帮忙看这条`;
    // #122 的第二段：这一格现在发的是**当前版本**会发出去的那份形状 —— 名单 + 落点一起带
    // （落点按规则算，不手写：`n` 是"这个名字第几次出现"，写死就把要证的东西当已知用了）。
    // ⚠️ 为什么落在 `mt` 而不是 `mo`：`mo` 的正文刻意不含任何 `@`，当前版本给它算不出落点，
    //    那正好是"名单有、落点没有"的第三种形状（= 4.30.x 那个只带名单的对端），留着当对照。
    const gMentionName = `e2e-${INSTANCES[1].label}`;
    gMentionTargets = buildMentionTargets(gMentionText, [S.idB.runtimeId],
      (id) => (id === S.idB.runtimeId ? gMentionName : undefined));
    const mt = buildGroupEnvelope({
      ...base, kind: "text", content: gMentionText, ts: ts + 3, seq: 4,
      mentions: [S.idB.runtimeId],
      // ★ `--round=group-targets-lie` 就是从这里摘掉那一份：其余（拓扑、时序、名单、正文）
      //   与正向档一字不差，所以红只可能来自"落点没穿过 seal→网络→解析"这一件事。
      targets: TARGETS_LIE ? undefined : gMentionTargets,
    });
    // #103 的线级那一半（原来只有单元判据，跨进程没人证过）：**同一条管道**上排两封，
    // 一封明文带 `"mentions":["<B 的 id>"]`，一封**没有这个键**（旧版本的原样形状）。
    // 正文刻意不含任何 `@` ⇒ "B 被点名"这件事只能从名单里读到，按名字一律判不出 ——
    // 这正是"改过名字的人收不到历史上那些 @"的线上形态。
    const mo = buildGroupEnvelope({
      ...base, kind: "text", content: "这条只带身份号，正文里没有名字",
      mentions: [S.idB.runtimeId], ts: ts + 4, seq: 5,
    });
    const lg = buildGroupEnvelope({
      ...base, kind: "text", content: "旧形状的一条正文", ts: ts + 5, seq: 6,
    });
    gTextId = t.messageId;
    gRecallId = r.messageId;
    gText2Id = t2.messageId;
    gMentionId = mt.messageId;
    gMentionOnlyId = mo.messageId;
    gLegacyShapeId = lg.messageId;
    // ★ #143 正面判据那条 @ 的信封（只在 `GOSSLAN_AX=1` 构造，**此刻不入库**）
    if (process.env.GOSSLAN_AX === "1") {
      const LATE_CONTENT = "这条在界面就绪之后才送";
      const late = buildGroupEnvelope({
        ...base, kind: "text", content: LATE_CONTENT, ts: ts + 60, seq: 7,
        mentions: [S.idB.runtimeId],
      });
      noteId(late.messageId);
      gLateSeed = {
        messageId: late.messageId, wire: late.wire, content: LATE_CONTENT,
        convId, groupId: GROUP_ID, senderId: S.idA.runtimeId, peerId: S.idB.runtimeId, at: ts + 60,
      };
    }
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [t, 1, "text", "hello from group harness", ts],
        [r, 2, "recall", JSON.stringify({ target: t.messageId }), ts + 1],
        [t2, 3, "text", "second group message", ts + 2],
        [mt, 4, "text", gMentionText, ts + 3],
        [mo, 5, "text", "这条只带身份号，正文里没有名字", ts + 4],
        [lg, 6, "text", "旧形状的一条正文", ts + 5],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        // 逐列照发送内核（window.rs:179-190）：receiver_id 是**裸 group_id**、初始 status 是
        // 'sent'（不是 1:1 的 'sending'），content 存**明文**（密文只在信封里）
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, S.idA.runtimeId, GROUP_ID, kind, content, at, seq);
        // 每个非自身成员一行（window.rs:210-216）；payload 就是那整条已签名帧
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, S.idB.runtimeId, env.wire, at);
      }
      // 时钟必须一起推进，否则 A 之后自己发的消息会撞 seq（window.rs:127-130）
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 6);
    });
    console.log(`  · 群 ${GROUP_ID}：正文一 ${gTextId.slice(0, 12)}… / 撤回 ${gRecallId.slice(0, 12)}…`
      + ` / 正文二 ${gText2Id.slice(0, 12)}… / @ 那条 ${gMentionId.slice(0, 12)}…`
      + ` / 带名单 ${gMentionOnlyId.slice(0, 12)}… / 旧形状 ${gLegacyShapeId.slice(0, 12)}…`);
  });
}

export async function run() {
  step("群聊判据：每条各只落一行、明文要真解得开、@ 的那串字节不许被改写、撤回只物化不删行、Ack 必须把队列清干净", async () => {
    // 反向模式在这里翻的**只有判据读的 id**（预置、信封、投递全都一模一样）：
    // 真投递已经完成，却拿必定不存在的 id 去比 ⇒ 红只能来自断言本身，不来自基础设施噪声。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (GROUP_LIE ? flip(id) : id);
    const convId = `group:${GROUP_ID}`;
    // 前提断言（等四条都到齐）：没有它，下面几条会因为"链路根本没跑"而集体假绿
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [gTextId, gRecallId, gText2Id, gMentionId, gMentionOnlyId, gLegacyShapeId]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "B 侧六条群消息到齐（建链后 flush_group_outbox 送达）");
    // 撤回物化与投递是两次独立写盘，给它一个**有界**的等待（不许无条件睡）
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return db.prepare("SELECT kind FROM messages WHERE msg_id=?1").get(gTextId)?.kind
          === "recalled";
      } finally { db.close(); }
    }, 20_000, "B 侧那条正文被撤回物化成 recalled");

    const bDb = openDb(INSTANCES[1].db, true);
    const q = (id) => bDb.prepare(
      "SELECT m.conv_id,m.sender_id,m.kind,m.content,m.seq,m.status,c.kind conv_kind"
      + " FROM messages m LEFT JOIN conversations c ON c.id=m.conv_id WHERE m.msg_id=?1",
    ).all(id);
    const t1 = q(want(gTextId));
    const rec = q(want(gRecallId));
    const t2 = q(want(gText2Id));
    const mt = q(want(gMentionId));
    const mo = q(want(gMentionOnlyId));
    const lg = q(want(gLegacyShapeId));
    // 这一轮自己排的那六条（id 列表是唯一一处，条数由它推出来）：SQL 的占位符必须跟着长，
    // 写死 `IN (?2,?3,?4,?5)` 的话，加两条会让 node:sqlite 直接抛参数个数不符 —— 那是技术性红，
    // 不是判据红，读日志的人只会看见一个没头没尾的异常。
    const seededIds = [gTextId, gRecallId, gText2Id, gMentionId, gMentionOnlyId, gLegacyShapeId];
    const inList = (from) => seededIds.map((_, i) => `?${i + from}`).join(",");
    const leakedIntoOneToOne = bDb.prepare(
      `SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (${inList(2)})`,
    ).get(convId, ...seededIds).c;
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const stillQueued = aDb
      .prepare("SELECT COUNT(*) c FROM group_outbox WHERE group_id=?1").get(GROUP_ID).c;
    // 读的是**这一轮自己排的那四条**（按 id 取），不是"那个会话里现有多少行" ——
    // 后者会让"再加一条预置"变成改判据的分母，加一行就把这条判成红（实测：加完 @ 那条就红了，
    // 红的是判据不是产品）。id 列表在作用域里，条数由它推出来，不再写死。
    const aSeededIds = seededIds;
    const aStatus = aDb.prepare(
      `SELECT msg_id,status FROM messages WHERE msg_id IN (${inList(1)})`,
    ).all(...aSeededIds).map((r) => r.status);
    const aT1 = aDb.prepare("SELECT kind FROM messages WHERE msg_id=?1").get(gTextId);
    aDb.close();

    check("B 侧六条群消息各恰好一行（同 msg_id 多行=重复投递，少行=丢）",
      [t1, rec, t2, mt, mo, lg].every((rows) => rows.length === 1),
      "六条各 1 行", [t1, rec, t2, mt, mo, lg].map((r) => r.length).join("/"));
    // ★ §8 的存储侧不变量：**@ 的那串字节不许被任何一层改写**。
    // "被改写"有两种正好相反的死法 —— 替换成呈现层产物「@你」（把我的视角烧进公共数据），
    // 或"规范化"成 device id（把可读的那份弄没）。所以一条判"逐字等于打出去的原文"，
    // 一条判"这两个方向都没有出现"。前者是正向对照，缺了后者就不知道被换成了什么；
    // 反过来缺了前者，后者会因为"根本没投递"而假绿（同一形状见上面那条前提断言的注释）。
    check("A 打出的那句 @ 在 B 库里必须逐字等于原文（存储只保存真实昵称，「@你」是渲染时才换的）",
      mt.length === 1 && mt[0].content === gMentionText, gMentionText, JSON.stringify(mt[0]?.content));
    check("存储里既不许出现「@你」，也不许把昵称规范化成 device id（两个相反的破坏方向各钉一次）",
      mt.length === 1 && !mt[0].content.includes("@你") && !mt[0].content.includes(S.idB.runtimeId)
      && !mt[0].content.includes(S.idA.runtimeId),
      "只含真实昵称那一串", JSON.stringify(mt[0]?.content));
    // #103 的线级那一半：`mentions` 到底能不能穿过"seal → 网络 → 解密 → 解析"到达对端进程。
    // 读的是 **B 自己的日志**（不是 harness 写进去的东西）⇒ 判的是对端自己解出来的结果，
    // 不是"我发了"。两条成对：前一条红在"名单某一跳丢了"，后一条挡住
    // "把缺键也当成空名单"那种实现（那样旧对端发的 @ 会永久判不出来，比原缺陷更糟）。
    // ⚠️ 这一格判不到那枚红点：徽标活在 webview 的 store 里（不落库、也不进日志）
    //    ⇒ "界面上真的亮了"仍归 Smoke-10/11 人工（#117 记的就是这剩下的一半）。
    const mentionLineCount = countLog(
      INSTANCES[1].log, `群消息@输入 msg=${gMentionOnlyId} mentions=1`);
    const legacyLineCount = countLog(
      INSTANCES[1].log, `群消息@输入 msg=${gLegacyShapeId} mentions=none`);
    check("带 @ 名单那条：B 自己解密后读到 1 个身份号（名单穿过 seal→网络→解析没丢）",
      mentionLineCount >= 1, "≥1 行 mentions=1", mentionLineCount);
    check("没带这个键那条（旧形状）：B 判成「不知道」而不是「谁都没 @」——第三态跨进程成立",
      legacyLineCount >= 1, "≥1 行 mentions=none", legacyLineCount);
    // #122 的落点（`mention_targets`）走的是**同一条管道**，所以判据形状照上面那对抄：
    // 名单说"@ 的是谁"，落点说"正文里哪一段算他"。
    // 先钉夹具自己：镜像函数数出几条，决定下面那条正向判据有没有意义 ——
    // 夹具哪天算成 0 条，跨进程那条会红，而产品一个字没错（红要能归因，这是本文件的老规矩）。
    check("夹具自查：harness 侧按规则给这条正文算出了 1 个落点（否则下一条红在夹具、不在产品）",
      gMentionTargets.length === 1 && gMentionTargets[0].n === 1
      && gMentionTargets[0].id === S.idB.runtimeId && gMentionTargets[0].name
      && gMentionText.includes(`@${gMentionTargets[0].name}`),
      "1 条、n=1、id=B、name 真的出现在正文里", JSON.stringify(gMentionTargets));
    const targetLineCount = countLog(
      INSTANCES[1].log, `群消息@输入 msg=${gMentionId} mentions=1 targets=1`);
    check("名单+落点都带那条：B 自己解密后读到 1 个落点（mention_targets 穿过 seal→网络→解析没丢）",
      targetLineCount >= 1, "≥1 行 mentions=1 targets=1", targetLineCount);
    // 这一条是 INV-P24「新字段只许让新版更准、不许让老对端变暗」在**落点**这一层的跨进程版本：
    // 只带名单不带落点，是今天线上那些 4.30.x 对端的原样形状。
    // 它必须停在「不知道」：实现若"贴心地"补一个空数组，旧对端的 @ 就从"按昵称兜底"
    // 变成"权威地说谁都没 @ "—— 比原缺陷更糟，而且无声（单元层
    // `gossip_plaintext_with_targets_roundtrips_and_old_shape_still_parses` 钉过同一条，
    // 但那里没有真的第二进程；两边各钉是因为破坏点分别在这条管道的两截）。
    const rosterOnlyTargetCount = countLog(
      INSTANCES[1].log, `群消息@输入 msg=${gMentionOnlyId} mentions=1 targets=none`);
    check("只带名单那条：落点判成「不知道」而不是「空落点」——兜底路径跨进程仍然活着",
      rosterOnlyTargetCount >= 1, "≥1 行 mentions=1 targets=none", rosterOnlyTargetCount);
    // ★ **可选**的真界面采样（#140）：`GOSSLAN_AX=1` 时在"对端刚收下那条 @、窗口还活着"的这一刻，
    //   读一次 B 自己的系统无障碍树，把「那枚 @ 红点在真界面上到底叫什么」打出来并落进 run 目录。
    //   为什么必须在**这一处**：外部轮询采样实测会采到"还没建窗"的空读数（roadmap §12.6 记了那次）。
    //   ★ **有意不调 `check()`** ⇒ 本轮断言条数、`-lie` 那一趟的红数账、门禁各层输出全都不变；
    //   它是**读数**，判不判由人看（那枚红点是"摄入时判定 + 只在渲染进程内存里"，seed 库点不亮它 ⇒
    //   必须有这次真投递，而这一行就是那个时刻）。采样失败只说明原因，不判红 —— 它不在任何一层的判据里。
    //   ⚠️ 2026-09-28 第一次真跑到的读数**不能**当成"红点没亮"的证据：那一瞬 B 的窗口里那条会话是**开着**的
    //   （树里能看到「以下是未读消息」分隔线与那条正文），而未读名字「E2E-Group，5 条未读」「聊天，5 条未读」
    //   都在、唯独 @ 那句不在 ⇒ 分不清"该亮没亮"与"被看过所以已经清了"。
    //   ★ 那条歧义的根因同日查明，并已在这同一处消掉：桌面端启动会自动打开 convs[0]，而这一轮 B 的
    //   列表里只有那一个群 ⇒ @ 永远落在"开着的那个会话"上，按规则本来就不该亮。上面那个
    //   「读数前置」就是为此存在的（只在 GOSSLAN_AX=1 时写一行更晚的闲会话）⇒
    //   **带前置跑出来的"仍然没有那句"才是真故障证据**，不带前置跑出来的"没有"什么都不能说明。
    //   读的时候先确认这一步的日志里出现过「读数前置」那条 step，否则这次读数按无效处理。
    if (process.env.GOSSLAN_AX === "1") {
      try {
        const { probeTree } = await import("../../ax-tree.mjs");
        const bpid = procs.get(INSTANCES[1].n)?.pid;
        if (!bpid) console.log("  [ax] 拿不到 B 的 pid ⇒ 跳过（可选采样，不判红）");
        else {
          const r = probeTree(bpid, { tries: 6, gapMs: 2000 });
          const at = r.parsed.names.filter((n) => n.label.includes("@") || n.label.includes("有人"));
          console.log(`  [ax] B pid=${bpid}：第 ${r.tries} 次读挂上=${r.parsed.hung}`
            + ` 节点=${r.parsed.total} 带名字=${r.parsed.names.length} 个`
            + `；含「@／有人」的名字：${at.length ? at.map((h) => `${h.role}=${h.label}`).join(" | ") : "（一个都没有）"}`);
          // 全量名字直接打出来：这格今天第一次采到"没有那句"，而"没有"有两种成因
          // （红点真没渲染 / 那一屏根本没在会话列表上）——不打印全部名字就分不开这两种。
          console.log(`  [ax] B 界面上读得到的全部名字：${r.parsed.names.map((n) => n.label).join(" ｜ ")}`);
          fs.writeFileSync(path.join(RUN_DIR, "ax-B-tree.txt"), r.text);
          console.log(`  [ax] 原始读数：${path.join(RUN_DIR, "ax-B-tree.txt")}`);
        }
      } catch (e) {
        console.log(`  [ax] 采样不可用 ⇒ 跳过并说明原因：${e.message}`);
      }
    }
    // ★ #143 的**正面判据**（仍只在 `GOSSLAN_AX=1` 走，仍**有意不调 `check()`**）：
    //   上面那次读数是"开机同一秒就投递"⇒ 分不清「前端还没注册 listen 所以错过」与
    //   「摄入路径本身在真界面上也不点亮」。这里补上"界面**先证明渲染过**、再送一条 @"的那一格。
    //   送达靠的是**心跳也会 flush 群待发队列**（transport.rs:3011-3014，心跳 5 s），
    //   所以只往 A 的 `group_outbox` 写一行、不动生产码、不重启任何实例。
    //   三种结局各自的意思写在下面的打印里 —— 别再拿其中一种去当另一种的证据。
    if (process.env.GOSSLAN_AX === "1") {
      try {
        const { probeTree } = await import("../../ax-tree.mjs");
        const bpid = procs.get(INSTANCES[1].n)?.pid;
        if (!bpid) console.log("  [ax-late] 拿不到 B 的 pid ⇒ 跳过");
        else {
          // ① 就绪门：重试到树里出现 AXWebArea（= 前端真的挂载并渲染过），最多 ~24 s
          let ready = null;
          for (let i = 0; i < 8; i += 1) {
            ready = probeTree(bpid, { tries: 3, gapMs: 1500 });
            if (ready.text.includes("AXWebArea")) break;
            await sleep(1500);
          }
          const isReady = ready.text.includes("AXWebArea");
          console.log(`  [ax-late] 就绪门：AXWebArea=${isReady ? "在" : "不在"}`
            + ` 节点=${ready.parsed.total} 带名字=${ready.parsed.names.length} 个`);
          if (!isReady) {
            console.log("  [ax-late] ⇒ 前端没证明渲染过 ⇒ **这次读数按无效处理**，不判任何东西");
          } else if (!gLateSeed) {
            console.log("  [ax-late] 那条 @ 的信封没构造出来（非 AX 档或被跳过）⇒ 本次无效");
          } else {
            // ② 就绪之后才入库投递（seq=7 比时钟现有值 6 大，否则 A 自己后面发的会撞 seq）
            const L = gLateSeed;
            seed(INSTANCES[0].db, (db) => {
              db.prepare("DELETE FROM messages WHERE msg_id=?1").run(L.messageId);
              db.prepare(
                `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
                 VALUES(?1,?2,?3,?4,'text',?5,?6,7,'sent')`,
              ).run(L.messageId, L.convId, L.senderId, L.groupId, L.content, L.at);
              db.prepare(
                `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
                 VALUES(?1,?2,?3,?4,?5)`,
              ).run(L.messageId, L.groupId, L.peerId, L.wire, L.at);
              db.prepare(
                "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
                + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
              ).run(L.convId, 7);
            });
            // ③ 等 B 自己把这条摄入（日志行是发送侧那条判据用的同一族读数）
            let ingested = false;
            try {
              await waitFor(
                () => countLog(INSTANCES[1].log,
                  `群消息@输入 msg=${L.messageId} mentions=1`) >= 1,
                75_000, "B 摄入就绪后补送的那条 @",
              );
              ingested = true;
            } catch { /* 交给下面的三态打印 */ }
            console.log(`  [ax-late] 就绪后那条 @：摄入=${ingested ? "已发生" : "75 s 内没等到（读数无效，先修探针）"}`);
            if (ingested) {
              // ④ 摄入**之后**再采一次：徽标是响应式的，留几秒给渲染
              let lateTree = null;
              for (let i = 0; i < 3; i += 1) {
                lateTree = probeTree(bpid, { tries: 3, gapMs: 2000 });
                if (lateTree.text.includes("有人")) break;
                await sleep(1500);
              }
              const at = lateTree.parsed.names.filter(
                (n) => n.label.includes("@") || n.label.includes("有人"));
              fs.writeFileSync(path.join(RUN_DIR, "ax-B-tree-late.txt"), lateTree.text);
              console.log(`  [ax-late] 摄入后读数：带「有人／@」的名字 ${at.length} 个`
                + `${at.length ? `：${at.map((h) => `${h.role}=${h.label}`).join(" | ")}` : ""}`);
              console.log("  [ax-late] 全部名字："
                + lateTree.parsed.names.map((n) => n.label).join(" ｜ "));
              console.log(`  [ax-late] 原始读数：${path.join(RUN_DIR, "ax-B-tree-late.txt")}`);
              console.log("  [ax-late] 三种结局的读法：①出现「[有人@我]」⇒ 摄入路径在真界面上是通的"
                + "⇒ 先前那次未亮的成因就是启动窗口错过 emit（结构性事实：只有摄入那一个 .add()、无重算路径）；"
                + "②没出现⇒ 不只是启动窗口，就绪后收到也不亮⇒ 更严重，得查渲染链；"
                + "③这条压根没摄入⇒ 本次无效（心跳没冲到 / 外部写库没生效），别写进结论。");
            }
          }
        }
      } catch (e) {
        console.log(`  [ax-late] 正面判据不可用 ⇒ 跳过并说明原因：${e.message}`);
      }
    }
    // 两个键都缺（`lg`）那一格**不再单独钉一次**：它读的是同一行日志、同一条 map_or 分支，
    // 而"缺键⇒两份都是 None"已在 protocol.rs 的用例里钉住 —— 这里再钉一遍只是把同一个
    // 判据跑两遍（不产生新的可红面），按 §十四 不进账。
    check("落库的会话必须是群会话（conv_id 带 group: 前缀，且 conversations.kind='group'）",
      t2.length === 1 && t2[0].conv_id === convId && t2[0].conv_kind === "group",
      `${convId} / group`, `${t2[0]?.conv_id} / ${t2[0]?.conv_kind}`);
    check("未被撤回那条的正文必须是**解密后的明文**（拿到密文或空串都说明没真解密）",
      t2.length === 1 && t2[0].content === "second group message",
      "second group message", JSON.stringify(t2[0]?.content));
    check("发送方必须是 A 的 runtimeId、seq 必须照信封给（seq 是排序权威）",
      t2.length === 1 && t2[0].sender_id === S.idA.runtimeId && t2[0].seq === 3,
      `${S.idA.runtimeId} / seq=3`, `${t2[0]?.sender_id} / seq=${t2[0]?.seq}`);
    check("B 侧终态必须是 delivered（收到即记，不等 Ack 回传）",
      t2.length === 1 && t2[0].status === "delivered", "delivered", t2[0]?.status);
    check("撤回必须把目标**物化**成 kind=recalled + 空正文，而且**不许删行**（G-Set 语义）",
      t1.length === 1 && t1[0].kind === "recalled" && t1[0].content === "",
      "行还在 + kind=recalled + content=''",
      `${t1.length} 行 / ${t1[0]?.kind} / content=${JSON.stringify(t1[0]?.content)}`);
    check("撤回事件自身也必须留一行（历史只增不减，不许被当成一次性通知丢掉）",
      rec.length === 1, 1, rec.length);
    check("这几条群消息都不许串进 1:1 会话（两条管道共用同一对实例时的串味检查）",
      leakedIntoOneToOne === 0, 0, leakedIntoOneToOne);
    check("A 侧这个群的 group_outbox 必须被 GroupAck 清空（还留着=只发不认，重启会二次投递）",
      stillQueued === 0, 0, stillQueued);
    check("A 侧这一轮排的每条群气泡都不许被判成 failed（failed 的唯一裁决不能被群路径绕过）",
      aStatus.length === aSeededIds.length && !aStatus.includes("failed"),
      `${aSeededIds.length} 行都有状态且无 failed`, JSON.stringify(aStatus));
    check("A 侧那条正文此刻仍是 text —— ⚠️ **实测出来的边界**：harness 写的是入队形状，"
      + "没有执行产品的撤回命令 ⇒ 发送侧本地物化不在这一格的证明范围里",
      aT1?.kind === "text", "text（本格的已知边界）", aT1?.kind);
  });
}
