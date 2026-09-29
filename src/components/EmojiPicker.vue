<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { EMOJIS, type EmojiDef } from "@/utils/emoji";
import {
  readEmojiUsage,
  topEmojiTokens,
  withEmojiUse,
  writeEmojiUsage,
  type EmojiUsage,
} from "@/utils/emojiUsage";
import { t } from "@/i18n";

const props = withDefaults(
  defineProps<{
    open: boolean;
    /**
     * 相对**定位父级**展开的方向：`above` 向上（输入框上方的表情按钮用），
     * `below` 向下（消息上的表情回应入口用 —— 它在视口上半部分时要往下弹，否则会被顶出屏幕）。
     */
    placement?: "above" | "below";
  }>(),
  { placement: "above" },
);
const emit = defineEmits<{
  /** 选中表情 → 输出 token 语法（如 "[微笑]"），由输入框插入。 */
  (e: "select", displayName: string): void;
  (e: "close"): void;
}>();

/**
 * 键盘出口（原生感走查 N1）：面板是一个**普通浮层**，不是 `BaseModal`
 * （后者是 HeadlessUI 的 `Dialog`，Esc 与焦点管理是自带的）。所以这里必须自己补齐三件事，
 * 否则键盘用户打开面板后就出不去、也走不了格子：
 *   ① Esc 关闭；② 方向键在格子里移动（八列，见下面 COLS）；
 *   ③ 关闭后把焦点**还给打开它的那个按钮** —— 不还的话焦点掉回 <body>，
 *      键盘用户下一次按 Tab 要从页面开头重走（读屏用户等于丢了位置）。
 */
const COLS = 8; // ⚠️ 必须与模板里的 `grid-cols-8` 同步：改了列数不改这里，上下键会跳错行
const panelRef = ref<HTMLDivElement | null>(null);
let restoreFocusTo: HTMLElement | null = null;

/**
 * 「最常使用」那一格（用户 2026-09-29：「表情选择加一个最常使用，算法你来定」）。
 *
 * 做法是**把最常用的几个挪到同一个网格的最前面**（微信/飞书式），而不是再开第二块网格：
 * 上面那个 `COLS` 是写死的键盘行宽，它只对"一个 grid 容器"成立 —— 常用行单独成网格的话，
 * ↑↓ 的落点就会算错，而这在界面上只表现为"有点不对"，很难被发现。
 * 格子总数因此不变（`emojiUsage.test.ts` 那条按形状数的判据钉的就是这一点）。
 *
 * 计数记在**面板自己**的选中出口上：两处入口（输入框插入、消息表情回应）用的是同一个组件，
 * 在这一层记一次就够，不用调用点各写一遍（那样必然会漏掉其中一个）。
 */
const storage = typeof localStorage === "undefined" ? null : localStorage;
const usage = ref<EmojiUsage>(readEmojiUsage(storage));
const defByToken = new Map(EMOJIS.map((e) => [e.displayName, e]));

/** 账里的 token 可能指向已被删掉的表情（换过表情资源）：取不到定义就当没这回事。 */
const headDefs = computed(() =>
  topEmojiTokens(usage.value)
    .map((token) => defByToken.get(token))
    .filter((d): d is EmojiDef => !!d),
);
const headFiles = computed(() => new Set(headDefs.value.map((d) => d.file)));
const cells = computed(() => [
  ...headDefs.value,
  ...EMOJIS.filter((e) => !headFiles.value.has(e.file)),
]);
function isFrequent(e: EmojiDef): boolean {
  return headFiles.value.has(e.file);
}
function labelOf(e: EmojiDef): string {
  return isFrequent(e) ? `${t("emoji.frequent")} · ${e.displayName}` : e.displayName;
}

function pick(e: EmojiDef) {
  usage.value = withEmojiUse(usage.value, e.displayName, Date.now());
  writeEmojiUsage(storage, usage.value);
  emit("select", e.displayName);
}

function buttons(): HTMLButtonElement[] {
  return Array.from(panelRef.value?.querySelectorAll<HTMLButtonElement>("button") ?? []);
}
function focusAt(list: HTMLButtonElement[], idx: number) {
  const at = Math.max(0, Math.min(list.length - 1, idx));
  list[at]?.focus();
  list[at]?.scrollIntoView({ block: "nearest" });
}
function onKey(e: KeyboardEvent) {
  const list = buttons();
  if (!list.length) return;
  const cur = list.indexOf(document.activeElement as HTMLButtonElement);
  if (e.key === "Escape") {
    e.preventDefault();
    emit("close");
    return;
  }
  if (cur < 0) return; // 焦点不在格子上（比如刚打开还没落点）⇒ 不抢方向键
  const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLS, ArrowUp: -COLS }[e.key];
  if (step === undefined) return;
  e.preventDefault(); // 不拦的话上下键会滚整页 / 光标在输入框里跳走
  focusAt(list, cur + step);
}

watch(
  () => props.open,
  async (open) => {
    if (open) {
      restoreFocusTo = document.activeElement as HTMLElement | null;
      await nextTick();
      focusAt(buttons(), 0);
    } else if (restoreFocusTo) {
      // 只在"是我们自己关掉的"这一次还焦点；调用方重复传 open=false 不会误抢焦点
      restoreFocusTo.focus?.();
      restoreFocusTo = null;
    }
  },
);
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
    ref="panelRef"
    class="frost absolute left-0 z-50 w-[min(360px,calc(100vw-2rem))] select-none rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] shadow-xl"
    :class="placement === 'below' ? 'top-full mt-2' : 'bottom-full mb-2'"
    role="dialog"
    :aria-label="t('chat.composer.emoji')"
    tabindex="-1"
    @click.stop
    @keydown="onKey"
  >
    <div
      class="grid grid-cols-8 content-start gap-3 overflow-y-auto p-2"
      style="height: 300px"
    >
      <button
        v-for="e in cells"
        :key="e.file"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)]"
        :class="isFrequent(e) ? 'bg-[var(--gosslan-primary-light)]' : ''"
        :data-emoji-freq="isFrequent(e) ? '1' : undefined"
        :title="labelOf(e)"
        :aria-label="labelOf(e)"
        @click="pick(e)"
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
