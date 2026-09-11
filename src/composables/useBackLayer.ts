import { onBeforeUnmount, watch } from "vue";
import { createBackStack, type HistoryPort } from "@/utils/backStack";

/**
 * 分层返回（Android 系统返回键 / 桌面"后退"导航）——**浏览器侧接线**。
 *
 * 为什么需要：应用是单页 WebView，页面层级（移动端列表↔聊天、整页设置、全屏日志、
 * 各种弹窗）全是组件状态而非路由。Android 的系统返回键由 wry 交给 WebView 处理：
 * `canGoBack()` 为真就 `goBack()`，否则**结束 Activity（退出应用）**
 * （wry `android/kotlin/WryActivity.kt` 的 `handleBackNavigation`）。
 * 于是今天在聊天页/设置页按返回 = 应用直接退出，这是最像"网页壳"、也最容易被判定为
 * "不是原生应用"的一处。
 *
 * 做法：每打开一层 UI 就压一条**同 URL**的历史条目，返回键于是变成 `popstate`；
 * 我们据此只关最上面那层。层关完（条目也回退干净）后 `canGoBack()` 变回 false，
 * 返回键才真的退出应用 —— 与原生 Android 一致。
 *
 * 决策逻辑在 `utils/backStack.ts`（纯逻辑、有单测）；这里只负责：压/退历史条目的
 * 浏览器适配、`popstate` 监听、以及跟着组件生命周期压栈退栈。
 *
 * 刻意不做：不引入路由（会动到所有现有交互）；不接管 iOS 侧滑返回（wry 未开启
 * `allowsBackForwardNavigationGestures`，iOS 用页面内返回按钮）；瞬时浮层
 * （表情面板、右键菜单）不压条目，否则返回键要按好几下才有反应。
 */

/** 真实端口：`pushState` 在自定义协议下可能抛 SecurityError，退化用 hash（同样产生历史条目）。 */
const browserPort: HistoryPort = {
  push(state: unknown): boolean {
    try {
      history.pushState(state, "", location.href);
      return true;
    } catch {
      /* 落到 hash 方案 */
    }
    try {
      location.hash = `gosslan-layer-${(state as { gosslanBackLayer: number }).gosslanBackLayer}`;
      return true;
    } catch {
      return false; // 两个都不行：不压条目，返回键行为退化为"直接退出"（不比现状差）
    }
  },
  back() {
    history.back();
  },
};

const stack = createBackStack(browserPort);
let installed = false;

function install() {
  if (installed || typeof window === "undefined") return;
  installed = true;
  // 只听 popstate：pushState 与 hash 变化都会触发它，听两个事件会把一次返回处理两遍。
  window.addEventListener("popstate", () => {
    stack.onPop();
  });
}

/**
 * 把「一层 UI 是否打开」绑定到历史条目上。`active` 变真压条目，变假回收。
 *
 * ```ts
 * useBackLayer(() => app.isMobile && app.mobileView === "chat", () => (app.mobileView = "list"));
 * ```
 * 返回"手动释放"函数（组件卸载时自动调用）。
 */
export function useBackLayer(active: () => boolean, close: () => void): () => void {
  install();
  let release: (() => void) | null = null;

  watch(
    active,
    (on) => {
      if (on) {
        if (!release) release = stack.push(close);
      } else {
        release?.();
        release = null;
      }
    },
    { immediate: true },
  );

  const dispose = () => {
    release?.();
    release = null;
  };
  onBeforeUnmount(dispose);
  return dispose;
}

/** 诊断用：当前层数。 */
export function backLayerDepth(): number {
  return stack.depth();
}
