/**
 * 外观模式（跟随系统 / 浅色 / 深色）的解析规则 —— 抽成纯函数，为了能被测。
 *
 * 为什么单独成模块：这套规则有**两份实现**，且必须永远一致：
 *   ① `useAppStore`（bundle 之后运行）；
 *   ② `index.html` 里的内联首屏脚本（**必须跑在 bundle 之前**，否则 WebView 先按浅色
 *      渲染再切深色，用户会看到"闪一下白"，所以它无法 import 本模块）。
 * 两份实现一旦漂移，症状是"启动瞬间骨架与真界面外观不一致"，很难在日常自测中发现。
 * → 规则收敛到这里的纯函数，并由 `appearance.test.ts` **把 index.html 那段脚本真跑一遍**
 *   来对照，而不是靠注释提醒。
 */

/** 外观模式：跟随系统 / 强制浅色 / 强制深色。 */
import { parseConfigBool } from "./configBool.ts";

export type AppearanceMode = "system" | "light" | "dark";

/** 外观模式的存储键（localStorage 只作启动首帧的快路径，真值在后端 settings）。 */
export const APPEARANCE_STORAGE_KEY = "gosslan.appearance";

/** 旧版本（≤2.0.3）只存了这个布尔值，仅用于一次性迁移读取，新代码不再写入。 */
export const LEGACY_DARK_STORAGE_KEY = "gosslan.dark";

/** 只读存储的最小接口 —— 便于用普通对象在测试里替身，不依赖 DOM 的 Storage 类型。 */
export interface ReadonlyStorage {
  getItem(key: string): string | null;
}

export function isAppearanceMode(v: unknown): v is AppearanceMode {
  return v === "system" || v === "light" || v === "dark";
}

/**
 * 由「用户意图 + 系统偏好」解析出**实际**是否为深色。
 * 强制模式下系统怎么变都不影响结果 —— 用户明确选了，就不该被系统覆盖。
 */
export function resolveDark(mode: AppearanceMode, systemDark: boolean): boolean {
  return mode === "system" ? systemDark : mode === "dark";
}

/**
 * 读取存储中的外观模式；老数据（只有布尔 `gosslan.dark`）迁移为**显式**模式。
 *
 * ⚠️ 刻意不把老布尔值当"没设置过"：那是用户当时的一次显式选择，
 * 升级后直接变成"跟随系统"会把他的深色偏好静默丢掉。
 */
export function readStoredAppearance(storage: ReadonlyStorage): AppearanceMode {
  const raw = storage.getItem(APPEARANCE_STORAGE_KEY);
  if (isAppearanceMode(raw)) return raw;
  // 口径走 `parseConfigBool`（#125）：老数据只有 "1"/"0" 两种写法，行为与旧实现一致；
  // 但 "true"/"yes"/"off" 这些**同族写法**以前会被静默当成"没设置过"⇒ 用户的深色偏好会被丢掉。
  const legacy = parseConfigBool(storage.getItem(LEGACY_DARK_STORAGE_KEY));
  if (legacy === true) return "dark";
  if (legacy === false) return "light";
  return "system";
}
