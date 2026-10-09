// VirtualList 压测页（手动/自动化皆可）：加载**真实的** VirtualList 组件与**真实的**高度估算函数，
// 灌入 5 万 / 10 万条合成消息，测量滚动流畅度与定位精准度。
//
// 用法：npm run dev 后打开 /perf/vlist.html?n=100000，页面暴露 window.__perf（见 perf/run.mjs）。
//
// 与真实聊天页的差异（跑的是同一段代码，成本略有不同）：
//   - 行模板用等价结构（头像 + 昵称 + 气泡），未挂 MessageItem 的交互/右键/引用逻辑；
//   - 没有 ChatHeader/输入框，视口略大。这两点只会让数据偏乐观，不影响结论方向。

import { createApp, defineComponent, h, onMounted, ref } from "vue";
import { createPinia } from "pinia";
import VirtualList from "@/components/VirtualList.vue";
import { estimateMessageHeight } from "@/utils/messageHeight";
// ⚠️ 必须显式加载应用样式（与 `src/boot/boot.ts` 同一份）。漏掉它的后果不是"不好看"，而是**量具失效**：
// VirtualList 的滚动容器靠 `overflow-y-auto` 才成为滚动容器，而 Tailwind 的工具类只存在于这份 CSS 里。
// 没有它 ⇒ `scrollTop` 写入被浏览器整体忽略 ⇒ 压测跑的是"从未滚动过的空闲帧"。
import "@/style.css";

const params = new URLSearchParams(location.search);
const N = Math.max(1, Number(params.get("n") ?? 100000));
/**
 * 行模板选哪一套（roadmap N21 那一格的前置：消息行的真实成本从来没量过，
 * 因为 vlist 台的行模板是「等价结构」而不是真 `MessageItem`）。
 *   synthetic      —— 原来的等价结构（头像 + 昵称 + 气泡），历史帧数都在这条上量的
 *   msgitem        —— **真的 `MessageItem.vue`**，props 按 ChatWindow 那个调用点的形状**每次渲染现构造**
 *                     （`groupReaderIds` / `reactions` 都是新数组 ⇒ 浅比较恒判「变了」，正是 N21 说的形状）
 *   msgitem-stable —— 同上，但每个 msg_id **复用同一个 props 对象**（= 补 v-memo 的收益上界）
 *                     ⇒ 这一档量的是「补 v-memo 最多能省下多少」的**上界**（不动应用码）
 */
const ROW = params.get("row") ?? "synthetic";
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

/**
 * 第四档要回答的问题与第三档不同：第三档（stable）证的是「props 对象恒新」那一半的**上界**，
 * 而生产里真正恒新的只有**两条数组值 prop**（`ChatWindow.vue:1259` 的 `groupReaderIds()` 每次 `.map()`
 * 出新数组、`:1264` 的 `reactionMap.get(...) ?? []` 在没有回应时是新空数组）。
 * 所以这一档**只把那两条换成共享常量**（props 对象仍然每次现构造）⇒ 它量的就是
 * 「不动 v-memo、只把空数组身份收成一个家」能省下多少 —— 那是一处小得多、风险也低得多的改动
 * （消费者只读不写：`MessageReactionBar` 用 `find`/`v-for`，`MessageReceipt` 用 `slice`/`length`）。
 */
const EMPTY_IDS: string[] = Object.freeze([]) as never[];
const EMPTY_CHIPS: never[] = Object.freeze([]) as never[];

/** 每档各自的一份 props 构造：只有数组值那条不同，其余逐字相同。 */
function msgItemPropsWith(item: Item, index: number, freshArrays: boolean) {
  return {
    message: item as never,
    prev: (index > 0 ? items[index - 1] : null) as never,
    isGroup: true,
    canReact: true,
    senderName: `用户${index % 97}`,
    // 这两条故意现构造（与生产同形：`groupReaderIds()` 每次返回新数组、`reactionMap.get() ?? []` 同理）
    // —— N21 判的就是"浅比较恒判变了 ⇒ 整棵子树重新 patch"到底值多少钱。
    groupReaderIds: freshArrays ? [] as string[] : EMPTY_IDS,
    reactions: freshArrays ? [] as never[] : EMPTY_CHIPS,
    mentionNames: freshArrays ? [] as string[] : EMPTY_IDS,
    showUnreadDivider: false,
    highlightId: null,
    selfMention: null,
    pinned: false,
    selectMode: false,
    selected: false,
  };
}

/**
 * 第三档原本是 `memo()`，但这个 Vue 版本**没有这个导出**（2026-10-10 现跑：页面
 * `SyntaxError: The requested module ...vue.js does not provide an export named memo`）。
 * 那次报错还顺带解释了为什么前面**三档一起挂**：静态 import 一个不存在的导出会杀掉整个模块，
 * 连 `window.__perf` 都赋值不上 —— 三档一样的失败不等于"三档结果相同"，是**探针没生效**。
 *
 * 而 `v-memo` 真正买到的东西就是「依赖不变 ⇒ 同一份 props/subtree 不再 patch」，
 * 所以这一档直接**按 msg_id 复用同一个 props 对象**：那是那处改动的收益上界，且不动应用码。
 */
let MessageItemCmp: unknown = null;
const propsCache = new Map<string, unknown>();

function stableMsgItemProps(item: Item, index: number) {
  let p = propsCache.get(item.msg_id);
  if (!p) {
    p = msgItemPropsWith(item, index, true);
    propsCache.set(item.msg_id, p);
  }
  return p;
}

if (ROW !== "synthetic") {
  MessageItemCmp = (await import("@/components/MessageItem.vue")).default;
}

function syntheticRow(item: Item, index: number) {
  return h("div", { class: "pl-row" }, [
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
  ]);
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
            ROW === "msgitem"
              ? h(MessageItemCmp as never, msgItemPropsWith(item, index, true) as never)
              : ROW === "msgitem-const"
                ? h(MessageItemCmp as never, msgItemPropsWith(item, index, false) as never)
                : ROW === "msgitem-stable"
                ? h(MessageItemCmp as never, stableMsgItemProps(item, index) as never)
                : syntheticRow(item, index),
        },
      );
  },
});

// 真 MessageItem 会读 useAppStore()/useChatStore() ⇒ 没有 pinia 实例就直接抛；
// 合成行那一档不需要，但装上无害（与 src/boot/boot.ts:127 同一句写法）。
createApp(App).use(createPinia()).mount("#perf-root");

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
