<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

// 基于“估算高度”的虚拟滚动列表：适合消息列表（高度可变但有上限）。
// 通过前缀和 + 二分查找定位可视区间，仅渲染可视项，支持向上滚动触发加载更多。

const props = withDefaults(
  defineProps<{
    items: any[];
    estimateHeight: (item: any) => number;
    overscan?: number;
  }>(),
  { overscan: 6 },
);

const emit = defineEmits<{ (e: "loadMore"): void }>();

const container = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewport = ref(600);

const offsets = computed(() => {
  const arr = new Array<number>(props.items.length + 1);
  arr[0] = 0;
  for (let i = 0; i < props.items.length; i++) {
    arr[i + 1] = arr[i] + props.estimateHeight(props.items[i]);
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

function onScroll() {
  const el = container.value;
  if (!el) return;
  scrollTop.value = el.scrollTop;
  if (el.scrollTop < 60) emit("loadMore");
}
function onResize() {
  if (container.value) viewport.value = container.value.clientHeight;
}
function scrollToBottom() {
  const el = container.value;
  if (el) el.scrollTop = el.scrollHeight;
}

onMounted(() => {
  onResize();
  window.addEventListener("resize", onResize);
});
onBeforeUnmount(() => window.removeEventListener("resize", onResize));

defineExpose({ scrollToBottom });
</script>

<template>
  <div ref="container" class="h-full overflow-y-auto" @scroll="onScroll">
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
