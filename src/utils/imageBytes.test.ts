import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  bytesToBase64,
  dataUrlBase64,
  MAX_PASTED_IMAGE_BYTES,
  PASTED_IMAGE_LIMIT_MB,
  urlToBase64,
} from "./imageBytes.ts";

/** 参照实现：仓库原来那 7 行的做法（`+=` 累积 + btoa），逐字符必须一致。 */
function legacyBase64(buf: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < buf.length; i += chunk) {
    binary += String.fromCharCode(...buf.subarray(i, i + chunk));
  }
  return btoa(binary);
}

function pseudoRandom(n: number): Uint8Array {
  const out = new Uint8Array(n);
  let x = 123456789;
  for (let i = 0; i < n; i++) {
    x = (x * 1103515245 + 12345) & 0x7fffffff;
    out[i] = x & 0xff;
  }
  return out;
}

test("bytesToBase64 与旧实现逐字符一致（含 8KiB 分块边界的四种情形）", () => {
  // 8191/8192/8193 是刻意挑的：分块边界写错时只会在那一个长度上出错，
  // 随机大样本反而容易把它盖过去。
  const allBytes = new Uint8Array(256 * 4);
  for (let i = 0; i < allBytes.length; i++) allBytes[i] = i % 256;
  const cases: Uint8Array[] = [
    new Uint8Array(0),
    new Uint8Array([0]),
    new Uint8Array([0xff]),
    new Uint8Array([0, 1, 2, 253, 254, 255]),
    pseudoRandom(8191),
    pseudoRandom(8192),
    pseudoRandom(8193),
    pseudoRandom(40000),
    allBytes,
  ];
  for (const buf of cases) {
    const got = bytesToBase64(buf);
    assert.equal(got, legacyBase64(buf), `与旧实现不一致，长度 ${buf.length}`);
    assert.equal(got.length % 4, 0, `base64 长度必须是 4 的倍数，长度 ${buf.length}`);
  }
});

test("bytesToBase64 处理全零与满值：填充位不能被吃掉", () => {
  assert.equal(bytesToBase64(new Uint8Array([0, 0, 0])), "AAAA");
  assert.equal(bytesToBase64(new Uint8Array([255, 255, 255])), "////");
  assert.equal(bytesToBase64(new Uint8Array([73])), "SQ==");
  assert.equal(bytesToBase64(new Uint8Array([73, 32])), "SSA=");
});

test("dataUrlBase64 只认 ;base64 那一种 data URL", () => {
  assert.equal(dataUrlBase64("data:image/png;base64,AAAA"), "AAAA");
  assert.equal(
    dataUrlBase64("data:application/octet-stream;base64,/w=="),
    "/w==",
    "首字符是 / 或 + 时也不能被误判",
  );
  // ⚠️ 不带 `;base64` 的 data URL 是**百分号编码原文**：直接切尾巴会得到
  // "看着像 base64、解出来是乱码"的文件，必须返回 null 退回 fetch 路径。
  assert.equal(dataUrlBase64("data:text/plain,hello%20world"), null);
  assert.equal(dataUrlBase64("data:image/png;base64"), null, "没有逗号 ⇒ null");
  assert.equal(dataUrlBase64("blob:http://localhost/abc"), null);
  assert.equal(dataUrlBase64("https://example.com/a.png"), null);
});

test("urlToBase64：data URL 直接摘载荷（不再解码-再-编码同一个串）", async () => {
  const payload = "iVBORw0KGgoAAAANSUhEUg==";
  assert.equal(await urlToBase64(`data:image/png;base64,${payload}`), payload);
});

test("urlToBase64：URL 里没有逗号或不是 data: 前缀 ⇒ 不短路（交回 fetch）", () => {
  // 只断言"决策"而不真去 fetch —— 拿运行时网络行为当判据会变成看运气红绿的测试。
  assert.equal(dataUrlBase64("data:text/plain,abc"), null);
  assert.equal(dataUrlBase64("data:text/plain"), null);
  assert.equal(dataUrlBase64("data:;base64,AAA"), "AAA");
});

/**
 * 跨语言契约：TS 侧的前置体积闸必须等于 Rust 侧的 `MAX_OUTGOING_IMAGE_BYTES`
 * （`src-tauri/src/commands.rs`）。写死两份而不比对，就是"改了那边忘了这边"的
 * 标准剧本 —— 而这道闸存在的意义正是"别把超限的图读进 JS 堆"。
 */
test("图片上限与 Rust 的 MAX_OUTGOING_IMAGE_BYTES 必须一致", () => {
  const rs = readFileSync(
    join(import.meta.dirname, "..", "..", "src-tauri", "src", "commands.rs"),
    "utf8",
  );
  const m = /const MAX_OUTGOING_IMAGE_BYTES: u64 = ([^;]+);/.exec(rs);
  assert.ok(m, "Rust 侧找不到 MAX_OUTGOING_IMAGE_BYTES —— 改名了就要同步这道闸");
  // 只允许 `数字 * 数字 * 数字` 这种字面量乘积（不引 eval：手写求值）
  const value = m[1]
    .split("*")
    .map((t) => Number(t.trim()))
    .reduce((a, b) => a * b, 1);
  assert.ok(Number.isFinite(value) && value > 0, `无法解析 Rust 侧的上限字面量：${m[1]}`);
  assert.equal(MAX_PASTED_IMAGE_BYTES, value, "前后端上限漂移：前端会放行后端要拒的图");
  assert.equal(PASTED_IMAGE_LIMIT_MB, value / (1024 * 1024));
});
