<script setup lang="ts">
import { t } from "@/i18n";
import { computed, type CSSProperties } from "vue";
import type { FileMeta } from "@/types";
import { humanSize, rgba } from "@/utils/color";
import { FILE_KIND_COLORS, FILE_KIND_ICONS, fileExt, fileKindOf } from "@/utils/fileKind";
import { useAppStore } from "@/stores/useAppStore";
import { Download, Loader2, RotateCw, X } from "lucide-vue-next";

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
  /** 是否允许"点一下打开/另存"。Android 上自己发的文件为 false：点击应无任何响应，
   *  但长按气泡弹出的菜单不受影响（仍可另存/复制）。缺省 true（旧调用点行为不变）。 */
  tappable?: boolean;
  /** 本地文件已被「存储清理」删除。此时不提供"下载"入口——重新获取需要对方重发，
   *  而不是等对方上线自动补传（那个入口的文案会误导）。 */
  missing?: boolean;
  /** 统一内容状态为「未完成 / 校验失败」：按钮改成「重新获取」（按 cid 拉一份）。 */
  contentRetry?: boolean;
}>();
const emit = defineEmits<{
  (e: "open"): void;
  (e: "save"): void;
  /** 尚未就绪时点「下载」：接收是自动的，父组件负责解释（提示等待上线）。 */
  (e: "download"): void;
  /** 未完成 / 校验失败：按 cid 向对端重新拉取（ADR-0019）。 */
  (e: "refetch"): void;
}>();
const app = useAppStore();

const ext = computed(() => fileExt(props.meta.name));
const kind = computed(() => fileKindOf(props.meta.name));
const icon = computed(() => FILE_KIND_ICONS[kind.value]);
/** 强调色：失败态统一走红，其余按类型取明/暗档。 */
const accent = computed(() => {
  const c = props.failed ? FILE_KIND_COLORS.pdf : FILE_KIND_COLORS[kind.value];
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
/** 整卡是否可点：就绪 + 允许点。Android 自己发的文件 tappable=false ⇒ 鼠标手型/title/键盘都不给。 */
const canOpen = computed(() => props.ready && props.tappable !== false);
</script>

<template>
  <div
    class="flex min-w-0 w-[264px] max-w-full flex-col gap-2 rounded-[var(--gosslan-bubble-radius)] px-3 py-2.5"
    :class="canOpen ? 'cursor-pointer' : ''"
    :style="bubbleStyle"
    :title="canOpen ? t('msg.clickToOpen') : undefined"
    :role="canOpen ? 'button' : undefined"
    :tabindex="canOpen ? 0 : undefined"
    @click="canOpen && emit('open')"
    @keydown.enter.prevent="canOpen && emit('open')"
    @keydown.space.prevent="canOpen && emit('open')"
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
          class="tap-safe flex h-7 w-7 items-center justify-center rounded-[var(--gosslan-radius-sm)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t(contentRetry ? 'msg.imageReRequest' : 'msg.downloadFile')"
          :aria-label="t(contentRetry ? 'msg.imageReRequest' : 'msg.downloadFile')"
          @click="contentRetry ? emit('refetch') : emit('download')"
        >
          <RotateCw v-if="contentRetry" class="h-3.5 w-3.5" />
          <Download v-else class="h-3.5 w-3.5" />
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
      <div class="h-1 overflow-hidden rounded-full bg-[var(--gosslan-divider)]">
        <div
          class="h-full rounded-full bg-[var(--gosslan-primary)] transition-all duration-200"
          :style="{ width: `${Math.round(progress * 100)}%` }"
        ></div>
      </div>
      <div class="text-[11px] opacity-70">{{ statusText }}</div>
    </template>
    <span aria-hidden="true" class="bubble-tail" :class="mine ? 'tail-mine' : 'tail-other'"></span>
  </div>
</template>
