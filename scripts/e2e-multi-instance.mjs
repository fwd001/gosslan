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
//   --fault=recv-readonly     → 注入⑤：接收目录只读（磁盘写不进去）⇒ 必须明确失败并止步，不许假 done、不许无限重试
//   --fault=recv-readonly-lie → 同样的注入，只把"该落到哪个终态"换成 done ⇒ 预期报红
//   （每轮各几条断言**不在这里写**：`check-doc-numbers.mjs` 从下面的 check(" 调用点现算，
//    写进文档时要以「<轮次名> N 断言」的形式，否则不会被对账）
//   （kill 轮的文件尺寸用 E2E_KILL_MB 调，默认 100 —— 回环上实测 ~0.78s 走完，窗口够打）
//   （freeze 轮用 E2E_FREEZE_S 选调结长度，默认 30：**<45s** 是"链路还活着、只是没回执"，
//    **>45s** 越过 watchdog（健康阈值 15s×3）⇒ 真的拆链 + 重拨 + 重试，两种都是同一组结局判据）

const NEGATIVE = process.argv.includes("--negative");
/// 故障注入模式（§八）。`--fault=poison-part` 见下方 preset 步骤的注释。
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
/// `poison-part-lie` = 这组判据自己的**非空转证明**：注入完全一样，只把比对用的期望摘要
/// 换成一个必定不相等的值。产品没坏 ⇒ 判据必须报红；报不出红 ⇒ 那几条断言读的不是真字节。
const LIE = FAULT.endsWith("-lie");
const LIE_SHA = "0".repeat(64);
/// 传输尺寸（默认 1 MB，`E2E_FILE_MB=N` 覆盖）。这个旋钮不是为了测"大文件"本身，
/// 而是先量出**一次传输在回环上真实耗时多久**：「接收中杀进程」这类注入能不能做成
/// 非竞态，取决于窗口有没有那么长。量不出来就老实标 SIMULATED，不许伪装 PASS（§十）。
const FILE_BYTES = Number(process.env.E2E_FILE_MB || 1) * 1024 * 1024;

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createPrivateKey } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

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
const tailLog = (file, n = 400) =>
  fs.existsSync(file) ? fs.readFileSync(file, "utf8").split("\n").slice(-n).join("\n") : "";

// PKCS8 定长前缀 + 32 字节种子 → 导出公钥（Node 原生支持 X25519 / Ed25519）
const PKCS8 = { x25519: "302e020100300506032b656e04220420", ed25519: "302e020100300506032b657004220420" };
function pubFromSecret(kind, b64secret) {
  const der = Buffer.concat([Buffer.from(PKCS8[kind], "hex"), Buffer.from(b64secret, "base64")]);
  const jwk = createPrivateKey({ key: der, format: "der", type: "pkcs8" }).export({ format: "jwk" });
  return Buffer.from(jwk.x, "base64url").toString("base64"); // 应用侧统一标准 base64
}

// ── 生命周期 ───────────────────────────────────────────────────────
const procs = new Map();
function launch(inst) {
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
  for (const i of INSTANCES) launch(i);
  for (const i of INSTANCES) {
    await waitFor(() => tcpOpen(i.port), 60_000, `${what}：实例 ${i.label} 的 TCP ${i.port} 可连`);
    await waitFor(() => tailLog(i.log, 60).includes("AppState::init 完成"), 20_000,
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
    await waitFor(() => (tailLog(INSTANCES[1].log, 60) || "").includes("AppState::init 完成"), 30_000,
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
