import { ref, type Ref } from "vue";
import { useDeferredRef } from "@/composables/useDeferredRef";

/**
 * 搜索框的**纯输入状态**：`keyword` 绑输入框，`query` 是它的延迟镜像。
 *
 * ## 为什么不在这里搜（用户 2026-09-12 明确要求）
 * 「现在已经有聊天的列表，已经有搜索记录了。在上面输入，列表就不要有变化了。
 *   回车弹窗之后，在弹窗里面搜就行了。」
 * ⇒ 会话列表上的输入框**只当入口**：打字不改列表、不查消息；回车才打开
 * 「搜索聊天记录」弹窗，真正的搜索（跨会话 + 发送人/日期筛选 + loading）都在弹窗里做。
 * 联系人页仍用 `query` 做**姓名过滤**（那里就是要实时筛人）。
 *
 * ## 为什么保留延迟镜像
 * 姓名过滤会让整张联系人列表重渲染；按住 Ctrl+V 连发粘贴时关键词每秒变十几次，
 * 用原值会把主线程占满、输入框自己的 caret 掉帧（见 `useDeferredRef` 注释）。
 */
const DEFER_MS = 120;

export function useSearchKeyword(): { keyword: Ref<string>; query: Ref<string> } {
  const keyword = ref("");
  const query = useDeferredRef(keyword as Ref<string>, DEFER_MS);
  return { keyword, query };
}
