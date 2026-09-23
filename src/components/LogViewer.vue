<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { Copy, RefreshCw, Search, Trash2, X } from "lucide-vue-next";
import TitleBar from "@/components/TitleBar.vue";
import MobilePageFrame from "@/components/MobilePageFrame.vue";
import { api } from "@/api";
import { t } from "@/i18n";
import { highlightText } from "@/utils/highlight";
import { useDeferredRef } from "@/composables/useDeferredRef";
import { useBackLayer } from "@/composables/useBackLayer";
import { LOG_LEVEL_TEXT, filterLogLines } from "@/utils/logFilter";
import type { LogEntry } from "@/types";

/**
 * 运行日志查看页。两种形态：
 * - `standalone`（桌面独立窗口）：自己的文档 + 入口，外壳是共用的自绘标题栏
 *   （`TitleBar`，功能名 = 运行日志），关闭走 close_log_window。
 * - 移动端页面：作为全屏覆盖层渲染，返回按钮 emit('back')。
 *
 * 不用 `AuxWindowShell` 的原因：同一个组件要兼两种形态，动态根 class + `TitleBar` 比
 * 套壳再改一遍结构更稳（工具栏里的筛选/刷新/清空等操作两边共用）。
 */
const props = defineProps<{ standalone?: boolean }>();
const emit = defineEmits<{ (e: "back"): void }>();

/**
 * 移动端全屏日志页参与分层返回：系统返回键回上一步，而不是退出应用。
 * `standalone`（桌面独立日志窗口）不参与 —— 那个窗口由标题栏的关闭键承担关闭语义。
 */
useBackLayer(
  () => !props.standalone,
  () => emit("back"),
);

/** 时间正序（旧 → 新）的原始日志；展示与复制都从它派生。 */
const logs = ref<LogEntry[]>([]);
/** 展示用：最新在上（倒序），打开即见最近的错误。 */
const displayLogs = computed(() => [...logs.value].reverse());
const autoRefresh = ref(true);
const copied = ref(false);

/** 时间窗口过滤：null = 全部；30 / 60 / 120 = 近 N 秒。 */
const timeWindow = ref<number | null>(null);
/** 合并连续完全相同的行（默认开启：BLE 扫描、announce 等每 3s 刷一轮的信息噪音巨大）。 */
const mergeDup = ref(true);
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

/** 合并连续**完全相同**的行。判据只看原始字符串（不看 v-html 高亮标记），
 *  所以相同内容即使被高亮也会折叠。关掉 mergeDup 时直接透传 rows。 */
const mergedRows = computed(() => {
  if (!mergeDup.value) return rows.value.map((r) => ({ ...r, count: 1 }));
  const out: (typeof rows.value[number] & { count: number })[] = [];
  for (const r of rows.value) {
    const last = out[out.length - 1];
    if (
      last &&
      last.level === r.level &&
      last.target.replace(/<[^>]+>/g, "") === r.target.replace(/<[^>]+>/g, "") &&
      last.message.replace(/<[^>]+>/g, "") === r.message.replace(/<[^>]+>/g, "")
    ) {
      last.count += 1;
    } else {
      out.push({ ...r, count: 1 });
    }
  }
  return out;
});

async function load() {
  // 窗口被隐藏/最小化时不必每 2s 拉一次：`get_logs` 会快照整份日志（几千条时是可观的
  // 克隆 + 序列化开销），而用户根本看不到。重新可见时下面的 visibilitychange 会立刻补一次。
  if (typeof document !== "undefined" && document.hidden) return;
  try {
    logs.value = await api.getLogs(timeWindow.value);
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
  // 窗口重新可见时立刻补一次（否则要等下一个 2s 周期，看到的是过期日志）
  document.addEventListener("visibilitychange", onVisibility);
});
function onVisibility() {
  if (!document.hidden) void load();
}
onUnmounted(() => {
  window.clearTimeout(copyTimer);
  window.clearTimeout(confirmTimer);
  document.removeEventListener("visibilitychange", onVisibility);
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
    window.clearTimeout(copyTimer);
    copyTimer = window.setTimeout(() => (copied.value = false), 2000);
  } catch {
    /* 复制失败静默：不打断用户 */
  }
}

/** 「已复制」与"再点一次确认清空"的自动复位定时器（卸载时要清，见 onUnmounted）。 */
let copyTimer = 0;
let confirmTimer = 0;

function onClear() {
  if (!confirmClear.value) {
    confirmClear.value = true;
    window.clearTimeout(confirmTimer);
    confirmTimer = window.setTimeout(() => (confirmClear.value = false), 3000);
    return;
  }
  confirmClear.value = false;
  void api.clearLogs().then(load).catch(() => {});
}

/**
 * 返回（**仅移动端整页形态**）。
 * 桌面独立窗口的关闭由共用标题栏的关闭键承担（→ `window_close` → 该窗口 `close()`）。
 */
function close() {
  emit("back");
}

/** 级别 → 文字色（仅 level 列彩色，message 不继承） */
const levelTextClass = (lv: string) =>
  lv === "error"
    ? "text-[var(--gosslan-danger-ink)]"
    : lv === "warn"
      ? "text-[var(--gosslan-warning-ink)]"
      : "text-[var(--gosslan-text-2)]";

/** 级别 → 整行浅底色（hover 时叠加，error=浅红、warn=浅黄、其他=透明） */
const rowLevelBg = (lv: string) =>
  lv === "error"
    ? "bg-[var(--gosslan-danger-soft)]/60"
    : lv === "warn"
      ? "bg-[var(--gosslan-warning-soft)]/60"
      : "";
</script>

<template>
  <!-- 桌面独立窗口：自绘标题栏（功能名 + 最小化/关闭）承载关闭语义，工具栏含全部操作。 -->
  <div
    v-if="standalone"
    class="flex h-screen flex-col overflow-hidden bg-[var(--gosslan-app-bg)] font-gosslan text-[var(--gosslan-text)] ring-1 ring-inset ring-[var(--gosslan-window-ring)]"
  >
    <TitleBar
      :title="t('logs.title')"
      :show-maximize="false"
      :close-to-tray="false"
    />

    <!-- 顶部工具栏：过滤/刷新/清空等操作。standalone 不再重复返回键（标题栏的关闭键承担语义）。 -->
    <div
      class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] px-3"
      :class="'bg-[var(--gosslan-app-bg)]'"
      :style="{ height: 'var(--gosslan-header-h)' }"
    >
      <div class="flex min-w-0 items-baseline gap-2">
        <span class="truncate text-[15px] font-medium" :title="t('logs.title')">{{ t("logs.title") }}</span>
        <span class="shrink-0 text-xs text-[var(--gosslan-text-2)]">
          {{ t("logs.count", { n: logs.length }) }}
        </span>
      </div>

      <div class="ml-auto flex items-center gap-1">
        <!-- 自动刷新开关 -->
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs transition hover:bg-[var(--gosslan-hover)]"
          :class="autoRefresh ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-text-2)]'"
          :title="t('logs.autoRefresh')"
          :aria-label="t('logs.autoRefresh')"
          :aria-pressed="autoRefresh"
          @click="autoRefresh = !autoRefresh; syncTimer()"
        >
          <RefreshCw class="h-3.5 w-3.5" :class="autoRefresh ? 'animate-spin' : ''" />
          <span class="hidden sm:inline">{{ t("logs.autoRefresh") }}</span>
        </button>
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('logs.refresh')"
          :aria-label="t('logs.refresh')"
          @click="load"
        >
          <RefreshCw class="h-3.5 w-3.5" />
          <span class="hidden sm:inline">{{ t("logs.refresh") }}</span>
        </button>
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs transition hover:bg-[var(--gosslan-hover)]"
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
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs text-white transition"
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

    <!-- 时间窗口过滤：最近 N 秒 / 全部。
         一排小按钮，移动端也能点。切换后自动触发 load。 -->
    <div
      class="flex shrink-0 items-center gap-1 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-caption)] px-3 py-1"
    >
      <span class="shrink-0 text-[11px] text-[var(--gosslan-text-2)]">窗口</span>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
        :class="timeWindow === null ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        @click="timeWindow = null; void load()"
      >全部</button>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
        :class="timeWindow === 30 ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        @click="timeWindow = 30; void load()"
      >30s</button>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
        :class="timeWindow === 60 ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        @click="timeWindow = 60; void load()"
      >1 分钟</button>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
        :class="timeWindow === 120 ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        @click="timeWindow = 120; void load()"
      >2 分钟</button>
      <span class="mx-1 h-3 w-px shrink-0 bg-[var(--gosslan-divider)]"></span>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
        :class="mergeDup ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        :title="mergeDup ? '展开相同日志' : '合并相同日志'"
        @click="mergeDup = !mergeDup"
      >合并重复</button>
    </div>

    <!-- 过滤条：字面包含匹配，命中处高亮。 -->
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
        class="min-w-0 flex-1 rounded-[var(--gosslan-radius-sm)] border border-transparent bg-transparent text-[13px] placeholder:text-[var(--gosslan-text-2)] outline-none transition focus:border-transparent"
        :placeholder="t('logs.filterPlaceholder')"
        :aria-label="t('logs.filter')"
      />
      <span v-if="trimmedFilter" class="shrink-0 text-xs text-[var(--gosslan-text-2)]">
        {{ t("logs.filterCount", { n: mergedRows.length, total: logs.length }) }}
      </span>
      <button
        v-if="trimmedFilter"
        class="tap-safe flex h-6 w-6 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
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
      <div v-else-if="mergedRows.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("logs.filterEmpty") }}
      </div>
      <div v-else>
        <!-- `v-memo`：行内容全部来自这几个字符串（v-html 的 4 段 + 级别色），
             所以只要它们没变就跳过 patch。两个收益：
             ① 打字/粘贴时（过滤值走延迟镜像，这些字符串没变）每行零 patch；
             ② 日志每 2 秒轮询刷新时，未变动的行也不重新写 innerHTML。
             改这一行时注意：**新增任何渲染字段都要加进依赖数组**，否则该字段不刷新。 -->
        <div
          v-for="r in mergedRows"
          :key="r.key"
          v-memo="[r.time, r.levelText, r.target, r.message, r.level, r.count]"
          class="flex gap-2 rounded px-1 py-0.5 transition"
          :class="[rowLevelBg(r.level), 'hover:bg-[var(--gosslan-hover)]']"
        >
          <span class="shrink-0 select-none text-[var(--gosslan-text-2)]" v-html="r.time"></span>
          <span class="w-12 shrink-0 select-none font-semibold" :class="levelTextClass(r.level)" v-html="r.levelText"></span>
          <span class="min-w-0 flex-1 break-all">
            <span class="text-[var(--gosslan-text-2)]" v-html="r.target"></span>
            <span v-html="r.message"></span>
          </span>
          <span
            v-if="r.count > 1"
            class="ml-1 shrink-0 text-[11px] font-medium text-[var(--gosslan-text-2)]"
            :title="`已合并 ${r.count} 条相同日志`"
          >
            ×{{ r.count }}
          </span>
        </div>
      </div>
    </div>
  </div>

  <!-- 移动端整页：MobilePageFrame 统一头部（返回 + 标题）；工具栏操作进 #actions，
       日志列表自管滚动（frame 的 scroll=false）。 -->
  <Transition v-else name="page-slide">
    <MobilePageFrame mode="overlay" :scroll="false" :title="t('logs.title')" @back="close">
      <template #actions>
        <span class="shrink-0 text-xs text-[var(--gosslan-text-2)]">
          {{ t("logs.count", { n: logs.length }) }}
        </span>
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs transition hover:bg-[var(--gosslan-hover)]"
          :class="autoRefresh ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-text-2)]'"
          :title="t('logs.autoRefresh')"
          :aria-label="t('logs.autoRefresh')"
          :aria-pressed="autoRefresh"
          @click="autoRefresh = !autoRefresh; syncTimer()"
        >
          <RefreshCw class="h-3.5 w-3.5" :class="autoRefresh ? 'animate-spin' : ''" />
          <span class="hidden sm:inline">{{ t("logs.autoRefresh") }}</span>
        </button>
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('logs.refresh')"
          :aria-label="t('logs.refresh')"
          @click="load"
        >
          <RefreshCw class="h-3.5 w-3.5" />
          <span class="hidden sm:inline">{{ t("logs.refresh") }}</span>
        </button>
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs transition hover:bg-[var(--gosslan-hover)]"
          :class="confirmClear ? 'text-[var(--gosslan-danger-ink)]' : 'text-[var(--gosslan-text-2)]'"
          :title="t('logs.clear')"
          :aria-label="t('logs.clear')"
          @click="onClear"
        >
          <Trash2 class="h-3.5 w-3.5" />
          <span class="hidden sm:inline">{{ confirmClear ? t("logs.clearConfirm") : t("logs.clear") }}</span>
        </button>
        <span class="sr-only" role="status" aria-live="polite">
          {{ copied ? t("logs.copied") : confirmClear ? t("logs.clearConfirm") : "" }}
        </span>
        <button
          class="tap-safe flex h-8 items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2 text-xs text-white transition"
          :class="copied ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-primary)] hover:bg-[var(--gosslan-primary-hover)]'"
          :title="t('logs.copy')"
          :aria-label="t('logs.copy')"
          @click="copyAll"
        >
          <Copy class="h-3.5 w-3.5" />
          <span>{{ copied ? t("logs.copied") : t("logs.copy") }}</span>
        </button>
      </template>

      <div
        class="flex shrink-0 items-center gap-1 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-caption)] px-3 py-1"
      >
        <span class="shrink-0 text-[11px] text-[var(--gosslan-text-2)]">窗口</span>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
          :class="timeWindow === null ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          @click="timeWindow = null; void load()"
        >全部</button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
          :class="timeWindow === 30 ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          @click="timeWindow = 30; void load()"
        >30s</button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
          :class="timeWindow === 60 ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          @click="timeWindow = 60; void load()"
        >1 分钟</button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
          :class="timeWindow === 120 ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          @click="timeWindow = 120; void load()"
        >2 分钟</button>
        <span class="mx-1 h-3 w-px shrink-0 bg-[var(--gosslan-divider)]"></span>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-sm)] px-2 py-0.5 text-[11px] transition"
          :class="mergeDup ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          :title="mergeDup ? '展开相同日志' : '合并相同日志'"
          @click="mergeDup = !mergeDup"
        >合并重复</button>
      </div>

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
          class="min-w-0 flex-1 rounded-[var(--gosslan-radius-sm)] border border-transparent bg-transparent text-[13px] placeholder:text-[var(--gosslan-text-2)] outline-none transition focus:border-transparent"
          :placeholder="t('logs.filterPlaceholder')"
          :aria-label="t('logs.filter')"
        />
        <span v-if="trimmedFilter" class="shrink-0 text-xs text-[var(--gosslan-text-2)]">
          {{ t("logs.filterCount", { n: mergedRows.length, total: logs.length }) }}
        </span>
        <button
          v-if="trimmedFilter"
          class="tap-safe flex h-6 w-6 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('logs.filterClear')"
          :aria-label="t('logs.filterClear')"
          @click="filter = ''"
        >
          <X class="h-3.5 w-3.5" />
        </button>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto px-3 py-2 font-mono text-[13px] leading-relaxed">
        <div v-if="logs.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
          {{ t("logs.empty") }}
        </div>
        <div v-else-if="mergedRows.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
          {{ t("logs.filterEmpty") }}
        </div>
        <div v-else>
          <div
            v-for="r in mergedRows"
            :key="r.key"
            v-memo="[r.time, r.levelText, r.target, r.message, r.level, r.count]"
            class="flex gap-2 rounded px-1 py-0.5 transition"
            :class="[rowLevelBg(r.level), 'hover:bg-[var(--gosslan-hover)]']"
          >
            <span class="shrink-0 select-none text-[var(--gosslan-text-2)]" v-html="r.time"></span>
            <span class="w-12 shrink-0 select-none font-semibold" :class="levelTextClass(r.level)" v-html="r.levelText"></span>
            <span class="min-w-0 flex-1 break-all">
              <span class="text-[var(--gosslan-text-2)]" v-html="r.target"></span>
              <span v-html="r.message"></span>
            </span>
            <span
              v-if="r.count > 1"
              class="ml-1 shrink-0 text-[11px] font-medium text-[var(--gosslan-text-2)]"
              :title="`已合并 ${r.count} 条相同日志`"
            >
              ×{{ r.count }}
            </span>
          </div>
        </div>
      </div>
    </MobilePageFrame>
  </Transition>
</template>
