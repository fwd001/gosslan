<script setup lang="ts">
/**
 * 独立「设置」窗口的内容（桌面端 label="settings"，由 Rust `open_settings_window` 创建）。
 *
 * 布局照用户给的参考图：**左侧窄导航 + 右侧内容**（微信 4.0 设置窗口）——
 * 用户 2026-09-12 反馈：「PC 端的设置页面可以按照这种布局，弹一个单独的窗口」。
 *
 * 与 `SettingsPanel` 的关系：两者复用**同一批分区组件**（ProfileSection … ResetSection），
 * 只有外壳不同（这里是窗口内的左右两栏，那里是移动端整页/卡片弹窗）。
 * 这样「设置长什么样」只有一份实现，不会三处漂移。
 */
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
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
import { UserRound, Palette, SlidersHorizontal, Wifi, HardDrive, Lock, Info, Bell, FolderOpen } from "lucide-vue-next";
import { t } from "@/i18n";

type SectionKey =
  | "profile"
  | "general"
  | "notifications"
  | "appearance"
  | "network"
  | "files"
  | "storage"
  | "security"
  | "about";

const section = ref<SectionKey>("profile");
/** 各分区按需加载：恢复默认等改动后 bump 令牌触发重载（与 SettingsPanel 同一套语义）。 */
const reloadToken = ref(0);
const devDiagOpen = ref(false);

/** 导航项：图标 + 文案。`icon` 用组件引用，避免在模板里写一长串 v-if。 */
/**
 * 导航项按 **iOS 设置的概念**排序（2026-09-12 用户反馈「分类不合理」后重排）：
 * 个人资料 → 通用 → 通知 → 外观 → 网络与连接 → 文件与共享 → 存储 → 隐私与安全 → 关于。
 *
 * ⚠️ 教训：导航项的**标签必须与它打开的分区内容一致**。此前 `general` 这一项
 * 写着「通知」，点开却是「语言 + 通知 + 共享目录」—— 用户看到"通知里第一项是语言、
 * 第三项是共享目录"，这就是分类错误。现在每一项只放它字面意思里的东西：
 *   · 通用（iOS「通用」）＝ 语言与地区 + 还原；
 *   · 通知（iOS「通知」）＝ 允许通知 + 显示预览（消息正文）；
 *   · 文件与共享 ＝ 接收文件目录 + 共享目录（都是"文件放哪儿/给谁看"）。
 */
const navItems = computed<{ key: SectionKey; label: string; icon: unknown }[]>(() => [
  { key: "profile", label: t("settings.group.profile"), icon: UserRound },
  { key: "general", label: t("settings.group.general"), icon: SlidersHorizontal },
  { key: "notifications", label: t("settings.group.notifications"), icon: Bell },
  { key: "appearance", label: t("settings.group.appearance"), icon: Palette },
  { key: "network", label: t("settings.group.network"), icon: Wifi },
  { key: "files", label: t("settings.group.files"), icon: FolderOpen },
  { key: "storage", label: t("settings.group.storage"), icon: HardDrive },
  { key: "security", label: t("settings.group.security"), icon: Lock },
  { key: "about", label: t("settings.group.about"), icon: Info },
]);

/**
 * Esc 关闭设置窗口（桌面惯例）。
 * 窗口本身有系统标题栏的关闭按钮，但设置页是一个"看两眼就走"的页面，
 * Esc 是用户预期存在的最短路径。失败（非 Tauri 环境）时静默忽略。
 */
function onKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  // 有弹层（如清除数据二次确认）时先让弹层处理，避免一层 Esc 连窗口一起关掉。
  if (document.querySelector(".vel-modal, [role='dialog']")) return;
  void invoke("close_settings_window").catch(() => {});
}
onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <div class="flex h-screen overflow-hidden bg-[var(--gosslan-bg)] text-[var(--gosslan-text)]">
    <!-- 左：窄导航。
         间距刻意**左右对称**（`px-2` + 项内 `px-2.5`）：用户 2026-09-12 反馈
         「右边的箭头两边都不够对称」——参考图里那一列的对齐问题就出在
         左内边距与右侧留白/箭头位置不匹配。这里导航项为纯文字+图标（无展开箭头），
         一旦将来加展开子项，用同一个 `pr-2.5` 承接箭头即可保持对称。 -->
    <nav class="flex w-[176px] shrink-0 flex-col gap-0.5 border-r border-[var(--gosslan-divider)] bg-[var(--gosslan-panel)] px-2 py-3">
      <button
        v-for="item in navItems"
        :key="item.key"
        class="tap-safe flex h-8 w-full items-center gap-2 rounded-[var(--gosslan-radius-md)] px-2.5 text-left text-[13px] transition"
        :aria-current="section === item.key ? 'true' : undefined"
        :class="section === item.key
          ? 'bg-[var(--gosslan-list-active)] font-medium text-[var(--gosslan-text)]'
          : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]'"
        @click="section = item.key"
      >
        <component :is="item.icon" class="h-4 w-4 shrink-0" :stroke-width="1.9" />
        <span class="min-w-0 flex-1 truncate" :title="item.label">{{ item.label }}</span>
      </button>
    </nav>

    <!-- 右：内容区（独立滚动，切换导航不改变窗口尺寸） -->
    <div class="min-w-0 flex-1 overflow-y-auto px-5 py-4">
      <ProfileSection v-if="section === 'profile'" :active="true" :reload-token="reloadToken" />
      <div v-else-if="section === 'appearance'" class="space-y-5">
        <AppearanceSection />
        <ChatStyleSection />
      </div>
      <!-- iOS 的「通用」里同时有「语言与地区」与「还原」⇒ 这里把 ResetSection 一起放在本分区 -->
      <div v-else-if="section === 'general'" class="space-y-5">
        <GeneralSection />
        <ResetSection @restored="reloadToken++" />
      </div>
      <NotificationSection v-else-if="section === 'notifications'" />
      <FilesSection v-else-if="section === 'files'" :active="true" :reload-token="reloadToken" />
      <NetworkSection v-else-if="section === 'network'" :active="true" :reload-token="reloadToken" />
      <StorageSection v-else-if="section === 'storage'" :active="true" :reload-token="reloadToken" />
      <SecuritySection v-else-if="section === 'security'" />
      <!-- 显式写 about 分支（不用兜底 v-else）：`SectionKey` 是闭合联合，且静态守卫要求
           「每个导航项都有可被检索到的渲染分支」，兜底分支会让守卫漏检 -->
      <div v-else-if="section === 'about'" class="space-y-5">
        <AboutSection @dev-open="devDiagOpen = true" />
      </div>
    </div>

    <!-- 开发者诊断面板（隐藏入口：连续点击设备指纹 7 次） -->
    <DevDiagPanel :open="devDiagOpen" @close="devDiagOpen = false" />
  </div>
</template>
