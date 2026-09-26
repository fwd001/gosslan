/**
 * 运行日志的过滤逻辑（纯函数，便于单测）。
 *
 * 抽出来的理由：这层是"精准匹配"这个**用户可见承诺**的落点。若哪天有人把字面包含
 * 改成模糊/分词匹配，过滤器会开始返回用户没预期的行 —— 但界面看不出来，只有测试能拦住。
 */

/** 日志级别 → 展示文本（对应后端 `Level{Info,Warn,Error}`）。 */
export const LOG_LEVEL_TEXT: Record<string, string> = {
  info: "INFO",
  warn: "WARN",
  error: "ERROR",
};

export interface LogLineInput {
  /** **已格式化**的时间（HH:MM:SS）。刻意由调用方传入：时区格式化不可单测，过滤逻辑要能。 */
  time: string;
  level: string;
  target: string;
  message: string;
}

/**
 * 一行日志在界面上**真实渲染出来**的文本 —— 过滤的唯一判据。
 *
 * 「所见即所匹配」：判据必须与屏幕上的文本一致，否则会出现「这一行被保留了，
 * 但整行没有任何高亮」的困惑（例如把未显示的日期部分也纳入匹配）。
 * 顺序与模板中的渲染顺序一致：时间 · 级别 · target · 消息。
 */
export function logLineText(line: LogLineInput): string {
  return `${line.time} ${LOG_LEVEL_TEXT[line.level] ?? line.level} ${line.target} ${line.message}`;
}

/**
 * 是否命中过滤词。
 *
 * 语义刻意收紧为**字面包含**：
 * - 两端**不**做模糊/分词/首字母匹配 —— 用户输入什么就找什么（"精准匹配"）；
 * - 大小写不敏感（能同时搜到小写 target 与大写 `ERROR`，是过滤框的通行预期）；
 * - 空串（含纯空白）视为不过滤。
 */
export function matchesLogQuery(line: LogLineInput, query: string): boolean {
  const q = query.trim().toLowerCase();
  return q === "" || logLineText(line).toLowerCase().includes(q);
}

/** 过滤日志行，保持输入顺序（调用方已按展示顺序排好）。 */
export function filterLogLines<T extends LogLineInput>(lines: T[], query: string): T[] {
  return lines.filter((l) => matchesLogQuery(l, query));
}

// ---------------- 合并重复（用户看到的「×N」） ----------------

/** 可参与合并的行：`time` 是已格式化的 `HH:MM:SS`，`target`/`message` 允许带高亮标记。 */
export interface MergeableLogRow {
  level: string;
  time: string;
  target: string;
  message: string;
}

/** 剥掉 v-html 的高亮标记，回到屏幕上真正可读的那串字。 */
const stripTags = (s: string): string => s.replace(/<[^>]+>/g, "");

/**
 * 合并用的比较键：先把"每次都变的那部分"折成占位符。
 *
 * 为什么非做不可：日志里最啰嗦的那几类行（BLE 退避的剩余毫秒、分片序号、`msg_id` /
 * `seq` / 字节数）**几乎每行都带一个变化量**，逐字比较等于永不合并 —— 用户看到的
 * 「×N」就是这么消失的（2026-09-26 实测，逻辑本身从没被删过）。
 * 规则刻意只做两件事，宁可少折也不许把不同事件折成同一类：
 *  - 16 位以上的十六进制串 → `<id>`（设备 id / msg_id / 密钥指纹）
 *  - 连续数字 → `<n>`（毫秒、字节、分片号、seq、端口）
 */
export function logMergeKey(row: MergeableLogRow): string {
  const message = stripTags(row.message)
    .replace(/[0-9a-fA-F]{16,}/g, "<id>")
    .replace(/\d+/g, "<n>");
  return `${row.level}|${stripTags(row.target)}|${message}`;
}

/** `HH:MM:SS` → 当天秒数。解析不了返回 null：宁可少合并，也不在渲染路径上抛错。 */
function secondsOfDay(time: string): number | null {
  const m = /^(\d{1,2}):(\d{2}):(\d{2})/.exec(stripTags(time));
  if (!m) return null;
  const h = Number(m[1]);
  const mi = Number(m[2]);
  const s = Number(m[3]);
  if (h > 23 || mi > 59 || s > 59) return null;
  return h * 3600 + mi * 60 + s;
}

/** 环形秒差：跨午夜时 `23:59:59` 与 `00:00:01` 差 2 秒，不是 86398 秒。 */
function circularSecondGap(a: number, b: number): number {
  const d = Math.abs(a - b);
  return Math.min(d, 86400 - d);
}

/**
 * 把同类行收成「第一条 + count」。与旧实现（组件里的相邻逐字比较）有两处必要差别：
 *  1. 比归一化后的键，不是逐字原文；
 *  2. 允许**非相邻**，但限制在 `windowSec` 秒内 —— 完全不限窗口会把几小时前那次同类
 *     失败并进这一波，等于谎报"只发生了一次"。
 *
 * 输出保持**首次出现**的位置与原文（也就是显示的是第一条自带的数字 + `×N`，
 * 这与各家日志聚合器一样是取舍，不是把每次的值都显示出来）。
 */
export function mergeLogRows<T extends MergeableLogRow>(
  rows: T[],
  windowSec = 5,
): (T & { count: number })[] {
  const out: (T & { count: number })[] = [];
  const open = new Map<string, { idx: number; t0: number }>();
  for (const r of rows) {
    const key = logMergeKey(r);
    const t = secondsOfDay(r.time);
    const prev = open.get(key);
    if (prev !== undefined && t !== null && circularSecondGap(t, prev.t0) <= windowSec) {
      out[prev.idx].count += 1;
      continue;
    }
    out.push({ ...r, count: 1 });
    if (t !== null) open.set(key, { idx: out.length - 1, t0: t });
  }
  return out;
}
