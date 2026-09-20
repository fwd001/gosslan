<script setup lang="ts">
import { useBackLayer } from "@/composables/useBackLayer";
import { ChevronRight } from "lucide-vue-next";
import { computed, ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import BaseModal from "@/components/BaseModal.vue";
import MobilePageFrame from "@/components/MobilePageFrame.vue";
import DevDiagPanel from "@/components/DevDiagPanel.vue";
import ProfileSection from "@/components/settings/ProfileSection.vue";
import AppearanceSection from "@/components/settings/AppearanceSection.vue";
import ChatStyleSection from "@/components/settings/ChatStyleSection.vue";
import GeneralSection from "@/components/settings/GeneralSection.vue";
import NotificationSection from "@/components/settings/NotificationSection.vue";
import FilesSection from "@/components/settings/FilesSection.vue";
import NetworkSection from "@/components/settings/NetworkSection.vue";
import StorageSection from "@/components/settings/StorageSection.vue";
import SecuritySection from "@/components/settings/SecuritySection.vue";
import AboutSection from "@/components/settings/AboutSection.vue";
import ResetSection from "@/components/settings/ResetSection.vue";
import { t } from "@/i18n";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();

/** 各分区按需加载：打开时刷新一次，恢复默认等外部改动后 bump 令牌触发重载。 */
const reloadToken = ref(0);
const devDiagOpen = ref(false);

/** 移动端二级导航：null=一级列表；string=二级详情页 key。 */
const mobileNav = ref<string | null>(null);

/** 二级页打开时，系统返回键先回一级，再返回关闭整个设置层。 */
useBackLayer(
  () => props.open && mobileNav.value !== null,
  () => (mobileNav.value = null),
);
useBackLayer(
  () => props.open && mobileNav.value === null,
  () => emit("close"),
);

/** 二级页配置：key → 标题 + 渲染函数（用 h 避免模板里大 switch）。 */
const SECTIONS: Record<string, { titleKey: string; render: () => any }> = {
  profile: {
    titleKey: "settings.group.general",
    render: () => ProfileSection,
  },
  general: {
    titleKey: "settings.group.general",
    render: () => GeneralSection,
  },
  appearance: {
    titleKey: "settings.group.appearance",
    render: () => AppearanceSection,
  },
  chatStyle: {
    titleKey: "settings.group.appearance",
    render: () => ChatStyleSection,
  },
  notification: {
    titleKey: "settings.group.appearance",
    render: () => NotificationSection,
  },
  network: {
    titleKey: "settings.group.data",
    render: () => NetworkSection,
  },
  files: {
    titleKey: "settings.group.data",
    render: () => FilesSection,
  },
  storage: {
    titleKey: "settings.group.data",
    render: () => StorageSection,
  },
  security: {
    titleKey: "settings.group.security",
    render: () => SecuritySection,
  },
  about: {
    titleKey: "settings.group.about",
    render: () => AboutSection,
  },
  reset: {
    titleKey: "settings.group.about",
    render: () => ResetSection,
  },
};

/** 一级列表的分组结构：每组包含多个条目，每条对应一个二级页。 */
const GROUPS: { key: string; titleKey: string; items: { key: string; titleKey: string }[] }[] = [
  {
    key: "general",
    titleKey: "settings.group.general",
    items: [
      { key: "profile", titleKey: "settings.item.profile" },
      { key: "general", titleKey: "settings.item.general" },
    ],
  },
  {
    key: "appearance",
    titleKey: "settings.group.appearance",
    items: [
      { key: "appearance", titleKey: "settings.item.appearance" },
      { key: "chatStyle", titleKey: "settings.item.chatStyle" },
      { key: "notification", titleKey: "settings.item.notification" },
    ],
  },
  {
    key: "data",
    titleKey: "settings.group.data",
    items: [
      { key: "network", titleKey: "settings.item.network" },
      { key: "files", titleKey: "settings.item.files" },
      { key: "storage", titleKey: "settings.item.storage" },
    ],
  },
  {
    key: "security",
    titleKey: "settings.group.security",
    items: [
      { key: "security", titleKey: "settings.item.security" },
    ],
  },
  {
    key: "about",
    titleKey: "settings.group.about",
    items: [
      { key: "about", titleKey: "settings.item.about" },
      { key: "reset", titleKey: "settings.item.reset" },
    ],
  },
];

/** 移动端头部标题：一级=设置总标题；二级=该分区标题。 */
const settingsTitle = computed(() =>
  mobileNav.value === null ? t("settings.title") : t(SECTIONS[mobileNav.value].titleKey),
);
/** 移动端返回：二级先回一级，一级再关闭整个设置层。 */
function onSettingsBack() {
  if (mobileNav.value === null) emit("close");
  else mobileNav.value = null;
}
</script>

<template>
  <!-- 移动端：二级 push 导航，iOS 设置风格。头部（返回 + 标题）统一交给 MobilePageFrame，
       详情页不再各自画返回键（用户 2026-09-20）。二级→一级→关闭 由 onSettingsBack 路由。 -->
  <Transition v-if="app.isMobile" name="page-slide">
    <MobilePageFrame
      v-if="open"
      mode="overlay"
      :title="settingsTitle"
      @back="onSettingsBack"
    >
      <!-- 一级页：分组列表 -->
      <div v-if="mobileNav === null" class="min-h-0 flex-1 overflow-y-auto overflow-x-hidden p-4">
        <div v-for="g in GROUPS" :key="g.key" class="mb-5">
          <div class="pb-1 text-[11px] font-medium uppercase tracking-wider text-[var(--gosslan-text-3)]">
            {{ t(g.titleKey) }}
          </div>
          <div class="overflow-hidden rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]">
            <button
              v-for="(item, idx) in g.items"
              :key="item.key"
              class="tap-safe flex w-full items-center gap-3 px-3 py-3 text-left text-[14px] transition hover:bg-[var(--gosslan-hover)]"
              :class="idx > 0 ? 'border-t border-[var(--gosslan-divider)]' : ''"
              @click="mobileNav = item.key"
            >
              <span class="flex-1 text-[var(--gosslan-text)]">{{ t(item.titleKey) }}</span>
              <ChevronRight class="h-4 w-4 text-[var(--gosslan-text-3)]" />
            </button>
          </div>
        </div>
      </div>

      <!-- 二级页：完整 Section 组件 -->
      <div v-else class="min-h-0 flex-1 space-y-5 overflow-y-auto overflow-x-hidden p-4">
        <ProfileSection v-if="mobileNav === 'profile'" :active="open" :reload-token="reloadToken" />
        <GeneralSection v-else-if="mobileNav === 'general'" />
        <AppearanceSection v-else-if="mobileNav === 'appearance'" />
        <ChatStyleSection v-else-if="mobileNav === 'chatStyle'" />
        <NotificationSection v-else-if="mobileNav === 'notification'" />
        <NetworkSection v-else-if="mobileNav === 'network'" :active="open" :reload-token="reloadToken" />
        <FilesSection v-else-if="mobileNav === 'files'" :active="open" :reload-token="reloadToken" />
        <StorageSection v-else-if="mobileNav === 'storage'" :active="open" :reload-token="reloadToken" />
        <SecuritySection v-else-if="mobileNav === 'security'" />
        <AboutSection v-else-if="mobileNav === 'about'" @dev-open="devDiagOpen = true" />
        <ResetSection v-else-if="mobileNav === 'reset'" @restored="reloadToken++" />
      </div>
    </MobilePageFrame>
  </Transition>

  <!-- 桌面端：卡片式弹窗（独立窗口见 `SettingsWindow.vue`；这里保留给「窗口不可用」的场景） -->
  <BaseModal v-else :open="open" :title="t('settings.title')" width="max-w-xl" @close="emit('close')">
    <div class="-mx-5 -mb-5 max-h-[75vh] space-y-5 overflow-y-auto overflow-x-hidden rounded-b-[var(--gosslan-radius-xl)] bg-[var(--gosslan-bg)] p-5">
      <ProfileSection :active="open" :reload-token="reloadToken" />
      <GeneralSection />
      <NotificationSection />
      <AppearanceSection />
      <ChatStyleSection />
      <NetworkSection :active="open" :reload-token="reloadToken" />
      <FilesSection :active="open" :reload-token="reloadToken" />
      <StorageSection :active="open" :reload-token="reloadToken" />
      <SecuritySection />
      <AboutSection @dev-open="devDiagOpen = true" />
      <ResetSection @restored="reloadToken++" />
    </div>
  </BaseModal>

  <!-- 开发者诊断面板（隐藏入口：连续点击设备指纹 7 次） -->
  <DevDiagPanel :open="devDiagOpen" @close="devDiagOpen = false" />
</template>
