// 量具七：页面**隐藏**与**重新显示**期间，应用自己注册的定时器到底还跑不跑、跑了多少次。
//
// 为什么要这一台（roadmap N25 那一格）：`useChatStore.ts:2228` 的 5 秒拓扑刷新回调里没有
// `document.hidden` 门，而「隐藏期间它实际跑了多少次」在本仓从来没有过一个数 ——
// 没有数就不许写"它在耗电"，也不许写"浏览器会替我节流"（那是常识，不是实测）。
//
// 用法（与前面几台一样要三个进程，详见 perf/README.md 量具七那一节）：
//   1) npx vite --port 5199 --strictPort
//   2) "Brave Browser" --headless=new --disable-gpu --remote-debugging-port=9223 --user-data-dir=/tmp/brave-perf-bg
//   3) node perf/background.mjs                                  # 三档各 30 s
//      PERF_HIDDEN_WINDOW=330 node perf/background.mjs            # 隐藏档拉长到 5.5 分钟，
//                                                                  # 跨过 Chrome「隐藏满 5 分钟才强节流」那条界
//
// ⚠️ 读的是**真实应用页**（`http://127.0.0.1:5199/`），不是 perf/vlist.html：
// 被量的对象是 `chat.init()` 里那条 setInterval，合成台页面上根本没有它。
// ⚠️ 无 Rust 后端 ⇒ 每个 tick 的 getTopology() 都失败。失败是这个环境的 artifact、不是产品行为
// ⇒ CDP 侧的异常计数只当**第二把尺子**（与页内 shim 互相核对），不进任何结论。

const CDP = "http://127.0.0.1:9223";
const PORT = process.env.PERF_PORT ?? "5199";
const BASE = `http://127.0.0.1:${PORT}`;
const WINDOW_S = Number(process.env.PERF_WINDOW ?? 30);
const HIDDEN_WINDOW_S = Number(process.env.PERF_HIDDEN_WINDOW ?? WINDOW_S);
const SERIES_EVERY_S = Number(process.env.PERF_SERIES_EVERY ?? 30);
const TOPO_DELAY = 5000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function listTargets() {
  return (await fetch(`${CDP}/json/list`)).json();
}

/** 应用主页面 = 根路径。不能把 /perf/* 那些台子当成它。 */
const isAppRoot = (u) => /^http:\/\/127\.0\.0\.1:\d+\/(\?.*)?$/.test(u);

async function openTarget(url, filter) {
  for (let i = 0; i < 40; i++) {
    const list = await listTargets();
    const page = list.find((t) => t.type === "page" && filter(t.url));
    if (page) return page;
    await fetch(`${CDP}/json/new?${encodeURIComponent(url)}`, { method: "PUT" }).catch(() => {});
    await sleep(500);
  }
  throw new Error("找不到目标页（9223 是不是没起？）");
}

/**
 * 计时的两只眼睛，互相独立：
 *  ① 页内 shim —— 在应用脚本之前把 setInterval 包一层，按回调源码前 72 字归属次数。
 *     只转发：注册一次、返回真 id、参数原样透传（写错这里就会重复注册，次数假一倍）。
 *     同时挂**量具自己那只** visibilitychange 监听，记「这一页被投递到几次可见性变化」。
 *  ② CDP 事件 —— Runtime.exceptionThrown 计数，完全不碰页面。
 * 两个数对得上才说明①没把被量对象量坏；对不上就两把都别信。
 * ⚠️ ①那只监听收到 ≠ 应用自己那几只（reportActivity / onVisibility）收到，这一半买不到。
 */
const SHIM = [
  "window.__bg = { registrations: [], seenDoc: 0 };",
  "(function () {",
  "  var orig = window.setInterval;",
  "  window.setInterval = function (fn, ms) {",
  "    var args = Array.prototype.slice.call(arguments, 1);",
  "    var rec = { delay: String(ms), src: typeof fn === 'function' ? String(fn).replace(/\\s+/g, ' ').slice(0, 72) : '[non-fn]', fires: 0 };",
  "    window.__bg.registrations.push(rec);",
  "    if (typeof fn !== 'function') return orig.apply(window, [fn].concat(args));",
  "    var wrapped = function () {",
  "      rec.fires += 1;",
  "      return fn.apply(this, arguments);",
  "    };",
  "    return orig.apply(window, [wrapped].concat(args));",
  "  };",
  "  document.addEventListener('visibilitychange', function () { window.__bg.seenDoc += 1; });",
  "})();",
  "true;",
].join("\n");

function makeClient(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let id = 0;
  const pending = new Map();
  const events = [];
  ws.addEventListener("message", (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
      return;
    }
    if (msg.method) events.push(msg);
  });
  const ready = new Promise((res, rej) => {
    ws.addEventListener("open", res);
    ws.addEventListener("error", () => rej(new Error("ws 连接失败 " + wsUrl)));
  });
  function send(method, params = {}) {
    const mid = ++id;
    ws.send(JSON.stringify({ id: mid, method, params }));
    return new Promise((resolve, reject) => {
      pending.set(mid, (m) => (m.error ? reject(new Error(method + ": " + JSON.stringify(m.error))) : resolve(m.result)));
      setTimeout(() => pending.has(mid) && (pending.delete(mid), reject(new Error(method + " 超时"))), 120000);
    });
  }
  async function evalJs(expression) {
    const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 400));
    return r.result.value;
  }
  return { ready, send, evalJs, drain: () => events.splice(0, events.length), close: () => ws.close() };
}

/** 状态判据：轮询到 document.hidden 真是那个值为止；到点没翻 ⇒ 这一档读数作废。 */
async function waitHidden(app, want) {
  for (let i = 0; i < 20; i++) {
    const s = await app.evalJs("({hidden: document.hidden, vis: document.visibilityState})");
    if (s.hidden === want) return { ...s, landed: true, polls: i + 1 };
    await sleep(250);
  }
  const last = await app.evalJs("({hidden: document.hidden, vis: document.visibilityState})");
  return { ...last, landed: false, polls: 20 };
}

async function metrics(app) {
  const { metrics: m } = await app.send("Performance.getMetrics");
  const g = (k) => {
    const e = m.find((x) => x.name === k);
    return e && typeof e.value === "number" ? e.value : 0;
  };
  return {
    scriptS: +g("ScriptDuration").toFixed(3),
    taskS: +g("TaskDuration").toFixed(3),
    styleS: +g("RecalcStyleDuration").toFixed(3),
    layoutS: +g("LayoutDuration").toFixed(3),
    heapMB: +(g("JSHeapUsedSize") / 1048576).toFixed(2),
    nodes: g("Nodes"),
    listeners: g("JSEventListeners"),
  };
}

async function pageCounters(app) {
  return app.evalJs(`(() => {
    const b = window.__bg;
    if (!b) return null;
    const isTopo = (r) => r.delay === "${TOPO_DELAY}" && /Topology/i.test(r.src);
    const topo = b.registrations.filter(isTopo);
    return {
      seenDoc: b.seenDoc,
      totalIntervalFires: b.registrations.reduce((s, r) => s + r.fires, 0),
      topoRegistrations: topo.length,
      topoFires: topo.reduce((s, r) => s + r.fires, 0),
      registrations: b.registrations.length,
      topoSamples: topo.map((r) => ({ src: r.src, fires: r.fires })),
      sameDelayOthers: b.registrations.filter((r) => r.delay === "${TOPO_DELAY}" && !isTopo(r)).map((r) => ({ src: r.src, fires: r.fires })),
    };
  })()`);
}

/**
 * 一个观测窗口。注意两处刻意的口径：
 *  · `transitionsIntoWindow` 用「本窗口首样本 − 上一窗口末样本」的绝对值差 ——
 *    翻档发生在上一样本之后、本样本之前，写成窗口内增量会**结构性恒 0**（第一版就踩了）。
 *  · 长窗口按 SERIES_EVERY_S 记中间序列，节流什么时候开始生效只有序列看得见。
 */
async function observe(app, label, wantHidden, windowMs, prevEnd) {
  const state = await waitHidden(app, wantHidden);
  const t0 = Date.now();
  const a = await metrics(app);
  const pa = await pageCounters(app);
  app.drain();
  const series = [];
  // 窗口级 exceptions 必须是**整窗**累加：只取末段 drain 会把「16 次触发 / 0 条异常」
  // 打印成两把尺子互相打脸，而真相是逐段相等。
  let exceptionsTotal = 0;
  let prevCum = 0;
  const stepMs = Math.max(1000, SERIES_EVERY_S * 1000);
  let next = stepMs;
  while (Date.now() - t0 < windowMs) {
    const wait = Math.max(0, t0 + next - Date.now());
    await sleep(Math.min(wait, t0 + windowMs - Date.now()));
    if (Date.now() - t0 >= windowMs) break;
    const s = await pageCounters(app);
    const exc = app.drain().filter((e) => e.method === "Runtime.exceptionThrown").length;
    exceptionsTotal += exc;
    // topoCum = 距窗口起点的累计；topoStep = 这一段自己加了多少。两个语义分开写清楚，
    // 否则一格里既藏累计又藏增量，读的人只能猜。
    const cum = s.topoFires - pa.topoFires;
    series.push({ atS: Math.round((Date.now() - t0) / 1000), topoCum: cum, topoStep: cum - prevCum, excStep: exc });
    prevCum = cum;
    next += stepMs;
  }
  const b = await metrics(app);
  const pb = await pageCounters(app);
  const evts = app.drain();
  exceptionsTotal += evts.filter((e) => e.method === "Runtime.exceptionThrown").length;
  const wallMs = Date.now() - t0;
  return {
    label,
    state,
    windowMs,
    wallMs,
    counters: pa && pb ? {
      topoFires: pb.topoFires - pa.topoFires,
      totalIntervalFires: pb.totalIntervalFires - pa.totalIntervalFires,
      exceptions: exceptionsTotal,
      expectedTopoTicks: Math.floor(windowMs / TOPO_DELAY),
      ratePerMin: +(((pb.topoFires - pa.topoFires) * 60000) / wallMs).toFixed(1),
      transitionsIntoWindow: prevEnd && pb ? pb.seenDoc - prevEnd.seenDoc : null,
    } : null,
    series,
    metricsDelta: {
      scriptS: +(b.scriptS - a.scriptS).toFixed(3),
      taskS: +(b.taskS - a.taskS).toFixed(3),
      styleS: +(b.styleS - a.styleS).toFixed(3),
      layoutS: +(b.layoutS - a.layoutS).toFixed(3),
      heapMB: +(b.heapMB - a.heapMB).toFixed(2),
      nodesDelta: b.nodes - a.nodes,
      listeners: b.listeners,
    },
  };
}

// ── 开跑 ────────────────────────────────────────────────────────────────
const appTarget = await openTarget(BASE + "/", isAppRoot);
const app = makeClient(appTarget.webSocketDebuggerUrl);
await app.ready;
await app.send("Page.enable");
await app.send("Runtime.enable");
await app.send("Performance.enable");
// shim 对**已加载**的页无效，所以先挂 new-document 脚本再导航
await app.send("Page.addScriptToEvaluateOnNewDocument", { source: SHIM });
await app.send("Page.navigate", { url: BASE + "/" });

// 等 chat.init() 真把那条 5 秒定时器注册起来 —— 本量具的阳性对照
let reg = null;
for (let i = 0; i < 80; i++) {
  await sleep(500);
  reg = await pageCounters(app).catch(() => null);
  if (reg && reg.topoRegistrations > 0) break;
}
if (!reg) {
  console.error("❌ 页面没有 __bg（shim 没装上 / 导航失败）⇒ 整轮作废");
  process.exit(1);
}
if (reg.topoRegistrations === 0) {
  console.log(JSON.stringify({ registrations: reg }, null, 2));
  console.error(
    "❌ 没看到 delay=5000 且回调含 Topology 的注册 ⇒ 应用侧那条定时器在这一环境根本没起来。" +
      "读数作废：不许把 0 次读成「隐藏期间没跑」"
  );
  process.exit(1);
}

const decoyTarget = await openTarget(`${BASE}/perf/vlist.html?n=1`, (u) => u.includes("perf/vlist.html"));
const decoy = makeClient(decoyTarget.webSocketDebuggerUrl);
await decoy.ready;
await decoy.send("Page.enable");

await app.send("Page.bringToFront").catch(() => {});
const visible1 = await observe(app, "visible-1（应用页在前台）", false, WINDOW_S * 1000, null);

// 隐藏：把另一张标签推到前台 —— headless 里这是能把 document.hidden 真翻成 true 的通道。
// ⚠️ prevEnd 必须在**翻转之前**取：翻在取之后就把那一次 visibilitychange 记到窗口外面了
// （第一版就是这样，into-hidden 恒打印 0，而那条计数本来是用来判「事件到底投没投递到这一页」的）。
const beforeHide = await pageCounters(app);
await decoy.send("Page.bringToFront");
const hidden = await observe(app, "hidden（另一标签在前台）", true, HIDDEN_WINDOW_S * 1000, beforeHide);

// 重新显示：关掉那张挡路的标签
const beforeClose = await pageCounters(app);
await decoy.send("Target.closeTarget", { targetId: decoyTarget.id }).catch(() => {});
await sleep(700);
const visible2 = await observe(app, "visible-2（回到前台）", false, WINDOW_S * 1000, beforeClose);

const stateOK = visible1.state.landed && hidden.state.landed && visible2.state.landed;
const reads = [visible1, hidden, visible2];

console.log(
  JSON.stringify(
    {
      env: {
        base: BASE,
        visibleWindowS: WINDOW_S,
        hiddenWindowS: HIDDEN_WINDOW_S,
        seriesEveryS: SERIES_EVERY_S,
        topoDelayMs: TOPO_DELAY,
        note: "无 Rust 后端 ⇒ 每个 tick 的 getTopology() 都失败；exceptions 是第二把尺子，不是产品行为",
      },
      positiveControl: reg,
      summary: reads.map((r) => ({
        label: r.label,
        hidden: r.state.hidden,
        landed: r.state.landed,
        topoFires: r.counters.topoFires,
        expected: r.counters.expectedTopoTicks,
        exceptions: r.counters.exceptions,
        transitionsIntoWindow: r.counters.transitionsIntoWindow,
        ratePerMin: r.counters.ratePerMin,
        scriptS: r.metricsDelta.scriptS,
      })),
      reads,
      verdicts: {
        stateControlsLanded: stateOK,
        // 两只眼睛是否一致：shim 记的 topo 次数与 CDP 侧异常次数逐档相等
        crossCheckShimVsExceptions: reads.map((r) => [r.counters.topoFires, r.counters.exceptions]),
        keepRunningWhileHidden: hidden.counters.topoFires > 0,
        // 速率一律用真实墙钟（wallMs）：第一版拿"秒"当"毫秒"除，打印出 2909 次/分钟这种荒唐数
        ratePerMin: { visible: visible1.counters.ratePerMin, hidden: hidden.counters.ratePerMin, back: visible2.counters.ratePerMin },
        // 节流从什么时候开始的，只有隐藏档的中间序列看得见
        hiddenSeries: hidden.series,
        visibilitychangeDelivered: {
          intoHidden: hidden.counters.transitionsIntoWindow,
          intoVisible: visible2.counters.transitionsIntoWindow,
        },
      },
    },
    null,
    2
  )
);

if (!stateOK) {
  console.error("❌ 至少一档 document.hidden 没翻到预期值 ⇒ 上面三档不是「隐藏 / 显示」两态，读数作废");
  app.close();
  process.exit(1);
}
app.close();
process.exit(0);
