<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import BaseModal from "@/components/BaseModal.vue";
import {
  Bluetooth,
  FolderOpen,
  HardDrive,
  Monitor,
  Moon,
  Network,
  RotateCcw,
  Sun,
  Trash2,
} from "lucide-vue-next";
import type { CacheInfo, ChannelStatus } from "@/types";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();

const nickname = ref(app.device?.nickname ?? "");
const avatar = ref<string | null>(app.device?.avatar ?? null);
const selectedIp = ref<string>("0.0.0.0");
const avatarInput = ref<HTMLInputElement | null>(null);

// 通道与缓存状态
const channels = ref<ChannelStatus[]>([]);
const cacheInfo = ref<CacheInfo | null>(null);
const retentionDays = ref<number>(0); // 0 = 永久
const maxQuotaMb = ref<number>(0); // 0 = 无限制
const cleaning = ref(false);
const savingPolicy = ref(false);

const presets = ["#3370ff", "#00b578", "#ff6b35", "#8b5cf6", "#e53e3e", "#0ea5e9"];
const fonts = [
  { value: "", label: "系统默认" },
  { value: "-apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif", label: "苹方 / 微软雅黑" },
  { value: "'Noto Sans SC', 'Source Han Sans SC', sans-serif", label: "思源黑体" },
  { value: "'JetBrains Mono', Consolas, monospace", label: "等宽字体" },
];
const quotas = [
  { value: 0, label: "无限制" },
  { value: 256, label: "256 MB" },
  { value: 512, label: "512 MB" },
  { value: 1024, label: "1 GB" },
  { value: 2048, label: "2 GB" },
];

const interfaceOptions = computed(() => [
  { value: "0.0.0.0", label: "自动（所有网卡）" },
  ...app.interfaces.map((i) => ({ value: i.ip, label: `${i.name}（${i.ip}）` })),
]);

const shortId = computed(() => {
  const id = app.device?.device_id ?? "";
  return id.length > 18 ? `${id.slice(0, 12)}…` : id;
});

const btStatus = computed(() => channels.value.find((c) => c.channel === "bluetooth"));
const lanStatus = computed(() => channels.value.find((c) => c.channel === "lan"));

watch(
  () => props.open,
  async (v) => {
    if (v) {
      nickname.value = app.device?.nickname ?? "";
      avatar.value = app.device?.avatar ?? null;
      selectedIp.value = app.boundIp ?? "0.0.0.0";
      await app.refreshInterfaces();
      await Promise.all([loadChannels(), loadCache()]);
    }
  },
);

async function loadChannels() {
  channels.value = await api.getChannelStatus();
}
async function loadCache() {
  cacheInfo.value = await api.getCacheInfo();
  retentionDays.value = cacheInfo.value?.retention_days ?? 0;
  maxQuotaMb.value = Math.round((cacheInfo.value?.max_bytes ?? 0) / 1048576);
}

function formatBytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}

async function saveProfile() {
  if (!nickname.value.trim()) {
    app.toast("昵称不能为空", "error");
    return;
  }
  await app.updateProfile(nickname.value.trim(), avatar.value);
  await chat.refreshFriends();
  app.toast("资料已保存并同步", "success");
}

function onAvatarChange(e: Event) {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  if (!f) return;
  const r = new FileReader();
  r.onload = () => (avatar.value = r.result as string);
  r.readAsDataURL(f);
}

/** 局域网开关：与「网卡选择」联动，统一以 selectedIp 为绑定地址。 */
async function toggleLan() {
  if (app.online) {
    await app.stopNetwork();
    app.toast("局域网通道已关闭", "info");
  } else {
    try {
      await app.startNetwork(selectedIp.value);
      app.toast("局域网通道已开启，正在扫描节点…", "success");
      await chat.refreshPeers();
    } catch (e) {
      app.toast(String(e), "error");
    }
  }
  await loadChannels();
}

/** 选择网卡：局域网未开启 → 直接以该网卡开启并扫描；已开启 → 切换到新网卡重新扫描。 */
async function onInterfaceChange() {
  const ip = selectedIp.value;
  try {
    if (app.online) {
      if (app.boundIp === ip) return;
      await app.stopNetwork();
      await app.startNetwork(ip);
      app.toast(`已切换到网卡 ${ip}，正在重新扫描…`, "success");
    } else {
      await app.startNetwork(ip);
      app.toast("局域网通道已开启，正在扫描节点…", "success");
    }
    await chat.refreshPeers();
  } catch (e) {
    app.toast(String(e), "error");
  }
  await loadChannels();
}

async function toggleBluetooth() {
  const cur = btStatus.value?.enabled ?? false;
  try {
    await api.setChannelEnabled("bluetooth", !cur);
    if (!cur) app.toast("蓝牙通道已开启", "success");
    else app.toast("蓝牙通道已关闭", "info");
  } catch (e) {
    app.toast(String(e), "error");
  }
  await loadChannels();
}

async function applyCachePolicy(silent = false) {
  savingPolicy.value = true;
  try {
    await api.setCachePolicy(
      retentionDays.value === 0 ? null : retentionDays.value,
      maxQuotaMb.value === 0 ? null : maxQuotaMb.value * 1048576,
    );
    if (!silent) app.toast("缓存策略已保存", "success");
    await loadCache();
  } finally {
    savingPolicy.value = false;
  }
}

async function cleanNow() {
  cleaning.value = true;
  try {
    const r = await api.cleanCacheNow();
    app.toast(`清理完成：删除 ${r.removed} 个文件，释放 ${formatBytes(r.freed_bytes)}`, "success");
  } catch (e) {
    app.toast(String(e), "error");
  } finally {
    cleaning.value = false;
    await loadCache();
  }
}

async function pickShareDir() {
  const picked = await openDialog({ directory: true });
  if (typeof picked === "string") {
    try {
      await app.setShareDir(picked);
      app.toast("共享目录已设置", "success");
    } catch (e) {
      app.toast(String(e), "error");
    }
  }
}

/** 恢复默认：外观 / 网卡 / 缓存策略全部回到默认值（不动好友与聊天数据）。 */
async function restoreDefaults() {
  await app.resetDefaults();
  selectedIp.value = "0.0.0.0";
  retentionDays.value = 0;
  maxQuotaMb.value = 0;
  await applyCachePolicy(true);
  app.toast("已恢复默认设置", "success");
  await loadChannels();
}
</script>

<template>
  <BaseModal :open="open" title="设置" width="max-w-xl" @close="emit('close')">
    <div class="max-h-[75vh] space-y-6 overflow-y-auto pr-1">
      <!-- 个人资料 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">个人资料</h3>
        <div class="flex items-center gap-4">
          <div class="flex h-16 w-16 shrink-0 items-center justify-center overflow-hidden rounded-full bg-primary text-white">
            <img v-if="avatar" :src="avatar" class="h-full w-full object-cover" />
            <span v-else class="text-2xl font-semibold">{{ nickname.slice(0, 1) || "我" }}</span>
          </div>
          <div class="min-w-0 flex-1">
            <input
              v-model="nickname"
              class="mb-2 w-full rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
              placeholder="昵称"
            />
            <button
              class="rounded-lg border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)]"
              @click="avatarInput?.click()"
            >
              更换头像
            </button>
            <input ref="avatarInput" type="file" accept="image/*" class="hidden" @change="onAvatarChange" />
          </div>
        </div>
        <button
          class="mt-3 w-full rounded-xl bg-primary py-2 text-sm font-medium text-white transition hover:bg-primary-hover"
          @click="saveProfile"
        >
          保存资料
        </button>
      </section>

      <!-- 外观 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">外观</h3>
        <div class="space-y-3">
          <div class="flex items-center justify-between">
            <span class="text-sm">深色模式</span>
            <button
              class="relative h-6 w-11 rounded-full transition"
              :class="app.dark ? 'bg-primary' : 'bg-[var(--gosslan-border)]'"
              role="switch"
              :aria-checked="app.dark"
              @click="app.toggleDark()"
            >
              <span
                class="absolute top-0.5 flex h-5 w-5 items-center justify-center rounded-full bg-white shadow transition-all"
                :class="app.dark ? 'left-[22px]' : 'left-0.5'"
              >
                <Sun v-if="app.dark" class="h-3 w-3 text-amber-500" />
                <Moon v-else class="h-3 w-3 text-[var(--gosslan-text-2)]" />
              </span>
            </button>
          </div>
          <div>
            <div class="mb-1.5 text-sm">主题色</div>
            <div class="flex items-center gap-2">
              <button
                v-for="c in presets"
                :key="c"
                class="h-6 w-6 rounded-full transition"
                :style="{ background: c, outline: app.themeColor === c ? '2px solid var(--gosslan-text)' : 'none' }"
                @click="app.setThemeColor(c)"
              ></button>
              <input
                type="color"
                :value="app.themeColor"
                class="h-6 w-8 cursor-pointer rounded border-0 bg-transparent p-0"
                title="自定义颜色"
                @input="(e) => app.setThemeColor((e.target as HTMLInputElement).value)"
              />
            </div>
          </div>
          <div>
            <div class="mb-1.5 text-sm">字体</div>
            <select
              class="w-full rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
              :value="app.fontFamily"
              @change="(e) => app.setFontFamily((e.target as HTMLSelectElement).value)"
            >
              <option v-for="f in fonts" :key="f.value" :value="f.value">{{ f.label }}</option>
            </select>
          </div>
        </div>
      </section>

      <!-- 网络通道 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">网络通道</h3>

        <!-- 局域网 -->
        <div class="rounded-xl border border-[var(--gosslan-border)] p-3">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <Network class="h-4 w-4 text-primary" />
              <span class="text-sm">局域网通道</span>
            </div>
            <div class="flex items-center gap-2">
              <span
                class="text-xs"
                :class="app.online ? 'text-emerald-500' : 'text-[var(--gosslan-text-2)]'"
              >
                {{ app.online ? `${lanStatus?.peers ?? 0} 个节点在线` : "未开启" }}
              </span>
              <button
                class="relative h-5 w-9 rounded-full transition"
                :class="app.online ? 'bg-primary' : 'bg-[var(--gosslan-border)]'"
                role="switch"
                :aria-checked="app.online"
                @click="toggleLan"
              >
                <span
                  class="absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all"
                  :class="app.online ? 'left-[18px]' : 'left-0.5'"
                ></span>
              </button>
            </div>
          </div>
          <div class="mt-3 flex items-center gap-2">
            <Monitor class="h-4 w-4 shrink-0 text-[var(--gosslan-text-2)]" />
            <select
              v-model="selectedIp"
              class="w-full rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
              @change="onInterfaceChange"
            >
              <option v-for="o in interfaceOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
            </select>
          </div>
          <p class="mt-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
            选择网卡后即开启通道并扫描节点；通道开启时可随时切换网卡。
          </p>
        </div>

        <!-- 蓝牙 -->
        <div
          class="mt-2 flex items-center justify-between rounded-xl border border-[var(--gosslan-border)] p-3 opacity-60"
          :class="{ 'pointer-events-none': !btStatus?.available }"
        >
          <div class="flex items-center gap-2">
            <Bluetooth class="h-4 w-4 text-[var(--gosslan-text-2)]" />
            <span class="text-sm">蓝牙通道</span>
          </div>
          <div class="flex items-center gap-2">
            <span class="text-xs text-[var(--gosslan-text-2)]">
              {{ btStatus?.available ? (btStatus.enabled ? "已开启" : "已关闭") : "暂不可用" }}
            </span>
            <button
              class="relative h-5 w-9 rounded-full transition"
              :class="btStatus?.enabled ? 'bg-primary' : 'bg-[var(--gosslan-border)]'"
              :disabled="!btStatus?.available"
              role="switch"
              :aria-checked="!!btStatus?.enabled"
              @click="toggleBluetooth"
            >
              <span
                class="absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all"
                :class="btStatus?.enabled ? 'left-[18px]' : 'left-0.5'"
              ></span>
            </button>
          </div>
        </div>
        <p v-if="!btStatus?.available" class="mt-1 text-[11px] text-[var(--gosslan-text-2)]">
          蓝牙后端尚未编译（当前版本暂不支持），将在后续版本提供。
        </p>
      </section>

      <!-- 共享目录 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">共享目录</h3>
        <div class="flex items-center gap-2">
          <button
            class="flex items-center gap-1.5 rounded-lg border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)]"
            @click="pickShareDir"
          >
            <FolderOpen class="h-4 w-4" />
            选择文件夹
          </button>
          <span class="truncate text-xs text-[var(--gosslan-text-2)]">{{ app.shareDir || "未设置" }}</span>
        </div>
      </section>

      <!-- 存储与缓存 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">存储与缓存</h3>
        <div class="mb-3 flex items-center gap-2 text-xs text-[var(--gosslan-text-2)]">
          <HardDrive class="h-3.5 w-3.5" />
          当前缓存 {{ cacheInfo?.file_count ?? 0 }} 个文件 · 占用
          {{ formatBytes(cacheInfo?.total_bytes ?? 0) }}
        </div>
        <div class="mb-3 flex items-center gap-3">
          <span class="w-16 shrink-0 text-sm">保留时长</span>
          <select
            v-model="retentionDays"
            class="flex-1 rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
          >
            <option :value="0">永久保存</option>
            <option :value="3">3 天</option>
            <option :value="7">7 天</option>
            <option :value="30">30 天</option>
          </select>
        </div>
        <div class="flex items-center gap-3">
          <span class="w-16 shrink-0 text-sm">磁盘上限</span>
          <select
            v-model.number="maxQuotaMb"
            class="flex-1 rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
          >
            <option v-for="q in quotas" :key="q.value" :value="q.value">{{ q.label }}</option>
          </select>
        </div>
        <p class="mb-3 mt-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
          磁盘上限为「无限制」时缓存不会被自动清理；设置上限后，缓存超过该值会自动删除最旧的图片 / 文件。
        </p>
        <div class="flex items-center gap-2">
          <button
            class="flex-1 rounded-xl bg-primary py-2 text-sm font-medium text-white transition hover:bg-primary-hover disabled:opacity-50"
            :disabled="savingPolicy"
            @click="applyCachePolicy()"
          >
            保存策略
          </button>
          <button
            class="flex items-center gap-1.5 rounded-xl border border-[var(--gosslan-border)] px-3 py-2 text-sm transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
            :disabled="cleaning"
            @click="cleanNow"
          >
            <Trash2 class="h-4 w-4" />
            立即清理
          </button>
        </div>
      </section>

      <!-- 关于 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">关于</h3>
        <div class="text-xs text-[var(--gosslan-text-2)]">设备指纹：{{ shortId }}</div>
        <div class="mt-1 text-xs text-[var(--gosslan-text-2)]">
          Gosslan v0.3.1 · 无服务器 P2P · 端到端加密 · 数据仅存本机
        </div>
      </section>

      <!-- 恢复默认 -->
      <button
        class="flex w-full items-center justify-center gap-2 rounded-xl border border-[var(--gosslan-border)] py-2 text-sm text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        @click="restoreDefaults"
      >
        <RotateCcw class="h-4 w-4" />
        恢复默认设置
      </button>
      <p class="text-center text-[11px] text-[var(--gosslan-text-2)]">
        将外观、网卡与缓存策略恢复为默认值，不影响好友与聊天记录。
      </p>
    </div>
  </BaseModal>
</template>
