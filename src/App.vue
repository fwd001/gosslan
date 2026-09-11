<script setup lang="ts">
import { onMounted, watch } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import { isMac } from "@/utils/platform";
import ResponsiveLayout from "@/layouts/ResponsiveLayout.vue";
import LogViewer from "@/components/LogViewer.vue";
import SettingsWindow from "@/components/SettingsWindow.vue";

const app = useAppStore();
const chat = useChatStore();

/**
 * 是否为独立的「运行日志」窗口（桌面端由 open_log_window 动态创建，label="logs"）。
 * 该窗口**不初始化主业务**（不跑 app.init / chat.init，避免重复网络、事件监听），
 * 只渲染日志页；主题由 index.html 内联首屏脚本按 localStorage 处理好。
 */
const isLogsWindow = (() => {
  try {
    return getCurrentWindow().label === "logs";
  } catch {
    return false; // 纯 vite dev / 非 Tauri 环境
  }
})();

/**
 * 是否为独立的「设置」窗口（桌面端由 open_settings_window 动态创建，label="settings"）。
 * 与日志窗口同理：**不初始化主业务**（不跑 app.init / chat.init，避免重复网络与事件监听），
 * 只渲染设置内容；主题由 index.html 内联首屏脚本按 localStorage 处理好。
 */
const isSettingsWindow = (() => {
  try {
    return getCurrentWindow().label === "settings";
  } catch {
    return false;
  }
})();

/** 独立窗口（日志 / 设置）只渲染各自页面，不参与主窗口的圆角与托盘逻辑。 */
const isStandaloneWindow = isLogsWindow || isSettingsWindow;

/** macOS 窗口圆角：必须在 WebView 加载完成后设（wry 此时才用 parent_view 替换 contentView，
 *  setup 阶段设会被替换丢失），并让窗口背景色跟随主题（消除暗色下圆角外露浅色的"白角"）。
 *  仅主窗口需要；日志窗口用系统标题栏，不画自绘圆角。 */
async function applyWindowShape() {
  if (isMac && !app.isMobile && !isStandaloneWindow) {
    await api.applyMacosWindowShape(app.dark).catch(() => {});
  }
}

onMounted(async () => {
  // 屏蔽 WebView 默认右键菜单（返回 / 刷新 / 另存为等），改为应用自定义交互：
  // 有功能的元素自行绑定右键菜单（见 MessageItem 的复制菜单），无功能的区域右键无效果。
  window.addEventListener("contextmenu", (e) => e.preventDefault());
  if (isLogsWindow) {
    // 日志窗口：只撤骨架，不初始化任何业务状态（日志数据由 get_logs 命令按需拉取）。
    window.dispatchEvent(new Event("gosslan:app-ready"));
    return;
  }
  try {
    // ⚠️ 设置窗口也要跑 `app.init()`：它只做**只读**的偏好/设备/网卡/网络状态拉取
    // （外观、语言、聊天样式、设备指纹、共享目录…），不会启动网络或注册聊天事件。
    // 不跑它，设置页会整片空白或显示默认值（用户会以为设置丢了）。
    await app.init();
    // 主业务（会话/好友/网络事件监听）只在主窗口初始化：
    // 在设置窗口重复初始化会注册第二份监听、并可能重复触发后端动作。
    if (!isSettingsWindow) await chat.init();
  } finally {
    // 真实数据就绪 → 让 main.ts 撤掉首屏骨架（index.html 内联）。
    // 放在 finally：init 失败也要撤，否则骨架会一直挡在界面上。
    window.dispatchEvent(new Event("gosslan:app-ready"));
  }
  // 圆角与窗口底色只对主窗口有意义（独立窗口用系统标题栏）。
  if (!isStandaloneWindow) await applyWindowShape();
});

// 主题切换（浅/深/跟随系统变化）时同步窗口背景色，保持圆角外与内容同色。
watch(() => app.dark, () => void applyWindowShape());
</script>

<template>
  <LogViewer v-if="isLogsWindow" standalone />
  <SettingsWindow v-else-if="isSettingsWindow" />
  <ResponsiveLayout v-else />
</template>
