<script setup lang="ts">
import { Dialog, DialogPanel, DialogTitle, TransitionChild, TransitionRoot } from "@headlessui/vue";
import { useBackLayer } from "@/composables/useBackLayer";
import { t } from "@/i18n";

const props = withDefaults(
  defineProps<{
    open: boolean;
    title?: string;
    width?: string;
    /**
     * 移动端「整页」形态（iOS 标准：占满全屏、可上下滑动、不是卡片弹窗）。
     *
     * 为什么放在这里而不是每个调用方各写一套 `fixed inset-0`：整页浮层与卡片弹窗的
     * **语义完全一样**（模态、Esc/返回键关闭、焦点约束），差别只是外壳。外壳写在唯一
     * 一处，才能保证「移动端整页」都带上安全区、关闭按钮与滚动容器，不会各写各的。
     */
    fullscreen?: boolean;
  }>(),
  { width: "max-w-md", fullscreen: false },
);
const emit = defineEmits<{ (e: "close"): void }>();

/**
 * 系统返回键 / 后退导航：先关最上面的弹窗，而不是**直接退出应用**
 * （Android 上没有这层绑定的话，用户在"添加好友"弹窗里按返回会退出整个应用）。
 * 见 `composables/useBackLayer.ts`。
 */
useBackLayer(
  () => props.open,
  () => emit("close"),
);
</script>

<template>
  <TransitionRoot :show="open" as="template">
    <Dialog as="div" class="relative z-50" @close="emit('close')">
      <TransitionChild
        as="template"
        enter="duration-200 ease-out"
        enter-from="opacity-0"
        enter-to="opacity-100"
        leave="duration-150 ease-in"
        leave-from="opacity-100"
        leave-to="opacity-0"
      >
        <div class="glass fixed inset-0 bg-black/40" aria-hidden="true" />
      </TransitionChild>
      <div class="fixed inset-0 overflow-y-auto">
        <div
          :class="fullscreen
            ? 'flex min-h-full items-stretch justify-center'
            : 'flex min-h-full items-center justify-center p-4'"
        >
          <TransitionChild
            as="template"
            enter="duration-200 ease-out"
            enter-from="opacity-0 scale-95"
            enter-to="opacity-100 scale-100"
            leave="duration-150 ease-in"
            leave-from="opacity-100 scale-100"
            leave-to="opacity-0 scale-95"
          >
            <!-- 整页形态：铺满 + 自带标题栏（含左上返回/关闭）与安全区；
                 卡片形态：保持原有的居中卡片。 -->
            <DialogPanel
              v-if="fullscreen"
              class="flex h-full w-full flex-col bg-[var(--gosslan-app-bg)] pt-[env(safe-area-inset-top)] text-left text-[var(--gosslan-text)]"
            >
              <header
                class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-panel)] px-3"
                :style="{ height: 'var(--gosslan-header-h)' }"
              >
                <button
                  class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
                  :title="t('common.back')" :aria-label="t('common.back')"
                  @click="emit('close')"
                >
                  <svg viewBox="0 0 24 24" class="h-5 w-5" fill="none" stroke="currentColor" stroke-width="1.9">
                    <path d="m15 18-6-6 6-6" />
                  </svg>
                </button>
                <DialogTitle v-if="title" as="h2" class="text-[15px] font-medium">{{ title }}</DialogTitle>
              </header>
              <div class="flex min-h-0 flex-1 flex-col p-4">
                <slot />
              </div>
            </DialogPanel>
            <DialogPanel
              v-else
              class="elevated w-full rounded-[var(--gosslan-radius-xl)] bg-[var(--gosslan-panel)] p-5 text-left align-middle text-[var(--gosslan-text)]"
              :class="width"
            >
              <DialogTitle v-if="title" as="h3" class="text-base font-semibold mb-4">
                {{ title }}
              </DialogTitle>
              <slot />
            </DialogPanel>
          </TransitionChild>
        </div>
      </div>
    </Dialog>
  </TransitionRoot>
</template>
