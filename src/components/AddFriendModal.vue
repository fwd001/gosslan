<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import { Check, UserPlus } from "lucide-vue-next";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const loading = ref(false);
const keyword = ref("");

// 大规模局域网（500-1000 节点）下，列表按需过滤 + 截断渲染，避免一次挂载上千行
const MAX_RENDER = 200;

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
  return name.slice(0, 1).toUpperCase();
}

async function add(peerId: string) {
  try {
    await chat.sendFriendRequest(peerId);
    app.toast("好友申请已发送，等待对方确认", "success");
  } catch (e) {
    app.toast(`发送失败：${e}`, "error");
  }
}
</script>

<template>
  <BaseModal :open="open" title="添加好友" @close="emit('close')">
    <div v-if="loading" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
      正在扫描局域网节点…
    </div>

    <div v-else-if="chat.peers.length === 0" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
      未发现局域网节点，请先确保双方已启动网络
    </div>

    <template v-else>
      <input
        v-model="keyword"
        class="mb-2 w-full rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
        placeholder="搜索昵称 / IP / 设备 ID"
      />
      <div class="max-h-72 overflow-y-auto">
        <div
          v-for="p in filteredPeers.list"
          :key="p.device_id"
          class="flex items-center gap-3 border-b border-[var(--gosslan-border)] px-1 py-2 last:border-0"
        >
          <div class="flex h-9 w-9 items-center justify-center overflow-hidden rounded-full bg-primary text-white">
            <img v-if="p.avatar" :src="p.avatar" class="h-full w-full object-cover" />
            <span v-else class="text-sm font-semibold">{{ initials(p.nickname) }}</span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm">{{ p.nickname }}</div>
            <div class="text-xs text-[var(--gosslan-text-2)]">{{ p.ip }}</div>
          </div>
          <span
            v-if="friendIds.has(p.device_id)"
            class="flex items-center gap-1 rounded-lg px-3 py-1.5 text-xs font-medium text-[var(--gosslan-text-2)]"
          >
            <Check class="h-3.5 w-3.5" />
            已加好友
          </span>
          <button
            v-else
            class="flex items-center gap-1 rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-white transition hover:bg-primary-hover"
            @click="add(p.device_id)"
          >
            <UserPlus class="h-3.5 w-3.5" />
            加好友
          </button>
        </div>
      </div>
      <div
        v-if="filteredPeers.total > MAX_RENDER"
        class="mt-1 text-center text-xs text-[var(--gosslan-text-2)]"
      >
        仅显示前 {{ MAX_RENDER }} 个，共 {{ filteredPeers.total }} 个节点，可用搜索缩小范围
      </div>
    </template>
  </BaseModal>
</template>
