/**
 * 表情「最常使用」的本机计数（用户 2026-09-29：「表情选择加一个最常使用，算法你来定」）。
 *
 * ## 算法
 * 每选中一次 ⇒ 该表情 `次数 +1` 并记下时刻。面板取前 `TOP_VISIBLE`（=8，正好一行）个，
 * 排序 `次数 desc → 最近使用 desc → token 字典序`（第三级是为了同样常用时顺序不随机飘）。
 * **不做时间衰减**：表情总量才一百多个，常用的自然会排到前面；加衰减只会让顺序在两次
 * 打开之间自己变，那种"没动它却变了"的表现比不加衰减更难解释。
 *
 * 记的是**选中**（从面板里点下去那一下），不是"消息发出去了"：两处入口（输入框插入、
 * 表情回应）都走同一个 `EmojiPicker`，所以在这一层记一次就够，不用两个调用点各写一遍。
 *
 * ## 为什么只记本机、不进协议
 * "我常用哪些"是设备偏好，与消息内容无关；进协议等于为一个人偏好扩一次线上字段。
 *
 * ## 失败方向 = 没有常用行，且绝不抛
 * 读失败/脏数据一律退回空表 ⇒ 面板就是原来的全量网格；写失败静默丢掉 ⇒ 顺序不更新而已。
 * 这条链上任何一步都不许抛 —— 面板挂在聊天输入框上，抛错的表现是整块面板白屏。
 */

/** 存储键：换算法时改后缀，旧数据直接不读（不写迁移代码，最坏是重新攒）。 */
export const EMOJI_USAGE_KEY = "gosslan.emojiUsage.v1";

/** 面板最前面露几个（= EmojiPicker 的 COLS，一行的格子数）。 */
export const TOP_VISIBLE = 8;

/** 本机最多记这么多种表情：用不到的长尾剪掉，localStorage 里不会无限长。 */
export const MAX_TRACKED = 64;

export type EmojiUse = { n: number; t: number };
export type EmojiUsage = Record<string, EmojiUse>;

type KVStore = { getItem(key: string): string | null; setItem(key: string, value: string): void };

/** 读整份计数：脏数据 / 非对象 / 存储不可用 ⇒ 空表（见文件头那条失败方向）。 */
export function readEmojiUsage(store: KVStore | null): EmojiUsage {
  if (!store) return {};
  let raw: string | null = null;
  try {
    raw = store.getItem(EMOJI_USAGE_KEY);
  } catch {
    return {};
  }
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    const out: EmojiUsage = {};
    for (const [token, v] of Object.entries(parsed as Record<string, unknown>)) {
      // 只收「正整数次数 + 有限时刻」这两样都对得上的条目：
      // 脏条目顶着用会让 top 排序拿到 NaN，比较器返回 NaN ⇒ 整个顺序不可预期。
      if (typeof v !== "object" || v === null) continue;
      const { n, t } = v as Partial<EmojiUse>;
      if (typeof n !== "number" || !Number.isFinite(n) || n <= 0) continue;
      if (typeof t !== "number" || !Number.isFinite(t)) continue;
      out[token] = { n, t };
    }
    return out;
  } catch {
    return {};
  }
}

export function writeEmojiUsage(store: KVStore | null, usage: EmojiUsage): void {
  if (!store) return;
  try {
    store.setItem(EMOJI_USAGE_KEY, JSON.stringify(usage));
  } catch {
    /* 存储被禁用/写满：常用行只是省几次滚动，没了它面板照样能用 */
  }
}

/** 记一次选中，返回新表（不改入参 —— 调用点是 Vue 的 ref，就地改会让旧值跟着变）。 */
export function withEmojiUse(usage: EmojiUsage, token: string, nowMs: number): EmojiUsage {
  if (!token) return usage;
  const next: EmojiUsage = { ...usage };
  const prev = next[token];
  next[token] = { n: (prev?.n ?? 0) + 1, t: nowMs };
  const keys = Object.keys(next);
  if (keys.length <= MAX_TRACKED) return next;
  // 剪枝用的排序与面板那一次是同一个比较器 ⇒ "被剪掉的"和"排在上方的"不会打架
  const keep = new Set(sortTokens(next).slice(0, MAX_TRACKED));
  for (const k of keys) if (!keep.has(k)) delete next[k];
  return next;
}

/** token 列表按「次数 → 最近使用 → 字典序」排。 */
function sortTokens(usage: EmojiUsage): string[] {
  return Object.keys(usage).sort((a, b) => {
    const x = usage[a];
    const y = usage[b];
    if (x.n !== y.n) return y.n - x.n;
    if (x.t !== y.t) return y.t - x.t;
    return a < b ? -1 : a > b ? 1 : 0;
  });
}

/** 取最常用的一小撮（去重、不超上限）。 */
export function topEmojiTokens(usage: EmojiUsage, limit = TOP_VISIBLE): string[] {
  return sortTokens(usage).slice(0, Math.max(0, limit));
}
