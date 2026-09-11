<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import { avatarInitial, nameToColor } from "@/utils/color";
import { Check, UserPlus } from "lucide-vue-next";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const loading = ref(false);
const keyword = ref("");

// 大规模局域网（设计规模 500–1000 节点）下最多渲染多少行。
//
// 原先这里截断到 200 行并提示"可用搜索缩小范围"——但那等于第 201 个之后的节点
// 压根找不到，是真实的功能缺口。现在行上加了 `content-visibility: auto`：
// 离屏行不参与布局与绘制，多渲染几百行几乎不花代价（节点表本身也已按 300ms 节流推送），
// 所以上限提到设计规模本身。保留上限只为挡住异常膨胀的节点表（例如广播洪水）。
const MAX_RENDER = 1000;

const friendIds = computed(() => new Set(chat.friends.map((f) => f.device_id)));

const filteredPeers = computed(() => {
  const k = keyword.value.trim().toLowerCase();
  const pool = k
    ? chat.peers.filter(
        (p) =>
          p.nickname.toLowerCase().includes(k) ||
          p.ip.toLowerCase().includes(k) ||
          p.device_id.toLowerCase().includes(k),
      )
    : chat.peers;
  return { total: pool.length, list: pool.slice(0, MAX_RENDER) };
});

watch(
  () => props.open,
  async (v) => {
    if (v) {
      keyword.value = "";
      loading.value = true;
      await chat.searchNearbyPeers(); // 按需 who_has 群发探测
      loading.value = false;
    }
  },
);

function initials(name: string) {
  return avatarInitial(name);
}

/** 发送好友申请后的冷却期：同一节点 3 秒内置灰防连点（显示「完成」）。 */
const SEND_COOLDOWN_MS = 3000;
const cooldown = ref<Record<string, number>>({});

function inCooldown(peerId: string): boolean {
  return Date.now() - (cooldown.value[peerId] ?? 0) < SEND_COOLDOWN_MS;
}

async function add(peerId: string) {
  if (inCooldown(peerId)) return; // 防抖：3 秒内不重复发送
  cooldown.value = { ...cooldown.value, [peerId]: Date.now() };
  setTimeout(() => {
    const next = { ...cooldown.value };
    delete next[peerId];
    cooldown.value = next;
  }, SEND_COOLDOWN_MS);
  try {
    await chat.sendFriendRequest(peerId);
    app.toast(t("friend.add.toast.sent"), "success");
  } catch (e) {
    app.toastError(e, t("msg.sendFailed"));
  }
}
</script>

<template>
  <BaseModal :open="open" :title="t('friend.add.title')" @close="emit('close')">
    <div v-if="loading" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
      {{ t("friend.add.scanning") }}
    </div>

    <div v-else-if="chat.peers.length === 0" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
      {{ t("friend.add.noPeers") }}
    </div>

    <template v-else>
      <input
        v-model="keyword"
        maxlength="100"
        class="mb-2 w-full rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
        :placeholder="t('friend.add.searchPlaceholder')"
      />
      <div class="max-h-72 overflow-y-auto">
        <div
          v-for="p in filteredPeers.list"
          :key="p.device_id"
          class="flex items-center gap-3 border-b border-[var(--gosslan-border)] px-1 py-2 last:border-0 [contain-intrinsic-size:auto_53px] [content-visibility:auto]"
        >
          <div
            class="flex h-9 w-9 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
            :style="{ backgroundColor: nameToColor(p.nickname) }"
          >
            <img alt="" v-if="p.avatar" :src="p.avatar" class="h-full w-full object-cover" />
            <span v-else class="text-sm font-semibold">{{ initials(p.nickname) }}</span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm">{{ p.nickname }}</div>
            <div class="text-xs text-[var(--gosslan-text-2)]">{{ p.ip }}</div>
          </div>
          <span
            v-if="friendIds.has(p.device_id)"
            class="flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-xs font-medium text-[var(--gosslan-text-2)]"
          >
            <Check class="h-3.5 w-3.5" />
            {{ t("friend.add.alreadyFriend") }}
          </span>
          <button
            v-else-if="inCooldown(p.device_id)"
            disabled
            class="flex cursor-default items-center gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-1.5 text-xs font-medium text-[var(--gosslan-text-2)]"
          >
            <Check class="h-3.5 w-3.5" />
            {{ t("friend.add.done") }}
          </button>
          <button
            v-else
            class="flex items-center gap-1 rounded-[var(--gosslan-radius-md)] bg-primary px-3 py-1.5 text-xs font-medium text-white transition hover:bg-primary-hover"
            @click="add(p.device_id)"
          >
            <UserPlus class="h-3.5 w-3.5" />
            {{ t("friend.add.add") }}
          </button>
        </div>
      </div>
      <div
        v-if="filteredPeers.total > MAX_RENDER"
        class="mt-1 text-center text-xs text-[var(--gosslan-text-2)]"
      >
        {{ t("friend.add.truncated", { shown: MAX_RENDER, total: filteredPeers.total }) }}
      </div>
    </template>
  </BaseModal>
</template>
