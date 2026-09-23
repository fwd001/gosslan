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

/**
 * 上报「应用是否在前台且窗口聚焦」给后端（用户 2026-09-13 的功耗策略）。
 *
 * 后端蓝牙扫描据此在快/慢节奏间切换（前台 5s、后台/失焦 30s，见 `network/ble.rs`），
 * 并在从后台切回前台时**立刻补扫一轮**，而不是等完当前慢周期。
 *
 * 判定：
 *   · 页面不可见（后台 / 最小化 / 关闭到托盘后隐藏）⇒ 降频；
 *   · 桌面端窗口失焦（用户点了别的窗口）⇒ 也降频（用户明确要求）；
 *   · 移动端**不看 focus / blur**：软键盘弹出、系统弹框都会触发 blur，
 *     那会把"用户正在用"误判成后台。
 * 「APP 被杀死 ⇒ 直接关掉」不需要前端上报：进程没了，后端的扫描任务自然不存在。
 */
function reportActivity() {
  const active = !document.hidden && (app.isMobile || document.hasFocus());
  void api.setAppActive(active).catch(() => {});
}

onMounted(async () => {
  // 屏蔽 WebView 默认右键菜单（返回 / 刷新 / 另存为等），改为应用自定义交互：
  // 有功能的元素自行绑定右键菜单（见 MessageItem 的复制菜单），无功能的区域右键无效果。
  //
  // ⚠️ 例外（用户 2026-09-13）：「选中文本之后，没有弹出『复制』等选项」。
  // 一刀切 preventDefault 会把**有选区时**的系统菜单也吃掉 —— 桌面端右键选区拿不到
  // 原生「复制」，Android WebView 的选择工具条同样依赖 contextmenu 的默认行为。
  // 所以：**有非空选区时放行系统菜单**，其余情况维持原来的屏蔽。
  window.addEventListener("contextmenu", (e) => {
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && sel.toString().trim().length > 0) return;
    e.preventDefault();
  });
  // 前台/焦点变化 → 调整蓝牙扫描节奏（后端自适应，见 reportActivity）
  document.addEventListener("visibilitychange", reportActivity);
  window.addEventListener("focus", reportActivity);
  window.addEventListener("blur", reportActivity);
  try {
    // `app.init()`：只读偏好/设备/网卡（两个窗口都要）。
    // `chat.init()`：会话、好友、网络事件监听 —— **只在主窗口**跑，
    // 在独立窗口重复初始化会注册第二份监听并重复触发后端动作。
    //
    // 两者各自兜底（2026-09-23 审计 1.6）：app.init 失败（如 settings IPC 抖动）
    // 不得跳过 chat.init —— 那会让主窗口**永远收不到任何事件**（"活着但功能全死"，
    // 需重启恢复）；chat.init 失败也要留痕而不是静默吞进 unhandled rejection。
    try {
      await app.init();
    } catch (e) {
      console.error("[app] app.init 失败（偏好/设备可能未加载，继续启动）", e);
    }
    try {
      await chat.init();
    } catch (e) {
      console.error("[app] chat.init 失败", e);
    }
  } finally {
    // 真实数据就绪 → 让首屏骨架淡出（index.html 内联骨架，由 src/boot/boot.ts 撤除）。
    // 放在 finally：init 失败也要撤，否则骨架会一直挡在界面上。
    window.dispatchEvent(new Event("gosslan:app-ready"));
  }
  // init 之后再报一次：此时 `app.isMobile` 才是真实值（移动端不判 focus/blur）
  reportActivity();
  await applyWindowShape();
});

// 主题切换（浅/深/跟随系统变化）时同步窗口背景色，保持圆角外与内容同色。
watch(() => app.dark, () => void applyWindowShape());
</script>

<template>
  <ResponsiveLayout />
</template>
