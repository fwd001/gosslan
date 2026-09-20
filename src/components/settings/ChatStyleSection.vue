<script setup lang="ts">
import { computed, ref } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import SettingsGroup from "@/components/settings/SettingsGroup.vue";
import { CHAT_FONT_SIZES, CHAT_PRESETS, fontPx, resolveChatColors, type ChatPreset } from "@/utils/chatStyle";
import { t } from "@/i18n";

const app = useAppStore();

/** 当前字号在 CHAT_FONT_SIZES 里的索引（0..4）。 */
const fontIndex = computed(() => CHAT_FONT_SIZES.findIndex((f) => f.key === app.chatStyle.fontSize));

/** thumb 绝对定位 left 值，百分比。5 档时 0/25/50/75/100%。 */
const thumbPct = computed(() => {
  const maxIdx = CHAT_FONT_SIZES.length - 1;
  return `${(fontIndex.value / maxIdx) * 100}%`;
});

/** 把索引映射回 key 存到 store。 */
function applyFontIndex(idx: number) {
  const clamped = Math.max(0, Math.min(CHAT_FONT_SIZES.length - 1, idx));
  const key = CHAT_FONT_SIZES[clamped]?.key ?? "md";
  app.setChatStyle({ fontSize: key });
}

/** 点击/拖拽共用：把鼠标 x 坐标转成档位索引。 */
const sliderRef = ref<HTMLElement | null>(null);
function xToIndex(clientX: number) {
  const el = sliderRef.value;
  if (!el) return 0;
  const rect = el.getBoundingClientRect();
  const x = clientX - rect.left;
  const pct = Math.max(0, Math.min(1, x / rect.width));
  return Math.round(pct * (CHAT_FONT_SIZES.length - 1));
}

/** 拖拽支持：pointerdown 开始 → pointermove 跟随 → pointerup 结束。
 *
 * 为什么用 Pointer Events（而非 mouse/touch 分别处理）：
 *   一套事件覆盖三种输入——桌面鼠标 / 手机触摸 / 触控笔。
 *   浏览器会自动把 touch 翻译成 pointer 事件，不需要写 @touchstart/@touchmove。
 *
 * 移动端防坑：
 *   1) CSS `touch-action: none` —— 告诉浏览器"这个区域不要拦截 touch 做页面滚动"
 *   2) pointerdown 里 preventDefault —— 防止触摸触发 text selection / hover
 *   3) setPointerCapture —— 手指滑出滑块范围仍能收到 move/up
 */
let dragging = false;
function onSliderPointerDown(e: PointerEvent) {
  e.preventDefault(); // 阻止触摸触发文字选中 / hover
  dragging = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  applyFontIndex(xToIndex(e.clientX));
}
function onSliderPointerMove(e: PointerEvent) {
  if (!dragging) return;
  e.preventDefault(); // 防止 touchmove 被当成页面滚动
  applyFontIndex(xToIndex(e.clientX));
}
function onSliderPointerUp(e: PointerEvent) {
  dragging = false;
  try {
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  } catch {
    /* 某些 Android WebView 在 releasePointerCapture 抛错，忽略即可 */
  }
}

/** 键盘 ←/→ 步进一档。 */
function stepFont(delta: -1 | 1) {
  applyFontIndex(fontIndex.value + delta);
}

/** 当前字号的 px 值（供预览气泡实时用 inline style）。 */
const currentFontPx = computed(() => fontPx(app.chatStyle.fontSize));

/** 预览色："theme" 预设按当前主题色实时派生，其余取表明暗值。 */
function swatchOf(p: ChatPreset): { mineBubble: string; otherBubble: string } {
  const c = resolveChatColors(p.key, app.themeColor, app.dark);
  return { mineBubble: c.mineBubble, otherBubble: c.otherBubble };
}

/** 预览气泡用当前配色 + 当前字号（不跟随 clamped —— 预览里的短文本就是普通气泡）。 */
const previewMineStyle = computed(() => {
  const c = resolveChatColors(app.chatStyle.preset, app.themeColor, app.dark);
  return {
    background: c.mineBubble,
    color: c.mineText,
    fontSize: `${currentFontPx.value}px`,
    borderRadius: "var(--gosslan-bubble-radius, 4px)",
    border: "1px solid transparent",
    padding: "6px 10px",
    lineHeight: "1.45",
    maxWidth: "75%",
    "--bubble-bg": c.mineBubble,
  };
});
const previewOtherStyle = computed(() => {
  const c = resolveChatColors(app.chatStyle.preset, app.themeColor, app.dark);
  return {
    background: c.otherBubble,
    color: c.otherText,
    fontSize: `${currentFontPx.value}px`,
    borderRadius: "var(--gosslan-bubble-radius, 4px)",
    border: "1px solid var(--gosslan-border)",
    padding: "6px 10px",
    lineHeight: "1.45",
    maxWidth: "75%",
    "--bubble-bg": c.otherBubble,
  };
});
</script>

<template>
  <SettingsGroup
    :title="t('settings.group.chatStyle')"
    :footer="t('settings.group.chatStyle.footer')"
  >
    <!-- 字体大小：预览气泡 + 滑块（仿微信"字体"设置）。
         上面摆一对方向相反的预览气泡（你/对方），字号实时跟随滑块变；
         下面一个 5 档离散滑块（step=1，min=0，max=4），两端 A 字标小/大。 -->
    <div class="px-4 py-3">
      <div class="mb-2 flex items-baseline justify-between">
        <div class="text-sm text-[var(--gosslan-text)]">{{ t("settings.chatStyle.fontSize") }}</div>
        <div class="text-[11px] text-[var(--gosslan-text-2)]">
          {{ currentFontPx }}px · {{ t(CHAT_FONT_SIZES[fontIndex]?.label ?? "chatStyle.fontSize.md") }}
        </div>
      </div>

      <!-- 预览气泡（两个反方向摆）。尖角位置和真实聊天一致——你发的右对齐、尖角朝右；对方左对齐、尖角朝左。
           但我们复用真实气泡的样式：background/color/fontSize 全部从当前主题实时算。 -->
      <div
        class="mb-3 flex flex-col gap-2 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] px-3 py-3"
      >
        <!-- 对方气泡（左对齐）。 -->
        <div class="flex">
          <span class="relative inline-block" :style="previewOtherStyle">
            {{ t("settings.chatStyle.fontSize.preview") }}
          </span>
        </div>
        <!-- 我的气泡（右对齐）。 -->
        <div class="flex justify-end">
          <span class="relative inline-block" :style="previewMineStyle">
            {{ t("settings.chatStyle.fontSize.preview") }}
          </span>
        </div>
      </div>

      <!-- 离散字号滑块（自实现，不依赖原生 range）。
           5 档段落式：一条 4px 灰条 + 5 个等间距刻度圆点 + 一个主题色 thumb。
           点击灰条任意位置 → 就近吸附到一档；thumb 根据当前档位绝对定位。
           比原生 range 靠谱：WebKit 把 track 设透明后经常塌掉，自己写完全可控。 -->
      <div class="flex items-center gap-3">
        <span class="font-medium text-[var(--gosslan-text-2)]" style="font-size: 11px;">A</span>
        <div
          ref="sliderRef"
          class="font-size-slider"
          @pointerdown="onSliderPointerDown"
          @pointermove="onSliderPointerMove"
          @pointerup="onSliderPointerUp"
          @pointercancel="onSliderPointerUp"
          @keydown.left="stepFont(-1)"
          @keydown.right="stepFont(1)"
          role="slider"
          :aria-valuemin="0"
          :aria-valuemax="CHAT_FONT_SIZES.length - 1"
          :aria-valuenow="fontIndex"
          tabindex="0"
        >
          <!-- 轨道：灰条 + 5 个刻度圆点（段落感）。 -->
          <div class="font-size-slider-track">
            <span
              v-for="i in CHAT_FONT_SIZES.length"
              :key="i"
              class="font-size-slider-dot"
              :class="{ active: i - 1 === fontIndex }"
            />
          </div>
          <!-- thumb：主题色实心圆，绝对定位。位置 = 当前档位 / (总档数-1) * 100%。 -->
          <div
            class="font-size-slider-thumb"
            :style="{ left: thumbPct }"
          />
        </div>
        <span class="font-medium text-[var(--gosslan-text-2)]" style="font-size: 18px;">A</span>
      </div>
    </div>

    <div class="ml-4 h-px bg-[var(--gosslan-divider)]" />

    <!-- 气泡配色 -->
    <div class="px-4 py-3">
      <div class="mb-2 text-sm text-[var(--gosslan-text)]">{{ t("settings.chatStyle.bubble") }}</div>
      <div class="grid grid-cols-3 gap-2">
        <button
          v-for="p in CHAT_PRESETS"
          :key="p.key"
          class="rounded-[var(--gosslan-radius-md)] border p-2 transition hover:bg-[var(--gosslan-hover)]"
          :class="app.chatStyle.preset === p.key ? 'border-[var(--gosslan-primary)] ring-1 ring-[var(--gosslan-primary-ring)]' : 'border-[var(--gosslan-border)]'"
          :title="t(p.label)"
          :aria-pressed="app.chatStyle.preset === p.key"
          @click="app.setChatStyle({ preset: p.key })"
        >
          <div class="mb-1 text-center text-[11px] text-[var(--gosslan-text-2)]">{{ t(p.label) }}</div>
          <div class="flex items-center gap-1">
            <span class="h-4 flex-1 rounded-[var(--gosslan-radius-xs)]" :style="{ background: swatchOf(p).mineBubble }"></span>
            <span
              class="h-4 flex-1 rounded-[var(--gosslan-radius-xs)] border border-[var(--gosslan-border)]"
              :style="{ background: swatchOf(p).otherBubble }"
            ></span>
          </div>
        </button>
      </div>
    </div>
  </SettingsGroup>
</template>

<style scoped>
/**
 * 自实现离散字号滑块（5 档段落式，微信同款）。
 *
 * 为什么不用原生 input[type=range]：
 *   WebKit 把 track 设透明后经常塌成 0 高，或 thumb 定位错位。
 *   自实现只有 15 行 CSS + 简单的点击/键盘逻辑，完全可控。
 *
 * 结构：
 *   ┌── .font-size-slider (flex-1, relative) ──┐
 *   │  .font-size-slider-track (absolute)       │
 *   │    └── .font-size-slider-dot × 5 (flex)   │
 *   │  .font-size-slider-thumb (absolute)       │
 *   └───────────────────────────────────────────┘
 */

/* 滑块容器：relative + 垂直居中。
   左右 padding = thumb 半径（8px），让 thumb 在 0% / 100% 时正好贴齐容器边缘。
   touch-action: none —— 移动端触摸不触发页面滚动 / 缩放 / 点击高亮。 */
.font-size-slider {
  position: relative;
  flex: 1;
  height: 24px;
  display: flex;
  align-items: center;
  cursor: pointer;
  outline: none;
  padding: 0 8px;
  touch-action: none;
  -webkit-user-select: none;
  user-select: none;
}
.font-size-slider:focus-visible {
  border-radius: 4px;
  box-shadow: 0 0 0 2px var(--gosslan-primary-ring);
}

/* 轨道：一条 4px 圆角灰条，绝对铺满容器中心。 */
.font-size-slider-track {
  position: absolute;
  left: 0;
  right: 0;
  height: 4px;
  border-radius: 2px;
  background: var(--gosslan-border);
  /* 5 个刻度圆点等间距分布。 */
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0 2px; /* 首尾点留余量不贴边 */
  z-index: 0;
}
.font-size-slider-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  /* 用 color-mix 往黑色方向压 40% —— 比 border 深一档，和轨道（#e2e8f0）有明显对比。
     不用 transparent 混：transparent 在某些浏览器上会让 color-mix 输出半透明色，
     半透明圆叠在同色轨道上=看不见。直接混 black 出实色。 */
  background: color-mix(in srgb, var(--gosslan-border) 60%, black 40%);
  flex-shrink: 0;
  transition: background 120ms;
}
/* 选中档位对应的刻度点用主题色填充。 */
.font-size-slider-dot.active {
  background: var(--gosslan-primary);
}

/* thumb：主题色实心圆，绝对定位。left 由 JS 计算（0 / 25 / 50 / 75 / 100%）。
   用 translateX(-50%) 让 thumb 中心对齐百分比位置。 */
.font-size-slider-thumb {
  position: absolute;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--gosslan-primary);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
  transform: translateX(-50%);
  transition: left 160ms ease, transform 120ms;
  z-index: 1;
}
.font-size-slider:hover .font-size-slider-thumb {
  transform: translateX(-50%) scale(1.1);
}
.font-size-slider:active .font-size-slider-thumb {
  transform: translateX(-50%) scale(1.15);
}
</style>
