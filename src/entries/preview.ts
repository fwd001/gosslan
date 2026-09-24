/**
 * 独立「图片预览」窗口入口（`preview.html`，桌面端固定 label=`preview`，全局唯一）。
 *
 * 与设置/日志同口径：只跑 `app.init()`（只读：主题/语言/设备），
 * ⚠️ **绝不**调 `chat.init()` 或 `bindEvents()` —— 那会注册第二套后端事件监听
 * （重复通知 / 重复计未读 / 重复发群已读回执）。这个窗口连聊天 store 都不 import：
 * 它要的相册由自己的文档按 `msgId` / `cid` 去取字节（见 `PreviewWindow.vue`）。
 */
import PreviewWindow from "@/components/window/PreviewWindow.vue";
import { useAppStore } from "@/stores/useAppStore";
import {
  createWindowApp,
  installFrontendErrorReporting,
  mountAuxWindow,
} from "@/boot/boot";

installFrontendErrorReporting();

void mountAuxWindow(createWindowApp(PreviewWindow), () => useAppStore().init());
