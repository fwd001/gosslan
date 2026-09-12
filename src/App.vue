<script setup lang="ts">
import { onMounted, watch } from "vue";
import { useAppStore } from "@/stores/useAppStore";
import { useChatStore } from "@/stores/useChatStore";
import { api } from "@/api";
import { isMac } from "@/utils/platform";
import ResponsiveLayout from "@/layouts/ResponsiveLayout.vue";

/**
 * 主窗口根组件。
 *
 * ⚠️ 这里**只**管主窗口。独立「设置」/「运行日志」窗口各有自己的 HTML 与入口
 * （`settings.html` → `src/entries/settings.ts`、`logs.html` → `src/entries/logs.ts`），
 * 不再由本组件按窗口 label 分支渲染。
 *
 * 为什么改掉旧做法（用户 2026-09-12 实测：「第二次打开设置，窗口会先刷成主聊天窗口，
 * 然后立马变成设置界面」）：旧模板是
 * `<LogViewer v-if="isLogs" /><SettingsWindow v-else-if="isSettings && ready" /><ResponsiveLayout v-else />`
 * —— `settings && !ready` 这段窗口期会落到 `v-else`，也就是**把整棵聊天布局挂起来**，
 * 于是每次开设置都要白等一整棵聊天组件树，还会先显示聊天界面。现在这个分支根本不存在了。
 */
const app = useAppStore();
const chat = useChatStore();

/** macOS 窗口圆角：必须在 WebView 加载完成后设（wry 此时才用 parent_view 替换 contentView，
 *  setup 阶段设会被替换丢失），并让窗口背景色跟随主题（消除暗色下圆角外露浅色的"白角"）。 */
async function applyWindowShape() {
  if (isMac && !app.isMobile) {
    await api.applyMacosWindowShape(app.dark).catch(() => {});
  }
}

onMounted(async () => {
  // 屏蔽 WebView 默认右键菜单（返回 / 刷新 / 另存为等），改为应用自定义交互：
  // 有功能的元素自行绑定右键菜单（见 MessageItem 的复制菜单），无功能的区域右键无效果。
  window.addEventListener("contextmenu", (e) => e.preventDefault());
  try {
    // `app.init()`：只读偏好/设备/网卡（两个窗口都要）。
    // `chat.init()`：会话、好友、网络事件监听 —— **只在主窗口**跑，
    // 在独立窗口重复初始化会注册第二份监听并重复触发后端动作。
    await app.init();
    await chat.init();
  } finally {
    // 真实数据就绪 → 让首屏骨架淡出（index.html 内联骨架，由 src/boot/boot.ts 撤除）。
    // 放在 finally：init 失败也要撤，否则骨架会一直挡在界面上。
    window.dispatchEvent(new Event("gosslan:app-ready"));
  }
  await applyWindowShape();
});

// 主题切换（浅/深/跟随系统变化）时同步窗口背景色，保持圆角外与内容同色。
watch(() => app.dark, () => void applyWindowShape());
</script>

<template>
  <ResponsiveLayout />
</template>
