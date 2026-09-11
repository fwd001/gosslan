import { test } from "node:test";
import assert from "node:assert/strict";
import { LOG_LEVEL_TEXT, filterLogLines, logLineText, matchesLogQuery } from "./logFilter.ts";

const L = (time: string, level: string, target: string, message: string) => ({
  time,
  level,
  target,
  message,
});

const SAMPLE = [
  L("00:01:02", "info", "mesh", "+conn peer=aab-control-peer ep=192.168.31.113:61999 conns=1"),
  L("00:01:03", "error", "transport", "拒绝未通过身份认证的 Hello: 签名不匹配"),
  L("00:01:04", "warn", "routed", "拨号未成功：连接超时（5s 内未建立）"),
  L("00:01:05", "info", "presence", "学到远端节点 peer=gosslan-718562a258f7cf55"),
];

test("logLineText：按「时间 · 级别 · target · 消息」拼出屏幕上真实的行文本", () => {
  assert.equal(
    logLineText(L("00:01:02", "info", "mesh", "+conn conns=1")),
    "00:01:02 INFO mesh +conn conns=1",
  );
  // 未知级别原样透出（与模板 `LEVEL_TEXT[l.level] ?? l.level` 一致）
  assert.equal(logLineText(L("01:00:00", "trace", "x", "y")), "01:00:00 trace x y");
});

test("空过滤词（含纯空白）→ 不过滤，全部保留", () => {
  assert.deepEqual(filterLogLines(SAMPLE, ""), SAMPLE);
  assert.deepEqual(filterLogLines(SAMPLE, "   "), SAMPLE);
});

test("子串匹配：命中 message / target / 时间", () => {
  assert.equal(filterLogLines(SAMPLE, "conns=1").length, 1);
  assert.equal(filterLogLines(SAMPLE, "presence").length, 1);
  assert.equal(filterLogLines(SAMPLE, "00:01:0").length, 4);
  assert.equal(filterLogLines(SAMPLE, "+conn").length, 1);
});

test("大小写不敏感：小写查询能命中大写级别", () => {
  assert.equal(filterLogLines(SAMPLE, "error").length, 1);
  assert.equal(filterLogLines(SAMPLE, "ERROR").length, 1);
  assert.equal(filterLogLines(SAMPLE, "Transport").length, 1);
});

test("精准：只匹配连续字面串，不做模糊/分词/跳字符", () => {
  // 跨词不连续不算命中 —— 这正是「精准匹配」与模糊匹配的分界。
  // （注意别选到恰好连续的组合：`+conn peer=aab…` 里 `conn peer` 本身就是连续子串。）
  assert.equal(filterLogLines(SAMPLE, "peer conns").length, 0);
  assert.equal(filterLogLines(SAMPLE, "peer=aab").length, 1);
  // 打乱字符顺序也不是命中
  assert.equal(filterLogLines(SAMPLE, "nnoc").length, 0);
});

test("正则元字符按字面处理（不会被当模式）", () => {
  // `[mesh]` / `(5s` / `1.2` 这类在日志里很常见，必须能原样搜到
  const withBrackets = [L("00:00:00", "info", "mesh", "[mesh] +conn 1.2s")];
  assert.equal(filterLogLines(withBrackets, "[mesh]").length, 1);
  assert.equal(filterLogLines(withBrackets, "1.2").length, 1);
  // 若被当正则：`.` 会匹配任意字符 → 这条错误命中
  assert.equal(filterLogLines(withBrackets, "1x2").length, 0);
  // 若被当正则：`[mesh]` 是字符集 → 会匹配到 m/e/s/h
  assert.equal(filterLogLines([L("00:00:00", "info", "x", "mmm")], "[mesh]").length, 0);
});

test("matchesLogQuery 与 filterLogLines 语义一致", () => {
  const line = SAMPLE[0];
  assert.equal(matchesLogQuery(line, "conns=1"), true);
  assert.equal(matchesLogQuery(line, "nope"), false);
  assert.deepEqual(filterLogLines([line], "nope"), []);
});

test("过滤保持输入顺序（模板已按倒序排好，不能被打乱）", () => {
  const out = filterLogLines(SAMPLE, "00:01:0");
  assert.deepEqual(
    out.map((l) => l.time),
    ["00:01:02", "00:01:03", "00:01:04", "00:01:05"],
  );
});

test("级别文本表覆盖后端全部三档", () => {
  assert.deepEqual(Object.keys(LOG_LEVEL_TEXT).sort(), ["error", "info", "warn"]);
});
