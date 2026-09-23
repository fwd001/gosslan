/**
 * 独立「设置」窗口入口（`settings.html`，桌面端 label="settings"）。
 *
 * 与主窗口的差别只有两点，其余（主题、骨架、错误上报、标题）全在 `src/boot/boot.ts` 共用：
 *   1. 根组件是 `SettingsWindow`（不挂载聊天布局 —— 这是"窗口先闪成主聊天窗口"的根因修复）；
 *   2. 挂载前先跑 `app.init()`：它只做**只读**拉取（外观/语言/设备/网卡/共享目录），
 *      不启动网络、不注册聊天事件；不跑它设置页会显示默认值或空白。
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import SettingsWindow from "@/components/SettingsWindow.vue";
import { useAppStore } from "@/stores/useAppStore";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";

installFrontendErrorReporting();
void mountAuxWindow(createWindowApp(SettingsWindow), () => useAppStore().init()).then(() => {
  // 焦点刷新是这个窗口的**补救通道**（每次重新显示都重拉一遍会变的环境数据），
  // 所以它必须在"挂载前初始化失败"时也注册上 —— 这正是 `mountAuxWindow` 不再外抛的理由。
  // 这个窗口是**常驻**的（关闭只是隐藏），不会重新加载 ⇒ 每次重新显示（获得焦点）时
  // 刷一遍"会变的环境数据"（网卡/IP、共享目录、在线状态），否则用户切了 Wi-Fi 再打开设置
  // 看到的还是上次的快照。偏好类数据由 `settings-changed` 事件同步，不在这里重拉。
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused) void useAppStore().refreshEnvironment();
  });
});
