<script setup lang="ts">
/**
 * 移动端详情页统一框架头部（"框架的返回图标"）。
 *
 * 所有移动端详情页（设置 / 日志 / 收藏 / 链接 / 资料）共用这同一个头部，
 * 而不是各自画一个返回键——之前设置用自绘 chevron、收藏用 `←` 文字、
 * 日志用 `ArrowLeft` 图标，三套不一致（用户 2026-09-20：「详情页面不要做单独的返回，
 * 统一用框架的返回图标」）。
 *
 * 返回箭头本身由 `ui/BackArrow.vue` 提供（按平台适配：Android 用 Material ←，
 * 其他平台用细线 chevron），全应用只此一份造型。
 *
 * 用法：
 *   <MobilePageFrame mode="overlay" :title="t('x')" @back="onBack">
 *     <slot />           页面内容（默认滚动）
 *     <template #actions> 头部右侧操作（如日志的刷新/清空/复制） </template>
 *   </MobilePageFrame>
 *
 * - `overlay`：全屏覆盖层（`fixed inset-0 z-[60]`），盖住底部导航（z-40）。
 *   用于从「我的」下钻出来的设置/日志/收藏/链接/资料。
 * - `inline`：填满父容器（`flex h-full`），底部导航仍可见。用于桌面侧栏里的链接列表等。
 *
 * 转场由父级用 `<Transition name="page-slide">` 包裹本组件即可（见各调用处）。
 */
import { t } from "@/i18n";
import BackArrow from "@/components/ui/BackArrow.vue";

const props = withDefaults(
  defineProps<{
    /** 头部标题（中部）。不传则不显示标题文字（仅返回键）。 */
    title?: string;
    /** 是否显示左侧返回键（默认 true）。inline 形态在桌面不需要返回键时可关。 */
    showBack?: boolean;
    /** 是否显示头部栏（默认 true）。桌面端某些内嵌场景（链接列表、收藏）不需要头部时传 false。 */
    header?: boolean;
    /** overlay=全屏覆盖（盖住底部导航）；inline=填满父容器。 */
    mode?: "overlay" | "inline";
    /** 内容区是否自带滚动（默认 true）。日志页自己管滚动时传 false。 */
    scroll?: boolean;
    /** 返回键的无障碍标签，默认取 common.back。 */
    backLabel?: string;
  }>(),
  { title: "", showBack: true, header: true, mode: "inline", scroll: true, backLabel: undefined },
);

const emit = defineEmits<{ (e: "back"): void }>();

const backAria = () => props.backLabel ?? t("common.back");
</script>

<template>
  <div
    :class="mode === 'overlay'
      ? 'fixed inset-0 z-[60] flex flex-col bg-[var(--gosslan-bg)] pt-[env(safe-area-inset-top)]'
      : 'flex h-full min-h-0 flex-col'"
  >
    <!-- 统一头部：返回键（BackArrow，按平台适配）+ 标题 + 右侧操作区。
         返回键是原生 button + tap-safe，保证触控可达（designGuards）。 -->
    <!-- 头部底色按形态走：
         · `overlay`（移动端整页）：`--gosslan-panel` —— 它是一级页面的标题栏；
         · `inline`（嵌在桌面左列里，如链接列表）：`--gosslan-list` —— 与所在列表同色。
           否则这一条会比下面的列表亮一档，和聊天/通讯录列表的搜索行对不上（用户 2026-09-20）。 -->
    <header
      v-if="header"
      class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] px-3"
      :class="mode === 'overlay' ? 'bg-[var(--gosslan-panel)]' : 'bg-[var(--gosslan-list)]'"
      :style="{ height: 'var(--gosslan-header-h)' }"
    >
      <button
        v-if="showBack"
        class="tap-safe flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="backAria()"
        :aria-label="backAria()"
        @click="emit('back')"
      >
        <!-- 返回箭头：统一走 BackArrow（Android 用 Material ←，其他平台用细线 chevron）。 -->
        <BackArrow />
      </button>
      <span
        v-if="title"
        class="min-w-0 truncate text-[15px] font-medium text-[var(--gosslan-text)]"
        :title="title"
        >{{ title }}</span
      >
      <div class="ml-auto flex items-center gap-1">
        <slot name="actions" />
      </div>
    </header>

    <!-- 内容区：flex 列，保证子页面自己的 flex-1 + 内部滚动布局不被破坏；
         scroll=true 时本层也兜底滚动（列表超长）；scroll=false（日志）把滚动交给页面自己。 -->
    <div :class="scroll ? 'min-h-0 flex-1 flex flex-col overflow-y-auto' : 'min-h-0 flex-1 flex flex-col'">
      <slot />
    </div>
  </div>
</template>
