// VirtualList 压测页（手动/自动化皆可）：加载**真实的** VirtualList 组件与**真实的**高度估算函数，
// 灌入 5 万 / 10 万条合成消息，测量滚动流畅度与定位精准度。
//
// 用法：npm run dev 后打开 /perf/vlist.html?n=100000，页面暴露 window.__perf（见 perf/run.mjs）。
//
// 与真实聊天页的差异（跑的是同一段代码，成本略有不同）：
//   - 行模板用等价结构（头像 + 昵称 + 气泡），未挂 MessageItem 的交互/右键/引用逻辑；
//   - 没有 ChatHeader/输入框，视口略大。这两点只会让数据偏乐观，不影响结论方向。

import { createApp, defineComponent, h, onMounted, ref } from "vue";
import VirtualList from "@/components/VirtualList.vue";
import { estimateMessageHeight } from "@/utils/messageHeight";
// ⚠️ 必须显式加载应用样式（与 `src/boot/boot.ts` 同一份）。漏掉它的后果不是"不好看"，而是**量具失效**：
// VirtualList 的滚动容器靠 `overflow-y-auto` 才成为滚动容器，而 Tailwind 的工具类只存在于这份 CSS 里。
// 没有它 ⇒ `scrollTop` 写入被浏览器整体忽略 ⇒ 压测跑的是"从未滚动过的空闲帧"。
import "@/style.css";

const params = new URLSearchParams(location.search);
const N = Math.max(1, Number(params.get("n") ?? 100000));
/** `?fit=1` ⇒ 容器高度跟着视口走（默认写死 700px：历史帧数要能同条件横向比）。 */
const FIT = params.get("fit") === "1";

/** 合成消息：文本/代码/图片/文件混合——高度估算对不同 kind 的成本差别很大。 */
function makeItems(n: number) {
  const out = [];
  for (let i = 0; i < n; i++) {
    const mod = i % 10;
    let kind: "text" | "code" | "image" | "file" = "text";
    let content = `第 ${i + 1} 条消息：用于压测的普通文本，长度不一。`.repeat((i % 3) + 1);
    if (mod === 7) {
      kind = "code";
      content = `function f${i}(x) {\n  return x * ${i};\n}`;
    } else if (mod === 8) {
      kind = "image";
      content = JSON.stringify({ name: `p${i}.png`, path: `/tmp/p${i}.png`, size: 12345, subtype: "image" });
    } else if (mod === 9) {
      kind = "file";
      content = JSON.stringify({ name: `文档${i}.pdf`, path: `/tmp/d${i}.pdf`, size: 654321 });
    }
    out.push({
      msg_id: `m-${i}`,
      conv_id: "perf",
      sender_id: i % 2 === 0 ? "peer" : "me",
      receiver_id: "peer",
      kind,
      content,
      ts: 1700000000000 + i * 1000,
      seq: i + 1,
    });
  }
  return out;
}
type Item = ReturnType<typeof makeItems>[number];

const items = makeItems(N);
const ctx = { messages: items, isGroup: true, selfId: "me", fontSize: "md" as const };

/** estimateHeight 调用次数 = offsets 重算成本：每次重算整表都会调 N 次。 */
let estimateCalls = 0;
function estimateHeight(it: Item, index: number) {
  estimateCalls += 1;
  return estimateMessageHeight(it as never, index, ctx as never);
}

interface VListApi {
  scrollToIndex: (i: number, align?: "top" | "bottom") => void;
  scrollToBottom: () => void;
}
interface PerfApi {
  n: number;
  scrollTest: (steps?: number, stepPx?: number, fromBottom?: boolean) => Promise<unknown>;
  accuracy: (index: number, align?: "top" | "bottom") => Promise<unknown>;
  calibrate: (reps?: number) => unknown;
  snapshot: () => unknown;
  pinBottom: () => void;
  resetCounters: () => void;
}

const App = defineComponent({
  setup(_props, { expose }) {
    const listRef = ref<VListApi | null>(null);

    function scroller(): HTMLElement {
      const el = document.querySelector<HTMLElement>("#perf-root .overflow-y-auto");
      if (!el) throw new Error("找不到 VirtualList 滚动容器");
      return el;
    }
    const nextFrame = () => new Promise((r) => requestAnimationFrame(() => r(null)));
    const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

    /**
     * 连续滚动：在 rAF 里按固定步长推进 scrollTop，记录每帧间隔。
     * 测的是**渲染 + 布局 + offsets 重算**的成本（真实触摸滚动的差别在事件频率，不在计算量）。
     */
    async function scrollTest(steps = 300, stepPx = 800, fromBottom = true) {
      const el = scroller();
      el.scrollTop = fromBottom ? el.scrollHeight : 0;
      await nextFrame();
      estimateCalls = 0;
      const frames: number[] = [];
      let last = performance.now();
      const t0 = last;
      for (let s = 0; s < steps; s++) {
        el.scrollTop = fromBottom ? el.scrollTop - stepPx : el.scrollTop + stepPx;
        await nextFrame();
        const now = performance.now();
        frames.push(now - last);
        last = now;
      }
      const total = performance.now() - t0;
      const sorted = [...frames].sort((a, b) => a - b);
      const pct = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0;
      return {
        steps,
        scrolledPx: steps * stepPx,
        wallMs: Math.round(total),
        avgFrameMs: +(total / steps).toFixed(1),
        p50: +pct(0.5).toFixed(1),
        p95: +pct(0.95).toFixed(1),
        maxFrameMs: +Math.max(...frames).toFixed(1),
        longFramesOver50ms: frames.filter((f) => f > 50).length,
        estimateCalls,
        offsetsRebuilds: +(estimateCalls / items.length).toFixed(2),
      };
    }

    /** 定位精准度：用组件自己的 scrollToIndex，核对该下标是否落在视口内、行间有无重叠/空隙。 */
    async function accuracy(index: number, align: "top" | "bottom" = "top") {
      const el = scroller();
      listRef.value?.scrollToIndex(index, align);
      await nextFrame();
      await sleep(150);
      const rows = Array.from(el.querySelectorAll<HTMLElement>("[data-vlist-key]"))
        .map((n) => ({
          key: n.getAttribute("data-vlist-key") ?? "",
          top: Number.parseFloat(n.style.top || "0"),
          h: n.offsetHeight,
        }))
        .sort((a, b) => a.top - b.top);
      let overlap = 0;
      let gap = 0;
      for (let i = 1; i < rows.length; i++) {
        const prevBottom = rows[i - 1].top + rows[i - 1].h;
        if (rows[i].top < prevBottom - 1) overlap += 1;
        else if (rows[i].top > prevBottom + 1) gap += 1;
      }
      const idxOf = (k: string) => Number.parseInt(k.replace("m-", ""), 10);
      const target = rows.find((r) => idxOf(r.key) === index);
      return {
        askedIndex: index,
        renderedRows: rows.length,
        firstIdx: rows.length ? idxOf(rows[0].key) : -1,
        lastIdx: rows.length ? idxOf(rows[rows.length - 1].key) : -1,
        targetRendered: !!target,
        targetTopInViewport: target ? Math.round(target.top - el.scrollTop) : null,
        overlapPairs: overlap,
        gapPairs: gap,
        scrollHeight: el.scrollHeight,
        clientHeight: el.clientHeight,
        avgRowHeight: rows.length ? +(rows.reduce((s, r) => s + r.h, 0) / rows.length).toFixed(1) : 0,
      };
    }

    /**
     * 「整表前缀和里估算那一段」的直接耗时：热缓存下把 n 条各估一次并计时。
     *
     * 为什么要单独量这个（不是把 scrollTest 的帧时间拆开看）：帧时间是**总量**，
     * 而 N10 要判的是"每帧一次整表重算"里有多少落在估算上 —— 只有直接量才知道
     * "少重算几次"能省下多少（省不下就不许动 VirtualList，约束 7）。
     *
     * ⚠️ 这是**下界**：VirtualList 的重算循环里每条还多一次响应式 ref 读取
     * （`heightVersion.value`）+ 一次 `heightOverride` Map 查询 + 一次 `keyOf`，
     * 这些不经过本函数 ⇒ 真实整表成本 ≥ 这里报的数。
     * ⚠️ 跑完要把 `estimateCalls` 清零：这 n×reps 次调用不属于任何一帧，
     * 留着会把 `offsetsRebuilds`（= estimateCalls / n）撑成假数。
     */
    function calibrate(reps = 5) {
      const runs: number[] = [];
      for (let r = 0; r < reps; r++) {
        const t0 = performance.now();
        for (let i = 0; i < items.length; i++) estimateHeight(items[i], i);
        runs.push(performance.now() - t0);
      }
      estimateCalls = 0;
      const sorted = [...runs].sort((a, b) => a - b);
      const med = sorted[Math.floor(sorted.length / 2)] ?? 0;
      return {
        reps,
        onePassMs: +med.toFixed(2),
        minMs: +Math.min(...runs).toFixed(2),
        maxMs: +Math.max(...runs).toFixed(2),
        perItemUs: +((med * 1000) / items.length).toFixed(2),
      };
    }

    /**
     * 「我正在读的那一条还在不在原位」的几何读数（只读，不改任何东西）。
     *
     * 为什么要 rect 而不是 `style.top`：这一格要判的就是**屏幕上那一行有没有跳**，
     * 而 `style.top` 是组件自己写的偏移量——它和 rect 一起坏的时候，只读 style.top 会跟着一起骗人。
     * 锚点取「跨过视口上沿的那一条」+ 它上沿相对视口上沿的偏移，两者都稳住才算没跳。
     */
    function snapshot() {
      const el = scroller();
      const cr = el.getBoundingClientRect();
      const rows = Array.from(el.querySelectorAll<HTMLElement>("[data-vlist-key]"));
      let anchorKey: string | null = null;
      let anchorOffset: number | null = null;
      for (const r of rows) {
        const b = r.getBoundingClientRect();
        if (b.top <= cr.top + 0.5 && b.bottom > cr.top + 0.5) {
          anchorKey = r.getAttribute("data-vlist-key");
          anchorOffset = Math.round(b.top - cr.top);
          break;
        }
      }
      return {
        innerHeight: window.innerHeight,
        clientHeight: el.clientHeight,
        scrollTop: Math.round(el.scrollTop),
        scrollHeight: el.scrollHeight,
        renderedRows: rows.length,
        anchorKey,
        anchorOffset,
        bottomGap: Math.round(el.scrollHeight - el.scrollTop - el.clientHeight),
      };
    }

    /** 贴底态（聊天页最常见的那一态）：走组件自己的 scrollToBottom，好让 pinned 被点亮。 */
    function pinBottom() {
      listRef.value?.scrollToBottom();
    }

    expose({ scrollTest, accuracy, calibrate, snapshot, pinBottom });
    // ⚠️ 必须给滚动容器**确定高度**，否则它的 clientHeight 会等于内容高度 ——
    // 而"可视区高度"正是虚拟化的输入：高度不约束 → 全部行都算"可见" → 5 万/10 万行全进 DOM。
    // 这里直接写在容器元素上（不走 h-full 类），避免依赖 Tailwind 是否扫到本页。
    onMounted(() => {
      const el = scroller();
      el.style.height = FIT ? `${Math.max(240, window.innerHeight - 24)}px` : "700px";
      // fit 模式下由 harness 把"窗口变了"翻译成"容器高了/矮了"——真实聊天页那一层的容器高度
      // 本来就是窗口尺寸的 flex 结果；这里只借这一段布局效应，被测的仍是组件自己的 ResizeObserver。
      if (FIT) {
        window.addEventListener("resize", () => {
          scroller().style.height = `${Math.max(240, window.innerHeight - 24)}px`;
        });
      }
    });
    return () =>
      h(
        VirtualList,
        { ref: listRef, items: items as never, estimateHeight: estimateHeight as never, "auto-scroll-on-swap": false } as never,
        {
          default: ({ item, index }: { item: Item; index: number }) =>
            h("div", { class: "pl-row" }, [
              h("div", { class: "pl-ava" }, item.sender_id === "me" ? "我" : "A"),
              h("div", { class: "pl-col" }, [
                h("div", { class: "pl-name" }, `用户${index % 97}`),
                // 行形态与「高度估算」的假设对齐（图片 288 / 文件 92），否则实测与估算严重不符，
                // 测出来的"重叠/空隙"只是 harness 自己的 artifact。
                h(
                  "div",
                  { class: "pl-bubble", style: item.kind === "image" ? "height:272px" : item.kind === "file" ? "height:76px" : "" },
                  item.kind === "image" ? "［图片］" : item.kind === "file" ? "［文件］" : item.content.slice(0, 120),
                ),
              ]),
            ]),
        },
      );
  },
});

createApp(App).mount("#perf-root");

// app 实例上的 expose 挂在根组件实例上：通过 __vue_app__ 取回
const mountEl = document.querySelector("#perf-root") as HTMLElement & { __vue_app__?: { _instance?: { exposed?: PerfApi } } };
const exposed = mountEl.__vue_app__?._instance?.exposed;
(window as unknown as { __perf: unknown }).__perf = {
  n: N,
  resetCounters: () => {
    estimateCalls = 0;
  },
  scrollTest: exposed?.scrollTest,
  accuracy: exposed?.accuracy,
  calibrate: exposed?.calibrate,
  snapshot: exposed?.snapshot,
  pinBottom: exposed?.pinBottom,
} satisfies PerfApi & { n: number };
