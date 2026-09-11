import { onBeforeUnmount, ref, watch, type Ref } from "vue";
import { debounce } from "@/utils/defer";

/**
 * 输入框的**延迟镜像**：输入值立即变、派生渲染慢一步。
 *
 * 用法（见 `ConversationList.vue` / `LogViewer.vue` 等）：
 *
 * ```ts
 * const keyword = ref("");                       // v-model 绑这个（DOM 立即更新）
 * const query = useDeferredRef(keyword, 120);    // 过滤/高亮/后端查询用这个
 * ```
 *
 * 为什么不是"两个都即时"：派生的重活（列表过滤 + 上千行 `v-html`、跨进程查询）
 * 会把主线程占满，反而让输入框的 caret/上屏丢掉帧 —— 用户看到的就是"一顿一顿"。
 * 让原值走在前面、派生渲染按时间窗合并，输入体验就与"不做任何派生"一样跟手。
 *
 * `delay` 取值：本地过滤 120ms（打字停顿一下就能看到结果，感觉不到延迟）；
 * 需要跨进程的查询另有自己的去抖（见 `useConversationSearch` 的 300ms），
 * 两者相加仍在可接受范围。
 */
export function useDeferredRef<T>(source: Ref<T>, delay = 120): Ref<T> {
  const deferred = ref(source.value) as Ref<T>;
  const commit = debounce((v: T) => {
    deferred.value = v;
  }, delay);
  watch(source, (v) => commit(v));
  // 卸载时丢弃待执行的提交：避免组件没了还回写状态（也避免悬挂定时器）
  onBeforeUnmount(() => commit.cancel());
  return deferred;
}
