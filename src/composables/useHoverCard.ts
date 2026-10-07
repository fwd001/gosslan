/**
 * 一份「贴着锚点弹、四个方向都会翻、列表一滚就收」的浮层状态。
 *
 * 配合 `utils/hoverCard.placeCard` 用：摆位的几何在那边，这里的职责是把
 * **开/关 + 视口失效**这套样板收掉（每个调用点各写一遍 scroll/resize 监听
 * 与"忽略浮层自己的内部滚动"那段判断，是漂移的温床）。
 *
 * ⚠️ 这里只管"同一时刻这一份开没开"。跨浮层的互斥（点开 A 的名单要收掉 B 的菜单）
 *    仍归 `useExclusivePopup`，两件事不是一个开关。
 */
import { computed, onMounted, onUnmounted, ref } from "vue";
import { cardStyle, placeCard, type CardOptions, type CardPlacement } from "@/utils/hoverCard";

export function useHoverCard(opts: CardOptions = {}) {
  const pos = ref<CardPlacement | null>(null);
  let node: HTMLElement | null = null;

  function open(anchor: HTMLElement) {
    pos.value = placeCard(anchor.getBoundingClientRect(), opts);
  }
  function close() {
    pos.value = null;
  }
  /** 挂到 Teleport 出去那一层上（`:ref="card.setEl"`），供下面的"自己的滚动"判断用。 */
  function setEl(el: unknown) {
    node = el instanceof HTMLElement ? el : null;
  }

  /**
   * 滚动 / 改窗口 ⇒ 收起：fixed 坐标不跟着列表滚，留着就是飘在别处。
   * 忽略浮层**自己**的内部滚动（卡片超高时有 `overflow-y: auto`，
   * 那个滚动带 capture 也会冒到 window 上，不排除就会"一拉滚动条弹框就消失"）。
   */
  function onScrollOrResize(e: Event) {
    if (!pos.value) return;
    if (e.type === "scroll" && node && e.target instanceof Node && node.contains(e.target)) return;
    close();
  }

  onMounted(() => {
    window.addEventListener("scroll", onScrollOrResize, true);
    window.addEventListener("resize", onScrollOrResize);
  });
  onUnmounted(() => {
    window.removeEventListener("scroll", onScrollOrResize, true);
    window.removeEventListener("resize", onScrollOrResize);
  });

  return {
    pos,
    open,
    close,
    setEl,
    /** 直接绑到那一层的 `:style="card.style"`。 */
    style: computed(() => (pos.value ? cardStyle(pos.value) : {})),
  };
}
