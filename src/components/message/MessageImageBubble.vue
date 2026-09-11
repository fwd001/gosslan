<script setup lang="ts">
import { t } from "@/i18n";
import { ref, watch } from "vue";
import { ImageOff, ImageIcon } from "lucide-vue-next";

const props = defineProps<{ src: string }>();
const emit = defineEmits<{ (e: "open", src: string): void }>();

/** 加载态：先撑出骨架占位，避免大图加载时气泡高度塌陷、列表跳动。 */
const state = ref<"loading" | "loaded" | "failed">("loading");
watch(
  () => props.src,
  () => {
    state.value = "loading";
  },
);
</script>

<template>
  <!-- 图片容器**定宽**（`w-52` = 13rem，与加载骨架同宽）。
       ⚠️ 这**不是**随手写的宽度：此处原先只有 `max-w-full`，图片宽度因此变成
       「父容器剩余宽度」的函数 —— 而群聊已读回执（头像列 + `+N`）是同一 flex 行的
       兄弟节点，回执一出现就占宽、把图片**压小**。用户 2026-09-12 反馈：
       「已读列表和已读的小头像会让图片稍微缩小一下，这是不应该的。图片发出来之后，
       大小应该是固定的。」⇒ 用**定宽**让图片尺寸与兄弟节点无关（`max-w-full`
       仅作为窄窗口下的安全下限保留）。
       代价（已知并接受）：竖长图会在 13rem 的框内左右留白，换来「发出后尺寸恒定」。 -->
  <div
    class="relative w-52 max-w-full cursor-pointer overflow-hidden rounded-[var(--gosslan-bubble-radius)]"
    @click="state === 'loaded' && emit('open', src)"
  >
    <!-- 骨架：加载中占位，尺寸与常见截图相近，加载完成后被图片替换。
         与容器同宽，加载前后不跳变。 -->
    <div
      v-if="state !== 'loaded'"
      class="flex h-32 w-full items-center justify-center bg-black/5 dark:bg-white/5"
    >
      <ImageOff v-if="state === 'failed'" class="h-6 w-6 opacity-50" />
      <ImageIcon v-else class="h-6 w-6 animate-pulse opacity-40" />
    </div>
    <span v-if="state === 'failed'" class="absolute inset-x-0 bottom-1 text-center text-[11px] opacity-70">
      {{ t("msg.imageLoadFailed") }}
    </span>
    <img :alt="t('msg.imageMessage')"
      :src="src"
      class="block max-h-72 w-full rounded-[var(--gosslan-bubble-radius)] object-contain"
      :class="state === 'loaded' ? '' : 'hidden'"
      @load="state = 'loaded'"
      @error="state = 'failed'"
    />
  </div>
</template>
