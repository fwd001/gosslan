<script setup lang="ts">
/**
 * 气泡下方的表情回应条（飞书/微信同款位置）。
 *
 * 为什么单独成条而不是把回应做成"一条消息"：回应是**状态**不是内容 —— 它挂在被回应的
 * 那条下面，且同一个人反复点只应看到最终结果。协议层它确实是一条条独立事件消息
 * （见 utils/reactions.ts 的说明），但**渲染层必须折叠**，否则群里回三个赞就多三条消息。
 *
 * 添加回应的入口不在本组件：飞书式入口（悬停消息 → 气泡外侧笑脸按钮 → 完整表情选择器）
 * 在 `MessageItem` 的消息行里 —— 入口必须挂在 `.group/msg` **之内**才能被悬停揭示，
 * 而本条是它的兄弟节点（用户 2026-09-21 报「表情回应在哪儿？没看见」）。
 *
 * ## 「谁点的」名单怎么露（用户 2026-09-29 需求汇总三）
 * 同一个表情多人点 ⇒ **聚合成一枚胶囊 + 数字**（不是一人一枚），名单按**追加先后**排，
 * 超过 `ROSTER_VISIBLE` 人折成「+N」。揭示方式按端给，两条都必须存在：
 * - **PC**：悬停 / 键盘聚焦（`hover: hover` 才启用，触屏上 mouseenter 是"点完才来"的假事件，
 *   用它当唯一出口就会在移动端变成"点一下既切了回应又弹名单"）；
 * - **移动端**：能点的时候点就是切回应（这是最高频动作，不能让位），所以名单走**长按**；
 *   不可点的那一侧点 = 直接看名单。
 *   ⚠️ 这一支今天真实存在，但**不再是"1:1"**：入口开关由调用方 `ChatWindow.vue` 的
 *   `canReact = 群聊 || (是对端好友 && 不是自聊)` 决定 —— 不可点的只剩「自聊」和「不是好友的对端」。
 *   （旧注释写的是"现在只有 1:1"，那是 2026-09-24 之前的口径，已经不对了。）
 * 名单不抢戏：不显示头像、不常驻、只有一行文字，因为这条的第一信息是"有几个、我点没点"。
 *
 * ## 胶囊的长相（用户 2026-09-29：「表情回应样式参考图片样式做」）
 * 贴飞书那一种：**没有阴影**、别人点的用一层墨迹淡底（`--gosslan-hover`）而不是一圈边框，
 * 只有"我自己也点了"那一枚描主色边 + 主色淡底。两态**都带 border**（别人那枚是透明边），
 * 否则我点一下会让胶囊宽窄跳 2px。表情比数字大一档，让表情本身是主角。
 */
import { computed, onUnmounted, ref, watch } from "vue";
import { t } from "@/i18n";
import { emojiUrl } from "@/utils/emoji";
import { summarizeActors, ROSTER_VISIBLE, type ReactionChip } from "@/utils/reactions";
import { useChatStore } from "@/stores/useChatStore";
import { useHoverCard } from "@/composables/useHoverCard";

const props = defineProps<{
  chips: ReactionChip[];
  /** 自己的消息：回应条靠右对齐（与气泡的朝向一致） */
  mine: boolean;
  /** 是否允许我添加/取消（不可点的那一支见上面「谁点的」名单那节；自己的消息也允许自嘲式回应） */
  interactive: boolean;
}>();
const emit = defineEmits<{
  (e: "toggle", emoji: string): void;
}>();

const chat = useChatStore();
/** 设备 id 对用户没有意义；取不到昵称才退回 id（宁可长也不许空）。 */
function nameOf(id: string): string {
  return chat.nicknameOf(id) || id;
}

/** 一行最多列几人，其余折成 +N（同一份判据也用于悬停 title，两处不能各写一个数）。 */
const visible = (c: ReactionChip) => summarizeActors(c.actors, nameOf, ROSTER_VISIBLE);

/**
 * 名单里**列出来**的那几人。上限必须和 `visible()` 是同一个数 ——
 * `summarizeActors` 内部取 `cap = max(1, limit)`，这里跟着取同一条，
 * 否则"列几个人"和"说还剩几个人"会各算各的（曾经就是各算各的：
 * 名单 `v-for` 全量 actor，末行又挂一个「等 N 人」⇒ 4 个名字 + "还有 1 人"）。
 */
const rosterIds = (c: ReactionChip) => c.actors.slice(0, Math.max(1, ROSTER_VISIBLE));

/** 胶囊上的原生 tooltip：数量 + 前几人 + 「+N」（PC 悬停的第一层，零成本、不占布局）。 */
function chipTitle(c: ReactionChip): string {
  const { shown, hidden } = visible(c);
  const who = shown.join("、");
  const rest = hidden > 0 ? ` ${t("msg.reactionPlus", { n: hidden })}` : "";
  return `${t("msg.reactionWho", { n: c.count })}：${who}${rest}`;
}

/** 同时只开一份名单：一条消息上三个表情全展开会把气泡区糊住。 */
const openEmoji = ref<string | null>(null);
/** 只有真有指针悬停能力的端才走 hover（触屏上 mouseenter 是"点完才来"的假事件）。 */
const canHover =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(hover: hover)").matches
    : false;

/**
 * 名单浮层：**Teleport 到 body + fixed 坐标**，摆位走通用那一套
 * （`composables/useHoverCard` → `utils/hoverCard.placeCard`）。
 *
 * 为什么要 Teleport（用户 2026-10-07 第二轮：「这个列表还是会被遮挡」）：原来它是
 * `position: absolute` 挂在胶囊旁边，而胶囊在消息列表的 `overflow: hidden` 容器里
 * ⇒ 名单被裁。表情选择器和已读成员弹层早就为同一个原因改成 Teleport + fixed
 * （`MessageItem.positionReactionPicker`，以及 `MessageReceipt` 里同样走 `useHoverCard` 的那一段）。
 *
 * 四个方向都按**这颗胶囊离哪条边近**现算，不跟消息朝向绑（一排十几颗会折行，
 * 同一侧既有贴左缘的也有贴右缘的）。宽度只给 `max-width` ⇒ 按名字长短撑开
 * （用户 2026-10-07：「框框是固定长度，名字不够长后面有空白」），超出上限交给行内 truncate。
 */
const { pos, style, setEl, open, close } = useHoverCard({ maxWidth: 256 });

/** 当前展开名单的那颗胶囊（Teleport 后不在 v-for 内部，按 emoji 回查）。 */
const activeChip = computed(() =>
  openEmoji.value
    ? (props.chips.find((c) => c.emoji === openEmoji.value) ?? null)
    : null,
);

/** 模板里传的是 `$event.currentTarget`，它的静态类型是 EventTarget ⇒ 只在这一处收窄。 */
function asEl(x: EventTarget | null | undefined): HTMLElement | null {
  return x instanceof HTMLElement ? x : null;
}

/**
 * 展开名单。**必须拿到胶囊元素**才摆得出四个方向 ⇒ 模板统一传 `$event.currentTarget`；
 * 拿不到时只置 `openEmoji` 不摆位，`v-if` 那头的 `pos` 还是 null，宁可不出
 * 也不要出一个贴在错误位置上的。
 */
function reveal(emoji: string, chipEl?: EventTarget | null) {
  openEmoji.value = emoji;
  const el = asEl(chipEl);
  if (el) open(el);
}
/**
 * 收起。`openEmoji` 与 `pos` 必须成对清 —— 只清一个就会留下
 * "胶囊挂着 `aria-expanded="true"` 而名单根本不在屏幕上"。
 * （`pos` 被滚动清掉时靠上面那条 watch 反向同步 `openEmoji`。）
 */
function dismiss() {
  openEmoji.value = null;
  close();
}
function hide(emoji: string) {
  if (openEmoji.value === emoji) dismiss();
}
function togglePanel(emoji: string, chipEl?: EventTarget | null) {
  if (openEmoji.value === emoji) dismiss();
  else reveal(emoji, chipEl);
}

/**
 * 长按揭示（移动端）。刻意不借 `utils/longPress.ts`：那两个函数判的是"消息行的长按菜单
 * 要不要吞掉松手"，与这里的"胶囊长按看名单"不是同一个开关 —— 共用会把两条链路绑成
 * 一条（长按胶囊会同时弹消息菜单和名单）。
 */
const LONG_PRESS_MS = 450;
let pressTimer: ReturnType<typeof setTimeout> | null = null;
/** 长按已触发 ⇒ 紧接着的那次 click 必须吃掉，否则"看个名单"会把回应点掉。 */
let heldEmoji: string | null = null;

function onPressStart(emoji: string, chipEl?: EventTarget | null) {
  if (!props.interactive || canHover) return;
  clearPress();
  const el = asEl(chipEl);
  pressTimer = setTimeout(() => {
    pressTimer = null;
    heldEmoji = emoji;
    reveal(emoji, el);
  }, LONG_PRESS_MS);
}
function clearPress() {
  if (pressTimer !== null) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
}
function onClick(c: ReactionChip, chipEl?: EventTarget | null) {
  clearPress();
  if (heldEmoji === c.emoji) {
    heldEmoji = null;
    return;
  }
  if (!props.interactive) {
    togglePanel(c.emoji, chipEl);
    return;
  }
  emit("toggle", c.emoji);
}

/**
 * `openEmoji` 与 `pos` 必须同开同关：`aria-expanded` 与守卫读的 `data-open-emoji`
 * 都按 `openEmoji` 判，而"看不看得见"按 `pos` 判。滚一下就只清掉后者的话，
 * 胶囊会挂着 `aria-expanded="true"` 而名单根本不在屏幕上。
 */
watch(pos, (p) => {
  if (!p) openEmoji.value = null;
});

onUnmounted(clearPress);
</script>

<template>
  <!-- 没有任何回应且不可交互时整条不渲染，避免给每条消息都留一行空白 -->
  <!-- 与气泡**左/右边缘对齐**（用户 2026-10-07：「左右两条表情回复边缘都与上面气泡对齐」）。
       本条是消息行的**兄弟节点**，所以要从容器左/右缘让出「消息行的 px-4(16) + 头像 w-9(36)
       + 行 gap-2(8)」= **60px**，胶囊的那条边才正好落在气泡那条边上。
       ⚠️ 为什么是一个 60 而不是"px-4 再加头像"：Tailwind 里 `pl-*` 与 `px-*` 同属性、按顺序覆盖
       （实测证据：旧写法 `pr-1` + `pr-12` 同时挂在这一颗元素上，生效的只有 48px 那一档），
       拆成两档写就会静默少掉一层 —— 旧值 48 正是这么来的：注释按"头像 40 + 间距 8"算，
       而头像其实是 36，还整个漏掉了行的 16px 内边距，两边各差 12px。
       ⚠️ 这个 60 由 `scripts/check-ui-runtime.mjs` 的 roster 段在**真浏览器里**量着（|差| <= 1px），
       所以头像尺寸或行内边距哪天变了，红的是那条判据，不是这段注释。 -->
  <div
    v-if="chips.length > 0 || interactive"
    data-reaction-bar
    :data-open-emoji="String(openEmoji)"
    class="flex flex-wrap items-center gap-1"
    :class="mine ? 'justify-end pr-[60px]' : 'pl-[60px]'"
  >
    <span v-for="c in chips" :key="c.emoji" class="relative inline-flex">
      <button
        data-reaction-chip
        :data-reaction-emoji="c.emoji"
        :data-hover-capable="String(canHover)"
        class="tap-safe flex h-7 items-center gap-1 rounded-full border px-1.5 text-[12px] leading-none transition"
        :class="c.mine
          ? 'border-[var(--gosslan-primary)] bg-[var(--gosslan-primary-light)] text-[var(--gosslan-primary)]'
          : 'border-transparent bg-[var(--gosslan-hover)] text-[var(--gosslan-text-2)] hover:bg-[var(--gosslan-border)] hover:text-[var(--gosslan-text)]'"
        :title="chipTitle(c)"
        :aria-label="t('msg.reactionToggle', { emoji: c.emoji })"
        :aria-expanded="openEmoji === c.emoji"
        :disabled="!interactive"
        @click="onClick(c, $event.currentTarget)"
        @pointerdown="onPressStart(c.emoji, $event.currentTarget)"
        @pointerup="clearPress"
        @pointercancel="clearPress"
        @pointerleave="clearPress"
        @mouseenter="canHover && reveal(c.emoji, $event.currentTarget)"
        @mouseleave="canHover && hide(c.emoji)"
        @focus="canHover && reveal(c.emoji, $event.currentTarget)"
        @blur="canHover && hide(c.emoji)"
        @keydown.esc="hide(c.emoji)"
      >
        <img
          v-if="emojiUrl(c.emoji)"
          :src="emojiUrl(c.emoji) ?? undefined"
          alt=""
          class="h-[18px] w-[18px] shrink-0"
        />
        <span v-else class="text-[14px] leading-none">{{ c.emoji }}</span>
        <span class="tabular-nums font-medium">{{ c.count }}</span>
      </button>
    </span>
  </div>

  <!-- 名单浮层：Teleport 到 body + fixed 坐标（摆位见 utils/hoverCard）。
       宽度只给 max-width ⇒ 按名字长短撑开；行内 truncate 负责超过那条上限之后的省略号。
       顺序就是**追加先后**（先点的在上、后点的在下），不再单独标"最新"
       （用户 2026-10-07：「最新的那个标签去掉」）。 -->
  <Teleport to="body">
    <div
      v-if="activeChip && activeChip.actors.length > 0 && pos"
      :ref="setEl"
      data-reaction-roster
      class="fixed z-[70] rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-panel)] px-2.5 py-1.5 text-[11px] leading-relaxed text-[var(--gosslan-text-2)] shadow-md"
      :style="style"
      role="tooltip"
      @click.stop
    >
      <span
        v-for="id in rosterIds(activeChip)"
        :key="id"
        data-roster-row
        class="block truncate"
        :title="nameOf(id)"
      >
        {{ nameOf(id) }}
      </span>
      <span v-if="visible(activeChip).hidden > 0" class="block">{{
        t("msg.reactionPlus", { n: visible(activeChip).hidden })
      }}</span>
    </div>
  </Teleport>
</template>
