/**
 * 独立「设置」窗口入口（`settings.html`，桌面端 label="settings"）。
 *
 * 与主窗口的差别只有两点，其余（主题、骨架、错误上报、标题）全在 `src/boot/boot.ts` 共用：
 *   1. 根组件是 `SettingsWindow`（不挂载聊天布局 —— 这是"窗口先闪成主聊天窗口"的根因修复）；
 *   2. 挂载前先跑 `app.init()`：它只做**只读**拉取（外观/语言/设备/网卡/共享目录），
 *      不启动网络、不注册聊天事件；不跑它设置页会显示默认值或空白。
 */
import SettingsWindow from "@/components/SettingsWindow.vue";
import { useAppStore } from "@/stores/useAppStore";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";

installFrontendErrorReporting();
void mountAuxWindow(createWindowApp(SettingsWindow), () => useAppStore().init());
