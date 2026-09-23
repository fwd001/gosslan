<script setup lang="ts">
import { t } from "@/i18n";
import { ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import BaseModal from "@/components/BaseModal.vue";
import { Download, Folder, RefreshCw } from "lucide-vue-next";
import { humanSize } from "@/utils/color";
import { reportError } from "@/utils/errors";
import { StaleGuard } from "@/utils/staleGuard";
import type { ShareEntry } from "@/types";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const entries = ref<ShareEntry[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

const friendId = () => chat.activeConv ?? "";
const friendName = () => chat.activeConversation?.name ?? "";

function depth(p: string) {
  return p.split("/").length - 1;
}

/**
 * 树是为哪个好友拉来的 ⇒ `download` 用它，而不是"点击那一刻的 activeConv"。
 * 与下面的 `loadGuard` 一起构成同一条不变量的两面：显示的那棵树，只有它自己的主人能下载。
 */
const loadedFor = ref<string | null>(null);
/** 后发先至守卫（审计阶段 4 · 4.2）：见 `utils/staleGuard`。 */
const loadGuard = new StaleGuard();

async function load() {
  const tok = loadGuard.begin("tree");
  const forId = friendId();
  loading.value = true;
  error.value = null;
  entries.value = [];
  loadedFor.value = null;
  try {
    const tree = await api.requestShareTree(forId);
    // 期间切了会话、或有更晚发起的一次在飞 ⇒ 这份树已经不是当前会话的，整个丢弃
    if (!loadGuard.isCurrent("tree", tok)) return;
    entries.value = tree;
    loadedFor.value = forId;
  } catch (e) {
    // catch/finally 同样要过闸：旧请求的报错会把新会话的正常界面变成"加载失败"
    if (!loadGuard.isCurrent("tree", tok)) return;
    // 不把 Rust 的 Err(String) 原样丢给用户（项目其它路径都走 reportError 出可读文案）；
    // 同时给读屏一个 role="alert" 的提示（见模板）。
    error.value = reportError(e, t("share.loadFail"));
  } finally {
    if (loadGuard.isCurrent("tree", tok)) loading.value = false;
  }
}

async function download(e: ShareEntry) {
  // ⚠️ 用 `loadedFor` 而不是当场 `friendId()`：弹窗开着时 activeConv 会被**程序化路径**换掉
  // （点系统通知 → `openConversation`），那一刻点下载会把"A 的树里的一行"发给 B ——
  // 后端按 B 的共享目录校验路径，结果是莫名"下载失败"，或更糟：拿到 B 上同名的另一个文件。
  const owner = loadedFor.value;
  if (!owner) return;
  try {
    await api.downloadSharedFile(owner, e.path);
    app.toast(t("share.toast.downloading", { name: e.name }), "success");
    chat.refreshTransfers();
  } catch (err) {
    app.toastError(err, t("share.toast.downloadFail"));
  }
}

watch(
  () => props.open,
  (v) => {
    if (v) load();
  },
);
// 面板开着时换会话 ⇒ 重拉（与 GroupFilesPanel 的 groupId watch 同一条口径）。
// 只靠 loadGuard 的话，旧树会一直挂在当前好友名下 —— 守卫挡住了错数据，也挡住了刷新。
watch(
  () => chat.activeConv,
  () => {
    if (props.open) load();
  },
);
</script>

<template>
  <BaseModal :open="open" :title="t('share.title', { name: friendName() })" width="max-w-lg" @close="emit('close')">
    <div class="mb-2 flex items-center justify-between">
      <span class="text-xs text-[var(--gosslan-text-2)]">{{ t("share.desc") }}</span>
      <button
        class="tap-safe flex h-7 w-7 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        :title="t('share.refresh')"
        :aria-label="t('share.refreshAria')"
        @click="load"
      >
        <RefreshCw class="h-4 w-4" :class="loading ? 'animate-spin' : ''" />
      </button>
    </div>

    <div class="max-h-80 overflow-y-auto">
      <div v-if="error" class="py-4 text-sm text-[var(--gosslan-danger-ink)]" role="alert">{{ error }}</div>
      <div v-else-if="entries.length === 0 && !loading" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("share.empty") }}
      </div>

      <div
        v-for="e in entries"
        :key="e.path"
        class="flex items-center gap-2 rounded-[var(--gosslan-radius-sm)] px-2 transition hover:bg-[var(--gosslan-hover)]"
        :style="{ paddingLeft: `${12 + depth(e.path) * 16}px`, height: '36px' }"
      >
        <Folder v-if="e.is_dir" class="h-4 w-4 shrink-0 text-[var(--gosslan-warning-ink)]" />
        <span class="flex-1 truncate text-sm" :title="e.name">{{ e.name }}</span>
        <span v-if="!e.is_dir" class="text-[11px] text-[var(--gosslan-text-2)]">{{ humanSize(e.size) }}</span>
        <button
          v-if="!e.is_dir"
          class="tap-safe flex h-7 w-7 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-primary)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('common.download')" :aria-label="t('common.download')"
          @click="download(e)"
        >
          <Download class="h-4 w-4" />
        </button>
      </div>
    </div>
  </BaseModal>
</template>
