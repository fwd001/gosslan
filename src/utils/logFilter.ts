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
