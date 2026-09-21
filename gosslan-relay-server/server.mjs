#!/usr/bin/env node
/**
 * Gosslan 公网中转（哑管道 / blind circuit relay）。
 *
 * ## 它只做一件事
 * 把两条「登记了同一个通道哈希」的 TCP 连接拼成一个双向字节管道，然后**再也不看内容**。
 * 不解析 Gosslan 帧、不解析 JSON、不落盘、不缓存消息：断开即丢，重启即清空
 * （全部状态就是下面一个 Map 加一个 Set，都在内存里）。
 *
 * ## 它为什么读不到消息
 * Gosslan 正文本来就是端到端加密的（X25519 + ChaCha20-Poly1305）。本服务器只搬字节，
 * 连"一条消息"的边界都不需要知道。电路建立后两端客户端之间还有一层记录层封装
 * （`src-tauri/src/transport/relay_seal.rs`，用好友已绑定的 Ed25519 公钥验签），
 * 所以本服务器**既读不到正文与元数据，也注入不进任何一帧**。
 *
 * ## 它仍然能看到什么（诚实边界，README 里同样写了）
 * 有连接进来、什么时候、持续多久、双向各搬了多少字节。通道哈希是 32 字节摘要，
 * 不知道两端公钥就还原不出设备 ID；但"同一串哈希再次出现"本身是可关联的。
 *
 * ## 协议（客户端发出的第一行，只有这一行）
 * ```text
 * GSRL1 <token> <64 位十六进制通道哈希>\n        （整行 ≤ 256 字节）
 * ```
 * 首行之后的字节一律原样转发，不做任何解析。
 *
 * 零依赖：只用 node:net + node:crypto，Node ≥ 18。
 */

import net from 'node:net';
import { createHash } from 'node:crypto';

const PORT = Number(envInt('PORT', 59993));
const HOST = process.env.HOST ?? '0.0.0.0';
const TOKEN = process.env.TOKEN ?? '';
/** 同时存活的客户端连接上限（含等待配对的）。30 人规模留了 4 倍余量。 */
const MAX_CLIENTS = Number(envInt('MAX_CLIENTS', 128));
/** 首行超时：连上却不说话的连接一律掐掉，不给它占坑的机会。 */
const HELLO_TIMEOUT_MS = Number(envInt('HELLO_TIMEOUT_MS', 10_000));
/** 等对端来配对的上限时间；超时放弃（客户端的 10s 循环会重来）。 */
const WAIT_TIMEOUT_MS = Number(envInt('WAIT_TIMEOUT_MS', 30_000));
/** 已配对电路的空闲上限：客户端心跳 5s、Presence 10s，180s 无字节即判死。 */
const IDLE_TIMEOUT_MS = Number(envInt('IDLE_TIMEOUT_MS', 180_000));
/** 等待阶段能替对端暂存的字节上限（正常远达不到，这是防单侧开讲的内存兜底）。 */
const PENDING_MAX = Number(envInt('PENDING_MAX', 8 * 1024 * 1024));
/** 是否打印事件行（不含任何载荷）。 */
const LOG_EVENTS = process.env.LOG !== '0';

const MAX_LINE = 256;
const MAGIC = 'GSRL1';
const LF = 0x0a;

if (!TOKEN) {
  // 没有 token 的中继就是一个开放代理：任何人都能拿它转发任意字节。
  // 这不该是"配置宽松一点"的事，所以拒绝启动而不是警告（公网机器上后果很实在）。
  console.error('[gosslan-relay] 必须设置 TOKEN 环境变量，否则不启动。');
  console.error('  例：TOKEN=$(openssl rand -hex 16) node server.mjs');
  process.exit(2);
}

/** channel hex → 正在等对端的连接（每个通道最多一个等待者）。 */
const waiting = new Map();
/** 已配对电路。 */
const circuits = new Set();
const stats = { accepted: 0, paired: 0, rejected: 0, bytes: 0, clients: 0 };

const log = (...a) => LOG_EVENTS && console.log(new Date().toISOString(), ...a);

function envInt(name, dflt) {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return String(dflt);
  const n = Number(raw);
  if (!Number.isFinite(n) || n <= 0) {
    console.error(`[gosslan-relay] ${name}=${raw} 不是正整数，改用 ${dflt}`);
    return String(dflt);
  }
  return raw;
}

/** token 定长比较（两侧先各过一遍 SHA-256 拉成等长，避免按字节早退的时间差）。 */
function tokenOk(given) {
  const a = createHash('sha256').update(given, 'utf8').digest();
  const b = createHash('sha256').update(TOKEN, 'utf8').digest();
  return a.equals(b);
}

function clearTimer(which, c) {
  if (c[which]) {
    clearTimeout(c[which]);
    c[which] = null;
  }
}

/** 首行非法或超限：留一行原因、断开。载荷一个字节都不碰。 */
function reject(c, why) {
  stats.rejected++;
  log(`[reject] ${why}`);
  clearTimer('helloTimer', c);
  if (c.ch && waiting.get(c.ch) === c) waiting.delete(c.ch);
  if (!c.sock.destroyed) c.sock.destroy();
}

/** 把两条连接拼成电路；先把等待期暂存的字节按序冲出，再切到转发态。 */
function pair(a, b) {
  clearTimer('waitTimer', a);
  waiting.delete(a.ch);
  const circuit = { a, b, up: 0, down: 0, last: Date.now() };
  circuits.add(circuit);
  stats.paired++;
  for (const [x, y] of [[a, b], [b, a]]) {
    x.phase = 'paired';
    x.other = y;
    x.circuit = circuit;
  }
  log(`[circuit] 配对 ch=${a.ch.slice(0, 8)}… 电路数=${circuits.size}`);
  for (const [from, to] of [[a, b], [b, a]]) {
    if (from.pending.length) {
      const buf = Buffer.concat(from.pending);
      from.pending = [];
      forward(from, to, buf);
    }
  }
}

function forward(from, to, buf) {
  const circuit = from.circuit;
  if (to.sock.destroyed || !to.sock.writable) {
    closeCircuit(circuit, 'peer-gone');
    return;
  }
  if (circuit) {
    circuit.last = Date.now();
    if (circuit.a === from) circuit.up += buf.length;
    else circuit.down += buf.length;
  }
  stats.bytes += buf.length;
  if (!to.sock.write(buf)) {
    // 对端写不动 ⇒ 暂停真正的源头，等 drain 再恢复。不排队、不丢弃，
    // 所以内存上限就是两端各自的 TCP 缓冲，与文件大小无关。
    from.sock.pause();
    to.sock.once('drain', () => {
      if (!from.sock.destroyed) from.sock.resume();
    });
  }
}

function closeCircuit(circuit, why) {
  if (!circuit || circuit.dead) return;
  circuit.dead = true;
  circuits.delete(circuit);
  log(
    `[circuit] 结束 ch=${circuit.a.ch.slice(0, 8)}… ` +
      `上行=${circuit.up}B 下行=${circuit.down}B 原因=${why}`,
  );
  for (const c of [circuit.a, circuit.b]) dropConn(c);
}

/** 单条连接的退出：摘等待槽、断 socket。计数只减一次。 */
function dropConn(c) {
  clearTimer('waitTimer', c);
  clearTimer('helloTimer', c);
  if (c.ch && waiting.get(c.ch) === c) waiting.delete(c.ch);
  c.circuit = null;
  if (!c.countedDown) {
    c.countedDown = true;
    stats.clients--;
  }
  if (!c.sock.destroyed) c.sock.destroy();
}

const server = net.createServer((sock) => {
  stats.accepted++;
  if (stats.clients >= MAX_CLIENTS) {
    stats.rejected++;
    log('[reject] 连接数已达上限');
    sock.destroy();
    return;
  }
  stats.clients++;
  sock.setNoDelay(true); // 长度前缀的小帧撞上 Nagle + 延迟确认 = 聊天发涩
  sock.setKeepAlive(true, 30_000);

  const c = {
    sock,
    phase: 'hello',
    head: [],
    headLen: 0,
    pending: [],
    pendingLen: 0,
    ch: null,
    other: null,
    circuit: null,
    waitTimer: null,
    countedDown: false,
    helloTimer: setTimeout(() => {
      if (c.phase === 'hello') reject(c, '首行超时');
    }, HELLO_TIMEOUT_MS),
  };

  // 一个监听器负责三个阶段，靠 phase 分支 —— 避免「摘了首行的监听器、忘了装转发的」
  // 这类配对竞态：首行、等待期字节、转发期字节可能挤在同一个 tick 里到达。
  sock.on('data', (chunk) => {
    if (c.phase === 'hello') return onHello(c, chunk);
    if (c.phase === 'waiting') {
      c.pending.push(chunk);
      c.pendingLen += chunk.length;
      if (c.pendingLen > PENDING_MAX) reject(c, '等待期暂存超限');
      return;
    }
    forward(c, c.other, chunk);
  });
  const gone = (why) => (c.circuit ? closeCircuit(c.circuit, why) : dropConn(c));
  sock.on('end', () => gone('eof'));
  sock.on('close', () => gone('closed'));
  sock.on('error', () => gone('io-error'));
});

/** 首行阶段：按 Buffer 累积，找到 \n 才解码头部（按字符串切会把多字节字符切断）。 */
function onHello(c, chunk) {
  const nl = chunk.indexOf(LF);
  if (nl < 0) {
    c.head.push(chunk);
    c.headLen += chunk.length;
    if (c.headLen > MAX_LINE) reject(c, '首行超长');
    return;
  }
  c.head.push(chunk.subarray(0, nl));
  const head = Buffer.concat(c.head).toString('utf8').replace(/\r$/, '');
  c.head = [];
  // 同一 chunk 里首行之后的字节属于电路载荷，必须留到配对后按序写出。
  const rest = chunk.subarray(nl + 1);
  if (rest.length) {
    c.pending.push(rest);
    c.pendingLen += rest.length;
  }
  clearTimer('helloTimer', c);
  acceptHello(c, head);
}

function acceptHello(c, head) {
  const parts = head.split(' ');
  const [magic, token, ch] = parts;
  const ok =
    parts.length === 3 &&
    magic === MAGIC &&
    /^[0-9a-f]{64}$/.test(ch ?? '') &&
    tokenOk(token ?? '');
  if (!ok) {
    reject(c, '首行非法（协议版本 / token / 通道哈希格式）');
    return;
  }
  c.ch = ch;
  const peer = waiting.get(ch);
  if (peer && peer !== c && !peer.sock.destroyed && peer.sock.writable) {
    pair(peer, c);
    return;
  }
  if (peer && peer !== c) {
    // 旧等待者已不健康：让位给本连接，旧的由自己的超时清理。
    clearTimer('waitTimer', peer);
    waiting.delete(ch);
  }
  c.phase = 'waiting';
  c.waitedSince = Date.now();
  waiting.set(ch, c);
  c.waitTimer = setTimeout(() => {
    if (waiting.get(c.ch) === c) {
      waiting.delete(c.ch);
      log(`[wait] 对端未出现，放弃 ch=${c.ch.slice(0, 8)}…`);
      dropConn(c);
    }
  }, WAIT_TIMEOUT_MS);
}

// 空闲清理：半开电路、卡死的等待槽。
setInterval(() => {
  const now = Date.now();
  for (const circuit of [...circuits]) {
    if (now - circuit.last > IDLE_TIMEOUT_MS) closeCircuit(circuit, 'idle');
  }
  for (const [ch, c] of [...waiting]) {
    if (now - (c.waitedSince ?? now) > WAIT_TIMEOUT_MS) {
      waiting.delete(ch);
      dropConn(c);
    }
  }
}, Math.min(30_000, IDLE_TIMEOUT_MS)).unref();

if (process.env.STATS !== '0') {
  setInterval(() => {
    log(
      `[stats] 客户端=${stats.clients} 电路=${circuits.size} 等待槽=${waiting.size}｜` +
        `累计 接入=${stats.accepted} 配对=${stats.paired} 拒绝=${stats.rejected} ` +
        `搬运=${Math.round(stats.bytes / 1024)}KiB`,
    );
  }, 60_000).unref();
}

server.listen(PORT, HOST, () => {
  log(
    `[gosslan-relay] 监听 ${HOST}:${PORT}｜上限 ${MAX_CLIENTS} 连接｜` +
      `首行 ${HELLO_TIMEOUT_MS}ms｜等待 ${WAIT_TIMEOUT_MS}ms｜空闲 ${IDLE_TIMEOUT_MS}ms`,
  );
  log('[gosslan-relay] 不解析载荷、不写任何文件；进程退出即清空全部状态。');
});

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    log(`[gosslan-relay] ${sig}，清理后退出`);
    for (const circuit of [...circuits]) closeCircuit(circuit, 'shutdown');
    for (const c of [...waiting.values()]) dropConn(c);
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(0), 2_000).unref();
  });
}
