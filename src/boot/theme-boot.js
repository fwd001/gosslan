// Gosslan 首屏主题脚本（**内联进每个窗口的 HTML**，由 vite.config.ts 的
// `gosslan:inline-boot` 插件注入；不要写成模块 —— 它必须早于样式/模块脚本执行）。
//
// 三件事必须在「第一批像素」之前做完，否则用户会看到闪白/闪错外观：
//   ① 亮暗与主色（localStorage，与 useAppStore 的 APPEARANCE_KEY/THEME_KEY 保持一致）；
//   ② 语言（决定骨架里的 brand 文案与 <title>）；
//   ③ macOS 平台标记（骨架标题栏高度 28px vs 38px）。

      // 首屏主题：必须在样式/脚本资源就位**之前**决定亮暗与主色，
      // 否则 WebView 会先按浅色渲染再切深色（用户看到"闪一下白"）。
      // 键名与 useAppStore 里的 APPEARANCE_KEY / THEME_KEY / LEGACY_DARK_KEY 保持一致，改动请同步。
      //
      // 外观三态：显式 light / dark 直接生效；"system"（或旧数据）跟随系统偏好。
      // ⚠️ 这段逻辑必须与 useAppStore 的 readLocalAppearance() + dark computed 完全一致，
      //    否则首帧骨架与真界面会呈现不同外观（启动瞬间闪一下）。
      (function () {
        try {
          var el = document.documentElement;
          var mode = localStorage.getItem("gosslan.appearance");
          if (mode !== "system" && mode !== "light" && mode !== "dark") {
            // 旧版本只存了 gosslan.dark 布尔值：迁移为显式模式（不静默丢弃用户偏好）
            var legacy = localStorage.getItem("gosslan.dark");
            mode = legacy === "1" ? "dark" : legacy === "0" ? "light" : "system";
          }
          var wantDark =
            mode === "dark" ||
            (mode === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
          if (wantDark) el.classList.add("dark");
          var c = localStorage.getItem("gosslan.themeColor");
          if (c) el.style.setProperty("--gosslan-primary", c);

          // 语言首帧：跟随系统语言（与 src/i18n 的 detectSystemLocale 逻辑一致，改动请同步）。
          // 显式偏好（gosslan.locale）优先；否则 zh* → 中文，其余（含 en*）→ 英文。
          var lang = "en-US";
          var prefLang = localStorage.getItem("gosslan.locale");
          if (prefLang === "zh-CN" || prefLang === "en-US") {
            lang = prefLang;
          } else {
            var sys = navigator.languages && navigator.languages.length
              ? navigator.languages
              : [navigator.language];
            for (var i = 0; i < sys.length; i++) {
              var tag = (sys[i] || "").toLowerCase();
              if (tag.indexOf("zh") === 0) { lang = "zh-CN"; break; }
              if (tag.indexOf("en") === 0) { lang = "en-US"; break; }
            }
          }
          el.lang = lang;
          // 应用显示名本地化：中文「相闻」/ 英文 "Gosslan"（<title> 与骨架 brand 共用）。
          // 每个窗口的标题由它自己的 HTML 用 data-title-zh / data-title-en 声明
          // （主窗口「相闻」、设置窗口「相闻 · 设置」、日志窗口「相闻 · 运行日志」）——
          // 三个窗口共用这一份脚本，不需要各自写分支。
          var zh = el.getAttribute("data-title-zh");
          var en = el.getAttribute("data-title-en");
          if (zh || en) document.title = lang === "zh-CN" ? zh || en : en || zh;
          // macOS 标记：首屏骨架需要按平台取标题栏高度（原生 macOS caption 是 28px，
          // 而 Windows/Linux 用 38px 的自绘 caption）——骨架先于 app.init 运行，
          // 所以这里自己判一次。平台判定只认 `Macintosh`（不要用 `Mac OS X`，本项目既有约定）。
          try {
            if (/Macintosh/.test(navigator.userAgent)) el.classList.add("is-mac");
          } catch (e) {
            /* 忽略：非浏览器环境 */
          }        } catch (e) {
          /* localStorage / matchMedia 不可用（隐私模式等）→ 按浅色渲染即可，不影响后续 */
        }
      })();
