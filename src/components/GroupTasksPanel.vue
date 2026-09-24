<script setup lang="ts">
/**
 * 群任务面板（**应用内弹窗壳**）。
 *
 * 桌面端正常路径是**独立窗口**（`ChatWindow.openTasks` → `open_group_todos_window`）；
 * 这个弹窗留给两处：① 移动端（独立窗口是桌面能力）；② 桌面窗口创建失败时的回退。
 *
 * 看板本体在 `GroupTasksBoard`（与独立窗口 `GroupTodosWindow` 共用同一份），
 * 这里只负责套 `BaseModal` 与算标题里的计数。
 */
import { computed } from "vue";
import { useChatStore } from "@/stores/useChatStore";
import { useAppStore } from "@/stores/useAppStore";
import BaseModal from "@/components/BaseModal.vue";
import GroupTasksBoard from "@/components/GroupTasksBoard.vue";
import { foldTodos, isEffectivelyArchived } from "@/utils/todos";
import { t } from "@/i18n";

const props = defineProps<{
  open: boolean;
  groupId: string | null;
  /** 从时间线的任务卡片点进来时要直达详情的那条任务（null = 只打开看板）。 */
  focusTodoId?: string | null;
}>();
const emit = defineEmits<{ (e: "close"): void }>();

const chat = useChatStore();
const app = useAppStore();
/** 移动端 → 整页全屏（push/pop 式），桌面端 → max-w-lg 弹窗。 */
const fullscreen = computed(() => app.isMobile);
/**
 * 标题里的计数：**活动**任务数 —— 与看板里「全部」那一档同一个判据（折叠后用
 * `isEffectivelyArchived` 滤掉已归档）。此前算的是"含归档的全部"，
 * 于是弹窗标题写 (9) 而进去看到 6 条（用户 #23 的同类漂移：三处消费者两种口径）。
 */
const count = computed(() =>
  props.groupId
    ? foldTodos(chat.messages[`group:${props.groupId}`] ?? []).filter((x) => !isEffectivelyArchived(x))
        .length
    : 0,
);
</script>

<template>
  <BaseModal
    :open="open"
    :fullscreen="fullscreen"
    :title="count ? t('todo.title') + ` (${count})` : t('todo.title')"
    width="max-w-lg"
    @close="emit('close')"
  >
    <GroupTasksBoard :group-id="groupId" :open="open" :focus-todo-id="focusTodoId" />
  </BaseModal>
</template>
