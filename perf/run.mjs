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

// PERF_ONLY=resize ⇒ 量具五：窗口缩放那一瞬间，"我正在读的那一条"还在不在原位
// （§七 必测行为 7「窗口缩放」+ 领域 E「列表高度变化是否引起页面跳动」；roadmap N9 只量到"虚拟化跟不跟尺寸"，
//  这一格量的是"跟上了之后位置跳不跳"，两件事不是一件事）。
// ⚠️ 目标页必须带 `&fit=1` 打开，否则容器写死 700px ⇒ 视口再怎么变，组件那条 ResizeObserver 都不会被点到
//   ⇒ 这一格在旧形状下是**结构性测不到**的，而不是"测过没问题"。
if (ONLY === "resize") {
  const setVp = (h) => send("Emulation.setDeviceMetricsOverride", { width: 1280, height: h, deviceScaleFactor: 1, mobile: false });
  const clearVp = () => send("Emulation.setDeviceMetricsOverride", { width: 0, height: 0, deviceScaleFactor: 0, mobile: false });
  const settle = () => new Promise((r) => setTimeout(r, 500));
  const snap = () => evalJs("window.__perf.snapshot()");

  await setVp(900);
  await settle();
  // 阅读态：从底部往历史上翻一段，停在中间某一条上（贴底那一态单独量）
  await evalJs("window.__perf.scrollTest(20, 600, true)");
  await settle();
  const read0 = await snap();
  await setVp(560); await settle(); const readShrink = await snap();
  await setVp(900); await settle(); const readBack = await snap();
  await setVp(1200); await settle(); const readGrow = await snap();
  await setVp(900); await settle();
  await evalJs("window.__perf.pinBottom()"); await settle();
  const bot0 = await snap();
  await setVp(420); await settle(); const botShrink = await snap();
  await setVp(1200); await settle(); const botGrow = await snap();
  // 撤掉覆写后必须**等它真的落回窗口原尺寸**再读：实测 clearVp 之后 500ms 内 innerHeight 仍停在 1200
  // ⇒ 那一格读数不是"恢复原窗口"，把它写成 back900 就是在造一条会漂的假账。
  await clearVp();
  let afterClear = null;
  let clearLanded = false;
  for (let i = 0; i < 12; i++) {
    await settle();
    const s = await snap();
    if (s.innerHeight !== 1200) { afterClear = s; clearLanded = true; break; }
  }

  // 「同一行、同一亚像素位置」才算没跳；锚点取不到（null）一律算没判成，否则两个 null 相等会造出假绿
  const kept = (a, b) => !!a.anchorKey && a.anchorKey === b.anchorKey && Math.abs((a.anchorOffset ?? 0) - (b.anchorOffset ?? 0)) <= 1;
  const linkage = readShrink.clientHeight !== read0.clientHeight;
  console.log(JSON.stringify({
    n: await evalJs("window.__perf.n"),
    // 阳性对照：容器高度必须真的跟着视口变过，否则下面所有读数都是"从没缩放过"的读数
    linkageOK: linkage,
    read: { h900: read0, h560: readShrink, back900: readBack, h1200: readGrow },
    bottom: { h900: bot0, h420: botShrink, h1200: botGrow },
    afterClearVp: { landed: clearLanded, snap: afterClear },
    verdicts: {
      anchorKeptOnShrink: kept(read0, readShrink),
      anchorKeptOnRestore: kept(read0, readBack),
      anchorKeptOnGrow: kept(read0, readGrow),
      scrollTopRestored: readBack.scrollTop === read0.scrollTop,
      bottomPinnedAfterShrink: botShrink.bottomGap <= 1,
      bottomPinnedAfterGrow: botGrow.bottomGap <= 1,
      rowsRenderedEverywhere: [read0, readShrink, readGrow, botShrink, botGrow].every((s) => s.renderedRows > 0),
    },
  }, null, 2));
  ws.close();
  if (!linkage) {
    console.error("❌ 容器 clientHeight 没随视口变 ⇒ 量具没连上（忘了 &fit=1？），上面所有读数作废");
    process.exit(1);
  }
  process.exit(0);
}

// PERF_ONLY=throttle ⇒ 量具六：§七 必测行为第 9 项「低性能设备上的关键交互」与 §八「长帧/连续掉帧」
// 在慢 CPU 下的第一份数。走 CDP 的 Emulation.setCPUThrottlingRate，**不给页面加任何钩子**。
// ⚠️ 这是**同一台机器上的模拟降速**，不是低端 Android 实机 —— 读出来的数只能写"模拟 4×/6× 下怎样"，
//    不许写成"低端机可用"（约束 12：没跑过的平台不算通过）。
// 阳性对照用**已有的定长工作** calibrate(3)：同一段整表估算在降频后必须真的变慢（倍数 > 1.5 才采信），
// 否则说明那条 CDP 调用没落地 —— "没有长帧"就可能来自"降频根本没生效"，那是最省事的一种假绿。
if (ONLY === "throttle") {
  const rates = (process.env.PERF_RATES ?? "4,6").split(",").map((x) => Number.parseInt(x, 10)).filter((x) => x >= 1);
  const steps = Number.parseInt(process.env.PERF_STEPS ?? "150", 10);
  const setRate = (r) => send("Emulation.setCPUThrottlingRate", { rate: r });
  await send("Page.enable");
  /**
   * 每个条件都从**冷启动**跑同一段：先重载、等页面回来，**再**设速率。
   * 顺序不能反：导航会把 emulation 一起冲掉（设早了等于没设），而反过来若不重载，
   * 降频档就白拿上一遍暖好的高度缓存 ⇒ offsetsRebuilds 偏低、帧时间偏乐观（§八 要同条件比较）。
   */
  const coldStart = async (rate) => {
    await send("Page.reload", { ignoreCache: true });
    for (let i = 0; i < 120; i++) {
      const ok = await evalJs("!!(window.__perf && window.__perf.scrollTest && window.__perf.calibrate)");
      if (ok) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    await setRate(rate);
    await new Promise((r) => setTimeout(r, 300));
  };
  const n = await evalJs("window.__perf.n");
  await coldStart(1);
  const baseCalib = await evalJs("window.__perf.calibrate(3)");
  // ⚠️ 基线必须**两个方向各跑一遍**，而且顺序与降频档里完全一致（up→down）。第一版只跑了 up，于是
  // 降频 4× 那一档的 up 吃到了基线 up 刚暖好的高度缓存（offsetsRebuilds 21 vs 137），
  // 而它的 down 又拿去和"只跑过 up 的基线"比 ⇒ 两个方向不是同一条件，§八 明令不许那样比。
  const baseUp = await evalJs(`window.__perf.scrollTest(${steps}, 800, true)`);
  const baseDown = await evalJs(`window.__perf.scrollTest(${steps}, 800, false)`);
  const runs = [];
  for (const rate of rates) {
    await coldStart(rate);
    const calib = await evalJs("window.__perf.calibrate(3)");
    const up = await evalJs(`window.__perf.scrollTest(${steps}, 800, true)`);
    const down = await evalJs(`window.__perf.scrollTest(${steps}, 800, false)`);
    runs.push({
      rate,
      // 对照倍数：降频后同一段定长工作慢了多少倍（理想 ≈ rate，但只看"有没有显著变慢"）
      calibRatio: +(calib.onePassMs / baseCalib.onePassMs).toFixed(2),
      scroll: { up, down },
    });
  }
  await setRate(1);
  const backToNormal = await evalJs("window.__perf.calibrate(3)");
  const confirmed = runs.every((r) => r.calibRatio > 1.5);
  console.log(JSON.stringify({
    n,
    steps,
    rates,
    throttleConfirmed: confirmed,
    baseline: { calibrate: baseCalib, scrollUp: baseUp, scrollDown: baseDown, calibAfterReset: backToNormal },
    runs,
  }, null, 2));
  ws.close();
  if (!confirmed) {
    console.error("❌ 降频后同一段定长工作没显著变慢 ⇒ CDP 降频没落地，上面所有读数作废");
    process.exit(1);
  }
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
