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

const NEGATIVE = process.argv.includes("--negative");

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
    const headIso = spawnSync("git", ["log", "-1", "--format=%cI"],
      { cwd: ROOT, encoding: "utf8" }).stdout.trim();
    const headMs = Date.parse(headIso);
    if (!dirty && Number.isFinite(headMs) && binM > headMs) {
      console.warn(`⚠️ 有 .rs 的 mtime 比二进制新，但 src-tauri/src 与 HEAD 内容完全一致，`
        + `且二进制晚于 HEAD 提交 —— 判定为护栏写回造成的 mtime 抖动，继续测当前内容。`);
    } else {
      console.error(`✗ 二进制比源码旧（二进制 ${new Date(binM).toISOString()}，源码最新 ${new Date(newest).toISOString()}）`);
      console.error(`  src-tauri/src 未提交改动：${dirty ? "有 ⇒ 源码真的动过" : "无"}；二进制晚于 HEAD：${Number.isFinite(headMs) && binM > headMs}`);
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
  const ids = [msgId, xferId].filter(Boolean);
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
  const buf = Buffer.alloc(1024 * 1024);
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
  check("B 侧字节数与发送端一致", bytes.length === 1024 * 1024, 1024 * 1024, bytes.length);
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
