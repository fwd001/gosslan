#!/usr/bin/env node
// 职责边界：
// - 双实例 E2E harness 的引擎层：跑起来要用的常量、共享量，以及被复用的工具函数。
// 为什么搬出来：主文件 4,845 行里 26 个 `if (FLAG) {…}` 轮次块占 3,157 行 —— 要把轮次块按族拆走，
//   前提是轮次块与驱动脚本拿到的是同一份共享量与工具 ⇒ 先有这一个家。
// 「只搬不改」在这里是可证的：搬的段全是模块级声明（本来就在第 0 列），唯一改动是行首加 `export `；
//   例外只有一处、且是必须的：位置相关量 ROOT 按新目录重算（见该处注释）。
// ⚠️ 三条硬约束（都是本轮实测踩出来的，别绕过）：
//   · 会被别处赋值的标量不许搬 —— ESM 给 import 绑定赋值 = Assignment to constant variable；
//   · core 不许引用留在主文件的声明（链接期不报，跑起来才炸）；
//   · 本文件不许有顶层副作用（起进程 / 建目录 / 发请求）—— 驱动脚本 import 它就会立刻执行。
import { spawn, spawnSync } from "node:child_process";
import { createHash, createCipheriv, createPrivateKey, randomBytes, randomUUID, sign } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { BOOT_LINE, bootBaseline, bootReady, countLog, readLogTail, selfcheckLogtail, stashLogs } from "../e2e-logtail.mjs";
import { captureShot, captureWindowShot, describeShotDir, screenBlockedReason, selfcheckShot } from "../e2e-shot.mjs";

export const S = {
  curStep: null, curStepIdx: -1, stashSeq: 0,
  /// 两端身份与在途单：写在驱动的停机预置里、读在**各轮次分册**里 ⇒ 只能放对象字段
  /// （ESM 不给 import 绑定赋值）。名字与原来的 let 声明一致，一个都不改。
  idA: null, idB: null, msgId: null, peerTo: null, xferId: null, srcFile: null, srcSha: null,
};

// ── 轮次配置：argv/env ⇒ 开关与常量（驱动与轮次分册共用同一份，别在第二处重新解析）──

/// 故障注入模式（§八）。`--fault=poison-part` 见驱动脚本 e2e-multi-instance.mjs 里那段 preset 步骤的注释。
/// 另有**旅程轮** `--round=`（不是注入，是补一整条没测过的用户路径）：
///   --round=group    → 群聊这一族跨实例真跑：两端预置群 → A 排三条群消息（正文/撤回/正文）→
///                      对端上线后靠 flush_group_outbox 补发 → 判落库/解密/清队列/G-Set/不串味
///   --round=group-lie→ 预置与投递完全不动，只把判据读的 msg_id 换成不存在的值 ⇒ 预期按设计报红
///   --round=group-targets-lie→ 同上但**只摘掉线上明文里的 mention_targets 键** ⇒
///                       预期恰好"落点穿过管道"那一条红（`group-lie` 够不到它，因为它读真 id）
///   断言条数不在这里写，由 check-doc-numbers 现算对账（同每一轮）。
///   --round=gfile    → 群文件跨实例（A 只备货、B 上线后生产自己投）；--round=gfile-lie → 只翻判据读的那份摘要
///   断言条数不在这里写，由 check-doc-numbers 现算对账（同每一轮）。
export const FAULT = (process.argv.find((a) => a.startsWith("--fault=")) || "").slice("--fault=".length);

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
export const ROUND = (process.argv.find((a) => a.startsWith("--round=")) || "").slice("--round=".length);

export const GROUP_ID = "g-e2e-harness";

export const GROUP_NAME = "E2E-Group";

/// 群对称密钥：settings 表 `gk:{group_id}` = base64 的**正好 32 字节**
/// （transport.rs:5984-5986 解码后 `try_into::<[u8;32]>()`，长度不对直接 None ⇒ 永不解密）。
/// 先例：`e2e_peer.rs:62` 的 `GROUP_KEY_B64` 就是同一形状。
export const GROUP_KEY_B64 = Buffer.alloc(32);

for (let i = 0; i < 32; i += 4) GROUP_KEY_B64.writeUInt32BE(0x6e00_0000 + i, i);

export const GROUP_KEY_STR = GROUP_KEY_B64.toString("base64");


/// `poison-part-lie` = 这组判据自己的**非空转证明**：注入完全一样，只把比对用的期望摘要
/// 换成一个必定不相等的值。产品没坏 ⇒ 判据必须报红；报不出红 ⇒ 那几条断言读的不是真字节。
export const LIE = FAULT.endsWith("-lie");

export const LIE_SHA = "0".repeat(64);

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
export const SIZE_ARG = process.argv.find((a) => a.startsWith("--size="));

export const FILE_MB = SIZE_ARG
  ? Number(SIZE_ARG.slice("--size=".length))
  : Number(process.env.E2E_FILE_MB || 1);

export const FILE_BYTES = Math.round(FILE_MB * 1024 * 1024);
/// 反证开关（只喂给 `seedPair`）：清掉两端的手动 Routed 端点 ⇒ 那条 Routed 判据必须报红。
/// 它改的是**判据读的那个输入**（配了什么端点），不是判据本身。
export const NO_ROUTED = process.env.E2E_NO_ROUTED === "1";
/// `GossipEngine` 的 ttl（`GossipEngine::new(bloom, lru, fanout, ttl)` 第四参，见
/// gossip_engine.rs:244 那组测试的形状）；转发每跳减一，写 0 会让对端直接丢。
export const GROUP_TTL = 6;
// ⚠️ 位置相关量：本文件在 scripts/e2e/ 下，ROOT 要退两层；不改这行会静默指到 scripts/，
//    实测 pre-flight 找 scripts/src-tauri/target/release/gosslan 并以「没有二进制」拒跑。
export const ROOT = path.resolve(import.meta.dirname, "..", "..");
if (!fs.existsSync(path.join(ROOT, "package.json"))) throw new Error(`core.mjs 的 ROOT 指错了：${ROOT}`);
export const ISO = new Date().toISOString().replace(/[:.]/g, "-");
/// §十六「本轮每一单都要数得出来」：所有 trace id 一律从这里铸，铸出来就登记。
/// 为什么非要注册表而不是最后把变量名抄一遍 —— 同一个坑 #70 踩过：手写名单在注入轮出现后
/// 悄悄漏掉那些单，报告的 trace 指着别的一单，而没有任何一格会红。
export const MINTED_IDS = [];
export function eid(prefix = "", tail = Math.random().toString(36).slice(2, 8)) {
  const id = `e2e-${prefix ? `${prefix}-` : ""}${ISO}-${tail}`;
  MINTED_IDS.push(id);
  return id;
}
/// 反向轮会拿真 id 拼一个**故意不存在**的 decoy 去查库。它同样是"报告里出现过的本形状 id"，
/// 所以照样登记 —— 否则反向轮会因为报告契约不合格多红一次，把"红恰好落在断言上"那条证据搅浑。
export function noteId(id) {
  MINTED_IDS.push(id);
  return id;
}
export const RUN_DIR = path.join(ROOT, "test-results", `run-${ISO}`);
export function citedRunIds() {
  const listed = spawnSync("git", ["ls-files", "-z", "--", "*.md", "*.html"], { cwd: ROOT, encoding: "utf8" });
  if (listed.status !== 0) return null;
  const set = new Set();
  for (const rel of listed.stdout.split("\0").filter(Boolean)) {
    let txt;
    try {
      txt = fs.readFileSync(path.join(ROOT, rel), "utf8");
    } catch {
      continue; // 台账里点过名、文件已经不在了 ⇒ 不构成对现场的引用
    }
    for (const m of txt.matchAll(/run-2026-\d\d-\d\dT[\d-]+Z/g)) set.add(m[0]);
  }
  return set;
}
export function prunePlan({ runs, keep, negative, cited }) {
  if (negative) return [];
  if (!Number.isFinite(keep) || keep < 0) return [];
  if (!cited) return []; // fail-closed：引用名单拿不到 ⇒ 一个都不删（和"自证不过就不删"同一立场）
  const greens = runs
    .filter((r) => r.outcome === "green" && !cited.has(r.name)) // ★ 被文档点名的绿轮不占删除名额
    .sort((a, b) => (a.name < b.name ? 1 : -1)); // 目录名是 ISO 时间戳 ⇒ 字典序倒排 = 新的在前
  // 返回**按名字升序**（= 从最老的删起），让调用侧与自证都不依赖 sort 的方向
  return greens.slice(keep).map((r) => r.name).sort();
}
export function selfcheckPrune() {
  const fails = [];
  const eq = (name, got, want) => {
    const a = JSON.stringify(got), b = JSON.stringify(want);
    if (a !== b) fails.push(`${name}：预期 ${b} / 实际 ${a}`);
  };
  const mk = (tag, outcome) => (outcome ? { name: "run-" + tag, outcome } : { name: "run-" + tag });
  const greens = [mk("a", "green"), mk("b", "green"), mk("c", "green")]; // c 最新
  const NONE = new Set();
  eq("绿轮超上限 ⇒ 删最老的那几个", prunePlan({ runs: greens, keep: 1, negative: false, cited: NONE }), ["run-a", "run-b"]);
  eq("没超上限 ⇒ 一个都不删", prunePlan({ runs: greens, keep: 9, negative: false, cited: NONE }), []);
  eq("红轮永远保留（哪怕上限 0）", prunePlan({ runs: [mk("x", "green"), mk("y", "red")], keep: 0, negative: false, cited: NONE }), ["run-x"]);
  eq("跑不出 summary ⇒ 按红处理", prunePlan({ runs: [mk("z", "unknown")], keep: 0, negative: false, cited: NONE }), []);
  eq("反向模式 ⇒ 一趟都不删", prunePlan({ runs: greens, keep: 1, negative: true, cited: NONE }), []);
  eq("上限写坏了（NaN/负数）⇒ 不删", prunePlan({ runs: greens, keep: Number.NaN, negative: false, cited: NONE }), []);
  // ★ 这两格是一对：同一批输入，**只换"有没有被文档点名 / 名单拿不拿得到"这一个输入**。
  //   第一格红 ⇒ 证明"留下 a"是引用给的，不是排序巧合；第二格红 ⇒ 证明 fail-closed 那条真的关着门。
  eq("被文档点名的绿轮 ⇒ 哪怕超上限也不删", prunePlan({ runs: greens, keep: 1, negative: false, cited: new Set(["run-a"]) }), ["run-b"]);
  eq("引用名单拿不到（null）⇒ 一个都不删", prunePlan({ runs: greens, keep: 1, negative: false, cited: null }), []);
  return fails;
}
// ── 环境事实（不认识的平台直接退，不猜）────────────────────────────
export function appDataDir() {
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
export function binaryPath() {
  if (process.env.GOSSLAN_E2E_BIN) return process.env.GOSSLAN_E2E_BIN;
  const exe = process.platform === "win32" ? "gosslan.exe" : "gosslan";
  return path.join(ROOT, "src-tauri", "target", "release", exe);
}
export const APPDATA = appDataDir();
export const BIN = binaryPath();
export function newestMtime(dir, depth) {
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
export const TCP_BASE = 59992; // protocol.rs TCP_PORT；instance>0 时端口 = TCP_PORT + N*10
export const INSTANCES = [1, 2].map((n) => ({
  n,
  label: n === 1 ? "A" : "B",
  port: TCP_BASE + n * 10,
  db: path.join(APPDATA, `gosslan-${n}.db`),
  log: path.join(APPDATA, "logs", `gosslan-${n}.log`),
}));
/// 第三实例（只有 `--round=gossip3` 会真启动它）。产品侧对实例号没有上限，也没有特判：
/// 端口 = `TCP_PORT + instance*10`（state.rs:1282）、运行时身份 = `base-iN`（state.rs:1272）、
/// 库与日志各自一份（state.rs:1186/1207）—— 所以"第三个实例"不需要动产品码，只是把同一套隔离再套一份。
export const INST_C = {
  n: 3,
  label: "C",
  port: TCP_BASE + 3 * 10,
  db: path.join(APPDATA, "gosslan-3.db"),
  log: path.join(APPDATA, "logs", "gosslan-3.log"),
};
/// **可能被本轮真启动过**的全部实例。备份/还原、`after-*.db`、失败时的日志关联一律按这份清单走：
/// 少算一格 = 把本轮写出来的测试库留在用户 appdata 里，而且第三实例红的时候报告里没有它的现场。
export const ALL_INST = [...INSTANCES, INST_C];
// ── 小工具 ─────────────────────────────────────────────────────────
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const nowMs = () => Date.now();
export function openDb(file, readonly = false) {
  return new DatabaseSync(file, { readOnly: readonly });
}
export function seed(file, fn) {
  const db = openDb(file);
  try {
    fn(db);
    db.exec("PRAGMA wal_checkpoint(TRUNCATE);");
  } finally {
    db.close();
  }
}
export async function waitFor(cond, ms, what) {
  const until = nowMs() + ms;
  while (nowMs() < until) {
    if (await cond()) return true;
    await sleep(500);
  }
  throw new Error(`超时（${ms}ms）等 ${what}`);
}
/// 有界等**发送侧自己走到终态**，返回最后一次快照（不抛 —— 到点就把实际值交给 check 判红）。
///
/// 为什么需要它：**「B 的终名文件出现了」不是 A 的同步点**。A 的 `done` 与删 `file_outbox` 行
/// 发生在收到对端 `FileCompleteAck` 之后，而那条 ack 在接收端 rename **之后**才发
/// ⇒ 天然顺序是"先见 B 落地、后见 A 终态"。
/// 实测（2026-09-26 本地层第 12 步）：解冻→落地只 5.5s 的那一刻读 A ⇒ `{A:active, outbox:1}`
/// 被判成红，而同一步里 B 已 done、字节与 sha 全对、无 `.part` 残留 —— **红的是判据读早了，不是产品分叉**。
/// 窗口默认 60s：心跳 5s（transport.rs:1992）+ 文件队列退避 5s（file.rs:303）⇒ 该走得到的路最长也就几跳，
/// 60s 还停在 `active`/`pending` 才是真红（用户界面就是"永远转圈"）。
export async function waitSendTerminal(id, timeoutMs = 60_000) {
  const t0 = nowMs();
  let row = null;
  let queued = -1;
  // 句柄只开一次：这一条循环最多要读 240 次，若每轮都 `openDb()`，
  // 撞上 SQLITE_BUSY 的概率被放大两个量级，而那时抛的是"技术错误"不是断言红（读的是别人进程的库）。
  const db = openDb(INSTANCES[0].db, true);
  try {
    const qRow = db.prepare("SELECT status,progress FROM file_transfers WHERE id=?1");
    const qQ = db.prepare("SELECT COUNT(*) c FROM file_outbox WHERE transfer_id=?1");
    for (;;) {
      row = qRow.get(id) ?? null;
      queued = qQ.get(id).c;
      if (row && (row.status === "done" || row.status === "failed" || row.status === "cancelled")) break;
      if (nowMs() - t0 >= timeoutMs) break;
      await sleep(250);
    }
  } finally {
    db.close();
  }
  return { row, queued, waitedMs: nowMs() - t0 };
}
export function tcpOpen(port) {
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
export const tailLog = (file, n = 400) => readLogTail(file, n);
// PKCS8 定长前缀 + 32 字节种子 → 导出公钥（Node 原生支持 X25519 / Ed25519）
export const PKCS8 = { x25519: "302e020100300506032b656e04220420", ed25519: "302e020100300506032b657004220420" };
export function pubFromSecret(kind, b64secret) {
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
export function buildGroupEnvelope(o) {
  const iv = randomBytes(12);
  const c = createCipheriv("chacha20-poly1305", o.groupKey, iv, { authTagLength: 16 });
  // 明文形状必须与产品侧 `protocol::gossip_plaintext_with_targets` 逐字同形，
  // **包括 mentions / mention_targets 各自的三态**：
  // 键不存在 = 发这条的那个版本压根不认识这个字段；键存在（含空数组）= 发送方的权威回答。
  // 这一格要能在 harness 里分别造出这几种，所以由调用方"传不传 o.mentions / o.targets"决定。
  // ⚠️ 落点**不许在这里凭空补**：`Some([])` 与"缺键"是两件事，替调用方决定就等于把
  //    旧对端的形状悄悄改成了新对端的形状，那条三态判据会假绿。
  const plain = { kind: o.kind, content: o.content };
  if (o.mentions !== undefined) plain.mentions = o.mentions;
  if (o.targets !== undefined) plain.mention_targets = o.targets;
  const sealed = Buffer.concat([
    c.update(JSON.stringify(plain), "utf8"),
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
/// harness 侧的 `protocol::build_mention_targets` 镜像（protocol.rs 里那条同名函数）。
///
/// 为什么镜像而不直接写死一条数组：落点里的 `n` 是"@这个名字在正文里第几次出现"，
/// 它由**正文 + 名单顺序**算出来。写死 `n:1` 就等于"我以为产品会这么发"，
/// 而 #122 要修的正是"两个人同名"那种情况下第二个人该拿到 `n:2` ——
/// 只有照规则数出来的那份，才谈得上"穿过真实管道"。规则必须与 Rust 逐条同：
/// ① 查不到昵称的 id 不分配；② 昵称空的不分配；③ 正文里没有 `@<name>` 的不分配
/// （蒙一段别的文字上比留空更坏）；④ 同名按名单顺序递增；⑤ 封顶 64 条。
/// ⚠️ 刻意**不**用字符偏移：Rust 按字节、JS 按 UTF-16，一个 emoji 就能把两端错开。
export function buildMentionTargets(content, ids, nameOf) {
  const MAX_GOSSIP_MENTIONS = 64;
  const seen = new Map();
  const out = [];
  for (const id of ids) {
    const name = nameOf(id);
    if (!name) continue;
    const needle = `@${name}`;
    if (!content.includes(needle)) continue;
    const k = (seen.get(name) ?? 0) + 1;
    seen.set(name, k);
    if (k > MAX_GOSSIP_MENTIONS) continue;
    out.push({ id, name, n: k });
  }
  return out;
}
/// 从实例库里取回 ed25519 私钥对象（只有 harness 需要，产品侧从不导出私钥）。
export function ed25519Priv(inst) {
  const db = openDb(inst.db, true);
  try {
    const b64 = db.prepare("SELECT value FROM settings WHERE key='ed25519_secret'").get()?.value;
    if (!b64) throw new Error(`${inst.label} 库里没有 ed25519_secret`);
    const der = Buffer.concat([Buffer.from(PKCS8.ed25519, "hex"), Buffer.from(b64, "base64")]);
    return createPrivateKey({ key: der, format: "der", type: "pkcs8" });
  } finally { db.close(); }
}
// ── 生命周期 ───────────────────────────────────────────────────────
export const procs = new Map();
export const bootBaseOf = new Map();
export function launch(inst) {
  stashLogs(inst.log, RUN_DIR, `${inst.label}-${++S.stashSeq}`);
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
export async function stopAll() {
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
/// 只停**一个**实例（`#89` 那一轮要用：观察者必须一直活着，被观察的那台重启）。
/// ⚠️ 别用 `p.exitCode === null` 判"还活着"：**被信号杀死时 exitCode 就是 null**（第一次跑这条
///    把自己判成了"SIGKILL 之后还没退"）。要看的是 pid 还在不在，与 `stopAll` 末尾那个残留扫描同一口径。
export async function stopOne(inst) {
  const p = procs.get(inst.n);
  if (!p) throw new Error(`stopOne(${inst.label})：procs 里没有这个实例（它没被 launch 过？）`);
  const alive = () => {
    try { process.kill(p.pid, 0); return true; } catch { return false; }
  };
  if (alive()) p.kill("SIGTERM");
  await Promise.race([
    new Promise((r) => p.once("exit", r)),
    sleep(8000).then(() => { if (alive()) p.kill("SIGKILL"); }),
  ]);
  let gone = false;
  for (let i = 0; i < 40 && !gone; i++) {
    gone = !alive();
    if (!gone) await sleep(250);
  }
  procs.delete(inst.n);
  if (!gone) throw new Error(`stopOne(${inst.label})：SIGTERM + SIGKILL 之后 pid ${p.pid} 还在`);
}
export function procsLeft() {
  if (process.platform === "win32") {
    const exe = path.basename(BIN);
    const r = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${exe}`, "/FO", "CSV", "/NH"], { encoding: "utf8" });
    return ((r.stdout || "").includes(exe)) ? [exe] : [];
  }
  const r = spawnSync("pgrep", ["-f", `${BIN}`], { encoding: "utf8" });
  return (r.stdout || "").trim().split("\n").filter(Boolean);
}
export async function bootAndStop(what) {
  for (const i of INSTANCES) launch(i); // launch 内部会先给该实例清档，基线随之一并重置
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `${what}：实例 ${i.label} 的 TCP ${i.port} 可连`);
    await waitFor(() => bootReady(i.log, bootBaseOf.get(i.n), BOOT_LINE), 20_000,
      `${what}：实例 ${i.label} 打出 boot 完成行`);
  }
  await stopAll();
}
// ── 预置（L-A）─────────────────────────────────────────────────────
export function readIdentity(inst) {
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
export function seedPair(nodes) {
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
      // E2E_NO_ROUTED=1 把这条预置清空 —— 给下面那条 Routed 判据当"只换判据的输入"的反证：
      // 端点没配 ⇒ 那一格必须报红，其余格照常（LAN 广播还在，投递不该受影响）。
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('routed_endpoints',?1)")
        .run(NO_ROUTED ? "[]" : JSON.stringify([{ address: `127.0.0.1:${peer.port}` }]));
      // 陷阱：macOS 上 load() 优先信书签 ⇒ 只写路径，绝不写 downloads_dir_bookmark
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('downloads_dir',?1)").run(recv);
      db.prepare("INSERT OR REPLACE INTO settings(key,value) VALUES('lan_enabled','true')").run();
    });
  });
}
// ── 断言账本 ───────────────────────────────────────────────────────
export const steps = [];
export const shotFiles = [];
export const shotSources = [];
export function takeShot(tag, pid, note) {
  const w = pid ? captureWindowShot(RUN_DIR, tag, pid) : { ok: false, why: note ?? "没有可读的 pid" };
  if (w.ok) {
    shotFiles.push(w.file);
    shotSources.push({
      file: path.relative(RUN_DIR, w.file), source: "window", pid, window_id: w.windowId,
      bounds: `${w.bounds.w}x${w.bounds.h}`, png: `${w.png.w}x${w.png.h}`, scale: w.scale,
    });
    return;
  }
  const f = captureShot(RUN_DIR, tag);
  if (f) shotFiles.push(f);
  shotSources.push({
    file: f ? path.relative(RUN_DIR, f) : null, source: "screen", pid: pid ?? null, why: w.why,
  });
}
export const assertions = [];
export function step(name, fn) { steps.push({ name, fn }); }
export function check(name, pass, expect, actual) {
  // `undefined` 会被 JSON.stringify **整个键丢掉** —— 于是"读不到那一行"的断言一红，
  // summary.json 反而缺了 §十六 点名的「预期/实际」两栏（实测 run-2026-09-26T09-20-42-139Z：
  // 报告契约判红两条，理由正是"有条断言缺 预期/实际 之一"）。判红的那一格必须依然读得出预期与实际。
  const show = (v) => (v === undefined ? "<无此行/undefined>" : v);
  assertions.push({
    stepIdx: S.curStepIdx, step: S.curStep?.name ?? null,
    name, verdict: pass ? "PASS" : "FAIL", expect: show(expect), actual: show(actual),
  });
  console.log(`  ${pass ? "✅" : "❌"} ${name}${pass ? "" : `\n      预期 ${expect} / 实际 ${actual}`}`);
  return pass;
}
/// 失败时把两端日志里出现这条 trace 的行摘出来 —— §十六要的「日志关联」不是写个文件名，
/// 而是要能顺着 msg_id / transfer_id 直接看见对端说过什么。
// 失败时"沿链追踪"要看的是**本轮自己造的那些单**的日志。
// ⚠️ 以前这里手写四个变量名，于是注入轮（⑤⑥⑦⑧⑨⑩ 各自另造 id）的日志一行都摘不到，
// 而报告里 `transfer_id` 还指着同一轮里另一单（J2 那一单）⇒ 追踪会指到错的那一单。
// 实测锚点：run-2026-09-26T01-42-36-986Z —— 注入⑧ 的 `e2e-i-…-dqy5zx` 在 A 侧日志有 132 行，
// 报告摘出的行里含它 **0 行**，`trace.transfer_id` = `e2e-x-…`。
// 所有 id 都是 `e2e-…<本轮 ISO>-随机` 的形状 ⇒ 按形状匹配，新增轮次不必登记，也就不会再次漏。
export function traceExcerpt() {
  const re = new RegExp(`e2e-\\S*${ISO.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);
  const out = {};
  for (const i of ALL_INST) {
    const body = tailLog(i.log, 20000) || "";
    out[i.label] = body.split("\n").filter((l) => re.test(l)).slice(-8);
  }
  return out;
}
export async function runStep(i, s) {
  S.curStep = s;
  S.curStepIdx = i;
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
// §十六 报告契约：把「报告至少显示」那几条点名翻译成对**产物**的判据，不是对源码字面量的存在性检查。
// 判据只吃一个已经落盘的 summary.json —— 所以「改坏报告生成器」和「手工改坏一份报告」走的是同一条判据。
export function reportContractGaps(s) {
  /// 本轮铸出来的 trace id 的**形状**（`e2e-` + 可选单字母段 + 本轮 ISO + 尾巴）。
  /// 按形状认，不按变量名点名 —— 理由同 `traceExcerpt()`：手写名单会漏新轮次。
  /// 写成函数内的字面量而不是模块级 `const`：`selfcheckReportContract()` 在文件**第 82 行**就被调用，
  /// 模块级常量那时还没初始化（TDZ 会直接抛，第一次跑就把整层判据打挂）。
  const RUN_ID_RE = /e2e-(?:[a-z]-)?\d{4}-\d{2}-\d{2}T[\d-]+Z-[A-Za-z0-9-]+/g;
  const gaps = [];
  if (!s || typeof s !== "object") return ["报告不是一个对象"];
  if (!["PASS", "FAIL"].includes(s.verdict)) gaps.push("总 verdict 不是 PASS/FAIL");
  if (!s.trace || !("msg_id" in s.trace) || !("transfer_id" in s.trace))
    gaps.push("缺 trace 里的 msg_id / transfer_id（§十六 要求这两个 id 贯穿整轮）");
  // ★ §十六 的另一半：一轮里**每一单**都要有自己那条 trace。多文件轮同时有 3 个 transfer_id，
  // 只报一个标量就等于"报告说这轮只发过一单"。所以判：**报告里出现的每个本形状 id，都必须已登记**。
  // 不登记的那一格会在追故障时凭空消失 —— 而它恰好就是"报告指到错的那一单"的形状。
  if (!Array.isArray(s.trace?.ids)) {
    gaps.push("trace.ids 不是数组（本轮造过的每一单都要在报告里数得出来）");
  } else {
    const reg = new Set(s.trace.ids);
    const hay = JSON.stringify({ ...s, trace: { ...(s.trace ?? {}), ids: [] } });
    for (const id of new Set(hay.match(RUN_ID_RE) ?? [])) {
      if (!reg.has(id)) gaps.push(`报告里出现没登记进 trace.ids 的 id：${id}`);
    }
  }
  if (typeof s.duration_s !== "number") gaps.push("缺总耗时 duration_s");
  if (!Array.isArray(s.steps) || !s.steps.length) gaps.push("缺「步骤」表");
  for (const st of s.steps ?? []) {
    if (!st.name) gaps.push("有条步骤连名字都没有 —— 报告读不出这是哪个功能");
    if (!["PASS", "FAIL", "NO-ASSERT", "NOT-RUN"].includes(st.verdict))
      gaps.push(`步骤「${String(st.name ?? "?").slice(0, 24)}」没有终态 verdict`);
  }
  if (!Array.isArray(s.assertions) || !s.assertions.length) gaps.push("缺「预期/实际」账本（assertions 为空）");
  for (const a of s.assertions ?? []) {
    if (!a.step || !a.name || !("expect" in a) || !("actual" in a) || !["PASS", "FAIL"].includes(a.verdict)) {
      gaps.push(`有条断言缺 步骤/预期/实际/PASS-FAIL 之一：${JSON.stringify(a).slice(0, 90)}`);
      break;
    }
  }
  const failSteps = (s.steps ?? []).filter((x) => x.verdict === "FAIL");
  if (s.verdict === "FAIL" && failSteps.length && !failSteps.some((x) => x.logs && Object.keys(x.logs).length))
    gaps.push("报了 FAIL 却没有一步带「日志关联」—— §十六 要求失败能追到实例日志");
  return gaps;
}
export function readReportContract(dir) {
  const p = path.join(dir, "summary.json");
  if (!fs.existsSync(p)) return [`没有 ${p}`];
  if (!fs.existsSync(path.join(dir, "summary.html"))) return ["§十六 要求 summary.html，但没落盘"];
  let s;
  try { s = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { return [`summary.json 解析失败：${e.message}`]; }
  return reportContractGaps(s);
}
// 步骤徽章：以前是写在模板里的三元式，「没有 verdict」直接落到 else ⇒ 没跑的步骤显示成 ✅。
// 报告把没跑标成通过，和被它标成通过的那些格一起算进覆盖度 —— 这正是 §十 禁止的「把没有测试伪装成 PASS」。
export function stepBadge(v) {
  return v === "FAIL" ? "❌"
    : v === "NO-ASSERT" ? "⚠️"
    : v == null || v === "NOT-RUN" ? "⛔ 未跑"
    : "✅";
}
export function selfcheckReportContract() {
  const base = () => JSON.parse(JSON.stringify({
    verdict: "PASS", duration_s: 1.2,
    trace: { msg_id: "m1", transfer_id: "t1", ids: ["e2e-2026-01-01T00-00-00-000Z-aaaaaa"] },
    steps: [{ name: "跑通的一步", ms: 1, verdict: "PASS", checks: 1 }, { name: "没跑到的步骤", verdict: "NOT-RUN" }],
    assertions: [{ step: "跑通的一步", name: "判据", expect: 1, actual: "e2e-2026-01-01T00-00-00-000Z-aaaaaa", verdict: "PASS" }],
  }));
  const mut = (f) => { const c = base(); f(c); return c; };
  const cases = [
    ["真：字段齐全判得出合格", reportContractGaps(base()), 0],
    ["假：缺 msg_id 判得出", reportContractGaps(mut((c) => delete c.trace.msg_id)), 1],
    ["假：trace.ids 不是数组判得出", reportContractGaps(mut((c) => delete c.trace.ids)), 1],
    // ★ 这一条是"本轮有一单没被登记进 trace"的形状：断言里出现了本形状 id，但 trace.ids 里没有它。
    ["假：报告里出现没登记的 id 判得出",
      reportContractGaps(mut((c) => { c.trace.ids = []; })), 1],
    ["真：多单全部登记判得出合格",
      reportContractGaps(mut((c) => {
        c.trace.ids = ["e2e-2026-01-01T00-00-00-000Z-aaaaaa", "e2e-x-2026-01-01T00-00-00-000Z-bbbbbb"];
        c.assertions.push({ step: "跑通的一步", name: "第二单", expect: 1, actual: "e2e-x-2026-01-01T00-00-00-000Z-bbbbbb", verdict: "PASS" });
      })), 0],
    ["假：缺耗时判得出", reportContractGaps(mut((c) => delete c.duration_s)), 1],
    ["假：步骤没有终态判得出", reportContractGaps(mut((c) => delete c.steps[1].verdict)), 1],
    ["假：断言少了「实际值」判得出", reportContractGaps(mut((c) => delete c.assertions[0].actual)), 1],
    ["假：报 FAIL 却没日志关联判得出", reportContractGaps(mut((c) => { c.verdict = "FAIL"; c.steps[0].verdict = "FAIL"; })), 1],
    ["真：FAIL 且带日志关联判得出合格", reportContractGaps(mut((c) => {
      c.verdict = "FAIL"; c.steps[0].verdict = "FAIL"; c.steps[0].logs = { A: ["一行日志"] };
    })), 0],
    ["假：断言账本整体为空判得出", reportContractGaps(mut((c) => { c.assertions = []; })), 1],
  ];
  const fails = [];
  for (const [name, got, want] of cases)
    if (got.length !== want) fails.push(`${name} —— 期望判出 ${want} 条，实际 ${got.length} 条${got.length ? `（首条：${got[0].slice(0, 50)}）` : ""}`);
  const badges = [
    ["PASS 才是 ✅", stepBadge("PASS") === "✅"],
    ["没终态不许是 ✅", stepBadge(undefined) !== "✅"],
    ["NOT-RUN 要写明未跑", stepBadge("NOT-RUN").includes("未跑")],
  ];
  for (const [name, ok] of badges) if (!ok) fails.push(`步骤徽章：${name} 不成立`);
  return { fails, notes: `${cases.length + badges.length} 格` };
}
