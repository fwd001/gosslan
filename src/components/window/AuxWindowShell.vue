<script setup lang="ts">
/**
 * 应用自有的**辅助窗口外壳**（设置 / 群任务）：自绘标题栏 + 内容区。
 *
 * 用户 2026-09-17：「新窗口好像用的是系统的样式？标题栏和整体窗口的背景颜色有明显的界限，
 * 没有融合」—— 根因是辅助窗口的 builder 没设 `decorations`，于是留了系统标题栏，
 * 那条的底色由系统决定、我们改不了。现在一律 `decorations(false)` + 这套自绘 chrome。
 *
 * `decorations:false` 带来两条后果，**只在这一处**表达（免得各窗口各写一份再漂移）：
 * 1. 窗口最外层底色 = caption 底色（顶部与内容连成一体）；
 * 2. 系统不再画窗口描边 ⇒ 自己用 1px inset ring 画（与主窗口 `ResponsiveLayout` 同款）。
 *
 * 小窗口不给最大化：`show-maximize=false`（OS 层也在 builder 上设了 `.maximizable(false)`）。
 * 日志窗口结构不同（同一组件要兼移动端整页形态），因此它直接用 `TitleBar`，不套这个壳。
 */
import { onMounted, watch } from "vue";
import TitleBar from "@/components/TitleBar.vue";
import ToastHud from "@/components/ToastHud.vue";
import { useDocumentDark } from "@/composables/useDocumentDark";
import { api } from "@/api";
import { isMac } from "@/utils/platform";

defineProps<{ title: string }>();

const dark = useDocumentDark();

/**
 * macOS：`decorations:false` 之后系统不再给圆角与主题背景，得由应用补
 * （与主窗口 `App.vue` 调的是同一条命令，它本来就作用于**调用窗口**）。
 * ⚠️ 必须在 `onMounted`：wry 载入后才替换 contentView，setup 阶段设会被丢掉
 * （见 `macos_window.rs` 的说明）。
 */
function applyShape() {
  if (!isMac) return;
  void api.applyMacosWindowShape(dark.value).catch(() => {
    /* 非 macOS / 命令不可用：忽略，不影响功能 */
  });
}
onMounted(applyShape);
watch(dark, applyShape);
</script>

<template>
  <div
    class="flex h-screen flex-col overflow-hidden bg-[var(--gosslan-app-bg)] font-gosslan text-[var(--gosslan-text)] ring-1 ring-inset ring-[var(--gosslan-window-ring)]"
  >
    <TitleBar :title="title" :show-maximize="false" :close-to-tray="false" />
    <slot />
    <!-- Toast 宿主：**辅助窗口的统一反馈通道**。
         `ToastHud` 是应用内唯一的轻量反馈层（成功/失败都靠它）。此前只在主窗口与群任务
         窗口挂过，设置窗口**没有** ⇒ 里面的按钮（恢复默认 / 清除聊天数据 / 导出…）点下去
         弹窗一关就再没有任何可见反馈，看起来就是"点了没反应"（用户 2026-09-21）。
         放在外壳里而不是各窗口各挂一份：新增辅助窗口时不会再漏（`windowEntries` 守卫钉住）。 -->
    <ToastHud />
  </div>
</template>
