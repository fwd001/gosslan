#!/usr/bin/env node
/**
 * 中转服务器的自测（零依赖，`node selftest.mjs` 直接跑）。
 *
 * 它验的是这台机器的**四条承诺**，不是 Gosslan 协议：
 *   1. 同一通道哈希的两条连接被拼成双向字节管道，字节严格保序、不丢不重；
 *   2. 配对之前就已到达的字节不会被吞（首行和首帧挤在同一个 TCP 包里的真实情况）；
 *   3. token 不对 / 通道哈希格式不对 ⇒ 直接拒，且**服务器 stdout 里不出现任何载荷字节**
 *      —— 这是"读不到信息"的可证伪版本：载荷一旦被解析或打印，这里立刻失败；
 *   4. 任一端断开 ⇒ 两端一起干净退出；等待超时 ⇒ 不泄漏槽位。
 *
 * 跑法：TOKEN=… node selftest.mjs   （退出码 0 = 全部通过）
 */

import { spawn } from 'node:child_process';
import net from 'node:net';
import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';

const TOKEN = 'selftest-token-please-ignore';
const PORT = 25993 + (process.pid % 500);

const child = spawn(process.execPath, [new URL('./server.mjs', import.meta.url).pathname], {
  env: {
    ...process.env,
    TOKEN,
    PORT: String(PORT),
    HOST: '127.0.0.1',
    // 把三个超时压到自测尺度（默认值是给生产用的）
    HELLO_TIMEOUT_MS: '1500',
    WAIT_TIMEOUT_MS: '2000',
    IDLE_TIMEOUT_MS: '3000',
    STATS: '0',
  },
  stdio: ['ignore', 'pipe', 'pipe'],
});

let out = '';
child.stdout.on('data', (b) => (out += b.toString()));
child.stderr.on('data', (b) => (out += b.toString()));

const ch = (seed) => seed.repeat(64).slice(0, 64); // 任意 64 位十六进制串

function connect() {
  return new Promise((resolve, reject) => {
    const s = net.connect(PORT, '127.0.0.1', () => resolve(s));
    s.on('error', reject);
  });
}

function collect(sock) {
  const parts = [];
  sock.on('data', (b) => parts.push(b));
  return () => Buffer.concat(parts);
}

const results = [];
async function test(name, fn) {
  try {
    await fn();
    results.push(['PASS', name]);
    console.log(`  ✓ ${name}`);
  } catch (e) {
    results.push(['FAIL', name]);
    console.log(`  ✗ ${name}\n    ${e.message}`);
  }
}

await new Promise((r) => child.stdout.once('data', r));

// ── 1 + 2：配对、双向、以及"首行同包到达的后续字节" ─────────────────────────
await test('两条同通道连接拼成双向管道，且首包内跟随的字节不丢', async () => {
  const A = await connect();
  const B = await connect();
  const gotA = collect(A);
  const gotB = collect(B);

  // B 先把「首行 + 紧跟的 2 字节」塞进同一个 write —— 服务端必须在配对后按序吐出。
  // 这不是边角情况：客户端拿到配对确认后立刻开讲，两个方向会在同一个包里。
  B.write(`GSRL1 ${TOKEN} ${ch('b')}\nXQ`);
  await sleep(120);
  A.write(`GSRL1 ${TOKEN} ${ch('b')}\n`);

  await sleep(200);
  assert.equal(gotA().toString(), 'XQ', '配对前排队的字节必须在配对后原样送到对端');

  A.write(Buffer.from('ping-from-A'));
  await sleep(120);
  assert.equal(gotB().toString(), 'ping-from-A');

  B.write(Buffer.from('pong'));
  await sleep(120);
  assert.equal(gotA().toString(), 'XQpong', '两个方向都严格按到达顺序');
  A.destroy();
  B.destroy();
});

// ── 3a：token 错 ⇒ 拒 ────────────────────────────────────────────────────
await test('token 不对的连接被拒绝，不会被配对', async () => {
  const A = await connect();
  const B = await connect();
  const gotB = collect(B);
  B.write(`GSRL1 ${TOKEN} ${ch('c')}\n`);
  await sleep(120);
  A.write(`GSRL1 wrong-token ${ch('c')}\n`);
  await assertRejected(A);
  await sleep(150);
  assert.equal(gotB().length, 0, '被拒的一侧不得把任何字节送到对端');
  B.destroy();
});

// ── 3b：载荷不出现在服务器输出里 ──────────────────────────────────────────
await test('服务器 stdout 里不出现任何电路载荷字节', async () => {
  const marker = 'TOP-SECRET-PAYLOAD-MUST-NOT-APPEAR';
  const A = await connect();
  const B = await connect();
  A.write(`GSRL1 ${TOKEN} ${ch('d')}\n`);
  await sleep(80);
  B.write(`GSRL1 ${TOKEN} ${ch('d')}\n`);
  await sleep(120);
  A.write(Buffer.from(marker));
  await sleep(400);
  assert.ok(!out.includes(marker), '服务器把载荷打进了日志 ⇒ 它就不是"只搬字节"了');
  A.destroy();
  B.destroy();
});

// ── 3c：通道哈希格式非法 ⇒ 拒 ─────────────────────────────────────────────
await test('通道哈希格式非法被拒绝（不给脏数据留槽位）', async () => {
  const A = await connect();
  A.write(`GSRL1 ${TOKEN} deadbeef\n`);
  await assertRejected(A);
});

// ── 4a：一端断开 ⇒ 两端一起退出 ───────────────────────────────────────────
await test('一端断开时两端同时退出', async () => {
  const A = await connect();
  const B = await connect();
  A.write(`GSRL1 ${TOKEN} ${ch('e')}\n`);
  await sleep(80);
  B.write(`GSRL1 ${TOKEN} ${ch('e')}\n`);
  await sleep(150);
  const closed = new Promise((r) => B.once('close', r));
  A.destroy();
  await closed;
});

// ── 4b：等待超时不泄漏 ────────────────────────────────────────────────────
await test('等不到对端时自己退出，且槽位可被后来者复用', async () => {
  const A = await connect();
  A.write(`GSRL1 ${TOKEN} ${ch('f')}\n`);
  const closed = new Promise((r) => A.once('close', r));
  await closed; // WAIT_TIMEOUT_MS = 2s
  const C = await connect();
  const D = await connect();
  const gotD = collect(D);
  C.write(`GSRL1 ${TOKEN} ${ch('f')}\n`);
  await sleep(80);
  D.write(`GSRL1 ${TOKEN} ${ch('f')}\n`);
  await sleep(120);
  C.write(Buffer.from('reuse-ok'));
  await sleep(120);
  assert.equal(gotD().toString(), 'reuse-ok', '超时后同一通道必须能重新配对');
  C.destroy();
  D.destroy();
});

// ── 4c：空闲电路被回收 ────────────────────────────────────────────────────
await test('长期无字节的电路被空闲回收', async () => {
  const A = await connect();
  const B = await connect();
  A.write(`GSRL1 ${TOKEN} ${ch('7')}\n`);
  await sleep(80);
  B.write(`GSRL1 ${TOKEN} ${ch('7')}\n`);
  const closed = new Promise((r) => B.once('close', r));
  await closed; // IDLE_TIMEOUT_MS = 3s（两端此后不再发任何字节）
  A.destroy();
});

// ── 5：首行不说话的连接被超时掐掉 ─────────────────────────────────────────
await test('连上不发首行的连接被超时掐掉', async () => {
  const A = await connect();
  await assertRejected(A); // HELLO_TIMEOUT_MS = 1.5s
});

child.kill('SIGTERM');
await sleep(200);
if (child.exitCode === null) child.kill('SIGKILL');

const failed = results.filter(([r]) => r === 'FAIL');
console.log(`\n${results.length - failed.length}/${results.length} 通过`);
if (failed.length) {
  console.log('未通过：', failed.map(([, n]) => n).join(' | '));
  process.exit(1);
}

function assertRejected(sock) {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error('连接没有被拒绝，仍然开着')), 4000);
    sock.once('close', () => {
      clearTimeout(t);
      resolve();
    });
    sock.once('error', () => {
      clearTimeout(t);
      resolve();
    });
  });
}
