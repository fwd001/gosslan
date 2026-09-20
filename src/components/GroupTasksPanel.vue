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
import { foldTodos } from "@/utils/todos";
import { t } from "@/i18n";

const props = defineProps<{ open: boolean; groupId: string | null }>();
const emit = defineEmits<{ (e: "close"): void }>();

const chat = useChatStore();
const app = useAppStore();
/** 移动端 → 整页全屏（push/pop 式），桌面端 → max-w-lg 弹窗。 */
const fullscreen = computed(() => app.isMobile);
/** 标题里的计数：只读一次折叠结果，真正的看板在 `GroupTasksBoard` 里自己算。 */
const count = computed(() =>
  props.groupId ? foldTodos(chat.messages[`group:${props.groupId}`] ?? []).length : 0,
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
    <GroupTasksBoard :group-id="groupId" :open="open" />
  </BaseModal>
</template>
