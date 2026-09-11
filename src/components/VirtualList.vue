<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

// 基于"估算高度"的虚拟滚动列表：适合消息列表（高度可变但有上限）。
// 通过前缀和 + 二分查找定位可视区间，仅渲染可视项，支持向上滚动触发加载更多。
//
// 性能与体验要点：
// - 滚动事件 rAF 节流（每帧至多一次重算，passive 监听不阻塞滚动线程）
// - 仅纵向滚动：容器 overflow-x-hidden，内容超宽由内部元素（代码块）自行处理
// - scrollToIndex：跳到指定消息（打开会话定位第一条未读用），任意方向均无布局抖动
// - prepend 锚定：向上加载历史后按"旧首条消息"锚定滚动位置，视口内容不跳动

const props = withDefaults(
  defineProps<{
    items: any[];
    estimateHeight: (item: any, index?: number) => number;
    overscan?: number;
    /** 切换会话（末条 key 变化）时是否自动贴底；未读跳转场景由父组件关掉，改走 scrollToIndex */
    autoScrollOnSwap?: boolean;
    /**
     * 作为**实时日志区域**播报（用户 2026-09-12 HIG 审查）：聊天列表的核心事件是
     * "来了新消息"，但本组件此前没有任何 live region ⇒ 读屏用户**完全听不到新消息**。
     * 打开后容器带 `role="log" aria-live="polite" aria-relevant="additions"`
     * （只播报新增，不做整体重读；虚拟列表回收旧行不会造成刷屏）。
     * 默认关闭：本组件是通用的，其他用途（如设置里的长列表）不该被当作 live region。
     */
    live?: boolean;
  }>(),
  { overscan: 6, autoScrollOnSwap: true, live: false },
);

const emit = defineEmits<{
  (e: "loadMore"): void;
  (e: "nearBottom", v: boolean): void;
}>();

const container = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewport = ref(600);

// 每个已渲染项的「实测高度」覆盖：图片、附件预览和字体布局都以真实 DOM 为准。
// offsets 用实测值重算，既避免真实内容变高时重叠，也避免估算过大造成假间距。
// 以 msg_id/id 为键（而非数组下标），向上加载历史（prepend）导致下标偏移时覆盖仍对应正确消息。
const heightOverride = new Map<string | number, number>();
const heightVersion = ref(0);

function keyOf(it: any): string | number {
  return it?.msg_id ?? it?.id ?? "";
}

function itemHeight(i: number): number {
  heightVersion.value;
  const it = props.items[i];
  const k = keyOf(it);
  return heightOverride.get(k) ?? props.estimateHeight(it, i);
}

const offsets = computed(() => {
  const arr = new Array<number>(props.items.length + 1);
  arr[0] = 0;
  for (let i = 0; i < props.items.length; i++) {
    arr[i + 1] = arr[i] + itemHeight(i);
  }
  return arr;
});
const totalHeight = computed(() => offsets.value[offsets.value.length - 1] ?? 0);

/** 记录某槽位实测高度，图片加载或预览切换后由 ResizeObserver 及时更新。 */
function commitHeight(i: number, measured: number) {
  if (i < 0 || i >= props.items.length) return;
  // 快速滚动时部分节点可能处于未布局/隐藏状态，offsetHeight 会短暂为 0 或异常值；
  // 一旦把 0 写进 heightOverride，后续消息的 top 会全部塌缩到顶部，表现为消息堆叠。
  if (!Number.isFinite(measured) || measured <= 0) return;
  const it = props.items[i];
  const k = keyOf(it);
  const est = props.estimateHeight(it, i);
  const h = Math.round(measured);
  if (Math.abs((heightOverride.get(k) ?? est) - h) > 1) {
    heightOverride.set(k, h);
    heightVersion.value += 1;
  }
}

function lowerBound(top: number) {
  const arr = offsets.value;
  let lo = 0;
  let hi = arr.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (arr[mid] < top) lo = mid + 1;
    else hi = mid;
  }
  return Math.max(0, lo - 1);
}

const start = computed(() => Math.max(0, lowerBound(scrollTop.value) - props.overscan));
const end = computed(() => {
  const e = lowerBound(scrollTop.value + viewport.value) + props.overscan;
  return Math.min(props.items.length, e + 1);
});
const visible = computed(() => {
  const out: { item: any; index: number; top: number }[] = [];
  for (let i = start.value; i < end.value; i++) {
    out.push({ item: props.items[i], index: i, top: offsets.value[i] });
  }
  return out;
});

// 数据变化 / 窗口变化后，对可见项做一次实测 → 覆盖估算偏差
let remeasureRaf = 0;
let rowResizeObserver: ResizeObserver | null = null;

function observeRenderedRows(nodes: NodeListOf<HTMLElement>) {
  rowResizeObserver?.disconnect();
  rowResizeObserver = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const node = entry.target as HTMLElement;
      const idx = Number(node.getAttribute("data-vlist-index"));
      const key = node.getAttribute("data-vlist-key");
      if (Number.isInteger(idx) && key === String(keyOf(props.items[idx]))) {
        commitHeight(idx, node.offsetHeight);
      }
    }
  });
  nodes.forEach((node) => rowResizeObserver?.observe(node));
}

function scheduleRemeasure() {
  if (remeasureRaf) return;
  remeasureRaf = requestAnimationFrame(() => {
    remeasureRaf = 0;
    const el = container.value;
    if (!el) return;
    const nodes = el.querySelectorAll<HTMLElement>("[data-vlist-index]");
    nodes.forEach((node) => {
      const idx = Number(node.getAttribute("data-vlist-index"));
      commitHeight(idx, node.offsetHeight);
    });
    observeRenderedRows(nodes);
  });
}

// ---------------- 滚动状态机（贴底 / 「回到最新」按钮 的完整逻辑） ----------------
//
// 核心只有一个状态：pinned（是否吸附在底部）。
//   pinned = true  → 新内容自动贴底，「回到最新」按钮隐藏
//   pinned = false → 不再自动贴底，离开底部足够远(>150px)时显示按钮
//
// pinned 的更新只有一个出口：computeScrollState()（滚动 rAF / resize /
// 总高度变化 / 静默期结束补算 都汇聚到这里）。
//
// 两个辅助机制：
// - settling（静默期）：程序化滚动（scrollToBottom/scrollToIndex/切会话换数据）
//   后高度重测会造成 scrollHeight 抖动，期间 nearBottom 不可信 → 不对外广播，
//   静默期结束补算一次，保证最终状态正确、按钮不闪现。
// - 用户上滚豁免：用户向上滚动后 1.2s 内，一切自动贴底（钉底/新消息贴底）让位，
//   否则会和新消息贴底"抢滚动条"，用户根本滚不上去。
//   ⚠ 程序化滚动（贴底/定位/锚定）在静默期内也会移动 scrollTop，必须通过
//   setScrollTop 打标记，computeScrollState 才能把「程序化下移」与「用户上滚」
//   区分开——否则静默期结束的 pinToBottom 会把用户刚滚上去的视口拽回底部。
let raf = 0;
let pinned = true;
let prevScrollTop = 0;
let lastScrollUpAt = 0;
let suppressNearBottomUntil = 0;
let settleTimer = 0;
/** 置位：下一次 computeScrollState 把 scrollTop 下降视为程序化滚动，不计用户上滚。 */
let programmaticTop = false;

/** 距底部 150px 内视为底部：容忍高度估算与实测的抖动 */
const NEAR_BOTTOM_THRESHOLD = 150;

/** 统一入口写 scrollTop：标记为程序化滚动，避免被当成用户上滚。 */
function setScrollTop(v: number) {
  const el = container.value;
  if (!el) return;
  programmaticTop = true;
  el.scrollTop = v;
}

function suppressTransient() {
  suppressNearBottomUntil = Math.max(suppressNearBottomUntil, Date.now() + 300);
}

/** 近 ms 毫秒内用户是否向上滚动过（父组件贴底前应检查）。 */
function recentScrollUp(ms = 1200): boolean {
  return Date.now() - lastScrollUpAt < ms;
}

/** 若处于贴底吸附态，把视口钉到当前真实底部。
 *  估算高度与实测高度的偏差会随重测多次修正，每次总高度变化都要重新跟随，
 *  直到高度收敛 —— 否则会停在「估算底部」（实测底部更远处的某条消息上）。 */
function pinToBottom() {
  const el = container.value;
  if (el && pinned && !recentScrollUp()) setScrollTop(el.scrollHeight);
}

function computeScrollState() {
  const el = container.value;
  if (!el) return;
  const settling = Date.now() < suppressNearBottomUntil;
  // 程序化滚动（贴底/定位/锚定）不算用户上滚；静默期内的真实上滚也必须记录，
  // 否则静默期结束的 pinToBottom 会把用户刚滚上去的视口拽回底部。
  if (!programmaticTop && el.scrollTop < prevScrollTop - 1) lastScrollUpAt = Date.now();
  programmaticTop = false;
  prevScrollTop = el.scrollTop;
  scrollTop.value = el.scrollTop;
  const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_THRESHOLD;
  // 静默期内不广播（按钮会闪现），定时补算一次真实值；补算前先做最终锚定
  if (settling) {
    if (!settleTimer) {
      settleTimer = window.setTimeout(() => {
        settleTimer = 0;
        pinToBottom();
        computeScrollState();
      }, suppressNearBottomUntil - Date.now() + 30);
    }
    return;
  }
  if (nearBottom !== pinned) {
    pinned = nearBottom;
    emit("nearBottom", pinned);
  }
  if (el.scrollTop < 60) emit("loadMore");
}

function onScroll() {
  if (raf) return;
  raf = requestAnimationFrame(() => {
    raf = 0;
    computeScrollState();
  });
}

function onResize() {
  if (container.value) viewport.value = container.value.clientHeight;
  // 尺寸变化同样瞬态影响 nearBottom，纳入静默
  suppressTransient();
  computeScrollState();
}

// ---------------- 定位 ----------------
function scrollToBottom() {
  const el = container.value;
  if (!el) return;
  // 显式贴底 = 贴底意图，恢复吸附态（否则重测偏差没人纠正）
  if (!pinned) {
    pinned = true;
    emit("nearBottom", true);
  }
  suppressTransient();
  setScrollTop(el.scrollHeight);
}

/** 跳到指定消息。align=top：该消息出现在视口顶部（未读定位）；
 *  align=bottom：该消息出现在视口底部（向上引用跳转）。任意方向直接定位，无动画抖动。 */
function scrollToIndex(index: number, align: "top" | "bottom" = "top") {
  const el = container.value;
  if (!el || props.items.length === 0) return;
  // 跳转 = 离开底部（未读定位/引用定位），解除吸附态，防止重测钉底拽回
  if (pinned) {
    pinned = false;
    emit("nearBottom", false);
  }
  suppressTransient();
  const i = Math.max(0, Math.min(index, props.items.length - 1));
  const top = offsets.value[i];
  if (align === "top") {
    setScrollTop(Math.max(0, top - 8));
  } else {
    setScrollTop(Math.max(0, top - el.clientHeight + itemHeight(i)));
  }
  computeScrollState();
}

// ---------------- prepend 锚定（向上加载历史时视口不跳动） ----------------
function itemKey(it: any): string | number {
  return it?.msg_id ?? it?.id ?? "";
}

let prevFirstKey: string | number | null = null;
watch(
  () => props.items,
  (arr) => {
    if (prevFirstKey !== null && arr.length > 0) {
      // 旧首条消息在新数组中的位置 = 新增的历史条数 → 滚动位置补偿同样的高度
      const idx = arr.findIndex((it) => itemKey(it) === prevFirstKey);
      if (idx > 0) {
        const delta = offsets.value[idx];
        void nextTick(() => {
          const el = container.value;
          if (el) setScrollTop(el.scrollTop + delta);
        });
      }
    }
    prevFirstKey = arr.length > 0 ? itemKey(arr[0]) : null;
  },
  { flush: "post" },
);

onMounted(() => {
  prevFirstKey = props.items.length > 0 ? itemKey(props.items[0]) : null;
  onResize();
  window.addEventListener("resize", onResize);
  scheduleRemeasure();
});
onBeforeUnmount(() => {
  rowResizeObserver?.disconnect();
  window.removeEventListener("resize", onResize);
  if (raf) cancelAnimationFrame(raf);
  if (remeasureRaf) cancelAnimationFrame(remeasureRaf);
  if (settleTimer) window.clearTimeout(settleTimer);
});

// 数据或可见窗口变化后重测可见行，收敛估算与实测偏差
watch(visible, () => scheduleRemeasure(), { flush: "post" });
let prevLastKey: string | number | null = null;
watch(
  () => props.items,
  (arr) => {
    // 会话切换/消息增删：内容高度即将大变，静默瞬态期避免「回到最新」闪现
    suppressTransient();
    // 切换会话/重新加载（末条消息 key 变化）时按贴底处理：pinned 立即置真、按钮隐藏，
    // 不沿用上一个会话的旧值；若随后定位不在底部（如未读跳转），
    // 静默期结束的补算会再正确显示。向上加载历史（prepend）只变首条，不影响。
    const newLastKey = arr.length > 0 ? itemKey(arr[arr.length - 1]) : null;
    const isSwap = prevLastKey !== null && newLastKey !== null && newLastKey !== prevLastKey;
    if (isSwap && !pinned) {
      pinned = true;
      emit("nearBottom", true);
    }
    prevLastKey = newLastKey;
    scheduleRemeasure();
    if (isSwap && props.autoScrollOnSwap) {
      // post flush：DOM 已是新会话内容，此刻按真实 scrollHeight 贴底，比 pre-flush
      // 阶段对着旧高度滚的那次准确（修「定位到合并消息首行」）
      void nextTick(() => {
        const el = container.value;
        if (!el) return;
        suppressTransient();
        setScrollTop(el.scrollHeight);
        computeScrollState();
      });
    }
  },
  { flush: "post" },
);

// 总高度变化（实测修正估算 / 追加消息）后：
// pinned 时无条件钉住贴底 —— 关键：重测期间估算→实测的高度差必须持续跟随，
// 否则切会话后会停在"估算底部"（实测底部更远），表现为定位到中间某条消息。
// 防误伤交给两个条件：用户上滚豁免（recentScrollUp）+ 外部显式 setPinned(false)
// （未读跳转 / 引用定位后由父组件解除贴底）。
watch(totalHeight, () => {
  void nextTick(() => {
    const el = container.value;
    if (!el) return;
    if (pinned && !recentScrollUp()) setScrollTop(el.scrollHeight);
    computeScrollState();
  });
});

/** 外部显式设定贴底态（未读跳转/引用定位后置 false，点击回到最新置 true）。 */
function setPinned(v: boolean) {
  pinned = v;
  emit("nearBottom", pinned);
}

defineExpose({ scrollToBottom, scrollToIndex, recentScrollUp, setPinned });
</script>

<template>
  <div
    ref="container"
    class="h-full overflow-y-auto overflow-x-hidden pb-6"
    :role="live ? 'log' : undefined"
    :aria-live="live ? 'polite' : undefined"
    :aria-relevant="live ? 'additions' : undefined"
    @scroll.passive="onScroll"
  >
    <div :style="{ height: `${totalHeight}px`, position: 'relative' }">
      <div
        v-for="v in visible"
        :key="(v.item as any).msg_id ?? v.index"
        :data-vlist-index="v.index"
        :data-vlist-key="String((v.item as any).msg_id ?? (v.item as any).id ?? '')"
        :style="{ position: 'absolute', top: `${v.top}px`, left: 0, right: 0 }"
      >
        <slot :item="v.item" :index="v.index" />
      </div>
    </div>
  </div>
</template>
