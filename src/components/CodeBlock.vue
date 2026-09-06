<script setup lang="ts">
import { computed, ref } from "vue";
import hljs from "highlight.js";
import { useAppStore } from "@/stores/useAppStore";
import darkCss from "highlight.js/styles/github-dark.css?raw";
import lightCss from "highlight.js/styles/github.css?raw";
import { Check, Copy } from "lucide-vue-next";

const app = useAppStore();

const props = defineProps<{ code: string; language?: string }>();

const copied = ref(false);
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
const toolBtnFg = computed(() => (app.dark ? "rgba(255,255,255,0.55)" : "rgba(0,0,0,0.45)"));

const html = computed(() => {
  try {
    const lang = detectedLang.value;
    if (lang !== "plaintext" && hljs.getLanguage(lang)) {
      return hljs.highlight(props.code, { language: lang }).value;
    }
  } catch { /* 忽略 */ }
  return escapeHtml(props.code);
});

async function copy() {
  try {
    await navigator.clipboard.writeText(props.code);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch { /* ignore */ }
}
</script>

<template>
  <component :is="'style'">{{ app.dark ? darkCss : lightCss }}</component>
  <div class="overflow-hidden rounded-lg text-left" :class="borderStyle" :style="{ borderWidth: '1px' }">
    <div class="flex items-center justify-between px-3" style="height: 32px" :style="{ background: toolbarBg }">
      <span class="text-xs" :style="{ color: toolbarFg }">{{ langLabel }} · {{ lineCount }} 行</span>
      <button class="code-tool" title="复制" @click="copy" :style="{ color: toolBtnFg }">
        <Check v-if="copied" class="h-3.5 w-3.5 text-green-500" />
        <Copy v-else class="h-3.5 w-3.5" />
      </button>
    </div>
    <pre
      class="code-pre"
      :style="{ background: codeBg, color: codeFg }"
    ><code v-html="html"></code></pre>
  </div>
</template>

<style scoped>
.code-pre {
  margin: 0;
  padding: 12px 14px;
  font-size: 12.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-x: hidden;
  max-height: 80vh;
  overflow-y: auto;
}
.code-pre code {
  font-family: "JetBrains Mono", "Fira Code", Consolas, Menlo, monospace;
}
.code-tool {
  display: inline-flex;
  align-items: center;
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
}
.code-tool:hover {
  background: rgba(128, 128, 128, 0.1);
}
</style>
