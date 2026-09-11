import { createApp } from "vue";
import { createPinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./style.css";
import App from "./App.vue";

const app = createApp(App);
app.use(createPinia());
app.mount("#app");

// ---------------- 窗口延迟显示（消除暗色主题下的亮色闪） ----------------
// 窗口在 tauri.conf.json 里以 `visible: false` 创建，原因：窗口的 `backgroundColor`
// 是**静态**色值，只能在浅色/深色里二选一。若启动即显示，深色主题用户在 WebView
// 画出首帧之前会看到整屏浅色（index.html 的内联骨架只解决了 WebView 内部的首帧，
// 解决不了 WebView 之前的窗口底色）。
//
// 这里在挂载完成后立刻显示窗口：此刻 index.html 的内联骨架（含主题判断）已经在
// DOM 里且样式已生效，所以窗口被显示出来的第一帧就是骨架，不存在空白/异色帧。
// ⚠️ 不要改成「等 requestAnimationFrame」——窗口隐藏时合成器可能不产出帧，
// rAF 可能永远不触发，会把窗口永久留在隐藏态。
// Rust 侧另有超时兜底（见 lib.rs），前端初始化异常时也不会出现"没有窗口的应用"。
function revealMainWindow() {
  // 独立的「运行日志」窗口由 Rust open_log_window 自行 show，不触发主窗口显示
  // （否则打开日志窗口会把已隐藏到托盘的主窗口也拉出来）。
  try {
    if (getCurrentWindow().label === "logs") return;
  } catch {
    /* 非 Tauri 环境（纯 vite dev）忽略 */
  }
  // 非 Tauri 环境（纯 vite dev）会 reject，忽略即可
  void invoke("focus_window").catch(() => {});
}
revealMainWindow();

// ---------------- 首屏骨架（见 index.html） ----------------
// 骨架写在 index.html 里内联，先于样式/脚本给出"正在启动"的画面，消除启动白屏。
// 真实数据就绪后由 App.vue 派发 `gosslan:app-ready` → 这里淡出并移除。
// 兜底定时器：初始化异常/卡住时也必须移除，绝不能把骨架永久挡在界面上。
const boot = document.getElementById("boot");
let bootDismissed = false;

function dismissBoot() {
  if (bootDismissed || !boot) return;
  bootDismissed = true;
  boot.classList.add("boot-hide");
  window.setTimeout(() => boot.remove(), 220);
}

window.addEventListener("gosslan:app-ready", dismissBoot);
window.setTimeout(dismissBoot, 5000);
