import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  APPEARANCE_STORAGE_KEY,
  LEGACY_DARK_STORAGE_KEY,
  isAppearanceMode,
  readStoredAppearance,
  resolveDark,
  type ReadonlyStorage,
} from "./appearance.ts";

// ---------------- 解析规则：用户意图 × 系统偏好 ----------------

test("resolveDark：跟随系统时取系统值", () => {
  assert.equal(resolveDark("system", true), true);
  assert.equal(resolveDark("system", false), false);
});

test("resolveDark：强制模式不随系统变化（用户选了就不该被系统覆盖）", () => {
  assert.equal(resolveDark("dark", false), true, "系统浅色也不能把强制深色改掉");
  assert.equal(resolveDark("dark", true), true);
  assert.equal(resolveDark("light", true), false, "系统深色也不能把强制浅色改掉");
  assert.equal(resolveDark("light", false), false);
});

test("isAppearanceMode：只认三种合法值", () => {
  for (const v of ["system", "light", "dark"]) assert.equal(isAppearanceMode(v), true);
  for (const v of ["", "System", "auto", "夜间", null, undefined, 1]) {
    assert.equal(isAppearanceMode(v), false, `${String(v)} 不应被判为合法模式`);
  }
});

// ---------------- 老数据迁移：不能把用户此前的选择静默丢掉 ----------------

const store = (m: Record<string, string>): ReadonlyStorage => ({
  getItem: (k) => (k in m ? m[k] : null),
});

test("显式模式原样读回", () => {
  assert.equal(readStoredAppearance(store({ [APPEARANCE_STORAGE_KEY]: "system" })), "system");
  assert.equal(readStoredAppearance(store({ [APPEARANCE_STORAGE_KEY]: "light" })), "light");
  assert.equal(readStoredAppearance(store({ [APPEARANCE_STORAGE_KEY]: "dark" })), "dark");
});

test("旧版本只存了布尔值 → 迁移为显式模式（而不是当成'跟随系统'）", () => {
  assert.equal(readStoredAppearance(store({ [LEGACY_DARK_STORAGE_KEY]: "1" })), "dark");
  assert.equal(readStoredAppearance(store({ [LEGACY_DARK_STORAGE_KEY]: "0" })), "light");
});

test("显式模式优先于残留的旧布尔值", () => {
  const both = { [APPEARANCE_STORAGE_KEY]: "system", [LEGACY_DARK_STORAGE_KEY]: "1" };
  assert.equal(readStoredAppearance(store(both)), "system", "用户后来选了跟随系统，就不能被旧值拽回深色");
});

test("什么都没有 → 跟随系统（默认值）", () => {
  assert.equal(readStoredAppearance(store({})), "system");
});

test("存储值非法（脏数据）→ 退回旧布尔值，再退回跟随系统", () => {
  assert.equal(readStoredAppearance(store({ [APPEARANCE_STORAGE_KEY]: "auto" })), "system");
  assert.equal(
    readStoredAppearance(store({ [APPEARANCE_STORAGE_KEY]: "auto", [LEGACY_DARK_STORAGE_KEY]: "1" })),
    "dark",
    "显式值损坏时应继续尝试旧值，而不是直接放弃",
  );
});

// ---------------- 与首屏脚本（src/boot/theme-boot.js）交叉验证 ----------------
//
// 这是**本轮新增的关键护栏**。外观规则有两份实现：
//   ① useAppStore（本模块）；
//   ② 首屏脚本 —— 它必须先于 bundle 执行（否则启动会"闪一下白"），因此无法 import 本模块。
// 两份漂移的症状是"骨架与真界面外观不一致"，只在启动瞬间出现、极难自测发现。
// 与其在注释里写"两处必须一致"，不如**把那段脚本真跑一遍**来对照。
//
// ⚠️ 脚本从 `index.html` 内联搬到了 `src/boot/theme-boot.js`（三个窗口共用一份，
//    由 vite 插件内联回各自的 HTML）—— 因为"主/设置/日志三个窗口各抄一份主题逻辑"
//    迟早会漂移。本护栏直接读那份**事实来源**，比读某个 HTML 里的拷贝更准。

/** 取出首屏主题脚本（三窗口共用的事实来源）。 */
function readBootScript(): string {
  return readFileSync(join(import.meta.dirname, "..", "boot", "theme-boot.js"), "utf8");
}

/** 在替身环境里执行首屏脚本，返回它最终是否加了 dark 类。 */
function runBootScript(env: {
  appearance?: string | null;
  legacyDark?: string | null;
  systemDark: boolean;
}): boolean {
  const values: Record<string, string> = {};
  if (env.appearance != null) values[APPEARANCE_STORAGE_KEY] = env.appearance;
  if (env.legacyDark != null) values[LEGACY_DARK_STORAGE_KEY] = env.legacyDark;

  const added = new Set<string>();
  const g = globalThis as unknown as Record<string, unknown>;
  const saved = { ls: g.localStorage, doc: g.document, win: g.window };
  g.localStorage = { getItem: (k: string) => values[k] ?? null };
  g.document = {
    documentElement: {
      classList: { add: (c: string) => added.add(c) },
      style: { setProperty: () => {} },
    },
  };
  g.window = { matchMedia: () => ({ matches: env.systemDark }) };
  try {
    // eslint-disable-next-line @typescript-eslint/no-implied-eval
    new Function(readBootScript())();
  } finally {
    g.localStorage = saved.ls;
    g.document = saved.doc;
    g.window = saved.win;
  }
  return added.has("dark");
}

test("首屏脚本（src/boot/theme-boot.js）与 utils/appearance 的解析结果**逐例一致**", () => {
  const cases = [
    { appearance: "system", legacyDark: null, systemDark: true },
    { appearance: "system", legacyDark: null, systemDark: false },
    { appearance: "dark", legacyDark: null, systemDark: false },
    { appearance: "light", legacyDark: null, systemDark: true },
    { appearance: null, legacyDark: "1", systemDark: false },
    { appearance: null, legacyDark: "0", systemDark: true },
    { appearance: null, legacyDark: null, systemDark: true },
    { appearance: null, legacyDark: null, systemDark: false },
    // 脏数据 + 旧值：两份实现都要走到"先看显式、再退回旧布尔"的顺序
    { appearance: "auto", legacyDark: "1", systemDark: false },
    { appearance: "auto", legacyDark: "0", systemDark: true },
  ];
  for (const c of cases) {
    const expected = resolveDark(
      readStoredAppearance(store({
        ...(c.appearance != null ? { [APPEARANCE_STORAGE_KEY]: c.appearance } : {}),
        ...(c.legacyDark != null ? { [LEGACY_DARK_STORAGE_KEY]: c.legacyDark } : {}),
      })),
      c.systemDark,
    );
    assert.equal(
      runBootScript(c),
      expected,
      `首屏脚本与 store 的判定不一致：appearance=${String(c.appearance)} legacy=${String(c.legacyDark)} ` +
        `系统深色=${c.systemDark} → 脚本=${runBootScript(c)}，期望=${expected}`,
    );
  }
});

test("首屏脚本不依赖 localStorage 可用（隐私模式抛错时按浅色渲染，不崩）", () => {
  const g = globalThis as unknown as Record<string, unknown>;
  const saved = { doc: g.document, win: g.window };
  g.document = { documentElement: { classList: { add: () => {} }, style: { setProperty: () => {} } } };
  g.window = { matchMedia: () => ({ matches: true }) };
  // 只在 window 上放 matchMedia、不给 localStorage → getItem 抛错，脚本必须自行兜住
  const savedLs = g.localStorage;
  delete g.localStorage;
  try {
    assert.doesNotThrow(() => new Function(readBootScript())(), "localStorage 不可用时不应抛错");
  } finally {
    g.localStorage = savedLs;
    g.document = saved.doc;
    g.window = saved.win;
  }
});
