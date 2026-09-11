<script setup lang="ts">
import { t } from "@/i18n";
import { fmtConversationTime } from "@/utils/time";
import { highlightText } from "@/utils/highlight";
import { avatarInitial, nameToColor } from "@/utils/color";
import { X } from "lucide-vue-next";
import { computed } from "vue";
import { useChatStore } from "@/stores/useChatStore";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { haptic } from "@/utils/haptics";
import UnreadBadge from "@/components/UnreadBadge.vue";
import type { Conversation } from "@/types";

const props = defineProps<{
  conv: Conversation;
  active: boolean;
  /** 单聊查好友表；群聊无在线概念，传 null 表示不显示状态点。 */
  online: boolean | null;
  /** 搜索命中摘要（null 时显示最后一条消息）。 */
  snippet: string | null;
  keyword: string;
}>();
const emit = defineEmits<{
  (e: "open", conv: Conversation): void;
  (e: "ask-delete", conv: Conversation, ev: MouseEvent): void;
}>();

const chat = useChatStore();
const { memberProfile } = useMemberProfile();

/**
 * 打开会话：先给一下「选择」触觉（切会话属于离散选择变化，对应 iOS 的
 * UISelectionFeedbackGenerator），再抛事件。触觉只在支持的平台生效。
 */
function openConv(conv: Conversation) {
  haptic("selection");
  emit("open", conv);
}

/** 群里有人 @ 我且未读（打开会话即清除）：微信式红色标签，显示在摘要前。 */
const mentioned = computed(() => chat.mentionedConvs.has(props.conv.id));

function initials(name: string) {
  return avatarInitial(name);
}

/** 群头像九宫格成员（微信式 2x2）：资料解析统一走 useMemberProfile（本机/好友/节点，
 *  离线好友照常显示）。必须保持响应式：好友/群数据是异步加载的，非响应式会在
 *  启动时算死成占位块且不再更新。九宫格每格用成员名 hash → nameToColor，保证
 *  同一成员在单聊列表和群聊九宫格里默认色一致。 */
const gridTiles = computed(() => {
  if (props.conv.kind !== "group") return [];
  const groupId = props.conv.id.replace(/^group:/, "");
  const memberIds = chat.groups.find((g) => g.id === groupId)?.members ?? [];
  const tiles: { avatar: string | null; label: string; color: string }[] = [];
  for (const id of memberIds.slice(0, 4)) {
    const p = memberProfile(id);
    tiles.push({ avatar: p.avatar, label: initials(p.name), color: nameToColor(p.name) });
  }
  while (tiles.length < Math.min(4, Math.max(memberIds.length, 1))) {
    tiles.push({ avatar: null, label: initials(props.conv.name), color: nameToColor(props.conv.name) });
  }
  return tiles;
});
</script>

<template>
  <!-- 微信 4.0：行高 ~64px、贴边；选中态中性浅灰底 + 正文深色文字（不用主题色/彩色，用户要求） -->
  <!-- 键盘可达（HIG "Full Keyboard Access"）：整行 role=button + tabindex，Enter/Space 激活。
       焦点环复用 style.css 的全局 :focus-visible 规则，无需额外样式。
       说明：删除键是本行的子元素，严格 ARIA 不建议在 role=button 里嵌交互元素；
       这里可接受——本行是 div[role=button] 而非真 <button>（真 button 才会把后代
       强制视为 presentational），后代仍留在无障碍树中，读屏能分别读到两者。 -->
  <div
    role="button"
    tabindex="0"
    class="group/conv relative flex h-[64px] cursor-pointer items-center gap-3 px-3 transition-colors"
    :class="active
      ? 'bg-[var(--gosslan-list-active)] text-[var(--gosslan-list-active-text)]'
      : 'hover:bg-[var(--gosslan-list-hover)]'"
    :aria-label="conv.unread > 0 ? t('conv.unread', { name: conv.name, n: conv.unread }) : conv.name"
    @click="openConv(conv)"
    @keydown.enter.prevent="openConv(conv)"
    @keydown.space.prevent="openConv(conv)"
  >
    <div class="relative shrink-0">
      <!-- 群聊：微信式 2x2 九宫格头像；单聊：单头像 -->
      <div
        v-if="conv.kind === 'group' && gridTiles.length"
        class="grid h-10 w-10 grid-cols-2 grid-rows-2 gap-px overflow-hidden rounded-[var(--gosslan-avatar-radius)] bg-[var(--gosslan-divider)]"
      >
        <div
          v-for="(t, i) in gridTiles"
          :key="i"
          class="flex items-center justify-center overflow-hidden text-[11px] font-medium text-white"
          :style="{ backgroundColor: t.color }"
        >
          <img alt="" v-if="t.avatar" :src="t.avatar" class="h-full w-full object-cover" />
          <span v-else>{{ t.label }}</span>
        </div>
      </div>
      <div
        v-else
        class="flex h-10 w-10 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
        :class="online === false ? 'grayscale opacity-70' : ''"
        :style="{ backgroundColor: nameToColor(conv.name) }"
      >
        <img alt="" v-if="conv.avatar" :src="conv.avatar" class="h-full w-full object-cover" />
        <span v-else class="text-sm font-medium">{{ initials(conv.name) }}</span>
      </div>
      <!-- 在线标识：群聊不显示；离线标灰半透 -->
      <span
        v-if="online !== null"
        class="absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-list)]"
        :class="online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
      ></span>
      <!-- 未读小红点：正常显示 -->
      <UnreadBadge v-if="conv.unread > 0" :count="conv.unread" class="absolute -right-1 -top-1" />
    </div>
    <div class="min-w-0 flex-1 overflow-hidden">
      <div class="flex items-center justify-between gap-2">
        <!-- 名字会被截断（`truncate`），必须给 title：否则悬停看不到完整名字。
             整行的 aria-label 只服务读屏，不产生 tooltip。 -->
        <span
          class="truncate text-[13px] leading-5"
          :class="active ? 'font-medium text-[var(--gosslan-list-active-text)]' : 'text-[var(--gosslan-text)]'"
          :title="conv.name"
        >
          {{ conv.name }}
        </span>
        <span
          class="shrink-0 whitespace-nowrap text-[11px]"
          :class="active ? 'text-[var(--gosslan-list-active-text)] opacity-90' : 'text-[var(--gosslan-text-2)]'"
        >
          {{ fmtConversationTime(conv.last_ts) }}
        </span>
      </div>
      <div class="mt-0.5 flex items-center justify-between gap-2">
        <span
          class="truncate text-[12px] leading-5"
          :class="active ? 'text-[var(--gosslan-list-active-text)] opacity-90' : 'text-[var(--gosslan-text-2)]'"
          :title="snippet || conv.last_msg || t('msg.noMessage')"
        >
          <template v-if="snippet">
            <span v-html="highlightText(snippet, keyword.trim())"></span>
          </template>
          <template v-else>
            <span v-if="mentioned" class="font-medium text-[var(--gosslan-danger-ink)]">{{ t("msg.mentioned") }}</span
            >{{ conv.last_msg || t("msg.noMessage") }}
          </template>
        </span>
      </div>
    </div>
    <!-- 微信式行间细分隔线：从文本列起（头像后缩进），最后一行不显（由容器裁边） -->
    <div class="absolute bottom-0 left-[64px] right-0 h-px bg-[var(--gosslan-divider)]"></div>
    <!-- 删除聊天记录入口：桌面端悬停行时浮现。
         `hover-reveal`：触屏没有 hover —— 没有它这个按钮在手机上永远不显示，
         等于「桌面能删、手机删不掉」（见 2026-09-10 审计 P0-1）。
         `tap-safe`：24px 小于 44pt 最小点按目标，触屏下垂直扩命中区（见 style.css）。 -->
    <button
      v-if="!active"
      class="hover-reveal tap-safe absolute bottom-1.5 right-1.5 z-10 hidden h-6 w-6 items-center justify-center rounded-[var(--gosslan-radius-xs)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-danger-soft)] hover:text-[var(--gosslan-danger-ink)] group-hover/conv:flex"
      :title="t('conv.delete')"
      :aria-label="t('conv.deleteAria', { name: conv.name })"
      @click="emit('ask-delete', conv, $event)"
    >
      <X class="h-3.5 w-3.5" />
    </button>
  </div>
</template>
