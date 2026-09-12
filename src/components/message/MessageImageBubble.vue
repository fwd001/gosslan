<script setup lang="ts">
import { t } from "@/i18n";
import { computed, onUnmounted, ref, watch } from "vue";
import { ImageOff, ImageIcon } from "lucide-vue-next";

const props = defineProps<{ src: string }>();
const emit = defineEmits<{ (e: "open", src: string): void }>();

/** 加载态：先撑出骨架占位，避免大图加载时气泡高度塌陷、列表跳动。 */
const state = ref<"loading" | "loaded" | "failed">("loading");
/**
 * 加载尝试次数与"防缓存"参数。
 *
 * 用户 2026-09-12 实测（收图侧）：「刚收到时显示『图片加载失败』，过一会儿图片才出来 ——
 * 中间到底在干什么？合理的逻辑是：能加载了才弹出来，或者给我一个『加载中』，
 * 而『加载失败』应该是**不可逆的最终结果**。」
 *
 * 根因：收到图片消息时**文件可能还在传输/落盘**，此时 `<img src>` 直接报 error，
 * 而 `@error` 把状态钉死成 failed（此前只在 `src` 变化时才重置）。
 * 现在：报错后先按退避重试（0.4s→0.8s→1.6s→…，总计约 10s），期间保持"加载中"，
 * 只有重试全部失败才显示"加载失败"（并且可以点一下手动重试）。
 */
const RETRY_DELAYS_MS = [400, 800, 1600, 2400, 3200];
const attempt = ref(0);
let timer: ReturnType<typeof setTimeout> | null = null;

function clearTimer() {
  if (timer !== null) clearTimeout(timer);
  timer = null;
}

function onError() {
  if (attempt.value < RETRY_DELAYS_MS.length) {
    const delay = RETRY_DELAYS_MS[attempt.value];
    attempt.value += 1;
    // 状态保持 loading：绝不在"可能马上就好"的时候告诉用户失败
    state.value = "loading";
    clearTimer();
    timer = setTimeout(() => {
      // 换一个查询串强制重新请求（同 URL 的失败会被浏览器缓存/复用）
      loadKey.value = attempt.value;
    }, delay);
    return;
  }
  state.value = "failed";
}

/** 手动重试（点一下"加载失败"的气泡）。 */
function retry() {
  attempt.value = 0;
  state.value = "loading";
  loadKey.value += 1;
}

const loadKey = ref(0);
const effectiveSrc = computed(() => {
  if (loadKey.value === 0) return props.src;
  const sep = props.src.includes("?") ? "&" : "?";
  return `${props.src}${sep}r=${loadKey.value}`;
});

watch(
  () => props.src,
  () => {
    clearTimer();
    attempt.value = 0;
    state.value = "loading";
  },
);
onUnmounted(clearTimer);
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
    :role="state === 'loaded' ? 'button' : undefined"
    :tabindex="state === 'loaded' ? 0 : undefined"
    :aria-label="state === 'loaded' ? t('msg.clickToOpen') : undefined"
    @click="state === 'loaded' ? emit('open', src) : state === 'failed' && retry()"
    @keydown.enter.prevent="state === 'loaded' && emit('open', src)"
    @keydown.space.prevent="state === 'loaded' && emit('open', src)"
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
      :src="effectiveSrc"
      class="block max-h-72 w-full rounded-[var(--gosslan-bubble-radius)] object-contain"
      :class="state === 'loaded' ? '' : 'hidden'"
      @load="clearTimer(); state = 'loaded'"
      @error="onError()"
    />
  </div>
</template>
