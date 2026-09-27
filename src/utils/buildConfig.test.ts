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

/**
 * **每一个"打生产包"的命令都必须开 `bluetooth` feature**（2026-09-13 合并评审）。
 *
 * 为什么：BLE 在 Cargo 里是**可选 feature**（ADR-0015 §2：不开时依赖不下载、代码不编译）。
 * 漏了 `--features bluetooth` 的后果是**静默**的 —— 构建成功、产物正常、只是那个包
 * **完全没有蓝牙**（设置页的开关永远起不来、也搜不到任何设备）。
 * 真机代价：用户拿一个"没有蓝牙的包"去测 Windows ↔ Android，白跑一轮。
 * 这个坑在 `dist:win` / `dist:win:msi` / 便携版脚本上都出现过，而**一键入口
 * `npm run dist`（scripts/package.mjs）**也漏过一次 —— 那才是日常用的那条。
 */
test("打生产包的命令必须带 --features bluetooth（否则产物静默地没有蓝牙）", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as {
    scripts: Record<string, string>;
  };
  const missing: string[] = [];
  for (const [name, cmd] of Object.entries(pkg.scripts)) {
    // 只看"真的在编应用"的命令：tauri build / bash scripts/build-android-releases.sh
    const buildsApp = /tauri build|build-android-releases\.sh/.test(cmd);
    if (!buildsApp) continue;
    // 安卓那条脚本内部自己带 feature（见该脚本头注释），这里只查 tauri build
    if (!/tauri build/.test(cmd)) continue;
    if (!/--features\s+bluetooth/.test(cmd)) missing.push(`package.json:${name} → ${cmd}`);
  }
  const packer = readFileSync(join(root, "scripts", "package.mjs"), "utf8");
  for (const m of packer.matchAll(/cmd:\s*`([^`]*tauri build[^`]*)`/g)) {
    if (!/--features\s+bluetooth/.test(m[1])) missing.push(`scripts/package.mjs → ${m[1].trim()}`);
  }
  assert.deepEqual(
    missing,
    [],
    "下面这些命令会打出**没有蓝牙**的包（构建照样成功！）：\n" +
      missing.map((m) => `  · ${m}`).join("\n") +
      "\n请补上 `--features bluetooth`（Tauri 的 feature 默认关闭，ADR-0015 §2）。",
  );
});

/**
 * #126（2026-09-27 用户拍板）：门禁里跑双实例 E2E 的那份二进制，**必须与打包同一 feature 集**。
 *
 * 判的形状：`scripts/verify.mjs` 的 local 层必须存在一步"用 `--features bluetooth` 编 release 二进制"，
 * 且 harness 自己打印的补救提示也带这个 flag（人照着提示做的事，就是门禁实际做的事 —— 两处不一致时
 * 下一次一定按提示做，于是又测回不带蓝牙的那份）。
 */
test("门禁的 E2E 二进制必须与打包同 feature 集（测的那份＝发的那份，#126）", () => {
  const verify = readFileSync(new URL("../../scripts/verify.mjs", import.meta.url), "utf8");
  const harness = readFileSync(new URL("../../scripts/e2e-multi-instance.mjs", import.meta.url), "utf8");
  const step = /group: "local",\s*name: "构建与打包同 feature 集[^]*?args: \["build", "--release", "--features", "bluetooth"\]/.exec(
    verify,
  );
  assert.ok(
    step,
    "verify.mjs 的 local 层必须有一步 `cargo build --release --features bluetooth`，否则 E2E 测的是不含蓝牙的那份二进制",
  );
  assert.match(
    harness,
    /cargo build --release --features bluetooth/,
    "harness 的补救提示必须与门禁实际做的事一致（不带 feature 的提示会把人带回旧的缺口）",
  );
});
