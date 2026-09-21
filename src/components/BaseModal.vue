<script setup lang="ts">
import { Dialog, DialogPanel, DialogTitle, TransitionChild, TransitionRoot } from "@headlessui/vue";
import { X } from "lucide-vue-next";
import { useBackLayer } from "@/composables/useBackLayer";
import BackArrow from "@/components/ui/BackArrow.vue";
import { useAppStore } from "@/stores/useAppStore";
import { t } from "@/i18n";

/**
 * 移动端软键盘适配：弹窗内容要给键盘**让位**。
 *
 * 用户实测：「弹出创建群聊界面时，默认点击输入框创建群名，输入框会遮挡部分创建群聊的面板」。
 * 原因是键盘补偿（`app.keyboardInset`）此前只加在聊天页的滚动容器上，**弹窗没有**。
 * 这里统一给弹窗的滚动容器加同样的下内边距 —— 弹窗里聚焦输入框时，
 * 浏览器会把输入框滚进可视区，而可视区高度已经扣掉了键盘，于是输入框不会再被挡住。
 * 放在 `BaseModal` 一处，所有弹窗（创建群聊/改名/转发/加好友/搜索…）一起生效。
 */
const app = useAppStore();

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
  <!-- ⚠️ z 序是本组件最容易改坏的一处：HeadlessUI 的 `Dialog` 会把自己挂到
       **`<body>` 下的 `#headlessui-portal-root`**（不是留在调用者的子树里，实测确认）。
       也就是说弹窗的 z-index 是在**文档根层级**上与别的浮层比大小 —— 而移动端的整页下钻页
       （设置 / 日志 / 收藏 / 链接 / 资料）是 `MobilePageFrame` 的 `fixed inset-0 z-[60]`。
       原先这里写 `z-50`：弹窗虽然挂进了 DOM，却被 z-[60] 的整页框架**盖在后面**，
       用户看到的就是"点了没反应、连弹窗都没出来"（用户 2026-09-21，移动端设置 → 重置与数据）。
       桌面端没有 z-[60] 的整页框架，所以只有移动端复现。

       `z-[65]` 是重新选的位置。应用现有阶梯：
         整页框架 `z-[60]` < **弹窗 `z-[65]`** < 右键菜单 `z-[70]`
         < 操作面板 / 图片预览 `z-[80]` < Toast `z-[90]`
       即：盖住一切页面（含移动端那些整页下钻页），但不挡右键菜单、弹层里的图片预览，
       也不挡 toast（失败提示必须能盖在弹窗上）。改这个值前先看这条阶梯。

       `font-gosslan` 同理：弹窗挂在 portal root（应用根节点**之外**），拿不到根节点上的字体，
       用户选的字体对弹窗不生效 —— 带上这个类才与界面其余部分一致。
       主题色/暗色不受影响（那是 `:root`/`.dark` 上的 CSS 变量，本来就能继承）。 -->
  <TransitionRoot :show="open" as="template">
    <Dialog as="div" class="relative z-[65] font-gosslan" @close="emit('close')">
      <!-- 整页形态不画遮罩：整页面板本身不透明，遮罩只会在滑入的过程里
           给「上一页」糊一层黑（观感成了 modal，而不是页面推进）。卡片形态才需要遮罩。 -->
      <TransitionChild
        v-if="!fullscreen"
        as="template"
        enter="duration-200 ease-out"
        enter-from="opacity-0"
        enter-to="opacity-100"
        leave="duration-150 ease-in"
        leave-from="opacity-100"
        leave-to="opacity-0"
      >
        <div class="glass fixed inset-0 bg-[var(--gosslan-overlay)]" aria-hidden="true" />
      </TransitionChild>
      <div
        class="fixed inset-0 overflow-y-auto overflow-x-hidden"
        :style="app.isMobile && app.keyboardInset > 0
          ? { paddingBottom: `${app.keyboardInset + 12}px` }
          : undefined"
      >
        <div
          :class="fullscreen
            ? 'flex min-h-full items-stretch justify-center'
            : 'flex min-h-full items-center justify-center p-4'"
        >
          <!-- 整页形态：铺满 + 自带标题栏（含左上返回/关闭）与安全区；
               卡片形态：保持原有的居中卡片。
               ⚠️ 这条注释**必须在 `<TransitionChild>` 之外**：dev 构建会保留 HTML 注释，
               而 `as="template"` 的插槽里多出一个注释节点就会让 Headless UI 抛
               "Passing props on template!"（Vue 渲染直接炸 ⇒ 整个窗口卡死）。
               —— 整页形态按「一页」处理：与设置/日志/收藏等详情页**同一套 page-slide**
               （从右滑入 / 滑出），而不是卡片的淡入缩放。否则同样是整页下钻，
               群任务 / 搜索页的转场却和别的详情页不一样（用户 2026-09-20「群任务的转场好像不对」）。 -->
          <TransitionChild
            as="template"
            :enter="fullscreen
              ? 'transition-[transform,opacity] duration-[var(--gosslan-duration)] ease-[var(--gosslan-ease)]'
              : 'duration-200 ease-out'"
            :enter-from="fullscreen ? 'translate-x-full opacity-40' : 'opacity-0 scale-95'"
            :enter-to="fullscreen ? 'translate-x-0 opacity-100' : 'opacity-100 scale-100'"
            :leave="fullscreen
              ? 'transition-[transform,opacity] duration-[var(--gosslan-duration)] ease-[var(--gosslan-ease)]'
              : 'duration-150 ease-in'"
            :leave-from="fullscreen ? 'translate-x-0 opacity-100' : 'opacity-100 scale-100'"
            :leave-to="fullscreen ? 'translate-x-full opacity-40' : 'opacity-0 scale-95'"
          >
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
                  <BackArrow />
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
              <div v-if="title" class="mb-4 flex items-start justify-between gap-3">
                <DialogTitle as="h3" class="text-base font-semibold leading-6">
                  {{ title }}
                </DialogTitle>
                <button
                  class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
                  :title="t('common.close')" :aria-label="t('common.close')"
                  @click="emit('close')"
                >
                  <X class="h-4 w-4" />
                </button>
              </div>
              <slot />
            </DialogPanel>
          </TransitionChild>
        </div>
      </div>
    </Dialog>
  </TransitionRoot>
</template>
