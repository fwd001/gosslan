// 输入→首帧可见反馈 的驱动器（手动运行，不是门禁）。
//
// 用法（三个进程，与 perf/README.md 同一套路）：
//   1) npx vite --port 5199 --strictPort
//   2) "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser" \
//        --headless=new --disable-gpu --no-sandbox --user-data-dir=/tmp/brave-lat \
//        --remote-debugging-port=9223 --window-size=1280,900 "http://127.0.0.1:5199/perf/latency.html"
//   3) node perf/latency.mjs
//
// 它先自证"页面在出帧"，再报数 —— 顺序不能倒：headless 的页面 visibilityState=hidden 时静止不出帧，
// 那种状态下量到的"延迟"是量具自己的空档（stability-roadmap §12.6.1 的 N13 就是这么产生的）。
// 数字**不进门禁**：它会随机器负载漂，钉成红只会造出假红。

const CDP = "http://127.0.0.1:9223";
const PORT = process.env.PERF_PORT ?? "5199";
const N = Number(process.env.LAT_N ?? 12);
const BUSY_MS = Number(process.env.LAT_BUSY ?? 100);
const PAGE = `http://127.0.0.1:${PORT}/perf/latency.html`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function pickPage() {
  for (let i = 0; i < 40; i++) {
    try {
      const list = await (await fetch(`${CDP}/json`)).json();
      const page = list.find((t) => t.type === "page" && t.url.includes("latency.html"));
      if (page) return page;
      await fetch(`${CDP}/json/new?${encodeURIComponent(PAGE)}`, { method: "PUT" });
    } catch {
      /* 调试端口还没起来 */
    }
    await sleep(500);
  }
  throw new Error("找不到 CDP 目标页（Brave 是否带 --remote-debugging-port=9223 启动？）");
}

const page = await pickPage();
const ws = new WebSocket(page.webSocketDebuggerUrl);
let mid = 0;
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
  const id = ++mid;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => {
    pending.set(id, (m) => (m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result)));
    setTimeout(() => pending.has(id) && (pending.delete(id), reject(new Error(`${method} 超时`))), 60000);
  });
}

async function evalJs(expression) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 400));
  return r.result.value;
}

let shotFail = 0;

/**
 * 强制走一遍"更新渲染"：rAF 回调在这一步里跑（headless 静止时不会自己跑）。
 * ⚠️ 截图失败必须计数并打印 —— 上一版把错误 catch 掉不管，于是"泵了 6 次"其实是"一次都没泵成"，
 * 量具自证那条红得莫名其妙。吞掉自己判据的输入 = 造假红，跟吞掉产品缺陷同罪。
 */
async function pump(times = 4) {
  for (let i = 0; i < times; i++) {
    try {
      await send("Page.captureScreenshot", { format: "png" });
    } catch {
      shotFail += 1;
    }
    await sleep(40);
  }
}

async function click(x, y) {
  const base = { x, y, button: "left", clickCount: 1, pointerType: "mouse" };
  await send("Input.dispatchMouseEvent", { ...base, type: "mousePressed" });
  await send("Input.dispatchMouseEvent", { ...base, type: "mouseReleased" });
}

function pct(arr, p) {
  if (!arr.length) return null;
  const s = [...arr].sort((a, b) => a - b);
  const i = Math.min(s.length - 1, Math.max(0, Math.round((p / 100) * (s.length - 1))));
  return Math.round(s[i] * 100) / 100;
}

for (let i = 0; i < 60; i++) {
  if (await evalJs("!!(window.__lat && window.__lat.mounted())")) break;
  await sleep(400);
}
if (!(await evalJs("window.__lat.mounted()"))) {
  console.log(
    JSON.stringify(
      {
        致命: "真实组件没挂起来（#opener 不在 DOM 里），不报任何数字",
        页面自报的错: await evalJs("window.__lat.error()"),
        面板读数: await evalJs("window.__lat.panel()"),
        根节点子数: await evalJs("document.getElementById('lat-root').childElementCount"),
      },
      null,
      2,
    ),
  );
  process.exit(1);
}

// ── ① 量具自证：这段泵帧的时间里 rAF 必须真的回调过 ────────────────────────────
await send("Page.enable").catch(() => {});
// /json/new 开出来的页可能不是所在窗口的活动标签 ⇒ captureScreenshot 直接失败 ⇒ 一次帧都没驱动。
await send("Page.bringToFront").catch(() => {});
await evalJs("window.__lat.frameStart()");
await pump(6);
// frameStart 把计数清零，所以 frameStop 读回来的就是「这一段窗口里的帧数」，本身即判据。
const framesInWindow = await evalJs("window.__lat.frameStop()");
const framesFlowing = framesInWindow >= 1;
const shotOk = 6 - shotFail;
console.log(
  `量具自证｜出帧 ${framesFlowing ? "✅" : "❌"}` +
    `（泵帧期间截图成功 ${shotOk}/6 次、rAF 回调 ${framesInWindow} 次；` +
    `截图失败 ${shotFail} 次说明"泵"根本没驱动渲染 ⇒ 那是量具坏）` +
    ` visibilityState=${await evalJs("window.__lat.visibility()")}`,
);
if (!framesFlowing) {
  console.log(JSON.stringify({ 结论: "页面不出帧 ⇒ 此时量的任何数都是量具自己的空档，拒绝报数" }, null, 2));
  process.exit(1);
}

async function measure(count, label) {
  const deltas = [];
  const frames = [];
  for (let i = 0; i < count; i++) {
    await evalJs("window.__lat.close()");
    await pump(5);
    if (await evalJs("window.__lat.isOpen()")) throw new Error(`${label}：上一轮没关干净，测量前提不成立`);
    await evalJs("window.__lat.begin()");
    const pt = await evalJs("window.__lat.point()");
    await click(pt.x, pt.y);
    await sleep(260);
    const s = await evalJs("window.__lat.end()");
    if (!s || s.delta === null) throw new Error(`${label}：没观察到样式变化（event=${s && s.eventT}）`);
    deltas.push(s.delta);
    frames.push(s.frames);
  }
  return { label, n: deltas.length, p50: pct(deltas, 50), p95: pct(deltas, 95), min: pct(deltas, 0), max: pct(deltas, 100), framesObserved: { min: Math.min(...frames), max: Math.max(...frames) }, deltas: deltas.map((d) => Math.round(d * 100) / 100) };
}

const plain = await measure(N, "一次真点击 → 弹窗面板第一次带上计算样式");
await evalJs(`window.__lat.setBusy(${BUSY_MS})`);
const busy = await measure(Math.max(5, Math.ceil(N / 2)), "同上，但处理里先空转 100ms（灵敏度对照）");
await evalJs("window.__lat.setBusy(0)");

/**
 * 打开一次读页面自己那几档，再关掉 —— 证明仿真真的落到了计算样式上，不是只改了 matchMedia。
 * ⚠️ 必须先关再点：上一轮测量结束时弹窗是**开着**的，此时那一点会落在遮罩上把弹窗关掉，
 * 于是读到 "-"（第一版就是这么被骗的：默认那一档报 "-"，reduce 那档反而有数）。
 * ⚠️ 必须在**过渡进行中**读（泵 1 次 ≈ 40ms，还在那 150ms 里）：过渡类名只在做过渡时挂着，
 * 落定之后面板自己的 `transition-duration` 是 `0s`（第一版就是等太久才读，于是把 reduce 判成了空转）。
 */
async function openProbe(label) {
  await evalJs("window.__lat.close()");
  await pump(5);
  const pt = await evalJs("window.__lat.point()");
  await click(pt.x, pt.y);
  await pump(1);
  const j = JSON.parse(await evalJs("JSON.stringify(window.__lat.media())"));
  await evalJs("window.__lat.close()");
  await pump(4);
  if (j.panel === "-") throw new Error(`仿真对照｜${label}：打开状态下没找到面板 ⇒ 探针坏，不拿这个数去判`);
  console.log(`仿真对照｜${label}：${j.probes}`);
  console.log(`           过渡期内的面板：transition=${j.transition} animation=${j.animation} panel=${j.panel}`);
  return j;
}

/**
 * 把 "0.15s, 0.01ms" 这种串解析成最大毫秒数 —— 仿真生效判据要比的是数，不是字符串没变。
 * ⚠️ 单位要先判再 parseFloat：浏览器会给 `.01ms !important` 回 "1e-05s" 这种指数写法，
 * 用 `([0-9.]+)(ms|s)` 去抠会把它读成 "05s" = 5000ms（第一版就这么把 0.01ms 量成了 5 秒）。
 */
function maxMs(str) {
  if (!str || str === "-") return null;
  const parts = String(str)
    .split(",")
    .map((raw) => {
      const s = raw.trim();
      const v = parseFloat(s);
      if (Number.isNaN(v)) return 0;
      return s.endsWith("ms") ? v : v * 1000;
    });
  return parts.length ? Math.max(...parts) : null;
}

const mediaPlain = await openProbe("默认（no-preference）");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
const mediaReduce = await openProbe("reduce 仿真");
const reduceRun = await measure(Math.max(6, Math.ceil(N / 2)), "同上，但 prefers-reduced-motion=reduce");
await send("Emulation.setEmulatedMedia", { features: [] });
const mediaBack = await openProbe("撤掉仿真");

const tPlain = maxMs(mediaPlain.transition);
const tReduce = maxMs(mediaReduce.transition);
const emulated = mediaReduce.probes.includes("(prefers-reduced-motion: reduce) = true");
const backToPlain =
  mediaBack.probes.includes("(prefers-reduced-motion: reduce) = false") &&
  maxMs(mediaBack.transition) === tPlain;
// 仿真有没有落到样式上：同一个元素在 reduce 下的计算时长必须与默认**不同**，且撤掉仿真要退回原值。
// ⚠️ 这条**只证到**「引擎按 prefers-reduced-motion 改了这台的计算样式」。
// 它证不到「那 150ms 的进入过渡被缩短」—— 因为 `duration-150` 这类时长只挂在过渡类名上，
// 落定之后元素自己读回来是 `0s`（默认 0s、reduce 0.01ms 就是这么来的）。
// 要证那一半，得在**离开过渡进行中**读（N13 那套泵帧时机），这台量具现在还没买那一条。
const styleBited = tPlain !== null && tReduce !== null && tReduce !== tPlain;
const reduceWithinTwoFrames = reduceRun.p95 !== null && reduceRun.p95 <= 33.4;

const shift = Math.round((busy.p50 - plain.p50) * 100) / 100;
console.log(
  JSON.stringify(
    {
      说明: "只含 输入被交付 → 处理器跑完 → Vue 改到 DOM → 那一帧；不含网络/数据库/IPC",
      构建: "vite dev（未压缩）",
      基线: plain,
      对照_处理里压一个长任务: busy,
      长任务带来的位移ms: shift,
      量具有没有灵敏度: shift >= BUSY_MS * 0.5 ? "✅ 跟得上（摘掉对照它会红）" : "❌ 不敏感 ⇒ 这条数不能用",
      减少动效: {
        reduce档p50: reduceRun.p50,
        reduce档p95: reduceRun.p95,
        reduce档每窗帧数: reduceRun.framesObserved,
        仿真有没有点亮: emulated ? "✅ matchMedia 报 true" : "❌ 仿真没生效",
        撤掉仿真有没有退回: backToPlain ? "✅ 退回 false" : "❌ 撤不掉 ⇒ 这条判据恒真，不能用",
        仿真落到样式上了吗: `同一元素 transition ${mediaPlain.transition} → ${mediaReduce.transition}（撤掉退回 ${mediaBack.transition}）${
          styleBited ? " ✅ 引擎按偏好改了计算样式" : " ❌ 仿真是空转"
        }`,
        这条买不到的一半:
          "「那 150ms 进入过渡被缩短」没证到 —— duration 只挂在过渡类名上，落定后元素读回来是 0s；要证那一半得在离开过渡进行中读",
        反馈还在两帧内: reduceWithinTwoFrames ? "✅" : `❌ p95=${reduceRun.p95}ms 超两帧`,
      },
    },
    null,
    2,
  ),
);
const ok = shift >= BUSY_MS * 0.5 && emulated && backToPlain && styleBited && reduceWithinTwoFrames;
ws.close();
process.exit(ok ? 0 : 1);
