<script setup lang="ts">
import { computed } from "vue";
import hljs from "highlight.js";
import { useAppStore } from "@/stores/useAppStore";
import darkCss from "highlight.js/styles/github-dark.css?raw";
import lightCss from "highlight.js/styles/github.css?raw";

const app = useAppStore();

const props = defineProps<{ code: string; language?: string }>();

const lineCount = computed(() => props.code.split("\n").length);

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const AUTO_LANGS = [
  "rust", "javascript", "typescript", "python", "java", "go", "c", "cpp", "csharp",
  "json", "bash", "shell", "sql", "html", "css", "xml", "yaml", "markdown",
  "kotlin", "swift", "php", "ruby", "toml", "ini", "diff", "dockerfile",
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

const codeBg = computed(() => (app.dark ? "#0d1117" : "#f6f8fa"));
const codeFg = computed(() => (app.dark ? "#e6edf3" : "#24292e"));
const toolbarBg = computed(() => (app.dark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.04)"));
const toolbarFg = computed(() => (app.dark ? "rgba(255,255,255,0.5)" : "rgba(0,0,0,0.4)"));
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
  <div class="overflow-hidden rounded-lg text-left" :class="borderStyle" :style="{ borderWidth: '1px' }">
    <div class="flex items-center justify-between px-3" style="height: 32px" :style="{ background: toolbarBg }">
      <span class="text-xs" :style="{ color: toolbarFg }">{{ langLabel }} · {{ lineCount }} 行</span>
    </div>
    <pre
      class="code-pre"
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
