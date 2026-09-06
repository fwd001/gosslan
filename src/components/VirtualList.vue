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
  }>(),
  { overscan: 6 },
);

const emit = defineEmits<{
  (e: "loadMore"): void;
  (e: "nearBottom", v: boolean): void;
}>();

const container = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewport = ref(600);

const offsets = computed(() => {
  const arr = new Array<number>(props.items.length + 1);
  arr[0] = 0;
  for (let i = 0; i < props.items.length; i++) {
    arr[i + 1] = arr[i] + props.estimateHeight(props.items[i], i);
  }
  return arr;
});
const totalHeight = computed(() => offsets.value[offsets.value.length - 1] ?? 0);

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

// ---------------- 滚动（rAF 节流） ----------------
let raf = 0;
let lastNearBottom = true;

function computeScrollState() {
  const el = container.value;
  if (!el) return;
  scrollTop.value = el.scrollTop;
  const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  if (nearBottom !== lastNearBottom) {
    lastNearBottom = nearBottom;
    emit("nearBottom", nearBottom);
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
  computeScrollState();
}

// ---------------- 定位 ----------------
function scrollToBottom() {
  const el = container.value;
  if (el) el.scrollTop = el.scrollHeight;
}

/** 跳到指定消息。align=top：该消息出现在视口顶部（未读定位）；
 *  align=bottom：该消息出现在视口底部（向上引用跳转）。任意方向直接定位，无动画抖动。 */
function scrollToIndex(index: number, align: "top" | "bottom" = "top") {
  const el = container.value;
  if (!el || props.items.length === 0) return;
  const i = Math.max(0, Math.min(index, props.items.length - 1));
  const top = offsets.value[i];
  if (align === "top") {
    el.scrollTop = Math.max(0, top - 8);
  } else {
    el.scrollTop = Math.max(0, top - el.clientHeight + props.estimateHeight(props.items[i]));
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
          if (container.value) container.value.scrollTop += delta;
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
});
onBeforeUnmount(() => {
  window.removeEventListener("resize", onResize);
  if (raf) cancelAnimationFrame(raf);
});

defineExpose({ scrollToBottom, scrollToIndex });
</script>

<template>
  <div ref="container" class="h-full overflow-y-auto overflow-x-hidden pb-6" @scroll.passive="onScroll">
    <div :style="{ height: `${totalHeight}px`, position: 'relative' }">
      <div
        v-for="v in visible"
        :key="(v.item as any).msg_id ?? v.index"
        :style="{ position: 'absolute', top: `${v.top}px`, left: 0, right: 0 }"
      >
        <slot :item="v.item" :index="v.index" />
      </div>
    </div>
  </div>
</template>
