import { test } from "node:test";
import assert from "node:assert/strict";
import { escHtml, highlightText } from "./highlight.ts";

test("escHtml 转义 HTML 元字符（XSS 第一道防线）", () => {
  assert.equal(escHtml(`<img src=x onerror="alert(1)">`), "&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  assert.equal(escHtml("a & b"), "a &amp; b");
});

test("无关键词 → 只转义，不加任何标记", () => {
  assert.equal(highlightText("plain text", ""), "plain text");
  assert.equal(highlightText("<b>hi</b>", ""), "&lt;b&gt;hi&lt;/b&gt;");
});

test("命中处包 <mark>（日志过滤的「颜色标识」就靠它）", () => {
  const out = highlightText("mesh +conn peer=abc", "conn");
  assert.match(out, /<mark[^>]*>conn<\/mark>/);
  assert.ok(out.startsWith("mesh +"), "未命中部分原样保留");
});

test("大小写不敏感，且**每一处**都标记（不是只标第一处）", () => {
  const out = highlightText("ERROR error Error", "error");
  assert.equal(out.match(/<mark/g)?.length, 3);
});

test("正则元字符按字面处理（关键词不会被当成模式）", () => {
  // `.` 若被当正则会匹配任意字符 → 「1x2」也会命中；这里必须不命中
  assert.equal(highlightText("1x2", "1.2").includes("<mark"), false);
  assert.equal(highlightText("1.2", "1.2").includes("<mark"), true);
  // `[mesh]` 若被当字符集 → 「m」也会命中
  assert.equal(highlightText("mmm", "[mesh]").includes("<mark"), false);
  // 半开括号不能让正则构造抛错
  assert.doesNotThrow(() => highlightText("a(b", "("));
});

test("文本里的 HTML 先被转义，再标记 —— 不产生可执行标记", () => {
  const out = highlightText("<script>x</script>", "script");
  assert.ok(out.includes("<mark"), "关键词仍应被标记");
  assert.ok(!out.includes("<script"), "原始标签不得原样出现在结果里");
  assert.ok(!out.includes("</script"), "闭合标签同样不得原样出现");
});
