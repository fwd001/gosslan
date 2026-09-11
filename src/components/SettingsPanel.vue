<script setup lang="ts">
import { useBackLayer } from "@/composables/useBackLayer";
import { ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import BaseModal from "@/components/BaseModal.vue";
import DevDiagPanel from "@/components/DevDiagPanel.vue";
import ProfileSection from "@/components/settings/ProfileSection.vue";
import AppearanceSection from "@/components/settings/AppearanceSection.vue";
import ChatStyleSection from "@/components/settings/ChatStyleSection.vue";
import GeneralSection from "@/components/settings/GeneralSection.vue";
import NetworkSection from "@/components/settings/NetworkSection.vue";
import StorageSection from "@/components/settings/StorageSection.vue";
import SecuritySection from "@/components/settings/SecuritySection.vue";
import AboutSection from "@/components/settings/AboutSection.vue";
import ResetSection from "@/components/settings/ResetSection.vue";
import { t } from "@/i18n";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

/** 移动端整页设置也是一"层"：系统返回键先回上一页，而不是退出应用。 */
useBackLayer(
  () => props.open,
  () => emit("close"),
);

const app = useAppStore();

/** 各分区按需加载：打开时刷新一次，恢复默认等外部改动后 bump 令牌触发重载。 */
const reloadToken = ref(0);
const devDiagOpen = ref(false);
</script>

<template>
  <!-- 移动端：整页设置（iOS 标准：占满全屏、可上下滑动、不用弹窗遮罩）。
       用户 2026-09-12 反馈：「安卓端可以保持现有的功能，但它不是一个弹窗，
       而是占满整个页面、可以上下滑动的，类似于 iOS 的那种标准。」
       桌面端仍走 BaseModal（卡片式），三种外壳共用同一批分区组件（ProfileSection … ResetSection）。 -->
  <div
    v-if="app.isMobile && open"
    class="fixed inset-0 z-50 flex flex-col bg-[var(--gosslan-bg)] pt-[env(safe-area-inset-top)]"
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
      <span class="text-[15px] font-medium text-[var(--gosslan-text)]">{{ t("settings.title") }}</span>
    </header>
    <div class="min-h-0 flex-1 space-y-5 overflow-y-auto overflow-x-hidden p-4">
      <ProfileSection :active="open" :reload-token="reloadToken" />
      <AppearanceSection />
      <ChatStyleSection />
      <GeneralSection />
      <NetworkSection :active="open" :reload-token="reloadToken" />
      <StorageSection :active="open" :reload-token="reloadToken" />
      <SecuritySection />
      <AboutSection @dev-open="devDiagOpen = true" />
      <ResetSection @restored="reloadToken++" />
    </div>
  </div>

  <!-- 桌面端：卡片式弹窗（独立窗口见 `SettingsWindow.vue`；这里保留给「窗口不可用」的场景） -->
  <BaseModal v-else :open="open" :title="t('settings.title')" width="max-w-xl" @close="emit('close')">
    <div class="-mx-5 -mb-5 max-h-[75vh] space-y-5 overflow-y-auto overflow-x-hidden rounded-b-[var(--gosslan-radius-xl)] bg-[var(--gosslan-bg)] p-5">
      <ProfileSection :active="open" :reload-token="reloadToken" />
      <AppearanceSection />
      <ChatStyleSection />
      <GeneralSection />
      <NetworkSection :active="open" :reload-token="reloadToken" />
      <StorageSection :active="open" :reload-token="reloadToken" />
      <SecuritySection />
      <AboutSection @dev-open="devDiagOpen = true" />
      <ResetSection @restored="reloadToken++" />
    </div>
  </BaseModal>

  <!-- 开发者诊断面板（隐藏入口：连续点击设备指纹 7 次） -->
  <DevDiagPanel :open="devDiagOpen" @close="devDiagOpen = false" />
</template>
