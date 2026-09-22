import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { zhCN, enUS } from "./locales.ts";
import {
  applyPreference,
  currentLocale,
  currentPreference,
  detectSystemLocale,
  isLanguagePreference,
  isLocale,
  refreshSystemLocale,
  t,
} from "./index.ts";

// ---------------- 字典护栏：中英 key 必须一一对应（漏翻译会被逮到） ----------------

test("zh-CN 与 en-US 的 key 集合完全一致（防漏翻译）", () => {
  const zh = Object.keys(zhCN).sort();
  const en = Object.keys(enUS).sort();
  const onlyZh = zh.filter((k) => !enUS[k]);
  const onlyEn = en.filter((k) => !zhCN[k]);
  assert.deepEqual(onlyZh, [], `仅中文有、英文缺失的 key：${onlyZh.join(", ")}`);
  assert.deepEqual(onlyEn, [], `仅英文有、中文缺失的 key：${onlyEn.join(", ")}`);
});

test("字典值非空（不允许空字符串占位）", () => {
  for (const [k, v] of Object.entries(zhCN)) assert.ok(v.length > 0, `zh-CN ${k} 为空`);
  for (const [k, v] of Object.entries(enUS)) assert.ok(v.length > 0, `en-US ${k} 为空`);
});

// ---------------- 系统语言检测（纯函数） ----------------

test("detectSystemLocale：zh* → 中文，en* → 英文，非中英 → 英文", () => {
  assert.equal(detectSystemLocale(["zh-CN"]), "zh-CN");
  assert.equal(detectSystemLocale(["zh-Hans-CN"]), "zh-CN");
  assert.equal(detectSystemLocale(["zh-TW"]), "zh-CN");
  assert.equal(detectSystemLocale(["en-US"]), "en-US");
  assert.equal(detectSystemLocale(["en-GB", "zh-CN"]), "en-US"); // 第一个命中 en
  assert.equal(detectSystemLocale(["ja-JP", "en-US"]), "en-US");
  assert.equal(detectSystemLocale(["ja-JP"]), "en-US"); // 非中英回落英文
  assert.equal(detectSystemLocale(["zh-CN", "en-US"]), "zh-CN"); // 第一个命中 zh
  assert.equal(detectSystemLocale([]), "en-US");
  assert.equal(detectSystemLocale([undefined, null, "zh-CN"]), "zh-CN");
});

// ---------------- t() 翻译与插值 ----------------

test("默认跟随系统：没有 navigator 时回落英文", () => {
  // ⚠️ 必须**真的**构造出"没有 navigator"的环境（用户 2026-09-17）：
  //   · Node ≥21 **自带全局 `navigator`**（本机 language === "zh-CN"），所以原测试名里
  //     "node 环境无 navigator" 这个前提早已不成立 —— 它只是**恰好**在 en-US 的机器
  //     （含 CI runner）上通过，而中文开发者本地必红。假绿把这条契约缺陷藏了很久。
  //   · 另：`applyPreference("system")` 并**不**重解析系统语言（重解析在
  //     `refreshSystemLocale`）。原测试靠"模块加载那一刻恰好没有 navigator"间接成立，
  //     这里显式重解析一次，才是在测它声称要测的东西。
  const saved = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", { value: undefined, configurable: true });
  try {
    applyPreference("system");
    refreshSystemLocale();
    assert.equal(currentPreference(), "system");
    assert.equal(t("nav.chats"), "Chats");
  } finally {
    if (saved) Object.defineProperty(globalThis, "navigator", saved);
    refreshSystemLocale(); // 别把"无 navigator"的环境泄漏给后面的用例
  }
});

test("显式中文翻译", () => {
  applyPreference("zh-CN");
  assert.equal(t("nav.chats"), "聊天");
  assert.equal(t("settings.title"), "设置");
});

test("切换英文后立即生效", () => {
  applyPreference("en-US");
  assert.equal(t("nav.chats"), "Chats");
  assert.equal(t("settings.title"), "Settings");
});

test("插值：{n} 等占位符被替换", () => {
  applyPreference("zh-CN");
  assert.equal(t("nav.chats.unread", { n: 3 }), "聊天，3 条未读");
  applyPreference("en-US");
  assert.equal(t("nav.chats.unread", { n: 3 }), "Chats, 3 unread");
});

test("缺 key 回退为 key 本身（不抛错、不显示 undefined）", () => {
  assert.equal(t("no.such.key"), "no.such.key");
});

test("插值参数缺失时占位符保留原文", () => {
  applyPreference("zh-CN");
  assert.equal(t("nav.chats.unread"), "聊天，{n} 条未读");
});

// ---------------- 跟随系统：refreshSystemLocale 重解析 ----------------

function withNavigator(langs: string[], fn: () => void) {
  const saved = (globalThis as Record<string, unknown>).navigator;
  Object.defineProperty(globalThis, "navigator", {
    value: { languages: langs, language: langs[0] },
    configurable: true,
    writable: true,
  });
  try {
    fn();
  } finally {
    if (saved === undefined) delete (globalThis as Record<string, unknown>).navigator;
    else Object.defineProperty(globalThis, "navigator", { value: saved, configurable: true, writable: true });
  }
}

test("跟随系统：系统语言变化后重解析生效", () => {
  applyPreference("system");
  withNavigator(["zh-CN"], () => {
    refreshSystemLocale();
    assert.equal(currentLocale(), "zh-CN");
    assert.equal(t("nav.chats"), "聊天");
  });
  withNavigator(["en-US"], () => {
    refreshSystemLocale();
    assert.equal(currentLocale(), "en-US");
    assert.equal(t("nav.chats"), "Chats");
  });
  applyPreference("zh-CN"); // 还原
});

test("显式偏好不受系统语言变化影响", () => {
  applyPreference("zh-CN");
  withNavigator(["en-US"], () => {
    refreshSystemLocale();
    assert.equal(currentLocale(), "zh-CN");
  });
  applyPreference("zh-CN");
});

// ---------------- 校验 ----------------

test("isLocale 只认两种合法语言", () => {
  assert.equal(isLocale("zh-CN"), true);
  assert.equal(isLocale("en-US"), true);
  for (const v of ["", "en", "中文", "zh", null, undefined, 1]) {
    assert.equal(isLocale(v), false, `${String(v)} 不应判为合法语言`);
  }
});

test("isLanguagePreference 认三态", () => {
  for (const v of ["system", "zh-CN", "en-US"]) {
    assert.equal(isLanguagePreference(v), true, `${v} 应为合法偏好`);
  }
  for (const v of ["", "auto", "en", null, undefined, 1]) {
    assert.equal(isLanguagePreference(v), false, `${String(v)} 不应判为合法偏好`);
  }
});

test("currentPreference / currentLocale 反映最近一次 applyPreference", () => {
  applyPreference("en-US");
  assert.equal(currentPreference(), "en-US");
  assert.equal(currentLocale(), "en-US");
  applyPreference("zh-CN");
  assert.equal(currentLocale(), "zh-CN");
});

/**
 * 启动时必须把**解析后**的语言推给后端。
 *
 * 为什么必须守（用户 2026-09-16 实测「加群的提示怎么是英文？」）：
 * 后端自己生成的文案（群成员变更 / 文件下载 / 托盘提示 / 窗口标题）都按 `AppState::is_zh()`
 * 选语言，而「跟随系统」的解析规则（`navigator.language`）**只在前端有一份**，
 * 后端的兜底（`LANG` 等 POSIX 环境变量）在 Windows 上恒为「否」。
 * 所以「前端启动时推一次」是这条链路的**唯一**入口：漏了它，默认设置（跟随系统）的中文用户
 * 会把所有后端文案看成英文，而界面本身是中文 —— 这类"两边不一致"只有真机才看得见。
 */
test("app.init() 必须把解析后的语言推给后端（后端系统消息文案依赖它）", () => {
  const store = readFileSync(
    join(import.meta.dirname, "..", "stores", "useAppStore.ts"),
    "utf8",
  );
  const start = store.indexOf("async function init()");
  assert.ok(start > 0, "找不到 app store 的 init()");
  // 截出 init 的函数体：到下一个同级函数声明（缩进两空格的 `function` / `async function`）为止
  const rest = store.slice(start);
  const next = rest.slice(1).search(/\n {2}(?:async )?function /);
  const body = next > 0 ? rest.slice(0, next + 1) : rest;
  assert.match(
    body,
    /pushUiLanguage\(\)/,
    "init() 里必须调用 pushUiLanguage()：否则「跟随系统」（默认）的用户，后端永远不知道界面是中文，\n" +
      "群成员变更 / 文件下载等系统消息会显示英文（Rust 侧的 is_zh() 详见 state.rs 的 resolve_is_zh）。",
  );
});

// ---------------- 后端机器可读枚举 ↔ 界面文案 ----------------

/**
 * 中继探测的三档结论（`unreachable` / `rejected` / `held`）是**按串取文案**的：
 * `t(\`settings.network.relayServer.probe.${kind}\`)`。所以两头任一漂移都会变成
 * "那一档什么都不显示" —— 而它是三档里唯一告诉用户"配置是对的、问题在对方"的那一档。
 *
 * 这条断言把三件事钉在一起：Rust 的 `as_str` 产出的串、TS 里的联合类型、两种语言的键。
 * 任何一处改名，这里必须红（不是"少一行翻译"那么轻）。
 */
test("中继探测档位的三个名字在 Rust / TS / 两种语言里是同一套", () => {
  const root = join(import.meta.dirname, "../..");
  const rust = readFileSync(join(root, "src-tauri/src/network/transport/relay.rs"), "utf8");
  // `Self::Held => "held"` 这种臂，只在 `as_str` 那个 match 里出现
  const asStr = rust.slice(rust.indexOf("pub fn as_str"));
  const kinds = [...asStr.matchAll(/Self::\w+\s*=>\s*"([a-z_]+)"/g)].map((m) => m[1]).sort();
  assert.ok(kinds.length >= 3, `没从 Rust 里扫到档位串，护栏要失效了：${kinds}`);

  const tsTypes = readFileSync(join(root, "src/types.ts"), "utf8");
  const iface = tsTypes.slice(tsTypes.indexOf("export interface RelayProbe"));
  const union = [...iface.matchAll(/kind:\s*"([a-z_|"\s]+)"/g)][0][1];
  const tsKinds = union.split("|").map((s) => s.replace(/["\s]/g, "")).filter(Boolean).sort();
  assert.ok(tsKinds.length >= 3, `RelayProbe.kind 的联合类型没解析出来：${iface.slice(0, 80)}`);

  // TS 侧必须是 Rust 侧的子集（Rust 多出来的档位 = 前端不会遇到的分支，允许）
  for (const k of tsKinds) {
    assert.ok(kinds.includes(k), `前端声明了 Rust 不产生的档位：${k}`);
    for (const [name, dict] of [["zh-CN", zhCN], ["en-US", enUS]] as const) {
      assert.ok(
        dict[`settings.network.relayServer.probe.${k}`],
        `${name} 缺 settings.network.relayServer.probe.${k} —— 界面上那一档会变成空白`,
      );
    }
  }
});
