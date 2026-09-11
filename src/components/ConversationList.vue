<script setup lang="ts">
import { t } from "@/i18n";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useConversationSearch } from "@/composables/useConversationSearch";
import { useExclusivePopup } from "@/composables/useExclusivePopup";
import ConversationListItem from "@/components/conversation/ConversationListItem.vue";
import FriendListItem from "@/components/conversation/FriendListItem.vue";
import FriendContextMenu from "@/components/conversation/FriendContextMenu.vue";
import BaseModal from "@/components/BaseModal.vue";
import UnreadBadge from "@/components/UnreadBadge.vue";
import { APP_ACTION } from "@/api";
import { Plus, Search, UserPlus, UsersRound } from "lucide-vue-next";
import type { Conversation, Friend } from "@/types";

const props = defineProps<{
  view: "chats" | "contacts";
  /** 正在查看资料的好友 ID：通讯录里对应好友高亮 */
  activeFriendId?: string | null;
  /** 「新的朋友」页是否在右侧打开（列表项高亮） */
  requestsActive?: boolean;
}>();
const emit = defineEmits<{
  (e: "update:view", v: "chats" | "contacts"): void;
  (e: "open-add-friend"): void;
  (e: "open-group"): void;
  (e: "open-friend", f: Friend): void;
  (e: "open-requests"): void;
}>();

const app = useAppStore();
const chat = useChatStore();

const { keyword, results, filtered, snippet, hitMsgId } = useConversationSearch(
  computed(() => chat.conversations),
);

const filteredFriends = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return chat.friends;
  return chat.friends.filter((f) => f.nickname.toLowerCase().includes(kw));
});

/**
 * 加号下拉菜单展开态：点击外部自动收起；并参与全局浮层互斥
 * ——右键消息菜单 / 已读弹层展开时会自动收起它（右键不触发 click，靠点击关闭会漏）。
 */
const plusPopup = useExclusivePopup("plus-menu");
const plusOpen = ref(false);

watch(plusPopup.isActive, (mine) => {
  if (!mine && plusOpen.value) plusOpen.value = false;
});
watch(() => props.view, () => closePlus());

function togglePlus() {
  if (plusOpen.value) {
    closePlus();
    return;
  }
  plusOpen.value = true;
  plusPopup.claim();
}

function closePlus() {
  plusPopup.release();
  plusOpen.value = false;
}

function onDocClickForPlus() {
  closePlus();
}
onMounted(() => document.addEventListener("click", onDocClickForPlus));
onUnmounted(() => document.removeEventListener("click", onDocClickForPlus));

// 聚焦搜索框（⌘F / Ctrl+F 与 macOS 菜单「会话 → 搜索」共用同一动作）
const searchInput = ref<HTMLInputElement | null>(null);
function focusSearch() {
  searchInput.value?.focus();
  searchInput.value?.select();
}
onMounted(() => window.addEventListener(APP_ACTION.focusSearch, focusSearch));
onUnmounted(() => window.removeEventListener(APP_ACTION.focusSearch, focusSearch));

/** 对端在线状态：群聊返回 null（无在线概念）；单聊查好友表。 */
function isOnline(id: string): boolean | null {
  if (id.startsWith("group:")) return null;
  return chat.friends.find((f) => f.device_id === id)?.online ?? false;
}

/**
 * 打开会话。若会话列表正处于**搜索结果**态，且命中里有具体消息 → 直接跳到那一条。
 * 只把用户丢进会话、让他自己翻，搜索就只完成了一半（HIG：搜索的价值是"降低定位成本"）。
 * 定位失败时按**原因**分别告知，不静默、也不说错原因。
 */
async function openConv(conv: Conversation) {
  if (app.isMobile) app.mobileView = "chat";
  const hit = hitMsgId(conv.id);
  if (hit) {
    const outcome = await chat.locateMessageInConv(conv.id, hit);
    if (outcome === "not-found") app.toast(t("conv.toast.locateNotFound"), "info");
    else if (outcome === "error") app.toast(t("conv.toast.locateError"), "error");
    return;
  }
  chat.openConversation(conv.id);
}
/** 通讯录点击好友 → 打开资料页（发消息由资料页按钮触发，不再直接开会话） */
function openFriend(f: Friend) {
  emit("open-friend", f);
  if (app.isMobile) app.mobileView = "chat";
}

// ---------------- 右键菜单：删除好友 ----------------
// 参与全局浮层互斥：右键消息、或打开「已读成员」弹层时，本菜单会自动收起
// （右键只触发 contextmenu 不触发 click，仅靠 document click 关闭会漏）。
const friendMenuPopup = useExclusivePopup("friend-menu");
const friendMenu = ref<{ x: number; y: number; friend: Friend } | null>(null);

watch(friendMenuPopup.isActive, (mine) => {
  if (!mine && friendMenu.value) friendMenu.value = null;
});

/** 右键（桌面）/ 长按（移动端）触发：菜单定位贴近屏幕边缘时向内收，避免溢出。 */
function onFriendContext(f: Friend, x: number, y: number) {
  const mw = 150;
  const mh = 90;
  friendMenu.value = {
    x: Math.max(8, Math.min(x, window.innerWidth - mw - 8)),
    y: Math.max(8, Math.min(y, window.innerHeight - mh - 8)),
    friend: f,
  };
  friendMenuPopup.claim();
}

/** 删除好友：保留聊天记录；对方仍出现在扫描列表，可重新添加。乐观移除，失败回滚。
 *
 *  ⚠️ 必须二次确认：右键菜单此前**单击即删**，而「删除聊天记录」和资料页的「删除好友」
 *  都有确认弹窗——同一类破坏性操作三种行为不一致，右键菜单那条最容易误触。
 *  这里刻意**不做"撤销"**：删除好友在后端不是可本地回滚的操作（对方可能已同步移除，
 *  重新建立关系要走一次好友申请），给一个假的"撤销"比不给更糟。 */
const pendingRemoveFriend = ref<Friend | null>(null);

function onAskDeleteFriend() {
  pendingRemoveFriend.value = friendMenu.value?.friend ?? null;
  closeFriendMenu();
}

async function confirmDeleteFriend() {
  const f = pendingRemoveFriend.value;
  pendingRemoveFriend.value = null;
  if (!f) return;
  try {
    await chat.removeFriend(f.device_id);
    app.toast(t("conv.toast.friendRemoved", { name: f.nickname }), "info");
  } catch (e) {
    app.toastError(e, t("common.deleteFail"));
  }
}

// ---------------- 删除聊天记录（仅本地，二次确认） ----------------
const pendingDelete = ref<Conversation | null>(null);

function onAskDeleteConv(conv: Conversation, e: MouseEvent) {
  e.stopPropagation();
  pendingDelete.value = conv;
}

async function confirmDeleteConv() {
  const c = pendingDelete.value;
  pendingDelete.value = null;
  if (!c) return;
  try {
    await chat.deleteConversation(c.id);
    app.toast(t("conv.toast.historyDeleted", { name: c.name }), "success");
  } catch (e) {
    app.toastError(e, t("common.deleteFail"));
  }
}

function closeFriendMenu() {
  friendMenuPopup.release();
  friendMenu.value = null;
}
onMounted(() => document.addEventListener("click", closeFriendMenu));
onUnmounted(() => document.removeEventListener("click", closeFriendMenu));
</script>

<template>
  <div class="flex h-full flex-col bg-[var(--gosslan-list)]">
    <!-- 列表头：搜索框（白底+细边，在浅灰栏上清晰）+ 操作按钮。
         高度必须与右栏 ChatHeader 同源（--gosslan-header-h）并同样画下边框：
         两栏从同一个 y 起算，只有高度与底边线都一致，那条分隔线才是**一条连续的线**。
         之前这里是 px-3 py-2 + h-9 搜索框 = 52px（比右栏 56px 矮 4px）且无底边线，
         于是左右永远差 4px、右栏那条线在左栏没有对应物。改高度/内边距时留意这条约束。 -->
    <div
      class="flex shrink-0 items-center gap-1.5 border-b border-[var(--gosslan-divider)] px-3"
      :style="{ height: 'var(--gosslan-header-h)' }"
    >
      <!-- 搜索框底色必须用 --gosslan-field 而不是 panel：
           暗色下 panel(#1e293b) 与本栏 list(#1e293b) 是同一个值，输入框会"消失"；
           field 在两套主题里都与所在栏拉开一档。 -->
      <div
        class="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-field)] px-2.5 transition focus-within:border-[var(--gosslan-primary)]"
      >
        <Search class="h-4 w-4 shrink-0 text-[var(--gosslan-text-2)]" />
        <input
          ref="searchInput"
          v-model="keyword"
          maxlength="100"
          class="w-full bg-transparent text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)]"
          :placeholder="view === 'chats' ? t('common.search') : t('common.searchContacts')"
        />
      </div>
      <div class="relative flex shrink-0 items-center">
        <!-- 微信式：单个加号，点开下拉（添加好友 / 创建群聊） -->
        <button
          class="tap-safe flex h-7 w-7 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-list-hover)]"
          :title="t('conv.addTitle')" :aria-label="t('conv.addTitle')"
          @click.stop="togglePlus"
        >
          <Plus class="h-[18px] w-[18px]" />
        </button>
        <div
          v-if="plusOpen"
          class="frost absolute right-0 top-8 z-30 w-36 overflow-hidden rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] py-1 shadow-lg"
        >
          <button
            class="flex w-full items-center gap-2 px-3 py-2 text-[13px] text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
            @click.stop="closePlus(); emit('open-add-friend')"
          >
            <UserPlus class="h-4 w-4 text-[var(--gosslan-text-2)]" />
            {{ t("common.addFriend") }}
          </button>
          <button
            class="flex w-full items-center gap-2 px-3 py-2 text-[13px] text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
            @click.stop="closePlus(); emit('open-group')"
          >
            <UsersRound class="h-4 w-4 text-[var(--gosslan-text-2)]" />
            {{ t("common.createGroup") }}
          </button>
        </div>
      </div>
    </div>

    <div class="flex-1 select-none overflow-y-auto">
      <template v-if="view === 'chats'">
        <ConversationListItem
          v-for="c in filtered"
          :key="c.id"
          v-memo="[c.last_ts, c.last_msg, c.unread, c.avatar, c.name, chat.activeConv === c.id, isOnline(c.id), keyword, results.length, chat.friends.length, chat.groups.length]"
          :conv="c"
          :active="chat.activeConv === c.id"
          :online="isOnline(c.id)"
          :snippet="snippet(c.id)"
          :keyword="keyword"
          @open="openConv"
          @ask-delete="onAskDeleteConv"
        />
        <div v-if="filtered.length === 0" class="mt-16 flex flex-col items-center gap-3 text-center text-sm text-[var(--gosslan-text-2)]">
          <span>{{ keyword.trim() ? t("conv.noMatchConv") : t("conv.noConversation") }}</span>
          <!-- 空态给下一步：新用户在这里直接能去加人（搜索无结果时不给，那是"换个词"的场景） -->
          <button
            v-if="!keyword.trim()"
            class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
            @click="emit('open-add-friend')"
          >
            {{ t("common.addFriend") }}
          </button>
        </div>
      </template>

      <template v-else>
        <!-- 通讯录固定首项：新的朋友（好友申请入口，微信式），点击在右侧打开申请页 -->
        <button
          class="flex h-[56px] w-full cursor-pointer items-center gap-3 px-3 transition-colors"
          :class="props.requestsActive ? 'bg-[var(--gosslan-list-active)]' : 'hover:bg-[var(--gosslan-list-hover)]'"
          @click="emit('open-requests')"
        >
          <span class="relative shrink-0">
            <span class="flex h-10 w-10 items-center justify-center rounded-[var(--gosslan-avatar-radius)] brand-surface text-white">
              <UserPlus class="h-5 w-5" />
            </span>
            <UnreadBadge
              v-if="chat.pendingRequests.length"
              :count="chat.pendingRequests.length"
              class="absolute -right-1 -top-1"
            />
          </span>
          <span class="min-w-0 flex-1 text-left">
            <span class="block truncate text-[13px] leading-5 text-[var(--gosslan-text)]">{{ t("conv.newFriends") }}</span>
            <span class="block truncate text-[12px] leading-5 text-[var(--gosslan-text-2)]">
              {{ chat.pendingRequests.length ? t("conv.pendingRequests", { n: chat.pendingRequests.length }) : t("friend.request.empty") }}
            </span>
          </span>
        </button>
        <FriendListItem
          v-for="f in filteredFriends"
          :key="f.device_id"
          v-memo="[f.nickname, f.avatar, f.online, props.activeFriendId === f.device_id]"
          :friend="f"
          :active="props.activeFriendId === f.device_id"
          @open="openFriend"
          @context="onFriendContext"
        />
        <div v-if="filteredFriends.length === 0" class="mt-16 flex flex-col items-center gap-3 text-center text-sm text-[var(--gosslan-text-2)]">
          <span>{{ keyword.trim() ? t("conv.noMatchContact") : t("conv.noFriends") }}</span>
          <button
            v-if="!keyword.trim()"
            class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-xs text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
            @click="emit('open-add-friend')"
          >
            {{ t("common.addFriend") }}
          </button>
        </div>
      </template>
    </div>

    <FriendContextMenu
      v-if="friendMenu"
      :x="friendMenu.x"
      :y="friendMenu.y"
      @close="closeFriendMenu"
      @confirm="onAskDeleteFriend"
    />

    <!-- 二次确认：删除好友（保留聊天记录，可重新添加） -->
    <BaseModal
      :open="pendingRemoveFriend !== null"
      :title="t('common.deleteFriend')"
      @close="pendingRemoveFriend = null"
    >
      <div class="space-y-3">
        <p class="text-sm text-[var(--gosslan-text)]">
          {{ t("conv.removeFriend.bodyPrefix") }}<span class="font-medium text-[var(--gosslan-danger-ink)]">{{ pendingRemoveFriend?.nickname }}</span>{{ t("conv.removeFriend.bodySuffix") }}
        </p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· {{ t("conv.removeFriend.item1") }}</li>
          <li>· {{ t("conv.removeFriend.item2") }}</li>
          <li>· {{ t("conv.removeFriend.item3") }}</li>
        </ul>
        <div class="flex justify-end gap-2 pt-2">
          <button
            class="rounded-[var(--gosslan-radius-md)] px-4 py-1.5 text-sm transition hover:bg-[var(--gosslan-hover)]"
            @click="pendingRemoveFriend = null"
          >{{ t("common.cancel") }}</button>
          <button
            class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-1.5 text-sm text-white transition hover:bg-[var(--gosslan-danger)]"
            @click="confirmDeleteFriend"
          >{{ t("common.deleteFriend") }}</button>
        </div>
      </div>
    </BaseModal>

    <!-- 二次确认：删除聊天记录（仅本地清理，不影响对方） -->
    <BaseModal :open="pendingDelete !== null" :title="t('conv.delete')" @close="pendingDelete = null">
      <div class="space-y-3">
        <p class="text-sm text-[var(--gosslan-text)]">
          {{ t("conv.delete.bodyPrefix") }}<span class="font-medium text-[var(--gosslan-danger-ink)]">{{ pendingDelete?.name }}</span>{{ t("conv.delete.bodySuffix") }}
        </p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· {{ t("conv.delete.item1") }}</li>
          <li>· {{ t("conv.delete.item2") }}</li>
          <li>· {{ t("conv.delete.item3") }}</li>
        </ul>
        <div class="flex justify-end gap-2 pt-2">
          <button
            class="rounded-[var(--gosslan-radius-md)] px-4 py-1.5 text-sm transition hover:bg-[var(--gosslan-hover)]"
            @click="pendingDelete = null"
          >{{ t("common.cancel") }}</button>
          <button
            class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-1.5 text-sm text-white transition hover:bg-[var(--gosslan-danger)]"
            @click="confirmDeleteConv"
          >{{ t("common.delete") }}</button>
        </div>
      </div>
    </BaseModal>
  </div>
</template>
