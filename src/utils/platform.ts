/**
 * 平台判定。
 *
 * 单文件只放一个判定，是因为它已经有多个消费者（标题栏的窗口按钮布局、
 * 快捷键的修饰键选择），各写一份迟早会漂移。
 *
 * 判据：Tauri 的 WebView 里 `navigator.userAgent` 在 macOS 含 `Macintosh`。
 * ⚠️ 这是**运行时**判定，不能用于 gating 原生代码（那是 `#[cfg(target_os)]` 的事）。
 */

/**
 * 判断一段 UA 是否来自桌面 macOS（纯函数，便于单测）。
 *
 * ⚠️ 只用 `Macintosh`，不能用 `Mac OS X`：iOS / iPadOS 的 UA 形如
 * `Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) ...`，
 * 其中 `like Mac OS X` 会命中 `/Mac OS X/`，从而把 iPhone/iPad 误判成 Mac ——
 * 表现是移动端标题栏错误地渲染红绿灯、快捷键误用 ⌘ 而非 ctrl。
 * 桌面 macOS 的 UA 恒含 `Macintosh`（Apple Silicon 上也是 `Macintosh; Intel Mac OS X`）。
 */
export function isMacUA(ua: string): boolean {
  return /Macintosh/i.test(ua);
}

/**
 * 运行时平台判定（Tauri WebView UA）。
 * 加 `typeof navigator` 守卫：本模块被 Node 单测 import 时 navigator 不存在，
 * 这里不能因取 UA 抛错（单测只关心 isMacUA 这个纯函数）。
 */
export const isMac =
  typeof navigator !== "undefined" && typeof navigator.userAgent === "string"
    ? isMacUA(navigator.userAgent)
    : false;

/**
 * 判断一段 UA 是否来自 Android（纯函数，便于单测）。
 *
 * 用途：移动端文件交互与桌面不同 —— Android 上系统经常没有能"打开"某类文件的应用
 * （用户实测：除图片外基本都报错），所以点文件应改为"另存为"（系统 SAF 保存对话框）。
 * 必须按 UA 判平台，不能按屏幕宽度（窄桌面窗口也会命中 isMobile）。
 */
export function isAndroidUA(ua: string): boolean {
  return /Android/i.test(ua);
}

/** 运行时平台判定（Android）。与 isMac 同一套 navigator 守卫。 */
export const isAndroid =
  typeof navigator !== "undefined" && typeof navigator.userAgent === "string"
    ? isAndroidUA(navigator.userAgent)
    : false;

/**
 * 判断一段 UA 是否来自 iOS / iPadOS（纯函数，便于单测）。
 *
 * 用途：返回箭头按平台给原生观感 —— iOS 用 chevron（`<`），其余平台（Android / 桌面）
 * 用 Material 的左箭头（`←`，用户 2026-09-20：「桌面版的箭头回退到 Android，iOS 保持 iOS 风格」）。
 *
 * ⚠️ 用 `iPhone|iPad|iPod`，**不能**用 `/Mac OS X/`：桌面 macOS 的 UA 也含 `Mac OS X`，
 * 会把 Mac 误判成 iOS。已知局限：iPadOS 13+ 的「桌面模式」UA 伪装成 `Macintosh`，
 * 这里会落到非 iOS 分支（本项目不发行 iPad，可接受，与 isMacUA 的注释同一口径）。
 */
export function isIOSUA(ua: string): boolean {
  return /iPad|iPhone|iPod/.test(ua);
}

/** 运行时平台判定（iOS）。与 isMac 同一套 navigator 守卫。 */
export const isIOS =
  typeof navigator !== "undefined" && typeof navigator.userAgent === "string"
    ? isIOSUA(navigator.userAgent)
    : false;
