<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import MessageItem from "@/components/MessageItem.vue";
import VirtualList from "@/components/VirtualList.vue";
import {
  ArrowDown,
  ArrowLeft,
  Code2,
  FilePlus,
  FolderOpen,
  Lock,
  Send,
} from "lucide-vue-next";
import type { MessageRecord } from "@/types";

const emit = defineEmits<{ (e: "open-share"): void }>();

const app = useAppStore();
const chat = useChatStore();

const draft = ref("");
const codeMode = ref(false);
const listRef = ref<InstanceType<typeof VirtualList> | null>(null);
const inputRef = ref<HTMLTextAreaElement | null>(null);

/** 输入框自适应高度：内容换行时自动长高，超过 5 行（max-h-32 ≈ 8rem）出现滚动。 */
function autoResize() {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = "auto";
  el.style.height = `${Math.min(el.scrollHeight, 128)}px`;
}

watch(draft, () => autoResize());
watch(codeMode, () => nextTick(() => autoResize()));

// 打开会话即聚焦输入框（移动端不自动弹软键盘）
watch(
  () => chat.activeConv,
  async () => {
    await nextTick();
    if (!app.isMobile) inputRef.value?.focus();
    autoResize();
  },
  { immediate: true },
);

const conv = computed(() => chat.activeConversation);
const isGroup = computed(() => chat.activeConv?.startsWith("group:") ?? false);
const messages = computed(() => chat.messages[chat.activeConv ?? ""] ?? []);

const online = computed(() => {
  if (!conv.value || conv.value.kind !== "single") return false;
  return chat.friends.some((f) => f.device_id === conv.value!.id && f.online);
});

/** 当前会话的第一条未读索引（后端 markRead 前已记录，随历史 prepend 偏移）。 */
const unreadIndex = computed(() => {
  const uj = chat.unreadJump;
  if (!uj || uj.convId !== chat.activeConv) return -1;
  return uj.index;
});

// 滚动贴底状态（离开底部时显示「回到最新」按钮）
const nearBottom = ref(true);
function onNearBottom(v: boolean) {
  nearBottom.value = v;
}

function jumpToLatest() {
  listRef.value?.scrollToBottom();
}

const LONG_TEXT_CHARS = 280;

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
      // 长文本默认折叠为 5 行 + 底部操作条
      const lines = m.content.length > LONG_TEXT_CHARS ? 5 : Math.max(1, Math.ceil(m.content.length / 40));
      return 24 + lines * 22 + (m.content.length > LONG_TEXT_CHARS ? 34 : 0) + 22;
    }
  }
}

// 打开会话/未读定位：优先跳到第一条未读（该消息贴视口顶部，分割线置上），
// 无未读则贴底。方向（上跳/下跳）由虚拟列表绝对定位直接定位，无布局抖动。
watch(
  () => chat.unreadJump,
  async (uj) => {
    if (!uj || uj.convId !== chat.activeConv) return;
    await nextTick();
    await nextTick();
    listRef.value?.scrollToIndex(uj.index, "top");
  },
  { immediate: true },
);

watch(
  () => messages.value.length,
  async (_, oldLen) => {
    if (oldLen === undefined) return;
    if (oldLen === 0) {
      // 初始加载：无未读定位才贴底（有未读时由 unreadJump watcher 跳到第一条未读）
      if (!(chat.unreadJump && chat.unreadJump.convId === chat.activeConv)) {
        await nextTick();
        listRef.value?.scrollToBottom();
      }
      return;
    }
    // 追加新消息：仅在本来贴近底部时贴底；向上加载历史由 VirtualList 锚定保持位置
    if (messages.value.length > oldLen && nearBottom.value) {
      const last = messages.value[messages.value.length - 1];
      const prev = messages.value[messages.value.length - 2];
      if (!prev || last.ts >= prev.ts) {
        await nextTick();
        listRef.value?.scrollToBottom();
      }
    }
  },
);

async function sendMsg(content?: string, kind?: string) {
  const convId = chat.activeConv;
  if (!convId) return;
  const text = content ?? draft.value;
  const k = kind ?? (codeMode.value ? "code" : "text");
  if (k === "text" && !text.trim()) return;
  try {
    await chat.send(convId, text, k);
    if (!kind) draft.value = "";
  } catch (e) {
    // 发送失败必须显式反馈（此前静默失败表现为「点了没反应」）
    app.toast(`发送失败：${e}`, "error");
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    void sendMsg();
  }
}

/** 统一发送文件：自动路由（直连优先，弱网/无直连自动中继），无需用户选择。 */
async function attachFile() {
  const convId = chat.activeConv;
  if (!convId) return;
  const picked = await openDialog({ multiple: false });
  if (typeof picked === "string") {
    await chat.sendFileTo(convId, picked);
  }
}

/** 触顶加载更早的历史消息。 */
function onLoadMore() {
  const convId = chat.activeConv;
  if (convId) void chat.loadMoreMessages(convId);
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
        <!-- 对方头像 + 在线角标（绿点=在线可连接；离线头像置灰 + 灰点） -->
        <div v-if="!isGroup" class="relative shrink-0">
          <div
            class="flex h-9 w-9 items-center justify-center overflow-hidden rounded-full bg-primary text-white"
            :class="!online ? 'grayscale opacity-70' : ''"
          >
            <img v-if="conv?.avatar" :src="conv.avatar" class="h-full w-full object-cover" />
            <span v-else class="text-sm font-semibold">{{ (conv?.name || "?").slice(0, 1).toUpperCase() }}</span>
          </div>
          <span
            class="absolute bottom-0 right-0 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-panel)]"
            :class="online ? 'bg-emerald-500' : 'bg-neutral-400'"
          ></span>
        </div>
        <span class="truncate text-base font-semibold">{{ conv?.name || "会话" }}</span>
        <!-- E2EE 徽标：v0.11.0 起恒开且不可关闭，始终显示绿锁 -->
        <span
          class="flex shrink-0 items-center gap-1 rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] text-emerald-600"
          title="端到端加密：消息经 X25519 + ChaCha20-Poly1305 加密，中继与旁观者无法查看"
        >
          <Lock class="h-3 w-3" />
          端到端加密
        </span>
        <span v-if="!isGroup" class="flex items-center gap-1 text-xs" :class="online ? 'text-emerald-600' : 'text-[var(--gosslan-text-2)]'">
          <span class="h-1.5 w-1.5 rounded-full" :class="online ? 'bg-emerald-500' : 'bg-neutral-400'"></span>
          {{ online ? "对方在线" : "对方离线" }}
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

    <!-- 消息区（虚拟滚动，仅纵向） -->
    <div class="relative min-h-0 flex-1 overflow-hidden bg-[var(--gosslan-bg)]">
      <div v-if="messages.length === 0" class="mt-20 text-center text-sm text-[var(--gosslan-text-2)]">
        暂无消息，打个招呼吧
      </div>
      <VirtualList
        v-else
        ref="listRef"
        :items="messages"
        :estimate-height="estimateHeight"
        @load-more="onLoadMore"
        @near-bottom="onNearBottom"
      >
        <template #default="{ item, index }">
          <MessageItem
            :message="item"
            :prev="index > 0 ? messages[index - 1] : null"
            :is-group="isGroup"
            :sender-name="isGroup ? chat.nicknameOf(item.sender_id) : ''"
            :show-unread-divider="index === unreadIndex"
          />
        </template>
      </VirtualList>

      <!-- 回到最新（离开底部时出现） -->
      <button
        v-if="!nearBottom"
        class="absolute bottom-4 right-5 z-10 flex items-center gap-1.5 rounded-full border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-3 py-1.5 text-xs text-[var(--gosslan-text)] shadow-lg transition hover:bg-[var(--gosslan-hover)]"
        @click="jumpToLatest"
      >
        <ArrowDown class="h-3.5 w-3.5" />
        回到最新
      </button>
    </div>

    <!-- 输入区：输入框在上，操作行（代码/文件/提示/发送）移到底部 -->
    <div class="border-t border-[var(--gosslan-border)] px-4 pb-4 pt-2.5">
      <div class="flex items-end gap-2">
        <textarea
          ref="inputRef"
          v-model="draft"
          rows="1"
          class="max-h-32 min-h-10 flex-1 resize-none overflow-y-auto rounded-xl bg-[var(--gosslan-bg)] px-3.5 py-2.5 text-sm leading-relaxed outline-none placeholder:text-[var(--gosslan-text-2)]"
          :class="codeMode ? 'font-mono' : ''"
          :placeholder="codeMode ? '粘贴或输入代码…' : '输入消息…'"
          @keydown="onKeydown"
          @paste="onPaste"
        ></textarea>
      </div>
      <div class="mt-2 flex items-center justify-between gap-2">
        <div class="flex min-w-0 items-center gap-1">
          <button
            class="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-xs transition"
            :class="codeMode ? 'bg-primary-light text-primary' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
            @click="codeMode = !codeMode"
          >
            <Code2 class="h-3.5 w-3.5" />
            代码
          </button>
          <button
            class="flex shrink-0 items-center justify-center rounded-md px-2 py-1 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
            title="发送文件（自动选择最优路线）"
            @click="attachFile"
          >
            <FilePlus class="h-4 w-4" />
          </button>
          <span class="ml-1 truncate text-[11px] text-[var(--gosslan-text-2)]">Enter 发送 · Shift+Enter 换行 · 支持粘贴图片</span>
        </div>
        <button
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-primary px-3.5 text-xs font-medium text-white transition hover:bg-primary-hover disabled:opacity-40"
          :disabled="!draft.trim()"
          @click="sendMsg()"
        >
          <Send class="h-3.5 w-3.5" />
          发送
        </button>
      </div>
    </div>
  </div>
</template>
