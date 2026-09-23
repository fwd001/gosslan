<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useClipboard } from "@/composables/useClipboard";
import { useExclusivePopup } from "@/composables/useExclusivePopup";
import { useMessageDisplay } from "@/composables/useMessageDisplay";
import { useMessageFile } from "@/composables/useMessageFile";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { textNeedsClamp } from "@/utils/previewMetrics";
import {
  isFavoritableKind,
  isForwardableKind,
  isKnownKind,
  isMultiSelectable,
  isTipKind,
  kindClass,
  UNSUPPORTED_KIND_LABEL,
} from "@/utils/messageKinds";
import { cardCopyText } from "@/utils/cardText";
import { isSelfMessage } from "@/utils/selfChat";
import { parseQuote, stripQuoteMsgId } from "@/utils/quote";
import { QUOTE_BG, QUOTE_TEXT_STYLE } from "@/utils/quoteStyle";
import { parseFileMeta } from "@/utils/fileMeta";
import { openLocalFile } from "@/utils/localFile";
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
import EmojiPicker from "@/components/EmojiPicker.vue";
import type { ReactionChip } from "@/utils/reactions";
import MessageContentModal from "@/components/message/MessageContentModal.vue";
import TodoCardBubble from "@/components/TodoCardBubble.vue";
import UnsupportedKindBubble from "@/components/message/UnsupportedKindBubble.vue";
import MessageContextMenu from "@/components/message/MessageContextMenu.vue";
import ActionSheet from "@/components/ActionSheet.vue";
import { Check, Copy, CornerUpLeft, ImageOff, ListChecks, Pin, Save, Share2, Smile, Star, TextSelect, Undo2 } from "lucide-vue-next";
import type { MessageRecord, MsgKind } from "@/types";
import { urlToBase64 } from "@/utils/imageBytes";

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
  // 载荷解析走唯一一份 `utils/fileMeta`（此前这里又手写了一次 JSON.parse）
  const sha = parseFileMeta(props.message.content)?.sha256;
  if (!sha) return null;
  const rec = chat.contentTransfers.find((c) => c.cid === sha);
  if (!rec) return null;
  return rec.status === "incomplete" || rec.status === "rejected" ? rec : null;
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
 * 本条消息在长按面板里是否给「复制」这一条（复制的是**文字形态**）。
 * 正文/代码自不必说；**卡片类（待办/投票/群公告）也给** —— 复制的是它的文字形态，
 * 与收藏页的「复制」同源（`utils/cardText`）。用户 2026-09-21：
 * 「群任务也不能复制啊，收藏咋还有复制呢」。
 * ⚠️ 图片/文件在面板里有各自的「复制图片 / 复制文件」，不走这一条。
 */
const copyableText = computed(
  () =>
    props.message.kind === "text" ||
    props.message.kind === "code" ||
    kindClass(props.message.kind) === "card",
);

/**
 * 执行复制（右键菜单 / 长按面板共用）。卡片走 `cardCopyText`；
 * 解析不出来（载荷非法、待办已删）就明确报失败，而不是把 JSON 原样塞进剪贴板。
 */
function copyFromMenu() {
  if (kindClass(props.message.kind) === "card") {
    const text = cardCopyText(props.message.kind, props.message.content);
    if (!text) {
      app.toast(t("msg.copyFail"), "error");
      return;
    }
    void copyTextWithToast("card", text);
    return;
  }
  void copyTextWithToast(
    props.message.kind === "code" ? "code" : "text",
    props.message.content,
  );
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

/**
 * 选中态的**底色**（铺在整条消息项的最外层）—— 多选选中与"引用定位"**共用同一种颜色**。
 *
 * ⚠️ 两者必须一致（用户 2026-09-21：「引用消息定位的选中颜色怎么和消息多选的颜色不一样？」）：
 * 定位高亮原先用主题浅色 `--gosslan-primary-light`（橙），多选用 `--gosslan-hover`（浅灰），
 * 同一件事两套观感 ⇒ 统一到多选那档浅灰。定位是 1.6s 的瞬时高亮，正常态本来没有底色，
 * 所以"闪一下浅灰"足够显眼，不需要再靠颜色区分。
 */
const tintClass = computed(() => {
  const on = highlighted.value || (props.selectMode && props.selected);
  return on ? "bg-[var(--gosslan-chat)]" : "";
});

/**
 * 选中底色的**绘制方式**：把浅色 `--gosslan-hover` 叠在同色底（`--gosslan-chat`）上合成出来，
 * 结果**不透明**。
 *
 * ⚠️ 为什么要这么绕（用户 2026-09-21：「出现黑线了」）：底色原来是半透明的，
 * 而条目盒子的高度是小数（代码气泡行高不是整数）⇒ 相邻两条盒子的边界落在**小数坐标**上，
 * 浏览器光栅化时会把边界那一行**双重覆盖**，半透明色叠两次 ⇒ 颜色变深 ⇒ 一条深色发丝线。
 * 换成"底层不透明 + 上层是同色渐变"后：每个盒子画的都是**不透明**的一整块，
 * 相邻两块在边界处的覆盖度之和恰好为 1 ⇒ 不深不浅，边界彻底干净。
 * （视觉结果与原来完全一致：就是 `--gosslan-hover` 铺在 `--gosslan-chat` 上的颜色。）
 */
const tintStyle = computed(() => {
  const on = highlighted.value || (props.selectMode && props.selected);
  if (!on) return undefined;
  return {
    backgroundColor: "var(--gosslan-chat)",
    backgroundImage: "linear-gradient(var(--gosslan-hover), var(--gosslan-hover))",
  };
});

/**
 * 正文块（气泡行 / 提示行）要不要自己带 6px **上**间距。
 *
 * 那 6px 原本来自列表项的 `py-1.5`，现在改由"本项的第一个可见块"承担（列表项只留 `pb-1.5`）：
 * 底色要包住整块、相邻块要相接，所以正文块的盒子必须从列表项的上边缘就开始。
 * 但分割行压在正文块上面时不能重复给 —— 分割行的 `pt` 里已经含了这 6px（见模板）。
 */
const needsTopPad = computed(() => !showTimeDivider.value && !props.showUnreadDivider);

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

/**
 * 转发 / 收藏的判据从 `utils/messageKinds` 取（**唯一一份**）：右键菜单、长按面板、
 * 收藏详情三处的动作集合必须一致（此前各写一份 ⇒ "菜单没有转发、收藏页却有"，
 * 而且点下去必被发送侧白名单拒）。见 `isForwardableKind` / `isFavoritableKind` 的说明。
 */
const forwardable = isForwardableKind;
const favoritable = isFavoritableKind;

/**
 * 本条消息引用的原消息 id（没有引用则为空）。
 * 供「定位到引用消息」菜单项使用 —— 与微信一致：引用跳转既可以从引用块点，
 * 也可以在消息菜单里选（用户 2026-09-21）。
 */
const quotedMsgId = computed(() => parseQuote(props.message.content).msgId ?? "");

/**
 * 引用块的头一行（「引用 发送者：片段」）：引用块画在**气泡外的正文下方**（微信同款），
 * 只有文本/代码消息才会带引用（回复永远是文本消息）。
 */
const quoteHeader = computed(() => {
  if (props.message.kind !== "text" && props.message.kind !== "code") return "";
  return parseQuote(props.message.content).header;
});

/**
 * 点引用块 = **直接查看**被引用的内容（用户 2026-09-21：「能直接查看的要能直接查看，不要跳转」）：
 *   图片 → 相册大图（ChatWindow 按 msg_id 定位）；合并卡片 → 卡片详情；
 *   文本/代码 → 全文弹窗；文件 → 用系统应用打开。
 * 原消息不在本机（未加载/已删除）时提示，而不是偷偷跳转。
 * 「定位到原消息」仍然有 —— 在消息菜单里（见 `quotedMsgId`），与微信一致。
 */
async function viewQuoted() {
  const id = quotedMsgId.value;
  if (!id) return;
  const convId = props.message.conv_id;
  const orig = chat.messages[convId]?.find((m) => m.msg_id === id);
  if (!orig) {
    app.toast(t("msg.quoteOriginalMissing"), "info");
    return;
  }
  if (orig.kind === "image") {
    emit("open-image", id);
    return;
  }
  if (orig.kind === "merge") {
    emit("open-merge", { content: orig.content, senderId: orig.sender_id });
    return;
  }
  if (orig.kind === "file") {
    // 被引用的是**另一条**消息 ⇒ 拿不到它的传输记录，只能用载荷里已落下的路径
    // （解析走唯一一份 `utils/fileMeta`，与气泡那侧同一个实现）。
    const meta = parseFileMeta(orig.content);
    if (!meta?.path) {
      app.toast(t("msg.filePathUnavailable"), "error");
      return;
    }
    try {
      await openLocalFile(meta.path, meta.name || t("common.file"));
    } catch (e) {
      app.toastError(e, t("msg.openFileFail"));
    }
    return;
  }
  if (orig.kind === "text" || orig.kind === "code") {
    openFullModal(orig.kind, orig.content);
    return;
  }
  app.toast(t("msg.quoteOriginalMissing"), "info");
}

// ---------------- 表情回应入口（飞书式：气泡外侧笑脸按钮 → 完整表情选择器） ----------------
/**
 * 选择器的开/关与**全局浮层互斥**（key 按消息区分 ⇒ 两条消息的选择器不会并存）。
 * 点外部收起（入口按钮与选择器根上都 `@click.stop`，不会误关）。
 *
 * ⚠️ 弹出位置必须**自己算**（用户 2026-09-21：「表情弹出的时候不计算弹出位置吗？」）：
 * 选择器挂在消息里会被消息列表的 `overflow-y: auto` **裁掉**，所以这里
 * ① Teleport 到 body、② 用入口按钮的 `getBoundingClientRect()` 算出 fixed 坐标：
 *    横向夹在视口内（面板宽 min(360, 视口-32)），纵向按入口在视口的上/下半决定往上还是往下弹。
 * 滚动/改窗口大小时直接收起（比跟着飘更稳）。
 */
const reactionPickerOpen = ref(false);
const reactionBtnRef = ref<HTMLElement | null>(null);
/** Teleport 到 body 的那层浮层（用来排除"它自己的内部滚动"被当成列表滚动） */
const reactionPickerRef = ref<HTMLElement | null>(null);
const reactionPickerPos = ref<{ left: number; top: number; placement: "above" | "below" } | null>(null);
const reactionPopup = useExclusivePopup(`reaction-picker:${String(props.message.msg_id ?? props.message.id ?? "")}`);

function positionReactionPicker() {
  const btn = reactionBtnRef.value;
  if (!btn) return;
  const r = btn.getBoundingClientRect();
  const pad = 8;
  const w = Math.min(360, window.innerWidth - pad * 2);
  const left = Math.max(pad, Math.min(r.left, window.innerWidth - pad - w));
  // 入口在视口下半 ⇒ 往上弹；上半 ⇒ 往下弹（避免被顶出屏幕）
  const placement: "above" | "below" = r.top > window.innerHeight / 2 ? "above" : "below";
  const top = placement === "above" ? r.top - pad : r.bottom + pad;
  reactionPickerPos.value = { left, top, placement };
}
function toggleReactionPicker() {
  if (reactionPickerOpen.value) {
    closeReactionPicker();
    return;
  }
  positionReactionPicker();
  reactionPickerOpen.value = true;
  reactionPopup.claim();
}
function closeReactionPicker() {
  reactionPopup.release();
  reactionPickerOpen.value = false;
  reactionPickerPos.value = null;
}
function onDocClickForReactionPicker() {
  closeReactionPicker();
}
/** 滚动/改尺寸就收起：列表在滚，固定坐标的浮层会飘。
 *  ⚠️ 必须**忽略选择器自己的内部滚动**（用户 2026-09-21：「我拉滚动条，弹框怎么没了」）：
 *  监听带了 `capture: true`，表情网格自己的 `overflow-y: auto` 一滚也会冒到 window 上，
 *  不做这个排除就会"一拉滚动条弹框就消失"。 */
function onScrollOrResizeForReactionPicker(e: Event) {
  if (!reactionPickerOpen.value) return;
  const panel = reactionPickerRef.value;
  if (e.type === "scroll" && panel && e.target instanceof Node && panel.contains(e.target)) return;
  closeReactionPicker();
}
onMounted(() => {
  document.addEventListener("click", onDocClickForReactionPicker);
  window.addEventListener("scroll", onScrollOrResizeForReactionPicker, true);
  window.addEventListener("resize", onScrollOrResizeForReactionPicker);
});
onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClickForReactionPicker);
  window.removeEventListener("scroll", onScrollOrResizeForReactionPicker, true);
  window.removeEventListener("resize", onScrollOrResizeForReactionPicker);
});
function onReactionPick(emoji: string) {
  closeReactionPicker();
  emit("react", emoji);
}

/** 「定位到引用的消息」：先收掉菜单/面板，再跳（否则浮层会盖在目标上，看起来像"跳错了"） */
function onLocateQuote() {
  closeContextMenu();
  closeActionSheet();
  if (quotedMsgId.value) emit("locate", quotedMsgId.value);
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
    // 取字节 + base64 收在 utils/imageBytes（与 ImageLightbox 的「另存图片」共用一份）
    const base64Data = await urlToBase64(url);
    await invoke("save_data_file", { base64Data, destination });
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
  // 本机不认识的 kind（对端版本更新）：载荷通常是 JSON，截进引用块等于把 JSON 露出来
  if (!isKnownKind(kind)) return UNSUPPORTED_KIND_LABEL;
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
  <!-- 选中底色（`:class="tintClass"`）铺在**最外层这一圈**上 —— 它包住本项的**全部**内容：
       消息块（`.group/msg`）+ 它下面的兄弟节点"表情回应条"。

       ⚠️ 为什么必须包到最外层（这是用户 2026-09-21 连报几轮的"消息中间一条白线"的真身）：
       群里 `interactive` 恒为真 ⇒ 表情回应条**每条都会渲染**，且自带 `mt-1`(4px)。
       底色只铺在 `.group/msg` 上时，每条选中块的**下方**会留出那 4px 白缝，
       两条相邻选中消息之间就是"一条白线"（之前几轮我盯着分割行/焦点环找，都找错了）。
       包到外层后：底色上下连续，相邻两条直接相接，白线消失。

       这一层是**纯多包一层**：没有内外边距、没有边框 ⇒ 项高、各行高度、VirtualList 的
       高度估算全都不变（`messageHeight` 不用动）。横向也没有左右内边距 ⇒ 底色**通栏满宽**。

       ⚠️ 表情回应条的悬停揭示**在桌面端已按用户要求关掉**（2026-09-21：「表情回复桌面端先禁用掉」）：
       回应条是本层下面的兄弟节点，**在这层的包裹之内**；它的悬停揭示键是
       `group-hover/msg:` ⇒ **这层必须带 `group/msg`**，否则揭示永远不匹配
       （用户 2026-09-21 上一轮报「表情回应桌面端没有」、这一轮报「表情回应在哪儿？没看见」，
       都是因为这层少了个组名）。入口做成**飞书式**：气泡外侧单个表情按钮 → 点开完整选择器。
       （触屏仍可用：那排按钮自带 `hover-reveal` 兜底；已有的表情胶囊照常显示与点击。）

       ⚠️ 不要给它加 `background-clip`/`-inset-*`/`w-fit`/`ring-*`，也不要把底色挪回 `.group/msg`
       或消息行上：挪回 `.group/msg` 就露出那 4px 白缝；挪到消息行则连"头像上面 / 气泡下面"
       的 6px 内边距都没有了 —— 这几条都被用户逐个否过。 -->
  <div class="group/msg" :class="tintClass" :style="tintStyle">
    <!-- 消息块：纵向内边距只留 `pb-1.5`（那 6px 的上半段由"本项第一个可见块"自己带，
         见 needsTopPad），这样时间/未读分割行不会重复给间距，
         各项总高与改动前逐像素一致（messageHeight 的高度估算表不用动）。 -->
    <div
      class="relative pb-1.5"
    >
    <!-- 多选态：透明覆盖层 + 左侧勾选框，两者都**绝对定位**，不进 flex 流。
         为什么必须这样：气泡宽度是 `max-w-[72%]`，勾选框若作为 flex 兄弟插进来会挤窄气泡、
         正文折行变多，而虚拟列表按 `previewMetrics.COLUMNS_PER_LINE` 估的高度不会跟着变
         ⇒ 相邻消息互相遮挡（`messageHeight` 文件头专门写过这个坑）。
         覆盖层的第二个作用：多选时气泡里的链接/图片/引用点击都不该响应，它一并吃掉。
         提示行（系统消息/已撤回）不可选，所以 `!isTip`。 -->
    <template v-if="selectMode && !isTip && isMultiSelectable(message.kind)">
      <!-- ⚠️ `outline-none` + `data-focus-ring-ok`（元素级豁免，理由见下）：
           这是"整行的透明点击层"，点选之后**它自己就是焦点元素**；此时再按任意键
           （用户按的是 ESC），Chromium 会把 `:focus-visible` 判为真 ⇒ 全局那圈
           2px 的焦点环画在**满宽的行**上，上下两条边就成了横贯整列的两条橙线
           （用户 2026-09-21：「我在选中状态按 ESC 就出现这个线」）。
           它没有任何可见内容，选中与否由左侧勾选圈（选中态实心 + 白勾）表达 ⇒ 不需要再画环。 -->
      <button
        class="absolute inset-0 z-10 outline-none"
        data-focus-ring-ok
        :aria-label="selected ? t('multi.deselect') : t('multi.select')"
        :aria-pressed="selected"
        @click="emit('toggle-select')"
      ></button>
    </template>
    <!-- 时间分割线（间隔 ≥ 5 分钟）：居中浅灰小字。
         · `pt-3.5`(14px) = 原来"列表项 pt-1.5(6) + 本行 py-2 的上半(8)"；
           `pb-2`(8px) 不含那 6px —— 正文块自己会带（见 needsTopPad）。纵向总高不变。
         · `bg-[var(--gosslan-chat)]` 不透明是**必须的**：底色铺在最外层、纵向连着铺，
           这行必须把自己那段盖回去，否则「连时间都被选中了」（用户 2026-09-21 原话）——
           时间戳只该在选中块**外面**（微信同样是白底的时间行）。 -->
    <div
      v-if="showTimeDivider"
      class="bg-[var(--gosslan-chat)] pt-3.5 pb-2 text-center text-[11px] text-[var(--gosslan-text-2)]"
    >
      {{ timeDividerText }}
    </div>

    <!-- 未读分割线（打开会话时定位的第一条未读上方）。总高同样不变：
         `mt-3`(12px) = 原"列表项 pt-1.5(6) + 本行 my-1.5(6)"，`mb-1.5`(6) 承原下外边距；
         正文块的 6px 上间距由它自己带。不透明的原因同时间分割线（这一行也不属于选中块）。 -->
    <div
      v-if="showUnreadDivider"
      class="mt-3 mb-1.5 flex items-center gap-2 bg-[var(--gosslan-chat)] px-3"
    >
      <div class="h-px flex-1 bg-[var(--gosslan-primary-soft)]"></div>
      <span class="rounded-full bg-[var(--gosslan-primary-light)] px-2 py-0.5 text-[11px] text-[var(--gosslan-accent-ink)]">{{ t("msg.unreadDivider") }}</span>
      <div class="h-px flex-1 bg-[var(--gosslan-primary-soft)]"></div>
    </div>

    <!-- 提示行（系统消息 / 已撤回）：微信式居中灰字，**通栏**、无头像、无气泡。
         之所以是"消息行"的**兄弟节点**而不是它内部的一支：放进消息行就会带上 36px 头像
         和 `max-w-[72%]` 的列宽，居中后仍偏向一侧、看着还是一条普通消息 ——
         那正是用户 2026-09-16 报的问题。
         纵向间距同正文块：上间距看 `needsTopPad`（分割行会自己带），下间距由 `.group/msg` 的 `pb-1.5`。 -->
    <div
      v-if="isTip"
      class="px-4 text-center text-xs text-[var(--gosslan-text-2)]"
      :class="needsTopPad ? 'pt-1.5' : ''"
    >
      {{ message.kind === "recalled" ? t("msg.recalled") : message.content }}
    </div>

    <!-- 多选态给左侧勾选圈让出边槽：**只有"别人的"那一行**需要整排右移。
         自己的行头像在右侧、气泡是右对齐的，左移不动它 —— 加了这个内边距只会白白挤窄
         自己的气泡（多一圈折行），换不来任何观感收益。
         别人的行 `pl-10`(40px) = 圈 left-3(12) + 圆 18 + 间隙 10，头像正好从圈右侧干净地起排；
         圈本身**落在选中底色里**（微信同款，见下面那行的说明）。
         ⚠️ 这里只动横向内边距：高度估算用的 `COLUMNS_PER_LINE` 是**常量**、不随宽度变，
         所以不会破坏 VirtualList 的估算（横向挪动与"相邻消息互相遮挡"那个坑无关）。 -->
    <!-- 正文块（气泡行）：**只带条件性的上间距** `pt-1.5`（分割行压在上面时由分割行带），
         下间距统一由 `.group/msg` 的 `pb-1.5` 出（底色要包到那一层）。纵向总高与改动前一致。 -->
    <div
      v-else
      class="relative flex gap-2 px-4 transition"
      :class="[
        needsTopPad ? 'pt-1.5' : '',
        mine ? 'flex-row-reverse' : '',
        selectMode && !mine ? 'pl-10' : '',
      ]"
    >
      <!-- 选中态**底色铺在整条消息（列表项）上**：通栏满宽、直角、无边框，块内含上下 6px 间距，
           相邻两条选中的块**直接相接**，微信同款。
           ── 为什么最终是这个形态（别再改成"框住气泡"或"裁掉上下"）──
           用户同一天前后七轮：①「选中的背景色和边框左边没有边距，上下也没有」；
           ②「框和背景比消息气泡还小、整个消息都包不起来」；③对"整行满宽灰带"说「多选不对劲」；
           ④「实在不行就更微信一样这样也行啊」（附微信截图：整行浅底、勾选圈就在底色里）；
           ⑤「跟微信一样紧贴的成一片」；⑥「微信的选中的背景色他距离上下都有边距，你这个为啥没有？」；
           ⑦「微信他有间距（头像上面，气泡下面），连续消息没间距」← **最终口径**。
           中间试过、都已回退的形态：`inset-y-1 left-2 right-2` 装饰层（框比气泡小 —— 行盒**就是**
           消息的高度，纵向内缩等于把框画进气泡里）、`-inset-y-1` + 行盒 `w-fit` 外扩
           （开发环境几何正确，真机上退化成"整行一条 1px 横线"）、以及 `background-clip: content-box`
           裁掉上下内边距（⑥的误解：那样块与块之间会空 12px，与⑤⑦矛盾）。
           ⚠️ 不要加 `background-clip`、`-inset-*`、`w-fit`、`ring-*`，不要换回绝对定位的框，
           也不要把底色挪到这一行上（行盒 = 消息本身，挪过去就没有"头像上面/气泡下面"的间距了）。 -->
      <!-- 头像：每条消息独立完整渲染 -->
      <MessageAvatar :name="avatarName" :avatar="avatarSrc" />

      <!-- 勾选框（微信款）：18px 圆、未选 1px 细边、选中实底 + 细白勾。
           为什么不用 border-2：2px 的环在 16-18px 的圆里内孔只剩 12-14px，深色下是一圈
           又重又闷的「O」（用户 2026-09-17 反馈"太丑"）。微信的勾选圈之所以轻，
           靠的就是 1px 边 + 选中瞬间整个圆变实底，而不是靠加粗描边。
           填充色用 bg-[var(--gosslan-primary)]（正牌 token）—— 之前写的 --gosslan-accent **并不存在**，
           var() 解析失败会让整条声明被丢弃（选中态变成无色圆 + 看不见的白勾）。
           ⚠️ 竖向**对齐头像**，不是行的垂直居中（用户 2026-09-21：「微信的那个 radio 和头像对齐的」）：
           长消息里"行居中"会把圈甩到消息中间去。所以锚点挂在**消息行**上（`absolute` 的
           包含块 = 行的 padding box），`top-0`/`top-1.5` 对齐行的 content 顶（= 头像顶，
           行有 `pt-1.5` 时补上那 6px）＋ `mt-2`(8px) —— 头像 36px 高，8+9 ≈ 18 = 头像的竖向中心，
           与头像同高对齐、**与消息多高无关**。用全是标准间距类，避免再踩"新类没进 CSS"的坑。
           横向 12px（`left-3`）：别人的行由 `pl-10`(40px) 把头像整排让开（圈 12..30 + 间隙 + 头像 40 起），
           自己的行消息在右侧、左边是空的，圈落在整行的选中底色之内。
           `z-20` 压在选中覆盖层（z-10）之上，但 `pointer-events-none` ⇒ 点击仍落到覆盖层。 -->
      <span
        v-if="selectMode && isMultiSelectable(message.kind)"
        class="pointer-events-none absolute left-3 z-20 mt-2 flex h-[18px] w-[18px] items-center justify-center rounded-full border transition"
        :class="[
          needsTopPad ? 'top-1.5' : 'top-0',
          selected
            ? 'border-primary bg-[var(--gosslan-primary)]'
            : 'border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]',
        ]"
      >
        <Check v-if="selected" class="h-2.5 w-2.5 text-white" :stroke-width="2.5" aria-hidden="true" />
      </span>

      <div class="relative flex min-w-0 max-w-[72%] flex-col" :class="mine ? 'items-end' : 'items-start'">
        <!-- 表情回应入口（飞书式）：悬停本条消息 ⇒ **气泡外侧**出现笑脸按钮，
             点开 = 完整表情选择器（见下方 EmojiPicker），选中即作为表情回应发送/取消。
             ⚠️ 按钮与选择器都 **absolute 脱离文档流**：悬停/选表情不改变本条消息的高度
             （上一版把一排快捷表情挂在流内，一悬停整条长高 ⇒ 「鼠标划过跳来跳去」）。
             位置：别人的消息在**气泡右侧**（`-right-11`）、自己的消息在**气泡左侧**（`-left-11`），
             竖向对齐气泡中线；锚在**本列**（列宽=气泡宽）所以按钮紧贴气泡，不贴面板边。
             `hidden group-hover/msg:flex`：悬停本条才出现（组名在 `.group/msg` 上，本列在其内 ✓）。 -->
        <button
          ref="reactionBtnRef"
          v-if="isGroup && !selectMode"
          class="tap-safe hover-reveal pointer-events-auto absolute -right-11 top-1/2 z-20 h-8 w-8 -translate-y-1/2 items-center justify-center rounded-full border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] shadow-sm transition"
          :class="[
            mine ? '-left-11' : '-right-11',
            reactionPickerOpen ? 'flex' : 'hidden group-hover/msg:flex',
          ]"
          :title="t('chat.composer.emoji')"
          :aria-label="t('chat.composer.emoji')"
          @click.stop="toggleReactionPicker"
        >
          <Smile class="h-4 w-4 text-[var(--gosslan-text-2)]" :stroke-width="1.75" />
        </button>
        <!-- 完整表情选择器：**Teleport 到 body + fixed 坐标**（坐标由入口按钮算出，见 positionReactionPicker）。
             挂在消息里会被列表的 `overflow-y: auto` 裁掉 —— 这就是"弹出位置不对"的原因。 -->
        <Teleport to="body">
          <div
            v-if="reactionPickerOpen && reactionPickerPos"
            ref="reactionPickerRef"
            class="fixed z-[70] w-[min(360px,calc(100vw-2rem))]"
            :style="{ left: `${reactionPickerPos.left}px`, top: `${reactionPickerPos.top}px` }"
            @click.stop
          >
            <EmojiPicker
              :open="reactionPickerOpen"
              :placement="reactionPickerPos.placement"
              @select="onReactionPick"
              @close="closeReactionPicker"
            />
          </div>
        </Teleport>
        <!-- 引用块：挂在**气泡外面、正文下方**（微信同款：先自己的话，紧接着被引用的消息）。
             点击 = **直接查看**被引用的内容（用户 2026-09-21：「能直接查看的要能直接查看，不要跳转」）：
             图片→相册大图、合并卡片→卡片详情、文本/代码→全文弹窗、文件→系统打开；
             原消息不在本机时给提示，而不是无脑跳转。「定位到原消息」在消息菜单里
             （`quotedMsgId` 的说明 + 菜单项「定位到引用的消息」）。 -->
        <button
          v-if="quoteHeader"
          class="mt-1 block w-full cursor-pointer rounded-[var(--gosslan-radius-sm)] px-2 py-1 text-left text-[12px] leading-4 transition hover:brightness-110"
          :style="{ background: QUOTE_BG }"
          @click="viewQuoted"
        >
          <span class="quote-text" :style="QUOTE_TEXT_STYLE">{{ quoteHeader }}</span>
        </button>
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
            :card-style="cardStyle"
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
            class="flex h-32 w-52 cursor-pointer flex-col items-center justify-center gap-1 rounded-[var(--gosslan-bubble-radius)] bg-[var(--gosslan-hover)] text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-pressed)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary "
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
            :card-style="cardStyle"
            :mine="mine"
            @open="emit('open-merge', { content: message.content, senderId: message.sender_id })"
          />

          <!-- 群任务卡片（`todo` = Card kind）：把 JSON 载荷渲染成可读卡片，替代原始 JSON 兜底 -->
          <TodoCardBubble
            v-else-if="message.kind === 'todo'"
            :message="message"
            :card-style="cardStyle"
            :mine="mine"
            @open="emit('open-tasks')"
          />

          <!-- 本机不认识的 kind（= 对端 Gosslan 比本机新）：给可解释的占位，
               绝不把载荷原文甩上屏（INV-P24 第 2 条）。判据只能走 isKnownKind ——
               `kindClass` 对未知值也返回 bubble，用它判会永远不进这个分支。 -->
          <UnsupportedKindBubble
            v-else-if="!isKnownKind(message.kind)"
            :kind="message.kind"
            :content="message.content"
            :bubble-style="bubbleStyle"
          />

          <!-- 已知但没有专门渲染分支的 kind：按纯文本兜底。
               排版必须与 MessageTextBubble 一致（py-1.5 / leading-normal），
               否则虚拟列表按 `previewMetrics.TEXT_BUBBLE_PADDING` 估的高度会对不上。 -->
          <div v-else class="select-text px-3 py-1.5 text-sm leading-normal break-all" :style="bubbleStyle">
            {{ message.content }}
          </div>
        </div>
      </div>
    </div>
  </div>

  <!-- 表情回应条：挂在消息行**下方**（飞书/微信同款位置），与气泡同侧对齐。
       放在行内会被 `flex items-end` 摆到气泡右侧，语义不对。
       ⚠️ 它必须留在上面那层"选中底色"的包裹**之内**：它有 `mt-1`(4px)，群里又恒渲染，
       漏在外面就会在每条选中块下方留一条 4px 白缝（用户 2026-09-21 报的"消息中间一条白线"）。 -->
  <MessageReactionBar
    v-if="!isTip"
    :chips="reactions ?? []"
    :mine="mine"
    :interactive="!!isGroup"
    :class="mine ? 'self-end pr-1' : 'self-start pl-1'"
    @toggle="emit('react', $event)"
  />
  </div>

  <!-- ↓ 以下都是**浮层**（弹窗/右键菜单/长按面板）：它们 Teleport 到 body 或 `fixed`，
       不参与布局，所以留在选中底色的包裹之外（既不占高度、也不该被选中态影响）。 -->

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
        class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-2 text-sm text-white transition hover:bg-[var(--gosslan-danger-hover)]"
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
    :quoted-id="quotedMsgId || undefined"
    @close="closeContextMenu()"
    @locate-quote="onLocateQuote"
    @copy-text="closeContextMenu(); copyFromMenu()"
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
        v-if="copyableText"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="closeActionSheet(); copyFromMenu()"
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
      <!-- 收藏：与桌面右键菜单同一份判据（`favoritable`，含卡片类）与同一份 emit（`doFavorite`）。
           ⚠️ 移动端入口必须在这里**再写一遍** —— 右键菜单与 ActionSheet 是两套独立模板，
           只加一处的结果是"桌面能收藏、手机不能"（这类漂移在项目里已发生过多次）。 -->
      <button
        v-if="favoritable(message.kind)"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="doFavorite"
      >
        <Star class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("favorite.add") }}
      </button>
      <!-- 定位到引用消息：本条引用了谁就从菜单跳过去（与微信一致，用户 2026-09-21）。
           与引用块上的点击同一条 emit（`locate`）。 -->
      <button
        v-if="quotedMsgId"
        class="flex items-center gap-3 px-4 py-3 text-left text-[15px] text-[var(--gosslan-text)] transition active:bg-[var(--gosslan-hover)]"
        @click="closeActionSheet(); emit('locate', quotedMsgId)"
      >
        <CornerUpLeft class="h-5 w-5 text-[var(--gosslan-text-2)]" />
        {{ t("msg.locateQuote") }}
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
