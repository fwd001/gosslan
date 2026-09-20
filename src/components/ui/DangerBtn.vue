<script setup lang="ts">
/**
 * 危险操作按钮（Danger）—— danger 实底 + 白字 + hover 亮一档（danger-hover）。
 *
 * 统一了之前各处手搓的 Danger 按钮：
 *   ConversationList / ChatWindow / GroupTasksBoard / GroupMemberPanel /
 *   FriendProfile / MessageItem / StorageSection / ResetSection / FavoritePanel。
 *
 * 三种尺寸同 PrimaryBtn。hover 统一为 `hover:bg-[var(--gosslan-danger-hover)]`（color-mix 派生），
 * 消除了之前的自引用 `hover:bg-[var(--gosslan-danger)]`（无效）+ `opacity-90`（暗化）两种错误。
 */

withDefaults(
  defineProps<{
    size?: "dense" | "default" | "large";
    shrink?: boolean;
    disabled?: boolean;
    ariaLabel?: string;
    type?: "button" | "submit";
  }>(),
  { size: "default", shrink: false, disabled: false, type: "button" },
);
</script>

<template>
  <button
    :type="type"
    :disabled="disabled"
    :aria-label="ariaLabel"
    class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] text-white transition hover:bg-[var(--gosslan-danger-hover)] disabled:opacity-50"
    :class="[
      shrink ? 'shrink-0' : '',
      size === 'dense' ? 'px-3 py-2 text-[13px]' : '',
      size === 'default' ? 'px-3 py-1.5 text-[13px]' : '',
      size === 'large' ? 'px-4 py-2 text-sm font-medium' : '',
    ]"
  >
    <slot />
  </button>
</template>
