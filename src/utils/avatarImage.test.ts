import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bytesToBase64 } from "./imageBytes.ts";
import {
  AVATAR_INPUT_LIMIT_MB,
  AVATAR_INPUT_MAX_BYTES,
  AVATAR_OUTPUT_MAX_BYTES,
  dataUrlByteLength,
  pickAvatarJpeg,
} from "./avatarImage.ts";

/** 造一个「解码后正好 n 字节」的 JPEG data URL（用已单独验过的 bytesToBase64，不手搓 base64）。 */
function urlOfBytes(n: number): string {
  return `data:image/jpeg;base64,${bytesToBase64(new Uint8Array(n))}`;
}

// ---------------- dataUrlByteLength ----------------

/**
 * 与上限比的是**解码后的字节数**，不是 data URL 的字符串长度（后者约为前者的 4/3，
 * 还多一个 `data:image/jpeg;base64,` 前缀）。用真实 base64 往返来判，三种填充都要覆盖 ——
 * 填充位算错时只会在那一两个长度上偏，大样本容易盖过去。
 */
test("dataUrlByteLength 算的是解码后的字节数（含三种填充）", () => {
  for (const n of [0, 1, 2, 3, 4, 5, 6, 7, 255, 4096, 1024 * 1024]) {
    assert.equal(dataUrlByteLength(urlOfBytes(n)), n, `长度 ${n}`);
  }
});

/** 反例那一面：不是 base64 载荷就不能拿来比体积，必须抛而不是给个近似值。 */
test("dataUrlByteLength 拒绝非 base64 的 data URL 与其它 URL", () => {
  assert.throws(() => dataUrlByteLength("data:text/plain,hello"), /不是 base64/);
  assert.throws(() => dataUrlByteLength("blob:http://localhost/abc"), /不是 base64/);
});

// ---------------- pickAvatarJpeg ----------------

/**
 * 「尽量接近无损、尽可能压到 1MB 以内」= **从高往低**取第一个够小的档位。
 *
 * 这一条同时挡两种写反：从低往高找（会白压一档）、以及"固定用某一档"（根本不比大小）。
 */
test("质量从高往低找，取第一个落进 1MB 的档位", () => {
  const sizeFor = (q: number): number => (q === 0.95 ? 1_400_000 : q === 0.9 ? 1_000_000 : 700_000);
  const tried: number[] = [];
  const got = pickAvatarJpeg((q) => {
    tried.push(q);
    return urlOfBytes(sizeFor(q));
  });
  assert.deepEqual(tried, [0.95, 0.9], "0.95 超标 ⇒ 退到 0.9；0.9 已经够小 ⇒ 不该再往下压");
  assert.equal(got, urlOfBytes(1_000_000), "返回的必须是 0.9 那一份，不是别档的");
});

test("全都够小时用最高档（不为了省体积提前压质量）", () => {
  const tried: number[] = [];
  const got = pickAvatarJpeg((q) => {
    tried.push(q);
    return urlOfBytes(50_000);
  });
  assert.deepEqual(tried, [0.95], "第一档就合格 ⇒ 只该编码一次");
  assert.ok(got.startsWith("data:image/jpeg;base64,"));
});

/**
 * 一档都装不下 ⇒ 抛，而不是"就用最差的那档"。
 *
 * 静默返回超限的那份，表现是后端以"头像过大"拒收、界面上点了没反应 ——
 * 这正是这条链最不该有的失败形状（宁可明确说换一张）。
 */
test("没有一档装得下时报错，不静默产出超限头像", () => {
  assert.throws(
    () => pickAvatarJpeg(() => urlOfBytes(AVATAR_OUTPUT_MAX_BYTES + 1)),
    /超过 1MB/,
  );
});

test("边界：恰好等于上限放行（超出才算不合格）", () => {
  const exact = pickAvatarJpeg(() => urlOfBytes(AVATAR_OUTPUT_MAX_BYTES));
  assert.equal(dataUrlByteLength(exact), AVATAR_OUTPUT_MAX_BYTES);
});

// ---------------- 跨语言 / 跨层契约 ----------------

/**
 * 前端产出的头像上限必须**落在后端允许的范围之内**（`MAX_AVATAR_BYTES`，`commands.rs`）。
 * 反过来的话，用户点「换头像」会得到一句"头像过大，请压缩到 2MB 以内"，而那句话他执行不了
 * —— 压缩是这段代码做的，不是他做的。
 */
test("头像产出上限不得超过 Rust 的 MAX_AVATAR_BYTES", () => {
  const rs = readFileSync(join(import.meta.dirname, "..", "..", "src-tauri", "src", "commands.rs"), "utf8");
  const m = /const MAX_AVATAR_BYTES: usize = ([^;]+);/.exec(rs);
  assert.ok(m, "commands.rs 里找不到 MAX_AVATAR_BYTES —— 改名了就要同步这道闸");
  const value = m[1]
    .split("*")
    .map((t) => Number(t.trim()))
    .reduce((a, b) => a * b, 1);
  assert.ok(Number.isFinite(value) && value > 0, `无法解析 Rust 侧的头像上限：${m[1]}`);
  assert.ok(
    AVATAR_OUTPUT_MAX_BYTES <= value,
    `前端会产出最大 ${AVATAR_OUTPUT_MAX_BYTES} 的头像，而后端只收到 ${value} ⇒ 点了没反应`,
  );
});

/** 输入闸只在前端（原始文件从不跨 IPC），所以这里钉的是"文案与判点用的是同一个数"。 */
test("输入上限 10MB，文案标签与它是同一个数", () => {
  assert.equal(AVATAR_INPUT_MAX_BYTES, 10 * 1024 * 1024);
  assert.equal(AVATAR_INPUT_LIMIT_MB, AVATAR_INPUT_MAX_BYTES / (1024 * 1024));
});
