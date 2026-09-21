/**
 * 文件/图片消息载荷解析的契约测试。
 *
 * 这份解析被两处共用（消息气泡 `useMessageFile`、引用块 `viewQuoted`），而两处对"缺字段"
 * 的兜底**不同**：气泡那侧用传输记录和 i18n 默认名补，引用那侧没有传输记录，缺 path 就提示。
 * 所以这里要钉住的核心契约是：**只填载荷里真的存在且类型正确的字段**，别替调用方编默认值。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { parseFileMeta } from "./fileMeta.ts";

test("完整载荷：字段原样取出（含 sha256）", () => {
  assert.deepEqual(
    parseFileMeta(
      JSON.stringify({
        name: "报告.pdf",
        path: "C:/files/a.pdf",
        size: 1234,
        subtype: "file",
        sha256: "abc",
      }),
    ),
    { name: "报告.pdf", path: "C:/files/a.pdf", size: 1234, subtype: "file", sha256: "abc" },
  );
});

test("缺字段 ≠ 编默认值：没有的键就不出现（调用方各自兜底）", () => {
  // 乐观上屏时只有名字与大小：path 缺失 ⇒ 结果里**没有** path（气泡那侧会用传输记录补）
  assert.deepEqual(parseFileMeta(JSON.stringify({ name: "a.png", size: 10 })), {
    name: "a.png",
    size: 10,
  });
  assert.deepEqual(parseFileMeta(JSON.stringify({ path: "C:/f" })), { path: "C:/f" });
  assert.deepEqual(parseFileMeta("{}"), {});
});

test("类型不对 / 空串 一律丢弃（不能把脏值当真的用）", () => {
  assert.deepEqual(
    parseFileMeta(JSON.stringify({ name: 42, path: "", size: "9", subtype: null, sha256: 7 })),
    {},
  );
  // size 允许 0（空文件是合法的），但 NaN/Infinity 不是
  assert.deepEqual(parseFileMeta(JSON.stringify({ size: 0 })), { size: 0 });
  assert.deepEqual(parseFileMeta(JSON.stringify({ size: Number.NaN })), {});
});

test("畸形载荷返回 null（与 parseTodo 等解析器同一种约定）", () => {
  for (const bad of ["", "{", "null", "0", '"str"', "[1,2]", "true"]) {
    assert.equal(parseFileMeta(bad), null, bad);
  }
});

test("旧格式（data URL 正文）不算文件元信息 ⇒ null", () => {
  // 开发阶段遗留的 image 消息正文可能是 data URL，那不是 JSON
  assert.equal(parseFileMeta("data:image/png;base64,AAAA"), null);
});
