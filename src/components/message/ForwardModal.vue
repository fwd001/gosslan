<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref, watch } from "vue";
import { useDeferredRef } from "@/composables/useDeferredRef";
import { useChatStore } from "@/stores/useChatStore";
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
import BaseModal from "@/components/BaseModal.vue";
import type { MsgKind } from "@/types";

const props = defineProps<{
  open: boolean;
  /** 转发的消息类型（决定预览文案）。 */
  kind: MsgKind;
  /** 预览片段（截断后）。 */
  snippet: string;
  /**
   * 待转发的条数。`> 1` 即"多选批量转发"。
   *
   * 批量时**转发方式由调用方预先定好**（见下面的 `mode`）：多选操作条上直接是
   * 「逐条转发 / 合并转发」两个按钮（用户 2026-09-21：「不要先转发再选合并转发还是逐条转发」）。
   */
  count?: number;
  /**
   * 调用方已经定好的转发方式。给了它 ⇒ 点会话**直接转发**（与单条消息同一条路径，少一步）；
   * 不给 ⇒ 沿用"先选会话、再在底部选方式"的老流程（保留给其它/将来的批量入口）。
   */
  mode?: "per-message" | "merged";
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "pick", convId: string, mode: "per-message" | "merged"): void;
}>();

/** 是否多选批量转发。 */
const multi = computed(() => (props.count ?? 0) > 1);
/** 批量、且调用方**没**给方式 ⇒ 才需要"先选会话再选方式"（见 `mode`）。 */
const needsModeChoice = computed(() => multi.value && !props.mode);
/** 多选时选中的目标会话（未选中时底部两个按钮不可点）。 */
const picked = ref<string | null>(null);
// 每次打开都清空选择：上一轮的选中态留到下一轮会让"点了转发直接发出去"。
watch(
  () => props.open,
  (v) => {
    if (v) picked.value = null;
  },
);

function onPickConversation(id: string) {
  // 方式已定（或本来就是单条）⇒ 点会话直接转发，少一步"再选方式"
  if (needsModeChoice.value) picked.value = id;
  else emit("pick", id, props.mode ?? "per-message");
}

const chat = useChatStore();
const keyword = ref("");
/** 延迟镜像：过滤会话列表用（连发粘贴时避免每个字符重渲染整列，见 useDeferredRef）。 */
const query = useDeferredRef(keyword);

const filtered = computed(() => {
  const kw = query.value.trim().toLowerCase();
  if (!kw) return chat.conversations;
  return chat.conversations.filter((c) => c.name.toLowerCase().includes(kw));
});

// 穷举所有 MsgKind：新增 kind 时这里会编译报错，逼你决定它的展示文案 ——
// 比运行期回落到「消息」两个字更容易发现遗漏。
const KIND_LABELS: Record<MsgKind, string> = {
  text: t("common.text"),
  code: t("common.code"),
  image: t("common.image"),
  file: t("common.file"),
  system: t("common.system"),
  // 静默事件（表情回应）不该出现在转发列表里；给个中性文案兜底，
  // 真正的拦截在转发入口（不在时间线上渲染，就没有转发菜单可点）。
  reaction: t("msg.reactionMore"),
  recall: t("msg.recall"),
  recalled: t("msg.recalled"),
  // 静默事件：不在时间线上渲染，就不会有转发入口。给中性文案兜底。
  pin: t("msg.pin"),
  announcement: t("group.announce"),
  announcement_delete: t("group.announce"),
  todo: t("todo.title"),
  todo_update: t("todo.title"),
  poll: t("poll.title"),
  poll_vote: t("poll.title"),
  merge: t("merge.title"),
};
const kindLabel = computed(() => KIND_LABELS[props.kind] ?? t("msg.message"));
</script>

<template>
  <BaseModal :open="open" :title="t('msg.forwardTo')" @close="emit('close')">
    <div class="space-y-3">
      <!-- 引用预览：多选时换成"已选 N 条"（逐条内容不适合塞进一行预览） -->
      <div class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-2 text-xs text-[var(--gosslan-text-2)]">
        <template v-if="multi">
          <span class="align-middle">{{ t("multi.forwardPreview", { n: props.count ?? 0 }) }}</span>
        </template>
        <template v-else>
          <span class="mr-1 rounded-[var(--gosslan-radius-xs)] bg-[var(--gosslan-panel)] px-1.5 py-0.5 text-[11px] text-[var(--gosslan-text)]">{{ kindLabel }}</span>
          <span class="align-middle">{{ snippet }}</span>
        </template>
      </div>

      <input
        v-model="keyword"
        maxlength="50"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
        :placeholder="t('msg.searchConversation')"
        class="w-full rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-transparent px-3 py-2 text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)] focus:border-transparent"
      />

      <div class="max-h-64 select-none overflow-y-auto">
        <!-- 选中态用**内缩**环（ring-inset）：Tailwind 的 ring 默认画在元素**外面**，
             而本行在 `overflow-y-auto` 的滚动容器里 —— 外扩的 1px 会被容器裁掉左右两边
             （`overflow-y: auto` 会把 `overflow-x` 一并算成 auto），上下则压到相邻的 52px 行上，
             看着就是"描边串行"（用户 2026-09-17 报的选中态 UI bug）。 -->
        <button
          v-for="c in filtered"
          :key="c.id"
          class="flex h-[52px] w-full items-center gap-3 rounded-[var(--gosslan-radius-md)] px-2 text-left transition"
          :class="[
            picked === c.id ? 'bg-[var(--gosslan-hover)] ring-1 ring-inset ring-[var(--gosslan-primary-ring)]' : 'hover:bg-[var(--gosslan-hover)]',
            // 多选时点会话只是**选中目标**（模式在底部两个按钮上选），不再立即转发
            multi ? 'cursor-pointer' : '',
          ]"
          @click="onPickConversation(c.id)"
        >
          <span
            v-if="c.kind === 'group'"
            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--gosslan-avatar-radius)] bg-[var(--gosslan-rail-active)] text-xs text-[var(--gosslan-text-2)]"
          >
            {{ t("common.group") }}
          </span>
          <span
            v-else
            class="gosslan-avatar-box flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-sm text-white"
            :style="{ backgroundColor: nameToColor(c.name) }"
          >
            <img alt="" v-if="c.avatar" :src="c.avatar" class="h-full w-full object-cover" />
            <span v-else class="gosslan-avatar-initial" :data-len="avatarInitialLen(c.name)">{{ avatarInitial(c.name) }}</span>
          </span>
          <span class="min-w-0 flex-1 truncate text-[13px] text-[var(--gosslan-text)]" :title="c.name">{{ c.name }}</span>
        </button>
        <div v-if="filtered.length === 0" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">{{ t("msg.noMatch") }}</div>
      </div>

      <!-- 批量转发且**调用方没给方式**时的兜底：先选会话，再选转发方式（微信同款）。
           正常入口（多选操作条）已经在条上把方式选好了 ⇒ 这里不出现，点会话直接转发。
           单条消息同理不出现 —— 点会话就直接转发，少一步。 -->
      <div v-if="needsModeChoice" class="flex justify-end gap-2">
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] px-4 py-2 text-sm text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)] disabled:opacity-40"
          :disabled="!picked"
          @click="picked && emit('pick', picked, 'per-message')"
        >
          {{ t("multi.forwardPerMessage") }}
        </button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-4 py-2 text-sm text-white transition hover:bg-[var(--gosslan-primary-hover)] disabled:opacity-40"
          :disabled="!picked"
          @click="picked && emit('pick', picked, 'merged')"
        >
          {{ t("multi.forwardMerged") }}
        </button>
      </div>
    </div>
  </BaseModal>
</template>
