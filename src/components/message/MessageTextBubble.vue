<script setup lang="ts">
import { t } from "@/i18n";
import { computed, type CSSProperties } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAppStore } from "@/stores/useAppStore";
import { PREVIEW_LINES } from "@/utils/previewMetrics";
import { linkify, displayUrl, type LinkSegment } from "@/utils/linkify";
import { splitEmoji } from "@/utils/emoji";
import { mentionHighlightColor } from "@/utils/chatStyle";
import { QUOTE_BORDER, QUOTE_BG, QUOTE_TEXT_STYLE } from "@/utils/quoteStyle";
import { Check, Copy } from "lucide-vue-next";

const props = defineProps<{
  content: string;
  bubbleStyle: CSSProperties;
  /** 超过预览行数：正文截断为固定行数，「展开显示」走独立 Modal。 */
  clamped: boolean;
  copied: boolean;
  /** 自己的消息气泡尖角朝右、对方朝左，指向头像。 */
  mine: boolean;
  /** 群成员名列表：正文里的 @name 按此高亮（不传不高亮）。 */
  mentionNames?: string[];
}>();
const emit = defineEmits<{
  (e: "expand", content: string): void;
  (e: "copy", content: string): void;
  (e: "locate", msgId: string): void;
}>();

const app = useAppStore();

/** 截断行数由 PREVIEW_LINES 驱动，避免 Tailwind 类名与估算常量各写一份。 */
const clampStyle = computed<CSSProperties>(() =>
  props.clamped
    ? {
        display: "-webkit-box",
        WebkitBoxOrient: "vertical",
        WebkitLineClamp: PREVIEW_LINES,
        overflow: "hidden",
      }
    : {},
);
const bubbleStyle = computed<CSSProperties>(() => ({
  ...props.bubbleStyle,
  fontSize: "var(--gosslan-msg-size, 14px)",
}));

// ---------------- 引用解析 ----------------
// 引用消息的 content 首行为 `「引用 {sender}：{snippet}|{msg_id}」`（msg_id 可省略），
// 其余为正文。渲染成引用块（左侧竖线 + 灰字），带 msg_id 时可点击跳转原消息；
// 复制/转发仍是完整原文。
const QUOTE_PREFIX = "「引用 ";

const parsed = computed(() => {
  if (!props.content.startsWith(QUOTE_PREFIX)) return { quote: "", body: props.content, msgId: "" };
  const nl = props.content.indexOf("\n");
  if (nl < 0 || !props.content.slice(0, nl).trimEnd().endsWith("」")) {
    return { quote: "", body: props.content, msgId: "" };
  }
  let line = props.content.slice(0, nl).trimEnd();
  let msgId = "";
  const m = line.match(/\|([^\s|」]+)」$/);
  if (m) {
    msgId = m[1];
    line = line.slice(0, m.index) + "」";
  }
  return { quote: line, body: props.content.slice(nl + 1), msgId };
});

/** 渲染段：text / link / mention / emoji，按段渲染（不拼 HTML，天然防 XSS）。 */
type RenderSegment =
  | LinkSegment
  | { kind: "emoji"; value: string; name: string; url: string };

/** 正文先按表情 token 切段，文本段再交给 linkify 切链接/提及。 */
const segments = computed<RenderSegment[]>(() => {
  const out: RenderSegment[] = [];
  for (const s of splitEmoji(parsed.value.body)) {
    if (s.kind === "emoji") {
      out.push({ kind: "emoji", value: s.value, name: s.name, url: s.url });
    } else {
      out.push(...linkify(s.value, props.mentionNames ?? []));
    }
  }
  return out;
});

/**
 * @提及 高亮文字色（微信式蓝字）：按主题色派生，并以当前气泡的实际底色
 * （bubbleStyle.background）校验对比 ≥4.5——预设差异被天然覆盖。
 * 不直接用主题色：text-primary 在浅蓝气泡上对比只有 2.94（历史坑）。
 */
const mentionFg = computed(() => {
  const bg = typeof props.bubbleStyle.background === "string" ? props.bubbleStyle.background : "";
  return mentionHighlightColor(app.themeColor, app.dark, bg || "#ffffff");
});

/** @提及 淡背景：取 mentionFg（主题色派生）的低透明度，做成互联网公司式的浅色块。 */
const mentionBg = computed(() => {
  const fg = mentionFg.value;
  return fg ? `color-mix(in srgb, ${fg} 16%, transparent)` : undefined;
});

/** 点击链接：调 Tauri opener 走系统默认浏览器；失败 toast 提示。 */
async function openLink(href: string) {
  try {
    await openUrl(href);
  } catch (e) {
    app.toastError(e, t("msg.openLinkFail"));
  }
}
</script>

<template>
  <!-- 气泡排版（用户 2026-09-12 反馈：「气泡高度太高了，不如微信里和谐；字重又太细了，
       一眼看上去不够清晰」）：
       - 纵向内边距 py-1.5(12px 合计) 与 leading-normal(行高 1.5)：更紧凑、更接近微信；
       - `font-medium`(500)：比默认 400 更清晰，又不至于到 600 显得"加粗标题"。
       ⚠️ 这三个值都被 `utils/previewMetrics.ts` 的 `TEXT_LINE_RATIO` / `TEXT_BUBBLE_PADDING`
       镜像用于虚拟列表高度估算 —— **改这里必须同步改那里**，否则相邻消息会互相遮挡
       （该文件顶部写明了这条契约）。 -->
  <div
    class="group relative min-w-0 px-3 py-1.5 font-medium leading-normal"
    :style="bubbleStyle"
  >
    <!-- 引用块：首行「引用 发送者：片段」，带 msg_id 时可点击跳转原消息 -->
    <button
      v-if="parsed.quote && parsed.msgId"
      class="quote-block mb-1.5 block w-full cursor-pointer rounded-[var(--gosslan-radius-sm)] border-l-2 px-2 py-1 text-left text-[12px] leading-4 transition hover:brightness-110"
      :style="{ borderColor: QUOTE_BORDER, background: QUOTE_BG }"
      :title="t('msg.locateOriginal', { id: parsed.msgId })"
      @click="emit('locate', parsed.msgId)"
    >
      <span class="quote-text" :style="QUOTE_TEXT_STYLE">{{ parsed.quote }}</span>
    </button>
    <div
      v-else-if="parsed.quote"
      class="quote-block mb-1.5 rounded-[var(--gosslan-radius-sm)] border-l-2 px-2 py-1 text-[12px] leading-4"
      :style="{ borderColor: QUOTE_BORDER, background: QUOTE_BG }"
    >
      <span class="quote-text" :style="QUOTE_TEXT_STYLE">{{ parsed.quote }}</span>
    </div>
    <div
      class="gosslan-selectable whitespace-pre-wrap break-words"
      :style="{ wordBreak: 'break-word', ...clampStyle }"
    >
      <template v-for="(seg, i) in segments" :key="i">
        <a
          v-if="seg.kind === 'link'"
          class="cursor-pointer break-all underline decoration-1 underline-offset-2 transition hover:opacity-80"
          :title="seg.href"
          @click.stop.prevent="openLink(seg.href)"
        >{{ displayUrl(seg.value) }}</a>
        <img
          v-else-if="seg.kind === 'emoji'"
          :src="seg.url"
          :alt="seg.value"
          :title="seg.value"
          class="emoji-img"
        />
        <span
          v-else-if="seg.kind === 'mention'"
          class="mention-token"
          :style="{ color: mentionFg || undefined, background: mentionBg || undefined }"
        >{{ seg.value }}</span>
        <span v-else>{{ seg.value }}</span>
      </template>
    </div>
    <!-- 长文本操作条：高度固定，展开走独立 Modal，消息 DOM 不再变化 -->
    <div
      v-if="clamped"
      class="mt-1.5 flex items-center gap-2 border-t pt-1.5"
      :style="{ borderColor: 'rgba(128,128,128,0.2)' }"
    >
      <button class="tap-safe text-xs opacity-70 transition hover:opacity-100" @click="emit('expand', content)">
        {{ t("common.expand") }}
      </button>
      <button
        class="tap-safe flex items-center gap-1 whitespace-nowrap text-xs transition"
        :class="copied ? 'opacity-100' : 'opacity-70 hover:opacity-100'"
        @click="emit('copy', content)"
      >
        <Check v-if="copied" class="h-3 w-3" />
        <Copy v-else class="h-3 w-3" />
        {{ copied ? t("common.copied") : t("common.copy") }}
      </button>
    </div>
    <!-- 普通文本：不显示悬停复制气泡（复制走右键菜单），避免干扰 -->
    <span aria-hidden="true" class="bubble-tail" :class="mine ? 'tail-mine' : 'tail-other'"></span>
  </div>
</template>
