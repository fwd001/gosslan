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
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import AuxWindowShell from "@/components/window/AuxWindowShell.vue";
import GroupTasksBoard from "@/components/GroupTasksBoard.vue";
import { groupTodosGroupId } from "@/utils/auxWindowLabels";
import { currentLocale, t } from "@/i18n";

const groupId = groupTodosGroupId(getCurrentWindow().label);
const chat = useChatStore();
const group = computed(() => (groupId ? (chat.groups.find((g) => g.id === groupId) ?? null) : null));

/**
 * 「该展开哪条任务」—— 两条路都收在这一个函数里（用户 2026-09-24 #39）。
 *
 * 后端 `open_group_todos_window` 每次都会先写一份一次性暂存：
 * - **窗口是新建设的**：那条定向事件发给了一个还没有监听者的文档（等于发丢），
 *   所以只能靠挂载时主动取一次 —— 只发事件的表现就是"第一次点卡片没反应、第二次才有"；
 * - **窗口本来就开着**：不会经历挂载 ⇒ 由定向事件叫醒，再取同一个暂存（取走即清，
 *   所以不会重复展开，也不会把上次那条带进下次打开）。
 * 载荷只带 groupId，取回来的 id 也按本窗口自己的群去要 ⇒ 别群的任务串不过来。
 */
const boardRef = ref<InstanceType<typeof GroupTasksBoard> | null>(null);
/**
 * 拿到的目标先存这里，等首屏数据落地再投。
 *
 * 为什么必须等（2026-09-24 随"先挂载"改造一起改）：窗口现在挂载时数据还没到，
 * 而 `focusTodo(id)` 是在**当前列表**里找那条任务 —— 列表为空时它按设计"什么都不做"
 * （任务被删 / 折叠结果里没有它 ⇒ 不弹"找不到"也不空指针）。少这一步的表现就是
 * "点了卡片，窗口开了却没展开那条"（#39 的原始诉求）。
 */
const pendingFocusId = ref<string | null>(null);

function deliverFocus(id: string) {
  boardRef.value?.focusTodo(id);
}

async function applyFocusRequest() {
  if (!groupId) return;
  const id = await api.takeGroupTodoFocus(groupId).catch(() => null);
  if (!id) return;
  if (chat.todosLoadedOnce(groupId)) {
    deliverFocus(id);
    return;
  }
  pendingFocusId.value = id; // 连点多次以最后一次为准
}

watch(
  () => (groupId ? chat.todosLoadedOnce(groupId) : true),
  (ready) => {
    const id = pendingFocusId.value;
    if (!ready || !id) return;
    pendingFocusId.value = null;
    deliverFocus(id);
  },
);

let unlistenFocus: (() => void) | null = null;
let disposed = false;
onMounted(() => {
  void applyFocusRequest();
  api
    .onGroupTodoFocus((payload) => {
      if (payload?.groupId && payload.groupId !== groupId) return;
      void applyFocusRequest();
    })
    .then((fn) => {
      // 窗口被秒关时 `onUnmounted` 可能已经跑完 ⇒ 拿到 unlisten 就立刻补一次取消，
      // 否则这条监听永久留着（与 `boot.ts` 里"降级挂载也要把注册补上"是同一条纪律）。
      if (disposed) {
        fn();
        return;
      }
      unlistenFocus = fn;
    })
    .catch(() => {
      /* 订阅失败只影响"窗口已开着时再点卡片"那一条路：新建那条仍走挂载时取暂存 */
    });
});
onUnmounted(() => {
  disposed = true;
  unlistenFocus?.();
  unlistenFocus = null;
});

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
      <GroupTasksBoard ref="boardRef" :group-id="groupId" standalone />
    </div>
  </AuxWindowShell>
</template>
