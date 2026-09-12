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
// `?? []` 是**防御性**写法：万一 store 还没就绪（或热更新留下了旧实例），
// 这里也只是"列表为空"，而不会 `undefined.find` 抛错 —— 后者会让**整个设置页**
// 再也 patch 不动（用户实测的"点设置卡死/过一会儿弹出好几个设置"）。
const channels = computed(() => app.channels ?? []);
const btStatus = computed(() => channels.value.find((c) => c.channel === "bluetooth"));
const lanStatus = computed(() => channels.value.find((c) => c.channel === "lan"));
const interfaceOptions = computed(() => [
  { value: "0.0.0.0", label: "settings.network.interface.auto" },
  ...app.interfaces.map((i) => ({ value: i.ip, label: `${i.name}（${i.ip}）` })),
]);

/** 当前策略的一句话说明（放在行描述里，用户不用猜 off/friends 到底转发给谁）。 */
const policyDescription = computed(() => t(`settings.relay.policy.${app.relayPolicy}.desc`));

async function loadChannels() {
  try {
    await app.refreshRuntime?.();
  } catch {
    /* 通道状态取不到不影响本页其它设置渲染 */
  }
}

watch(
  () => [props.active, props.reloadToken],
  async () => {
    if (!props.active) return;
    // 手机端：打开网络设置时才按需拉起蓝牙通道（同上）
    void app.ensureBluetoothOn();
    selectedIp.value = app.boundIp ?? app.preferredIp ?? "0.0.0.0";
    await app.refreshInterfaces();
    await loadChannels();
    await loadEndpoints();
  },
  { immediate: true },
);

/**
 * 局域网开关。
 *
 * ⚠️ 与「添加好友」页**必须走同一条路径**（`app.setChannelEnabled`），并且**以通道状态为准**
 * （`lanStatus.enabled`，来自 `get_channel_status`），不再用 `app.online` 当开关值。
 * 真实缺陷（用户 2026-09-12 安卓实测）：「添加好友里把局域网打开，设置里还是关的」——
 * 因为添加好友页改的是 `channels[lan].enabled`（后端真实运行状态），
 * 而设置页显示的是 `app.online`（另一份快照，没人去刷新它）。
 * 现在 store 的 `setChannelEnabled` 会同时刷新两者，且两处 UI 都读同一份通道状态。
 *
 * 「网卡选择」仍需按 IP 启停（`onInterfaceChange`），因为要指定绑定地址；
 * 那条路径同样以 `loadChannels()` 收尾，所以两个开关不会各说各话。
 */
async function toggleLan() {
  const target = !(lanStatus.value?.enabled ?? app.online);
  try {
    await app.setChannelEnabled("lan", target);
    app.toast(
      target ? t("settings.network.toast.lanOn") : t("settings.network.toast.lanOff"),
      target ? "success" : "info",
    );
    if (target) await chat.refreshPeers();
  } catch (e) {
    app.toastError(e, t("settings.network.toast.lanFail"));
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
  const target = !cur;
  try {
    await app.setChannelEnabled("bluetooth", target);
    app.toast(cur ? t("settings.network.toast.btOff") : t("settings.network.toast.btOn"), cur ? "info" : "success");
  } catch (e) {
    if (!target) {
      app.toastError(e, t("settings.network.toast.btFail"));
      await loadChannels();
      return;
    }
    // 打开失败**最常见的**原因是 Android 缺「附近的设备」运行时权限：
    // 先申请（系统弹框）再自动重试一次，而不是让用户自己去设置里翻。
    let retried = false;
    try {
      await api.requestBlePermissions();
      await app.setChannelEnabled("bluetooth", true);
      app.toast(t("settings.network.toast.btOn"), "success");
      retried = true;
    } catch (e2) {
      app.toastError(e2, t("settings.network.toast.btFail"));
    }
    if (!retried) {
      // 用户拒绝过权限：给一条能照着做的提示（系统设置里的路径）
      app.toast(t("settings.network.toast.btPermissionHint"), "info");
    }
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
        <!-- 开关值取通道状态（唯一真相源），与「添加好友」页同一份 -->
        <SettingsToggle
          :label="t('settings.network.lan')"
          :model-value="!!lanStatus?.enabled"
          @update:model-value="toggleLan"
        />
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
      :description="app.isMobile
        ? t('settings.network.bluetooth.alwaysOn')
        : (btStatus?.available ? undefined : t('settings.network.bluetooth.unavailable'))"
      last
    >
      <div class="flex items-center gap-2">
        <span class="text-xs text-[var(--gosslan-text-2)]">
          {{ btStatus?.available ? (btStatus.enabled ? t("settings.network.bluetooth.on") : t("settings.network.bluetooth.off")) : t("settings.network.bluetooth.na") }}
        </span>
        <!-- 手机端**不给开关**（用户 2026-09-12 要求：像 BitChat 那样默认就开、不用设置）：
             只要应用在跑，它就是 mesh 的一个中继节点；被系统回收就自然停止。
             桌面端保留开关（有线/局域网是主路径，蓝牙是可选通道）。 -->
        <SettingsToggle
          v-if="!app.isMobile"
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
