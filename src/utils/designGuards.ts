/**
 * 设计规范里"靠自觉维持"的条款，做成可执行断言。
 *
 * 前两个模块（`tokenContrast` 读 style.css 核对比度、`templateBranches` 扫模板分支链）
 * 已经把"最容易悄悄漂移"的两类规则变成测试。本模块补的是本轮新确立、
 * 而**当时只写在注释与文档里**的两条 —— 它们各自都已经真实踩过一次：
 *
 *   ① 悬停揭示必须有触屏兜底（`group-hover` / `opacity-0` → `.hover-reveal` / `.hover-reveal-op`）
 *      踩坑：会话行的删除键只有 `group-hover:flex`，Android 上永远不显示 = 功能缺失。
 *   ② 辅助功能媒体查询必须排在 `style.css` 末尾
 *      踩坑：`.glass` / `.frost` 的定义在文件更靠后处，同优先级下"后定义者胜"，
 *      降级规则写在前面被**完整覆盖**（写了日志、加了注释，但实测才发现无效）。
 *
 *   ⑨ Headless UI 的 `as="template"` 插槽里不得有 HTML 注释/多个顶层节点
 *      踩坑（用户实测"点加好友整个窗口卡死"的真因）：dev 构建**保留 HTML 注释**，
 *      于是 `as="template"` 的插槽里多出一个注释节点，Headless UI 的 render 直接抛
 *      "Passing props on template!"，Vue 渲染抛错 ⇒ 整个界面再也 patch 不动（看起来就是卡死）。
 *      生产构建会剥掉注释，所以这类缺陷**只在 dev 出现**，最容易把人带偏。
 *
 *   ⑧ 小尺寸可交互元素必须有 `tap-safe`（触屏点按目标 ≥44pt，HIG）
 *      踩坑：iOS HIG 的最小点按目标是 44×44pt，而项目里大量图标按钮是 28–32px
 *      （桌面鼠标没问题，**手指容易点不中甚至误触相邻项**）。项目已有 `.tap-safe`
 *      （`:pointer: coarse` 下把热区垂直撑到 +16px），但靠自觉使用，本次复核仍扫出 8 处漏网
 *      —— 其中移动端返回键、删除端点键、取色控件都在手机上真会被点到。
 *
 *   ⑦ `outline-none` 必须自带焦点指示（否则全局焦点环被它"静默盖掉"）
 *      踩坑：全局焦点环写在 `:where(button, a, input, …, [tabindex], [contenteditable]):focus-visible`
 *      —— **`:where()` 的特异性是 0**，而 Tailwind 的 `.outline-none`（`outline: 2px solid
 *      transparent`）是 0,1,0 ⇒ 只要元素带 `outline-none`，那条"让键盘用户看得见焦点"的
 *      规则就**一条都不生效**。7 处输入框（含最高频的输入框）因此完全没有焦点指示。
 *      这正是"写对了但不起作用"，与本模块其它条同源。
 *
 *   ⑥ 可点击元素必须能用键盘触发（`div` + `@click` 是"只有鼠标/手指能用"的按钮）
 *      踩坑：好友选择行、图片/文件气泡、指纹复制等处都是 `div @click` ——
 *      触屏能用、鼠标能用，但**键盘完全够不着**（Tab 跳不到、回车没反应）。
 *      在"为 iOS 上架铺路 + 去网页感"的目标下，这类元素还缺 `role`，
 *      读屏软件也只会念成一段普通文本。
 *
 *   ③ 未读徽标必须走唯一实现（`UnreadBadge.vue`）
 *      踩坑：徽标数字需要 1.5px 光学补偿才能垂直居中，而 5 处手写副本里有 2 处漏了
 *      配套的 `leading-none` —— 同一种徽标在不同位置基线不一致，肉眼可见「没居中」，
 *      读代码看不出来。
 *
 * 三者都是"写对了但不起作用"，读代码几乎看不出来 —— 正是最该由机器盯住的那类。
 */

export interface GuardIssue {
  /** 源文件行号（1 基）；0 表示与具体行无关（整文件级问题） */
  line: number;
  message: string;
}

function countNewlines(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) if (s.charCodeAt(i) === 10) n++;
  return n;
}

/** 一行里 `<template>` 之前的行数，用于把行号换算回源文件。 */
function lineAt(src: string, index: number): number {
  return countNewlines(src.slice(0, index)) + 1;
}

// ---------------- ① 悬停揭示必须有触屏兜底 ----------------

/** 展示类工具（揭示 = 让它显示出来）。 */
const DISPLAY_UTILS = ["flex", "block", "grid", "inline-flex", "inline-block", "table"];

/** 静态 class 属性；`class="…"`。动态 `:class` 里的字符串拼接不做静态判定（无法可靠推断）。 */
const CLASS_ATTR_RE = /\bclass="([^"]*)"/g;

/**
 * 检查一份 .vue 源码里"靠悬停揭示、却没有触屏兜底"的元素。
 *
 * 判据（刻意收紧，只看能确定的两种揭示方式）：
 *   —— class 里同时有 `hidden` 与 `group-hover…:<展示类>` → 需要 `.hover-reveal`
 *      （这正是会话删除键的原始写法：`hidden … group-hover/conv:flex`）
 *   —— class 里同时有 `opacity-0` 与 `group-hover…:opacity-*` → 需要 `.hover-reveal-op`
 * 逃生阀：文件里带 `hover-reveal-ok` 注释则整文件跳过。
 */
export function findHoverRevealIssues(src: string): GuardIssue[] {
  if (src.includes("hover-reveal-ok")) return [];
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    const classes = m[1].split(/\s+/).filter(Boolean);
    const has = (c: string) => classes.includes(c);
    // Tailwind 的 group 变体：`group-hover:flex` 或带命名的 `group-hover/conv:flex`
    const groupHoverUtils = classes
      .filter((c) => c.startsWith("group-hover"))
      .map((c) => c.slice(c.indexOf(":") + 1));

    const displayReveal = has("hidden") && groupHoverUtils.some((u) => DISPLAY_UTILS.includes(u));
    const opacityReveal =
      has("opacity-0") && groupHoverUtils.some((u) => u.startsWith("opacity-"));

    if (displayReveal && !has("hover-reveal")) {
      out.push({
        line: lineAt(src, m.index ?? 0),
        message:
          "元素用 `hidden` + `group-hover…:<展示类>` 揭示，但缺少 `hover-reveal` —— " +
          "触屏没有 hover，这个操作在手机上**永远不显示**（等于功能缺失）。加 `hover-reveal`。",
      });
    }
    if (opacityReveal && !has("hover-reveal-op")) {
      out.push({
        line: lineAt(src, m.index ?? 0),
        message:
          "元素用 `opacity-0` + `group-hover…:opacity-*` 揭示，但缺少 `hover-reveal-op` —— " +
          "触屏上看不见（虽然可点，但用户只能靠猜）。加 `hover-reveal-op`。",
      });
    }
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ③ 未读徽标必须走唯一实现 ----------------

/**
 * 检查一份 .vue 源码里"手写的未读徽标"。
 *
 * 判据：同一个静态 `class` 里同时出现 `min-w-4` 与 `gosslan-danger` —— 这个组合是
 * 未读徽标独有的（红底胶囊 + 最小宽度 16px），不会误伤其他元素。
 *
 * 为什么必须走唯一实现：徽标数字需要 **1.5px 光学补偿**才能在圆内垂直居中
 * （字形在行盒里天然偏下：实测上间隙 10 / 下间隙 7，2x 截图）。此前有 5 处手写副本，
 * 其中 2 处漏了配套的 `leading-none` —— 同一种徽标在不同位置的基线不一致，
 * 肉眼能看出「数字没居中」，但读代码几乎看不出来。用组件收敛后由本护栏守住。
 */
export function findHandWrittenBadges(src: string): GuardIssue[] {
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    const classes = m[1].split(/\s+/).filter(Boolean);
    if (classes.includes("min-w-4") && classes.some((c) => c.includes("gosslan-danger"))) {
      out.push({
        line: lineAt(src, m.index ?? 0),
        message:
          "手写的未读徽标 —— 请改用 `@/components/UnreadBadge.vue`。" +
          "徽标数字依赖 1.5px 光学补偿做到垂直居中，手写副本极易漏掉配套的 `leading-none`，" +
          "导致同一徽标在不同位置基线不一致（详见组件注释）。",
      });
    }
  }
  return out.sort((a, b) => a.line - b.line);
}

/**
 * `UnreadBadge.vue` 自身必须保留「垂直居中补偿」与其前提 `leading-none`。
 *
 * 这两条是**一对**：补偿量 `pb-[1.5px]` 是按「行盒高 = font-size」推算的
 * （`(16 − 1.5 − 11) / 2 = 1.75`，未补偿时 2.5 → 上移 0.75px），
 * 单独删掉任何一个都会让数字重新偏下。数值本身是实测结果，不是随手写的边距。
 */
export function checkUnreadBadgeComponent(src: string): GuardIssue[] {
  // ⚠️ 必须只看**真实的 class 属性**，不能 `src.includes(...)` 扫全文：
  // 组件里的注释本来就会提到这些类名，扫全文会让护栏「因为注释而通过」——
  // 这正是本模块要防的假通过（首版就是这么写的，靠非空转验证才抓出来）。
  let badgeLine = 0;
  let classes: string[] = [];
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    const cs = m[1].split(/\s+/).filter(Boolean);
    if (cs.includes("min-w-4")) {
      badgeLine = lineAt(src, m.index ?? 0);
      classes = cs;
      break;
    }
  }
  if (classes.length === 0) {
    return [{ line: 0, message: "`UnreadBadge.vue` 里找不到徽标的 class 属性（含 `min-w-4` 的那一行）" }];
  }

  const out: GuardIssue[] = [];
  if (!classes.includes("pb-[1.5px]")) {
    out.push({
      line: badgeLine,
      message:
        "`UnreadBadge.vue` 缺少 `pb-[1.5px]` 垂直居中补偿 —— 数字会偏下（实测上间隙 10 / 下间隙 7）。",
    });
  }
  if (!classes.includes("leading-none")) {
    out.push({
      line: badgeLine,
      message:
        "`UnreadBadge.vue` 缺少 `leading-none` —— 补偿值按「行盒高 = font-size」推算，" +
        "行高变 normal（≈13.2px）后补偿不再成立。",
    });
  }
  return out;
}

// ---------------- ④ 被截断的文本必须有可访问名（title / aria-label） ----------------

/** 从 `class="…"` 的匹配位置取回**整个开标签**。 */
function enclosingTag(src: string, classIndex: number): string {
  let start = -1;
  for (let i = classIndex; i >= 0; i--) {
    if (src[i] === "<") {
      start = i;
      break;
    }
  }
  if (start < 0) return "";
  const end = src.indexOf(">", classIndex);
  return end < 0 ? src.slice(start) : src.slice(start, end + 1);
}

/**
 * 检查一份 .vue 源码里"会被 `truncate` 截断、却没有 hover title / aria-label"的元素。
 *
 * 为什么要机器盯：`truncate`（`overflow:hidden` + `text-overflow:ellipsis`）在**视觉上**
 * 把名字/地址/文件名截成「…」，而完整内容只存在于 DOM 里 —— 鼠标悬停拿不到、读屏
 * 也可能拿不到。这类缺陷**编译通过、测试全绿、代码看着也正常**，只有真去 hover 才发现。
 *
 * 判据（收紧到能确定的形态）：
 *   —— 同一个开标签里有 `truncate` 类，且**没有** `title` / `:title` / `aria-label` /
 *      `:aria-label`（任一即可）→ 报出。
 *   —— 逃生阀：文件里带 `truncate-title-ok` 注释则整文件跳过（例如父级已有整行
 *      `aria-label`、且文本短到不可能截断的场合）。
 *
 * ⚠️ 与 ③ 同样的教训：只看**真实 class 属性**，不能用 `src.includes("truncate")` 扫全文 ——
 * 注释里提到 `truncate` 会让护栏「因为注释而通过」。
 */
/**
 * 默认头像的字母必须对读屏隐藏（`aria-hidden="true"`）。
 *
 * 踩坑（2026-09-26 用系统无障碍控件树实测）：字母头像那一段文字（`KEEN` / `BRIG` / `HAPP`）
 * 会作为 `AXStaticText` 出现在树里 ⇒ 读屏把**无意义的截断字母**念在人名前面。
 * 同一位置的上传头像早就是 `alt=""`（装饰性、名字由旁边的文本承载），字母这一支却没人管。
 * 这条判据把两者拉回同一个口径，也防止新站点漏写。
 */
export function findAvatarInitialWithoutAriaHidden(src: string): GuardIssue[] {
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    if (!m[1].split(/\s+/).includes("gosslan-avatar-initial")) continue;
    const line = lineAt(src, m.index ?? 0);
    const open = src.lastIndexOf("<", m.index ?? 0);
    const close = src.indexOf(">", m.index ?? 0);
    const tag = open >= 0 && close > open ? src.slice(open, close + 1) : "";
    if (!tag.includes("aria-hidden")) {
      out.push({
        line,
        message:
          "字母头像没有 `aria-hidden` —— 读屏会把截断字母（如 `KEEN`）当正文念出来。" +
          "装饰性头像一律对辅助技术隐藏，人名由旁边的文本承载（上传头像那支已是 `alt=\"\"`）。",
      });
    }
  }
  return out.sort((a, b) => a.line - b.line);
}

export function findTruncationWithoutTitle(src: string): GuardIssue[] {
  if (src.includes("truncate-title-ok")) return [];
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    const classes = m[1].split(/\s+/).filter(Boolean);
    if (!classes.includes("truncate")) continue;
    // 全角/半角都要认：`:title` 是 v-bind 简写，`v-bind:title` 是完整写法。
    const tag = enclosingTag(src, m.index ?? 0);
    const named =
      /(?:^|\s)(?::|v-bind:)title\s*=/.test(tag) ||
      /(?:^|\s)title\s*=/.test(tag) ||
      /(?:^|\s)(?::|v-bind:)aria-label\s*=/.test(tag) ||
      /(?:^|\s)aria-label\s*=/.test(tag);
    if (!named) {
      out.push({
        line: lineAt(src, m.index ?? 0),
        message:
          "被 `truncate` 截断的文本没有 `title` / `aria-label` —— 截断后完整内容只能靠悬停或读屏获取，" +
          "两者都拿不到就等于用户永远看不到全名。加 `:title=\"…\"`（纯图标元素用 `aria-label` 也可）。" +
          "确属不会截断的短文案，可在文件内加 `truncate-title-ok` 注释整文件跳过。",
      });
    }
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ⑥ 可点击元素必须能用键盘触发 ----------------

/**
 * 非交互标签（这些标签天生没有"可点击"语义，只能靠 `role` / `tabindex` / 键盘事件补）。
 * `button` / `a` / `input` / `label` / `summary` 等有原生语义，不在扫描范围。
 */
const NON_INTERACTIVE_TAGS = "div|span|li|tr|td|section|article|header|footer|p|img|main|aside|figure";

/** 元素开标签（跨行；到第一个 `>` 为止）。 */
const OPEN_TAG_RE = new RegExp(`<(${NON_INTERACTIVE_TAGS})\\b[^>]*>`, "gs");

/**
 * 从开标签里取"真实动作型"的 `@click` / `v-on:click`。
 *
 * 两种**不算动作**的写法（都是修护栏时才发现的真实存在）：
 *   —— 只有修饰符、没有表达式：`@click.stop`（这是"阻止冒泡"，不是按钮）；
 *   —— 元素本身 `aria-hidden="true"`：整块对读屏隐藏（例如遮罩层点外部关闭），
 *      给它加 `tabindex` 反而会做出"读屏看不见、键盘却能聚焦"的怪东西。
 */
function clickAction(tag: string): boolean {
  if (/aria-hidden\s*=\s*"true"/.test(tag)) return false;
  for (const m of tag.matchAll(/(?:@|v-on:)click(?:\.\w+)*\s*=\s*"([^"]*)"/g)) {
    if (m[1].trim().length > 0) return true;
  }
  return false;
}

/**
 * 检查一份 .vue 源码里"能点、但键盘够不着"的元素。
 *
 * 判据（能确定的才报）：
 *   —— 非交互标签 + 有真实动作的 `@click`；
 *   —— 同一开标签里既没有 `role`（含 `:role`），也没有 `tabindex`（含 `:tabindex`），
 *      也没有 `@keydown` / `@keyup`（含 `v-on:` 与 `:keydown` 简写）。
 * 逃生阀：文件里带 `tap-keyboard-ok` 注释则整文件跳过。
 */
export function findTappableWithoutKeyboard(src: string): GuardIssue[] {
  if (src.includes("tap-keyboard-ok")) return [];
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(OPEN_TAG_RE)) {
    const tag = m[0];
    if (!clickAction(tag)) continue;
    const hasRole = /(?:^|\s)(?::|v-bind:)?role\s*=/.test(tag);
    const hasTabindex = /(?:^|\s)(?::|v-bind:)?tabindex\s*=/.test(tag);
    const hasKey = /(?:@|v-on:|:)key(?:down|up)/.test(tag);
    if (hasRole || hasTabindex || hasKey) continue;
    out.push({
      line: lineAt(src, m.index ?? 0),
      message:
        "`" +
        m[1] +
        "` 上有 @click，但它没有原生按钮语义 —— 键盘用户 Tab 不到、按回车也没反应" +
        "（读屏只会念成普通文本）。请改用 <button type=\"button\">，或补上 " +
        "`role=\"button\"` + `tabindex=\"0\"` + `@keydown.enter`（空格键用 `@keydown.space.prevent`）。" +
        "确实不该聚焦的元素（遮罩层等），加 `aria-hidden=\"true\"` 或用 `@click.stop` 表明它只是拦事件；" +
        "整文件例外可加 `tap-keyboard-ok` 注释。",
    });
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ⑦ `outline-none` 必须自带焦点指示 ----------------

/**
 * 可见的焦点指示替代写法（Tailwind）。
 *
 * ⚠️ 为什么必须显式要求：全局焦点环用的是
 * `:where(button, a, input, textarea, select, [tabindex], [contenteditable]):focus-visible`
 * —— `:where()` 让整条选择器的特异性变成 **0**，而 `.outline-none`
 * （`outline: 2px solid transparent; outline-offset: 2px`）是 0,1,0。
 * 于是**只要元素带 `outline-none`，全局焦点环一定被它覆盖**（透明 2px 边框 = 看不见）。
 */
const FOCUS_INDICATOR_RE = /(?:focus|focus-visible):(?:ring|border|outline|bg|shadow)[-\w[]/;

/**
 * 检查一份 .vue 源码里"关掉了轮廓、却没给替代焦点指示"的元素。
 *
 * 判据：开标签里有 `outline-none`，且同一标签里既没有 `focus:ring/border/outline/bg/shadow`
 * 也没有 `focus-visible:` 同款 → 报出。
 *
 * ## 两个逃生阀（粒度不同，按需选）
 *
 * ① **元素级**（推荐）：给该元素加 `data-focus-ring-ok` 属性 ⇒ **只豁免这一个元素**，
 *    文件里其它元素照旧受保护。
 * ② **文件级**：文件里带 `focus-ring-ok:file` 注释 ⇒ 整文件跳过。只适用于"整个组件只有
 *    一个不该画环的容器"的场合（菜单容器 `role="menu" tabindex="-1"`、全屏对话框容器
 *    `role="dialog" tabindex="-1"` —— 它们的焦点由内部条目承担）。
 *
 * ⚠️ **文件级令牌为什么带 `:file` 后缀**：元素级的 `data-focus-ring-ok` **含有**
 * `focus-ring-ok` 这个子串，所以文件级判据若还写成 `src.includes("focus-ring-ok")`，
 * 就会把"只豁免了一个元素"的文件当成"整文件豁免" ⇒ 元素级标记形同虚设、护栏静默失效。
 * 两个令牌必须是互不为子串的。
 *
 * ⚠️ **为什么必须有元素级**（2026-09-16）：`MessageComposer.vue` 因为"消息输入框不画焦点环"
 * 这个**元素级**需求，用了**文件级**逃生阀 ⇒ 整个文件（含其中的按钮等键盘可聚焦元素）
 * 一起失去本条保护，且 `scripts/verify-guards.py` 里注入该文件的用例退化成**空转**
 * （改坏也不报），而项目里没有任何东西自动跑 verify-guards，所以一直没人发现。
 * 粒度给错，护栏就会在你看不见的地方悄悄失效。
 */
export function findOutlineNoneWithoutFocusRing(src: string): GuardIssue[] {
  if (src.includes("focus-ring-ok:file")) return [];
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    const classes = m[1].split(/\s+/).filter(Boolean);
    if (!classes.includes("outline-none")) continue;
    const tag = enclosingTag(src, m.index ?? 0);
    if (FOCUS_INDICATOR_RE.test(tag)) continue;
    // 元素级逃生阀：只豁免**这一个**元素（见函数上方"两个逃生阀"）
    if (tag.includes("data-focus-ring-ok")) continue;
    out.push({
      line: lineAt(src, m.index ?? 0),
      message:
        "带 `outline-none` 却没有替代的焦点指示 —— 全局焦点环用的是 `:where(...)`（特异性 0），" +
        "会被 `.outline-none`（特异性 0,1,0）**静默覆盖**，键盘用户看不到焦点在哪（WCAG 2.4.7）。" +
        "两种改法：① 直接删掉 `outline-none`（让全局环生效，文本框类控件推荐这个）；" +
        "② 自己给一个可见指示，如 `focus:ring-2 focus:ring-primary` / `focus:border-[…]`。" +
        "确实不该画环时用逃生阀，**优先元素级**：给这个元素加 `data-focus-ring-ok`（只豁免它）；" +
        "只有整个组件都该跳过时才用文件级的 `focus-ring-ok:file` 注释 —— 文件级会让同文件里" +
        "其它键盘可聚焦元素一起失去保护。",
    });
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ⑧ 小尺寸可交互元素必须有 tap-safe ----------------

/**
 * 小尺寸工具类：Tailwind 里 `h-8` = 32px、`h-7` = 28px、`h-6` = 24px、`h-5` = 20px，
 * 全部**小于 HIG 的 44pt 最小点按目标**。（`h-11` = 44px 才是达标尺寸。）
 */
const SMALL_SIZE_RE = /\b(?:h-5|h-6|h-7|h-8|w-5|w-6|w-7|w-8)\b/;

/** 参与点按判定的标签（`button`/`a`/`label`/`select`/`input` 天生可交互；其余看 `@click`）。 */
const TAPPABLE_TAG_RE = /<(button|a|div|span|li|label|select|input)\b[^>]*>/gs;

/**
 * 检查一份 .vue 源码里"点按目标过小、又没有触屏撑大"的元素。
 *
 * 判据：可交互元素（原生可交互标签，或带 `@click`）+ class 里有小尺寸工具类
 * + 同一开标签里没有 `tap-safe` → 报出。
 *
 * ⚠️ 说清 `tap-safe` 的能力边界（写进提示里，免得以为加上就万无一失）：
 * 它只在 `@media (pointer: coarse)` 下把热区**垂直**撑 ±8px ⇒ `h-6`→40、`h-7`→44、`h-8`→48；
 * 因此 `h-5`（20px→36px）仍然不达标，必须改成更大的尺寸。
 * 逃生阀：文件里带 `tap-target-ok` 注释则整文件跳过。
 */
export function findSmallTapTargets(src: string): GuardIssue[] {
  if (src.includes("tap-target-ok")) return [];
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(TAPPABLE_TAG_RE)) {
    const tag = m[0];
    const native = /^<(button|a|label|select|input)\b/.test(tag);
    if (!native && !/(?:@|v-on:)click/.test(tag)) continue;
    const classMatch = tag.match(/class="([^"]*)"/);
    if (!classMatch) continue;
    if (!SMALL_SIZE_RE.test(classMatch[1])) continue;
    if (/\btap-safe\b/.test(classMatch[1])) continue;
    out.push({
      line: lineAt(src, m.index ?? 0),
      message:
        "小尺寸可交互元素没有 `tap-safe` —— iOS HIG 的最小点按目标是 44×44pt，" +
        "而 `h-8`=32px / `h-7`=28px / `h-6`=24px，手指容易点不中或误触相邻项。" +
        "加 `tap-safe`（`:pointer: coarse` 下热区垂直 +16px：h-6→40、h-7→44、h-8→48）；" +
        "`h-5`（20px）即便加了也只有 36px，请改用 `h-7` 以上。" +
        "确属不需要触屏撑大的场合可加 `tap-target-ok` 注释整文件跳过。",
    });
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ⑨ `as="template"` 插槽不得有注释/多根节点 ----------------

/** 需要"单子节点"约束的第三方组件属性（Headless UI 的模板渲染）。 */
const TEMPLATE_ATTR_RE = /<([A-Z][\w]*)\b[^>]*\bas="template"[^>]*>/g;

/**
 * 检查一份 .vue 源码里 `as="template"` 的**直接插槽内容**是否安全。
 *
 * 判据（只报能确定的）：
 *   ① 直接内容里出现 HTML 注释 ⇒ 报（dev 构建把注释编译成节点，Headless UI 会抛错）；
 *   ② 直接子节点里的**顶层元素多于一个** ⇒ 报（同样会抛错）。
 * 嵌套元素内部的内容不参与判定（先按标签配对把子树跳过）。
 */
export function findTemplateSlotIssues(src: string): GuardIssue[] {
  if (src.includes("template-slot-ok")) return [];
  const out: GuardIssue[] = [];
  for (const m of src.matchAll(TEMPLATE_ATTR_RE)) {
    const tag = m[1];
    const openEnd = (m.index ?? 0) + m[0].length;
    // 是否为自闭合（`<Foo as="template" />` 没有插槽，跳过）
    if (m[0].trimEnd().endsWith("/>")) continue;

    // 扫描到配对结束标签，同时统计「顶层」元素与注释
    const closeTag = `</${tag}>`;
    let i = openEnd;
    let depth = 0;
    let topLevel = 0;
    let comments = 0;
    while (i < src.length) {
      if (src.startsWith(closeTag, i) && depth === 0) break;
      if (src.startsWith(`<${tag}`, i)) {
        depth += 1;
        i += tag.length + 1;
        continue;
      }
      if (src.startsWith(closeTag, i)) {
        depth -= 1;
        i += closeTag.length;
        continue;
      }
      if (src.startsWith("<!--", i)) {
        if (depth === 0) comments += 1;
        const end = src.indexOf("-->", i);
        i = end < 0 ? src.length : end + 3;
        continue;
      }
      // 跳过注释/字符串之外的普通标签：遇到元素开标签且 depth=0 就计一个顶层节点，
      // 并把它的子树整体跳过（避免把子元素算成兄弟）。
      if (src[i] === "<" && /[A-Za-z]/.test(src[i + 1] ?? "")) {
        let j = i;
        const nameMatch = /^<([A-Za-z][\w-]*)/.exec(src.slice(i));
        const name = nameMatch ? nameMatch[1] : "";
        const tagText = src.slice(i, src.indexOf(">", i) + 1);
        const selfClosing = /\/>/.test(tagText);
        // `v-if` / `v-else-if` / `v-else` 是一条链：运行时**只渲染一个**节点，
        // 所以后续的 `v-else*` 分支不再单独计数（否则会误报，第一次跑这条护栏就误报了
        // BaseModal 的 DialogPanel v-if/v-else 两个分支）。
        const isElseBranch = /\bv-else(-if)?\b/.test(tagText);
        if (depth === 0 && !isElseBranch) topLevel += 1;
        if (selfClosing || !name) {
          i = src.indexOf(">", i) + 1;
          continue;
        }
        // 跳过整棵子树
        j = i;
        while (j < src.length) {
          if (src.startsWith(`</${name}>`, j)) {
            j += name.length + 3;
            break;
          }
          if (src.startsWith("<" + name, j) || (src[j] === "<" && /[A-Za-z]/.test(src[j + 1] ?? ""))) {
            j = src.indexOf(">", j) + 1;
            continue;
          }
          j += 1;
        }
        i = j;
        continue;
      }
      i += 1;
    }

    if (comments > 0) {
      out.push({
        line: lineAt(src, m.index ?? 0),
        message:
          `\`<${tag} as="template">\` 的插槽里有 HTML 注释 —— dev 构建**保留注释**，` +
          "它会变成一个额外的节点，Headless UI 随即抛 \"Passing props on template!\"，" +
          "Vue 渲染抛错后整个界面都 patch 不动（现象就是\"窗口卡死\"）。" +
          "把注释放到该组件**外面**（或挪进 <script>）。",
      });
    } else if (topLevel > 1) {
      out.push({
        line: lineAt(src, m.index ?? 0),
        message:
          `\`<${tag} as="template">\` 的插槽里有 ${topLevel} 个顶层节点 —— Headless UI 的` +
          "模板渲染要求**恰好一个**，多根会抛 \"Passing props on template!\"。" +
          "请用一层真实元素包起来，或改用 `as=\"div\"`。",
      });
    }
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ⑤ 文本气泡排版必须与虚拟列表高度度量一致 ----------------

/** Tailwind 默认 `leading-*` → 行高倍数（tailwind.config.js 未覆盖 leadings，故可静态判定）。 */
const LEADING_RATIO: Record<string, number> = {
  "leading-none": 1,
  "leading-tight": 1.25,
  "leading-snug": 1.375,
  "leading-normal": 1.5,
  "leading-relaxed": 1.625,
  "leading-loose": 2,
};

/** 从一份 .vue 源码里取「类名恰好等于 cls」的元素的 `class` 片段位置（-1 = 没有）。 */
function classAttrIndexOf(src: string, cls: string): number {
  for (const m of src.matchAll(CLASS_ATTR_RE)) {
    const classes = m[1].split(/\s+/).filter(Boolean);
    if (classes.includes(cls)) return m.index ?? -1;
  }
  return -1;
}

/**
 * 检查「气泡真实渲染高度」与「虚拟列表估算高度」这一对**必须成对演化**的常量。
 *
 * 为什么要机器盯（与 ③ 未读徽标同一类）：`previewMetrics.ts` 的
 * `TEXT_LINE_RATIO` / `TEXT_BUBBLE_PADDING` 是 `MessageTextBubble.vue` 的
 * `leading-*` / `py-*` 的**镜像**。改了一边忘了另一边时，编译通过、测试全绿、
 * 单条消息看着也正常 —— 只有**滚动到相邻消息**才会发现互相遮挡/留白跳变。
 * 这种「写对了但不起作用、且不在出错点报错」正是本模块要守的东西。
 *
 * 参数：组件源码 + 度量文件源码。返回不一致项（含建议修正的具体数值）。
 */
export function checkBubbleMetricsCoupling(
  bubbleSrc: string,
  metricsSrc: string,
): GuardIssue[] {
  const out: GuardIssue[] = [];
  const at = classAttrIndexOf(bubbleSrc, "min-w-0");
  if (at < 0) {
    return [{ line: 0, message: "在气泡组件里找不到带 `min-w-0` 的根元素（度量对照失效）" }];
  }
  const tag = enclosingTag(bubbleSrc, at);
  const line = lineAt(bubbleSrc, at);
  const classes = (tag.match(/\bclass="([^"]*)"/)?.[1] ?? "").split(/\s+/).filter(Boolean);

  // 行高：必须先能识别出 `leading-*`，再从度量文件取比值。
  const leadingClass = classes.find((c) => c in LEADING_RATIO);
  const ratioRaw = metricsSrc.match(/TEXT_LINE_RATIO\s*=\s*([\d.]+)/)?.[1];
  if (!leadingClass || ratioRaw === undefined) {
    out.push({
      line,
      message:
        "无法核对气泡行高：组件缺少可识别的 `leading-*` 类，或度量文件里找不到 `TEXT_LINE_RATIO`。" +
        "两者必须成对存在，否则虚拟列表无法估算高度。",
    });
  } else if (Math.abs(LEADING_RATIO[leadingClass] - Number(ratioRaw)) > 1e-9) {
    out.push({
      line,
      message:
        `气泡用 \`${leadingClass}\`（行高 ${LEADING_RATIO[leadingClass]}），` +
        `但预览度量 \`TEXT_LINE_RATIO\` = ${ratioRaw} —— 两边不一致会让虚拟列表的` +
        `高度估算与真实渲染对不上（相邻消息遮挡）。把度量值改成 ${LEADING_RATIO[leadingClass]}。`,
    });
  }

  // 纵向内边距：`py-N` 一档 = 4px，上下合计 ×2。
  const pyClass = classes.find((c) => /^py-[\d.]+$/.test(c));
  const paddingRaw = metricsSrc.match(/TEXT_BUBBLE_PADDING\s*=\s*(\d+)/)?.[1];
  if (!pyClass || paddingRaw === undefined) {
    out.push({
      line,
      message:
        "无法核对气泡纵向内边距：组件缺少 `py-*` 类，或度量文件里找不到 `TEXT_BUBBLE_PADDING`。",
    });
  } else {
    const expected = Number(pyClass.slice(3)) * 4 * 2;
    if (expected !== Number(paddingRaw)) {
      out.push({
        line,
        message:
          `气泡用 \`${pyClass}\`（上下内边距合计 ${expected}px），` +
          `但预览度量 \`TEXT_BUBBLE_PADDING\` = ${paddingRaw} —— 同上，改成 ${expected}。`,
      });
    }
  }
  return out.sort((a, b) => a.line - b.line);
}

// ---------------- ② 媒体查询必须排在 style.css 末尾 ----------------

/** 取某选择器**顶级**定义（行首、无缩进）的最后一次出现位置；找不到返回 -1。 */
function lastTopLevelDefIndex(css: string, selector: string): number {
  const re = new RegExp(`(?:^|\\n)${selector}\\s*\\{`, "g");
  let idx = -1;
  for (const m of css.matchAll(re)) idx = m.index ?? -1;
  return idx;
}

/**
 * 检查 `src/style.css` 里两条"必须留在文件末尾"的降级规则。
 *
 * 为什么必须靠机器盯：降级块要覆盖 `.glass` / `.frost` 的 `backdrop-filter`，
 * 而 CSS 同优先级下**后定义者胜**。把块写到文件中部不会报错、也不会让任何行为测试变红，
 * 只是**静默失效** —— 只有真去开系统开关、或在无头浏览器里读计算值才会发现。
 */
export function checkStyleCascade(css: string): GuardIssue[] {
  const out: GuardIssue[] = [];

  // ②-1 降级媒体查询必须晚于 .glass / .frost / .vel-modal 的顶级定义
  const baseIdx = Math.max(
    lastTopLevelDefIndex(css, "\\.glass"),
    lastTopLevelDefIndex(css, "\\.frost"),
    lastTopLevelDefIndex(css, "\\.vel-modal"),
  );
  if (baseIdx < 0) {
    out.push({ line: 0, message: "style.css 里找不到 .glass / .frost / .vel-modal 的定义" });
  }
  const blocks: [string, string][] = [
    ["降低透明度", "@media (prefers-reduced-transparency: reduce)"],
    ["提高对比度", "@media (prefers-contrast: more)"],
  ];
  for (const [label, header] of blocks) {
    const at = css.indexOf(header);
    if (at < 0) {
      out.push({ line: 0, message: `style.css 缺少「${label}」媒体查询（${header}）` });
      continue;
    }
    if (baseIdx >= 0 && at < baseIdx) {
      out.push({
        line: lineAt(css, at),
        message:
          `「${label}」媒体查询排在 .glass/.frost 定义**之前** → 会被它们覆盖而静默失效。` +
          "必须移到 style.css 末尾（详见 docs/design-guidelines.md §10.1）。",
      });
    }
  }

  // ②-2 .hover-reveal / .hover-reveal-op 必须定义在 @media (hover: none) 里
  const hoverNoneAt = css.indexOf("@media (hover: none)");
  if (hoverNoneAt < 0) {
    out.push({ line: 0, message: "style.css 缺少 @media (hover: none) 触屏兜底块" });
  } else {
    for (const cls of [".hover-reveal", ".hover-reveal-op"]) {
      const at = css.indexOf(`${cls} {`);
      if (at < 0) {
        out.push({ line: 0, message: `style.css 缺少 ${cls} 的定义` });
      } else if (at < hoverNoneAt) {
        out.push({
          line: lineAt(css, at),
          message: `${cls} 定义在 @media (hover: none) 之外 → 在有 hover 的设备上也会常显，兜底失效。`,
        });
      }
    }
  }

  // ②-3 触屏命中扩展块里**不得直接声明 `position`**
  // ------------------------------------------------------------------
  // 真实缺陷（用户 2026-09-12 安卓实测）：「回到最新」按钮写的是
  // `tap-safe absolute bottom-4 right-5`，而 `@media (pointer: coarse)` 里的
  // `.tap-safe { position: relative }` 与本文件位置关系是：本文件在 `@tailwind utilities`
  // **之后**、两者特异性**相同**（都是单类）⇒ 触屏设备上 position 被改成 relative，
  // 按钮掉回文档流、不再贴右下角。桌面 `pointer: fine` 不走这条媒体查询，所以只有安卓复现。
  // 判据：该块内任何声明了 `position:` 的选择器都必须是 `:where(...)`（特异性 0），
  // 这样组件的 `absolute`/`fixed` 工具类才能正常生效。用 `:where()` 也让这条**面向未来**：
  // 以后往这个块里加任何"只负责扩大命中区"的类，都不会再压掉别人的定位。
  out.push(...findOverridingPositionInCoarsePointer(css));
  return out.sort((a, b) => a.line - b.line);
}

/** `@media (pointer: coarse)` 块内直接声明 `position` 的选择器（`:where()` 包裹的除外）。 */
function findOverridingPositionInCoarsePointer(css: string): GuardIssue[] {
  const out: GuardIssue[] = [];
  let cursor = 0;
  while (true) {
    const at = css.indexOf("@media (pointer: coarse)", cursor);
    if (at < 0) break;
    const open = css.indexOf("{", at);
    if (open < 0) break;
    // 括号配平找块尾
    let depth = 0;
    let end = css.length - 1;
    for (let i = open; i < css.length; i++) {
      if (css[i] === "{") depth += 1;
      else if (css[i] === "}") {
        depth -= 1;
        if (depth === 0) {
          end = i;
          break;
        }
      }
    }
    // 去掉注释（并且**等长替换成空白**，这样 ruleRe 的下标仍能对回原文行号）：
    // 否则注释会被当成选择器文本（这个块里的注释恰好写了 `:where()` 与类名）。
    const body = css
      .slice(open + 1, end)
      .replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));
    const ruleRe = /([^{}]+)\{([^{}]*)\}/g;
    let m: RegExpExecArray | null;
    while ((m = ruleRe.exec(body)) !== null) {
      const selector = m[1].trim();
      if (!/(^|;)\s*position\s*:/.test(m[2])) continue;
      if (selector.startsWith(":where(")) continue; // 特异性 0 ⇒ 压不掉工具类
      out.push({
        line: lineAt(css, open + 1 + m.index),
        message:
          `@media (pointer: coarse) 里的 \`${selector}\` 直接声明了 position：` +
          "本文件在 @tailwind utilities 之后且特异性相同，会覆盖组件自己的定位" +
          "（真实缺陷：安卓端「回到最新」按钮从 absolute 变成 relative）。" +
          "请写成 `:where(…)` 把特异性压到 0。",
      });
    }
    cursor = end + 1;
  }
  return out;
}

// ---------------- ⑧ 聊天区「文本选择」契约（用户 2026-09-13） ----------------

/** 取 `selector { … }` 的规则体（朴素匹配：本项目 CSS 里这几条规则都没有嵌套花括号）。 */
function cssRuleBody(css: string, selector: string): string | null {
  const i = css.indexOf(selector);
  if (i < 0) return null;
  const open = css.indexOf("{", i);
  const close = css.indexOf("}", open);
  if (open < 0 || close < 0) return null;
  return css.slice(open + 1, close);
}

/** 取「class 里含 needle 的那个 <img …> 标签」的完整文本。 */
function imgTagWithClass(src: string, needle: string): string | null {
  for (const m of src.matchAll(/<img\b[^>]*>/g)) {
    if (m[0].includes(needle)) return m[0];
  }
  return null;
}

export interface SelectionContractSources {
  /** `components/message/MessageTextBubble.vue` */
  textBubble: string;
  /** `components/message/MessageAvatar.vue` */
  avatar: string;
  /** `components/MessageItem.vue` */
  messageItem: string;
  /** `App.vue` */
  app: string;
  /** `components/message/MessageLinkText.vue` */
  linkText: string;
  /** `style.css` */
  css: string;
}

/**
 * 聊天区的**文本选择契约**。
 *
 * 用户 2026-09-13 报的三件事，全都是"写对了但不起作用"的静默退化：
 *
 * ① PC 上拖选气泡文字，**刚选中立刻被取消**
 *    —— 正文被 `px-3 py-1.5` 包着，从内边距/气泡边缘起拖时选区锚点落在不可选区域，
 *       WebKit 会立刻收敛选区；正文里的表情是 `<img>`，浏览器默认允许拖图，
 *       拖选划过表情会改成"拖图片"，同样打断选区。
 * ② **头像不该被选中**（用户明确要求：将来会有点击事件，但依然不能选择）。
 * ③ 移动端**选中文字后不弹系统「复制/全选」工具条**
 *    —— `button,[role=button]` 那条把 `-webkit-touch-callout` 关了会盖住正文；
 *       全局 `contextmenu` 无条件 `preventDefault()` 也会吃掉选区菜单
 *       （Android WebView 的选择工具条依赖它的默认行为）。
 *
 * 判据全部是"能不能选中/拖拽"的静态事实，改坏了不会报错、只会让用户用不了，
 * 所以必须由机器盯住。
 */
export function checkSelectionContract(s: SelectionContractSources): GuardIssue[] {
  const out: GuardIssue[] = [];

  // ① 正文必须可选字。
  //    ⚠️ 这个类在触屏下**不再**意味着"长按让路给原生选字"（那是 4.3.0 之前的模型）：
  //    现在 `@media (pointer: coarse)` 会把正文气泡里的它关掉选中，选字改走菜单里的
  //    「选择文字」二级入口（见 style.css 与 utils/longPress.ts）。
  if (!s.textBubble.includes("gosslan-selectable")) {
    out.push({
      line: 0,
      message:
        "MessageTextBubble：正文缺少 `gosslan-selectable` —— 消息文字将无法选字/复制，" +
        "「选择文字」模式也没有可选的落点。",
    });
  }
  // ② 气泡根也要可选：否则从内边距起拖会被 WebKit 立刻收敛（用户实测的"刚选中就没了"）
  const rootSelectable = [...s.textBubble.matchAll(CLASS_ATTR_RE)].some((m) =>
    m[1].split(/\s+/).includes("select-text"),
  );
  if (!rootSelectable) {
    out.push({
      line: 0,
      message:
        "MessageTextBubble：气泡根缺少 `select-text` —— 从气泡内边距/边缘起拖时选区锚点落在" +
        "不可选区域，WebKit 会立刻把选区收敛掉（表现为「刚选中立马取消选中」）。",
    });
  }
  // ③ 正文里的表情图片必须禁用拖拽（否则拖选划过表情会变成拖图片、选区中断）
  const emojiImg = imgTagWithClass(s.textBubble, "emoji-img");
  if (!emojiImg) {
    out.push({ line: 0, message: "MessageTextBubble：找不到 `class=\"emoji-img\"` 的表情 `<img>`（模板结构变了？）。" });
  } else if (!emojiImg.includes('draggable="false"')) {
    out.push({
      line: 0,
      message: "MessageTextBubble：表情 `<img>` 缺少 `draggable=\"false\"` —— 拖选划过表情会被浏览器改成拖图片，选区中断。",
    });
  }

  // ④ 头像不可选、不可拖（用户明确要求）
  const avatarBox = cssRuleBody(s.css, ".gosslan-avatar-box");
  if (!avatarBox || !/user-select:\s*none/.test(avatarBox)) {
    out.push({
      line: 0,
      message: "style.css：`.gosslan-avatar-box` 缺少 `user-select: none` —— 头像会被拖选进选区（用户要求头像不可选）。",
    });
  }
  const avatarImg = imgTagWithClass(s.avatar, 'class="h-full w-full object-cover"');
  if (avatarImg && !avatarImg.includes('draggable="false"')) {
    out.push({
      line: 0,
      message: "MessageAvatar：头像 `<img>` 缺少 `draggable=\"false\"` —— 拖动头像会变成拖图片。",
    });
  }

  // ⑤ 表情图片同样不许拖（CSS 兜底，防止某个调用点忘了 draggable 属性）
  const emojiCss = cssRuleBody(s.css, ".emoji-img");
  if (!emojiCss || !/-webkit-user-drag:\s*none/.test(emojiCss)) {
    out.push({
      line: 0,
      message: "style.css：`.emoji-img` 缺少 `-webkit-user-drag: none` —— 正文表情仍可被拖走、打断选区。",
    });
  }
  // ⑥ 可选文本必须显式打开 touch-callout，否则 iOS/Android 选中后不弹「复制」工具条
  const selCss = cssRuleBody(s.css, ".gosslan-selectable");
  if (!selCss || !/-webkit-touch-callout:\s*default/.test(selCss)) {
    out.push({
      line: 0,
      message:
        "style.css：`.gosslan-selectable` 缺少 `-webkit-touch-callout: default` —— " +
        "`button,[role=button]` 的 `none` 会盖住正文，选中文字后系统不弹「复制/全选」工具条。",
    });
  }

  // ⑦ 长按必须带手指抖动容差：`@touchmove` 直接接 cancel 会"一动就取消"（真机长按失灵）
  if (/@touchmove="cancelLongPress"/.test(s.messageItem)) {
    out.push({
      line: 0,
      message:
        "MessageItem：`@touchmove` 直接绑了 `cancelLongPress` —— 手指动 1px 就取消长按，" +
        "真机上等于长按永远不触发。应绑 `onTouchMove`（带像素容差）。",
    });
  } else if (!/@touchmove="onTouchMove"/.test(s.messageItem)) {
    out.push({ line: 0, message: "MessageItem：`@touchmove` 没有绑 `onTouchMove`（长按容差判据缺失）。" });
  }

  // ⑧ 有选区时必须放行系统右键菜单（否则选中的文字没法用原生「复制」）
  if (!/getSelection\(\)/.test(s.app)) {
    out.push({
      line: 0,
      message:
        "App.vue：全局 `contextmenu` 没有检查选区 —— 有选中文字时仍然 preventDefault，" +
        "用户拿不到系统「复制」，Android 的选择工具条也会被吃掉。",
    });
  }

  // ⑨ 超长链接的「中间省略」只准动样式，不许动 DOM 文本
  //
  // 用户 2026-09-16：「链接…复制的时候会复制不完整的，应该复制原始数据」——
  // 原先把截断后的字符串当 DOM 文本渲染，而**选区复制取的就是选区文本**，于是复制到残缺 URL。
  // 现行契约：`splitUrl` 切出的 head/mid/tail 三段**都在 DOM 里、顺序不变**，
  // 视觉省略只由 CSS 表达。下面每条都是"改坏了不报错、只会静默复制出错"的静态事实。
  const midCss = cssRuleBody(s.css, ".gosslan-url-mid");
  if (!midCss || !/font-size:\s*0/.test(midCss)) {
    out.push({
      line: 0,
      message:
        "style.css：`.gosslan-url-mid` 缺少 `font-size: 0` —— 要么链接中段直接显示出来，" +
        "要么（换成别的方式隐藏）中段被踢出选区，复制又得到残缺 URL。",
    });
  }
  if (midCss && /display:\s*none|visibility:\s*hidden|user-select:\s*none/.test(midCss)) {
    out.push({
      line: 0,
      message:
        "style.css：`.gosslan-url-mid` 用 display:none / visibility:hidden / user-select:none 隐藏 —— " +
        "这三种都会把中段文字从选区里剔除，选中复制又将拿到残缺链接。只能用 font-size: 0。",
    });
  }
  const dotsCss = cssRuleBody(s.css, ".gosslan-url-dots");
  if (!dotsCss || !/background-image/.test(dotsCss)) {
    out.push({
      line: 0,
      message:
        "style.css：`.gosslan-url-dots` 必须仍是 CSS 画的点（含 background-image）—— " +
        '改成 "…" 文本的话，省略号本身会被一起复制进 URL。',
    });
  }

  // 模板：三段齐全、DOM 顺序 head→mid→dots→tail（选区按 DOM 顺序拼接）
  // ⚠️ 判据只扫 `<a>` 标签**内部**：组件注释里也会写到这些名字，扫全文会被注释带偏。
  const anchorInner = /<a\b[^>]*>([\s\S]*?)<\/a>/.exec(s.linkText)?.[1] ?? "";
  const linkOrder = ["parts.head", "parts.mid", "gosslan-url-dots", "parts.tail"].map((k) =>
    anchorInner.indexOf(k),
  );
  if (linkOrder.some((i) => i < 0)) {
    out.push({
      line: 0,
      message: "MessageLinkText：`<a>` 里缺少 head/mid/tail（或省略号 span）—— 复制拿不到完整 URL。",
    });
  } else if (!linkOrder.every((v, i) => i === 0 || linkOrder[i - 1] < v)) {
    out.push({
      line: 0,
      message:
        "MessageLinkText：head/mid/dots/tail 的 DOM 顺序被打乱 —— 选区复制按 DOM 顺序拼接，" +
        "mid 必须夹在 head 与 tail 之间才能还原原始 URL。",
    });
  }
  // 省略号 span 内不得有任何文本（含插值）
  const dotsInner = /<span[^>]*gosslan-url-dots[^>]*>([\s\S]*?)<\/span>/.exec(anchorInner);
  if (dotsInner && dotsInner[1].trim() !== "") {
    out.push({
      line: 0,
      message: `MessageLinkText：省略号 span 里带了文本「${dotsInner[1].trim()}」—— 它会被原样复制进 URL。`,
    });
  }
  // ⑩ 引用块必须能被框进选区，且触屏下默认仍不放开
  //
  // 「选择文字」的全选范围挂在「引用块 + 正文」的容器上，而引用块本身是个 `<button>`
  // （要能点着跳原消息），会吃到全局的 `button { user-select: none }` ——
  // 没有这条覆盖，全选复制拿到的是纯正文，而操作条的「复制」给的是带引用头的完整原文，
  // 同一个气泡两条复制路径结果不一致。
  // 反过来，触屏上**不能**无条件放开：那样在引用块上长按会变成拉原生选区、
  // 弹不出消息菜单（移动端主行为被抢）。所以必须成对存在。
  const quoteCss = cssRuleBody(s.css, ".quote-block");
  if (!quoteCss || !/user-select:\s*text/.test(quoteCss)) {
    out.push({
      line: 0,
      message:
        "style.css：`.quote-block` 缺少 `user-select: text` —— 引用块的 `<button>` 会吃到全局的 " +
        "`user-select: none`，全选复制拿不到引用头，与「复制」按钮给的完整原文不一致。",
    });
  }
  const quoteTouchCss = cssRuleBody(s.css, ":not(.gosslan-selecting) .quote-block");
  if (!quoteTouchCss || !/user-select:\s*none/.test(quoteTouchCss)) {
    out.push({
      line: 0,
      message:
        "style.css：触屏下 `.gosslan-bubble-text:not(.gosslan-selecting) .quote-block` 必须保持 " +
        "`user-select: none` —— 否则在引用块上长按会拉原生选区、弹不出消息菜单。",
    });
  }

  // `<a>` 内的 span 之间不得留空白：模板空白是真实文本节点，会进选区 ⇒ 复制出的 URL 里多空格
  if (/>\s+</.test(anchorInner)) {
    out.push({
      line: 0,
      message:
        "MessageLinkText：`<a>` 里的 span 之间夹了空白（换行/缩进）—— 模板空白会成为文本节点、" +
        "被选进选区，复制出来的 URL 中间会多出空格。写成一行。",
    });
  }

  return out;
}

// ---------------- ⑨ 文本输入类的焦点提示：不许画外圈方框 ----------------

/**
 * 文本输入类的焦点提示必须画在**边线**上，不能画成外圈方框。
 *
 * 真实反馈（用户 2026-09-16）：「整个应用的输入框在焦点态会默认有个主题色的方框，很难看」。
 * 根因是全局焦点环的选择器里含 `input` / `textarea` / `select` / `[contenteditable]` ——
 * 浏览器对文本类控件一律把 `:focus-visible` 判成"永远成立"（**点一下就成立**，不需要键盘
 * Tab），于是每次点击输入框都会冒出一个 2px 外圈方框。消息输入框尤其突兀：它的矩形只是
 * 卡片里一块**透明的编辑区**，框出来像卡片内部浮着一个方框。
 *
 * 三条判据（缺一不可）：
 * 1. 全局 `:focus-visible` 的选择器里**不得**出现文本输入类；
 * 2. 必须有规则把 `input` / `textarea` / `select` 的焦点提示画在**边线**上（与应用既有的
 *    `.gosslan-select:focus`、`focus:border-[var(--gosslan-primary)]` 同一套观感）；
 * 3. 消息输入框的卡片必须带上 `gosslan-composer` 钩子（`:focus-within` 变边框色）——
 *    编辑区自己不能画框，少了这条消息输入框就完全没有焦点提示。
 */
export function checkTextFieldFocusRing(css: string, composer: string): GuardIssue[] {
  const out: GuardIssue[] = [];
  const textControls = ["input", "textarea", "select", "contenteditable"];

  // ⚠️ 先**按原长度**把注释抹掉（换行保留 ⇒ 行号与下标不变）：说明这条规则的那段注释里
  // 天然会提到 `input` / `textarea` / `[contenteditable]`，不抹掉就会"被自己的说明误伤"
  // （本项目踩过这种假阳性）。
  const code = css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));

  // ① 全局 :focus-visible 规则的选择器里不许有文本输入类
  let scanned = 0;
  for (const m of code.matchAll(/([^{}]*:focus-visible[^{}]*)\{/g)) {
    scanned += 1;
    const selector = m[1];
    const offenders = textControls.filter((t) => selector.includes(t));
    if (offenders.length > 0) {
      // 行号要落在**选择器**那一行（匹配段会带上选择器前面的换行/缩进）
      const at = (m.index ?? 0) + Math.max(0, selector.search(/\S/));
      out.push({
        line: lineAt(css, at),
        message:
          "全局 :focus-visible 的选择器里不该有文本输入类（" +
          offenders.join(" / ") +
          "）：浏览器对它们恒判 :focus-visible ⇒ **点一下**输入框就冒出一个外圈方框" +
          "（用户 2026-09-16 反馈）。文本输入类的焦点提示请画在边线上。",
      });
    }
  }
  if (scanned === 0) {
    out.push({ line: 0, message: "style.css 里找不到任何 :focus-visible 规则（键盘焦点环被删了？）" });
  }

  // ② 文本输入类必须有一条"边线级"的焦点提示
  if (!/input:focus[\s\S]{0,160}?textarea:focus[\s\S]{0,160}?select:focus/.test(code)) {
    out.push({
      line: 0,
      message:
        "找不到文本输入类的焦点提示规则（形如 `input:focus, textarea:focus, select:focus { … }`）：" +
        "去掉外圈方框之后，本来就没有边框的输入框会完全没有焦点提示（WCAG 2.4.7）。",
    });
  }

  // ③ 消息输入框的卡片钩子
  if (!/\.gosslan-composer\s*:focus-within/.test(code)) {
    out.push({
      line: 0,
      message:
        "style.css 缺少 `.gosslan-composer:focus-within` 规则：消息输入框的编辑区不能画外框，" +
        "焦点提示必须挂在卡片边框上。",
    });
  }
  if (!composer.includes("gosslan-composer")) {
    out.push({
      line: 0,
      message:
        "MessageComposer 的卡片必须带 `gosslan-composer` 类（style.css 的焦点钩子），" +
        "否则消息输入框没有焦点提示。",
    });
  }
  return out;
}
