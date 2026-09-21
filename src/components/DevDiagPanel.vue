<script setup lang="ts">
/**
 * 网络诊断（开发者）。
 *
 * ## 为什么重做（用户 2026-09-13）
 *
 * 旧面板的数据**全是局域网的**：纯蓝牙用户（或"局域网 + 蓝牙同时开"）看到的是
 * `模式：offline`，而蓝牙其实连得好好的；「网卡-候选」表也只有网卡，看不到蓝牙这条链路。
 * 重做后：
 *   · 一条通道一张卡（局域网 / 蓝牙各自说自己的状态），谁在跑一眼看得出，不再"整机 offline"；
 *   · 「候选链路」把网卡与蓝牙放进同一张表，按 `kind` 各自渲染（蓝牙没有 IP/广播概念）；
 *   · 「最近事件」整块移除 —— 诊断事件已合并进**运行日志**（可搜索/可复制/可落盘，
 *     见 Rust `AppState::push_diag_event`），单独一个面板反而两头都查不到。
 *
 * 布局用卡片/列表而不是宽表：桌面与手机（三端）都能读，窄屏不会横向滚动。
 */
import { t } from "@/i18n";
import { computed, onMounted, ref } from "vue";
import { Bluetooth, RefreshCw, Wifi } from "lucide-vue-next";
import BaseModal from "@/components/BaseModal.vue";
import { api } from "@/api";
import type { DiscoveryDiag, PeerVersionDiag } from "@/types";

defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const diag = ref<DiscoveryDiag | null>(null);
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    // 一次命令拿全（通道 + 候选）：避免两个命令各拿一份、数据对不上
    diag.value = await api.getDiscoveryDiag();
  } catch {
    /* 后端未就绪时保持上一次快照，用户可手动重试 */
  } finally {
    loading.value = false;
  }
}

onMounted(refresh);

/** 局域网是否在跑（只有它决定"局域网发现"那一块显不显示）。 */
const lanOn = computed(() => !!diag.value && diag.value.mode !== "offline");
const bt = computed(() => diag.value?.bluetooth ?? null);

/** 蓝牙总状态文案的 i18n key：未编译 / 不可用 / 未开启 / 已开启。 */
const btStateKey = computed(() => {
  const b = bt.value;
  if (!b || !b.feature_compiled) return "diag.notCompiled";
  if (b.running) return "diag.on";
  return b.available ? "diag.off" : "diag.na";
});

/** 蓝牙扫描节奏文案（前台/后台）。 */
const btCadenceKey = computed(() =>
  bt.value?.activity === "active" ? "diag.btCadenceActive" : "diag.btCadenceIdle",
);

function fmtTs(ts: number): string {
  if (!ts) return t("diag.btScanNever");
  return new Date(ts).toLocaleTimeString();
}

/** 候选链路的图标：蓝牙 / 网卡。 */
function iconFor(kind: string) {
  return kind === "bluetooth" ? Bluetooth : Wifi;
}

/** 版本行的主语：昵称优先，昵称空了退回 device_id（不显示空串）。 */
function peerLabel(p: PeerVersionDiag): string {
  return p.nickname || p.device_id;
}

/** 对端声明的版本一句话。**没声明就说"未声明"**，不替老版本猜一个号。 */
function peerVersionText(p: PeerVersionDiag): string {
  if (p.protocol_version === null && !p.app_version) return t("diag.peerNotDeclared");
  return t("diag.peerVersionLine", {
    p: p.protocol_version ?? "?",
    a: p.app_version || "-",
  });
}

/** 对端线格式版本比本机高 ⇒ 这一帧我们大概率看不懂（INV-P24 要求这个事实可见）。 */
function peerIsNewer(p: PeerVersionDiag): boolean {
  return p.protocol_version !== null && p.protocol_version > (diag.value?.protocol_version ?? 0);
}
</script>

<template>
  <BaseModal :open="open" :title="t('diag.title')" width="max-w-3xl" @close="emit('close')">
    <div class="max-h-[75vh] space-y-5 overflow-y-auto text-xs leading-relaxed">
      <!-- 刷新 -->
      <div class="flex items-center justify-between">
        <button
          class="inline-flex items-center gap-1.5 rounded-[var(--gosslan-radius-sm)] px-2.5 py-1 transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
          :disabled="loading"
          @click="refresh"
        >
          <RefreshCw class="h-3.5 w-3.5" :class="loading ? 'animate-spin' : ''" />
          {{ loading ? t("diag.refreshing") : t("diag.refresh") }}
        </button>
      </div>

      <template v-if="diag">
        <!-- ① 通道状态：一条通道一张卡（蓝牙用户不再被判 offline） -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.channels") }}</h4>
          <div class="grid gap-2 sm:grid-cols-2">
            <!-- 局域网 -->
            <div class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] p-3">
              <div class="mb-2 flex items-center gap-1.5 text-[13px] font-medium">
                <Wifi class="h-3.5 w-3.5 opacity-70" />
                {{ t("diag.lan") }}
                <span
                  class="ml-auto rounded-full px-2 py-0.5 text-[10px]"
                  :class="lanOn
                    ? 'bg-[var(--gosslan-success-soft)] text-[var(--gosslan-success-ink)]'
                    : 'bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]'"
                >
                  {{ lanOn ? t("diag.on") : t("diag.off") }}
                </span>
              </div>
              <div class="space-y-0.5">
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.mode") }}</span>
                  <span>{{ diag.mode === "offline" ? "-" : diag.mode }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.boundIp") }}</span>
                  <span class="truncate font-mono" :title="diag.bound_ip">{{ diag.bound_ip || "-" }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.selectedInterface") }}</span>
                  <span class="truncate" :title="diag.selected_interface">{{ diag.selected_interface || t("diag.auto") }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.selectedIp") }}</span>
                  <span class="truncate font-mono" :title="diag.selected_ip || diag.bound_ip">{{ diag.selected_ip || diag.bound_ip || "-" }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.tcpListen") }}</span>
                  <span class="truncate font-mono" :title="diag.tcp_listen">{{ diag.tcp_listen || "-" }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.udpPort") }}</span>
                  <span class="font-mono">{{ diag.udp_port || "-" }}</span>
                </div>
              </div>
            </div>

            <!-- 蓝牙 -->
            <div class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] p-3">
              <div class="mb-2 flex items-center gap-1.5 text-[13px] font-medium">
                <Bluetooth class="h-3.5 w-3.5 opacity-70" />
                {{ t("diag.bt") }}
                <span
                  class="ml-auto max-w-[60%] truncate rounded-full px-2 py-0.5 text-[10px]"
                  :class="bt?.running
                    ? 'bg-[var(--gosslan-success-soft)] text-[var(--gosslan-success-ink)]'
                    : 'bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]'"
                  :title="t(btStateKey)"
                >
                  {{ t(btStateKey) }}
                </span>
              </div>
              <div v-if="bt" class="space-y-0.5">
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.btCadence") }}</span>
                  <span>{{ t(btCadenceKey, { n: Math.round(bt.scan_interval_ms / 1000) }) }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.btWindow") }}</span>
                  <span>{{ t("diag.seconds", { n: Math.round(bt.scan_window_ms / 1000) }) }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.btLastScan") }}</span>
                  <span>{{ fmtTs(bt.last_scan_ts) }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">&nbsp;</span>
                  <span>{{ t("diag.btScanResult", { total: bt.last_scan_total, matched: bt.last_scan_matched }) }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.btPeers") }}</span>
                  <span>{{ bt.peers }}</span>
                </div>
                <div class="flex justify-between gap-3">
                  <span class="opacity-60">{{ t("diag.btBackoff") }}</span>
                  <span>{{ bt.backoff.length }}</span>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- ② 候选链路：网卡 + 蓝牙同一张表，按 kind 各自渲染 -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.candidates") }}</h4>
          <div class="space-y-1">
            <div
              v-for="c in diag.candidates"
              :key="c.kind + c.name + c.ip"
              class="flex items-start gap-2 rounded-[var(--gosslan-radius-md)] border px-2.5 py-2"
              :class="c.selected
                ? 'border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)]'
                : 'border-[var(--gosslan-border)]'"
            >
              <component :is="iconFor(c.kind)" class="mt-0.5 h-3.5 w-3.5 shrink-0 opacity-70" />
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span class="font-medium">{{ c.name }}</span>
                  <span class="rounded-full bg-[var(--gosslan-hover)] px-1.5 py-0.5 text-[10px] text-[var(--gosslan-text-2)]">
                    {{ c.kind === "bluetooth" ? t("diag.kindBt") : t("diag.kindLan") }}
                  </span>
                  <span v-if="c.selected" class="text-[10px] font-medium text-[var(--gosslan-primary)]">
                    {{ t("diag.chosen") }}
                  </span>
                </div>
                <div class="mt-0.5 break-all font-mono text-[11px] text-[var(--gosslan-text-2)]">
                  <template v-if="c.kind === 'bluetooth'">{{ c.detail }}</template>
                  <template v-else>
                    {{ c.ip }}
                    · {{ t("diag.broadcast") }} {{ c.broadcast || "-" }}
                    · {{ t("diag.score", { n: c.score }) }}
                    <template v-if="c.is_rfc1918"> · {{ t("diag.privateNet") }}</template>
                    <template v-if="c.is_virtual"> · {{ t("diag.virtual") }}</template>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- ③ 局域网发现（局域网没开就不占地方，明确说清"不影响蓝牙"） -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.discovery") }}</h4>
          <div v-if="lanOn" class="space-y-0.5">
            <div class="flex justify-between gap-3">
              <span class="opacity-60">{{ t("diag.broadcastTarget") }}</span>
              <span class="truncate font-mono" :title="diag.broadcast_target">{{ diag.broadcast_target || "-" }}</span>
            </div>
            <div class="flex justify-between gap-3">
              <span class="opacity-60">{{ t("diag.multicastGroup") }}</span>
              <span class="truncate font-mono" :title="diag.multicast_group">{{ diag.multicast_group || "-" }}</span>
            </div>
            <div class="flex justify-between gap-3">
              <span class="opacity-60">multicast_if：</span>
              <span
                class="truncate font-mono"
                :title="diag.multicast_if_result"
                :class="diag.multicast_if_result.startsWith('ok') || diag.multicast_if_result === 'not_set'
                  ? ''
                  : 'text-[var(--gosslan-danger-ink)]'"
              >
                {{ diag.multicast_if_result || "-" }}
              </span>
            </div>
            <div class="flex justify-between gap-3">
              <span class="opacity-60">join_multicast：</span>
              <span
                class="truncate font-mono"
                :title="diag.multicast_join_result"
                :class="diag.multicast_join_result.startsWith('ok') ? '' : 'text-[var(--gosslan-danger-ink)]'"
              >
                {{ diag.multicast_join_result || "-" }}
              </span>
            </div>
          </div>
          <p v-else class="opacity-50">{{ t("diag.lanOff") }}</p>
        </section>

        <!-- ④ 蓝牙失败退避明细（"为什么这一轮没拨它"的唯一可查之处） -->
        <section v-if="bt?.feature_compiled">
          <h4 class="mb-1.5 font-semibold text-[13px]">
            {{ t("diag.btBackoffList", { n: bt.backoff.length }) }}
          </h4>
          <p v-if="bt.backoff.length === 0" class="opacity-50">{{ t("diag.btBackoffEmpty") }}</p>
          <div v-else class="max-h-48 space-y-0.5 overflow-y-auto font-mono text-[11px]">
            <div v-for="b in bt.backoff" :key="b.id" class="flex gap-3">
              <span class="min-w-0 flex-1 truncate opacity-70" :title="b.id">{{ b.id }}</span>
              <span class="shrink-0">{{ t("diag.btBackoffItem", { n: b.failures, s: Math.round(b.remaining_ms / 1000) }) }}</span>
            </div>
          </div>
          <div v-if="bt.no_dial > 0" class="mt-1 opacity-60">
            {{ t("diag.btNoDial") }}{{ bt.no_dial }}
          </div>
        </section>

        <!-- ⑤ 版本互通：跨版本问题时第一眼要看"谁老、谁根本没报版本" -->
        <section>
          <h4 class="mb-1.5 font-semibold text-[13px]">{{ t("diag.versions") }}</h4>
          <div class="opacity-70">
            {{ t("diag.localVersion", { p: diag.protocol_version, a: diag.app_version }) }}
          </div>
          <p v-if="diag.peer_versions.length === 0" class="mt-1 opacity-50">
            {{ t("diag.peerVersionsEmpty") }}
          </p>
          <div v-else class="mt-1 max-h-48 space-y-0.5 overflow-y-auto font-mono text-[11px]">
            <div v-for="p in diag.peer_versions" :key="p.device_id" class="flex gap-3">
              <span class="min-w-0 flex-1 truncate opacity-70" :title="p.device_id">
                {{ peerLabel(p) }}
              </span>
              <span
                class="shrink-0"
                :class="peerIsNewer(p) ? 'text-[var(--gosslan-danger-ink)]' : ''"
              >
                {{ peerVersionText(p) }}
                <template v-if="peerIsNewer(p)"> · {{ t("diag.peerNewer") }}</template>
              </span>
            </div>
          </div>
        </section>
      </template>

      <div v-else-if="loading" class="py-4 text-center opacity-50">{{ t("diag.loading") }}</div>
    </div>
  </BaseModal>
</template>
