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
import { isMultiSelectable, isTipKind } from "@/utils/messageKinds";
import { isSelfMessage } from "@/utils/selfChat";
import { stripQuoteMsgId } from "@/utils/quote";
import { isDialogCancelled, saveDestinationOf } from "@/utils/saveDestination";
import { haptic } from "@/utils/haptics";
import { shouldStartLongPress, shouldSwallowLongPressRelease } from "@/utils/longPress";
import { t } from "@/i18n";
import MessageAvatar from "@/components/message/MessageAvatar.vue";
import MessageTextBubble from "@/components/message/MessageTextBubble.vue";
import MessageCodeBubble from "@/components/message/MessageCodeBubble.vue";
import MergeCard from "@/components/message/MergeCard.vue";import MessageFileBubble from "@/components/message/MessageFileBubble.vue";
import MessageImageBubble from "@/components/message/MessageImageBubble.vue";
import MessageReceipt from "@/components/message/MessageReceipt.vue";
import BaseModal from "@/components/BaseModal.vue";
import MessageReactionBar from "@/components/message/MessageReactionBar.vue";
import type { ReactionChip } from "@/utils/reactions";
import MessageContentModal from "@/components/message/MessageContentModal.vue";
import TodoCardBubble from "@/components/TodoCardBubble.vue";
import MessageContextMenu from "@/components/message/MessageContextMenu.vue";
import ActionSheet from "@/components/ActionSheet.vue";
import { Check, Copy, CornerUpLeft, ImageOff, ListChecks, Pin, Save, Share2, Star, TextSelect, Undo2 } from "lucide-vue-next";
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
    /** 已折叠的表情回应（由会话层算好传入，避免每条消息各自 O(n) 重算）。 */
    reactions?: ReactionChip[];
    /** 该消息当前是否被置顶（决定菜单显示「置顶」还是「取消置顶」） */
    pinned?: boolean;
    /**
     * 多选模式（**会话级**状态，由 ChatWindow 持有）：点击本行 = 勾选/取消勾选，
     * 长按不弹菜单，气泡内的链接/图片/引用一律让路（用透明覆盖层吃掉点击）。
     */
    selectMode?: boolean;
    /** 多选模式下本行是否已选中（决定勾选框的实心态）。 */
    selected?: boolean;
  }>(),
  {
    prev: null,
    isGroup: false,
    showUnreadDivider: false,
    senderName: "",
    groupReaderIds: () => [],
    highlightId: null,
    mentionNames: () => [],
    selectMode: false,
    selected: false,
  },
);

const app = useAppStore();

/** 快捷回应表情：与 EmojiPicker 同一套「[名字]」token（后端按同一形态校验）。 */
const QUICK_REACTIONS = ["[赞]", "[微笑]", "[捂脸]", "[流泪]"];

/**
 * 「点击重取」：文件/图片没拿到（未完成 / 已被清理）时，请对端按 cid 再发一份。
 * 对方无需确认（拥有即授权）；对方版本不支持时给出明确提示，而不是静默。
 */
async function refetchContent() {
  const myId = app.device?.device_id;
  const peer =
    props.message.sender_id === myId ? props.message.receiver_id : props.message.sender_id;
  if (!peer) return;
  try {
    const ok = await invoke<boolean>("request_content", {
      peerId: peer,
      msgId: props.message.msg_id,
    });
    app.toast(t(ok ? "msg.refetchRequested" : "msg.refetchUnsupported"), ok ? "info" : "error");
  } catch (e) {
    app.toastError(e, t("msg.refetchFail"));
  }
}

/** 这条内容在统一状态里是否「未完成 / 校验失败」⇒ 文件卡片给「重新获取」。 */
const retryableContent = computed(() => {
  if (props.message.kind !== "file" && props.message.kind !== "image") return null;
  try {
    const sha = (JSON.parse(props.message.content) as { sha256?: string }).sha256;
    if (!sha) return null;
    const rec = chat.contentTransfers.find((c) => c.cid === sha);
    if (!rec) return null;
    return rec.status === "incomplete" || rec.status === "rejected" ? rec : null;
  } catch {
    return null;
  }
});
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
  fileTappable,
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
  const ok = await copyContent(key, copyOut(text));
  app.toast(ok ? t("common.copied") : t("msg.copyFail"), ok ? "success" : "error");
}

/**
 * 复制出去的文本（右键菜单 / 操作面板 / 气泡按钮 / 全文弹窗四条路径共用）。
 * 引用头里的 `|msg_id` 是内部路由信息，不该混进用户复制的内容 ——
 * 用户看到的是「引用 张三：…」，复制出来却是 `…|msg_ab12」`。
 * ⚠️ 只在复制路径上剥：转发要原样保留，接收方靠它跳到被引用的那条消息。
 */
function copyOut(content: string): string {
  return stripQuoteMsgId(content);
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

/**
 * 提示行（撤回 / 文件下载 / 群成员变更…）：微信式居中灰字，**不是一条消息** ——
 * 不带头像、不带气泡、不带昵称行，也没有任何菜单入口。
 *
 * 判定共用 `messageKinds.isTipKind`：`messageHeight` 的高度估算要按同一份清单算，
 * 两边各写一份的话，估算与实际渲染差一行就会让虚拟列表的两条消息互相遮挡。
 */
const isTip = computed(() => isTipKind(props.message.kind));

/** 自聊消息（收发双方都是本机）：不挂回执（见模板里的说明）。 */
const isSelfMsg = computed(() => isSelfMessage(props.message));

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
  if (isTip.value) return;
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
/**
 * 移动端「选择文字」模式（用户 2026-09-13）。
 *
 * 触屏下气泡**默认不可选**，长按一律弹消息菜单；"部分选字"是菜单里的一个二级入口
 * —— 这是微信 / Telegram / WhatsApp / iMessage 的通行模型，也是唯一能同时要"长按必出菜单"
 * 和"能选字"的做法（见 `style.css` 里 `@media (pointer: coarse)` 那段注释）。
 */
const textSelecting = ref(false);
let longPressTimer: ReturnType<typeof setTimeout> | null = null;
/** 长按起点：用来做"手指抖动容差"（见 `onTouchMove`）。 */
let longPressOrigin: { x: number; y: number } | null = null;
/**
 * 长按那根手指是否还按着（面板就是这次按压弹出来的）。
 *
 * 用途只有一个：**吞掉抬手指那一下**（见 `swallowLongPressRelease`）。
 * 放在 `touchstart` 置位、在 `touchend`/`touchcancel` 的**窗口捕获**处理里复位。
 */
let longPressHeld = false;
/** 长按判定时长：与 iOS/微信一致（500ms 是 HIG 的常用值）。 */
const LONG_PRESS_MS = 500;
/**
 * 手指抖动容差（px）。用户 2026-09-13：「长按触发的效果感觉不太灵」。
 * 旧实现把 `@touchmove` 直接接到 `cancelLongPress` —— **手指动 1px 就取消**，
 * 真机上几乎不可能"完全不动地按住 500ms"，于是长按十次九次不触发。
 * 现在只有移动超过容差（或明显是滑动/滚动）才取消。
 */
const LONG_PRESS_MOVE_TOLERANCE = 12;

function openActionSheet() {
  if (isTip.value) return;
  // 多选态下不弹操作面板：这一点是"勾选/取消"，弹面板会让用户分不清选上没有
  // （与 `shouldStartLongPress` 的 multiSelect 判据同一条口径，这里是第二道闸门）。
  if (props.selectMode) return;
  sheetOpen.value = true;
  ctxMenuPopup.claim();
}

function closeActionSheet() {
  ctxMenuPopup.release();
  sheetOpen.value = false;
}

/**
 * 吞掉"弹面板那一下"的抬手事件。
 *
 * 根因（用户 2026-09-13 Android 实测「弹出 sheet 之后一放手立马就缩回去了」）：
 * 面板是 HeadlessUI `Dialog`，它的 `useOutsideClick` 在 **document 捕获阶段** 挂了 `touchend`，
 * 判据是"`touchend` 的 target 在不在对话框容器里" —— 而 touch 事件的 target 在
 * **`touchstart` 那一刻就固定**成那条消息了，所以抬手必被判成"点了外面" ⇒ 立刻 `@close`。
 *
 * ⚠️ 监听**必须挂在 `window` 的捕获阶段**：HeadlessUI 挂的是 `document` 捕获，两者同阶段时
 * 按注册顺序执行（它先注册，我们一定排在后面）；而捕获路径是 `window → document → … → target`，
 * 只有挂 `window` 才抢得到它前面。它内部有 `if (e.defaultPrevented) return`，`preventDefault` 就够。
 * 顺带也杀掉了这次 tap 的合成 `click`（不会误触气泡里的链接）。
 */
function swallowLongPressRelease(e: TouchEvent) {
  if (!shouldSwallowLongPressRelease({ openedByHeldPress: longPressHeld, sheetOpen: sheetOpen.value })) {
    longPressHeld = false;
    return;
  }
  longPressHeld = false;
  e.preventDefault();
}

/** 面板展开期间才需要拦（平时一次监听都不挂，避免影响滚动/其它手势）。 */
watch(sheetOpen, (open) => {
  if (open) {
    window.addEventListener("touchend", swallowLongPressRelease, { capture: true, passive: false });
    window.addEventListener("touchcancel", swallowLongPressRelease, { capture: true });
  } else {
    window.removeEventListener("touchend", swallowLongPressRelease, { capture: true });
    window.removeEventListener("touchcancel", swallowLongPressRelease, { capture: true });
    longPressHeld = false;
  }
});

/** 进「选择文字」：关掉菜单 → 本条气泡开放原生选字（`MessageTextBubble` 会自动全选）。 */
function enterTextSelect() {
  closeActionSheet();
  textSelecting.value = true;
}

/**
 * 选区一消失就退出选择模式（用户点了别处、收起了系统手柄）。
 * 不退出的后果：这条气泡一直"可选择"，下次长按又弹不出菜单（典型的状态残留）。
 */
function onSelectingChanged() {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.toString().length === 0) textSelecting.value = false;
}
watch(textSelecting, (on) => {
  if (on) document.addEventListener("selectionchange", onSelectingChanged);
  else document.removeEventListener("selectionchange", onSelectingChanged);
});

function onTouchStart(e: TouchEvent) {
  cancelLongPress();
  const el = e.target as HTMLElement | null;
  // 判据全部收在 `utils/longPress.ts`（纯函数、有真值表单测）：
  // 桌面走右键 / 系统消息没有菜单 / 「选择文字」模式让给系统手柄 /
  // ⚠️ 触屏下**正文气泡里**的 `.gosslan-selectable` 不再让路 —— 它已经被
  // `@media (pointer: coarse)` 关掉选中，而那个类还在 DOM 上，之前因此导致
  // "按在文字上长按不弹、按到内边距才弹"（用户 2026-09-13 实测）。
  const canStart = shouldStartLongPress({
    isMobile: app.isMobile,
    isSystem: isTip.value,
    selectMode: textSelecting.value,
    multiSelect: props.selectMode,
    hitSelectable: !!el?.closest(".gosslan-selectable"),
    insideTextBubble: !!el?.closest(".gosslan-bubble-text"),
  });
  if (!canStart) return;
  const t0 = e.touches[0];
  if (!t0) return;
  longPressHeld = true;
  longPressOrigin = { x: t0.clientX, y: t0.clientY };
  longPressTimer = setTimeout(() => {
    longPressTimer = null;
    longPressOrigin = null;
    // 触觉反馈：长按"到点了"必须有一下明确的反馈，否则用户会以为没生效而反复长按
    // （`heavy` 的语义就是"长按菜单弹出"，见 utils/haptics.ts）
    haptic("heavy");
    openActionSheet();
  }, LONG_PRESS_MS);
}

/** 手指移动超过容差才取消长按（旧实现是"一动就取消"，真机上等于长按失灵）。 */
function onTouchMove(e: TouchEvent) {
  if (!longPressTimer || !longPressOrigin) return;
  const t0 = e.touches[0];
  if (!t0) return;
  const dx = t0.clientX - longPressOrigin.x;
  const dy = t0.clientY - longPressOrigin.y;
  if (Math.hypot(dx, dy) > LONG_PRESS_MOVE_TOLERANCE) cancelLongPress();
}

function cancelLongPress() {
  if (longPressTimer) {
    clearTimeout(longPressTimer);
    longPressTimer = null;
  }
  longPressOrigin = null;
}

onBeforeUnmount(() => {
  cancelLongPress();
  // 窗口级监听不随组件卸载自动消失（它挂在 window 上），必须自己摘
  window.removeEventListener("touchend", swallowLongPressRelease, { capture: true });
  window.removeEventListener("touchcancel", swallowLongPressRelease, { capture: true });
  // ⚠️ `selectionchange` 挂在 **document** 上，由 `watch(textSelecting)` 增删 ——
  // 那条 watch 只在开关时才跑，组件若**在选择模式下**被卸载（滚出虚拟列表就回收），
  // 它永远等不到 else 分支，监听会一直留在 document 上。每进入一次漏一个，
  // 之后在输入框/搜索框里选字，每个 selectionchange 都要白跑 N 次 getSelection()。
  document.removeEventListener("selectionchange", onSelectingChanged);
});

/** 转发支持：与 MessageContextMenu 同一判据。 */
function forwardable(k: MsgKind) {
  // 合并转发卡片本身也可以再转（微信允许"转发聊天记录"），它是自包含的内容。
  return k === "text" || k === "code" || k === "image" || k === "file" || k === "merge";
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
    const picked: unknown = await save({ defaultPath: `${t("common.image")}-${Date.now()}.png` });
    const destination = saveDestinationOf(picked);
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
    if (isDialogCancelled(e)) return; // Android 取消是 reject，不是返回 null
    app.toastError(e, t("msg.saveImageFail"));
  }
}

/** 引用片段：文本取前 40 字，其它类型用占位标签。 */
function quoteSnippet(kind: MsgKind, content: string): string {
  if (kind === "image") return t("msg.image");
  if (kind === "code") return t("msg.code");
  if (kind === "file") return t("msg.file");
  // 合并转发：引用它是"引用一张聊天记录卡片"，正文是 JSON，不能截进引用块
  if (kind === "merge") return t("merge.title");
  const oneLine = content.replace(/\s+/g, " ").trim();
  return oneLine.length > 40 ? `${oneLine.slice(0, 40)}…` : oneLine;
}

const emit = defineEmits<{
  (e: "quote", payload: { sender: string; snippet: string; msgId: string | number }): void;
  (e: "forward", payload: { kind: MsgKind; content: string; snippet: string; filePath?: string }): void;
  (e: "locate", msgId: string): void;
  /** 点了某个表情 chip（已点过则是取消） */
  (e: "react", emoji: string): void;
  /** 切换置顶（群聊） */
  (e: "pin"): void;
  /** 收藏这条消息（微信式：独立存储，删会话/清缓存都不影响） */
  (e: "favorite"): void;
  (e: "open-image", msgId: string): void;
  /** 点开合并转发卡片（上抛载荷 JSON，由 ChatWindow 统一渲染详情弹窗） */
  (e: "open-merge", payload: { content: string; senderId: string }): void;
  /** 进入多选模式（微信式批量操作） */
  (e: "multi-select"): void;
  /** 多选模式下切换本行的勾选态（由覆盖层点击触发） */
  (e: "toggle-select"): void;
  /** 点了任务卡片里的「查看任务」：打开群任务面板（由 ChatWindow 接住）。 */
  (e: "open-tasks"): void;
}>();

/**
 * 能否撤回：**只有自己发的、且未被撤回的**消息才给入口。
 *
 * 后端也只在 `sender_id == 自己` 时才接受撤回 —— 前端隐藏入口不是为了安全
 * （安全由签名保证），而是不让用户白点一次再收到报错。
 */
// ⚠️ 必须判 isGroup：单聊没有撤回（后端只实现了群撤回），
// 否则入口可见、点了确认后 `confirmRecall` 里静默 return —— 用户看到的是"什么都没发生"。
// ⚠️ `mine` 是从 composable 解构出来的 **ComputedRef**（本文件其它地方都写 `mine.value`）。
// 在模板里 Vue 自动解包，但在 script 的 computed 内部**不会** —— 裸写 `mine` 是个对象、
// 恒为真值，于是群聊里对**任何人的消息**都会显示「撤回」（用户真机反馈的那个 bug）。
const canRecall = computed(
  () => !!props.isGroup && mine.value && props.message.kind !== "recalled",
);

/** 文件消息正在发送中 —— 显示"取消发送"菜单项。
 * 条件：自己发的 + 文件类 + status 是 sending。
 * 群文件（gfile-）也支持取消 —— 后端 cancel_file_transfer 统一处理。 */
const canCancelSend = computed(() => {
  if (!isSelfMsg.value) return false;
  const mid = props.message.msg_id;
  const isFileMsg = mid.startsWith("file-") || mid.startsWith("gfile-");
  if (!isFileMsg) return false;
  return props.message.status === "sending";
});

function doCancelSend() {
  closeContextMenu();
  closeActionSheet();
  const mid = props.message.msg_id;
  let transferId = "";
  if (mid.startsWith("file-")) transferId = mid.slice(5);
  else if (mid.startsWith("gfile-")) transferId = mid.slice(6);
  if (!transferId) return;
  invoke<boolean>("cancel_file_transfer", { transferId }).then(
    (signalled) => {
      app.toast(
        signalled ? "已请求取消发送" : "标记为已取消（传输可能已结束）",
        "info",
      );
    },
    (e: unknown) => app.toastError(e, "取消发送失败"),
  );
}

/** 撤回前的二次确认：破坏性且不可逆（对方看到的是「消息已撤回」，收不回来）。 */
const confirmingRecall = ref(false);

function doRecall() {
  closeContextMenu();
  closeActionSheet();
  confirmingRecall.value = true;
}

async function confirmRecall() {
  confirmingRecall.value = false;
  const convId = props.message.conv_id;
  if (!convId.startsWith("group:")) return;
  try {
    await chat.recallMessage(convId.slice(6), props.message.msg_id);
    app.toast(t("msg.recallDone"), "success");
  } catch (e) {
    app.toastError(e, t("msg.recallFail"));
  }
}

function doQuote() {
  // ⚠️ 必须收起**底部面板**（不只是桌面右键菜单）：用户 2026-09-13 安卓实测
  // 「点『引用』之后那个 sheet 还挂在那儿」。引用会跳到输入框去操作，浮层留着就是挡路。
  // `ActionSheet` 面板层已经统一做了"点任何一项即收起"，这里是**第二道保险**：
  // 这两个动作是"跳到别处去操作"，即使以后面板的通用规则改了，也不该让 sheet 留在新界面上面。
  closeActionSheet();
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
  // 同上：转发会打开转发弹窗（跳转到界面内去操作），底部面板必须先收掉
  closeActionSheet();
  closeContextMenu();
  emit("forward", payload);
}

/**
 * 收藏这条消息。
 *
 * 只上抛事件、这里不直接调 api：**两端（桌面右键 / 移动长按面板）都要走同一处**，
 * 而 toast 与"已在收藏中"的提示逻辑在 `ChatWindow` 里 —— 与转发同一条路子
 * （见 doForward），避免同一个动作在桌面和移动上出现两套提示。
 * 同样必须先收掉浮层：收藏会弹 toast，浮层留着就是挡路。
 */
function doFavorite() {
  closeActionSheet();
  closeContextMenu();
  emit("favorite");
}

/**
 * 进入多选模式（微信式批量操作）。
 *
 * 与转发/收藏同一条路子：只上抛事件，状态与批量动作都归 `ChatWindow` 管 ——
 * 桌面右键菜单与移动端底部面板是两套独立模板，两处各存一份"进入多选"的状态必然漂移。
 */
function doMultiSelect() {
  closeActionSheet();
  closeContextMenu();
  emit("multi-select");
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
  <!-- group/msg：表情回应条是"消息行"的**兄弟节点**，不在 group/row 的作用域内 ——
       悬停揭示必须挂在这一层，否则 group-hover/msg 永远不触发（那个组名以前根本不存在）。 -->
  <div
    class="group/msg relative py-1.5"
    :class="[
      highlighted ? 'rounded-[var(--gosslan-radius-md)] bg-primary/5 ring-1 ring-primary/25' : '',
      selectMode && selected ? 'rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)]' : '',
    ]"
  >
    <!-- 多选态：透明覆盖层 + 左侧勾选框，两者都**绝对定位**，不进 flex 流。
         为什么必须这样：气泡宽度是 `max-w-[72%]`，勾选框若作为 flex 兄弟插进来会挤窄气泡、
         正文折行变多，而虚拟列表按 `previewMetrics.COLUMNS_PER_LINE` 估的高度不会跟着变
         ⇒ 相邻消息互相遮挡（`messageHeight` 文件头专门写过这个坑）。
         覆盖层的第二个作用：多选时气泡里的链接/图片/引用点击都不该响应，它一并吃掉。
         提示行（系统消息/已撤回）不可选，所以 `!isTip`。 -->
    <template v-if="selectMode && !isTip && isMultiSelectable(message.kind)">
      <button
        class="absolute inset-0 z-10"
        :aria-label="selected ? t('multi.deselect') : t('multi.select')"
        :aria-pressed="selected"
        @click="emit('toggle-select')"
      ></button>
      <!-- 勾选框（微信款）：20px 圆、未选 1px 细边、选中实底 + 粗白勾。
           刻意**不用** border-2：2px 的环在 18px 的圆里内孔只剩 14px，深色下是一圈
           又重又闷的「O」（用户 2026-09-17 反馈"太丑"）。微信的勾选圈之所以轻，
           靠的就是 1px 边 + 选中瞬间整个圆变实底，而不是靠加粗描边。
           填充色用 bg-primary（正牌 token）—— 之前写的 --gosslan-accent **并不存在**，
           var() 解析失败会让整条声明被丢弃（选中态变成无色圆 + 看不见的白勾）。
           位置**在左侧**（微信一比一：微信多选的勾选圈就在消息左侧的边槽里）。
           左边距 8px（`left-2`）：自己的消息那一行左边是空的，圈贴着面板边缘会显得局促。
           别人的行则由右侧的行内边距把头像整排让开（见下面 `pl-10`），圈独占一条干净边槽。 -->
      <span
        class="pointer-events-none absolute left-2 top-1/2 z-20 flex h-5 w-5 -translate-y-1/2 items-center justify-center rounded-full border transition"
        :class="selected
          ? 'border-primary bg-primary'
          : 'border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]'"
      >
        <Check v-if="selected" class="h-3 w-3 text-white" :stroke-width="3" aria-hidden="true" />
      </span>
    </template>
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

    <!-- 提示行（系统消息 / 已撤回）：微信式居中灰字，**通栏**、无头像、无气泡。
         之所以是"消息行"的**兄弟节点**而不是它内部的一支：放进消息行就会带上 36px 头像
         和 `max-w-[72%]` 的列宽，居中后仍偏向一侧、看着还是一条普通消息 ——
         那正是用户 2026-09-16 报的问题。 -->
    <div
      v-if="isTip"
      class="px-4 text-center text-xs text-[var(--gosslan-text-2)]"
    >
      {{ message.kind === "recalled" ? t("msg.recalled") : message.content }}
    </div>

    <!-- 多选态给左侧勾选圈让出边槽：**只有"别人的"那一行**需要整排右移。
         自己的行头像在右侧、气泡是右对齐的，左移不动它 —— 加了这个内边距只会白白挤窄
         自己的气泡（多一圈折行），换不来任何观感收益。
         别人的行 `pl-10`(40px) = 圈 left-2(8) + 圆 20 + 间隙 12，头像正好从圈右侧干净地起排。
         ⚠️ 这里只动横向内边距：高度估算用的 `COLUMNS_PER_LINE` 是**常量**、不随宽度变，
         所以不会破坏 VirtualList 的估算（横向挪动与"相邻消息互相遮挡"那个坑无关）。 -->
    <div
      v-else
      class="flex gap-2 px-4"
      :class="[mine ? 'flex-row-reverse' : '', selectMode && !mine ? 'pl-10' : '']"
    >
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

        <!-- 消息行：气泡 + 侧挂回执（mine 时回执在气泡左侧）；右键（桌面）/长按（移动端）弹消息菜单 -->
        <div
          class="group/row flex w-full items-end gap-1.5"
          :class="mine ? 'justify-end' : 'justify-start'"
          :title="fullTime"
          @contextmenu.prevent="openContextMenu"
          @touchstart="onTouchStart"
          @touchend="cancelLongPress"
          @touchmove="onTouchMove"
          @touchcancel="cancelLongPress"
        >
          <!-- 回执：自聊消息**没有回执** —— 收发双方都是本机，不存在"已送达/已读"这个过程，
               挂上去只会显示一个永远转圈的圈（后端给自聊消息的状态直接是 read，
               但"绿勾已读"出现在自己发给自己上同样没有意义）。见 utils/selfChat。 -->
          <MessageReceipt
            v-if="mine && !isSelfMsg"
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
            :select-mode="textSelecting"
            @expand="openFullModal('text', $event)"
            @copy="copyContent('text', copyOut($event))"
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
            @copy="copyContent('code', copyOut($event))"
          />

          <!-- 图片：已被存储清理时给出明确占位，而不是一个永远转圈/裂开的图片框。
               尺寸与 MessageImageBubble 的骨架一致（h-32 w-52），避免清理前后高度跳变。
               ⚠️ 必须是 v-else-if：这里若写成 v-if 会**切断上面的 v-if/v-else-if 链**，
               使这条新链末尾的 <div v-else> 变成"对所有 text / code 消息都成立的兜底"——
               于是每条文本消息都被渲染两遍（MessageTextBubble 一遍 + 原始文字一遍，
               表现为表情显示成 [摊手] 原文、普通消息整条重复）。历史缺陷见 fd02f62。 -->
          <button
            v-else-if="message.kind === 'image' && attachmentMissing"
            type="button"
            class="flex h-32 w-52 cursor-pointer flex-col items-center justify-center gap-1 rounded-[var(--gosslan-bubble-radius)] bg-black/5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-black/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary dark:bg-white/5 dark:hover:bg-white/10"
            @click="refetchContent"
          >
            <ImageOff class="h-6 w-6 opacity-50" />
            <span>{{ t("msg.imageCleaned") }}</span>
            <span class="opacity-70">{{ t("msg.imageReRequest") }}</span>
          </button>

          <!-- 图片 -->
          <MessageImageBubble
            v-else-if="message.kind === 'image'"
            :src="imageDataUrl"
            @open="openImageLightbox"
            @refetch="refetchContent"
          />

          <!-- 附件图片预览（file + subtype:image，接收完成后显示本地图片） -->
          <MessageImageBubble
            v-else-if="message.kind === 'file' && attachmentUrl"
            :src="attachmentUrl"
            @open="openImageLightbox"
            @refetch="refetchContent"
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
            :tappable="fileTappable"
            :content-retry="!!retryableContent"
            @open="openFile"
            @save="saveAs"
            @download="onFileDownload"
            @refetch="refetchContent"
          />

          <!-- 合并转发的聊天记录（微信式卡片）：定高 96px，与 `messageHeight.MERGE_CARD`
               的估算对齐（改卡片尺寸必须同时改那里，否则虚拟列表会遮挡相邻消息）。 -->
          <MergeCard
            v-else-if="message.kind === 'merge'"
            :content="message.content"
            :mine="mine"
            @open="emit('open-merge', { content: message.content, senderId: message.sender_id })"
          />

          <!-- 群任务卡片（`todo` = Card kind）：把 JSON 载荷渲染成可读卡片，替代原始 JSON 兜底 -->
          <TodoCardBubble
            v-else-if="message.kind === 'todo'"
            :message="message"
            :mine="mine"
            @open="emit('open-tasks')"
          />

          <!-- 未知 kind 的兜底气泡：排版必须与 MessageTextBubble 一致（py-1.5 / leading-normal），
               否则虚拟列表按 `previewMetrics.TEXT_BUBBLE_PADDING` 估的高度会对不上。 -->
          <div v-else class="select-text px-3 py-1.5 text-sm leading-normal" :style="bubbleStyle">
            {{ message.content }}
          </div>
        </div>
      </div>
    </div>
  </div>

  <!-- 表情回应条：挂在消息行**下方**（飞书/微信同款位置），与气泡同侧对齐。
       放在行内会被 `flex items-end` 摆到气泡右侧，语义不对。 -->
  <MessageReactionBar
    v-if="!isTip"
    :chips="reactions ?? []"
    :mine="mine"
    :interactive="!!isGroup"
    :quick="QUICK_REACTIONS"
    :class="mine ? 'self-end pr-1' : 'self-start pl-1'"
    @toggle="emit('react', $event)"
  />

  <!-- 撤回二次确认：破坏性且不可逆，必须显式确认（各端一致，移动端同样弹这个） -->
  <BaseModal :open="confirmingRecall" :title="t('msg.recall')" @close="confirmingRecall = false">
    <p class="text-sm leading-relaxed text-[var(--gosslan-text)]">{{ t("msg.recallConfirm") }}</p>
    <div class="mt-5 flex justify-end gap-2">
      <button
        class="tap-safe rounded-[var(--gosslan-radius-md)] px-4 py-2 text-sm text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        @click="confirmingRecall = false"
      >
        {{ t("common.cancel") }}
      </button>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-2 text-sm text-white transition hover:opacity-90"
        @click="confirmRecall"
      >
        {{ t("common.confirm") }}
      </button>
    </div>
  </BaseModal>

  <MessageContentModal
    :open="fullModalOpen"
    :kind="fullModalKind"
    :content="fullModalContent"
    :mention-names="mentionNames"
    :copied="copiedKey === 'full'"
    @close="fullModalOpen = false"
    @copy="copyContent('full', copyOut($event))"
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
    :can-recall="canRecall"
    :can-pin="isGroup && message.kind !== 'recalled'"
    :pinned="!!pinned"
    :can-cancel-send="canCancelSend"
    @pin="emit('pin')"
    @recall="doRecall"
    @cancel-send="doCancelSend"
    @forward="doForward"
    @favorite="doFavorite"
    @multi-select="doMultiSelect"
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
      <!-- 「选择文字」：触屏下部分选字的**唯一入口**（长按已被消息菜单占用）。
           Telegram 的 Select Text / iMessage 的再长按是同一个模型；微信则在菜单里直接"复制整条"。
           进入后本条气泡开放原生选字并自动全选，系统工具条随即出现。
           ⚠️ 只对 **text** 开放：代码气泡的可选元素在 `CodeBlock` 里，本轮没给它接选择模式，
           给个点了没反应的入口比不给更糟。 -->
      <button
        v-if="message.kind === 'text'"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="enterTextSelect"
      >
        <TextSelect class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("common.selectText") }}
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

      <!-- 置顶：任意群成员都能置（可逆、低风险），与撤回不同 -->
      <button
        v-if="isGroup && message.kind !== 'recalled'"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="closeActionSheet(); emit('pin')"
      >
        <Pin class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ pinned ? t("msg.unpin") : t("msg.pin") }}
      </button>

      <!-- 撤回：与桌面右键菜单**共用同一个判定**（`canRecall`）——
           此前这里是内联条件且漏了 isGroup，导致单聊长按也出现「撤回」，
           而后端只实现了群撤回 ⇒ 点了确认后什么都不发生。 -->
      <button
        v-if="canRecall"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-danger-ink)] transition active:bg-[var(--gosslan-hover)]"
        @click="doRecall"
      >
        <Undo2 class="h-5 w-5" />
        {{ t("msg.recall") }}
      </button>
      <button
        v-if="forwardable(message.kind)"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="doForward"
      >
        <Share2 class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("common.forward") }}
      </button>
      <!-- 收藏：与桌面右键菜单同一份 emit（`doFavorite`）。
           ⚠️ 移动端入口必须在这里**再写一遍** —— 右键菜单与 ActionSheet 是两套独立模板，
           只加一处的结果是"桌面能收藏、手机不能"（这类漂移在项目里已发生过多次）。 -->
      <button
        v-if="forwardable(message.kind)"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="doFavorite"
      >
        <Star class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("favorite.add") }}
      </button>
      <!-- 多选：移动端同样要有入口（桌面右键菜单是另一套模板）。
           待办卡片不给（见 isMultiSelectable）。 -->
      <button
        v-if="isMultiSelectable(message.kind)"
        class="flex items-center gap-3 border-t border-[var(--gosslan-divider)] px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="doMultiSelect"
      >
        <ListChecks class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("multi.enter") }}
      </button>
    </div>
  </ActionSheet>
</template>
