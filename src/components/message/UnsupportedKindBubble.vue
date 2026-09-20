<script setup lang="ts">
/**
 * 认不出来的消息类型怎么显示（INV-P24 第 2 条：未知内容不得成为裸 JSON）。
 *
 * 触发条件只有一个：`messageKinds.isKnownKind(kind)` 为 false —— 也就是这条 kind 不在
 * `protocol.rs` 的 `WIRE_KINDS` 表里。实际只有一种情况会走到这里：**对端的 Gosslan 比本机新**，
 * 它发了一个本机还不认识的消息类型。
 *
 * 为什么不是"直接隐藏"：那样等于静默丢消息 —— 用户看到的是"对方发了什么却没有"，
 * 比看到一句"看不懂"更糟，也更难排查（对方会说"我发了啊"）。
 *
 * 为什么原文要**主动展开**才给：载荷通常是 JSON，默认铺在时间线上就是用户明确要求
 * 禁止的那个形态；但完全抹掉又会丢信息（升级前也可能需要人工看出那是个什么东西），
 * 所以收进一个折叠区，同时保留 kind 字面量供诊断。
 *
 * ⚠️ 纵向排版（`px-3 py-1.5` + `text-sm leading-normal`）与 `MessageTextBubble` 一致：
 * 虚拟列表按 `utils/previewMetrics.ts` 的常量估高，不一致会让相邻消息互相遮挡。
 */
import { t } from "@/i18n";
import { ref, type CSSProperties } from "vue";

defineProps<{ kind: string; content: string; bubbleStyle: CSSProperties }>();

const expanded = ref(false);
</script>

<template>
  <div class="select-text px-3 py-1.5 text-sm leading-normal" :style="bubbleStyle">
    <div class="font-medium">{{ t("msg.unsupportedKind") }}</div>
    <button
      class="tap-safe mt-1 text-xs opacity-70 transition hover:opacity-100"
      @click="expanded = !expanded"
    >
      {{ expanded ? t("msg.hideRawContent") : t("msg.showRawContent") }}
    </button>
    <!-- 展开后才给原文：默认状态时间线上不出现任何载荷字节 -->
    <template v-if="expanded">
      <pre
        class="mt-1 max-h-40 overflow-y-auto whitespace-pre-wrap break-all rounded-[var(--gosslan-radius-sm)] bg-[var(--gosslan-hover)] p-2 font-mono text-[11px]"
      >{{ content }}</pre>
      <div class="mt-1 font-mono text-[10px] opacity-60">type: {{ kind }}</div>
    </template>
  </div>
</template>
