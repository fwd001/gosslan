/**
 * 「输入框优先」的延迟工具：把**昂贵的派生工作**从每次按键上挪开。
 *
 * ## 为什么需要（真实缺陷）
 * 项目里的输入框大多和"重"的派生计算挤在同一个组件里：
 * 搜索框的每个字符都会重算过滤结果并重渲染整个列表（日志窗口一次要重建上千行
 * `v-html`）、拖颜色选择器时每个 `input` 事件都要落一次库。平时打字感觉不出来，
 * **但按住 Ctrl+V 连发粘贴时**（系统按键重复，每秒十几到几十次）每次都触发一遍，
 * 就会"一顿一顿"：输入框本身的字符是 DOM 原生行为（不受 Vue 影响），
 * 卡的是那些派生渲染 —— 它们把主线程占满，输入框的 caret/上屏就被挤掉了。
 *
 * ## 做法：输入值"两档"
 * 输入框 `v-model` 绑**原值**（DOM 立即更新，零响应式成本）；
 * 过滤/高亮/后端查询用 `useDeferredRef` 得到的**延迟镜像**。
 * 于是连发期间：原值每次都变，派生渲染每 `delay` 毫秒最多一次，输入框始终跟手。
 *
 * ## 为什么不用 `requestAnimationFrame` 合并
 * rAF 只保证"每帧一次"，但一次派生渲染（例如重建上千行日志）本身就超过一帧的预算，
 * 60fps 合并仍然会把帧吃满；对这类输入要用**时间窗**（trailing debounce）来限流，
 * 并且窗口结束（用户停手）时必定补一次，不会丢最终状态。
 */

/** 去抖后的函数：`cancel()` 丢弃待执行，`flush()` 立刻执行待执行。 */
export interface Debounced<A extends unknown[]> {
  (...args: A): void;
  /** 取消尚未触发的调用（组件卸载时用，避免悬挂的定时器）。 */
  cancel(): void;
  /** 立刻执行尚未触发的调用（窗口卸载/隐藏前落库用，避免丢掉最后一次写入）。 */
  flush(): void;
}

/**
 * 尾沿去抖：连续调用只在**最后一次**之后 `ms` 毫秒执行一次。
 *
 * 语义细节（都有单测钉住）：
 * - 参数取**最后一次**调用的（用户停手时的输入才是最终输入）；
 * - `flush()` 在无待执行调用时是空操作，不会凭空多执行一次；
 * - `cancel()` 之后 `flush()` 同样不执行；
 * - 执行后自动清空待执行状态（可以被下一轮复用）。
 */
export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number): Debounced<A> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending: A | null = null;

  const run = () => {
    timer = null;
    if (pending === null) return;
    const args = pending;
    pending = null;
    fn(...args);
  };

  const debounced = ((...args: A) => {
    pending = args;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(run, ms);
  }) as Debounced<A>;

  debounced.cancel = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    pending = null;
  };

  debounced.flush = () => {
    if (timer !== null) clearTimeout(timer);
    run();
  };

  return debounced;
}

/**
 * 节流判据（纯函数，便于单测）：距上次执行不足 `minIntervalMs` 就不必再执行。
 *
 * 用途：`peers-updated` 这类事件最多 3/s，而由它触发的拓扑刷新（一次 IPC 往返）变化很慢，
 * 每次都发就是白白的 IPC 风暴 —— 每次 IPC 都要跨进程、进主线程消息循环，攒起来就是"顿"。
 * `last === 0`（从未执行过）一律放行。
 */
export function shouldRunThrottled(now: number, last: number, minIntervalMs: number): boolean {
  if (last <= 0) return true;
  return now - last >= minIntervalMs;
}

/**
 * ⚠️ 这里**删掉**了 `shouldResyncFromBackend`（"收到设置事件要不要重拉"的守卫）。
 *
 * 它存在的唯一理由是：`settings-changed` 以前无载荷、且会**回发给发起窗口**，
 * 于是本窗口刚写进去的值会被自己事件触发的重拉用**旧快照**盖掉（症状："点了主题又跳回去"）。
 * 现在后端 `emit_filter` 按窗口标签过滤、不把事件回发给发起窗口，
 * 加上事件本身带了"变了哪些键 + 那些键的新值"，这个守卫连同它的状态（`settingsDirty`）
 * 一起没有了存在意义 —— 少一套需要长期维护的状态机。
 */
