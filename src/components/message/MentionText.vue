<script setup lang="ts">
/**
 * 「@ 与链接」的统一渲染件（第二阶段 §9：同一份 @ 渲染要覆盖聊天**和任务**）。
 *
 * 语义**不在这里**：段怎么切、@ 到哪个人算「@到我」、显示成什么文案，全部单源在
 * `utils/linkify`（`mention` / `mention-self` 两个 kind）。本件只负责把这些段画出来，
 * 于是"再加一个显示任务描述的界面"不必重新实现一遍判定 —— 那正是 §6① 记的那类 bug
 * （同一个判断在两处各算一次，迟早分叉）。
 *
 * 样式走 `style.css` 里全局那两条 `.mention-token` / `.mention-token--self`。
 * ⚠️ 这里**不**按底色现算 @ 的文字色（聊天气泡那样做是因为它的底色逐条消息变，
 * 对比度会掉到 2.94；任务卡与任务详情的底色是固定的），别把两套做法混起来。
 */
import { computed } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAppStore } from "@/stores/useAppStore";
import { linkify } from "@/utils/linkify";
import { splitEmoji } from "@/utils/emoji";
import MessageLinkText from "@/components/message/MessageLinkText.vue";
import { t } from "@/i18n";

const props = defineProps<{
  text: string;
  /** 群成员名列表：正文里的 @name 按此高亮（不传不高亮）。 */
  mentionNames?: string[];
  /** 查看者自己是谁（本机昵称 + 本地化标签）⇒ @到我自己 那一段换成「@你」并加重。 */
  selfMention?: { name: string; label: string } | null;
}>();

const app = useAppStore();

/** 与聊天气泡同一套两级切分：先按表情 token 切段，文本段再交给 linkify 切链接/提及。 */
const segments = computed(() => {
  const out: Array<
    | { kind: "text" | "link" | "mention" | "mention-self"; value: string; href?: string }
    | { kind: "emoji"; value: string; url: string }
  > = [];
  for (const s of splitEmoji(props.text ?? "")) {
    if (s.kind === "emoji") out.push({ kind: "emoji", value: s.value, url: s.url });
    else out.push(...linkify(s.value, props.mentionNames ?? [], props.selfMention ?? undefined));
  }
  return out;
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
  <template v-for="(seg, i) in segments" :key="i">
    <MessageLinkText
      v-if="seg.kind === 'link'"
      :href="seg.href!"
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
      >{{ seg.value }}</span
    >
    <span v-else>{{ seg.value }}</span>
  </template>
</template>
