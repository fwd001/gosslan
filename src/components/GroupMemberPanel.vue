<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import { useMemberProfile } from "@/composables/useMemberProfile";
import { avatarInitial, avatarInitialLen, nameToColor } from "@/utils/color";
import { TODO_STATUSES, TODO_STATUS_CLASS, TODO_STATUS_LABEL_KEY, foldTodos } from "@/utils/todos";
import { ArrowRightLeft, Crown, ListChecks, LogOut, Plus, UserMinus, X } from "lucide-vue-next";
import type { Friend } from "@/types";

const props = defineProps<{ open: boolean; groupId: string | null }>();
const emit = defineEmits<{ (e: "close"): void; (e: "open-tasks"): void }>();

const app = useAppStore();
const chat = useChatStore();
const { memberProfile, myId } = useMemberProfile();
const group = computed(() => chat.groups.find((g) => g.id === props.groupId) ?? null);

/**
 * 任务分区的一行摘要：各状态计数 + 完成数。
 *
 * 折叠口径与群任务面板完全一致（同一个 `foldTodos`，同一份会话消息）——
 * 只是这里只用来显示"有几项、什么状态"，完整列表在 `GroupTasksPanel` 里。
 */
const todoSummary = computed(() => {
  const items = props.groupId ? foldTodos(chat.messages[`group:${props.groupId}`] ?? []) : [];
  const byStatus: Record<string, number> = {};
  for (const it of items) byStatus[it.status] = (byStatus[it.status] ?? 0) + 1;
  return { total: items.length, done: byStatus.done ?? 0, byStatus };
});
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

// ---------------- 群名称（用户 2026-09-17：改名并入本弹窗，一个弹窗管两件事） ----------------

/** 编辑中的群名（打开面板时预填当前名）。 */
const nameDraft = ref("");
const renaming = ref(false);

watch(
  () => [props.open, group.value?.name] as const,
  ([open, name]) => {
    if (open) nameDraft.value = name ?? "";
  },
  { immediate: true },
);

async function saveName() {
  const gid = props.groupId;
  const name = nameDraft.value.trim();
  if (!gid || !name || name === group.value?.name) return;
  renaming.value = true;
  try {
    await chat.renameGroup(gid, name);
    app.toast(t("chat.toast.groupRenamed"), "success");
  } catch (e) {
    app.toastError(e, t("chat.toast.renameFail"));
  } finally {
    renaming.value = false;
  }
}

// ---------------- 群公告（用户 2026-09-17：发布/修改并入本弹窗） ----------------
// 横幅只在**有公告**时出现；没有公告时聊天区不再常驻一条空横幅，发布入口收进这里。

const announceDraft = ref("");
const publishing = ref(false);
/** 当前生效的公告（后端全量折叠，store 缓存成 map）。 */
const currentAnnouncement = computed(() =>
  props.groupId ? (chat.activeAnnouncements.get(props.groupId) ?? null) : null,
);

watch(
  () => [props.open, currentAnnouncement.value?.text] as const,
  ([open, text]) => {
    if (open) announceDraft.value = text ?? "";
  },
  { immediate: true },
);

async function publishAnnouncement() {
  const gid = props.groupId;
  const text = announceDraft.value.trim();
  if (!gid || !text) return;
  publishing.value = true;
  try {
    await chat.publishAnnouncement(gid, text);
    app.toast(t("group.announceDone"), "success");
  } catch (e) {
    app.toastError(e, t("group.announceFail"));
  } finally {
    publishing.value = false;
  }
}

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
  <!-- 群管理面板比一般弹窗内容多（群名/公告/成员/添加/转让），用更宽的一档：
       默认 max-w-md 会挤成"瘦高"一条（用户 2026-09-17）。 -->
  <BaseModal :open="open" width="max-w-lg" :title="group ? t('group.membersCount', { n: group.members.length }) : t('group.members')" @close="emit('close')">
    <div v-if="group" class="space-y-3">
      <!-- 群名称：群主可改（用户 2026-09-17：改名并入本弹窗）。
           非群主**只读文本** —— 没权限就不给一个灰掉的输入框（改不了还长得像能改）。 -->
      <div>
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("group.rename.title") }}</div>
        <div v-if="isOwner" class="flex items-center gap-2">
          <input
            v-model="nameDraft"
            maxlength="40"
            class="w-full rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none transition focus:border-transparent"
            :placeholder="t('group.namePlaceholder')"
            @keydown.enter.prevent="saveName"
          />
          <button
            type="button"
            class="tap-safe shrink-0 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-3 py-2 text-[13px] text-white transition hover:bg-[var(--gosslan-primary-hover)] disabled:opacity-50"
            :disabled="renaming || !nameDraft.trim() || nameDraft.trim() === group.name"
            @click="saveName"
          >
            {{ t("common.save") }}
          </button>
        </div>
        <p v-else class="px-3 py-2 text-sm text-[var(--gosslan-text)]">{{ group.name }}</p>
      </div>

      <!-- 群公告：发布/修改入口（仅群主）。聊天区的横幅只在**有公告**时出现 ——
           没有公告时不常驻空横幅，发布入口收进这里（用户 2026-09-17）。 -->
      <div v-if="isOwner">
        <div class="mb-1.5 text-xs text-[var(--gosslan-text-2)]">{{ t("group.announce") }}</div>
        <textarea
          v-model="announceDraft"
          maxlength="500"
          rows="3"
          class="w-full resize-none rounded-[var(--gosslan-radius-md)] border border-transparent bg-[var(--gosslan-bg)] px-3 py-2 text-sm outline-none transition focus:border-transparent"
          :placeholder="t('group.announcePlaceholder')"
        ></textarea>
        <div class="mt-2 flex justify-end">
          <button
            type="button"
            class="tap-safe rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-primary)] px-3 py-1.5 text-[13px] text-white transition hover:bg-[var(--gosslan-primary-hover)] disabled:opacity-50"
            :disabled="publishing || !announceDraft.trim()"
            @click="publishAnnouncement"
          >
            {{ currentAnnouncement ? t("group.announceEdit") : t("group.announcePublish") }}
          </button>
        </div>
      </div>

      <!-- 当前成员 -->
      <div class="max-h-56 overflow-y-auto">
        <div
          v-for="id in group.members"
          :key="id"
          class="flex items-center gap-2.5 rounded-[var(--gosslan-radius-md)] px-2 py-2"
        >
          <div class="relative shrink-0">
            <div
              class="gosslan-avatar-box flex h-9 w-9 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
              :class="!memberProfile(id).online ? 'grayscale opacity-70' : ''"
              :style="{ backgroundColor: nameToColor(memberProfile(id).name) }"
            >
              <img alt="" v-if="memberProfile(id).avatar" :src="memberProfile(id).avatar ?? undefined" class="h-full w-full object-cover" />
              <span v-else class="gosslan-avatar-initial text-xs font-semibold" :data-len="avatarInitialLen(memberProfile(id).name)">{{ initials(memberProfile(id).name) }}</span>
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

      <!-- 群任务分区：只给"有几项、各什么状态"+入口，完整列表在 GroupTasksPanel
           （成员面板已经装了成员列表 + 添加成员，再塞一张任务清单会把两个用途挤在一起） -->
      <div class="rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-border)] p-2.5">
        <div class="flex items-center justify-between gap-2">
          <div class="flex min-w-0 items-center gap-1.5 text-[13px] font-medium">
            <ListChecks class="h-4 w-4 shrink-0 text-[var(--gosslan-text-2)]" aria-hidden="true" />
            <span class="truncate" :title="t('todo.title')">{{ t("todo.title") }}</span>
            <span v-if="todoSummary.total" class="shrink-0 text-[11px] font-normal text-[var(--gosslan-text-2)]">
              {{ t("todo.doneCount", { n: todoSummary.done, total: todoSummary.total }) }}
            </span>
          </div>
          <button
            class="tap-safe shrink-0 rounded-[var(--gosslan-radius-md)] px-2 py-1 text-[12px] text-[var(--gosslan-primary)] transition hover:bg-[var(--gosslan-hover)]"
            @click="emit('open-tasks')"
          >
            {{ todoSummary.total ? t("todo.viewAll") : t("todo.create") }}
          </button>
        </div>
        <div v-if="todoSummary.total" class="mt-1.5 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[11px]">
          <span
            v-for="s in TODO_STATUSES"
            v-show="todoSummary.byStatus[s]"
            :key="s"
            :class="TODO_STATUS_CLASS[s]"
          >
            {{ t(TODO_STATUS_LABEL_KEY[s]) }} {{ todoSummary.byStatus[s] }}
          </span>
        </div>
        <div v-else class="mt-1.5 text-[11px] text-[var(--gosslan-text-2)]">{{ t("todo.empty") }}</div>
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
            <!-- 真按钮（不是 `div @click`）：键盘要能 Tab 到并回车添加 -->
            <button
              v-for="f in addableFriends"
              :key="f.device_id"
              type="button"
              class="flex w-full cursor-pointer items-center gap-2 rounded-[var(--gosslan-radius-md)] px-2 py-1.5 text-left transition hover:bg-[var(--gosslan-hover)]"
              @click="addMember(f)"
            >
              <div
                class="gosslan-avatar-box flex h-7 w-7 shrink-0 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
                :style="{ backgroundColor: nameToColor(f.nickname) }"
              >
                <img alt="" v-if="f.avatar" :src="f.avatar" class="h-full w-full object-cover" />
                <span v-else class="gosslan-avatar-initial text-[11px] font-semibold" :data-len="avatarInitialLen(f.nickname)">{{ initials(f.nickname) }}</span>
              </div>
              <span class="min-w-0 flex-1 truncate text-sm" :title="f.nickname">{{ f.nickname }}</span>
              <Plus class="h-3.5 w-3.5 shrink-0 text-[var(--gosslan-text-2)]" />
            </button>
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
          class="rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-danger)] px-3 py-1.5 text-sm text-white transition hover:bg-[var(--gosslan-danger-hover)]"
          @click="confirmAction"
        >
          {{ t("common.confirm") }}
        </button>
      </div>
    </template>
  </BaseModal>
</template>
