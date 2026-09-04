<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { Menu, MenuButton, MenuItems, MenuItem } from "@headlessui/vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import MessageItem from "@/components/MessageItem.vue";
import VirtualList from "@/components/VirtualList.vue";
import {
  ArrowLeft,
  Code2,
  FilePlus,
  FolderOpen,
  Network,
  Send,
} from "lucide-vue-next";
import type { MessageRecord } from "@/types";

const emit = defineEmits<{ (e: "open-share"): void }>();

const app = useAppStore();
const chat = useChatStore();

const draft = ref("");
const codeMode = ref(false);
const listRef = ref<InstanceType<typeof VirtualList> | null>(null);

const conv = computed(() => chat.activeConversation);
const isGroup = computed(() => chat.activeConv?.startsWith("group:") ?? false);
const messages = computed(() => chat.messages[chat.activeConv ?? ""] ?? []);

const online = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return false;
  return chat.friends.some((f) => f.device_id === conv.value!.id && f.online);
});

function estimateHeight(m: MessageRecord): number {
  switch (m.kind) {
    case "code":
      return 340;
    case "image":
      return 300;
    case "file":
      return 92;
    case "system":
      return 28;
    default: {
      const lines = Math.max(1, Math.ceil(m.content.length / 40));
      return 24 + lines * 22 + 22;
    }
  }
}

watch(
  () => messages.value.length,
  async () => {
    await nextTick();
    listRef.value?.scrollToBottom();
  },
);

async function sendMsg(content?: string, kind?: string) {
  const convId = chat.activeConv;
  if (!convId) return;
  const text = content ?? draft.value;
  const k = kind ?? (codeMode.value ? "code" : "text");
  if (k === "text" && !text.trim()) return;
  await chat.send(convId, text, k);
  if (!kind) draft.value = "";
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    void sendMsg();
  }
}

async function attachFile(relay: boolean) {
  const convId = chat.activeConv;
  if (!convId) return;
  const picked = await openDialog({ multiple: false });
  if (typeof picked === "string") {
    if (relay) await chat.sendFileRelayTo(convId, picked);
    else await chat.sendFileTo(convId, picked);
  }
}

async function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  if (!items) return;
  for (const item of Array.from(items)) {
    if (item.kind === "file" && item.type.startsWith("image/")) {
      const f = item.getAsFile();
      if (f) {
        const dataUrl = await fileToDataUrl(f);
        await sendMsg(dataUrl, "image");
      }
      break;
    }
  }
}

function fileToDataUrl(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = reject;
    r.readAsDataURL(f);
  });
}
</script>

<template>
  <div class="flex h-full flex-col bg-[var(--gosslan-panel)]">
    <!-- 头部 -->
    <div
      class="flex items-center justify-between border-b border-[var(--gosslan-border)] px-4"
      style="height: 56px"
    >
      <div class="flex min-w-0 items-center gap-2">
        <button
          v-if="app.isMobile"
          class="flex h-8 w-8 items-center justify-center rounded-lg text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          @click="app.mobileView = 'list'"
        >
          <ArrowLeft class="h-5 w-5" />
        </button>
        <span class="truncate text-base font-semibold">{{ conv?.name || "会话" }}</span>
        <span v-if="!isGroup" class="text-xs text-[var(--gosslan-text-2)]">
          {{ online ? "在线" : "离线" }}
        </span>
      </div>
      <button
        v-if="!isGroup"
        class="flex items-center justify-center rounded-lg p-2 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        title="共享目录"
        @click="emit('open-share')"
      >
        <FolderOpen class="h-5 w-5" />
      </button>
    </div>

    <!-- 消息区（虚拟滚动） -->
    <div class="min-h-0 flex-1 bg-[var(--gosslan-bg)]">
      <div v-if="messages.length === 0" class="mt-20 text-center text-sm text-[var(--gosslan-text-2)]">
        暂无消息，打个招呼吧
      </div>
      <VirtualList
        v-else
        ref="listRef"
        :items="messages"
        :estimate-height="estimateHeight"
      >
        <template #default="{ item }">
          <MessageItem :message="item" />
        </template>
      </VirtualList>
    </div>

    <!-- 输入区 -->
    <div class="border-t border-[var(--gosslan-border)] px-3 py-2">
      <div class="mb-1.5 flex items-center gap-1">
        <button
          class="flex items-center gap-1 rounded-md px-2 py-1 text-xs transition"
          :class="codeMode ? 'bg-primary-light text-primary' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          @click="codeMode = !codeMode"
        >
          <Code2 class="h-3.5 w-3.5" />
          代码
        </button>
        <Menu as="div" class="relative">
          <MenuButton
            class="flex items-center justify-center rounded-md px-2 py-1 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
            title="发送文件"
          >
            <FilePlus class="h-4 w-4" />
          </MenuButton>
          <MenuItems
            class="absolute bottom-9 left-0 z-20 w-40 rounded-lg border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] p-1 shadow-lg"
          >
            <MenuItem v-slot="{ active }">
              <button
                class="flex w-full items-center gap-2 rounded-md px-3 py-2 text-sm"
                :class="active ? 'bg-[var(--gosslan-hover)]' : ''"
                @click="attachFile(false)"
              >
                <FilePlus class="h-4 w-4" /> 直接发送
              </button>
            </MenuItem>
            <MenuItem v-slot="{ active }">
              <button
                class="flex w-full items-center gap-2 rounded-md px-3 py-2 text-sm"
                :class="active ? 'bg-[var(--gosslan-hover)]' : ''"
                @click="attachFile(true)"
              >
                <Network class="h-4 w-4" /> 中继发送
              </button>
            </MenuItem>
          </MenuItems>
        </Menu>
        <span class="ml-auto text-[11px] text-[var(--gosslan-text-2)]">Enter 发送 · 支持粘贴图片</span>
      </div>
      <div class="flex items-end gap-2">
        <textarea
          v-model="draft"
          rows="1"
          class="max-h-32 min-h-9 flex-1 resize-none rounded-xl bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none placeholder:text-[var(--gosslan-text-2)]"
          :class="codeMode ? 'font-mono' : ''"
          :placeholder="codeMode ? '粘贴或输入代码…' : '输入消息…'"
          @keydown="onKeydown"
          @paste="onPaste"
        ></textarea>
        <button
          class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-primary text-white transition hover:bg-primary-hover disabled:opacity-40"
          :disabled="!draft.trim()"
          @click="sendMsg()"
        >
          <Send class="h-4 w-4" />
        </button>
      </div>
    </div>
  </div>
</template>
