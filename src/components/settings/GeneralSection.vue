<script setup lang="ts">
/**
 * 「通用」分区（iOS 概念：设置 → 通用）。
 *
 * 这里**只放语言与地区**，对应 iOS「通用 → 语言与地区」的定位。
 * 曾经这个分区同时塞了「通知」和「共享目录」，而外壳的导航项又只写「通知」——
 * 于是用户点开「通知」看到的第 1 项是语言、第 3 项是共享目录（2026-09-12 反馈）。
 * 现在按 iOS 的概念拆开：
 *   · 通知 → `NotificationSection`（通知）；
 *   · 共享目录 → `FilesSection`（文件与共享，与"接收文件目录"同属文件位置）；
 *   · 还原（恢复默认 / 清除聊天数据）→ `ResetSection`，桌面端是**自己的导航项「重置」**、
 *     移动端是「关于与重置」分组下的**「还原与重置」**二级页（用户 2026-09-21：
 *     「桌面版的关于和重置里没有重置」——此前它被塞在「关于与重置」项里，等于标签说了
 *     却没有；也**不再**借用 iOS「通用里有还原」的做法，那会让「通用」里出现破坏性操作）。
 *
 * 抽出独立组件的原因：设置内容要在**三种外壳**里复用（独立窗口 / 移动端整页 / 旧弹窗），
 * 三份重复标记必然漂移，所以「长什么样」只写在这里，外壳只负责怎么摆。
 */
import { computed } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import { t, type LanguagePreference } from "@/i18n";

const app = useAppStore();

/**
 * 语言切换三态。语言名（简体中文 / English）按国际惯例**不自翻译**，任何语言下都认得；
 * 「跟随系统」是动作说明，必须随当前语言翻译 → 放 computed 里调 t() 建立响应式依赖。
 */
const languageOptions = computed<{ value: LanguagePreference; label: string }[]>(() => [
  { value: "system", label: t("settings.language.system") },
  { value: "zh-CN", label: "简体中文" },
  { value: "en-US", label: "English" },
]);
</script>

<template>
  <SettingsGroup :title="t('settings.group.language')" :footer="t('settings.language.desc')">
    <SettingsRow :label="t('settings.language.title')" last>
      <!-- flex-wrap + whitespace-nowrap：英文「Follow System」较长，放不下时换行而不是溢出/挤压 -->
      <SegmentedControl
        :model-value="app.language"
        :options="languageOptions"
        size="sm"
        :aria-label="t('settings.language.title')"
        @update:model-value="(v) => app.setLanguage(v as LanguagePreference)"
      />
    </SettingsRow>
  </SettingsGroup>
</template>
