/**
 * 三个窗口（主 / 设置 / 日志）**共用**的首屏运行时。
 *
 * ## 为什么要有这个文件
 * 以前三个窗口共用 `index.html` + 一个 Vue 应用，由 `App.vue` 按窗口 label 决定渲染哪一屏。
 * 后果（用户 2026-09-12 实测）：
 *   - 设置窗口会**先把聊天三栏挂载出来**（`App.vue` 的 `v-else` 就是主界面），再换成设置页
 *     ⇒ "窗口先变成主聊天窗口、又立马变成设置界面"；
 *   - 每次打开都要加载并执行整棵聊天组件树（dev 下是几百个模块请求）⇒ 点一下要等很久；
 *   - 首屏骨架、主题决策、错误上报、骨架撤除这些**每个窗口都要做的事**，
 *     当时散在 `index.html` 内联脚本与 `main.ts` 里，靠 `window.__GOSSLAN_WINDOW__` 分支。
 *
 * 现在：每个窗口一个 HTML + 一个入口（`src/entries/*.ts`），本文提供三者共用的启动步骤，
 * 窗口之间的差异只剩「挂载哪个根组件、挂载前要不要先取数据」——没有分支、没有拷贝。
 */
// ⚠️ 应用样式必须在这里 import：三个窗口（主/设置/日志）都经过这个模块，
// 所以它保证**每个窗口都加载同一份 Tailwind + 应用样式**。
// 真实缺陷（用户 2026-09-12 实测「dev 起来整个聊天页和设置页样式全没了、像没有 CSS」）：
// 一窗一入口重构时，旧的 `src/main.ts`（里面有 `import "./style.css"`）被删掉，
// 而新的 `src/entries/*.ts` 没有带这句 ⇒ 打包出来的 CSS 为空、界面全裸。
// 这类退化**不会报错、也不影响任何测试**，只会让"样式全没"，所以另有护栏盯着（见 windowEntries.test.ts）。
import "@/style.css";
import { createApp, nextTick, watch, type App, type Component } from "vue";
import { createPinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { currentLocale } from "@/i18n";

// ---------------- 前端异常上报（必须在挂载之前注册） ----------------
// 为什么需要它：界面上"点了没反应"最常见的原因就是**一次 JS 异常** —— 渲染或事件处理里
// 抛出之后，整个交互看起来就死了；而前端异常此前**不留任何痕迹**，用户只能描述成"卡住了"。
// 现在它会进「运行日志」（logger.warn channel=ui），可以复制给我们定位。
// 去重 + 上限：同一个异常只报一次，整场最多 50 条，避免自己把日志刷爆。
const reportedErrorKeys = new Set<string>();
let reportedErrorCount = 0;

export function reportFrontendError(kind: string, detail: string) {
  const key = `${kind}:${detail.slice(0, 200)}`;
  if (reportedErrorKeys.has(key) || reportedErrorCount >= 50) return;
  reportedErrorKeys.add(key);
  reportedErrorCount += 1;
  void invoke("log_frontend_error", { kind, text: detail }).catch(() => {});
}

export function installFrontendErrorReporting() {
  window.addEventListener("error", (e) => {
    const where = e.filename ? `${e.filename}:${e.lineno}:${e.colno}` : "(位置未知)";
    const stack = e.error instanceof Error && e.error.stack ? `\n${e.error.stack}` : "";
    reportFrontendError("error", `${e.message} @ ${where}${stack}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const r: unknown = e.reason;
    // ⚠️ 必须**显式带上 message**：WebKit(Safari/WKWebView) 的 `error.stack` **不含消息行**，
    // 只打 stack 会把最关键的信息（例如 Headless UI 那句 "Passing props on template!" 里
    // 的组件名与属性清单）丢掉 —— 这是真踩过的坑。
    const text =
      r instanceof Error
        ? `${r.message}\n${r.stack ?? ""}`
        : typeof r === "string"
          ? r
          : JSON.stringify(r);
    reportFrontendError("rejection", text);
  });
}

// ---------------- 窗口标题 ----------------
/**
 * 让 `document.title` 跟随语言。标题由**每个窗口自己的 HTML** 用
 * `data-title-zh` / `data-title-en` 声明（主窗口「相闻」、设置窗口「相闻 · 设置」…），
 * 所以这里不需要按窗口写分支，也就不会出现"三个窗口各写一套"的冗余。
 * 切换语言时也会同步（独立窗口是常驻的，不能只在首次加载时算一遍）。
 */
export function installDocumentTitle() {
  const apply = () => {
    const el = document.documentElement;
    const zh = el.getAttribute("data-title-zh");
    const en = el.getAttribute("data-title-en");
    if (!zh && !en) return;
    document.title = currentLocale() === "zh-CN" ? zh || en || "" : en || zh || "";
  };
  apply();
  // `currentLocale()` 内部读的就是 i18n 的响应式 locale，所以这个 watcher 会在切换语言时触发。
  watch(currentLocale, apply);
}

// ---------------- 首屏骨架撤除 ----------------
let bootDismissed = false;

/**
 * 撤掉首屏骨架（淡出后移除）。三处触发：入口挂载完成（见 `mountAuxWindow`）、
 * 主窗口 `App.vue` 在真实数据就绪后派发的 `gosslan:app-ready`、以及兜底定时器。
 * 兜底必须留着：初始化异常时也不能把骨架永久挡在界面上。
 */
export function dismissBoot() {
  if (bootDismissed) return;
  bootDismissed = true;
  // 每个窗口只会有其中一个骨架（各自的 HTML 只写自己的那一份），
  // 这里统一处理，缺失的自然跳过。
  for (const id of ["boot", "boot-logs", "boot-settings"]) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.add("boot-hide");
    window.setTimeout(() => el.remove(), 220);
  }
}

function installBootDismissal() {
  window.addEventListener("gosslan:app-ready", dismissBoot);
  window.setTimeout(dismissBoot, 5000);
}

// ---------------- 应用装配 ----------------
/**
 * 建 Vue 应用（Pinia + 全局错误处理）。三个窗口完全一致，只是根组件不同。
 */
export function createWindowApp(root: Component): App {
  const app = createApp(root);
  // 渲染/生命周期里的异常：记日志（带组件名）并让 Vue 继续跑别的组件。
  // 一个组件抛错若没人接，Vue 只会把错误抛到 window —— 界面当次 patch 会中断，
  // 表现就是"这一页再也点不动"。这里至少把它变成**有名字、有位置**的一条日志。
  app.config.errorHandler = (err, _instance, info) => {
    const e = err instanceof Error ? err : new Error(String(err));
    reportFrontendError("vue", `${e.message} [${info}]\n${e.stack ?? ""}`);
  };
  app.use(createPinia());
  return app;
}

/**
 * 主窗口：挂载后立刻把窗口显示出来（`tauri.conf.json` 里主窗口是 `visible: false` 创建的）。
 *
 * 为什么由前端显示：窗口的静态 `backgroundColor` 只能是浅色或深色之一，若启动即显示，
 * 深色主题用户会先看到整屏浅色。这里在挂载完成后调用 `focus_window`，此刻
 * `index.html` 的内联骨架（含主题判断）已经在 DOM 里，露出来的第一帧就是骨架。
 * ⚠️ 不要改成「等 requestAnimationFrame」——窗口隐藏时合成器可能不产出帧，rAF 可能永远
 * 不触发，会把窗口永久留在隐藏态。Rust 侧另有 4s 超时兜底（见 lib.rs）。
 */
export function revealMainWindow() {
  void invoke("focus_window").catch(() => {
    /* 非 Tauri 环境（纯 vite dev）会 reject，忽略即可 */
  });
}

/** 主窗口：挂载 + 显示。骨架由 `App.vue` 在数据就绪后派发 `gosslan:app-ready` 撤除。 */
export function mountMainWindow(app: App) {
  installBootDismissal();
  installDocumentTitle();
  app.mount("#app");
  revealMainWindow();
}

/**
 * 独立窗口（设置 / 日志）：**先在挂载前把数据取回来，再挂载**。
 *
 * 为什么顺序不能反（真实缺陷，用户 2026-09-12 反馈「设置窗口的头像和名字好像有问题」）：
 * 子组件在 setup 阶段就会把 store 里的值**快照**进自己的 ref（例如 `ProfileSection` 的
 * `watch(..., { immediate: true })` 读 `app.device`），而数据是异步取的 ——
 * "先挂载、后拿数据"会把**空昵称/null 头像**写进各分区，数据到位后没人再同步。
 *
 * 撤骨架放在 `nextTick()` 之后：等真实内容真的挂上去了再让骨架淡出，中间不露空白帧。
 */
export async function mountAuxWindow(app: App, beforeMount?: () => Promise<void>) {
  installBootDismissal();
  installDocumentTitle();
  try {
    if (beforeMount) await beforeMount();
  } finally {
    app.mount("#app");
    await nextTick();
    window.dispatchEvent(new Event("gosslan:app-ready"));
  }
}
