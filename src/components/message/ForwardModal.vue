<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref } from "vue";
import { useChatStore } from "@/stores/useChatStore";
import { avatarInitial, nameToColor } from "@/utils/color";
import BaseModal from "@/components/BaseModal.vue";
import type { MsgKind } from "@/types";

const props = defineProps<{
  open: boolean;
  /** 转发的消息类型（决定预览文案）。 */
  kind: MsgKind;
  /** 预览片段（截断后）。 */
  snippet: string;
}>();
const emit = defineEmits<{ (e: "close"): void; (e: "pick", convId: string): void }>();

const chat = useChatStore();
const keyword = ref("");

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return chat.conversations;
  return chat.conversations.filter((c) => c.name.toLowerCase().includes(kw));
});

const kindLabel = computed(
  () => ({ text: t("common.text"), code: t("common.code"), image: t("common.image"), file: t("common.file"), system: t("common.system") })[props.kind] ?? t("msg.message"),
);
</script>

<template>
  <BaseModal :open="open" :title="t('msg.forwardTo')" @close="emit('close')">
    <div class="space-y-3">
      <!-- 引用预览 -->
      <div class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-2 text-xs text-[var(--gosslan-text-2)]">
        <span class="mr-1 rounded-[var(--gosslan-radius-xs)] bg-[var(--gosslan-panel)] px-1.5 py-0.5 text-[11px] text-[var(--gosslan-text)]">{{ kindLabel }}</span>
        <span class="align-middle">{{ snippet }}</span>
      </div>

      <input
        v-model="keyword"
        maxlength="50"
        :placeholder="t('msg.searchConversation')"
        class="w-full rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-transparent px-3 py-2 text-[13px] outline-none placeholder:text-[var(--gosslan-text-2)] focus:border-[var(--gosslan-primary)]"
      />

      <div class="max-h-64 select-none overflow-y-auto">
        <button
          v-for="c in filtered"
          :key="c.id"
          class="flex h-[52px] w-full items-center gap-3 rounded-[var(--gosslan-radius-md)] px-2 text-left transition hover:bg-[var(--gosslan-hover)]"
          @click="emit('pick', c.id)"
        >
          <span
            v-if="c.kind === 'group'"
            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--gosslan-avatar-radius)] bg-[var(--gosslan-rail-active)] text-xs text-[var(--gosslan-text-2)]"
          >
            {{ t("common.group") }}
          </span>
          <span
            v-else
            class="flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-sm text-white"
            :style="{ backgroundColor: nameToColor(c.name) }"
          >
            <img alt="" v-if="c.avatar" :src="c.avatar" class="h-full w-full object-cover" />
            <span v-else>{{ avatarInitial(c.name) }}</span>
          </span>
          <span class="min-w-0 flex-1 truncate text-[13px] text-[var(--gosslan-text)]" :title="c.name">{{ c.name }}</span>
        </button>
        <div v-if="filtered.length === 0" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">{{ t("msg.noMatch") }}</div>
      </div>
    </div>
  </BaseModal>
</template>
