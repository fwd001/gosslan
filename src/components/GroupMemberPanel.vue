<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { avatarInitial, nameToColor } from "@/utils/color";
import { ArrowRightLeft, Crown, LogOut, Plus, UserMinus, X } from "lucide-vue-next";
import type { Friend } from "@/types";

const props = defineProps<{ open: boolean; groupId: string | null }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const { memberProfile, myId } = useMemberProfile();
const group = computed(() => chat.groups.find((g) => g.id === props.groupId) ?? null);
/** 当前用户是否为群主（可见「添加/移除成员」操作） */
const isOwner = computed(() => !!group.value && group.value.creator === myId.value);
/** 展示「添加成员」面板 */
const showAdd = ref(false);
/** 待确认的破坏性操作（转让群主 / 移除成员 / 退出群聊）。null = 无弹窗。
 *  「移除成员」是**对别人生效**的操作，一次点击就执行不合适（HIG：让用户容易从错误中恢复）。 */
const pendingConfirm = ref<
  | null
  | { kind: "transfer"; targetId: string; name: string }
  | { kind: "remove"; targetId: string; name: string }
  | { kind: "leave"; name: string }
>(null);

watch(
  () => props.open,
  (v) => {
    if (v) {
      showAdd.value = false;
      if (props.groupId) void chat.refreshGroups();
    }
  },
);

function initials(n: string) {
  return avatarInitial(n);
}

/** 成员资料解析统一走 useMemberProfile（本机/好友表/在线节点），此处不再重复实现。 */

/** 可用于加入该群的好友 = 好友 - 已是成员 - 自己 */
const addableFriends = computed(() => {
  const members = group.value?.members ?? [];
  return chat.friends.filter((f) => !members.includes(f.device_id) && f.device_id !== myId.value);
});

async function addMember(f: Friend) {
  if (!props.groupId) return;
  try {
    await chat.addGroupMember(props.groupId, f.device_id);
    app.toast(t("group.toast.added", { name: f.nickname }), "success");
  } catch (e) {
    app.toastError(e, t("group.toast.addFail"));
  }
}

/** 移除成员：先弹确认，再由 `confirmAction` 执行（复用本组件既有机制）。 */
function askRemoveMember(id: string) {
  pendingConfirm.value = { kind: "remove", targetId: id, name: memberProfile(id).name };
}

/** 转让群主（仅当前群主）：把管理权交给指定成员，避免换机后群无法管理。 */
function transferOwner(id: string) {
  if (!props.groupId) return;
  const p = memberProfile(id);
  pendingConfirm.value = { kind: "transfer", targetId: id, name: p.name };
}

/** 退出群聊（群主须先转让，后端会拒绝并给出提示）。 */
function leaveGroup() {
  if (!props.groupId) return;
  const name = group.value?.name ?? t("group.thisGroup");
  pendingConfirm.value = { kind: "leave", name };
}

/** 确认弹窗的「确定」：按类型执行真实的破坏性操作。 */
async function confirmAction() {
  const a = pendingConfirm.value;
  if (!a || !props.groupId) return;
  pendingConfirm.value = null;
  if (a.kind === "transfer") {
    try {
      await chat.transferGroupCreator(props.groupId, a.targetId);
      app.toast(t("group.toast.transferred", { name: a.name }), "success");
    } catch (e) {
      app.toastError(e, t("group.toast.transferFail"));
    }
  } else if (a.kind === "remove") {
    try {
      await chat.removeGroupMember(props.groupId, a.targetId);
      app.toast(t("group.toast.removed", { name: a.name }), "success");
    } catch (e) {
      app.toastError(e, t("group.toast.removeFail"));
    }
  } else {
    try {
      await chat.leaveGroup(props.groupId);
      app.toast(t("group.toast.left"), "success");
      emit("close");
    } catch (e) {
      app.toastError(e, t("group.toast.leaveFail"));
    }
  }
}
</script>

<template>
  <BaseModal :open="open" :title="group ? t('group.membersCount', { n: group.members.length }) : t('group.members')" @close="emit('close')">
    <div v-if="group" class="space-y-3">
      <!-- 当前成员 -->
      <div class="max-h-56 overflow-y-auto">
        <div
          v-for="id in group.members"
          :key="id"
          class="flex items-center gap-2.5 rounded-[var(--gosslan-radius-md)] px-2 py-2"
        >
          <div class="relative shrink-0">
            <div
              class="flex h-9 w-9 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
              :class="!memberProfile(id).online ? 'grayscale opacity-70' : ''"
              :style="{ backgroundColor: nameToColor(memberProfile(id).name) }"
            >
              <img alt="" v-if="memberProfile(id).avatar" :src="memberProfile(id).avatar ?? undefined" class="h-full w-full object-cover" />
              <span v-else class="text-xs font-semibold">{{ initials(memberProfile(id).name) }}</span>
            </div>
            <span
              class="absolute bottom-0 right-0 h-2.5 w-2.5 rounded-full border-2 border-[var(--gosslan-panel)]"
              :class="memberProfile(id).online ? 'bg-[var(--gosslan-success)]' : 'bg-[var(--gosslan-status-offline)]'"
            ></span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-1.5">
              <span class="truncate text-sm font-medium" :title="memberProfile(id).name">{{ memberProfile(id).name }}</span>
              <Crown v-if="group.creator === id" class="h-3.5 w-3.5 shrink-0 text-[var(--gosslan-warning-ink)]" :title="t('group.owner')" />
              <span v-if="id === myId" class="shrink-0 text-[11px] text-[var(--gosslan-text-2)]">{{ t("group.me") }}</span>
            </div>
            <div class="text-xs text-[var(--gosslan-text-2)]">
              {{ memberProfile(id).online ? t("common.online") : t("common.offline") }}
            </div>
          </div>
          <!-- 群主操作：转让群主 / 移除成员（不能操作自己/创建者本人） -->
          <button
            v-if="isOwner && id !== myId"
            class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-warning-soft)] hover:text-[var(--gosslan-warning-ink)]"
            :title="t('group.transferOwner', { name: memberProfile(id).name })" :aria-label="t('group.transferOwner', { name: memberProfile(id).name })"
            @click="transferOwner(id)"
          >
            <ArrowRightLeft class="h-4 w-4" />
          </button>
          <button
            v-if="isOwner && id !== myId"
            class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-radius-md)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-danger-soft)] hover:text-[var(--gosslan-danger-ink)]"
            :title="t('group.removeMember', { name: memberProfile(id).name })" :aria-label="t('group.removeMember', { name: memberProfile(id).name })"
            @click="askRemoveMember(id)"
          >
            <UserMinus class="h-4 w-4" />
          </button>
        </div>
        <div v-if="group.members.length === 0" class="py-6 text-center text-sm text-[var(--gosslan-text-2)]">
          {{ t("group.noMembers") }}
        </div>
      </div>

      <!-- 添加成员（仅群主）：切换出一列可选好友 -->
      <template v-if="isOwner">
        <button
          v-if="!showAdd"
          class="flex w-full items-center justify-center gap-1.5 rounded-[var(--gosslan-radius-lg)] border border-dashed border-[var(--gosslan-border)] py-2 text-sm text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
          @click="showAdd = true"
        >
          <Plus class="h-4 w-4" />
          {{ t("group.addMember") }}
        </button>
        <div v-else class="rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] p-2">
          <div class="mb-1 flex items-center justify-between px-1">
            <span class="text-xs font-medium text-[var(--gosslan-text-2)]">{{ t("group.selectFriends") }}</span>
            <button
              class="tap-safe flex items-center justify-center rounded-[var(--gosslan-radius-xs)] p-1 text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)]"
              :title="t('common.collapse')" :aria-label="t('common.collapse')"
              @click="showAdd = false"
            >
              <X class="h-3.5 w-3.5" />
            </button>
          </div>
          <div class="max-h-40 overflow-y-auto">
            <div
              v-for="f in addableFriends"
              :key="f.device_id"
              class="flex cursor-pointer items-center gap-2 rounded-[var(--gosslan-radius-md)] px-2 py-1.5 transition hover:bg-[var(--gosslan-hover)]"
              @click="addMember(f)"
            >
              <div
                class="flex h-7 w-7 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
                :style="{ backgroundColor: nameToColor(f.nickname) }"
              >
                <img alt="" v-if="f.avatar" :src="f.avatar" class="h-full w-full object-cover" />
                <span v-else class="text-[11px] font-semibold">{{ initials(f.nickname) }}</span>
              </div>
              <span class="min-w-0 flex-1 truncate text-sm" :title="f.nickname">{{ f.nickname }}</span>
              <Plus class="h-3.5 w-3.5 shrink-0 text-[var(--gosslan-text-2)]" />
            </div>
            <div v-if="addableFriends.length === 0" class="py-3 text-center text-xs text-[var(--gosslan-text-2)]">
              {{ t("group.allInGroup") }}
            </div>
          </div>
        </div>
      </template>

      <!-- 退出群聊：群主须先转让后再退出（后端会拒绝群主直接退群） -->
      <button
        v-if="!isOwner"
        class="flex w-full items-center justify-center gap-1.5 rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] py-2 text-sm text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-danger-soft)]"
        @click="leaveGroup"
      >
        <LogOut class="h-4 w-4" />
        {{ t("group.leave") }}
      </button>
      <p v-if="!isOwner" class="text-[11px] text-[var(--gosslan-text-2)]">
        {{ t("group.manageHint") }}
      </p>
      <p v-else class="text-[11px] text-[var(--gosslan-text-2)]">
        {{ t("group.leaveHint") }}
      </p>
    </div>
  </BaseModal>

  <!-- 破坏性操作二次确认（替代 window.confirm：应用内弹窗，与整体样式一致） -->
  <BaseModal
    :open="!!pendingConfirm"
    :title="pendingConfirm?.kind === 'transfer'
      ? t('group.confirmTransfer.title')
      : pendingConfirm?.kind === 'remove'
        ? t('group.confirmRemove.title')
        : t('group.confirmLeave.title')"
    @close="pendingConfirm = null"
  >
    <template v-if="pendingConfirm">
      <p class="text-sm leading-relaxed text-[var(--gosslan-text-2)]">
        <template v-if="pendingConfirm.kind === 'transfer'">
          {{ t("group.confirmTransfer.body", { name: pendingConfirm.name }) }}
        </template>
        <template v-else-if="pendingConfirm.kind === 'remove'">
          {{ t("group.confirmRemove.body", { name: pendingConfirm.name }) }}
        </template>
        <template v-else>
          {{ t("group.confirmLeave.body", { name: pendingConfirm.name }) }}
        </template>
      </p>
      <div class="mt-5 flex justify-end gap-2">
        <button
          class="rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] px-3 py-1.5 text-sm text-[var(--gosslan-text)] transition hover:bg-[var(--gosslan-hover)]"
          @click="pendingConfirm = null"
        >
          {{ t("common.cancel") }}
        </button>
        <button
          class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-3 py-1.5 text-sm text-white transition hover:opacity-90"
          @click="confirmAction"
        >
          {{ t("common.confirm") }}
        </button>
      </div>
    </template>
  </BaseModal>
</template>
