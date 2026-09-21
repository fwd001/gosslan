<script setup lang="ts">
import { Loader2 } from "lucide-vue-next";

withDefaults(
  defineProps<{
    modelValue: boolean;
    /**
     * 开关的可访问名。**必填**：开关本体只有滑块、没有任何文字，
     * 名字只能来自外部（所在行的标题）。做成必填是为了让"无名开关"编译不过 ——
     * 读屏下 `role="switch"` 没有名字等于一个读不出来的控件（审计 P0-4）。
     */
    label: string;
    /** md 用于深色模式这类主开关，sm 用于分组内的次要开关。 */
    size?: "sm" | "md";
    disabled?: boolean;
    /**
     * 正在等后端确认（乐观更新期间）。
     *
     * ⚠️ 它**不参与开关取值** —— 取值永远是"用户意图"（`modelValue`）。
     * 乐观更新的要义就是"先按用户意图显示，再让后端执行"（用户 2026-09-13 的规则），
     * 所以这里只补一个"还在跑"的反馈（滑杆上一枚小转圈 + `aria-busy`）；
     * 失败时由 store 回退取值、调用方 toast。
     */
    pending?: boolean;
  }>(),
  { size: "sm", disabled: false, pending: false },
);
const emit = defineEmits<{ (e: "update:modelValue", v: boolean): void }>();
</script>

<template>
  <button
    class="tap-safe relative shrink-0 rounded-full transition"
    :class="[
      size === 'md' ? 'h-6 w-11' : 'h-5 w-9',
      modelValue ? 'bg-[var(--gosslan-primary)]' : 'bg-[var(--gosslan-border)]',
      disabled ? 'opacity-50' : '',
    ]"
    role="switch"
    :aria-checked="modelValue"
    :aria-busy="pending"
    :aria-label="label"
    :disabled="disabled"
    @click="emit('update:modelValue', !modelValue)"
  >
    <span
      class="absolute top-0.5 flex items-center justify-center rounded-full bg-white shadow transition-all"
      :class="[
        size === 'md' ? 'h-5 w-5' : 'h-4 w-4',
        modelValue ? (size === 'md' ? 'left-[22px]' : 'left-[18px]') : 'left-0.5',
      ]"
    >
      <!-- 乐观更新期间的小转圈：开关已经按用户意图动了，这里只是"还在跑"的提示 -->
      <Loader2
        v-if="pending"
        class="animate-spin text-[var(--gosslan-text-2)]"
        :class="size === 'md' ? 'h-3.5 w-3.5' : 'h-3 w-3'"
        aria-hidden="true"
      />
      <slot />
    </span>
  </button>
</template>
