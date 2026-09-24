/**
 * 独立「群任务」窗口入口（`todos.html`，桌面端 label=`todo-<groupId>`，每群一个）。
 *
 * 与主窗口的差别（其余主题/骨架/错误上报/标题全在 `src/boot/boot.ts` 共用）：
 *   1. 根组件是 `GroupTodosWindow`（不挂载聊天布局）；
 *   2. 群 ID 从**窗口自己的 label** 解析（label 是身份/参数，不是"按 label 分支渲染"）；
 *   3. 挂载前**只**跑 `app.init()`（只读：主题/语言/设备）。
 *
 * ⚠️ 为什么不在这个门里等数据（用户 2026-09-24：「点查看任务，弹窗弹出很慢，我还以为没点了，
 * 点了好几下一会才弹出来」）：原先这里串了 `loadGroupTodos` + `watchGroupTodos`，
 * 而骨架屏是在 `mountAuxWindow` 挂载之后才撤的 ⇒ 四次刷新 + 一次消息拉取的整段时间里
 * 窗口内容什么都没有出现，只剩一块灰底；再叠加启动器的单飞/防抖把连点吃掉，
 * 体验就是"点了没反应"。现在窗口立刻挂载（看板自带"正在加载"态，见 `GroupTasksBoard`），
 * 数据与实时订阅在挂载之后补 —— 取数失败要 toast，不能安静地留一个空列表。
 *
 * ⚠️ **绝不**调用聊天 store 的初始化入口（`init`）：它会注册第二套后端事件监听
 * （重复通知 / 未读 / 群已读回执），见主窗口根组件顶部的说明。本窗口只订阅
 * **本会话**的 `message-received` 做实时刷新（`watchGroupTodos`）。
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import GroupTodosWindow from "@/components/GroupTodosWindow.vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";
import { groupTodosGroupId } from "@/utils/auxWindowLabels";
import { t } from "@/i18n";

installFrontendErrorReporting();

const groupId = groupTodosGroupId(getCurrentWindow().label);

void mountAuxWindow(createWindowApp(GroupTodosWindow), () => useAppStore().init()).then(
  async () => {
    if (!groupId) return; // 根组件会渲染明确的错误态
    const chat = useChatStore();
    const app = useAppStore();
    try {
      await chat.loadGroupTodos(groupId);
    } catch (e) {
      // 取数失败：看板已经不再转圈（`loadGroupTodos` 的 finally 置了位），但必须说清楚
      // "这是没读到，不是这个群没有任务" —— 静默空列表会被当成"任务丢了"。
      app.toastError(e, t("todo.loadFail"));
    }
    // 实时订阅与首屏取数无先后依赖：它失败只影响"远端改动自动刷新"，不该拖住开窗。
    await chat.watchGroupTodos(groupId).catch(() => {
      /* 订阅失败 ⇒ 窗口内不再自动刷新；用户关掉再开就重新读一次 */
    });
  },
);
