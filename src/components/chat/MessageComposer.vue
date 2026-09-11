<script setup lang="ts">
import { t } from "@/i18n";
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "@/stores/useAppStore";
import EmojiPicker from "@/components/EmojiPicker.vue";
import { QUOTE_BORDER, QUOTE_BG, QUOTE_TEXT_STYLE } from "@/utils/quoteStyle";
import { useExclusivePopup } from "@/composables/useExclusivePopup";
import { haptic } from "@/utils/haptics";
import { mentionHighlightColor, resolveChatColors } from "@/utils/chatStyle";
import { avatarInitial, nameToColor } from "@/utils/color";
import { classifyPaste } from "@/utils/clipboard";
import { isImeKey } from "@/utils/ime";
import { Folder, Smile, SquareCode, X } from "lucide-vue-next";
import type { MsgKind } from "@/types";

const props = defineProps<{
  /** 会话切换时聚焦输入框（切换会话 = 新会话，重置草稿由父组件卸载/挂载决定）。 */
  convId: string | null;
  /** 待引用消息（右键"引用"设置）；发送时拼进消息首行，显示为引用块。msgId 用于点击跳转原消息。 */
  quote?: { sender: string; snippet: string; msgId?: string | number } | null;
  /** 群成员（不含自己）：非空时输入 @ 弹出成员选择（群聊 @ 功能）。 */
  mentionMembers?: { id: string; name: string }[];
}>();
const emit = defineEmits<{
  (e: "send", payload: { content: string; kind: MsgKind }): void;
  (e: "send-image", dataUrl: string): void;
  (e: "attach"): void;
  (e: "close-quote"): void;
  (e: "paste-files", paths: string[]): void;
}>();

const app = useAppStore();
const codeMode = ref(false);

/** 输入框单条消息的字符硬上限：超过即截断。粘贴与发送两处都会兜底，
 *  防止粘贴超大文本时 contenteditable 塞进几十万字符、把界面卡死。 */
const MAX_INPUT_LENGTH = 50_000;
// ---------------- contenteditable 输入框（DOM 为源，uncontrolled） ----------------
// textarea 画不了局部颜色、overlay mirror 又会排版错位（已踩坑回退），改用
// contenteditable：@提及 是真正的内联原子 token（contenteditable=false 的 span，
// 退格整删、光标原生管理）。Vue 不控制 innerHTML（受控重渲染会毁光标/IME），
// 只在 input 事件里把「是否可发送」投影成响应式 hasDraft；发送时读 innerText。
const editorRef = ref<HTMLDivElement | null>(null);
const hasDraft = ref(false);

/** @提及 高亮文字色：输入框底是面板（亮白/暗深灰），按对方气泡同档中性底校验对比。 */
const composerMentionFg = computed(() =>
  mentionHighlightColor(
    app.themeColor,
    app.dark,
    resolveChatColors("theme", app.themeColor, app.dark).otherBubble,
  ),
);

/** 输入框自适应高度：内容换行时自动长高，超过 5 行（128px）出现滚动。 */
function autoResize() {
  const el = editorRef.value;
  if (!el) return;
  el.style.height = "auto";
  el.style.height = `${Math.min(el.scrollHeight, 128)}px`;
}

/** 清空但残留空壳（空的 div/br）时规范化为真·空，让 :empty 的 placeholder 回来。 */
function normalizeEmpty() {
  const el = editorRef.value;
  // 用 textContent 而非 innerText：innerText 会强制同步 reflow（对超长文本极慢），
  // 这里只需判断"是否空壳"，textContent 语义足够且不触发布局。
  if (el && (el.textContent ?? "").trim() === "") el.innerHTML = "";
}

watch(codeMode, () => nextTick(() => autoResize()));

// 打开会话即聚焦输入框（移动端不自动弹软键盘）
watch(
  () => props.convId,
  async () => {
    await nextTick();
    if (!app.isMobile) focusEditor();
    autoResize();
  },
  { immediate: true },
);

/** 聚焦编辑器；atEnd 时把 caret 挪到内容末尾（发送后/切会话用）。 */
function focusEditor(atEnd = true) {
  const el = editorRef.value;
  if (!el) return;
  el.focus();
  if (!atEnd) return;
  const sel = window.getSelection();
  if (!sel) return;
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(false);
  sel.removeAllRanges();
  sel.addRange(range);
}

/** 序列化草稿：innerText 把 token 读成 @名字、<br>/块边界读成 \n；
 *  块尾的 \n 是渲染 artifact，剥掉；maxlength 语义挪到发送前截断兜底。 */
function serializeDraft(): string {
  return (editorRef.value?.innerText ?? "").replace(/\n+$/, "").slice(0, MAX_INPUT_LENGTH);
}

/** 发送：立即清空输入框（optimistic UI，不等 IPC 返回）。引用消息在首行拼接引用头。 */
function send(kind?: MsgKind, content?: string) {
  let text = content ?? serializeDraft();
  const k = kind ?? (codeMode.value ? "code" : "text");
  if (k === "text" && !text.trim()) return;
  if (k === "text" && props.quote && text.trim()) {
    const idSuffix = props.quote.msgId != null ? `|${props.quote.msgId}` : "";
    text = `「引用 ${props.quote.sender}：${props.quote.snippet}${idSuffix}」\n${text}`;
  }
  if (editorRef.value) editorRef.value.innerHTML = "";
  hasDraft.value = false;
  mention.value = null;
  if (!kind) codeMode.value = false;
  if (props.quote) emit("close-quote");
  // 清空 + DOM 更新后重新聚焦并把 caret 放到末尾：连续发送/继续输入无缝衔接
  void nextTick(() => {
    autoResize();
    if (!app.isMobile) focusEditor();
  });
  // 消息已经乐观入列（DOM 立刻更新）→ 给一下轻触觉，确认"发出去了"。
  // 按 Apple 的触觉规则：只在关键动作给，且是按下即给（不是等网络回来才给）。
  haptic("light");
  emit("send", { content: text, kind: k });
}

/**
 * 输入法组合态（拼音/日文等）。
 *
 * ⚠️ 不能只依赖 `KeyboardEvent.isComposing`：macOS WKWebView 上用 Enter 提交候选时，
 * 事件顺序是 `compositionend` → `keydown`，keydown 那一刻 `isComposing` 已是 false
 * ⇒ 会把"选字"当成"发送"，把半成品中文直接发出去（用户 2026-09-12 反馈的正是这个）。
 * 所以自己跟踪组合态，并保留一个"提交后短窗口"兜底；判定逻辑抽在
 * `utils/ime.ts`（纯函数，有单测钉死）。
 */
const composing = ref(false);
let compositionEndedAt = 0;

function onCompositionStart() {
  composing.value = true;
}
function onCompositionEnd() {
  composing.value = false;
  compositionEndedAt = Date.now();
}

/** 这次按键是否属于输入法组合（必须放行给 IME / 内容）。 */
function imeKey(e: KeyboardEvent): boolean {
  return isImeKey(e, composing.value, compositionEndedAt, Date.now());
}

function onKeydown(e: KeyboardEvent) {
  // 输入法组合中的按键**一律不拦截**：
  //   · Enter 交给 IME 提交字母（用户要的"把拼音字母落下来"）；
  //   · 若该 Enter 其实是给内容的，则走浏览器默认行为插入换行
  //     （用户要的"回车应该响应聊天内容的回车"）—— 两种情形都不该由我们发送。
  if (imeKey(e)) return;
  // 微信式 token 联删：caret 前是「token + 尾随 nbsp」时一次退格删掉两者
  // （token 本身是原子，浏览器默认已整删；只补 nbsp 这一格的差距）。
  if (e.key === "Backspace" && !e.isComposing && deleteMentionBeforeCaret()) {
    e.preventDefault();
    return;
  }
  // @ 选择器打开时：↑↓ 导航、Enter 选中（吞掉发送）、Esc 关闭，其余键正常输入
  if (mention.value && mentionFiltered.value.length > 0) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      mentionActive.value = (mentionActive.value + 1) % mentionFiltered.value.length;
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      mentionActive.value =
        (mentionActive.value - 1 + mentionFiltered.value.length) % mentionFiltered.value.length;
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      applyMention(mentionFiltered.value[mentionActive.value]);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      mention.value = null;
      return;
    }
  }
  // 到这里的按键都已排除输入法组合（见上面的 `imeKey` 提前返回）
  if (e.key === "Enter" && e.shiftKey) {
    // 统一换行为 <br>：浏览器默认 insertParagraph 会造嵌套 div，序列化不可控
    e.preventDefault();
    document.execCommand("insertLineBreak");
    return;
  }
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    send();
  }
}

// ---------------- 群聊 @ 成员选择 ----------------
/** 触发态：caret 前最近的 @query（无空白隔断）；startIndex 是 @ 在所在文本节点内的偏移。 */
const mention = ref<{ query: string; startIndex: number } | null>(null);
const mentionActive = ref(0);

const mentionFiltered = computed(() => {
  if (!mention.value) return [];
  const q = mention.value.query.toLowerCase();
  const list = props.mentionMembers ?? [];
  return (q ? list.filter((m) => m.name.toLowerCase().includes(q)) : list).slice(0, 8);
});

/** 原子 token：@提及 与 表情 都是 contentEditable=false 的 span，退格时都应整体删除。 */
function isTokenSpan(n: Node | null): boolean {
  if (n === null || n.nodeType !== Node.ELEMENT_NODE) return false;
  const cl = (n as Element).classList;
  return cl.contains("mention-token") || cl.contains("emoji-token");
}

/** caret 的 Range 上下文：仅当 selection 折叠且落在编辑器内的文本节点上时有效。 */
function caretContext(): { node: Text; offset: number } | null {
  const el = editorRef.value;
  const sel = window.getSelection();
  if (!el || !sel || sel.rangeCount === 0 || !sel.isCollapsed) return null;
  const node = sel.focusNode;
  if (!node || node.nodeType !== Node.TEXT_NODE || !el.contains(node)) return null;
  return { node: node as Text, offset: sel.focusOffset };
}

/** 由 caret 位置推导 @ 触发态（输入/点击/方向键挪 caret 时都会调用）。 */
function updateMentionState() {
  if (!props.mentionMembers?.length) {
    mention.value = null;
    return;
  }
  const ctx = caretContext();
  if (!ctx) {
    mention.value = null;
    return;
  }
  const m = (ctx.node.textContent ?? "").slice(0, ctx.offset).match(/@([^\s@]{0,20})$/);
  if (m) {
    const start = ctx.offset - m[0].length;
    const sameAt = mention.value?.startIndex === start;
    mention.value = { query: m[1], startIndex: start };
    if (!sameAt) mentionActive.value = 0;
  } else {
    mention.value = null;
  }
}

/** 选中成员：把「@query」替换为 mention token（原子 span）+ 尾随 nbsp，caret 落到 nbsp 后。 */
function applyMention(member: { id: string; name: string }) {
  const el = editorRef.value;
  const sel = window.getSelection();
  if (!el || !sel || sel.rangeCount === 0) return;
  const ctx = caretContext();
  if (!ctx) return;
  const m = (ctx.node.textContent ?? "").slice(0, ctx.offset).match(/@([^\s@]{0,20})$/);
  if (!m) return;
  const range = document.createRange();
  range.setStart(ctx.node, ctx.offset - m[0].length);
  range.setEnd(ctx.node, ctx.offset);
  range.deleteContents();
  // token：不可编辑原子 → 退格/选区删除天然整块处理；nbsp 保证 token 与后续文字不粘连
  // （接收端「被 @ 检测」要求 @名字 后是空白边界，nbsp 的 \u00A0 恰在 JS \s 集合内）
  const span = document.createElement("span");
  span.className = "mention-token";
  span.contentEditable = "false";
  span.dataset.mentionId = member.id;
  span.dataset.mentionName = member.name;
  if (composerMentionFg.value) span.style.color = composerMentionFg.value;
  span.textContent = `@${member.name}`;
  range.collapse(false);
  range.insertNode(span);
  const space = document.createTextNode("\u00A0");
  span.after(space);
  range.setStart(space, 1);
  range.collapse(true);
  sel.removeAllRanges();
  sel.addRange(range);
  mention.value = null;
  if (!app.isMobile) el.focus();
  autoResize();
}

/** caret 前是「token 尾随的 nbsp」→ 连 token 一起删（微信式一次退格删全）。
 *  token 紧邻 caret（无 nbsp）的场景交给浏览器：原子 span 本就整块删除。 */
function deleteMentionBeforeCaret(): boolean {
  const el = editorRef.value;
  if (!el) return false;
  const ctx = caretContext();
  if (!ctx) return false;
  const { node, offset } = ctx;
  const text = node.textContent ?? "";
  if (offset !== 1 || text[offset - 1] !== "\u00A0") return false;
  const prev = node.previousSibling;
  if (!isTokenSpan(prev) || !prev) return false;
  const range = document.createRange();
  range.setStartBefore(prev);
  range.setEnd(node, 1);
  range.deleteContents();
  normalizeEmpty();
  return true;
}

/**
 * 把「输入框里有没有内容」投影成响应式 `hasDraft`（模板只读它，避免每次渲染都读 innerText）。
 *
 * ⚠️ 程序化改动内容后**必须**手动调它：浏览器的 `input` 事件只在用户输入时触发，
 * 我们直接改 DOM（如插入表情 token）不会触发 → 不调它就会出现「只有表情时发送键点不动」。
 */
function syncDraftState() {
  const el = editorRef.value;
  // textContent 替代 innerText：innerText 每次读取都触发同步 reflow，
  // 粘贴长文本后这里会被 input 事件高频调用，reflow 累加即"卡死"。
  hasDraft.value = ((el?.textContent ?? "").trim().length) > 0;
}

/** input 统一入口：投影 hasDraft、规范化空壳、非组合输入时更新 @ 触发态。 */
function onInput(e: Event) {
  syncDraftState();
  normalizeEmpty();
  if (!(e as InputEvent).isComposing) updateMentionState();
}

/** 方向键挪 caret 不触发 input/click：监听 selectionchange 兜底关弹层/换过滤。 */
function onSelectionChange() {
  const el = editorRef.value;
  if (!el || document.activeElement !== el) return;
  updateMentionState();
}

// ---------------- 表情面板 ----------------
const emojiOpen = ref(false);

/**
 * 表情面板参与全局浮层互斥：右键菜单/已读弹层打开时会自动收起它，
 * 反之打开表情面板也会收起那两者（避免两层浮层叠在一起）。
 */
const emojiPopup = useExclusivePopup("emoji-picker");
watch(emojiPopup.isActive, (mine) => {
  if (!mine) emojiOpen.value = false;
});

function toggleEmoji() {
  if (emojiOpen.value) {
    closeEmoji();
    return;
  }
  emojiOpen.value = true;
  emojiPopup.claim();
}

function closeEmoji() {
  emojiPopup.release();
  emojiOpen.value = false;
}

/** 点击面板外关闭（面板自身已 @click.stop，触发按钮也 stop） */
function onDocClickForEmoji() {
  closeEmoji();
}
/** @ 成员选择：点击输入卡以外任意处关闭（卡内点击交给 updateMentionState 按光标推断） */
function onDocClickForMention(e: MouseEvent) {
  if (composerCard.value?.contains(e.target as Node)) return;
  mention.value = null;
}
const composerCard = ref<HTMLElement | null>(null);
onMounted(() => {
  document.addEventListener("click", onDocClickForEmoji);
  document.addEventListener("click", onDocClickForMention);
  document.addEventListener("selectionchange", onSelectionChange);
});
onUnmounted(() => {
  document.removeEventListener("click", onDocClickForEmoji);
  document.removeEventListener("click", onDocClickForMention);
  document.removeEventListener("selectionchange", onSelectionChange);
});
watch(emojiOpen, () => {
  if (emojiOpen.value) autoResize();
});

/** 把表情作为原子 token 插入输入框 caret 处（contentEditable=false，退格/选区整体删除，
 *  与 @mention token 同一机制）；caret 不在编辑器内则追加末尾。 */
function insertEmoji(e: string) {
  const el = editorRef.value;
  closeEmoji();
  if (!el) return;
  // ⚠️ 两个分支都必须插「token span」，不能插纯文本：
  // token 是 contentEditable=false → 退格/选区删除整体生效；
  // 插纯文本会被浏览器一个字一个字地删掉（就是"表情文字被拆开"的那个 bug）。
  const span = document.createElement("span");
  span.className = "emoji-token";
  span.contentEditable = "false";
  span.textContent = e; // 如 "[黄脸干杯]"，serializeDraft 经 innerText 读回原文
  const sel = window.getSelection();
  if (sel && sel.rangeCount > 0 && el.contains(sel.anchorNode)) {
    const range = sel.getRangeAt(0);
    range.deleteContents();
    range.insertNode(span);
    // 与 @mention 同一套：token 后补一个不换行空格，caret 才能落到 token 之后
    const space = document.createTextNode("\u00A0");
    span.after(space);
    range.setStartAfter(space);
    range.collapse(true);
    sel.removeAllRanges();
    sel.addRange(range);
  } else {
    el.appendChild(span);
    el.appendChild(document.createTextNode("\u00A0"));
    if (!app.isMobile) focusEditor();
  }
  // 直接改 DOM 不会触发 input 事件 → 必须手动同步，否则「只有表情时发送键是灰的」
  syncDraftState();
  autoResize();
}

async function onPaste(e: ClipboardEvent) {
  const cd = e.clipboardData;
  if (!cd) return;
  // 无论最终归到哪条分支，都在同步阶段拦掉默认插入（图片/文件/文本都不该由浏览器塞进编辑器）
  e.preventDefault();

  const types = Array.from(cd.types ?? []);
  const files = Array.from(cd.files ?? []);
  const items = Array.from(cd.items).map((i) => ({ kind: i.kind, type: i.type }));

  // 同步捕获图片 File：files 优先，其次 items.getAsFile()。
  // ⚠️ 必须在任何 await 之前完成——Chromium/WebKit 会在 paste 事件返回后清空 clipboardData，
  // 若先 await 再读 items，getAsFile() 会拿到 null，表现为"截图有时发不出去"。
  let imageFile: File | null = files.find((x) => x.type.startsWith("image/")) ?? null;
  if (!imageFile) {
    for (const item of Array.from(cd.items)) {
      if (item.kind === "file" && item.type.startsWith("image/")) {
        imageFile = item.getAsFile();
        if (imageFile) break;
      }
    }
  }

  // 图片优先：截图位图即使同时带 "Files"（Win11 临时文件引用），也按图片发送，
  // 避免去发那个可能已被清理的临时路径。
  if (imageFile) {
    emit("send-image", await fileToDataUrl(imageFile));
    return;
  }

  // 无图片 → 尝试真实文件路径（资源管理器复制的 CF_HDROP）。非 Windows 命令返回空，回退文本。
  let filePaths: string[] = [];
  if (types.includes("Files")) {
    try {
      filePaths = await invoke<string[]>("read_clipboard_file_paths");
    } catch {
      filePaths = [];
    }
  }

  const action = classifyPaste(types, items, files, filePaths.length > 0);
  if (action.kind === "files") {
    emit("paste-files", filePaths);
    return;
  }

  // 纯文本：contenteditable 默认粘贴会带外来 HTML 结构（污染 token/样式），
  // 统一拦掉按纯文本插入（execCommand 保 undo 栈；含 \n 时 Chromium 自行转 <br>）。
  const text = cd.getData("text/plain");
  // 截断到硬上限：超长文本若完整塞进 contenteditable，插入 + 后续 innerText 读都会
  // 触发大范围 reflow，几十万字符足以把界面卡死（"粘贴一大段就卡死"的根因）。
  if (text) document.execCommand("insertText", false, text.slice(0, MAX_INPUT_LENGTH));
}

function fileToDataUrl(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = reject;
    r.readAsDataURL(f);
  });
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <!-- 微信 4.0 输入卡：白底圆角带细边；文本域在上，图标行在卡内底部，发送键靠右下。
         `px-4`（不是 px-3）与工具栏的 `-mx-1` 成对：编辑器的文字左边缘与工具栏第一个
         图标的**点击热区**左边缘取同一个 16px 起点（图标墨迹在其 28px 热区内再内缩 6px，
         与文字字形的光学起点对齐）—— 这是用户反馈「左右两边视觉上不在同一条线上」的修法。 -->
    <div ref="composerCard" class="relative rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-4 pb-2.5 pt-2">
      <!-- 群聊 @ 成员选择：输入 @ 后浮出，↑↓ 导航 / Enter 或点击选中 -->
      <div
        v-if="mention && mentionFiltered.length > 0"
        class="frost absolute bottom-full left-2 right-2 z-30 mb-2 max-h-44 overflow-y-auto rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] p-1 shadow-lg"
      >
        <div class="px-2 py-1 text-[11px] text-[var(--gosslan-text-2)]">{{ t("chat.composer.remind") }}</div>
        <button
          v-for="(m, i) in mentionFiltered"
          :key="m.id"
          class="flex w-full items-center gap-2 rounded-[var(--gosslan-radius-sm)] px-2 py-1.5 text-left text-[13px] transition"
          :class="i === mentionActive ? 'bg-[var(--gosslan-list-active)]' : 'hover:bg-[var(--gosslan-hover)]'"
          @mousedown.prevent
          @click="applyMention(m)"
        >
          <span
            class="flex h-6 w-6 shrink-0 items-center justify-center overflow-hidden rounded-full text-[11px] text-white"
            :style="{ backgroundColor: nameToColor(m.name) }"
          >{{ avatarInitial(m.name) }}</span>
          <span class="min-w-0 flex-1 truncate" :title="m.name">{{ m.name }}</span>
        </button>
      </div>
      <!-- 引用预览条：右键"引用"后出现在输入框上方，可取消 -->
      <div
        v-if="quote"
        class="mb-1.5 flex items-center gap-2 rounded-[var(--gosslan-radius-sm)] border-l-2 px-2 py-1 text-[12px]"
        :style="{ borderColor: QUOTE_BORDER, background: QUOTE_BG, color: 'var(--gosslan-text)' }"
      >
        <span
          class="min-w-0 flex-1 truncate"
          :style="QUOTE_TEXT_STYLE"
          :title="`${t('common.quote')} ${quote.sender}：${quote.snippet}`"
        >{{ t("common.quote") }} {{ quote.sender }}：{{ quote.snippet }}</span>
        <button
          class="flex h-5 w-5 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-xs)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('chat.composer.cancelQuote')" :aria-label="t('chat.composer.cancelQuote')"
          @click="emit('close-quote')"
        >
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
      <!-- contenteditable 编辑区：@提及 为内联原子 token（高亮+整删）。
           ⚠️ **不放 placeholder**：用户 2026-09-12 明确要求「输入框里也不用 placeholder」
           （参考图是干净的输入区）。原先走 `:data-placeholder` + `:empty::before`，
           现连同 style.css 里的那条规则与两个 i18n key 一起删除，避免留死代码。
           `normalizeEmpty` 保留 —— 它现在只服务 `hasDraft`（空壳 div/br 会让"有草稿"误判）。 -->
      <div
        ref="editorRef"
        contenteditable="true"
        role="textbox"
        aria-multiline="true"
        :aria-label="t('chat.composer.inputAria')"
        enterkeyhint="send"
        :spellcheck="!codeMode"
        :autocorrect="codeMode ? 'off' : 'on'"
        :autocapitalize="codeMode ? 'off' : 'sentences'"
        class="min-h-10 w-full overflow-y-auto bg-transparent px-0.5 py-0.5 leading-normal outline-none whitespace-pre-wrap break-words"
        :class="codeMode ? 'font-mono text-[13px]' : ''"
        :style="{ fontSize: 'var(--gosslan-msg-size, 14px)', overflowWrap: 'anywhere', wordBreak: 'break-word' }"
        @keydown="onKeydown"
        @compositionstart="onCompositionStart"
        @compositionend="onCompositionEnd"
        @input="onInput"
        @click="updateMentionState"
        @paste="onPaste"
      ></div>
      <!-- 工具栏行（2026-09-12 按用户反馈重做：对齐 + 统一规格 + 去掉"网页感"）。
           三条硬性约定，改这一行时必须同时满足：
           ① **对齐**：行用 `items-center`，左右两组**同高（h-7 = 28px）**。原先右侧发送键
              是 `h-7 + px-4 + text-[13px]`，而左侧图标按钮 28px 见方、图标 18px ——
              两者行盒不同（图标按钮的 flex 行盒 vs 文本基线），`items-center` 居中的
              是两个不同高度的行盒 ⇒ 视觉上看不出在同一条中线上。
           ② **两侧留白一致**：卡片是 `px-4`（16px），编辑器滚到左边缘 ⇒ 工具栏左右各加
              `-mx-1`（4px）+ 按钮自身 4px 内缩 = 4px 光学内缩，左侧第一个图标与右侧发送键
              的边距对称；`-mx-1` 同时让 28px 按钮的点击热区不越出卡片。
           ③ **统一规格**（2026-09-12 晚按微信参考图二次校准）：
              图标按钮 **32×32**（`h-8 w-8`）/ 图标 **20px**（`h-5 w-5`）/ 线宽 1.75 /
              圆角 **radius-md(8px)** —— 圆角要与圆形字形"同心"，hover 底色块才像微信那样
              是一个包住图标的圆角方块（用户原话：「它的 hover 和周围的圆角感觉也是同心圆」）；
              按钮间距 8px、与文本间距 4px、卡片底距 10px。
           发送键改为**实心主按钮**（有草稿才点亮）：微信 4.0 的观感，
           无草稿时是低对比的占位态，不抢视觉。 -->
      <div class="-mx-1 mt-1 flex h-8 items-center gap-2">
        <div class="relative">
          <button
            class="flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] transition"
            :class="emojiOpen ? 'text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
            :title="t('chat.composer.emoji')" :aria-label="t('chat.composer.emoji')"
            @click.stop="toggleEmoji"
          >
            <Smile class="h-5 w-5" :stroke-width="1.75" />
          </button>
          <EmojiPicker :open="emojiOpen" @select="insertEmoji" @close="closeEmoji" />
        </div>
        <!-- @mousedown.prevent 保持编辑器焦点：否则点击按钮后焦点落到按钮上，
             紧接着按 Enter 会激活按钮（把 codeMode 再切回去）而非走编辑器 keydown 发送。 -->
        <button
          class="flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] transition"
          :class="codeMode ? 'text-[var(--gosslan-accent-ink)]' : 'text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-hover)]'"
          :title="t('chat.composer.code')" :aria-label="t('chat.composer.code')"
          @mousedown.prevent
          @click="codeMode = !codeMode"
        >
          <!-- 代码模式图标（用户 2026-09-12 晚二次反馈：「找一个和代码相关的图标，
               但是又和左右两边的图标是统一风格类型的。现在这个图标不像是发送代码」）。
               上一版按参考图取了 `Box`（立体方块）—— 几何一致但**语义不对**（看不出是代码）。
               现改为 `SquareCode`：方形描边外框 + 内部 `</>`，既是代码语义，
               又与左 `Smile`（圆）、右 `Folder`（方）同属「几何外框 + 内部细节」一套；
               尺寸 16px / 线宽 1.75 与两侧完全一致（见上方工具栏规格说明）。 -->
          <SquareCode class="h-5 w-5" :stroke-width="1.75" />
        </button>
        <button
          class="flex h-8 w-8 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          :title="t('chat.composer.sendFile')" :aria-label="t('chat.composer.sendFile')"
          @click="emit('attach')"
        >
          <Folder class="h-5 w-5" :stroke-width="1.75" />
        </button>
        <button
          class="ml-auto flex h-8 shrink-0 items-center rounded-[6px] px-3.5 text-[13px] font-medium transition"
          :class="hasDraft
            ? 'bg-primary text-white hover:bg-primary-hover'
            : 'cursor-default bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)]'"
          :disabled="!hasDraft"
          @mousedown.prevent
          @click="send()"
        >
          {{ t("common.send") }}
        </button>
      </div>
    </div>
  </div>
</template>
