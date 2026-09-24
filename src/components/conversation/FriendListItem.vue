<script setup lang="ts">
import { t } from "@/i18n";
import { computed, onUnmounted } from "vue";
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
import { haptic } from "@/utils/haptics";
import LinkIcon from "@/components/ui/LinkIcon.vue";
import { linkIconName, linkLabelKey } from "@/utils/peerConnectionInfo";
import type { Friend } from "@/types";

const props = defineProps<{
  friend: Friend;
  active: boolean;
  /**
   * 通讯录分组模式（用户需求 2026-09-12 第 17 条）：
   * 「这个头像可以小一些……但是离线、在线的这个还是要有的。就是它跟那个消息列表可以区别一下。」
   * compact = 头像 40→32px、行高 64→56px、缩进线跟着左移；**在线状态点保留**（离线仍标灰半透），
   * 只是省掉「在线/离线」文字行把列表压得更紧凑。
   */
  compact?: boolean;
  /**
   * 该好友**当前的链路类型**（来自 `peers-updated` 的 `Peer.link`：lan/routed/relay/bluetooth）。
   * 好友记录本身不带链路（`Friend` 没有 link 字段），由列表层按 device_id 关联节点表后传进来。
   * 无链路（离线 / 只发现未建链）时为空 ⇒ 不画图标。
   */
  link?: string | null;
  /**
   * 这一行就是**我自己**（用户 2026-09-24 #31：通讯录里认不出哪一个是自己）。
   * 「自己」在通讯录里是一条伪好友行（`ConversationList.selfFriend`），与好友同一条
   * 渲染/排序/搜索路径 ⇒ 只靠名字区分不了。判据由调用方给，本组件不读 store。
   */
  isSelf?: boolean;
}>();

/** 显示名与读屏文案共用同一份，避免"看得见（我）但读屏听不到"。 */
const displayName = computed(
  () => props.friend.nickname + (props.isSelf ? t("friend.selfSuffix") : ""),
);

/** 连接图标名；无真实链路时返回 null（模板据此不画图标 —— 画了就是骗）。判据与聊天头同源。 */
const linkIcon = computed(() => {
  const name = linkIconName({ link: props.link ?? null });
  return name === "discovered" ? null : name;
});
/** 图标的悬停/读屏文案（与资料页、聊天头同一套 `peer.link.*` 判据）。 */
const linkLabel = computed(() =>
  linkIcon.value ? t(linkLabelKey({ link: props.link ?? null })) : "",
);
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
  // 键盘唤起（Shift+F10 / 菜单键）时 clientX/Y 为 0 ⇒ 会用窗口左上角定位，
  // 与"这一行"对不上。此时改用行的矩形（见 ConversationListItem 同款处理）。
  const fromKeyboard = e.clientX === 0 && e.clientY === 0;
  if (fromKeyboard) {
    const rect = (e.currentTarget as HTMLElement | null)?.getBoundingClientRect();
    if (rect) {
      emit("context", friend, rect.left + 24, rect.bottom - 4);
      return;
    }
  }
  emit("context", friend, e.clientX, e.clientY);
}

// 移动端没有 contextmenu：长按 500ms 视为「删除好友」入口
const LONG_PRESS_MS = 500;
/** 手指抖动容差（px）：小于它不算"滑动"，不取消长按（否则轻微抖动就长按不出来）。 */
const PRESS_MOVE_TOLERANCE = 10;
let pressTimer: ReturnType<typeof setTimeout> | null = null;
let pressStart: { x: number; y: number } | null = null;
/**
 * 本次手势已经触发过长按 ⇒ 抑制随之而来的 click。
 * 不抑制的话同一次长按会「一边弹出菜单、一边把会话/资料页打开」，菜单刚出来就被盖住。
 * 只在**同一次手势内**生效：下一次 touchstart 会复位，所以不会吃掉用户的下一次点击。
 */
let suppressClick = false;

function clearPress() {
  if (pressTimer) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
  pressStart = null;
}

/** 位移超阈值才取消长按（横向滑动列表 / 纵向滚动时不误弹菜单）。 */
function onPressMove(e: TouchEvent) {
  const t = e.touches[0];
  if (!t || !pressStart) return;
  if (Math.hypot(t.clientX - pressStart.x, t.clientY - pressStart.y) > PRESS_MOVE_TOLERANCE) {
    clearPress();
  }
}

function onTouchStart(friend: Friend, e: TouchEvent) {
  const t = e.touches[0];
  if (!t) return;
  clearPress();
  suppressClick = false; // 新手势开始：复位上一次的抑制标记
  pressStart = { x: t.clientX, y: t.clientY };
  pressTimer = setTimeout(() => {
    pressTimer = null;
    suppressClick = true;
    // 长按菜单弹出时给一次"重"触觉（对应 iOS 的 impact(.heavy) at menu appear）
    haptic("heavy");
    emit("context", friend, t.clientX, t.clientY);
  }, LONG_PRESS_MS);
}

/** 打开资料页；吃掉"长按那次手势"遗留的 click（见 suppressClick）。 */
function openFriend(friend: Friend) {
  if (suppressClick) {
    suppressClick = false;
    return;
  }
  emit("open", friend);
}

onUnmounted(clearPress);
</script>

<template>
  <!-- 键盘可达（HIG "Full Keyboard Access"）：role=button + tabindex，Enter/Space 打开资料页。
       本行不是真 <button>，因为要挂 contextmenu / 长按等手势，用 div 更直接。 -->
  <div
    role="button"
    tabindex="0"
    class="relative flex cursor-pointer items-center gap-3 px-3 transition-colors"
    :class="[
      compact ? 'h-[56px]' : 'h-[64px]',
      active ? 'bg-[var(--gosslan-list-active)]' : 'hover:bg-[var(--gosslan-list-hover)]',
    ]"
    :aria-label="t('friend.listItem.aria', { name: displayName, status: friend.online ? t('common.online') : t('common.offline') })"
    @click="openFriend(friend)"
    @keydown.enter.prevent="openFriend(friend)"
    @keydown.space.prevent="openFriend(friend)"
    @contextmenu="onContextMenu(friend, $event)"
    @touchstart.passive="onTouchStart(friend, $event)"
    @touchmove.passive="onPressMove"
    @touchend.passive="clearPress"
    @touchcancel.passive="clearPress"
  >
    <!-- 微信式行间细分隔线：从文本列起（头像后缩进） -->
    <div
      class="absolute bottom-0 right-0 h-px bg-[var(--gosslan-divider)]"
      :class="compact ? 'left-[56px]' : 'left-[64px]'"
    ></div>
    <div class="relative shrink-0">
      <div
        class="gosslan-avatar-box flex items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
        :class="[compact ? 'h-8 w-8' : 'h-10 w-10', !friend.online ? 'grayscale opacity-70' : '']"
        :style="{ backgroundColor: nameToColor(friend.nickname) }"
      >
        <img alt="" v-if="friend.avatar" :src="friend.avatar" class="h-full w-full object-cover" />
        <span v-else class="gosslan-avatar-initial" :class="compact ? 'text-xs font-medium' : 'text-sm font-medium'" :data-len="avatarInitialLen(friend.nickname)">{{ initials(friend.nickname) }}</span>
      </div>
      <span
        class="absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-list)]"
        :class="friend.online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
      ></span>
    </div>
    <div class="min-w-0 flex-1">
      <!-- 名字同样会被截断，补 title（读屏有整行 aria-label，但悬停要能看全名）。 -->
      <div class="flex min-w-0 items-center gap-1">
        <!-- 连接类型图标（与聊天头/资料页同源判据，见 utils/peerConnectionInfo）：只在有真实链路时画。 -->
        <span
          v-if="linkIcon"
          class="inline-flex shrink-0 items-center text-[var(--gosslan-text-2)]"
          :title="linkLabel"
          aria-hidden="true"
        >
          <LinkIcon :name="linkIcon" class="h-3.5 w-3.5" />
        </span>
        <div
          class="truncate text-[13px] leading-5"
          :class="active ? 'font-medium text-[var(--gosslan-text)]' : 'text-[var(--gosslan-text)]'"
          :title="displayName"
        >
          {{ displayName }}
        </div>
      </div>
      <div
        v-if="!compact"
        class="truncate text-xs leading-5 text-[var(--gosslan-text-2)]"
        :title="friend.online ? t('common.online') : t('common.offline')"
      >
        {{ friend.online ? t("common.online") : t("common.offline") }}
      </div>
    </div>
  </div>
</template>
