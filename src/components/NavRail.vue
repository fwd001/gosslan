<script setup lang="ts">
import { computed } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { avatarInitial, nameToColor } from "@/utils/color";
import { Moon, ScrollText, Sun } from "lucide-vue-next";
import UnreadBadge from "@/components/UnreadBadge.vue";
import { t } from "@/i18n";

defineProps<{
  view: "chats" | "contacts";
  /** 正在打开独立窗口时的忙碌态（单飞/防抖状态在 `useWindowLauncher` 里，见该文件）。 */
  settingsOpening?: boolean;
  logsOpening?: boolean;
}>();
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
        :class="settingsOpening ? 'opacity-60' : ''"
        :aria-busy="settingsOpening"
        :style="{ backgroundColor: nameToColor(app.device?.nickname ?? '') }"
        :title="app.present ? t('nav.me.online') : t('nav.me.offline')"
        :aria-label="t('nav.me.openSettings', { status: app.present ? t('nav.me.online') : t('nav.me.offline') })"
        @click="emit('open-settings')"
      >
        <img alt="" v-if="app.device?.avatar" :src="app.device.avatar" class="h-full w-full object-cover" />
        <span v-else class="text-sm font-medium">{{ initials }}</span>
      </button>
      <!-- 本人在线状态点 -->
      <span
        class="absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-rail)]"
        :class="app.present ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
      ></span>
    </div>

    <!-- 中部：聊天 / 通讯录（用户 2026-09-12 晚反馈：
         「整体参考图就是微信最左侧栏里的图标风格。选中态是实心的，而不是框框变颜色。
           现在上面的『聊天』和『通讯录』这两个图标不好看，参考微信。」）
         ⇒ 两点改动：
         ① **选中态改为实心**（图标本身 fill=currentColor + 主题色），不再用底色块；
         ② 「聊天」「通讯录」改为**自绘 SVG**（微信式造型 + 统一 1.9 线宽 / 22px 视觉盒）：
            · 聊天 = 带左下小尾巴的圆角气泡；
            · 通讯录 = 圆角卡片内镂空一个人像（`fill-rule="evenodd"` ⇒ 实心时人像成"洞"，
              与微信选中态那种"实心方块里透着人形"的观感一致）。
            自绘而不继续用 lucide：lucide 的线性图标**填充后大多变成墨团**（双人图标尤其），
            而这套需求要的正是"实心选中态"，所以造型必须自己控制。 -->
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
        <svg
          viewBox="0 0 24 24"
          class="h-[22px] w-[22px]"
          :fill="view === 'chats' ? 'currentColor' : 'none'"
          stroke="currentColor"
          stroke-width="1.9"
          stroke-linejoin="round"
        >
          <path d="M12 3.4c-4.9 0-8.7 3.1-8.7 7 0 2.2 1.3 4.2 3.3 5.5-.1 1-.5 2.2-1.3 3.3 0 0 2.4-.4 4-1.6.9.2 1.8.3 2.7.3 4.9 0 8.7-3.1 8.7-7s-3.8-7.5-8.7-7.5z" />
        </svg>
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
        <!-- 单条 path + evenodd：外框实心时人像自动成为镂空（微信选中态的观感）。
             未选中时只有描边，人像以线条呈现。 -->
        <svg
          viewBox="0 0 24 24"
          class="h-[22px] w-[22px]"
          :fill="view === 'contacts' ? 'currentColor' : 'none'"
          stroke="currentColor"
          stroke-width="1.9"
          stroke-linejoin="round"
          fill-rule="evenodd"
          clip-rule="evenodd"
        >
          <path
            d="M6 3.2h12A2.8 2.8 0 0 1 20.8 6v12A2.8 2.8 0 0 1 18 20.8H6A2.8 2.8 0 0 1 3.2 18V6A2.8 2.8 0 0 1 6 3.2z
               M12 7.4a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z
               M7.6 17.4c0-2.5 2-3.9 4.4-3.9s4.4 1.4 4.4 3.9z"
          />
        </svg>
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
        class="flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :title="app.dark ? t('nav.lightMode') : t('nav.darkMode')" :aria-label="app.dark ? t('nav.lightMode') : t('nav.darkMode')"
        @click="app.toggleDark()"
      >
        <Sun v-if="app.dark" class="h-5 w-5" />
        <Moon v-else class="h-5 w-5" />
      </button>
      <button
        class="flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :class="settingsOpening ? 'opacity-60' : ''"
        :aria-busy="settingsOpening"
        :title="t('nav.settings')" :aria-label="t('nav.settings')"
        @click="emit('open-settings')"
      >
        <svg viewBox="0 0 24 24" class="h-5 w-5" fill="none" stroke="currentColor" stroke-width="1.9">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.6 1.65 1.65 0 0 0 10 3.09V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </button>
      <button
        class="flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :class="logsOpening ? 'opacity-60' : ''"
        :aria-busy="logsOpening"
        :title="t('nav.logs')" :aria-label="t('nav.logs')"
        @click="emit('open-logs')"
      >
        <ScrollText class="h-5 w-5" />
      </button>
    </div>
  </aside>
</template>
