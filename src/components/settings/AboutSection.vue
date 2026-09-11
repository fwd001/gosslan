<script setup lang="ts">
import { computed } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { version } from "../../../package.json";
import { t } from "@/i18n";

const emit = defineEmits<{ (e: "dev-open"): void }>();
const app = useAppStore();
const fullId = computed(() => app.device?.device_id ?? "");

/** 隐藏开发者诊断面板：连续点击设备指纹 7 次（2.5 秒窗口）。 */
const DEV_TAP_TARGET = 7;
const DEV_TAP_WINDOW_MS = 2500;
let tapCount = 0;
let tapTimer: ReturnType<typeof setTimeout> | null = null;

function onFingerprintTap() {
  tapCount++;
  if (tapTimer) clearTimeout(tapTimer);
  tapTimer = setTimeout(() => {
    tapCount = 0;
  }, DEV_TAP_WINDOW_MS);
  if (tapCount >= DEV_TAP_TARGET) {
    tapCount = 0;
    if (tapTimer) {
      clearTimeout(tapTimer);
      tapTimer = null;
    }
    emit("dev-open");
  }
}
</script>

<template>
  <SettingsGroup :title="t('settings.group.about')" :footer="t('settings.group.about.footer', { version })">
    <div class="px-4 py-3">
      <div class="text-sm text-[var(--gosslan-text)]">{{ t("settings.about.fingerprint") }}</div>
      <!-- 保留 `select-text`（指纹要能手动选中复制），所以不换成 <button>，
           而是补 role/tabindex/键盘 —— 键盘用户同样要能触发"点击复制"。 -->
      <div
        class="mt-1 select-text break-all font-mono text-xs leading-relaxed text-[var(--gosslan-text-2)]"
        role="button"
        tabindex="0"
        @click="onFingerprintTap"
        @keydown.enter.prevent="onFingerprintTap"
        @keydown.space.prevent="onFingerprintTap"
      >
        {{ fullId }}
      </div>
    </div>
  </SettingsGroup>
</template>
