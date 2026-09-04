<script setup lang="ts">
import { ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import BaseModal from "@/components/BaseModal.vue";
import { Download, Folder, RefreshCw } from "lucide-vue-next";
import { humanSize } from "@/utils/color";
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

async function load() {
  loading.value = true;
  error.value = null;
  entries.value = [];
  try {
    entries.value = await api.requestShareTree(friendId());
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

async function download(e: ShareEntry) {
  try {
    await api.downloadSharedFile(friendId(), e.path);
    app.toast(`开始下载「${e.name}」`, "success");
    chat.refreshTransfers();
  } catch (err) {
    app.toast(`下载失败：${err}`, "error");
  }
}

watch(
  () => props.open,
  (v) => {
    if (v) load();
  },
);
</script>

<template>
  <BaseModal :open="open" :title="`共享目录 · ${friendName()}`" width="max-w-lg" @close="emit('close')">
    <div class="mb-2 flex items-center justify-between">
      <span class="text-xs text-[var(--gosslan-text-2)]">对方共享的文件，点击下载将点对点传输</span>
      <button
        class="flex h-7 w-7 items-center justify-center rounded-lg text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        @click="load"
      >
        <RefreshCw class="h-4 w-4" :class="loading ? 'animate-spin' : ''" />
      </button>
    </div>

    <div class="max-h-80 overflow-y-auto">
      <div v-if="error" class="py-4 text-sm text-red-500">{{ error }}</div>
      <div v-else-if="entries.length === 0 && !loading" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
        对方未设置共享目录或目录为空
      </div>

      <div
        v-for="e in entries"
        :key="e.path"
        class="flex items-center gap-2 rounded-md px-2 transition hover:bg-[var(--gosslan-hover)]"
        :style="{ paddingLeft: `${12 + depth(e.path) * 16}px`, height: '36px' }"
      >
        <Folder v-if="e.is_dir" class="h-4 w-4 shrink-0 text-amber-500" />
        <span class="flex-1 truncate text-sm">{{ e.name }}</span>
        <span v-if="!e.is_dir" class="text-[11px] text-[var(--gosslan-text-2)]">{{ humanSize(e.size) }}</span>
        <button
          v-if="!e.is_dir"
          class="flex h-7 w-7 items-center justify-center rounded-lg text-primary transition hover:bg-[var(--gosslan-hover)]"
          title="下载"
          @click="download(e)"
        >
          <Download class="h-4 w-4" />
        </button>
      </div>
    </div>
  </BaseModal>
</template>
