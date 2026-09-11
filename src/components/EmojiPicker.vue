<script setup lang="ts">
import { EMOJIS } from "@/utils/emoji";

defineProps<{ open: boolean }>();
const emit = defineEmits<{
  /** 选中表情 → 输出 token 语法（如 "[微笑]"），由输入框插入。 */
  (e: "select", displayName: string): void;
  (e: "close"): void;
}>();
</script>

<template>
  <!-- 抖音式表情面板：单分类、上下滚动；正方形格子 + object-contain 保持原图比例。
       ⚠️ 面板宽度与网格是**精确配合**的：
         内宽 = 360 - 16(p-2 左右各 8) = 344 ≈ 8 列 × 32(h-8/w-8) + 7 间隙 × 12(gap-3)
       改格子尺寸或间隙时必须同步改面板宽度，否则 8 列 1fr 会把固定 36px 的格子挤到溢出、
       表情互相重叠（列宽由 1fr 决定，格子却是固定 px）。
       间隙由 4px 提到 8px 的原因：原间隙只有表情宽度的 1/9，整片网格看起来"贴死"很挤；
       8px 后留白翻倍，而可视高度 300 内仍是 7 行（36 + 44×6 = 300，正好 7 行）不损失行数。 -->
  <div
    v-if="open"
    class="frost absolute bottom-full left-0 z-50 mb-2 w-[min(360px,calc(100vw-2rem))] select-none rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] shadow-xl"
    @click.stop
  >
    <div
      class="grid grid-cols-8 content-start gap-3 overflow-y-auto p-2"
      style="height: 300px"
    >
      <button
        v-for="e in EMOJIS"
        :key="e.file"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)]"
        :title="e.displayName"
        @click="emit('select', e.displayName)"
      >
        <img
          :src="e.url"
          :alt="e.name"
          class="h-full w-full object-contain"
          draggable="false"
        />
      </button>
    </div>
  </div>
</template>
