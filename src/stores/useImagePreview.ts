/**
 * 全局图片预览的**唯一状态源**（用户 2026-09-24 #40：看图要做成一个公共能力）。
 *
 * ## 为什么不是"每个面板各挂一个 `<ImageLightbox>`"
 * 原先有**四份**实例（会话、任务看板、任务详情、合并转发卡片），各自持有自己的
 * `images/index/open`。这带来两类一直存在的东西：
 * - **能力不一致**：同一件"看图"在不同入口的表现取决于那个面板有没有把数组传全
 *   （会话里是"整屏上下文循环"，任务里只有"这一条任务的几张图"，本来就该是同一个契约）；
 * - **加一个入口就得再抄一遍**：合并卡片那份还额外处理过"objectURL 在组件卸载后被回收"
 *   —— 那份知识锁在那个组件里，别处复用不到。
 *
 * 现在契约是 `{ items, startIndex, source }` 一次调用，实现只有 `ImageLightbox` 一份，
 * 实例只有本 store 驱动的**这一个**。桌面端把同一份状态投递给**全局唯一的独立预览窗口**
 * （`inWindow` 为真时壳层那份覆盖层让位），所以新增入口仍然只改这一个地方。
 *
 * ## `source` 为什么必须留
 * 预览浮层不再长在来源面板里，来源面板关闭时**不能无条件关掉预览**
 * （否则"从任务详情点开图、再把详情关掉"会把图一起弄没），
 * 但也不能放着不管 —— 所以按来源标记关闭：只有"这份相册就是它给的"才收。
 */
import { defineStore } from "pinia";
import { ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { api } from "@/api";
import type { PreviewImage } from "@/types";

/** 线格式与渲染入参是同一个类型（放在 `@/types`：api 与 store 都要用，别互相 import）。 */
export type { PreviewImage } from "@/types";

/**
 * 这份相册能不能交给独立预览窗口：**每个条目都可寻址**（`msgId` / `cid`），
 * 或者带一个**自包含**的 `data:` URL。
 *
 * `blob:` objectURL 是唯一要挡住的形状 —— 它只在发起它的那个文档里有效，
 * 拿到另一个文档去 `<img src>` 就是破图。判不了"取不取得到"就别硬传。
 */
export function deliverableToWindow(items: PreviewImage[]): boolean {
  return items.every((it) => !!it.msgId || !!it.cid || (it.dataSrc ?? "").startsWith("data:"));
}

export const useImagePreviewStore = defineStore("imagePreview", () => {
  const app = useAppStore();
  const images = ref<PreviewImage[]>([]);
  const index = ref(0);
  const open = ref(false);
  /** 这份相册是谁给的（`conv:<id>` / `task:<todoId>` / `merge:<msgId>`）。 */
  const source = ref<string | null>(null);
  /** 当前内容是否由**独立预览窗口**显示（桌面端）。壳层那份覆盖层据此让位。 */
  const inWindow = ref(false);

  /**
   * 打开相册。`items` 为空**什么都不做** —— 调用方不必各自判空
   * （"点了没反应"比弹一个空白看图器好，也不该把空数组塞进状态里让别处读到）。
   * 下标越界一律夹到范围内：越界说明调用点已经把顺序算错了，静默夹住比跳到不存在的图好。
   */
  function openGallery(items: PreviewImage[], startIndex = 0, from: string | null = null) {
    if (items.length === 0) return;
    const at = Math.min(Math.max(startIndex, 0), items.length - 1);
    images.value = items;
    index.value = at;
    source.value = from;
    open.value = true;
    // 桌面端 + 每个条目都可跨文档寻址 ⇒ 交给那扇**全局唯一**的预览窗口（用户 #40：
    // "不同界面查看图片会替换里面的内容"）。两种情况宁可留在覆盖层也不递过去：
    //  - 移动端没有独立窗口（那条命令只有桌面实现）；
    //  - 带 `blob:` objectURL 的相册跨文档一定是破图（见 `deliverableToWindow`）。
    // 投递被拒（超出上限、窗口建不出来）同样退回覆盖层 ——
    // 用户点了就必须看到图，而不是"什么都没发生"。
    if (app.isMobile || !deliverableToWindow(items)) {
      inWindow.value = false;
      return;
    }
    inWindow.value = true;
    api
      .openImagePreview(items, at)
      .catch(() => {
        inWindow.value = false;
      });
  }

  function setIndex(i: number) {
    if (i >= 0 && i < images.value.length) index.value = i;
  }

  function close() {
    open.value = false;
    source.value = null;
    images.value = [];
    index.value = 0;
  }

  /** 来源页关闭：只有这份相册确实是它给的才收起预览。 */
  function closeIfFrom(from: string) {
    if (source.value === from) close();
  }

  return {
    images,
    index,
    open,
    source,
    inWindow,
    openGallery,
    setIndex,
    close,
    closeIfFrom,
  };
});
