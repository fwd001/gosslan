<script setup lang="ts">
import { t } from "@/i18n";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import MessageItem from "@/components/MessageItem.vue";
import VirtualList from "@/components/VirtualList.vue";
import GroupMemberPanel from "@/components/GroupMemberPanel.vue";
import ChatHeader from "@/components/chat/ChatHeader.vue";
import MessageComposer from "@/components/chat/MessageComposer.vue";
import RenameGroupModal from "@/components/chat/RenameGroupModal.vue";
import ForwardModal from "@/components/message/ForwardModal.vue";
import ImageLightbox from "@/components/message/ImageLightbox.vue";
import { estimateMessageHeight } from "@/utils/messageHeight";
import { ArrowDown } from "lucide-vue-next";
import type { LinkState, MessageRecord, MsgKind } from "@/types";

const emit = defineEmits<{ (e: "open-share"): void }>();

const app = useAppStore();
const chat = useChatStore();

const listRef = ref<InstanceType<typeof VirtualList> | null>(null);

const conv = computed(() => chat.activeConversation);
const isGroup = computed(() => chat.activeConv?.startsWith("group:") ?? false);
const messages = computed(() => chat.messages[chat.activeConv ?? ""] ?? []);

/**
 * 消息是否还没加载完：`loadMessages` 完成前 `messages[convId]` 是 undefined。
 * 用它先渲染骨架 —— 否则切会话会先闪一句"暂无消息"再跳出内容（UI 不能等数据）。
 */
const messagesLoading = computed(
  () => !!chat.activeConv && chat.messages[chat.activeConv] === undefined,
);

const online = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return false;
  return chat.friends.some((f) => f.device_id === conv.value!.id && f.online);
});
/** 单聊对方的设备类型（"desktop"/"mobile"，空串 = 未知）。群聊不显示。 */
const deviceType = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return "";
  return chat.friends.find((f) => f.device_id === conv.value!.id)?.device_type ?? "";
});

/** 会话当前链路（最近一条消息的链路 + 跳数）。单聊显示，群聊不显示。 */
const linkState = ref<LinkState | null>(null);
async function refreshLinkState() {
  const id = chat.activeConv;
  if (!id || id.startsWith("group:")) {
    linkState.value = null;
    return;
  }
  linkState.value = await api.getConvLink(id);
}
watch(
  () => [chat.activeConv, messages.value.length] as const,
  refreshLinkState,
  { immediate: true },
);
const isPeerFriend = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return true;
  return chat.friends.some((f) => f.device_id === conv.value!.id);
});

/** 与 MessageItem 共用 previewMetrics 常量：估算高度 = 真实渲染高度。 */
function estimateHeight(m: MessageRecord, index?: number): number {
  return estimateMessageHeight(m, index, {
    messages: messages.value,
    isGroup: isGroup.value,
    selfId: app.device?.device_id,
    fontSize: app.chatStyle.fontSize,
  });
}

// ---------------- 图片相册预览（点击图片 → 打开本会话全部图片，可左右切换） ----------------
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
/**
 * 会话内图片列表（kind=image，或 kind=file 且 subtype=image），**打开预览时才构建**。
 *
 * ⚠️ 这里刻意**不用 computed**（用户 2026-09-12 要求「不要有任何阻断渲染的操作」）：
 * 原先它是 computed，于是**每次消息变化都会重扫全部消息并对每条文件消息 JSON.parse**
 * —— 而消息变化发生在每收一条、每改一次状态（送达/已读回执）时；单会话缓存上限是
 * 10 页 × 100 条 = 1000 条，等于每条消息都要付一次 O(n) 扫描 + 解析。
 * 而这份列表**只在打开图片预览时用得到**，且打开期间图片集合不会变
 * （新图片到达时用户正在看图，让他下次打开再看到即可）。
 * 因此改为**命令式快照**：只在 `openImageAt` 里构建一次并存入 ref，
 * 热路径（消息流）不再有任何全表扫描。
 */
interface LightboxImage {
  msgId: string;
  name: string;
  dataSrc: string | null;
}

function buildLightboxImages(): LightboxImage[] {
  return messages.value
    .filter((m) => {
      if (m.kind === "image") return true;
      if (m.kind === "file") {
        try {
          return (JSON.parse(m.content) as { subtype?: string }).subtype === "image";
        } catch {
          return false;
        }
      }
      return false;
    })
    .map((m) => {
      let name = "image.png";
      let dataSrc: string | null = null;
      if (m.content.startsWith("data:")) {
        dataSrc = m.content; // 旧格式：base64 data URL 直接作为 src
      } else {
        try {
          name = (JSON.parse(m.content) as { name?: string }).name ?? "image.png";
        } catch {
          /* 异常内容按默认名处理 */
        }
      }
      return { msgId: m.msg_id, name, dataSrc };
    });
}

/** 打开期间的图片快照（只在 openImageAt 里赋值）。 */
const lightboxImages = ref<LightboxImage[]>([]);

function openImageAt(msgId: string) {
  lightboxImages.value = buildLightboxImages();
  const idx = lightboxImages.value.findIndex((x) => x.msgId === msgId);
  if (idx < 0) return;
  lightboxIndex.value = idx;
  lightboxOpen.value = true;
}

// ---------------- 群：成员面板 + 改名 ----------------
const membersOpen = ref(false);
const activeGroupId = computed(() =>
  isGroup.value && chat.activeConv ? chat.activeConv.slice(6) : null,
);
const memberCount = computed(() => {
  const gid = activeGroupId.value;
  if (!gid) return 0;
  return chat.groups.find((g) => g.id === gid)?.members.length ?? 0;
});
const canRename = computed(() => {
  const gid = activeGroupId.value;
  if (!gid) return false;
  return chat.groups.find((g) => g.id === gid)?.creator === app.device?.device_id;
});
const renameOpen = ref(false);
const renameCurrent = computed(
  () => chat.groups.find((g) => g.id === activeGroupId.value)?.name ?? "",
);

// ---------------- 群聊 @ ----------------
/** @ 选择选项（不含自己）：名字与消息流昵称同源（nicknameOf），插入的 @名字 必须能和渲染端对上。 */
const mentionMembers = computed(() => {
  const gid = activeGroupId.value;
  const g = gid ? chat.groups.find((x) => x.id === gid) : null;
  if (!g) return [];
  const me = app.device?.device_id;
  return g.members.filter((id) => id !== me).map((id) => ({ id, name: chat.nicknameOf(id) }));
});
/** 渲染端 @ 高亮用的成员名列表（含自己：别人发的消息里可以 @ 我）。
 *  ⚠️ 自己的名字必须直接取本机昵称：`nicknameOf(我的 device_id)` 查不到——
 *  我既不在自己的好友表里、也不在 peers（那是"别的节点"），会退化成设备指纹，
 *  导致别人 @我 时匹配不上、不高亮，与 @其他人 的样式不一致。 */
const mentionNames = computed(() => {
  const gid = activeGroupId.value;
  const g = gid ? chat.groups.find((x) => x.id === gid) : null;
  if (!g) return [];
  const me = app.device?.device_id;
  const myName = app.device?.nickname ?? "";
  // 其余成员保持原样（nicknameOf 查不到时回退设备指纹，与插入端行为一致）；
  // 只有"自己"这一项必须换成昵称，否则 @我 永远匹配不上。
  return g.members.map((id) => (id === me ? myName || id : chat.nicknameOf(id)));
});
async function confirmRename(name: string) {
  renameOpen.value = false;
  const gid = activeGroupId.value;
  if (!gid || !name) return;
  try {
    await chat.renameGroup(gid, name);
    app.toast(t("chat.toast.groupRenamed"), "success");
  } catch (e) {
    app.toastError(e, t("chat.toast.renameFail"));
  }
}

/** 当前会话的第一条未读索引（后端 markRead 前已记录，随历史 prepend 偏移）。 */
const unreadIndex = computed(() => {
  const uj = chat.unreadJump;
  if (!uj || uj.convId !== chat.activeConv) return -1;
  return uj.index;
});

// ---------------- 滚动 ----------------
const nearBottom = ref(true);

// 打开会话/未读定位：优先跳到第一条未读（该消息贴视口顶部），无未读则贴底。
// 跳转后显式解除贴底：防止总高度重测的钉底把用户从未读位置拽回底部。
// unreadJump.index = -1 表示「有未读但消息还在加载、索引未知」，此时不动滚动。
watch(
  () => chat.unreadJump,
  async (uj) => {
    if (!uj || uj.convId !== chat.activeConv) return;
    if (uj.index < 0) return;
    await nextTick();
    await nextTick();
    listRef.value?.scrollToIndex(uj.index, "top");
    listRef.value?.setPinned(false);
  },
  { immediate: true },
);

// 区分 append（末尾新增）、prepend（开头插入历史）与切会话：
// - append 且（自己发的 / 已在底部）→ 贴底；prepend 不滚动（VirtualList 锚定保持位置）；
// - 切会话时重置跟踪，首屏定位交给 unreadJump / VirtualList 的 swap 逻辑，
//   不做 append 式贴底（否则缓存会话切换时会先被拽到底部，再跳未读，来回闪）。
let lastMsgId: string | number | null = null;
watch(
  () => [chat.activeConv, messages.value.length] as const,
  async ([convId], old) => {
    const [oldConvId, oldLen] = old ?? [null, 0];
    if (convId !== oldConvId) {
      lastMsgId = messages.value.at(-1)?.msg_id ?? null;
      // 首次加载（0→N）且没有未读跳转 → 打开即贴底；有未读则等 unreadJump 定位
      if (oldLen === 0 && !(chat.unreadJump && chat.unreadJump.convId === convId)) {
        await nextTick();
        listRef.value?.scrollToBottom();
      }
      return;
    }

    if (oldLen === 0) {
      if (!(chat.unreadJump && chat.unreadJump.convId === convId)) {
        await nextTick();
        listRef.value?.scrollToBottom();
      }
      lastMsgId = messages.value.at(-1)?.msg_id ?? null;
      return;
    }

    const newLast = messages.value.at(-1);
    const newLastId = newLast?.msg_id ?? null;
    const isAppend = newLastId !== null && newLastId !== lastMsgId;
    lastMsgId = newLastId;

    if (isAppend) {
      // 自己发的消息无论 nearBottom 都贴底；对方的消息仅在用户已在底部附近时贴底。
      // 用户 1.2s 内主动向上滚动过则不打扰（否则新消息会反复把人拽回底部）
      const isMine = newLast?.sender_id === app.device?.device_id;
      if ((isMine || nearBottom.value) && !listRef.value?.recentScrollUp?.()) {
        await nextTick();
        listRef.value?.scrollToBottom();
      }
    }
  },
);

// ---------------- 发送 ----------------
async function onSend({ content, kind }: { content: string; kind: MsgKind }) {
  const convId = chat.activeConv;
  if (!convId || !isPeerFriend.value) return;
  try {
    await chat.send(convId, content, kind);
  } catch (e) {
    app.toastError(e, t("msg.sendFailed"));
  }
}

/** 粘贴图片：走 save_outgoing_image → 文件传输，data URL 不进入 SQLite。 */
async function onSendImage(dataUrl: string) {
  const convId = chat.activeConv;
  if (!convId || !isPeerFriend.value) return;
  await chat.sendImage(convId, dataUrl);
}

// ---------------- 引用 / 转发 ----------------
/** 待引用消息（MessageItem 右键"引用"设置，随发送或手动取消清除）。 */
const quote = ref<{ sender: string; snippet: string; msgId: string | number } | null>(null);

/** 转发弹窗状态（MessageItem 右键"转发"设置）。文件消息带本地路径，转发即重发文件。 */
const forward = ref<{ kind: MsgKind; content: string; snippet: string; filePath?: string } | null>(null);

/** 点击引用块定位原消息：滚动 + 短暂高亮 */
const highlightId = ref<string | number | null>(null);
let highlightTimer = 0;
/** `id` 用 String 比较：msg_id 是字符串，但历史数据/乐观记录里可能是数字 id，
 *  宽松比较能同时覆盖"引用定位"与"搜索定位"两条来源。 */
function locateMessage(id: string | number) {
  const target = String(id);
  const idx = messages.value.findIndex((m) => String(m.msg_id ?? m.id) === target);
  if (idx < 0) {
    app.toast(t("chat.toast.originalEarlier"), "info");
    return;
  }
  listRef.value?.scrollToIndex(idx, "top");
  listRef.value?.setPinned(false);
  highlightId.value = id;
  window.clearTimeout(highlightTimer);
  highlightTimer = window.setTimeout(() => (highlightId.value = null), 1600);
}

// 搜索命中定位：store 侧已把会话打开并（必要时逐页）把目标消息加载进来，
// 这里只负责滚动 + 高亮，然后立即清掉请求，避免下次切会话时重复触发。
watch(
  () => chat.locateRequest,
  async (req) => {
    if (!req || req.convId !== chat.activeConv) return;
    await nextTick();
    await nextTick();
    locateMessage(req.msgId);
    chat.clearLocateRequest();
  },
  { immediate: true },
);

async function doForward(convId: string) {
  const f = forward.value;
  forward.value = null;
  if (!f) return;
  try {
    if (f.kind === "file") {
      // 文件转发＝按本地路径把文件重发一遍（内容 JSON 只是元信息，直接转发会指向本机路径）
      if (!f.filePath) {
        app.toast(t("chat.toast.fileNotForward"), "info");
        return;
      }
      if (convId.startsWith("group:")) {
        await chat.sendGroupFileTo(convId.slice(6), f.filePath);
      } else {
        await chat.sendFileTo(convId, f.filePath);
      }
    } else {
      await chat.send(convId, f.content, f.kind);
    }
    app.toast(t("chat.toast.forwarded"), "success");
  } catch (e) {
    app.toastError(e, t("chat.toast.forwardFail"));
  }
}

/** 统一发送文件：自动路由（直连优先，弱网/无直连自动中继），无需用户选择。
 *  群聊会话走群文件链路（send_group_file：Offer → Chunk → Done → CompleteAck）。 */
async function sendOneFile(convId: string, picked: string) {
  if (isGroup.value) {
    const gid = activeGroupId.value;
    if (!gid) return;
    await chat.sendGroupFileTo(gid, picked);
  } else {
    await chat.sendFileTo(convId, picked);
  }
}

async function attachFile() {
  const convId = chat.activeConv;
  if (!convId) return;
  if (!isGroup.value && !isPeerFriend.value) return;
  const picked = await openDialog({ multiple: false });
  if (typeof picked !== "string") return;
  await sendOneFile(convId, picked);
}

// ---------------- 拖拽文件发送 ----------------
// 用 Tauri 的**原生**拖拽事件，而不是 HTML5 drag：WebView 里的 HTML5 drag 拿不到真实文件路径
// （File 对象没有 path），而 Tauri 的 drag-drop 直接给绝对路径，正好接上 sendPastedFiles。
const chatAreaRef = ref<HTMLElement | null>(null);
const fileDragOver = ref(false);
let unlistenDragDrop: (() => void) | null = null;
let dragDropDisposed = false;

/** 拖拽位置是否落在消息区上（Tauri 给的是物理像素，除以 DPR 才能跟 DOM 坐标比）。 */
function isOverChatArea(pos: { x: number; y: number }) {
  const el = chatAreaRef.value;
  if (!el) return false;
  const r = el.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const x = pos.x / dpr;
  const y = pos.y / dpr;
  return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
}

/** 准入条件与附件按钮一致：好友单聊 / 群聊才允许拖进来发。 */
function canDropInto() {
  return !!chat.activeConv && (isGroup.value || isPeerFriend.value);
}

onMounted(() => {
  if (app.isMobile) return; // 拖拽是桌面端行为
  void getCurrentWebview()
    .onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === "enter" || p.type === "over") {
        fileDragOver.value = canDropInto() && isOverChatArea(p.position);
        return;
      }
      if (p.type === "drop") {
        const droppedHere = fileDragOver.value;
        fileDragOver.value = false;
        if (!droppedHere || p.paths.length === 0) return;
        void sendPastedFiles(p.paths);
        return;
      }
      fileDragOver.value = false; // leave
    })
    .then((un) => {
      // 组件可能已卸载：那时立刻退订，避免监听泄漏
      if (dragDropDisposed) un();
      else unlistenDragDrop = un;
    });
});

onBeforeUnmount(() => {
  dragDropDisposed = true;
  unlistenDragDrop?.();
});

/** 输入框粘贴文件（资源管理器复制后 Ctrl+V，微信式）：按真实路径直接走发送链路。 */
async function sendPastedFiles(paths: string[]) {
  const convId = chat.activeConv;
  if (!convId) return;
  for (const p of paths) {
    try {
      await sendOneFile(convId, p);
    } catch (e) {
      app.toastError(e, t("msg.sendFailed"));    }
  }
}

/** 触顶加载更早的历史消息。 */
function onLoadMore() {
  const convId = chat.activeConv;
  if (convId) void chat.loadMoreMessages(convId);
}
</script>

<template>
  <div class="flex h-full flex-col bg-[var(--gosslan-chat)]">
    <ChatHeader
      :conv="conv"
      :is-group="isGroup"
      :online="online"
      :device-type="deviceType"
      :link-state="linkState"
      :member-count="memberCount"
      :can-rename="canRename"
      :show-back="app.isMobile"
      @back="app.mobileView = 'list'"
      @open-members="membersOpen = true"
      @rename="renameOpen = true"
      @open-share="emit('open-share')"
    />

    <!-- 消息区（虚拟滚动，仅纵向）：与头部同底色，无缝衔接 -->
    <div ref="chatAreaRef" class="relative min-h-0 flex-1 overflow-hidden bg-[var(--gosslan-chat)]">
      <!-- 拖拽文件到聊天区：松手即发送。
           用 Tauri 的原生拖拽事件（HTML5 drag 拿不到真实路径），发送复用「粘贴文件」那条链路。
           pointer-events-none：提示层不能吃掉拖拽/点击事件。 -->
      <div
        v-if="fileDragOver"
        class="pointer-events-none absolute inset-2 z-30 flex items-center justify-center rounded-[var(--gosslan-radius-lg)] border-2 border-dashed border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)]"
      >
        <span
          class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-panel)] px-3 py-2 text-[13px] text-[var(--gosslan-text)] shadow-lg"
        >
          {{ isGroup ? t("chat.dropToGroup") : t("chat.dropToSend") }}
        </span>
      </div>
      <!-- 加载骨架：切会话时**立即**渲染（store 的 loadMessages 完成前 messages[convId] 是 undefined）。
           没有它就会先闪一句"暂无消息，打个招呼吧"再跳出内容——既不准又显得卡。
           规则：UI 先出、数据后到（详见 docs/design-guidelines.md §9）。 -->
      <div v-if="messagesLoading" class="flex flex-col gap-3 px-4 py-5" aria-hidden="true">
        <div class="flex gap-2">
          <div class="h-9 w-9 shrink-0 animate-pulse rounded-[var(--gosslan-avatar-radius)] bg-[var(--gosslan-hover)]"></div>
          <div class="h-9 w-40 animate-pulse rounded-[var(--gosslan-bubble-radius)] bg-[var(--gosslan-hover)]"></div>
        </div>
        <div class="flex flex-row-reverse gap-2">
          <div class="h-9 w-9 shrink-0 animate-pulse rounded-[var(--gosslan-avatar-radius)] bg-[var(--gosslan-hover)]"></div>
          <div class="h-9 w-56 animate-pulse rounded-[var(--gosslan-bubble-radius)] bg-[var(--gosslan-hover)]"></div>
        </div>
        <div class="flex gap-2">
          <div class="h-9 w-9 shrink-0 animate-pulse rounded-[var(--gosslan-avatar-radius)] bg-[var(--gosslan-hover)]"></div>
          <div class="h-9 w-32 animate-pulse rounded-[var(--gosslan-bubble-radius)] bg-[var(--gosslan-hover)]"></div>
        </div>
      </div>
      <div
        v-else-if="messages.length === 0"
        class="mt-20 text-center text-sm text-[var(--gosslan-text-2)]"
      >
        {{ t("chat.empty") }}
      </div>
      <VirtualList
        v-else
        ref="listRef"
        :items="messages"
        live
        :auto-scroll-on-swap="!(chat.unreadJump && chat.unreadJump.convId === chat.activeConv)"
        :estimate-height="estimateHeight"
        @load-more="onLoadMore"
        @near-bottom="nearBottom = $event"
      >
        <template #default="{ item, index }">
          <MessageItem
            :message="item"
            :prev="index > 0 ? messages[index - 1] : null"
            :is-group="isGroup"
            :sender-name="isGroup ? chat.nicknameOf(item.sender_id) : ''"
            :group-reader-ids="isGroup && activeGroupId ? chat.groupReaderIds(activeGroupId, item.ts) : []"
            :show-unread-divider="index === unreadIndex"
            :highlight-id="highlightId"
            :mention-names="mentionNames"
            @quote="quote = $event"
            @forward="forward = $event"
            @locate="locateMessage"
            @open-image="openImageAt"
          />
        </template>
      </VirtualList>

      <!-- 回到最新（离开底部时出现） -->
      <button
        v-if="!nearBottom"
        class="tap-safe absolute bottom-4 right-5 z-10 flex items-center gap-1.5 rounded-full border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-3 py-1.5 text-xs text-[var(--gosslan-text)] shadow-lg transition hover:bg-[var(--gosslan-hover)]"
        @click="nearBottom = true; listRef?.scrollToBottom()"
      >
        <ArrowDown class="h-3.5 w-3.5" />
        {{ t("chat.backToLatest") }}
      </button>
    </div>

    <!-- 输入区：浅灰底上放一个白底圆角卡片，无顶部分割线 -->
    <div class="shrink-0 bg-[var(--gosslan-chat)] px-4 pb-3 pt-2">
      <MessageComposer
        v-if="isGroup || isPeerFriend"
        :conv-id="chat.activeConv"
        :quote="quote"
        :mention-members="mentionMembers"
        @send="onSend"
        @send-image="onSendImage"
        @attach="attachFile"
        @paste-files="sendPastedFiles"
        @close-quote="quote = null"
      />
      <div
        v-else
        class="flex min-h-16 items-center justify-center rounded-[var(--gosslan-bubble-radius)] bg-[var(--gosslan-panel)] px-4 text-center text-[13px] text-[var(--gosslan-text-2)]"
      >
        {{ t("chat.notFriend") }}
      </div>
    </div>

    <!-- 群成员面板 -->
    <GroupMemberPanel :open="membersOpen" :group-id="activeGroupId" @close="membersOpen = false" />

    <!-- 转发弹窗 -->
    <ForwardModal
      v-if="forward"
      :open="true"
      :kind="forward.kind"
      :snippet="forward.snippet"
      @close="forward = null"
      @pick="doForward"
    />

    <!-- 修改群名称（仅群主可见入口） -->
    <RenameGroupModal
      :open="renameOpen"
      :current-name="renameCurrent"
      @close="renameOpen = false"
      @confirm="confirmRename"
    />

    <!-- 图片相册预览（会话内全部图片，左右箭头 / 键盘 ←→ 切换） -->
    <ImageLightbox
      :images="lightboxImages"
      v-model:index="lightboxIndex"
      :open="lightboxOpen"
      @close="lightboxOpen = false"
    />
  </div>
</template>
