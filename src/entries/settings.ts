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
  // 焦点重拉是**备用通道**：这扇窗今天不是常驻的（`AUX_WINDOWS_RESIDENT = false`，
  // 关闭即销毁 ⇒ 每次打开都是新数据），所以它不承担"防旧快照"的责任 —— 那条硬要求由
  // `windowEntries.test.ts` 的常驻判据现场从 Rust 读标记来核对，别把这里当兜底清单。
  // 留着的原因：万一哪天改回常驻，缺的就是这一处；而它一次只做四个只读拉取，不重订阅事件。
  // 偏好类数据由 `settings-changed` 事件同步，不在这里重拉。
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused) void useAppStore().refreshEnvironment();
  });
});
