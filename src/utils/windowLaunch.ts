/**
 * 「打开独立窗口」的**单飞 + 防抖**判据（纯函数，便于单测）。
 *
 * 为什么需要（用户 2026-09-12 实测）：
 *   - 「连续点几下设置」不应该开出第二个窗口，也不应该把一串 IPC 打进去 ——
 *     每次 `invoke` 都要跨进程进主线程消息循环，而窗口创建本身是重活；
 *   - 按钮要有防抖：即便窗口这次没打开（例如创建失败），连点也不该变成"雪崩"。
 *
 * 判据只有两条：
 *   ① 该窗口**正在打开**（上一次的 invoke 还没回来）⇒ 合并到那一次，直接不发起；
 *   ② 距上次发起不足 `minIntervalMs` ⇒ 视为连点，忽略。
 *
 * 窗口实例的唯一性由后端保证（`ensure_aux_window` 的单例 + 串行创建），
 * 这里只负责"别把请求打爆"和按钮的 pending 反馈。
 */
import { shouldRunThrottled } from "./defer.ts";

/** 同一个窗口两次发起之间的最小间隔：够短（不影响"关掉再开"），又足以吃掉连点。 */
export const WINDOW_LAUNCH_MIN_INTERVAL_MS = 300;

export function shouldLaunchWindow(
  now: number,
  isOpening: boolean,
  lastLaunchAt: number,
  minIntervalMs: number = WINDOW_LAUNCH_MIN_INTERVAL_MS,
): boolean {
  if (isOpening) return false;
  return shouldRunThrottled(now, lastLaunchAt, minIntervalMs);
}
