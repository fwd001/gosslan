<script setup lang="ts">
/**
 * 好友资料页（用户 2026-09-12 晚 #14：「用户详情页的样式也参考微信。信息还是现有的」）。
 *
 * 微信 macOS 的资料页结构：顶部头像 + 昵称（+ 状态徽标），紧跟着几行**小字段**
 * （微信号 / 地区这类）；下面按分组罗列（朋友资料 / 更多信息），
 * 最底部是**图标在上、文字在下**的动作按钮（发消息 / 语音聊天 / 视频聊天）。
 * 这里照这个结构重排，但**字段沿用本应用现有的**（设备 ID / 安全码 / IP / 端口 / E2EE）——
 * 通信工具的身份核对面是公钥派生的安全码，不是微信号，凭空照搬反而误导。
 */
import { api } from "@/api";
import type { LinkState } from "@/types";
import { t } from "@/i18n";
import {
  addressText,
  deviceTypeKey,
  linkLabelKey,
  linkLabelParams,
} from "@/utils/peerConnectionInfo";
import { computed, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useClipboard } from "@/composables/useClipboard";
import { ArrowLeft, Copy, MessageCircle, UserMinus } from "lucide-vue-next";
import BaseModal from "@/components/BaseModal.vue";
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
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
/** 这条资料是「我自己」（通讯录里的「自己」走同一渲染路径）：安全码/连接地址/删除好友都不适用。 */
const isSelf = computed(() => props.friend.device_id === app.device?.device_id);
const initial = computed(() => avatarInitial(props.friend.nickname));
/**
 * **安全码**：本机与这位好友之间那串双方一致的数字，供带外核对。
 *
 * 这里**刻意不再用「device_id 尾 8 位」**当核对码。尾码只反映对方**自称**的 ID，
 * 而 ID 在广播里是明文 —— 冒充者伪造同一个 device_id 就能得出同样的尾码，
 * 看着"对得上"却毫无防护作用，比不给还危险。
 * 安全码由双方的公钥派生，冒充者手里的密钥不同，算出来必然不同。
 *
 * `null` = 还缺对方公钥（尚未通过 Hello/announce 学到）：如实显示"暂时算不出"，
 * **不拿 device_id 凑一个**，凑出来的码在真正的攻击下会误导用户。
 */
const safetyNumber = ref<string | null>(null);
const safetyFailed = ref(false);

watch(
  () => props.friend.device_id,
  async (id) => {
    safetyNumber.value = null;
    safetyFailed.value = false;
    if (id === app.device?.device_id) return; // 自己没有"双方安全码"这回事
    try {
      safetyNumber.value = await api.getSafetyNumber(id);
    } catch {
      safetyFailed.value = true;
    }
  },
  { immediate: true },
);

/** 首段（用于头部的快速一瞥）：完整 6 段太长，不适合放在小字段里。 */
const safetyFirstGroup = computed(() => safetyNumber.value?.split(" ")[0] ?? null);

const { copyContent } = useClipboard();

async function copySafetyNumber() {
  if (!safetyNumber.value) return;
  const ok = await copyContent("safety", safetyNumber.value);
  app.toast(
    ok ? t("friend.profile.safetyCopied") : t("friend.profile.safetyFail"),
    ok ? "success" : "error",
  );
}
/** 中继跳数：与聊天头部那颗链路徽标**同一来源**（`get_conv_link`），不另造一份。 */
const convLink = ref<LinkState | null>(null);
watch(
  () => props.friend.device_id,
  async (id) => {
    convLink.value = id ? await api.getConvLink(id).catch(() => null) : null;
  },
  { immediate: true },
);
/** 连接信息：**按链路类型说各自的事实**（见 utils/peerConnectionInfo.ts 的约定）。 */
const linkInfo = computed(() => ({
  // `Peer.link` 是**活链路**（最权威）；没有活链路时退回会话链路快照
  link: peer.value?.link ?? convLink.value?.path ?? null,
  ip: peer.value?.ip ?? null,
  tcp_port: peer.value?.tcp_port ?? null,
  hop: convLink.value?.hop ?? 0,
  online: props.friend.online,
  device_type: props.friend.device_type,
}));
/** 头部那行"怎么连上他"：蓝牙说"蓝牙直连"，局域网给 ip:port，中继说"经 N 跳"。 */
const address = computed(() => {
  const info = linkInfo.value;
  const label = t(linkLabelKey(info), linkLabelParams(info));
  const addr = addressText(info);
  return addr ? `${label} · ${addr}` : label;
});
/** 地址行：蓝牙/中继**不显示**（蓝牙上没有 IP，中继没有直连地址）。 */
const addressLine = computed(() => addressText(linkInfo.value));

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
        <!-- 头部（紧凑版）：头像 + 昵称行（含在线状态 + 发消息按钮一行内）
             信息密度翻倍：原来 mt-7 才到分组卡，现在 mt-3 就到了。
             发消息按钮放到头像右侧，一进来就能点，不用滑。 -->
        <div class="flex items-start gap-4">
          <div
            class="gosslan-avatar-box flex h-14 w-14 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-xl font-medium text-white"
            :class="!friend.online ? 'grayscale opacity-70' : ''"
            :style="{ backgroundColor: nameToColor(friend.nickname) }"
          >
            <img alt="" v-if="friend.avatar" :src="friend.avatar" class="h-full w-full object-cover" />
            <span v-else class="gosslan-avatar-initial" :data-len="avatarInitialLen(friend.nickname)">{{ initial }}</span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="truncate text-lg font-semibold" :title="friend.nickname">{{ friend.nickname }}</span>
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
              <!-- 发消息按钮：primary，跟在昵称后面，一抬头就能点；
                   shrink-0 whitespace-nowrap：长昵称时不被挤得换行（移动端窄屏最容易撞此问题）。 -->
              <button
                class="tap-safe ml-auto inline-flex h-7 shrink-0 items-center gap-1 whitespace-nowrap rounded-full bg-primary px-3 text-[12px] font-medium text-white transition hover:bg-primary-hover"
                @click="emit('send-message', friend.device_id)"
              >
                <MessageCircle class="h-3.5 w-3.5" />
                {{ t("common.sendMessage") }}
              </button>
            </div>
            <!-- 次行：连接方式 + 安全码首段（一行，不再两行） -->
            <div v-if="!isSelf" class="mt-1 flex items-center gap-3 truncate text-[12px] leading-relaxed text-[var(--gosslan-text-2)]" :title="safetyFirstGroup ? `${address} · ${safetyFirstGroup}` : address">
              <span class="truncate" :title="address">{{ t("peer.link.label") }}：<span class="font-mono">{{ address }}</span></span>
              <span class="h-3 w-px shrink-0 bg-[var(--gosslan-divider)]"></span>
              <span>{{ t("friend.profile.safetyFirst") }}：<span class="font-mono">{{ safetyFirstGroup ?? t("common.notSet") }}</span><span v-if="safetyFirstGroup" class="opacity-60">…</span></span>            </div>
          </div>
        </div>

        <!-- 单张合并卡：去掉两个 section 标题 + 收紧 py-3 → py-2.5。
             重要性排序：连接方式 → 设备 → 端口/IP → 安全码 → device_id → E2EE。
             原来两张卡（朋友资料 + 更多信息）共 5 行标题 + 4 条分割线 + mt-7 + mt-6，
             现在一张卡，总高度砍半。 -->
        <section class="mt-4">          <div class="overflow-hidden rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]">
            <!-- 安全码（防中间人核心，放在最上面让它显眼，但不再占两大块）。「自己」不适用 -->
            <template v-if="!isSelf">
            <div class="flex items-start justify-between gap-3 px-4 py-2.5">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <span class="text-sm text-[var(--gosslan-text-2)]">{{ t("friend.profile.safety") }}</span>
                  <button
                    v-if="safetyNumber"
                    class="tap-safe flex h-6 w-6 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
                    :title="t('friend.profile.safetyCopy')" :aria-label="t('friend.profile.safetyCopy')"
                    @click="copySafetyNumber"
                  >
                    <Copy class="h-3 w-3" />
                  </button>
                </div>
                <div
                  v-if="safetyNumber"
                  class="mt-0.5 font-mono text-[14px] tracking-wide text-[var(--gosslan-text)]"
                  :title="safetyNumber"
                >
                  {{ safetyNumber }}
                </div>
                <div v-else class="mt-0.5 text-[12px] text-[var(--gosslan-text-2)]">
                  {{ safetyFailed ? t("friend.profile.safetyFail") : t("friend.profile.safetyUnavailable") }}
                </div>
              </div>
            </div>
            </template>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-center justify-between gap-4 px-4 py-2.5 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.deviceType") }}</span>
              <span>{{ t(deviceTypeKey(friend.device_type)) }}</span>
            </div>
            <!-- 版本行：只在"知道点什么"时出现。老实例（两个字段都没报）整行不显示 ——
                 不猜版本、也不用"未知"占位糊一面墙。 -->
            <template v-if="friend.peer_app_version || friend.peer_version_newer">
              <div class="h-px bg-[var(--gosslan-divider)]"></div>
              <div class="flex items-start justify-between gap-4 px-4 py-2.5 text-sm">
                <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.version") }}</span>
                <span class="text-right">
                  <span>{{ friend.peer_app_version || t("friend.profile.versionUnknown") }}</span>
                  <!-- 这才是 INV-P24 要的"可解释状态"：说清差异、说清该升级哪一台 -->
                  <span
                    v-if="friend.peer_version_newer"
                    class="mt-0.5 block text-xs text-[var(--gosslan-warning-ink)]"
                  >
                    {{ t("friend.profile.versionNewerHint") }}
                  </span>
                </span>
              </div>
            </template>
            <template v-if="addressLine">
              <div class="h-px bg-[var(--gosslan-divider)]"></div>
              <div class="flex items-center justify-between gap-4 px-4 py-2.5 text-sm">
                <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.ip") }}</span>
                <span class="truncate font-mono text-xs" :title="addressLine">{{ addressLine }}</span>
              </div>
            </template>
            <template v-if="peer?.tcp_port">
              <div class="h-px bg-[var(--gosslan-divider)]"></div>
              <div class="flex items-center justify-between gap-4 px-4 py-2.5 text-sm">
                <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.port") }}</span>
                <span class="font-mono text-xs">{{ peer.tcp_port }}</span>
              </div>
            </template>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-start justify-between gap-4 px-4 py-2.5 text-sm">
              <span class="shrink-0 pt-0.5 text-[var(--gosslan-text-2)]">{{ t("friend.profile.deviceId") }}</span>              <span class="break-all text-right font-mono text-xs">{{ friend.device_id }}</span>
            </div>
            <div class="h-px bg-[var(--gosslan-divider)]"></div>
            <div class="flex items-center justify-between gap-4 px-4 py-2.5 text-sm">
              <span class="shrink-0 text-[var(--gosslan-text-2)]">{{ t("friend.profile.e2ee") }}</span>
              <span class="text-right text-xs text-[var(--gosslan-success-ink)]">{{ t("friend.profile.e2eeOn") }}</span>
            </div>
          </div>
        </section>

        <!-- 底部只留"删除好友"——发消息按钮已经在头部。用紧凑横排而不是图标在上的大按钮。 -->
        <div class="mt-4 flex justify-center pb-4">
          <button
            v-if="!isSelf"
            class="tap-safe inline-flex items-center gap-1.5 rounded-full border border-[var(--gosslan-border)] px-4 py-1.5 text-[12px] text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-danger-soft)]"
            @click="confirmRemove = true"
          >
            <UserMinus class="h-3.5 w-3.5" />
            {{ t("common.deleteFriend") }}          </button>
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
