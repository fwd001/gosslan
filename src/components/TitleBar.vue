<script setup lang="ts">
/**
 * 自绘标题栏（caption）—— **主窗口与所有「应用自己的」辅助窗口共用这一份**。
 *
 * 为什么共用（用户 2026-09-17：「新窗口好像用的是系统的样式？标题栏和整体窗口的背景颜色有
 * 明显的界限，没有融合」）：窗口一律 `decorations:false`，顶部那条 caption 由前端画，
 * 底色就能与内容连成一体；系统标题栏的底色我们改不了，必然出现一条接缝。
 *
 * 差异全部靠 props 表达（不再读 store —— 辅助窗口的入口也要能 import 它）：
 * - `title`：辅助窗口显示**功能名**（设置/运行日志/群名 · 群任务）；主窗口不传 = 纯拖拽条；
 * - `showMaximize` / `showMinimize`：小窗口不给最大化（OS 层也设了 `.maximizable(false)`）；
 * - `isMobile`：移动端不画这条（由调用方传入，避免共享组件反向依赖 app store）。
 */
import { onBeforeUnmount, onMounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "@/api";
import { X } from "lucide-vue-next";
import { isMac } from "@/utils/platform";
import { t } from "@/i18n";

const props = withDefaults(
  defineProps<{
    /** 功能名（辅助窗口传；主窗口不传 ⇒ 纯拖拽条）。 */
    title?: string;
    /** 是否渲染「最大化/还原」（Win/Linux 按钮 + macOS 绿灯）。小窗口传 false。 */
    showMaximize?: boolean;
    /** 是否渲染「最小化」。 */
    showMinimize?: boolean;
    /** 移动端不渲染标题栏。 */
    isMobile?: boolean;
    /** 关闭键的语义：主窗口 = 关闭到托盘（默认）；辅助窗口传 false = 真的关闭本窗口。 */
    closeToTray?: boolean;
  }>(),
  { title: "", showMaximize: true, showMinimize: true, isMobile: false, closeToTray: true },
);

const maximized = ref(false);

async function refreshMaximized() {
  if (!props.showMaximize) return; // 不可最大化 ⇒ 不必问（也避开无意义的 IPC）
  try {
    maximized.value = await api.windowIsMaximized();
  } catch {
    maximized.value = false; // 移动端 / 命令不可用时忽略
  }
}

async function toggleMaximize() {
  if (!props.showMaximize) return;
  try {
    maximized.value = await api.windowToggleMaximize();
  } catch {
    /* 忽略 */
  }
}

/**
 * 窗口是否聚焦（macOS 红绿灯的**失焦变灰**是原生行为）。
 *
 * 用户 2026-09-12 反馈：「mac 自己写的三个关闭按钮，和原生的关闭按钮的样式好像不太一样，
 * 还有大小。」最明显的差异就是这条：原生在窗口失焦时把整组变成**中性灰**（不带红黄绿、
 * 也不显示符号），而我们自绘的一直是彩色的 —— 一旦主窗口与设置窗口并排（设置窗口聚焦、
 * 主窗口失焦），两套摆在一起就明显不像。
 */
const focused = ref(true);
let unlistenFocus: (() => void) | null = null;

/** macOS 绿灯的 Option-click → 全屏（HIG：缩放按钮按住 Option 进入/退出全屏）。 */
const fullscreen = ref(false);
async function toggleFullscreen() {
  try {
    fullscreen.value = await api.windowToggleFullscreen();
  } catch {
    fullscreen.value = false;
  }
}

function onGreenClick(e: MouseEvent) {
  if (e.altKey) void toggleFullscreen();
  else void toggleMaximize();
}

/**
 * 双击标题栏 → 缩放（仅 macOS；Windows 由 tao 的拖拽区原生处理双击最大化）。
 * ⚠️ 系统偏好「双击标题栏的动作」在 WebView 里读不到，这里用系统默认的"缩放"。
 * 不可最大化的窗口（辅助窗口）直接不响应。
 */
function onTitlebarDblClick() {
  if (props.isMobile || !isMac || !props.showMaximize) return;
  void toggleMaximize();
}

/**
 * 手动拖拽兜底（Windows 主方案，macOS 也能用）。
 *
 * Tauri v2 Windows WebView2 上 `data-tauri-drag-region` 有已知缺陷：
 * 原生实现只在 drag-region 最顶部几像素响应拖拽，下半部分不触发。
 * 改为监听 mousedown 显式调 `startDragging()`，绕开 WebView2 的 bug。
 * macOS 上手动实现和原生等价，统一用这一套减少平台分叉。
 *
 * 按钮用 `data-no-drag-region` + `@mousedown.stop` 阻止冒泡到这里。
 */
function onDragStart(e: MouseEvent) {
  if (props.isMobile) return;
  if (e.button !== 0) return; // 只响应左键
  try {
    void getCurrentWindow().startDragging();
  } catch {
    /* 非 Tauri 环境忽略 */
  }
}

/**
 * Ctrl+W 关闭窗口（仅 Windows/Linux）：与标题栏「×」一致。
 *
 * macOS **不在这里处理**：自绘标题栏 + `decorations:false` 下本无系统 ⌘W，
 * 现已由 src-tauri/src/menu.rs 补回原生「窗口 → 关闭」菜单（`decorate_aux_window` / lib.rs
 * 亦恢复了 NSWindow 的 `Closable` 位），交给系统处理即可，避免前端兜底与原生菜单双触发。
 */
function onKeydown(e: KeyboardEvent) {
  if (props.isMobile) return;
  if (isMac) return;
  if (e.ctrlKey && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "w") {
    e.preventDefault();
    void api.windowClose();
  }
}

onMounted(() => {
  refreshMaximized();
  window.addEventListener("keydown", onKeydown);
  // 焦点跟踪：失败（非 Tauri 环境 / 权限）就保持"聚焦"外观，不影响任何功能。
  void (async () => {
    try {
      const w = getCurrentWindow();
      focused.value = await w.isFocused();
      unlistenFocus = await w.onFocusChanged(({ payload }) => {
        focused.value = payload;
      });
    } catch {
      /* 忽略：非 Tauri 环境（纯 vite dev） */
    }
  })();
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  unlistenFocus?.();
  unlistenFocus = null;
});
</script>

<template>
  <!-- 顶部 caption（桌面端）：横贯整窗，作为窗口拖拽区 + 窗口控制按钮 + 功能名。
       整条浅灰，与下方三列的 rail/chat 浅灰连成一体（list 白底除外）。
       辅助窗口只多一个功能名（`title`），其余与主窗口完全同一套。 -->
  <div
    v-if="!isMobile"
    class="flex shrink-0 select-none items-center bg-[var(--gosslan-caption)]"
    :class="isMac ? 'justify-start pl-[13px]' : 'justify-end'"
    :style="{ height: 'var(--gosslan-title-h)' }"
    @mousedown="onDragStart"
    @dblclick="onTitlebarDblClick"
  >
    <!-- macOS：红绿灯。按**原生度量与状态**绘制（用户 2026-09-12 反馈：
         「mac 自己写的三个关闭按钮，和原生的关闭按钮的样式好像不太一样，还有大小」）：
         · 尺寸/间距：直径 **12px**、间距 **8px**（圆心相距 20px）、距左 **13px** —— 与系统一致；
         · 颜色与描边取系统取值（#ff5f57/#febc2e/#28c840 + 各自的深色 1px 内描边），
           而不是笼统的 `border-black/15`（那会让三个球的重量看起来不匀）；
         · **失焦整组变灰**（原生行为）：主窗口与设置窗口并排时，这一条差异最显眼；
         · 符号只在"聚焦 + 悬停整组"时出现，且用与原生一致的细线造型（× / − / 对角三角），
           不再用 lucide 的箭头（`Maximize2` 那种双箭头与系统造型不同）。
         组上加 @dblclick.stop：双击红绿灯不应冒泡成"双击标题栏"触发缩放。
         ⚠️ 绿灯在「不可最大化」的窗口（设置/日志/群任务）里**不渲染**。 -->
    <div v-if="isMac" class="group/traffic flex h-full items-center" style="gap: 8px" @dblclick.stop>
      <button data-no-drag-region @mousedown.stop
        class="traffic-hit flex h-3 w-3 items-center justify-center rounded-full transition-colors"
        :class="focused
          ? 'bg-[#ff5f57] shadow-[inset_0_0_0_1px_#e0443e]'
          : 'bg-[#d4d4d4] dark:bg-[#575757]'"
        :title="t('window.close')" :aria-label="t('window.close')"
        @click="api.windowClose()"
      >
        <svg
          v-if="focused"
          viewBox="0 0 12 12"
          class="hover-reveal-op h-3 w-3 opacity-0 transition-opacity group-hover/traffic:opacity-100"
          fill="none" stroke="#4d0000" stroke-width="1.3" stroke-linecap="round"
        >
          <path d="M4.2 4.2l3.6 3.6M7.8 4.2L4.2 7.8" />
        </svg>
      </button>
      <button data-no-drag-region @mousedown.stop
        class="traffic-hit flex h-3 w-3 items-center justify-center rounded-full transition-colors"
        :class="focused
          ? 'bg-[#febc2e] shadow-[inset_0_0_0_1px_#dea123]'
          : 'bg-[#d4d4d4] dark:bg-[#575757]'"
        :title="t('window.minimize')" :aria-label="t('window.minimize')"
        @click="api.windowMinimize()"
      >
        <svg
          v-if="focused"
          viewBox="0 0 12 12"
          class="hover-reveal-op h-3 w-3 opacity-0 transition-opacity group-hover/traffic:opacity-100"
          fill="none" stroke="#5a3a00" stroke-width="1.3" stroke-linecap="round"
        >
          <path d="M4 6h4" />
        </svg>
      </button>
      <button data-no-drag-region @mousedown.stop
        v-if="showMaximize"
        class="traffic-hit flex h-3 w-3 items-center justify-center rounded-full transition-colors"
        :class="focused
          ? 'bg-[#28c840] shadow-[inset_0_0_0_1px_#1aab29]'
          : 'bg-[#d4d4d4] dark:bg-[#575757]'"
        :title="fullscreen ? t('window.fullscreenExit') : maximized ? t('window.restore') : t('window.zoom')"
        :aria-label="fullscreen ? t('window.fullscreenExit') : maximized ? t('window.restore') : t('window.zoom')"
        @click="onGreenClick"
      >
        <!-- 原生造型：非全屏时是两个**对角三角**（进出全屏）；全屏/最大化时是收拢的三角。
             用自绘路径而不是 lucide 箭头，是为了和系统观感一致。 -->
        <svg
          v-if="focused"
          viewBox="0 0 12 12"
          class="hover-reveal-op h-3 w-3 opacity-0 transition-opacity group-hover/traffic:opacity-100"
          fill="#0b3d0b"
        >
          <template v-if="fullscreen || maximized">
            <path d="M3.2 3.2h3v3z" />
            <path d="M8.8 8.8h-3v-3z" />
          </template>
          <template v-else>
            <path d="M3.2 3.2h3.4L3.2 6.6z" />
            <path d="M8.8 8.8H5.4l3.4-3.4z" />
          </template>
        </svg>
      </button>
    </div>

    <!-- 功能名：辅助窗口传 `title`；主窗口不传 ⇒ 空占位（把 Win/Linux 的按钮推到最右）。
         拖拽区本身不接收点击，这里是纯文字，不影响拖拽。 -->
    <div
      class="min-w-0 flex-1 truncate px-3 text-[13px] font-medium text-[var(--gosslan-text-2)]"
      :title="title || undefined"
    >
      {{ title }}
    </div>

    <!-- Windows/Linux：右侧三键（顶到窗口最右缘）。
         字形按 **Windows 11 Fluent** 的细线造型自绘（10×10 视觉盒、1px 线）——
         lucide 的 `Maximize2` 是双箭头、`Minus` 偏粗，与系统窗口按钮明显不像
         （用户 2026-09-17：「最大化和最小化的图标和系统长得不太一样」）。
         ⚠️ 这三个按钮**不要加圆角**：外框容器（窗口根节点）是圆角 + overflow-hidden，关闭键右上角
         由它裁——两者曲线重合，hover 底与窗口边界严丝合缝。若给按钮自己加 border-radius（试过 8px），
         按钮的圆角曲线与窗口边界曲线不重合，中间会夹出一条浅色月牙缝（看起来"没贴合"）。
         同理只加圆角不加宽度补偿也不行：那会让 hover 底与窗口边缘脱开。
         原生 Windows 的窗口按钮 hover 也是整块矩形、由窗口圆角裁切。 -->
    <div v-if="!isMac" class="flex h-full items-stretch">
      <button data-no-drag-region @mousedown.stop
        v-if="showMinimize"
        class="flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
        :title="t('window.minimize')" :aria-label="t('window.minimize')"
        @click="api.windowMinimize()"
      >
        <svg viewBox="0 0 10 10" class="h-[10px] w-[10px]" fill="none" stroke="currentColor" stroke-width="1">
          <path d="M0.5 5h9" data-win-glyph="minimize" />
        </svg>
      </button>
      <button data-no-drag-region @mousedown.stop
        v-if="showMaximize"
        class="flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
        :title="maximized ? t('window.restore') : t('window.maximize')" :aria-label="maximized ? t('window.restore') : t('window.maximize')"
        @click="toggleMaximize"
      >
        <svg viewBox="0 0 10 10" class="h-[10px] w-[10px]" fill="none" stroke="currentColor" stroke-width="1">
          <!-- 最大化 = 一个方框；还原 = 前后两个错位方框（系统同款造型） -->
          <rect
            v-if="!maximized"
            x="0.75" y="0.75" width="8.5" height="8.5"
            data-win-glyph="maximize"
          />
          <template v-else>
            <path d="M3.25 0.75h6v6" data-win-glyph="restore-back" />
            <rect x="0.75" y="2.75" width="6.5" height="6.5" data-win-glyph="restore" />
          </template>
        </svg>
      </button>
      <button data-no-drag-region @mousedown.stop
        class="flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-danger)] hover:text-white"
        :title="closeToTray ? t('window.closeToTray') : t('window.close')"
        :aria-label="closeToTray ? t('window.closeToTray') : t('window.close')"
        @click="api.windowClose()"
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>
  </div>
</template>
