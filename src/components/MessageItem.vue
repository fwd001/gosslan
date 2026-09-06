<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import dayjs from "dayjs";
import { loadFilePreview } from "@/utils/filePreview";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { save } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import CodeBlock from "@/components/CodeBlock.vue";
import BaseModal from "@/components/BaseModal.vue";
import { Check, Circle, Copy, Download, FileText, Loader2, RefreshCw, RotateCcw, X, ZoomIn, ZoomOut } from "lucide-vue-next";
import { humanSize } from "@/utils/color";
import { findPreset, parsePeerStyle } from "@/utils/chatStyle";
import type { MessageRecord } from "@/types";

const props = withDefaults(
  defineProps<{
    message: MessageRecord;
    /** 上一条消息（同会话），用于连续消息合并与时间分割线 */
    prev?: MessageRecord | null;
    /** 下一条消息（同会话），用于判断是否为分钟组末条 */
    next?: MessageRecord | null;
    /** 群聊：显示发送者昵称 */
    isGroup?: boolean;
    /** 在本条消息上方显示未读分割线 */
    showUnreadDivider?: boolean;
    /** 群聊发送者昵称（由父组件解析） */
    senderName?: string;
  }>(),
  { prev: null, next: null, isGroup: false, showUnreadDivider: false, senderName: "" },
);

const app = useAppStore();
const chat = useChatStore();
const mine = computed(() => props.message.sender_id === app.device?.device_id);
const copied = ref(false);

// ---------------- 显示样式（本机偏好 + 对端广播偏好） ----------------
/** 我发的消息用我的样式；对方发的消息优先用对方广播的样式（未同步过则回退本机）。 */
const preset = computed(() => {
  if (!mine.value) {
    const raw = app.peerStyles[props.message.sender_id];
    if (raw) return findPreset(parsePeerStyle(raw).preset);
  }
  return findPreset(app.chatStyle.preset);
});
const colors = computed(() =>
  app.dark ? preset.value.dark : preset.value.light,
);

/** 连续消息合并：同一发送者 5 分钟内的消息省略头像/昵称（紧凑模式可关）。 */
const sameSenderRun = computed(() => {
  if (!app.chatStyle.compact) return false;
  const p = props.prev;
  if (!p || p.kind === "system" || p.sender_id !== props.message.sender_id) return false;
  return props.message.ts - p.ts < 5 * 60 * 1000;
});
/** 同一分钟内的连续消息：合并显示（省略时间行、气泡更紧凑），不依赖紧凑开关。
 *  时间属于时间轴，不属于发送者——不因 sender 变化而重复时间。 */
const sameMinuteRun = computed(() => {
  const p = props.prev;
  if (!p || p.kind === "system") return false;
  return dayjs(p.ts).isSame(props.message.ts, "minute");
});
/** 分钟组末条：下一条不在同一分钟，或是列表最后一条。 */
const isLastInMinute = computed(() => {
  const n = props.next;
  if (!n) return true; // 最后一条消息
  return !dayjs(n.ts).isSame(props.message.ts, "minute");
});
/** 紧凑布局：连续 run 或同分钟消息。 */
const tight = computed(() => sameSenderRun.value || sameMinuteRun.value);
/** 时间分割线：与上一条间隔 ≥ 5 分钟。 */
const showTimeDivider = computed(() => {
  const p = props.prev;
  return !p || props.message.ts - p.ts >= 5 * 60 * 1000;
});
/** 群聊非本人消息首条：显示昵称。 */
const showNickname = computed(() => props.isGroup && !mine.value && !sameSenderRun.value);

const time = computed(() => dayjs(props.message.ts).format("YYYY年MM月DD日 HH:mm"));
const fullTime = computed(() => dayjs(props.message.ts).format("YYYY-MM-DD HH:mm:ss"));
const timeDividerText = computed(() => dayjs(props.message.ts).format("YYYY-MM-DD HH:mm"));

// ---------------- 发送状态（sending / delivered / read / failed） ----------------
/** sending/sent=转圈（发出中或未确认送达）；delivered=空圆框（对方收到未读）；read=绿勾（已读）。 */
const sendState = computed(() => props.message.status as "sending" | "sent" | "delivered" | "read" | "failed");
const receiptTitle = computed(() => {
  switch (sendState.value) {
    case "sending":
    case "sent":
      return "发送中…";
    case "delivered":
      return "对方已收到，未读";
    case "read":
      return "对方已读";
    case "failed":
      return "发送失败";
    default:
      return "";
  }
});

// ---------------- 文件传输进度 ----------------
/** 文件消息的 msg_id 即 "file-{transfer_id}"，据此查传输记录。 */
const transferId = computed(() =>
  props.message.msg_id.startsWith("file-") ? props.message.msg_id.slice(5) : null,
);
const transfer = computed(() =>
  transferId.value ? chat.transfers.find((t) => t.id === transferId.value) : null,
);

// ---------------- 文件 ----------------
interface FileMeta {
  name: string;
  path: string;
  size: number;
  /** 后端按扩展名分类的附件子类型；历史消息缺省按 file 处理。 */
  subtype: string;
}
/** 乐观上屏的文件气泡可能缺 size/path，用传输记录补齐。 */
const fileMeta = computed<FileMeta | null>(() => {
  if (props.message.kind !== "file") return null;
  try {
    const meta = JSON.parse(props.message.content) as Partial<FileMeta>;
    const t = transfer.value;
    return {
      name: meta.name ?? t?.name ?? "文件",
      path: meta.path ?? t?.path ?? "",
      size: meta.size ?? t?.size ?? 0,
      subtype: meta.subtype ?? "file",
    };
  } catch {
    return null;
  }
});

/** 进度 0~1；无记录（历史消息）返回 null 表示不显示进度条。 */
const fileProgress = computed(() => {
  if (!transfer.value) return null;
  const t = transfer.value;
  if (t.status === "done") return null; // 完成：不再显示条
  return t.progress;
});
const fileStatusText = computed(() => {
  const t = transfer.value;
  if (!t) return null;
  if (t.status === "done") return null;
  const pct = Math.round((t.progress ?? 0) * 100);
  return t.direction === "send" ? `发送中 ${pct}%` : `接收中 ${pct}%`;
});

// ---------------- 长文本折叠 ----------------
const LONG_TEXT_CHARS = 280;
const isLongText = computed(
  () => props.message.kind === "text" && props.message.content.length > LONG_TEXT_CHARS,
);
const collapsed = ref(true);

// 图片预览（已由 imageModalOpen/imageModalSrc 管理）
function openPreview() {
  openImageModal(props.message.content);
}

// ---------------- 附件预览（file + subtype = image | code） ----------------
/** 附件图片 objectURL / 附件代码文本 / 超限提示。全空 = 尚未就绪或失败 → 回退文件卡片。 */
const attachmentUrl = ref<string | null>(null);
const attachmentCode = ref<string | null>(null);
const previewNote = ref<string | null>(null);

/** 仅当 file 消息本地路径就绪（= 已完成接收 / 本机自选文件）、未失败、且为 image/code 时可预览。 */
const previewSubtype = computed<"image" | "code" | null>(() => {
  const meta = fileMeta.value;
  if (props.message.kind !== "file" || !meta || !meta.path) return null;
  if (sendState.value === "failed") return null;
  return meta.subtype === "image" || meta.subtype === "code" ? meta.subtype : null;
});

async function ensureAttachmentPreview() {
  const sub = previewSubtype.value;
  const meta = fileMeta.value;
  if (!sub || !meta) {
    attachmentUrl.value = null;
    attachmentCode.value = null;
    previewNote.value = null;
    return;
  }
  const r = await loadFilePreview(props.message.msg_id, sub, meta.name);
  // await 期间该气泡可能已不满足预览条件（切换/失败）→ 丢弃，避免贴到错误气泡。
  if (previewSubtype.value !== sub) return;
  attachmentUrl.value = r.url ?? null;
  attachmentCode.value = r.text ?? null;
  previewNote.value = r.note ?? null;
}

// 传输完成或乐观→真实替换后，path/subtype/name 变化会自动触发，无需仅在 mounted 读一次。
// 必须显式读 path/name：Vue 对 watcher 返回的数组做浅比对，若源里不包含这些字段
// 的读取，optimistic({path:""})→real({path:"/…",name:"x"}) 替换时 msg_id+subtype 不变
// → 数组值相等 → callback 不触发。
watch(
  () => [
    previewSubtype.value,
    props.message.msg_id,
    fileMeta.value?.path,
    fileMeta.value?.name,
  ] as const,
  () => void ensureAttachmentPreview(),
  { immediate: true },
);

// ---------------- 代码附件弹窗 ----------------
const codeModalOpen = ref(false);
const codeModalCode = ref("");

function openCodeModal() {
  codeModalCode.value = attachmentCode.value ?? "";
  codeModalOpen.value = true;
}
function closeCodeModal() {
  codeModalOpen.value = false;
}

// ---------------- 图片附件弹窗（含缩放） ----------------
const imageModalOpen = ref(false);
const imageModalSrc = ref("");
const imageZoom = ref(1);
const ZOOM_MIN = 0.25;
const ZOOM_MAX = 4;
const ZOOM_STEP = 0.25;

function openImageModal(src: string) {
  imageModalSrc.value = src;
  imageZoom.value = 1;
  imageModalOpen.value = true;
}
function closeImageModal() {
  imageModalOpen.value = false;
  imageZoom.value = 1;
}
function zoomIn() {
  imageZoom.value = Math.min(ZOOM_MAX, imageZoom.value + ZOOM_STEP);
}
function zoomOut() {
  imageZoom.value = Math.max(ZOOM_MIN, imageZoom.value - ZOOM_STEP);
}
function resetZoom() {
  imageZoom.value = 1;
}
function onImageWheel(e: WheelEvent) {
  if (!imageModalOpen.value) return;
  if (e.ctrlKey || e.metaKey) {
    e.preventDefault();
    const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
    imageZoom.value = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, imageZoom.value + delta));
  }
}

// 两个弹窗的 body scroll lock + Escape
function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  if (codeModalOpen.value) closeCodeModal();
  else if (imageModalOpen.value) closeImageModal();
}
watch([codeModalOpen, imageModalOpen], ([code, img]) => {
  const locked = code || img;
  document.body.classList.toggle("overflow-hidden", locked);
  if (locked) {
    window.addEventListener("keydown", onGlobalKeydown);
    window.addEventListener("wheel", onImageWheel, { passive: false });
  } else {
    window.removeEventListener("keydown", onGlobalKeydown);
    window.removeEventListener("wheel", onImageWheel);
  }
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onGlobalKeydown);
  window.removeEventListener("wheel", onImageWheel);
  document.body.classList.remove("overflow-hidden");
});

async function copyText() {
  try {
    await navigator.clipboard.writeText(props.message.content);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    /* ignore */
  }
}
async function openFile() {
  const path = fileMeta.value?.path;
  if (!path) {
    app.toast("文件路径不可用", "error");
    return;
  }
  try {
    await openPath(path);
  } catch (e) {
    app.toast(`打开文件失败：${e}`, "error");
  }
}
async function saveAs() {
  const source = fileMeta.value?.path;
  const filename = fileMeta.value?.name;
  if (!source || !filename) {
    app.toast("文件路径不可用", "error");
    return;
  }
  try {
    const destination = await save({ defaultPath: filename });
    if (!destination) return; // 用户取消
    await invoke("copy_file", { source, destination });
    app.toast("文件已保存", "success");
  } catch (e) {
    app.toast(`保存文件失败：${e}`, "error");
  }
}
async function retrySend() {
  const msg = props.message;
  if (msg.status !== "failed" || msg.kind === "file") return;
  try {
    await chat.send(msg.conv_id, msg.content, msg.kind);
  } catch {
    // 失败状态已由 send() 内部处理
  }
}
</script>

<template>
  <div :class="tight ? 'py-0.5' : 'pt-2 pb-2'">
    <!-- 时间分割线（间隔 ≥ 5 分钟） -->
    <div v-if="showTimeDivider" class="my-2 text-center text-[11px] text-[var(--gosslan-text-2)]">
      {{ timeDividerText }}
    </div>

    <!-- 未读分割线（打开会话时定位的第一条未读上方） -->
    <div v-if="showUnreadDivider" class="my-1.5 flex items-center gap-2 px-3">
      <div class="h-px flex-1 bg-primary/30"></div>
      <span class="rounded-full bg-primary-light px-2 py-0.5 text-[11px] text-primary">以下是未读消息</span>
      <div class="h-px flex-1 bg-primary/30"></div>
    </div>

    <div class="flex gap-2 px-3" :class="mine ? 'flex-row-reverse' : ''">
      <!-- 头像：连续消息合并时省略（保留占位对齐） -->
      <div v-if="!sameSenderRun" class="flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-full bg-primary text-white">
        <img
          v-if="mine && app.device?.avatar"
          :src="app.device.avatar"
          class="h-full w-full object-cover"
        />
        <span v-else class="text-xs font-semibold">
          {{ mine ? (app.device?.nickname.slice(0, 1) || "我") : (senderName || props.message.sender_id).slice(0, 1) }}
        </span>
      </div>
      <div v-else class="w-8 shrink-0"></div>

      <div class="flex min-w-0 max-w-[78%] flex-col" :class="mine ? 'items-end' : 'items-start'">
        <!-- 群聊发送者昵称 -->
        <div v-if="showNickname" class="mb-0.5 px-1 text-xs text-[var(--gosslan-text-2)]">
          {{ senderName || props.message.sender_id }}
        </div>

        <!-- 系统消息 -->
        <div
          v-if="message.kind === 'system'"
          class="w-full text-center text-xs text-[var(--gosslan-text-2)]"
        >
          {{ message.content }}
        </div>

        <!-- 消息行：气泡 + 侧挂回执（mine 时回执在气泡左侧） -->
        <div
          v-else
          class="group/row flex w-full items-end gap-1.5"
          :class="mine ? 'justify-end' : 'justify-start'"
        >
        <!-- mine 时回执固定在气泡左侧（视觉上贴近对话人头像方向） -->
        <span v-if="mine" class="shrink-0 pb-1.5" :title="receiptTitle">
          <Loader2 v-if="sendState === 'sending' || sendState === 'sent'" class="h-3.5 w-3.5 animate-spin text-[var(--gosslan-text-2)]" />
          <button v-else-if="sendState === 'failed'" class="flex h-5 w-5 items-center justify-center rounded text-red-500 transition hover:bg-red-500/10" title="重新发送" @click="retrySend">
            <RefreshCw class="h-3.5 w-3.5" />
          </button>
          <Circle v-else-if="sendState === 'delivered'" class="h-3.5 w-3.5 text-[var(--gosslan-text-2)]" />
          <Check v-else-if="sendState === 'read'" class="h-4 w-4 text-emerald-500" />
        </span>

        <!-- 文本 -->
        <div
          v-if="message.kind === 'text'"
          class="group relative min-w-0 rounded-2xl px-3 py-2 leading-relaxed shadow-sm"
          :style="{
            background: mine ? colors.mineBubble : colors.otherBubble,
            color: mine ? colors.mineText : colors.otherText,
            fontSize: 'var(--gosslan-msg-size, 14px)',
          }"
        >
          <div
            class="overflow-hidden whitespace-pre-wrap break-words"
            :class="isLongText && collapsed ? 'line-clamp-5' : ''"
            :style="{ wordBreak: 'break-word' }"
          >{{ message.content }}</div>
          <!-- 长文本折叠条：展开/收起 + 复制 -->
          <div v-if="isLongText" class="mt-1.5 flex items-center gap-2 border-t pt-1.5" :style="{ borderColor: 'rgba(128,128,128,0.2)' }">
            <button
              class="text-xs opacity-70 transition hover:opacity-100"
              @click="collapsed = !collapsed"
            >
              {{ collapsed ? "展开全文" : "收起" }}
            </button>
            <button
              class="flex items-center gap-1 whitespace-nowrap text-xs transition"
              :class="copied ? 'text-emerald-600 dark:text-emerald-400' : 'opacity-70 hover:opacity-100'"
              @click="copyText"
            >
              <Check v-if="copied" class="h-3 w-3" />
              <Copy v-else class="h-3 w-3" />
              {{ copied ? "已复制" : "复制" }}
            </button>
          </div>
          <!-- 普通文本悬停复制按钮 -->
          <button
            v-else
            class="absolute -top-3 right-1 z-10 hidden items-center gap-1 whitespace-nowrap rounded-md border px-1.5 py-0.5 text-xs shadow transition group-hover:flex"
            :class="copied
              ? 'border-emerald-300 bg-emerald-50 text-emerald-600 dark:bg-emerald-900/30 dark:text-emerald-400'
              : 'border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] text-[var(--gosslan-text-2)]'"
            @click="copyText"
          >
            <Check v-if="copied" class="h-3 w-3" />
            <Copy v-else class="h-3 w-3" />
            {{ copied ? "已复制" : "复制" }}
          </button>
        </div>

        <!-- 代码 -->
        <div v-else-if="message.kind === 'code'" class="min-w-0 flex-1">
          <CodeBlock :code="message.content" />
        </div>

        <!-- 图片 -->
        <div v-else-if="message.kind === 'image'" class="overflow-hidden rounded-xl cursor-pointer" @click="openPreview">
          <img :src="message.content" class="block max-h-72 max-w-full rounded-xl object-contain" />
        </div>

        <!-- 附件图片预览（file + subtype:image，接收完成后显示本地图片，点击→弹窗预览） -->
        <div v-else-if="message.kind === 'file' && attachmentUrl" class="overflow-hidden rounded-xl cursor-pointer" @click="openImageModal(attachmentUrl)">
          <img :src="attachmentUrl" class="block max-h-72 max-w-full rounded-xl object-contain" />
        </div>

        <!-- 附件代码预览（file + subtype:code，完成后读取本地文本交给 CodeBlock，最多5行+弹窗预览） -->
        <div v-else-if="message.kind === 'file' && attachmentCode !== null" class="min-w-0 flex-1">
          <CodeBlock :code="attachmentCode" :preview="true" :preview-lines="5" :preview-action="openCodeModal" />
        </div>

        <!-- 文件 -->
        <div
          v-else-if="message.kind === 'file' && fileMeta"
          class="flex min-w-0 flex-1 flex-col gap-2 rounded-xl px-3 py-2.5 shadow-sm"
          :style="{
            background: mine ? colors.mineBubble : colors.otherBubble,
            color: mine ? colors.mineText : colors.otherText,
          }"
        >
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg"
              :class="sendState === 'failed' ? 'bg-red-100 text-red-500 dark:bg-red-900/30' : 'bg-primary-light text-primary'"
            >
              <FileText v-if="sendState !== 'failed'" class="h-5 w-5" />
              <X v-else class="h-5 w-5" />
            </div>
            <div class="min-w-0 flex-1">
              <div class="truncate text-sm font-medium">{{ fileMeta.name }}</div>
              <div v-if="sendState === 'failed'" class="text-xs text-red-500">发送失败</div>
              <div v-else class="text-xs opacity-70">{{ humanSize(fileMeta.size) }}</div>
              <div v-if="previewNote" class="text-[11px] opacity-70">{{ previewNote }}</div>
            </div>
            <button
              v-if="sendState !== 'failed'"
              class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-primary transition hover:bg-[var(--gosslan-hover)]"
              title="打开文件"
              @click="openFile"
            >
              <FileText class="h-4 w-4" />
            </button>
            <button
              v-if="sendState !== 'failed'"
              class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-primary transition hover:bg-[var(--gosslan-hover)]"
              title="下载文件"
              @click="saveAs"
            >
              <Download class="h-4 w-4" />
            </button>
          </div>
          <!-- 传输进度条（发送/接收中实时显示，完成后消失） -->
          <template v-if="fileProgress !== null">
            <div class="h-1.5 overflow-hidden rounded-full bg-black/10 dark:bg-white/10">
              <div
                class="h-full rounded-full bg-primary transition-all duration-200"
                :style="{ width: `${Math.round(fileProgress * 100)}%` }"
              ></div>
            </div>
            <div class="text-[11px] opacity-70">{{ fileStatusText }}</div>
          </template>
        </div>

        <div v-else class="rounded-2xl px-3 py-2 text-sm shadow-sm" :style="{ background: colors.otherBubble, color: colors.otherText }">
          {{ message.content }}
        </div>
        </div>

        <!-- 时间行：仅分钟组末条显示时间，hover 可见秒级 -->
        <div
          v-if="isLastInMinute"
          class="mt-0.5 px-1 text-[11px] text-[var(--gosslan-text-2)]"
          :class="mine ? 'text-right' : ''"
        >
          <span class="group-hover/row:hidden">{{ time }}</span>
          <span class="hidden group-hover/row:inline">{{ fullTime }}</span>
        </div>
        <!-- tight 非末条：仅 hover 显示秒级 -->
        <div
          v-else-if="tight"
          class="mt-0.5 hidden px-1 text-[10px] text-[var(--gosslan-text-2)] group-hover/row:block"
          :class="mine ? 'text-right' : ''"
        >
          {{ fullTime }}
        </div>
      </div>
    </div>
  </div>

  <!-- 图片预览弹窗（含缩放控制） -->
  <BaseModal :open="imageModalOpen" width="max-w-5xl" @close="closeImageModal">
    <div class="relative flex flex-col items-center gap-3">
      <!-- 缩放工具栏 -->
      <div class="flex items-center gap-2">
        <button class="rounded-lg p-1.5 transition hover:bg-black/10 dark:hover:bg-white/10" title="缩小" @click="zoomOut">
          <ZoomOut class="h-4 w-4" />
        </button>
        <span class="min-w-[3.5rem] text-center text-xs tabular-nums">{{ Math.round(imageZoom * 100) }}%</span>
        <button class="rounded-lg p-1.5 transition hover:bg-black/10 dark:hover:bg-white/10" title="放大" @click="zoomIn">
          <ZoomIn class="h-4 w-4" />
        </button>
        <button class="rounded-lg p-1.5 transition hover:bg-black/10 dark:hover:bg-white/10" title="恢复默认" @click="resetZoom">
          <RotateCcw class="h-4 w-4" />
        </button>
      </div>
      <!-- 图片（Ctrl+滚轮缩放，点击不关闭由 BaseModal 处理背景点击） -->
      <div class="max-h-[65vh] overflow-auto">
        <img
          v-if="imageModalSrc"
          :src="imageModalSrc"
          class="block rounded shadow-2xl transition-transform duration-150 origin-center"
          :style="{ transform: `scale(${imageZoom})` }"
        />
      </div>
    </div>
  </BaseModal>

  <!-- 代码附件弹窗：完整代码 + 语法高亮/复制，关闭后聊天布局不变 -->
  <BaseModal :open="codeModalOpen" title="代码预览" width="max-w-4xl" @close="closeCodeModal">
    <div class="max-h-[70vh] overflow-y-auto">
      <CodeBlock v-if="codeModalCode" :code="codeModalCode" />
    </div>
  </BaseModal>
</template>
