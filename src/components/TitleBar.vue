<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAppStore } from "@/stores/useAppStore";
import { api } from "@/api";
import { Minus, X } from "lucide-vue-next";
import { isMac } from "@/utils/platform";
import { t } from "@/i18n";

const app = useAppStore();
const maximized = ref(false);

async function refreshMaximized() {
  try {
    maximized.value = await api.windowIsMaximized();
  } catch {
    maximized.value = false; // 移动端 / 命令不可用时忽略
  }
}

async function toggleMaximize() {
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
 * 若要完全跟随用户偏好，需改为 `decorations + titleBarStyle: Overlay` 交给系统，属后续项。
 */
function onTitlebarDblClick() {
  if (app.isMobile || !isMac) return;
  void toggleMaximize();
}

/**
 * Ctrl+W 关闭窗口（仅 Windows/Linux）：与标题栏「×」一致，最小化到托盘。
 *
 * macOS **不在这里处理**：自绘标题栏 + `decorations:false` 下本无系统 ⌘W，
 * 现已由 src-tauri/src/menu.rs 补回原生「窗口 → 关闭」菜单（lib.rs 亦恢复了
 * NSWindow 的 `Closable` 位），交给系统处理即可，避免前端兜底与原生菜单双触发。
 */
function onKeydown(e: KeyboardEvent) {
  if (app.isMobile) return;
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
  <!-- 顶部 caption（桌面端）：横贯整窗，作为窗口拖拽区 + 窗口控制按钮。
       整条浅灰，与下方三列的 rail/chat 浅灰连成一体（list 白底除外）。 -->
  <div
    v-if="!app.isMobile"
    data-tauri-drag-region
    class="flex shrink-0 select-none items-center bg-[var(--gosslan-caption)]"
    :class="isMac ? 'justify-start pl-[13px]' : 'justify-end'"
    :style="{ height: 'var(--gosslan-title-h)' }"
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
         组上加 @dblclick.stop：双击红绿灯不应冒泡成"双击标题栏"触发缩放。 -->
    <div v-if="isMac" class="group/traffic flex h-full items-center" style="gap: 8px" @dblclick.stop>
      <button
        class="flex h-3 w-3 items-center justify-center rounded-full transition-colors"
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
      <button
        class="flex h-3 w-3 items-center justify-center rounded-full transition-colors"
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
      <button
        class="flex h-3 w-3 items-center justify-center rounded-full transition-colors"
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

    <!-- Windows/Linux：右侧三键（顶到窗口最右缘）。
         ⚠️ 这三个按钮**不要加圆角**：外框容器（ResponsiveLayout 根节点）是
         `rounded-[var(--gosslan-radius-lg)] + overflow-hidden`，关闭键右上角由它裁——两者曲线重合，hover 底
         与窗口边界严丝合缝。若给按钮自己加 border-radius（试过 8px），按钮的圆角曲线
         与窗口边界曲线不重合，中间会夹出一条浅色月牙缝（看起来"没贴合"）。
         同理只加圆角不加宽度补偿也不行：那会让 hover 底与窗口边缘脱开。
         原生 Windows 的窗口按钮 hover 也是整块矩形、由窗口圆角裁切。 -->
    <div v-else class="flex h-full items-stretch">
      <button
        class="flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
        :title="t('window.minimize')" :aria-label="t('window.minimize')"
        @click="api.windowMinimize()"
      >
        <Minus class="h-3.5 w-3.5" />
      </button>
      <button
        class="flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-hover)] hover:text-[var(--gosslan-text)]"
        :title="maximized ? t('window.restore') : t('window.maximize')" :aria-label="maximized ? t('window.restore') : t('window.maximize')"
        @click="toggleMaximize"
      >
        <Minimize2 v-if="maximized" class="h-3 w-3" />
        <Maximize2 v-else class="h-3 w-3" />
      </button>
      <button
        class="flex w-11 items-center justify-center text-[var(--gosslan-rail-text)] transition hover:bg-[var(--gosslan-danger)] hover:text-white"
        :title="t('window.closeToTray')" :aria-label="t('window.closeToTray')"
        @click="api.windowClose()"
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>
  </div>
</template>
