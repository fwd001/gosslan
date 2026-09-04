<script setup lang="ts">
import { computed, ref } from "vue";
import dayjs from "dayjs";
import { useAppStore } from "@/stores/useAppStore";
import { openPath } from "@tauri-apps/plugin-opener";
import CodeBlock from "@/components/CodeBlock.vue";
import { Check, Copy, Download, FileText } from "lucide-vue-next";
import { humanSize } from "@/utils/color";
import type { MessageRecord } from "@/types";

const props = defineProps<{ message: MessageRecord }>();
const app = useAppStore();
const mine = computed(() => props.message.sender_id === app.device?.device_id);
const copied = ref(false);

interface FileMeta {
  name: string;
  path: string;
  size: number;
}

const fileMeta = computed<FileMeta | null>(() => {
  if (props.message.kind !== "file") return null;
  try {
    return JSON.parse(props.message.content) as FileMeta;
  } catch {
    return null;
  }
});

const time = computed(() => dayjs(props.message.ts).format("HH:mm"));

async function copyText() {
  try {
    await navigator.clipboard.writeText(props.message.content);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    /* ignore */
  }
}
async function openFile() {
  if (fileMeta.value?.path) await openPath(fileMeta.value.path);
}
</script>

<template>
  <div class="flex gap-2 px-3" :class="mine ? 'flex-row-reverse' : ''">
    <div
      class="flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-full bg-primary text-white"
    >
      <img
        v-if="mine && app.device?.avatar"
        :src="app.device.avatar"
        class="h-full w-full object-cover"
      />
      <span v-else class="text-xs font-semibold">{{ mine ? (app.device?.nickname.slice(0, 1) || "我") : "对" }}</span>
    </div>

    <div class="flex min-w-0 max-w-[78%] flex-col" :class="mine ? 'items-end' : 'items-start'">
      <!-- 系统消息 -->
      <div
        v-if="message.kind === 'system'"
        class="w-full text-center text-xs text-[var(--gosslan-text-2)]"
      >
        {{ message.content }}
      </div>

      <!-- 文本 -->
      <div
        v-else-if="message.kind === 'text'"
        class="group relative rounded-2xl px-3 py-2 text-sm leading-relaxed shadow-sm"
        :class="mine ? 'bg-[var(--gosslan-bubble-mine)]' : 'bg-[var(--gosslan-bubble-other)]'"
      >
        <div class="whitespace-pre-wrap break-words">{{ message.content }}</div>
        <button
          class="absolute -top-3 right-1 hidden items-center gap-1 rounded-md border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-1.5 py-0.5 text-xs text-[var(--gosslan-text-2)] shadow group-hover:flex"
          @click="copyText"
        >
          <Check v-if="copied" class="h-3 w-3 text-emerald-500" />
          <Copy v-else class="h-3 w-3" />
          复制
        </button>
      </div>

      <!-- 代码 -->
      <div v-else-if="message.kind === 'code'" class="w-full min-w-[280px]">
        <CodeBlock :code="message.content" />
      </div>

      <!-- 图片 -->
      <div v-else-if="message.kind === 'image'" class="overflow-hidden rounded-xl" @click="openFile">
        <img :src="message.content" class="block max-h-72 max-w-full rounded-xl object-contain" />
      </div>

      <!-- 文件 -->
      <div
        v-else-if="message.kind === 'file' && fileMeta"
        class="flex min-w-[220px] items-center gap-3 rounded-xl bg-[var(--gosslan-bubble-other)] px-3 py-2.5 shadow-sm"
      >
        <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary-light text-primary">
          <FileText class="h-5 w-5" />
        </div>
        <div class="min-w-0 flex-1">
          <div class="truncate text-sm font-medium">{{ fileMeta.name }}</div>
          <div class="text-xs text-[var(--gosslan-text-2)]">{{ humanSize(fileMeta.size) }}</div>
        </div>
        <button
          class="flex h-8 w-8 items-center justify-center rounded-lg text-primary transition hover:bg-[var(--gosslan-hover)]"
          title="打开"
          @click="openFile"
        >
          <Download class="h-4 w-4" />
        </button>
      </div>

      <div v-else class="rounded-2xl bg-[var(--gosslan-bubble-other)] px-3 py-2 text-sm shadow-sm">
        {{ message.content }}
      </div>

      <div class="mt-0.5 text-[11px] text-[var(--gosslan-text-2)]">{{ time }}</div>
    </div>
  </div>
</template>
