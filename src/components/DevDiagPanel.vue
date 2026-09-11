<script setup lang="ts">
import { t } from "@/i18n";
import { ref, onMounted } from "vue";
import BaseModal from "@/components/BaseModal.vue";
import { api } from "@/api";
import type { DiscoveryDiag, InterfaceCandidate } from "@/types";

defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const diag = ref<DiscoveryDiag | null>(null);
const candidates = ref<InterfaceCandidate[]>([]);
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    const [d, c] = await Promise.all([api.getDiscoveryDiag(), api.getInterfaceCandidates()]);
    diag.value = d;
    // 标记被选中的接口
    const selectedIp = d.selected_ip || d.bound_ip;
    candidates.value = c.map((x) => ({ ...x, selected: x.ip === selectedIp }));
  } catch {
    /* ignore */
  } finally {
    loading.value = false;
  }
}

onMounted(refresh);

function fmtTs(ts: number) {
  if (!ts) return "-";
  return new Date(ts).toLocaleTimeString();
}
</script>

<template>
  <BaseModal :open="open" :title="t('diag.title')" width="max-w-3xl" @close="emit('close')">
    <div class="max-h-[75vh] overflow-y-auto space-y-5 text-xs leading-relaxed">
      <!-- 刷新 -->
      <div class="flex items-center justify-between">
        <button class="rounded-[var(--gosslan-radius-sm)] px-2.5 py-1 transition hover:bg-black/5 dark:hover:bg-white/10" @click="refresh">
          {{ loading ? t("diag.refreshing") : t("diag.refresh") }}
        </button>
      </div>

      <template v-if="diag">
        <!-- 网络模式 -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.networkStatus") }}</h4>
          <div class="grid grid-cols-2 gap-x-6 gap-y-1">
            <div><span class="opacity-60">{{ t("diag.mode") }}</span>{{ diag.mode }}</div>
            <div><span class="opacity-60">{{ t("diag.boundIp") }}</span>{{ diag.bound_ip || "-" }}</div>
            <div><span class="opacity-60">{{ t("diag.selectedInterface") }}</span>{{ diag.selected_interface || t("diag.auto") }}</div>
            <div><span class="opacity-60">{{ t("diag.selectedIp") }}</span>{{ diag.selected_ip || diag.bound_ip || "-" }}</div>
            <div><span class="opacity-60">{{ t("diag.tcpListen") }}</span>{{ diag.tcp_listen || "-" }}</div>
            <div><span class="opacity-60">{{ t("diag.udpPort") }}</span>{{ diag.udp_port || "-" }}</div>
          </div>
        </section>

        <!-- Broadcast / Multicast -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.discovery") }}</h4>
          <div class="grid grid-cols-2 gap-x-6 gap-y-1">
            <div><span class="opacity-60">{{ t("diag.broadcastTarget") }}</span>{{ diag.broadcast_target || "-" }}</div>
            <div><span class="opacity-60">{{ t("diag.multicastGroup") }}</span>{{ diag.multicast_group || "-" }}</div>
            <div>
              <span class="opacity-60">multicast_if：</span>
              <span :class="diag.multicast_if_result.startsWith('ok') || diag.multicast_if_result === 'not_set' ? '' : 'text-[var(--gosslan-danger-ink)]'">
                {{ diag.multicast_if_result || "-" }}
              </span>
            </div>
            <div>
              <span class="opacity-60">join_multicast：</span>
              <span :class="diag.multicast_join_result.startsWith('ok') ? '' : 'text-[var(--gosslan-danger-ink)]'">
                {{ diag.multicast_join_result || "-" }}
              </span>
            </div>
          </div>
        </section>

        <!-- 候选接口 -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.candidates") }}</h4>
          <div class="overflow-x-auto">
            <table class="w-full text-left">
              <thead>
                <tr class="border-b border-[var(--gosslan-border)] opacity-60">
                  <th class="pr-3 py-1">{{ t("diag.name") }}</th>
                  <th class="pr-3 py-1">IPv4</th>
                  <th class="pr-3 py-1">Broadcast</th>
                  <th class="pr-3 py-1">RFC1918</th>
                  <th class="pr-3 py-1">{{ t("diag.virtual") }}</th>
                  <th class="pr-3 py-1 text-right">Score</th>
                  <th class="py-1">{{ t("diag.selected") }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="c in candidates"
                  :key="c.ip"
                  class="border-b border-[var(--gosslan-border)]"
                  :class="c.selected ? 'bg-primary/10 font-medium' : ''"
                >
                  <td class="pr-3 py-1 font-mono text-[11px]">{{ c.name }}</td>
                  <td class="pr-3 py-1 font-mono">{{ c.ip }}</td>
                  <td class="pr-3 py-1 font-mono text-[11px]">{{ c.broadcast || "-" }}</td>
                  <td class="pr-3 py-1">{{ c.is_rfc1918 ? "Y" : "" }}</td>
                  <td class="pr-3 py-1">{{ c.is_virtual ? "Y" : "" }}</td>
                  <td class="pr-3 py-1 text-right font-mono">{{ c.score }}</td>
                  <td class="py-1">{{ c.selected ? "\u2713" : "" }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        <!-- 事件日志 -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.events", { n: diag.recent_events.length }) }}</h4>
          <div v-if="diag.recent_events.length === 0" class="opacity-50">{{ t("diag.noEvents") }}</div>
          <div class="max-h-48 overflow-y-auto space-y-0.5 font-mono text-[11px]">
            <div v-for="(ev, i) in diag.recent_events" :key="i" class="flex gap-2">
              <span class="shrink-0 opacity-50 w-20">{{ fmtTs(ev.ts) }}</span>
              <span class="shrink-0 w-28 truncate" :class="ev.kind.includes('error') ? 'text-[var(--gosslan-danger-ink)]' : ''" :title="ev.kind">{{ ev.kind }}</span>
              <span class="opacity-70 truncate" :title="ev.detail">{{ ev.detail }}</span>
            </div>
          </div>
        </section>
      </template>

      <div v-else-if="loading" class="opacity-50 text-center py-4">{{ t("diag.loading") }}</div>
    </div>
  </BaseModal>
</template>
