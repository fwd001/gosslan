<script setup lang="ts">
import { computed, ref } from "vue";
import hljs from "highlight.js";
import "highlight.js/styles/github-dark.css";
import { Check, ChevronDown, ChevronUp, Copy, Maximize2, Minimize2 } from "lucide-vue-next";

const props = defineProps<{ code: string; language?: string }>();

const copied = ref(false);
/** 超过 7 行的代码块默认折叠（可手动展开/收起）。 */
const COLLAPSE_LINES = 7;
const lineCount = computed(() => props.code.split("\n").length);
const collapsed = ref(lineCount.value > COLLAPSE_LINES);
const expanded = ref(false);

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** 自动检测语言：指定语言优先；highlightAuto 命中常用语言且置信度足够才采用，否则兜底纯文本。 */
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
  } catch {
    /* 忽略，走纯文本 */
  }
  return "plaintext";
});

const langLabel = computed(() => (detectedLang.value === "plaintext" ? "text" : detectedLang.value));

const html = computed(() => {
  try {
    const lang = detectedLang.value;
    if (lang !== "plaintext" && hljs.getLanguage(lang)) {
      return hljs.highlight(props.code, { language: lang }).value;
    }
  } catch {
    /* 忽略 */
  }
  // 纯文本兜底（仅转义，无高亮）
  return escapeHtml(props.code);
});

async function copy() {
  try {
    await navigator.clipboard.writeText(props.code);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    /* ignore */
  }
}
</script>

<template>
  <div class="overflow-hidden rounded-lg border border-white/10 text-left">
    <div class="flex items-center justify-between bg-white/5 px-3" style="height: 32px">
      <span class="text-xs text-white/50">{{ langLabel }} · {{ lineCount }} 行</span>
      <div class="flex items-center gap-1">
        <button class="code-tool" title="复制" @click="copy">
          <Check v-if="copied" class="h-3.5 w-3.5 text-green-400" />
          <Copy v-else class="h-3.5 w-3.5" />
        </button>
        <button v-if="lineCount > COLLAPSE_LINES" class="code-tool" :title="collapsed ? '展开' : '折叠'" @click="collapsed = !collapsed">
          <ChevronUp v-if="collapsed" class="h-3.5 w-3.5" />
          <ChevronDown v-else class="h-3.5 w-3.5" />
        </button>
        <button class="code-tool" :title="expanded ? '退出全屏' : '全屏'" @click="expanded = !expanded">
          <Minimize2 v-if="expanded" class="h-3.5 w-3.5" />
          <Maximize2 v-else class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
    <!-- 自动换行，无横向滚动条；超过 7 行默认折叠，底部渐隐 + 展开按钮 -->
    <div class="relative">
      <pre
        class="code-pre"
        :class="expanded ? '' : collapsed ? 'code-collapsed' : 'max-h-80 overflow-y-auto'"
      ><code v-html="html"></code></pre>
      <div
        v-if="!expanded && collapsed"
        class="pointer-events-none absolute inset-x-0 bottom-0 h-10 bg-gradient-to-t from-[#0d1117] to-transparent"
      ></div>
      <button
        v-if="!expanded && collapsed"
        class="absolute bottom-1.5 left-1/2 -translate-x-1/2 rounded-full bg-white/10 px-3 py-1 text-[11px] text-white/80 backdrop-blur transition hover:bg-white/20"
        @click="collapsed = false"
      >
        展开全部 {{ lineCount }} 行
      </button>
    </div>
  </div>
</template>

<style scoped>
.code-pre {
  margin: 0;
  padding: 12px 14px;
  background: #0d1117;
  font-size: 12.5px;
  line-height: 1.6;
  /* 自动换行：不出现横向滚动条 */
  white-space: pre-wrap;
  word-break: break-word;
  overflow-x: hidden;
}
.code-collapsed {
  max-height: 9.5rem; /* ≈7 行 */
  overflow: hidden;
}
.code-pre code {
  font-family: "JetBrains Mono", "Fira Code", Consolas, Menlo, monospace;
}
.code-tool {
  display: inline-flex;
  align-items: center;
  background: transparent;
  border: none;
  color: rgba(255, 255, 255, 0.55);
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
}
.code-tool:hover {
  background: rgba(255, 255, 255, 0.1);
  color: #fff;
}
</style>
