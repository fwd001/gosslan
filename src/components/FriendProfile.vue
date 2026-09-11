<script setup lang="ts">
/**
 * 好友资料页（用户 2026-09-12 晚 #14：「用户详情页的样式也参考微信。信息还是现有的」）。
 *
 * 微信 macOS 的资料页结构：顶部头像 + 昵称（+ 状态徽标），紧跟着几行**小字段**
 * （微信号 / 地区这类）；下面按分组罗列（朋友资料 / 更多信息），
 * 最底部是**图标在上、文字在下**的动作按钮（发消息 / 语音聊天 / 视频聊天）。
 * 这里照这个结构重排，但**字段沿用本应用现有的**（设备 ID / 指纹 / IP / 端口 / E2EE）——
 * 通信工具的身份核对面是设备指纹与公钥，不是微信号，凭空照搬反而误导。
 */
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
/** 连接地址：微信资料页在头部展示"地区/微信号"，这里对应我们的"怎么连上他"。 */
const address = computed(() => {
  const p = peer.value;
  return p && p.ip ? `${p.ip}${p.tcp_port ? `:${p.tcp_port}` : ""}` : "—";
});

const confirmRemove = ref(false);
</script>

<template>
  <div class="flex h-full flex-col bg-[var(--gosslan-chat)]">
    <!-- 移动端返回条：资料页占据整个内容区，需显式返回列表 -->
    <div
      v-if="app.isMobile"
      class="flex items-center gap-2 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)] px-2"
      :style="{ height: 'var(--gosslan-header-h)' }"
    >
      <button
        class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('common.back')" :aria-label="t('common.back')"
        @click="app.mobileView = 'list'"
      >
        <ArrowLeft class="h-5 w-5" />
      </button>
      <span class="truncate text-[15px] font-medium" :title="friend.nickname">{{ friend.nickname }}</span>
    </div>

    <div class="flex-1 overflow-y-auto px-6 py-6">
      <div class="mx-auto w-full max-w-[560px]">
        <!-- 头部：头像 + 昵称 + 状态徽标，右侧跟随几行小字段（微信式） -->
        <div class="flex items-start gap-4">
          <div
            class="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-2xl font-medium text-white"
            :class="!friend.online ? 'grayscale opacity-70' : ''"
            :style="{ backgroundColor: nameToColor(friend.nickname) }"
          >
            <img alt="" v-if="friend.avatar" :src="friend.avatar" class="h-full w-full object-cover" />
            <span v-else>{{ initial }}</span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="truncate text-xl font-semibold" :title="friend.nickname">{{ friend.nickname }}</span>
              <!-- 在线状态用一个小圆点 + 文案，位置与微信的"状态徽标"一致 -->
              <span
                class="inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px]"
                :class="friend.online
                  ? 'bg-[var(--gosslan-success-soft)] text-[var(--gosslan-success-ink)]'
                  : 'bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]'"
              >
                <span
                  class="h-1.5 w-1.5 rounded-full"
                  :class="friend.online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
                ></span>
                {{ friend.online ? t("common.online") : t("common.offline") }}
              </span>
            </div>
            <!-- 小字段：（微信这里是 微信号/地区；我们对应 设备指纹尾码/连接地址） -->
            <div class="mt-2 space-y-0.5 text-[12px] leading-relaxed text-[var(--gosslan-text-2)]">
              <div>{{ t("friend.profile.fingerprint") }}：<span class="font-mono">{{ shortId }}</span></div>
              <div class="truncate" :title="address">{{ t("friend.profile.ip") }}：<span class="font-mono">{{ address }}</span></div>
            </div>
          </div>
        </div>

        <!-- 分组一：朋友资料（沿用现有字段，微信式的"标题 + 卡片行"） -->
        <section class="mt-7">
          <h3 class="mb-2 px-1 text-xs font-medium tracking-wide text-[var(--gosslan-text-2)]">
            {{ t("friend.profile.title") }}
          </h3>
          <div class="overflow-hidden rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]">
            <div class="flex items-center justify-between gap-4 px-4 py-3 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.nickname") }}</span>
              <span class="truncate font-medium" :title="friend.nickname">{{ friend.nickname }}</span>
            </div>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-center justify-between gap-4 px-4 py-3 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.deviceType") }}</span>
              <span>{{ friend.device_type || "—" }}</span>
            </div>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-center justify-between gap-4 px-4 py-3 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.ip") }}</span>
              <span class="font-mono text-xs">{{ peer?.ip || "—" }}</span>
            </div>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-center justify-between gap-4 px-4 py-3 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.port") }}</span>
              <span class="font-mono text-xs">{{ peer?.tcp_port || "—" }}</span>
            </div>
          </div>
        </section>

        <!-- 分组二：更多信息（身份核对相关 —— 本应用的"微信号"就是设备 ID 与指纹） -->
        <section class="mt-6">
          <h3 class="mb-2 px-1 text-xs font-medium tracking-wide text-[var(--gosslan-text-2)]">
            {{ t("friend.profile.more") }}
          </h3>
          <div class="overflow-hidden rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]">
            <div class="flex items-start justify-between gap-4 px-4 py-3 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.deviceId") }}</span>
              <span class="break-all text-right font-mono text-xs">{{ friend.device_id }}</span>
            </div>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-center justify-between gap-4 px-4 py-3 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.e2ee") }}</span>
              <span class="text-right text-xs text-[var(--gosslan-success-ink)]">{{ t("friend.profile.e2eeOn") }}</span>
            </div>
          </div>
        </section>

        <p class="mt-3 px-1 text-xs leading-relaxed text-[var(--gosslan-text-2)]">
          {{ t("friend.profile.note") }}
        </p>

        <!-- 底部动作：微信式「图标在上、文字在下」居中排列 -->
        <div class="mt-8 flex items-start justify-center gap-12 pb-4">
          <button class="group flex flex-col items-center gap-2" @click="emit('send-message', friend.device_id)">
            <span
              class="grid h-11 w-11 place-items-center rounded-full bg-primary text-white transition group-hover:bg-primary-hover"
            >
              <MessageCircle class="h-5 w-5" />
            </span>
            <span class="text-[11px] text-[var(--gosslan-text-2)]">{{ t("common.sendMessage") }}</span>
          </button>
          <button class="group flex flex-col items-center gap-2" @click="confirmRemove = true">
            <span
              class="grid h-11 w-11 place-items-center rounded-full border border-[var(--gosslan-border)] text-[var(--gosslan-danger-ink)] transition group-hover:bg-[var(--gosslan-danger-soft)]"
            >
              <UserMinus class="h-5 w-5" />
            </span>
            <span class="text-[11px] text-[var(--gosslan-text-2)]">{{ t("common.deleteFriend") }}</span>
          </button>
        </div>
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
