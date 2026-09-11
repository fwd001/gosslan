import { ref } from "vue";
import { isImeKey } from "@/utils/ime";

/**
 * 「输入框里的回车」必须让路给输入法 —— 可复用的守卫。
 *
 * 为什么需要：`MessageComposer` 已经修过这个真实缺陷（用户：「输入拼音/英文后按回车，
 * 英文没落下来，中文却发出去了」）。根因是**不能只信 `KeyboardEvent.isComposing`**：
 * macOS WKWebView 上用 Enter 提交候选的顺序是 `compositionend` → `keydown`，
 * keydown 那一刻 `isComposing` 已是 false。
 *
 * 只要某处"回车 = 触发一个动作"（发送、打开搜索、提交表单），就会踩同一个坑。
 * 把它收成一个 composable，调用方三行接完：
 *
 * ```vue
 * const ime = useImeEnterGuard();
 * function onEnter(e: KeyboardEvent) {
 *   if (ime.isIme(e)) return; // 交给输入法，别执行动作
 *   emit("submit");
 * }
 * ```
 * 模板上同时挂 `@compositionstart="ime.onStart"` 与 `@compositionend="ime.onEnd"`。
 * （注意：JSDoc 里不要写嵌套的块注释，前一个 `*／` 会提前结束注释。）
 */
export function useImeEnterGuard() {
  const composing = ref(false);
  /** 最近一次 `compositionend` 的时间戳（ms）；从未发生为 0。 */
  let endedAt = 0;

  return {
    onStart: () => {
      composing.value = true;
    },
    onEnd: () => {
      composing.value = false;
      endedAt = Date.now();
    },
    /** true ⇒ 这次按键属于输入法提交，调用方必须直接 return（不要执行动作）。 */
    isIme: (e: KeyboardEvent) => isImeKey(e, composing.value, endedAt, Date.now()),
  };
}
