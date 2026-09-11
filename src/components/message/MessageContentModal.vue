<script setup lang="ts">
import { t } from "@/i18n";
import { computed } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAppStore } from "@/stores/useAppStore";
import BaseModal from "@/components/BaseModal.vue";
import CodeBlock from "@/components/CodeBlock.vue";
import { linkify, displayUrl, type LinkSegment } from "@/utils/linkify";
import { splitEmoji } from "@/utils/emoji";
import { mentionHighlightColor } from "@/utils/chatStyle";
import { Check, Copy } from "lucide-vue-next";

const props = defineProps<{
  open: boolean;
  kind: "text" | "code";
  content: string;
  copied: boolean;
  /** 群成员名列表：与气泡共用同一套 @ 高亮，避免"气泡里高亮、全文里是纯文本"。 */
  mentionNames?: string[];
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "copy", content: string): void;
}>();

const app = useAppStore();
type RenderSegment = LinkSegment | { kind: "emoji"; value: string; name: string; url: string };
const segments = computed<RenderSegment[]>(() => {
  if (props.kind !== "text") return [];
  const out: RenderSegment[] = [];
  for (const s of splitEmoji(props.content)) {
    if (s.kind === "emoji") out.push({ kind: "emoji", value: s.value, name: s.name, url: s.url });
    else out.push(...linkify(s.value, props.mentionNames ?? []));
  }
  return out;
});

/** @提及配色：以弹窗面板底色为对比基准（气泡那套是按气泡底色算的，这里不能复用）。 */
const mentionFg = computed(() =>
  mentionHighlightColor(app.themeColor, app.dark, app.dark ? "#1e293b" : "#ffffff"),
);

/** @提及 淡背景：与气泡同款取法（mentionFg 的 16%）。
 *  ⚠️ 必须显式给：不给就落到 style.css 里 12% 的兜底值，同一句话在气泡与全文里
 *  色块深浅会不一致（这正是本次要消除的"两处样式不一样"）。 */
const mentionBg = computed(() => {
  const fg = mentionFg.value;
  return fg ? `color-mix(in srgb, ${fg} 16%, transparent)` : undefined;
});

async function openLink(href: string) {
  try {
    await openUrl(href);
  } catch (e) {
    app.toastError(e, t("msg.openLinkFail"));
  }
}
</script>

<template>
  <!-- 全文弹窗：文本 / 代码共用，内容可滚动、可复制；关闭后消息布局不变 -->
  <BaseModal
    :open="open"
    :title="kind === 'code' ? t('msg.codePreview') : t('msg.fullText')"
    width="max-w-4xl"
    @close="emit('close')"
  >
    <div class="max-h-[70vh] overflow-y-auto">
      <CodeBlock v-if="kind === 'code'" :code="content" />
      <div
        v-else
        class="whitespace-pre-wrap break-words text-sm leading-relaxed text-[var(--gosslan-text)]"
        :style="{ wordBreak: 'break-word' }"
      >
        <template v-for="(seg, i) in segments" :key="i">
          <a
            v-if="seg.kind === 'link'"
            class="cursor-pointer break-all text-[var(--gosslan-accent-ink)] underline decoration-1 underline-offset-2 transition hover:opacity-80"
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
    </div>
    <div class="mt-3 flex justify-end">
      <button
        class="tap-safe preview-action"
        :class="copied ? 'text-[var(--gosslan-accent-ink)]' : ''"
        @click="emit('copy', content)"
      >
        <Check v-if="copied" class="h-3 w-3" />
        <Copy v-else class="h-3 w-3" />
        {{ copied ? t("common.copied") : t("common.copy") }}
      </button>
    </div>
  </BaseModal>
</template>
