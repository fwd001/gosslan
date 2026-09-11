<script setup lang="ts">
/* focus-ring-ok：本组件的**容器**带 `outline-none`（`role="menu"` / `role="dialog"` +
   `tabindex="-1"`，只用于把焦点接进来），焦点指示由内部条目/控件承担；
   给整块容器画 2px 环在全屏遮罩/弹出菜单上只会变成噪声。
   ⇒ 按 `designGuards` ⑦ 的约定，用文件级逃生阀显式声明，而不是靠"没人发现"。 */
/**
 * 右键/上下文菜单的**统一外壳**（用户需求 2026-09-12 晚 #4：「全局的右键菜单样式尽量保持统一」）。
 *
 * 它只负责"怎么弹"这一层：Teleport 到 body、贴边内收、点外部/Esc 关闭、阻止冒泡与二次右键。
 * 「长什么样」全部交给 `style.css` 的 `.gosslan-menu*` 类 —— 这样任何菜单（包括锚定在按钮上的
 * 下拉）都能共用同一套外观，不必各自复刻内边距/圆角/悬停色（那正是此前四处不一致的来源）。
 *
 * 为什么要 Teleport：菜单挂在触发元素内部时会被父级的 `overflow: hidden`（会话列表就是）
 * 裁掉，也会被 `z-index` 层级困住。
 *
 * ## 键盘可达（HIG *Full Keyboard Access*）
 * 菜单可以用键盘唤起（Shift+F10 / 菜单键 → `contextmenu` 事件），所以它也必须能用键盘操作：
 * 打开时把焦点移进菜单，↑/↓ 在条目间移动、Home/End 跳首尾、Esc 关闭并把焦点还给触发元素。
 * 条目本身由调用方写成 `<button class="gosslan-menu-item">`（见各 `*ContextMenu.vue`），
 * 这里按同一个类名收集条目 —— 用类名而不是 `button` 标签，是为了不把菜单里的说明文字
 * / 分隔线误当成条目。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

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

const rootRef = ref<HTMLElement | null>(null);
/** 打开前的焦点位置：关闭后还回去，避免键盘用户"焦点丢失"（落到 body）。 */
let restoreFocus: HTMLElement | null = null;

/** 当前可聚焦的菜单条目（`.gosslan-menu-item` 与调用方模板约定，见文件头注释）。 */
function items(): HTMLElement[] {
  const root = rootRef.value;
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>(".gosslan-menu-item:not([disabled])"));
}

/** ↑/↓ 循环移动焦点；当前焦点不在菜单内时，向下从第一条开始、向上从最后一条开始。 */
function moveFocus(delta: 1 | -1) {
  const list = items();
  if (!list.length) return;
  const idx = list.indexOf(document.activeElement as HTMLElement);
  const next = idx < 0 ? (delta > 0 ? 0 : list.length - 1) : (idx + delta + list.length) % list.length;
  list[next]?.focus();
}

// 点击菜单外 / Esc 关闭。菜单根节点 `@click.stop`，所以内部点击不会触发关闭。
function onDocClick() {
  emit("close");
}
function onWindowKey(e: KeyboardEvent) {
  // Esc 在 window 上也听一遍：焦点若因某些操作跑出菜单（例如条目里再开面板），仍能关掉。
  if (e.key === "Escape") emit("close");
}
/** 菜单内的键盘导航（焦点在菜单里时才会走到这里）。 */
function onMenuKey(e: KeyboardEvent) {
  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      moveFocus(1);
      break;
    case "ArrowUp":
      e.preventDefault();
      moveFocus(-1);
      break;
    case "Home": {
      e.preventDefault();
      items()[0]?.focus();
      break;
    }
    case "End": {
      e.preventDefault();
      const list = items();
      list[list.length - 1]?.focus();
      break;
    }
    default:
      break;
  }
}

onMounted(() => {
  document.addEventListener("click", onDocClick);
  window.addEventListener("keydown", onWindowKey);
  // 打开即把焦点移进菜单（没有条目时落在容器上），这样 ↑/↓ 立刻可用
  restoreFocus = document.activeElement as HTMLElement | null;
  void nextTick(() => {
    (items()[0] ?? rootRef.value)?.focus();
  });
});
onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClick);
  window.removeEventListener("keydown", onWindowKey);
  // 关闭后把焦点还给触发元素：仅当焦点还留在菜单里、或已经掉到 body 上时才还，
  // 以免抢走用户用鼠标点到的其它元素（鼠标点击不会给 div 聚焦，所以这里通常是必要的）。
  const active = document.activeElement;
  const stuck = active === document.body || (!!rootRef.value && rootRef.value.contains(active));
  if (stuck) restoreFocus?.focus?.();
  restoreFocus = null;
});
</script>

<template>
  <Teleport to="body">
    <div
      ref="rootRef"
      class="frost gosslan-menu fixed z-[70] select-none outline-none"
      :style="pos"
      role="menu"
      aria-orientation="vertical"
      tabindex="-1"
      @click.stop
      @contextmenu.prevent
      @keydown="onMenuKey"
    >
      <slot />
    </div>
  </Teleport>
</template>
