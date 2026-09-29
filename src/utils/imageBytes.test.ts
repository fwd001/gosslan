import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  bytesToBase64,
  dataUrlBase64,
  MAX_PASTED_IMAGE_BYTES,
  MAX_TODO_IMAGE_BYTES,
  PASTED_IMAGE_LIMIT_MB,
  TODO_IMAGE_LIMIT_MB,
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
 * 从 Rust 源码里读一个「`数字 * 数字 * 数字`」形式的字节常量。
 *
 * 找不到就**指名道姓地报红**（而不是返回 undefined 让后面的比较出一个看不出原因的差值）：
 * 这类跨语言判据最常见的失效方式不是数值漂了，是那一边改了名 / 换了写法。
 * 类型同时认 `u64` 和 `usize`（两处声明用的不是同一个类型）。
 */
function rustByteConst(rs: string, name: string, file: string): number {
  const m = new RegExp(`const ${name}: (?:u64|usize) = ([^;]+);`).exec(rs);
  assert.ok(m, `${file} 里找不到 ${name} —— 改名或改写法了，这道跨语言闸要跟着改`);
  // 不引 eval：只允许纯数字相乘的字面量
  const value = m[1]
    .split("*")
    .map((t) => Number(t.trim()))
    .reduce((a, b) => a * b, 1);
  assert.ok(Number.isFinite(value) && value > 0, `无法解析 ${name} 的上限字面量：${m[1]}`);
  return value;
}

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
  const value = rustByteConst(rs, "MAX_OUTGOING_IMAGE_BYTES", "commands.rs");
  assert.equal(MAX_PASTED_IMAGE_BYTES, value, "前后端上限漂移：前端会放行后端要拒的图");
  assert.equal(PASTED_IMAGE_LIMIT_MB, value / (1024 * 1024));
});

/**
 * 同一条跨语言契约的第二格：群任务图片的 10MB 闸（用户 2026-09-29）。
 *
 * 后端是权威（`todo_image_meta` / `save_todo_image_bytes` 两条入口都判），前端这一道只是
 * 少读一次文件 —— 所以两份必须同值：前端放行、后端拒收的表现是"图加进列表了但保存时整条任务报错"，
 * 而用户看到的只有那句报错。
 */
test("群任务图片上限与 Rust 的 MAX_TODO_IMAGE_BYTES 必须一致", () => {
  const rs = readFileSync(
    join(import.meta.dirname, "..", "..", "src-tauri", "src", "commands", "group_todo_media.rs"),
    "utf8",
  );
  const value = rustByteConst(rs, "MAX_TODO_IMAGE_BYTES", "group_todo_media.rs");
  assert.equal(MAX_TODO_IMAGE_BYTES, value, "前后端任务图上限漂移");
  assert.equal(TODO_IMAGE_LIMIT_MB, value / (1024 * 1024));
});

/**
 * 「同步过去会变糊」这一族里唯一能在代码层钉住的那一格：**放行的图必须能按原始字节显示**。
 *
 * `read_content_preview` 在文件字节数超过它自己那道 `max_bytes.min(...)` 时返回 `TOO_LARGE`，
 * 而前端拿到那句**不会降级显示、也不会有第二份图** —— 表现就是"图明明发过去了，界面上一片空白/
 * 只剩文件名"。所以任务图的体积闸必须**严格小于**显示层那道上限，否则就存在一张
 * "发得出去、看不了"的图。
 *
 * 反向模式：把任一侧的数字改成 10MB ≥ 上限，这一条必须红（它不是同义反复）。
 */
test("放行的任务图必须能原分辨率显示（体积闸 < 预览读取上限）", () => {
  const rs = readFileSync(
    join(import.meta.dirname, "..", "..", "src-tauri", "src", "commands", "favorites.rs"),
    "utf8",
  );
  const at = rs.indexOf("pub fn read_content_preview");
  assert.ok(at > 0, "favorites.rs 里找不到 read_content_preview —— 这条判据的锚点断了");
  // 只看函数开头那一段：那道夹取是 `let max_bytes = max_bytes.min(...)`，在两个点查之前
  const head = rs.slice(at, at + 1500);
  const m = /max_bytes\.min\(([^)]+)\)/.exec(head);
  assert.ok(m, "read_content_preview 不再夹 max_bytes —— 显示层那道上限换了形状，判据要跟着改");
  const cap = m[1]
    .split("*")
    .map((t) => Number(t.trim()))
    .reduce((a, b) => a * b, 1);
  assert.ok(Number.isFinite(cap) && cap > 0, `无法解析预览上限：${m[1]}`);
  assert.ok(
    MAX_TODO_IMAGE_BYTES < cap,
    `任务图上限 ${MAX_TODO_IMAGE_BYTES} 不低于预览读取上限 ${cap} ⇒ 会出现"发得出去但看不了"的图`,
  );
});
