import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * 图片预览的**宿主契约**（用户 2026-09-29 需求汇总第六条：独立窗口调用时隐藏组件内部的关闭按钮、
 * 同一窗口内可以显示；并修「从任务列表进入任务后查看图片可能打不开」）。
 *
 * 为什么是源码形状判据：这几条都住在"谁宿主这份看图器"的接线里，而单测环境没有 WebView
 * （开不了第二扇窗口，也造不出"预览窗口建不出来"那种失败）。形状判据挡的是以后有人把它改回去。
 *
 * ⚠️ 每条取反判据都配了一条正面判据（同一份源码里必须存在什么），否则"把两处一起删掉"也算通过。
 */
const ROOT = join(import.meta.dirname, "..");
const read = (rel: string) => readFileSync(join(ROOT, rel), "utf8");
const LIGHTBOX = read("components/message/ImageLightbox.vue");
const WINDOW = read("components/window/PreviewWindow.vue");
const SHELL = read("layouts/ResponsiveLayout.vue");
const THUMB = read("components/TodoImageThumb.vue");
const STORE = read("stores/useImagePreview.ts");

test("内部那颗 ✕ 由宿主决定画不画（独立窗口不重复、覆盖层仍要画）", () => {
  // 反面：不许再出现"无条件渲染关闭按钮"
  assert.match(LIGHTBOX, /<button\s+v-if="!hideClose"[\s\S]{0,400}?@click\.stop="emit\('close'\)"/);
  assert.equal(
    /<button(?![\s\S]{0,20}v-if)[\s\S]{0,300}?emit\('close'\)/.test(LIGHTBOX.split("<template>")[1]),
    false,
    "看图器里出现了一颗没有开关的关闭按钮 ⇒ 预览窗口又会有两把叉",
  );
  // 正面：两个宿主各自的选择都必须存在
  assert.match(WINDOW, /\n      hide-close/, "独立预览窗口没关内部 ✕ ⇒ 与标题栏那把重复（用户点名的就是这处）");
  assert.equal(SHELL.includes("hide-close"), false, "主窗口那份覆盖层不该关掉 ✕：那是它唯一的可见关闭出口");
});

/**
 * 「点了没反应」的真根因（本轮读出来的，不是猜的）：缩略图与大图是在**两个文档**里各读一次的
 * （主窗口这份 objectURL 缓存 vs 独立预览窗口那份），所以"字节其实取得了、只有这里的读取
 * 还没成功"是一个会稳定出现的状态。旧写法把可点性与 `url` 绑死 ⇒ 那一格点击完全静默。
 */
test("任务缩略图的可点性不再依赖缩略图是否加载成功", () => {
  assert.match(THUMB, /if \(props\.clickable\) emit\("open"\);/, "open() 不再无条件 emit ⇒ 打不开那条回来了");
  assert.equal(THUMB.includes("props.clickable && url.value"), false, "可点性仍被 url 绑住");
  // 正面：模板里的 role/tabindex 也得跟着只看 clickable，否则键盘走不到那一下
  assert.match(THUMB, /:role="clickable \? 'button' : undefined"/);
  assert.match(THUMB, /:tabindex="clickable \? 0 : undefined"/);
});

/**
 * 投递被拒时，**没有覆盖层的文档**必须收起状态并说一声。
 *
 * 为什么这不是"体验小改善"而是缺陷：主窗口退回覆盖层就行，而独立群任务窗口根本没有覆盖层
 * （全应用只允许一份 `ImageLightbox`）。留着 `open = true` 的话，任务详情会以为"预览正开着"，
 * 下一次点遮罩只吃掉"关预览" ⇒ 一个从没显示出来的预览把弹窗锁成关不掉。
 */
test("投递被拒：没有覆盖层的文档要收起状态并明确报错", () => {
  assert.match(STORE, /if \(!overlayMounted\.value\) \{\s*\n\s*close\(\);/, "拒绝分支没收起状态 ⇒ 弹窗会被一个看不见的预览锁住");
  assert.match(STORE, /app\.toast\(t\("preview\.unavailable"\), "error"\)/, "拒绝分支没有面向用户的说法");
  assert.match(STORE, /    overlayMounted,/, "标记没导出 ⇒ 宿主设了也没人读得到");
  // 正面：覆盖层的宿主确实把它置真（否则上面那两条永远走"报错"那一支，主窗口反而变成误报）
  assert.match(SHELL, /preview\.overlayMounted = true;/, "ResponsiveLayout 没认领自己是覆盖层宿主");
});
