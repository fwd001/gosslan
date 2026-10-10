#!/usr/bin/env node
/**
 * 量具八的**采集端**：把 roadmap N23 那一格「预览缓存每条约占多少」真正量出来。
 *
 * ## 它量什么
 * 走**产品那份缓存**（`src/utils/filePreview.ts` 的 `loadFilePreview`）：喂 N 张不同 msg_id 的图片字节
 * ⇒ 字节 → `new Uint8Array` → `new Blob` → `URL.createObjectURL` → `cache.set`。
 * 前后各拍一份 V8 堆快照，交给 `heapDelta.mjs` 归并相减 ⇒ 每条目在 V8 侧留下多少字节、是什么形状。
 *
 * ## 唯一被换掉的那一条
 * `api.readFilePreview`（Tauri 那条读字节的命令）换成"当场生成本地字节"的替身，其余全是生产码。
 * 为什么可以换：这一格要答的是**缓存那一份内存形状**，而后端读盘只是字节的来源；
 * 在浏览器里根本没有那个后端（vite dev 起来的那一页 `invoke` 一律失败）。
 * ⚠️ 边界：替身喂的是 `Uint8Array`，走的是生产码 `new Uint8Array(raw)` 那条**非 macOS 序列化路径**；
 * WKWebView 上 raw 是 `number[]`，那一档的字节账另算（这条未证，见下面"读不到什么"）。
 *
 * ## 用法（自驱：自己起 vite 与 headless 浏览器，退出前把它们等到端口空出来）
 *     node perf/previewHeap.mjs                       # 默认 N=200、每张 512 KiB
 *     PROBE_N=500 PROBE_BYTES=2097152 node perf/previewHeap.mjs
 *     PERF_HEAP_KEEP=1 node perf/previewHeap.mjs      # 顺带把两份快照落到 /tmp（各几十 MB）
 * 端口可用 `PROBE_VITE_PORT` / `PROBE_CDP_PORT` 改；被占就**当场红**（那是假绿通道，不是"换个端口接着跑"）。
 *
 * ## 读不到什么（① 已经是实测，不是免责声明）
 * ① V8 的 `self_size` 不含字符串 / `ArrayBuffer` 的**外部**内存：本机三次 200 条的读数都是 +35 KB 左右，
 *    而喂进去的字节分别是 12.5 / 100 / 200 MB ⇒ 增量与字节数无关，Blob 那一档确实不在快照里。
 *    所以这一格量到的是**缓存条目自身**（每条 ≈180 B、约 3.5 个节点）＋ 200 条 objectURL 字符串的
 *    外部部分（`native system / ExternalStringData`），**不是**「一张图值多少」。
 * ② 解码后的位图（`<img>` 真画出来那份）不在这里 —— 本工具**不挂任何 `<img>`**，只测缓存自身；
 * ③ 合成字节不等于真照片：字节数一样，但真图的 `image/png` 解码成本、`Blob` 分块都不一样；
 * ④ 快照里"谁拽着它不放"（GC root 路径）要全图 BFS，这里刻意不做。
 */

import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { diff, summarize } from "./heapDelta.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const VITE_PORT = Number(process.env.PROBE_VITE_PORT || 5199);
const CDP_PORT = Number(process.env.PROBE_CDP_PORT || 9446);
const N = Number(process.env.PROBE_N || 200);
const BYTES = Number(process.env.PROBE_BYTES || 512 * 1024);
const KEEP = process.env.PERF_HEAP_KEEP === "1";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function portListening(port, timeoutMs = 250) {
  return new Promise((resolve) => {
    const sock = net.connect({ host: "127.0.0.1", port });
    const done = (v) => { sock.destroy(); resolve(v); };
    sock.setTimeout(timeoutMs);
    sock.once("connect", () => done(true));
    sock.once("timeout", () => done(false));
    sock.once("error", () => done(false));
  });
}

/** 找一台起得来的 Chromium 系浏览器（与 scripts/check-ui-runtime.mjs 同一档优先级）。 */
function findChrome() {
  const envBin = process.env.GOSSLAN_CHROME;
  if (envBin && fs.existsSync(envBin)) return { bin: envBin, source: "GOSSLAN_CHROME 指定" };
  const base = process.platform === "darwin"
    ? path.join(os.homedir(), "Library", "Caches", "ms-playwright")
    : path.join(process.env.HOME || os.homedir(), ".cache", "ms-playwright");
  const cands = [];
  if (fs.existsSync(base)) {
    for (const dir of fs.readdirSync(base)) {
      if (!/^chromium-/.test(dir)) continue;
      cands.push(path.join(base, dir, "chrome-mac-arm64", "Google Chrome for Testing.app",
        "Contents", "MacOS", "Google Chrome for Testing"));
      cands.push(path.join(base, dir, "chrome-linux64", "chrome"));
    }
  }
  const hit = cands.find((p) => fs.existsSync(p));
  if (hit) return { bin: hit, source: "playwright 缓存（Chrome for Testing）" };
  const system = [
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  ];
  const alt = system.find((p) => fs.existsSync(p));
  if (alt) return { bin: alt, source: "系统浏览器回退" };
  throw new Error("找不到可用浏览器（设 GOSSLAN_CHROME 或装一台 Chromium 系的）");
}

async function main() {
  for (const [label, port] of [["vite", VITE_PORT], ["CDP", CDP_PORT]]) {
    if (await portListening(port)) {
      throw new Error(`${label} 端口 ${port} 已被占用 ⇒ 这一趟没跑。`
        + `那多半是上次被中断留下的实例（连上去会读到几小时前那份构建 = 假绿通道）。`
        + `核对：lsof -nP -iTCP:${port} -sTCP:LISTEN`);
    }
  }
  const { bin: chrome, source } = findChrome();
  console.log(`· 浏览器：${chrome}\n· 来源：${source}`);
  const viteBin = path.join(ROOT, "node_modules", "vite", "bin", "vite.js");
  const vite = spawn(process.execPath, [viteBin, "dev", "--port", String(VITE_PORT), "--strictPort"],
    { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "gosslan-heap-"));
  const chromeProc = spawn(chrome, [
    "--headless=new", `--remote-debugging-port=${CDP_PORT}`, `--user-data-dir=${profile}`,
    "--no-first-run", "--no-default-browser-check", "--disable-gpu", "about:blank",
  ], { stdio: "ignore" });

  let ws = null;
  let exitCode = 0;
  try {
    // 等 vite 起来
    let url = "";
    for (let i = 0; i < 90 && !url; i += 1) {
      await sleep(500);
      if (await portListening(VITE_PORT)) url = `http://127.0.0.1:${VITE_PORT}/`;
    }
    if (!url) throw new Error("vite 没起来（90 × 500 ms 预算用完）");
    console.log(`· dev server：${url}`);
    // 等 CDP
    let wsUrl = null;
    for (let i = 0; i < 60 && !wsUrl; i += 1) {
      await sleep(300);
      try {
        const list = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json();
        wsUrl = (list.find((t) => t.type === "page") || {}).webSocketDebuggerUrl ?? null;
      } catch { /* 还在起 */ }
    }
    if (!wsUrl) throw new Error(`CDP ${CDP_PORT} 上没有 page target ⇒ 这一趟没跑`);
    const ver = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`)).json().catch(() => ({}));
    console.log(`· 引擎：${ver.Browser ?? "问不出"}｜来源：${source}`);

    // 极简 CDP 客户端：id 配对 + 事件分流 + **每次调用都有预算**（今天学到的一课：静等最坏）
    ws = new WebSocket(wsUrl);
    await new Promise((res, rej) => {
      ws.addEventListener("open", res, { once: true });
      ws.addEventListener("error", rej, { once: true });
      setTimeout(() => rej(new Error("CDP 握手超时")), 10_000);
    });
    const pending = new Map();
    let chunkSink = null;
    let seq = 0;
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(typeof ev.data === "string" ? ev.data : ev.data.toString());
      if (msg.method === "HeapProfiler.addHeapSnapshotChunk") {
        if (chunkSink) chunkSink.push(msg.params.chunk);
        return;
      }
      if (msg.id && pending.has(msg.id)) {
        const { resolve, reject } = pending.get(msg.id);
        pending.delete(msg.id);
        if (msg.error) reject(new Error(JSON.stringify(msg.error))); else resolve(msg.result);
      }
    });
    const send = (method, params = {}, timeoutMs = 240_000) => new Promise((resolve, reject) => {
      const id = ++seq;
      const guard = setTimeout(() => {
        if (pending.delete(id)) reject(new Error(`CDP「${method}」${timeoutMs} ms 内没回包`));
      }, timeoutMs);
      pending.set(id, {
        resolve: (v) => { clearTimeout(guard); resolve(v); },
        reject: (e) => { clearTimeout(guard); reject(e); },
      });
      ws.send(JSON.stringify({ id, method, params }));
    });
    const evalJs = async (expression) => {
      const r = await send("Runtime.evaluate",
        { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) {
        throw new Error("页面里抛错：" + JSON.stringify(r.exceptionDetails).slice(0, 400));
      }
      return r.result && r.result.value;
    };
    const snapshot = async () => {
      await send("HeapProfiler.enable");
      await send("HeapProfiler.collectGarbage");
      await sleep(300);
      const chunks = [];
      chunkSink = chunks;
      try {
        await send("HeapProfiler.takeHeapSnapshot", { reportProgress: false });
      } finally {
        chunkSink = null;
      }
      return chunks.join("");
    };

    await send("Runtime.enable");
    await send("Page.enable");
    // ⚠️ 试过的另一条路（别再试第二遍，除非连错误一起打出来）：用 CDP `Fetch` 在响应阶段注入
    // COOP/COEP 换取第三把尺子（measureUserAgentSpecificMemory 会把 Blob 单列）。本机实测：
    // 导航那一发被 pause 之后 `Fetch.continueResponse` 一旦失败（当时被 catch 咽掉了），
    // Page.navigate 就**再也不回包**，整趟跑到 240 s 预算才炸 ⇒ 代价明显大于收益。
    // 也试过另配一份 vite config 加那两个头：那份没有 alias 与 vue 插件，
    // `@/api` 解析不到、动态 import 当场失败（所以项目自己的 vite 配置保持不动）。
    // ⇒ 第三把尺子这次**不亮**，Blob 那一档改由 renderer RSS 卡上界，读数里明写它是粗尺。
    await send("Page.navigate", { url: `http://127.0.0.1:${VITE_PORT}/perf/vlist.html?n=1` });
    // 等 vite 现编译那几个模块（第一次 import 真的慢）
    await sleep(3_000);

    const setup = await evalJs(`(async () => {
      const apiMod = await import('/src/api/index.ts');
      const fp = await import('/src/utils/filePreview.ts');
      const api = apiMod.api;
      window.__hp = { made: 0, failures: 0, bytesPer: ${BYTES} };
      api.readFilePreview = function () {
        const b = new Uint8Array(window.__hp.bytesPer);
        for (let i = 0; i < b.length; i += 1) b[i] = (i * 31 + 7) & 0xff;
        window.__hp.made += 1;
        return Promise.resolve(b);
      };
      // 走的就是生产那份 loadFilePreview：缓存命中与 objectURL 归它，本脚本一行都不替
      window.__hp.load = async function (n, prefix) {
        let ok = 0;
        for (let i = 0; i < n; i += 1) {
          const r = await fp.loadFilePreview(prefix + i, 'image', 'shot-' + i + '.png');
          if (r && r.url) ok += 1; else window.__hp.failures += 1;
        }
        return ok;
      };
      const warm = await window.__hp.load(8, 'warm-');
      return { ok: true, warm, isolated: !!self.crossOriginIsolated, heap: (performance.memory || {}).usedJSHeapSize ?? null };
    })()`);
    if (!setup || setup.ok !== true) throw new Error("夹具没装上：" + JSON.stringify(setup));
    console.log(`· 预热 8 条完成（读到 ${setup.warm} 条带 url 的结果）｜crossOriginIsolated=${setup.isolated}`);

    const beforeRaw = await snapshot();
    const before = summarize(JSON.parse(beforeRaw));
    const heapA = await evalJs("(() => (performance.memory || {}).usedJSHeapSize ?? null)()");
    const loaded = await evalJs(`(async () => await window.__hp.load(${N}, 'bulk-'))()`);
    const failures = await evalJs("window.__hp.failures");
    const afterRaw = await snapshot();
    const after = summarize(JSON.parse(afterRaw));
    const heapB = await evalJs("(() => (performance.memory || {}).usedJSHeapSize ?? null)()");

    if (loaded !== N) {
      console.log(`⚠️ 只有 ${loaded}/${N} 条拿到 url（生产那份记了 ${failures} 条失败）`
        + ` ⇒ 下面的每条均摊按 ${loaded} 条算，而"失败不缓存"那条规则同时会改变结果形状`);
    } else {
      console.log(`· ${N} 条全部拿到 objectURL（失败计数 ${failures}）`);
    }
    const d = diff(before, after);
    const per = loaded > 0 ? Math.round(d.totalDelta / loaded) : 0;
    // 自证"这份快照里真有本次新增的东西"：每条缓存项都会留下一格**唯一**的 blob: URL 字符串节点。
    // 对不上就说明拍空了或只拍到旧状态 ⇒ 当场判红，不许把看起来漂亮的增量当读数引
    //（本仓规矩：「有断言」不等于「被证明会失败」，这一条的失败形状写在 perf/README 量具八那一节）。
    const urlRows = d.rows.filter((r) => r.key.startsWith("string blob:") && r.countDelta === 1);
    const selfCheckOk = urlRows.length === loaded;
    console.log(`${selfCheckOk ? "✅" : "⚠️"} 自证：新增唯一 blob: URL 字符串节点 ${urlRows.length} 格，应等于 loaded ${loaded}`
      + `${selfCheckOk ? "" : " ⇒ 这台量具这次没拍到东西，上面的增量一律不作数"}`);
    if (!selfCheckOk) exitCode = 1;
    console.log(`\n喂进去 ${loaded} 条 × ${BYTES} 字节 = ${(loaded * BYTES / 1024 / 1024).toFixed(1)} MB（Blob 侧）`);
    console.log(`量到的 V8 侧增量 ${fmtAbs(d.totalDelta)}，节点数变化 ${d.countDelta >= 0 ? "+" : ""}${d.countDelta}`
      + ` ⇒ 每条均摊 ≈ ${fmtAbs(per)}（self_size 口径）`);
    if (heapA != null && heapB != null) {
      console.log(`标量对照 usedJSHeapSize ${(heapA / 1024 / 1024).toFixed(2)} → ${(heapB / 1024 / 1024).toFixed(2)} MB`
        + `（与上面那份快照增量同档 ⇒ 两把独立尺子互点，不是解析器造的数）`);
    }
    console.log(`★ 这一档与喂进去的字节数**无关**：本机三次 200 条的读数分别是 +35.1 / +35.2 / +35.2 KB，`
      + `对应 12.5 / 100 / 200 MB 的字节量（PROBE_BYTES=65536 / 524288 / 1048576）`
      + ` ⇒ "self_size 不计 Blob 外部内存"这句已从引文变成实测`);
    console.log(`⚠️ Blob 那一档这台机器**量不到归因**（三条路都试过）：heap snapshot 不含外部内存；`
      + `measureUserAgentSpecificMemory 要跨源隔离（CDP Fetch 注入头会把导航卡死、另配 vite config 会丢 alias）；`
      + `renderer RSS 经剂量对照判为不可用（喂 12.5 MB 涨 130 MB、喂 200 MB 只涨 11.8 MB ⇒ 不跟剂量走）。`);
    console.log(`\n按「type+name」归并的前 12 格（按差绝对值排）：`);    for (const r of d.rows.slice(0, 12)) {
      const flag = r.newKey ? " 新增" : r.gone ? " 消失" : "";
      console.log(`  ${r.delta >= 0 ? "+" : "-"}${fmtAbs(r.delta).padStart(10)}（${String(r.countDelta).padStart(7)} 个节点）${flag}  ${r.key.slice(0, 72)}`);
    }
    const blobish = d.rows.filter((r) => /blob|Blob|external|ArrayBuffer|system-/i.test(r.key)).slice(0, 8);
    console.log(`\n名字里带 blob / external / ArrayBuffer / system- 的行（这把尺子到底看不看得见外部内存 —— 现读，不猜）：`);
    if (!blobish.length) console.log("  一行都没有 ⇒ 这份快照里根本没有外部内存那一档，上面的「盲区」不是一句免责声明而是实测结论");
    for (const r of blobish) {
      console.log(`  ${r.delta >= 0 ? "+" : "-"}${fmtAbs(r.delta).padStart(10)}（${String(r.countDelta).padStart(7)} 个节点）  ${r.key.slice(0, 72)}`);
    }
    if (KEEP) {
      const stamp = Date.now();
      const fileA = path.join(os.tmpdir(), `gosslan-preview-before-${stamp}.heapsnapshot`);
      const fileB = path.join(os.tmpdir(), `gosslan-preview-after-${stamp}.heapsnapshot`);
      fs.writeFileSync(fileA, beforeRaw);
      fs.writeFileSync(fileB, afterRaw);
      console.log(`· 两份快照落盘（系统临时目录，不在仓里）：各 ${(beforeRaw.length / 1024 / 1024).toFixed(1)}`
        + ` / ${(afterRaw.length / 1024 / 1024).toFixed(1)} MB`);
      console.log(`  复跑归并：node perf/heapDelta.mjs "${fileA}" "${fileB}"`);
    }
  } catch (e) {
    console.error(`✗ ${e && e.message || e}`);
    exitCode = 1;
  } finally {
    if (ws) try { ws.close(); } catch { /* 已经断了 */ }
    vite.kill("SIGTERM");
    chromeProc.kill("SIGTERM");
    await sleep(500);
    for (let i = 0; i < 20; i += 1) {
      if (!(await portListening(VITE_PORT)) && !(await portListening(CDP_PORT))) break;
      await sleep(500);
    }
    try { fs.rmSync(profile, { recursive: true, force: true }); } catch { /* 交给系统 */ }
  }
  process.exit(exitCode);
}

function fmtAbs(n) {
  const a = Math.abs(n);
  const sign = n < 0 ? "-" : "+";
  if (a >= 1024 * 1024) return `${sign}${(a / 1024 / 1024).toFixed(2)} MB`;
  if (a >= 1024) return `${sign}${(a / 1024).toFixed(1)} KB`;
  return `${sign}${a} B`;
}

await main();
