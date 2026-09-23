/**
 * 「注册 ↔ 卸载」配对容器：一轮初始化登记若干副作用，结束时按逆序全部拆掉。
 *
 * ## 为什么需要
 * 本仓有过一类隐性缺陷：`init()` 注册监听时没有对应的卸载路径（`void onAction(...)`
 * 把返回的 listener 直接丢了、匿名 `addEventListener` 摘不掉、`setInterval` 没有句柄）。
 * 单次冷启动看不出问题，**第二次 init**（HMR 热替换、将来的重连重建）就会把上一轮的
 * 回调继续挂在事件总线上 —— 于是每个事件跑两遍，而第一遍的闭包绑的是一份已经被丢弃的
 * state（消息重复入账、通知翻倍、给已切走的会话补发已读回执）。
 *
 * ## 为什么 `onDispose` 要处理"迟到的注册"
 * `listen()`/`onAction()` 拿到卸载函数要等一次 IPC 往返。若这段时间里本轮已被拆掉，
 * 把卸载函数攒进一个已经 dispose 的列表等于**永远没人调用它** —— 监听就真的留下了。
 * 所以拆过之后的注册**就地执行**。
 */
export interface InitScope {
  /** 登记一个卸载器；本轮已 `dispose()` 时立刻执行。 */
  onDispose(fn: () => void): void;
  /** 拆除全部已登记的卸载器（逆序，后注册的先拆）。可重复调用，只有第一次有效果。 */
  dispose(): void;
}

export function createInitScope(): InitScope {
  const disposers: Array<() => void> = [];
  let torn = false;
  return {
    onDispose(fn) {
      if (torn) fn();
      else disposers.push(fn);
    },
    dispose() {
      torn = true;
      while (disposers.length) {
        const d = disposers.pop();
        try {
          d?.();
        } catch {
          /* 卸载器互不牵连：IPC 抖动摘掉一个，不能因此跳过剩下的 */
        }
      }
    },
  };
}
