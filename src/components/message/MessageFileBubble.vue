<script setup lang="ts">
import { t } from "@/i18n";
import { computed, type Component, type CSSProperties } from "vue";
import type { FileMeta } from "@/types";
import { humanSize, rgba } from "@/utils/color";
import { useAppStore } from "@/stores/useAppStore";
import {
  Download,
  FileArchive,
  FileAudio,
  FileCode,
  FileImage,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Loader2,
  X,
} from "lucide-vue-next";

const props = defineProps<{
  meta: FileMeta;
  bubbleStyle: CSSProperties;
  /** 0~1；null 表示不显示进度条（历史消息 / 已完成）。 */
  progress: number | null;
  statusText: string | null;
  failed: boolean;
  note: string | null;
  /** 群文件（gfile-）专用：按成员聚合的投递状态（单聊为 null 不显示）。 */
  delivery?: { completed: number; failed: number; waiting: number } | null;
  /** 自己的消息气泡尖角朝右、对方朝左，指向头像。 */
  mine: boolean;
  /** 文件已就绪（本地路径可用）**且文件确实还在**：整卡可点击打开（微信式：下载完成后点消息即打开）。 */
  ready: boolean;
  /** 本地文件已被「存储清理」删除。此时不提供"下载"入口——重新获取需要对方重发，
   *  而不是等对方上线自动补传（那个入口的文案会误导）。 */
  missing?: boolean;
}>();
const emit = defineEmits<{
  (e: "open"): void;
  (e: "save"): void;
  /** 尚未就绪时点「下载」：接收是自动的，父组件负责解释（提示等待上线）。 */
  (e: "download"): void;
}>();
const app = useAppStore();

type FileKind = "sheet" | "image" | "archive" | "video" | "audio" | "code" | "pdf" | "doc";

/** 文件类型配色：亮色取压得住白卡片的深调，暗色取能在深色卡片上跳出来的亮调。
 *  图标属图形而非文字，与卡片底色对比 ≥ 3 即达标（实测 3.9~5.2）。 */
const KIND_COLORS: Record<FileKind, { light: string; dark: string }> = {
  sheet: { light: "#16a34a", dark: "#4ade80" },
  image: { light: "#a855f7", dark: "#c084fc" },
  archive: { light: "#d97706", dark: "#fbbf24" },
  video: { light: "#db2777", dark: "#f472b6" },
  audio: { light: "#0891b2", dark: "#22d3ee" },
  code: { light: "#2563eb", dark: "#60a5fa" },
  pdf: { light: "#dc2626", dark: "#f87171" },
  doc: { light: "#475569", dark: "#94a3b8" },
};
const KIND_ICONS: Record<FileKind, Component> = {
  sheet: FileSpreadsheet,
  image: FileImage,
  archive: FileArchive,
  video: FileVideo,
  audio: FileAudio,
  code: FileCode,
  pdf: FileText,
  doc: FileText,
};

/** 文件类型：一眼看出是什么文件，不再所有附件都套同一个通用文档图标。 */
const ext = computed(() => (props.meta.name.split(".").pop() ?? "").toLowerCase());
const kind = computed<FileKind>(() => {
  switch (ext.value) {
    case "xls":
    case "xlsx":
    case "csv":
    case "numbers":
      return "sheet";
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "bmp":
    case "svg":
    case "avif":
    case "heic":
      return "image";
    case "zip":
    case "rar":
    case "7z":
    case "tar":
    case "gz":
    case "bz2":
    case "xz":
    case "dmg":
    case "iso":
      return "archive";
    case "mp4":
    case "mov":
    case "avi":
    case "mkv":
    case "webm":
    case "flv":
    case "wmv":
      return "video";
    case "mp3":
    case "wav":
    case "flac":
    case "m4a":
    case "ogg":
    case "aac":
      return "audio";
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
    case "rs":
    case "py":
    case "go":
    case "java":
    case "c":
    case "h":
    case "cpp":
    case "json":
    case "html":
    case "css":
    case "scss":
    case "vue":
    case "sh":
    case "bash":
    case "toml":
    case "yml":
    case "yaml":
    case "xml":
      return "code";
    case "pdf":
      return "pdf";
    default:
      return "doc";
  }
});
const icon = computed(() => KIND_ICONS[kind.value]);
/** 强调色：失败态统一走红，其余按类型取明/暗档。 */
const accent = computed(() => {
  const c = props.failed ? KIND_COLORS.pdf : KIND_COLORS[kind.value];
  return app.dark ? c.dark : c.light;
});
/** 图标井：底色＝强调色 12% 的超淡调，图标＝强调色实色（lucide 取 currentColor）。 */
const iconWellStyle = computed(() => ({
  backgroundColor: rgba(accent.value, 0.12),
  color: accent.value,
}));
const extLabel = computed(() => (ext.value ? ext.value.toUpperCase().slice(0, 4) : t("common.file")));
/** 传输中按钮上的百分比（微信下载按钮同款：下载中显示进度数字）。 */
const pct = computed(() => Math.round((props.progress ?? 0) * 100));
</script>

<template>
  <div
    class="flex min-w-0 w-[264px] max-w-full flex-col gap-2 rounded-[var(--gosslan-bubble-radius)] px-3 py-2.5"
    :class="ready ? 'cursor-pointer' : ''"
    :style="bubbleStyle"
    :title="ready ? t('msg.clickToOpen') : undefined"
    @click="ready && emit('open')"
  >
    <div class="flex items-center gap-2.5">
      <div
        class="flex h-10 w-10 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)]"
        :style="iconWellStyle"
      >
        <component :is="icon" v-if="!failed" class="h-5 w-5" />
        <X v-else class="h-5 w-5" />
      </div>
      <div class="min-w-0 flex-1">
        <div class="line-clamp-2 break-all text-[13px] font-medium leading-snug">{{ meta.name }}</div>
        <div v-if="failed" class="mt-0.5 text-[11px] text-[var(--gosslan-danger-ink)]">{{ t("msg.sendFailed") }}</div>
        <div v-else class="mt-0.5 flex items-center gap-1 text-[11px] opacity-70">
          <span>{{ extLabel }}</span>
          <span>·</span>
          <span>{{ humanSize(meta.size) }}</span>
          <template v-if="note">
            <span>·</span>
            <span class="truncate" :title="note">{{ note }}</span>
          </template>
        </div>
      </div>
      <!-- 微信式按钮逻辑：未就绪才显示——接收中＝转圈+百分比；等待传输＝下载按钮。
           就绪后整卡可点打开，另存/转发/复制走右键菜单，不再放常驻按钮。
           已被清理时不显示下载入口：文件不会"等对方上线"自己回来。 -->
      <div v-if="!failed && !ready && !missing" class="flex shrink-0 items-center">
        <span
          v-if="progress !== null"
          class="flex h-7 items-center gap-1 text-[11px] tabular-nums opacity-70"
          :title="t('msg.receiving')"
        >
          <Loader2 class="h-3.5 w-3.5 animate-spin" />
          {{ pct }}%
        </span>
        <button
          v-else
          class="tap-safe flex h-7 w-7 items-center justify-center rounded-[var(--gosslan-radius-sm)] transition hover:bg-black/10 dark:hover:bg-white/15"
          :title="t('msg.downloadFile')" :aria-label="t('msg.downloadFile')"
          @click="emit('download')"
        >
          <Download class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
    <!-- 群文件成员投递状态：独立成底栏，用一条极淡分隔线与文件信息区隔开（微信式卡片层次） -->
    <div
      v-if="delivery"
      class="flex items-center gap-1.5 border-t border-current/10 pt-1.5 text-[11px] opacity-70"
    >
      <span>{{ t("msg.sentTo", { n: delivery.completed }) }}</span>
      <span v-if="delivery.waiting > 0">· {{ t("msg.waitingOnline", { n: delivery.waiting }) }}</span>
      <span v-if="delivery.failed > 0" class="text-[var(--gosslan-danger-ink)]">· {{ t("msg.failedCount", { n: delivery.failed }) }}</span>
    </div>
    <!-- 传输进度条（发送/接收中实时显示，完成后消失）。卡片是中性色，进度条用主题色做强调 -->
    <template v-if="progress !== null">
      <div class="h-1 overflow-hidden rounded-full bg-black/10 dark:bg-white/10">
        <div
          class="h-full rounded-full bg-primary transition-all duration-200"
          :style="{ width: `${Math.round(progress * 100)}%` }"
        ></div>
      </div>
      <div class="text-[11px] opacity-70">{{ statusText }}</div>
    </template>
    <span aria-hidden="true" class="bubble-tail" :class="mine ? 'tail-mine' : 'tail-other'"></span>
  </div>
</template>
