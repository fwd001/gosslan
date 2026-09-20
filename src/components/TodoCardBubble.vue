<script setup lang="ts">
import type { CSSProperties } from "vue";
/**
 * 时间线里的群任务卡片气泡（用户 2026-09-17：任务消息不该在聊天里显示原始 JSON）。
 *
 * `todo` 是 Card kind：进时间线、计未读、弹通知，但不属于"聊天历史"。这里把它的 JSON 载荷
 * 渲染成一张可读的卡片（标题 / 状态 / 描述 / 图片 / 指派人 + 打开面板入口）。
 *
 * 载荷是**创建那一刻的快照**（改状态走的是 `todo_update`，那一条是静默事件、不进时间线），
 * 所以卡片显示的是创建时的样子；最新状态请看任务面板（`foldTodos` 折叠出的权威结果）。
 */
import { computed } from "vue";
import { useMemberProfile } from "@/composables/useMemberProfile";
import TodoImageThumb from "@/components/TodoImageThumb.vue";
import { TODO_STATUS_LABEL_KEY, TODO_STATUS_PILL, parseTodo, type TodoStatus } from "@/utils/todos";
import { t } from "@/i18n";
import { ChevronRight, ListTodo } from "lucide-vue-next";
import type { MessageRecord } from "@/types";

const props = defineProps<{
  message: MessageRecord;
  /** 自己发的卡片：尖角朝右指向右侧头像。 */
  mine?: boolean;
  /** 从父层 MessageItem 透传的卡片样式（固定底色，不跟随 mine/other）。 */
  cardStyle?: CSSProperties;
}>();
const emit = defineEmits<{ (e: "open"): void }>();

const { cardStyle } = props;

const { memberProfile } = useMemberProfile();

const todo = computed(() => parseTodo(props.message));
const status = computed<TodoStatus>(() => todo.value?.status ?? "todo");
const assignees = computed(() => todo.value?.assignees ?? []);

function statusText(s: TodoStatus): string {
  return t(TODO_STATUS_LABEL_KEY[s]);
}
</script>

<template>
  <div class="relative w-64 max-w-full" :style="cardStyle">
    <div class="overflow-hidden rounded-[var(--gosslan-bubble-radius)]">
    <div class="flex items-center gap-2 px-3 pt-2.5">
      <ListTodo class="h-4 w-4 shrink-0 text-[var(--gosslan-primary)]" />
      <span
        class="min-w-0 flex-1 truncate text-[13px] font-medium text-[var(--gosslan-card-ink)]"
        :title="todo?.title || t('todo.title')"
      >
        {{ todo?.title || t("todo.title") }}
      </span>
      <!-- 状态胶囊走共享映射（`TODO_STATUS_PILL`）；字号 11px 是设计规范的最小可用档。 -->
      <span class="shrink-0 inline-flex h-[18px] items-center justify-center rounded-full px-1.5 text-[11px] font-medium leading-none" :class="TODO_STATUS_PILL[status]">
        {{ statusText(status) }}
      </span>
    </div>

    <p
      v-if="todo?.description"
      class="mt-1 max-h-[4.2em] overflow-hidden whitespace-pre-wrap break-words px-3 text-[12px] leading-relaxed text-[var(--gosslan-card-ink)] opacity-70"
    >
      {{ todo.description }}
    </p>

    <div v-if="todo?.images?.length" class="mt-1.5 flex flex-wrap gap-1 px-3">
      <TodoImageThumb v-for="img in todo.images" :key="img.sha256" :image="img" />
    </div>

    <div class="mt-1.5 flex min-w-0 flex-wrap items-center gap-1 px-3">
      <span
        v-for="a in assignees"
        :key="a"
        class="truncate rounded-full bg-[var(--gosslan-hover)] px-1.5 py-0.5 text-[11px] text-[var(--gosslan-card-ink)] opacity-70"
        :title="memberProfile(a).name"
      >
        {{ memberProfile(a).name }}
      </span>
      <span v-if="assignees.length === 0" class="text-[11px] text-[var(--gosslan-card-ink)] opacity-70">
        {{ t("todo.assigneesEmpty") }}
      </span>
    </div>

    <button
      type="button"
      class="mt-2 flex w-full items-center justify-center gap-1 border-t border-[var(--gosslan-card-line)] py-1.5 text-[12px] text-[var(--gosslan-primary)] transition hover:bg-[var(--gosslan-hover)]"
      @click="emit('open')"
    >
      {{ t("todo.openPanel") }}
      <ChevronRight class="h-3 w-3" />
    </button>
    <!-- 内部包裹层在此闭合（带 overflow-hidden 做圆角裁切）；尖角必须放在外面，
         root 是 relative 但不 overflow-hidden，否则会被裁掉（尖角在卡片外 -5px 处）。 -->
    </div>
    <!-- 指向发送者头像的小尖角（与文本/代码气泡同款 .bubble-tail）：卡片无描边，
         直接用标准 -5px 偏移即可贴合，尖角色取 --bubble-bg（= card 底色）。 -->
    <span aria-hidden="true" class="bubble-tail" :class="mine ? 'tail-mine' : 'tail-other'"></span>
  </div>
</template>
