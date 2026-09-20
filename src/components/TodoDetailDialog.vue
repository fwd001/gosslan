<script setup lang="ts">
/**
 * 任务详情（用户 2026-09-17：列表改成「微信式极简」—— 行内只留序号/标题/状态/元信息，
 * **描述与图片、以及所有状态与编辑操作都收进这里**，点行打开）。
 *
 * 纯展示 + 意图上抛：真正的改状态/归档/编辑/删除都在 `GroupTasksBoard` 里
 * （那里已经握有折叠数据、权限判据与 API 调用），这里只渲染与 `emit`。
 */
import { computed, ref, watch } from "vue";
import BaseModal from "@/components/BaseModal.vue";
import TodoImageThumb from "@/components/TodoImageThumb.vue";
import { TODO_STATUSES, TODO_STATUS_LABEL_KEY, TODO_STATUS_PILL, type TodoItem, type TodoStatus } from "@/utils/todos";
import { fmtConversationTime } from "@/utils/time";
import { Archive, Check, ChevronDown, Pencil, RotateCcw, Trash2 } from "lucide-vue-next";
import { t } from "@/i18n";

const props = defineProps<{
  open: boolean;
  item: TodoItem | null;
  /** 该任务当前是否实际处于归档态（显式归档 或 完成满 7 天）。 */
  archived: boolean;
  canChangeStatus: boolean;
  canEditStructure: boolean;
  /** 能否改指派人（创建者/群主/当前被指派人）—— 被指派人也能通过编辑改指派人。 */
  canEditAssignees: boolean;
  /** 名字解析（id → 昵称）；由看板注入 `memberProfile`，避免这里再依赖成员数据源。 */
  nameOf: (id: string) => string;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "status", status: TodoStatus): void;
  (e: "complete"): void;
  (e: "archive"): void;
  (e: "restore"): void;
  (e: "edit"): void;
  (e: "remove"): void;
}>();

const assignees = computed(() => (props.item?.assignees ?? []).map((id) => props.nameOf(id)).join("、"));
const creator = computed(() => (props.item?.creator ? props.nameOf(props.item.creator) : ""));

function statusText(s: TodoStatus): string {
  return t(TODO_STATUS_LABEL_KEY[s]);
}

/**
 * 状态改成**显式两步**（用户 2026-09-17：「一不小心就把状态改了」）。
 *
 * 此前是一排 4 个分段按钮，一点即写库 —— 而且它长得跟看板顶部的**筛选**分段控件一模一样，
 * 用户会以为是"切视图"，于是顺手点、状态就变了。现在：当前状态是一个胶囊（一眼看清现状），
 * 改动要点开菜单再选（当前项直接置灰不可点），误触代价从"改错状态"变成"多看一眼"。
 */
const menuOpen = ref(false);
function choose(s: TodoStatus) {
  menuOpen.value = false;
  if (s === props.item?.status) return; // 当前项本就不可点，双保险
  emit("status", s);
}
// 关掉详情时收起菜单（下次打开是干净的）
watch(
  () => props.open,
  (v) => {
    if (!v) menuOpen.value = false;
  },
);
</script>

<template>
  <BaseModal :open="open" :title="item?.title ?? t('todo.title')" width="max-w-lg" @close="emit('close')">
    <div v-if="item" class="space-y-4">
      <!-- 状态：**显式两步**（用户 2026-09-17：「一不小心就把状态改了」）。
           此前是一排 4 个分段按钮、一点即写库，而且与看板顶部的**筛选**分段控件长得一样，
           用户当成"切视图"就顺手点了。现在当前状态是一个胶囊（看清现状），改动要点开菜单再选，
           当前项置灰不可点 —— 误触代价从"改错状态"变成"多看一眼"。 -->
      <div class="flex items-center justify-between gap-2">
        <span class="text-xs text-[var(--gosslan-text-2)]">{{ t("todo.statusLabel") }}</span>
        <div class="relative">
          <button
            v-if="canChangeStatus"
            type="button"
            class="tap-safe inline-flex h-6 items-center justify-center gap-1 rounded-full px-2.5 text-[12px] font-medium leading-none transition hover:opacity-80"
            :class="TODO_STATUS_PILL[item.status]"
            :title="t('todo.statusChange')"
            :aria-label="t('todo.statusChange')"
            :aria-expanded="menuOpen"
            aria-haspopup="menu"
            @click="menuOpen = !menuOpen"
          >
            {{ statusText(item.status) }}
            <ChevronDown class="h-3 w-3" />
          </button>
          <span
            v-else
            class="inline-flex h-6 items-center justify-center rounded-full px-2.5 text-[12px] font-medium leading-none"
            :class="TODO_STATUS_PILL[item.status]"
          >
            {{ statusText(item.status) }}
          </span>

          <!-- 状态菜单（应用统一的 .gosslan-menu 观感；当前项置灰不可点） -->
          <template v-if="canChangeStatus && menuOpen">
            <button
              type="button"
              class="fixed inset-0 z-40 cursor-default"
              :aria-label="t('todo.closeMenu')"
              @click="menuOpen = false"
            />
            <div class="gosslan-menu frost absolute right-0 top-full z-50 mt-1" role="menu" aria-orientation="vertical">
              <button
                v-for="s in TODO_STATUSES"
                :key="s"
                type="button"
                role="menuitem"
                class="gosslan-menu-item"
                :class="s === item.status ? 'font-medium text-[var(--gosslan-primary)]' : ''"
                :disabled="s === item.status"
                :aria-current="s === item.status ? 'true' : undefined"
                @click="choose(s)"
              >
                {{ statusText(s) }}
              </button>
            </div>
          </template>
        </div>
      </div>

      <!-- 元信息：创建时间 / 创建人 / 指派 / 完成时间（用户 2026-09-17：时间要回显） -->
      <div class="space-y-1 text-[12px] text-[var(--gosslan-text-2)]">
        <div v-if="item.createdAt">{{ t("todo.createdAt", { time: fmtConversationTime(item.createdAt) }) }}</div>
        <div v-if="creator">{{ t("todo.creator", { name: creator }) }}</div>
        <div v-if="assignees" :title="assignees">{{ t("todo.assigneesInline", { names: assignees }) }}</div>
        <div v-if="item.doneAt">{{ t("todo.doneAt", { time: fmtConversationTime(item.doneAt) }) }}</div>
      </div>

      <!-- 描述（全文，不截断） -->
      <div>
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("todo.description") }}</div>
        <p
          v-if="item.description"
          class="whitespace-pre-wrap break-words text-[12px] leading-relaxed text-[var(--gosslan-text)]"
        >
          {{ item.description }}
        </p>
        <p v-else class="text-[12px] text-[var(--gosslan-text-2)]">{{ t("todo.noDescription") }}</p>
      </div>

      <!-- 图片 -->
      <div v-if="item.images.length">
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("todo.imagesLabel") }}</div>
        <div class="flex flex-wrap gap-1.5">
          <TodoImageThumb v-for="img in item.images" :key="img.sha256" :image="img" />
        </div>
      </div>

      <!-- 操作：完成（主）/ 归档 / 恢复 / 编辑 / 删除 -->
      <div class="flex flex-wrap items-center gap-2 border-t border-[var(--gosslan-divider)] pt-3">
        <button
          v-if="canChangeStatus && item.status !== 'done'"
          type="button"
          class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-3 py-1.5 text-[13px] text-white transition hover:bg-[var(--gosslan-primary-hover)]"
          @click="emit('complete')"
        >
          <Check class="mr-1 inline h-3.5 w-3.5" />{{ t("todo.complete") }}
        </button>
        <!-- 完成之后**手动**归档（不再自动归档） -->
        <button
          v-else-if="canChangeStatus && !archived"
          type="button"
          class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:text-[var(--gosslan-text)]"
          @click="emit('archive')"
        >
          <Archive class="h-3.5 w-3.5" />{{ t("todo.archive") }}
        </button>
        <button
          v-if="archived && canChangeStatus"
          type="button"
          class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-1.5 text-[13px] text-[var(--gosslan-text-2)] transition hover:text-[var(--gosslan-text)]"
          @click="emit('restore')"
        >
          <RotateCcw class="h-3.5 w-3.5" />{{ t("todo.restore") }}
        </button>

        <span class="flex-1"></span>

        <button
          v-if="canEditStructure || canEditAssignees"
          type="button"
          class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('todo.edit')"
          :aria-label="t('todo.edit')"
          @click="emit('edit')"
        >
          <Pencil class="h-4 w-4" />
        </button>
        <button
          v-if="canEditStructure"
          type="button"
          class="tap-safe flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-danger-soft)] hover:text-[var(--gosslan-danger-ink)]"
          :title="t('todo.remove')"
          :aria-label="t('todo.remove')"
          @click="emit('remove')"
        >
          <Trash2 class="h-4 w-4" />
        </button>
      </div>
    </div>
  </BaseModal>
</template>
