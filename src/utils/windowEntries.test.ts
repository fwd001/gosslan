/**
 * 窗口架构守卫：三个窗口（主 / 设置 / 日志）必须**各是一个文档 + 一个入口**。
 *
 * ## 为什么要有这层守卫（用户 2026-09-12 实测的三个现象）
 * 旧结构是「一个 index.html + 一个 Vue 应用」，由 `App.vue` 按窗口 label 决定渲染哪一屏：
 *   - 设置窗口会**先把聊天三栏挂载出来**（`App.vue` 的 `v-else` 就是主界面），再换成设置页
 *     ⇒ "第二次打开设置，窗口先刷成主聊天窗口、又立马变成设置界面"；
 *   - 每个窗口都要加载并执行整棵聊天组件树 ⇒ "点一下要等很久"；
 *   - 首屏骨架/主题/错误上报靠 `window.__GOSSLAN_WINDOW__` 分支 + 三份拷贝。
 *
 * 现在：`index.html` / `settings.html` / `logs.html` 各自一个入口。这类"退化"肉眼很难发现
 * （功能都还在，只是慢、只是闪一下），所以用静态守卫钉住下面这些结构性事实。
 */
import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";

const root = join(import.meta.dirname, "..", "..");
const read = (p: string) => readFileSync(join(root, p), "utf8");

/** 三个窗口：HTML 文件 ↔ 入口模块 ↔ 自己的骨架 id ↔ `<html>` 上的骨架类。 */
const WINDOWS = [
  { html: "index.html", entry: "src/entries/main.ts", skeleton: "boot", htmlClass: null },
  {
    html: "settings.html",
    entry: "src/entries/settings.ts",
    skeleton: "boot-settings",
    htmlClass: "boot-settings",
  },
  { html: "logs.html", entry: "src/entries/logs.ts", skeleton: "boot-logs", htmlClass: "boot-logs" },
] as const;

test("每个窗口都有自己的 HTML 与入口（不再共用一个 index.html）", () => {
  for (const w of WINDOWS) {
    assert.ok(existsSync(join(root, w.html)), `${w.html} 必须存在`);
    assert.ok(existsSync(join(root, w.entry)), `${w.entry} 必须存在`);
    const html = read(w.html);
    assert.match(
      html,
      new RegExp(`<script type="module" src="/${w.entry}"></script>`),
      `${w.html} 必须加载自己的入口 /${w.entry}`,
    );
  }
  // 反向：入口文件不能互相复用（否则又回到"一个入口按 label 分支"的老路）
  const entries = WINDOWS.map((w) => w.entry);
  assert.equal(new Set(entries).size, entries.length, "三个窗口必须是三个不同的入口");
});

test("每个窗口的 HTML 都必须带上主题脚本与骨架样式（否则该窗口首帧会闪白/闪错外观）", () => {
  for (const w of WINDOWS) {
    const html = read(w.html);
    for (const placeholder of ["<!-- GOSSLAN_THEME_BOOT -->", "<!-- GOSSLAN_SKELETON_CSS -->"]) {
      assert.ok(html.includes(placeholder), `${w.html} 缺少占位 ${placeholder}`);
    }
  }
  // 占位必须真的被 vite 插件替换：否则内联内容不会出现，占位就只是一句注释
  const vite = read("vite.config.ts");
  assert.match(vite, /GOSSLAN_THEME_BOOT/, "vite 插件要替换主题脚本占位");
  assert.match(vite, /GOSSLAN_SKELETON_CSS/, "vite 插件要替换骨架样式占位");
  assert.match(vite, /src\/boot\/theme-boot\.js/, "首屏脚本的事实来源是 src/boot/theme-boot.js");
  assert.match(vite, /src\/boot\/skeleton\.css/, "骨架样式的事实来源是 src/boot/skeleton.css");
});

test("每个窗口只带自己的骨架（设置/日志窗口绝不能出现聊天三栏骨架）", () => {
  for (const w of WINDOWS) {
    const html = read(w.html);
    for (const other of WINDOWS) {
      const id = `id="${other.skeleton}"`;
      // 用 `[\s>]` 而不是 `\b`：属性后面跟的是空格，`\b` 在 `"` 与空格之间不成立（真踩过）；
      // 同时它也不会把 `id="boot"` 误配到 `id="boot-settings"` 上。
      const present = new RegExp(`<div ${id}[\\s>]`).test(html);
      if (other.skeleton === w.skeleton) {
        assert.ok(present, `${w.html} 必须包含自己的骨架 ${id}`);
      } else {
        assert.ok(!present, `${w.html} 不该包含别的窗口的骨架 ${id}（那正是旧结构的做法）`);
      }
    }
    // `<html class="...">` 决定共用 CSS 里哪一套骨架规则生效：漏了类名 = 骨架不显示
    if (w.htmlClass) {
      assert.match(
        html,
        new RegExp(`<html[^>]*class="${w.htmlClass}"`),
        `${w.html} 的 <html> 必须有 class="${w.htmlClass}"（共用骨架 CSS 靠它生效）`,
      );
    }
  }
});

test("每个窗口都声明自己的标题（按语言切换，不再由 Rust 维护第二份文案）", () => {
  for (const w of WINDOWS) {
    const html = read(w.html);
    assert.match(html, /data-title-zh="[^"]+"/, `${w.html} 要有中文标题`);
    assert.match(html, /data-title-en="[^"]+"/, `${w.html} 要有英文标题`);
  }
  const boot = read("src/boot/theme-boot.js");
  assert.match(boot, /data-title-zh/, "首屏脚本要按语言设置 document.title");
});

test("辅助窗口的入口不得把聊天那一套拉进来（这是「设置窗口先闪成聊天界面」的根因）", () => {
  const forbidden = ["ResponsiveLayout", "App.vue", "useChatStore", "ChatWindow", "ConversationList"];
  for (const entry of ["src/entries/settings.ts", "src/entries/logs.ts"]) {
    const code = read(entry);
    for (const bad of forbidden) {
      assert.ok(
        !code.includes(bad),
        `${entry} 不该引用 ${bad} —— 独立窗口只加载自己需要的代码，` +
          `否则会把整棵聊天组件树挂起来再换掉（慢 + 闪）`,
      );
    }
  }
  // 反向：主窗口入口当然要挂聊天布局（否则这个守卫可能只是"全都空了"而通过）
  assert.match(read("src/entries/main.ts"), /App\.vue/, "主窗口入口要挂 App.vue");
  assert.match(read("src/App.vue"), /ResponsiveLayout/, "主窗口根组件要渲染聊天布局");
});

test("App.vue 不得再按窗口 label 分支渲染（分支回来 = 老问题复现）", () => {
  // ⚠️ 变量别叫 `app`：`storeContract` 守卫会把 `app.*` 当成"界面用到的 store 成员"来核对，
  //    在测试文件里叫 `app` 会被它误判成用了不存在的 store 成员（真踩过）。
  const appVue = read("src/App.vue");
  for (const bad of ["isSettingsWindow", "isLogsWindow", "isStandaloneWindow", "__GOSSLAN_WINDOW__"]) {
    assert.ok(!appVue.includes(bad), `App.vue 里不该再有 ${bad}（窗口差异应由各自的文档/入口承担）`);
  }
  // 全局也不该再有"注入窗口标识再在同一个 HTML 里切换"的老机制
  assert.ok(
    !read("index.html").includes("__GOSSLAN_WINDOW__"),
    "index.html 不该再依赖注入的窗口标识",
  );
});
