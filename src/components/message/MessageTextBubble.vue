<script setup lang="ts">
import { t } from "@/i18n";
import { computed, nextTick, ref, watch, type CSSProperties } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAppStore } from "@/stores/useAppStore";
import { PREVIEW_LINES } from "@/utils/previewMetrics";
import { linkify, type LinkSegment } from "@/utils/linkify";
import MessageLinkText from "@/components/message/MessageLinkText.vue";
import { splitEmoji } from "@/utils/emoji";
import { mentionHighlightColor } from "@/utils/chatStyle";
import { parseQuote } from "@/utils/quote";
import { Check, Copy } from "lucide-vue-next";

const props = defineProps<{
  content: string;
  bubbleStyle: CSSProperties;
  cardStyle: CSSProperties;
  /** 超过预览行数：正文截断为固定行数，「展开显示」走独立 Modal。 */
  clamped: boolean;
  copied: boolean;
  /** 自己的消息气泡尖角朝右、对方朝左，指向头像。 */
  mine: boolean;
  /** 群成员名列表：正文里的 @name 按此高亮（不传不高亮）。 */
  mentionNames?: string[];
  /**
   * 查看者自己是谁（本机昵称 + 本地化标签）。传了以后，正文里 @到我自己 的那一段
   * 渲染成「@你」并加重（用户 2026-09-26）。刻意由调用方传而不是在这里读 store ——
   * 本组件是纯展示件（与 `mine` 同理），且**同一份文本在两端渲染成不同标签**，
   * 绝不能在发送/落库侧改文案（那会污染对端视图与历史）。
   */
  selfMention?: { name: string; label: string } | null;
  /**
   * 移动端「选择文字」模式（用户 2026-09-13）：
   * 触屏下气泡默认**不可选**（长按归消息菜单），只有进入这个模式才开放原生选字。
   * 见 style.css 里 `@media (pointer: coarse)` 的说明。
   */
  selectMode?: boolean;
}>();
const emit = defineEmits<{
  (e: "expand", content: string): void;
  (e: "copy", content: string): void;
  (e: "locate", msgId: string): void;
}>();

/**
 * 进入「选择文字」模式时**自动选中整条正文**：系统那条「复制/全选」工具条只有存在选区时
 * 才会弹出来 —— 不自动选的话用户会以为"点了没反应"（他只看到气泡变了下样子）。
 * 随后用户可以拖手柄调整范围（系统的选区手柄就是为此存在的）。
 */
const contentEl = ref<HTMLElement | null>(null);
watch(
  () => props.selectMode,
  async (on) => {
    if (!on) return;
    await nextTick();
    const el = contentEl.value;
    const sel = window.getSelection();
    if (!el || !sel) return;
    const range = document.createRange();
    range.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(range);
  },
);

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
const finalStyle = computed<CSSProperties>(() => ({
  ...(props.clamped ? props.cardStyle : props.bubbleStyle),
  fontSize: "var(--gosslan-msg-size, 14px)",
}));

// ---------------- 引用解析 ----------------
// 引用消息渲染成引用块（左侧竖线 + 灰字），带 msg_id 时可点击跳转原消息。
// 解析规则统一在 `utils/quote.ts` —— 截断判定、高度估算、复制路径都在用同一份，
// 在这里再写一遍迟早会分叉（历史上「正文正好 5 行却多出展开条」就是这么来的）。
const parsed = computed(() => {
  const q = parseQuote(props.content);
  return { quote: q.header, body: q.body, msgId: q.msgId };
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
      out.push(...linkify(s.value, props.mentionNames ?? [], props.selfMention ?? undefined));
    }
  }
  return out;
});

/**
 * @提及 高亮文字色（微信式蓝字）：按主题色派生，并以当前气泡的实际底色
 * 校验对比 ≥4.5——预设差异被天然覆盖。
 * 不直接用主题色：text-primary 在浅蓝气泡上对比只有 2.94（历史坑）。
 *
 * ⚠️ 气泡底色是 CSS 变量字符串（"var(--bubble-bg)"），contrastRatio 算不了——
 * 必须先 resolve 成真实色：bubbleStyle 里 --bubble-bg 是 inline 设置的真实 hex
 * （不是 CSS var），getComputedStyle 能读出来。
 */
function resolveBgColor(style: CSSProperties | undefined, el: HTMLElement | null): string {
  if (!style) return "";
  // 优先读我们塞进去的 --bubble-bg-raw（真实 hex，bubbleStyle 有，cardStyle 没有）
  const raw = (style as Record<string, unknown>)["--bubble-bg-raw"];
  if (typeof raw === "string" && raw.startsWith("#")) return raw;
  // fallback：从 DOM 上解析 --bubble-bg（也是 inline 设置的真实 hex）
  if (el) {
    const cs = getComputedStyle(el);
    const v = cs.getPropertyValue("--bubble-bg").trim();
    if (v && v.startsWith("#")) return v;
  }
  return "";
}
const mentionFg = computed(() => {
  const bg = resolveBgColor(props.clamped ? props.cardStyle : props.bubbleStyle, contentEl.value);
  return mentionHighlightColor(app.themeColor, app.dark, bg);
});

/** @提及 淡背景：取 mentionFg（主题色派生）的低透明度，做成互联网公司式的浅色块。 */
const mentionBg = computed(() => {
  const fg = mentionFg.value;
  return fg ? `color-mix(in srgb, ${fg} 16%, transparent)` : undefined;
});

/** 点击链接：调 Tauri opener 走系统默认浏览器；失败 toast 提示。 */
async function openLink(href: string) {
  /**
   * 「选择文字」模式下点链接**不打开**：那一模式下手指落在正文上是在挪选区/定位，
   * 而 `<a>` 的 click 照旧会合成 —— 拉系统浏览器等于把用户正在调的选区打断。
   * （`swallowLongPressRelease` 只吞"弹面板那一次"抬手，管不到进入选择模式之后的 tap。）
   * 这一下点击不白费：它会把选区收起来，于是自动退出选择模式，再点就是正常打开。
   */
  if (props.selectMode) return;
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
  <!-- ⚠️ 气泡根也 `select-text`（用户 2026-09-13：「鼠标去划选不中，刚选中立马取消」）：
       正文被 `px-3 py-1.5` 的内边距包着，从内边距或气泡边缘起拖时选区锚点落在
       "不可选"区域上，WebKit 会立刻把选区收敛掉 —— 表现就是"刚选中就没了"。
       刻意**不用** `.gosslan-selectable` 这个类：它同时是"移动端长按让路给原生选字"的
       标记，标到气泡根上会让长按再也弹不出操作面板（气泡正是长按的主落点）。 -->
  <div
    class="group gosslan-bubble-text select-text relative min-w-0 px-3 py-1.5 font-medium leading-normal"
    :class="selectMode ? 'gosslan-selecting' : ''"
    :style="finalStyle"
  >
    <!-- ⚠️ 这一层只为了框住「引用块 + 正文」，让「选择文字」的全选范围=
         用户在这个气泡里看得见的内容。引用块与正文是兄弟节点，ref 挂在正文上时
         选区会漏掉引用头 —— 于是划选复制拿到纯正文、而操作条的「复制」给的是
         带引用头的完整原文，同一个气泡两条复制路径结果不一样。
         ⚠️ 不挂在气泡根上：根里还有「展开/复制」操作条和尖角，全选会把按钮文字也框进去。 -->
    <div ref="contentEl">
      <div
        class="gosslan-selectable whitespace-pre-wrap break-words"
        :style="{ wordBreak: 'break-word', ...clampStyle }"
      >
        <template v-for="(seg, i) in segments" :key="i">
          <MessageLinkText
            v-if="seg.kind === 'link'"
            :href="seg.href"
            :label="seg.value"
            @open="openLink"
          />
          <img
            v-else-if="seg.kind === 'emoji'"
            :src="seg.url"
            :alt="seg.value"
            :title="seg.value"
            draggable="false"
            class="emoji-img"
          />
          <span
            v-else-if="seg.kind === 'mention' || seg.kind === 'mention-self'"
            class="mention-token"
            :class="{ 'mention-token--self': seg.kind === 'mention-self' }"
            :style="{ color: mentionFg || undefined, background: mentionBg || undefined }"
          >{{ seg.value }}</span>
          <span v-else>{{ seg.value }}</span>
        </template>
      </div>
    </div>
    <!-- 引用块不再画在气泡里：按用户 2026-09-21 的微信口径，它挂在**气泡外的正文下方**
         （见 MessageItem 的说明：点击=直接查看被引用内容，跳转在消息菜单里）。 -->
    <!-- 长文本操作条：高度固定，展开走独立 Modal，消息 DOM 不再变化 -->
    <div
      v-if="clamped"
      class="mt-1.5 flex items-center gap-2 border-t pt-1.5"
      :style="{ borderColor: 'var(--gosslan-divider)' }"
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
