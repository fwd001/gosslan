<script setup lang="ts">
/**
 * 独立「群任务」窗口的根组件（`todos.html`，桌面端固定 label=`tasks`，**全局只有一扇**）。
 *
 * ## 为什么不再是"每群一扇"
 * 原先 label 是 `todo-<groupId>`，窗口靠自己的 label 找回"我是哪个群的窗口"。
 * 用户 2026-09-24 要求"启动时可以不传参数就把 WebView 先建好，用的时候瞬间激活"——
 * 动态 label 在预热那一刻根本不知道该建哪一扇，所以秒开做不到。改成固定 label 之后，
 * "现在该显示哪个群"由后端那份**当前上下文**给（`get_group_todos_context` +
 * 定向事件 `group-todos-target`），与图片预览窗口"换内容不换窗口"完全同一套做法。
 * 代价说清楚：任务栏里不再能按群区分这扇窗。
 *
 * ## 换群时必须做的两件事（常驻窗口的固有责任）
 * 1. **重读数据** —— 以前"每次打开都是新数据"是销毁重建顺带保证的，现在窗口活着，
 *    不重读就是上一次那个群的列表；
 * 2. **重置看板内部状态** —— 筛选、正在编辑的草稿都属于上一个群。做法是给看板加
 *    `:key="groupId"` 让它整块重挂（比在组件里手写"切群清草稿"少一处会漏的地方）。
 *
 * 外壳 = `AuxWindowShell`（自绘标题栏），与设置/日志/预览窗口同一套。
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import AuxWindowShell from "@/components/window/AuxWindowShell.vue";
import GroupTasksBoard from "@/components/GroupTasksBoard.vue";
import { currentLocale, t } from "@/i18n";

const app = useAppStore();
const chat = useChatStore();

/** 这扇窗现在服务哪个群（`null` = 还没被打开过，只是预热放着）。 */
const groupId = ref<string | null>(null);
const group = computed(
  () => (groupId.value ? (chat.groups.find((g) => g.id === groupId.value) ?? null) : null),
);

/** 标题栏文案：「群名 · 群任务」（群信息还没加载出来时只显示「群任务」）。 */
const windowTitle = computed(() =>
  group.value?.name ? `${group.value.name} · ${t("todo.title")}` : t("todo.title"),
);

/**
 * 系统窗口标题（任务栏/窗口列表）跟随标题栏文案。
 *
 * `boot.ts` 的 `installDocumentTitle` 也会按 `data-title-*` 设一次；本 watch 在挂载之后
 * 注册，所以以后者为准 —— 切群时就是这里把任务栏那一条改过来的。
 */
watch(
  [() => group.value?.name, () => currentLocale()],
  () => {
    document.title = windowTitle.value;
  },
  { immediate: true },
);

const boardRef = ref<InstanceType<typeof GroupTasksBoard> | null>(null);
/**
 * 拿到的"该展开哪条"先存这里，等首屏数据落地再投。
 *
 * 为什么必须等：`focusTodo(id)` 是在**当前列表**里找那条任务，列表为空时它按设计什么都不做
 * （任务被删 / 折叠结果里没有它 ⇒ 不弹"找不到"也不空指针）。少这一步的表现就是
 * "点了卡片，窗口开了却没展开那条"（#39 的原始诉求）。
 */
const pendingFocusId = ref<string | null>(null);

async function deliverFocus(id: string) {
  await nextTick(); // 看板可能刚刚因换群整块重挂，等它挂上再投
  boardRef.value?.focusTodo(id);
}

/**
 * 切到某个群：换上下文 → （换群时）把实时订阅带过去 → 重读数据 → 处理待展开。
 *
 * `watchGroupTodos` 只在**群真的变了**时调用：store 里那份实现自己会摘掉上一个群的监听，
 * 重复调用只是白建一次订阅。
 */
async function applyTarget(next: string | null) {
  if (!next) return;
  const switched = groupId.value !== next;
  const fresh = !chat.todosLoadedOnce(next);
  groupId.value = next;
  if (switched) void chat.watchGroupTodos(next).catch(() => {});
  try {
    await chat.loadGroupTodos(next);
  } catch (e) {
    // 首次（或换群）读失败要说出来：那看起来和"这个群没有任务"一模一样。
    // 只是刷新一个已加载过的群失败则保持静默 —— 列表里还有内容可看，比清空重来更好。
    if (fresh) app.toastError(e, t("todo.loadFail"));
  }
  const id = await api.takeGroupTodoFocus(next).catch(() => null);
  if (!id) return;
  if (chat.todosLoadedOnce(next)) {
    void deliverFocus(id);
    return;
  }
  pendingFocusId.value = id; // 连点多次以最后一次为准
}

// 首屏落地时补投等着的那条
watch(
  () => (groupId.value ? chat.todosLoadedOnce(groupId.value) : true),
  (ready) => {
    const id = pendingFocusId.value;
    if (!ready || !id) return;
    pendingFocusId.value = null;
    void deliverFocus(id);
  },
);

let unlistenTarget: (() => void) | null = null;
let disposed = false;
onMounted(() => {
  // 两条路都要：预热过 / 新建的窗口靠挂载这次取上下文；已经存在的那扇靠定向事件被叫醒。
  void api
    .getGroupTodosContext()
    .then((ctx) => void applyTarget(ctx))
    .catch(() => {
      /* 取不到上下文 ⇒ 窗口显示"还没选群"那一档，下次点卡片的事件仍会把它带起来 */
    });
  void api
    .onGroupTodosTarget((payload) => {
      if (!payload?.groupId) return;
      void applyTarget(payload.groupId);
    })
    .then((fn) => {
      // 窗口被秒关时 `onUnmounted` 可能已经跑完 ⇒ 拿到 unlisten 就立刻补一次取消
      if (disposed) {
        fn();
        return;
      }
      unlistenTarget = fn;
    })
    .catch(() => {
      /* 订阅失败只影响"窗口已存在时再点卡片"那一条路：挂载那条路仍会取一次上下文 */
    });
});
onUnmounted(() => {
  disposed = true;
  unlistenTarget?.();
  unlistenTarget = null;
});
</script>

<template>
  <AuxWindowShell :title="windowTitle">
    <div v-if="!groupId" class="p-6 text-sm text-[var(--gosslan-text-2)]">
      {{ t("todo.windowNoGroup") }}
    </div>
    <div v-else class="min-h-0 flex-1 overflow-hidden p-4">
      <!-- `:key` 是换群必须做的重置：筛选/草稿/展开态都属于上一个群，留着就是串群。 -->
      <GroupTasksBoard :key="groupId" ref="boardRef" :group-id="groupId" standalone />
    </div>
  </AuxWindowShell>
</template>
