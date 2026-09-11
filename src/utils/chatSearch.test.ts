import assert from "node:assert/strict";
import { test } from "node:test";
import {
  dateRangeFor,
  hitSnippet,
  senderOptionsFrom,
  totalHits,
  type SearchDatePreset,
} from "./chatSearch.ts";
import type { ChatSearchGroup } from "@/types";

/** 固定"现在"：2026-09-12（周六）15:00 本地时间，避免用例随运行时间漂移。 */
const NOW = new Date(2026, 8, 12, 15, 0, 0).getTime();
const DAY = 86_400_000;

test("日期预设按本地日历日算，不是「最近 24 小时」", () => {
  const startOfToday = new Date(2026, 8, 12, 0, 0, 0).getTime();
  assert.deepEqual(dateRangeFor("all", NOW), { sinceMs: null, untilMs: null });
  assert.deepEqual(dateRangeFor("today", NOW), { sinceMs: startOfToday, untilMs: null });
  assert.deepEqual(dateRangeFor("yesterday", NOW), {
    sinceMs: startOfToday - DAY,
    untilMs: startOfToday - 1,
  });
  // 近 7 天含今天 ⇒ 从 6 天前的零点开始
  assert.equal(dateRangeFor("week", NOW).sinceMs, startOfToday - 6 * DAY);
  assert.equal(dateRangeFor("month", NOW).sinceMs, startOfToday - 29 * DAY);

  // 关键边界：昨天 23:59 属于「昨天」，不属于「今天」
  const lastNight = startOfToday - 60_000;
  const today = dateRangeFor("today", NOW);
  assert.ok(today.sinceMs !== null && lastNight < today.sinceMs, "昨晚的消息不该算今天");
  const y = dateRangeFor("yesterday", NOW);
  assert.ok(
    y.sinceMs !== null && y.untilMs !== null && lastNight >= y.sinceMs && lastNight <= y.untilMs,
    "昨晚的消息应落在「昨天」区间里",
  );

  // 每个预设都必须能回到"不限"或给出区间（防止将来加预设时漏分支）
  for (const p of ["all", "today", "yesterday", "week", "month"] as SearchDatePreset[]) {
    const r = dateRangeFor(p, NOW);
    assert.ok(r.sinceMs === null || typeof r.sinceMs === "number");
    assert.ok(r.untilMs === null || typeof r.untilMs === "number");
  }
});

function group(convId: string, msgs: { sender_id: string; sender_name: string; content: string }[], total?: number): ChatSearchGroup {
  return {
    conv_id: convId,
    name: convId,
    kind: "single",
    avatar: null,
    total: total ?? msgs.length,
    latest_ts: 1,
    messages: msgs.map((m, i) => ({
      msg_id: `${convId}-${i}`,
      sender_id: m.sender_id,
      sender_name: m.sender_name,
      kind: "text",
      content: m.content,
      ts: 1,
    })),
  };
}

test("发送人筛选项：按命中条数降序、去掉重复、含自己", () => {
  const opts = senderOptionsFrom([
    group("c1", [
      { sender_id: "a", sender_name: "阿宝", content: "x" },
      { sender_id: "b", sender_name: "宝宝", content: "x" },
      { sender_id: "a", sender_name: "阿宝", content: "x" },
    ]),
    group("c2", [{ sender_id: "a", sender_name: "阿宝", content: "x" }]),
  ]);
  assert.deepEqual(
    opts.map((o) => [o.id, o.count]),
    [
      ["a", 3],
      ["b", 1],
    ],
  );
});

test("命中片段必须包含关键词，且长文本两端加省略号", () => {
  const long = "前".repeat(100) + "关键词" + "后".repeat(100);
  const s = hitSnippet(long, "关键词", 10);
  assert.ok(s.includes("关键词"), "截出来的片段必须包含关键词");
  assert.ok(s.startsWith("…") && s.endsWith("…"), "两端都要有省略号");
  assert.ok(s.length < long.length, "必须比原文短");
  // 短文本原样返回（不加多余省略号）
  assert.equal(hitSnippet("想你", "想你"), "想你");
  // 大小写不敏感（与后端 LIKE 的语义一致）
  assert.ok(hitSnippet("Hello World", "world").includes("World"));
  // 关键词为空 ⇒ 原样返回
  assert.equal(hitSnippet("abc", "  "), "abc");
  // emoji / 代理对不被切断（按字符截取而不是 UTF-16 码元）
  const emoji = "😀".repeat(50) + "找到我" + "😀".repeat(50);
  const es = hitSnippet(emoji, "找到我", 3);
  assert.ok(es.includes("找到我"));
  assert.ok(!es.includes("\uFFFD"), "不能出现半个代理对");
});

test("总数 = 各会话命中数之和（不是返回条数）", () => {
  const gs = [group("c1", [{ sender_id: "a", sender_name: "A", content: "x" }], 11), group("c2", [], 4)];
  assert.equal(totalHits(gs), 15);
});
