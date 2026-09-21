/**
 * 卡片消息「可复制文字形态」的契约测试。
 *
 * 这份实现存在的理由是**单一来源**：消息右键/长按面板与收藏页的「复制」必须给出同一段文字
 * （用户 2026-09-21：「群任务也不能复制啊，收藏咋还有复制呢」）。所以这里除了逐 kind 的
 * 结果，还要锁住"非卡片一律 null"这条边界 —— 调用方就是靠 null 回退到各自的正文复制。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { cardCopyText } from "./cardText.ts";
import { t } from "../i18n/index.ts";

/** 待办创建记录的最小载荷（`todo_id` 是 `parseTodo` 的硬要求，漏了就整条判废）。 */
const todoPayload = (extra: Record<string, unknown> = {}) =>
  JSON.stringify({ todo_id: "td_1", title: "修复登录", description: "复现后修", ...extra });

test("待办：标题 + 描述（描述为空时不留尾随空行）", () => {
  assert.equal(
    cardCopyText("todo", todoPayload()),
    `${t("favorite.cardKind.todo")}：修复登录\n复现后修`,
  );
  assert.equal(
    cardCopyText("todo", todoPayload({ description: "" })),
    `${t("favorite.cardKind.todo")}：修复登录`,
  );
  // 两侧空白不该带进剪贴板
  assert.equal(
    cardCopyText("todo", todoPayload({ title: "  修复登录  ", description: "  复现后修  " })),
    `${t("favorite.cardKind.todo")}：修复登录\n复现后修`,
  );
});

test("待办：墓碑 / 缺 todo_id / 标题为空白 ⇒ null（调用方据此报失败）", () => {
  assert.equal(cardCopyText("todo", todoPayload({ deleted: true })), null);
  assert.equal(cardCopyText("todo", JSON.stringify({ title: "修复登录" })), null);
  assert.equal(cardCopyText("todo", JSON.stringify({ todo_id: "", title: "x" })), null);
  assert.equal(cardCopyText("todo", todoPayload({ title: "   " })), null);
});

test("投票：问题 + 选项逐行；没有问题的空壳不复制", () => {
  assert.equal(
    cardCopyText("poll", JSON.stringify({ question: "几点开会？", options: ["9点", "10点"] })),
    `${t("favorite.cardKind.poll")}：几点开会？\n9点\n10点`,
  );
  // 没有 options 字段 ⇒ 只复制问题那一行（不抛错）
  assert.equal(
    cardCopyText("poll", JSON.stringify({ question: "几点开会？" })),
    `${t("favorite.cardKind.poll")}：几点开会？`,
  );
  // options 里的脏数据（非字符串）被丢掉，不写进剪贴板
  assert.equal(
    cardCopyText("poll", JSON.stringify({ question: "q", options: ["a", 1, null, "b"] })),
    `${t("favorite.cardKind.poll")}：q\na\nb`,
  );
  assert.equal(cardCopyText("poll", JSON.stringify({ question: "   " })), null);
  assert.equal(cardCopyText("poll", "{}"), null);
});

test("群公告：正文即文字形态；空正文 ⇒ null", () => {
  assert.equal(cardCopyText("announcement", JSON.stringify({ text: "今晚发版" })), "今晚发版");
  assert.equal(cardCopyText("announcement", JSON.stringify({ text: "  今晚发版  " })), "今晚发版");
  assert.equal(cardCopyText("announcement", JSON.stringify({ text: "   " })), null);
  assert.equal(cardCopyText("announcement", "{}"), null);
});

test("非卡片 kind 一律 null —— 正文类由各自的复制路径处理，卡片文字形态不得越界", () => {
  for (const kind of ["text", "code", "image", "file", "merge", "system", "todo_update", "reaction"]) {
    assert.equal(cardCopyText(kind, "随便什么内容"), null, kind);
    // 即使内容恰好是一段合法 JSON 也不接管
    assert.equal(cardCopyText(kind, JSON.stringify({ question: "q", text: "t" })), null, kind);
  }
});

test("畸形载荷不抛错（整段 JSON 解析失败 = 没有可复制文字）", () => {
  for (const kind of ["todo", "poll", "announcement"]) {
    assert.equal(cardCopyText(kind, "{oops"), null, kind);
    assert.equal(cardCopyText(kind, ""), null, kind);
    assert.equal(cardCopyText(kind, "null"), null, kind);
    assert.equal(cardCopyText(kind, "[1,2]"), null, kind);
    assert.equal(cardCopyText(kind, '"a string"'), null, kind);
  }
});
