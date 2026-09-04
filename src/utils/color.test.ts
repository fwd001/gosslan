import { test } from "node:test";
import assert from "node:assert/strict";
import { hexToRgb, lighten, darken, rgba, humanSize } from "./color.ts";

test("hexToRgb 解析 6 位与 3 位 hex", () => {
  assert.deepEqual(hexToRgb("#3370ff"), [51, 112, 255]);
  assert.deepEqual(hexToRgb("#ffffff"), [255, 255, 255]);
  assert.deepEqual(hexToRgb("#fff"), [255, 255, 255]);
});

test("lighten 向白混合、darken 向黑混合", () => {
  assert.equal(lighten("#000000", 1), "rgb(255, 255, 255)");
  assert.equal(darken("#ffffff", 1), "rgb(0, 0, 0)");
});

test("rgba 输出带透明度", () => {
  assert.equal(rgba("#3370ff", 0.12), "rgba(51, 112, 255, 0.12)");
});

test("主题色派生链：hover 比主色浅、active 比主色深", () => {
  const base = [51, 112, 255];
  const hv = (lighten("#3370ff", 0.08).match(/\d+/g) ?? []).map(Number);
  const av = (darken("#3370ff", 0.08).match(/\d+/g) ?? []).map(Number);
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
