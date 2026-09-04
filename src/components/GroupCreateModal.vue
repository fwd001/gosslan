<script setup lang="ts">
import { ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
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
  return n.slice(0, 1).toUpperCase();
}

async function create() {
  if (!name.value.trim()) {
    app.toast("请输入群名称", "error");
    return;
  }
  if (selected.value.length === 0) {
    app.toast("请至少选择一位好友", "error");
    return;
  }
  try {
    const g = await chat.createGroup(name.value.trim(), selected.value);
    app.toast(`群聊「${g.name}」已创建`, "success");
    emit("close");
    chat.openConversation(`group:${g.id}`);
  } catch (e) {
    app.toast(String(e), "error");
  }
}
</script>

<template>
  <BaseModal :open="open" title="创建群聊" @close="emit('close')">
    <div class="mb-4">
      <div class="mb-1.5 text-sm">群名称</div>
      <input
        v-model="name"
        class="w-full rounded-lg bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none"
        placeholder="输入群聊名称"
        maxlength="32"
      />
    </div>

    <div class="mb-1.5 text-sm">选择成员（P2P 组网）</div>
    <div class="max-h-56 overflow-y-auto">
      <div
        v-for="f in chat.friends"
        :key="f.device_id"
        class="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-2 transition hover:bg-[var(--gosslan-hover)]"
        @click="toggle(f.device_id)"
      >
        <div
          class="flex h-4 w-4 items-center justify-center rounded border"
          :class="selected.includes(f.device_id) ? 'border-primary bg-primary' : 'border-[var(--gosslan-border)]'"
        >
          <Check v-if="selected.includes(f.device_id)" class="h-3 w-3 text-white" />
        </div>
        <div class="flex h-8 w-8 items-center justify-center overflow-hidden rounded-full bg-primary text-white">
          <img v-if="f.avatar" :src="f.avatar" class="h-full w-full object-cover" />
          <span v-else class="text-xs font-semibold">{{ initials(f.nickname) }}</span>
        </div>
        <span class="flex-1 text-sm">{{ f.nickname }}</span>
        <span class="text-xs text-[var(--gosslan-text-2)]">{{ f.online ? "在线" : "离线" }}</span>
      </div>
      <div v-if="chat.friends.length === 0" class="py-6 text-center text-sm text-[var(--gosslan-text-2)]">
        暂无好友，请先添加好友
      </div>
    </div>

    <button
      class="mt-4 w-full rounded-xl bg-primary py-2.5 text-sm font-medium text-white transition hover:bg-primary-hover disabled:opacity-40"
      :disabled="selected.length === 0 || !name.trim()"
      @click="create"
    >
      创建群聊
    </button>
  </BaseModal>
</template>
