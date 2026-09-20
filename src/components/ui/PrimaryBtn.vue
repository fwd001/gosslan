<script setup lang="ts">
/**
 * 主要操作按钮（Primary）—— 主题色实底 + 白字 + hover 亮一档。
 *
 * 统一了之前各处手搓的 Primary 按钮：
 *   ResponsiveLayout / TodoDetailDialog / LinksList / GroupTasksBoard /
 *   GroupMemberPanel / NetworkSection / LogViewer / ForwardModal 等。
 *
 * 三种尺寸：dense(py-2) / default(py-1.5 text-[13px]) / large(py-2 text-sm font-medium)
 */

withDefaults(
  defineProps<{
    /** dense: py-2 / default: py-1.5 / large: py-2 text-sm */
    size?: "dense" | "default" | "large";
    /** 不要 flex-1（默认按钮会占满容器宽度） */
    shrink?: boolean;
    disabled?: boolean;
    /** aria-label（图标按钮时必须给） */
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
    class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] text-white transition hover:bg-[var(--gosslan-primary-hover)] disabled:opacity-50"
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
