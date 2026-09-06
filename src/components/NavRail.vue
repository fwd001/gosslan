<script setup lang="ts">
import { computed } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { MessageCircle, Moon, Settings, Sun, Users } from "lucide-vue-next";

defineProps<{ view: "chats" | "contacts" }>();
const emit = defineEmits<{
  (e: "update:view", v: "chats" | "contacts"): void;
  (e: "open-settings"): void;
}>();

const app = useAppStore();
const chat = useChatStore();
const initials = computed(() => (app.device?.nickname ?? "?").slice(0, 1).toUpperCase());
</script>

<template>
  <aside
    class="hidden md:flex w-16 shrink-0 flex-col items-center select-none border-r border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] py-3"
  >
    <button
      class="relative flex h-10 w-10 items-center justify-center overflow-visible rounded-full bg-primary text-white transition hover:opacity-90"
      :title="app.online ? '我在线（局域网已连接）' : '离线（局域网未连接）'"
      @click="emit('open-settings')"
    >
      <span class="flex h-full w-full overflow-hidden rounded-full">
        <img v-if="app.device?.avatar" :src="app.device.avatar" class="h-full w-full object-cover" />
        <span v-else class="flex h-full w-full items-center justify-center text-base font-semibold">{{ initials }}</span>
      </span>
      <!-- 本人在线状态点：绿=局域网已连接 -->
      <span
        class="absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full border-2 border-[var(--gosslan-panel)]"
        :class="app.online ? 'bg-emerald-500' : 'bg-neutral-400'"
      ></span>
    </button>

    <div class="mt-5 flex flex-col items-center gap-3">
      <button
        class="flex items-center justify-center rounded-lg p-2.5 transition"
        :class="view === 'chats' ? 'bg-primary-light text-primary' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        title="消息"
        @click="emit('update:view', 'chats')"
      >
        <MessageCircle class="h-5 w-5" />
      </button>
      <button
        class="relative flex items-center justify-center rounded-lg p-2.5 transition"
        :class="view === 'contacts' ? 'bg-primary-light text-primary' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        title="联系人"
        @click="emit('update:view', 'contacts')"
      >
        <Users class="h-5 w-5" />
        <span
          v-if="chat.pendingRequests.length"
          class="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[10px] font-medium text-white"
        >
          {{ chat.pendingRequests.length }}
        </span>
      </button>
    </div>

    <div class="mt-auto flex flex-col items-center gap-3 pb-2">
      <button
        class="flex items-center justify-center rounded-lg p-2.5 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="app.dark ? '浅色模式' : '深色模式'"
        @click="app.toggleDark()"
      >
        <Sun v-if="app.dark" class="h-5 w-5" />
        <Moon v-else class="h-5 w-5" />
      </button>
      <button
        class="flex items-center justify-center rounded-lg p-2.5 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        title="设置"
        @click="emit('open-settings')"
      >
        <Settings class="h-5 w-5" />
      </button>
    </div>
  </aside>
</template>
