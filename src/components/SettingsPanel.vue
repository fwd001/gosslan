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
  Lock,
  Monitor,
  Moon,
  Network,
  RotateCcw,
  Sun,
  Trash2,
} from "lucide-vue-next";
import type { CacheInfo, ChannelStatus } from "@/types";
import { CHAT_FONT_SIZES, CHAT_PRESETS } from "@/utils/chatStyle";
import { version } from "../../package.json";

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
// 旧版本可能存了预设之外的数值，补一个回显选项避免下拉框空白
const quotaOptions = computed(() => {
  const existing = quotas.map((q) => q.value);
  if (maxQuotaMb.value > 0 && !existing.includes(maxQuotaMb.value)) {
    return [...quotas, { value: maxQuotaMb.value, label: `${maxQuotaMb.value} MB` }];
  }
  return quotas;
});

const interfaceOptions = computed(() => [
  { value: "0.0.0.0", label: "自动（所有网卡）" },
  ...app.interfaces.map((i) => ({ value: i.ip, label: `${i.name}（${i.ip}）` })),
]);

const fullId = computed(() => app.device?.device_id ?? "");

const btStatus = computed(() => channels.value.find((c) => c.channel === "bluetooth"));
const lanStatus = computed(() => channels.value.find((c) => c.channel === "lan"));

watch(
  () => props.open,
  async (v) => {
    if (v) {
      nickname.value = app.device?.nickname ?? "";
      avatar.value = app.device?.avatar ?? null;
      selectedIp.value = app.boundIp ?? app.preferredIp ?? "0.0.0.0";
      await app.refreshInterfaces();
      await Promise.all([loadChannels(), loadCache()]);
    }
  },
);

// ---- 即点即保存：所有设置改动立即持久化，无「保存」按钮 ----

/** 昵称：失焦或回车即保存。 */
async function saveProfileNow() {
  const name = nickname.value.trim();
  if (!name) {
    app.toast("昵称不能为空", "error");
    nickname.value = app.device?.nickname ?? "";
    return;
  }
  if (name === app.device?.nickname && avatar.value === app.device?.avatar) return;
  await app.updateProfile(name, avatar.value);
  await chat.refreshFriends();
  app.toast("资料已保存并同步", "success");
}

function onNicknameKeydown(e: KeyboardEvent) {
  if (e.key === "Enter") {
    e.preventDefault();
    (e.target as HTMLInputElement).blur();
  }
}

async function loadChannels() {
  channels.value = await api.getChannelStatus();
}
async function loadCache() {
  cacheInfo.value = await api.getCacheInfo();
  suppressAutoSave = true;
  retentionDays.value = cacheInfo.value?.retention_days ?? 0;
  maxQuotaMb.value = Math.round((cacheInfo.value?.max_bytes ?? 0) / 1048576);
  // 等 watch 同步跳过这一轮由「回显赋值」触发的回调
  setTimeout(() => (suppressAutoSave = false), 0);
}

function formatBytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}

function onAvatarChange(e: Event) {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  if (!f) return;
  const r = new FileReader();
  r.onload = async () => {
    avatar.value = r.result as string;
    await saveProfileNow();
  };
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

// 缓存策略：改动即保存（回显赋值时跳过）
let suppressAutoSave = false;
watch([retentionDays, maxQuotaMb], () => {
  if (suppressAutoSave) return;
  void applyCachePolicy(true);
});

async function applyCachePolicy(silent = false) {
  await api.setCachePolicy(
    retentionDays.value === 0 ? null : retentionDays.value,
    maxQuotaMb.value === 0 ? null : maxQuotaMb.value * 1048576,
  );
  if (!silent) app.toast("缓存策略已保存", "success");
  await loadCache();
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
  app.toast("已恢复默认设置", "success");
  await loadChannels();
}
</script>

<template>
  <BaseModal :open="open" title="设置" width="max-w-xl" @close="emit('close')">
    <div class="max-h-[75vh] space-y-6 overflow-y-auto py-1 pr-3">
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
              placeholder="昵称（修改后自动保存）"
              @blur="saveProfileNow"
              @keydown="onNicknameKeydown"
            />
            <button
              class="rounded-lg border border-[var(--gosslan-border)] px-3 py-1.5 text-xs transition hover:bg-[var(--gosslan-hover)]"
              @click="avatarInput?.click()"
            >
              更换头像
            </button>
            <input ref="avatarInput" type="file" accept="image/*" class="hidden" @change="onAvatarChange" />
            <!-- 本人在线状态（从原右上角拓扑栏移入，明确标识是「我」的状态） -->
            <div class="mt-2.5 flex items-center gap-1.5 text-xs">
              <span
                class="h-2 w-2 rounded-full"
                :class="app.online ? 'bg-emerald-500' : 'bg-neutral-400'"
              ></span>
              <span :class="app.online ? 'text-emerald-600' : 'text-[var(--gosslan-text-2)]'">
                {{ app.online ? "我在线 · 局域网已连接" : "我离线 · 局域网未连接" }}
              </span>
            </div>
          </div>
        </div>
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

      <!-- 聊天显示（即点即存 + 广播同步） -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">聊天显示</h3>
        <div class="space-y-3">
          <!-- 字体大小 -->
          <div>
            <div class="mb-1.5 text-sm">字体大小</div>
            <div class="flex gap-1 rounded-lg border border-[var(--gosslan-border)] p-0.5">
              <button
                v-for="f in CHAT_FONT_SIZES"
                :key="f.key"
                class="flex-1 rounded-md py-1.5 text-sm transition"
                :class="app.chatStyle.fontSize === f.key ? 'bg-primary text-white' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
                @click="app.setChatStyle({ fontSize: f.key })"
              >
                {{ f.label }}
              </button>
            </div>
          </div>

          <!-- 气泡配色（6 套可读性预设，双气泡预览） -->
          <div>
            <div class="mb-1.5 text-sm">气泡配色</div>
            <div class="grid grid-cols-3 gap-2">
              <button
                v-for="p in CHAT_PRESETS"
                :key="p.key"
                class="rounded-lg border p-2 transition hover:bg-[var(--gosslan-hover)]"
                :class="app.chatStyle.preset === p.key ? 'border-primary ring-1 ring-primary' : 'border-[var(--gosslan-border)]'"
                :title="p.label"
                @click="app.setChatStyle({ preset: p.key })"
              >
                <div class="mb-1 text-center text-[11px] text-[var(--gosslan-text-2)]">{{ p.label }}</div>
                <div class="flex items-center gap-1">
                  <span class="h-4 flex-1 rounded" :style="{ background: p[app.dark ? 'dark' : 'light'].mineBubble }"></span>
                  <span class="h-4 flex-1 rounded border border-[var(--gosslan-border)]" :style="{ background: p[app.dark ? 'dark' : 'light'].otherBubble }"></span>
                </div>
              </button>
            </div>
            <p class="mt-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
              我的消息用所选配色；对方也会按我的配色看到我发的消息（自动同步到已连接设备）。
            </p>
          </div>

          <!-- 紧凑模式 -->
          <div class="flex items-center justify-between rounded-lg border border-[var(--gosslan-border)] px-3 py-2">
            <div class="min-w-0">
              <div class="text-sm">消息合并显示</div>
              <div class="text-[11px] text-[var(--gosslan-text-2)]">连续消息省略头像与昵称（群聊推荐）</div>
            </div>
            <button
              class="relative h-5 w-9 shrink-0 rounded-full transition"
              :class="app.chatStyle.compact ? 'bg-primary' : 'bg-[var(--gosslan-border)]'"
              role="switch"
              :aria-checked="app.chatStyle.compact"
              @click="app.setChatStyle({ compact: !app.chatStyle.compact })"
            >
              <span
                class="absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all"
                :class="app.chatStyle.compact ? 'left-[18px]' : 'left-0.5'"
              ></span>
            </button>
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
            <option v-for="q in quotaOptions" :key="q.value" :value="q.value">{{ q.label }}</option>
          </select>
        </div>
        <p class="mb-3 mt-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
          磁盘上限为「无限制」时缓存不会被自动清理；设置上限后，缓存超过该值会自动删除最旧的图片 / 文件。改动即时生效并自动保存。
        </p>
        <button
          class="flex w-full items-center justify-center gap-1.5 rounded-xl border border-[var(--gosslan-border)] py-2 text-sm transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
          :disabled="cleaning"
          @click="cleanNow"
        >
          <Trash2 class="h-4 w-4" />
          立即清理
        </button>
      </section>

      <!-- 安全 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">安全</h3>
        <div class="flex items-center gap-2">
          <Lock class="h-4 w-4 text-emerald-600" />
          <span class="text-sm">端到端加密（E2EE）</span>
          <span class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] text-emerald-600">已启用</span>
        </div>
        <div class="mt-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
          所有单聊与群聊消息在发送前经 X25519 密钥交换 + ChaCha20-Poly1305 加密，
          中继节点只透传密文、无法查看内容；聊天窗口顶部的锁形标识实时显示该状态。
          发送前需获取对方公钥（对方上线后自动同步），因此向从未上线的好友发送会提示稍后重试。
        </div>
      </section>

      <!-- 关于 -->
      <section>
        <h3 class="mb-3 text-[13px] font-semibold text-[var(--gosslan-text)]">关于</h3>
        <div class="text-xs leading-relaxed text-[var(--gosslan-text-2)]">
          设备指纹：<span class="break-all font-mono select-text">{{ fullId }}</span>
        </div>
        <div class="mt-1 text-xs text-[var(--gosslan-text-2)]">
          Gosslan v{{ version }} · 无服务器 P2P · 端到端加密 · 数据仅存本机
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
