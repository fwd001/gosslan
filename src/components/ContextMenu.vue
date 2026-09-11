<script setup lang="ts">
/**
 * 右键/上下文菜单的**统一外壳**（用户需求 2026-09-12 晚 #4：「全局的右键菜单样式尽量保持统一」）。
 *
 * 它只负责"怎么弹"这一层：Teleport 到 body、贴边内收、点外部/Esc 关闭、阻止冒泡与二次右键。
 * 「长什么样」全部交给 `style.css` 的 `.gosslan-menu*` 类 —— 这样任何菜单（包括锚定在按钮上的
 * 下拉）都能共用同一套外观，不必各自复刻内边距/圆角/悬停色（那正是此前四处不一致的来源）。
 *
 * 为什么要 Teleport：菜单挂在触发元素内部时会被父级的 `overflow: hidden`（会话列表就是）
 * 裁掉，也会被 `z-index` 层级困住。
 */
import { computed, onBeforeUnmount, onMounted } from "vue";

const props = withDefaults(
  defineProps<{
    x: number;
    y: number;
    /** 预估高度，用于贴近屏幕底部时向上收（不必精确，宁大勿小）。 */
    estimatedHeight?: number;
  }>(),
  { estimatedHeight: 260 },
);

const emit = defineEmits<{ (e: "close"): void }>();

/** 贴边内收：菜单宽按 184px（`.gosslan-menu` 最小宽 + 余量）估。 */
const pos = computed(() => ({
  left: `${Math.max(8, Math.min(props.x, window.innerWidth - 192))}px`,
  top: `${Math.max(8, Math.min(props.y, window.innerHeight - props.estimatedHeight))}px`,
}));

// 点击菜单外 / Esc 关闭。菜单根节点 `@click.stop`，所以内部点击不会触发关闭。
function onDocClick() {
  emit("close");
}
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") emit("close");
}
onMounted(() => {
  document.addEventListener("click", onDocClick);
  window.addEventListener("keydown", onKey);
});
onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClick);
  window.removeEventListener("keydown", onKey);
});
</script>

<template>
  <Teleport to="body">
    <div
      class="frost gosslan-menu fixed z-[70] select-none"
      :style="pos"
      @click.stop
      @contextmenu.prevent
    >
      <slot />
    </div>
  </Teleport>
</template>
