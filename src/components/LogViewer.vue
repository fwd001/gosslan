<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { ArrowLeft, Copy, RefreshCw, Search, Trash2, X } from "lucide-vue-next";
import { api } from "@/api";
import { t } from "@/i18n";
import { highlightText } from "@/utils/highlight";
import { useDeferredRef } from "@/composables/useDeferredRef";
import { LOG_LEVEL_TEXT, filterLogLines } from "@/utils/logFilter";
import type { LogEntry } from "@/types";

/**
 * 运行日志查看页。两种形态：
 * - `standalone`（桌面独立窗口）：由 App.vue 按窗口 label 直接渲染，关闭按钮走 close_log_window。
 * - 移动端页面：作为全屏覆盖层渲染，返回按钮 emit('back')。
 */
const props = defineProps<{ standalone?: boolean }>();
const emit = defineEmits<{ (e: "back"): void }>();

/** 时间正序（旧 → 新）的原始日志；展示与复制都从它派生。 */
const logs = ref<LogEntry[]>([]);
/** 展示用：最新在上（倒序），打开即见最近的错误。 */
const displayLogs = computed(() => [...logs.value].reverse());
const autoRefresh = ref(true);
const copied = ref(false);
/** 清空两段式确认：第一次点击进入待确认态，再次点击才真正清空。 */
const confirmClear = ref(false);

function fmt(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(
    d.getMinutes(),
  )}:${p(d.getSeconds())}`;
}

/** 过滤词。字面**包含**匹配（不区分大小写，不做模糊/正则）—— 见 `@/utils/logFilter`。 */
/** 绑在过滤框上的原值（立即更新）。 */
const filter = ref("");
/**
 * 延迟镜像：过滤 + 每行 4 处高亮都用它。
 * 日志动辄上千行，`rows` 每次重算都要重建上千个 `v-html` 节点；连发粘贴时若每个
 * 字符都算一遍，主线程会被占满 ⇒ 过滤框打字/粘贴一顿一顿（见 utils/defer.ts 说明）。
 */
const deferredFilter = useDeferredRef(filter);
const trimmedFilter = computed(() => deferredFilter.value.trim());

/**
 * 展示行（倒序）—— 只补上「已格式化的时间」，匹配判据完全交给 `@/utils/logFilter`。
 * 判据必须与屏幕上渲染的文本一致（「所见即所匹配」），所以时间取 `HH:MM:SS`，
 * 不含未显示的日期部分。
 */
const lines = computed(() =>
  displayLogs.value.map((l, i) => ({
    key: `${l.ts}-${l.target}-${i}`,
    level: l.level,
    time: fmt(l.ts).slice(11),
    target: l.target,
    message: l.message,
  })),
);

/** 命中过滤词的行（保持倒序）。 */
const matched = computed(() => filterLogLines(lines.value, deferredFilter.value));

/** 渲染行：命中处加高亮标记（复用会话搜索同一套 `highlightText`，视觉语言一致）。 */
const rows = computed(() =>
  matched.value.map((l) => ({
    key: l.key,
    level: l.level,
    time: highlightText(l.time, trimmedFilter.value),
    levelText: highlightText(LOG_LEVEL_TEXT[l.level] ?? l.level, trimmedFilter.value),
    target: highlightText(`[${l.target}]`, trimmedFilter.value),
    message: highlightText(l.message, trimmedFilter.value),
  })),
);

async function load() {
  try {
    logs.value = await api.getLogs();
  } catch {
    /* 后端暂不可用（如 logs 窗口创建瞬间 state 未就绪）→ 保持现状，下次轮询重试 */
  }
}

let timer: ReturnType<typeof setInterval> | null = null;
function syncTimer() {
  if (timer) {
    clearInterval(timer);
    timer = null;
  }
  if (autoRefresh.value) {
    timer = setInterval(load, 2000);
  }
}

onMounted(() => {
  void load();
  syncTimer();
});
onUnmounted(() => {
  if (timer) clearInterval(timer);
});

/** 一键复制：时间正序（便于按因果排查）。 */
async function copyAll() {
  const text = logs.value
    .map((l) => `[${fmt(l.ts)}] [${LOG_LEVEL_TEXT[l.level] ?? l.level}] [${l.target}] ${l.message}`)
    .join("\n");
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      // fallback：非 secure context（老 WebView）用 execCommand
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
    copied.value = true;
    setTimeout(() => (copied.value = false), 2000);
  } catch {
    /* 复制失败静默：不打断用户 */
  }
}

function onClear() {
  if (!confirmClear.value) {
    confirmClear.value = true;
    setTimeout(() => (confirmClear.value = false), 3000);
    return;
  }
  confirmClear.value = false;
  void api.clearLogs().then(load).catch(() => {});
}

async function close() {
  if (props.standalone) {
    await api.closeLogWindow().catch(() => {});
  } else {
    emit("back");
  }
}

const levelClass = (lv: string) =>
  lv === "error"
    ? "text-[var(--gosslan-danger-ink)]"
    : lv === "warn"
      ? "text-[var(--gosslan-warning-ink)]"
      : "text-[var(--gosslan-text-2)]";
</script>

<template>
  <div class="fixed inset-0 z-[70] flex flex-col bg-[var(--gosslan-app-bg)] font-gosslan text-[var(--gosslan-text)]">
    <!-- 顶部工具栏 -->
    <div
      class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-caption)] px-3"
      :style="{ height: 'var(--gosslan-header-h)' }"
    >
      <button
        class="flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="standalone ? t('logs.close') : t('logs.back')"
        :aria-label="standalone ? t('logs.close') : t('logs.back')"
        @click="close"
      >
        <X v-if="standalone" class="h-4 w-4" />
        <ArrowLeft v-else class="h-5 w-5" />
      </button>

      <div class="flex min-w-0 items-baseline gap-2">
        <span class="truncate text-[15px] font-medium" :title="t('logs.title')">{{ t("logs.title") }}</span>
        <span class="shrink-0 text-xs text-[var(--gosslan-text-2)]">
          {{ t("logs.count", { n: logs.length }) }}
        </span>
      </div>

      <div class="ml-auto flex items-center gap-1">
        <!-- 自动刷新开关 -->
        <button
          class="flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs transition hover:bg-[var(--gosslan-hover)]"
          :class="autoRefresh ? 'text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)]'"
          :title="t('logs.autoRefresh')"
          :aria-label="t('logs.autoRefresh')"
          :aria-pressed="autoRefresh"
          @click="autoRefresh = !autoRefresh; syncTimer()"
        >
          <RefreshCw class="h-3.5 w-3.5" :class="autoRefresh ? 'animate-spin' : ''" />
          <span class="hidden sm:inline">{{ t("logs.autoRefresh") }}</span>
        </button>
        <button
          class="flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('logs.refresh')"
          :aria-label="t('logs.refresh')"
          @click="load"
        >
          <RefreshCw class="h-3.5 w-3.5" />
          <span class="hidden sm:inline">{{ t("logs.refresh") }}</span>
        </button>
        <button
          class="flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs transition hover:bg-[var(--gosslan-hover)]"
          :class="confirmClear ? 'text-[var(--gosslan-danger-ink)]' : 'text-[var(--gosslan-text-2)]'"
          :title="t('logs.clear')"
          :aria-label="t('logs.clear')"
          @click="onClear"
        >
          <Trash2 class="h-3.5 w-3.5" />
          <span class="hidden sm:inline">{{ confirmClear ? t("logs.clearConfirm") : t("logs.clear") }}</span>
        </button>
        <!-- 读屏播报：只播「用户动作的结果」（复制成功 / 清空待确认）。
             日志正文**刻意不加 aria-live**：它是持续追加的，实时区会把读屏刷屏。 -->
        <span class="sr-only" role="status" aria-live="polite">
          {{ copied ? t("logs.copied") : confirmClear ? t("logs.clearConfirm") : "" }}
        </span>
        <button
          class="flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs text-white transition"
          :class="copied ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-primary)] hover:bg-[var(--gosslan-primary-hover)]'"
          :title="t('logs.copy')"
          :aria-label="t('logs.copy')"
          @click="copyAll"
        >
          <Copy class="h-3.5 w-3.5" />
          <span>{{ copied ? t("logs.copied") : t("logs.copy") }}</span>
        </button>
      </div>
    </div>

    <!-- 过滤条：字面包含匹配，命中处高亮。
         单独一行而不是塞进工具栏 —— 工具栏已有 4 个按钮，移动端会被挤爆。 -->
    <div class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] px-3 py-1.5">
      <Search class="h-3.5 w-3.5 shrink-0 text-[var(--gosslan-text-2)]" />
      <input
        v-model="filter"
        type="text"
        maxlength="200"
        enterkeyhint="search"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        class="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)]"
        :placeholder="t('logs.filterPlaceholder')"
        :aria-label="t('logs.filter')"
      />
      <span v-if="trimmedFilter" class="shrink-0 text-xs text-[var(--gosslan-text-2)]">
        {{ t("logs.filterCount", { n: rows.length, total: logs.length }) }}
      </span>
      <button
        v-if="trimmedFilter"
        class="flex h-6 w-6 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('logs.filterClear')"
        :aria-label="t('logs.filterClear')"
        @click="filter = ''"
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>

    <!-- 日志列表 -->
    <div class="min-h-0 flex-1 overflow-y-auto px-3 py-2 font-mono text-[13px] leading-relaxed">
      <div v-if="logs.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("logs.empty") }}
      </div>
      <div v-else-if="rows.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("logs.filterEmpty") }}
      </div>
      <div v-else>
        <!-- `v-memo`：行内容全部来自这几个字符串（v-html 的 4 段 + 级别色），
             所以只要它们没变就跳过 patch。两个收益：
             ① 打字/粘贴时（过滤值走延迟镜像，这些字符串没变）每行零 patch；
             ② 日志每 2 秒轮询刷新时，未变动的行也不重新写 innerHTML。
             改这一行时注意：**新增任何渲染字段都要加进依赖数组**，否则该字段不刷新。 -->
        <div
          v-for="r in rows"
          :key="r.key"
          v-memo="[r.time, r.levelText, r.target, r.message, r.level]"
          class="flex gap-2 rounded px-1 py-0.5 hover:bg-[var(--gosslan-hover)]"
        >
          <span class="shrink-0 select-none text-[var(--gosslan-text-2)]" v-html="r.time"></span>
          <span class="w-12 shrink-0 select-none font-semibold" :class="levelClass(r.level)" v-html="r.levelText"></span>
          <span class="min-w-0 break-all">
            <span class="text-[var(--gosslan-text-2)]" v-html="r.target"></span>
            <span v-html="r.message"></span>
          </span>
        </div>
      </div>
    </div>
  </div>
</template>
