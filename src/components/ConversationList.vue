<script setup lang="ts">
import { computed, ref } from "vue";
import dayjs from "dayjs";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { Search, UserPlus, UsersRound } from "lucide-vue-next";
import type { Conversation, Friend } from "@/types";

defineProps<{ view: "chats" | "contacts" }>();
const emit = defineEmits<{
  (e: "update:view", v: "chats" | "contacts"): void;
  (e: "open-add-friend"): void;
  (e: "open-group"): void;
}>();

const app = useAppStore();
const chat = useChatStore();
const keyword = ref("");

const filteredConversations = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return chat.conversations;
  return chat.conversations.filter((c) => c.name.toLowerCase().includes(kw));
});

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

function open(conv: Conversation) {
  chat.openConversation(conv.id);
  if (app.isMobile) app.mobileView = "chat";
}
function openFriend(f: Friend) {
  chat.openConversation(f.device_id);
  if (app.isMobile) app.mobileView = "chat";
}
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
          class="flex cursor-pointer items-center gap-3 rounded-xl px-2 py-2 transition hover:bg-[var(--gosslan-hover)]"
          :class="chat.activeConv === c.id ? 'bg-primary-light' : ''"
          @click="open(c)"
        >
          <div class="relative">
            <div
              class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-full bg-primary text-white"
            >
              <img v-if="c.avatar" :src="c.avatar" class="h-full w-full object-cover" />
              <span v-else class="text-sm font-semibold">{{ initials(c.name) }}</span>
            </div>
            <span
              v-if="c.unread > 0"
              class="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full bg-red-500"
            ></span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex items-center justify-between">
              <span class="truncate text-sm font-medium">{{ c.name }}</span>
              <span class="text-[11px] text-[var(--gosslan-text-2)]">{{ fmtTime(c.last_ts) }}</span>
            </div>
            <div class="flex items-center justify-between">
              <span class="truncate text-xs text-[var(--gosslan-text-2)]">{{ c.last_msg || "暂无消息" }}</span>
              <span
                v-if="c.unread > 0"
                class="ml-2 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[10px] text-white"
              >
                {{ c.unread }}
              </span>
            </div>
          </div>
        </div>
        <div v-if="filteredConversations.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
          暂无会话
        </div>
      </template>

      <template v-else>
        <div
          v-for="f in filteredFriends"
          :key="f.device_id"
          class="flex cursor-pointer items-center gap-3 rounded-xl px-2 py-2 transition hover:bg-[var(--gosslan-hover)]"
          @click="openFriend(f)"
        >
          <div class="relative">
            <div
              class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-full bg-primary text-white"
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
            <div class="truncate text-sm">{{ f.nickname }}</div>
            <div class="text-xs text-[var(--gosslan-text-2)]">{{ f.online ? "在线" : "离线" }}</div>
          </div>
        </div>
        <div v-if="filteredFriends.length === 0" class="mt-16 text-center text-sm text-[var(--gosslan-text-2)]">
          暂无好友
        </div>
      </template>
    </div>
  </div>
</template>
