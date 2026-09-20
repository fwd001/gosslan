<script setup lang="ts">
/**
 * 分段控件（Segmented Control）—— 统一应用内所有 tabs / 切换器的视觉。
 *
 * 用法：
 *   <SegmentedControl v-model="value" :options="[{value:'a',label:'A'},{value:'b',label:'B'}]" />
 *
 * 激活态配方（设计规范）：
 *   border + primary-light 底 + accent-ink 字 + font-medium
 * 非激活：border-transparent + text-2 + hover-bg
 *
 * 为什么要抽：此前 AppearanceSection / ChatStyleSection / GroupTasksBoard 过滤 /
 * 状态切换 四处手搓，激活态各写各的（有的 border + 主题色，有的 bg-panel shadow-sm，
 * 有的干脆没样式）。统一后所有 segmented 切换器一份代码维护。
 */

type Option = {
  value: string;
  label: string;
  /** 可选：激活态时 label 旁边展示的徽标（如计数小圆片）。 */
  badge?: string | number;
};

withDefaults(
  defineProps<{
    modelValue: string;
    options: Option[];
    /** 按钮内边距尺寸 —— 不同场景密度不同：Appearance 用 xs，ChatStyle 用 default */
    size?: "xs" | "sm" | "default";
    /** aria-label（无障碍：给读屏人知道这是啥分组） */
    ariaLabel?: string;
    /** 禁用某些选项 */
    disabled?: boolean;
  }>(),
  { size: "default" },
);

const emit = defineEmits<{
  (e: "update:modelValue", v: string): void;
}>();

const sizeClasses: Record<string, string> = {
  xs: "px-2 py-0.5 text-[11px]",
  sm: "px-2.5 py-1 text-xs",
  default: "py-1.5 text-[12px]",
};
</script>

<template>
  <div
    role="radiogroup"
    :aria-label="ariaLabel"
    class="flex gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] p-0.5"
  >
    <button
      v-for="opt in options"
      :key="opt.value"
      type="button"
      role="radio"
      :aria-checked="modelValue === opt.value"
      :disabled="disabled"
      class="tap-safe flex flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded-[var(--gosslan-radius-sm)] border transition disabled:opacity-50"
      :class="[
        sizeClasses[size],
        modelValue === opt.value
          ? 'border-transparent bg-[var(--gosslan-primary)] font-medium text-white shadow-sm'
          : 'border-transparent text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]',
      ]"
      @click="emit('update:modelValue', opt.value)"
    >
      {{ opt.label }}
      <span
        v-if="opt.badge !== undefined"
        class="rounded-full px-1.5 text-[11px] leading-4 tabular-nums"
        :class="
          modelValue === opt.value
            ? 'bg-white/25 text-white'
            : 'text-[var(--gosslan-text-2)] opacity-70'
        "
      >
        {{ opt.badge }}
      </span>
    </button>
  </div>
</template>
