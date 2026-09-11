<script setup lang="ts">
import { t } from "@/i18n";
import { computed } from "vue";
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import go from "highlight.js/lib/languages/go";
import xml from "highlight.js/lib/languages/xml";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import yaml from "highlight.js/lib/languages/yaml";
import { useAppStore } from "@/stores/useAppStore";
import { CODE_SURFACE } from "@/utils/previewMetrics";
import darkCss from "highlight.js/styles/github-dark.css?raw";
import lightCss from "highlight.js/styles/github.css?raw";

const app = useAppStore();

const props = defineProps<{
  code: string;
  language?: string;
  /**
   * 作为「气泡卡片的上半部分」使用（消息流里的代码气泡）：
   * - **不画描边**——描边会在卡片右缘与气泡尖角处露出一条竖线，尖角看着像"贴上去的"；
   *   去掉后卡片与尖角同色同边，融为一体（卡片与画布的区分由底色明暗差承担）。
   * - **下圆角抹平**、上圆角保留——下方紧接操作条，两者拼成一整块圆角卡片。
   * 独立展示（全文弹窗）时保持默认值，仍有描边与四角圆角。
   */
  attached?: boolean;
}>();

for (const [name, language] of Object.entries({
  bash,
  css,
  go,
  xml,
  java,
  javascript,
  json,
  markdown,
  python,
  rust,
  sql,
  typescript,
  yaml,
})) {
  hljs.registerLanguage(name, language);
}
hljs.registerLanguage("html", xml);

const lineCount = computed(() => props.code.split("\n").length);

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const AUTO_LANGS = [
  "rust", "javascript", "typescript", "python", "java", "go", "json", "bash", "shell",
  "sql", "html", "css", "xml", "yaml", "markdown",
];
const detectedLang = computed(() => {
  if (props.language && hljs.getLanguage(props.language)) return props.language;
  try {
    const r = hljs.highlightAuto(props.code, AUTO_LANGS);
    if (r.language && r.relevance >= 5) return r.language;
  } catch { /* 忽略 */ }
  return "plaintext";
});

const langLabel = computed(() => (detectedLang.value === "plaintext" ? "text" : detectedLang.value));

const codeBg = computed(() => (app.dark ? CODE_SURFACE.dark : CODE_SURFACE.light));
const codeFg = computed(() => (app.dark ? "#e6edf3" : "#24292e"));
const toolbarBg = computed(() => (app.dark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.04)"));
/** 工具栏文字（语言 · 行数，12px）：透明度是"白/黑蒙层的比例"，不是颜色本身，
 *  所以必须按两套底色分别验一遍对比度——亮色原为 0.4，实测只有 2.90（不达标），
 *  提到 0.55 后 ≈ 4.6；暗色 0.5 在 #161b22 上有 5.1，保持不变。 */
const toolbarFg = computed(() => (app.dark ? "rgba(255,255,255,0.5)" : "rgba(0,0,0,0.55)"));
const borderStyle = computed(() => app.dark ? "border-white/10" : "border-black/10");

const html = computed(() => {
  try {
    const lang = detectedLang.value;
    if (lang !== "plaintext" && hljs.getLanguage(lang)) {
      return hljs.highlight(props.code, { language: lang }).value;
    }
  } catch { /* 忽略 */ }
  return escapeHtml(props.code);
});
</script>

<template>
  <component :is="'style'">{{ app.dark ? darkCss : lightCss }}</component>
  <div
    class="overflow-hidden text-left"
    :class="attached ? 'rounded-t-[var(--gosslan-radius-sm)]' : ['rounded-[var(--gosslan-radius-md)]', borderStyle]"
    :style="{ borderWidth: attached ? '0px' : '1px' }"
  >
    <div class="flex items-center justify-between px-3" style="height: 32px" :style="{ background: toolbarBg }">
      <span class="text-xs" :style="{ color: toolbarFg }">{{ langLabel }} · {{ t("common.lines", { n: lineCount }) }}</span>
    </div>
    <pre
      class="code-pre gosslan-selectable"
      :style="{ background: codeBg, color: codeFg }"
    ><code v-html="html"></code></pre>
  </div>
</template>

<style scoped>
/* 纯展示：不折叠、不自带滚动条。截断由 MessageItem 的固定高度容器负责，
   完整内容的滚动由外层 Modal 容器负责。 */
.code-pre {
  margin: 0;
  padding: 12px 14px;
  font-size: 12.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-x: hidden;
}
.code-pre code {
  font-family: "JetBrains Mono", "Fira Code", Consolas, Menlo, monospace;
}
</style>
