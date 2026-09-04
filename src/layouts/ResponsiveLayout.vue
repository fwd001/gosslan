<script setup lang="ts">
import { ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import NavRail from "@/components/NavRail.vue";
import ConversationList from "@/components/ConversationList.vue";
import ChatWindow from "@/components/ChatWindow.vue";
import TopologyBar from "@/components/TopologyBar.vue";
import SettingsPanel from "@/components/SettingsPanel.vue";
import AddFriendModal from "@/components/AddFriendModal.vue";
import GroupCreateModal from "@/components/GroupCreateModal.vue";
import ShareDirectory from "@/components/ShareDirectory.vue";
import { MessageCircle, Settings, Users } from "lucide-vue-next";

const app = useAppStore();
const chat = useChatStore();

const view = ref<"chats" | "contacts">("chats");
const settingsOpen = ref(false);
const addFriendOpen = ref(false);
const groupOpen = ref(false);
const shareOpen = ref(false);

function openSettings() {
  settingsOpen.value = true;
  if (app.isMobile) app.mobileView = "list";
}
</script>

<template>
  <div class="flex h-screen w-full overflow-hidden bg-[var(--gosslan-bg)] font-gosslan text-[var(--gosslan-text)]">
    <!-- 左导航（桌面） -->
    <NavRail :view="view" @update:view="view = $event" @open-settings="openSettings" />

    <!-- 会话列表（桌面 300px / 移动端全宽，滑动切换） -->
    <aside
      class="h-full shrink-0 overflow-hidden border-r border-[var(--gosslan-border)] transition-[width] duration-300 ease-out md:w-[300px]"
      :class="app.isMobile ? (app.mobileView === 'list' ? 'w-full' : 'w-0') : 'w-[300px]'"
    >
      <div class="h-full w-[100vw] max-w-full md:w-[300px]">
        <ConversationList
          :view="view"
          @update:view="view = $event"
          @open-add-friend="addFriendOpen = true"
          @open-group="groupOpen = true"
        />
      </div>
    </aside>

    <!-- 聊天区 -->
    <main
      class="flex h-full min-w-0 flex-1 flex-col"
      :class="app.isMobile && app.mobileView === 'list' ? 'hidden' : ''"
    >
      <TopologyBar v-if="chat.activeConv" />
      <div class="min-h-0 flex-1 pb-16 md:pb-0">
        <ChatWindow v-if="chat.activeConv" @open-share="shareOpen = true" />
        <div
          v-else
          class="flex h-full select-none flex-col items-center justify-center gap-3 text-[var(--gosslan-text-2)]"
        >
          <MessageCircle class="h-16 w-16 opacity-25" />
          <div class="text-base">选择会话，开始局域网聊天</div>
          <div class="text-xs opacity-70">无服务器 · 纯 P2P · 端到端加密 · 数据仅存本机</div>
        </div>
      </div>
    </main>

    <!-- 移动端底部导航 -->
    <nav
      v-if="app.isMobile"
      class="safe-bottom fixed bottom-0 left-0 right-0 z-40 flex items-center justify-around border-t border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]"
    >
      <button
        class="flex flex-1 flex-col items-center gap-0.5 py-2.5"
        :class="view === 'chats' && app.mobileView === 'list' ? 'text-primary' : 'text-[var(--gosslan-text-2)]'"
        @click="app.mobileView = 'list'; view = 'chats'"
      >
        <MessageCircle class="h-5 w-5" />
        <span class="text-[10px]">消息</span>
      </button>
      <button
        class="flex flex-1 flex-col items-center gap-0.5 py-2.5"
        :class="view === 'contacts' && app.mobileView === 'list' ? 'text-primary' : 'text-[var(--gosslan-text-2)]'"
        @click="app.mobileView = 'list'; view = 'contacts'"
      >
        <Users class="h-5 w-5" />
        <span class="text-[10px]">联系人</span>
      </button>
      <button
        class="flex flex-1 flex-col items-center gap-0.5 py-2.5 text-[var(--gosslan-text-2)]"
        @click="openSettings"
      >
        <Settings class="h-5 w-5" />
        <span class="text-[10px]">设置</span>
      </button>
    </nav>

    <!-- 弹窗 -->
    <SettingsPanel :open="settingsOpen" @close="settingsOpen = false" />
    <AddFriendModal :open="addFriendOpen" @close="addFriendOpen = false" />
    <GroupCreateModal :open="groupOpen" @close="groupOpen = false" />
    <ShareDirectory :open="shareOpen" @close="shareOpen = false" />

    <!-- Toast -->
    <div class="pointer-events-none fixed left-1/2 top-4 z-[60] flex -translate-x-1/2 flex-col items-center gap-2">
      <div
        v-for="t in app.toasts"
        :key="t.id"
        class="rounded-lg px-4 py-2 text-sm text-white shadow-lg"
        :class="t.type === 'success' ? 'bg-emerald-600' : t.type === 'error' ? 'bg-red-600' : 'bg-neutral-800'"
      >
        {{ t.text }}
      </div>
    </div>
  </div>
</template>
