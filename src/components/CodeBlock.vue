<script setup lang="ts">
import { computed, ref } from "vue";
import hljs from "highlight.js";
import { useAppStore } from "@/stores/useAppStore";
import darkCss from "highlight.js/styles/github-dark.css?raw";
import lightCss from "highlight.js/styles/github.css?raw";
import { Check, ChevronDown, ChevronUp, Copy, Maximize2, Minimize2 } from "lucide-vue-next";

const app = useAppStore();

const props = withDefaults(defineProps<{ code: string; language?: string; preview?: boolean; previewLines?: number; previewAction?: () => void }>(), {
  preview: false,
  previewLines: 5,
});

const copied = ref(false);
/** 超过 7 行的代码块默认折叠（可手动展开/收起）。 */
const COLLAPSE_LINES = 7;
const lineCount = computed(() => props.code.split("\n").length);
const collapsed = ref(lineCount.value > COLLAPSE_LINES);
const expanded = ref(false);

/** preview 模式：代码行数超过 previewLines 时只显示截断预览，由父组件处理展开（Modal） */
const previewActive = computed(() => props.preview && lineCount.value > props.previewLines);
const clampedCode = computed(() => {
  if (!previewActive.value) return props.code;
  return props.code.split("\n").slice(0, props.previewLines).join("\n");
});
/** preview 模式下 pre 的固定高度（行数 × 行高 20px） */
const previewHeight = computed(() => `${props.previewLines * 20}px`);

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

// 主题相关颜色：dark = 深色代码块，light = 浅色代码块
const codeBg = computed(() => (app.dark ? "#0d1117" : "#f6f8fa"));
const codeFg = computed(() => (app.dark ? "#e6edf3" : "#24292e"));
const toolbarBg = computed(() => (app.dark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.04)"));
const toolbarFg = computed(() => (app.dark ? "rgba(255,255,255,0.5)" : "rgba(0,0,0,0.4)"));
const borderStyle = computed(() => app.dark ? "border-white/10" : "border-black/10");
const toolBtnFg = computed(() => (app.dark ? "rgba(255,255,255,0.55)" : "rgba(0,0,0,0.45)"));
const toolBtnHoverBg = computed(() => (app.dark ? "rgba(255,255,255,0.1)" : "rgba(0,0,0,0.06)"));
const toolBtnHoverFg = computed(() => (app.dark ? "#fff" : "#24292e"));

const html = computed(() => {
  try {
    const lang = detectedLang.value;
    if (lang !== "plaintext" && hljs.getLanguage(lang)) {
      return hljs.highlight(clampedCode.value, { language: lang }).value;
    }
  } catch {
    /* 忽略 */
  }
  // 纯文本兜底（仅转义，无高亮）
  return escapeHtml(clampedCode.value);
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
  <!-- highlight.js 主题样式：dark 用 github-dark，light 用 github -->
  <component :is="'style'">{{ app.dark ? darkCss : lightCss }}</component>
  <div class="overflow-hidden rounded-lg text-left" :class="borderStyle" :style="{ borderWidth: '1px' }">
    <div class="flex items-center justify-between px-3" style="height: 32px" :style="{ background: toolbarBg }">
      <span class="text-xs" :style="{ color: toolbarFg }">{{ langLabel }} · {{ lineCount }} 行</span>
      <div class="flex items-center gap-1">
        <!-- 复制：所有模式都显示 -->
        <button class="code-tool" title="复制" @click="copy" :style="{ color: toolBtnFg }">
          <Check v-if="copied" class="h-3.5 w-3.5 text-green-500" />
          <Copy v-else class="h-3.5 w-3.5" />
        </button>
        <!-- preview 模式：「弹窗预览」按钮（调用父组件传入的 previewAction） -->
        <button v-if="previewActive" class="code-tool text-[11px] whitespace-nowrap" :style="{ color: toolBtnFg }" @click.stop="previewAction?.()">
          弹窗预览
        </button>
        <!-- 非 preview 模式：展开/折叠 + 全屏 -->
        <button v-if="!previewActive && lineCount > COLLAPSE_LINES" class="code-tool" :title="collapsed ? '展开' : '折叠'" @click.stop="collapsed = !collapsed" :style="{ color: toolBtnFg }">
          <ChevronUp v-if="collapsed" class="h-3.5 w-3.5" />
          <ChevronDown v-else class="h-3.5 w-3.5" />
        </button>
        <button v-if="!previewActive" class="code-tool" :title="expanded ? '退出全屏' : '全屏'" @click="expanded = !expanded" :style="{ color: toolBtnFg }">
          <Minimize2 v-if="expanded" class="h-3.5 w-3.5" />
          <Maximize2 v-else class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
    <!-- preview 模式：固定高度，无渐变无展开按钮 -->
    <div v-if="previewActive" class="relative">
      <pre class="code-pre" :style="{ background: codeBg, color: codeFg, height: previewHeight, overflow: 'hidden' }"><code v-html="html"></code></pre>
    </div>
    <!-- 非 preview 模式：原有折叠/全屏逻辑 -->
    <div v-else class="relative">
      <pre
        class="code-pre"
        :class="expanded ? '' : collapsed ? 'code-collapsed' : 'max-h-80 overflow-y-auto'"
        :style="{ background: codeBg, color: codeFg }"
      ><code v-html="html"></code></pre>
      <div
        v-if="!expanded && collapsed"
        class="pointer-events-none absolute inset-x-0 bottom-0 h-10 bg-gradient-to-t to-transparent"
        :style="{ background: `linear-gradient(to top, ${codeBg}, transparent)` }"
      ></div>
      <button
        v-if="!expanded && collapsed"
        class="absolute bottom-1.5 left-1/2 -translate-x-1/2 rounded-full px-3 py-1 text-[11px] backdrop-blur transition"
        :style="{ background: toolBtnHoverBg, color: toolBtnHoverFg }"
        @click.stop="collapsed = false"
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
  font-size: 12.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-x: hidden;
}
.code-collapsed {
  max-height: 9.5rem;
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
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
}
.code-tool:hover {
  background: rgba(128, 128, 128, 0.1);
}
</style>
