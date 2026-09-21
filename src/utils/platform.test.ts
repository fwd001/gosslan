import { test } from "node:test";
import assert from "node:assert/strict";
import { isAndroidUA, isMacUA, resolveMobileLayout } from "./platform.ts";

test("isMacUA：桌面 macOS UA 判为 Mac", () => {
  const uas = [
    // Safari（Intel）
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
    // Chrome（Apple Silicon，UA 仍写 Intel Mac OS X）
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    // 老版本 macOS
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.0 Safari/605.1.15",
  ];
  for (const ua of uas) assert.equal(isMacUA(ua), true, ua);
});

test("isMacUA：Windows / Linux / Android 判为非 Mac", () => {
  const uas = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
  ];
  for (const ua of uas) assert.equal(isMacUA(ua), false, ua);
});

test("isMacUA：iOS / iPadOS 判为非 Mac（回归：旧正则 /Mac OS X/ 会把 iPhone 误判成 Mac）", () => {
  const uas = [
    // iPhone —— "like Mac OS X" 会命中 /Mac OS X/，必须排除
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    // iPad
    "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    // iPod touch（老设备）
    "Mozilla/5.0 (iPod touch; CPU iPhone OS 14_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.0 Mobile/15E148 Safari/604.1",
  ];
  for (const ua of uas) assert.equal(isMacUA(ua), false, ua);
});

test("isMacUA：空 / 缺失 UA 不抛错、判为非 Mac", () => {
  assert.equal(isMacUA(""), false);
});

test("isAndroidUA：Android 手机 / 平板 / 桌面模式判为 Android", () => {
  const uas = [
    "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
    // Android 平板（无 Mobile 段）
    "Mozilla/5.0 (Linux; Android 13; SM-X700) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    // Android 桌面模式 / 输入法 WebView 等小写变体
    "mozilla/5.0 (linux; android 12) applewebkit/537.36",
  ];
  for (const ua of uas) assert.equal(isAndroidUA(ua), true, ua);
});

test("isAndroidUA：桌面三端 / iOS 判为非 Android", () => {
  const uas = [
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    "",
  ];
  for (const ua of uas) assert.equal(isAndroidUA(ua), false, ua);
});

test("resolveMobileLayout：移动平台**一律**走移动布局，宽度判否也不算数", () => {
  // 回归（用户 2026-09-21，安卓首次启动）：权限弹框盖在 WebView 首次布局上时，
  // matchMedia 会读到兜底视口宽度（980px 档）⇒ narrow=false。若判据只看宽度，
  // 手机上就会渲染成桌面三栏布局。
  assert.equal(
    resolveMobileLayout({ android: true, ios: false, narrow: false }),
    true,
    "安卓必须恒为移动布局（竖屏锁定，没有窄窗口这种中间态）",
  );
  assert.equal(resolveMobileLayout({ android: false, ios: true, narrow: false }), true, "iOS 同理");
  assert.equal(resolveMobileLayout({ android: true, ios: false, narrow: true }), true);
});

test("resolveMobileLayout：桌面端按宽度切（窄窗口用移动布局，宽窗口用三栏）", () => {
  assert.equal(resolveMobileLayout({ android: false, ios: false, narrow: true }), true);
  assert.equal(
    resolveMobileLayout({ android: false, ios: false, narrow: false }),
    false,
    "桌面宽窗口要保留三栏布局",
  );
});
