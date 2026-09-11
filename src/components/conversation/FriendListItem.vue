<script setup lang="ts">
import { t } from "@/i18n";
import { onUnmounted } from "vue";
import { avatarInitial, nameToColor } from "@/utils/color";
import { haptic } from "@/utils/haptics";
import type { Friend } from "@/types";

defineProps<{ friend: Friend; active: boolean }>();
const emit = defineEmits<{
  (e: "open", friend: Friend): void;
  /** 右键 / 移动端长按：上报坐标，由父组件定位菜单（x/y 为视口坐标）。 */
  (e: "context", friend: Friend, x: number, y: number): void;
}>();

function initials(name: string) {
  return avatarInitial(name);
}

function onContextMenu(friend: Friend, e: MouseEvent) {
  e.preventDefault();
  e.stopPropagation();
  emit("context", friend, e.clientX, e.clientY);
}

// 移动端没有 contextmenu：长按 500ms 视为「删除好友」入口
const LONG_PRESS_MS = 500;
let pressTimer: ReturnType<typeof setTimeout> | null = null;

function clearPress() {
  if (pressTimer) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
}

function onTouchStart(friend: Friend, e: TouchEvent) {
  const t = e.touches[0];
  if (!t) return;
  clearPress();
  pressTimer = setTimeout(() => {
    pressTimer = null;
    // 长按菜单弹出时给一次"重"触觉（对应 iOS 的 impact(.heavy) at menu appear）
    haptic("heavy");
    emit("context", friend, t.clientX, t.clientY);
  }, LONG_PRESS_MS);
}

onUnmounted(clearPress);
</script>

<template>
  <!-- 键盘可达（HIG "Full Keyboard Access"）：role=button + tabindex，Enter/Space 打开资料页。
       本行不是真 <button>，因为要挂 contextmenu / 长按等手势，用 div 更直接。 -->
  <div
    role="button"
    tabindex="0"
    class="relative flex h-[64px] cursor-pointer items-center gap-3 px-3 transition-colors"
    :class="active
      ? 'bg-[var(--gosslan-list-active)]'
      : 'hover:bg-[var(--gosslan-list-hover)]'"
    :aria-label="t('friend.listItem.aria', { name: friend.nickname, status: friend.online ? t('common.online') : t('common.offline') })"
    @click="emit('open', friend)"
    @keydown.enter.prevent="emit('open', friend)"
    @keydown.space.prevent="emit('open', friend)"
    @contextmenu="onContextMenu(friend, $event)"
    @touchstart.passive="onTouchStart(friend, $event)"
    @touchmove.passive="clearPress"
    @touchend.passive="clearPress"
    @touchcancel.passive="clearPress"
  >
    <!-- 微信式行间细分隔线：从文本列起（头像后缩进） -->
    <div class="absolute bottom-0 left-[64px] right-0 h-px bg-[var(--gosslan-divider)]"></div>
    <div class="relative shrink-0">
      <div
        class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
        :class="!friend.online ? 'grayscale opacity-70' : ''"
        :style="{ backgroundColor: nameToColor(friend.nickname) }"
      >
        <img alt="" v-if="friend.avatar" :src="friend.avatar" class="h-full w-full object-cover" />
        <span v-else class="text-sm font-medium">{{ initials(friend.nickname) }}</span>
      </div>
      <span
        class="absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-list)]"
        :class="friend.online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
      ></span>
    </div>
    <div class="min-w-0 flex-1">
      <!-- 名字同样会被截断，补 title（读屏有整行 aria-label，但悬停要能看全名）。 -->
      <div
        class="truncate text-[13px] leading-5"
        :class="active ? 'font-medium text-[var(--gosslan-text)]' : 'text-[var(--gosslan-text)]'"
        :title="friend.nickname"
      >
        {{ friend.nickname }}
      </div>
      <div
        class="truncate text-xs leading-5 text-[var(--gosslan-text-2)]"
        :title="friend.online ? t('common.online') : t('common.offline')"
      >
        {{ friend.online ? t("common.online") : t("common.offline") }}
      </div>
    </div>
  </div>
</template>
