<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import dayjs from "dayjs";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import { Check, Search, UserMinus, UserPlus, UsersRound, X } from "lucide-vue-next";
import BaseModal from "@/components/BaseModal.vue";
import type { Conversation, Friend, PendingRequest, SearchResult } from "@/types";

defineProps<{ view: "chats" | "contacts" }>();
const emit = defineEmits<{
  (e: "update:view", v: "chats" | "contacts"): void;
  (e: "open-add-friend"): void;
  (e: "open-group"): void;
}>();

const app = useAppStore();
const chat = useChatStore();
const keyword = ref("");
const searchResults = ref<SearchResult[]>([]);
const isSearching = ref(false);

// 防抖搜索
let searchTimer: ReturnType<typeof setTimeout> | null = null;
watch(keyword, (kw) => {
  if (searchTimer) clearTimeout(searchTimer);
  const trimmed = kw.trim();
  if (!trimmed) {
    searchResults.value = [];
    isSearching.value = false;
    return;
  }
  isSearching.value = true;
  searchTimer = setTimeout(async () => {
    try {
      searchResults.value = await api.searchMessages(trimmed);
    } catch {
      searchResults.value = [];
    }
    isSearching.value = false;
  }, 300);
});

const filteredConversations = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return chat.conversations;
  // 如果有消息搜索结果，合并名称匹配 + 内容匹配
  const matchedIds = new Set(searchResults.value.map((r) => r.conv_id));
  const nameMatches = chat.conversations.filter((c) => c.name.toLowerCase().includes(kw));
  // 合并去重，名称匹配优先
  const result = [...nameMatches];
  for (const c of chat.conversations) {
    if (matchedIds.has(c.id) && !result.some((r) => r.id === c.id)) {
      result.push(c);
    }
  }
  return result;
});

/** 获取会话的搜索匹配摘要 */
function getSearchSnippet(convId: string): string | null {
  const r = searchResults.value.find((x) => x.conv_id === convId);
  if (!r) return null;
  // 截取匹配位置前后的内容
  const kw = keyword.value.trim().toLowerCase();
  const content = r.match_content;
  const idx = content.toLowerCase().indexOf(kw);
  if (idx < 0) return content.slice(0, 60);
  const start = Math.max(0, idx - 20);
  const end = Math.min(content.length, idx + kw.length + 40);
  let snippet = content.slice(start, end);
  if (start > 0) snippet = "…" + snippet;
  if (end < content.length) snippet = snippet + "…";
  return snippet;
}

/** 安全高亮：将文本按关键词分割，用 <mark> 包裹匹配部分。不做 HTML 解析。 */
function highlightText(text: string, kw: string): string {
  if (!kw) return escHtml(text);
  const safe = escHtml(text);
  const safeKw = escHtml(kw);
  const regex = new RegExp(`(${safeKw.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "gi");
  return safe.replace(regex, '<mark class="bg-yellow-200 dark:bg-yellow-800/50 rounded px-0.5">$1</mark>');
}
function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

const filteredFriends = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return chat.friends;
  return chat.friends.filter((f) => f.nickname.toLowerCase().includes(kw));
});

function fmtTime(ts: number | null) {
  if (!ts) return "";
  const d = dayjs(ts);
  if (d.isSame(dayjs(), "day")) return d.format("HH:mm");
  if (d.isSame(dayjs().subtract(1, "day"), "day")) return "昨天";
  return d.format("MM-DD");
}

function initials(name: string) {
  return name.slice(0, 1).toUpperCase();
}

/** 对端在线状态：群聊返回 null（无在线概念）；单聊查好友表。 */
function isOnline(id: string): boolean | null {
  if (id.startsWith("group:")) return null;
  return chat.friends.find((f) => f.device_id === id)?.online ?? false;
}

function open(conv: Conversation) {
  chat.openConversation(conv.id);
  if (app.isMobile) app.mobileView = "chat";
}
function openFriend(f: Friend) {
  chat.openConversation(f.device_id);
  if (app.isMobile) app.mobileView = "chat";
}
async function accept(r: PendingRequest) {
  try {
    await chat.respondRequest(r.from, true);
    app.toast(`已同意 ${r.from_nickname} 的好友申请`, "success");
  } catch (e) {
    app.toast(`操作失败：${e}`, "error");
  }
}
async function reject(r: PendingRequest) {
  try {
    await chat.respondRequest(r.from, false);
    app.toast("已拒绝该好友申请", "info");
  } catch (e) {
    app.toast(`操作失败：${e}`, "error");
  }
}

// ---------------- 右键菜单：删除好友 ----------------
const friendMenu = ref<{ x: number; y: number; friend: Friend } | null>(null);

function onFriendContext(e: MouseEvent, f: Friend) {
  e.preventDefault();
  e.stopPropagation();
  const mw = 150;
  const mh = 90;
  const x = Math.min(e.clientX, window.innerWidth - mw - 8);
  const y = Math.min(e.clientY, window.innerHeight - mh - 8);
  friendMenu.value = { x: Math.max(8, x), y: Math.max(8, y), friend: f };
}

function closeFriendMenu() {
  friendMenu.value = null;
}

/** 删除好友：保留聊天记录；对方仍出现在扫描列表，可重新添加。乐观移除，失败回滚。 */
async function confirmDeleteFriend() {
  const f = friendMenu.value?.friend;
  closeFriendMenu();
  if (!f) return;
  try {
    await chat.removeFriend(f.device_id);
    app.toast(`已删除好友 ${f.nickname}（可在添加好友中重新添加）`, "info");
  } catch (e) {
    app.toast(`删除失败：${e}`, "error");
  }
}

// ---------------- 删除聊天记录（仅本地，二次确认） ----------------
const pendingDelete = ref<Conversation | null>(null);

function onAskDeleteConv(c: Conversation, e: MouseEvent) {
  e.stopPropagation();
  pendingDelete.value = c;
}

function cancelDeleteConv() {
  pendingDelete.value = null;
}

async function confirmDeleteConv() {
  const c = pendingDelete.value;
  pendingDelete.value = null;
  if (!c) return;
  try {
    await chat.deleteConversation(c.id);
    app.toast(`已删除与「${c.name}」的聊天记录`, "success");
  } catch (e) {
    app.toast(`删除失败：${e}`, "error");
  }
}

onMounted(() => {
  document.addEventListener("click", closeFriendMenu);
});
onUnmounted(() => {
  document.removeEventListener("click", closeFriendMenu);
});
</script>

<template>
  <div class="flex h-full flex-col bg-[var(--gosslan-panel)]">
    <div class="flex items-center justify-between px-4" style="height: 56px">
      <span class="text-base font-semibold">{{ view === "chats" ? "消息" : "联系人" }}</span>
      <div class="flex items-center gap-1">
        <button
          class="flex items-center justify-center rounded-lg p-2 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          title="添加好友"
          @click="emit('open-add-friend')"
        >
          <UserPlus class="h-[18px] w-[18px]" />
        </button>
        <button
          class="flex items-center justify-center rounded-lg p-2 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          title="创建群聊"
          @click="emit('open-group')"
        >
          <UsersRound class="h-[18px] w-[18px]" />
        </button>
      </div>
    </div>

    <div class="px-3 pb-2">
      <div class="flex items-center gap-2 rounded-lg bg-[var(--gosslan-bg)] px-3" style="height: 34px">
        <Search class="h-4 w-4 text-[var(--gosslan-text-2)]" />
        <input
          v-model="keyword"
          class="w-full bg-transparent text-sm outline-none placeholder:text-[var(--gosslan-text-2)]"
          :placeholder="view === 'chats' ? '搜索' : '搜索联系人'"
        />
      </div>
    </div>

    <div class="flex-1 overflow-y-auto px-2 pb-2">
      <template v-if="view === 'chats'">
        <div
          v-for="c in filteredConversations"
          :key="c.id"
          v-memo="[c.last_ts, c.last_msg, c.unread, c.avatar, c.name, chat.activeConv === c.id, isOnline(c.id)]"
          class="group/conv relative flex cursor-pointer items-center gap-3 rounded-xl px-2 py-2 transition hover:bg-[var(--gosslan-hover)]"
          :class="chat.activeConv === c.id ? 'bg-primary-light' : ''"
          @click="open(c)"
        >
          <!-- 飞书式选中态：左侧主色指示条 -->
          <span
            v-if="chat.activeConv === c.id"
            class="absolute left-0 top-1/2 h-6 w-[3px] -translate-y-1/2 rounded-full bg-primary"
          ></span>
          <div class="relative">
            <div
              class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-full bg-primary text-white"
              :class="isOnline(c.id) === false ? 'grayscale opacity-70' : ''"
            >
              <img v-if="c.avatar" :src="c.avatar" class="h-full w-full object-cover" />
              <span v-else class="text-sm font-semibold">{{ initials(c.name) }}</span>
            </div>
            <!-- 在线标识：绿点=在线可连接，灰点=离线（群聊不显示） -->
            <span
              v-if="isOnline(c.id) !== null"
              class="absolute bottom-0 right-0 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-panel)]"
              :class="isOnline(c.id) ? 'bg-emerald-500' : 'bg-neutral-400'"
            ></span>
            <span
              v-if="c.unread > 0"
              class="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full bg-red-500"
            ></span>
          </div>
          <div class="min-w-0 flex-1 overflow-hidden pr-8">
            <div class="flex items-center justify-between">
              <span
                class="truncate text-sm font-medium"
                :class="chat.activeConv === c.id ? 'text-primary' : ''"
              >{{ c.name }}</span>
              <span class="ml-2 shrink-0 whitespace-nowrap text-[11px] text-[var(--gosslan-text-2)]">{{ fmtTime(c.last_ts) }}</span>
            </div>
            <div class="flex items-center justify-between">
              <span class="truncate text-xs text-[var(--gosslan-text-2)]">
                <template v-if="getSearchSnippet(c.id)">
                  <span v-html="highlightText(getSearchSnippet(c.id)!, keyword.trim())"></span>
                </template>
                <template v-else>{{ c.last_msg || "暂无消息" }}</template>
              </span>
              <span
                v-if="c.unread > 0"
                class="ml-2 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[10px] text-white"
              >
                {{ c.unread }}
              </span>
            </div>
          </div>
          <!-- 删除聊天记录入口：右下角，hover 行时浮现，避免列表常驻视觉噪音 -->
          <button
            class="absolute bottom-1 right-1 z-10 hidden h-6 w-6 items-center justify-center rounded-md text-[var(--gosslan-text-2)] transition hover:bg-red-500/10 hover:text-red-500 group-hover/conv:flex"
            title="删除聊天记录"
            @click="onAskDeleteConv(c, $event)"
          >
            <X class="h-3.5 w-3.5" />
          </button>
        </div>
        <div v-if="filteredConversations.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
          暂无会话
        </div>
      </template>

      <template v-else>
        <!-- 好友申请 -->
        <div v-if="chat.pendingRequests.length" class="mb-2">
          <div class="px-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-[var(--gosslan-text-2)]">
            好友申请
          </div>
          <div
            v-for="r in chat.pendingRequests"
            :key="r.from"
            class="flex items-center gap-3 rounded-xl px-2 py-2"
          >
            <div class="flex h-10 w-10 shrink-0 items-center justify-center overflow-hidden rounded-full bg-primary text-white">
              <img v-if="r.from_avatar" :src="r.from_avatar" class="h-full w-full object-cover" />
              <span v-else class="text-sm font-semibold">{{ initials(r.from_nickname) }}</span>
            </div>
            <div class="min-w-0 flex-1">
              <div class="truncate text-sm font-medium">{{ r.from_nickname }}</div>
              <div class="text-xs text-[var(--gosslan-text-2)]">请求添加你为好友</div>
            </div>
            <button
              class="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-primary text-white transition hover:bg-primary-hover"
              title="同意"
              @click="accept(r)"
            >
              <Check class="h-4 w-4" />
            </button>
            <button
              class="flex h-7 w-7 shrink-0 items-center justify-center rounded-full border border-[var(--gosslan-border)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              title="拒绝"
              @click="reject(r)"
            >
              <X class="h-4 w-4" />
            </button>
          </div>
        </div>

        <div
          v-for="f in filteredFriends"
          :key="f.device_id"
          v-memo="[f.nickname, f.avatar, f.online, chat.activeConv === f.device_id]"
          class="relative flex cursor-pointer items-center gap-3 rounded-xl px-2 py-2 transition hover:bg-[var(--gosslan-hover)]"
          :class="chat.activeConv === f.device_id ? 'bg-primary-light' : ''"
          @click="openFriend(f)"
          @contextmenu="onFriendContext($event, f)"
        >
          <!-- 选中态指示条：与右侧聊天窗联动 -->
          <span
            v-if="chat.activeConv === f.device_id"
            class="absolute left-0 top-1/2 h-6 w-[3px] -translate-y-1/2 rounded-full bg-primary"
          ></span>
          <div class="relative">
            <div
              class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-full bg-primary text-white"
              :class="!f.online ? 'grayscale opacity-70' : ''"
            >
              <img v-if="f.avatar" :src="f.avatar" class="h-full w-full object-cover" />
              <span v-else class="text-sm font-semibold">{{ initials(f.nickname) }}</span>
            </div>
            <span
              class="absolute bottom-0 right-0 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-panel)]"
              :class="f.online ? 'bg-emerald-500' : 'bg-neutral-400'"
            ></span>
          </div>
          <div class="min-w-0 flex-1">
            <div
              class="truncate text-sm font-medium"
              :class="chat.activeConv === f.device_id ? 'text-primary' : ''"
            >{{ f.nickname }}</div>
            <div class="text-xs text-[var(--gosslan-text-2)]">{{ f.online ? "在线" : "离线" }}</div>
          </div>
        </div>
        <div v-if="filteredFriends.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
          暂无好友
        </div>
      </template>
    </div>

    <!-- 右键菜单：删除好友 -->
    <Teleport to="body">
      <div
        v-if="friendMenu"
        class="fixed z-50 min-w-[150px] rounded-lg border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] p-1 shadow-xl"
        :style="{ left: friendMenu.x + 'px', top: friendMenu.y + 'px' }"
        @click.stop
      >
        <button
          class="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm text-red-600 transition hover:bg-[var(--gosslan-hover)]"
          @click="confirmDeleteFriend"
        >
          <UserMinus class="h-4 w-4" />
          删除好友
        </button>
        <div class="px-3 pb-1.5 pt-1 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
          保留聊天记录，可通过「添加好友」重新添加
        </div>
      </div>
    </Teleport>

    <!-- 二次确认：删除聊天记录（仅本地清理，不影响对方） -->
    <BaseModal :open="pendingDelete !== null" title="删除聊天记录" @close="cancelDeleteConv">
      <div class="space-y-3">
        <p class="text-sm text-[var(--gosslan-text)]">
          将删除与「<span class="font-medium text-red-500">{{ pendingDelete?.name }}</span>」的全部本地聊天记录。
        </p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· 对方聊天记录不受影响</li>
          <li>· 好友关系将保留（删除好友请在「联系人」中操作）</li>
          <li>· 此操作不可撤销</li>
        </ul>
        <div class="flex justify-end gap-2 pt-2">
          <button
            class="rounded-lg px-4 py-1.5 text-sm transition hover:bg-[var(--gosslan-hover)]"
            @click="cancelDeleteConv"
          >取消</button>
          <button
            class="rounded-lg bg-red-500 px-4 py-1.5 text-sm text-white transition hover:bg-red-600"
            @click="confirmDeleteConv"
          >删除</button>
        </div>
      </div>
    </BaseModal>
  </div>
</template>
