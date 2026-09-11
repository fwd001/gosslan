import { computed, ref, watch, type Ref } from "vue";
import { api } from "@/api";
import { useDeferredRef } from "@/composables/useDeferredRef";
import type { Conversation, SearchResult } from "@/types";

/** 名称过滤的延迟镜像时长（ms）：让输入框先走，列表后跟。 */
const DEFER_MS = 120;

/** 消息内容搜索的防抖时长（ms）：跨进程调用，窗口给大一些。 */
const DEBOUNCE_MS = 300;

/**
 * 会话列表搜索：名称匹配即时生效，消息内容匹配走后端 searchMessages（防抖 + 竞态保护）。
 * 同一时刻可能有多个在途请求，只接受最新一次的结果（seq 比对）。
 */
export function useConversationSearch(conversations: Ref<Conversation[]>) {
  /** 绑在输入框上的**原值**：每次按键都变（DOM 立即更新，不触发任何重算）。 */
  const keyword = ref("");
  /**
   * 延迟镜像：过滤、空态判断、列表项高亮都用它。
   *
   * 为什么要有这一层：下面的 `filtered` 一变，整个会话列表（含 `v-memo` 依赖里的
   * 关键词）就要重渲染；按住 Ctrl+V 连发粘贴时关键词每秒变十几次，重渲染把主线程
   * 占满 ⇒ 输入框自己的 caret 掉帧。原值即时、派生延迟，输入就始终跟手。
   */
  const query = useDeferredRef(keyword, DEFER_MS);
  const results = ref<SearchResult[]>([]);
  const isSearching = ref(false);

  let timer: ReturnType<typeof setTimeout> | null = null;
  let seq = 0;

  watch(keyword, (kw) => {
    if (timer) clearTimeout(timer);
    const trimmed = kw.trim();
    if (!trimmed) {
      results.value = [];
      isSearching.value = false;
      return;
    }
    isSearching.value = true;
    const mine = ++seq;
    timer = setTimeout(async () => {
      try {
        const r = await api.searchMessages(trimmed);
        if (mine === seq) results.value = r;
      } catch {
        if (mine === seq) results.value = [];
      }
      if (mine === seq) isSearching.value = false;
    }, DEBOUNCE_MS);
  });

  /** 名称匹配优先，其次补上内容命中的会话（去重）。 */
  const filtered = computed<Conversation[]>(() => {
    const kw = query.value.trim().toLowerCase();
    if (!kw) return conversations.value;
    const matchedIds = new Set(results.value.map((r) => r.conv_id));
    const out = conversations.value.filter((c) => c.name.toLowerCase().includes(kw));
    for (const c of conversations.value) {
      if (matchedIds.has(c.id) && !out.some((r) => r.id === c.id)) out.push(c);
    }
    return out;
  });

  /** 会话的搜索命中摘要：截取关键词前后的内容，过长加省略号。 */
  function snippet(convId: string): string | null {
    const r = results.value.find((x) => x.conv_id === convId);
    if (!r) return null;
    const kw = query.value.trim().toLowerCase();
    const content = r.match_content;
    const idx = content.toLowerCase().indexOf(kw);
    if (idx < 0) return content.slice(0, 60);
    const start = Math.max(0, idx - 20);
    const end = Math.min(content.length, idx + kw.length + 40);
    let s = content.slice(start, end);
    if (start > 0) s = "…" + s;
    if (end < content.length) s = s + "…";
    return s;
  }

  /** 命中消息的 msg_id —— 供"点进去直接跳到那一条"用；无命中返回 null。 */
  function hitMsgId(convId: string): string | null {
    return results.value.find((r) => r.conv_id === convId)?.match_msg_id ?? null;
  }

  return { keyword, query, results, isSearching, filtered, snippet, hitMsgId };
}
