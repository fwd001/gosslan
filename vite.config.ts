import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath, URL } from "node:url";

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [vue()],
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
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
}));
