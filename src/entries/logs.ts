/**
 * 独立「运行日志」窗口入口（`logs.html`，桌面端 label="logs"）。
 *
 * 日志窗口**不初始化任何业务状态**：日志数据由 `get_logs` 命令按需拉取（`LogViewer` 自己
 * 每 2s 轮询、窗口不可见时自动跳过），也不需要设置快照 —— 主题与语言在首帧由内联脚本
 * （`src/boot/theme-boot.js`）按 localStorage 处理好，所以这里没有 `beforeMount`。
 */
import { h } from "vue";
import LogViewer from "@/components/LogViewer.vue";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";

installFrontendErrorReporting();
void mountAuxWindow(
  createWindowApp({
    // 日志窗口永远是"独立窗口"形态（系统标题栏 + 自己的关闭按钮）
    render: () => h(LogViewer, { standalone: true }),
  }),
);
