<script setup lang="ts">
/**
 * 独立「群任务」窗口的根组件（`todos.html`，label = `todo-<groupId>`，每群一个）。
 *
 * 群 ID 从**窗口自己的 label** 解析（label 是身份/参数，不是"按 label 分支渲染"——
 * 本窗口仍然只有一份文档与一个入口，见 ADR-0018）。
 *
 * 窗口外壳 = `AuxWindowShell`（自绘标题栏 + 1px inset ring），与主窗口/设置窗口同一套
 * —— 用户 2026-09-17：「新窗口用的是系统样式？标题栏和窗口背景有界限」；群名进 caption。
 */
import { computed, watch } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useChatStore } from "@/stores/useChatStore";
import AuxWindowShell from "@/components/window/AuxWindowShell.vue";
import GroupTasksBoard from "@/components/GroupTasksBoard.vue";
import { groupTodosGroupId } from "@/utils/auxWindowLabels";
import { currentLocale, t } from "@/i18n";

const groupId = groupTodosGroupId(getCurrentWindow().label);
const chat = useChatStore();
const group = computed(() => (groupId ? (chat.groups.find((g) => g.id === groupId) ?? null) : null));

/** 标题栏文案：「群名 · 群任务」（群信息还没加载出来时只显示「群任务」）。 */
const windowTitle = computed(() =>
  group.value?.name ? `${group.value.name} · ${t("todo.title")}` : t("todo.title"),
);

/**
 * 系统窗口标题（任务栏/窗口列表）跟随标题栏文案。
 *
 * `boot.ts` 的 `installDocumentTitle` 也会按 `data-title-*` 设一次；本 watch 在入口里
 * **挂载之后**才注册，所以后者生效。依赖里显式读 `currentLocale()` 是为了在切语言时重算。
 */
watch(
  [() => group.value?.name, () => currentLocale()],
  () => {
    document.title = windowTitle.value;
  },
  { immediate: true },
);
</script>

<template>
  <AuxWindowShell :title="windowTitle">
    <div v-if="!groupId" class="p-6 text-sm text-[var(--gosslan-text-2)]">
      {{ t("todo.windowBadLabel") }}
    </div>
    <div v-else class="min-h-0 flex-1 overflow-hidden p-4">
      <GroupTasksBoard :group-id="groupId" standalone />
    </div>
  </AuxWindowShell>
</template>
