<script setup lang="ts">
import type { CSSProperties } from "vue";
/**
 * 时间线里的群任务卡片气泡（用户 2026-09-17：任务消息不该在聊天里显示原始 JSON）。
 *
 * `todo` 是 Card kind：进时间线、计未读、弹通知，但不属于"聊天历史"。这里把它的 JSON 载荷
 * 渲染成一张可读的卡片（标题 / 状态 / 描述 / 图片 / 指派人 + 打开面板入口）。
 *
 * 载荷是**创建那一刻的快照**（改状态走的是 `todo_update`，那一条是静默事件、不进时间线），
 * 所以状态**必须**查父层传来的实时表 `liveStatus`（用户 #23：不查的话卡片永远挂着「待办」，
 * 而任务其实早干完了）；表里查不到（任务已删 / 那条消息没被折叠进来）才退回快照。
 * 标题/描述/指派人仍是创建时的样子 —— 那些字段没有实时源，看板才是权威列表。
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
  /**
   * 任务 `todo_id` → **当前**状态（会话层折叠一次传下来，见 `ChatWindow.todoLiveStatus`）。
   * 缺省空表 = 单测/别的宿主没传 ⇒ 退回卡片自己的快照。
   */
  liveStatus?: Map<string, TodoStatus>;
}>();
/** 带上 `todo_id`：点卡片要直达**这一条**任务的详情（用户 #23），不是只把看板打开。 */
const emit = defineEmits<{ (e: "open", todoId: string | undefined): void }>();

const { cardStyle } = props;

const { memberProfile } = useMemberProfile();

const todo = computed(() => parseTodo(props.message));
const todoId = computed(() => todo.value?.todoId);
/**
 * 卡片显示的状态 = **当前**状态（折叠结果），查不到才退回创建时的快照。
 *
 * 为什么要专门查这一张表（用户 #23）：载荷是创建那一刻的快照，而之后的改动都走
 * `todo_update`（静默事件、不进时间线）⇒ 卡片自己的载荷**永远停在创建时**，
 * 而新建任务恒为「待办」。表现就是"群里任务早干完了，聊天里那条还挂着待办"，
 * 也正是用户去点卡片的原因 —— 查不到（任务被删 / 消息页没折叠到）时宁可显示快照，
 * 也不要空着或骗人说已同步。
 */
const status = computed<TodoStatus>(
  () => (todoId.value ? props.liveStatus?.get(todoId.value) : undefined) ?? todo.value?.status ?? "todo",
);
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
      @click="emit('open', todoId)"
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
