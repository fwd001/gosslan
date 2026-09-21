#!/usr/bin/env node
/**
 * 中转服务器的容量实测（零依赖，`node bench.mjs [对端数]`）。
 *
 * 要回答的是两个具体说法，不是"看起来能跑"：
 *   ① 30 人规模（这里用「同时存在的电路数」建模）下，一问一答的**往返延迟**是多少；
 *   ② 服务器进程的 **RSS 内存**随电路数怎么涨（"占用非常少"要么给出数字，要么别这么说）。
 *
 * 混合两种真实帧形：聊天帧（~300B）与文件分片帧（~16KB，MAX_CHUNK 量级）。
 * 输出 p50/p99/max + RSS。本机回环测的是这台 Node 进程本身，不含公网 RTT ——
 * 所以真实体感还要叠加一次互联网往返（见 README 的解读方式）。
 */

import { spawn, execFile } from 'node:child_process';
import net from 'node:net';
import { setTimeout as sleep } from 'node:timers/promises';

const PAIRS = Number(process.argv[2] ?? 15); // 15 对 = 30 个客户端各连一条
const ROUNDS = 200;
const TOKEN = 'bench-token';
const PORT = 26993 + (process.pid % 300);

const child = spawn(process.execPath, [new URL('./server.mjs', import.meta.url).pathname], {
  env: { ...process.env, TOKEN, PORT: String(PORT), HOST: '127.0.0.1', STATS: '0', LOG: '0' },
  stdio: ['ignore', 'pipe', 'pipe'],
});
child.stdout.on('data', () => {});
child.stderr.on('data', () => {});

const connect = () =>
  new Promise((res, rej) => {
    const s = net.connect(PORT, '127.0.0.1', () => res(s));
    s.on('error', rej);
  });

function circuit(idx) {
  const ch = idx.toString(16).padStart(64, '0');
  const mk = () =>
    connect().then((s) => {
      s.setNoDelay(true);
      s.write(`GSRL1 ${TOKEN} ${ch}\n`);
      return s;
    });
  return Promise.all([mk(), mk()]).then(([a, b]) => [a, b]);
}

await sleep(400);
const circuits = [];
for (let i = 0; i < PAIRS; i++) circuits.push(await circuit(i));
await sleep(200);

const samples = [];
let wireBytes = 0;
/** 等到 sock 上累计到达 want 字节（回环上 16KB 可能被拆成多块，按 once('data') 会串味）。 */
function waitForBytes(sock, want) {
  return new Promise((resolve) => {
    let got = 0;
    const onData = (b) => {
      got += b.length;
      if (got >= want) {
        sock.removeListener('data', onData);
        resolve();
      }
    };
    sock.on('data', onData);
  });
}

async function pingPong(srv, cli, payload) {
  const t0 = process.hrtime.bigint();
  wireBytes += 2 * payload.length; // 去程 + 回程，各过中继一次
  const one = waitForBytes(srv, payload.length);
  cli.write(payload);
  await one;
  const back = waitForBytes(cli, payload.length);
  srv.write(payload);
  await back;
  return Number(process.hrtime.bigint() - t0) / 1e6; // ms
}

const small = Buffer.alloc(300, 0x61);
const big = Buffer.alloc(16 * 1024, 0x62);
const t0 = Date.now();
for (let r = 0; r < ROUNDS; r++) {
  await Promise.all(
    circuits.map(async ([a, b], i) => {
      const p = (r + i) % 10 === 0 ? big : small;
      samples.push(await pingPong(a, b, p));
    }),
  );
}
const wall = (Date.now() - t0) / 1000;

samples.sort((x, y) => x - y);
const pct = (q) => samples[Math.min(samples.length - 1, Math.floor(samples.length * q))];
const frames = ROUNDS * PAIRS; // 一次往返 = 2 帧，但样本数就是往返数

// RSS：从 ps 取（macOS / Linux 同形）
const rss = await new Promise((res) => {
  execFile('ps', ['-o', 'rss=', '-p', String(child.pid)], (e, o) =>
    res(e ? -1 : Number(o.trim())),
  );
});

console.log(`电路数=${PAIRS}（模拟 ${PAIRS * 2} 个客户端各一条中继链路）`);
console.log(
  `往返样本=${samples.length}（${frames} 次往返） 总耗时=${wall.toFixed(1)}s ` +
    `中继搬运=${(wireBytes / 1024 / 1024).toFixed(1)}MiB ≈ ${(wireBytes / wall / 1024 / 1024).toFixed(2)}MiB/s`,
);
console.log(`单跳往返延迟 p50=${pct(0.5).toFixed(2)}ms p99=${pct(0.99).toFixed(2)}ms max=${samples[samples.length - 1].toFixed(2)}ms`);
console.log(`服务器 RSS=${rss} KiB（含 Node 运行时本身）`);

child.kill('SIGKILL');
