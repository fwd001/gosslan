/**
 * 输入法（IME）组合态的按键判定 —— 纯函数，便于单测钉死。
 *
 * ## 为什么需要它（真实缺陷）
 * 用户 2026-09-12 反馈：「输入拼音，按回车应该是把拼音字母落下来，而不是选择文字或者发送。
 * 但是现在的 bug 是：我输入英文或拼音之后按回车，英文没落下来，中文却发出去了。」
 *
 * 原因：**不能只信 `KeyboardEvent.isComposing`**。在 macOS 的 WKWebView 上，用 Enter
 * 提交候选时事件顺序是 `compositionend` → `keydown`，于是 keydown 那一刻
 * `isComposing` 已经是 `false` —— 只看它就会把"选字"当成"发送"（半成品中文直接发出去）。
 * 各平台/输入法行为不一致，所以这里用**三条互补判据**：
 *
 * 1. `composing`：我们自己跟踪的 `compositionstart`/`compositionend` 状态（最可靠）；
 * 2. `e.isComposing || e.keyCode === 229`：标准字段 + 旧式 IME 键码（Chromium 传统）；
 * 3. **提交后短窗口内的 Enter**：兜住上面那个"compositionend 先于 keydown"的怪癖。
 *    该窗口**只对 Enter 生效**（提交键就是 Enter / 空格，空格不需要特判，
 *    因为它不会触发发送），且窗口很短（默认 80ms）。
 *
 * 取舍：窗口内宁可不发送（用户再按一下即可），也不能把半成品发出去 ——
 * 误发的代价远大于多点一次回车。
 */

/** 只取判定所需的字段，避免与具体事件类型耦合（也便于测试构造）。 */
export interface ImeKeyLike {
  key: string;
  isComposing?: boolean;
  keyCode?: number;
}

/** 「compositionend 之后仍算提交键」的默认窗口（毫秒）。 */
export const IME_COMMIT_WINDOW_MS = 80;

/**
 * 这次按键是否属于输入法组合（必须放行给 IME / 内容，**绝不能**触发发送）。
 *
 * @param e 按键事件（只需 key / isComposing / keyCode）
 * @param composing 组件跟踪的组合态（`compositionstart` 后为 true）
 * @param lastCompositionEndAt 最近一次 `compositionend` 的时间戳（ms；从未发生传 0）
 * @param now 当前时间戳（ms；显式传入便于测试）
 * @param windowMs 提交后窗口，默认 {@link IME_COMMIT_WINDOW_MS}
 */
export function isImeKey(
  e: ImeKeyLike,
  composing: boolean,
  lastCompositionEndAt: number,
  now: number,
  windowMs: number = IME_COMMIT_WINDOW_MS,
): boolean {
  // ① 自跟踪的组合态：只要没收到 compositionend，就还在组合中
  if (composing) return true;
  // ② 标准字段 + 旧式 IME 键码
  if (e.isComposing === true) return true;
  if (e.keyCode === 229) return true;
  // ③ 提交后的短窗口：只对 Enter 生效（它才是"确认候选"的键，也是我们的发送键）
  if (e.key !== "Enter") return false;
  if (lastCompositionEndAt <= 0) return false;
  // 闭区间（与项目既有约定一致，见 `ConnectionHealth::is_healthy` 的边界测试）：
  // 恰好等于窗口宽度仍算"提交键"，避免边界上的抖动导致偶发误发。
  return now - lastCompositionEndAt <= windowMs;
}
