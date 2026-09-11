<script setup lang="ts">
import {
  ArrowLeft,
  Bluetooth,
  FolderOpen,
  Monitor,
  Network,
  Pencil,
  Share2,
  Smartphone,
  Users,
  Wifi,
} from "lucide-vue-next";
import type { Conversation, LinkState } from "@/types";
import { t } from "@/i18n";

defineProps<{
  conv: Conversation | null;
  isGroup: boolean;
  online: boolean;
  memberCount: number;
  /** 单聊对方的设备类型（"desktop" / "mobile"，空串 = 未知/不显示）。 */
  deviceType?: string;
  /** 会话当前链路（最近一条消息的链路 + 跳数）。单聊显示。 */
  linkState?: LinkState | null;
  /** 仅群主可改名。 */
  canRename: boolean;
  /** 移动端：显示返回列表的箭头。 */
  showBack?: boolean;
}>();
const emit = defineEmits<{
  (e: "back"): void;
  (e: "open-members"): void;
  (e: "rename"): void;
  (e: "open-share"): void;
}>();

/** 连接图标：桥接（跳数>0）→ Share2；直连按 path 选 Wifi/Network/Bluetooth。 */
function linkIcon(path: string, hop: number): { icon: string; label: string } {
  if (hop > 0) {
    return {
      icon: "relay",
      label: t("chat.header.linkRelay", { n: hop }),
    };
  }
  if (path === "bluetooth") {
    return { icon: "bluetooth", label: t("chat.header.linkBluetooth") };
  }
  if (path === "routed") {
    return { icon: "routed", label: t("chat.header.linkRouted") };
  }
  return { icon: "lan", label: t("chat.header.linkLan") };
}
</script>

<template>
  <!-- 微信 4.0 头部：~56px 浅灰底，与消息区无缝衔接，无分割线；右侧 通话 / 更多 / 共享 -->
  <div
    class="flex shrink-0 items-center justify-between border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)] px-4"
    :style="{ height: 'var(--gosslan-header-h)' }"
  >
    <div class="flex min-w-0 items-center gap-1.5">
      <button
        v-if="showBack"
        class="tap-safe -ml-2 mr-1 flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.back')" :aria-label="t('chat.header.back')"
        @click="emit('back')"
      >
        <ArrowLeft class="h-5 w-5" />
      </button>
      <span class="truncate text-[15px] font-medium leading-6" :title="conv?.name || t('chat.header.conversation')">{{ conv?.name || t("chat.header.conversation") }}<template v-if="isGroup && memberCount > 0"> ({{ memberCount }})</template></span>
      <span
        v-if="!isGroup && deviceType"
        class="inline-flex shrink-0 items-center text-[var(--gosslan-text-2)]"
        :title="deviceType === 'mobile' ? t('chat.header.deviceMobile') : t('chat.header.deviceDesktop')"
        :aria-label="deviceType === 'mobile' ? t('chat.header.deviceMobile') : t('chat.header.deviceDesktop')"
      >
        <Smartphone v-if="deviceType === 'mobile'" class="h-4 w-4" />
        <Monitor v-else class="h-4 w-4" />
      </span>
      <span
        v-if="!isGroup && linkState"
        class="inline-flex shrink-0 items-center gap-0.5 text-[var(--gosslan-text-2)]"
        :title="linkIcon(linkState.path, linkState.hop).label"
        :aria-label="linkIcon(linkState.path, linkState.hop).label"
      >
        <Share2 v-if="linkIcon(linkState.path, linkState.hop).icon === 'relay'" class="h-4 w-4" />
        <Bluetooth v-else-if="linkIcon(linkState.path, linkState.hop).icon === 'bluetooth'" class="h-4 w-4" />
        <Network v-else-if="linkIcon(linkState.path, linkState.hop).icon === 'routed'" class="h-4 w-4" />
        <Wifi v-else class="h-4 w-4" />
        <span v-if="linkState.hop > 0" class="text-[11px] font-medium leading-none">{{ linkState.hop }}</span>
      </span>
    </div>
    <div class="flex shrink-0 items-center gap-0.5">
      <!-- 只保留有真实功能的入口；不放没有实现的功能按钮（音视频通话/更多已移除） -->
      <button
        v-if="isGroup"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.members')" :aria-label="t('chat.header.members')"
        @click="emit('open-members')"
      >
        <Users class="h-[18px] w-[18px]" />
      </button>
      <button
        v-if="isGroup && canRename"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.rename')" :aria-label="t('chat.header.rename')"
        @click="emit('rename')"
      >
        <Pencil class="h-[17px] w-[17px]" />
      </button>
      <button
        v-if="!isGroup"
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('chat.header.share')" :aria-label="t('chat.header.share')"
        @click="emit('open-share')"
      >
        <FolderOpen class="h-[18px] w-[18px]" />
      </button>
    </div>
  </div>
</template>
