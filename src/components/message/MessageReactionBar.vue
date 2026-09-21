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
 */
import { t } from "@/i18n";
import { emojiUrl } from "@/utils/emoji";
import type { ReactionChip } from "@/utils/reactions";

defineProps<{
  chips: ReactionChip[];
  /** 自己的消息：回应条靠右对齐（与气泡的朝向一致） */
  mine: boolean;
  /** 是否允许我添加/取消（单聊暂不开放，且自己的消息也允许自嘲式回应） */
  interactive: boolean;
}>();
const emit = defineEmits<{
  (e: "toggle", emoji: string): void;
}>();
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
    <button
      v-for="c in chips"
      :key="c.emoji"
      class="tap-safe flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-[12px] leading-none shadow-sm transition"
      :class="c.mine
        ? 'border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] text-[var(--gosslan-primary)]'
        : 'border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]'"
      :title="t('msg.reactionWho', { n: c.count })"
      :aria-label="t('msg.reactionToggle', { emoji: c.emoji })"
      :disabled="!interactive"
      @click="emit('toggle', c.emoji)"
    >
      <img v-if="emojiUrl(c.emoji)" :src="emojiUrl(c.emoji) ?? undefined" alt="" class="h-4 w-4 shrink-0" />
      <span v-else class="text-[13px] leading-none">{{ c.emoji }}</span>
      <span class="tabular-nums font-medium">{{ c.count }}</span>
    </button>
  </div>
</template>
