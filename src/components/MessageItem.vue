<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import dayjs from "dayjs";
import { useAppStore } from "@/stores/useAppStore";
import { openPath } from "@tauri-apps/plugin-opener";
import CodeBlock from "@/components/CodeBlock.vue";
import { Check, Copy, Download, FileText } from "lucide-vue-next";
import { humanSize } from "@/utils/color";
import { findPreset, parsePeerStyle } from "@/utils/chatStyle";
import type { MessageRecord } from "@/types";

const props = withDefaults(
  defineProps<{
    message: MessageRecord;
    /** 上一条消息（同会话），用于连续消息合并与时间分割线 */
    prev?: MessageRecord | null;
    /** 群聊：显示发送者昵称 */
    isGroup?: boolean;
    /** 在本条消息上方显示未读分割线 */
    showUnreadDivider?: boolean;
    /** 群聊发送者昵称（由父组件解析） */
    senderName?: string;
  }>(),
  { prev: null, isGroup: false, showUnreadDivider: false, senderName: "" },
);

const app = useAppStore();
const mine = computed(() => props.message.sender_id === app.device?.device_id);
const copied = ref(false);

// ---------------- 显示样式（本机偏好 + 对端广播偏好） ----------------
/** 我发的消息用我的样式；对方发的消息优先用对方广播的样式（未同步过则回退本机）。 */
const preset = computed(() => {
  if (!mine.value) {
    const raw = app.peerStyles[props.message.sender_id];
    if (raw) return findPreset(parsePeerStyle(raw).preset);
  }
  return findPreset(app.chatStyle.preset);
});
const colors = computed(() =>
  app.dark ? preset.value.dark : preset.value.light,
);

/** 连续消息合并：同一发送者 5 分钟内的消息省略头像/昵称（紧凑模式可关）。 */
const sameSenderRun = computed(() => {
  if (!app.chatStyle.compact) return false;
  const p = props.prev;
  if (!p || p.kind === "system" || p.sender_id !== props.message.sender_id) return false;
  return props.message.ts - p.ts < 5 * 60 * 1000;
});
/** 时间分割线：与上一条间隔 ≥ 5 分钟。 */
const showTimeDivider = computed(() => {
  const p = props.prev;
  return !p || props.message.ts - p.ts >= 5 * 60 * 1000;
});
/** 群聊非本人消息首条：显示昵称。 */
const showNickname = computed(() => props.isGroup && !mine.value && !sameSenderRun.value);

const time = computed(() => dayjs(props.message.ts).format("HH:mm"));
const timeDividerText = computed(() => dayjs(props.message.ts).format("M月D日 HH:mm"));

// ---------------- 长文本折叠 ----------------
const LONG_TEXT_CHARS = 280;
const isLongText = computed(
  () => props.message.kind === "text" && props.message.content.length > LONG_TEXT_CHARS,
);
const collapsed = ref(true);

// ---------------- 文件 ----------------
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

// ---------------- 自定义右键菜单（复制） ----------------
const menu = ref<{ x: number; y: number } | null>(null);

function onContext(e: MouseEvent) {
  if (props.message.kind !== "text") return;
  e.preventDefault();
  // 位置夹紧，避免菜单溢出视口
  const mw = 140;
  const mh = 40;
  const x = Math.min(e.clientX, window.innerWidth - mw - 8);
  const y = Math.min(e.clientY, window.innerHeight - mh - 8);
  menu.value = { x: Math.max(8, x), y: Math.max(8, y) };
}

function closeMenu() {
  menu.value = null;
}
function onCopyFromMenu() {
  void copyText();
  closeMenu();
}

onMounted(() => {
  document.addEventListener("click", closeMenu);
  document.addEventListener("scroll", closeMenu, true);
});
onUnmounted(() => {
  document.removeEventListener("click", closeMenu);
  document.removeEventListener("scroll", closeMenu, true);
});
</script>

<template>
  <div :class="sameSenderRun ? 'py-0.5' : 'pb-2 pt-1'">
    <!-- 时间分割线（间隔 ≥ 5 分钟） -->
    <div v-if="showTimeDivider" class="my-2 text-center text-[11px] text-[var(--gosslan-text-2)]">
      {{ timeDividerText }}
    </div>

    <!-- 未读分割线（打开会话时定位的第一条未读上方） -->
    <div v-if="showUnreadDivider" class="my-1.5 flex items-center gap-2 px-3">
      <div class="h-px flex-1 bg-primary/30"></div>
      <span class="rounded-full bg-primary-light px-2 py-0.5 text-[11px] text-primary">以下是未读消息</span>
      <div class="h-px flex-1 bg-primary/30"></div>
    </div>

    <div class="flex gap-2 px-3" :class="mine ? 'flex-row-reverse' : ''">
      <!-- 头像：连续消息合并时省略（保留占位对齐） -->
      <div v-if="!sameSenderRun" class="flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-full bg-primary text-white">
        <img
          v-if="mine && app.device?.avatar"
          :src="app.device.avatar"
          class="h-full w-full object-cover"
        />
        <span v-else class="text-xs font-semibold">
          {{ mine ? (app.device?.nickname.slice(0, 1) || "我") : (senderName || props.message.sender_id).slice(0, 1) }}
        </span>
      </div>
      <div v-else class="w-8 shrink-0"></div>

      <div class="flex min-w-0 max-w-[78%] flex-col" :class="mine ? 'items-end' : 'items-start'">
        <!-- 群聊发送者昵称 -->
        <div v-if="showNickname" class="mb-0.5 px-1 text-xs text-[var(--gosslan-text-2)]">
          {{ senderName || props.message.sender_id }}
        </div>

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
          class="group relative rounded-2xl px-3 py-2 leading-relaxed shadow-sm"
          :style="{
            background: mine ? colors.mineBubble : colors.otherBubble,
            color: mine ? colors.mineText : colors.otherText,
            fontSize: 'var(--gosslan-msg-size, 14px)',
          }"
          @contextmenu="onContext"
        >
          <div
            class="whitespace-pre-wrap break-words"
            :class="isLongText && collapsed ? 'line-clamp-5' : ''"
          >{{ message.content }}</div>
          <!-- 长文本折叠条：展开/收起 + 复制 -->
          <div v-if="isLongText" class="mt-1.5 flex items-center gap-2 border-t pt-1.5" :style="{ borderColor: 'rgba(128,128,128,0.2)' }">
            <button
              class="text-xs opacity-70 transition hover:opacity-100"
              @click="collapsed = !collapsed"
            >
              {{ collapsed ? "展开全文" : "收起" }}
            </button>
            <button
              class="flex items-center gap-1 text-xs opacity-70 transition hover:opacity-100"
              @click="copyText"
            >
              <Check v-if="copied" class="h-3 w-3" />
              <Copy v-else class="h-3 w-3" />
              {{ copied ? "已复制" : "复制" }}
            </button>
          </div>
          <!-- 普通文本悬停复制按钮 -->
          <button
            v-else
            class="absolute -top-3 right-1 hidden items-center gap-1 rounded-md border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-1.5 py-0.5 text-xs text-[var(--gosslan-text-2)] shadow group-hover:flex"
            @click="copyText"
          >
            <Check v-if="copied" class="h-3 w-3 text-emerald-500" />
            <Copy v-else class="h-3 w-3" />
            复制
          </button>
        </div>

        <!-- 代码 -->
        <div v-else-if="message.kind === 'code'" class="w-full min-w-0">
          <CodeBlock :code="message.content" />
        </div>

        <!-- 图片 -->
        <div v-else-if="message.kind === 'image'" class="overflow-hidden rounded-xl" @click="openFile">
          <img :src="message.content" class="block max-h-72 max-w-full rounded-xl object-contain" />
        </div>

        <!-- 文件 -->
        <div
          v-else-if="message.kind === 'file' && fileMeta"
          class="flex min-w-[220px] items-center gap-3 rounded-xl px-3 py-2.5 shadow-sm"
          :style="{
            background: mine ? colors.mineBubble : colors.otherBubble,
            color: mine ? colors.mineText : colors.otherText,
          }"
        >
          <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary-light text-primary">
            <FileText class="h-5 w-5" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm font-medium">{{ fileMeta.name }}</div>
            <div class="text-xs opacity-70">{{ humanSize(fileMeta.size) }}</div>
          </div>
          <button
            class="flex h-8 w-8 items-center justify-center rounded-lg text-primary transition hover:bg-[var(--gosslan-hover)]"
            title="打开"
            @click="openFile"
          >
            <Download class="h-4 w-4" />
          </button>
        </div>

        <div v-else class="rounded-2xl px-3 py-2 text-sm shadow-sm" :style="{ background: colors.otherBubble, color: colors.otherText }">
          {{ message.content }}
        </div>

        <!-- 连续消息合并时省略时间戳（悬浮可见的最后一条） -->
        <div v-if="!sameSenderRun" class="mt-0.5 text-[11px] text-[var(--gosslan-text-2)]">{{ time }}</div>
      </div>
    </div>

    <!-- 右键复制菜单 -->
    <Teleport to="body">
      <div
        v-if="menu"
        class="fixed z-50 min-w-[120px] rounded-lg border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] p-1 shadow-xl"
        :style="{ left: menu.x + 'px', top: menu.y + 'px' }"
        @click.stop
      >
        <button
          class="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm transition hover:bg-[var(--gosslan-hover)]"
          @click="onCopyFromMenu"
        >
          <Check v-if="copied" class="h-4 w-4 text-emerald-500" />
          <Copy v-else class="h-4 w-4" />
          复制文本
        </button>
      </div>
    </Teleport>
  </div>
</template>
