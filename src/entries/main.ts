/**
 * 主窗口入口（`index.html`）。
 *
 * 只有主窗口做"重"初始化：`App.vue` 在挂载后 `await app.init()`（偏好/设备/网卡）
 * 并 `await chat.init()`（会话、好友、网络事件监听）。设置/日志窗口各有自己的入口
 * （`src/entries/settings.ts` / `logs.ts`），不碰聊天状态、不注册聊天事件。
 */
import App from "@/App.vue";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountMainWindow,
} from "@/boot/boot";

installFrontendErrorReporting();
mountMainWindow(createWindowApp(App));
