import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activePopupKey,
  claimPopup,
  releasePopup,
  subscribePopup,
} from "./popupRegistry.ts";

/** 每个用例开始前把注册表复位（模块级单例，用例之间会串）。 */
function reset() {
  const cur = activePopupKey();
  if (cur !== null) releasePopup(cur);
}

test("浮层互斥：打开 B 会让 A 收到通知（右键连点不再叠两个菜单）", () => {
  reset();
  let aClosed = 0; // A 被告知「别再展开了」
  let bClosed = 0;
  const offA = subscribePopup(() => {
    if (activePopupKey() !== "A") aClosed += 1;
  });
  const offB = subscribePopup(() => {
    if (activePopupKey() !== "B") bClosed += 1;
  });

  claimPopup("A");
  assert.equal(activePopupKey(), "A");
  assert.equal(aClosed, 0, "自己刚打开，不该收到收起通知");
  assert.equal(bClosed, 1, "A 展开时应通知其余浮层（B 若正展开则收起）");

  claimPopup("B");
  assert.equal(activePopupKey(), "B");
  assert.equal(aClosed, 1, "A 必须收到收起通知——这就是「两个菜单不并存」的关键");
  assert.equal(bClosed, 1, "B 已成为当前展开者，不该再被告知收起");

  offA();
  offB();
  releasePopup("B");
});

test("浮层互斥：重复 claim 同一个 key 不产生多余通知", () => {
  reset();
  let n = 0;
  const off = subscribePopup(() => {
    n += 1;
  });
  claimPopup("A");
  claimPopup("A");
  assert.equal(n, 1, "同 key 重复 claim 只应通知一次");
  off();
  releasePopup("A");
});

test("浮层互斥：release 只清自己的，不能误清别人的展开态", () => {
  reset();
  claimPopup("A");
  releasePopup("B"); // 非当前展开者：应为空操作
  assert.equal(activePopupKey(), "A", "release 别人的 key 不得清空当前浮层");
  releasePopup("A");
  assert.equal(activePopupKey(), null);
});

test("浮层互斥：取消订阅后不再收到通知", () => {
  reset();
  let n = 0;
  const off = subscribePopup(() => {
    n += 1;
  });
  off();
  claimPopup("A");
  assert.equal(n, 0);
  reset();
});

test("浮层互斥：跨类型互斥（消息菜单 ↔ 好友菜单 ↔ 已读弹层）", () => {
  reset();
  claimPopup("menu:m1");
  claimPopup("friend-menu");
  assert.equal(activePopupKey(), "friend-menu", "打开好友菜单应接管，消息菜单自动收起");
  claimPopup("readers:m2");
  assert.equal(activePopupKey(), "readers:m2");
  reset();
  assert.equal(activePopupKey(), null);
});

// ---------------- 使用侧协议（源码扫描） ----------------

test("浮层协议：每个 `const X = useExclusivePopup(...)` 都必须 watch(X.isActive)", async () => {
  // 协议有两半：claim/release 是"我要展开"，watch(isActive) 是"我被抢了 ⇒ 收起自己"。
  // 只写前半的表现很具体：B 抢到展开权后 A 的 isActive 变 false，但没有任何人读它
  // ⇒ 两块面板同时挂在屏幕上（2026-09-24 真机：连续点两条消息的表情入口，两个面板并存 ——
  // 当时全仓 8 个浮层里只有表情面板漏了这条 watch）。
  const { readFileSync, readdirSync } = await import("node:fs");
  const { join } = await import("node:path");
  const files: string[] = [];
  const walk = (d: string) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith(".vue")) files.push(p);
    }
  };
  walk(join(import.meta.dirname, "..", "components"));
  const missing: string[] = [];
  for (const f of files) {
    const src = readFileSync(f, "utf8");
    for (const m of src.matchAll(/const (\w+) = useExclusivePopup\(/g)) {
      const v = m[1];
      if (!new RegExp(`watch\\(\\s*${v}\\.isActive`).test(src)) {
        missing.push(`${f.slice(f.indexOf("components"))} → ${v}`);
      }
    }
  }
  assert.deepEqual(missing, [], "这些浮层只 claim 不监听被抢 ⇒ 同类缺陷会原地复发");
});
