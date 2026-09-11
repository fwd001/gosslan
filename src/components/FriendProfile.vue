<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { ArrowLeft, MessageCircle, UserMinus } from "lucide-vue-next";
import BaseModal from "@/components/BaseModal.vue";
import { avatarInitial, nameToColor } from "@/utils/color";
import type { Friend } from "@/types";

const props = defineProps<{ friend: Friend }>();
const emit = defineEmits<{
  (e: "send-message", id: string): void;
  (e: "remove", f: Friend): void;
}>();

const app = useAppStore();
const chat = useChatStore();
/** 在线节点信息（IP / 端口 / 公钥），离线好友为 undefined */
const peer = computed(() => chat.peers.find((p) => p.device_id === props.friend.device_id));
const initial = computed(() => avatarInitial(props.friend.nickname));
/** 设备指纹尾码：用于当面核对身份（完整 ID 过长，不便口头比对） */
const shortId = computed(() => props.friend.device_id.slice(-8).toUpperCase());

const confirmRemove = ref(false);
</script>

<template>
  <div class="flex h-full flex-col bg-[var(--gosslan-chat)]">
    <!-- 移动端返回条：资料页占据整个内容区，需显式返回列表 -->
    <div
      v-if="app.isMobile"
      class="flex items-center gap-2 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)] px-2 py-2"
    >
      <button
        class="flex h-9 w-9 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('common.back')" :aria-label="t('common.back')"
        @click="app.mobileView = 'list'"
      >
        <ArrowLeft class="h-5 w-5" />
      </button>
      <span class="truncate text-sm font-semibold" :title="friend.nickname">{{ friend.nickname }}</span>
    </div>
    <div class="flex-1 overflow-y-auto px-6 py-8">
      <div class="mx-auto w-full max-w-[520px]">
        <!-- 头部：头像 + 昵称 + 在线状态 -->
        <div class="flex items-center gap-4">
          <div
            class="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-2xl font-medium text-white"
            :class="!friend.online ? 'grayscale opacity-70' : ''"
            :style="{ backgroundColor: nameToColor(friend.nickname) }"
          >
            <img alt="" v-if="friend.avatar" :src="friend.avatar" class="h-full w-full object-cover" />
            <span v-else>{{ initial }}</span>
          </div>
          <div class="min-w-0">
            <div class="truncate text-xl font-semibold" :title="friend.nickname">{{ friend.nickname }}</div>
            <div class="mt-1 flex items-center gap-1.5 text-sm text-[var(--gosslan-text-2)]">
              <span
                class="h-2 w-2 rounded-full"
                :class="friend.online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
              ></span>
              {{ friend.online ? t("common.online") : t("common.offline") }}
            </div>
          </div>
        </div>

        <!-- 操作：发消息 / 删除好友 -->
        <div class="mt-6 flex gap-3">
          <button
            class="flex flex-1 items-center justify-center gap-2 rounded-[var(--gosslan-avatar-radius)] bg-primary px-4 py-2.5 text-sm font-medium text-white transition hover:bg-primary-hover"
            @click="emit('send-message', friend.device_id)"
          >
            <MessageCircle class="h-4 w-4" />
            {{ t("common.sendMessage") }}
          </button>
          <button
            class="flex items-center justify-center gap-2 rounded-[var(--gosslan-avatar-radius)] border border-[var(--gosslan-border)] px-4 py-2.5 text-sm text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-danger-soft)]"
            @click="confirmRemove = true"
          >
            <UserMinus class="h-4 w-4" />
            {{ t("common.deleteFriend") }}
          </button>
        </div>

        <!-- 资料明细 -->
        <div class="mt-8 overflow-hidden rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]">
          <div class="border-b border-[var(--gosslan-border)] px-4 py-2.5 text-sm font-medium">{{ t("friend.profile.title") }}</div>
          <dl class="divide-y divide-[var(--gosslan-border)] text-sm">
            <div class="flex items-start justify-between gap-4 px-4 py-3">
              <dt class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.deviceId") }}</dt>
              <dd class="break-all text-right font-mono text-xs">{{ friend.device_id }}</dd>
            </div>
            <div class="flex items-center justify-between gap-4 px-4 py-3">
              <dt class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.fingerprint") }}</dt>
              <dd class="font-mono text-xs">{{ shortId }}</dd>
            </div>
            <div class="flex items-center justify-between gap-4 px-4 py-3">
              <dt class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.ip") }}</dt>
              <dd class="font-mono text-xs">{{ peer?.ip || "—" }}</dd>
            </div>
            <div class="flex items-center justify-between gap-4 px-4 py-3">
              <dt class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.port") }}</dt>
              <dd class="font-mono text-xs">{{ peer?.tcp_port || "—" }}</dd>
            </div>
            <div class="flex items-center justify-between gap-4 px-4 py-3">
              <dt class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.e2ee") }}</dt>
              <dd class="text-xs text-[var(--gosslan-success-ink)]">{{ t("friend.profile.e2eeOn") }}</dd>
            </div>
          </dl>
        </div>
        <p class="mt-3 px-1 text-xs leading-relaxed text-[var(--gosslan-text-2)]">
          {{ t("friend.profile.note") }}
        </p>
      </div>
    </div>

    <!-- 删除好友二次确认 -->
    <BaseModal :open="confirmRemove" :title="t('friend.remove.title')" @close="confirmRemove = false">
      <div class="space-y-3">
        <p class="text-sm">
          {{ t("friend.remove.bodyPrefix") }}<span class="font-medium text-[var(--gosslan-danger-ink)]">{{ friend.nickname }}</span>{{ t("friend.remove.bodySuffix") }}
        </p>
        <ul class="space-y-1 text-xs text-[var(--gosslan-text-2)]">
          <li>· {{ t("friend.remove.item1") }}</li>
          <li>· {{ t("friend.remove.item2") }}</li>
        </ul>
        <div class="flex justify-end gap-2 pt-2">
          <button
            class="rounded-[var(--gosslan-radius-md)] px-4 py-1.5 text-sm transition hover:bg-[var(--gosslan-hover)]"
            @click="confirmRemove = false"
          >{{ t("common.cancel") }}</button>
          <button
            class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-1.5 text-sm text-white transition hover:bg-[var(--gosslan-danger)]"
            @click="confirmRemove = false; emit('remove', friend)"
          >{{ t("common.delete") }}</button>
        </div>
      </div>
    </BaseModal>
  </div>
</template>
