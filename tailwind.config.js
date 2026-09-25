/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{vue,ts,tsx}"],
  theme: {
    extend: {
      colors: {
        primary: {
          DEFAULT: "var(--gosslan-primary)",
          hover: "var(--gosslan-primary-hover)",
          active: "var(--gosslan-primary-active)",
          light: "var(--gosslan-primary-light)",
        },
      },
      fontFamily: {
        gosslan: "var(--gosslan-font-family, -apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif)",
      },
    },
  },
  plugins: [
    /**
     * `desktop:` 变体 = 「本机不是移动布局」。
     *
     * 为什么要它：Tailwind 的 `md:`/`sm:` 只看视口宽度，而**移动端的视口宽度会说谎** ——
     * 安卓启动时系统权限弹框盖住 WebView 的首次布局，那一刻 `matchMedia` 读到的是
     * 兜底档位（980px 那档）⇒ 手机上 JS 判"移动"、CSS 判"桌面"，导航栏回来、
     * 底部安全区内边距被清零、抽屉按 980px 铺满（#27 现场）。
     * `is-mobile` 这个类由 `useAppStore.applyIsMobile()` 独家写（判据 = platform.ts 的
     * `resolveMobileLayout`，平台优先），CSS 只读它 —— 一处判、一处读，不留第二份口径。
     *
     * 用法是 `desktop:md:flex` 这种**叠加**：既要"桌面"也要"≥768px"，
     * 所以桌面窄窗口（拖到 500px）的行为与改造前一字不差。
     */
    function desktopVariant({ addVariant }) {
      addVariant("desktop", "html:not(.is-mobile) &");
    },
  ],
};
