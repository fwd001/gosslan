import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import {
  checkBubbleMetricsCoupling,
  checkStyleCascade,
  checkUnreadBadgeComponent,
  findHandWrittenBadges,
  findHoverRevealIssues,
  findOutlineNoneWithoutFocusRing,
  findTappableWithoutKeyboard,
  findTruncationWithoutTitle,
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

test("focus-ring-ok 逃生阀：菜单/对话框容器整文件跳过", () => {
  const withEscape = `<!-- focus-ring-ok -->
<template>
  <div role="menu" tabindex="-1" class="frost outline-none">…</div>
</template>`;
  assert.deepEqual(findOutlineNoneWithoutFocusRing(withEscape), []);
});

test("注释里提到 outline-none 不算（只看真实 class 属性）", () => {
  const commentOnly = `<!-- 注意：不要在这里加 outline-none -->
<template>
  <input class="w-full" />
</template>`;
  assert.deepEqual(findOutlineNoneWithoutFocusRing(commentOnly), []);
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
