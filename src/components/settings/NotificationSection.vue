<script setup lang="ts">
/**
 * 「通知」分区（iOS 概念：设置 → 通知）。
 *
 * 只放**通知本身**的开关，对应 iOS「通知」里的「允许通知」与「显示预览」：
 *   · 允许通知 = iOS 的 Allow Notifications；
 *   · 显示消息正文 = iOS 的 Show Previews（锁屏/通知中心是否显示内容）。
 * 与通知无关的项（语言、共享目录、存储…）一律不放 —— 那正是 2026-09-12 用户反馈的问题。
 */
import { ref } from "vue";
import { BellRing } from "lucide-vue-next";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import SettingsToggle from "@/components/settings/SettingsToggle.vue";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import { t } from "@/i18n";

const app = useAppStore();
const testing = ref(false);

/**
 * 发送一条测试通知：设置页唯一的「系统通知链路」自检入口。
 *
 * 为什么需要（用户 2026-09-14：Windows 同事收不到任何通知）：Windows 上通知失败可能是
 * **完全静默**的 —— 应用未安装（未注册 AUMID）、专注助手/勿扰、系统里把 Gosslan 的通知
 * 关了。用户只能描述"收不到"，无法区分是应用问题还是系统设置问题。后端会返回真实失败
 * 原因与平台排查说明，这里照实弹给用户（也让日志留下一条可发我们的记录）。
 */
async function sendTestNotification() {
  if (testing.value) return;
  testing.value = true;
  try {
    const hint = await api.sendTestNotification();
    app.toast(t("settings.notify.test.sent", { hint }), "success");
  } catch (e) {
    app.toastError(e, t("settings.notify.test.failed"));
  } finally {
    testing.value = false;
  }
}
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
    <SettingsRow :label="t('settings.notify.showContent')" :description="t('settings.notify.showContent.desc')">
      <SettingsToggle
        :label="t('settings.notify.showContent')"
        :model-value="app.notifyShowContent"
        :disabled="!app.notifyEnabled"
        @update:model-value="app.setNotifyShowContent"
      />
    </SettingsRow>
    <SettingsRow :label="t('settings.notify.remindSound')" :description="t('settings.notify.remindSound.desc')">
      <SettingsToggle
        :label="t('settings.notify.remindSound')"
        :model-value="app.remindSound"
        :disabled="!app.notifyEnabled"
        @update:model-value="app.setRemindSound"
      />
    </SettingsRow>
    <SettingsRow
      :label="t('settings.notify.remindVibrate')"
      :description="t('settings.notify.remindVibrate.desc')"
    >
      <SettingsToggle
        :label="t('settings.notify.remindVibrate')"
        :model-value="app.remindVibrate"
        :disabled="!app.notifyEnabled"
        @update:model-value="app.setRemindVibrate"
      />
    </SettingsRow>
    <SettingsRow :label="t('settings.notify.test')" :description="t('settings.notify.test.desc')" last>
      <button
        class="flex shrink-0 items-center gap-1.5 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
        :disabled="testing"
        :title="t('settings.notify.test.desc')"
        @click="sendTestNotification"
      >
        <BellRing class="h-3.5 w-3.5" />
        {{ testing ? t("settings.notify.test.sending") : t("settings.notify.test.btn") }}
      </button>
    </SettingsRow>
  </SettingsGroup>
</template>
