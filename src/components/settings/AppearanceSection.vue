<script setup lang="ts">
import { useAppStore, type AppearanceMode } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import { t } from "@/i18n";

const app = useAppStore();

const presets = ["#3b82f6", "#00b578", "#ff6b35", "#8b5cf6", "#e53e3e", "#0ea5e9"];
/** label 存 i18n key，由模板 `t(f.label)` 翻译显示。 */
const fonts = [
  { value: "", label: "settings.appearance.font.system" },
  {
    value: "-apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif",
    label: "settings.appearance.font.pingfang",
  },
  { value: "'Noto Sans SC', 'Source Han Sans SC', sans-serif", label: "settings.appearance.font.siyuan" },
  { value: "'JetBrains Mono', Consolas, monospace", label: "settings.appearance.font.mono" },
];

/**
 * 外观三态。原来只有"深色模式"开关（二态），表达不出"跟随系统"——
 * 用户在 macOS / Android 上切换外观时 App 不跟随，是最容易被感知的"不像原生"之处
 * （见 2026-09-10 Apple HIG 审计 P0-2）。改为显式三选一，默认跟随系统。
 * 导航栏的太阳/月亮按钮仍保留为快捷开关（它总是切成显式的浅/深）。
 */
const appearanceOptions: { value: AppearanceMode; label: string }[] = [
  { value: "system", label: "settings.appearance.system" },
  { value: "light", label: "settings.appearance.light" },
  { value: "dark", label: "settings.appearance.dark" },
];
</script>

<template>
  <SettingsGroup :title="t('settings.group.appearance')">
    <SettingsRow
      :label="t('settings.appearance.mode')"
      :description="t('settings.appearance.mode.desc')"
    >
      <!-- 分段控件：外圆角 md(8) + p-0.5(2) → 内圆角取 sm(6)，符合同心公式（design-guidelines §1.3） -->
      <div
        role="radiogroup"
        :aria-label="t('settings.appearance.mode.aria')"
        class="flex items-center gap-0.5 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] p-0.5"
      >
        <button
          v-for="m in appearanceOptions"
          :key="m.value"
          role="radio"
          :aria-checked="app.appearance === m.value"
          class="rounded-[var(--gosslan-radius-sm)] px-2.5 py-1 text-xs transition"
          :class="app.appearance === m.value
            ? 'bg-[var(--gosslan-panel)] font-medium text-[var(--gosslan-accent-ink)]'
            : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          @click="app.setAppearance(m.value)"
        >
          {{ t(m.label) }}
        </button>
      </div>
    </SettingsRow>

    <SettingsRow
      :label="t('settings.appearance.themeColor')"
      :description="t('settings.appearance.themeColor.desc')"
    >
      <div class="flex items-center gap-1.5">
        <!-- aria-label：色板格子只有颜色没有文字，读屏下必须靠 label 才知道它是什么 -->
        <button
          v-for="c in presets"
          :key="c"
          class="h-6 w-6 rounded-full transition hover:scale-110"
          :style="{ background: c, outline: app.themeColor === c ? '2px solid var(--gosslan-text)' : 'none', outlineOffset: '1px' }"
          :aria-label="t('settings.appearance.themeColor.aria', { color: c })"
          :aria-pressed="app.themeColor === c"
          @click="app.setThemeColor(c)"
        ></button>
        <input
          type="color"
          :value="app.themeColor"
          class="h-6 w-7 cursor-pointer rounded-[var(--gosslan-radius-xs)] border-0 bg-transparent p-0"
          :title="t('settings.appearance.customColor')"
          :aria-label="t('settings.appearance.customColor.aria')"
          @input="(e) => app.setThemeColor((e.target as HTMLInputElement).value)"
        />
      </div>
    </SettingsRow>

    <SettingsRow :label="t('settings.appearance.font')" last>
      <!-- 外壳 + `.gosslan-select`：跨平台高度与箭头一致（见 style.css 说明） -->
      <span class="gosslan-select-wrap max-w-[180px]">
        <select
          :aria-label="t('settings.appearance.font.aria')"
          class="gosslan-select max-w-[180px]"
          :value="app.fontFamily"
          @change="(e) => app.setFontFamily((e.target as HTMLSelectElement).value)"
        >
          <option v-for="f in fonts" :key="f.value" :value="f.value">{{ t(f.label) }}</option>
        </select>
      </span>
    </SettingsRow>
  </SettingsGroup>
</template>
