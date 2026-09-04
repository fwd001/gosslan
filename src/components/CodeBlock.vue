<script setup lang="ts">
import { computed, ref } from "vue";
import hljs from "highlight.js";
import "highlight.js/styles/github-dark.css";
import { Check, ChevronDown, ChevronUp, Copy, Maximize2, Minimize2 } from "lucide-vue-next";

const props = defineProps<{ code: string; language?: string }>();

const copied = ref(false);
const collapsed = ref(false);
const expanded = ref(false);

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const html = computed(() => {
  try {
    if (props.language && hljs.getLanguage(props.language)) {
      return hljs.highlight(props.code, { language: props.language }).value;
    }
    return hljs.highlightAuto(props.code).value;
  } catch {
    return escapeHtml(props.code);
  }
});

const detectedLang = computed(() => {
  if (props.language && hljs.getLanguage(props.language)) return props.language;
  try {
    return hljs.highlightAuto(props.code).language ?? "code";
  } catch {
    return "code";
  }
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
      <span class="text-xs text-white/50">{{ detectedLang }}</span>
      <div class="flex items-center gap-1">
        <button class="code-tool" title="复制" @click="copy">
          <Check v-if="copied" class="h-3.5 w-3.5 text-green-400" />
          <Copy v-else class="h-3.5 w-3.5" />
        </button>
        <button class="code-tool" :title="collapsed ? '展开' : '折叠'" @click="collapsed = !collapsed">
          <ChevronUp v-if="collapsed" class="h-3.5 w-3.5" />
          <ChevronDown v-else class="h-3.5 w-3.5" />
        </button>
        <button class="code-tool" :title="expanded ? '退出全屏' : '全屏'" @click="expanded = !expanded">
          <Minimize2 v-if="expanded" class="h-3.5 w-3.5" />
          <Maximize2 v-else class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
    <pre
      class="code-pre overflow-x-auto"
      :class="expanded ? '' : collapsed ? 'max-h-16 overflow-hidden' : 'max-h-80'"
    ><code v-html="html"></code></pre>
  </div>
</template>

<style scoped>
.code-pre {
  margin: 0;
  padding: 12px 14px;
  background: #0d1117;
  font-size: 12.5px;
  line-height: 1.6;
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
