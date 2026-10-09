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
    },
    null,
    2,
  ),
);
ws.close();
process.exit(shift >= BUSY_MS * 0.5 ? 0 : 1);
