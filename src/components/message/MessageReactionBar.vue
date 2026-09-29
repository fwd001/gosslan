<script setup lang="ts">
/**
 * 气泡下方的表情回应条（飞书/微信同款位置）。
 *
 * 为什么单独成条而不是把回应做成"一条消息"：回应是**状态**不是内容 —— 它挂在被回应的
 * 那条下面，且同一个人反复点只应看到最终结果。协议层它确实是一条条独立事件消息
 * （见 utils/reactions.ts 的说明），但**渲染层必须折叠**，否则群里回三个赞就多三条消息。
 *
 * 添加回应的入口不在本组件：飞书式入口（悬停消息 → 气泡外侧笑脸按钮 → 完整表情选择器）
 * 在 `MessageItem` 的消息行里 —— 入口必须挂在 `.group/msg` **之内**才能被悬停揭示，
 * 而本条是它的兄弟节点（用户 2026-09-21 报「表情回应在哪儿？没看见」）。
 *
 * ## 「谁点的」名单怎么露（用户 2026-09-29 需求汇总三）
 * 同一个表情多人点 ⇒ **聚合成一枚胶囊 + 数字**（不是一人一枚），名单按**追加先后**排，
 * 超过 `ROSTER_VISIBLE` 人折成「+N」。揭示方式按端给，两条都必须存在：
 * - **PC**：悬停 / 键盘聚焦（`hover: hover` 才启用，触屏上 mouseenter 是"点完才来"的假事件，
 *   用它当唯一出口就会在移动端变成"点一下既切了回应又弹名单"）；
 * - **移动端**：能点的时候点就是切回应（这是最高频动作，不能让位），所以名单走**长按**；
 *   不可点的那一侧（现在只有 1:1）点 = 直接看名单。
 * 名单不抢戏：不显示头像、不常驻、只有一行文字，因为这条的第一信息是"有几个、我点没点"。
 */
import { onUnmounted, ref } from "vue";
import { t } from "@/i18n";
import { emojiUrl } from "@/utils/emoji";
import { summarizeActors, ROSTER_VISIBLE, type ReactionChip } from "@/utils/reactions";
import { useChatStore } from "@/stores/useChatStore";

const props = defineProps<{
  chips: ReactionChip[];
  /** 自己的消息：回应条靠右对齐（与气泡的朝向一致） */
  mine: boolean;
  /** 是否允许我添加/取消（单聊暂不开放，且自己的消息也允许自嘲式回应） */
  interactive: boolean;
}>();
const emit = defineEmits<{
  (e: "toggle", emoji: string): void;
}>();

const chat = useChatStore();
/** 设备 id 对用户没有意义；取不到昵称才退回 id（宁可长也不许空）。 */
function nameOf(id: string): string {
  return chat.nicknameOf(id) || id;
}

/** 一行最多列几人，其余折成 +N（同一份判据也用于悬停 title，两处不能各写一个数）。 */
const visible = (c: ReactionChip) => summarizeActors(c.actors, nameOf, ROSTER_VISIBLE);

/** 胶囊上的原生 tooltip：数量 + 前几人 + 「+N」（PC 悬停的第一层，零成本、不占布局）。 */
function chipTitle(c: ReactionChip): string {
  const { shown, hidden } = visible(c);
  const who = shown.join("、");
  const rest = hidden > 0 ? ` ${t("msg.reactionPlus", { n: hidden })}` : "";
  return `${t("msg.reactionWho", { n: c.count })}：${who}${rest}`;
}

/** 同时只开一份名单：一条消息上三个表情全展开会把气泡区糊住。 */
const openEmoji = ref<string | null>(null);
const canHover =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(hover: hover)").matches
    : false;

function reveal(emoji: string) {
  openEmoji.value = emoji;
}
function hide(emoji: string) {
  if (openEmoji.value === emoji) openEmoji.value = null;
}
function togglePanel(emoji: string) {
  openEmoji.value = openEmoji.value === emoji ? null : emoji;
}

/**
 * 长按揭示（移动端）。刻意不借 `utils/longPress.ts`：那两个函数判的是"消息行的长按菜单
 * 要不要吞掉松手"，与这里的"胶囊长按看名单"不是同一个开关 —— 共用会把两条链路绑成
 * 一条（长按胶囊会同时弹消息菜单和名单）。
 */
const LONG_PRESS_MS = 450;
let pressTimer: ReturnType<typeof setTimeout> | null = null;
/** 长按已触发 ⇒ 紧接着的那次 click 必须吃掉，否则"看个名单"会把回应点掉。 */
let heldEmoji: string | null = null;

function onPressStart(emoji: string) {
  if (!props.interactive || canHover) return;
  clearPress();
  pressTimer = setTimeout(() => {
    pressTimer = null;
    heldEmoji = emoji;
    reveal(emoji);
  }, LONG_PRESS_MS);
}
function clearPress() {
  if (pressTimer !== null) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
}
function onClick(c: ReactionChip) {
  clearPress();
  if (heldEmoji === c.emoji) {
    heldEmoji = null;
    return;
  }
  if (!props.interactive) {
    togglePanel(c.emoji);
    return;
  }
  emit("toggle", c.emoji);
}

onUnmounted(clearPress);
</script>

<template>
  <!-- 没有任何回应且不可交互时整条不渲染，避免给每条消息都留一行空白 -->
  <!-- 与气泡**左/右对齐**：本组件是消息行的兄弟节点，默认会从「头像」那一列起排，
       看起来像挂在头像下面而不是气泡下面。左右各让出「头像 40px + 行间距 8px」= 48px。
       这是纯排版补偿，不改变任何行为。 -->
  <div
    v-if="chips.length > 0 || interactive"
    class="mt-1 flex flex-wrap items-center gap-1.5"
    :class="mine ? 'justify-end pr-12' : 'pl-12'"
  >
    <span v-for="c in chips" :key="c.emoji" class="relative inline-flex">
      <button
        class="tap-safe flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-[12px] leading-none shadow-sm transition"
        :class="c.mine
          ? 'border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] text-[var(--gosslan-primary)]'
          : 'border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]'"
        :title="chipTitle(c)"
        :aria-label="t('msg.reactionToggle', { emoji: c.emoji })"
        :aria-expanded="openEmoji === c.emoji"
        :disabled="!interactive"
        @click="onClick(c)"
        @pointerdown="onPressStart(c.emoji)"
        @pointerup="clearPress"
        @pointercancel="clearPress"
        @pointerleave="clearPress"
        @mouseenter="canHover && reveal(c.emoji)"
        @mouseleave="canHover && hide(c.emoji)"
        @focus="canHover && reveal(c.emoji)"
        @blur="canHover && hide(c.emoji)"
        @keydown.esc="hide(c.emoji)"
      >
        <img v-if="emojiUrl(c.emoji)" :src="emojiUrl(c.emoji) ?? undefined" alt="" class="h-4 w-4 shrink-0" />
        <span v-else class="text-[13px] leading-none">{{ c.emoji }}</span>
        <span class="tabular-nums font-medium">{{ c.count }}</span>
      </button>

      <!-- 名单：按**追加先后**列，末位标「最新」。点名单本身不切回应（它是读，不是写）。 -->
      <span
        v-if="openEmoji === c.emoji && c.actors.length > 0"
        class="absolute bottom-full left-0 z-30 mb-1 block min-w-[8rem] max-w-[16rem] rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-2.5 py-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)] shadow-md"
        role="tooltip"
      >
        <span v-for="(id, i) in c.actors" :key="id" class="block truncate" :title="nameOf(id)">
          {{ nameOf(id) }}
          <span v-if="i === c.actors.length - 1" class="text-[var(--gosslan-primary)]">{{
            t("msg.reactionLatest")
          }}</span>
        </span>
        <span v-if="visible(c).hidden > 0" class="block">{{
          t("msg.reactionPlus", { n: visible(c).hidden })
        }}</span>
      </span>
    </span>
  </div>
</template>
