/**
 * 独立「群任务」窗口入口（`todos.html`，桌面端固定 label=`tasks`，全局只有一扇）。
 *
 * 与主窗口的差别（其余主题/骨架/错误上报/标题全在 `src/boot/boot.ts` 共用）：
 *   1. 根组件是 `GroupTodosWindow`（不挂载聊天布局）；
 *   2. 挂载前**只**跑 `app.init()`（只读：主题/语言/设备）。
 *
 * ⚠️ 为什么不在这个门里等数据（用户 2026-09-24：「点查看任务，弹窗弹出很慢，我还以为没点了，
 * 点了好几下一会才弹出来」）：原先这里串了 `loadGroupTodos` + `watchGroupTodos`，而骨架屏
 * 是 `mountAuxWindow` 挂载之后才撤的 ⇒ 四次刷新 + 一次消息拉取的整段时间里窗口内容什么都没有
 * 出现，只剩一块灰底；再叠加启动器的单飞/防抖把连点吃掉，体验就是"点了没反应"。
 *
 * ⚠️ 为什么取数与订阅**不在这里、而在根组件里**（同一批改动里 label 从 `todo-<groupId>`
 * 换成固定的 `tasks`）：那扇窗看的是哪个群会在运行中被换掉（预热时甚至还没有群），
 * 写在入口的门里只有"第一次打开"那条路有效，换群时反而没人负责重读数据。
 *
 * ⚠️ **绝不**调用聊天 store 的初始化入口（`init`）：它会注册第二套后端事件监听
 * （重复通知 / 未读 / 群已读回执），见主窗口根组件顶部的说明。实时刷新由根组件按
 * **当前那个会话**订阅 `message-received`（`watchGroupTodos`）。
 */
import GroupTodosWindow from "@/components/GroupTodosWindow.vue";
import { useAppStore } from "@/stores/useAppStore";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";

installFrontendErrorReporting();

void mountAuxWindow(createWindowApp(GroupTodosWindow), () => useAppStore().init());
