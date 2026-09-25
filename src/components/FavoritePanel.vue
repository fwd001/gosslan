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
import { t } from "@/i18n";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useBackLayer } from "@/composables/useBackLayer";
import ContextMenu from "@/components/ContextMenu.vue";
import ForwardModal from "@/components/message/ForwardModal.vue";
import BackArrow from "@/components/ui/BackArrow.vue";
import { useClipboard } from "@/composables/useClipboard";
import { fmtConversationTime } from "@/utils/time";
import { humanSize, rgba } from "@/utils/color";
import { FILE_KIND_COLORS, FILE_KIND_ICONS, fileExt, fileKindOf } from "@/utils/fileKind";
import { openLocalFile, saveLocalFile } from "@/utils/localFile";
import { dropFavoritePreview, loadFavoritePreview } from "@/utils/favoritePreview";
import { mergeItemLine, mergeSummary, parseMergePayload } from "@/utils/mergeCard";
import { isForwardableKind, isKnownKind, kindClass, UNSUPPORTED_KIND_LABEL } from "@/utils/messageKinds";
import { cardCopyText } from "@/utils/cardText";
import {
  TODO_STATUS_LABEL_KEY,
  TODO_STATUS_PILL,
  isTodoStatus,
  type TodoImage,
  type TodoStatus,
} from "@/utils/todos";
import TodoImageThumb from "@/components/TodoImageThumb.vue";
import {
  AlignLeft,
  BarChart3,
  Code,
  Copy,
  CornerUpLeft,
  Download,
  FolderOpen,
  Image as ImageIcon,
  ListTodo,
  Loader2,
  Megaphone,
  MoreHorizontal,
  ScrollText,
  Share2,
  Trash2,
  X,
} from "lucide-vue-next";
import type { FavoriteEntry, FileMeta, MsgKind } from "@/types";
import { api } from "@/api";

const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const { copyContent } = useClipboard();

/** 头部标题：选中某条收藏时显示该条名称，否则显示收藏总数/标题。 */
const favTitle = computed(() => (active.value ? displayName(active.value) : headerTitle.value));

/** 统一返回：详情态先退回列表，列表态再关闭整个收藏层（与框架返回键一致）。 */
function onFavBack() {
  if (active.value) active.value = null;
  else emit("close");
}

// 移动端两层返回（与框架返回键一致）：
//   —— 详情态（active）先退回列表；列表态（tab）再关掉整个收藏 tab（回到 chats）。
//   两条 active 互斥（一条看 active、一条看 !active），不会同时压栈，层级天然正确。
useBackLayer(
  () => app.isMobile && active.value !== null,
  () => {
    active.value = null;
  },
);
useBackLayer(
  () => app.isMobile && active.value === null,
  () => emit("close"),
);

type FilterKey = "all" | "text" | "image" | "file" | "card";
/** 类型筛选（与 PC 微信一致：全部 / 文本 / 图片 / 文件；卡片类是我们自己的：待办/投票/公告）。 */
const FILTERS: { key: FilterKey; label: string }[] = [
  { key: "all", label: "favorite.filterAll" },
  { key: "text", label: "favorite.filterText" },
  { key: "image", label: "favorite.filterImage" },
  { key: "file", label: "favorite.filterFile" },
  { key: "card", label: "favorite.filterCard" },
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
  // 卡片类（待办/投票/公告）：按用户 2026-09-21 的"无害改法"放进了收藏，搜索也要覆盖
  const card = cardText(f);
  if (card !== null) return card;
  return metaOf(f)?.name ?? "";
}

/**
 * 卡片类收藏（待办/投票/群公告）的**标题行**（列表行与详情标题共用）。
 * 完整文字形态走 `cardCopyText`（与消息右键的「复制」同一份实现，见 utils/cardText）。
 */
function cardText(f: FavoriteEntry): string | null {
  const full = cardCopyText(f.kind, f.content);
  if (!full) return null;
  // 去掉「群任务：」这类前缀与换行，给列表一行干净的标题
  const first = full.split("\n")[0] ?? "";
  const cut = first.indexOf("：");
  return (cut >= 0 ? first.slice(cut + 1) : first).trim() || t("favorite.untitled");
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
    // 卡片类：待办/投票/群公告（与 forwardable/favoritable 的白名单同一判据）
    if (filterKind.value === "card") return kindClass(f.kind) === "card";
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
  // 未知 kind（对端 Gosslan 比本机新）的载荷是 JSON：既不能原样显示，也不能落到
  // `displayName()` 的兜底 —— 那等于给一条不认识的东西编一个"文件"身份。
  if (!isKnownKind(f.kind)) return UNSUPPORTED_KIND_LABEL;
  // 卡片类（待办/投票/公告）：用户 2026-09-21 的"无害改法"放进收藏 ⇒ 这里要能正常显示
  const card = cardText(f);
  if (card !== null) return card.slice(0, 40) || t("favorite.untitled");
  if (f.kind === "text" || f.kind === "code") {
    const one = f.content.replace(/\s+/g, " ").trim();
    return one.slice(0, 40) || t("favorite.untitled");
  }
  if (f.kind === "merge") return parseMergePayload(f.content)?.title || t("merge.title");
  return displayName(f);
}

/**
 * 详情面板里那条收藏的"卡片文字形态"（待办=标题、投票=问题、公告=文本）；非卡片类为空。
 * 与列表行共用同一份 `cardText`，避免两处漂移。
 */
const activeCardText = computed(() => (active.value ? cardText(active.value) : null));

/** 卡片收藏的完整载荷（待办/投票/公告）；非卡片类为空对象。 */
const activeCardPayload = computed<Record<string, unknown>>(() => {
  const f = active.value;
  if (!f || kindClass(f.kind) !== "card") return {};
  try {
    const p = JSON.parse(f.content) as unknown;
    return p && typeof p === "object" ? (p as Record<string, unknown>) : {};
  } catch {
    return {};
  }
});

/**
 * 卡片收藏里带的图片（目前只有待办载荷有 `images`，每项是 `TodoImage`，字节按 sha256/cid 取）。
 * ⚠️ 收藏**详情**此前只渲染了卡片的文字 ⇒ 群任务的图片不显示（用户 2026-09-21：
 * 「收藏的详情里群任务图片不显示，你要做就做完整」）。这里复用 `TodoImageThumb`
 * —— 它自带 cid 读取 + 退避重试，与聊天里任务卡的表现完全一致。
 */
const activeCardImages = computed<TodoImage[]>(() => {
  const imgs = activeCardPayload.value.images;
  if (!Array.isArray(imgs)) return [];
  return imgs.filter(
    (x): x is TodoImage => !!x && typeof x === "object" && typeof (x as TodoImage).sha256 === "string",
  );
});

/** 待办的描述（没有就空串）；投票用不到。 */
const activeCardDesc = computed(() => {
  const d = activeCardPayload.value.description;
  return typeof d === "string" ? d.trim() : "";
});

/** 待办的状态胶囊（没有 status 字段时为空）。 */
const activeCardStatus = computed(() =>
  active.value && active.value.kind === "todo" && isTodoStatus(activeCardPayload.value.status)
    ? (activeCardPayload.value.status as TodoStatus)
    : null,
);

/** 投票的选项（只读展示）。 */
const activePollOptions = computed<string[]>(() => {
  const opts = activeCardPayload.value.options;
  return Array.isArray(opts) ? opts.filter((x): x is string => typeof x === "string") : [];
});

/** 列表行的摘要（标题之外再给一行，便于在列表里分辨）。 */
function rowSubtitle(f: FavoriteEntry): string {
  // 与 rowTitle 同一判据：不认识就不给任何"看起来像内容"的东西（载荷是 JSON）
  if (!isKnownKind(f.kind)) return "";
  const card = cardText(f);
  if (card !== null) return card.length > 40 ? `…${card.slice(40, 100)}` : "";
  if (f.kind === "text" || f.kind === "code") {
    const one = f.content.replace(/\s+/g, " ").trim();
    return one.length > 40 ? `…${one.slice(40, 100)}` : "";
  }
  if (f.kind === "merge") return mergeSummary(f.content);
  const size = metaOf(f)?.size ?? f.media_size;
  return size > 0 ? humanSize(size) : "";
}

function kindIcon(f: FavoriteEntry) {
  if (f.kind === "todo") return ListTodo;
  if (f.kind === "poll") return BarChart3;
  if (f.kind === "announcement") return Megaphone;
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
    if (kindClass(f.kind) === "card") {
      // 卡片收藏的"复制"= 它的完整文字形态（与消息右键的「复制」同一份实现）
      const text = cardCopyText(f.kind, f.content);
      if (!text) {
        app.toast(t("favorite.copyFail"), "error");
        return;
      }
      const ok = await copyContent("favorite", text);
      app.toast(t(ok ? "common.copied" : "favorite.copyFail"), ok ? "success" : "error");
      return;
    }
    if (!f.media_path || !f.available) {
      app.toast(t("favorite.mediaGone"), "error");
      return;
    }
    await api.copyFileToClipboard(f.media_path);
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
  // ⚠️ 先收起本页并翻到会话，再发起定位（与 `ResponsiveLayout.onOpenSearchHit` 同一顺序，
  // 用户 2026-09-24 #29「切换要瞬间响应」）。`locateMessageInConv` 会从最新一页往前
  // **逐页翻到 MAX_PAGES** 才可能报"没找到"，把它整个 await 在翻页之前 =
  // 点一条收藏后收藏页原地卡住几轮读库；找不到时如实 toast，人已经在会话里。
  if (app.isMobile) app.mobileView = "chat";
  emit("close");
  const r = await chat.locateMessageInConv(f.conv_id, f.msg_id);
  if (r !== "found") app.toast(t("favorite.msgGone"), "info");
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
  <!-- 普通整页内容（桌面端内嵌右栏、移动端作为底部 tab）。
       详情是「点具体收藏」才新开的一页：移动端用 fixed 全屏覆盖层（盖住底部 tab 栏），
       桌面端仍是左列表 + 右详情的两栏。详情页的 chevron 返回由下方移动端头部提供，
       不再各自画返回键（用户 2026-09-20）。 -->
  <div class="flex h-full min-h-0 flex-col">

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

    <div v-else class="flex min-h-0 flex-1 flex-col">
      <!-- 两栏：左列表（搜索+筛选+列表）+ 右详情；窄屏退化成两级（列表 ↔ 详情）。
           PC 微信收藏里搜索/筛选就收在左栏列表上方，不单独占一整条横栏。
           ⚠️ 整页**铺满、外层不留外边距**：`p-3` 会在左栏（列表底）外露出一圈白边，
           且左栏顶部缩进 12px，左上角就不是圆角了（用户 2026-09-20）。
           搜索框 / 筛选自己留内边距，列表行仍满宽。 -->
      <div class="flex min-h-0 flex-1">
        <!-- 左栏 = 列表底 `--gosslan-list`（与聊天/通讯录/搜索列表同族）：
             此前用 main 的白色 `--gosslan-chat` 打底，整栏发白，和其它列表不一致（用户 2026-09-20）。 -->
        <!-- ⚠️ `desktop:sm:flex-none` 不能省：`flex-1` 把 `flex-basis` 设成 0%，会让下面的 width 失效、
             左列被拉伸成「和详情各占一半」，宽度就和别处的列表列对不上了（用户 2026-09-20）。
             宽度走 `var(--gosslan-list-w)`（= 布局里可拖拽的列表宽），与聊天/通讯录列表一致。 -->
        <!-- ⚠️ 只在**移动端**才在选中后收起左列（两级：列表 ↔ 详情）：桌面端详情的返回头部是
             `v-if="app.isMobile"`，收起左列后**没有返回入口**、没法再选下一条，所以桌面端保持两栏。
             ⚠️ 更正（用户 2026-09-20 指出）：早先这条注释把「这里怎么有圆角」的根因写成这个，是**错的** ——
             那个圆角来自主区 `<main>` 自身的 `rounded-tl`，与是否收起左列无关，已单独删除。 -->
        <!-- ⚠️ 这里**不加** `desktop:sm:border-r`：两栏靠底色分栏（列表底 vs 详情白），与聊天/通讯录页一致；
             加一条竖线反而是别的页面没有的东西（用户 2026-09-20：「收藏页两栏中间多个分割线」）。 -->
        <div
          class="min-h-0 bg-[var(--gosslan-list)] desktop:sm:flex desktop:sm:w-[var(--gosslan-list-w)] desktop:sm:flex-none desktop:sm:flex-col"
          :class="app.isMobile && active ? 'hidden' : 'flex flex-1 flex-col'"
        >
          <div class="flex shrink-0 flex-col gap-2 px-2 pt-2">
            <input
              v-model="keyword"
              maxlength="50"
              autocomplete="off"
              :placeholder="t('favorite.searchPlaceholder')"
              class="w-full rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-field)] px-3 py-2 text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)] focus:border-transparent"
            />
            <!-- 类型筛选：独立胶囊 chips，与「群任务过滤 / 日志时间窗口」同款
                 （此前激活态写了不存在的 `bg-primary` 类导致选中没底色，用户 2026-09-20；
                 这里对齐 GroupTasksBoard 的标准 chip 配方）。 -->
            <div class="flex shrink-0 flex-wrap items-center gap-1">
              <button
                v-for="f in FILTERS"
                :key="f.key"
                type="button"
                class="tap-safe rounded-full px-2.5 py-1 text-xs transition"
                :class="filterKind === f.key
                  ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]'
                  : 'border border-transparent text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
                :aria-pressed="filterKind === f.key"
                @click="filterKind = f.key"
              >
                {{ t(f.label) }}
              </button>
            </div>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto">
          <!-- 每行：行本身是 <button>（可聚焦、可回车），移动端的「⋯」放在**兄弟层**用绝对定位
               —— 不能嵌在行按钮里（<button> 套 <button> 是非法 HTML，浏览器会把它拆出去，
               表现是"⋯ 点了没反应"或整行都被点）。 -->
          <div v-for="f in filtered" :key="f.id" class="relative">
            <button
              class="flex w-full items-start gap-2.5 px-3 py-2.5 text-left transition"
              :class="active?.id === f.id
                ? 'bg-[var(--gosslan-list-active)] text-[var(--gosslan-list-active-text)]'
                : 'hover:bg-[var(--gosslan-list-hover)]'"
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
            <!-- 微信式行间细分隔线：从文本列起（图标后缩进 = px-3 + 32 图标 + gap-2.5），
                 与聊天/通讯录列表同款（用户 2026-09-20：收藏列表的风格要和别的列表统一）。 -->
            <div class="pointer-events-none absolute bottom-0 left-[54px] right-0 h-px bg-[var(--gosslan-divider)]"></div>
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
             （用户 2026-09-17："这个操作的样式不行"）。
             移动端：详情是「点具体收藏」才新开的一页 —— 用 fixed 全屏覆盖层（盖住底部 tab 栏），
             顶部带框架式 chevron 返回（覆盖层自己带头部，不再依赖外部框架）。 -->
        <!-- 转场：只有移动端详情是「新开的一页」，才走 page-slide；桌面端是内嵌右栏，不转场。 -->
        <Transition :name="app.isMobile ? 'page-slide' : ''">
        <div
          v-if="active"
          :class="app.isMobile
            ? 'fixed inset-0 z-[60] flex flex-col bg-[var(--gosslan-bg)] pt-[env(safe-area-inset-top)]'
            : 'flex min-h-0 flex-1 flex-col'"
        >
          <!-- 移动端详情头部：与 MobilePageFrame **完全一致**的框架返回键
               （同款 BackArrow / h-8 w-8 热区 / panel 底 / px-3）。
               此前自绘了 stroke-width=2 + 圆角端点 + bg-bg，与其他转场页的返回箭头不一致
               （用户 2026-09-20「头部标题的返回箭头也不一样」，统一走 BackArrow 按平台适配）。 -->
          <header
            v-if="app.isMobile"
            class="flex shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-panel)] px-3"
            :style="{ height: 'var(--gosslan-header-h)' }"
          >
            <button
              class="tap-safe flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              :title="t('common.back')"
              :aria-label="t('common.back')"
              @click="onFavBack"
            >
              <BackArrow />
            </button>
            <span class="min-w-0 truncate text-[15px] font-medium text-[var(--gosslan-text)]" :title="favTitle">{{ favTitle }}</span>
          </header>

          <div class="min-h-0 flex-1 overflow-y-auto p-3">
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

            <!-- 卡片类（待办/投票/群公告）：收藏**详情**按卡片自己的文字形态渲染。
                 ⚠️ 必须排在下面"文件"的 `v-else` 之前 —— 否则卡片会落到文件分支，
                 把 JSON 载荷当文件名/后缀显示（用户 2026-09-21：「收藏的卡片收藏详情里不显示」）。 -->
            <div
              v-else-if="activeCardText !== null"
              class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-panel)] p-3"
            >
              <div class="flex items-start gap-2">
                <component
                  :is="kindIcon(active)"
                  class="mt-0.5 h-4 w-4 shrink-0 text-[var(--gosslan-text-2)]"
                  aria-hidden="true"
                />
                <div class="min-w-0 flex-1">
                  <div class="break-words whitespace-pre-wrap text-[13px] leading-relaxed text-[var(--gosslan-text)]">
                    {{ activeCardText }}
                  </div>
                  <!-- 待办的描述（卡片信息补全：之前只渲染了标题） -->
                  <p
                    v-if="activeCardDesc"
                    class="mt-1 whitespace-pre-wrap break-words text-[12px] leading-relaxed text-[var(--gosslan-text-2)]"
                  >
                    {{ activeCardDesc }}
                  </p>
                  <!-- 投票选项（只读） -->
                  <ul v-if="activePollOptions.length" class="mt-1.5 space-y-0.5">
                    <li
                      v-for="(o, i) in activePollOptions"
                      :key="i"
                      class="break-words text-[12px] leading-relaxed text-[var(--gosslan-text-2)]"
                    >
                      {{ o }}
                    </li>
                  </ul>
                  <!-- 群任务图片：与聊天里任务卡**同一组件**（按 cid 取字节 + 退避重试）。
                       用户 2026-09-21：「收藏的详情里群任务图片不显示，你要做就做完整」。 -->
                  <div v-if="activeCardImages.length" class="mt-2 flex flex-wrap gap-1.5">
                    <TodoImageThumb v-for="img in activeCardImages" :key="img.sha256" :image="img" />
                  </div>
                </div>
              </div>
              <div class="mt-1.5 flex items-center gap-2 text-[11px] text-[var(--gosslan-text-2)]">
                <span
                  v-if="activeCardStatus"
                  class="inline-flex h-5 items-center justify-center rounded-full px-2 font-medium leading-none"
                  :class="TODO_STATUS_PILL[activeCardStatus]"
                >
                  {{ t(TODO_STATUS_LABEL_KEY[activeCardStatus]) }}
                </span>
                <span v-else>{{ t(`favorite.cardKind.${active.kind}`) }}</span>
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
          <div class="mt-2 flex shrink-0 items-stretch gap-0.5 overflow-x-auto border-t border-[var(--gosslan-divider)] px-2 pb-2 pt-1">
            <button
              class="tap-safe flex min-w-14 flex-1 flex-col items-center gap-0.5 rounded-[var(--gosslan-radius-sm)] py-1.5 text-[11px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              @click="copyItem(active)"
            >
              <Copy class="h-4 w-4" aria-hidden="true" />
              <span class="whitespace-nowrap">{{ t("favorite.copy") }}</span>
            </button>
            <button
              v-if="isForwardableKind(active.kind)"
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
        </Transition>
        <div v-if="!active" class="hidden flex-1 items-center justify-center text-xs text-[var(--gosslan-text-2)] desktop:sm:flex">
          {{ t("favorite.pickOne") }}
        </div>
      </div>

      <!-- 删除二次确认（内联块，不再叠一个 Dialog） -->
      <div v-if="pendingDelete" class="m-3 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger-soft)] p-3">
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
