<script setup lang="ts">
import { t } from "@/i18n";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useChatStore } from "@/stores/useChatStore";
import { useExclusivePopup } from "@/composables/useExclusivePopup";
import { useHoverCard } from "@/composables/useHoverCard";
import { avatarSeedFor } from "@/utils/avatarSeed";
import type { SendState } from "@/composables/useMessageDisplay";
import { Check, CheckCheck, Loader2, X } from "lucide-vue-next";

const props = defineProps<{
  state: SendState;
  title: string;
  /** 群聊：显示实际已读成员头像；单聊显示单个状态图标。 */
  isGroup?: boolean;
  /** 已读该消息的成员 ID（由父组件按消息时间计算）。 */
  readerIds?: string[];
  /** 消息唯一键：作为全局展开互斥的键。 */
  msgKey?: string | number;
}>();
const emit = defineEmits<{ (e: "retry"): void }>();

const chat = useChatStore();
const readerIds = computed(() => props.readerIds ?? []);
const visibleReaders = computed(() => readerIds.value.slice(0, 3));
const extraReaders = computed(() => readerIds.value.slice(3));

/**
 * 「已读成员」弹层：接入全局浮层互斥（同一时刻只允许一个弹层展开）。
 * 点另一条消息的已读头像会自动收起上一条，虚拟列表回收行时也不会残留；
 * 与右键菜单/表情面板同属一套机制，互相之间也会自动收起。
 * 模板里用到 `readersOpen`，故解构出来（模板不会自动解包嵌套在对象里的 ref）。
 */
const {
  isActive: readersOpen,
  claim: claimReaders,
  release: releaseReaders,
} = useExclusivePopup(`readers:${props.msgKey ?? ""}`);

/**
 * 弹层摆位与收起：**Teleport 到 body + fixed 坐标**，走通用那一套
 * （`composables/useHoverCard` → `utils/hoverCard.placeCard`），与表情名单同一个家。
 *
 * 原来它挂在消息行里（`absolute right-0`），而消息列表是 `overflow-y: auto` 的滚动容器
 * ⇒ 横向一并被裁（2026-09-24 真机：长文字消息旁"已读列表靠右被裁掉一半"）。
 *
 * ⚠️ 共用之后横向**不再写死"永远右对齐"**：入口在视口左半时改成往右长，
 * 只有真贴右缘才向左长 —— 这正是 placeCard 存在的理由（名单那一处先撞出来的）。
 */
const {
  pos: readersPos,
  style: readersStyle,
  setEl: setReadersEl,
  open: openReadersCard,
  close: closeReadersCard,
} = useHoverCard({ maxWidth: 208 });
/** 入口按钮：摆位要从它的矩形现算。 */
const readersBtnRef = ref<HTMLElement | null>(null);

/** 关闭时坐标与展开权一起还掉：`v-if` 关掉后 Teleport 的那层要跟着消失。 */
function closeReaders() {
  releaseReaders();
  closeReadersCard();
}

function toggleReaders() {
  if (readersOpen.value) {
    closeReaders();
    return;
  }
  const btn = readersBtnRef.value;
  if (btn) openReadersCard(btn);
  claimReaders();
}

function onDocClick() {
  closeReaders();
}
onMounted(() => document.addEventListener("click", onDocClick));
onUnmounted(() => document.removeEventListener("click", onDocClick));

// 两条 watch 各管一个方向，合起来才等价于"看不见 ⇔ 不持有展开权"：
// · 滚动 / 改窗口只清得掉**坐标**（收起住在 useHoverCard 里）⇒ 坐标一空就把展开权还掉；
// · 别的浮层抢走展开权、或虚拟列表回收这一行，只改得了 **isActive** ⇒ 反过来把坐标清掉。
// 只留一条就会有一种"两个浮层并存"或"占着展开权却什么都看不见"的形态。
watch(readersPos, (p) => {
  if (!p) closeReaders();
});
watch(readersOpen, (mine) => {
  if (!mine) closeReadersCard();
});

function readerName(id: string) {
  return chat.nicknameOf(id);
}
function readerAvatar(id: string): string | null {
  const friend = chat.friends.find((item) => item.device_id === id);
  if (friend?.avatar) return friend.avatar;
  return chat.peers.find((item) => item.device_id === id)?.avatar ?? null;
}
</script>

<template>
  <!-- 群聊：已读成员头像 + 展开完整列表 -->
  <div v-if="isGroup" class="relative shrink-0 pb-1.5">
    <button
      v-if="readerIds.length > 0"
      ref="readersBtnRef"
      class="tap-safe -space-x-1 flex items-center rounded-full p-0.5 transition hover:bg-[var(--gosslan-hover)]"
      :title="t('msg.readBy', { n: readerIds.length })"
      :aria-label="t('msg.readByView', { n: readerIds.length })"
      @click.stop="toggleReaders"
    >
      <span
        v-for="id in visibleReaders"
        :key="id"
        class="gosslan-avatar-box flex h-4 w-4 items-center justify-center overflow-hidden rounded-full border border-[var(--gosslan-panel)] text-[11px] text-white"
        :style="{ backgroundColor: avatarSeedFor(id)?.bg }"
      >
        <img alt="" v-if="readerAvatar(id)" :src="readerAvatar(id) ?? undefined" class="h-full w-full object-cover" />
        <span v-else class="gosslan-avatar-emoji" aria-hidden="true">{{ avatarSeedFor(id)?.emoji }}</span>
      </span>
      <span
        v-if="extraReaders.length > 0"
        class="ml-1 rounded-full bg-[var(--gosslan-hover)] px-1 text-[11px] text-[var(--gosslan-text-2)]"
      >
        +{{ extraReaders.length }}
      </span>
    </button>
    <!-- Teleport 到 body + fixed 坐标：挂在消息行里会被列表的 `overflow-y: auto`
         连横向一起裁掉（真机：「已读列表靠右被裁一半」）。坐标由上面的 `useHoverCard`
         在点开那一刻从入口按钮的矩形现算，摆位规则见 `utils/hoverCard.placeCard`。 -->
    <Teleport to="body">
      <div
        v-if="readersOpen && readerIds.length > 0 && readersPos"
        :ref="setReadersEl"
        data-readers-popover
        class="frost fixed z-[70] max-h-60 overflow-y-auto rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] p-1.5 text-xs shadow-lg"
        :style="readersStyle"
        @click.stop
      >
      <div class="px-2 py-1 text-[var(--gosslan-text-2)]">{{ t("msg.readMembers", { n: readerIds.length }) }}</div>
      <div
        v-for="id in readerIds"
        :key="id"
        class="flex items-center gap-2 rounded-[var(--gosslan-radius-xs)] px-2 py-1 hover:bg-[var(--gosslan-hover)]"
      >
        <span
          class="gosslan-avatar-box flex h-5 w-5 items-center justify-center overflow-hidden rounded-full text-[11px] text-white"
          :style="{ backgroundColor: avatarSeedFor(id)?.bg }"
        >
          <img alt="" v-if="readerAvatar(id)" :src="readerAvatar(id) ?? undefined" class="h-full w-full object-cover" />
          <span v-else class="gosslan-avatar-emoji" aria-hidden="true">{{ avatarSeedFor(id)?.emoji }}</span>
        </span>
        <span class="max-w-28 truncate" :title="readerName(id)">{{ readerName(id) }}</span>
      </div>
      </div>
    </Teleport>
  </div>
  <!-- 单聊：回执图标固定在气泡左侧（视觉上贴近对话人头像方向）。
       ♿ 回执是**纯图标**状态（转圈/空心圆/绿勾/红叉），读屏下原本什么也读不到 ——
       发送中 / 已送达 / 已读 是聊天最核心的状态，必须给可访问名。
       ⚠️ `role="img"` 只能加在**只包静态图标**的元素上：ARIA 的 `img` 会应用
       *Children Presentational*，把后代的角色/名字/动作**从无障碍树里抹掉**。
       此前它套在含「重发」按钮的外层 ⇒ 读屏用户**点不到重发**（失败消息无法重发）。
       现在：按钮是兄弟节点，`role="img"` 只包状态图标。 -->
  <!-- 单聊回执图标：借鉴 Telegram 的状态视觉语言
       （单勾=已发到链路、双勾=对方已收到、双勾变主题色=对方已读）。
       ♿ 回执是纯图标状态，aria-label 提供语义。 -->
  <span v-else class="flex shrink-0 items-center pb-1.5">
    <!-- 失败：红叉 + 可点重发（两个并列小图标，X 在左、RefreshCw 在右）。
         之前用 RefreshCw 单独当"失败"图标，但 RefreshCw 的语义是「刷新/重试」，
         而不是「这条消息失败了」—— 用户需要的是明确的失败感 + 可修复入口。 -->
    <button
      v-if="state === 'failed'"
      class="tap-safe flex h-5 w-5 items-center justify-center rounded-[var(--gosslan-radius-xs)] text-[var(--gosslan-danger-ink)] transition hover:bg-[var(--gosslan-danger-soft)]"
      :title="t('msg.resend')" :aria-label="t('msg.resend')"
      @click="emit('retry')"
    >
      <X class="h-4 w-4" stroke-width="2.5" />
    </button>
    <span v-else class="relative flex items-center" role="img" :title="title" :aria-label="title">
      <!-- sending：单勾右下角叠一个小 spinner —— 表达"已经在发但还没到位"。
           TG 用的是纯 spinner，但纯 spinner 在气泡上很不显眼（小 + 灰）；
           加一个勾让用户一眼就知道这不是空状态。 -->
      <template v-if="state === 'sending'">
        <Check class="h-3.5 w-3.5 text-[var(--gosslan-text-3)]" />
        <Loader2 class="absolute -right-1 -bottom-0.5 h-2.5 w-2.5 animate-spin text-[var(--gosslan-text-2)]" />
      </template>
      <!-- sent：单勾（灰色）—— 已经发出但还没到对方设备。我们 P2P 链路下 sent 到 delivered
           之间几乎没有停留（写出去 = 对方收到），所以这状态通常一闪而过。 -->
      <Check v-else-if="state === 'sent'" class="h-3.5 w-3.5 text-[var(--gosslan-text-2)]" />
      <!-- delivered：双勾灰色 —— 对方设备已收到但还没打开看。 -->
      <CheckCheck v-else-if="state === 'delivered'" class="h-4 w-4 text-[var(--gosslan-text-2)]" />
      <!-- read：双勾主题色 —— 对方已读（TG 是蓝色，我们用 app 主题色）。
           用 primary 而不是 success：绿色在深色模式下容易跟"在线"状态混淆；
           TG 选蓝色就是为了跟"送达灰色"形成明确对比。 -->
      <CheckCheck v-else-if="state === 'read'" class="h-4 w-4 text-[var(--gosslan-primary)]" />
    </span>
  </span>
</template>
