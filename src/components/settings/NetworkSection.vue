<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Trash2 } from "lucide-vue-next";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import SettingsRow from "@/components/settings/SettingsRow.vue";
import SettingsToggle from "@/components/settings/SettingsToggle.vue";
import { t } from "@/i18n";
import type {RelayPolicy, RoutedEndpoint} from "@/types";

const props = defineProps<{ active: boolean; reloadToken?: number }>();

const app = useAppStore();
const chat = useChatStore();

const selectedIp = ref("0.0.0.0");

// 通道状态来自 store（唯一真相源）—— 另一处（添加好友页）开关也会立刻反映到这里
const channels = computed(() => app.channels);
const btStatus = computed(() => channels.value.find((c) => c.channel === "bluetooth"));
const lanStatus = computed(() => channels.value.find((c) => c.channel === "lan"));
const interfaceOptions = computed(() => [
  { value: "0.0.0.0", label: "settings.network.interface.auto" },
  ...app.interfaces.map((i) => ({ value: i.ip, label: `${i.name}（${i.ip}）` })),
]);

/** 当前策略的一句话说明（放在行描述里，用户不用猜 off/friends 到底转发给谁）。 */
const policyDescription = computed(() => t(`settings.relay.policy.${app.relayPolicy}.desc`));

async function loadChannels() {
  await app.refreshChannels();
}

watch(
  () => [props.active, props.reloadToken],
  async () => {
    if (!props.active) return;
    selectedIp.value = app.boundIp ?? app.preferredIp ?? "0.0.0.0";
    await app.refreshInterfaces();
    await loadChannels();
    await loadEndpoints();
  },
  { immediate: true },
);

/** 局域网开关：与「网卡选择」联动，统一以 selectedIp 为绑定地址。 */
async function toggleLan() {
  if (app.online) {
    await app.stopNetwork();
    app.toast(t("settings.network.toast.lanOff"), "info");
  } else {
    try {
      await app.startNetwork(selectedIp.value);
      app.toast(t("settings.network.toast.lanOn"), "success");
      await chat.refreshPeers();
    } catch (e) {
      app.toastError(e, t("settings.network.toast.lanFail"));
    }
  }
  await loadChannels();
}

/** 选择网卡：未开启 → 直接以该网卡开启并扫描；已开启 → 切换到新网卡重新扫描。 */
async function onInterfaceChange() {
  const ip = selectedIp.value;
  try {
    if (app.online) {
      if (app.boundIp === ip) return;
      await app.stopNetwork();
      await app.startNetwork(ip);
      app.toast(t("settings.network.toast.interfaceSwitched", { ip }), "success");
    } else {
      await app.startNetwork(ip);
      app.toast(t("settings.network.toast.lanOn"), "success");
    }
    await chat.refreshPeers();
  } catch (e) {
    app.toastError(e, t("settings.network.toast.interfaceFail"));
  }
  await loadChannels();
}

async function toggleBluetooth() {
  const cur = btStatus.value?.enabled ?? false;
  try {
    await app.setChannelEnabled("bluetooth", !cur);
    app.toast(cur ? t("settings.network.toast.btOff") : t("settings.network.toast.btOn"), cur ? "info" : "success");
  } catch (e) {
    app.toastError(e, t("settings.network.toast.btFail"));
  }
  await loadChannels();
}

// ---- 跨网段（Routed）端点配置 ----
const endpoints = ref<RoutedEndpoint[]>([]);
const newAddress = ref("");

async function loadEndpoints() {
  endpoints.value = await api.listRoutedEndpoints();
}

async function addEndpoint() {
  const addr = newAddress.value.trim();
  if (!addr) return;
  try {
    endpoints.value = await api.addRoutedEndpoint(addr);
    newAddress.value = "";
    app.toast(t("settings.network.routed.toast.added"), "success");
  } catch (e) {
    app.toastError(e, t("settings.network.routed.toast.addFailed"));
  }
}

async function removeEndpoint(address: string) {
  try {
    endpoints.value = await api.removeRoutedEndpoint(address);
    app.toast(t("settings.network.routed.toast.removed"), "info");
  } catch (e) {
    app.toastError(e, t("settings.network.routed.toast.removeFailed"));
  }
}
</script>

<template>
  <SettingsGroup
    :title="t('settings.group.network')"
    :footer="t('settings.group.network.footer')"
  >
    <SettingsRow :label="t('settings.network.lan')" :description="t('settings.network.lan.desc')">
      <div class="flex items-center gap-2">
        <span class="text-xs" :class="app.online ? 'text-[var(--gosslan-success-ink)]' : 'text-[var(--gosslan-text-2)]'">
          {{ app.online ? t("settings.network.lan.peers", { n: lanStatus?.peers ?? 0 }) : t("settings.network.lan.off") }}
        </span>
        <SettingsToggle :label="t('settings.network.lan')" :model-value="app.online" @update:model-value="toggleLan" />
      </div>
    </SettingsRow>

    <SettingsRow :label="t('settings.network.interface')">
      <span class="gosslan-select-wrap max-w-[200px]">
        <select
          v-model="selectedIp"
          class="gosslan-select max-w-[200px]"
          @change="onInterfaceChange"
        >
          <option v-for="o in interfaceOptions" :key="o.value" :value="o.value">{{ t(o.label) }}</option>
        </select>
      </span>
    </SettingsRow>

    <SettingsRow
      :label="t('settings.network.bluetooth')"
      :description="btStatus?.available ? undefined : t('settings.network.bluetooth.unavailable')"
      last
    >
      <div class="flex items-center gap-2">
        <span class="text-xs text-[var(--gosslan-text-2)]">
          {{ btStatus?.available ? (btStatus.enabled ? t("settings.network.bluetooth.on") : t("settings.network.bluetooth.off")) : t("settings.network.bluetooth.na") }}
        </span>
        <SettingsToggle
          :label="t('settings.network.bluetooth')"
          :model-value="!!btStatus?.enabled"
          :disabled="!btStatus?.available"
          @update:model-value="toggleBluetooth"
        />
      </div>
    </SettingsRow>
  </SettingsGroup>

  <SettingsGroup
    :title="t('settings.network.routed')"
    :footer="t('settings.network.routed.desc')"
  >
    <!-- 已配置端点列表 -->
    <div
      v-for="ep in endpoints"
      :key="ep.address"
      class="flex items-center gap-2 px-4 py-2.5"
    >
      <span class="min-w-0 flex-1 truncate font-mono text-[13px] text-[var(--gosslan-text)]" :title="ep.address">
        {{ ep.address }}
      </span>
      <button
        class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-danger-soft)] hover:text-[var(--gosslan-danger-ink)]"
        :aria-label="t('settings.network.routed.remove')"
        :title="t('settings.network.routed.remove')"
        @click="removeEndpoint(ep.address)"
      >
        <Trash2 :size="14" />
      </button>
    </div>
    <div v-if="endpoints.length" class="ml-4 h-px bg-[var(--gosslan-divider)]" />

    <!-- 空状态 -->
    <p v-if="!endpoints.length" class="px-4 py-3 text-xs text-[var(--gosslan-text-2)]">
      {{ t("settings.network.routed.empty") }}
    </p>

    <!-- 添加 -->
    <div class="flex items-center gap-2 px-4 py-3">
      <input
        v-model="newAddress"
        type="text"
        :placeholder="t('settings.network.routed.placeholder')"
        class="min-w-0 flex-1 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-transparent px-3 py-1.5 text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)] focus:border-[var(--gosslan-primary)]"
        @keyup.enter="addEndpoint"
      />
      <button
        class="shrink-0 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-3.5 py-1.5 text-[13px] font-medium text-white transition hover:bg-[var(--gosslan-primary-hover)]"
        @click="addEndpoint"
      >
        {{ t("settings.network.routed.add") }}
      </button>
    </div>
  </SettingsGroup>

    <!-- 中继授权（P2 / M4）——「我愿不愿意替别人转发消息」是本机策略，不读远端自报 -->
    <SettingsGroup :title="t('settings.relay.title')" :footer="t('settings.relay.footer')">
      <SettingsRow :label="t('settings.relay.policy')" :description="policyDescription">
        <span class="gosslan-select-wrap">
          <select
            :aria-label="t('settings.relay.policy')"
            class="gosslan-select"
            :value="app.relayPolicy"
            @change="app.setRelayPolicy(($event.target as HTMLSelectElement).value as RelayPolicy)"
          >
            <option value="all">{{ t("settings.relay.policy.all") }}</option>
            <option value="friends">{{ t("settings.relay.policy.friends") }}</option>
            <option value="allowlist">{{ t("settings.relay.policy.allowlist") }}</option>
            <option value="off">{{ t("settings.relay.policy.off") }}</option>
          </select>
        </span>
      </SettingsRow>

      <!-- 白名单：只在 allowlist 模式下出现，平时不占地方 -->
      <template v-if="app.relayPolicy === 'allowlist'">
        <div v-if="!chat.friends.length" class="px-4 py-3 text-xs text-[var(--gosslan-text-2)]">
          {{ t("settings.relay.allowlist.empty") }}
        </div>
        <div v-for="f in chat.friends" :key="f.device_id" class="flex items-center gap-3 px-4 py-2.5">
          <span class="min-w-0 flex-1 truncate text-sm text-[var(--gosslan-text)]" :title="f.nickname">
            {{ f.nickname }}
          </span>
          <SettingsToggle
            size="sm"
            :model-value="app.relayAllowlist.includes(f.device_id)"
            :label="t('settings.relay.allowlist.toggle', { name: f.nickname })"
            @update:model-value="app.toggleRelayAllowlist(f.device_id)"
          />
        </div>
      </template>
    </SettingsGroup>
</template>
