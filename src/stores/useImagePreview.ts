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
 * 实例只有本 store 驱动的**这一个**。桌面端后续把同一份状态镜像到独立预览窗口
 * （见 `openInWindow` 那条路径），届时只改这一个地方。
 *
 * ## `source` 为什么必须留
 * 预览浮层不再长在来源面板里，来源面板关闭时**不能无条件关掉预览**
 * （否则"从任务详情点开图、再把详情关掉"会把图一起弄没），
 * 但也不能放着不管 —— 所以按来源标记关闭：只有"这份相册就是它给的"才收。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * 相册里的一张图 —— 三种取字节的来源，按优先级：
 *  ① `dataSrc`：已经在手边的 data/object URL（旧格式消息、合并转发卡片）；
 *  ② `cid`：待办描述图片（sha256 = content-store cid）；
 *  ③ `msgId`：聊天消息（走 `readFilePreview`）。
 * 与 `ImageLightbox` 的入参同形（那边是渲染侧的唯一实现）。
 */
export interface PreviewImage {
  name: string;
  dataSrc?: string | null;
  msgId?: string;
  cid?: string;
}

export const useImagePreviewStore = defineStore("imagePreview", () => {
  const images = ref<PreviewImage[]>([]);
  const index = ref(0);
  const open = ref(false);
  /** 这份相册是谁给的（`conv:<id>` / `task:<todoId>` / `merge:<msgId>`）。 */
  const source = ref<string | null>(null);

  /**
   * 打开相册。`items` 为空**什么都不做** —— 调用方不必各自判空
   * （"点了没反应"比弹一个空白看图器好，也不该把空数组塞进状态里让别处读到）。
   * 下标越界一律夹到范围内：越界说明调用点已经把顺序算错了，静默夹住比跳到不存在的图好。
   */
  function openGallery(items: PreviewImage[], startIndex = 0, from: string | null = null) {
    if (items.length === 0) return;
    images.value = items;
    index.value = Math.min(Math.max(startIndex, 0), items.length - 1);
    source.value = from;
    open.value = true;
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

  return { images, index, open, source, openGallery, setIndex, close, closeIfFrom };
});
