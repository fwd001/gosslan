// 通过 CDP 驱动 perf/vlist.html 跑压测，把结果打到 stdout。
// 用法：node perf/run.mjs 50000   （需要先用 `npx vite --port 5199` 起 dev server）
//      另开一个终端跑："Brave Browser" --headless=new --remote-debugging-port=9223

const CDP = "http://127.0.0.1:9223";
const PORT = process.env.PERF_PORT ?? "5199";

async function pickPage() {
  for (let i = 0; i < 30; i++) {
    try {
      const list = await (await fetch(`${CDP}/json`)).json();
      const page = list.find((t) => t.type === "page" && t.url.includes("vlist.html"));
      if (page) return page;
      // 没有目标页就新开一个
      await fetch(`${CDP}/json/new?${encodeURIComponent(`http://127.0.0.1:${PORT}/perf/vlist.html?n=${process.argv[2] ?? 50000}`)}`, { method: "PUT" });
    } catch {
      /* 调试端口还没起来 */
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("找不到 CDP 目标页（Brave 是否带 --remote-debugging-port=9223 启动？）");
}

const page = await pickPage();
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.addEventListener("message", (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
});
await new Promise((r) => ws.addEventListener("open", r));
function send(method, params = {}) {
  const mid = ++id;
  ws.send(JSON.stringify({ id: mid, method, params }));
  return new Promise((resolve, reject) => {
    pending.set(mid, (m) => (m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result)));
    setTimeout(() => pending.has(mid) && (pending.delete(mid), reject(new Error(`${method} 超时`))), 180000);
  });
}
async function evalJs(expression) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 400));
  return r.result.value;
}

// 等页面就绪
for (let i = 0; i < 60; i++) {
  const ok = await evalJs("!!(window.__perf && window.__perf.scrollTest)");
  if (ok) break;
  await new Promise((r) => setTimeout(r, 500));
}

// PERF_ONLY=calib ⇒ 只跑"整表估算一遍"的计时，跳过 660 帧滚动与 accuracy。
// 为什么要这条出口：n=100000 那一档整条链在 Runtime.evaluate 的 180s 超时里跑不完（历史归因未定），
// 而"重算成本随条数怎么涨"这件事只需要一遍整表，不需要真滚 660 帧。
const ONLY = process.env.PERF_ONLY ?? "";

// PERF_ONLY=mem ⇒ 量具四：长时间滚动前后的堆与 DOM 节点数（roadmap N26 那格原先连量具都没有）。
// 走 CDP 的 Performance.getMetrics + HeapProfiler.collectGarbage，**不给页面加任何钩子**：
// 量具不改被量对象，否则量的就是量具自己。
if (ONLY === "mem") {
  await send("Performance.enable");
  await send("HeapProfiler.enable");
  const snap = async () => {
    await send("HeapProfiler.collectGarbage");
    const { metrics: m } = await send("Performance.getMetrics");
    const g = (k) => { const e = m.find((x) => x.name === k); return e ? e.value : null; };
    const dom = await evalJs("document.querySelectorAll('*').length");
    return {
      heapMB: +(g("JSHeapUsedSize") / 1048576).toFixed(2),
      nodes: g("Nodes"),
      listeners: g("JSEventListeners"),
      domElements: dom,
      layoutS: +g("LayoutDuration").toFixed(3),
      styleS: +g("RecalcStyleDuration").toFixed(3),
      scriptS: +g("ScriptDuration").toFixed(3),
      taskS: +g("TaskDuration").toFixed(3),
    };
  };
  const n = await evalJs("window.__perf.n");
  const before = await snap();
  const up = await evalJs("window.__perf.scrollTest(300, 800, true)");
  const mid = await snap();
  const down = await evalJs("window.__perf.scrollTest(300, 800, false)");
  const after = await snap();
  console.log(JSON.stringify({
    n,
    churn: { upFrames: up?.frames ?? null, downFrames: down?.frames ?? null },
    before, mid, after,
    delta: {
      heapMB: +(after.heapMB - before.heapMB).toFixed(2),
      nodes: after.nodes - before.nodes,
      listeners: after.listeners - before.listeners,
      domElements: after.domElements - before.domElements,
    },
  }, null, 2));
  ws.close();
  process.exit(0);
}

const result = await evalJs(`(async () => {
  const p = window.__perf;
  if (${JSON.stringify(ONLY)} === "calib") return { n: p.n, calib: p.calibrate ? p.calibrate(5) : null };
  const idxs = [0, 1, Math.floor(p.n/2), p.n-2, p.n-1].filter((v,i,a)=>a.indexOf(v)===i);
  // 先量"整表估算一遍"的直接耗时（会把 estimateCalls 归零，所以必须在 scrollTest 之前）
  const calib = p.calibrate ? p.calibrate(5) : null;
  p.resetCounters();
  const cold = await p.scrollTest(60, 800, false);           // 冷启动：大量首测行 → 触发最多的 offsets 重算
  const warmUp = await p.scrollTest(300, 800, true);         // 向上翻（从底部往历史翻）
  const warmDown = await p.scrollTest(300, 800, false);      // 向下翻
  const acc = [];
  for (const i of idxs) acc.push(await p.accuracy(i));
  return { n: p.n, calib, cold, warmUp, warmDown, acc };
})()`);

console.log(JSON.stringify(result, null, 2));
ws.close();
