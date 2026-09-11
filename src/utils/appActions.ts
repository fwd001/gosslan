/**
 * 应用级动作名：以 **window 事件** 广播，由 UI 层订阅执行。
 *
 * 为什么绕一层 window 事件：macOS 原生菜单项与键盘快捷键最终要执行**同一批** UI 动作
 * （打开设置 / 添加好友 / 聚焦搜索）。若菜单直接去改组件内部状态，两条路径很容易
 * 行为不一致。统一广播 window 事件后，菜单、快捷键、未来的其它入口都只管"发出意图"，
 * UI 层负责执行 —— 与既有的 `navigate-to-contacts` 是同一套约定。
 *
 * ⚠️ 本模块必须是**零依赖**（不得 import 任何 `@/…`）：`useShortcuts` / `shortcuts` 这类
 * 纯逻辑也要用它，而它们会被 `node:test` 直接 import —— Node 无法解析 `@/` 别名。
 */
export const APP_ACTION = {
  openSettings: "gosslan:open-settings",
  addFriend: "gosslan:add-friend",
  focusSearch: "gosslan:focus-search",
  openLogs: "gosslan:open-logs",
} as const;

export type AppAction = (typeof APP_ACTION)[keyof typeof APP_ACTION];

/** 触发一个应用级动作（供菜单监听与快捷键共用）。 */
export function emitAction(action: string) {
  window.dispatchEvent(new Event(action));
}
