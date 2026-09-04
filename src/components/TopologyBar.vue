<script setup lang="ts">
import { computed } from "vue";
import { useChatStore } from "@/stores/useChatStore";
import { Activity, Network, Server } from "lucide-vue-next";

const chat = useChatStore();

const rtt = computed(() => {
  if (chat.topology.avg_rtt_ms == null) return "—";
  return `${chat.topology.avg_rtt_ms}ms`;
});

const status = computed(() => (chat.topology.online ? "在线" : "离线"));
const statusColor = computed(() => (chat.topology.online ? "text-emerald-500" : "text-neutral-400"));
</script>

<template>
  <div
    class="flex items-center gap-4 px-4 text-xs text-[var(--gosslan-text-2)] border-b border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]"
    style="height: 32px"
  >
    <span class="flex items-center gap-1.5">
      <Network class="h-3.5 w-3.5" />
      <span>{{ chat.topology.node_count }} 节点</span>
    </span>
    <span class="flex items-center gap-1.5">
      <Server class="h-3.5 w-3.5" />
      <span>{{ chat.topology.relay_count }} 中继</span>
    </span>
    <span class="flex items-center gap-1.5">
      <Activity class="h-3.5 w-3.5" />
      <span>平均时延 {{ rtt }}</span>
    </span>
    <span class="ml-auto flex items-center gap-1.5">
      <span class="h-1.5 w-1.5 rounded-full" :class="chat.topology.online ? 'bg-emerald-500' : 'bg-neutral-400'"></span>
      <span :class="statusColor">{{ status }}</span>
    </span>
  </div>
</template>
