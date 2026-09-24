<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref, onMounted, onUnmounted, watch, watchEffect } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api, APP_ACTION, bindMenuEvents } from "@/api";
import { launchAuxWindow, useWindowOpening } from "@/composables/useWindowLauncher";
import { useShortcuts } from "@/composables/useShortcuts";
import { useBackLayer } from "@/composables/useBackLayer";
import type { UnlistenFn } from "@tauri-apps/api/event";
import NavRail from "@/components/NavRail.vue";
import TitleBar from "@/components/TitleBar.vue";
import ConversationList from "@/components/ConversationList.vue";
import ChatWindow from "@/components/ChatWindow.vue";
import UnreadBadge from "@/components/UnreadBadge.vue";
import FriendProfile from "@/components/FriendProfile.vue";
import FriendRequestList from "@/components/conversation/FriendRequestList.vue";
import SettingsPanel from "@/components/SettingsPanel.vue";
import AddFriendModal from "@/components/AddFriendModal.vue";
import ChatSearchDialog from "@/components/search/ChatSearchDialog.vue";
import GroupCreateModal from "@/components/GroupCreateModal.vue";
import ShareDirectory from "@/components/ShareDirectory.vue";
import LogViewer from "@/components/LogViewer.vue";
import FavoritePanel from "@/components/FavoritePanel.vue";
import ToastHud from "@/components/ToastHud.vue";
import LinksList from "@/components/LinksList.vue";
import MobilePageFrame from "@/components/MobilePageFrame.vue";
import ProfileSection from "@/components/settings/ProfileSection.vue";
import { Compass, MessageCircle, ScrollText, Settings, Star, UserCircle, Users } from "lucide-vue-next";
import type { ExternalLink, Friend, PendingRequest } from "@/types";
const app = useAppStore();
const chat = useChatStore();

/**
 * 导航状态——**单一数据源**，替代之前分散的 `view` + `favoritesOpen`。
 *
 * 之前的问题：`view = "chats"` 和 `favoritesOpen = true` 可以**同时为真**，
 * 导致 NavRail 聊天按钮和收藏按钮一起高亮（用户反馈 2026-09-20「多选了侧边栏的按钮」）。
 * 现在四个枚举值互斥，NavRail 只读这一个值决定高亮，不会再出现双选中。
 *
 * 收藏关闭后（如用户点其他 rail 按钮 / 打开会话）自动回到 `chats`，
 * 确保左中右三列始终同步。
 */
type NavState = "chats" | "contacts" | "links" | "favorites" | "me";
const navState = ref<NavState>("chats");

/**
 * 喂给 `ConversationList` 的视图：它只认 chats / contacts。
 * `links` / `favorites` 时左列换成 `LinksList` / 整个隐藏，
 * 但 `ConversationList` 仍按 `chats` 挂着（不卸载 = 保留滚动位置与搜索框状态）。
 */
const listView = computed<"chats" | "contacts">(() => {
  const s = navState.value;
  return s === "contacts" ? "contacts" : "chats";
});
/** 收藏打开时左列隐藏（整页态），links 时换成 LinksList。 */
const favoritesOpen = computed(() => navState.value === "favorites");
const isLinksView = computed(() => navState.value === "links");

/**
 * 移动端「列表 ↔ 详情」平移动画开关。
 *
 * ⚠️ 只在**同一个 tab 内** list↔detail（点会话 / 点好友进详情、再返回）时平移；
 * **底部 tab 切换必须瞬时切换、不做转场**（用户 2026-09-20：「tab 切换不要转场啊，
 * 我要的是从列表点进详情的这种」——上一版按 `mobileView` 无脑动画，切「我的 / 收藏」
 * 也整屏滑，是错的）。
 *
 * 实现：所有 **tab 选择**都走 `selectMobileTab`（它会先在**本帧**关掉 transform 过渡、
 * 让面板瞬时到位，下一帧再恢复）；其余 `mobileView` 改动（`openConversation` /
 * `openFriendProfile` / `openRequests` / 各类返回）直接赋值，于是照常平移。
 */
const paneAnimate = ref(true);

/**
 * 移动端 tab 选择：瞬时切换面板，不做 list↔detail 平移。
 * 双 `requestAnimationFrame` 保证「无过渡」的那一帧真的被绘制过 ——
 * 单 rAF 可能与本次 DOM 改动合并到同一帧，过渡仍会被触发。
 */
function selectMobileTab(nav: NavState, mv: "list" | "chat") {
  paneAnimate.value = false;
  navState.value = nav;
  app.mobileView = mv;
  requestAnimationFrame(() => requestAnimationFrame(() => (paneAnimate.value = true)));
}
/** 正在查看资料的好友（通讯录点击好友 → 展示资料页，而非直接开会话） */
const profileFriend = ref<Friend | null>(null);
const settingsOpen = ref(false);
/** 移动端资料页（可编辑资料，复用 ProfileSection）。桌面端无独立资料窗口，回落到设置。 */
const profileOpen = ref(false);
const profileReloadToken = ref(0);
const addFriendOpen = ref(false);
const groupOpen = ref(false);
const shareOpen = ref(false);
const logsOpen = ref(false);

/**
 * 移动端底部 TabBar 是否显示 —— **单一事实来源**，模板里的 TabBar `v-if`
 * 与主内容区的 `pb-[calc(4rem+…)]` 占位都读它（两处各写一份就会漂移：
 * 一边隐藏、另一边还留着内边距 → 底部空一大截，用户 2026-09-19）。
 *
 * 只在**四个一级 tab** 上显示：聊天 / 通讯录列表（`mobileView === 'list'`）、收藏、我的。
 * 进 Chat 详情、设置、日志、链接 等二级页时整个隐藏。收藏必须保留（用户 2026-09-20）。
 */
const showMobileTabBar = computed(
  () =>
    app.isMobile &&
    !app.keyboardOpen &&
    !app.multiSelectActive &&
    (app.mobileView === "list" || favoritesOpen.value || navState.value === "me") &&
    !settingsOpen.value &&
    !logsOpen.value,
);

/** 关闭收藏页。**回到 chats 而不是任意之前的值**——收藏是临时整页，
 * 微信关闭收藏后默认回到聊天列表，不猜用户之前在哪个 tab。 */
function closeFavorites() {
  // 收藏是 tab，关闭走 tab 选择：瞬时切回聊天列表，不做平移（见 `selectMobileTab`）。
  selectMobileTab("chats", "list");
}

/** NavRail 切换：直接设 navState（四选一枚举）。 */
function onUpdateNavState(v: NavState) {
  navState.value = v;
  // 移动端：切到发现页（links/favorites）→ 让 main 区全屏显示（aside 滑到左边）
  // 切回 chats/contacts → 回到列表视图
  if (app.isMobile) {
    if (v === "links" || v === "favorites") {
      app.mobileView = "chat";
    } else {
      app.mobileView = "list";
    }
  }
  // 切到聊天时也清掉会话打开状态——避免"左侧聊天列表，右侧还停着旧会话"
  if (v === "chats" && chat.activeConv) void chat.openConversation(chat.activeConv);
}

/**
 * 打开设置。
 * 桌面端：**独立窗口**（用户 2026-09-12 反馈：「PC 端的设置页面可以按照这种布局，
 * 弹一个单独的窗口」——参考图是左侧窄导航 + 右侧内容的设置窗口，不是盖在聊天上的弹窗）。
 * 移动端：整页设置（`SettingsPanel` 的全屏分支，iOS 标准）。
 * 独立窗口不可用时**回退到应用内弹窗**，保证「设置」在任何环境下都打得开。
 */
function openSettings() {
  if (app.isMobile) {
    // ⚠️ **不要**在这里改 `app.mobileView`：设置是整页浮层，盖在当前页面之上；
    // 一旦改成 "list"，用户从「聊天」里打开设置、返回时就会落到会话列表，
    // 而不是回到进入前的页面（用户 2026-09-12 晚 #1：「点击返回的话，
    // 就是返回到进入之前的上一个页面」）。保持底层视图不动，返回即还原。
    settingsOpen.value = true;
    return;
  }
  // 独立窗口：单飞 + 连点防抖（`useWindowLauncher`）。窗口实例唯一性由后端保证。
  void launchAuxWindow("settings", () => api.openSettingsWindow()).catch(() => {
    // 独立窗口开不出来（能力缺失 / 创建失败）时回退到应用内设置页：
    // 「设置」在任何环境下都必须打得开，宁可退化成弹窗也不能点了没反应。
    settingsOpen.value = true;
  });
}

/** 打开运行日志：桌面端开独立窗口，移动端跳全屏页面（带返回）。 */
function openLogs() {
  if (app.isMobile) {
    logsOpen.value = true;
    return;
  }
  void launchAuxWindow("logs", () => api.openLogWindow()).catch((e) =>
    app.toastError(e, t("common.operationFail")),
  );
}

/**
 * 打开资料页。
 * 移动端：整页可编辑资料（头像 / 昵称 / 状态），复用 `ProfileSection`，从「我的」下钻进来。
 * 桌面端没有独立的资料窗口——资料就是设置第一项（通用 → 资料），直接进设置即可。
 */
function openProfile() {
  if (app.isMobile) {
    profileOpen.value = true;
    return;
  }
  openSettings();
}

/** 移动端链接页返回：链接是从「我的」下钻进来的，返回即回到「我的」页。 */
function onLinksBack() {
  navState.value = "me";
}

/**
 * 在独立窗口里打开一条外部链接（桌面端）。
 *
 * ⚠️ 链接窗口加载的是**远端页面**、且刻意不在 capabilities 里（远端拿不到任何命令权限，
 * 见 Rust `open_link_window`/`link_window_is_not_capability_covered`）。这里只负责触发；
 * 同一窗口再次打开会 `navigate` 到新网址（不会越开越多）。
 */
function openLink(link: ExternalLink) {
  void launchAuxWindow("link", () => api.openLinkWindow(link.url, link.name)).catch((e) =>
    app.toastError(e, t("links.openFail")),
  );
}

/** 按钮 pending 反馈：正在打开时按钮显示忙碌态（冷启动那一下用户能立刻看到"点到了"）。 */
const settingsOpening = useWindowOpening("settings");
const logsOpening = useWindowOpening("logs");
const linksOpening = useWindowOpening("link");

/** 搜索聊天记录结果页的开关与初始关键词（由会话列表搜索框回车触发）。 */
const searchOpen = ref(false);
const searchSeed = ref("");

function openSearchHistory(keyword: string) {
  searchSeed.value = keyword;
  searchOpen.value = true;
}

/**
 * 从结果页「进入聊天」：打开该会话并**跳到命中那一条**。
 * 跳转复用 `locateMessageInConv`（它会翻页直到找到那条消息），失败时提示而不是静默
 * —— 用户点"进入聊天"就是想看那条，没跳到会以为功能坏了。
 */
async function onOpenSearchHit(payload: { convId: string; msgId: string }) {
  searchOpen.value = false;
  if (app.isMobile) app.mobileView = "chat";
  const r = await chat.locateMessageInConv(payload.convId, payload.msgId);
  if (r !== "found") app.toast(t("search.locateFail"), "error");
}

function openFriendProfile(f: Friend) {
  profileFriend.value = f;
  showRequests.value = false;
  if (app.isMobile) app.mobileView = "chat";
}

/** 「新的朋友」页：右侧主区展示好友申请列表（微信式） */
const showRequests = ref(false);

function openRequests() {
  showRequests.value = true;
  profileFriend.value = null;
  // ⚠️ 移动端：申请页渲染在**右侧主面板**里，而移动端靠 `mobileView` 平移切换面板 ——
  // 不切过去的话用户还停在会话列表上，表现就是「点了『新的朋友』没反应」
  // （用户 2026-09-12 安卓实测）。与 `openFriendProfile` / `openSearchHistory` 同一处理。
  if (app.isMobile) app.mobileView = "chat";
}

/** 收起「新的朋友」页：有会话在聊时切回「聊天」tab，
 *  否则会出现右侧在聊、左侧还停在通讯录的错位。 */
function closeRequests() {
  showRequests.value = false;
  if (chat.activeConv) navState.value = "chats";
  // 移动端返回会话列表（iOS push/pop 语义：申请页是从列表推进去的一层）
  if (app.isMobile) app.mobileView = "list";
}

async function acceptRequest(r: PendingRequest) {
  try {
    await chat.respondRequest(r.from, true);
    app.toast(t("layout.toast.accepted", { name: r.from_nickname }), "success");
  } catch (e) {
    app.toastError(e, t("common.operationFail"));
  }
}
async function rejectRequest(r: PendingRequest) {
  try {
    await chat.respondRequest(r.from, false);
    app.toast(t("layout.toast.rejected"), "info");
  } catch (e) {
    app.toastError(e, t("common.operationFail"));
  }
}

/** 资料页「发消息」：回到消息视图并打开与该好友的会话 */
async function sendMessageTo(id: string) {
  profileFriend.value = null;
  navState.value = "chats";
  // ⚠️ 视图切换排在一切 await 之前（用户 2026-09-24 #29「切换会话要瞬间响应，数据后台异步加载」）。
  // `openConversation` 第一行就同步写下 `activeConv`，其后才是骨架 + 异步拉消息；
  // 把 `mobileView` 写在 await 之后 ⇒ 移动端点「发消息」要等完两轮 IPC 才翻页，
  // 点下去那一下毫无反应。与 `ConversationList.openConv` 同一顺序。
  if (app.isMobile) app.mobileView = "chat";
  // 「自己」的会话行可能还不存在：先 ensure（后端有 self 分支，会用本机昵称/头像命名），
  // 否则列表里会显示成一串 gosslan-xxxx。
  if (id === app.device?.device_id) await api.ensureConversation(id);
  await chat.openConversation(id);
}

async function removeFriend(f: Friend) {
  profileFriend.value = null;
  try {
    await chat.removeFriend(f.device_id);
    app.toast(t("conv.toast.friendRemoved", { name: f.nickname }), "info");
  } catch (e) {
    app.toastError(e, t("common.deleteFail"));
  }
}

// 打开会话/切走时收起资料页与新朋友页，避免右侧同时出现多个内容区；
// 会话一旦打开（接受好友申请自动开会话 / 通知点击跳转等），主视图切回「聊天」tab，
// 否则右侧在聊、左侧 tab 还停在通讯录，布局与底部高亮都错位。
// navState 设为 chats 自动意味着 favoritesOpen = false（computed 派生）。
watch(
  () => chat.activeConv,
  (convId) => {
    profileFriend.value = null;
    showRequests.value = false;
    // 切到 links/favorites 时**不要**被强制拉回 chats——发现页是独立导航目标，
    // 即使有活跃会话也应该让用户继续看发现内容。
    if (convId && navState.value !== "links" && navState.value !== "favorites") {
      navState.value = "chats";
    }
  },
);

/**
 * 导航切换 —— 只决定渲染，不碰子状态值。
 *
 * 每个 navState 自带"记忆"：
 *   chats     → activeConv （当前会话）
 *   contacts  → profileFriend + showRequests （上次点开的好友/申请页）
 *   links     → 独立面板，无子状态
 *   favorites → 独立整页，无子状态
 *
 * 导航切换只改变 navState 这个枚举，右侧内容块的 `v-if/v-else-if` 守卫
 * （已经全部加了 `navState === 'xxx' &&` 前缀）会自动决定显不显示。
 * 值不清除 = 下次切回来自动恢复。
 *
 * 示例流程：
 *   1. 在 chats，activeConv = group:abc → 右侧 ChatWindow
 *   2. 切到 contacts，profileFriend = null → 右侧空态好友列表
 *   3. 点好友张三 → profileFriend = 张三 → 右侧 FriendProfile
 *   4. 切回 chats → activeConv 还在 → 右侧 ChatWindow 恢复
 *   5. 再切到 contacts → profileFriend 还在 → 右侧 FriendProfile 张三
 */

// 桌面端默认选中：启动后会话列表就绪且当前无选中会话时，自动打开最近的一个会话，
// 右侧直接进入聊天（对齐微信桌面版行为）；移动端保持列表优先，不自动跳转。
watch(
  () => chat.conversations,
  (convs) => {
    if (app.isMobile) return;
    if (!chat.activeConv && convs.length > 0) void chat.openConversation(convs[0].id);
  },
  { immediate: true },
);

// 通知点击跳转：好友申请通知 → 切换到联系人视图
function onNavigateToContacts() {
  navState.value = "contacts";
  if (app.isMobile) app.mobileView = "list";
}

// ---------------- 应用级动作：原生菜单与键盘快捷键共用 ----------------
// macOS 的菜单栏（src-tauri/src/menu.rs）与下面的快捷键都只"发出意图"，
// 真正的动作在这里执行 —— 保证两条路径行为完全一致。
function onAddFriendAction() {
  addFriendOpen.value = true;
  if (app.isMobile) app.mobileView = "list";
}
useShortcuts();

/**
 * 移动端「聊天页」是一层：系统返回键 → 回到会话列表（而不是退出应用）。
 * 只在移动端且当前在聊天页时压历史条目；`inert` 那条平移面板同样是状态驱动的，
 * 两者一起保证"返回"和"侧滑"语义一致。
 */
useBackLayer(
  () => app.isMobile && app.mobileView === "chat",
  () => {
    app.mobileView = "list";
  },
);

// 「我的」页是一层：系统返回键先离开「我的」回到聊天列表（iOS push/pop 语义，与聊天页一致）。
useBackLayer(
  () => app.isMobile && navState.value === "me",
  () => {
    navState.value = "chats";
    app.mobileView = "list";
  },
);

// 「资料」覆盖层（从「我的」下钻进来）是一层：系统返回键先关掉它，回到「我的」，
// 再按一次才离开「我的」——层级关系与桌面侧"设置二级页"一致。
useBackLayer(
  () => app.isMobile && profileOpen.value,
  () => {
    profileOpen.value = false;
  },
);

let unlistenMenu: UnlistenFn[] | null = null;

// 这些"整页内容"里的任何一个盖上来，聊天就不再可见 ⇒ 同步给 store（判已读/发回执要用）。
// 为什么放在这里、且必须是 watchEffect：它立即执行一次，所以**必须**在所有浮层 ref 声明之后
// （放在前面会撞上 const 的 TDZ）。只有布局层知道这些浮层开没开，store 只保留一个布尔。
watchEffect(() => {
    app.setMobileChatObscured(
      app.isMobile &&
        (settingsOpen.value ||
          logsOpen.value ||
          profileOpen.value ||
          showRequests.value ||
          profileFriend.value !== null ||
          shareOpen.value),
    );
});

onMounted(() => {
  window.addEventListener("navigate-to-contacts", onNavigateToContacts);
  window.addEventListener(APP_ACTION.openSettings, openSettings);
  window.addEventListener(APP_ACTION.addFriend, onAddFriendAction);
  window.addEventListener(APP_ACTION.openLogs, openLogs);
  // 原生菜单（仅 macOS）；非 macOS 平台该 Promise 仍会 resolve，只是收不到事件
  void bindMenuEvents().then((fns) => (unlistenMenu = fns));
});
onUnmounted(() => {
  window.removeEventListener("navigate-to-contacts", onNavigateToContacts);
  window.removeEventListener(APP_ACTION.openSettings, openSettings);
  window.removeEventListener(APP_ACTION.addFriend, onAddFriendAction);
  window.removeEventListener(APP_ACTION.openLogs, openLogs);
  unlistenMenu?.forEach((fn) => fn());
});

// ---------------- 桌面端：列表栏宽度拖拽（rail 64px 固定，列表 200~420px，持久化） ----------------
const RAIL_W = 64;
const LIST_W_MIN = 200;
const LIST_W_MAX = 420;
const listW = ref(
  Math.min(LIST_W_MAX, Math.max(LIST_W_MIN, Number(localStorage.getItem("gosslan-list-w")) || 250)),
);
const resizing = ref(false);

function onResizeStart(e: PointerEvent) {
  // 只认鼠标左键：中键/右键拖拽不该改变列表宽度（右键还会弹系统菜单）
  if (e.button !== 0) return;
  resizing.value = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  document.body.style.cursor = "col-resize";
  document.body.style.userSelect = "none";
}
function onResizeMove(e: PointerEvent) {
  if (!resizing.value) return;
  listW.value = Math.min(LIST_W_MAX, Math.max(LIST_W_MIN, e.clientX - RAIL_W));
}
/**
 * 结束拖拽。除了正常抬手，还要覆盖：指针被系统抢走（`lostpointercapture`）、
 * 指针移出窗口后在其他窗口抬起（收不到 pointerup）——那样 `body` 会残留
 * `col-resize` 与全局 `user-select: none`，用户会以为"界面卡住/选不中字了"。
 */
function onResizeEnd() {
  if (!resizing.value) return;
  resizing.value = false;
  document.body.style.cursor = "";
  document.body.style.userSelect = "";
  localStorage.setItem("gosslan-list-w", String(listW.value));
}
</script>

<template>
    <!-- ⚠️ 根容器**不要**加圆角（曾经是 rounded-[var(--gosslan-radius-lg)]）：
        窗口形状由系统负责——Windows 由 DWM（`shadow:true`，Win11 自动圆角，**最大化时不再圆角**）、
        macOS 由系统窗口圆角。应用再画一层圆角就会与系统边界错位：容器圆角之外那圈露出 body 底色
        （亮色 #edf1f6 ≈ 白），表现为「窗口角上有一道白缝」，关闭键 hover 成红色后尤其刺眼，
        最大化时四角全会出现。圆角交给系统 = 曲线唯一、严丝合缝。 -->
    <div
    class="flex w-full flex-col overflow-hidden bg-[var(--gosslan-caption)] font-gosslan text-[var(--gosslan-text)]"
    :class="app.isMobile
      ? 'h-dvh'
      : 'h-screen ring-1 ring-inset ring-[var(--gosslan-window-ring)]'"
  >
    <!-- 移动端顶部安全区：大圆角/刘海屏下为状态栏留出空间，避免搜索框顶到屏幕外框 -->
    <div v-if="app.isMobile" class="safe-top shrink-0"></div>

    <!-- 顶部 caption：横贯整个窗口（盖在 rail + list + chat 三列之上），微信 4.0 顶部是整条浅灰拖拽条 -->
    <TitleBar :is-mobile="app.isMobile" />

    <!-- 桌面：rail（左）| 列表（中）| 聊天（右）三列；移动端按 mobileView 抽屉切换。
         把可拖拽的列表宽 `listW` 暴露成 CSS 变量：会话列表用内联 width，
         收藏页的左列在 `FavoritePanel` 里（拿不到这个 ref），靠 `var(--gosslan-list-w)` 共享同一宽度
         （`style.css` 里已有同名 token，默认 250px 兜底）。 -->
    <div class="relative flex min-h-0 flex-1 overflow-hidden" :style="{ '--gosslan-list-w': `${listW}px` }">
    <!-- 左侧导航栏：顶格到 caption 之下，浅灰与 caption 一体 -->
    <NavRail
      :nav-state="navState"
      :settings-opening="settingsOpening"
      :logs-opening="logsOpening"
      @update:nav-state="onUpdateNavState"
      @open-settings="openSettings"
      @open-logs="openLogs"
    />

    <!-- 会话列表：桌面宽度可拖拽调（默认250px，持久化）；移动端整屏抽屉。
         收藏是**整页**（参考 PC 微信：点收藏后左侧不再显示聊天/通讯录列表），
         所以桌面端收藏打开时把这一列整个收起，主区（收藏页）顶到 rail 右边。
         ⚠️ 移动端 translate 与下方 main 镜像对称（同用「'list' 且非收藏」作为列表态判据）。
         收藏打开时 aside 必须滑走，否则会盖住 main 上的 FavoritePanel 看不见
         （用户 2026-09-19 Android 实测）—— 本分支把 z 序**反过来**（main z-20 在 aside z-10 之上），
         收藏/详情永远盖住列表，所以只滑 30% 做视差即可、不必整列滑出。
         —— 列表 ↔ 详情用 iOS push/pop：两个面板**一起平移**（列表左移、详情从右滑入），
         而**不是** `hidden` 硬切 —— 硬切在滑动窗口里会露出根节点底色，且只有单边在动，
         观感像"网页换页"（用户 2026-09-20「会话列表点进聊天没转场、好友点资料也没有」）。
         离屏面板用 `inert` 摘掉焦点与交互（键盘 Tab 不进不可见面板）。 -->
    <aside
      v-if="app.isMobile || !favoritesOpen"
      class="h-full shrink-0 overflow-hidden bg-[var(--gosslan-list)]"
      :class="app.isMobile
        ? 'absolute inset-y-0 left-0 z-10 w-full ' +
          (paneAnimate ? 'transition-transform duration-[var(--gosslan-duration)] ease-[var(--gosslan-ease)] ' : '') +
          (app.mobileView === 'list' && !favoritesOpen ? 'translate-x-0' : '-translate-x-[30%]')
        : ''"
      :inert="app.isMobile && app.mobileView === 'chat'"
      :style="app.isMobile ? undefined : { width: `${listW}px` }"
    >
      <!-- 「链接」视图（仅桌面 rail 能切到）：整列换成链接列表；其余视图仍是会话列表。 -->
      <LinksList v-if="!app.isMobile && isLinksView" :opening="linksOpening" @open="openLink" />
      <ConversationList
        v-else
        :view="listView"
        :active-friend-id="profileFriend?.device_id ?? null"
        :requests-active="showRequests"
        @update:view="navState = $event"
        @open-add-friend="addFriendOpen = true"
        @open-group="groupOpen = true"
        @open-friend="openFriendProfile"
        @open-requests="openRequests"
        @search-history="openSearchHistory"
      />
    </aside>

    <!-- 拖拽分隔条：悬浮叠在列表/聊天交界上（负外边距抵消布局宽度），不留缝；
         平时透明，悬停/拖拽时高亮。收藏打开时这一列已收起，这条跟着消失
         （收藏页另有下面那条，见下）。 -->
    <div
      v-if="!app.isMobile && !favoritesOpen"
      class="relative z-10 hidden w-2 cursor-col-resize transition-colors hover:bg-[var(--gosslan-divider)] md:block"
      :class="resizing ? '-mx-1 bg-[var(--gosslan-divider)]' : '-mx-1'"
      @pointerdown="onResizeStart"
      @pointermove="onResizeMove"
      @pointerup="onResizeEnd"
      @pointercancel="onResizeEnd"
      @lostpointercapture="onResizeEnd"
    ></div>

    <!-- 收藏页（整页态）的拖拽条：它的左列渲染在 `FavoritePanel` 里（不是上面的 `<aside>`），
         所以要单独挂一条 —— 绝对定位到「rail + listW」处（main 的左缘就是 rail 右缘），
         复用同一份 `listW` 与拖拽逻辑。这样收藏页的左列宽度与会话列表完全一致、且同样可拖
         （用户 2026-09-20：「宽度和其他页面不一样，而且不能拖动调整」）。 -->
    <div
      v-if="!app.isMobile && favoritesOpen"
      class="absolute inset-y-0 z-10 w-2 cursor-col-resize transition-colors hover:bg-[var(--gosslan-divider)]"
      :class="resizing ? 'bg-[var(--gosslan-divider)]' : ''"
      :style="{ left: `${RAIL_W + listW - 4}px` }"
      @pointerdown="onResizeStart"
      @pointermove="onResizeMove"
      @pointerup="onResizeEnd"
      @pointercancel="onResizeEnd"
      @lostpointercapture="onResizeEnd"
    ></div>

    <!-- 右侧聊天区：面板色差替代分割线；移动端聊天区与列表**一起**平移（iOS push/pop 观感），
         用 transform 而非 hidden。详情在上层（z-20）：列表→详情时从右滑入、列表在下层左移 30%；
         详情→列表时反向。这样滑动窗口里始终有内容，不会露出根节点底色。
         离屏时用 `inert` 摘掉焦点与交互（键盘用户 Tab 不进不可见面板）。
         ⚠️ 主区**一律直角**，不加 `rounded-tl`（用户 2026-09-20：「最右边栏的左上角都有个圆角」）——
         之前主区在非整页态带 `rounded-tl`，会在每个页面主区左上角留一个缺角。 -->
    <main
      class="flex h-full min-w-0 flex-1 flex-col bg-[var(--gosslan-chat)]"
      :class="[
        app.isMobile
          ? 'absolute inset-y-0 left-0 z-20 w-full ' +
            (paneAnimate ? 'transition-transform duration-[var(--gosslan-duration)] ease-[var(--gosslan-ease)] ' : '') +
            (app.mobileView === 'chat' || favoritesOpen ? 'translate-x-0' : 'translate-x-full')
          : '',
      ]"
      :inert="app.isMobile && app.mobileView === 'list' && !favoritesOpen"
    >
      <!-- pb-[calc(4rem+safe-area)] 是给 fixed 定位的 TabBar 留的占位，只有 TabBar 真正**显示**时才需要。
           ⚠️ 这里与下面 TabBar 的 v-if **共用同一个 `showMobileTabBar`**（不要再各写一份条件）：
           两边一旦不一致，TabBar 已隐藏时 pb 还白留着 → 底部空一大截（用户 2026-09-19）。 -->
      <div
        class="min-h-0 flex flex-1 flex-col md:pb-0"
        :class="showMobileTabBar ? 'pb-[calc(4rem+env(safe-area-inset-bottom))]' : ''"
        :style="app.isMobile && app.keyboardInset > 0
          ? { paddingBottom: `${app.keyboardInset + 8}px` }
          : undefined"
      >
        <!-- 发现页（navState='discover'）：纯内容，无额外头部。
             切换入口藏在内容右上角（FavoritePanel 里点 Compass 切链接，反之亦然）。 -->
        <!-- 我的页面（navState='me'） -->
        <template v-if="navState === 'me'">
          <!-- 与聊天/通讯录列表**同族**：列表底 `--gosslan-list`、行悬停 `--gosslan-list-hover`、
               行间内缩分隔线。此前是白底 + `--gosslan-hover` + 无分隔线，和别的列表对不上
               （用户 2026-09-20：「我的也是一样的问题」）。 -->
          <div class="flex h-full flex-col overflow-y-auto bg-[var(--gosslan-list)]">
            <!-- 头部：头像 + 昵称（点进去编辑资料） -->
            <button
              class="flex w-full shrink-0 items-center gap-3 border-b border-[var(--gosslan-divider)] px-4 py-4 text-left transition hover:bg-[var(--gosslan-list-hover)]"
              :aria-label="t('nav.profile')"
              @click="openProfile()"
            >
              <div class="flex h-14 w-14 shrink-0 items-center justify-center overflow-hidden rounded-full bg-[var(--gosslan-primary)]/10 text-2xl text-[var(--gosslan-primary)]">
                <img v-if="app.device?.avatar" :src="app.device.avatar" alt="" class="h-full w-full object-cover" />
                <UserCircle v-else class="h-10 w-10" />
              </div>
              <div class="flex min-w-0 flex-1 flex-col">
                <span class="truncate text-base font-medium" :title="app.device?.nickname || app.device?.device_id || t('nav.me')">{{ app.device?.nickname || app.device?.device_id || t("nav.me") }}</span>
                <span class="truncate text-xs text-[var(--gosslan-text-2)]" :title="app.device?.device_id">{{ app.device?.device_id || "" }}</span>
              </div>
            </button>
            <!-- 菜单列表。不再重复放「资料」：上方头像本身已是资料入口（用户 2026-09-20）。
                 顺序：设置 → 链接 → 日志（链接在日志上面，用户 2026-09-20）。
                 图标与桌面 NavRail 一致用 lucide 线性图标，不再用 📋 emoji（用户 2026-09-20「日志图标好奇怪」）。 -->
            <div class="flex flex-col">
              <button class="flex items-center gap-3 px-4 py-3.5 text-left transition hover:bg-[var(--gosslan-list-hover)]" @click="openSettings()">
                <Settings class="h-5 w-5 text-[var(--gosslan-text-2)]" />
                <span class="flex-1 text-sm">{{ t("nav.settings") }}</span>
              </button>
              <!-- 行间内缩分隔线（从文本列起 = px-4 + 20 图标 + gap-3 = 48px ⇒ ml-12），
                   与聊天/通讯录列表同款；末行不加。
                   ⚠️ 放成**兄弟节点**而不是塞进 `<button>` 里：`<div>` 不是 phrasing content，
                   塞进 button 是非法 HTML。 -->
              <div class="ml-12 h-px shrink-0 bg-[var(--gosslan-divider)]"></div>
              <button class="flex items-center gap-3 px-4 py-3.5 text-left transition hover:bg-[var(--gosslan-list-hover)]" @click="selectMobileTab('links', 'chat')">
                <Compass class="h-5 w-5 text-[var(--gosslan-text-2)]" />
                <span class="flex-1 text-sm">{{ t("nav.links") }}</span>
              </button>
              <div class="ml-12 h-px shrink-0 bg-[var(--gosslan-divider)]"></div>
              <button class="flex items-center gap-3 px-4 py-3.5 text-left transition hover:bg-[var(--gosslan-list-hover)]" @click="openLogs()">
                <ScrollText class="h-5 w-5 text-[var(--gosslan-text-2)]" />
                <span class="flex-1 text-sm">{{ t("nav.logs") }}</span>
              </button>
            </div>
          </div>
        </template>
        <!-- 收藏页：桌面端在右栏整页渲染（左列表 + 右详情两栏）；
             移动端作为普通底部 tab（保留 tab 栏），点具体收藏才在 FavoritePanel 内部新开详情页。
             ⚠️ 这里必须是 `v-else-if`（接到上面「我的」那条 `v-if="navState === 'me'"`），
             否则「我的」页会和链尾的兜底空态（「选择一个会话开始聊天」）**同时渲染**——
             两条互不相干的条件链各自命中，用户会看到「我的」下面还挂着一个空会话页（用户 2026-09-20）。 -->
        <FavoritePanel v-else-if="favoritesOpen && !app.isMobile" @close="closeFavorites" />
        <FavoritePanel v-else-if="app.isMobile && navState === 'favorites'" />
        <div
          v-else-if="!app.isMobile && isLinksView"
          class="flex h-full select-none flex-col items-center justify-center gap-3 text-[var(--gosslan-text-2)]"
        >
          <Compass class="h-16 w-16 opacity-25" />
          <div class="text-base">{{ t("layout.linksTitle") }}</div>
          <div class="max-w-[280px] text-center text-xs leading-relaxed opacity-70">
            {{ t("layout.linksHint") }}
          </div>
        </div>
        <!-- 新的朋友页：右侧展示好友申请列表（微信式） -->
        <div v-else-if="navState === 'contacts' && showRequests" class="flex h-full flex-col">
          <div class="flex shrink-0 items-center border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)] px-4" :style="{ height: 'var(--gosslan-header-h)' }">
            <span class="text-[15px] font-medium">{{ t("conv.newFriends") }}</span>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto bg-[var(--gosslan-chat)] p-2">
            <FriendRequestList
              :requests="chat.pendingRequests"
              :open="true"
              @close="closeRequests"
              @accept="acceptRequest"
              @reject="rejectRequest"
            />
            <div v-if="chat.pendingRequests.length === 0" class="mt-20 text-center text-sm text-[var(--gosslan-text-2)]">
              {{ t("friend.request.empty") }}
            </div>
          </div>
        </div>
        <FriendProfile
          v-else-if="navState === 'contacts' && profileFriend !== null"
          :friend="profileFriend"
          @send-message="sendMessageTo"
          @remove="removeFriend"
        />
        <ChatWindow
          v-else-if="navState === 'chats' && chat.activeConv"
          @open-share="shareOpen = true"
        />
        <div
          v-else
          class="flex h-full select-none flex-col items-center justify-center gap-3 text-[var(--gosslan-text-2)]"
        >
          <MessageCircle class="h-16 w-16 opacity-25" />
          <div class="text-base">{{ t("layout.selectConversation") }}</div>
          <div class="text-xs opacity-70">{{ t("layout.tagline") }}</div>
          <!-- 空态要给**下一步**，不只陈述状态（HIG：empty state should guide）。
               用户 2026-09-12 晚 #13：「聊天界面如果没有聊天信息的话，这一块左右两边有点割裂。
               他们的样式能不能统一一点？主要的功能是：1. 如果你有好友，就有一个『发起聊天』；
               2. 如果你没有好友列表，就只有一个『添加好友』的按钮。这个按钮在会话框页面和
               聊天列表页面，你可以做一个文字说明兜底，或者在样式上做一个空状态就行了。」
               ⇒ 与左侧会话列表**同一口径**：有好友 → 发起聊天（跳通讯录选人）；
                 无好友 → 添加好友；两种情况都配同一句说明文字。 -->
          <button
            v-if="chat.friends.length"
            class="tap-safe mt-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-4 py-2 text-sm font-medium text-white transition hover:bg-[var(--gosslan-primary-hover)]"
            @click="navState = 'contacts'"
          >
            {{ t("conv.startChat") }}
          </button>
          <button
            v-else
            class="tap-safe mt-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-4 py-2 text-sm font-medium text-white transition hover:bg-[var(--gosslan-primary-hover)]"
            @click="addFriendOpen = true"
          >
            {{ t("common.addFriend") }}
          </button>
          <div class="text-xs opacity-70">
            {{ chat.friends.length ? t("conv.emptyHintHasFriends") : t("conv.emptyHintNoFriends") }}
          </div>
        </div>
      </div>
    </main>
    </div>

    <!-- 移动端底部导航：4 按钮（聊天 / 通讯录 / 收藏 / 我的）；软键盘弹出时收起（避免浮在键盘上方遮挡输入）。
         显示条件**统一走 `showMobileTabBar`**（与上面内容区的 pb 占位同源，不要再各写一份）：
         只在**四个一级 tab** 上显示 —— 聊天/通讯录列表（`mobileView === 'list'`）、收藏、我的；
         进 Chat 详情 / 设置 / 日志 / 链接 等二级页时整个隐藏（二级页不该有一级页的 TabBar）。
         ⚠️ 收藏 tab 必须**保留** TabBar（用户 2026-09-20 明确要求，「收藏 tab 不要新开页面」）。 -->
    <nav
      v-if="showMobileTabBar"
      class="safe-bottom fixed bottom-0 left-0 right-0 z-40 flex items-center justify-around border-t border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]"
    >
      <button
        class="relative flex flex-1 flex-col items-center gap-0.5 py-2.5"
        :class="navState === 'chats' ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-text-2)]'"
        @click="selectMobileTab('chats', 'list')"
      >
        <span class="relative">
          <MessageCircle class="h-5 w-5" />
          <UnreadBadge
            v-if="chat.totalUnread > 0"
            :count="chat.totalUnread"
            class="absolute -right-2.5 -top-1"
          />
        </span>
        <span class="text-[11px]">{{ t("nav.chats") }}</span>
      </button>
      <button
        class="relative flex flex-1 flex-col items-center gap-0.5 py-2.5"
        :class="navState === 'contacts' ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-text-2)]'"
        @click="selectMobileTab('contacts', 'list')"
      >
        <span class="relative">
          <Users class="h-5 w-5" />
          <UnreadBadge
            v-if="chat.pendingRequests.length"
            :count="chat.pendingRequests.length"
            class="absolute -right-2.5 -top-1"
          />
        </span>
        <span class="text-[11px]">{{ t("nav.contacts") }}</span>
      </button>
      <!-- 收藏：移动端 rail hidden md:flex（看不到），TabBar 保留入口。
           桌面端 NavRail 也有星标按钮，风格一致。 -->
      <button
        class="relative flex flex-1 flex-col items-center gap-0.5 py-2.5"
        :class="navState === 'favorites' ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-text-2)]'"
        @click="selectMobileTab('favorites', 'chat')"
      >
        <Star class="h-5 w-5" />
        <span class="text-[11px]">{{ t("nav.favorites") }}</span>
      </button>
      <!-- 我的：设置 / 链接 / 运行日志收进「我的」页的菜单（TabBar 最多 4 项）。
           本分支用「我的」tab 取代了旧的「更多」弹出 sheet（用户 2026-09-20 的改版：
           设置/链接/日志都进「我的」，不再需要二级 sheet）。 -->
      <button
        class="relative flex flex-1 flex-col items-center gap-0.5 py-2.5"
        :class="navState === 'me' ? 'text-[var(--gosslan-primary)]' : 'text-[var(--gosslan-text-2)]'"
        @click="selectMobileTab('me', 'chat')"
      >
        <UserCircle class="h-5 w-5" />
        <span class="text-[11px]">{{ t("nav.me") }}</span>
      </button>
    </nav>

    <!-- 弹窗 -->
    <SettingsPanel :open="settingsOpen" @close="settingsOpen = false" />
    <!-- 搜索聊天记录结果页（会话列表搜索框回车打开） -->
    <ChatSearchDialog
      :open="searchOpen"
      :initial-keyword="searchSeed"
      @close="searchOpen = false"
      @open-conversation="onOpenSearchHit"
    />

    <AddFriendModal :open="addFriendOpen" @close="addFriendOpen = false" />
    <GroupCreateModal :open="groupOpen" @close="groupOpen = false" />
    <ShareDirectory :open="shareOpen" @close="shareOpen = false" />

    <!-- 移动端运行日志页：全屏覆盖、带返回（桌面端走独立窗口，见 open_log_window） -->
    <Transition name="page-slide">
      <LogViewer v-if="app.isMobile && logsOpen" @back="logsOpen = false" />
    </Transition>

    <!-- 移动端链接页：从「我的」下钻进来的全屏覆盖层（桌面端在左列渲染） -->
    <Transition name="page-slide">
      <LinksList v-if="app.isMobile && isLinksView" :opening="linksOpening" @open="openLink" @back="onLinksBack" />
    </Transition>

    <!-- 移动端资料页：可编辑资料，从「我的」下钻进来（桌面端回落到设置） -->
    <Transition name="page-slide">
      <MobilePageFrame
        v-if="app.isMobile && profileOpen"
        mode="overlay"
        :title="t('nav.profile')"
        @back="profileOpen = false"
      >
        <ProfileSection :active="profileOpen" :reload-token="profileReloadToken" />
      </MobilePageFrame>
    </Transition>

    <!-- Toast：主窗口与独立窗口共用的 HUD（见 `ToastHud.vue` 的说明）。 -->
    <ToastHud />
  </div>
</template>
