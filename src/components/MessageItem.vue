<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useClipboard } from "@/composables/useClipboard";
import { useExclusivePopup } from "@/composables/useExclusivePopup";
import { useMessageDisplay } from "@/composables/useMessageDisplay";
import { useMessageFile } from "@/composables/useMessageFile";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { textNeedsClamp } from "@/utils/previewMetrics";
import { t } from "@/i18n";
import MessageAvatar from "@/components/message/MessageAvatar.vue";
import MessageTextBubble from "@/components/message/MessageTextBubble.vue";
import MessageCodeBubble from "@/components/message/MessageCodeBubble.vue";
import MessageFileBubble from "@/components/message/MessageFileBubble.vue";
import MessageImageBubble from "@/components/message/MessageImageBubble.vue";
import MessageReceipt from "@/components/message/MessageReceipt.vue";
import MessageContentModal from "@/components/message/MessageContentModal.vue";
import MessageContextMenu from "@/components/message/MessageContextMenu.vue";
import ActionSheet from "@/components/ActionSheet.vue";
import { Copy, CornerUpLeft, Save, Share2, ImageOff } from "lucide-vue-next";
import type { MessageRecord, MsgKind } from "@/types";

const props = withDefaults(
  defineProps<{
    message: MessageRecord;
    /** 上一条消息（同会话），用于时间分割线判定 */
    prev?: MessageRecord | null;
    /** 群聊：显示发送者昵称 */
    isGroup?: boolean;
    /** 在本条消息上方显示未读分割线 */
    showUnreadDivider?: boolean;
    /** 群聊发送者昵称（由父组件解析） */
    senderName?: string;
    /** 已读该群消息的成员 ID（由父组件按消息时间计算） */
    groupReaderIds?: string[];
    /** 正在闪烁定位的消息键（点击引用块跳转时高亮 1.6s） */
    highlightId?: string | number | null;
    /** 群成员名列表：文本气泡据此高亮 @提及（单聊不传） */
    mentionNames?: string[];
  }>(),
  {
    prev: null,
    isGroup: false,
    showUnreadDivider: false,
    senderName: "",
    groupReaderIds: () => [],
    highlightId: null,
    mentionNames: () => [],
  },
);

const app = useAppStore();
const chat = useChatStore();
const { memberProfile } = useMemberProfile();

// 群文件（gfile-）投递摘要：成员状态语义（已发送给 N 人 · M 人待上线）。
// status 变化（含 CompleteAck 推进）时自动刷新。
const deliverySummary = ref<{ completed: number; failed: number; waiting: number } | null>(null);
const isGroupFile = computed(() => props.message.msg_id.startsWith("gfile-"));
watch(
  [() => props.message.msg_id, () => props.message.status],
  async ([msgId]: [string, unknown]) => {
    if (!msgId.startsWith("gfile-")) {
      deliverySummary.value = null;
      return;
    }
    try {
      deliverySummary.value = await invoke("get_group_file_delivery_summary", {
        transferId: msgId.slice(6),
      });
    } catch {
      deliverySummary.value = null;
    }
  },
  { immediate: true },
);

const display = useMessageDisplay({
  message: () => props.message,
  prev: () => props.prev,
  isGroup: () => props.isGroup,
});
const {
  mine,
  bubbleStyle,
  cardStyle,
  showTimeDivider,
  showNickname,
  fullTime,
  timeDividerText,
  sendState,
  receiptTitle,
} = display;

const {
  streamCode,
  streamCodeClamped,
  fileMeta,
  fileReady,
  fileProgress,
  fileStatusText,
  attachmentUrl,
  attachmentMissing,
  previewNote,
  openFile,
  saveAs,
} = useMessageFile(() => props.message, () => sendState.value);
const { copiedKey, copyContent } = useClipboard();

/**
 * 复制文本并**给出反馈**（右键菜单与移动端操作面板用）。
 * 这两处点完菜单立刻关闭，气泡上的"已复制"勾不会渲染（它只在长文本操作条里），
 * 所以必须用 toast 告知结果 —— 否则用户以为没复制上，会反复长按。
 * 气泡内联的复制按钮仍只靠自身勾选反馈（就在指尖，无需 toast 打扰）。
 */
async function copyTextWithToast(key: string, text: string) {
  const ok = await copyContent(key, text);
  app.toast(ok ? t("common.copied") : t("msg.copyFail"), ok ? "success" : "error");
}

/** 头像取色名：必须与列表/回执/弹层同源（昵称），否则同一人两处颜色分叉。
 *  群聊由父组件传 nicknameOf 结果；单聊在此兜一把，防止退化成按设备 ID 哈希。 */
const avatarName = computed(() =>
  mine.value
    ? app.device?.nickname || ""
    : props.senderName || chat.nicknameOf(props.message.sender_id) || props.message.sender_id,
);
/**
 * 头像：自己取本机；对端从好友/在线节点表取（peer 改资料后由 syncProfileFromPeers
 * 同步到 friends / peers，这里读到的就是最新头像）。没有头像就走 MessageAvatar 的
 * nameToColor 默认块——同一名字在单聊/群聊/消息列表永远同色。
 */
const avatarSrc = computed(() => {
  if (mine.value) return app.device?.avatar ?? null;
  return memberProfile(props.message.sender_id).avatar;
});

/** 是否为被点击引用所定位的消息（短暂高亮） */
const highlighted = computed(
  () => props.highlightId != null && props.highlightId === (props.message.msg_id ?? props.message.id),
);

/** 长文本判定与 estimateHeight 共用 textNeedsClamp：字号档位变了两边一起变。 */
const isLongText = computed(
  () =>
    props.message.kind === "text" &&
    textNeedsClamp(props.message.content, app.chatStyle.fontSize),
);

/** 全文弹窗：文本与代码共用一个 Modal，DOM 在消息之外，不参与 VirtualList 排布。 */
const fullModalOpen = ref(false);
const fullModalKind = ref<"text" | "code">("text");
const fullModalContent = ref("");
function openFullModal(kind: "text" | "code", content: string) {
  fullModalKind.value = kind;
  fullModalContent.value = content;
  fullModalOpen.value = true;
}

/** 图片查看器（相册式，由 ChatWindow 统一承载）：点图只上抛 msg_id，定位到该图。 */
function openImageLightbox() {
  emit("open-image", props.message.msg_id);
}

// ---------------- 消息右键菜单：复制 / 保存图片 / 引用 / 转发 ----------------
// 展开态参与全局浮层互斥：右键另一条消息（或好友菜单）时，本菜单自动收起。
// 关键在于右键只触发 contextmenu、不触发 click，靠 document click 关闭靠不住。
const ctxMenuPopup = useExclusivePopup(`menu:${props.message.msg_id ?? props.message.id}`);
const ctxMenu = ref<{ x: number; y: number } | null>(null);

watch(ctxMenuPopup.isActive, (mine) => {
  if (!mine && ctxMenu.value) ctxMenu.value = null;
});

function openContextMenu(e: MouseEvent) {
  if (props.message.kind === "system") return;
  // 移动端没有右键：长按走底部 ActionSheet。部分 WebView 在长按之后仍会补发
  // `contextmenu`（也会在长按选中文字时弹系统菜单），若这里再弹一次，就会出现
  // 「右键菜单 + ActionSheet」同时挂在屏幕上，而两者 claim 的是**同一个**互斥 key
  // （`menu:${msg_id}`）⇒ 谁也无法通过互斥关掉对方。
  if (app.isMobile) return;
  ctxMenu.value = { x: e.clientX, y: e.clientY };
  ctxMenuPopup.claim();
}

function closeContextMenu() {
  ctxMenuPopup.release();
  ctxMenu.value = null;
}

// ---------------- 移动端长按 → 底部 Action Sheet（HIG：触屏用长按唤出上下文操作） ----------------
// 桌面端走右键菜单（MessageContextMenu），移动端没有右键，用长按唤出底部操作面板。
const sheetOpen = ref(false);
let longPressTimer: ReturnType<typeof setTimeout> | null = null;

function openActionSheet() {
  if (props.message.kind === "system") return;
  sheetOpen.value = true;
  ctxMenuPopup.claim();
}

function closeActionSheet() {
  ctxMenuPopup.release();
  sheetOpen.value = false;
}

function onTouchStart(e: TouchEvent) {
  if (!app.isMobile || props.message.kind === "system") return;
  // 正文气泡里要能**原生选字/复制链接**（style.css 的约定：消息正文区不套 user-select:none）。
  // 整行无差别起长按定时器会把这套手势劫持掉：手指按住不动超过 500ms 就弹出操作面板，
  // 选区随之中断。所以命中可选文本气泡时不启动长按（要整条复制走气泡外侧的长按）。
  const el = e.target as HTMLElement | null;
  if (el?.closest(".gosslan-selectable")) return;
  longPressTimer = setTimeout(() => {
    longPressTimer = null;
    openActionSheet();
  }, 500);
}

function cancelLongPress() {
  if (longPressTimer) {
    clearTimeout(longPressTimer);
    longPressTimer = null;
  }
}

onBeforeUnmount(() => cancelLongPress());

/** 转发支持：与 MessageContextMenu 同一判据。 */
function forwardable(k: MsgKind) {
  return k === "text" || k === "code" || k === "image" || k === "file";
}

/** 图片可预览 URL：新格式走 objectURL（JSON 元数据），旧格式兼容 content=dataURL。 */
const imageDataUrl = computed(() => {
  if (props.message.kind === "image") {
    if (attachmentUrl.value) return attachmentUrl.value;
    // 旧格式兼容（开发阶段遗留的 data URL）
    if (props.message.content.startsWith("data:")) return props.message.content;
    return "";
  }
  if (props.message.kind === "file" && attachmentUrl.value) return attachmentUrl.value;
  return "";
});

async function copyImage() {
  closeContextMenu();
  const url = imageDataUrl.value;
  if (!url) return;
  try {
    const blob = await (await fetch(url)).blob();
    const png = blob.type === "image/png" ? blob : await toPngBlob(url);
    await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
    app.toast(t("msg.imageCopied"), "success");
  } catch {
    app.toast(t("msg.copyImageFail"), "error");
  }
}

/** 非 PNG 源转 PNG：经 canvas 重绘。 */
async function toPngBlob(url: string): Promise<Blob> {
  const img = new Image();
  img.src = url;
  await img.decode();
  const canvas = document.createElement("canvas");
  canvas.width = img.naturalWidth;
  canvas.height = img.naturalHeight;
  canvas.getContext("2d")!.drawImage(img, 0, 0);
  return await new Promise<Blob>((resolve) => canvas.toBlob((b) => resolve(b!), "image/png"));
}

async function saveImage() {
  closeContextMenu();
  const url = imageDataUrl.value;
  if (!url) return;
  try {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { invoke } = await import("@tauri-apps/api/core");
    const destination = await save({ defaultPath: `${t("common.image")}-${Date.now()}.png` });
    if (!destination) return; // 用户取消
    const buf = new Uint8Array(await (await fetch(url)).arrayBuffer());
    let binary = "";
    const chunk = 0x8000;
    for (let i = 0; i < buf.length; i += chunk) {
      binary += String.fromCharCode(...buf.subarray(i, i + chunk));
    }
    await invoke("save_data_file", { base64Data: btoa(binary), destination });
    app.toast(t("msg.imageSaved"), "success");
  } catch (e) {
    app.toastError(e, t("msg.saveImageFail"));
  }
}

/** 引用片段：文本取前 40 字，其它类型用占位标签。 */
function quoteSnippet(kind: MsgKind, content: string): string {
  if (kind === "image") return t("msg.image");
  if (kind === "code") return t("msg.code");
  if (kind === "file") return t("msg.file");
  const oneLine = content.replace(/\s+/g, " ").trim();
  return oneLine.length > 40 ? `${oneLine.slice(0, 40)}…` : oneLine;
}

const emit = defineEmits<{
  (e: "quote", payload: { sender: string; snippet: string; msgId: string | number }): void;
  (e: "forward", payload: { kind: MsgKind; content: string; snippet: string; filePath?: string }): void;
  (e: "locate", msgId: string): void;
  (e: "open-image", msgId: string): void;
}>();

function doQuote() {
  closeContextMenu();
  const msg = props.message;
  emit("quote", {
    sender: mine.value ? app.device?.nickname || t("common.me") : props.senderName || chat.nicknameOf(msg.sender_id),
    snippet: quoteSnippet(msg.kind, msg.content),
    msgId: msg.msg_id ?? msg.id,
  });
}

function doForward() {
  const msg = props.message;
  const payload = {
    kind: msg.kind as MsgKind,
    content: msg.content,
    snippet: quoteSnippet(msg.kind, msg.content),
    // 文件转发按本地路径重走传输链路（内容里的 JSON 只是元信息）
    filePath: msg.kind === "file" ? (fileMeta.value?.path ?? "") : undefined,
  };
  closeContextMenu();
  emit("forward", payload);
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

// ---------------- 文件消息：微信式交互（整卡打开 / 右键保存·转发·复制） ----------------

/** 未就绪时点「下载」：接收是自动的（对方设备上线即传输），这里只解释状态。 */
function onFileDownload() {
  closeContextMenu();
  app.toast(t("msg.fileWillReceive"), "info");
}

/** 右键「保存」：另存为（复制本地已就绪的文件到用户选择的位置）。 */
function saveFileTo() {
  closeContextMenu();
  void saveAs();
}

/** 右键「复制文件」：文件本体写系统剪贴板（CF_HDROP），
 *  可在资源管理器粘贴出文件，也可直接粘贴回聊天框发送（微信式）。 */
async function copyFileToClipboard() {
  closeContextMenu();
  const path = fileMeta.value?.path;
  if (!path) {
    app.toast(t("msg.fileNotSynced"), "info");
    return;
  }
  try {
    await invoke("copy_file_to_clipboard", { path });
    app.toast(t("msg.fileCopied"), "success");
  } catch (e) {
    app.toastError(e, t("msg.copyFail"));
  }
}
</script>

<template>
  <div class="py-1.5" :class="highlighted ? 'rounded-[var(--gosslan-radius-md)] bg-primary/5 ring-1 ring-primary/25' : ''">
    <!-- 时间分割线（间隔 ≥ 5 分钟）：居中浅灰小字 -->
    <div v-if="showTimeDivider" class="py-2 text-center text-[11px] text-[var(--gosslan-text-2)]">
      {{ timeDividerText }}
    </div>

    <!-- 未读分割线（打开会话时定位的第一条未读上方） -->
    <div v-if="showUnreadDivider" class="my-1.5 flex items-center gap-2 px-3">
      <div class="h-px flex-1 bg-primary/30"></div>
      <span class="rounded-full bg-primary-light px-2 py-0.5 text-[11px] text-[var(--gosslan-accent-ink)]">{{ t("msg.unreadDivider") }}</span>
      <div class="h-px flex-1 bg-primary/30"></div>
    </div>

    <div class="flex gap-2 px-4" :class="mine ? 'flex-row-reverse' : ''">
      <!-- 头像：每条消息独立完整渲染 -->
      <MessageAvatar :name="avatarName" :avatar="avatarSrc" />

      <div class="flex min-w-0 max-w-[72%] flex-col" :class="mine ? 'items-end' : 'items-start'">
        <!-- 群聊发送者昵称。
             视觉对齐：外层 flex 没有 items-*，所以**头像顶边 = 本行行盒顶边**。
             原先 `mb-0.5 text-[11px]`（行高 1.5 → 16.5px）会把墨迹往下推半行距约 3.7px，
             看起来名字比头像"低一点"（中英文都一样，实测 3.5px）。
             改成 `leading-none`（行盒 = 11px）+ `mb-[7px]`：墨迹贴到行盒顶（≈0.5px），
             与头像顶边齐平；且 **11 + 7 = 18px 与原来的 16.5 + 2 = 18.5 基本一致**，
             正好等于 messageHeight.NICKNAME_ROW(18)，虚拟列表估算不受影响。 -->
        <div
          v-if="showNickname"
          class="mb-[7px] px-1 text-[11px] leading-none text-[var(--gosslan-text-2)]"
        >
          {{ senderName || chat.nicknameOf(message.sender_id) }}
        </div>

        <!-- 系统消息 -->
        <div
          v-if="message.kind === 'system'"
          class="w-full text-center text-xs text-[var(--gosslan-text-2)]"
        >
          {{ message.content }}
        </div>

        <!-- 消息行：气泡 + 侧挂回执（mine 时回执在气泡左侧）；右键（桌面）/长按（移动端）弹消息菜单 -->
        <div
          v-else
          class="group/row flex w-full items-end gap-1.5"
          :class="mine ? 'justify-end' : 'justify-start'"
          :title="fullTime"
          @contextmenu.prevent="openContextMenu"
          @touchstart="onTouchStart"
          @touchend="cancelLongPress"
          @touchmove="cancelLongPress"
          @touchcancel="cancelLongPress"
        >
          <MessageReceipt
            v-if="mine"
            :state="sendState"
            :title="receiptTitle"
            :is-group="isGroup"
            :reader-ids="groupReaderIds"
            :msg-key="message.msg_id ?? message.id"
            @retry="retrySend"
          />

          <!-- 文本 -->
          <MessageTextBubble
            v-if="message.kind === 'text'"
            :content="message.content"
            :bubble-style="bubbleStyle"
            :clamped="isLongText"
            :copied="copiedKey === 'text'"
            :mine="mine"
            :mention-names="mentionNames"
            @expand="openFullModal('text', $event)"
            @copy="copyContent('text', $event)"
            @locate="emit('locate', $event)"
          />

          <!-- 代码：inline code 消息与代码附件同一套预览；超过 5 行裁断，全文进 Modal -->
          <MessageCodeBubble
            v-else-if="streamCode !== null"
            :code="streamCode"
            :clamped="streamCodeClamped"
            :copied="copiedKey === 'code'"
            :dark="app.dark"
            :mine="mine"
            @expand="openFullModal('code', $event)"
            @copy="copyContent('code', $event)"
          />

          <!-- 图片：已被存储清理时给出明确占位，而不是一个永远转圈/裂开的图片框。
               尺寸与 MessageImageBubble 的骨架一致（h-32 w-52），避免清理前后高度跳变。
               ⚠️ 必须是 v-else-if：这里若写成 v-if 会**切断上面的 v-if/v-else-if 链**，
               使这条新链末尾的 <div v-else> 变成"对所有 text / code 消息都成立的兜底"——
               于是每条文本消息都被渲染两遍（MessageTextBubble 一遍 + 原始文字一遍，
               表现为表情显示成 [摊手] 原文、普通消息整条重复）。历史缺陷见 fd02f62。 -->
          <div
            v-else-if="message.kind === 'image' && attachmentMissing"
            class="flex h-32 w-52 flex-col items-center justify-center gap-1 rounded-[var(--gosslan-bubble-radius)] bg-black/5 text-[11px] text-[var(--gosslan-text-2)] dark:bg-white/5"
          >
            <ImageOff class="h-6 w-6 opacity-50" />
            <span>{{ t("msg.imageCleaned") }}</span>
            <span class="opacity-70">{{ t("msg.imageReRequest") }}</span>
          </div>

          <!-- 图片 -->
          <MessageImageBubble
            v-else-if="message.kind === 'image'"
            :src="imageDataUrl"
            @open="openImageLightbox"
          />

          <!-- 附件图片预览（file + subtype:image，接收完成后显示本地图片） -->
          <MessageImageBubble
            v-else-if="message.kind === 'file' && attachmentUrl"
            :src="attachmentUrl"
            @open="openImageLightbox"
          />

          <!-- 文件 -->
          <MessageFileBubble
            v-else-if="message.kind === 'file' && fileMeta"
            :meta="fileMeta"
            :bubble-style="cardStyle"
            :progress="fileProgress"
            :status-text="fileStatusText"
            :failed="sendState === 'failed'"
            :note="previewNote"
            :missing="attachmentMissing"
            :delivery="isGroupFile ? deliverySummary : null"
            :mine="mine"
            :ready="fileReady"
            @open="openFile"
            @save="saveAs"
            @download="onFileDownload"
          />

          <!-- 未知 kind 的兜底气泡：排版必须与 MessageTextBubble 一致（py-1.5 / leading-normal），
               否则虚拟列表按 `previewMetrics.TEXT_BUBBLE_PADDING` 估的高度会对不上。 -->
          <div v-else class="px-3 py-1.5 text-sm leading-normal" :style="bubbleStyle">
            {{ message.content }}
          </div>
        </div>
      </div>
    </div>
  </div>

  <MessageContentModal
    :open="fullModalOpen"
    :kind="fullModalKind"
    :content="fullModalContent"
    :mention-names="mentionNames"
    :copied="copiedKey === 'full'"
    @close="fullModalOpen = false"
    @copy="copyContent('full', $event)"
  />

  <!-- 消息右键菜单 -->
  <MessageContextMenu
    v-if="ctxMenu"
    :x="ctxMenu.x"
    :y="ctxMenu.y"
    :kind="message.kind"
    @close="closeContextMenu()"
    @copy-text="closeContextMenu(); copyTextWithToast(message.kind === 'code' ? 'code' : 'text', message.content)"
    @copy-image="copyImage"
    @save-image="saveImage"
    @save-file="saveFileTo"
    @copy-file="copyFileToClipboard"
    @quote="doQuote"
    @forward="doForward"
  />

  <!-- 移动端长按 → 底部操作面板（Action Sheet）。操作与右键菜单同源，只是展示形态不同。 -->
  <ActionSheet :open="sheetOpen" @close="closeActionSheet()">
    <div class="flex flex-col">
      <button
        v-if="message.kind === 'text' || message.kind === 'code'"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="closeActionSheet(); copyTextWithToast(message.kind === 'code' ? 'code' : 'text', message.content)"
      >
        <Copy class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("common.copy") }}
      </button>
      <template v-if="message.kind === 'image'">
        <button
          class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
          @click="copyImage"
        >
          <Copy class="h-5 w-5 text-[var(--gosslan-text-2)]" />
          {{ t("common.copyImage") }}
        </button>
        <button
          class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
          @click="saveImage"
        >
          <Save class="h-5 w-5 text-[var(--gosslan-text-2)]" />
          {{ t("common.saveImage") }}
        </button>
      </template>
      <template v-if="message.kind === 'file'">
        <button
          class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
          @click="saveFileTo"
        >
          <Save class="h-5 w-5 text-[var(--gosslan-text-2)]" />
          {{ t("common.save") }}
        </button>
        <button
          class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
          @click="copyFileToClipboard"
        >
          <Copy class="h-5 w-5 text-[var(--gosslan-text-2)]" />
          {{ t("common.copyFile") }}
        </button>
      </template>
      <button
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="doQuote"
      >
        <CornerUpLeft class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("common.quote") }}
      </button>
      <button
        v-if="forwardable(message.kind)"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="doForward"
      >
        <Share2 class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("common.forward") }}
      </button>
    </div>
  </ActionSheet>
</template>
