<script setup lang="ts">
import { computed } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { avatarInitial, nameToColor } from "@/utils/color";
import { MessageCircle, Moon, ScrollText, Sun, Users } from "lucide-vue-next";
import UnreadBadge from "@/components/UnreadBadge.vue";
import { t } from "@/i18n";

defineProps<{ view: "chats" | "contacts" }>();
const emit = defineEmits<{
  (e: "update:view", v: "chats" | "contacts"): void;
  (e: "open-settings"): void;
  (e: "open-logs"): void;
}>();

const app = useAppStore();
const chat = useChatStore();
const initials = computed(() => avatarInitial(app.device?.nickname));
</script>

<template>
  <!-- 微信式窄导航栏：顶部本人头像 + 中部导航图标 + 底部偏好；
       选中项仅图标变色（主题色），不改底色，与微信一致 -->
  <aside
    class="hidden shrink-0 select-none flex-col items-center bg-[var(--gosslan-rail)] py-3 md:flex"
    :style="{ width: 'var(--gosslan-rail-w)' }"
  >
    <!-- 顶部：本人头像（点开设置/我）；在线点放在 overflow-hidden 按钮外层，避免被裁切 -->
    <div class="relative shrink-0">
      <button
        class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white transition hover:opacity-90"
        :style="{ backgroundColor: nameToColor(app.device?.nickname ?? '') }"
        :title="app.online ? t('nav.me.online') : t('nav.me.offline')"
        :aria-label="t('nav.me.openSettings', { status: app.online ? t('nav.me.online') : t('nav.me.offline') })"
        @click="emit('open-settings')"
      >
        <img alt="" v-if="app.device?.avatar" :src="app.device.avatar" class="h-full w-full object-cover" />
        <span v-else class="text-sm font-medium">{{ initials }}</span>
      </button>
      <!-- 本人在线状态点 -->
      <span
        class="absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-rail)]"
        :class="app.online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
      ></span>
    </div>

    <!-- 中部：聊天 / 通讯录（选中仅图标变主题色，无背景块） -->
    <div class="mt-5 flex flex-col items-center gap-2">
      <button
        class="relative flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] transition"
        :class="view === 'chats'
          ? 'text-[var(--gosslan-rail-text-active)]'
          : 'text-[var(--gosslan-rail-text)] hover:bg-[var(--gosslan-rail-hover)]'"
        :title="t('nav.chats')"
        :aria-label="chat.totalUnread > 0 ? t('nav.chats.unread', { n: chat.totalUnread }) : t('nav.chats')"
        @click="emit('update:view', 'chats')"
      >
        <MessageCircle class="h-[22px] w-[22px]" :fill="view === 'chats' ? 'currentColor' : 'none'" :stroke-width="view === 'chats' ? 2 : 1.9" />
        <UnreadBadge
          v-if="chat.totalUnread > 0"
          :count="chat.totalUnread"
          class="absolute -right-0.5 -top-0.5"
        />
      </button>
      <button
        class="relative flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] transition"
        :class="view === 'contacts'
          ? 'text-[var(--gosslan-rail-text-active)]'
          : 'text-[var(--gosslan-rail-text)] hover:bg-[var(--gosslan-rail-hover)]'"
        :title="t('nav.contacts')"
        :aria-label="chat.pendingRequests.length ? t('nav.contacts.pending', { n: chat.pendingRequests.length }) : t('nav.contacts')"
        @click="emit('update:view', 'contacts')"
      >
        <Users class="h-[22px] w-[22px]" :fill="view === 'contacts' ? 'currentColor' : 'none'" :stroke-width="view === 'contacts' ? 2 : 1.9" />
        <UnreadBadge
          v-if="chat.pendingRequests.length"
          :count="chat.pendingRequests.length"
          class="absolute -right-0.5 -top-0.5"
        />
      </button>
    </div>

    <!-- 底部：深浅色切换 + 设置 -->
    <div class="mt-auto flex flex-col items-center gap-2">
      <button
        class="flex h-10 w-10 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :title="app.dark ? t('nav.lightMode') : t('nav.darkMode')" :aria-label="app.dark ? t('nav.lightMode') : t('nav.darkMode')"
        @click="app.toggleDark()"
      >
        <Sun v-if="app.dark" class="h-[19px] w-[19px]" />
        <Moon v-else class="h-[19px] w-[19px]" />
      </button>
      <button
        class="flex h-10 w-10 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :title="t('nav.settings')" :aria-label="t('nav.settings')"
        @click="emit('open-settings')"
      >
        <svg viewBox="0 0 24 24" class="h-[19px] w-[19px]" fill="none" stroke="currentColor" stroke-width="1.9">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.6 1.65 1.65 0 0 0 10 3.09V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </button>
      <button
        class="flex h-10 w-10 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :title="t('nav.logs')" :aria-label="t('nav.logs')"
        @click="emit('open-logs')"
      >
        <ScrollText class="h-[19px] w-[19px]" />
      </button>
    </div>
  </aside>
</template>
