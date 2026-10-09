// 压测前断言：虚拟化是否真的生效（渲染行数 << 总数、clientHeight 明显小于 scrollHeight）。
// 用法：node perf/probe.mjs 9224
const CDP_PORT = process.argv[2] ?? "9223";
const CDP = `http://127.0.0.1:${CDP_PORT}`;
const list = await (await fetch(`${CDP}/json`)).json();
const page = list.find((t) => t.type === "page" && t.url.includes("vlist.html"));
if (!page) throw new Error("找不到 vlist.html 目标页");
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) (pending.get(m.id)(m), pending.delete(m.id));
});
await new Promise((r) => ws.addEventListener("open", r));
const evalJs = (expression) =>
  new Promise((resolve, reject) => {
    const mid = ++id;
    ws.send(JSON.stringify({ id: mid, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
    pending.set(mid, (m) => (m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result?.result?.value)));
  });

for (let i = 0; i < 40; i++) {
  if (await evalJs("!!window.__perf")) break;
  await new Promise((r) => setTimeout(r, 500));
}
const out = await evalJs(`(async () => {
  const el = document.querySelector("#perf-root .overflow-y-auto");
  if (!el) return { error: "找不到滚动容器" };
  const rows = () => el.querySelectorAll("[data-vlist-key]").length;
  // 量具自检：容器必须真的是「能滚的」。写一次 scrollTop 读不回同样的值 ⇒ 这个元素没有
  // overflow-y:auto（应用样式没加载），后面的所有帧间隔都会是「从未滚动过的空闲帧」——
  // 2026-09-10 那份报告里的“满帧”就是这么来的，所以这一条必须在采数据之前判掉。
  el.scrollTop = 1234;
  const scrollerReal = Math.round(el.scrollTop) === 1234;
  el.scrollTop = 0;
  return {
    n: window.__perf?.n,
    renderedRows: rows(),
    clientHeight: el.clientHeight,
    scrollHeight: el.scrollHeight,
    scrollerReal,
    virtualizationOK: rows() > 0 && rows() < 200,
  };
})()`);
console.log(JSON.stringify(out, null, 2));
ws.close();
