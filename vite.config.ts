import { defineConfig, type Plugin } from "vite";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath, URL } from "node:url";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const here = fileURLToPath(new URL(".", import.meta.url));

/**
 * 把「首屏主题脚本」与「骨架样式」**内联**进每个窗口的 HTML。
 *
 * 为什么需要插件（而不是在三个 HTML 里各抄一份）：三个窗口都必须在内联样式/模块脚本执行
 * **之前**决定亮暗、主色、语言，否则会闪白/闪错外观；这三份逻辑一旦复制成三份，
 * 改一处忘一处就是"某个窗口启动时闪一下"这种极难复现的 bug。留一份事实来源：
 *   - `src/boot/theme-boot.js`  —— 首帧主题/语言/平台标记；
 *   - `src/boot/skeleton.css`    —— 骨架关键样式。
 * 插件在 dev 与 build 下都会替换占位注释，两边行为一致。
 */
function inlineBoot(): Plugin {
  const read = (p: string) => readFileSync(resolve(here, p), "utf8");
  return {
    name: "gosslan:inline-boot",
    transformIndexHtml: {
      order: "pre",
      handler(html) {
        return html
          .replace(
            "<!-- GOSSLAN_THEME_BOOT -->",
            `<script>\n${read("src/boot/theme-boot.js")}    </script>`,
          )
          .replace(
            "<!-- GOSSLAN_SKELETON_CSS -->",
            `<style>\n${read("src/boot/skeleton.css")}    </style>`,
          );
      },
    },
  };
}

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [vue(), inlineBoot()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  // Tauri 期望一个固定端口；CI 环境下端口号需保持一致
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      // 避免对 src-tauri 的改动触发前端热更新
      ignored: ["**/src-tauri/**"],
    },
  },
  // 生产构建时排除 Tauri 相关环境变量
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target:
      process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    rollupOptions: {
      // 三个窗口 = 三个 HTML 入口（Rust 侧用 WebviewUrl::App("<name>.html") 打开）。
      // 各自的入口只 import 自己需要的代码，所以设置/日志窗口不会加载聊天那一大坨
      // —— 这是"点设置要等很久、还会先闪一下聊天界面"的主要修法之一。
      input: {
        main: resolve(here, "index.html"),
        settings: resolve(here, "settings.html"),
        logs: resolve(here, "logs.html"),
      },
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("highlight.js")) return "highlight";
          if (id.includes("vue-easy-lightbox")) return "lightbox";
          if (id.includes("lucide-vue-next")) return "icons";
          if (id.includes("@tauri-apps")) return "tauri";
          if (
            id.includes("node_modules/vue/") ||
            id.includes("node_modules/@vue/")
          )
            return "vue";
          return "vendor";
        },
      },
    },
  },
}));
