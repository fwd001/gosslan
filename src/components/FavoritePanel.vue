<script setup lang="ts">
/**
 * 收藏面板（仿 PC 微信）。
 *
 * ## 布局
 * PC 微信的收藏是「左列表 + 右详情」：左边一条条摘要，点一条在右边看全文 / 大图 / 文件卡片；
 * 顶部有搜索，下面有类型筛选（全部 / 文本 / 图片 / 文件）。移动端没有横向空间，
 * 退化成"列表 → 详情"两级（详情里给返回）。
 *
 * ## 数据来源与"为什么不用 useMessageFile"
 * 数据是 `chat.favorites`（后端 `favorites` 表），与消息列表无关 —— 收藏是**独立存储**：
 * 原消息删了、会话删了、缓存清了，收藏里的内容都还在（媒体在收藏时被复制到收藏目录）。
 * 因此图片预览走 `utils/favoritePreview`（按**收藏 id** 读副本），打开/另存走
 * `utils/localFile` 的**路径**入参。`useMessageFile` 是按 msg_id 反查消息表的，
 * 原消息没了就什么都读不到 —— 恰好是收藏要覆盖的场景。
 *
 * ## 浮层规则
 * 收藏页是**普通整页**（不再是 `BaseModal` Dialog），所以行菜单（`ContextMenu`）、图片查看
 * （普通全屏 `<button>`）、转发（`ForwardModal`，唯一的 Dialog）都可以直接压在它上面，互不冲突
 * —— 转发不再需要"先收面板再开"（那是旧版两个 Dialog 不能共存的约束）。
 * `ForwardModal` 必须写在页面根节点的**同级**：页面由 `v-if` 挂载，转发弹窗写进页面容器里
 * 会随页面一起卸载。
 */
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { t } from "@/i18n";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import ContextMenu from "@/components/ContextMenu.vue";
import ForwardModal from "@/components/message/ForwardModal.vue";
import { useClipboard } from "@/composables/useClipboard";
import { fmtConversationTime } from "@/utils/time";
import { humanSize, rgba } from "@/utils/color";
import { FILE_KIND_COLORS, FILE_KIND_ICONS, fileExt, fileKindOf } from "@/utils/fileKind";
import { openLocalFile, saveLocalFile } from "@/utils/localFile";
import { dropFavoritePreview, loadFavoritePreview } from "@/utils/favoritePreview";
import { mergeItemLine, mergeSummary, parseMergePayload } from "@/utils/mergeCard";
import {
  AlignLeft,
  Code,
  Copy,
  CornerUpLeft,
  Download,
  FolderOpen,
  Image as ImageIcon,
  Loader2,
  MoreHorizontal,
  ScrollText,
  Share2,
  Trash2,
  X,
} from "lucide-vue-next";
import type { FavoriteEntry, FileMeta, MsgKind } from "@/types";

const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const { copyContent } = useClipboard();

type FilterKey = "all" | "text" | "image" | "file";
/** 类型筛选（与 PC 微信一致：全部 / 文本 / 图片 / 文件）。 */
const FILTERS: { key: FilterKey; label: string }[] = [
  { key: "all", label: "favorite.filterAll" },
  { key: "text", label: "favorite.filterText" },
  { key: "image", label: "favorite.filterImage" },
  { key: "file", label: "favorite.filterFile" },
];

const loading = ref(false);
const failed = ref(false);
/** 搜索关键词（在标题 / 正文 / 文件名里找）。 */
const keyword = ref("");
const filterKind = ref<FilterKey>("all");
/** 右侧详情当前看的那条（null = 未选中）。移动端用它切换两级视图。 */
const active = ref<FavoriteEntry | null>(null);
/** 图片缩略图：收藏 id → objectURL（按需加载；删除时释放）。 */
const thumbs = ref<Record<string, string>>({});
const menu = ref<{ x: number; y: number; item: FavoriteEntry } | null>(null);
const viewer = ref<string | null>(null);
const pendingDelete = ref<FavoriteEntry | null>(null);
const forward = ref<FavoriteEntry | null>(null);

const items = computed(() => chat.favorites);

/** 页头标题（有收藏数时显示条数，否则显示「收藏」）。 */
const headerTitle = computed(() =>
  items.value.length ? t("favorite.count", { n: items.value.length }) : t("favorite.title"),
);

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    await chat.refreshFavorites();
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

// 每次进入页面都重拉：页面不在时用户可能在别处收藏过（消息菜单），缓存旧快照会少条目。
// 组件由 v-if 挂载，挂载即代表"刚打开"，各 ref 初始都是 null（干净状态），直接拉一次即可。
onMounted(() => void load());

/** 单条收藏里参与搜索的文本。 */
function searchText(f: FavoriteEntry): string {
  if (f.kind === "text" || f.kind === "code") return f.content;
  if (f.kind === "merge") {
    const p = parseMergePayload(f.content);
    if (!p) return mergeSummary(f.content);
    return [p.title, ...p.items.map((i) => `${i.sender} ${mergeItemLine(i)}`)].join(" ");
  }
  return metaOf(f)?.name ?? "";
}

/**
 * 搜索 + 筛选后的列表。
 *
 * 搜索是"在大列表里找一条"的主路径（PC 微信的搜索框在顶上），所以匹配范围要覆盖用户
 * 实际记得的信息：文本/代码看正文，图片/文件看文件名，合并转发看标题与每条摘要。
 */
const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  const byKind = (f: FavoriteEntry) => {
    if (filterKind.value === "all") return true;
    // 合并转发按"文本"归类：它的内容就是一段文字记录（筛选里没有单独的"聊天记录"档）
    if (filterKind.value === "text") return f.kind === "text" || f.kind === "code" || f.kind === "merge";
    return f.kind === filterKind.value;
  };
  return items.value.filter((f) => {
    if (!byKind(f)) return false;
    if (!kw) return true;
    return searchText(f).toLowerCase().includes(kw);
  });
});

/** 收藏里的 content 快照（图片/文件是 `{name,path,size,subtype}` 那段 JSON）。 */
function metaOf(f: FavoriteEntry): FileMeta | null {
  if (f.kind !== "image" && f.kind !== "file") return null;
  try {
    const m = JSON.parse(f.content) as Partial<FileMeta>;
    return {
      name: m.name ?? t("common.file"),
      path: m.path ?? f.media_path ?? "",
      size: m.size ?? f.media_size,
      subtype: m.subtype ?? (f.kind === "image" ? "image" : "file"),
    };
  } catch {
    return null;
  }
}

function displayName(f: FavoriteEntry): string {
  return metaOf(f)?.name ?? t("common.file");
}

/** 列表行的标题（PC 微信那种"一行标题 + 一行摘要"）。 */
function rowTitle(f: FavoriteEntry): string {
  if (f.kind === "text" || f.kind === "code") {
    const one = f.content.replace(/\s+/g, " ").trim();
    return one.slice(0, 40) || t("favorite.untitled");
  }
  if (f.kind === "merge") return parseMergePayload(f.content)?.title || t("merge.title");
  return displayName(f);
}

/** 列表行的摘要（标题之外再给一行，便于在列表里分辨）。 */
function rowSubtitle(f: FavoriteEntry): string {
  if (f.kind === "text" || f.kind === "code") {
    const one = f.content.replace(/\s+/g, " ").trim();
    return one.length > 40 ? `…${one.slice(40, 100)}` : "";
  }
  if (f.kind === "merge") return mergeSummary(f.content);
  const size = metaOf(f)?.size ?? f.media_size;
  return size > 0 ? humanSize(size) : "";
}

function kindIcon(f: FavoriteEntry) {
  if (f.kind === "merge") return ScrollText;
  if (f.kind === "image" || f.kind === "file") return FILE_KIND_ICONS[fileKindOf(displayName(f))];
  return f.kind === "code" ? Code : AlignLeft;
}

function kindStyle(f: FavoriteEntry) {
  if (f.kind === "image" || f.kind === "file") {
    const c = FILE_KIND_COLORS[fileKindOf(displayName(f))];
    const accent = app.dark ? c.dark : c.light;
    return { backgroundColor: rgba(accent, 0.12), color: accent };
  }
  const accent = app.dark ? "#a1a1aa" : "#71717a";
  return { backgroundColor: rgba(accent, 0.12), color: accent };
}

function senderName(f: FavoriteEntry): string {
  return chat.nicknameOf(f.sender_id);
}

/** 合并转发的详情行（在收藏详情里直接展开，不再套一层弹窗）。 */
const activeMergeItems = computed(() => {
  const f = active.value;
  if (!f || f.kind !== "merge") return [];
  return parseMergePayload(f.content)?.items ?? [];
});

/** 图片缩略图按需加载：只对"副本还在这台机器上"的图片收藏读字节。 */
watch(
  [items, thumbs],
  ([list]) => {
    for (const f of list) {
      if (f.kind !== "image" || !f.available || thumbs.value[f.id]) continue;
      void loadFavoritePreview(f.id, displayName(f)).then((r) => {
        if (r.url) thumbs.value = { ...thumbs.value, [f.id]: r.url };
      });
    }
  },
  { immediate: true },
);

function openMenuAt(e: MouseEvent, f: FavoriteEntry) {
  // 右键：菜单落在**鼠标处**；移动端「⋯」的 clientX/Y 往往是 0，只能按按钮位置算。
  if (e.type === "contextmenu") {
    menu.value = { x: e.clientX, y: e.clientY, item: f };
    return;
  }
  const rect = (e.currentTarget as HTMLElement | null)?.getBoundingClientRect();
  menu.value = rect
    ? { x: rect.right - 176, y: rect.bottom + 4, item: f }
    : { x: e.clientX, y: e.clientY, item: f };
}

/** 选中一条：桌面端同时更新右侧详情；移动端由模板切成详情视图。 */
function selectItem(f: FavoriteEntry) {
  active.value = f;
}

async function copyItem(f: FavoriteEntry) {
  menu.value = null;
  try {
    if (f.kind === "text" || f.kind === "code") {
      const ok = await copyContent("favorite", f.content);
      app.toast(t(ok ? "common.copied" : "favorite.copyFail"), ok ? "success" : "error");
      return;
    }
    if (f.kind === "merge") {
      // 合并转发的"复制"＝复制展开后的文字记录（复制一段 JSON 对用户没有意义）
      const p = parseMergePayload(f.content);
      const text = p
        ? [p.title, ...p.items.map((i) => `${i.sender}：${mergeItemLine(i)}`)].join("\n")
        : mergeSummary(f.content);
      const ok = await copyContent("favorite", text);
      app.toast(t(ok ? "common.copied" : "favorite.copyFail"), ok ? "success" : "error");
      return;
    }
    if (!f.media_path || !f.available) {
      app.toast(t("favorite.mediaGone"), "error");
      return;
    }
    await invoke("copy_file_to_clipboard", { path: f.media_path });
    app.toast(t("common.copied"), "success");
  } catch (e) {
    app.toastError(e, t("favorite.copyFail"));
  }
}

async function previewImage(f: FavoriteEntry) {
  if (!f.available) {
    app.toast(t("favorite.mediaGone"), "error");
    return;
  }
  const url = thumbs.value[f.id] ?? (await loadFavoritePreview(f.id, displayName(f))).url;
  if (!url) {
    app.toast(t("favorite.mediaGone"), "error");
    return;
  }
  viewer.value = url;
}

async function openItem(f: FavoriteEntry) {
  const meta = metaOf(f);
  if (!f.media_path || !meta || !f.available) {
    app.toast(t("favorite.mediaGone"), "error");
    return;
  }
  // 平台差异（Android 改走另存为）统一在 utils/localFile，与消息气泡同一条路径。
  try {
    await openLocalFile(f.media_path, meta.name);
  } catch (e) {
    app.toastError(e, t("favorite.openFail"));
  }
}

async function saveItem(f: FavoriteEntry) {
  const meta = metaOf(f);
  if (!f.media_path || !meta || !f.available) {
    app.toast(t("favorite.mediaGone"), "error");
    return;
  }
  try {
    const r = await saveLocalFile(f.media_path, meta.name);
    if (r === "done") app.toast(t("favorite.saved"), "success");
  } catch (e) {
    app.toastError(e, t("favorite.saveFail"));
  }
}

const forwardKind = computed<MsgKind>(() => forward.value?.kind ?? "text");
const forwardSnippet = computed(() => {
  const f = forward.value;
  return f ? rowTitle(f) : "";
});

function startForward(f: FavoriteEntry) {
  menu.value = null;
  forward.value = f;
  // 收藏页已是普通页面（不是 Dialog），转发弹窗直接压在它上面即可，
  // 不再需要先关页面（旧版是为了避开「BaseModal 与 ForwardModal 两个 Dialog 不能共存」）。
}

async function doForward(convId: string) {
  const f = forward.value;
  forward.value = null;
  if (!f) return;
  try {
    if (f.kind === "file") {
      if (!f.media_path) {
        app.toast(t("favorite.mediaGone"), "error");
        return;
      }
      if (convId.startsWith("group:")) await chat.sendGroupFileTo(convId.slice(6), f.media_path);
      else await chat.sendFileTo(convId, f.media_path);
    } else {
      await chat.send(convId, f.content, f.kind);
    }
    app.toast(t("chat.toast.forwarded"), "success");
  } catch (e) {
    app.toastError(e, t("chat.toast.forwardFail"));
  }
}

async function locate(f: FavoriteEntry) {
  menu.value = null;
  // 会话可能随「删除聊天记录」一起没了：`locateMessageInConv` 内部会 `openConversation`
  // → 会话不在列表时会**新建**一条（ensureConversation），用户只是点了个跳转，
  // 侧栏却凭空多出一个空会话。所以先判存在性。
  if (!chat.conversations.some((c) => c.id === f.conv_id)) {
    app.toast(t("favorite.msgGone"), "info");
    return;
  }
  const r = await chat.locateMessageInConv(f.conv_id, f.msg_id);
  if (r !== "found") {
    app.toast(t("favorite.msgGone"), "info");
    return;
  }
  if (app.isMobile) app.mobileView = "chat";
  emit("close");
}

function askDelete(f: FavoriteEntry) {
  menu.value = null;
  pendingDelete.value = f;
}

async function confirmDelete() {
  const f = pendingDelete.value;
  pendingDelete.value = null;
  if (!f) return;
  try {
    await chat.removeFavorite(f.id);
    // 顺手释放预览缓存：objectURL 不 revoke 会一直占着那份字节的内存。
    dropFavoritePreview(f.id);
    const rest = { ...thumbs.value };
    delete rest[f.id];
    thumbs.value = rest;
    if (active.value?.id === f.id) active.value = null;
    app.toast(t("favorite.deleted"), "success");
  } catch (e) {
    app.toastError(e, t("favorite.deleteFail"));
  }
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <!-- 页头：仅移动端需要（返回 + 标题）。桌面端参考 PC 微信收藏：搜索 + 筛选直接顶到顶部，
         不再垫一条大空白标题栏（用户 2026-09-18：「顶部这个大空白的设计不好看」）。 -->
    <header
      v-if="app.isMobile"
      class="flex shrink-0 items-center gap-1 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-chat)] px-4"
      :style="{ height: 'var(--gosslan-header-h)' }"
    >
      <button
        v-if="app.isMobile"
        class="tap-safe -ml-1 flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]"
        :aria-label="t('common.back')"
        @click="emit('close')"
      >
        ←
      </button>
      <span class="min-w-0 truncate text-[15px] font-medium" :title="headerTitle">
        {{ headerTitle }}
      </span>
    </header>

    <div
      v-if="loading"
      class="flex flex-1 items-center justify-center gap-2 text-sm text-[var(--gosslan-text-2)]"
    >
      <Loader2 class="h-4 w-4 animate-spin" />
      {{ t("favorite.loading") }}
    </div>

    <div
      v-else-if="failed"
      class="flex flex-1 items-center justify-center text-sm text-[var(--gosslan-text-2)]"
    >
      {{ t("favorite.loadFail") }}
    </div>

    <div
      v-else-if="items.length === 0"
      class="flex flex-1 items-center justify-center text-sm text-[var(--gosslan-text-2)]"
    >
      {{ t("favorite.empty") }}
    </div>

    <div v-else class="flex min-h-0 flex-1 flex-col gap-3 p-3">
      <!-- 两栏：左列表（搜索+筛选+列表）+ 右详情；移动端退化成两级（列表 ↔ 详情）。
           PC 微信收藏里搜索/筛选就收在左栏列表上方，不单独占一整条横栏。 -->
      <div class="flex min-h-0 flex-1 gap-3">
        <div
          class="min-h-0 border-[var(--gosslan-divider)] pr-1 sm:w-64 sm:shrink-0 sm:flex sm:flex-col sm:gap-2 sm:border-r"
          :class="active ? 'hidden' : 'flex flex-1 flex-col gap-2'"
        >
          <input
            v-model="keyword"
            maxlength="50"
            autocomplete="off"
            :placeholder="t('favorite.searchPlaceholder')"
            class="w-full rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-transparent px-3 py-2 text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)] focus:border-transparent"
          />
          <div class="flex items-center gap-1">
            <button
              v-for="f in FILTERS"
              :key="f.key"
              class="tap-safe rounded-full px-2.5 py-1 text-xs transition"
              :class="filterKind === f.key
                ? 'bg-primary text-white'
                : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
              @click="filterKind = f.key"
            >
              {{ t(f.label) }}
            </button>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto">
          <!-- 每行：行本身是 <button>（可聚焦、可回车），移动端的「⋯」放在**兄弟层**用绝对定位
               —— 不能嵌在行按钮里（<button> 套 <button> 是非法 HTML，浏览器会把它拆出去，
               表现是"⋯ 点了没反应"或整行都被点）。 -->
          <div v-for="f in filtered" :key="f.id" class="relative">
            <button
              class="flex w-full items-start gap-2.5 rounded-[var(--gosslan-radius-md)] px-2 py-2 text-left transition"
              :class="active?.id === f.id ? 'bg-[var(--gosslan-hover)]' : 'hover:bg-[var(--gosslan-hover)]'"
              @click="selectItem(f)"
              @contextmenu.prevent="openMenuAt($event, f)"
            >
              <span
                class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)]"
                :style="kindStyle(f)"
              >
                <component :is="kindIcon(f)" class="h-4 w-4" />
              </span>
              <span class="min-w-0 flex-1" :class="app.isMobile ? 'pr-7' : ''">
                <span class="block truncate text-[13px] text-[var(--gosslan-text)]" :title="rowTitle(f)">
                  {{ rowTitle(f) }}
                </span>
                <span
                  v-if="rowSubtitle(f)"
                  class="mt-0.5 block truncate text-[11px] text-[var(--gosslan-text-2)]"
                  :title="rowSubtitle(f)"
                >
                  {{ rowSubtitle(f) }}
                </span>
                <span class="mt-0.5 block text-[11px] text-[var(--gosslan-text-2)]">
                  {{ fmtConversationTime(f.favorited_at) }}
                  <template v-if="!f.available"> · {{ t("favorite.mediaGone") }}</template>
                </span>
              </span>
            </button>
            <button
              v-if="app.isMobile"
              type="button"
              class="tap-safe absolute right-1 top-2 flex h-7 w-7 items-center justify-center rounded-[var(--gosslan-radius-sm)] text-[var(--gosslan-text-2)]"
              :aria-label="t('favorite.more')"
              @click.stop="openMenuAt($event, f)"
            >
              <MoreHorizontal class="h-4 w-4" />
            </button>
          </div>
          <div v-if="filtered.length === 0" class="py-8 text-center text-xs text-[var(--gosslan-text-2)]">
            {{ t("favorite.noMatch") }}
          </div>
        </div>
        </div>

        <!-- 右侧详情：全文 / 大图 / 文件卡片 + 底部操作条（PC 微信的收藏详情同样如此）。
             操作条**钉在详情底部**而不是跟在正文后面：正文长短不一，跟排会忽上忽下；
             且旧版内联按钮排一半带图标一半不带，flex-wrap 一换行就参差不齐
             （用户 2026-09-17："这个操作的样式不行"）。 -->
        <div v-if="active" class="flex min-h-0 flex-1 flex-col">
          <div class="min-h-0 flex-1 overflow-y-auto">
            <button
              v-if="app.isMobile"
              class="tap-safe mb-2 text-[13px] text-[var(--gosslan-accent-ink)]"
              @click="active = null"
            >
              ← {{ t("common.back") }}
            </button>

            <div class="mb-2 flex items-center gap-2 text-[11px] text-[var(--gosslan-text-2)]">
              <span class="truncate" :title="senderName(active)">{{ senderName(active) }}</span>
              <span>·</span>
              <span>{{ t("favorite.from", { time: fmtConversationTime(active.favorited_at) }) }}</span>
            </div>

            <div
              v-if="active.kind === 'text' || active.kind === 'code'"
              class="gosslan-selectable break-words whitespace-pre-wrap rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-panel)] p-3 text-[13px] text-[var(--gosslan-text)]"
            >
              {{ active.content }}
            </div>

            <img
              v-else-if="active.kind === 'image'"
              :src="thumbs[active.id] ?? ''"
              class="max-h-72 w-full rounded-[var(--gosslan-radius-md)] object-contain"
              :alt="displayName(active)"
            />

            <div v-else-if="active.kind === 'merge'" class="space-y-2">
              <div class="text-xs text-[var(--gosslan-text-2)]">{{ mergeSummary(active.content) }}</div>
              <div class="space-y-1.5">
                <div v-for="(it, i) in activeMergeItems" :key="i" class="space-y-0.5">
                  <div class="text-[11px] text-[var(--gosslan-text-2)]">{{ it.sender }}</div>
                  <div class="break-words whitespace-pre-wrap text-[13px] text-[var(--gosslan-text)]">
                    <template v-if="it.kind === 'text' || it.kind === 'code'">{{ it.content }}</template>
                    <template v-else>{{ mergeItemLine(it) }}</template>
                  </div>
                </div>
              </div>
            </div>

            <div v-else class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-panel)] p-3">
              <div class="truncate text-[13px] font-medium text-[var(--gosslan-text)]" :title="displayName(active)">
                {{ displayName(active) }}
              </div>
              <div class="mt-1 text-[11px] text-[var(--gosslan-text-2)]">
                {{ fileExt(displayName(active)).toUpperCase() }} ·
                {{ humanSize(metaOf(active)?.size ?? active.media_size) }}
              </div>
            </div>
          </div>

          <!-- 底部操作条：图标在上、11px 字在下、等宽分布（微信 PC 收藏详情同款）。
               每项**必带图标** —— 旧版"查看图片/另存为"是裸文字，跟带图标项混排就是参差感的主因。
               min-w-14 保证字不挤压截断；真放不下时横向滚动兜底，绝不折行。 -->
          <div class="mt-2 flex shrink-0 items-stretch gap-0.5 overflow-x-auto border-t border-[var(--gosslan-divider)] pt-1">
            <button
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="copyItem(active)"
            >
              <Copy class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.copy") }}</span>
            </button>
            <button
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="startForward(active)"
            >
              <Share2 class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.forward") }}</span>
            </button>
            <button
              v-if="active.kind === 'image'"
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="previewImage(active)"
            >
              <ImageIcon class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.viewImage") }}</span>
            </button>
            <button
              v-if="active.kind === 'file'"
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="openItem(active)"
            >
              <FolderOpen class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.open") }}</span>
            </button>
            <button
              v-if="active.kind === 'image' || active.kind === 'file'"
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="saveItem(active)"
            >
              <Download class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.saveAs") }}</span>
            </button>
            <button
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="locate(active)"
            >
              <CornerUpLeft class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.locate") }}</span>
            </button>
            <button
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-danger-soft)]"
              @click="askDelete(active)"
            >
              <Trash2 class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.delete") }}</span>
            </button>
          </div>
        </div>
        <div v-else class="hidden flex-1 items-center justify-center text-xs text-[var(--gosslan-text-2)] sm:flex">
          {{ t("favorite.pickOne") }}
        </div>
      </div>

      <!-- 删除二次确认（内联块，不再叠一个 Dialog） -->
      <div v-if="pendingDelete" class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger-soft)] p-3">
        <p class="text-sm leading-relaxed text-[var(--gosslan-text)]">
          {{ t("favorite.deleteConfirm") }}
        </p>
        <div class="mt-3 flex justify-end gap-2">
          <button
            class="tap-safe rounded-[var(--gosslan-radius-md)] px-4 py-2 text-sm text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
            @click="pendingDelete = null"
          >
            <X class="mr-1 inline h-3.5 w-3.5" />{{ t("common.cancel") }}
          </button>
          <button
            class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-4 py-2 text-sm text-white transition hover:opacity-90"
            @click="confirmDelete"
          >
            {{ t("common.delete") }}
          </button>
        </div>
      </div>
    </div>

    <!-- 行菜单：复制 / 转发 / 跳回原消息 / 删除（桌面右键与移动 ⋯ 共用一份实现） -->
    <ContextMenu v-if="menu" :x="menu.x" :y="menu.y" :estimated-height="180" @close="menu = null">
      <button role="menuitem" class="gosslan-menu-item" @click="copyItem(menu.item)">
        <Copy />
        {{ t("favorite.copy") }}
      </button>
      <button role="menuitem" class="gosslan-menu-item" @click="startForward(menu.item)">
        <Share2 />
        {{ t("favorite.forward") }}
      </button>
      <button role="menuitem" class="gosslan-menu-item" @click="locate(menu.item)">
        <CornerUpLeft />
        {{ t("favorite.locate") }}
      </button>
      <div class="gosslan-menu-sep" role="separator"></div>
      <button
        role="menuitem"
        class="gosslan-menu-item gosslan-menu-item--danger"
        @click="askDelete(menu.item)"
      >
        <Trash2 />
        {{ t("favorite.delete") }}
      </button>
    </ContextMenu>

    <!-- 图片查看：整屏点一下就走 -->
    <button
      v-if="viewer"
      class="fixed inset-0 z-[90] flex items-center justify-center bg-black/80"
      :aria-label="t('common.closeEsc')"
      @click="viewer = null"
    >
      <img :src="viewer" class="max-h-[85vh] max-w-[92vw] object-contain" alt="" />
    </button>
  </div>

  <!-- 转发弹窗：必须是根节点的同级，不能放进页面容器（页面一关会被一起卸载） -->
  <ForwardModal
    :open="!!forward"
    :kind="forwardKind"
    :snippet="forwardSnippet"
    @close="forward = null"
    @pick="doForward"
  />
</template>
