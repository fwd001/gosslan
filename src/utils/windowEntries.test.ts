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

/**
 * 骨架**撤除**必须覆盖每一个窗口。
 *
 * 真踩过的形状（用户 2026-09-24 报"预览窗口只有骨架、没有内容"）：`dismissBoot` 原先写死一份
 * id 清单 `["boot","boot-logs","boot-settings","boot-todos"]`，加第 5 个窗口时没人回去改它
 * ⇒ `boot-preview` 永远撤不掉。那块骨架是 `position:fixed; inset:0; z-index:9999` + 不透明底色，
 * 于是 Vue 挂载成功、图片也取到了，**整页被一张看不见的骨架盖着**。上面那条"每个窗口只带自己
 * 的骨架"查的是 HTML 侧，看不见这件事 —— 它当时是绿的（假绿）。
 *
 * 判据刻意不做"文本里有没有某个 id"那种比对（把选择器改成 `#boot, #boot-settings` 一样能骗过，
 * 而 preview 的骨架又没人撤了）。现在两边共用一个**骨架元素自带的类**：HTML 里那一行写
 * `class="boot-skeleton"`，撤除方按类查 —— 新增窗口时"写骨架"和"打标"是同一次编辑，漏不掉。
 * 两头各钉一次，缺一头即红。
 */
test("骨架撤除必须覆盖每个窗口（靠元素自带的类，不靠别处的清单）", () => {  for (const w of WINDOWS) {
    const html = read(w.html);
    // 属性顺序不敏感：id 在前在后都算，只要那个骨架根确实带着类。
    const tagged = new RegExp(
      `<div [^>]*id="${w.skeleton}"[^>]*class="[^"]*\\bboot-skeleton\\b[^"]*"` +
        `|<div [^>]*class="[^"]*\\bboot-skeleton\\b[^"]*"[^>]*id="${w.skeleton}"`,
    ).test(html);
    assert.ok(
      tagged,
      `${w.html} 的骨架根 #${w.skeleton} 必须带 class="boot-skeleton" —— ` +
        `否则 dismissBoot 撤不掉它，这个窗口会永远停在骨架屏（内容全被那张 fixed 遮罩盖住）`,
    );
  }
  const boot = codeOnly(read("src/boot/boot.ts"));
  assert.ok(
    boot.includes('querySelectorAll<HTMLElement>(".boot-skeleton")'),
    'dismissBoot 必须按 `.boot-skeleton` 类撤除：回到逐个 id 的清单就一定会漏掉新增的那个窗口',
  );
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
  // 聊天组件树 / 聊天 store：设置、日志、预览窗口完全不该碰。群任务窗口**确实需要**聊天
  // 数据层（折叠任务、建/改任务），但那份需求已经从入口搬到根组件 `GroupTodosWindow.vue`
  // 了（取数搬走是"点了没反应"那次修复的一部分）⇒ 入口连 `useChatStore` 都不该出现：
  // 留在入口里的唯一用法就是 init/取数，两条都是红线。
  const commonForbidden = ["ResponsiveLayout", "App.vue", "ChatWindow", "ConversationList"];
  const perEntry: Record<string, string[]> = {
    "src/entries/settings.ts": ["useChatStore"],
    "src/entries/logs.ts": ["useChatStore"],
    "src/entries/todos.ts": ["useChatStore"],
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

/**
 * 群任务窗口的"挂载前那道门"里只准有 `app.init()`。
 *
 * 为什么钉这一条（用户 2026-09-24：「点群里的查看任务，弹窗弹出很慢，我还以为没点了，
 * 点了好几下一会才弹出来」）：入口原先在 `beforeMount` 里串了 `loadGroupTodos`
 * （四次刷新 + 一次消息拉取）和 `watchGroupTodos`，而骨架屏是**挂载之后**才撤的 ⇒
 * 整段时间窗口里什么都没有，只剩一块灰底；再叠加启动器的单飞/防抖把连点吃掉，
 * 就是"点了没反应"。改成先挂载、数据后台补之后，这个形状很容易被人"顺手改回去"
 * （看起来像是"更严谨的初始化顺序"），所以钉在这里。
 */
test("群任务窗口的挂载前门里只准有 app.init()，取数在根组件里", () => {
  const entry = codeOnly(read("src/entries/todos.ts"));
  const mountAt = entry.indexOf("mountAuxWindow(");
  assert.ok(mountAt >= 0, "找不到挂载调用，这条判据会空转");
  // 入口里出现任何"读数据/订阅"的调用都意味着又把网络往返塞回了挂载前那道门
  // （用户 2026-09-24：「点查看任务，弹窗弹出很慢，我还以为没点了」就是这个形状）。
  for (const forbidden of ["loadGroupTodos", "watchGroupTodos", "getGroupTodosContext"]) {
    assert.ok(!entry.includes(forbidden), `入口不得出现 ${forbidden}：那等于把开窗重新堵在数据后面（用户报的「点了没反应」）`);
  }
  assert.ok(
    entry.includes("useAppStore().init()"),
    "入口的门里只留 app.init()（外观必须先于渲染）",
  );
  // 取数没有消失，只是搬到根组件 —— 那边才是"当前该显示哪个群"的唯一拥有者。
  const win = read("src/components/GroupTodosWindow.vue");
  for (const call of ["loadGroupTodos(next)", "watchGroupTodos(next)", "getGroupTodosContext()"]) {
    assert.ok(win.includes(call), `根组件必须仍然调用 ${call}，否则窗口会永远停在「正在加载」`);
  }
});

test("设置窗口的环境数据重拉入口必须存在（注意：它今天**不是**常驻窗口，见下面那条常驻判据）", () => {
  // ⚠️ 这条原来叫「常驻的设置窗口必须…」，而 `AUX_WINDOWS_RESIDENT` 早就改成 `false`
  //（关闭即销毁，每次打开都是新数据）⇒ 标题与理由都在说一件已经不成立的事。
  // 判据本身继续留着（那个函数与它的四个只读拉取仍有用户价值），但"常驻窗口必须有重拉兜底"
  // 这件事交给下面那条**从 Rust 现场读标记**的守卫 —— 点名式判据会因为翻一个常量而静默失效。
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

/**
 * 常驻窗口的自愈兜底（架构复审第 5 步 · 判据）。
 *
 * 背景：关窗 = 隐藏 ⇒ 文档永远不重新加载，任何"只在挂载时取一次"的写法都会永久显示旧快照。
 * 这条规则以前只被**一个具体窗口**的守卫钉着（上面那条 `settings.ts` 的 `onFocusChanged`），
 * 而那是点名式判据：换一扇窗口、换一个函数名，它永远绿。更糟的是它的前提已经过期 ——
 * `AUX_WINDOWS_RESIDENT = false`（设置/日志关闭即销毁），被钉的那扇窗根本不再常驻。
 *
 * 所以这里把两件事分开钉：
 *  ① **常驻集合从 Rust 现场读**（`AUX_*_RESIDENT`）并强制登记 —— 谁翻了标记、谁新增一扇
 *     常驻窗口而没来这里表态，直接红；
 *  ② 每扇常驻窗口必须有兜底，但**允许两种合法形状**：自己取数的必须有"重新可见 ⇒ 重拉"；
 *     内容由后端定向推送的必须有那个推送监听（并且关闭时自己释放）。
 *     不这么分就会逼着预览窗口去重拉一份"它本来就没有的列表"。
 */
test("常驻窗口必须有自愈兜底（常驻集合从 Rust 的 *_RESIDENT 现场读，不许点名）", () => {
  const logs = read("src-tauri/src/commands/logs.rs");
  const flags = new Map<string, boolean>();
  for (const m of logs.matchAll(/const AUX_(\w+)_RESIDENT: bool = (true|false);/g)) {
    flags.set(m[1], m[2] === "true");
  }
  assert.ok(
    flags.size >= 3,
    `只从 logs.rs 读出 ${flags.size} 个 *_RESIDENT 标记 ⇒ 判据的覆盖范围正在失效，先修正则再谈别的`,
  );

  /**
   * 每扇辅助窗口的归属。`selfFetch` = 这个窗口的内容**是它自己向本地后端取**的
   * （那种窗口漏了重拉就会永久显示旧数据）；否则它的内容由后端定向事件带过来。
   */
  const WINDOWS: {
    flag: string;
    label: string;
    selfFetch: boolean;
    files: string[];
    /** 被推送形状所必需的监听（缺了就等于"隐藏后既不自取也没人推"）。 */
    pushListener?: RegExp;
  }[] = [
    {
      flag: "TASKS",
      label: "群任务窗口",
      selfFetch: true,
      // 取数与换群都在根组件里（入口刻意不取数，见 entries/todos.ts 顶部说明）
      files: ["src/components/GroupTodosWindow.vue", "src/entries/todos.ts"],
    },
    {
      flag: "PREVIEW",
      label: "图片预览窗口",
      selfFetch: false,
      files: ["src/components/window/PreviewWindow.vue", "src/entries/preview.ts"],
      pushListener: /onImagePreviewChanged/,
    },
    { flag: "LINK", label: "外链窗口", selfFetch: false, files: [] },
    {
      flag: "WINDOWS",
      label: "设置 / 日志窗口",
      selfFetch: false,
      files: ["src/entries/settings.ts"],
    },
  ];
  for (const [flag, resident] of flags) {
    const w = WINDOWS.find((x) => x.flag === flag);
    assert.ok(w, `AUX_${flag}_RESIDENT 没在判据表里登记 ⇒ 新增窗口必须先来这里表态（自愈/不自愈都行，别默认）`);
    if (!resident) continue;
    if (w!.selfFetch) {
      const src = w!.files.map(read).join("\n");
      assert.match(
        src,
        /onFocusChanged|visibilitychange/,
        `${w!.label} 是常驻窗口且内容自己取 ⇒ 必须有"重新可见/获得焦点"的兜底；今天只有 Rust 复用它时发的定向事件，从任务栏或 ⌘Tab 唤回来仍是旧快照`,
      );
      assert.match(
        src,
        /loadGroupTodos\(|refresh[A-Z]\w*\(/,
        `${w!.label} 的可见性兜底必须真的重拉数据，不能只改样式`,
      );
    } else if (w!.pushListener) {
      const src = w!.files.map(read).join("\n");
      assert.match(
        src,
        w!.pushListener,
        `${w!.label} 是常驻窗口：内容不靠自己取，就必须有后端定向事件把它推醒`,
      );
    }
  }

  // 主窗口不在 AUX_* 之下：它的"关窗即隐藏"由托盘那条路径实现，所以事实要单独读
  const tray = read("src-tauri/src/tray.rs");
  const mainResident = /prevent_close\(\)/.test(tray) && /hide\(\)/.test(tray);
  assert.ok(mainResident, "托盘的「关窗即隐藏」判据失效了（先确认实现搬去哪了，别把这条守卫删掉）");
  const chat = read("src/stores/useChatStore.ts");
  const vis = chat.slice(chat.indexOf("const onVisibility"));
  assert.ok(vis.length > 80, "找不到主窗口的可见性兜底函数");
  assert.match(
    vis,
    /refreshConversations\(\)[\s\S]{0,400}refreshTransfers\(\)|refreshTransfers\(\)[\s\S]{0,400}refreshConversations\(\)/,
    "主窗口重新可见时必须重拉会话与传输：`message-acked` / `peer-read` / `file-*` 都是就地改内存的轻量事件，错过一条就永久错，而这两张表是它们的真相源",
  );
});
