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
let unlistenClose: (() => void) | null = null;
let disposed = false;

/**
 * 关闭时把这个文档自己持有的相册放掉。
 *
 * 窗口现在是**关闭即隐藏**（常驻，为了让第二次看图秒开）⇒ ✕ 不再销毁 WebView。
 * 不主动清就有两个后果：一是几十 MB 的图解码位图一直挂在看不见的窗口里；
 * 二是下次打开会**先闪一下上一次的图**（复用时 Rust 先 show、事件后到、内容后拉）。
 * 挂在 `close-requested` 上而不是只挂在"我们自己的 ✕"上：标题栏 ✕、看图器 ✕/Esc、
 * ⌘W / Ctrl+W 三条路都会经过它，漏一条就是一个不会放资源的窗口。
 */
function releaseAlbum() {
  images.value = [];
  index.value = 0;
}

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
  void getCurrentWindow()
    .onCloseRequested(() => releaseAlbum())
    .then((fn) => {
      if (disposed) {
        fn();
        return;
      }
      unlistenClose = fn;
    })
    .catch(() => {
      /* 订阅失败最多退回"隐藏时留着上一份相册"，不影响看图本身 */
    });
});
onUnmounted(() => {
  disposed = true;
  unlisten?.();
  unlisten = null;
  unlistenClose?.();
  unlistenClose = null;
});

function closeSelf() {
  // 先放资源再关：`close-requested` 的监听也在同一时刻清，这里再清一次是为了
  // 不依赖那条订阅真的注册成功（幂等，成本是两行）。
  releaseAlbum();
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
      top-inset="var(--gosslan-title-h)"
      @close="closeSelf()"
    />
  </AuxWindowShell>
</template>
