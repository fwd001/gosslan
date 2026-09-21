<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useExclusivePopup } from "@/composables/useExclusivePopup";
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
import { MoreHorizontal, Moon, ScrollText, Settings, Sun } from "lucide-vue-next";
import UnreadBadge from "@/components/UnreadBadge.vue";
import { t } from "@/i18n";

const NAV_STATES = ["chats", "contacts", "links", "favorites", "me"] as const;
type NavState = (typeof NAV_STATES)[number];

defineProps<{
  /** 当前导航状态——**单一数据源**，所有按钮互斥高亮由这一个值决定。
   * 取代之前分散的 `view` + `favoritesOpen` 两个独立 ref（那两者会同时为真导致"多选"）。 */
  navState: NavState;
  /** 正在打开独立窗口时的忙碌态（单飞/防抖状态在 `useWindowLauncher` 里，见该文件）。 */
  settingsOpening?: boolean;
  logsOpening?: boolean;
}>();
const emit = defineEmits<{
  (e: "update:navState", v: NavState): void;
  (e: "open-settings"): void;
  (e: "open-logs"): void;
}>();

const app = useAppStore();
const chat = useChatStore();
const initials = computed(() => avatarInitial(app.device?.nickname));

// ---------------- 二级菜单（设置 / 运行日志 收进这里，用户 2026-09-17） ----------------
// 惯用法与 ConversationList 的「+」菜单完全一致（useExclusivePopup 互斥 + 键盘可达）。
const morePopup = useExclusivePopup("rail-more");
const moreOpen = ref(false);
const moreBtnRef = ref<HTMLButtonElement | null>(null);
const moreMenuRef = ref<HTMLDivElement | null>(null);

watch(morePopup.isActive, (mine) => {
  if (!mine && moreOpen.value) moreOpen.value = false;
});

function toggleMore() {
  if (moreOpen.value) {
    closeMore();
    return;
  }
  moreOpen.value = true;
  morePopup.claim();
  void nextTick(() => {
    moreMenuRef.value?.querySelector<HTMLElement>(".gosslan-menu-item:not([disabled])")?.focus();
  });
}

function closeMore() {
  morePopup.release();
  moreOpen.value = false;
  // 关闭后把焦点还给触发按钮（HIG：浮层关闭焦点不丢）
  const active = document.activeElement;
  const stuck = active === document.body || (!!moreMenuRef.value && moreMenuRef.value.contains(active));
  if (stuck) void nextTick(() => moreBtnRef.value?.focus());
}

/** 二级菜单的键盘可达（Esc / ↑↓ 循环 / Home / End），与「+」菜单同一套约定。 */
function onMoreMenuKey(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault();
    closeMore();
    return;
  }
  const items = Array.from(
    moreMenuRef.value?.querySelectorAll<HTMLElement>(".gosslan-menu-item:not([disabled])") ?? [],
  );
  if (items.length === 0) return;
  const idx = items.indexOf(document.activeElement as HTMLElement);
  if (e.key === "ArrowDown") {
    e.preventDefault();
    items[(idx + 1) % items.length]?.focus();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    items[(idx - 1 + items.length) % items.length]?.focus();
  } else if (e.key === "Home") {
    e.preventDefault();
    items[0]?.focus();
  } else if (e.key === "End") {
    e.preventDefault();
    items[items.length - 1]?.focus();
  }
}

function onDocClick(e: MouseEvent) {
  const target = e.target as HTMLElement;
  if (moreOpen.value && !moreBtnRef.value?.contains(target) && !moreMenuRef.value?.contains(target)) {
    closeMore();
  }
}
onMounted(() => document.addEventListener("click", onDocClick));
onUnmounted(() => document.removeEventListener("click", onDocClick));
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
        class="gosslan-avatar-box flex h-10 w-10 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white transition hover:opacity-90"
        :class="settingsOpening ? 'opacity-60' : ''"
        :aria-busy="settingsOpening"
        :style="{ backgroundColor: nameToColor(app.device?.nickname ?? '') }"
        :title="app.present ? t('nav.me.online') : t('nav.me.offline')"
        :aria-label="t('nav.me.openSettings', { status: app.present ? t('nav.me.online') : t('nav.me.offline') })"
        @click="emit('open-settings')"
      >
        <img alt="" v-if="app.device?.avatar" :src="app.device.avatar" class="h-full w-full object-cover" />
        <span
          v-else
          class="gosslan-avatar-initial text-sm font-medium"
          :data-len="avatarInitialLen(app.device?.nickname)"
          >{{ initials }}</span
        >
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
        :class="navState === 'chats'
          ? 'text-[var(--gosslan-rail-text-active)]'
          : 'text-[var(--gosslan-rail-text)] hover:bg-[var(--gosslan-rail-hover)]'"
        :title="t('nav.chats')"
        :aria-label="chat.totalUnread > 0 ? t('nav.chats.unread', { n: chat.totalUnread }) : t('nav.chats')"
        @click="emit('update:navState', 'chats')"
      >
        <svg
          viewBox="0 0 24 24"
          class="h-[22px] w-[22px]"
          :fill="navState === 'chats' ? 'currentColor' : 'none'"
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
        :class="navState === 'contacts'
          ? 'text-[var(--gosslan-rail-text-active)]'
          : 'text-[var(--gosslan-rail-text)] hover:bg-[var(--gosslan-rail-hover)]'"
        :title="t('nav.contacts')"
        :aria-label="chat.pendingRequests.length ? t('nav.contacts.pending', { n: chat.pendingRequests.length }) : t('nav.contacts')"
        @click="emit('update:navState', 'contacts')"
      >
        <!-- 单条 path + evenodd：外框实心时人像自动成为镂空（微信选中态的观感）。
             未选中时只有描边，人像以线条呈现。 -->
        <svg
          viewBox="0 0 24 24"
          class="h-[22px] w-[22px]"
          :fill="navState === 'contacts' ? 'currentColor' : 'none'"
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
      <!-- 收藏（微信最左侧栏就是这个位置）。
           自绘五角星而不是 lucide 的 Star：本栏的选中态靠"填充成实心"表达，
           而 lucide 的线性图标填充后会变成墨团（见上面「聊天/通讯录」的说明）。 -->
      <button
        class="relative flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] transition"
        :class="navState === 'favorites'
          ? 'text-[var(--gosslan-rail-text-active)]'
          : 'text-[var(--gosslan-rail-text)] hover:bg-[var(--gosslan-rail-hover)]'"
        :title="t('nav.favorites')"
        :aria-label="t('nav.favorites')"
        @click="emit('update:navState', navState === 'favorites' ? 'chats' : 'favorites')"
      >
        <svg
          viewBox="0 0 24 24"
          class="h-[22px] w-[22px]"
          :fill="navState === 'favorites' ? 'currentColor' : 'none'"
          stroke="currentColor"
          stroke-width="1.9"
          stroke-linejoin="round"
        >
          <path d="M12 3.6l2.6 5.3 5.8.8-4.2 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.2-4.1 5.8-.8z" />
        </svg>
      </button>
      <!-- 外部链接：点开把左列切成「链接」列表（用户 2026-09-17）。图标用**指南针**造型
           （用户指定：链接入口要有"探索/发现"的指向感，而不是一条链子）。 -->
      <button
        class="relative flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] transition"
        :class="navState === 'links'
          ? 'text-[var(--gosslan-rail-text-active)]'
          : 'text-[var(--gosslan-rail-text)] hover:bg-[var(--gosslan-rail-hover)]'"
        :title="t('nav.links')"
        :aria-label="t('nav.links')"
        @click="emit('update:navState', 'links')"
      >
        <!-- 指南针：**与「聊天 / 通讯录 / 收藏」同一套选中逻辑**（选中 = 实心 + 主题色）。
             自绘而不是用 lucide 的 `Compass`：lucide 是描边图标，直接给它 fill 会变成一个墨团
             （见上面「聊天/通讯录」的说明）。这里用**单条 path + evenodd**：外圆填充成实心时，
             里面的指针自然成为镂空 —— 与通讯录那张"实心卡片里透着人形"同一手法；
             未选中时只有描边，指针以线条呈现。
             用户 2026-09-21：「链接这个指南针为什么选中的时候不是实心选中变色的」——
             根因就是这里漏了 fill 绑定（前三个图标都有，只有它还是 lucide 原样）。 -->
        <svg
          viewBox="0 0 24 24"
          class="h-[22px] w-[22px]"
          :fill="navState === 'links' ? 'currentColor' : 'none'"
          stroke="currentColor"
          stroke-width="1.9"
          stroke-linejoin="round"
          fill-rule="evenodd"
          clip-rule="evenodd"
        >
          <path
            d="M12 2.6a9.4 9.4 0 1 0 0 18.8 9.4 9.4 0 0 0 0-18.8z
               M17.5 6.5l-2.7 8.3-8.3 2.7 2.7-8.3z"
          />
        </svg>
      </button>
    </div>

    <!-- 底部：深浅色切换 + 更多（设置/运行日志收进**二级菜单**，用户 2026-09-17） -->
    <div class="mt-auto flex flex-col items-center gap-2">
      <button
        class="flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
        :title="app.dark ? t('nav.lightMode') : t('nav.darkMode')" :aria-label="app.dark ? t('nav.lightMode') : t('nav.darkMode')"
        @click="app.toggleDark()"
      >
        <Sun v-if="app.dark" class="h-5 w-5" />
        <Moon v-else class="h-5 w-5" />
      </button>
      <div class="relative">
        <button
          ref="moreBtnRef"
          class="flex h-11 w-11 items-center justify-center rounded-[var(--gosslan-radius-lg)] text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-rail-hover)]"
          :title="t('nav.more')" :aria-label="t('nav.more')"
          :aria-haspopup="moreOpen ? 'menu' : undefined" :aria-expanded="moreOpen"
          @click.stop="toggleMore"
        >
          <MoreHorizontal class="h-5 w-5" />
        </button>
        <!-- 二级菜单：向右弹出（rail 只有 64px 宽，向下会顶出屏幕） -->
        <div
          v-if="moreOpen"
          ref="moreMenuRef"
          class="frost gosslan-menu absolute bottom-0 left-full z-30 ml-2"
          role="menu"
          aria-orientation="vertical"
          @keydown="onMoreMenuKey"
        >
          <button
            role="menuitem"
            class="gosslan-menu-item"
            :aria-busy="settingsOpening"
            @click.stop="closeMore(); emit('open-settings')"
          >
            <Settings class="h-4 w-4" />
            {{ t("nav.settings") }}
          </button>
          <button
            role="menuitem"
            class="gosslan-menu-item"
            :aria-busy="logsOpening"
            @click.stop="closeMore(); emit('open-logs')"
          >
            <ScrollText class="h-4 w-4" />
            {{ t("nav.logs") }}
          </button>
        </div>
      </div>
    </div>
  </aside>
</template>
