<script setup lang="ts">
/**
 * 「通知」分区（iOS 概念：设置 → 通知）。
 *
 * 只放**通知本身**的开关，对应 iOS「通知」里的「允许通知」与「显示预览」：
 *   · 允许通知 = iOS 的 Allow Notifications；
 *   · 显示消息正文 = iOS 的 Show Previews（锁屏/通知中心是否显示内容）。
 * 与通知无关的项（语言、共享目录、存储…）一律不放 —— 那正是 2026-09-12 用户反馈的问题。
 */
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import SettingsToggle from "@/components/settings/SettingsToggle.vue";
import { useAppStore } from "@/stores/useAppStore";
import { t } from "@/i18n";

const app = useAppStore();
</script>

<template>
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
</template>
