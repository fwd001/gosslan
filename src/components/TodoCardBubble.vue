<script setup lang="ts">
import type { CSSProperties } from "vue";
/**
 * 时间线里的群任务卡片气泡（用户 2026-09-17：任务消息不该在聊天里显示原始 JSON）。
 *
 * `todo` 是 Card kind：进时间线、计未读、弹通知，但不属于"聊天历史"。这里把它的 JSON 载荷
 * 渲染成一张可读的卡片（标题 / 状态 / 描述 / 图片 / 指派人 + 打开面板入口）。
 *
 * 载荷是**创建那一刻的快照**（改状态走的是 `todo_update`，那一条是静默事件、不进时间线），
 * 所以状态与**当前类型**必须查父层传来的实时表 `liveTodo`（用户 #23：不查的话卡片永远挂着「待办」，
 * 而任务其实早干完了）；表里查不到（任务已删 / 那条消息没被折叠进来）才退回快照。
 * 标题/描述/指派人仍是创建时的样子 —— 那些字段没有实时源，看板才是权威列表。
 */
import { computed } from "vue";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { useImagePreviewStore } from "@/stores/useImagePreview";
import MentionText from "@/components/message/MentionText.vue";
import TodoImageThumb from "@/components/TodoImageThumb.vue";
import {
  TODO_CATEGORY_LABEL_KEY,
  TODO_CATEGORY_TEXT_CLASS,
  TODO_PRIORITY_CLASS,
  TODO_PRIORITY_LABEL_KEY,
  TODO_STATUS_LABEL_KEY,
  TODO_STATUS_PILL,
  parseTodo,
  todoCode,
  type TodoLive,
  type TodoStatus,
} from "@/utils/todos";
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
   * 任务 `todo_id` → **当前**状态与当前编号码（会话层折叠一次传下来，见 `ChatWindow.todoLive`）。
   * 缺省空表 = 单测/别的宿主没传 ⇒ 退回卡片自己的创建快照。
   */
  liveTodo?: Map<string, TodoLive>;
  /**
   * @ 渲染的判定输入（由 MessageItem 透传，与聊天正文同一份）。
   * 缺省 = 单测/别的宿主没传 ⇒ 描述里的 @ 不高亮，但文案仍是原文（不会退化成「@你」）。
   */
  mentionNames?: string[];
  selfMention?: { name: string; label: string } | null;
}>();
/** 带上 `todo_id`：点卡片要直达**这一条**任务的详情（用户 #23），不是只把看板打开。 */
const emit = defineEmits<{ (e: "open", todoId: string | undefined): void }>();

const { cardStyle } = props;

const { memberProfile } = useMemberProfile();

const todo = computed(() => parseTodo(props.message));
const todoId = computed(() => todo.value?.todoId);
/**
 * 卡片里的图走**全局那一份**预览（§12「要做就做完整」）。
 *
 * 为什么这一格单独值得写一句：`TodoImageThumb` 的 `clickable` 默认是 **false**，所以"调用点漏给
 * 这一个 prop"不会编译报错、不会类型报错、单测也不红 —— 只是**图渲染出来了却点不动**。
 * 收藏那一处就是这么修的（`a077850`），而修完那一轮**同一个开关在聊天时间线的任务卡上仍然漏着**
 * ⇒ 现在由 `designGuards` 按形状数每个调用点，不靠人记得。
 * 来源标记用 `task-card:` 前缀，与任务详情弹窗那处的 `task:<todoId>` **刻意不同**：
 * 两处显示的是同一条任务的同一组图，但"按来源收预览"是精确匹配，共用一个 key 会让一边把另一边的收掉。
 */
const preview = useImagePreviewStore();
const cardImages = computed(() => todo.value?.images ?? []);
const cardGallery = computed(() =>
  cardImages.value.map((im) => ({ cid: im.sha256, name: im.name })),
);
const cardSource = computed(() => (todoId.value ? `task-card:${todoId.value}` : null));
function openCardImage(i: number) {
  preview.openGallery(cardGallery.value, i, cardSource.value);
}
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
  () => (todoId.value ? props.liveTodo?.get(todoId.value)?.status : undefined) ?? todo.value?.status ?? "todo",
);
/**
 * 卡片上那串编号（字母 + 数字）。口径整份在 `utils/todos.ts` 的 `todoCode`，这里只负责取用：
 * 字母取**当前**类型（折叠表里那一份），查不到才退回创建快照 ⇒「改成缺陷了还写着 R」不会发生；
 * 数字是创建那一刻那一个（`resolveTodoNumbers` 已保证各端对同一批定义算出同一套号）。
 */
const code = computed(() =>
  props.liveTodo?.get(todoId.value ?? "")?.code ?? todoCode(todo.value?.category, todo.value?.number),
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
      <!-- 群内固定编号（`R12` / `B2345`）：成员各自库里同一条任务是同一个号，口头引用才对得上。
           没号（旧版本对端建的载荷里没这个键）⇒ 不显示，也不按顺序补一个（补出来的号会随集合变）。
           数字超过 4 位时这一行只显**后四位**（用户 2026-09-30 规则 5），全码有三个出口：
           PC 悬停（`title`）、读屏（aria-label 永远给全码）、点卡片进详情（详情标题写全码）。
           缩短那一份只是视觉识别码 —— 落库、上线、引用的都是完整编号 + 内部 todoId（规则 7）。 -->
      <span
        v-if="code"
        class="shrink-0 font-mono text-[11px] text-[var(--gosslan-text-2)]"
        :aria-label="t('todo.codeAria', { code: code.full })"
        :title="code.shortened ? t('todo.codeShortTip', { full: code.full }) : undefined"
      >
        {{ code.label }}
      </span>
      <!-- 优先级（紧急 / 常规 / 不急）：只有「紧急」抢权重，其余压次级色，
           免得三档一起亮反而读不出重点。谁能改见 `canUpdateTodo`（发起人/关联人/群主）。 -->
      <span
        class="shrink-0 text-[11px]"
        :class="TODO_PRIORITY_CLASS[todo?.priority ?? 'normal']"
      >
        {{ t(TODO_PRIORITY_LABEL_KEY[todo?.priority ?? "normal"]) }}
      </span>
      <!-- 类型：只有「需求 / 缺陷」在这一行占一格（文字档，不画胶囊 —— 见
           `TODO_CATEGORY_TEXT_CLASS` 上那段与 messageHeight 成对的说明）。
           「任务」这一档不占位，旧数据读出来也是它 ⇒ 与不画完全同形。 -->
      <span
        v-if="todo && todo.category !== 'task'"
        class="shrink-0 text-[11px]"
        :class="TODO_CATEGORY_TEXT_CLASS[todo.category]"
      >
        {{ t(TODO_CATEGORY_LABEL_KEY[todo.category]) }}
      </span>
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

    <!-- 描述：卡片里只看得到前 3 **整行**（全文在详情弹窗）。
         ⚠️ 高度必须落在整行上：这段行距是 `leading-relaxed`(1.625)，原先写的 `max-h-[4.2em]`
         等于 2.58 行 ⇒ 第 3 行被削掉半截字，比"少显示一行"更像 bug。
         改行数必须同步 `utils/messageHeight.ts` 的 `TODO_CARD_DESC`（渲染与估算成对，
         否则虚拟列表按旧值排布 ⇒ 相邻消息互相遮挡，那是本仓另一条已记的契约）。 -->
    <p
      v-if="todo?.description"
      class="mt-1 max-h-[calc(3*1.625em)] overflow-hidden whitespace-pre-wrap break-words px-3 text-[12px] leading-relaxed text-[var(--gosslan-card-ink)] opacity-70"
    >
      <!-- 描述里的 @ 走与聊天正文**同一份**渲染件（§9）：段怎么切、@到算不算我、
           显示成什么，全部在 utils/linkify 那一处，这里只负责画出来。 -->
      <MentionText
        :text="todo.description"
        :mention-names="mentionNames"
        :self-mention="selfMention"
      />
    </p>

    <div v-if="cardImages.length" class="mt-1.5 flex flex-wrap gap-1 px-3">
      <TodoImageThumb
        v-for="(img, i) in cardImages"
        :key="img.sha256"
        :image="img"
        clickable
        @open="openCardImage(i)"
      />
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
