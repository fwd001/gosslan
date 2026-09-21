<script setup lang="ts">
import { computed, ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { useClipboard } from "@/composables/useClipboard";
import { version } from "../../../package.json";
import { t } from "@/i18n";

const emit = defineEmits<{ (e: "dev-open"): void }>();
const app = useAppStore();
const fullId = computed(() => app.device?.device_id ?? "");
const { copyContent } = useClipboard();

/**
 * 复制结果的就地反馈（`"" | "ok" | "fail"`，1.5s 后复位）。
 *
 * ⚠️ 用"就地提示"而不是 toast：指尖就在这一行上，弹 toast 反而是噪音
 * （同一取舍见 `MessageItem` 的复制按钮注释）。
 */
const copyState = ref<"" | "ok" | "fail">("");
let copyTimer: ReturnType<typeof setTimeout> | null = null;

/** 隐藏开发者诊断面板：连续点击设备指纹 7 次（2.5 秒窗口）。 */
const DEV_TAP_TARGET = 7;
const DEV_TAP_WINDOW_MS = 2500;
let tapCount = 0;
let tapTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * 点击设备指纹 = **复制**（这一行是 `role="button"`，点了必须有反应）。
 *
 * ⚠️ 原先这一行只累加连点计数、等第 7 次才开隐藏诊断面板 ⇒ **前 6 次点击毫无反馈**，
 * 用户看到的就是"按钮点了没反应、连弹窗都没有"（用户 2026-09-21，报的是「关于与重置」
 * 这一项里唯一可点的东西）。它自己的注释早就写着"要能触发『点击复制』"，但代码里
 * 从来没有复制这一步 —— 代码与注释不符也是这条缺陷的一部分。
 * 复制之后照旧累加计数：7 连点仍然打开诊断面板，只是现在每次点击都看得见。
 */
async function onFingerprintTap() {
  if (fullId.value) {
    const ok = await copyContent("device-id", fullId.value);
    copyState.value = ok ? "ok" : "fail";
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => {
      copyState.value = "";
    }, 1500);
  }
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
  <!-- 组标题用**条目名**「关于」（`settings.item.about`）而不是分组名「关于与重置」
       （`settings.group.about`）：分组名描述的是"关于 + 还原"两组，而本分区只有关于。
       桌面端导航项也读同一个键 ⇒ 「标签 = 内容」（settingsStructure 守卫钉住）。
       用户 2026-09-21：「桌面版的关于和重置里没有重置」——就是这条不一致。 -->
  <SettingsGroup :title="t('settings.item.about')" :footer="t('settings.group.about.footer', { version })">
    <div class="px-4 py-3">
      <div class="text-sm text-[var(--gosslan-text)]">{{ t("settings.about.fingerprint") }}</div>
      <!-- 保留 `select-text`（指纹要能手动选中复制）；`role`/`tabindex`/键盘让它同时是
           一个真按钮 —— 点了要能复制（见 `onFingerprintTap`），并就地给出结果提示。 -->
      <div class="mt-1 flex flex-wrap items-baseline gap-x-2">
        <div
          class="cursor-pointer select-text break-all font-mono text-xs leading-relaxed text-[var(--gosslan-text-2)] transition hover:text-[var(--gosslan-text)]"
          role="button"
          tabindex="0"
          :title="t('settings.about.fingerprintCopy')"
          @click="onFingerprintTap"
          @keydown.enter.prevent="onFingerprintTap"
          @keydown.space.prevent="onFingerprintTap"
        >
          {{ fullId }}
        </div>
        <span
          v-if="copyState"
          class="shrink-0 text-[11px]"
          :class="copyState === 'ok' ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-danger-ink)]'"
        >
          {{ copyState === "ok" ? t("common.copied") : t("msg.copyFail") }}
        </span>
      </div>
    </div>
  </SettingsGroup>
</template>
