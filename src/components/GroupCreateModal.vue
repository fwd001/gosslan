<script setup lang="ts">
import { t } from "@/i18n";
import { ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import { avatarInitial, nameToColor } from "@/utils/color";
import { Check } from "lucide-vue-next";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const name = ref("");
const selected = ref<string[]>([]);

watch(
  () => props.open,
  (v) => {
    if (v) {
      name.value = "";
      selected.value = [];
      chat.refreshFriends();
    }
  },
);

function toggle(id: string) {
  const i = selected.value.indexOf(id);
  if (i >= 0) selected.value.splice(i, 1);
  else selected.value.push(id);
}

function initials(n: string) {
  return avatarInitial(n);
}

async function create() {
  if (!name.value.trim()) {
    app.toast(t("group.toast.nameEmpty"), "error");
    return;
  }
  if (selected.value.length === 0) {
    app.toast(t("group.toast.noMember"), "error");
    return;
  }
  try {
    const g = await chat.createGroup(name.value.trim(), selected.value);
    app.toast(t("group.toast.created", { name: g.name }), "success");
    emit("close");
    chat.openConversation(`group:${g.id}`);
  } catch (e) {
    app.toastError(e, t("group.toast.createFail"));
  }
}
</script>

<template>
  <BaseModal :open="open" :title="t('group.create.title')" @close="emit('close')">
    <div class="mb-4">
      <div class="mb-1.5 text-sm">{{ t("group.name") }}</div>
      <input
        v-model="name"
        class="w-full rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
        :placeholder="t('group.namePlaceholder')"
        maxlength="40"
      />
    </div>

    <div class="mb-1.5 text-sm">{{ t("group.selectMembers") }}</div>
    <div class="max-h-56 overflow-y-auto">
      <div
        v-for="f in chat.friends"
        :key="f.device_id"
        class="flex cursor-pointer items-center gap-2 rounded-[var(--gosslan-radius-md)] px-2 py-2 transition hover:bg-[var(--gosslan-hover)]"
        @click="toggle(f.device_id)"
      >
        <div
          class="flex h-4 w-4 items-center justify-center rounded-[var(--gosslan-radius-xs)] border"
          :class="selected.includes(f.device_id) ? 'border-primary bg-primary' : 'border-[var(--gosslan-border)]'"
        >
          <Check v-if="selected.includes(f.device_id)" class="h-3 w-3 text-white" />
        </div>
        <div
          class="flex h-8 w-8 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
          :style="{ backgroundColor: nameToColor(f.nickname) }"
        >
          <img alt="" v-if="f.avatar" :src="f.avatar" class="h-full w-full object-cover" />
          <span v-else class="text-xs font-semibold">{{ initials(f.nickname) }}</span>
        </div>
        <span class="flex-1 text-sm">{{ f.nickname }}</span>
        <span class="text-xs text-[var(--gosslan-text-2)]">{{ f.online ? t("common.online") : t("common.offline") }}</span>
      </div>
      <div v-if="chat.friends.length === 0" class="py-6 text-center text-sm text-[var(--gosslan-text-2)]">
        {{ t("group.noFriends") }}
      </div>
    </div>

    <button
      class="mt-4 w-full rounded-[var(--gosslan-radius-lg)] bg-primary py-2.5 text-sm font-medium text-white transition hover:bg-primary-hover disabled:opacity-40"
      :disabled="selected.length === 0 || !name.trim()"
      @click="create"
    >
      {{ t("common.createGroup") }}
    </button>
  </BaseModal>
</template>
