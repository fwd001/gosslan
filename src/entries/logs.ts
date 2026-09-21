/**
 * 独立「运行日志」窗口入口（`logs.html`，桌面端 label="logs"）。
 *
 * 日志数据由 `get_logs` 命令按需拉取（`LogViewer` 自己每 2s 轮询、窗口不可见时自动跳过），
 * **不初始化任何业务状态、也不注册聊天事件监听**。
 *
 * 但要跑 `app.init()`（与设置窗口同口径，只读）：它会把**主题色**落到 CSS 变量上 ——
 * `theme-boot.js` 首帧只处理了亮暗与"当时"的主色，用户之后在设置里改了主题色，
 * 这个常驻窗口收不到 `settings-changed`（没初始化就没有订阅），操作按钮就会一直用旧色
 * （用户 2026-09-17：「日志弹框里的操作颜色没保持主题色」）。init 同时订阅
 * `settings-changed`，此后改主题/语言这里也会跟着变。
 */
import { h } from "vue";
import LogViewer from "@/components/LogViewer.vue";
import ToastHud from "@/components/ToastHud.vue";
import { useAppStore } from "@/stores/useAppStore";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";

installFrontendErrorReporting();
void mountAuxWindow(
  createWindowApp({
    // 日志窗口永远是"独立窗口"形态（自绘标题栏 + 窗口控制按钮）。
    // 本窗口不套 `AuxWindowShell`（日志页要兼移动端整页形态），所以 toast 宿主在这里显式挂
    // —— 否则「复制 / 清空 / 导出」的结果没有可见反馈（用户 2026-09-21：点了没反应）。
    render: () => [h(LogViewer, { standalone: true }), h(ToastHud)],
  }),
  () => useAppStore().init(),
);
