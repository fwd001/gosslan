<script setup lang="ts">
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { CHAT_FONT_SIZES, CHAT_PRESETS, resolveChatColors, type ChatPreset } from "@/utils/chatStyle";
import { t } from "@/i18n";

const app = useAppStore();

/** 预览色："theme" 预设按当前主题色实时派生，其余取表明暗值。 */
function swatchOf(p: ChatPreset): { mineBubble: string; otherBubble: string } {
  const c = resolveChatColors(p.key, app.themeColor, app.dark);
  return { mineBubble: c.mineBubble, otherBubble: c.otherBubble };
}
</script>

<template>
  <SettingsGroup
    :title="t('settings.group.chatStyle')"
    :footer="t('settings.group.chatStyle.footer')"
  >
    <!-- 字体大小 -->
    <div class="px-4 py-3">
      <div class="mb-2 text-sm text-[var(--gosslan-text)]">{{ t("settings.chatStyle.fontSize") }}</div>
      <div class="flex gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] p-0.5">
        <button
          v-for="f in CHAT_FONT_SIZES"
          :key="f.key"
          class="flex-1 rounded-[var(--gosslan-radius-sm)] py-1.5 text-sm transition"
          :class="app.chatStyle.fontSize === f.key ? 'bg-[var(--gosslan-panel)] text-[var(--gosslan-text)] shadow-sm' : 'text-[var(--gosslan-text-2)] hover:text-[var(--gosslan-text)]'"
          :aria-pressed="app.chatStyle.fontSize === f.key"
          @click="app.setChatStyle({ fontSize: f.key })"
        >
          {{ t(f.label) }}
        </button>
      </div>
    </div>

    <div class="ml-4 h-px bg-[var(--gosslan-divider)]" />

    <!-- 气泡配色 -->
    <div class="px-4 py-3">
      <div class="mb-2 text-sm text-[var(--gosslan-text)]">{{ t("settings.chatStyle.bubble") }}</div>
      <div class="grid grid-cols-3 gap-2">
        <button
          v-for="p in CHAT_PRESETS"
          :key="p.key"
          class="rounded-[var(--gosslan-radius-md)] border p-2 transition hover:bg-[var(--gosslan-hover)]"
          :class="app.chatStyle.preset === p.key ? 'border-primary ring-1 ring-primary' : 'border-[var(--gosslan-border)]'"
          :title="t(p.label)"
          :aria-pressed="app.chatStyle.preset === p.key"
          @click="app.setChatStyle({ preset: p.key })"
        >
          <div class="mb-1 text-center text-[11px] text-[var(--gosslan-text-2)]">{{ t(p.label) }}</div>
          <div class="flex items-center gap-1">
            <span class="h-4 flex-1 rounded-[var(--gosslan-radius-xs)]" :style="{ background: swatchOf(p).mineBubble }"></span>
            <span
              class="h-4 flex-1 rounded-[var(--gosslan-radius-xs)] border border-[var(--gosslan-border)]"
              :style="{ background: swatchOf(p).otherBubble }"
            ></span>
          </div>
        </button>
      </div>
    </div>
  </SettingsGroup>
</template>
