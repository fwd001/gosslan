import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import {
  checkBubbleMetricsCoupling,
  findTemplateSlotIssues,
  checkStyleCascade,
  checkUnreadBadgeComponent,
  findHandWrittenBadges,
  findHoverRevealIssues,
  findOutlineNoneWithoutFocusRing,
  findSmallTapTargets,
  findTappableWithoutKeyboard,
  findTruncationWithoutTitle,
  checkTextFieldFocusRing,
  checkSelectionContract,
} from "./designGuards.ts";

// ---------------- ① 悬停揭示必须有触屏兜底 ----------------
//
// 真实事故（2026-09-10 审计 P0-1）：会话行的删除键写成
// `hidden` + `group-hover/conv:flex` —— 桌面能删、**Android 上按钮永远不显示**，
// 因为触屏没有 hover。第一段用例就是照当时的真实写法缩写的。

test("复现历史缺陷：hidden + group-hover:flex 没有兜底类 → 报出", () => {
  const buggy = `<template>
  <div class="group/conv relative flex h-[64px]">
    <button class="absolute bottom-1.5 right-1.5 z-10 hidden h-6 w-6 group-hover/conv:flex">
      <X />
    </button>
  </div>
</template>`;
  const issues = findHoverRevealIssues(buggy);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].line, 3, "应精确指到那一行");
  assert.match(issues[0].message, /永远不显示/);
});

test("加了 hover-reveal 之后通过", () => {
  const fixed = `<template>
  <button class="hover-reveal hidden h-6 w-6 group-hover/conv:flex"><X /></button>
</template>`;
  assert.deepEqual(findHoverRevealIssues(fixed), []);
});

test("opacity-0 + group-hover 揭示 → 需要 hover-reveal-op", () => {
  const buggy = `<template>
  <span class="absolute inset-0 bg-black/40 opacity-0 transition group-hover:opacity-100"><Camera /></span>
</template>`;
  const issues = findHoverRevealIssues(buggy);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /hover-reveal-op/);

  const fixed = buggy.replace('class="absolute', 'class="hover-reveal-op absolute');
  assert.deepEqual(findHoverRevealIssues(fixed), []);
});

test("常显但 hover 时更不透明（opacity-70 → opacity-100）不算揭示，不报", () => {
  const ok = `<template>
  <button class="opacity-70 transition hover:opacity-100"><Copy /></button>
</template>`;
  assert.deepEqual(findHoverRevealIssues(ok), []);
});

test("独立的 hover 效果（不是揭示）不报", () => {
  const ok = `<template>
  <button class="hidden hover:bg-red-500/10"><X /></button>
  <button class="opacity-0 hover:opacity-100"><X /></button>
</template>`;
  assert.deepEqual(findHoverRevealIssues(ok), []);
});

test("逃生阀：带 hover-reveal-ok 的文件整体跳过", () => {
  const optedOut = `<!-- hover-reveal-ok -->
<template><button class="hidden group-hover:flex"><X /></button></template>`;
  assert.deepEqual(findHoverRevealIssues(optedOut), []);
});

// ---------------- ② 降级媒体查询必须排在 style.css 末尾 ----------------
//
// 真实踩坑：`.glass` / `.frost` 的定义在文件中更靠后，同优先级下"后定义者胜"，
// 降级块写在前面会被**完整覆盖**——不报错、不失败，只是静默无效。

const OK_CSS = `
.glass { backdrop-filter: blur(6px); }
.frost { backdrop-filter: blur(6px); }

@media (hover: none) {
  .hover-reveal { display: flex !important; }
  .hover-reveal-op { opacity: 1 !important; }
}

@media (prefers-reduced-transparency: reduce) {
  .frost { backdrop-filter: none; }
  .glass { backdrop-filter: none; }
}
@media (prefers-contrast: more) {
  :root { --gosslan-border: #94a3b8; }
}
`;

test("正确顺序（降级块在末尾）→ 通过", () => {
  assert.deepEqual(checkStyleCascade(OK_CSS), []);
});

test("降级块排在 .glass/.frost 定义之前 → 报出（会被覆盖而静默失效）", () => {
  const wrongOrder = `
@media (prefers-reduced-transparency: reduce) {
  .glass { backdrop-filter: none; }
}
@media (prefers-contrast: more) {
  :root { --gosslan-border: #94a3b8; }
}
.glass { backdrop-filter: blur(6px); }
.frost { backdrop-filter: blur(6px); }
@media (hover: none) {
  .hover-reveal { display: flex !important; }
  .hover-reveal-op { opacity: 1 !important; }
}
`;
  const issues = checkStyleCascade(wrongOrder);
  const messages = issues.map((i) => i.message).join("\n");
  assert.match(messages, /降低透明度/);
  assert.match(messages, /提高对比度/);
});

test("缺少降级块 / 缺少 hover 兜底块 → 都要报", () => {
  const bare = `.glass { backdrop-filter: blur(6px); }\n`;
  const messages = checkStyleCascade(bare).map((i) => i.message).join("\n");
  assert.match(messages, /降低透明度/);
  assert.match(messages, /提高对比度/);
  assert.match(messages, /hover: none/);
});

test(".hover-reveal 定义在 (hover: none) 之外 → 报出（会在有 hover 的设备上也常显）", () => {
  const wrong = `
.hover-reveal { display: flex !important; }
.hover-reveal-op { opacity: 1 !important; }
@media (hover: none) {
  .tap-safe { position: relative; }
}
.glass { backdrop-filter: blur(6px); }
@media (prefers-reduced-transparency: reduce) { .glass { backdrop-filter: none; } }
@media (prefers-contrast: more) { :root { --gosslan-border: #94a3b8; } }
`;
  const messages = checkStyleCascade(wrong).map((i) => i.message).join("\n");
  assert.match(messages, /@media \(hover: none\) 之外/);
});

// ---------------- ③ 未读徽标必须走唯一实现 ----------------
//
// 真实踩坑（2026-09-12，用户反馈「红点数字没在圆里居中，上宽下窄」）：
// 徽标数字需要 1.5px 光学补偿才能垂直居中（字形在行盒里天然偏下：实测上间隙 10 /
// 下间隙 7，2x 截图，三处徽标一致）。而 5 处手写副本里有 2 处漏了配套的
// `leading-none` —— 同一徽标在不同位置基线不一致。第一段用例照当时的真实写法缩写。

test("复现历史缺陷：手写未读徽标 → 报出并指向替代组件", () => {
  const buggy = `<template>
  <span class="relative">
    <MessageCircle class="h-5 w-5" />
    <span
      v-if="chat.totalUnread > 0"
      class="absolute -right-2.5 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-[var(--gosslan-danger)] px-1 text-[11px] font-medium text-white"
    >
      {{ chat.totalUnread }}
    </span>
  </span>
</template>`;
  const issues = findHandWrittenBadges(buggy);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].line, 6, "应精确指到那一行（class 所在行）");
  assert.match(issues[0].message, /UnreadBadge/);
});

test("改用 UnreadBadge 组件之后通过", () => {
  const fixed = `<template>
  <span class="relative">
    <MessageCircle class="h-5 w-5" />
    <UnreadBadge v-if="chat.totalUnread > 0" :count="chat.totalUnread" class="absolute -right-2.5 -top-1" />
  </span>
</template>`;
  assert.deepEqual(findHandWrittenBadges(fixed), []);
});

test("单独用 min-w-4 或单独用 danger 色、但不是徽标的元素，不报", () => {
  const ok = `<template>
  <span class="min-w-4 rounded px-2">标签</span>
  <span class="bg-[var(--gosslan-danger)] px-2 text-white">错误提示</span>
</template>`;
  assert.deepEqual(findHandWrittenBadges(ok), []);
});

test("徽标组件自身缺补偿或缺 leading-none → 都要报", () => {
  const full =
    '<span class="flex h-4 min-w-4 items-center justify-center rounded-full ' +
    'bg-[var(--gosslan-danger)] px-1 pb-[1.5px] text-[11px] leading-none text-white">3</span>';
  assert.deepEqual(checkUnreadBadgeComponent(full), []);

  const noPad = full.replace(" pb-[1.5px]", "");
  assert.equal(checkUnreadBadgeComponent(noPad).length, 1);
  assert.match(checkUnreadBadgeComponent(noPad)[0].message, /pb-\[1\.5px\]/);

  const noLeading = full.replace(" leading-none", "");
  assert.equal(checkUnreadBadgeComponent(noLeading).length, 1);
  assert.match(checkUnreadBadgeComponent(noLeading)[0].message, /leading-none/);
});

test("注释里提到类名、真实 class 属性里没有 → 仍要报（防「因为注释而通过」）", () => {
  // 首版护栏用 `src.includes("pb-[1.5px]")` 扫全文，于是组件 JSDoc 里提到这些类名
  // 就"通过"了 —— 是空转护栏。这条用例把这个假通过固化成反面样本。
  const commentOnly = `<!-- 参考写法：pb-[1.5px] leading-none -->
<span class="flex h-4 min-w-4 items-center justify-center rounded-full bg-[var(--gosslan-danger)] px-1 text-[11px] text-white">3</span>`;
  const issues = checkUnreadBadgeComponent(commentOnly);
  assert.equal(issues.length, 2, "补偿与 leading-none 都缺，两条都要报");
  assert.match(issues.map((i) => i.message).join("\n"), /pb-\[1\.5px\]/);
});

// ---------------- ④ 截断文本必须有 title / aria-label ----------------
//
// 真实背景：`truncate` 把名字/地址/文件名截成「…」，完整内容只留在 DOM 里 ——
// 只有在界面上真去 hover 才会发现「看不到全名」。用户 2026-09-10 审计与
// 2026-09-12 反馈各报了一次（消息列表名、通讯录名、引用预览条…）。
// 首段用例照当时的真实写法（截断但无 title）缩写。

test("复现历史缺陷：truncate 但没有 title → 报出并指到那一行", () => {
  const buggy = `<template>
  <span class="truncate text-sm">{{ name }}</span>
</template>`;
  const issues = findTruncationWithoutTitle(buggy);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].line, 2, "应精确指到那一行");
  assert.match(issues[0].message, /title/);
});

test("跨行的开标签（class 与内容分行）也要抓到", () => {
  const multiline = `<template>
  <span
    class="truncate text-sm"
  >{{ name }}</span>
</template>`;
  assert.equal(findTruncationWithoutTitle(multiline).length, 1);
});

test("补了 :title 或 aria-label 之后通过", () => {
  const fixed = `<template>
  <span class="truncate text-sm" :title="name">{{ name }}</span>
  <span class="truncate text-sm" aria-label="x">x</span>
</template>`;
  assert.deepEqual(findTruncationWithoutTitle(fixed), []);
});

test("注释里提到 truncate、真实 class 里没有 → 不误报", () => {
  const commentOnly = `<!-- 这里不要写 truncate -->
<template><span class="text-sm">{{ name }}</span></template>`;
  assert.deepEqual(findTruncationWithoutTitle(commentOnly), []);
});

test("注释里提到 title、真实元素没有 → 仍要报（防「因为注释而通过」）", () => {
  const commentOnly = `<!-- 参考写法：truncate + :title -->
<template>
  <span class="truncate text-sm">{{ name }}</span>
</template>`;
  assert.equal(findTruncationWithoutTitle(commentOnly).length, 1);
});

test("逃生阀：带 truncate-title-ok 注释的文件整体跳过", () => {
  const optedOut = `<!-- truncate-title-ok：恒为短文案 -->
<template><span class="truncate">在线</span></template>`;
  assert.deepEqual(findTruncationWithoutTitle(optedOut), []);
});

// ---------------- ⑤ 气泡排版必须与虚拟列表高度度量一致 ----------------
//
// 真实背景：`previewMetrics.ts` 的 `TEXT_LINE_RATIO` / `TEXT_BUBBLE_PADDING`
// 是气泡组件 `leading-*` / `py-*` 的镜像。2026-09-12 按用户反馈把气泡从
// `py-2 + leading-relaxed` 收紧到 `py-1.5 + leading-normal` 时，**两处必须同时改**
// —— 只改一处不会报错、不会让任何行为测试变红，只有滚动到相邻消息才会互相遮挡。

test("复现风险：气泡行高与度量不一致 → 报出并给出应改的数值", () => {
  const bubble = `<template>
  <div class="group relative min-w-0 px-3 py-1.5 leading-relaxed" :style="bubbleStyle"></div>
</template>`;
  const metrics = `const TEXT_LINE_RATIO = 1.5;\nconst TEXT_BUBBLE_PADDING = 12;`;
  const issues = checkBubbleMetricsCoupling(bubble, metrics);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /leading-relaxed/);
  assert.match(issues[0].message, /1\.625/, "应指出组件实际行高");
});

test("复现风险：气泡内边距与度量不一致 → 报出", () => {
  const bubble = `<template>
  <div class="group relative min-w-0 px-3 py-2 leading-normal"></div>
</template>`;
  const metrics = `const TEXT_LINE_RATIO = 1.5;\nconst TEXT_BUBBLE_PADDING = 12;`;
  const issues = checkBubbleMetricsCoupling(bubble, metrics);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /16px/);
});

test("两边一致 → 通过", () => {
  const bubble = `<template>
  <div class="group relative min-w-0 px-3 py-1.5 leading-normal"></div>
</template>`;
  const metrics = `const TEXT_LINE_RATIO = 1.5;\nconst TEXT_BUBBLE_PADDING = 12;`;
  assert.deepEqual(checkBubbleMetricsCoupling(bubble, metrics), []);
});

test("找不到气泡根元素 / 缺少常量 → 显式报出（不静默通过）", () => {
  assert.equal(checkBubbleMetricsCoupling(`<template><div></div></template>`, "x").length, 1);
  const bubble = `<template><div class="min-w-0 leading-normal py-1.5"></div></template>`;
  assert.equal(checkBubbleMetricsCoupling(bubble, "const NOTHING = 1;").length, 2);
});

// ---------------- ⑥ 可点击元素必须能用键盘触发 ----------------
//
// 真实情况（2026-09-12 复核）：好友选择行、图片/文件气泡、指纹复制都是 `div @click`。
// 触屏和鼠标都能用，**键盘完全够不着** —— 这类缺陷在真机上"能用"，只有拿键盘走一遍
// 或开读屏才会发现，因此必须由机器盯住。

test("复现真实缺陷：div + @click 没有任何键盘/语义补充 → 报出", () => {
  const buggy = `<template>
  <div class="cursor-pointer" @click="open()">打开</div>
</template>`;
  const issues = findTappableWithoutKeyboard(buggy);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].line, 2, "应精确指到那一行");
  assert.match(issues[0].message, /Tab 不到/);
});

test("role + tabindex + 回车/空格键处理之后通过", () => {
  const fixed = `<template>
  <div role="button" tabindex="0" @click="open()" @keydown.enter.prevent="open()" @keydown.space.prevent="open()">打开</div>
</template>`;
  assert.deepEqual(findTappableWithoutKeyboard(fixed), []);
});

test("三种补充写法任意一种即可（含 :role / :tabindex 绑定形式）", () => {
  for (const extra of [
    'role="button"',
    ':role="ready ? \'button\' : undefined"',
    'tabindex="0"',
    ':tabindex="ready ? 0 : undefined"',
    '@keydown.enter="open()"',
    'v-on:keyup.enter="open()"',
  ]) {
    const ok = `<template>\n  <div @click="open()" ${extra}>x</div>\n</template>`;
    assert.deepEqual(findTappableWithoutKeyboard(ok), [], `补 ${extra} 后不该再报`);
  }
});

test("遮罩层（aria-hidden）不是按钮，不报", () => {
  const backdrop = `<template>
  <div class="fixed inset-0 bg-black/40" aria-hidden="true" @click="emit('close')" />
</template>`;
  assert.deepEqual(findTappableWithoutKeyboard(backdrop), []);
});

test("只拦冒泡的 @click.stop 不是动作，不报", () => {
  const stopper = `<template>
  <div class="frost absolute" @click.stop>
    <span>内容</span>
  </div>
</template>`;
  assert.deepEqual(findTappableWithoutKeyboard(stopper), []);
});

test("原生按钮与链接不在扫描范围", () => {
  const native = `<template>
  <button type="button" @click="open()">打开</button>
  <a href="#" @click.prevent="open()">链接</a>
  <label @click="pick()">选择</label>
</template>`;
  assert.deepEqual(findTappableWithoutKeyboard(native), []);
});

test("tap-keyboard-ok 逃生阀：整文件跳过", () => {
  const withEscape = `<!-- tap-keyboard-ok -->
<template>
  <div @click="open()">x</div>
</template>`;
  assert.deepEqual(findTappableWithoutKeyboard(withEscape), []);
});

// ---------------- ⑦ `outline-none` 必须自带焦点指示 ----------------
//
// 真实缺陷（2026-09-12）：全局焦点环写在 `:where(...)` 里（特异性 0），
// 会被 Tailwind 的 `.outline-none`（0,1,0）**静默覆盖** —— 7 处输入框
// （含最高频的消息输入框）因此完全没有焦点指示，而代码看起来"有全局规则在管"。
// 这条护栏把"关掉了轮廓就必须自己给指示"钉死。

test("复现真实缺陷：outline-none 且没有替代指示 → 报出", () => {
  const buggy = `<template>
  <input class="w-full outline-none" />
</template>`;
  const issues = findOutlineNoneWithoutFocusRing(buggy);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].line, 2);
  assert.match(issues[0].message, /静默覆盖/);
});

test("给了替代焦点指示就通过（ring / border 都算）", () => {
  for (const extra of [
    "focus:ring-2 focus:ring-primary",
    "focus-visible:ring-2 focus-visible:ring-primary",
    "focus:border-[var(--gosslan-primary)]",
    "focus-visible:outline-none focus-visible:ring-1",
  ]) {
    const ok = `<template>\n  <input class="outline-none ${extra}" />\n</template>`;
    assert.deepEqual(findOutlineNoneWithoutFocusRing(ok), [], `带 ${extra} 时不该报`);
  }
});

test("删掉 outline-none（改用全局焦点环）就通过", () => {
  const ok = `<template>
  <input class="w-full" />
  <div contenteditable="true" class="min-h-10"></div>
</template>`;
  assert.deepEqual(findOutlineNoneWithoutFocusRing(ok), []);
});

test("文件级逃生阀（`focus-ring-ok:file`）：菜单/对话框容器整文件跳过", () => {
  const withEscape = `<!-- focus-ring-ok:file -->
<template>
  <div role="menu" tabindex="-1" class="frost outline-none">…</div>
</template>`;
  assert.deepEqual(findOutlineNoneWithoutFocusRing(withEscape), []);
});

// ---------------- 元素级逃生阀（2026-09-16） ----------------
//
// 真实教训：`MessageComposer.vue` 因为「消息输入框不画焦点环」这**一个元素**的需求，
// 用了**文件级**逃生阀 ⇒ 整个文件（含其中的按钮等键盘可聚焦元素）一起失去本条保护；
// 更糟的是 `scripts/verify-guards.py` 里注入该文件的非空转用例退化成**空转**
// （改坏也不报），而项目里没有任何东西自动跑 verify-guards，所以一直没人发现。
// ⇒ 粒度必须能到元素，且元素级的豁免**绝不能**外溢到同文件的其它元素。

test("元素级逃生阀（`data-focus-ring-ok`）：豁免它自己那一个元素", () => {
  const src = `<template>
  <div data-focus-ring-ok class="frost outline-none">输入区</div>
</template>`;
  assert.deepEqual(findOutlineNoneWithoutFocusRing(src), []);
});

test("⚠️ 元素级逃生阀不得豁免同文件里的其它元素（粒度错的回归）", () => {
  // 这条同时钉住两个坑：
  //  ① 豁免不能外溢 —— 旁边的按钮没有被标记，必须照旧报出来；
  //  ② 令牌互不为子串 —— `data-focus-ring-ok` **含有** `focus-ring-ok` 子串，
  //     所以文件级判据不能还写成 `src.includes("focus-ring-ok")`，
  //     否则"只豁免一个元素"会被当成"整文件豁免"，这条断言会直接失败。
  const src = `<template>
  <div data-focus-ring-ok class="frost outline-none">输入区</div>
  <button class="h-8 w-8 outline-none"><X /></button>
</template>`;
  const issues = findOutlineNoneWithoutFocusRing(src);
  assert.equal(issues.length, 1, "被标记的那个元素豁免了，但旁边未标记的按钮必须报出来");
  assert.match(issues[0].message, /静默覆盖/);
});

test("注释里提到 outline-none 不算（只看真实 class 属性）", () => {
  const commentOnly = `<!-- 注意：不要在这里加 outline-none -->
<template>
  <input class="w-full" />
</template>`;
  assert.deepEqual(findOutlineNoneWithoutFocusRing(commentOnly), []);
});

// ---------------- ⑧ 小尺寸可交互元素必须有 tap-safe ----------------
//
// 真实情况（2026-09-12 复核）：35 个小尺寸可交互元素里仍有 8 处漏掉 `tap-safe` ——
// 包括移动端的返回键、删除「跨网段端点」的垃圾桶、自定义主题取色控件。
// 桌面鼠标点 28px 没问题，**手指点就容易不中或误触相邻项**（HIG 最小 44pt）。

test("复现真实缺陷：h-7 图标按钮没有 tap-safe → 报出", () => {
  const buggy = `<template>
  <button class="flex h-7 w-7 items-center justify-center" @click="remove()">
    <Trash2 />
  </button>
</template>`;
  const issues = findSmallTapTargets(buggy);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].line, 2);
  assert.match(issues[0].message, /44×44pt/);
});

test("加了 tap-safe 就通过", () => {
  const ok = `<template>
  <button class="tap-safe flex h-7 w-7 items-center justify-center" @click="remove()">
    <Trash2 />
  </button>
</template>`;
  assert.deepEqual(findSmallTapTargets(ok), []);
});

test("达标的尺寸不算小（h-11 = 44px）", () => {
  const ok = `<template>
  <button class="flex h-11 w-11 items-center justify-center" @click="ok()">x</button>
</template>`;
  assert.deepEqual(findSmallTapTargets(ok), []);
});

test("不可交互的小元素不报（纯装饰）", () => {
  const decorative = `<template>
  <div class="h-6 w-6 rounded-full bg-primary"></div>
  <span class="h-5 w-5"><Check /></span>
</template>`;
  assert.deepEqual(findSmallTapTargets(decorative), []);
});

test("带 @click 的非按钮小元素同样要 tap-safe", () => {
  const buggy = `<template>
  <div class="h-6 w-6 cursor-pointer" role="button" tabindex="0" @click="go()">x</div>
</template>`;
  assert.equal(findSmallTapTargets(buggy).length, 1);
});

test("tap-target-ok 逃生阀：整文件跳过", () => {
  const withEscape = `<!-- tap-target-ok -->
<template>
  <button class="h-6 w-6" @click="go()">x</button>
</template>`;
  assert.deepEqual(findSmallTapTargets(withEscape), []);
});

// ---------------- ⑨ as="template" 插槽不得有注释/多根节点 ----------------
//
// 真实事故（2026-09-12 用户实测）：点「+ → 添加好友」整个窗口卡死。根因是
// `BaseModal.vue` 在 `<TransitionChild as="template">` 的插槽里放了一条 HTML 注释：
// **dev 构建保留注释** ⇒ 插槽多出一个节点 ⇒ Headless UI 抛 "Passing props on template!"
// ⇒ Vue 渲染抛错后整个界面再也 patch 不动。生产构建会剥掉注释，所以只在 dev 复现。

test("复现真实缺陷：as=template 的插槽里有 HTML 注释 → 报出", () => {
  const buggy = `<template>
<TransitionRoot :show="open" as="template">
  <Dialog as="div">
    <TransitionChild as="template">
      <!-- 说明文字 -->
      <DialogPanel v-if="fullscreen">A</DialogPanel>
      <DialogPanel v-else>B</DialogPanel>
    </TransitionChild>
  </Dialog>
</TransitionRoot>
</template>`;
  const issues = findTemplateSlotIssues(buggy);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /HTML 注释/);
  assert.match(issues[0].message, /卡死/);
});

test("注释移到组件外面就通过", () => {
  const ok = `<template>
<!-- 说明文字放在外面 -->
<TransitionRoot :show="open" as="template">
  <Dialog as="div"><TransitionChild as="template"><div /></TransitionChild></Dialog>
</TransitionRoot>
</template>`;
  assert.deepEqual(findTemplateSlotIssues(ok), []);
});

test("插槽里多个顶层节点 → 报出（同样会抛 template 错误）", () => {
  const buggy = `<template>
<Foo as="template">
  <div>A</div>
  <div>B</div>
</Foo>
</template>`;
  const issues = findTemplateSlotIssues(buggy);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /顶层节点/);
});

test("单节点、嵌套里有注释都不算（只看直接插槽）", () => {
  const ok = `<template>
<Foo as="template">
  <div>
    <!-- 子元素内部的注释无妨 -->
    <span>x</span>
  </div>
</Foo>
</template>`;
  assert.deepEqual(findTemplateSlotIssues(ok), []);
});

test("v-if / v-else 链只算一个节点", () => {
  const ok = `<template>
<Foo as="template">
  <DialogPanel v-if="fullscreen">A</DialogPanel>
  <DialogPanel v-else>B</DialogPanel>
</Foo>
</template>`;
  assert.deepEqual(findTemplateSlotIssues(ok), []);
});

test("两个**独立**元素（没有 v-else 关系）仍然报出", () => {
  const buggy = `<template>
<Foo as="template">
  <div v-if="a">A</div>
  <div>B</div>
</Foo>
</template>`;
  assert.equal(findTemplateSlotIssues(buggy).length, 1);
});

test("普通组件（没有 as=template）里有注释不报", () => {
  const ok = `<template>
<div>
  <!-- 随便注释 -->
  <span>x</span>
</div>
</template>`;
  assert.deepEqual(findTemplateSlotIssues(ok), []);
});

// ---------------- 全库扫描：真实文件必须干净 ----------------

function collectVueFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) collectVueFiles(full, out);
    else if (entry.name.endsWith(".vue")) out.push(full);
  }
  return out;
}

test("src 下所有小尺寸可交互元素都带 tap-safe", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(srcDir);
  assert.ok(files.length > 20, `应扫描到全部组件，实际 ${files.length} 个`);
  const bad: string[] = [];
  for (const f of files) {
    for (const issue of findSmallTapTargets(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `以下元素点按目标过小：\n${bad.join("\n")}`);
});

test("src 下所有 as=template 的插槽都干净（无注释、单节点）", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(srcDir);
  assert.ok(files.length > 20, `应扫描到全部组件，实际 ${files.length} 个`);
  const bad: string[] = [];
  for (const f of files) {
    for (const issue of findTemplateSlotIssues(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `以下 as="template" 插槽有问题：\n${bad.join("\n")}`);
});

test("src 下所有 outline-none 都自带焦点指示", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(srcDir);
  assert.ok(files.length > 20, `应扫描到全部组件，实际 ${files.length} 个`);
  const bad: string[] = [];
  for (const f of files) {
    for (const issue of findOutlineNoneWithoutFocusRing(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `以下元素关掉了焦点指示却没有替代：\n${bad.join("\n")}`);
});

test("src 下所有可点击元素都能用键盘触发", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(srcDir);
  assert.ok(files.length > 20, `应扫描到全部组件，实际 ${files.length} 个`);
  const bad: string[] = [];
  for (const f of files) {
    for (const issue of findTappableWithoutKeyboard(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `以下元素能点但键盘够不着：\n${bad.join("\n")}`);
});

test("src 下所有 .vue 的悬停揭示都带了触屏兜底", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(srcDir);
  assert.ok(files.length > 20, `应扫描到全部组件，实际 ${files.length} 个`);
  const bad: string[] = [];
  for (const f of files) {
    for (const issue of findHoverRevealIssues(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `发现缺少触屏兜底的悬停揭示：\n${bad.join("\n")}`);
});

test("src 下所有 .vue 的截断文本都带了 title / aria-label", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(srcDir);
  const bad: string[] = [];
  for (const f of files) {
    for (const issue of findTruncationWithoutTitle(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `发现被截断却没有 title 的文本：\n${bad.join("\n")}`);
});

test("文本气泡排版与虚拟列表高度度量一致（leading-* ↔ TEXT_LINE_RATIO / py-* ↔ TEXT_BUBBLE_PADDING）", () => {
  const srcDir = join(import.meta.dirname, "..");
  const bubble = readFileSync(join(srcDir, "components", "message", "MessageTextBubble.vue"), "utf8");
  const metrics = readFileSync(join(srcDir, "utils", "previewMetrics.ts"), "utf8");
  const issues = checkBubbleMetricsCoupling(bubble, metrics);
  assert.deepEqual(issues, [], issues.map((i) => `L${i.line} ${i.message}`).join("\n"));
});

test("触屏命中扩展类直接声明 position → 报出（会盖掉组件的 absolute）", () => {
  // 复现真实缺陷：安卓端「回到最新」按钮是 `tap-safe absolute bottom-4 right-5`，
  // 而 .tap-safe{position:relative} 与 .absolute 特异性相同、本文件更靠后 ⇒ 定位被覆盖。
  const buggy = `
@tailwind utilities;
@media (pointer: coarse) {
  .tap-safe { position: relative; }
  .tap-safe::after { content: ""; position: absolute; inset: -8px 0; }
}
`;
  const issues = checkStyleCascade(buggy);
  assert.ok(
    issues.some((i) => i.message.includes("pointer: coarse")),
    `应当报出触屏块里的 position 覆盖，实际：${JSON.stringify(issues)}`,
  );
});

test("用 :where() 压到 0 特异性 → 通过", () => {
  const fixed = `
@tailwind utilities;
@media (pointer: coarse) {
  :where(.tap-safe) { position: relative; }
  :where(.tap-safe)::after { content: ""; position: absolute; inset: -8px 0; }
}
`;
  const issues = checkStyleCascade(fixed).filter((i) => i.message.includes("pointer: coarse"));
  assert.deepEqual(issues, [], issues.map((i) => i.message).join("\n"));
});

test("真实的 src/style.css 级联顺序正确", () => {
  const css = readFileSync(join(import.meta.dirname, "..", "style.css"), "utf8");
  const issues = checkStyleCascade(css);
  assert.deepEqual(issues, [], issues.map((i) => `L${i.line} ${i.message}`).join("\n"));
});

test("未读徽标只有 UnreadBadge.vue 一处实现，且保留了垂直居中补偿", () => {
  const srcDir = join(import.meta.dirname, "..");
  const badgePath = join(srcDir, "components", "UnreadBadge.vue");
  const files = collectVueFiles(srcDir);

  const bad: string[] = [];
  for (const f of files) {
    if (f === badgePath) continue;
    for (const issue of findHandWrittenBadges(readFileSync(f, "utf8"))) {
      bad.push(`${f.replace(srcDir + "/", "")}:${issue.line}  ${issue.message}`);
    }
  }
  assert.deepEqual(bad, [], `发现有手写的未读徽标：\n${bad.join("\n")}`);

  const issues = checkUnreadBadgeComponent(readFileSync(badgePath, "utf8"));
  assert.deepEqual(issues, [], issues.map((i) => i.message).join("\n"));
});

// ---------------- ⑧ 聊天区「文本选择」契约 ----------------

test("复现历史缺陷：气泡根不可选 / 表情可拖 → 报出", () => {
  const buggy = {
    textBubble: `
      <div class="group relative px-3 py-1.5">
        <div class="whitespace-pre-wrap">{{ body }}</div>
        <img :src="emoji" class="emoji-img" />
      </div>`,
    avatar: `<img :src="avatar" class="h-full w-full object-cover" />`,
    messageItem: `<div @touchmove="cancelLongPress"></div>`,
    app: `window.addEventListener("contextmenu", (e) => e.preventDefault());`,
    // 旧写法：把截断后的字符串当 DOM 文本渲染 ⇒ 选中复制拿到残缺 URL（用户 2026-09-16 报的缺陷）
    linkText: `<a :href="href">{{ displayUrl(label) }}</a>`,
    css: `
.gosslan-avatar-box { container-type: inline-size; }
.emoji-img { display: inline-block; }
.gosslan-selectable { user-select: text; }
.gosslan-url-mid { display: none; }
.gosslan-url-dots { }`,
  };
  const issues = checkSelectionContract(buggy);
  const msgs = issues.map((i) => i.message).join("\n");
  assert.ok(msgs.includes("gosslan-selectable"), msgs);
  assert.ok(msgs.includes("select-text"), msgs);
  assert.ok(msgs.includes("emoji-img"), msgs);
  assert.ok(msgs.includes("user-select: none"), msgs);
  assert.ok(msgs.includes("onTouchMove"), msgs);
  assert.ok(msgs.includes("contextmenu"), msgs);
  assert.ok(msgs.includes("font-size: 0"), msgs);
  assert.ok(msgs.includes("background-image"), msgs);
  assert.ok(msgs.includes("MessageLinkText"), msgs);
  assert.ok(msgs.includes("quote-block"), msgs);
});

test("引用块可选性：漏掉任一半边 → 报出", () => {
  const base = {
    textBubble: `<div class="gosslan-bubble-text select-text"><div class="gosslan-selectable">
      <img class="emoji-img" draggable="false" /></div></div>`,
    avatar: `<img class="h-full w-full object-cover" draggable="false" />`,
    messageItem: `<div @touchmove="onTouchMove"></div>`,
    app: `const sel = window.getSelection();`,
    linkText: `<a><span>{{ parts.head }}</span><span class="gosslan-url-mid">{{ parts.mid }}</span><span class="gosslan-url-dots"></span><span>{{ parts.tail }}</span></a>`,
  };
  const common = `
.gosslan-avatar-box { user-select: none; }
.emoji-img { -webkit-user-drag: none; }
.gosslan-selectable { -webkit-touch-callout: default; }
.gosslan-url-mid { font-size: 0; }
.gosslan-url-dots { background-image: radial-gradient(circle, red, blue); }`;

  // 半边 A：只有 .quote-block 可选中，触屏那条缺失 ⇒ 引用块在手机上永远可拖选
  const noTouchGuard = checkSelectionContract({
    ...base,
    css: `${common}\n.quote-block { user-select: text; }`,
  });
  assert.match(noTouchGuard.map((i) => i.message).join("\n"), /弹不出消息菜单/);

  // 半边 B：只有触屏禁用，没把 <button> 的 none 覆盖回来 ⇒ 全选复制丢引用头
  const noSelectable = checkSelectionContract({
    ...base,
    css: `${common}\n.gosslan-bubble-text:not(.gosslan-selecting) .quote-block { user-select: none; }`,
  });
  assert.match(noSelectable.map((i) => i.message).join("\n"), /缺少 `user-select: text`/);
});

test("链接省略号的 DOM 顺序被打乱 / 夹了空白 → 报出", () => {
  const base = {
    textBubble: `<div class="gosslan-bubble-text select-text"><div class="gosslan-selectable">
      <img class="emoji-img" draggable="false" /></div></div>`,
    avatar: `<img class="h-full w-full object-cover" draggable="false" />`,
    messageItem: `<div @touchmove="onTouchMove"></div>`,
    app: `const sel = window.getSelection();`,
    css: `
.gosslan-avatar-box { user-select: none; }
.emoji-img { -webkit-user-drag: none; }
.gosslan-selectable { -webkit-touch-callout: default; }
.gosslan-url-mid { font-size: 0; }
.gosslan-url-dots { background-image: radial-gradient(circle, currentColor 1px, transparent 1px); }
.quote-block { user-select: text; }
.gosslan-bubble-text:not(.gosslan-selecting) .quote-block { user-select: none; }`,
  };
  // mid 跑到 tail 后面：拼回去就成了 head+tail+mid，URL 顺序错乱
  const swapped = checkSelectionContract({
    ...base,
    linkText: `<a><span>{{ parts.head }}</span><span>{{ parts.tail }}</span><span class="gosslan-url-dots"></span><span class="gosslan-url-mid">{{ parts.mid }}</span></a>`,
  });
  assert.match(swapped.map((i) => i.message).join("\n"), /DOM 顺序被打乱/);

  // span 之间夹了换行：空白文本节点会进选区，复制出的 URL 中间多空格
  const spaced = checkSelectionContract({
    ...base,
    linkText: `<a>
      <span>{{ parts.head }}</span>
      <span class="gosslan-url-mid">{{ parts.mid }}</span>
      <span class="gosslan-url-dots"></span>
      <span>{{ parts.tail }}</span>
    </a>`,
  });
  assert.match(spaced.map((i) => i.message).join("\n"), /夹了空白/);

  // 省略号里写了文本
  const dotted = checkSelectionContract({
    ...base,
    linkText: `<a><span>{{ parts.head }}</span><span class="gosslan-url-mid">{{ parts.mid }}</span><span class="gosslan-url-dots">…</span><span>{{ parts.tail }}</span></a>`,
  });
  assert.match(dotted.map((i) => i.message).join("\n"), /省略号 span 里带了文本/);
});

test("修好之后通过（真实的 6 个源码文件）", () => {
  const srcDir = join(import.meta.dirname, "..");
  const issues = checkSelectionContract({
    textBubble: readFileSync(join(srcDir, "components", "message", "MessageTextBubble.vue"), "utf8"),
    avatar: readFileSync(join(srcDir, "components", "message", "MessageAvatar.vue"), "utf8"),
    messageItem: readFileSync(join(srcDir, "components", "MessageItem.vue"), "utf8"),
    app: readFileSync(join(srcDir, "..", "src", "App.vue"), "utf8"),
    linkText: readFileSync(join(srcDir, "components", "message", "MessageLinkText.vue"), "utf8"),
    css: readFileSync(join(srcDir, "style.css"), "utf8"),
  });
  assert.deepEqual(issues, [], issues.map((i) => `L${i.line} ${i.message}`).join("\n"));
});

// ---------------- ⑨ 文本输入类的焦点提示：不许画外圈方框 ----------------
//
// 真实反馈（用户 2026-09-16）：「整个应用的输入框在焦点态会默认有个主题色的方框，很难看」。
// 根因见 `checkTextFieldFocusRing` —— 浏览器对 input/textarea/select/[contenteditable]
// 把 `:focus-visible` 判成**恒成立**，所以全局那条 2px 外圈方框会在**每次点击输入框**时出现。
// 这条护栏盯的是"改回去"：全局环里再加回输入类、或删掉替代的边线提示。

const BUGGY_FOCUS_CSS = `
:where(button, a, input, textarea, select, [tabindex], [contenteditable]):focus-visible {
  outline: 2px solid var(--gosslan-focus-ring);
  outline-offset: 1px;
}
`;

const FIXED_FOCUS_CSS = `
:where(button, a, [tabindex]):focus-visible {
  outline: 2px solid var(--gosslan-focus-ring);
  outline-offset: 1px;
}
input:focus,
textarea:focus,
select:focus {
  border-color: var(--gosslan-primary);
  box-shadow: inset 0 0 0 1px var(--gosslan-focus-ring);
}
.gosslan-composer:focus-within {
  border-color: var(--gosslan-primary);
}
`;

test("复现真实反馈：全局焦点环含文本输入类 → 报出（并指到那一行）", () => {
  // 这个夹具只有"改坏"的那一条规则（故意不带替代提示），所以只断言**方框**这条判据。
  const issues = checkTextFieldFocusRing(BUGGY_FOCUS_CSS, "gosslan-composer");
  const rings = issues.filter((i) => /选择器里不该有文本输入类/.test(i.message));
  assert.equal(rings.length, 1, "全局环含文本输入类必须报一次");
  assert.match(rings[0].message, /点一下/, "要说清「点一下就出现」这个关键点");
  assert.equal(rings[0].line, 2, "应指到那条 :focus-visible 规则所在行");
});

test("修好之后不再报（边线级提示 + 卡片钩子都在）", () => {
  assert.deepEqual(checkTextFieldFocusRing(FIXED_FOCUS_CSS, "gosslan-composer"), []);
});

test("删掉替代提示就报（去掉外圈方框≠可以没有焦点提示）", () => {
  const noFieldRule = FIXED_FOCUS_CSS.replace(/input:focus,[\s\S]*?\}\n/, "");
  const issues = checkTextFieldFocusRing(noFieldRule, "gosslan-composer");
  assert.ok(
    issues.some((i) => /找不到文本输入类的焦点提示规则/.test(i.message)),
    "少了边线级提示必须报（WCAG 2.4.7）",
  );
});

test("消息输入框卡片钩子被摘掉也报（编辑区自己不能画框）", () => {
  const issues = checkTextFieldFocusRing(FIXED_FOCUS_CSS, "relative rounded-md border");
  assert.ok(
    issues.some((i) => /gosslan-composer/.test(i.message)),
    "卡片少了钩子 ⇒ 消息输入框完全没有焦点提示，必须报",
  );
});

test("真实的 style.css + MessageComposer.vue 通过", () => {
  const srcDir = join(import.meta.dirname, "..");
  const issues = checkTextFieldFocusRing(
    readFileSync(join(srcDir, "style.css"), "utf8"),
    readFileSync(join(srcDir, "components", "chat", "MessageComposer.vue"), "utf8"),
  );
  assert.deepEqual(issues, [], issues.map((i) => `L${i.line} ${i.message}`).join("\n"));
});

// ---------------- ⑫ 浮层 z 序阶梯：弹窗必须盖住整页下钻页，但不能盖住菜单/预览/toast ----------------
//
// 真实缺陷（用户 2026-09-21，**只在移动端**）：`BaseModal` 原先写 `z-50`，而 HeadlessUI 的
// `Dialog` 会把自己挂到 `<body>` 下的 `#headlessui-portal-root`（即 z 是在**文档根层级**上比的，
// 不是留在调用者子树里比的）。移动端的整页下钻页是 `MobilePageFrame` 的 `fixed inset-0 z-[60]`
// ⇒ z-50 的弹窗被整页盖在后面：**DOM 里有、屏幕上看不见**，用户看到的就是「点了没反应」。
// 这类退化没有任何运行时报错、桌面上也完全正常，只能静态钉住阶梯。
test("浮层 z 序阶梯：整页框架 < 弹窗 < 右键菜单 < 图片预览、Toast", () => {
  const srcDir = join(import.meta.dirname, "..");
  /** 取该文件里**第一个** `z-[NN]`。注释里为解释阶梯写着一串 z-[NN]，必须先剥掉两种注释。 */
  const zOf = (rel: string) => {
    const src = readFileSync(join(srcDir, rel), "utf8")
      .replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "");
    const m = /z-\[(\d+)\]/.exec(src);
    assert.ok(m, `${rel} 里找不到 z-[NN]（浮层必须显式声明层级）`);
    const n = Number(m[1]);
    assert.ok(n > 0 && n < 100, `${rel} 的 z 值 ${n} 不像层级值`);
    return n;
  };
  const frame = zOf("components/MobilePageFrame.vue");
  const modal = zOf("components/BaseModal.vue");
  const menu = zOf("components/ContextMenu.vue");
  const preview = zOf("components/message/ImageLightbox.vue");
  const toast = zOf("components/ToastHud.vue");

  assert.ok(
    frame < modal,
    `弹窗(z-${modal}) 必须高于整页框架(z-${frame})：否则移动端「设置/日志/收藏…」里打开的弹窗会被整页盖住（用户看到"点了没反应"）`,
  );
  assert.ok(modal < menu, `右键菜单(z-${menu}) 应高于弹窗(z-${modal})，否则菜单会被弹窗盖住`);
  assert.ok(
    modal < preview,
    `图片预览(z-${preview}) 应高于弹窗(z-${modal})：合并转发卡等弹窗里点图要看大图`,
  );
  assert.ok(
    modal < toast,
    `Toast(z-${toast}) 必须高于弹窗(z-${modal})：弹窗里的失败提示要看得见`,
  );
});

// ---------------- ⑬ 可选中导航项的图标必须有「选中 = 实心」的 fill 绑定 ----------------
//
// 真实缺陷（用户 2026-09-21）：「链接这个指南针为什么选中的时候不是实心选中变色的」——
// 这一栏的选中态**不用底色块，而是把图标本身填成实心**（用户 2026-09-12 定的：参考微信）。
// 聊天 / 通讯录 / 收藏 都是自绘 SVG + `:fill="navState === 'x' ? 'currentColor' : 'none'"`
// （lucide 那种描边图标 fill 之后会变成墨团，所以必须自绘），唯独「链接」当时还是
// `<Compass />` 原样 ⇒ 选中只变颜色不变实心。这类"漏一个"在界面上很不起眼，只能静态钉住。
test("可选中导航项：图标必须有 :fill 的选中绑定（漏一个就只变颜色不变实心）", () => {
  const srcDir = join(import.meta.dirname, "..");
  const nav = readFileSync(join(srcDir, "components", "NavRail.vue"), "utf8");
  // 与实际实现同步的清单：新增可选中项时**必须**在这里补一行（漏了会被下面的断言拦下）
  const selectable = ["chats", "contacts", "favorites", "links"];
  for (const key of selectable) {
    assert.ok(
      nav.includes(`:fill="navState === '${key}' ? 'currentColor' : 'none'"`),
      `NavRail 的「${key}」缺少实心选中态：要自绘 SVG 并绑上 :fill="navState === '${key}' ? 'currentColor' : 'none'"`,
    );
  }
  // 反向：这四项之外不应再有别的 `navState === '<key>'` 图标高亮（否则清单就过期了）
  const keys = [...nav.matchAll(/navState === '([a-zA-Z]+)'/g)].map((m) => m[1]);
  const unknown = [...new Set(keys)].filter((k) => !selectable.includes(k));
  assert.deepEqual(
    unknown,
    [],
    `NavRail 里出现了清单外的可选中 key：${unknown.join(", ")} —— 请同步上面的 selectable 清单`,
  );
});

// ---------------- ⑭ 预览 objectURL 归缓存所有：消费者不得 revoke ----------------
//
// 真实缺陷（用户 2026-09-21）：「任务详情里的图片大概率加载失败」。
// 根因不在"读不到字节"，而在**生命周期**：`loadContentPreview` 把 objectURL 缓存在模块级
// Map 里，同一个 cid 的 URL 是**大家共用的同一个字符串**（一张待办描述图会同时存在于
// 聊天时间线的任务卡、看板表单、任务详情）。`TodoImageThumb` 当时在卸载/换图时
// `revokeObjectURL`，于是"关掉一次详情弹窗""虚拟列表回收一行"都会把所有其它视图的图
// 一起打回裂图；而缓存里那个 URL 已经死了却仍被命中 ⇒ 退避重试也救不回来（只能重启）。
// 判据：拿预览缓存 URL 的消费组件**一律不许出现 revokeObjectURL**（自己造 URL 的组件
// 如 MergeCardModal / ProfileSection 不在此列，它们的 URL 没进任何共享缓存）。
test("预览缓存 URL 的消费者不得 revokeObjectURL（会把同一张图的其它视图一起打裂）", () => {
  const srcDir = join(import.meta.dirname, "..");
  const consumers = [
    ["components", "TodoImageThumb.vue"],
    ["components", "message", "MessageImageBubble.vue"],
    ["components", "message", "ImageLightbox.vue"],
    ["components", "FavoritePanel.vue"],
  ];
  for (const rel of consumers) {
    const file = join(srcDir, ...rel);
    const src = readFileSync(file, "utf8");
    // 只认**调用**（`URL.revokeObjectURL(...)`）：注释里提到这个名字是允许的（本文件的说明就是要留痕）
    assert.ok(
      !/URL\.revokeObjectURL\s*\(/.test(src),
      `${rel.join("/")} 里出现了 revokeObjectURL：预览 URL 归 filePreview/favoritePreview 的缓存所有，` +
        `消费者 revoke 会连带杀掉同一 cid/msg_id 在别处的图，且缓存会一直返回那个死 URL`,
    );
  }
});

// ---------------- ⑮ 滚动落定的轮询定时器必须在卸载时清掉 ----------------
//
// 真实缺陷（PR #24 审查发现）：`scrollToIndex` 会开一个 1.5s 落定窗口，里面用 `setInterval`
// 每 100ms 校正一次（`pollJump`）。窗口的**自动收口只发生在 `applyJump` 内部**，而它开头就是
// `if (!j || !el) return;` —— `el` 是容器 ref，组件卸载后 Vue 会把它置 null，于是
// "过窗就 clearInterval"那一支永远走不到 ⇒ 定时器以 10Hz 常驻在一个已经死掉的组件上。
// 只要"关掉带列表的辅助窗口 / 切会话"正好落在落定窗口里就漏一条，且不会自愈。
// 判据：`onBeforeUnmount` 里必须**显式**清 `jumpTimer`（将来把落定逻辑改成别的形态时，
// 只要还留着一个跨窗口的定时器，这条就得跟着改，不能默默失效）。
test("VirtualList：落定轮询定时器必须在 onBeforeUnmount 里清理", () => {
  const src = readFileSync(join(import.meta.dirname, "..", "components", "VirtualList.vue"), "utf8");
  const start = src.indexOf("onBeforeUnmount(() => {");
  assert.notEqual(start, -1, "VirtualList 的卸载钩子不见了 ⇒ 这条判据失去锚点");
  const end = src.indexOf("\n});", start);
  assert.notEqual(end, -1, "VirtualList 的卸载钩子没有正常闭合");
  const cleanup = src.slice(start, end);
  assert.ok(
    cleanup.includes("clearInterval(jumpTimer)"),
    "onBeforeUnmount 没清 jumpTimer：组件在落定窗口内卸载后，100ms 轮询不会停" +
      "（applyJump 在 `!el` 处就返回了，走不到过窗自清那一支）",
  );
});

/**
 * 图片预览必须**只有一个渲染点**（用户 2026-09-24 #40：看图是一个公共能力）。
 *
 * 原先 `ImageLightbox` 被四处各挂一份（会话 / 任务看板 / 任务详情 / 合并转发卡片），
 * 每份自己持有 `images/index/open`。后果不是重复代码那么简单：
 * - 同一件"看图"在不同入口的能力取决于那个面板有没有把数组传全（会话里能整屏左右循环、
 *   任务里只有那一条的几张图，这本该是同一个契约）；
 * - 加一个入口就得再抄一遍，而"合并卡片的图是 objectURL、组件一关就被回收"这份知识
 *   锁在那个组件里 —— 预览搬到全局后正是靠**来源标记**才没把这条保证弄丢。
 *
 * 所以钉两件事：渲染点只有一个（在壳层），别的组件只能经 store 调用。
 */
test("图片预览只有一处渲染点，其余组件一律走 useImagePreviewStore", () => {
  const srcDir = join(import.meta.dirname, "..");
  const files = collectVueFiles(join(srcDir, "components")).concat(
    collectVueFiles(join(srcDir, "layouts")),
  );
  const renderers: string[] = [];
  const stateful: string[] = [];
  for (const f of files) {
    const src = readFileSync(f, "utf8");
    const rel = f.replace(srcDir + "/", "");
    // 只数**模板段里、去掉 HTML 注释之后**的元素。
    // 为什么不是"看行首是不是 `<!--`"：注释常常是多行的，提到 `<ImageLightbox>`
    // 的那一行本身不带 `<!--`（第一版就是这么写的，结果三处注释全被当成渲染点）。
    // 模板里的注释不可能嵌套 ⇒ `<!--[\s\S]*?-->` 这个非贪婪匹配就是精确的。
    const tplAt = src.indexOf("<template>");
    if (tplAt > 0) {
      const tpl = src.slice(tplAt).replace(/<!--[\s\S]*?-->/g, "");
      for (const _m of tpl.matchAll(/<ImageLightbox[\s/>]/g)) {
        // 只报文件不报行号：注释被剥掉之后偏移就变了，报出来的行号会是**错的**，
        // 宁可少给一个信息也不要给假的（要定位在文件里搜 `<ImageLightbox` 即可）。
        renderers.push(rel);
      }
    }
    // 别人不许再自己攒一套 open/index 状态（那等于又长出第二个预览实例）
    if (
      rel !== "layouts/ResponsiveLayout.vue" &&
      /const\s+\w*lightbox\w*\s*=\s*ref\(/i.test(src)
    ) {
      stateful.push(rel);
    }
  }
  // 允许**两个文档各一个实例**：主窗口的覆盖层（移动端 + 不可跨文档时的退回路径）
  // 与桌面端那个全局唯一的预览窗口。"全局只有一个预览"说的是**同一时刻用户看到的界面**，
  // 而这两处不会同时亮：store 的 `inWindow` 为真时壳层那份就让位（ResponsiveLayout 的 :open）。
  assert.deepEqual(
    [...new Set(renderers)].sort(),
    ["components/window/PreviewWindow.vue", "layouts/ResponsiveLayout.vue"],
    `图片预览的渲染点不对：${renderers.join(", ")} —— 只许壳层覆盖层 + 预览窗口这两处`,
  );
  assert.deepEqual(stateful, [], `这些组件还自己持有预览开关状态：${stateful.join(", ")}`);
  // store 侧：唯一那份实例绑在 store 上，不是本地 ref
  const shell = readFileSync(join(srcDir, "layouts", "ResponsiveLayout.vue"), "utf8");
  assert.match(shell, /const preview = useImagePreviewStore\(\)/, "壳层要挂 store");
  assert.match(shell, /:images="preview\.images"/, "渲染点的数据来自 store");
  // 左右切换写的也是 store 的 index（写成组件本地 ref 就等于把"当前看第几张"又搬回局部）
  assert.match(shell, /v-model:index="preview\.index"/, "切换下标也写回 store");
});
