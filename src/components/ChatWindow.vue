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
import GroupFilesPanel from "@/components/GroupFilesPanel.vue";
import GroupTasksPanel from "@/components/GroupTasksPanel.vue";
import BaseModal from "@/components/BaseModal.vue";
import ChatHeader from "@/components/chat/ChatHeader.vue";
import MessageComposer from "@/components/chat/MessageComposer.vue";
import ForwardModal from "@/components/message/ForwardModal.vue";
import MergeCardModal from "@/components/message/MergeCardModal.vue";
import { useImagePreviewStore } from "@/stores/useImagePreview";
import { estimateMessageHeight } from "@/utils/messageHeight";
import { launchAuxWindow, isWindowOpening } from "@/composables/useWindowLauncher";
import { MENTION_ALL_TOKEN } from "@/utils/messages";
import { fileToDataUrl } from "@/utils/imageBytes";
import { MAX_MERGE_ITEMS, buildMergePayload } from "@/utils/mergeCard";
import { foldReactions, hasMyReaction, type ReactionChip } from "@/utils/reactions";
import { foldPinned, isPinned } from "@/utils/pins";
import { foldTodos, type TodoStatus } from "@/utils/todos";
import { isRenderedInTimeline } from "@/utils/messageKinds";
import { activePopupKey } from "@/utils/popupRegistry";
import { previewText } from "@/utils/messages";
import { ArrowDown, Bluetooth, Layers, X, Pin, Megaphone, Trash2, Share2, Star } from "lucide-vue-next";
import type { LinkState, MessageRecord, MsgKind } from "@/types";

const emit = defineEmits<{ (e: "open-share"): void }>();

const app = useAppStore();
const chat = useChatStore();

const listRef = ref<InstanceType<typeof VirtualList> | null>(null);

const conv = computed(() => chat.activeConversation);
const isGroup = computed(() => chat.activeConv?.startsWith("group:") ?? false);
/** 自聊：会话 id 就是本机 device_id（后端 `send_message` 自聊分流写入 `conv_id = me`）。 */
const isSelfChat = computed(() => chat.activeConv === app.device?.device_id);
/**
 * 时间线上要渲染的消息。
 *
 * ⚠️ **静默类必须在这里被滤掉** —— 这是 `KindClass::Silent`（「不进时间线」）
 * 契约**唯一真正的落地点**。漏掉它的后果是：回一次表情、置顶、撤回，时间线上就多一条
 * `{"target":"...","emoji":"[赞]","add":true}` 的裸 JSON 气泡，而且与正确的聚合视图
 * （气泡下方的 chip / 顶部置顶条）**同时出现**。
 *
 * 后端已经按同一张表分支（不计未读、不进预览），这里补齐渲染侧。
 * `card`（公告 / 投票）一并过滤 —— 公告已有独立的常驻横幅，再进时间线会重复。
 *
 * **例外：`todo`**（2026-09-17 用户要求）—— 任务消息本属 Card，但要在时间线上以
 * 卡片形式展示（不再落到"未知 kind 兜底"里显示原始 JSON）。它进时间线、计未读、弹通知
 * （Card 口径），且卡片里有「查看任务」直接开面板，不会与面板重复。
 */
const messages = computed(() =>
  (chat.messages[chat.activeConv ?? ""] ?? []).filter((m) => isRenderedInTimeline(m.kind)),
);

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
/**
 * 对方 Gosslan 的线格式版本比本机高 ⇒ 头部一个标记（整句说明在联系人详情）。
 * 结论由后端算好随好友记录带下来；对方没报版本（老实例）时恒为 false。
 */
const peerVersionNewer = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return false;
  return chat.friends.find((f) => f.device_id === conv.value!.id)?.peer_version_newer ?? false;
});

/** 会话当前链路（最近一条消息的链路 + 跳数）。单聊显示，群聊不显示。 */
const linkState = ref<LinkState | null>(null);
async function refreshLinkState() {
  const id = chat.activeConv;
  if (!id || id.startsWith("group:")) {
    linkState.value = null;
    return;
  }
  const next = await api.getConvLink(id);
  // ⚠️ 过期守卫（审计阶段 4 · 4.1-3）：`getConvLink` 是 IPC，慢响应可能晚于用户切会话。
  // 没有这道判断时，上一个对端的链路会被写进**当前**聊天头 —— 与下方注释立的契约
  // （"提示必须和现在这条链路一致"）直接冲突，表现是"连着 Wi-Fi 却显示蓝牙/中继"。
  if (id !== chat.activeConv) return;
  linkState.value = next;
}
watch(
  () => {
    // 活跃对端的**实时链路**也要参与依赖：链路从蓝牙/中继切回局域网时，
    // peers-updated 会带上新的 link，聊天头必须立刻改成「直连」，
    // 而不是等用户再发一条消息。
    const peerLink = chat.peers.find((p) => p.device_id === chat.activeConv)?.link ?? null;
    return [chat.activeConv, messages.value.length, peerLink] as const;
  },
  refreshLinkState,
  { immediate: true },
);

/**
 * 当前单聊是否真的走在蓝牙链路上（用户 2026-09-13 要求：蓝牙聊天框要说明传输速度）。
 *
 * 判据取**在线节点表的实时链路**（`peer.link`，由后端 `fill_peer_links` 按真实链路填），
 * 而不是上面的 `linkState` —— 后者是"最近一条消息走的路径"的快照，链路可能早就切了。
 * 提示必须和"现在这条链路"一致，否则会在 Wi-Fi 链路上误导用户。
 */
const btLink = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return false;
  return chat.peers.find((p) => p.device_id === conv.value!.id)?.link === "bluetooth";
});
/** 速度提示可关闭：按会话各记一次（切走再回来重新提示，因为换会话就是换链路场景）。 */
const btHintDismissed = ref(false);
watch(
  () => chat.activeConv,
  () => {
    btHintDismissed.value = false;
  },
);
/**
 * 切会话：清掉"跟着输入框走的发送上下文"（引用 / 转发草稿）。
 *
 * 为什么必须显式清：这两项只在"发送成功"或"用户点取消"时才被清空，
 * 切会话时留着就会出现 —— 在 B 会话看到 A 的引用条，发出去的消息还带着 A 的消息片段。
 * 这是会把内容发错会话的缺陷，不是观感问题。
 * 输入框里的**文字**由模板上的 `:key="chat.activeConv"` 让 MessageComposer 整体重建来清
 * （编辑器是 contenteditable，DOM 是唯一真相，没有"清空"之外的复位路径）。
 */
watch(
  () => chat.activeConv,
  (_next, prev) => {
    quote.value = null;
    forward.value = null;
    // 切会话必须退出多选：已选集合里是对**上一个会话**的消息，留着会让"已选 N 条"
    // 与实际可见内容对不上（批量删除/转发会作用到看不见的消息上）。
    exitMultiSelect();
    // ⚠️ 会话级浮层也要一起收尾（审计阶段 4 · 4.1-7）：上面那三样是"会把内容发错会话"才修的，
    // 而这五样是"会把内容**看成**别的会话的" —— 点系统通知会直接换掉 activeConv 而不经任何
    // UI 收尾（`useChatStore.handleNotificationClick`），于是"挂着 A 群的文件/成员/任务面板、
    // 标题却是 B"是真的会发生的路径，不是理论。lightbox/公告视图同理。
    membersOpen.value = false;
    filesOpen.value = false;
    tasksOpen.value = false;
    taskFocusId.value = null;
    // 预览是全局那一份（#40），所以按**来源**收：只有"这份相册是上一个会话给的"才关掉。
    // 无条件 close() 会把"从任务详情点开的图"跟着切会话一起弄没。
    if (prev) preview.closeIfFrom(`conv:${prev}`);
    announceViewOpen.value = false;
  },
);
// 移动端「返回会话列表」是**布局层**改 `app.mobileView` 的导航动作：它既不卸载 ChatWindow
// （挂载条件只看 `navState === 'chats' && activeConv`），也不会触发上面那个 activeConv
// watcher —— 于是多选态一直挂着。而 `app.multiSelectActive` 同时是底部 TabBar 的显示条件
// 之一，用户看到的就是"返回之后 TabBar 永久消失，而它正是唯一的导航出口"（审计阶段 4 · 4.1-4）。
// 在这里收尾 = 离开聊天视图即放弃选择，与"切会话退出多选"同一口径；不改 TabBar 的判据
// （它为什么把 multiSelectActive 算进去没有取证，动等于改别处的既有意图）。
watch(
  () => app.mobileView,
  (v) => {
    if (v !== "chat" && multiSelect.value) exitMultiSelect();
  },
);
const isPeerFriend = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return true;
  if (conv.value.id === app.device?.device_id) return true; // 自聊：自己就是「好友」
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

function openImageAt(msgId: string) {
  const items = buildLightboxImages();
  const idx = items.findIndex((x) => x.msgId === msgId);
  if (idx < 0) return;
  // 交给全局那一份实例（#40）：本窗口只负责**构建数组**，渲染与左右切换在
  // `ResponsiveLayout` 里那个唯一的 `<ImageLightbox>`。`source` 带上会话 id ⇒
  // 切会话时只收掉"这个会话给出的"预览（见下面 activeConv 那个 watcher）。
  const convId = chat.activeConv ?? "";
  preview.openGallery(items, idx, convId ? `conv:${convId}` : null);
}

// ---------------- 群：成员面板 + 改名 ----------------
const membersOpen = ref(false);
const filesOpen = ref(false);
const tasksOpen = ref(false);
const activeGroupId = computed(() =>
  isGroup.value && chat.activeConv ? chat.activeConv.slice(6) : null,
);
/** 全局图片预览（#40：实例只有 `ResponsiveLayout` 里那一个，这里只负责给出数组）。 */
const preview = useImagePreviewStore();

/** 当前群的任务窗口是否正在打开（按钮 pending 反馈）。 */
// 一扇固定 label 的窗口 ⇒ pending 状态与"当前是哪个群"无关（切群不换窗口，只换内容）。
const tasksOpening = computed(() => isWindowOpening("tasks"));
/** 当前会话「与我相关的未完成任务」数（任务图标上的蓝色徽标）。
 *  数据只有一份，在 `chat.openTodoByConv`（判定见 `utils/todos::openTodosForMe`）；
 *  这里只做取值，不在组件里重新折一遍任务。 */
const openTasksForActive = computed(() => {
  const id = chat.activeConv;
  return id ? (chat.openTodoByConv[id] ?? 0) : 0;
});

/** 从任务卡片点进来时要直达详情的那条任务（null = 只是打开看板）。 */
const taskFocusId = ref<string | null>(null);
/**
 * 打开群任务：桌面端开**独立窗口**（每群一个），移动端/窗口创建失败回退到应用内弹窗。
 * 与设置/日志同一套单飞 + 防抖（`launchAuxWindow`），窗口实例唯一性由后端 `ensure_aux_window` 保证。
 *
 * `todoId`（用户 #23 与 #39）两条呈现路径都覆盖：
 * 应用内弹窗走 prop（`taskFocusId`）；桌面端独立窗口走后端那条「暂存 + 定向事件」——
 * 新建的窗口在挂载时 `take_group_todo_focus` 取走（事件那时没有监听者，只发事件就是
 * "第一次点没反应"），本来就开着的窗口靠事件被叫醒后再取同一个暂存。
 * 窗口本身是**固定 label 的一扇**（`tasks`）：群由后端那份当前上下文决定，切群 = 换内容。
 * 之所以这样：用户 2026-09-24 要"不传参数也能先把 WebView 建好、用时瞬间激活"，
 * 而每群一扇的动态 label 在预热时不知道该建哪一扇。
 */
function openTasks(todoId?: string) {
  const gid = activeGroupId.value;
  if (!gid) return;
  taskFocusId.value = todoId ?? null;
  if (app.isMobile) {
    tasksOpen.value = true;
    return;
  }
  // ⚠️ 投递展开目标必须**独立于** `launchAuxWindow`：那条启动器有单飞 + 连点防抖，
  // 判定"这次不算新打开"时根本不会调用 `open_group_todos_window` ⇒ 第二张卡片带的
  // todoId 就地消失，表现还是"点了没反应"。命令本身是幂等的（覆盖同一个暂存位）。
  if (todoId) {
    void api.requestGroupTodoFocus(gid, todoId).catch(() => {
      /* 只丢"自动展开"这一点便利：窗口照开，用户点进去就行，不为它弹窗 */
    });
  }
  void launchAuxWindow("tasks", () => api.openGroupTodosWindow(gid, todoId)).catch(
    (e) => {
      app.toastError(e, t("common.operationFail"));
      tasksOpen.value = true; // 独立窗口开不出来 → 回退到应用内弹窗
    },
  );
}
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

/**
 * 表情回应的折叠结果：**在会话层算一次**再按 msg_id 分发。
 * 放到 MessageItem 里各自算会让每条消息都遍历整份消息列表（O(n²)）——
 * 群聊一屏几十条时这是实打实的卡顿。
 */
const reactionMap = computed(() => {
  const convId = chat.activeConv;
  if (!convId) return new Map<string, ReactionChip[]>();
  return foldReactions(chat.messages[convId] ?? [], app.device?.device_id ?? "");
});

/**
 * 时间线上每张任务卡片的**当前状态**（`todo_id` → 状态）。
 *
 * 为什么必须单独算：卡片气泡读的是**创建那条 `todo` 消息的载荷**，而之后的每次改动都走
 * `todo_update`（静默事件、不进时间线）⇒ 卡片自己的载荷**永远停在创建那一刻**，
 * 而新建任务恒为「待办」，于是"干完了的任务在聊天里还挂着待办"（用户 #23）。
 * 判据仍只有 `foldTodos` 一份（与看板、成员面板同一折叠结果），这里只是把它换成
 * 按 `todo_id` 查表的形式。与 `reactionMap` 同构：**会话层算一次**，
 * 放进气泡里各自折叠就是 O(n²)。
 */
const todoLiveStatus = computed(() => {
  const m = new Map<string, TodoStatus>();
  const convId = chat.activeConv;
  if (!convId) return m;
  for (const x of foldTodos(chat.messages[convId] ?? [])) m.set(x.todoId, x.status);
  return m;
});

/**
 * 当前生效的群公告：按 **(seq, msg_id)** 取最大的那条发布事件。
 *
 * 不按墙上时间 —— 只有群主能发，而群主的 Lamport 时钟单调，自己两条公告不可能同 seq，
 * tie-break 只是防御。`announcement_delete`（墓碑，静默类）指向的公告视为不存在。
 * 折叠形状与 `foldPinned` 完全同构（那边有单测），故此处不再单开一份测试。
 */
const announcement = computed(() => {
  const convId = chat.activeConv;
  if (!convId?.startsWith("group:")) return null;
  const list = chat.messages[convId] ?? [];
  let best: { seq: number; msgId: string; text: string } | null = null;
  const tombstones = new Set<string>();
  for (const m of list) {
    if (m.kind === "announcement_delete") {
      try {
        const id = (JSON.parse(m.content) as { ann_id?: string }).ann_id;
        if (id) tombstones.add(id);
      } catch {
        /* 畸形载荷忽略 */
      }
      continue;
    }
    if (m.kind !== "announcement") continue;
    const newer = !best || m.seq > best.seq || (m.seq === best.seq && m.msg_id > best.msgId);
    if (!newer) continue;
    try {
      const text = (JSON.parse(m.content) as { text?: string }).text ?? "";
      best = { seq: m.seq, msgId: m.msg_id, text };
    } catch {
      /* 畸形载荷忽略 */
    }
  }
  if (!best || tombstones.has(best.msgId)) return null;
  return best;
});

/** 只有群主能发布/删除公告（后端同样校验；前端隐藏入口是为了不让用户白点一次）。
 *  发布/修改入口在「成员管理」弹窗（用户 2026-09-17：公告与群名/成员一起管）。 */
const canPublishAnnouncement = computed(
  () => !!activeGroupId.value && chat.groups.find((g) => g.id === activeGroupId.value)?.creator === app.device?.device_id,
);

// ---------------- 公告全文查看 + 删除（用户 2026-09-17） ----------------

/** 点横幅正文 = 打开**全文弹窗**（此前是 toast —— 长公告截断后 toast 也看不全）。 */
const announceViewOpen = ref(false);

/** 删除两段式确认：第一次点进入待确认态，再次点击才真正删（会同步到全群、不可恢复）。 */
const announceDeleteArmed = ref(false);

async function deleteAnnouncement() {
  const gid = activeGroupId.value;
  const ann = announcement.value;
  if (!gid || !ann) return;
  try {
    await chat.deleteAnnouncement(gid, ann.msgId);
    announceDeleteArmed.value = false;
    announceViewOpen.value = false;
    app.toast(t("group.announceDeleted"), "success");
  } catch (e) {
    app.toastError(e, t("group.announceDeleteFail"));
  }
}

/** 当前被置顶的消息 id（按置顶版本从新到旧）。与回应同理：会话层算一次。 */
const pinnedIds = computed(() => {
  const convId = chat.activeConv;
  if (!convId) return [];
  return foldPinned(chat.messages[convId] ?? []);
});

/** 置顶条要展示的条目：只保留还能在本机找到的消息（已被清空历史的就不列了）。 */
const pinnedItems = computed(() => {
  const convId = chat.activeConv;
  if (!convId) return [];
  const list = chat.messages[convId] ?? [];
  return pinnedIds.value
    .map((id) => list.find((m) => m.msg_id === id))
    .filter((m): m is NonNullable<typeof m> => !!m)
    .map((m) => ({ id: m.msg_id, text: previewText(m) }));
});

/**
 * 一行里能显示几条置顶。**移动端只给 1 条** —— 一行放三个的话每个都被压成
 * 省略号，等于三个都读不出来；桌面给 3 条。超出部分走 `+N` 展开。
 */
const visiblePinned = computed(() =>
  app.isMobile ? pinnedItems.value.slice(0, 1) : pinnedItems.value.slice(0, 3),
);
/** 「+N」的就地展开态（切换会话时收起，避免把上一次的展开带过来）。 */
const pinsExpanded = ref(false);
watch(() => chat.activeConv, () => {
  pinsExpanded.value = false;
});

/** 点置顶条：跳到那条消息（复用既有的定位机制）。 */
function gotoPinned(msgId: string) {
  const convId = chat.activeConv;
  if (!convId) return;
  void chat.locateMessageInConv(convId, msgId);
}

/** 切换置顶。菜单里的文案由 isPinned 决定。 */
function togglePin(msgId: string) {
  const convId = chat.activeConv;
  if (!convId?.startsWith("group:")) return;
  const next = !isPinned(chat.messages[convId] ?? [], msgId);
  void chat.pinMessage(convId.slice(6), msgId, next).catch((e) => {
    app.toastError(e, t("msg.pinFail"));
  });
}

/** 点 chip：已点过则取消，否则添加。 */
function toggleReaction(msgId: string, emoji: string) {
  const convId = chat.activeConv;
  if (!convId?.startsWith("group:")) return;
  const mine = hasMyReaction(
    chat.messages[convId] ?? [],
    msgId,
    emoji,
    app.device?.device_id ?? "",
  );
  void chat.sendReaction(convId.slice(6), msgId, emoji, !mine).catch((e) => {
    app.toastError(e, t("msg.reactionFail"));
  });
}

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
/** 「@到自己」在自己视角里显示成 `@你`（用户 2026-09-26）。
 *  ⚠️ 名字必须与上面 `mentionNames` 里的"自己"取同一处（本机昵称，而不是
 *  `nicknameOf(我的 device_id)`）—— 否则段永远匹配不上，只有别人那侧显示正常。 */
const selfMention = computed(() => {
  const name = app.device?.nickname ?? "";
  return name ? { name, label: t("mention.self") } : null;
});

const mentionNames = computed(() => {
  const gid = activeGroupId.value;
  const g = gid ? chat.groups.find((x) => x.id === gid) : null;
  if (!g) return [];
  const me = app.device?.device_id;
  const myName = app.device?.nickname ?? "";
  // 其余成员保持原样（nicknameOf 查不到时回退设备指纹，与插入端行为一致）；
  // 只有"自己"这一项必须换成昵称，否则 @我 永远匹配不上。
  // 「所有人」补进名单，让 @所有人 与 @成员 高亮样式一致（buildMentionRe 会去重，
  // 真有成员叫这个名字也不会生成重复分支）。
  return [...g.members.map((id) => (id === me ? myName || id : chat.nicknameOf(id))), MENTION_ALL_TOKEN];
});

/**
 * 当前会话第一条未读在**本列表（已过滤）**里的下标。
 * store 侧算它时用的就是上面这个 `messages` 的同一条判据（`isRenderedInTimeline`），
 * 所以这里直接拿来用，不再二次换算 —— 坐标系只有一份（审计阶段 4 · 4.1-1）。
 */
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
  if (!convId || (!isPeerFriend.value && !isSelfChat.value)) return;
  try {
    await chat.send(convId, content, kind);
  } catch (e) {
    app.toastError(e, t("msg.sendFailed"));
  }
}

/** 粘贴图片：走 save_outgoing_image → 文件传输，data URL 不进入 SQLite。 */
async function onSendImage(file: File) {
  // ⚠️ 会话必须在**任何 await 之前**取：粘贴发生在哪个会话，图片就该发到哪个会话。
  // 旧实现是 Composer 先 await 完 FileReader 再 emit，这里读到的 `activeConv`
  // 已经是"读取期间被切走之后"的那个会话 ⇒ 图片发给了另一个人（审计阶段 4 · 4.1-5）。
  const convId = chat.activeConv;
  if (!convId || !isPeerFriend.value) return;
  try {
    // 读取与落盘都在这一侧做；`sendImage` 内部只把**发送**两段包了 try，
    // `save_outgoing_image` 的 reject（超 8MiB / 非法 MIME / 解码失败）以前会一路无人
    // 接手 —— 用户看到的就是"按了 Ctrl+V 什么也没发生"。
    const dataUrl = await fileToDataUrl(file);
    await chat.sendImage(convId, dataUrl);
  } catch (e) {
    app.toastError(e, t("msg.sendFailed"));
  }
}

// ---------------- 引用 / 转发 ----------------
/** 待引用消息（MessageItem 右键"引用"设置，随发送或手动取消清除）。 */
const quote = ref<{ sender: string; snippet: string; msgId: string | number } | null>(null);

/** 转发弹窗状态（MessageItem 右键"转发"设置）。文件消息带本地路径，转发即重发文件。 */
const forward = ref<{ kind: MsgKind; content: string; snippet: string; filePath?: string } | null>(null);

/** 点击引用块定位原消息：滚动 + 短暂高亮 */
const highlightId = ref<string | number | null>(null);
let highlightTimer = 0;
/**
 * `id` 用 String 比较：msg_id 是字符串，但历史数据/乐观记录里可能是数字 id，
 * 宽松比较能同时覆盖"引用定位"与"搜索定位"两条来源。
 *
 * ⚠️ 目标**不在已加载范围**时要先向上翻历史（与搜索定位同一口径，见下面的 `locateRequest`）：
 * 引用的往往是几十上百条之前的消息，而列表只加载了最近一页 ⇒ 直接找会找不到、只弹一句
 * "原消息在更早的历史里"，用户看到的就是"定位不对/点了没反应"（用户 2026-09-21）。
 */
async function locateMessage(id: string | number) {
  const target = String(id);
  const find = () => messages.value.findIndex((m) => String(m.msg_id ?? m.id) === target);
  let idx = find();
  let guard = 0;
  let loaded = false;
  while (idx < 0 && guard < 20) {
    guard += 1;
    const before = messages.value.length;
    await chat.loadMoreMessages(chat.activeConv ?? "");
    if (messages.value.length === before) break; // 没有更早的历史了
    loaded = true;
    idx = find();
  }
  if (idx < 0) {
    app.toast(t("chat.toast.originalEarlier"), "info");
    return;
  }
  // ⚠️ 刚翻过页 ⇒ **必须等历史渲染进列表再滚**：`loadMoreMessages` 返回时 DOM 还是旧布局，
  //    此刻滚过去会被旧 scrollHeight 夹住 ⇒ 落点偏（用户 2026-09-21：「定位还是错的」，
  //    引用的往往是几十条之前的消息，走的正是这条路径）。与搜索定位同一口径：两次 nextTick。
  if (loaded) {
    await nextTick();
    await nextTick();
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

/**
 * 转发弹窗的落地入口：**单条**走原有路径，**多选**按用户选的模式走批量。
 *
 * 为什么用一个入口而不是两个 `@pick`：弹窗只有一个，来源却有两处（单条转发 / 多选转发），
 * 各绑一个 handler 就得在模板里写 `forward ? a : b` 这种判据 —— 而判据一旦写错，
 * 表现是"多选转发只发了第一条"（`forward` 恰好也非空时）。这里只判一次。
 */
function onForwardPick(convId: string, mode: "per-message" | "merged") {
  if (forwardSelection.value) return doBatchForward(convId, mode);
  return doForward(convId);
}

/**
 * 收藏一条消息。
 *
 * 页面只负责"提示"，内容与媒体副本全在后端决定（见 `add_favorite`）：
 * 前端连 content 都不传，避免收藏夹里存进一份与消息记录不一致的副本。
 * 返回值区分"新收藏"与"早就收过" —— 重复点收藏不该静默无反应，用户会以为没生效。
 */
async function doFavorite(msgId: string) {
  const convId = chat.activeConv;
  if (!convId) return;
  try {
    const added = await chat.addFavorite(msgId, convId);
    app.toast(t(added ? "favorite.added" : "favorite.already"), added ? "success" : "info");
  } catch (e) {
    app.toastError(e, t("favorite.addFail"));
  }
}

// ---------------- 多选（微信式批量操作：转发 / 收藏 / 删除） ----------------
/**
 * 多选态与已选集合放在**会话层**（这里是唯一持有整条消息列表的地方）。
 *
 * 为什么不放在 `MessageItem` 里：VirtualList 会回收滚出视口的行，每条消息各自的 `ref`
 * 一旦被回收，勾选态就丢了；而"已选 N 条"的计数、底部操作条、批量动作也都需要同一份状态。
 */
const multiSelect = ref(false);
const selectedIds = ref<Set<string>>(new Set());
const selectedCount = computed(() => selectedIds.value.size);
/** 多选转发：已选内容先存下来，选完会话再决定逐条还是合并。 */
const forwardSelection = ref<MessageRecord[] | null>(null);
/**
 * 多选转发时**已经在操作条上定好**的转发方式（`null` = 还没定，走弹窗里的老流程）。
 * 与 `forwardSelection` 同生命周期：关掉弹窗/转发完就一起清掉。
 */
const forwardMode = ref<"per-message" | "merged" | null>(null);
/** 合并转发详情弹窗的载荷（null = 未打开）。 */
/** 打开的合并转发卡片详情：content 是快照载荷；senderId 是**卡片发送者**，
 *  卡片里的图片按需拉取（ADR-0019 Phase 3）就以他为对端 —— 多数情况他就是字节持有者
 *  （自己转发的图自己有）。不做全网广播式查找（会把"拥有即授权"变成对全在线节点扫描）。 */
const openMerge = ref<{ content: string; senderId: string } | null>(null);
/** 批量删除的二次确认（本地删除不可逆）。 */
const confirmBatchDelete = ref(false);

function enterMultiSelect(msgId: string) {
  // 微信：从某条消息进入多选时**那一条默认已选中**（否则用户还得再点一下）
  multiSelect.value = true;
  selectedIds.value = new Set([msgId]);
  quote.value = null; // 多选与"引用草稿"是两种意图，不要叠在一起
  app.setMultiSelectActive(true); // 移动端据此隐藏底部 Tab 栏
}

function exitMultiSelect() {
  multiSelect.value = false;
  selectedIds.value = new Set();
  forwardSelection.value = null;
  forwardMode.value = null;
  confirmBatchDelete.value = false;
  app.setMultiSelectActive(false);
}

/**
 * ESC 退出多选（用户 2026-09-21：「我按 ESC 没取消选中啊」——多选是一键进入的，就该能一键退出）。
 *
 * 挂 window 而不是容器：多选态下焦点可能在选中覆盖层按钮、气泡里的链接或输入框里，
 * 容器收不到这个键。监听常驻、只在多选态动作，所以不影响其它页面。
 *
 * 三层"不抢"的判据：① 已经有人处理过（`defaultPrevented`，如引用菜单）；
 * ② 别的浮层正开着（右键菜单/表情面板走 `popupRegistry`，转发与删除确认是本组件的两个弹窗）
 * —— 那一次 ESC 归它们，再按一次才退多选；③ 本来就 0 条选中也照样退（"退出"是纯 UI 动作）。
 */
function onGlobalKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.defaultPrevented || !multiSelect.value) return;
  if (activePopupKey() !== null || forward.value || confirmBatchDelete.value) return;
  exitMultiSelect();
}

onMounted(() => window.addEventListener("keydown", onGlobalKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onGlobalKey));

function toggleSelect(msgId: string) {
  const next = new Set(selectedIds.value);
  if (next.has(msgId)) next.delete(msgId);
  else next.add(msgId);
  selectedIds.value = next;
}

/** 已选消息，**按会话内顺序**（勾选顺序不该影响合并卡片里的先后）。 */
const selectedMessages = computed(() => messages.value.filter((m) => selectedIds.value.has(m.msg_id)));

/** 合并转发卡片里"谁说的"：自己用本机昵称，别人用好友/群成员昵称。 */
function senderOf(rec: MessageRecord): string {
  const myId = app.device?.device_id;
  if (myId && rec.sender_id === myId) return app.device?.nickname || t("common.me");
  return chat.nicknameOf(rec.sender_id);
}

/** 文件消息的本地路径（内容 JSON 里的 `path`；乐观消息可能还没有）。 */
function filePathOf(rec: MessageRecord): string {
  try {
    return (JSON.parse(rec.content) as { path?: string }).path ?? "";
  } catch {
    return "";
  }
}

/**
 * 逐条转发一条：**媒体（图片 / 文件）按本地路径重走传输链路**，其余按内容重发。
 *
 * 图片必须和文件走同一条路子，不能只做 `file` 分支：两者的 `content` 都只是**元信息**
 * （`{name,size,subtype,path,…}`），其中的 `path` 是**本机**落盘路径。把 content 原样
 * 复制进新消息，接收方拿到的就是一条指向别人磁盘的路径 —— 后端 `resolve_media_path`
 * 的安全校验必然判 Gone，图片于是显示成「已被清理」/空白（用户 2026-09-17 报的
 * "图片多选的预览有问题"）。重走传输链路则由后端重建元信息：
 * `classify_file_subtype(name)` 判回 `kind="image"`，path 换成**接收方自己的**落盘路径。
 */
async function forwardOne(convId: string, m: MessageRecord) {
  if (m.kind === "file" || m.kind === "image") {
    const path = filePathOf(m);
    if (!path) {
      app.toast(t(m.kind === "image" ? "chat.toast.imageNotForward" : "chat.toast.fileNotForward"), "info");
      return;
    }
    if (convId.startsWith("group:")) await chat.sendGroupFileTo(convId.slice(6), path);
    else await chat.sendFileTo(convId, path);
    return;
  }
  await chat.send(convId, m.content, m.kind);
}

/**
 * 多选转发：**转发方式先选**（操作条上就是「逐条转发 / 合并转发」两个按钮），
 * 进弹窗只剩"选会话"这一步。
 *
 * 用户 2026-09-21：「桌面版的合并转发和逐条转发放在外面，不要先转发再选合并转发还是逐条转发」
 * —— 旧流程是 [转发] → 弹窗 → 选会话 → 再选方式，四步；现在方式在条上直选，弹窗直接落定。
 */
function startBatchForward(mode: "per-message" | "merged") {
  if (!selectedCount.value) return;
  forwardMode.value = mode;
  forwardSelection.value = selectedMessages.value;
}

/**
 * 多选转发：`per-message` = 逐条（媒体会真的重发）；`merged` = 合并成一张聊天记录卡片
 * （媒体只带元信息，见 `buildMergePayload` 的说明）。
 */
async function doBatchForward(convId: string, mode: "per-message" | "merged") {
  const items = forwardSelection.value ?? [];
  forwardSelection.value = null;
  forwardMode.value = null;
  if (!items.length) return;
  if (mode === "merged" && items.length > MAX_MERGE_ITEMS) {
    app.toast(t("multi.tooMany", { n: MAX_MERGE_ITEMS }), "error");
    return;
  }
  try {
    if (mode === "merged") {
      const title = isGroup.value
        ? t("merge.titleGroup")
        : t("merge.titleSingle", { name: chat.activeConversation?.name ?? "" });
      await chat.send(convId, buildMergePayload(items, title, senderOf), "merge");
    } else {
      for (const m of items) await forwardOne(convId, m);
    }
    app.toast(t("chat.toast.forwarded"), "success");
    exitMultiSelect();
  } catch (e) {
    app.toastError(e, t("chat.toast.forwardFail"));
  }
}

/** 批量收藏：逐条落库（后端幂等），结果按"新增 / 已在收藏 / 失败"分别报出来。 */
async function batchFavorite() {
  const convId = chat.activeConv;
  if (!convId || !selectedCount.value) return;
  const ids = selectedMessages.value.map((m) => m.msg_id);
  try {
    const r = await chat.addFavorites(ids, convId);
    const parts = [t("multi.favoriteDone", { n: r.added })];
    if (r.already) parts.push(t("multi.favoriteAlready", { n: r.already }));
    if (r.failed) parts.push(t("multi.favoriteFailed", { n: r.failed }));
    app.toast(parts.join("，"), r.failed ? "error" : "success");
    exitMultiSelect();
  } catch (e) {
    app.toastError(e, t("favorite.addFail"));
  }
}

/** 批量删除（本地删除，微信语义：对方那边照常保留）。 */
async function doBatchDelete() {
  confirmBatchDelete.value = false;
  const convId = chat.activeConv;
  if (!convId || !selectedCount.value) return;
  const ids = selectedMessages.value.map((m) => m.msg_id);
  try {
    const n = await chat.deleteMessages(convId, ids);
    app.toast(t("multi.deleted", { n }), "success");
    exitMultiSelect();
  } catch (e) {
    app.toastError(e, t("multi.deleteFail"));
  }
}

/** 统一发送文件：自动路由（直连优先，弱网/无直连自动中继），无需用户选择。 *  群聊会话走群文件链路（send_group_file：Offer → Chunk → Done → CompleteAck）。 */
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
  // multiple: true → 支持一次选多个文件（用户 2026-09-19：「只能一个一个发」）
  const pickedRaw = await openDialog({ multiple: true });
  // Tauri plugin-dialog 的 open 在 multiple=true 时返回 string[]，false 时返回 string；
  // 统一成数组处理，单次/多次走同一条发送逻辑。
  const pickedList: string[] = Array.isArray(pickedRaw) ? pickedRaw : pickedRaw ? [pickedRaw] : [];
  if (pickedList.length === 0) return;
  // 🟦 并发限制 2：Android ART heap 通常只有 256MB，一个 10MB 文件 import + sha256 + send
  // 全链路同时占 ~40MB，并发 3 很容易 OOM（真机：5 张图一起发直接 FATAL OutOfMemoryError）。
  // 并发 2 是 WhatsApp/Telegram 国产 Android 版的保守值，iOS 版可以并发 4+（512MB+ heap）。
  const CONCURRENCY = 2;
  let cursor = 0;
  const workers = Array.from(
    { length: Math.min(CONCURRENCY, pickedList.length) },
    async () => {
      while (cursor < pickedList.length) {
        const i = cursor++;
        try {
          // ⚠️ Android 的选择器给的是 `content://` URI，直接丢给后端发送必然失败
          //（`std::fs` 打不开 URI）—— 这个命令在安卓上把它复制进缓存并返回真实路径，桌面端原样返回。
          const local = await api.importPickedFile(pickedList[i]);
          await sendOneFile(convId, local);
        } catch (e) {
          // 单个失败不阻塞其余 — 后端 send_file 会把失败消息写入 failed 状态
          // （见 useChatStore.sendFileTo 的 catch），用户能看到哪一条失败了。
          console.error(`[attachFile] #${i} failed:`, e);
        }
      }
    },
  );
  await Promise.allSettled(workers);
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

/** 准入条件与附件按钮一致：好友单聊 / 群聊才允许拖进来发；且群任务表单没在接拖放。 */
function canDropInto() {
  // 群任务表单的图片投放区命中时，拖放归它（否则同一份文件既进任务又被当聊天附件发出去）
  return !!chat.activeConv && (isGroup.value || isPeerFriend.value) && !app.boardDropActive;
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
      :peer-version-newer="peerVersionNewer"
      :link-state="linkState"
      :member-count="memberCount"
      :can-rename="canRename"
      :show-back="app.isMobile"
      :unread-total="chat.totalUnread"
      @back="app.mobileView = 'list'"
      @open-members="membersOpen = true"
      @open-files="filesOpen = true"
      :tasks-opening="tasksOpening"
            :open-tasks="openTasksForActive"
      @open-tasks="openTasks()"
      @rename="membersOpen = true"
      @open-share="emit('open-share')"
    />

    <!-- 群公告条：**有公告才出现**（用户 2026-09-17：没有公告不该常驻一条空横幅；
         发布/修改入口在「成员管理」弹窗里）。浅警告底 + 描边 + 圆角，点正文开全文弹窗。 -->
    <div
      v-if="isGroup && announcement"
      class="mx-2 mt-1 flex shrink-0 items-start gap-2 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-warning-soft)] bg-[color-mix(in_srgb,var(--gosslan-warning)_8%,transparent)] px-3 py-2"
    >
      <Megaphone class="mt-0.5 h-3.5 w-3.5 shrink-0 text-[var(--gosslan-warning-ink)]" aria-hidden="true" />
      <button
        class="tap-safe min-w-0 flex-1 truncate rounded-[var(--gosslan-radius-sm)] px-1.5 py-0.5 text-left text-[12px] text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
        :title="announcement.text"
        @click="announceViewOpen = true"
      >
        {{ announcement.text }}
      </button>
    </div>

    <!-- 置顶条：钉钉/飞书同款位置（头部下方）。
         条数的**边界按端给**：移动端一行放不下三个（每个都会被压成省略号），只显示最近 1 条；
         桌面最多 3 条。超出部分用 `+N` 就地展开成纵向列表（带滚动上限），
         而不是挤在同一行 —— 否则置顶一多，置顶条自己就把消息区吃掉了。 -->
    <div
      v-if="isGroup && pinnedItems.length"
      class="flex shrink-0 flex-col border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)]"
    >
      <div class="flex items-center gap-2 px-4 py-1.5">
        <Pin class="h-3.5 w-3.5 shrink-0 text-[var(--gosslan-text-2)]" aria-hidden="true" />
        <button
          v-for="p in visiblePinned"
          :key="p.id"
          class="tap-safe group/pin flex min-w-0 flex-1 items-center gap-1 rounded-[var(--gosslan-radius-sm)] px-1.5 py-0.5 text-left text-[12px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
          :title="p.text"
          @click="gotoPinned(p.id)"
        >
          <span class="min-w-0 flex-1 truncate" :title="p.text">{{ p.text }}</span>
          <!-- 就地取消置顶：不必先跳到原消息再右键（用户明确要求） -->
          <span
            class="hover-reveal-op flex h-4 w-4 shrink-0 items-center justify-center rounded-full opacity-0 transition group-hover/pin:opacity-100"
            role="button"
            :title="t('msg.unpin')"
            :aria-label="t('msg.unpin')"
            @click.stop="togglePin(p.id)"
          >
            <X class="h-3 w-3" />
          </span>
        </button>
        <button
          v-if="pinnedItems.length > visiblePinned.length"
          class="tap-safe shrink-0 rounded-[var(--gosslan-radius-sm)] px-1.5 py-0.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
          @click="pinsExpanded = !pinsExpanded"
        >
          {{ pinsExpanded ? t("msg.pinCollapse") : `+${pinnedItems.length - visiblePinned.length}` }}
        </button>
      </div>
      <!-- 展开态：纵向列出全部置顶，带高度上限（置顶再多也不会吃掉消息区） -->
      <div v-if="pinsExpanded" class="max-h-32 overflow-y-auto px-4 pb-1.5">
        <button
          v-for="p in pinnedItems"
          :key="`all-${p.id}`"
          class="tap-safe flex w-full items-center gap-2 rounded-[var(--gosslan-radius-sm)] px-1.5 py-1 text-left text-[12px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
          :title="p.text"
          @click="gotoPinned(p.id)"
        >
          <span class="min-w-0 flex-1 truncate" :title="p.text">{{ p.text }}</span>
          <X
            class="h-3 w-3 shrink-0"
            role="button"
            :aria-label="t('msg.unpin')"
            @click.stop="togglePin(p.id)"
          />
        </button>
      </div>
    </div>

    <!-- 蓝牙链路速度提示（用户 2026-09-13 要求）：蓝牙分片载荷受 20 字节 MTU 限制，
         实测吞吐约 1 KB/s，一张 500 KB 的图片要几分钟。让用户在大文件开始**之前**
         就有预期，而不是看着进度条一直不动以为卡死。可关闭，切换会话后重新提示。 -->
    <div
      v-if="btLink && !btHintDismissed"
      class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-warning-soft)] px-4 py-1.5 text-[12px] leading-[18px] text-[var(--gosslan-warning-ink)]"
    >
      <Bluetooth class="h-3.5 w-3.5 shrink-0" />
      <span class="min-w-0 flex-1">{{ t("chat.bt.slowHint") }}</span>
      <button
        class="tap-safe shrink-0 rounded-[var(--gosslan-radius-sm)] px-1.5 py-0.5 transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.bt.dismiss')"
        :aria-label="t('chat.bt.dismiss')"
        @click="btHintDismissed = true"
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>

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
            :self-mention="selfMention"
            :reactions="reactionMap.get(item.msg_id) ?? []"
            :pinned="pinnedIds.includes(item.msg_id)"
            :select-mode="multiSelect"
            :selected="selectedIds.has(item.msg_id)"
            @quote="quote = $event"
            @react="toggleReaction(item.msg_id, $event)"
            @pin="togglePin(item.msg_id)"
            @forward="forward = $event"
            @favorite="doFavorite(item.msg_id)"
            @multi-select="enterMultiSelect(item.msg_id)"
            @toggle-select="toggleSelect(item.msg_id)"
            @open-merge="openMerge = $event"
            @locate="locateMessage"
            @open-image="openImageAt"
            @open-tasks="openTasks($event)"
            :todo-live-status="todoLiveStatus"
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

    <!-- 输入区：浅灰底上放一个白底圆角卡片，无顶部分割线。
         移动端底部只留 safe-area-inset-bottom（手机圆角/Home Indicator 区域），
         不要多余 padding — TabBar 在 ChatWindow 打开时已隐藏（mobileView='chat'），
         之前 pb-3 留了 12px 但没有 safe-area-inset，手机底部输入框会被圆角区域压住。 -->
    <!-- Material 3 规范：组件外最小 padding 8dp (pt-2)，safe-area 下额外 8px breathing room
         （Chrome 135 edge-to-edge 推荐的最小值，Android 15+ 强制 edge-to-edge 后手势导航条必留）。 -->
    <div class="shrink-0 bg-[var(--gosslan-chat)] px-4 pt-2" :style="{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 8px)' }">
      <!-- 多选态：输入区被操作条**替换**（微信同款）。
           高度固定 4rem，与 Composer 的最小高度一致 —— 否则进出多选时消息区高度跳变，
           虚拟列表会跟着滚一下。 -->
      <div
        v-if="multiSelect"
        class="flex h-16 items-center justify-between gap-2 rounded-[var(--gosslan-bubble-radius)] bg-[var(--gosslan-panel)] px-3"
      >
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          @click="exitMultiSelect"
        >
          {{ t("common.cancel") }}
        </button>
        <span class="text-[13px] text-[var(--gosslan-text-2)]">
          {{ t("multi.selected", { n: selectedCount }) }}
        </span>
        <div class="flex items-center gap-1">
          <button
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-2.5 py-1.5 text-[13px] text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)] disabled:opacity-40"
            :disabled="!selectedCount"
            :title="t('multi.forwardPerMessage')"
            :aria-label="t('multi.forwardPerMessage')"
            @click="startBatchForward('per-message')"
          >
            <Share2 class="h-4 w-4" aria-hidden="true" />
            <span class="hidden sm:inline">{{ t("multi.forwardPerMessage") }}</span>
          </button>
          <button
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-2.5 py-1.5 text-[13px] text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)] disabled:opacity-40"
            :disabled="!selectedCount"
            :title="t('multi.forwardMerged')"
            :aria-label="t('multi.forwardMerged')"
            @click="startBatchForward('merged')"
          >
            <Layers class="h-4 w-4" aria-hidden="true" />
            <span class="hidden sm:inline">{{ t("multi.forwardMerged") }}</span>
          </button>
          <button
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-2.5 py-1.5 text-[13px] text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)] disabled:opacity-40"
            :disabled="!selectedCount"
            @click="batchFavorite"
          >
            <Star class="h-4 w-4" aria-hidden="true" />
            {{ t("multi.favorite") }}
          </button>
          <button
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-2.5 py-1.5 text-[13px] text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-hover)] disabled:opacity-40"
            :disabled="!selectedCount"
            @click="confirmBatchDelete = true"
          >
            <Trash2 class="h-4 w-4" aria-hidden="true" />
            {{ t("multi.delete") }}
          </button>
        </div>
      </div>
      <MessageComposer
        v-else-if="isGroup || isPeerFriend || isSelfChat"
        :key="chat.activeConv ?? 'none'"
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
    <GroupMemberPanel
      :open="membersOpen"
      :group-id="activeGroupId"
      @close="membersOpen = false"
      @open-tasks="membersOpen = false; openTasks()"
      :tasks-opening="tasksOpening"
            :open-tasks="openTasksForActive"
    />

    <!-- 发布/修改群公告已并入「成员管理」弹窗（用户 2026-09-17：公告与群名/成员一起管）；
         这里只保留公告**全文查看 + 删除**（点横幅正文打开）。 -->

    <!-- 公告全文：点横幅正文打开（此前是 toast，长公告看不全） -->
    <BaseModal
      :open="announceViewOpen && !!announcement"
      :title="t('group.announce')"
      @close="announceViewOpen = false"
    >
      <template v-if="announcement">
        <p class="whitespace-pre-wrap break-words text-sm leading-relaxed text-[var(--gosslan-text)]">
          {{ announcement.text }}
        </p>
        <!-- 群主：删除（两段式确认，渲染在弹窗内 ⇒ 不会被遮罩挡住） -->
        <div v-if="canPublishAnnouncement" class="mt-5 flex items-center justify-end gap-2 border-t border-[var(--gosslan-divider)] pt-3">
          <button
            v-if="!announceDeleteArmed"
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-danger-soft)] hover:text-[var(--gosslan-danger-ink)]"
            @click="announceDeleteArmed = true"
          >
            <Trash2 class="h-3.5 w-3.5" />
            {{ t("group.announceDeleteTitle") }}
          </button>
          <template v-else>
            <span class="mr-auto text-[12px] text-[var(--gosslan-danger-ink)]">{{ t("group.announceDeleteBody") }}</span>
            <button
              class="tap-safe rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="announceDeleteArmed = false"
            >
              {{ t("common.cancel") }}
            </button>
            <button
              class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-3 py-1.5 text-[13px] text-white transition hover:bg-[var(--gosslan-danger-hover)]"
              @click="deleteAnnouncement"
            >
              {{ t("common.delete") }}
            </button>
          </template>
        </div>
      </template>
    </BaseModal>

    <!-- 群文件列表 -->
    <GroupFilesPanel :open="filesOpen" :group-id="activeGroupId" @close="filesOpen = false" />

    <!-- 群任务（Card kind：不进时间线，只在这个面板里折叠展示） -->
    <GroupTasksPanel
      :open="tasksOpen"
      :group-id="activeGroupId"
      :focus-todo-id="taskFocusId"
      @close="tasksOpen = false; taskFocusId = null"
    />

    <!-- 转发弹窗 -->
    <ForwardModal
      v-if="forward || forwardSelection"
      :open="true"
      :kind="forward?.kind ?? 'text'"
      :snippet="forward?.snippet ?? ''"
      :count="forwardSelection?.length ?? 0"
      :mode="forwardMode ?? undefined"
      @close="forward = null; forwardSelection = null; forwardMode = null"
      @pick="onForwardPick"
    />

    <!-- 合并转发卡片详情 -->
    <MergeCardModal
      :open="!!openMerge"
      :content="openMerge?.content ?? ''"
      :sender-id="openMerge?.senderId ?? ''"
      @close="openMerge = null"
    />

    <!-- 批量删除的二次确认：本地删除不可逆（微信也是"删除后无法恢复"） -->
    <BaseModal :open="confirmBatchDelete" :title="t('multi.delete')" @close="confirmBatchDelete = false">
      <p class="text-sm leading-relaxed text-[var(--gosslan-text)]">
        {{ t("multi.deleteConfirm", { n: selectedCount }) }}
      </p>
      <div class="mt-5 flex justify-end gap-2">
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] px-4 py-2 text-sm text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          @click="confirmBatchDelete = false"
        >
          {{ t("common.cancel") }}
        </button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-2 text-sm text-white transition hover:bg-[var(--gosslan-danger-hover)]"
          @click="doBatchDelete"
        >
          {{ t("common.delete") }}
        </button>
      </div>
    </BaseModal>


    <!-- 图片相册预览（会话内全部图片，左右箭头 / 键盘 ←→ 切换） -->
  </div>
</template>
