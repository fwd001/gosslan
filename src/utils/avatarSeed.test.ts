// 默认头像的种子（#32）：emoji 小动物 × 适配背景色，纯函数、跨端可复算。
// 跑法：node --test src/utils/avatarSeed.test.ts
import test from "node:test";
import assert from "node:assert/strict";
import { AVATAR_PAIRS, avatarSeedFor } from "./avatarSeed.ts";

/** 相对亮度（WCAG 的那条），用来判"这个背景色既不至于亮到看不见 emoji、也不至于暗到吃字"。 */
function relLuminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
}

test("结果不许读时钟或随机源：把它们换成乱跳的实现，输出必须一模一样", () => {
  const first = avatarSeedFor("gosslan-23fe9c32ee9fc");
  assert.ok(first, "合法 id 不该返回 null");
  const realNow = Date.now;
  const realRandom = Math.random;
  try {
    let n = 0;
    // 只重复调用不算判据：同一毫秒里跑 50 次，混进 Date.now() 的实现照样"通过"（实测过这个空转）。
    Date.now = () => 1_700_000_000_000 + n++ * 86_400_000;
    Math.random = () => (n++ % 7) / 7;
    for (let i = 0; i < 20; i++) {
      assert.deepEqual(avatarSeedFor("gosslan-23fe9c32ee9fc"), first,
        "结果里混进了时钟或随机源 —— 那会让同一个人每次上线换一张脸");
    }
  } finally {
    Date.now = realNow;
    Math.random = realRandom;
  }
});

test("id 缺失时返回 null，让调用方走自己的兜底，不许悄悄给一张默认脸", () => {
  assert.equal(avatarSeedFor(""), null);
  assert.equal(avatarSeedFor(null), null);
  assert.equal(avatarSeedFor(undefined), null);
});

test("组合表至少 100 对（条数现算，不手抄）", () => {
  assert.ok(AVATAR_PAIRS.length >= 100, `实际只有 ${AVATAR_PAIRS.length} 对`);
  const seen = new Set(AVATAR_PAIRS.map((p) => `${p.emoji}|${p.bg}`));
  assert.equal(seen.size, AVATAR_PAIRS.length, "表里有完全相同的两行 —— 那等于少一种头像");
});

test("每一对的背景色都得落在中间调：太亮或太暗都会让 emoji 看不清", () => {
  const bad = AVATAR_PAIRS.filter((p) => {
    const l = relLuminance(p.bg);
    return !(l >= 0.18 && l <= 0.72);
  });
  assert.deepEqual(bad.map((p) => `${p.emoji} ${p.bg}`), [],
    "这些背景色不在中间调（相对亮度 0.18–0.72）");
});

test("每个 bg 都得是 #rrggbb —— 写成 rgb()/命名色会让上面那条判据静默失效", () => {
  const bad = AVATAR_PAIRS.filter((p) => !/^#[0-9a-f]{6}$/.test(p.bg));
  assert.deepEqual(bad.map((p) => p.bg), [], "背景色必须是 6 位十六进制小写");
});

test("相邻 id 不该撞脸：造 200 个 id，去重后至少要有 50 种不同结果", () => {
  const kinds = new Set<string>();
  for (let i = 0; i < 200; i++) {
    const s = avatarSeedFor(`gosslan-device-${i}`);
    assert.ok(s);
    kinds.add(`${s!.emoji}|${s!.bg}`);
  }
  assert.ok(kinds.size >= 50, `200 个 id 只散出 ${kinds.size} 种头像 —— 哈希或取模写错了`);
});

test("结果只能取自表里的行，不能是拼出来的第三种东西", () => {
  for (const id of ["a", "gosslan-1", "dev:Mac-wendon"]) {
    const s = avatarSeedFor(id)!;
    assert.ok(AVATAR_PAIRS.some((p) => p.emoji === s.emoji && p.bg === s.bg),
      `${id} 的结果不在表里`);
  }
});
