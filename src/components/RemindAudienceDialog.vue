<script setup lang="ts">
/**
 * 群聊强提醒 · 受众选择弹窗（第一版不默认全群）。
 *
 * 从当前群成员里多选（排除自己），至少选 1 人才能确认；确认时把 device_id 名单
 * 交给上层写进 remind 载荷的 `actors`。资料名字/头像/在线点统一走
 * `useMemberProfile`，与群成员面板一致。
 */
import { computed, ref, watch } from "vue";
import { Check } from "lucide-vue-next";
import BaseModal from "@/components/BaseModal.vue";
import SelfAvatar from "@/components/SelfAvatar.vue";
import { useChatStore } from "@/stores/useChatStore";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { t } from "@/i18n";

const props = defineProps<{
  open: boolean;
  /** 群 id（不含 "group:" 前缀，上层已切好）。 */
  groupId: string;
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "picked", actors: string[]): void;
}>();

const chat = useChatStore();
const { memberProfile, myId } = useMemberProfile();

const group = computed(() => chat.groups.find((g) => g.id === props.groupId));
/** 候选受众：群成员排除自己。 */
const candidates = computed(() =>
  (group.value?.members ?? []).filter((id) => id !== myId.value),
);

const selected = ref<Set<string>>(new Set());

// 每次打开都重置选择，避免上次的勾选残留
watch(
  () => props.open,
  (on) => {
    if (on) selected.value = new Set();
  },
);

function toggle(id: string) {
  const next = new Set(selected.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selected.value = next;
}

function confirm() {
  if (selected.value.size === 0) return;
  emit("picked", [...selected.value]);
}
</script>

<template>
  <BaseModal :open="open" :title="t('remind.audienceTitle')" @close="emit('close')">
    <p class="mb-2 text-xs text-[var(--gosslan-text-3)]">
      {{ t("remind.audienceHint") }}
    </p>
    <div class="-mx-1 max-h-72 overflow-y-auto">
      <button
        v-for="id in candidates"
        :key="id"
        type="button"
        class="flex w-full items-center gap-3 rounded-[var(--gosslan-radius-md)] px-3 py-2 text-left transition hover:bg-[var(--gosslan-hover)] active:bg-[var(--gosslan-hover)]"
        @click="toggle(id)"
      >
        <SelfAvatar :src="memberProfile(id).avatar" :size="36" />
        <span
          class="min-w-0 flex-1 truncate text-sm text-[var(--gosslan-text)]"
          :title="memberProfile(id).name"
        >
          {{ memberProfile(id).name }}
        </span>
        <span
          v-if="selected.has(id)"
          class="flex h-5 w-5 items-center justify-center rounded-full bg-[var(--gosslan-primary)] text-white"
        >
          <Check class="h-3.5 w-3.5" />
        </span>
        <span v-else class="h-5 w-5 rounded-full border border-[var(--gosslan-divider)]" />
      </button>
    </div>
    <div class="mt-5 flex items-center justify-between gap-2">
      <span class="text-xs text-[var(--gosslan-text-3)]">
        {{ t("remind.audienceCount", { n: selected.size }) }}
      </span>
      <div class="flex gap-2">
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] px-4 py-2 text-sm text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          @click="emit('close')"
        >
          {{ t("common.cancel") }}
        </button>
        <button
          class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-4 py-2 text-sm text-white transition hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="selected.size === 0"
          @click="confirm"
        >
          {{ t("remind.audienceConfirm") }}
        </button>
      </div>
    </div>
  </BaseModal>
</template>
