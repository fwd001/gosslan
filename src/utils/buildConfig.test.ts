/**
 * 打包配置守卫（踩过就有价值的那几条）。
 *
 * ## 为什么要有
 * `vite.config.ts` 里一个"看起来只是风格问题"的写法，会让**所有 release 包**的前端
 * 不压缩还带 sourcemap：`TAURI_ENV_DEBUG` 是**字符串**（Tauri 对 release 设成 `"false"`），
 * 而 `!"false"` 是 **false** ⇒ `minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false`
 * 把 release 当成了 debug。实测（安卓 release 出包）：`main-*.js` 从 310KB 涨到 **500KB**，
 * 外加 735KB 的 `.map` 一起被打进包 —— 手机上就是多出来的解析时间。
 *
 * 这类配置退化**不会报错、也不会让任何测试变红**，只会悄悄让包变大变慢，
 * 所以必须用守卫钉住。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";

const root = join(import.meta.dirname, "..", "..");
const viteConfig = readFileSync(join(root, "vite.config.ts"), "utf8");
// 去掉注释再判"有没有踩坑的写法"：这段坑本身就在注释里写着（给后来人看），
// 不去注释会把"解释这个坑"误判成"又踩了这个坑"（真踩过这个假阳性）。
const viteCode = viteConfig.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

test("release 构建必须压缩前端：不得直接把 TAURI_ENV_DEBUG 取反", () => {
  assert.ok(
    !/!\s*process\.env\.TAURI_ENV_DEBUG/.test(viteCode),
    "`!process.env.TAURI_ENV_DEBUG` 会把字符串 \"false\"（release）当成真 ⇒ release 不压缩、还带 sourcemap。\n" +
      "请改成判断字面量：`process.env.TAURI_ENV_DEBUG === \"true\"`",
  );
});

test("debug 判定必须是字面量比较（并且真的用在了 minify / sourcemap 上）", () => {
  assert.match(
    viteConfig,
    /process\.env\.TAURI_ENV_DEBUG === "true"/,
    "必须显式判断 \"true\" 才算 debug 构建",
  );
  assert.match(viteConfig, /minify:\s*isDebugBuild \? false : "esbuild"/, "minify 必须用它判定");
  assert.match(viteConfig, /sourcemap:\s*isDebugBuild/, "sourcemap 必须用它判定");
});
