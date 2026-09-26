#!/usr/bin/env node
// 双实例 E2E harness（稳定版第二阶段 A-1 / v0）
//
// 设计判据见 docs/stability-roadmap.md §11：**零生产码改动** ——
// 用「停机预置 SQLite」表达用户动作（enqueue-before-deliver 保证入队即事实源），
// 用「进程信号 + 日志 + 磁盘」表达故障与断言，绝不新增任何可驱动面。
//
//   node scripts/e2e-multi-instance.mjs            # 跑 J1（文本消息 A→B 全链路）
//   node scripts/e2e-multi-instance.mjs --negative # 反向自证：B 不在，判据必须报红
//   GOSSLAN_E2E_BIN=<二进制> node scripts/e2e-multi-instance.mjs
//
// 退出码：0=全部 PASS，1=有 FAIL，2=环境不满足（没编译产物 / 平台不认识）。
// --negative 是**这条测试自己的非空转证明**（总指令§十四的「错误行为测试」）：
// 两边照常起、照常建链，只把这条消息的收件人换成幽灵 id —— 链路是好的，投递注定失败。
// 于是"报红"只能来自投递断言本身，而不是来自"B 没启动"这种基础设施噪声。
//
// 三种模式（每一个"正向绿"都要配一个"反向能红"，否则判据可能只是空转）：
//   默认                      → J1 文本 + J2 文件 + 重启，16 条断言，预期全绿
//   --negative                → 收件人换幽灵 id，投递断言预期报红
//   --fault=poison-part       → 注入脏 .part 前缀，20 条断言，预期全绿（产品须自愈或明确失败）
//   --fault=poison-part-lie   → 同样的注入，只把比对摘要换成必定不相等的值 ⇒ 预期报红
//   --fault=resume-prefix     → 注入②：真前缀必须被续传复用，21 条断言，预期全绿
//   --fault=resume-prefix-lie → 同样的注入，只把"期望已收字节数/期望摘要"换成错值 ⇒ 预期报红
//   --fault=kill-mid          → 注入③：接收中真 SIGKILL 对端，23 条断言，预期全绿
//   --fault=kill-mid-lie      → 同样的注入，只把"期望续发字节数/期望摘要"换成错值 ⇒ 预期报红
//   --fault=peer-freeze       → 注入④：对端被 SIGSTOP 冻住（有写无 ACK）⇒ 不许宣布送达，解冻后补齐
//   --fault=peer-freeze-lie   → 同样的注入，只换期望摘要 ⇒ 预期报红
//   --fault=src-shrunk        → 注入⑥：入队后源文件被改小 ⇒ 按磁盘真值收发，两侧终态一致
//   --fault=src-shrunk-lie    → 同样的注入，只换期望摘要 ⇒ 预期报红
//   --fault=multi-file        → 注入⑦：三单一起排队、其中两单同名 ⇒ 一张都不许丢、内容不许串味
//   --fault=multi-file-lie    → 同样的注入，只把其中一份的期望摘要换掉 ⇒ 预期报红
//   --fault=recv-readonly     → 注入⑤：接收目录只读（磁盘写不进去）⇒ 必须明确失败并止步，不许假 done、不许无限重试
//   --fault=recv-readonly-lie → 同样的注入，只把"该落到哪个终态"换成 done ⇒ 预期报红
//   --fault=recv-dir-rotted    → 注入⑧：预置可写 .part 后把目录改只读 ⇒ 只塌在最后那次 rename，
//                                「报 done 就必须有整份正确的文件」（A-5 那一格：⑤证不到的"半途才失败"）
//   --fault=recv-dir-rotted-lie→ 同样的注入，只把"接收侧不许假 done"换成"必须 done" ⇒ 预期报红
//   （每轮各几条断言**不在这里写**：`check-doc-numbers.mjs` 从下面的 check(" 调用点现算，
//    写进文档时要以「<轮次名> N 断言」的形式，否则不会被对账）
//   （kill 轮的文件尺寸用 E2E_KILL_MB 调，默认 100 —— 回环上实测 ~0.78s 走完，窗口够打）
//   （freeze 轮用 E2E_FREEZE_S 选调结长度，默认 30：**<45s** 是"链路还活着、只是没回执"，
//    **>45s** 越过 watchdog（健康阈值 15s×3）⇒ 真的拆链 + 重拨 + 重试，两种都是同一组结局判据）

const NEGATIVE = process.argv.includes("--negative");
// 判据自证先跑，**在任何实例启动之前**：这套 harness 用「日志里有没有某行」当就绪/投递证据，
// 而那层读法今天真的塌过一次（10 MB 轮的 boot 行被 512 KB 轮转藏进 .old.log ⇒ 20s 超时红，
// 连带 fail-fast 跳掉后面 9 步）。判据自己坏了的时候，必须以「判据坏了」退出，
// 不能留一个人去猜超时是谁的错。`--logtail-selfcheck` 是同一条检查的独立入口（不需要 release 产物）。
{
  const fails = selfcheckLogtail();
  if (process.argv.includes("--logtail-selfcheck")) {
    for (const f of fails) console.error(`  ❌ ${f}`);
    console.log(fails.length ? `✗ 日志判据自证红 ${fails.length} 条` : "✅ 日志判据自证 5 格全部成立");
    process.exit(fails.length ? 1 : 0);
  }
  if (fails.length) throw new Error(`日志判据自证不成立，先修判据再跑轮：\n  ${fails.join("\n  ")}`);
}
/// 故障注入模式（§八）。`--fault=poison-part` 见下方 preset 步骤的注释。
/// 另有**旅程轮** `--round=`（不是注入，是补一整条没测过的用户路径）：
///   --round=group    → 群聊这一族跨实例真跑：两端预置群 → A 排三条群消息（正文/撤回/正文）→
///                      对端上线后靠 flush_group_outbox 补发 → 判落库/解密/清队列/G-Set/不串味
///   --round=group-lie→ 预置与投递完全不动，只把判据读的 msg_id 换成不存在的值 ⇒ 预期按设计报红
///   断言条数不在这里写，由 check-doc-numbers 现算对账（同下面每一轮）。
const FAULT = (process.argv.find((a) => a.startsWith("--fault=")) || "").slice("--fault=".length);
const POISON = FAULT === "poison-part" || FAULT === "poison-part-lie";
/// 注入②：接收端已有**真实前缀** ⇒ 必须按前缀续传，不许从 0 重灌整份。
const RESUME = FAULT === "resume-prefix" || FAULT === "resume-prefix-lie";
/// 注入③：接收中**真杀进程**。窗口是实测的，不是猜的：100 MB 在回环上 ~0.78s 走完
/// （.part 每 ~52ms 涨 6.5MB）⇒ 20%~100% 之间有 ~0.6s 可打，所以这条不是掷骰子。
const KILL = FAULT === "kill-mid" || FAULT === "kill-mid-lie";
/// 这一条注入专用的尺寸（与 J2 的 1 MB 分开，免得把默认轮也拖慢）。
const KILL_BYTES = Number(process.env.E2E_KILL_MB || 100) * 1024 * 1024;
let xferId4, srcFile4, srcSha4, partAtKill = 0;
/// 注入④：对端**失联但没死** —— SIGSTOP 冻住 B。这一格钉的是
/// 「对端没回执期间两侧都不许假成功，对端回来必须自己补齐」。
/// ⚠️ 为什么不是"传输中途冻"：实测 100 MB 回环 0.78s 传完，而拆一条静默链路要 **45s**
/// （watchdog = 健康阈值 15s × 3）⇒ 在飞窗口等不到冻结生效。时序只能是
/// 「先冻 → 再入队 → 冻 N 秒 → 解冻」，N 决定落在哪个 regime（见 FREEZE_MS）。
/// ⚠️ 实测撞出的产品现状（30s / 60s 两跑相同）：**失联期间这一单一次都没被尝试过**
/// —— `file_outbox` 的重投只被入站事件触发，没有定时器 ⇒ 这一格**没覆盖**"到点重投"，
/// 也**没覆盖**"write 成功 ≠ 已送达"。已按 A 类风险登记在 roadmap，改前别把话说满。
const FREEZE = FAULT === "peer-freeze" || FAULT === "peer-freeze-lie";
/// 调结长度。**<45s**：链路还活着，只是对端不回话；
/// **>45s**：越过 watchdog ⇒ 拆链 + 重拨（解冻后由重拨/心跳重新触发 flush）。
/// 两种 regime 用同一组"结局空间"判据，不需要分叉。
const FREEZE_MS = Number(process.env.E2E_FREEZE_S || 30) * 1000;
const FREEZE_BYTES = Number(process.env.E2E_FREEZE_MB || 1) * 1024 * 1024;
/// 注入⑤：接收端**磁盘写不进去**（用户把接收目录设到只读盘 / 磁盘满 / 外接盘被拔）。
/// 与冻结轮正好成对：冻结轮里 A **一次都没尝试**（没有入站帧 ⇒ 没人触发 flush）；
/// 这里 B 活着、照常发心跳 ⇒ A 一定尝试、一定被拒，于是真正走的是
/// 「重试到上限 → GiveUp → 唯一出口记 failed」这条链（也是 outbox 第一次被真进程跑到 GiveUp）。
/// ⚠️ 注入落在 **offer 期**（`File::create` 就 EACCES，file.rs:1604）⇒ 走的是
///   `transport.rs:3720` 那条分支：只发 `FileReject{received:0}` + 记一条 error 日志，
///   **不写任何接收侧 DB 行**。所以这一轮**不许**断言"B 侧记了 failed"（那行根本不存在）；
///   B 侧可证的事实只有「日志里出现初始化失败」与「盘上没有这个 transfer 的任何东西」。
///   要测"收到一半才写失败 ⇒ 接收侧 upsert failed"得另开一条（预置可写 `.part` 再把目录改只读），
///   那是 A-5 的形状，不并进这一格。
const DISK = FAULT === "recv-readonly" || FAULT === "recv-readonly-lie";
const DISK_BYTES = Number(process.env.E2E_DISK_MB || 1) * 1024 * 1024;
/// 发送侧重试上限（file.rs:259 MAX_FILE_OUTBOX_RETRIES）—— 判据用它钉"不许无限重试"。
const DISK_MAX_ATTEMPTS = 5;
let xferId6, term6 = null;
/// 注入⑧：A-5 的形状 —— **预置可写 `.part`，再把接收目录改成只读**。
/// 与⑤成对但不重复：⑤ 打在 offer 期（`File::create` 就 EACCES ⇒ 只发 FileReject、
/// **接收侧不写任何行**，见上面那条注释），所以⑤**证明不了**"半途才失败时接收侧怎么收口"。
/// 这一轮把注落下得晚：真前缀已存在 ⇒ 往已存在的 inode 里写**不需要目录写权限**，
/// 于是字节照常流进来，唯一会 EACCES 的是**收尾那次 rename**（还有清理时的 unlink）。
/// ⇒ 这是「rename 才算完成」这条不变量第一次被活实例检验：报 done 就必须有整份正确的文件。
/// ⚠️ 判据刻意不预设"产品必须失败"（那等于替产品做决定）：只判**终态与磁盘自洽**。
/// 解除只读之后会不会自愈 —— 只打印实测，不设判据（没量过的事不写进断言）。
const ROT = FAULT === "recv-dir-rotted" || FAULT === "recv-dir-rotted-lie";
const ROT_BYTES = Number(process.env.E2E_ROT_MB || 1) * 1024 * 1024;
const ROT_PREFIX = 64 * 1024;
let xferId8, srcFile8, srcSha8, term8 = null;
let xferId5, srcFile5, srcSha5;
/// 注入⑥：§七「错误 size」的**真实用户形状** —— 不是线上收到一个谎报的 size（那一格协议层
/// 已经用 hash+length 判死了），而是**入队之后、真正发出去之前，磁盘上的原件被改小了**。
/// 现实触发：离线排队期间用户在原路径上裁掉/覆盖了同一个文件（视频剪完再发、同步盘回写）。
/// 窗口为什么是确定的：A 只在**收到对端某一帧**时才读盘（A-9 实测），所以
/// 「先冻住 B → 入队 → 截断 → 解冻」保证 A 一定读到截断后的版本，不靠运气。
/// ⚠️ 这里**入队三行**（messages 气泡 / file_transfers / file_outbox），与前五轮只插 outbox
///   不同：这一格的疑点正好在"入队时按磁盘算的那份 size 事后会不会被更正"，
///   少插一行就测不到它（`send_file_from_path_at` 的 offer size 来自 `meta.len()`，
///   而 `upsert_transfer` 的 ON CONFLICT 只改 status/path/progress，**不改 size**）。
const SHRINK = FAULT === "src-shrunk" || FAULT === "src-shrunk-lie";
const SHRINK_BYTES = Number(process.env.E2E_SHRINK_MB || 1) * 1024 * 1024;
const SHRINK_TO = Number(process.env.E2E_SHRINK_TO_BYTES || 4096);
let xferId7, srcFile7;
/// 注入⑦：§七「连续多文件」×「磁盘已有同名文件」这两格合起来测 —— 三个 transfer 一起排队，
/// 其中**两个文件名完全相同、内容不同**（真机形状：一次选两张同名截图、或连发两版同名文档）。
/// ⚠️ 为什么这一格值一次运行：接收端的落地名在 **offer 那一刻**由 `unique_path` 决定
///   （`file.rs:1598`，规则 `a.bin → a (1).bin`）。同名两单若在这之前都还没 rename，
///   两者会拿到**同一个 final_path** ⇒ 后一次 rename 直接覆盖前一次 = **静默数据丢失**
///   （`file.rs:1599-1602` 那段注释只解决了 `.part` 交错，没解决 final 撞名）。
///   少一个文件、或落地内容集合少一份，就是这条被判红 —— 不是"看着不顺眼"，是丢东西。
const MULTI = FAULT === "multi-file" || FAULT === "multi-file-lie";
let multiSpec = [];
/// 轮次（§九 旅程族，与 `--fault=` 的注入族并列）：`--round=group` = 群聊这一族跨实例真跑。
/// 为什么这一格值一轮：此前 harness **从未建过群** —— `grep -c group` 只命中 file_outbox 的
/// `group_id` 列名，§九「群聊：创建/同步/发送/成员离线/重新上线/撤回」在跨实例层面是零判据，
/// 而群消息走的是一条与 1:1 完全不同的管道（`group_outbox` 按成员一行 + Gossip 信封 +
/// `GroupAck` 删行 + G-Set 撤回）。
/// ⚠️ 与 1:1 的关键差异（决定了这一轮为什么要自己签名加密）：
///   `flush_group_outbox`（transport.rs:6689-6693）**不做 re-seal**，只是 `from_str` 之后原样
///   `try_send` —— 而 1:1 的 `flush_outbox` 每条都过 `reseal_for_send`。所以停机写入的那段
///   payload 必须**在写库那一刻就已经是合法、已密封、已签名的 Gossip 信封**，
///   放占位串只会得到"B 静默丢弃"（verify_envelope 不过 ⇒ handle_gossip 直接 return，
///   gossip.rs:61-99/194-204），那红的是脚本不是产品。
/// 这一轮顺带就是 §五 点名的两格组合：`群聊 + 离线成员重新上线`（入队时对端进程还没起，
/// 只能靠建链后的 flush 送达）与 `聊天 + 群聊 + 文件`（同一对实例同时背 1:1 与群两条管道，
/// 判据里专门有一格查两者互不串味）。
const ROUND = (process.argv.find((a) => a.startsWith("--round=")) || "").slice("--round=".length);
const GROUP = ROUND === "group" || ROUND === "group-lie";
/// 反向模式：注入与预置完全不动，只把**判据要去找的那个 msg_id** 换成一个必定不存在的值。
/// 报不出红 ⇒ 那几条断言读的不是真落库行。
const GROUP_LIE = ROUND === "group-lie";
const GROUP_ID = "g-e2e-harness";
const GROUP_NAME = "E2E-Group";
/// 群对称密钥：settings 表 `gk:{group_id}` = base64 的**正好 32 字节**
/// （transport.rs:5984-5986 解码后 `try_into::<[u8;32]>()`，长度不对直接 None ⇒ 永不解密）。
/// 先例：`e2e_peer.rs:62` 的 `GROUP_KEY_B64` 就是同一形状。
const GROUP_KEY_B64 = Buffer.alloc(32);
for (let i = 0; i < 32; i += 4) GROUP_KEY_B64.writeUInt32BE(0x6e00_0000 + i, i);
const GROUP_KEY_STR = GROUP_KEY_B64.toString("base64");
/// `GossipEngine` 的 ttl（`GossipEngine::new(bloom, lru, fanout, ttl)` 第四参，见
/// gossip_engine.rs:244 那组测试的形状）；转发每跳减一，写 0 会让对端直接丢。
const GROUP_TTL = 6;
let gTextId, gRecallId, gText2Id;

/// `poison-part-lie` = 这组判据自己的**非空转证明**：注入完全一样，只把比对用的期望摘要
/// 换成一个必定不相等的值。产品没坏 ⇒ 判据必须报红；报不出红 ⇒ 那几条断言读的不是真字节。
const LIE = FAULT.endsWith("-lie");
const LIE_SHA = "0".repeat(64);
/// 传输尺寸（默认 1 MB，`E2E_FILE_MB=N` 或 `--size=N` 覆盖）。这个旋钮不是为了测"大文件"本身，
/// 而是先量出**一次传输在回环上真实耗时多久**：「接收中杀进程」这类注入能不能做成
/// 非竞态，取决于窗口有没有那么长。量不出来就老实标 SIMULATED，不许伪装 PASS（§十）。
/// `--size=` 是给**尺寸阶梯**用的：同一轮旅程（文本 + 文件 + 重启）换档位重跑，
/// 证明"换尺寸"不是一条只在一个尺寸上成立的测试。**故意不做成新的注入轮次** ——
/// 加轮次要同步四处登记（MODE_LABEL / 门禁 local 层 / 判据 C 的轮次声明 / 正反两跑），
/// 而阶梯要测的东西与故障无关，复用默认轮的断言才是这一格的正解。
/// ⚠️ 必须 `Math.round`（实测，不是猜的）：`Buffer.alloc(1048.576)` **不抛错**，它静默给一个
/// **1048 字节**的 buffer ⇒ 于是后面那条 `bytes.length === FILE_BYTES` 变成
/// "1048 === 1048.576" = 假 ⇒ 报出来的红长得像产品 bug（"B 侧字节数与发送端一致"失败），
/// 坏的实际是档位算术。本仓已多次踩"红的是脚本不是被保护的东西"，所以取整是这条阶梯的承重。
const SIZE_ARG = process.argv.find((a) => a.startsWith("--size="));
const FILE_MB = SIZE_ARG
  ? Number(SIZE_ARG.slice("--size=".length))
  : Number(process.env.E2E_FILE_MB || 1);
const FILE_BYTES = Math.round(FILE_MB * 1024 * 1024);

import { spawn, spawnSync } from "node:child_process";
import { createHash, createCipheriv, createPrivateKey, randomBytes, randomUUID, sign } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { BOOT_LINE, bootBaseline, bootReady, readLogTail, selfcheckLogtail, stashLogs } from "./e2e-logtail.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const ISO = new Date().toISOString().replace(/[:.]/g, "-");
const RUN_DIR = path.join(ROOT, "test-results", `run-${ISO}`);

// ── 环境事实（不认识的平台直接退，不猜）────────────────────────────
function appDataDir() {
  const home = process.env.HOME || process.env.USERPROFILE;
  switch (process.platform) {
    case "darwin":
      return path.join(home, "Library", "Application Support", "com.gosslan.app");
    case "win32":
      return path.join(process.env.APPDATA, "com.gosslan.app");
    default:
      return null;
  }
}
function binaryPath() {
  if (process.env.GOSSLAN_E2E_BIN) return process.env.GOSSLAN_E2E_BIN;
  const exe = process.platform === "win32" ? "gosslan.exe" : "gosslan";
  return path.join(ROOT, "src-tauri", "target", "release", exe);
}

const APPDATA = appDataDir();
const BIN = binaryPath();
if (!APPDATA || !fs.existsSync(APPDATA)) {
  console.error(`✗ 找不到 app data 目录：${APPDATA ?? "(本平台不认识)"} —— 先跑过一次应用再说`);
  process.exit(2);
}
if (!fs.existsSync(BIN)) {
  console.error(`✗ 没有二进制：${BIN}\n  先 npm run build && (cd src-tauri && cargo build --release)`);
  process.exit(2);
}
// 测旧代码 = 白测（本项目真踩过）。判据：二进制必须不比源码新文件更旧。
//
// 但 **mtime 变新 ≠ 源码变了**：`verify-guards` 是「改坏 → 跑测试 → 原样写回」，
// 写回会把 `.rs` 的 mtime 推到当下，内容却一个字节都没动。真按 mtime 一刀切，
// 这条守卫会在每次护栏运行期间把 E2E 全部拒掉 —— 而它拒绝的理由是假的。
// 所以「变新了」必须再问一层：内容到底和 HEAD 一样吗？二进制是不是比那次提交更新？
// 两条都成立 ⇒ 只是 mtime 抖动，放行并说明；否则 ⇒ 真的可能在测旧码，红。
{
  const binM = fs.statSync(BIN).mtimeMs;
  const newest = newestMtime(path.join(ROOT, "src-tauri", "src"), 20);
  if (newest > binM + 1000) {
    const dirty = spawnSync("git", ["status", "--porcelain", "--", "src-tauri/src"],
      { cwd: ROOT, encoding: "utf8" }).stdout.trim();
    // 比的是**最后一次动过 src-tauri/src 的提交**，不是 HEAD。
    // 踩过的坑：只改文档/脚本的提交会把 HEAD 推到二进制之后，于是这条守卫把
    // "内容完全没变的二进制"判成过期 —— 守卫自己造假红，和被它挡住的旧二进制一样有害。
    const srcHeadIso = spawnSync("git", ["log", "-1", "--format=%cI", "--", "src-tauri/src"],
      { cwd: ROOT, encoding: "utf8" }).stdout.trim();
    const srcHeadMs = Date.parse(srcHeadIso);
    if (!dirty && Number.isFinite(srcHeadMs) && binM > srcHeadMs) {
      console.warn(`⚠️ 有 .rs 的 mtime 比二进制新，但 src-tauri/src 与 HEAD 内容完全一致，`
        + `且二进制晚于最后一次改动 src-tauri/src 的提交 —— 判定为 mtime 抖动，继续测当前内容。`);
    } else {
      console.error(`✗ 二进制比源码旧（二进制 ${new Date(binM).toISOString()}，源码最新 ${new Date(newest).toISOString()}）`);
      console.error(`  src-tauri/src 未提交改动：${dirty ? "有 ⇒ 源码真的动过" : "无"}；`
        + `二进制晚于「最后一次改动 src 的提交」（${srcHeadIso || "?"}）：${Number.isFinite(srcHeadMs) && binM > srcHeadMs}`);
      console.error("  ⇒ 你正在测旧代码。重编：cd src-tauri && cargo build --release");
      process.exit(2);
    }
  }
}
function newestMtime(dir, depth) {
  let m = 0;
  if (depth <= 0) return m;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name === "target" || e.name.startsWith(".")) continue;
    const p = path.join(dir, e.name);
    m = Math.max(m, e.isDirectory() ? newestMtime(p, depth - 1) : fs.statSync(p).mtimeMs);
  }
  return m;
}

// ── 实例定义 ───────────────────────────────────────────────────────
const TCP_BASE = 59992; // protocol.rs TCP_PORT；instance>0 时端口 = TCP_PORT + N*10
const INSTANCES = [1, 2].map((n) => ({
  n,
  label: n === 1 ? "A" : "B",
  port: TCP_BASE + n * 10,
  db: path.join(APPDATA, `gosslan-${n}.db`),
  log: path.join(APPDATA, "logs", `gosslan-${n}.log`),
}));

// ── 小工具 ─────────────────────────────────────────────────────────
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const nowMs = () => Date.now();

function openDb(file, readonly = false) {
  return new DatabaseSync(file, { readOnly: readonly });
}
/** 停机时写库：WAL 要先 checkpoint 再关，否则 -wal 里的内容下一次启动才被吸收（本项目实测口径）。 */
function seed(file, fn) {
  const db = openDb(file);
  try {
    fn(db);
    db.exec("PRAGMA wal_checkpoint(TRUNCATE);");
  } finally {
    db.close();
  }
}
async function waitFor(cond, ms, what) {
  const until = nowMs() + ms;
  while (nowMs() < until) {
    if (await cond()) return true;
    await sleep(500);
  }
  throw new Error(`超时（${ms}ms）等 ${what}`);
}
function tcpOpen(port) {
  return new Promise((res) => {
    const s = net.connect({ host: "127.0.0.1", port });
    s.setTimeout(800);
    s.once("connect", () => { s.destroy(); res(true); });
    s.once("timeout", () => { s.destroy(); res(false); });
    s.once("error", () => res(false));
  });
}
// 读实例日志一律走 e2e-logtail：应用会把超 512 KB 的日志整份轮转成 `.old.log`，
// 只看当前档会让「这一单真跑过」这类判据在轮转瞬间凭空看不见（10 MB 轮实测踩过）。
const tailLog = (file, n = 400) => readLogTail(file, n);

// PKCS8 定长前缀 + 32 字节种子 → 导出公钥（Node 原生支持 X25519 / Ed25519）
const PKCS8 = { x25519: "302e020100300506032b656e04220420", ed25519: "302e020100300506032b657004220420" };
function pubFromSecret(kind, b64secret) {
  const der = Buffer.concat([Buffer.from(PKCS8[kind], "hex"), Buffer.from(b64secret, "base64")]);
  const jwk = createPrivateKey({ key: der, format: "der", type: "pkcs8" }).export({ format: "jwk" });
  return Buffer.from(jwk.x, "base64url").toString("base64"); // 应用侧统一标准 base64
}

/// 停机时刻自制一个**合法**的群 Gossip 信封（`--round=group` 专用）。
/// 为什么必须由 harness 签名加密，而不是像 1:1 那样丢给应用去 re-seal：
/// `flush_group_outbox`（transport.rs:6689-6693）只 `serde_json::from_str` 再原样 `try_send`，
/// **不过 `reseal_for_send`** ⇒ 写进 `group_outbox.payload` 的那段 JSON 会被逐字节发上线。
/// 三把材料 harness 全都有：A 的 ed25519 私钥（settings）、A 的两把公钥（readIdentity 已导出）、
/// 以及 harness 自己写进 `settings['gk:{gid}']` 的群对称密钥。
/// 每一处序列化都必须与 Rust 侧逐字对齐，错一处得到的就是"B 静默丢弃"（红在脚本）：
/// - `message_id` = SHA-256(sender_id ‖ nonce ‖ payload) 的**小写 hex**（protocol.rs:856-863）
/// - 签名材料 = 这 15 个字段的**紧凑 JSON 数组**，顺序照 `signing_bytes()`（protocol.rs:868-885），
///   `ttl` 不在里面（中继会递减），`target` 为 `null`
/// - 载荷 = base64(nonce12 ‖ ChaCha20-Poly1305(群密钥, {"kind":..,"content":..}))，无 AAD
///   （crypto.rs:83-97 `seal`，`encrypt` 不带 associated data）
/// - `kind` 恒为 `"group"`（GossipKind 的 snake_case），`encrypted` 恒 true
function buildGroupEnvelope(o) {
  const iv = randomBytes(12);
  const c = createCipheriv("chacha20-poly1305", o.groupKey, iv, { authTagLength: 16 });
  const sealed = Buffer.concat([
    c.update(JSON.stringify({ kind: o.kind, content: o.content }), "utf8"),
    c.final(),
    c.getAuthTag(),
  ]);
  const payload = Buffer.concat([iv, sealed]).toString("base64");
  const nonce = randomUUID();
  const messageId = createHash("sha256")
    .update(Buffer.concat([
      Buffer.from(o.senderId, "utf8"),
      Buffer.from(nonce, "utf8"),
      Buffer.from(payload, "utf8"),
    ]))
    .digest("hex");
  const signing = JSON.stringify([
    messageId, o.senderId, nonce, o.x25519Pub, o.ed25519Pub,
    "group", o.groupId, o.groupName, o.creator, o.members, payload, o.ts, o.seq, true, null,
  ]);
  const env = {
    message_id: messageId,
    sender_id: o.senderId,
    nonce,
    sender_pubkey: o.x25519Pub,
    sender_ed25519: o.ed25519Pub,
    sender_sig: sign(null, Buffer.from(signing, "utf8"), o.priv).toString("base64"),
    ttl: GROUP_TTL,
    kind: "group",
    group_id: o.groupId,
    group_name: o.groupName,
    group_creator: o.creator,
    group_members: o.members,
    payload,
    ts: o.ts,
    seq: o.seq,
    encrypted: true,
  };
  return { messageId, wire: JSON.stringify({ type: "gossip", envelope: env }) };
}

/// 从实例库里取回 ed25519 私钥对象（只有 harness 需要，产品侧从不导出私钥）。
function ed25519Priv(inst) {
  const db = openDb(inst.db, true);
  try {
    const b64 = db.prepare("SELECT value FROM settings WHERE key='ed25519_secret'").get()?.value;
    if (!b64) throw new Error(`${inst.label} 库里没有 ed25519_secret`);
    const der = Buffer.concat([Buffer.from(PKCS8.ed25519, "hex"), Buffer.from(b64, "base64")]);
    return createPrivateKey({ key: der, format: "der", type: "pkcs8" });
  } finally { db.close(); }
}

// ── 生命周期 ───────────────────────────────────────────────────────
const procs = new Map();
let stashSeq = 0;
/** 每次 launch 前把该实例的历史日志移进 run 目录（不删）；本轮写的行因此必然在当前档里。 */
const bootBaseOf = new Map();
function launch(inst) {
  stashLogs(inst.log, RUN_DIR, `${inst.label}-${++stashSeq}`);
  bootBaseOf.set(inst.n, bootBaseline(inst.log, BOOT_LINE));
  const out = fs.openSync(path.join(RUN_DIR, `instance-${inst.label}.stdout.log`), "a");
  const p = spawn(BIN, [], {
    env: { ...process.env, GOSSLAN_INSTANCE: String(inst.n), GOSSLAN_AUTOSTART: "1" },
    stdio: ["ignore", out, out],
    detached: false,
  });
  procs.set(inst.n, p);
  return p;
}
async function stopAll() {
  for (const [n, p] of procs) {
    if (p.exitCode === null) p.kill("SIGTERM");
    try {
      await Promise.race([
        new Promise((r) => p.once("exit", r)),
        sleep(8000).then(() => { if (p.exitCode === null) p.kill("SIGKILL"); }),
      ]);
    } catch { /* 已经退了 */ }
    procs.delete(n);
  }
  // 残留必须是 0 —— 「没清理干净」本身就是失败，不许静默
  const left = procsLeft();
  if (left.length) throw new Error(`清理后仍有实例活着：${left.join(", ")}`);
}
function procsLeft() {
  if (process.platform === "win32") {
    const exe = path.basename(BIN);
    const r = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${exe}`, "/FO", "CSV", "/NH"], { encoding: "utf8" });
    return ((r.stdout || "").includes(exe)) ? [exe] : [];
  }
  const r = spawnSync("pgrep", ["-f", `${BIN}`], { encoding: "utf8" });
  return (r.stdout || "").trim().split("\n").filter(Boolean);
}
async function bootAndStop(what) {
  for (const i of INSTANCES) launch(i); // launch 内部会先给该实例清档，基线随之一并重置
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `${what}：实例 ${i.label} 的 TCP ${i.port} 可连`);
    await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 20_000,
      `${what}：实例 ${i.label} 打出 boot 完成行`);
  }
  await stopAll();
}

// ── 预置（L-A）─────────────────────────────────────────────────────
function readIdentity(inst) {
  const db = openDb(inst.db, true);
  try {
    const get = (k) => db.prepare("SELECT value FROM settings WHERE key=?1").get(k)?.value ?? null;
    const base = get("device_id");
    const xSec = get("x25519_secret");
    const eSec = get("ed25519_secret");
    if (!base || !xSec || !eSec) throw new Error(`${inst.label} 库里缺身份键（base=${base}）`);
    // 陷阱 4：secret 格式错时应用会**静默重新生成**，所以这里必须回读校验长度
    if (Buffer.from(xSec, "base64").length !== 32 || Buffer.from(eSec, "base64").length !== 32) {
      throw new Error(`${inst.label} 的密钥不是 32 字节 base64 —— 预置失败，别往下走`);
    }
    return {
      // 陷阱 1：instance>0 时运行时 id 是 base + "-iN"，settings.device_id 只是基名
      runtimeId: `${base}-i${inst.n}`,
      x25519Pub: pubFromSecret("x25519", xSec),
      ed25519Pub: pubFromSecret("ed25519", eSec),
    };
  } finally { db.close(); }
}
function seedPair(nodes) {
  INSTANCES.forEach((inst, idx) => {
    const peer = nodes[1 - idx];
    const recv = path.join(RUN_DIR, "recv", inst.label);
    fs.mkdirSync(recv, { recursive: true });
    seed(inst.db, (db) => {
      db.prepare("DELETE FROM friends WHERE device_id=?1").run(peer.runtimeId);
      // 陷阱 5：ed25519 留 NULL 交给 TOFU 自绑（预置错值会永久拒 Hello 且不可恢复）；
      // x25519 必须是真值，否则 re-seal 拿不到密钥 ⇒ 明文发出 ⇒ 对端静默丢弃。
      db.prepare(
        `INSERT INTO friends(device_id,nickname,avatar,x25519_pubkey,ed25519_pubkey,added_at)
         VALUES(?1,?2,NULL,?3,NULL,?4)`,
      ).run(peer.runtimeId, `e2e-${peer.label}`, peer.x25519Pub, Date.now());
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(JSON.stringify([{ address: `127.0.0.1:${peer.port}` }]));
      // 陷阱：macOS 上 load() 优先信书签 ⇒ 只写路径，绝不写 downloads_dir_bookmark
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recv);
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('lan_enabled','true')").run();
    });
  });
}

// ── 断言账本 ───────────────────────────────────────────────────────
const steps = [];
const assertions = [];
let curStep = null;
let curStepIdx = -1;
function step(name, fn) { steps.push({ name, fn }); }
function check(name, pass, expect, actual) {
  assertions.push({
    stepIdx: curStepIdx, step: curStep?.name ?? null,
    name, verdict: pass ? "PASS" : "FAIL", expect, actual,
  });
  console.log(`  ${pass ? "✅" : "❌"} ${name}${pass ? "" : `\n      预期 ${expect} / 实际 ${actual}`}`);
  return pass;
}
/// 本轮总判据**只能**由断言账本推出来。
/// 之前的写法是「没抛异常 = 成功」，而 `check()` 返回 false 并不抛 ⇒
/// 一条真回归会被写成 summary.json 里的 PASS，只有人盯着控制台才看得见。
/// 机器判据不能依赖人读日志。
const anyFail = () => assertions.some((a) => a.verdict === "FAIL");
/// 「起 A/B 并等链路真的建立」那一步的下标。反向自证要求红**落在它之后**：
/// 红若落在建链之前，那只是应用没起来，证明不了「断言依赖真实投递」。
const linkStepIdx = () => steps.findIndex((s) => s.name.startsWith("起 A/B"));
/// 失败时把两端日志里出现这条 trace 的行摘出来 —— §十六要的「日志关联」不是写个文件名，
/// 而是要能顺着 msg_id / transfer_id 直接看见对端说过什么。
function traceExcerpt() {
  const ids = [msgId, xferId, xferId2, xferId3].filter(Boolean);
  if (!ids.length) return {};
  const out = {};
  for (const i of INSTANCES) {
    const body = tailLog(i.log, 20000) || "";
    out[i.label] = body.split("\n").filter((l) => ids.some((id) => l.includes(id))).slice(-8);
  }
  return out;
}
async function runStep(i, s) {
  curStep = s;
  curStepIdx = i;
  const t0 = nowMs();
  const before = assertions.length;
  process.stdout.write(`\n[${i + 1}/${steps.length}] ${s.name}\n`);
  let threw = null;
  try {
    await s.fn();
  } catch (e) {
    threw = e;
    check(s.name, false, "不抛错", String(e.message ?? e));
  }
  const made = assertions.slice(before);
  s.ms = nowMs() - t0;
  s.checks = made.length;
  s.verdict = made.some((a) => a.verdict === "FAIL") ? "FAIL" : made.length ? "PASS" : "NO-ASSERT";
  if (s.verdict === "FAIL") s.logs = traceExcerpt();
  console.log(`      ${(s.ms / 1000).toFixed(1)}s · ${s.checks} 断言 · ${s.verdict}`);
  if (threw) throw threw;
}

// ── J1：文本消息 A→B 全链路 ────────────────────────────────────────
let idA, idB, msgId, NODES, peerTo, xferId, srcFile, srcSha;
let xferId2, srcFile2, srcSha2, xferId3, srcFile3, srcSha3;
step("停机预置：好友 + routed 端点 + 独立接收目录", () => seedPair(NODES));

step("L-A 入队：在 A 的库里留下「已入队待发送」的事实", () => {
  msgId = `e2e-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
  // 反向模式：链路照建，只是注定送不到 —— 报红必须来自投递断言本身
  peerTo = NEGATIVE ? `${idB.runtimeId}-ghost` : idB.runtimeId;
  const ts = nowMs();
  const payload = JSON.stringify({
    type: "chat_message",
    msg_id: msgId,
    from: idA.runtimeId,
    to: peerTo,
    kind: "text",
    content: "enc1:harness-placeholder", // 占位；应用会 re-seal 成真密文
    ts,
    seq: 1,
  });
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM messages WHERE msg_id=?1").run(msgId);
    db.prepare("DELETE FROM outbox WHERE msg_id=?1").run(msgId);
    // 陷阱 10：两行必须成对 —— 只插 outbox 则 re-seal 没有明文，只插 messages 则 Ack 找不到人
    db.prepare(
      `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
       VALUES(?1,?2,?3,?4,'text',?5,?6,1,'sending')`,
    ).run(msgId, idB.runtimeId, idA.runtimeId, peerTo, "hello from harness", ts);
    db.prepare("INSERT INTO outbox(msg_id,peer_id,payload,created_at) VALUES(?1,?2,?3,?4)")
      .run(msgId, peerTo, payload, ts);
  });
});

step("L-A 入队：A 的一个 1 MB 文件也排好队（停机窗口内）", () => {
  xferId = `e2e-x-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
  const dir = path.join(RUN_DIR, "src");
  fs.mkdirSync(dir, { recursive: true });
  srcFile = path.join(dir, `${xferId}.bin`);
  // 真随机字节：全零会被任何"压缩/去重"路径悄悄改掉而断言看不出来
  const buf = Buffer.alloc(FILE_BYTES);
  for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
  fs.writeFileSync(srcFile, buf);
  srcSha = createHash("sha256").update(buf).digest("hex");
  seed(INSTANCES[0].db, (db) => {
    db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId);
    // 陷阱 8：local_path 必须真实存在，否则每次重试白烧一个 attempts 配额
    db.prepare(
      `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
       VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
    ).run(xferId, peerTo, srcFile, `${xferId}.bin`, buf.length, nowMs());
  });
});

// ── 故障注入（总指令§八 / roadmap A-2）：--fault=poison-part ─────────
// 为什么选「给接收端预置一段脏 .part」，而不是「改 A 库里的 sha256」：
//   发送侧的 hash 是 `sha256_file_hex()` **发送时从磁盘现算**的（network/file.rs:375/665），
//   DB 里那一份改了就等于没改 —— 那条注入只会红得莫名其妙（我差点就写成那样）。
//   而接收侧 `resume_receive` 明写「不再 truncate，用已有前缀播种 hasher」（file.rs:1317-1388），
//   所以一个**严格短于文件**的脏 .part = 确定性地让最终 sha256 不匹配，零生产码改动、无竞态。
if (POISON) {
  step("故障注入：A 再排一个 1 MB 文件，同时给 B 预置 4 KB 脏 .part 前缀", () => {
    xferId2 = `e2e-p-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
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
      ).run(xferId2, peerTo, srcFile2, `${xferId2}.bin`, buf.length, nowMs());
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

// ── 注入②：--fault=resume-prefix ────────────────────────────────────
// 与"脏前缀"只差一件事：这里预置的 64 KiB 是**源文件自己的开头**。
// 于是接收端报出的已收字节是真值 ⇒ 发送端必须从 65536 续发；hasher 用真前缀播种后，
// 最终 sha256 必须仍然等于源。它钉的是 decide_offer 注释里那次 160MB 真机事故
// （"有活跃接收器时一律 Accept" ⇒ 每轮从 0 重灌 ⇒ 界面恒 0% ⇒ 最后判"分片失败"）：
// **对有效前缀不许重灌**是行为契约，不是性能偏好。
if (RESUME) {
  step("预置（注入②）：B 侧已有真前缀 64 KiB + A 侧待发一个 1 MB 文件", () => {
    xferId3 = `e2e-r-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
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
      ).run(xferId3, peerTo, srcFile3, `${xferId3}.bin`, buf.length, nowMs());
    });
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    fs.writeFileSync(path.join(dl, `${xferId3}.part`), buf.subarray(0, 64 * 1024));
    console.log(`      预置真前缀 65536 字节 / 全文件 ${buf.length} 字节（sha256=${srcSha3.slice(0, 12)}…）`);
  });
}

if (KILL) {
  step(`预置（注入③）：先生成一个 ${KILL_BYTES / 1024 / 1024} MB 源文件（行进库留到判据里，见下面那段注释）`, () => {
    xferId4 = `e2e-k-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
    const dir = path.join(RUN_DIR, "src");
    fs.mkdirSync(dir, { recursive: true });
    srcFile4 = path.join(dir, `${xferId4}.bin`);
    const buf = Buffer.alloc(KILL_BYTES);
    for (let i = 0; i < buf.length; i += 32) buf.writeUInt32BE(Math.floor(Math.random() * 2 ** 32), i);
    fs.writeFileSync(srcFile4, buf);
    srcSha4 = createHash("sha256").update(buf).digest("hex");
    console.log(`      源文件 ${buf.length / 1024 / 1024} MB（sha256=${srcSha4.slice(0, 12)}…）`);
  });
}

/// 群聊这一族（`--round=group`）的预置。形状照 `e2e_peer.rs:300-338` 的 `ensure_test_group`
/// （仓内既有先例：停机给真实例写群记录），两端各写三行：
/// `settings['gk:{gid}']`（对称密钥，transport.rs:5981-5986 要求 base64 解出正好 32 字节）、
/// `groups` + `group_members`（发送侧 window.rs:133-143 两者缺一就 `Err`）、
/// `conversations`（e2e_peer 也写了；不写也能跑，但搜索谓词会漏掉无会话行的历史）。
/// A 侧再排两条群消息（正文 seq=1、撤回 seq=2）：**入队时对端进程还没起** ⇒
/// 这一轮只能靠建链后的 `flush_group_outbox` 送达，正是 §五「群聊 + 离线成员重新上线」那一格。
if (GROUP) {
  step("群聊预置：两端各写一份群 + 同一份群密钥，A 再排两条群消息（正文 + 撤回）", () => {
    const members = [idA.runtimeId, idB.runtimeId];
    const convId = `group:${GROUP_ID}`;
    const ts = nowMs();
    for (const inst of INSTANCES) {
      seed(inst.db, (db) => {
        db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)")
          .run(`gk:${GROUP_ID}`, GROUP_KEY_STR);
        db.prepare("INSERT OR REPLACE INTO groups(id,name,creator,created_at) VALUES(?1,?2,?3,?4)")
          .run(GROUP_ID, GROUP_NAME, idA.runtimeId, ts);
        db.prepare("DELETE FROM group_members WHERE group_id=?1").run(GROUP_ID);
        for (const m of members) {
          db.prepare("INSERT OR IGNORE INTO group_members(group_id,device_id) VALUES(?1,?2)")
            .run(GROUP_ID, m);
        }
        db.prepare(
          "INSERT OR REPLACE INTO conversations(id,kind,name,avatar,unread,updated_at)"
          + " VALUES(?1,'group',?2,NULL,0,?3)",
        ).run(convId, GROUP_NAME, ts);
      });
    }
    const base = {
      groupKey: GROUP_KEY_B64, senderId: idA.runtimeId, priv: ed25519Priv(INSTANCES[0]),
      x25519Pub: idA.x25519Pub, ed25519Pub: idA.ed25519Pub,
      groupId: GROUP_ID, groupName: GROUP_NAME, creator: idA.runtimeId, members,
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
    gTextId = t.messageId;
    gRecallId = r.messageId;
    gText2Id = t2.messageId;
    seed(INSTANCES[0].db, (db) => {
      db.prepare("DELETE FROM group_outbox WHERE group_id=?1").run(GROUP_ID);
      for (const [env, seq, kind, content, at] of [
        [t, 1, "text", "hello from group harness", ts],
        [r, 2, "recall", JSON.stringify({ target: t.messageId }), ts + 1],
        [t2, 3, "text", "second group message", ts + 2],
      ]) {
        db.prepare("DELETE FROM messages WHERE msg_id=?1").run(env.messageId);
        // 逐列照发送内核（window.rs:179-190）：receiver_id 是**裸 group_id**、初始 status 是
        // 'sent'（不是 1:1 的 'sending'），content 存**明文**（密文只在信封里）
        db.prepare(
          `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
           VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'sent')`,
        ).run(env.messageId, convId, idA.runtimeId, GROUP_ID, kind, content, at, seq);
        // 每个非自身成员一行（window.rs:210-216）；payload 就是那整条已签名帧
        db.prepare(
          `INSERT OR IGNORE INTO group_outbox(msg_id,group_id,peer_id,payload,created_at)
           VALUES(?1,?2,?3,?4,?5)`,
        ).run(env.messageId, GROUP_ID, idB.runtimeId, env.wire, at);
      }
      // 时钟必须一起推进，否则 A 之后自己发的消息会撞 seq（window.rs:127-130）
      db.prepare(
        "INSERT INTO conversation_clocks(conv_id,seq) VALUES(?1,?2)"
        + " ON CONFLICT(conv_id) DO UPDATE SET seq=excluded.seq",
      ).run(convId, 3);
    });
    console.log(`  · 群 ${GROUP_ID}：正文一 ${gTextId.slice(0, 12)}… / 撤回 ${gRecallId.slice(0, 12)}…`
      + ` / 正文二 ${gText2Id.slice(0, 12)}…`);
  });
}

step("起 A/B 并等链路真的建立（routed 拨号一轮 10s）", async () => {
  for (const i of INSTANCES) launch(i);
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `实例 ${i.label} TCP ${i.port} 可连`);
  }
  // 断言链路成立，而不是靠 sleep：日志里的 +conn peer= 必须出现「对端那个 id」
  const other = { A: { inst: INSTANCES[0], id: idB }, B: { inst: INSTANCES[1], id: idA } };
  for (const side of ["A", "B"]) {
    const { inst, id } = other[side];
    await waitFor(
      () => tailLog(inst.log, 4000).includes(`peer=${id.runtimeId}`),
      90_000,
      `${side} 侧日志出现与 ${id.runtimeId} 的链路`,
    );
  }
});

step("A→B 送达 + Ack 回收 + 无重复", async () => {
  const [aDb, bDb] = [openDb(INSTANCES[0].db, true), openDb(INSTANCES[1].db, true)];
  const peekB = () => bDb.prepare("SELECT * FROM messages WHERE msg_id=?1").all(msgId);
  const outboxLeft = () => aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(msgId).c;
  await waitFor(async () => peekB().length > 0, 60_000, "B 侧出现这条消息");
  // 等条件而不是等时间：Ack 回来才继续（否则慢机器上会把"还没到"读成"丢了"）
  await waitFor(() => outboxLeft() === 0, 30_000, "A 侧 outbox 被 Ack 删除");
  await sleep(1500); // 多留一点窗口，让「重复投递」这种退化有机会显现

  const rowsB = peekB();
  check("B 侧恰好一条（重复投递 = 用户看到两条）", rowsB.length === 1, 1, rowsB.length);
  check("B 侧内容与应用解密结果一致",
    rowsB[0]?.content === "hello from harness", "hello from harness", rowsB[0]?.content);
  check("B 侧方向正确（sender 是 A）",
    rowsB[0]?.sender_id === idA.runtimeId, idA.runtimeId, rowsB[0]?.sender_id);

  const left = aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(msgId).c;
  check("A 侧 outbox 被 Ack 清空", left === 0, 0, left);
  const st = aDb.prepare("SELECT status FROM messages WHERE msg_id=?1").get(msgId)?.status;
  check("A 侧状态前进过 sending", !["sending", "failed"].includes(st), "sent/delivered/read", st);
  const uniq = bDb.prepare(
    "SELECT COUNT(*) c FROM messages WHERE msg_id=?1",
  ).get(msgId).c;
  check("msg_id 在 B 侧唯一（INV-P01）", uniq === 1, 1, uniq);
  aDb.close(); bDb.close();
});

step("J2 文件 A→B：只有 rename 之后才算完成，且字节与 hash 一致", async () => {
  const recvB = path.join(RUN_DIR, "recv", "B");
  const landed = path.join(recvB, `${xferId}.bin`);
  await waitFor(() => fs.existsSync(landed), 120_000, `B 的接收目录出现 ${xferId}.bin（只认 rename 后的最终名）`);
  const bytes = fs.readFileSync(landed);
  check("B 侧字节数与发送端一致", bytes.length === FILE_BYTES, FILE_BYTES, bytes.length);
  const got = createHash("sha256").update(bytes).digest("hex");
  check("B 侧 sha256 与源文件一致（INV-P17 分片可验证）", got === srcSha, srcSha.slice(0, 12) + "…", got.slice(0, 12) + "…");

  const aDb = openDb(INSTANCES[0].db, true);
  const left = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId).c;
  const aRow = aDb.prepare("SELECT status,progress FROM file_transfers WHERE id=?1").get(xferId);
  aDb.close();
  check("A 侧 file_outbox 行已被收尾删除", left === 0, 0, left);
  // 发送侧生命周期：pending → active → **sent（只是「我写完了 socket」）→ done（对端 FileCompleteAck 之后）**。
  // 钉 done 而不是 sent，正是总指令那条「不要因 TCP write 成功就认为已送达」的机器形状：
  // 只要本端写完就写 sent 就红，必须等对端确认才绿。接收侧终态同样是 done。
  check("A 侧 send 记录终态是 done（对端确认过，不是「我写完了 socket」的 sent）",
    aRow && aRow.status === "done", "done", aRow ? aRow.status : "无行");
  check("A 侧 send 记录进度到位", aRow && aRow.progress === 1.0, 1, aRow?.progress);

  const bDb = openDb(INSTANCES[1].db, true);
  const bRow = bDb.prepare("SELECT status,path FROM file_transfers WHERE id=?1").get(xferId);
  const bDup = bDb.prepare("SELECT COUNT(*) c FROM file_transfers WHERE id=?1").get(xferId).c;
  bDb.close();
  check("B 侧 receive 记录终态是 done（不是 active/failed）", bRow && bRow.status === "done", "done", bRow ? bRow.status : "无行");
  check("B 侧同一条传输只记一次", bDup === 1, 1, bDup);
  const strays = fs.existsSync(recvB)
    ? fs.readdirSync(recvB).filter((f) => f.includes(xferId) && f !== `${xferId}.bin`)
    : [];
  check("接收目录没有 .part / 改名副本残留", strays.length === 0, 0, strays.join(", ") || 0);
});

// §十四要的「错误行为测试」+ §七/§八的「文件 hash 不一致 / .part 已存在」：
// 坏内容必须要么被拒收、要么被补齐成正确字节 —— 但绝不允许"报成功却没有正确文件"。
if (GROUP) {
  step("群聊判据：三条各只落一行、明文要真解得开、撤回只物化不删行、Ack 必须把队列清干净", async () => {
    // 反向模式在这里翻的**只有判据读的 id**（预置、信封、投递全都一模一样）：
    // 真投递已经完成，却拿必定不存在的 id 去比 ⇒ 红只能来自断言本身，不来自基础设施噪声。
    const flip = (h) => h.slice(0, -1) + (h.endsWith("0") ? "1" : "0");
    const want = (id) => (GROUP_LIE ? flip(id) : id);
    const convId = `group:${GROUP_ID}`;
    // 前提断言（等三条都到齐）：没有它，下面几条会因为"链路根本没跑"而集体假绿
    await waitFor(() => {
      const db = openDb(INSTANCES[1].db, true);
      try {
        return [gTextId, gRecallId, gText2Id]
          .every((id) => db.prepare("SELECT COUNT(*) c FROM messages WHERE msg_id=?1").get(id).c > 0);
      } finally { db.close(); }
    }, 60_000, "B 侧三条群消息到齐（建链后 flush_group_outbox 送达）");
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
    const leakedIntoOneToOne = bDb.prepare(
      "SELECT COUNT(*) c FROM messages WHERE conv_id!=?1 AND msg_id IN (?2,?3,?4)",
    ).get(convId, gTextId, gRecallId, gText2Id).c;
    bDb.close();
    const aDb = openDb(INSTANCES[0].db, true);
    const stillQueued = aDb
      .prepare("SELECT COUNT(*) c FROM group_outbox WHERE group_id=?1").get(GROUP_ID).c;
    const aStatus = aDb
      .prepare("SELECT status FROM messages WHERE conv_id=?1").all(convId).map((r) => r.status);
    const aT1 = aDb.prepare("SELECT kind FROM messages WHERE msg_id=?1").get(gTextId);
    aDb.close();

    check("B 侧三条群消息各恰好一行（同 msg_id 多行=重复投递，少行=丢）",
      t1.length === 1 && rec.length === 1 && t2.length === 1,
      "1/1/1", `${t1.length}/${rec.length}/${t2.length}`);
    check("落库的会话必须是群会话（conv_id 带 group: 前缀，且 conversations.kind='group'）",
      t2.length === 1 && t2[0].conv_id === convId && t2[0].conv_kind === "group",
      `${convId} / group`, `${t2[0]?.conv_id} / ${t2[0]?.conv_kind}`);
    check("未被撤回那条的正文必须是**解密后的明文**（拿到密文或空串都说明没真解密）",
      t2.length === 1 && t2[0].content === "second group message",
      "second group message", JSON.stringify(t2[0]?.content));
    check("发送方必须是 A 的 runtimeId、seq 必须照信封给（seq 是排序权威）",
      t2.length === 1 && t2[0].sender_id === idA.runtimeId && t2[0].seq === 3,
      `${idA.runtimeId} / seq=3`, `${t2[0]?.sender_id} / seq=${t2[0]?.seq}`);
    check("B 侧终态必须是 delivered（收到即记，不等 Ack 回传）",
      t2.length === 1 && t2[0].status === "delivered", "delivered", t2[0]?.status);
    check("撤回必须把目标**物化**成 kind=recalled + 空正文，而且**不许删行**（G-Set 语义）",
      t1.length === 1 && t1[0].kind === "recalled" && t1[0].content === "",
      "行还在 + kind=recalled + content=''",
      `${t1.length} 行 / ${t1[0]?.kind} / content=${JSON.stringify(t1[0]?.content)}`);
    check("撤回事件自身也必须留一行（历史只增不减，不许被当成一次性通知丢掉）",
      rec.length === 1, 1, rec.length);
    check("三条群消息都不许串进 1:1 会话（两条管道共用同一对实例时的串味检查）",
      leakedIntoOneToOne === 0, 0, leakedIntoOneToOne);
    check("A 侧这个群的 group_outbox 必须被 GroupAck 清空（还留着=只发不认，重启会二次投递）",
      stillQueued === 0, 0, stillQueued);
    check("A 侧三条群气泡都不许被判成 failed（failed 的唯一裁决不能被群路径绕过）",
      aStatus.length === 3 && !aStatus.includes("failed"),
      "3 行且无 failed", JSON.stringify(aStatus));
    check("A 侧那条正文此刻仍是 text —— ⚠️ **实测出来的边界**：harness 写的是入队形状，"
      + "没有执行产品的撤回命令 ⇒ 发送侧本地物化不在这一格的证明范围里",
      aT1?.kind === "text", "text（本格的已知边界）", aT1?.kind);
  });
}

if (POISON) {
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

if (RESUME) {
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
    const aDb = openDb(INSTANCES[0].db, true);
    const a3 = aDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId3);
    const q3 = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId3).c;
    aDb.close();
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

if (KILL) {
  step("故障注入判据③：接收中被 SIGKILL ⇒ 不许假成功，重启后按盘上真实字节续完", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const partPath = path.join(dl, `${xferId4}.part`);
    const landed = path.join(dl, `${xferId4}.bin`);
    const partSize = () => {
      try {
        return fs.statSync(partPath).size;
      } catch {
        return 0;
      }
    };
    // 这一单**由我在判据里才入队**：前两版都在停机时预置，于是传输发生在"起 A/B 等链路"
    // 那一步里，等判据去看时早传完了 —— 打空的两轮报红全是我的时序问题，不是产品的。
    // ⚠️ 进程活着时写它的库是新用法：seed() 末尾的 wal_checkpoint(TRUNCATE) 撞上在写的
    //    连接会 SQLITE_BUSY ⇒ 重试几次；真进不去就该换成"停机入队 + 大文件"那条路。
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(xferId4);
          db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          ).run(xferId4, peerTo, srcFile4, `${xferId4}.bin`, KILL_BYTES, nowMs());
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }
    // waitFor 是 500ms 粒度，而实测一次 100 MB 传输只有 ~0.78s ⇒ 会打空。这里用 50ms 自旋。
    // 打没打中窗口是**这条注入自己**的成败，必须红给看，不许悄悄当成"已通过"。
    const inFlight = () => {
      const n = partSize();
      return n > 0 && n < KILL_BYTES;
    };
    let miss = "";
    const until = nowMs() + 120_000;
    while (nowMs() < until && !inFlight()) await sleep(50);
    if (!inFlight()) {
      miss =
        `120s 内没出现"在飞"的 .part（实际 ${partSize()} 字节）—— ` +
        `要么 A 没有在飞行中把这单捡起来，要么传得太快/太大没抓着`;
    }
    partAtKill = partSize();
    const pB = procs.get(INSTANCES[1].n);
    // ⚠️ 被信号杀死的子进程：`exitCode === null` + `signalCode === "SIGKILL"`。
    //    拿 exitCode !== null 判"死了没有"永远等不到（第一版就在这里超时）。
    const dead = () => !!pB && (pB.exitCode !== null || pB.signalCode !== null);
    if (!dead()) pB.kill("SIGKILL");
    await waitFor(dead, 15_000, "B 进程确认已死（SIGKILL 不给它收尾的机会）");
    await sleep(2_000); // 让 A 把"写失败了"变成状态
    const landedAtKill = fs.existsSync(landed);

    check("窗口必须真打中：杀的那一刻 .part 在 0~全量之间",
      !miss && partAtKill > 0 && partAtKill < KILL_BYTES,
      `0 < .part < ${KILL_BYTES}`, miss || `${partAtKill} 字节`);
    check("rename 才算完成：B 死在半路时接收目录不许出现终名文件",
      !landedAtKill, "不存在", landedAtKill ? "已出现" : "不存在");
    const aMid = openDb(INSTANCES[0].db, true);
    const a4mid = aMid.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId4);
    aMid.close();
    check("发送端不许在对端没确认时宣布完成：对端被杀的那一刻 A 不能是 done",
      a4mid?.status !== "done", "非 done", a4mid?.status ?? "无行");

    // 重启 B：链路该自己回来、outbox 该自己重投，且必须**接着盘上那点字节**发
    launch(INSTANCES[1]);
    await waitFor(() => tcpOpen(INSTANCES[1].port), 60_000, `重启后的 B 的 TCP ${INSTANCES[1].port} 可连`);
    await waitFor(() => bootReady(INSTANCES[1].log, bootBaseOf.get(INSTANCES[1].n), BOOT_LINE), 30_000,
      "重启后的 B 打出 boot 完成行");
    await waitFor(() => fs.existsSync(landed) || partSize() > partAtKill, 120_000,
      "重启后这一单被重新拾起（.part 比死时更长，或终名文件出现）");
    await sleep(15_000); // 让续传 / rename / 多轮重试都落定

    const wantFrom = LIE ? partAtKill + 1 : partAtKill;
    const aLog = tailLog(INSTANCES[0].log, 60000) || "";
    const line = aLog.split("\n").filter((l) => l.includes(xferId4) && l.includes("接收端已有")).pop() || "";
    const m = line.match(/接收端已有 (\d+) 字节/);
    check("重启后必须从**盘上真实字节数**续发，不许从 0 重灌（接收端真实进度优先）",
      !!m && Number(m[1]) === wantFrom, String(wantFrom),
      m ? m[1] : "A 日志里没有针对这一单的续发行");
    const exists4 = fs.existsSync(landed);
    const got4 = exists4 ? createHash("sha256").update(fs.readFileSync(landed)).digest("hex") : null;
    const wantSha4 = LIE ? LIE_SHA : srcSha4;
    check("死前写的前缀 + 重启后续发的尾段 = 源文件（sha256 逐字节对得上）",
      exists4 && got4 === wantSha4, wantSha4.slice(0, 12) + "…",
      exists4 ? got4.slice(0, 12) + "…" : "未落地");
    const bDb4 = openDb(INSTANCES[1].db, true);
    const b4 = bDb4.prepare("SELECT status FROM file_transfers WHERE id=?1").all(xferId4);
    bDb4.close();
    const aDb4 = openDb(INSTANCES[0].db, true);
    const a4 = aDb4.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId4);
    const q4 = aDb4.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId4).c;
    aDb4.close();
    const both4 = `B=${b4.map((r) => r.status).join("/") || "无行"} A=${a4?.status ?? "无行"} outbox=${q4}`;
    check("恢复的终局只有一个：两侧 done 且 outbox 已清（不许停在中间态，也不许弃单）",
      b4.length === 1 && b4[0].status === "done" && a4?.status === "done" && q4 === 0,
      "1 行 + 双侧 done + outbox=0", both4);
    const kept4 = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(xferId4)) : [];
    check("续完之后接收目录只剩 1 个终名文件：无 .part 残留、无半截副本",
      kept4.length === 1 && kept4[0] === `${xferId4}.bin`, `${xferId4}.bin`, kept4.join(", ") || "空");
  });
}

if (FREEZE) {
  step("故障注入判据④：对端失联（进程被冻住）⇒ 失联期间不许假成功，对端回来必须自己补齐", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId5 = `e2e-f-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
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
            ).run(xferId5, peerTo, srcFile5, `${xferId5}.bin`, FREEZE_BYTES, nowMs());
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
    const aDb = openDb(INSTANCES[0].db, true);
    const a5 = aDb.prepare("SELECT status FROM file_transfers WHERE id=?1").get(xferId5);
    const q5 = aDb.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1").get(xferId5).c;
    aDb.close();
    const both5 = `B=${b5.map((r) => r.status).join("/") || "无行"} A=${a5?.status ?? "无行"} outbox=${q5}`;
    check("对端解冻后必须自己补到终态：两侧 done 且 outbox 已清（不许停在中间态、不许弃单）",
      b5.length === 1 && b5[0].status === "done" && a5?.status === "done" && q5 === 0,
      "1 行 + 双侧 done + outbox=0", both5);
    const kept5 = fs.existsSync(dl) ? fs.readdirSync(dl).filter((f) => f.includes(xferId5)) : [];
    check("解冻之后不许留下第二次成功的痕迹",
      kept5.length === 1 && kept5[0] === `${xferId5}.bin`, `${xferId5}.bin`, kept5.join(", ") || "空");
  });
}

if (DISK) {
  step("故障注入判据⑤：接收目录写不进去 ⇒ 必须明确失败并止步，不许假 done、不许无限重试", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId6 = `e2e-g-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
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
            ).run(xferId6, peerTo, srcFile6, `${xferId6}.bin`, DISK_BYTES, nowMs());
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

if (ROT) {
  step("故障注入判据⑧：预置可写 .part 后把接收目录改成只读 ⇒ 半路失败不许被当成完成（rename 才算完成）", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId8 = `e2e-i-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
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
            ).run(xferId8, peerTo, srcFile8, name8, ROT_BYTES, nowMs());
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

if (SHRINK) {
  step("故障注入判据⑥：入队后源文件被改小 ⇒ 只许按磁盘上那份真值收发，两侧终态一致", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const srcDir = path.join(RUN_DIR, "src");
    fs.mkdirSync(srcDir, { recursive: true });
    xferId7 = `e2e-h-${ISO}-${Math.random().toString(36).slice(2, 8)}`;
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
              .get(peerTo).s;
            db.prepare(
              `INSERT INTO messages(msg_id,conv_id,sender_id,receiver_id,kind,content,ts,seq,status)
               VALUES(?1,?2,?3,?4,'file',?5,?6,?7,'sent')`,
            ).run(`file-${xferId7}`, peerTo, idA.runtimeId, peerTo,
              JSON.stringify({ name: name7, path: srcFile7, size: SHRINK_BYTES, sha256: "", subtype: "file" }),
              ts, seq);
            db.prepare(
              `INSERT INTO file_transfers(id,peer_id,name,size,direction,status,path,progress,created_at)
               VALUES(?1,?2,?3,?4,'send','pending',?5,0,?6)`,
            ).run(xferId7, peerTo, name7, SHRINK_BYTES, srcFile7, ts);
            db.prepare(
              `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
               VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
            ).run(xferId7, peerTo, srcFile7, name7, SHRINK_BYTES, ts);
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

if (MULTI) {
  step("故障注入判据⑦：三个文件一起排队、其中两个同名 ⇒ 一张都不许丢、内容不许串味", async () => {
    const dl = path.join(RUN_DIR, "recv", "B");
    fs.mkdirSync(dl, { recursive: true });
    const token = Math.random().toString(36).slice(2, 8);
    const photoName = `photo-${token}.bin`;
    const noteName = `note-${token}.bin`;
    // 同名的两张必须放在**不同目录**（同一路径放不下两个文件）：offer 的 name 取自路径的
    // file_name（`file.rs:358`），所以"同名不同内容"只能这样造。
    const layout = [
      { dir: "p1", name: photoName, bytes: 1024 * 1024, fill: 0xa1 },
      { dir: "p2", name: photoName, bytes: 64 * 1024, fill: 0xb2 },
      { dir: "p1", name: noteName, bytes: 256 * 1024, fill: 0xc3 },
    ];
    multiSpec = layout.map((l, i) => {
      const d = path.join(RUN_DIR, "src", l.dir);
      fs.mkdirSync(d, { recursive: true });
      const src = path.join(d, l.name);
      fs.writeFileSync(src, Buffer.alloc(l.bytes, l.fill));
      return {
        tid: `e2e-m-${ISO}-${token}-${i}`,
        src,
        name: l.name,
        bytes: l.bytes,
        sha: createHash("sha256").update(fs.readFileSync(src)).digest("hex"),
      };
    });
    // lie：注入完全一样，只把**其中一张**的期望摘要换掉 ⇒ 那条多重集合判据必须红。
    // 用集合而不是"随便挑一张比对"，就是为了验证"每份内容各自对上了"，不是"对上了三份里的任意一份"。
    const wantShas = multiSpec.map((s) => s.sha).sort();
    if (LIE) wantShas[0] = LIE_SHA;
    for (let i = 0; ; i++) {
      try {
        seed(INSTANCES[0].db, (db) => {
          const ins = db.prepare(
            `INSERT INTO file_outbox(transfer_id,peer_id,group_id,local_path,name,size,status,attempts,next_attempt_at,created_at)
             VALUES(?1,?2,NULL,?3,?4,?5,'pending',0,0,?6)`,
          );
          for (const s of multiSpec) {
            db.prepare("DELETE FROM file_outbox WHERE transfer_id=?1").run(s.tid);
            ins.run(s.tid, peerTo, s.src, s.name, s.bytes, nowMs());
          }
        });
        break;
      } catch (e) {
        if (i >= 5) throw e;
        await sleep(300);
      }
    }
    const t0 = nowMs();
    let snap = null;
    await waitFor(() => {
      const aDb = openDb(INSTANCES[0].db, true);
      const rows = aDb
        .prepare(`SELECT id,status FROM file_transfers WHERE id IN (${multiSpec.map(() => "?").join(",")})`)
        .all(...multiSpec.map((s) => s.tid));
      const queued = aDb
        .prepare(`SELECT COUNT(*) c FROM file_outbox WHERE transfer_id IN (${multiSpec.map(() => "?").join(",")})`)
        .get(...multiSpec.map((s) => s.tid)).c;
      aDb.close();
      const landed = fs.readdirSync(dl).filter((f) => f.includes(token));
      const parts = landed.filter((f) => f.endsWith(".part"));
      const files = landed.filter((f) => !f.endsWith(".part"));
      if (rows.length === multiSpec.length && rows.every((r) => r.status === "done")
        && queued === 0 && files.length === multiSpec.length) {
        snap = { rows, queued, landed, parts };
      }
      return !!snap;
    }, 180_000, "三单（含两张同名）要在同一批里全部投递完成");
    console.log(
      `  · 实测：入队 3 单 → 全部终态 ${((nowMs() - t0) / 1000).toFixed(1)}s · `
      + `落地 ${JSON.stringify(snap.landed.sort())}`,
    );
    const files = snap.landed.filter((f) => !f.endsWith(".part"));
    const gotShas = files
      .map((f) => createHash("sha256").update(fs.readFileSync(path.join(dl, f))).digest("hex"))
      .sort();
    const photoLanded = files.filter((f) => f.includes(photoName.replace(".bin", "")));
    check("一张都不许丢：三单必须各自落到一行 done 且队列已清空",
      snap.rows.length === multiSpec.length && snap.rows.every((r) => r.status === "done")
      && snap.queued === 0,
      "3 行 done + outbox=0", JSON.stringify(snap.rows) + ` outbox=${snap.queued}`);
    check("同名不许互相覆盖：接收目录里这张名字必须出现两次（少一次就是静默丢数据）",
      photoLanded.length === 2, 2, `${photoLanded.length} → ${photoLanded.join(", ")}`);
    check("每一张都必须是完整、各自对得上的内容（不许交错、不许串味、不许被顶掉）",
      gotShas.join(",") === wantShas.join(","),
      multiSpec.map((s) => s.sha.slice(0, 8)).join(","), gotShas.map((h) => h.slice(0, 8)).join(","));
    check("不许留下 .part 半成品（串行里每一单都得收尾）",
      snap.parts.length === 0, "无 .part", snap.parts.join(", ") || "无");
    const bDb = openDb(INSTANCES[1].db, true);
    const bRows = bDb
      .prepare(`SELECT id,status,size FROM file_transfers WHERE id IN (${multiSpec.map(() => "?").join(",")})`)
      .all(...multiSpec.map((s) => s.tid));
    bDb.close();
    check("接收侧每一单各记一行、都是 done（不重复记账、不把三单并成一条）",
      bRows.length === 3 && bRows.every((r) => r.status === "done"),
      "3 行 done", JSON.stringify(bRows));
    check("接收侧每单记的字节数必须等于它自己那张源（串味在这里也会露出来）",
      bRows.every((r) => multiSpec.some((s) => s.tid === r.id && s.bytes === r.size)),
      "逐单相等", JSON.stringify(bRows.map((r) => `${r.id.slice(-1)}=${r.size}`))
      + ` 期望 ${JSON.stringify(multiSpec.map((s) => s.bytes))}`);
  });
}

step("L-B 故障注入：两端重启后仍正确", async () => {
  await stopAll();
  await bootAndStop("重启");
  const bDb = openDb(INSTANCES[1].db, true);
  const rows = bDb.prepare("SELECT * FROM messages WHERE msg_id=?1").all(msgId);
  bDb.close();
  check("重启后 B 侧仍只有一条、内容不变",
    rows.length === 1 && rows[0].content === "hello from harness", 1, rows.length);
  const aDb = openDb(INSTANCES[0].db, true);
  const again = aDb.prepare("SELECT COUNT(*) c FROM outbox WHERE msg_id=?1").get(msgId).c;
  aDb.close();
  check("重启不复活已 Ack 的 outbox 行（不二次投递）", again === 0, 0, again);
});

// ── 主流程 ─────────────────────────────────────────────────────────
function writeReport(failed) {
  fs.mkdirSync(RUN_DIR, { recursive: true });
  for (const i of INSTANCES) {
    try { fs.copyFileSync(i.log, path.join(RUN_DIR, `instance-${i.label}.app.log`)); } catch { /* 还没生成 */ }
    for (const suffix of ["", "-wal", "-shm"]) {
      const src = i.db + suffix;
      if (fs.existsSync(src)) {
        try { fs.copyFileSync(src, path.join(RUN_DIR, `sqlite-${i.label}`, path.basename(src))); } catch { /* 占用中 */ }
      }
    }
  }
  const totalS = +(steps.reduce((n, s) => n + (s.ms ?? 0), 0) / 1000).toFixed(1);
  const sum = {
    run: ISO,
    // 只要账本里有一条红，报告就不许写 PASS —— 判据不许由调用方口头声明
    verdict: failed || anyFail() ? "FAIL" : "PASS",
    negative: NEGATIVE,
    binary: BIN, platform: process.platform, duration_s: totalS,
    instances: INSTANCES.map((i) => ({ label: i.label, n: i.n, port: i.port, runtimeId: (i.n === 1 ? idA : idB)?.runtimeId })),
    trace: { msg_id: msgId ?? null, transfer_id: xferId ?? null },
    // §十六要的「步骤 + 耗时 + 日志关联」：把闭包剔掉，只留事实
    steps: steps.map(({ fn, ...rest }) => rest),
    assertions,
  };
  fs.writeFileSync(path.join(RUN_DIR, "summary.json"), JSON.stringify(sum, null, 2));
  const esc = (x) => String(x ?? "").replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
  const stepRows = steps.map((s) =>
    `<tr><td>${s.verdict === "FAIL" ? "❌" : s.verdict === "NO-ASSERT" ? "⚠️" : "✅"}</td><td>${esc(s.name)}</td><td>${((s.ms ?? 0) / 1000).toFixed(1)}s</td><td>${s.checks ?? 0}</td></tr>`).join("\n");
  const logEx = steps.filter((s) => s.logs && Object.keys(s.logs).length)
    .map((s) => `<h4>❌ ${esc(s.name)} —— 日志里带这条 trace 的行</h4>` +
      Object.entries(s.logs).map(([label, lines]) =>
        `<p><code>instance-${label}.app.log</code></p><pre>${esc(lines.join("\n")) || "（没有匹配行 —— 说明这一侧根本没见过这条 trace）"}</pre>`).join("\n")).join("\n");
  const tr = assertions.map((a) =>
    `<tr><td>${a.verdict === "PASS" ? "✅" : "❌"}</td><td>${esc(a.step)}</td><td>${esc(a.name)}</td><td><code>${esc(a.expect)}</code></td><td><code>${esc(a.actual)}</code></td></tr>`).join("\n");
  fs.writeFileSync(path.join(RUN_DIR, "summary.html"),
`<!doctype html><meta charset=utf-8><title>Gosslan 多实例 E2E ${ISO}</title>
<body style="font:14px/1.6 system-ui;margin:32px;max-width:1100px">
<h1>${sum.verdict === "PASS" ? "✅ PASS" : "❌ FAIL"} — 双实例 E2E ${ISO}${NEGATIVE ? " · 反向自证" : ""}</h1>
<p>二进制 <code>${BIN}</code> · ${process.platform} · 总耗时 ${totalS}s · msg_id <code>${msgId ?? "-"}</code> · transfer_id <code>${xferId ?? "-"}</code></p>
<p>实例：${sum.instances.map((i) => `${i.label}=#${i.n} :${i.port} <code>${i.runtimeId ?? "-"}</code>`).join(" · ")}</p>
<h2>步骤</h2>
<table border=1 cellpadding=6 cellspacing=0 width=100%>
<tr><th></th><th>步骤</th><th>耗时</th><th>断言数</th></tr>
${stepRows}
</table>
<h2>断言</h2>
<table border=1 cellpadding=6 cellspacing=0 width=100%>
<tr><th></th><th>所属步骤</th><th>断言</th><th>预期</th><th>实际</th></tr>
${tr}
</table>
${logEx ? `<h2>失败步骤的日志关联</h2>${logEx}` : ""}
<p style="color:#666">日志/DB 快照在本目录：<code>instance-*.app.log</code> · <code>sqlite-*/</code> · <code>recv/</code> · <code>after-*.db</code></p>
<p style="color:#666">⚠️ 标 ⚠️ NO-ASSERT 的步骤只靠「超时即抛」把关，本身没下断言 —— 覆盖度按红字算，不按步骤数算。</p>`);
  console.log(`\n报告：${RUN_DIR}/summary.html`);
}

const backups = new Map();
try {
  fs.mkdirSync(RUN_DIR, { recursive: true });
  for (const i of INSTANCES) {
    if (fs.existsSync(i.db)) {
      const to = path.join(RUN_DIR, `backup-${i.label}`, path.basename(i.db));
      fs.mkdirSync(path.dirname(to), { recursive: true });
      fs.renameSync(i.db, to);
      backups.set(i.db, to);
      for (const s of ["-wal", "-shm"]) {
        if (fs.existsSync(i.db + s)) fs.renameSync(i.db + s, to + s);
      }
    }
  }
  console.log(`双实例 E2E · run ${ISO}\n  二进制 ${BIN}\n  appdata ${APPDATA}`);
  await bootAndStop("首启（生成身份密钥）");
  [idA, idB] = INSTANCES.map(readIdentity);
  NODES = INSTANCES.map((inst, i) => ({ ...[idA, idB][i], label: inst.label, port: inst.port }));
  console.log(`  A=${idA.runtimeId}\n  B=${idB.runtimeId}`);
  for (let i = 0; i < steps.length; i++) await runStep(i, steps[i]);
  // 总判据一律从断言账本推，不从「有没有抛异常」推。
  if (NEGATIVE) {
    const bad = assertions.filter((a) => a.verdict === "FAIL");
    const delivered = bad.some((a) => a.stepIdx > linkStepIdx());
    const linkBroke = steps.slice(0, linkStepIdx() + 1).some((s) => s.verdict === "FAIL");
    if (delivered && !linkBroke) {
      console.log("\n✅ 反向自证通过：链路成立而投递断言如期报红 —— 红不来自基础设施噪声");
      writeReport(true);
      process.exitCode = 0;
    } else {
      console.error(`\n✗ 反向自证不算通过：${
        linkBroke ? "红落在「建链」之前，那只是基础设施坏了"
                  : "送给一个不存在的对端却全绿 —— 这些断言不依赖真实投递，是空转"}`);
      writeReport(true);
      process.exitCode = 1;
    }
  } else {
    const bad = assertions.filter((a) => a.verdict === "FAIL");
    if (bad.length) {
      console.error(`\n✗ ${bad.length}/${assertions.length} 条断言报红：\n` +
        bad.map((a) => `  · [${a.step}] ${a.name} —— 预期 ${a.expect} / 实际 ${a.actual}`).join("\n"));
      writeReport(true);
      process.exitCode = 1;
    } else {
      writeReport(false);
      console.log(`\n✅ 多实例 E2E 全绿（${assertions.length} 条断言）`);
    }
  }
} catch (e) {
  // 反向模式：红必须落在「建链之后」才算自证成立 —— 否则「应用根本没起来」
  // 也会被判成「断言不依赖基础设施噪声」，这条自证就白写了。
  const deliveredFail = NEGATIVE && curStepIdx > linkStepIdx();
  if (deliveredFail) {
    console.log(`\n✅ 反向自证通过：链路正常但送不到时，投递断言如期报红 —— ${e.message}`);
    writeReport(true);
    process.exitCode = 0;
  } else {
    console.error(NEGATIVE
      ? `\n✗ 反向自证不算通过：红落在「建链」那一步之前，只是基础设施坏了 —— ${e.message}`
      : `\n✗ ${e.message}`);
    writeReport(true);
    process.exitCode = 1;
  }
} finally {
  await stopAll().catch((e) => console.error("清理失败：", e.message));
  for (const i of INSTANCES) {
    try { fs.copyFileSync(i.db, path.join(RUN_DIR, `after-${i.label}.db`)); } catch { /* 没有 */ }
  }
  // 用户原来的库必须回来 —— 覆盖掉本轮写出来的测试库
  for (const [dbFile, from] of backups) {
    for (const s of ["", "-wal", "-shm"]) {
      if (fs.existsSync(from + s)) fs.renameSync(from + s, dbFile + s);
      else if (fs.existsSync(dbFile + s)) fs.rmSync(dbFile + s);
    }
  }
  if (backups.size) console.log(`已还原用户原有实例库 ${backups.size} 个`);
}
