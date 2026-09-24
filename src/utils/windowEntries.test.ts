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

/**
 * 剥掉注释后的代码。**禁止性判据（`!includes(坏东西)`）必须用它**：
 * 否则"注释里写着这里绝不调用 chat.init()"会自己踩中红线 —— 越认真解释为什么不能用，
 * 守卫越红（真踩过一次）。正向断言仍读原文，别让剥注释把该出现的东西剥没。
 *
 * 行注释只在"行首或空白后"才算：这样 `https://x` 里的 `//` 不会被当成注释起点。
 */
const codeOnly = (src: string) =>
  src
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(?:^|(?<=\s))\/\/[^\n]*/gm, "");

/** 四个窗口：HTML 文件 ↔ 入口模块 ↔ 自己的骨架 id ↔ `<html>` 上的骨架类。 */
const WINDOWS = [
  { html: "index.html", entry: "src/entries/main.ts", skeleton: "boot", htmlClass: null },
  {
    html: "settings.html",
    entry: "src/entries/settings.ts",
    skeleton: "boot-settings",
    htmlClass: "boot-settings",
  },
  { html: "logs.html", entry: "src/entries/logs.ts", skeleton: "boot-logs", htmlClass: "boot-logs" },
  {
    html: "todos.html",
    entry: "src/entries/todos.ts",
    skeleton: "boot-todos",
    htmlClass: "boot-todos",
  },
  {
    html: "preview.html",
    entry: "src/entries/preview.ts",
    skeleton: "boot-preview",
    htmlClass: "boot-preview",
  },
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
  // 通用红线：**任何**辅助窗口都不得注册聊天事件（第二次 bindEvents/init 会重复通知、
  // 重复计未读、重复发群已读回执 —— 见 src/App.vue 顶部的说明）。
  for (const entry of [
    "src/entries/settings.ts",
    "src/entries/logs.ts",
    "src/entries/todos.ts",
    "src/entries/preview.ts",
  ]) {
    const code = codeOnly(read(entry));
    for (const bad of ["chat.init(", "bindEvents("]) {
      assert.ok(!code.includes(bad), `${entry} 不得调用 ${bad}（独立窗口注册第二套事件监听会重复通知/未读/回执）`);
    }
  }
  // 聊天组件树：设置/日志窗口完全不该碰；群任务窗口**需要**聊天数据层（它要折叠任务、
  // 建/改任务），但仍不得挂聊天组件树（ResponsiveLayout/ChatWindow/ConversationList）。
  const commonForbidden = ["ResponsiveLayout", "App.vue", "ChatWindow", "ConversationList"];
  const perEntry: Record<string, string[]> = {
    "src/entries/settings.ts": ["useChatStore"],
    "src/entries/logs.ts": ["useChatStore"],
    "src/entries/todos.ts": [],
    // 预览窗口只按 msgId/cid 自己取字节，连聊天 store 都不需要
    "src/entries/preview.ts": ["useChatStore"],
  };
  for (const [entry, extra] of Object.entries(perEntry)) {
    const code = codeOnly(read(entry));
    for (const bad of [...commonForbidden, ...extra]) {
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
  // 群任务窗口入口确实用了共享辅助窗口挂载路径（防"入口被清空也通过"）
  assert.match(read("src/entries/todos.ts"), /mountAuxWindow\(/, "群任务窗口入口要走 mountAuxWindow");
});

test("常驻的设置窗口必须在重新获得焦点时刷新环境数据（否则关了再开会看到旧快照）", () => {
  // 独立设置窗口现在是常驻的（关闭 = 隐藏，不重新加载），所以"只加载一次"就会把
  // 网卡/IP、共享目录、在线状态停在旧值上：用户切了 Wi-Fi 再打开设置，看到的还是上次的。
  const entry = read("src/entries/settings.ts");
  assert.match(entry, /onFocusChanged/, "设置窗口要监听重新获得焦点");
  assert.match(entry, /refreshEnvironment\(\)/, "获得焦点时刷新环境数据");

  const store = read("src/stores/useAppStore.ts");
  assert.match(store, /async function refreshEnvironment\(/, "store 要有 refreshEnvironment");
  assert.match(store, /^\s+refreshEnvironment,\s*$/m, "refreshEnvironment 必须从 store 导出");
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

test("每个窗口都必须加载应用样式（style.css 只能挂在共用的 boot 模块上）", () => {
  // 真实缺陷：一窗一入口重构时漏掉了 `import "./style.css"`，
  // 结果 dev 起来"整个界面像没有 CSS" —— 不报错、不影响任何测试，只能靠这条守卫。
  const boot = read("src/boot/boot.ts");
  assert.match(
    boot,
    /import "@\/style\.css";/,
    "共用的 boot 模块必须 import 应用样式（三个窗口都经过它 ⇒ 一处 import 全窗口生效）",
  );
  for (const w of WINDOWS) {
    const entry = read(w.entry);
    assert.ok(
      !entry.includes("style.css"),
      `${w.entry} 不要单独 import 样式：挂在 boot 模块上才能保证三个窗口一致，避免"某个窗口忘了带"`,
    );
  }
});

test("每个窗口都必须有 toast 宿主（否则该窗口里的操作是静默的）", () => {
  // 真实缺陷（用户 2026-09-21：「重置里面的按钮点了没反应」）：`ToastHud` 是应用内唯一的
  // 轻量反馈层，成功/失败都靠它 —— 但设置窗口没挂它。于是「恢复默认 / 清除聊天数据 / 导出」
  // 弹窗一关就再没有任何可见反馈：成功了看不出来、失败了也看不出来，和"按钮坏了"无法区分。
  // 这类退化不报错、不影响任何单测（HUD 只是少了一块 DOM），只能静态钉住。
  const shell = read("src/components/window/AuxWindowShell.vue");
  assert.match(shell, /<ToastHud\s*\/>/, "AuxWindowShell 必须挂 ToastHud（套壳的窗口共享这一份）");
  assert.match(
    read("src/layouts/ResponsiveLayout.vue"),
    /<ToastHud\s*\/>/,
    "主窗口的布局必须挂 ToastHud",
  );

  /** 入口 → 它挂载的根组件（这些根组件若是套 `AuxWindowShell`，就有了 toast 宿主）。 */
  const AUX: Record<string, string[]> = {
    "src/entries/settings.ts": ["src/components/SettingsWindow.vue"],
    "src/entries/todos.ts": ["src/components/GroupTodosWindow.vue"],
    // 日志窗口不套壳（同一组件要兼移动端整页形态）⇒ 必须由入口自己挂 ToastHud
    "src/entries/logs.ts": [],
  };
  for (const [entry, roots] of Object.entries(AUX)) {
    const selfHosted = read(entry).includes("ToastHud");
    const viaShell = roots.some((f) => read(f).includes("AuxWindowShell"));
    assert.ok(
      selfHosted || viaShell,
      `${entry} 既没自己挂 ToastHud，也没套 AuxWindowShell ⇒ 该窗口里所有操作的反馈都是静默的`,
    );
    // 套壳的窗口不要再自己挂一份：两个 HUD 会同时显示同一条 toast
    for (const f of roots) {
      assert.ok(
        !/<ToastHud\s*\/>/.test(read(f)),
        `${f} 不该自己挂 ToastHud：它套了 AuxWindowShell，那里已经有一份`,
      );
    }
  }
});

test("骨架主色必须跟随主题色（写死默认蓝 = 换了主题色还是闪一下蓝）", () => {
  // 真实缺陷（用户 2026-09-21）：「骨架屏没适配主题色，现在永远都是蓝色的」。
  // theme-boot.js 早就把用户主题色写成 `<html>` 上的内联 `--gosslan-primary`（先于骨架 CSS），
  // 骨架只要**引用**它即可；写死色值就会在用户换过主题色后启动瞬间先闪一下蓝。
  const css = read("src/boot/skeleton.css");
  assert.match(
    css,
    /--boot-primary:\s*var\(--gosslan-primary,\s*#[0-9a-fA-F]{3,8}\)/,
    "骨架的 --boot-primary 必须写成 var(--gosslan-primary, <兜底色>)，不能写死某个颜色",
  );
  // 反向：引用的来源必须真实存在（theme-boot 必须把主题色落到 --gosslan-primary 上），
  // 否则这条守卫可能只是"两边都空着"而通过。
  assert.match(
    read("src/boot/theme-boot.js"),
    /setProperty\("--gosslan-primary"/,
    "theme-boot.js 必须把主题色写到 --gosslan-primary —— 骨架跟随主题色靠的就是它",
  );
});
