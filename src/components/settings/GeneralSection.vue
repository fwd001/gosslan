<script setup lang="ts">
/**
 * 「通用」设置分区：语言 / 通知 / 共享目录。
 *
 * 抽出来的原因：设置内容要在**三种外壳**里复用 —— 桌面独立窗口（`SettingsWindow`）、
 * 移动端整页（`SettingsPanel` 的全屏分支）、以及旧的弹窗表单。三份重复标记必然会漂移，
 * 所以「长什么样」只写在这里，外壳只负责怎么摆。
 */
import { computed } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import SettingsToggle from "@/components/settings/SettingsToggle.vue";
import { FolderOpen } from "lucide-vue-next";
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

async function pickShareDir() {
  const picked = await openDialog({ directory: true });
  if (typeof picked === "string") {
    try {
      await app.setShareDir(picked);
      app.toast(t("settings.toast.shareSet"), "success");
    } catch (e) {
      app.toastError(e, t("settings.toast.shareFail"));
    }
  }
}
</script>

<template>
  <div class="space-y-5">
    <!-- 语言 -->
    <SettingsGroup :title="t('settings.language.title')" :footer="t('settings.language.desc')">
      <SettingsRow :label="t('settings.language.title')" last>
        <!-- flex-wrap + whitespace-nowrap：英文「Follow System」较长，放不下时换行而不是溢出/挤压 -->
        <div class="flex flex-wrap items-center gap-1">
          <button
            v-for="l in languageOptions"
            :key="l.value"
            class="whitespace-nowrap rounded-[var(--gosslan-radius-sm)] px-2.5 py-1 text-xs transition"
            :class="app.language === l.value
              ? 'bg-primary text-white'
              : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
            @click="app.setLanguage(l.value)"
          >
            {{ l.label }}
          </button>
        </div>
      </SettingsRow>
    </SettingsGroup>

    <!-- 通知 -->
    <SettingsGroup
      :title="t('settings.group.notifications')"
      :footer="t('settings.group.notifications.footer')"
    >
      <SettingsRow :label="t('settings.notify.enabled')" :description="t('settings.notify.enabled.desc')">
        <SettingsToggle
          :label="t('settings.notify.enabled')"
          :model-value="app.notifyEnabled"
          @update:model-value="app.setNotifyEnabled"
        />
      </SettingsRow>
      <SettingsRow
        :label="t('settings.notify.showContent')"
        :description="t('settings.notify.showContent.desc')"
        last
      >
        <SettingsToggle
          :label="t('settings.notify.showContent')"
          :model-value="app.notifyShowContent"
          :disabled="!app.notifyEnabled"
          @update:model-value="app.setNotifyShowContent"
        />
      </SettingsRow>
    </SettingsGroup>

    <!-- 共享目录 -->
    <SettingsGroup :title="t('settings.group.share')" :footer="t('settings.group.share.footer')">
      <SettingsRow :label="t('settings.share.folder')" :description="app.shareDir || t('common.notSet')" last>
        <button
          class="flex items-center gap-1.5 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)]"
          @click="pickShareDir"
        >
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("common.chooseFolder") }}
        </button>
      </SettingsRow>
    </SettingsGroup>
  </div>
</template>
