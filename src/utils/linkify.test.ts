import { test } from "node:test";
import assert from "node:assert/strict";
import { linkify, splitUrl } from "./linkify.ts";

test("linkify: 无 URL 时整段是 text", () => {
  const segs = linkify("hello world");
  assert.equal(segs.length, 1);
  assert.equal(segs[0].kind, "text");
  assert.equal(segs[0].value, "hello world");
});

test("linkify: 匹配单个 http/https URL", () => {
  assert.deepEqual(linkify("看 https://a.com"), [
    { kind: "text", value: "看 " },
    { kind: "link", value: "https://a.com", href: "https://a.com" },
  ]);
  assert.deepEqual(linkify("http://b.io/path"), [
    { kind: "link", value: "http://b.io/path", href: "http://b.io/path" },
  ]);
});

test("linkify: 剥尾随标点当文本", () => {
  // 句末 . , ; : ! ? ) ] } > 不算 URL 一部分，剥出来当普通文本。
  // 尾标点和后续文本会分成两段（功能等价，连在一起渲染视觉无差）。
  assert.deepEqual(linkify("看 https://a.com, 还有 b"), [
    { kind: "text", value: "看 " },
    { kind: "link", value: "https://a.com", href: "https://a.com" },
    { kind: "text", value: "," },
    { kind: "text", value: " 还有 b" },
  ]);
  assert.deepEqual(linkify("https://a.com."), [
    { kind: "link", value: "https://a.com", href: "https://a.com" },
    { kind: "text", value: "." },
  ]);
  assert.deepEqual(linkify("https://a.com)"), [
    { kind: "link", value: "https://a.com", href: "https://a.com" },
    { kind: "text", value: ")" },
  ]);
});

test("linkify: 中文句读终止链接（不吞掉后半句）", () => {
  // 用户场景：中文里最常见的「句子中间放链接」——句号/逗号后必须断开，
  // 否则整句都进链接，点击打开的是 "https://a.com。然后呢" 这种不存在的地址。
  assert.deepEqual(linkify("看 https://a.com。然后呢"), [
    { kind: "text", value: "看 " },
    { kind: "link", value: "https://a.com", href: "https://a.com" },
    { kind: "text", value: "。然后呢" },
  ]);
  assert.deepEqual(linkify("结束 https://a.com，还有"), [
    { kind: "text", value: "结束 " },
    { kind: "link", value: "https://a.com", href: "https://a.com" },
    { kind: "text", value: "，还有" },
  ]);
  // 全角括号同理：右括号不再被吞进 URL
  assert.deepEqual(linkify("（https://a.com/x）"), [
    { kind: "text", value: "（" },
    { kind: "link", value: "https://a.com/x", href: "https://a.com/x" },
    { kind: "text", value: "）" },
  ]);
  // 但汉字路径仍是链接的一部分（中文域名/路径是合法 URL）
  assert.deepEqual(linkify("见 https://zh.wikipedia.org/wiki/中国 条目"), [
    { kind: "text", value: "见 " },
    { kind: "link", value: "https://zh.wikipedia.org/wiki/中国", href: "https://zh.wikipedia.org/wiki/中国" },
    { kind: "text", value: " 条目" },
  ]);
});

test("linkify: 括号按是否配对决定归 URL 还是句末标点", () => {
  // 配对 → 属于 URL（维基/百科大量这种地址，曾被从中间切成 Foo_ + "(bar)")
  const wiki = linkify("见 https://zh.wikipedia.org/wiki/Foo_(bar) 谢谢");
  assert.deepEqual(wiki[1], {
    kind: "link",
    value: "https://zh.wikipedia.org/wiki/Foo_(bar)",
    href: "https://zh.wikipedia.org/wiki/Foo_(bar)",
  });
  // 不配对 → 是句末收尾
  assert.deepEqual(linkify("看 https://a.com/x) 呢")[1], {
    kind: "link",
    value: "https://a.com/x",
    href: "https://a.com/x",
  });
  // 标点与不配对括号叠在一起时，要一直剥到干净
  assert.deepEqual(linkify("https://a.com/x),")[0], {
    kind: "link",
    value: "https://a.com/x",
    href: "https://a.com/x",
  });
});

test("linkify: 多个 URL 交替穿插", () => {
  assert.deepEqual(linkify("a https://x.com b http://y.io c"), [
    { kind: "text", value: "a " },
    { kind: "link", value: "https://x.com", href: "https://x.com" },
    { kind: "text", value: " b " },
    { kind: "link", value: "http://y.io", href: "http://y.io" },
    { kind: "text", value: " c" },
  ]);
});

test("linkify: 不匹配危险 scheme 与裸域名", () => {
  // 只匹配 http(s)://，其它 scheme / 裸 www. 不动
  assert.equal(linkify("javascript:alert(1)").length, 1);
  assert.equal(linkify("javascript:alert(1)")[0].kind, "text");
  assert.equal(linkify("www.example.com 看看").length, 1);
  assert.equal(linkify("www.example.com 看看")[0].kind, "text");
});

// ---------------- @ 到自己（用户 2026-09-26：自己看是「@你」，别人看仍是名字） ----------------
// ⚠️ 判据必须落在**渲染层**：如果为了显示去改写发送内容，对端与历史里那条消息就被污染了，
// 而且换一台设备登录（昵称相同、身份不同）就会显示错。这里断言的正是"同一份文本、
// 只有 self 参数不同 ⇒ 段不同"。

test("@ 到自己 → 换成本地化标签；同一个串里的别人仍是名字", () => {
  assert.deepEqual(
    linkify("张三 @李四 和 @张三 到场", ["李四", "张三"], { name: "张三", label: "@你" }),
    [
      { kind: "text", value: "张三 " },
      { kind: "mention", value: "@李四" },
      { kind: "text", value: " 和 " },
      { kind: "mention-self", value: "@你" },
      { kind: "text", value: " 到场" },
    ],
  );
});

test("不传 self 时段形状完全不变（别人那侧的渲染不能被动到）", () => {
  assert.deepEqual(linkify("@张三 到场", ["张三"]), [
    { kind: "mention", value: "@张三" },
    { kind: "text", value: " 到场" },
  ]);
});

test("selfName 是别人名字的前缀时不许误判（长名字优先）", () => {
  // 「张三」是自己、「张三丰」是别人：@张三丰 必须还是 @张三丰，不能被折成 @你
  assert.deepEqual(
    linkify("@张三丰 @张三", ["张三丰", "张三"], { name: "张三", label: "@你" }),
    [
      { kind: "mention", value: "@张三丰" },
      { kind: "text", value: " " },
      { kind: "mention-self", value: "@你" },
    ],
  );
});

test("linkify: URL 内部标点不切断（常见合法字符）", () => {
  const segs = linkify("https://a.com/path?q=1&x=2#hash");
  assert.equal(segs.length, 1);
  assert.equal(segs[0].kind, "link");
  assert.equal((segs[0] as { href: string }).href, "https://a.com/path?q=1&x=2#hash");
});

test("linkify: 空字符串 / 非字符串", () => {
  assert.deepEqual(linkify(""), []);
  assert.deepEqual(linkify(undefined as unknown as string), []);
});

test("splitUrl: 短于阈值不切分（mid 为空）", () => {
  assert.deepEqual(splitUrl("https://a.com"), { head: "https://a.com", mid: "", tail: "" });
});

test("splitUrl: 超长 URL 切三段，拼回去必须等于原串", () => {
  const long = "https://very-long-domain.example.com/very/long/path/segment/file.html";
  const { head, mid, tail } = splitUrl(long, 32);
  // 核心契约：head + mid + tail === 原 URL。选区复制取的就是这三段，
  // 丢掉任何一段都会复制出残缺链接（用户 2026-09-16 报的缺陷）。
  assert.equal(head + mid + tail, long);
  assert.ok(mid.length > 0, "必须真的省略掉了一段");
  assert.ok(head.startsWith("https://"), "保留协议头");
  assert.equal(head.length, tail.length, "头尾等长");
  assert.ok(head.length + tail.length <= 32, "可见部分不超过阈值");
});

test("splitUrl: maxLen 过小不错位切片", () => {
  // half 为 0 时 `slice(-0)` 等于 `slice(0)`（整串），会切出 head/tail 重叠的错位结果
  const url = "https://a-very-long-url.example.com/x";
  assert.deepEqual(splitUrl(url, 2), { head: url, mid: "", tail: "" });
  assert.deepEqual(splitUrl(url, 0), { head: url, mid: "", tail: "" });
});

// ---------------- @提及（群聊） ----------------

test("linkify: @成员名切成 mention 段", () => {
  assert.deepEqual(linkify("叫上 @张三 开会", ["张三"]), [
    { kind: "text", value: "叫上 " },
    { kind: "mention", value: "@张三" },
    { kind: "text", value: " 开会" },
  ]);
});

test("linkify: 行首与多提 及、重名最长优先", () => {
  const segs = linkify("@张三 @张三四", ["张三", "张三四"]);
  assert.deepEqual(segs, [
    { kind: "mention", value: "@张三" },
    { kind: "text", value: " " },
    { kind: "mention", value: "@张三四" },
  ]);
});

test("linkify: 邮箱里的 @ 不误判（@ 前非空白）", () => {
  const segs = linkify("发到 a@b.com 了", ["b"]);
  assert.equal(segs.length, 1);
  assert.equal(segs[0].kind, "text");
});

test("linkify: 名字后跟中文标点仍高亮，未知名字不高亮", () => {
  const segs = linkify("@张三，收到请回复 @李四", ["张三"]);
  assert.deepEqual(segs, [
    { kind: "mention", value: "@张三" },
    { kind: "text", value: "，收到请回复 @李四" },
  ]);
});

test("linkify: 表情 token 收尾的 ] 也算 @ 前导边界", () => {
  // 与 messages.ts 的 messageMentionsName 共用 MENTION_BEFORE —— 这条一旦只改一边，
  // 就会出现「气泡高亮了但不通知」。
  assert.deepEqual(linkify("[微笑]@张三 快来", ["张三"]), [
    { kind: "text", value: "[微笑]" },
    { kind: "mention", value: "@张三" },
    { kind: "text", value: " 快来" },
  ]);
  // 中文标点仍**不算**边界（既有口径：紧贴标点的 @ 不高亮，也就不该触发红点）
  assert.equal(linkify("通知：@张三", ["张三"])[0].kind, "text");
});

test("linkify: mention 与 URL 混排", () => {
  const segs = linkify("@张三 看 https://a.com", ["张三"]);
  assert.deepEqual(segs, [
    { kind: "mention", value: "@张三" },
    { kind: "text", value: " 看 " },
    { kind: "link", value: "https://a.com", href: "https://a.com" },
  ]);
});

test("linkify: 不传成员名时行为不变", () => {
  assert.deepEqual(linkify("@张三 好"), [{ kind: "text", value: "@张三 好" }]);
});
