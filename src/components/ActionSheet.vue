<script setup lang="ts">
import { t } from "@/i18n";
import { Dialog, DialogPanel, TransitionChild, TransitionRoot } from "@headlessui/vue";

/**
 * 移动端底部操作面板（Action Sheet，iOS HIG）。
 *
 * 用于「多操作选择」场景（如长按消息 → 复制/引用/转发），与居中 Modal（二元确认）区分：
 * HIG 里破坏性**确认**（是/否）用 Alert（居中），**多操作选择**用 Action Sheet（底部）。
 * 桌面端右键菜单保持原样，本组件只在触屏语境使用。
 */
defineProps<{ open: boolean; title?: string }>();
const emit = defineEmits<{ (e: "close"): void }>();
</script>

<template>
  <TransitionRoot :show="open" as="template">
    <Dialog as="div" class="relative z-[80]" @close="emit('close')">
      <TransitionChild
        as="template"
        enter="duration-200 ease-out"
        enter-from="opacity-0"
        enter-to="opacity-100"
        leave="duration-150 ease-in"
        leave-from="opacity-100"
        leave-to="opacity-0"
      >
        <div class="fixed inset-0 bg-[var(--gosslan-overlay)]" aria-hidden="true" @click="emit('close')" />
      </TransitionChild>

      <div class="fixed inset-x-0 bottom-0">
        <!-- 面板里**任何一项**点完都要收起（成熟产品的通行行为：iOS ActionSheet / 微信 /
             Telegram 的底部菜单都是"点一项即消失"）。用户 2026-09-13 安卓实测：
             「点『引用』这个 sheet 应该自动隐藏；点『转发』也应该自动隐藏，因为它会跳转到
             界面内去操作聊天」—— 不在这一层统一收，就得每个入口各写一遍（漏一个就挂在那儿
             挡着跳转后的界面）。按钮自己的 `@click` 先跑，这里再收面板（Vue 事件冒泡），
             所以动作本身不受影响；取消按钮那句 `emit('close')` 于是成了幂等的第二次调用。
             HeadlessUI 会把它的 `stopPropagation` 与我们的 `onClick` 合并执行，且**用户传入的
             handler 先跑**（见 `@headlessui/vue/dist/utils/render.js` 的 mergeProps）。
             ⚠️ 这段注释**必须留在 `TransitionChild as="template"` 外面**：`as="template"` 的插槽
             里多一个注释节点就会触发 HeadlessUI 的 "Passing props on template!" 抛错
             （`designGuards` 里有护栏专门盯这条，2026-09-12 踩过）。 -->
        <TransitionChild
          as="template"
          enter="duration-200 ease-out"
          enter-from="translate-y-full"
          enter-to="translate-y-0"
          leave="duration-150 ease-in"
          leave-from="translate-y-0"
          leave-to="translate-y-full"
        >
          <DialogPanel
            class="rounded-t-[var(--gosslan-radius-xl)] bg-[var(--gosslan-panel)] px-2 pt-2 pb-[max(env(safe-area-inset-bottom),0.5rem)] shadow-2xl"
            @click="emit('close')"
          >
            <div
              class="mx-auto mb-2 h-1 w-10 rounded-full bg-[var(--gosslan-border)]"
              aria-hidden="true"
            />
            <p v-if="title" class="px-3 pb-2 text-center text-xs text-[var(--gosslan-text-2)]">
              {{ title }}
            </p>
            <slot />
            <button
              class="mt-2 w-full rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] py-3 text-center text-[15px] font-medium text-[var(--gosslan-text)] transition active:opacity-70"
              @click="emit('close')"
            >
              <slot name="cancel">{{ t("common.cancel") }}</slot>
            </button>
          </DialogPanel>
        </TransitionChild>
      </div>
    </Dialog>
  </TransitionRoot>
</template>
