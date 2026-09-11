<script setup lang="ts">
import { t } from "@/i18n";
import { computed, ref, watch } from "vue";
import { useDeferredRef } from "@/composables/useDeferredRef";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import BaseModal from "@/components/BaseModal.vue";
import { avatarInitial, nameToColor } from "@/utils/color";
import { Check, UserPlus, X } from "lucide-vue-next";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

const app = useAppStore();
const chat = useChatStore();
const loading = ref(false);
const keyword = ref("");
/**
 * 延迟镜像：过滤用。
 * 连发粘贴时关键词每秒变十几次，而每次变化都要重算并重渲染候选列表 ⇒
 * 输入框自己的 caret 会掉帧。原值即时、列表延迟，输入就始终跟手。
 */
const query = useDeferredRef(keyword);

// 大规模局域网（设计规模 500–1000 节点）下最多渲染多少行。
//
// 原先这里截断到 200 行并提示"可用搜索缩小范围"——但那等于第 201 个之后的节点
// 压根找不到，是真实的功能缺口。现在行上加了 `content-visibility: auto`：
// 离屏行不参与布局与绘制，多渲染几百行几乎不花代价（节点表本身也已按 300ms 节流推送），
// 所以上限提到设计规模本身。保留上限只为挡住异常膨胀的节点表（例如广播洪水）。
const MAX_RENDER = 1000;

const friendIds = computed(() => new Set(chat.friends.map((f) => f.device_id)));

/**
 * 「对方已经先申请加我」的 device_id 集合（用户需求 2026-09-12 第 18 条）。
 *
 * 用户原话：「当你在添加好友的时候，就是别人刚好先添加你为好友了。你在添加好友的那个界面，
 * 那个人的名字后面应该就直接变成『同意』或者『拒绝』……你不用再关掉『添加好友』再去
 * 『新朋友』里添加他，而是在『添加好友』界面，别人向你申请的那条搜到好友的记录，
 * 就直接变成了可以直接通过或拒绝。你通过之后，如果加好友成功的话，那这一行就变成
 * 已经加过好友的那种列表状态了。如果你拒绝的话，那对方也会收到拒绝信息，
 * 这条就变成你可以再去添加。」
 */
const pendingFromIds = computed(() => new Set(chat.pendingRequests.map((r) => r.from)));

/** 正在处理中的 device_id（防连点，避免同一申请被提交两次）。 */
const responding = ref<Record<string, boolean>>({});

async function respond(peerId: string, accept: boolean) {
  if (responding.value[peerId]) return;
  responding.value = { ...responding.value, [peerId]: true };
  try {
    await chat.respondRequest(peerId, accept);
    // 同意 → 好友表刷新后本行自动变「已加好友」（friendIds 重算）；
    // 拒绝 → 申请从 pendingRequests 移除后本行自动变回「加好友」可再次添加。
    // 两者都不需要额外的本地状态。
    app.toast(
      accept ? t("friend.add.toast.accepted") : t("friend.add.toast.rejected"),
      "success",
    );
  } catch (e) {
    app.toastError(e, t("common.operationFail"));
  } finally {
    const next = { ...responding.value };
    delete next[peerId];
    responding.value = next;
  }
}

const filteredPeers = computed(() => {
  const k = query.value.trim().toLowerCase();
  const pool = k
    ? chat.peers.filter(
        (p) =>
          p.nickname.toLowerCase().includes(k) ||
          p.ip.toLowerCase().includes(k) ||
          p.device_id.toLowerCase().includes(k),
      )
    : chat.peers;
  return { total: pool.length, list: pool.slice(0, MAX_RENDER) };
});

watch(
  () => props.open,
  async (v) => {
    if (v) {
      keyword.value = "";
      loading.value = true;
      try {
        await chat.searchNearbyPeers(); // 按需 who_has 群发探测
      } catch (e) {
        // 不接住的话：loading 永远停在 true（弹窗卡在「正在扫描…」），
        // 并且变成一个 unhandled rejection。
        app.toastError(e, t("friend.add.scanFail"));
      } finally {
        loading.value = false;
      }
    }
  },
);

function initials(name: string) {
  return avatarInitial(name);
}

/** 发送好友申请后的冷却期：同一节点 3 秒内置灰防连点（显示「完成」）。 */
const SEND_COOLDOWN_MS = 3000;
const cooldown = ref<Record<string, number>>({});

function inCooldown(peerId: string): boolean {
  return Date.now() - (cooldown.value[peerId] ?? 0) < SEND_COOLDOWN_MS;
}

async function add(peerId: string) {
  if (inCooldown(peerId)) return; // 防抖：3 秒内不重复发送
  cooldown.value = { ...cooldown.value, [peerId]: Date.now() };
  setTimeout(() => {
    const next = { ...cooldown.value };
    delete next[peerId];
    cooldown.value = next;
  }, SEND_COOLDOWN_MS);
  try {
    await chat.sendFriendRequest(peerId);
    app.toast(t("friend.add.toast.sent"), "success");
  } catch (e) {
    app.toastError(e, t("msg.sendFailed"));
  }
}
</script>

<template>
  <BaseModal :open="open" :title="t('friend.add.title')" @close="emit('close')">
    <div v-if="loading" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
      {{ t("friend.add.scanning") }}
    </div>

    <div v-else-if="chat.peers.length === 0" class="py-8 text-center text-sm text-[var(--gosslan-text-2)]">
      {{ t("friend.add.noPeers") }}
    </div>

    <template v-else>
      <input
        v-model="keyword"
        maxlength="100"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
        class="mb-2 w-full rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-bg)] px-3 py-2 text-sm"
        :placeholder="t('friend.add.searchPlaceholder')"
      />
      <div class="max-h-72 overflow-y-auto">
        <div
          v-for="p in filteredPeers.list"
          :key="p.device_id"
          class="flex items-center gap-3 border-b border-[var(--gosslan-border)] px-1 py-2 last:border-0 [contain-intrinsic-size:auto_53px] [content-visibility:auto]"
        >
          <div
            class="flex h-9 w-9 items-center justify-center overflow-hidden rounded-[var(--gosslan-avatar-radius)] text-white"
            :style="{ backgroundColor: nameToColor(p.nickname) }"
          >
            <img alt="" v-if="p.avatar" :src="p.avatar" class="h-full w-full object-cover" />
            <span v-else class="text-sm font-semibold">{{ initials(p.nickname) }}</span>
          </div>
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm" :title="p.nickname">{{ p.nickname }}</div>
            <div class="text-xs text-[var(--gosslan-text-2)]">{{ p.ip }}</div>
          </div>
          <span
            v-if="friendIds.has(p.device_id)"
            class="flex items-center gap-1 rounded-[var(--gosslan-radius-md)] px-3 py-1.5 text-xs font-medium text-[var(--gosslan-text-2)]"
          >
            <Check class="h-3.5 w-3.5" />
            {{ t("friend.add.alreadyFriend") }}
          </span>
          <!-- 对方已先申请加我 → 直接同意/拒绝（与「新朋友」页同一套动作） -->
          <span v-else-if="pendingFromIds.has(p.device_id)" class="flex shrink-0 items-center gap-1.5">
            <button
              class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--gosslan-avatar-radius)] bg-primary text-white transition hover:bg-primary-hover disabled:opacity-50"
              :title="t('common.agree')" :aria-label="t('common.agree')"
              :disabled="responding[p.device_id]"
              @click="respond(p.device_id, true)"
            >
              <Check class="h-4 w-4" />
            </button>
            <button
              class="tap-safe flex h-7 w-7 shrink-0 items-center justify-center rounded-full border border-[var(--gosslan-border)] text-[var(--gosslan-text-2)] transition hover:bg-[var(--gosslan-hover)] disabled:opacity-50"
              :title="t('common.reject')" :aria-label="t('common.reject')"
              :disabled="responding[p.device_id]"
              @click="respond(p.device_id, false)"
            >
              <X class="h-4 w-4" />
            </button>
          </span>
          <button
            v-else-if="inCooldown(p.device_id)"
            disabled
            class="flex cursor-default items-center gap-1 rounded-[var(--gosslan-radius-md)] bg-[var(--gosslan-hover)] px-3 py-1.5 text-xs font-medium text-[var(--gosslan-text-2)]"
          >
            <Check class="h-3.5 w-3.5" />
            {{ t("friend.add.done") }}
          </button>
          <button
            v-else
            class="tap-safe flex items-center gap-1 rounded-[var(--gosslan-radius-md)] bg-primary px-3 py-1.5 text-xs font-medium text-white transition hover:bg-primary-hover"
            @click="add(p.device_id)"
          >
            <UserPlus class="h-3.5 w-3.5" />
            {{ t("friend.add.add") }}
          </button>
        </div>
      </div>
      <div
        v-if="filteredPeers.total > MAX_RENDER"
        class="mt-1 text-center text-xs text-[var(--gosslan-text-2)]"
      >
        {{ t("friend.add.truncated", { shown: MAX_RENDER, total: filteredPeers.total }) }}
      </div>
    </template>
  </BaseModal>
</template>
