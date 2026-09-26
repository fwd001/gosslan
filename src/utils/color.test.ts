import { test } from "node:test";
import assert from "node:assert/strict";
import {
  hexToRgb,
  lighten,
  darken,
  rgba,
  humanSize,
    mixHex,
  luma,
  mixToLuma,
  contrastRatio,
  adjustHsl,
  toHsl,
  hslToHex,
    } from "./color.ts";

test("hexToRgb 解析 6 位与 3 位 hex", () => {
  assert.deepEqual(hexToRgb("#3b82f6"), [59, 130, 246]);
  assert.deepEqual(hexToRgb("#ffffff"), [255, 255, 255]);
  assert.deepEqual(hexToRgb("#fff"), [255, 255, 255]);
});

test("lighten 向白混合、darken 向黑混合", () => {
  assert.equal(lighten("#000000", 1), "rgb(255, 255, 255)");
  assert.equal(darken("#ffffff", 1), "rgb(0, 0, 0)");
});

test("rgba 输出带透明度", () => {
  assert.equal(rgba("#3b82f6", 0.12), "rgba(59, 130, 246, 0.12)");
});

test("主题色派生链：hover 比主色浅、active 比主色深", () => {
  const base = [59, 130, 246];
  const hv = (lighten("#3b82f6", 0.08).match(/\d+/g) ?? []).map(Number);
  const av = (darken("#3b82f6", 0.08).match(/\d+/g) ?? []).map(Number);
  assert.ok(hv[0] >= base[0]);
  assert.ok(av[0] <= base[0]);
});

test("humanSize 单位换算与精度", () => {
  assert.equal(humanSize(0), "0.0 B");
  assert.equal(humanSize(1024), "1.0 KB");
  assert.equal(humanSize(1024 * 1024), "1.0 MB");
  assert.equal(humanSize(1024 * 1024 * 1024), "1.0 GB");
  assert.equal(humanSize(1536), "1.5 KB");
});

test("luma: 黑白两端为 0/255，中间色介于其间", () => {
  assert.equal(Math.round(luma("#000000")), 0);
  assert.equal(Math.round(luma("#ffffff")), 255);
  const blue = luma("#3b82f6");
  assert.ok(blue > 0 && blue < 255, `蓝色亮度应在中间：${blue}`);
});

test("mixHex 返回 6 位 hex，可继续参与后续混合", () => {
  assert.match(mixHex("#000000", [255, 255, 255], 1), /^#[0-9a-f]{6}$/);
  assert.equal(mixHex("#000000", [255, 255, 255], 1), "#ffffff");
  // 混向深色画布：亮度必须下降
  const dimmed = mixHex("#3b82f6", [15, 23, 42], 0.5);
  assert.ok(luma(dimmed) < luma("#3b82f6"));
});

test("mixToLuma: 只压不提，已达到目标亮度的颜色原样返回", () => {
  const softWhite = mixToLuma("#ffffff", [0, 0, 0], 212);
  assert.ok(Math.abs(luma(softWhite) - 212) <= 1, `应压到 212 附近，实际 ${luma(softWhite)}`);
  // 已经比目标暗 → 不动（深色气泡不会被压没）
  assert.equal(mixToLuma("#374151", [15, 23, 42], 74), "#374151");
});

test("contrastRatio: 暗色气泡底色与文字达到 WCAG AA（≥ 4.5）", () => {
  // 默认主题蓝压暗后配柔和白字，正是聊天自己气泡的暗色组合
  const bubble = mixToLuma("#3b82f6", [15, 23, 42], 74);
  const text = mixToLuma("#ffffff", [0, 0, 0], 212);
  assert.ok(luma(bubble) <= 75, `气泡仍过亮：${luma(bubble)}`);
  assert.ok(contrastRatio(text, bubble) >= 4.5, `对比度不足：${contrastRatio(text, bubble)}`);
  // 黑底白字应远高于阈值
  assert.ok(contrastRatio("#ffffff", "#000000") > 15);
});

test("adjustHsl: 只动明度不动色相，同明度下比混白更浓", () => {
  const theme = "#3b82f6";
  const [h0, s0] = toHsl(theme);
  const [h1, s1, l1] = toHsl(adjustHsl(theme, { l: 0.92 }));
  assert.ok(Math.abs(h1 - h0) < 1, `色相漂了：${h0} → ${h1}`);
  assert.ok(Math.abs(s1 - s0) < 0.02, `饱和度掉了：${s0} → ${s1}`);
  assert.ok(Math.abs(l1 - 0.92) < 0.01, `明度没到位：${l1}`);
  // 色彩浓度（RGB 极差）：混白会一路洗到接近纯白，HSL 调 L 能留住更多色味
  const chroma = (hex: string) => {
    const [r, g, b] = hexToRgb(hex);
    return Math.max(r, g, b) - Math.min(r, g, b);
  };
  const washed = mixHex(theme, [255, 255, 255], 0.9);
  assert.ok(chroma(adjustHsl(theme, { l: 0.92 })) > chroma(washed), "浅底应比混白更有色味");
});

test("adjustHsl: sMax 只压不提，低饱和色不会被强行拉艳", () => {
  const gray = "#64748b";
  const s0 = toHsl(gray)[1];
  assert.ok(toHsl(adjustHsl(gray, { l: 0.92, sMax: 0.85 }))[1] <= s0 + 0.01);
  // 高饱和色会被压到上限
  assert.ok(toHsl(adjustHsl("#ff0000", { l: 0.92, sMax: 0.85 }))[1] <= 0.86);
});

test("toHsl / hslToHex 往返一致", () => {
  for (const hex of ["#3b82f6", "#0f172a", "#ffffff", "#000000", "#22c55e"]) {
    assert.equal(hslToHex(...toHsl(hex)), hex);
  }
});

