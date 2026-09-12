<script setup lang="ts">
/**
 * 「搜索聊天记录」结果页（用户需求 2026-09-12 晚）：
 * 「搜索并点击回车后，弹出一个框。样式参考微信，但配色/圆角按本项目的约束标准。」
 *
 * 布局对齐微信 macOS 的「搜索聊天记录」窗口：
 *   · 顶部：关键词输入框（带清除）+「发送人 / 日期」两个筛选入口；
 *   · 左栏：命中的会话（头像 / 名称 / 最新命中时间 / 命中片段 / 共 N 条相关聊天记录）；
 *   · 右栏：选中会话的命中消息 + 顶部「共 N 条与"关键词"的聊天记录」+「进入聊天 >」。
 *
 * 与「会话列表搜索框」的分工：列表里那点输入是**即时过滤**（联系人 + 内容命中的会话摘要，
 * 每会话一条），回车才进这里 —— 这里是**逐条命中**的结果页，可以一直往下看、可以筛选、
 * 可以跳到那一条。微信也是这个分工（输入即提示，回车进结果页）。
 *
 * 配色/圆角/间距一律用项目 token（`--gosslan-*`）与 `.gosslan-menu*`/`SettingsGroup` 同族样式，
 * 不引入微信的具体色值 —— 这是用户明确要求「按本项目约束标准」的部分。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { ChevronRight, Search, X } from "lucide-vue-next";
import { api } from "@/api";
import { useAppStore } from "@/stores/useAppStore";
import { useImeEnterGuard } from "@/composables/useImeEnterGuard";
import BaseModal from "@/components/BaseModal.vue";
import MessageAvatar from "@/components/message/MessageAvatar.vue";
import { highlightText } from "@/utils/highlight";
import { formatTimeDivider } from "@/utils/chatStyle";
import {
  SEARCH_DATE_PRESETS,
  dateRangeFor,
  hitSnippet,
  senderOptionsFrom,
  totalHits,
  type SearchDatePreset,
  type SenderOption,
} from "@/utils/chatSearch";
import { t } from "@/i18n";
import type { ChatSearchGroup } from "@/types";

const props = defineProps<{ open: boolean; initialKeyword?: string }>();
const emit = defineEmits<{
  (e: "close"): void;
  /** 跳到命中的那一条（父组件负责开会话 + 定位） */
  (e: "open-conversation", payload: { convId: string; msgId: string }): void;
}>();

const app = useAppStore();
/** 输入法守卫：拼音候选里按回车是选字，不该被当成"重新搜索"（与发送键同一套判定） */
const ime = useImeEnterGuard();

const keyword = ref("");
const groups = ref<ChatSearchGroup[]>([]);
const searching = ref(false);
const activeConvId = ref<string | null>(null);

/** 筛选条件。筛选项与日期都走后端（命中总数必须按当前筛选算，不能只筛已加载的那几条）。 */
const senderFilter = ref<string | null>(null);
const datePreset = ref<SearchDatePreset>("all");
const senderMenuOpen = ref(false);
const dateMenuOpen = ref(false);

/**
 * 发送人候选：**始终取自"不带发送人筛选"的那次结果**。
 * 否则一旦选了某个人，候选里就只剩他自己，用户无法切换（这是设计，不是漏考虑）。
 */
const senderOptions = ref<SenderOption[]>([]);

/** 「发送人」按钮上的文字：未筛选显示"发送人"，已筛选显示那个人的名字。 */
const senderName = computed(
  () => senderOptions.value.find((o) => o.id === senderFilter.value)?.name ?? null,
);

const activeGroup = computed(
  () => groups.value.find((g) => g.conv_id === activeConvId.value) ?? groups.value[0] ?? null,
);
const total = computed(() => totalHits(groups.value));
const inputRef = ref<HTMLInputElement | null>(null);

let seq = 0;
/** 是否把本次结果记为「发送人候选项的来源」（只在无发送人筛选时记）。 */
async function runSearch(keepSenderOptions = false) {
  const kw = keyword.value.trim();
  if (!kw) {
    groups.value = [];
    searchActive();
    return;
  }
  const mine = ++seq;
  searching.value = true;
  try {
    const range = dateRangeFor(datePreset.value);
    const result = await api.searchChatHistory({
      keyword: kw,
      senderId: senderFilter.value,
      sinceMs: range.sinceMs,
      untilMs: range.untilMs,
    });
    if (mine !== seq) return; // 只接受最后一次请求的结果（连打时会有多个在途）
    groups.value = result;
    if (keepSenderOptions) senderOptions.value = senderOptionsFrom(result);
    // 选中态：保持当前选中；若它已不在结果里，落到第一条
    if (!result.some((g) => g.conv_id === activeConvId.value)) {
      activeConvId.value = result[0]?.conv_id ?? null;
    }
  } catch (e) {
    if (mine === seq) {
      groups.value = [];
      app.toastError(e, t("search.fail"));
    }
  } finally {
    if (mine === seq) searching.value = false;
  }
}

/** 回车：立即按当前关键词重搜（输入时有 150ms 防抖，回车是"我确定，现在就查"）。 */
function onEnter(e: KeyboardEvent) {
  if (ime.isIme(e)) return;
  void runSearch(senderFilter.value === null);
}

function searchActive() {
  // 关键词清空 ⇒ 结果清空，但筛选条件保留（用户清掉关键词只是想换个词）
  activeConvId.value = null;
}

/** 关键词防抖：输入即搜（微信也是输入就出结果），150ms 足够合并连打又不觉得延迟。 */
let debounce: ReturnType<typeof setTimeout> | null = null;
watch(keyword, () => {
  // 关闭时也会改 keyword（重置状态），那时不该再发请求
  if (!props.open) return;
  if (debounce) clearTimeout(debounce);
  debounce = setTimeout(() => void runSearch(senderFilter.value === null), 150);
});

watch([senderFilter, datePreset], () => {
  if (!props.open) return;
  void runSearch(senderFilter.value === null);
});

onBeforeUnmount(() => {
  if (debounce) clearTimeout(debounce);
});

/** 打开时：带入关键词并立即检索一次（回车进来应马上看到结果）。 */
watch(
  () => props.open,
  async (open) => {
    if (!open) {
      // 关闭即清空：下次打开是干净的一次（也顺带避免残留关键词/筛选影响判断）
      keyword.value = "";
      senderFilter.value = null;
      datePreset.value = "all";
      groups.value = [];
      senderOptions.value = [];
      activeConvId.value = null;
      senderMenuOpen.value = false;
      dateMenuOpen.value = false;
      return;
    }
    keyword.value = (props.initialKeyword ?? "").trim();
    senderFilter.value = null;
    datePreset.value = "all";
    await nextTick();
    inputRef.value?.focus();
    // 打开时只检索这一次：关键词/筛选的 watcher 已用 `props.open` 守卫，
    // 重置这两个值不会再多发一个请求（否则每次打开都白跑一次全表扫描）。
    await runSearch(true);
  },
);

function chooseSender(id: string | null) {
  senderMenuOpen.value = false;
  senderFilter.value = id;
}

function choosePreset(p: SearchDatePreset) {
  dateMenuOpen.value = false;
  datePreset.value = p;
}

function enterChat() {
  const g = activeGroup.value;
  if (!g) return;
  const msgId = g.messages[0]?.msg_id ?? "";
  emit("open-conversation", { convId: g.conv_id, msgId });
  emit("close");
}

/** 单聊不重复显示发送者名字（微信同款）；群聊才显示"谁说的"。 */
function showSender(group: ChatSearchGroup): boolean {
  return group.kind === "group";
}
</script>

<template>
  <!-- 移动端用 `fullscreen`（整页、带安全区与返回键），桌面用卡片弹窗 —— **body 只写一份**，
       外壳差异交给 BaseModal（见其 `fullscreen` 注释）。 -->
  <BaseModal
    :open="open"
    :title="t('search.title')"
    :fullscreen="app.isMobile"
    width="max-w-3xl"
    @close="emit('close')"
  >
    <!-- 搜索框 + 筛选 -->
    <div class="flex shrink-0 items-center gap-2">
      <div
        class="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-[var(--gosslan-radius-md)] border border-[var(--gosslan-border)] bg-[var(--gosslan-field)] px-3 transition focus-within:border-[var(--gosslan-primary)]"
      >
        <Search class="h-4 w-4 shrink-0 text-[var(--gosslan-text-2)]" />
        <input
          ref="inputRef"
          v-model="keyword"
          maxlength="100"
          enterkeyhint="search"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck="false"
          :placeholder="t('search.placeholder')"
          :aria-label="t('search.placeholder')"
          class="w-full bg-transparent text-[13px] placeholder:text-[var(--gosslan-text-2)]"
          @keydown.enter.prevent="onEnter"
          @compositionstart="ime.onStart"
          @compositionend="ime.onEnd"
        />
        <button
          v-if="keyword"
          class="tap-safe flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--gosslan-border)] text-[var(--gosslan-panel)]"
          :title="t('search.clear')" :aria-label="t('search.clear')"
          @click="keyword = ''"
        >
          <X class="h-3 w-3" />
        </button>
      </div>

      <!-- 发送人 / 日期：与微信同位置、同语义（点开一个小菜单选，不做日历控件） -->
      <div class="relative shrink-0">
        <button
          class="gosslan-filter-chip"
          :class="senderFilter ? 'gosslan-filter-chip--on' : ''"
          :aria-expanded="senderMenuOpen"
          aria-haspopup="menu"
          @click="senderMenuOpen = !senderMenuOpen; dateMenuOpen = false"
        >
          {{ senderName ?? t("search.sender") }}
        </button>
        <div v-if="senderMenuOpen" class="gosslan-menu absolute right-0 top-9 z-20 max-h-64 overflow-y-auto">
          <button role="menuitem" class="gosslan-menu-item" @click="chooseSender(null)">
            {{ t("search.sender.all") }}
          </button>
          <button
            v-for="o in senderOptions"
            :key="o.id"
            role="menuitem"
            class="gosslan-menu-item"
            @click="chooseSender(o.id)"
          >
            <span class="min-w-0 flex-1 truncate" :title="o.name">{{ o.name }}</span>
            <span class="text-[var(--gosslan-text-2)]">{{ o.count }}</span>
          </button>
          <div v-if="!senderOptions.length" class="gosslan-menu-hint">{{ t("search.sender.empty") }}</div>
        </div>
      </div>

      <div class="relative shrink-0">
        <button
          class="gosslan-filter-chip"
          :class="datePreset !== 'all' ? 'gosslan-filter-chip--on' : ''"
          :aria-expanded="dateMenuOpen"
          aria-haspopup="menu"
          @click="dateMenuOpen = !dateMenuOpen; senderMenuOpen = false"
        >
          {{ datePreset === "all" ? t("search.date") : t(`search.date.${datePreset}`) }}
        </button>
        <div v-if="dateMenuOpen" class="gosslan-menu absolute right-0 top-9 z-20">
          <button
            v-for="p in SEARCH_DATE_PRESETS"
            :key="p"
            role="menuitem"
            class="gosslan-menu-item"
            @click="choosePreset(p)"
          >
            {{ t(`search.date.${p}`) }}
          </button>
        </div>
      </div>
    </div>

    <!-- 结果：左会话 / 右命中消息（微信同款两栏） -->
    <div class="mt-3 flex min-h-0 flex-1 flex-col overflow-hidden rounded-[var(--gosslan-radius-lg)] border border-[var(--gosslan-divider)] md:h-[52vh] md:flex-none md:flex-row">
      <!-- 左栏 -->
      <div class="flex max-h-[38%] w-full shrink-0 flex-col overflow-y-auto border-b border-[var(--gosslan-divider)] bg-[var(--gosslan-list)] md:max-h-none md:w-[240px] md:border-b-0 md:border-r">
        <p v-if="!keyword.trim()" class="px-3 py-6 text-center text-xs text-[var(--gosslan-text-2)]">
          {{ t("search.hint") }}
        </p>
        <!-- 搜索中：用户反馈「感觉显示得比较慢、当前状态没有提示」——
             以前只有"有没有结果"两种静态文案，请求在途时界面看起来像卡住了。 -->
        <p v-else-if="searching" class="flex items-center justify-center gap-2 px-3 py-6 text-center text-xs text-[var(--gosslan-text-2)]">
          <span class="h-3 w-3 animate-spin rounded-full border-2 border-[var(--gosslan-border)] border-t-[var(--gosslan-primary)]"></span>
          {{ t("search.searching") }}
        </p>
        <p v-else-if="!groups.length" class="px-3 py-6 text-center text-xs text-[var(--gosslan-text-2)]">
          {{ t("search.empty") }}
        </p>
        <button
          v-for="g in groups"
          :key="g.conv_id"
          class="flex items-start gap-2.5 px-3 py-2.5 text-left transition"
          :class="activeGroup?.conv_id === g.conv_id
            ? 'bg-[var(--gosslan-list-active)]'
            : 'hover:bg-[var(--gosslan-list-hover)]'"
          @click="activeConvId = g.conv_id"
        >
          <MessageAvatar :name="g.name" :avatar="g.avatar" />
          <span class="min-w-0 flex-1">
            <span class="flex items-baseline gap-2">
              <span class="min-w-0 flex-1 truncate text-[13px] font-medium text-[var(--gosslan-text)]" :title="g.name">{{ g.name }}</span>
              <span class="shrink-0 text-[11px] text-[var(--gosslan-text-2)]">{{ formatTimeDivider(g.latest_ts) }}</span>
            </span>
            <span
              class="mt-0.5 block truncate text-xs text-[var(--gosslan-text-2)]"
              :title="g.messages[0]?.content ?? ''"
              v-html="highlightText(hitSnippet(g.messages[0]?.content ?? '', keyword), keyword)"
            ></span>
            <span class="mt-0.5 block text-[11px] text-[var(--gosslan-text-2)]">
              {{ t("search.groupCount", { n: g.total }) }}
            </span>
          </span>
        </button>
      </div>

      <!-- 右栏 -->
      <div class="flex min-w-0 flex-1 flex-col bg-[var(--gosslan-chat)]">
        <div class="flex h-10 shrink-0 items-center gap-2 border-b border-[var(--gosslan-divider)] px-4">
          <span
            class="min-w-0 flex-1 truncate text-[13px] text-[var(--gosslan-text)]"
            :title="keyword.trim() ? t('search.header', { n: total, kw: keyword.trim() }) : t('search.hint')"
          >
            {{ keyword.trim() ? t("search.header", { n: total, kw: keyword.trim() }) : t("search.hint") }}
          </span>
          <button
            v-if="activeGroup"
            class="tap-safe flex shrink-0 items-center gap-1 text-[13px] text-[var(--gosslan-link-ink,var(--gosslan-primary))] transition hover:opacity-80"
            @click="enterChat"
          >
            {{ t("search.enterChat") }}
            <ChevronRight class="h-3.5 w-3.5" />
          </button>
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          <div
            v-for="m in activeGroup?.messages ?? []"
            :key="m.msg_id"
            class="flex gap-2.5 py-2.5"
          >
            <MessageAvatar :name="m.sender_name" />
            <div class="min-w-0 flex-1">
              <div class="flex items-baseline gap-2">
                <span
                  v-if="activeGroup && showSender(activeGroup)"
                  class="min-w-0 truncate text-xs text-[var(--gosslan-text-2)]"
                  :title="m.sender_name"
                >
                  {{ m.sender_name }}
                </span>
                <span class="ml-auto shrink-0 text-[11px] text-[var(--gosslan-text-2)]">{{ formatTimeDivider(m.ts) }}</span>
              </div>
              <div
                class="mt-0.5 gosslan-selectable whitespace-pre-wrap break-words text-[13px] leading-relaxed text-[var(--gosslan-text)]"
                v-html="highlightText(m.content, keyword)"
              ></div>
            </div>
          </div>
          <p
            v-if="activeGroup && activeGroup.total > activeGroup.messages.length"
            class="py-3 text-center text-[11px] text-[var(--gosslan-text-2)]"
          >
            {{ t("search.more", { n: activeGroup.total - activeGroup.messages.length }) }}
          </p>
        </div>
      </div>
    </div>
  </BaseModal>
</template>
