import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  TOKEN_CONTRAST_CONTRACT,
  findTokenContrastFailures,
  parseTokenScopes,
  resolveToken,
} from "./tokenContrast.ts";

// ---------------- 检查器自身：故意写坏的 CSS 必须被抓住 ----------------
//
// 先证明"尺子准"，再用它量真实样式表。否则护栏可能是永远绿的摆设。

const BROKEN = `
:root {
  --gosslan-text-2: #64748b;
  --gosslan-panel: #ffffff;
  --gosslan-list: #f1f5f9;
  --gosslan-chat: #fafafa;
  --gosslan-card: #eeeef0;
  --gosslan-text: #0f172a;
  --gosslan-list-active: #dbe3ed;
  --gosslan-list-active-text: var(--gosslan-text);
  --gosslan-accent-ink: #2d6099;
  --gosslan-success-ink: #047857;
  --gosslan-danger-ink: #ff3b30;
  --gosslan-warning-ink: #ff9500;
  --gosslan-status-offline: #a3a3a3;
  --gosslan-danger-soft: rgba(255, 59, 48, 0.12);
  --gosslan-hud: rgba(38, 38, 38, 0.9);
}
.dark {
  --gosslan-text-2: #94a3b8;
  --gosslan-panel: #1e293b;
  --gosslan-list: #1e293b;
  --gosslan-chat: #0f172a;
  --gosslan-card: #1c2434;
  --gosslan-text: #f1f5f9;
  --gosslan-list-active: #334155;
  --gosslan-list-active-text: var(--gosslan-text);
  --gosslan-accent-ink: #8fb3ea;
  --gosslan-success-ink: #30d158;
  --gosslan-danger-ink: #ff5548;
  --gosslan-warning-ink: #ff9f0a;
  --gosslan-status-offline: #8e8e93;
  --gosslan-danger-soft: rgba(255, 85, 72, 0.18);
  --gosslan-hud: rgba(72, 72, 74, 0.92);
}
`;

test("检查器能抓出「改回旧值」的四处违规", () => {
  const fails = findTokenContrastFailures(BROKEN);
  const keys = fails.map((f) => `${f.fg} on ${f.bg}`);
  // 亮色：次要文字（卡片/列表底）、危险色当文字、琥珀当图标、离线点
  assert.ok(keys.includes("--gosslan-text-2 on --gosslan-card"), `未抓到 text-2/card：${keys.join(", ")}`);
  assert.ok(keys.includes("--gosslan-text-2 on --gosslan-list"), "未抓到 text-2/list");
  assert.ok(keys.includes("--gosslan-danger-ink on --gosslan-panel"), "未抓到 danger 当文字");
  assert.ok(keys.includes("--gosslan-warning-ink on --gosslan-panel"), "未抓到 warning 当图标");
  assert.ok(keys.includes("--gosslan-status-offline on --gosslan-panel"), "未抓到离线点");
  for (const f of fails) assert.ok(f.ratio < f.min, "报出的组合必须确实不达标");
});

test("var() 链会被解析（list-active-text 指向 text）", () => {
  const { light } = parseTokenScopes(BROKEN);
  assert.equal(light.get("--gosslan-list-active-text"), "var(--gosslan-text)");
  // 解析到 --gosslan-text 的实值，而不是把 "var(...)" 当成颜色
  const resolved = resolveToken("--gosslan-list-active-text", light);
  assert.ok(resolved, "应能解析 var() 链");
  assert.equal(`${resolved.r},${resolved.g},${resolved.b}`, "15,23,42");
});

test("color-mix 派生的 hover 档要参与计算：朝白混必须报红，朝暗混才放行", () => {
  // 这条夹具是"尺子自己"的判据。以前解析器认不出 color-mix 就返回 null，
  // findTokenContrastFailures 把 null 当"判不了"跳过 ⇒ 「红底 hover 变浅、白字更看不见」
  // 这一整类漂移是静默漏判的（护栏看着绿，其实那一行什么都没读）。
  const shape = (mixTo: string) => `
:root {
  --gosslan-primary-active: #0a4bb5;
  --gosslan-danger: #d43d43;
  --gosslan-danger-hover: color-mix(in srgb, var(--gosslan-danger) 88%, ${mixTo});
}
`;
  const hoverFails = (mixTo: string) =>
    findTokenContrastFailures(shape(mixTo)).filter((f) => f.bg === "--gosslan-danger-hover");

  const towardWhite = hoverFails("#ffffff");
  assert.equal(towardWhite.length, 1, "hover 朝白混 = 白字更看不见，这条必须报红而不是被跳过");
  assert.ok(towardWhite[0].ratio < 4.5, `报出的红要带真实比值，实际 ${towardWhite[0]?.ratio}`);

  const towardBlack = hoverFails("#000000");
  assert.equal(towardBlack.length, 0, "hover 朝暗混 = 对比度只升不降，不该报红");
});

test("注释里出现的 `.dark {` 不会把作用域解析带偏", () => {
  // 真实踩过：:root 的注释里写着「暗色版本不能靠 `.dark { --gosslan-primary: … }` 覆盖」，
  // 朴素的 indexOf(".dark {") 会先匹配到这句注释，于是 dark 解析到的是 :root 的内容。
  const cssWithComment = `
:root {
  /* 说明：.dark { --gosslan-primary: … } 不会生效（inline 优先级更高） */
  --gosslan-text-2: #475569;
  --gosslan-panel: #ffffff;
}
.dark {
  --gosslan-text-2: #94a3b8;
  --gosslan-panel: #1e293b;
}
`;
  const { light, dark } = parseTokenScopes(cssWithComment);
  assert.equal(light.get("--gosslan-panel"), "#ffffff");
  assert.equal(dark.get("--gosslan-panel"), "#1e293b", "dark 必须解析到真正的 .dark 块");
  assert.equal(dark.get("--gosslan-text-2"), "#94a3b8");
});

// ---------------- 真实样式表：契约必须全绿 ----------------

const css = readFileSync(join(import.meta.dirname, "..", "style.css"), "utf8");

test("style.css 的配色契约全部达标", () => {
  const fails = findTokenContrastFailures(css);
  const detail = fails
    .map((f) => `${f.fg} on ${f.bg} = ${f.ratio.toFixed(2)} < ${f.min}（${f.why}）`)
    .join("\n");
  assert.deepEqual(fails.map((f) => `${f.fg}/${f.bg}`), [], `存在不达标组合：\n${detail}`);
});

test("新增的 *-ink 档在 :root 与 .dark 都成对定义", () => {
  // 漏写 .dark 会让暗色继承亮色的档位 —— 方向正好反了（曾经踩过 accent-ink 这个坑）
  const { light, dark } = parseTokenScopes(css);
  const lightOnly = [...light.keys()].filter((k) => k.endsWith("-ink"));
  assert.ok(lightOnly.length >= 4, `应至少 4 个 -ink token，实际 ${lightOnly.length}`);
  for (const k of lightOnly) {
    assert.ok(dark.has(k), `${k} 在 .dark 里缺少定义`);
    assert.notEqual(light.get(k), dark.get(k), `${k} 两种外观同值，可能忘了按外观调明度`);
  }
});

test("亮色语义色的实际取值（钉住本次审计结论）", () => {
  const { light } = parseTokenScopes(css);
  assert.equal(light.get("--gosslan-text-2"), "#475569", "次要文字应为 Slate-600");
  assert.equal(light.get("--gosslan-danger-ink"), "#cc2418");
  assert.equal(light.get("--gosslan-warning-ink"), "#c67600");
  assert.equal(light.get("--gosslan-status-offline"), "#8e8e93");
  // 填充档：承担白字的那一族（danger）必须过 4.5，所以它**不**保持系统色
  // （#ff3b30 白字只有 3.55 ⇒ 2026-09-26 压深到 #d43d43 = 4.62）；
  // 不承担白字的填充（warning 橙，只当图标/进度条底）仍保持 Apple 系统色。
  assert.equal(light.get("--gosslan-danger"), "#d43d43");
  assert.equal(light.get("--gosslan-warning"), "#ff9500");
});

test("契约表两种外观都非空且最低标准合法", () => {
  for (const theme of ["light", "dark"] as const) {
    assert.ok(TOKEN_CONTRAST_CONTRACT[theme].length >= 10, `${theme} 契约条目过少`);
    for (const p of TOKEN_CONTRAST_CONTRACT[theme]) {
      assert.ok(p.min === 4.5 || p.min === 3.0, `${p.fg}/${p.bg} 的 min 不在 4.5(文字)/3.0(非文字) 两档`);
      assert.ok(p.why.length > 0, `${p.fg}/${p.bg} 缺少 why 说明`);
    }
  }
});
