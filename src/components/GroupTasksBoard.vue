<script setup lang="ts">
/**
 * 群任务**看板**（无壳）：应用内弹窗（`GroupTasksPanel`）与独立窗口（`GroupTodosWindow`）
 * 共用这一份，区别只在 `standalone`（窗口形态：撑满高度、内部滚动占满剩余空间）。
 *
 * **观感（用户 2026-09-17：「还是太丑，至少要参考飞书/微信/钉钉」）**：走**微信式极简** ——
 * 无卡片无边框，纯列表行 + 分组小标题 + 分割线，尽量少色块；行内只保留
 * **序号 + 标题 + 状态 + 元信息**（描述/图片/所有操作点开**任务详情**看，见 `TodoDetailDialog`），
 * 这样行高统一（不再"行高参差、满屏 pill"）。
 *
 * **归档**：不再自动归档（完成只记 `doneAt`）——「已归档」是筛选里的一个胶囊（带计数），
 * 完成后可手动归档，没手动归档的满 7 天由 `isEffectivelyArchived` 自动归档。
 *
 * **数据来源**：`foldTodos(该群会话的已加载消息)`。任务与群公告同属 `Card` kind ——
 * 不进消息时间线，只在这里折叠展示。⚠️ 只统计**已加载的消息页**（既有架构口径）。
 *
 * **权限**：与后端 `commands::may_update_todo` 一致（`canUpdateTodo` / `canEditAssignees`
 * 是显示用的镜像）—— 界面只是"不给按钮"，真正的拦截在后端命令里。
 */
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isAndroid } from "@/utils/platform";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { useMemberProfile } from "@/composables/useMemberProfile";
import BaseModal from "@/components/BaseModal.vue";
import TodoDetailDialog from "@/components/TodoDetailDialog.vue";
import TodoImageThumb from "@/components/TodoImageThumb.vue";
import {
  TODO_STATUSES,
  TODO_STATUS_CLASS,
  TODO_STATUS_DEFAULT,
  TODO_STATUS_LABEL_KEY,
  TODO_STATUS_PILL,
  canEditAssignees,
  canUpdateTodo,
  foldTodos,
  isEffectivelyArchived,
  parseTodo,
  type TodoImage,
  type TodoItem,
  type TodoStatus,
} from "@/utils/todos";
import { api } from "@/api";
import { t } from "@/i18n";
import { Check, CheckCircle2, ChevronDown, Circle, CircleDot, Clock, ImagePlus, Plus, X } from "lucide-vue-next";

const props = defineProps<{ groupId: string | null; open?: boolean; standalone?: boolean }>();

const app = useAppStore();
const chat = useChatStore();
const { memberProfile, myId } = useMemberProfile();

const convId = computed(() => (props.groupId ? `group:${props.groupId}` : ""));
const group = computed(() => chat.groups.find((g) => g.id === props.groupId) ?? null);
const groupCreator = computed(() => group.value?.creator ?? "");

/** 折叠出全部任务（按创建版本从新到旧），再拆成「活动」与「已归档」。 */
const todos = computed(() => foldTodos(chat.messages[convId.value] ?? []));
const activeTodos = computed(() => todos.value.filter((x) => !isEffectivelyArchived(x)));
const archivedTodos = computed(() => todos.value.filter((x) => isEffectivelyArchived(x)));
const doneCount = computed(() => todos.value.filter((x) => x.status === "done").length);

// ---------------- 筛选（全部 / 给我的 / 我创建的 / 已归档） ----------------

type TodoFilter = "all" | "mine" | "created" | "archived";

const FILTERS: { key: TodoFilter; labelKey: string }[] = [
  { key: "all", labelKey: "todo.filter.all" },
  { key: "mine", labelKey: "todo.filter.mine" },
  { key: "created", labelKey: "todo.filter.created" },
  { key: "archived", labelKey: "todo.filter.archived" },
];

const filter = ref<TodoFilter>("all");

/** 行内状态菜单：同一时间只开一个（行是 v-for，不能每行一个 bool）。 */
const rowMenuId = ref<string | null>(null);
function toggleRowMenu(todoId: string) {
  rowMenuId.value = rowMenuId.value === todoId ? null : todoId;
}
function closeRowMenu() {
  rowMenuId.value = null;
}

function isAssignedToMe(x: TodoItem): boolean {
  return !!myId.value && x.assignees.includes(myId.value);
}
function isCreatedByMe(x: TodoItem): boolean {
  return !!myId.value && x.creator === myId.value;
}

/** 活动任务按当前筛选（"已归档"分支不走这里）。 */
const filteredActive = computed(() => {
  if (filter.value === "mine") return activeTodos.value.filter(isAssignedToMe);
  if (filter.value === "created") return activeTodos.value.filter(isCreatedByMe);
  return activeTodos.value;
});

/** 按状态分组（空组不占位）——只对**筛选后的活动任务**分组。 */
const grouped = computed(() =>
  TODO_STATUSES.map((status) => ({
    status,
    items: filteredActive.value.filter((x) => x.status === status),
  })).filter((g) => g.items.length > 0),
);

/** 各筛选项的条数（分段控件上的计数）。 */
const counts = computed<Record<TodoFilter, number>>(() => ({
  all: activeTodos.value.length,
  mine: activeTodos.value.filter(isAssignedToMe).length,
  created: activeTodos.value.filter(isCreatedByMe).length,
  archived: archivedTodos.value.length,
}));

/**
 * 序号：按**当前显示顺序** 1..N（切筛选/切分组会重排 —— 用户要的是"列表里第几个"，
 * 内部 `todoId` 不适合展示）。
 */
const ordinals = computed(() => {
  const m = new Map<string, number>();
  let n = 0;
  if (filter.value === "archived") {
    for (const x of archivedTodos.value) m.set(x.todoId, ++n);
  } else {
    for (const g of grouped.value) for (const x of g.items) m.set(x.todoId, ++n);
  }
  return m;
});

/** 当前视图的条数（空态分流用）。 */
const shownCount = computed(() =>
  filter.value === "archived" ? archivedTodos.value.length : filteredActive.value.length,
);

/** 分组小标题的图标（与状态语汇一致：待办=空心圈、进行中=实心点、延期=时钟、完成=对勾圈）。 */
const STATUS_ICON: Record<TodoStatus, typeof Circle> = {
  todo: Circle,
  doing: CircleDot,
  overdue: Clock,
  done: CheckCircle2,
};

function statusText(s: TodoStatus): string {
  return t(TODO_STATUS_LABEL_KEY[s]);
}

/** 指派人名（顿号连接；空 = 未指派）。 */
function assigneeNames(x: TodoItem): string {
  return x.assignees.map((a) => memberProfile(a).name).join("、");
}

// ---------------- 权限（显示用的镜像，后端才是权威） ----------------
function canChangeStatus(x: TodoItem): boolean {
  return canUpdateTodo(x, myId.value, groupCreator.value, false);
}
function canEditStructure(x: TodoItem): boolean {
  return canUpdateTodo(x, myId.value, groupCreator.value, true);
}
function canEditAssigneesOf(x: TodoItem): boolean {
  return canEditAssignees(x, myId.value, groupCreator.value);
}

// ---------------- 任务详情（点行打开） ----------------
/** 只存 id，详情项从折叠结果**实时查**：改完状态立刻反映，任务被删则自动关闭。 */
const detailId = ref<string | null>(null);
const detailItem = computed(
  () => (detailId.value ? (todos.value.find((x) => x.todoId === detailId.value) ?? null) : null),
);
const detailArchived = computed(() => (detailItem.value ? isEffectivelyArchived(detailItem.value) : false));

function openDetail(x: TodoItem) {
  detailId.value = x.todoId;
}
function closeDetail() {
  detailId.value = null;
}

// ---------------- 新建 / 编辑（内联表单，不开第二层弹窗） ----------------
const draft = ref<{
  todoId: string | null;
  title: string;
  assignees: string[];
  /** 草稿内的状态：**只在编辑时**可改（新建恒为「待办」，`send_group_todo` 不收状态）。
   *  表单里的改动要到「保存」才写库，所以这里用分段控件没有"误触改状态"的问题。 */
  status: TodoStatus;
  description: string;
  images: TodoImage[];
  /** 本次编辑新选中的图片本地路径，按 sha256 索引（仅需随字节管线投递，不进定义元数据）。
   *  用 sha256 而不是下标：删掉一张**已有**图片时，下标会与新图错位（把新图的待投递路径误删）。 */
  imagePaths: Record<string, string>;
} | null>(null);
const saving = ref(false);

/** 当前草稿能否改标题：新建随便改；编辑时只有创建者/群主能改（被指派人只能改指派人）。 */
const draftCanEditTitle = computed(() => {
  const d = draft.value;
  if (!d) return true;
  if (!d.todoId) return true;
  const cur = todos.value.find((x) => x.todoId === d.todoId);
  return cur ? canEditStructure(cur) : false;
});

function startCreate() {
  draft.value = {
    todoId: null,
    title: "",
    assignees: myId.value ? [myId.value] : [],
    status: TODO_STATUS_DEFAULT,
    description: "",
    images: [],
    imagePaths: {},
  };
}
function startEdit(x: TodoItem) {
  draft.value = {
    todoId: x.todoId,
    title: x.title,
    assignees: [...x.assignees],
    status: x.status,
    description: x.description,
    images: [...x.images],
    imagePaths: {},
  };
}
/** 详情里点「编辑」：关详情、展开内联表单。 */
function editFromDetail(x: TodoItem) {
  closeDetail();
  startEdit(x);
}
function toggleAssignee(id: string) {
  const d = draft.value;
  if (!d) return;
  d.assignees = d.assignees.includes(id) ? d.assignees.filter((a) => a !== id) : [...d.assignees, id];
}

/** 选图并写入「元数据 + 待投递路径」：元数据随定义跨端同步，字节另走群文件管线。 */
async function addImage() {
  const d = draft.value;
  if (!d) return;
  const picked = await openDialog({
    multiple: true,
    filters: [{ name: t("todo.imageFilter"), extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp"] }],
  });
  const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
  await addImageFromPaths(paths);
}

/** 公共入口：把若干本地图片路径加进草稿（文件选择器 / 粘贴 / 拖放共用）。 */
async function addImageFromPaths(paths: string[]) {
  const d = draft.value;
  if (!d) return;
  for (const p of paths) {
    try {
      // Android 给 content://，先落成本地路径；桌面原样返回。
      const local = await api.importPickedFile(p);
      const meta = await api.todoImageMeta(local);
      d.images.push(meta);
      d.imagePaths[meta.sha256] = local;
    } catch (e) {
      app.toastError(e, t("todo.imagePickFail"));
    }
  }
}

// ---------------- 图片的粘贴与拖放（用户 2026-09-17） ----------------
// 只在草稿弹窗打开期间挂监听；关闭即全部退订并复位标志。

const dropZoneRef = ref<HTMLElement | null>(null);
const imageDragOver = ref(false);
let unlistenImageDrop: (() => void) | null = null;

/** 图片区提示：桌面平台支持点击 / 粘贴 / 拖入；**Android 只有点击**（粘贴与拖放都不订阅，见下方 watch）。
 *  ⚠️ 按**平台**（`isAndroid`）判，不是按 `app.isMobile` —— 后者是 `max-width:767px` 的视口宽度，
 *  窄桌面窗口也会命中，会误把桌面当移动端、把粘贴/拖放一起关掉（用户 2026-09-20「桌面端也不能粘贴文件」）。 */
const imageHintText = computed(() =>
  imageDragOver.value
    ? t("todo.imageDropHere")
    : isAndroid
      ? t("todo.imageHintMobile")
      : t("todo.imageHint"),
);

/** 拖拽位置是否落在投放区上（Tauri 给的是物理像素，除以 DPR 才能跟 DOM 坐标比）。 */
function hitDropZone(pos: { x: number; y: number }): boolean {
  const el = dropZoneRef.value;
  if (!el) return false;
  const r = el.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const x = pos.x / dpr;
  const y = pos.y / dpr;
  return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
}

/** 图片扩展名（与文件选择器的 filters 一致）。 */
const IMAGE_PATH_RE = /\.(png|jpe?g|gif|webp|bmp)$/i;

/**
 * 粘贴：把剪贴板里的**图片**加进图片区。**文本粘贴不受影响**（没抓到图片就不 preventDefault）。
 *
 * 两条来源都要覆盖（与 `MessageComposer.onPaste` 同一套口径）：
 *   ① 位图（截图）：先 `clipboardData.files`，拿不到再退 `items.getAsFile()`；
 *   ② 资源管理器里复制的**图片文件**：Windows 走 CF_HDROP，`files` 里是个 **type 为空**的
 *      占位 File —— 只按 `image/` 过滤会一个都不剩，必须靠 `read_clipboard_file_paths`
 *      拿真实路径（用户 2026-09-20「桌面端也不能粘贴文件」）。
 * ⚠️ 位图 File 必须在任何 await 之前**同步**抓下来（paste 返回后 clipboardData 会被清空）。
 * ⚠️ 能力按**平台**（`isAndroid`）判，不按 `app.isMobile`（见 `imageHintText` 的说明）。
 */
function onDocPaste(e: ClipboardEvent) {
  if (!draft.value || isAndroid) return;
  const cd = e.clipboardData;
  if (!cd) return;
  const files = Array.from(cd.files ?? []);
  let image: File | null = files.find((f) => f.type.startsWith("image/")) ?? null;
  if (!image) {
    for (const it of Array.from(cd.items ?? [])) {
      if (it.kind === "file" && it.type.startsWith("image/")) {
        image = it.getAsFile();
        if (image) break;
      }
    }
  }
  const hasFiles = Array.from(cd.types ?? []).includes("Files");

  if (image) {
    e.preventDefault();
    const bitmap = image;
    void (async () => {
      try {
        const buf = await bitmap.arrayBuffer();
        const path = await api.saveTodoImageBytes(new Uint8Array(buf));
        await addImageFromPaths([path]);
      } catch (err) {
        app.toastError(err, t("todo.imagePasteFail"));
      }
    })();
    return;
  }

  // 没有位图、但剪贴板里是「文件」（复制的图片文件）：走真实路径。
  if (hasFiles) {
    e.preventDefault();
    void (async () => {
      try {
        const paths = await invoke<string[]>("read_clipboard_file_paths");
        const imgs = paths.filter((p) => IMAGE_PATH_RE.test(p));
        if (imgs.length) await addImageFromPaths(imgs);
      } catch {
        /* 非 Windows / 拿不到路径：静默（文本粘贴已放行） */
      }
    })();
  }
}

async function subscribeImageDrop() {
  if (unlistenImageDrop) return;
  try {
    unlistenImageDrop = await getCurrentWebview().onDragDropEvent((event) => {
      if (!draft.value) return;
      const p = event.payload;
      if (p.type === "enter" || p.type === "over") {
        const hit = hitDropZone(p.position);
        imageDragOver.value = hit;
        // 让 ChatWindow 的聊天拖放让位（否则同一份文件会被当成聊天附件发出去）
        app.boardDropActive = hit;
        return;
      }
      if (p.type === "drop") {
        const hit = hitDropZone(p.position);
        app.boardDropActive = false;
        imageDragOver.value = false;
        if (hit && p.paths.length) void addImageFromPaths(p.paths);
        return;
      }
      app.boardDropActive = false; // leave
      imageDragOver.value = false;
    });
  } catch {
    /* 非 Tauri 环境（纯 vite dev）忽略 */
  }
}
function unsubscribeImageDrop() {
  unlistenImageDrop?.();
  unlistenImageDrop = null;
  app.boardDropActive = false;
  imageDragOver.value = false;
}

// 草稿弹窗开/关：接上 / 退订 粘贴与拖放（**Android 两者都不订阅**；按平台判，不按视口宽度）。
watch(
  () => !!draft.value,
  (open) => {
    if (isAndroid) return;
    if (open) {
      document.addEventListener("paste", onDocPaste);
      void subscribeImageDrop();
    } else {
      document.removeEventListener("paste", onDocPaste);
      unsubscribeImageDrop();
    }
  },
);
onBeforeUnmount(() => {
  document.removeEventListener("paste", onDocPaste);
  unsubscribeImageDrop();
});
function removeImage(idx: number) {
  const d = draft.value;
  if (!d) return;
  const [img] = d.images.splice(idx, 1);
  // 只有"本次新加"的图片有本地路径；删掉已有图片时这里没有对应键，delete 是空操作。
  if (img) delete d.imagePaths[img.sha256];
}

async function saveDraft() {
  const d = draft.value;
  if (!d || !props.groupId) return;
  const title = d.title.trim();
  if (!title) {
    app.toast(t("todo.needTitle"), "error");
    return;
  }
  if (d.assignees.length === 0) {
    app.toast(t("todo.needAssignee"), "error");
    return;
  }
  saving.value = true;
  try {
    let todoId: string;
    if (d.todoId) {
      const cur = todos.value.find((x) => x.todoId === d.todoId);
      await chat.updateTodo(
        props.groupId,
        cur ?? { todoId: d.todoId, title, assignees: d.assignees, status: d.status, description: d.description, images: d.images },
        { title, assignees: d.assignees, status: d.status, description: d.description, images: d.images },
      );
      todoId = d.todoId;
      app.toast(t("todo.updateDone"), "success");
    } else {
      const rec = await chat.createTodo(props.groupId, title, d.assignees, d.description, d.images);
      const parsed = parseTodo(rec);
      todoId = parsed?.todoId ?? "";
      app.toast(t("todo.createDone"), "success");
      // 新建的是**活动**任务：若当时停在「已归档」，切回「全部」才看得到刚建的那条。
      if (filter.value === "archived") filter.value = "all";
    }
    // 投递新增图片字节（scope="todo"：不进时间线、不弹气泡）。
    for (const path of Object.values(d.imagePaths)) {
      try {
        await api.sendTodoImage(props.groupId, todoId, path);
      } catch (e) {
        app.toastError(e, t("todo.imageSendFail"));
      }
    }
    draft.value = null;
  } catch (e) {
    app.toastError(e, t(d.todoId ? "todo.updateFail" : "todo.createFail"));
  } finally {
    saving.value = false;
  }
}

/** 改状态（详情里的状态菜单、快捷完成、恢复都走它）。
 *
 * 成功后**关掉详情**：状态一变，任务就换分组了 —— 详情负责"看"，动作是"改 + 回列表看结果"。
 * 归档/恢复/完成的口径由此保持一致（用户 2026-09-17：「改『完成』也关闭弹窗保持一致」）。
 */
async function setStatus(x: TodoItem, status: string) {
  if (!props.groupId) return;
  try {
    await chat.updateTodo(props.groupId, x, { status });
    closeDetail();
  } catch (e) {
    app.toastError(e, t("todo.updateFail"));
  }
}
/** 标记完成：只改状态（后端在**首次**完成时记权威 `doneAt`）。完成**不再**自动归档。 */
async function completeTodo(x: TodoItem) {
  await setStatus(x, "done");
}
/** 完成之后**手动**归档（用户 2026-09-17）：显式 `archived=true`。
 *  `archived` 不是 status 字段，所以单独一条（成功后同样关掉详情）。 */
async function archiveTodo(x: TodoItem) {
  if (!props.groupId) return;
  try {
    await chat.updateTodo(props.groupId, x, { archived: true });
    closeDetail();
  } catch (e) {
    app.toastError(e, t("todo.updateFail"));
  }
}
/** 从归档恢复：状态回到待办，后端据此清掉 archived/done_at（= 重新打开）。 */
async function restoreTodo(x: TodoItem) {
  await setStatus(x, "todo");
}

// ---------------- 删除（破坏性且广播给全群 ⇒ 二次确认） ----------------
const pendingDelete = ref<TodoItem | null>(null);

async function confirmDelete() {
  const x = pendingDelete.value;
  pendingDelete.value = null;
  if (!x || !props.groupId) return;
  try {
    await chat.updateTodo(props.groupId, x, { deleted: true });
    app.toast(t("todo.removeDone"), "success");
    if (detailId.value === x.todoId) closeDetail();
  } catch (e) {
    app.toastError(e, t("todo.removeFail"));
  }
}

// 关闭面板时丢掉未保存的草稿/确认/详情，并把筛选复位：下次打开是一张干净的面板
// （独立窗口里 `open` 恒为 undefined ⇒ 该 watch 永不触发，窗口内的筛选会被保留，符合预期。）
watch(
  () => props.open,
  (v) => {
    if (!v) {
      draft.value = null;
      pendingDelete.value = null;
      detailId.value = null;
      rowMenuId.value = null;
      filter.value = "all";
    }
  },
);
</script>

<template>
  <div class="flex flex-col" :class="standalone ? 'h-full min-h-0' : ''">
    <!-- 头部：统计 + 新建 -->
    <div class="flex items-center justify-between gap-2 pb-2">
      <div class="min-w-0 flex-1 truncate text-xs text-[var(--gosslan-text-2)]" :title="t('todo.title')">
        <template v-if="todos.length">{{ t("todo.doneCount", { n: doneCount, total: todos.length }) }}</template>
        <template v-else>{{ t("todo.title") }}</template>
      </div>
      <button
        v-if="!draft"
        class="tap-safe flex shrink-0 items-center gap-1 rounded-[var(--gosslan-radius-md)] px-2.5 py-1.5 text-[13px] text-[var(--gosslan-accent-ink)] transition hover:bg-[var(--gosslan-hover)]"
        @click="startCreate"
      >
        <Plus class="h-4 w-4" />
        {{ t("todo.create") }}
      </button>
    </div>

    <!-- 筛选：独立胶囊 chips（与收藏过滤 / 日志时间窗口同款） -->
    <div v-if="todos.length" class="flex shrink-0 flex-wrap items-center gap-1">
      <button
        v-for="f in FILTERS"
        :key="f.key"
        type="button"
        class="tap-safe rounded-full px-2.5 py-1 text-xs transition"
        :class="filter === f.key
          ? 'border border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] font-medium text-[var(--gosslan-accent-ink)]'
          : 'border border-transparent text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
        :aria-pressed="filter === f.key"
        @click="filter = f.key"
      >
        {{ t(f.labelKey) }}
        <span
          v-if="counts[f.key] !== undefined"
          class="ml-1 rounded-full px-1 text-[11px] leading-4 tabular-nums opacity-70"
        >{{ counts[f.key] }}</span>
      </button>
    </div>

    <!-- 内容区：弹窗形态由 BaseModal 自己滚；窗口形态撑满剩余空间 -->
    <div class="overflow-y-auto pr-0.5 pt-2" :class="standalone ? 'min-h-0 flex-1' : ''">
      <!-- 空态（按当前筛选分流） -->
      <div v-if="shownCount === 0 && !draft" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ filter === "archived" ? t("todo.archivedEmpty") : filter === "all" ? t("todo.empty") : t("todo.filterEmpty") }}
      </div>

      <!-- 列表卡片：给条目一个**独立的面** —— 此前行是透明的、直接坐在窗口底色上，看起来
           "背景和任务条目混在一起"（用户 2026-09-17）。配方与设置页的分组卡片一致
           （圆角 + 描边 + panel 底），条目就有明确承载面了。 -->
      <div
        v-else
        class="overflow-hidden rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)]"
      >
      <!-- 已归档：扁平列表 + 说明；恢复在详情里 -->
      <template v-if="filter === 'archived'">
        <p class="border-b border-[var(--gosslan-divider)] px-3 py-2 text-[11px] leading-relaxed text-[var(--gosslan-text-2)]">
          {{ t("todo.archivedHint") }}
        </p>
        <div
          v-for="x in archivedTodos"
          :key="x.todoId"
          class="flex items-center gap-2.5 border-b border-[var(--gosslan-divider)] px-3 py-2.5 transition last:border-b-0 hover:bg-[var(--gosslan-hover)]"
        >
          <span class="w-5 shrink-0 text-center text-[11px] tabular-nums text-[var(--gosslan-text-2)]">
            {{ ordinals.get(x.todoId) }}
          </span>
          <button type="button" class="min-w-0 flex-1 text-left" @click="openDetail(x)">
            <div class="truncate text-[13px] font-medium text-[var(--gosslan-text-2)] line-through" :title="x.title">
              {{ x.title }}
            </div>
            <div class="mt-0.5 flex flex-wrap items-center gap-x-1 text-[11px] text-[var(--gosslan-text-2)]">
              <span v-if="x.creator">{{ t("todo.creator", { name: memberProfile(x.creator).name }) }}</span>
              <span v-if="assigneeNames(x)" :title="assigneeNames(x)">
                <span class="mx-1">·</span>{{ t("todo.assigneesInline", { names: assigneeNames(x) }) }}
              </span>
            </div>
          </button>
          <span class="shrink-0 inline-flex h-5 items-center justify-center rounded-full px-2 text-[11px] font-medium leading-none" :class="TODO_STATUS_PILL[x.status]">
            {{ statusText(x.status) }}
          </span>
        </div>
      </template>

      <!-- 活动任务：按状态分组（空组不占位） -->
      <template v-else>
        <div v-for="g in grouped" :key="g.status">
          <!-- 分组小标题：带底色的条（分组之间一眼分得开，也给白底卡片一个"分节"观感） -->
          <div class="flex items-center gap-1.5 bg-[var(--gosslan-bg)] px-3 py-1.5">
            <component
              :is="STATUS_ICON[g.status]"
              class="h-4 w-4 shrink-0"
              :class="TODO_STATUS_CLASS[g.status]"
              :stroke-width="1.9"
            />
            <span class="text-[12px] font-medium text-[var(--gosslan-text)]">{{ statusText(g.status) }}</span>
            <span class="text-[11px] text-[var(--gosslan-text-2)]">{{ g.items.length }}</span>
          </div>

          <!-- 任务行：序号 + 标题/元信息（点开详情）+ 状态 + 快捷完成 -->
          <div
            v-for="x in g.items"
            :key="x.todoId"
            class="flex items-center gap-2.5 border-b border-[var(--gosslan-divider)] px-3 py-2.5 transition last:border-b-0 hover:bg-[var(--gosslan-hover)]"
          >
            <span class="w-5 shrink-0 text-center text-[11px] tabular-nums text-[var(--gosslan-text-2)]">
              {{ ordinals.get(x.todoId) }}
            </span>
            <button type="button" class="min-w-0 flex-1 text-left" @click="openDetail(x)">
              <div
                class="truncate text-[13px] font-medium"
                :class="x.status === 'done' ? 'text-[var(--gosslan-text-2)] line-through' : 'text-[var(--gosslan-text)]'"
                :title="x.title"
              >
                {{ x.title }}
              </div>
              <div class="mt-0.5 flex flex-wrap items-center gap-x-1 text-[11px] text-[var(--gosslan-text-2)]">
                <span v-if="x.creator">{{ t("todo.creator", { name: memberProfile(x.creator).name }) }}</span>
                <span v-if="assigneeNames(x)" :title="assigneeNames(x)">
                  <span class="mx-1">·</span>{{ t("todo.assigneesInline", { names: assigneeNames(x) }) }}
                </span>
              </div>
            </button>
            <!-- 状态：有权限时是胶囊按钮（点开小菜单直接在列表上切状态，用户 2026-09-17）；
                 与详情共用同一份数据源（`foldTodos`），两处状态天然同步。 -->
            <div class="relative shrink-0">
              <button
                v-if="canChangeStatus(x)"
                type="button"
                class="tap-safe inline-flex h-5 shrink-0 items-center justify-center gap-1 rounded-full px-2 text-[11px] font-medium leading-none transition hover:opacity-80"
                :class="TODO_STATUS_PILL[x.status]"
                :title="t('todo.statusLabel')"
                :aria-label="t('todo.statusLabel')"
                :aria-expanded="rowMenuId === x.todoId"
                @click.stop="toggleRowMenu(x.todoId)"
              >
                {{ statusText(x.status) }}
                <ChevronDown class="h-3 w-3" />
              </button>
              <span
                v-else
                class="inline-flex h-5 shrink-0 items-center justify-center rounded-full px-2 text-[11px] font-medium leading-none"
                :class="TODO_STATUS_PILL[x.status]"
              >
                {{ statusText(x.status) }}
              </span>

              <template v-if="canChangeStatus(x) && rowMenuId === x.todoId">
                <button
                  type="button"
                  class="fixed inset-0 z-40 cursor-default"
                  :aria-label="t('todo.closeMenu')"
                  @click.stop="closeRowMenu"
                />
                <div class="gosslan-menu frost absolute right-0 top-full z-50 mt-1" role="menu" aria-orientation="vertical">
                  <button
                    v-for="s in TODO_STATUSES"
                    :key="s"
                    type="button"
                    role="menuitem"
                    class="gosslan-menu-item"
                    :class="s === x.status ? 'font-medium text-[var(--gosslan-accent-ink)]' : ''"
                    :disabled="s === x.status"
                    :aria-current="s === x.status ? 'true' : undefined"
                    @click.stop="setStatus(x, s); closeRowMenu()"
                  >
                    {{ statusText(s) }}
                  </button>
                </div>
              </template>
            </div>
            <!-- 快捷「完成」：待办清单最高频的动作，留在行内（飞书/钉钉同做法）。
                 完成后任务仍留在列表（只换分组），不会"点错就丢"。 -->
            <button
              v-if="canChangeStatus(x) && x.status !== 'done'"
              type="button"
              class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-success-soft)] hover:text-[var(--gosslan-success-ink)]"
              :title="t('todo.complete')"
              :aria-label="t('todo.complete')"
              @click.stop="completeTodo(x)"
            >
              <Check class="h-4 w-4" />
            </button>
          </div>
        </div>
      </template>
      </div>
    </div>
  </div>

  <!-- 任务详情：描述/图片 + 状态与编辑等全部操作 -->
  <TodoDetailDialog
    :open="!!detailItem"
    :item="detailItem"
    :archived="detailArchived"
    :can-change-status="detailItem ? canChangeStatus(detailItem) : false"
    :can-edit-structure="detailItem ? canEditStructure(detailItem) : false"
    :can-edit-assignees="detailItem ? canEditAssigneesOf(detailItem) : false"
    :name-of="(id: string) => memberProfile(id).name"
    @close="closeDetail"
    @status="(s: TodoStatus) => detailItem && setStatus(detailItem, s)"
    @complete="detailItem && completeTodo(detailItem)"
    @archive="detailItem && archiveTodo(detailItem)"
    @restore="detailItem && restoreTodo(detailItem)"
    @edit="detailItem && editFromDetail(detailItem)"
    @remove="detailItem && (pendingDelete = detailItem)"
  />

  <!-- 新建 / 编辑：表单弹窗（与「任务详情」同一套分区与字段样式）。
       放进弹窗而不是内联：① 列表不再被表单推来推去；② 表单打开时看板被遮罩挡住，
       不会再出现"新建任务时还能操作列表筛选"的错乱（用户 2026-09-17）。 -->
  <BaseModal
    :open="!!draft"
    :title="draft?.todoId ? t('todo.edit') : t('todo.create')"
    width="max-w-lg"
    @close="draft = null"
  >
    <div v-if="draft" class="space-y-4">
      <!-- 标题 -->
      <div>
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("todo.titleLabel") }}</div>
        <input
          v-model="draft.title"
          maxlength="200"
          :disabled="!draftCanEditTitle"
          class="w-full rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none transition focus:border-transparent disabled:opacity-60"
          :placeholder="t('todo.titlePlaceholder')"
          @keydown.enter.prevent="saveDraft"
        />
      </div>

      <!-- 状态：仅**编辑**时出现（新建恒为「待办」，`send_group_todo` 不收状态）。
           表单里的改动要到「保存」才写库 ⇒ 这里的分段控件没有"误触改状态"的问题。 -->
      <div v-if="draft.todoId">
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("todo.statusLabel") }}</div>
        <div class="flex gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] p-0.5">
          <button
            v-for="s in TODO_STATUSES"
            :key="s"
            type="button"
            class="tap-safe flex-1 whitespace-nowrap rounded-[var(--gosslan-radius-sm)] py-1.5 text-[12px] transition"
            :class="
              draft.status === s
                ? 'bg-[var(--gosslan-panel)] font-medium text-[var(--gosslan-text)] shadow-sm'
                : 'text-[var(--gosslan-text-2)] hover:text-[var(--gosslan-text)]'
            "
            :aria-pressed="draft.status === s"
            @click="draft.status = s"
          >
            <span :class="draft.status === s ? TODO_STATUS_CLASS[s] : ''">{{ statusText(s) }}</span>
          </button>
        </div>
      </div>

      <!-- 描述 -->
      <div>
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("todo.descriptionLabel") }}</div>
        <textarea
          v-model="draft.description"
          maxlength="4000"
          rows="3"
          class="w-full resize-y rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none transition focus:border-transparent"
          :placeholder="t('todo.descriptionPlaceholder')"
        ></textarea>
      </div>

      <!-- 图片：桌面端点击 / 粘贴 / 拖入（用户 2026-09-17），虚线框是投放区；
           移动端**只支持点击**，所以不画虚线（虚线在移动端没有可拖的东西，反而误导）。
           有图时缩略图排在区内；拖入命中时框体高亮。 -->
      <div>
        <div class="mb-1.5 flex items-center justify-between">
          <span class="text-xs text-[var(--gosslan-text-2)]">{{ t("todo.imagesLabel") }}</span>
          <button
            type="button"
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-sm)] px-1.5 py-0.5 text-[11px] text-[var(--gosslan-accent-ink)] transition hover:bg-[var(--gosslan-hover)]"
            @click="addImage"
          >
            <ImagePlus class="h-3.5 w-3.5" />
            {{ t("todo.addImage") }}
          </button>
        </div>
        <!-- 高亮（imageDragOver）与 drop 都由 webview 级的 `onDragDropEvent` 驱动
             （见脚本里的 subscribeImageDrop）：Tauri 开着 dragDropEnabled 时 HTML5 拖放事件
             会被拦掉，所以这里不写 @drop 之类的 HTML5 拖放属性。 -->
        <div
          ref="dropZoneRef"
          class="rounded-[var(--gosslan-radius-md)] border transition"
          :class="isAndroid
            ? 'border-[var(--gosslan-border)]'
            : imageDragOver
              ? 'border-dashed border-[var(--gosslan-primary)] bg-[var(--gosslan-hover)]'
              : 'border-dashed border-[var(--gosslan-border)]'"
        >
          <button
            v-if="draft.images.length === 0"
            type="button"
            class="w-full cursor-pointer px-3 py-6 text-center text-[11px] leading-relaxed text-[var(--gosslan-text-2)] transition hover:text-[var(--gosslan-text)]"
            @click="addImage"
          >
            {{ imageHintText }}
          </button>
          <div v-else class="flex flex-wrap gap-1.5 p-2">
            <div v-for="(img, i) in draft.images" :key="img.sha256" class="group relative">
              <TodoImageThumb :image="img" />
              <button
                type="button"
                class="tap-safe absolute -right-1.5 -top-1.5 flex h-4 w-4 items-center justify-center rounded-full bg-[var(--gosslan-danger)] text-white opacity-90 transition hover:opacity-100"
                :aria-label="t('common.delete')"
                @click="removeImage(i)"
              >
                <X class="h-3 w-3" />
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- 指派（**权限依据**，至少一人） -->
      <div>
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("todo.pickMembers") }}</div>
        <div class="flex max-h-40 flex-wrap gap-1.5 overflow-y-auto">
          <button
            v-for="id in group?.members ?? []"
            :key="id"
            type="button"
            class="flex items-center gap-1.5 rounded-[var(--gosslan-radius-md)] border px-2 py-1 text-[12px] transition"
            :class="
              draft.assignees.includes(id)
                ? 'border-[var(--gosslan-primary)] text-[var(--gosslan-accent-ink)]'
                : 'border-[var(--gosslan-border)] text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'
            "
            :aria-pressed="draft.assignees.includes(id)"
            @click="toggleAssignee(id)"
          >
            <Check v-if="draft.assignees.includes(id)" class="h-3.5 w-3.5" />
            {{ memberProfile(id).name }}
          </button>
        </div>
      </div>

      <!-- 操作 -->
      <div class="flex items-center justify-end gap-2 border-t border-[var(--gosslan-divider)] pt-3">
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          @click="draft = null"
        >
          {{ t("common.cancel") }}
        </button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-3 py-1.5 text-[13px] text-white transition hover:opacity-90 disabled:opacity-50"
          :disabled="saving"
          @click="saveDraft"
        >
          {{ t("todo.save") }}
        </button>
      </div>
    </div>
  </BaseModal>

  <!-- 删除二次确认：会广播给全群、不可撤销。渲染在详情之后 ⇒ 叠在它上面。 -->
  <BaseModal
    :open="!!pendingDelete"
    :title="t('todo.remove')"
    @close="pendingDelete = null"
  >
    <p v-if="pendingDelete" class="text-sm leading-relaxed text-[var(--gosslan-text)]">
      {{ t("todo.removeConfirm", { title: pendingDelete.title }) }}
    </p>
    <div class="mt-5 flex justify-end gap-2">
      <button
        class="tap-safe rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
        @click="pendingDelete = null"
      >
        {{ t("common.cancel") }}
      </button>
      <button
        class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-3 py-1.5 text-[13px] text-white transition hover:opacity-90"
        @click="confirmDelete"
      >
        {{ t("common.delete") }}
      </button>
    </div>
  </BaseModal>
</template>
