<script setup lang="ts">
/**
 * 独立「图片预览」窗口的根组件（`preview.html`，桌面端固定 label=`preview`，**全局只有一个**）。
 *
 * 用户 2026-09-24 #40 要的是：看图是一个公共能力 —— 任何界面点图都开这同一个窗口，
 * 从聊天点开的就在聊天那批图里左右循环，从任务/收藏点开的就在那一条的几张图里循环。
 * 所以这里的语义是"**替换内容**而不是再开一扇窗"：主窗口每次调用都会把新的相册写进后端
 * 那份暂存（`AppState::preview_gallery`），窗口要么挂载时取一次（新建），
 * 要么被 `preview-gallery` 事件叫醒后再取一次（已开着）。
 *
 * 内容靠 IPC 取、不从主窗口传：跨文档传 `blob:` objectURL 一定是破图，
 * 而 `msgId` / `cid` 这种引用在这个文档里能自己把字节取回来（`filePreview` 那套）。
 *
 * 外壳 = `AuxWindowShell`（自绘标题栏），与设置/日志/群任务窗口同一套。
 */
import { computed, onMounted, onUnmounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "@/api";
import AuxWindowShell from "@/components/window/AuxWindowShell.vue";
import ImageLightbox from "@/components/message/ImageLightbox.vue";
import type { PreviewImage } from "@/stores/useImagePreview";
import { t } from "@/i18n";

const images = ref<PreviewImage[]>([]);
const index = ref(0);
/**
 * 首帧没内容时**不算 open**：等第一次取到相册再点亮。
 * 这样组件内部那条"open 变化时复位缩放/焦点"的逻辑有一次真正的 false→true，
 * 而不是带着空数组在窗口里挂着一个空白全屏层。
 */
const open = computed(() => images.value.length > 0);

/**
 * 取当前该看的相册。⚠️ **不清**后端那份暂存 —— 每次投递都会整体覆盖它，
 * 所以留着也不会把"上一次的内容"带给下一次；反过来如果这里清掉，
 * 窗口在"事件先到、监听器还没注册"的窗口期就会拿不到内容（这正是批次 w 记过的竞态）。
 */
async function pull() {
  const gallery = await api.getImagePreviewGallery().catch(() => null);
  if (!gallery?.items?.length) return;
  images.value = gallery.items;
  index.value = Math.min(Math.max(gallery.index ?? 0, 0), gallery.items.length - 1);
}

let unlisten: (() => void) | null = null;
let disposed = false;
onMounted(() => {
  void pull();
  api
    .onImagePreviewChanged(() => void pull())
    .then((fn) => {
      // 窗口被秒关时 onUnmounted 可能已经跑完 ⇒ 拿到 unlisten 就立刻补一次取消
      if (disposed) {
        fn();
        return;
      }
      unlisten = fn;
    })
    .catch(() => {
      /* 订阅失败只影响"窗口已开着时再点图"这一条路（内容会停在上一次） */
    });
});
onUnmounted(() => {
  disposed = true;
  unlisten?.();
  unlisten = null;
});

function closeSelf() {
  void getCurrentWindow()
    .close()
    .catch(() => {
      /* 关不掉也不能把异常吞进 unhandled rejection */
    });
}
</script>

<template>
  <AuxWindowShell :title="t('preview.title')">
    <ImageLightbox
      :images="images"
      v-model:index="index"
      :open="open"
      @close="closeSelf()"
    />
  </AuxWindowShell>
</template>
