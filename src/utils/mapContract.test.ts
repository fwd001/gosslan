/**
 * 契约图对账：图里**手抄的那几份清单**必须与仓库现状双向一致。
 *
 * ## 为什么需要它
 * `docs/ARCHITECTURE-MAP.html` 是用户定的改码前必看/必同步的那张图（"每次修改代码都要回看它，
 * 改完把图同步对"）。但图里那些清单是**手工维护的第二份事实源**，而本仓库反复踩过同一类坑：
 * 第二份没人核对时，它不是"稍微旧一点"，而是**让所有人从界面上读到一个恒定错值**
 * （`get_topology` 的「N 中继」永远是 0 就是同一形状的代码版）。
 *
 * ## 三条判据各自拦的是哪种事故
 * 1. **`invoke("x")` 的名字必须在 `generate_handler!` 里注册过** —— 这条最硬：
 *    名字拼错**编译通过、类型通过、构建通过**，只在运行时表现为
 *    "command not found"，用户看到的就是"点了没反应/一直转圈"。
 * 2. **注册了的命令必须上图**（漏登记 = 图失去作用，改图的人看不见这一条边）。
 * 3. **上图了的命令必须真的注册**（图会骗人，而且是在人最容易相信它的地方骗人）。
 *
 * ⚠️ 已知不覆盖：图的其余手抄数字（事件 29 条、表 19 张、用例数）仍靠人诚实 ——
 * 那些是"说明性数字"而不是一一对应的清单，钉成判据要先把清单本身建出来。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dirname, "..", "..");
const MAP_FILE = join(ROOT, "docs", "ARCHITECTURE-MAP.html");
const LIB_RS = join(ROOT, "src-tauri", "src", "lib.rs");
const API_FILE = join(ROOT, "src", "api", "index.ts");

/** `generate_handler![ commands::a, commands::b, … ]` 里注册的那一份。 */
function registeredCommands(): string[] {
  const src = readFileSync(LIB_RS, "utf8");
  const start = src.indexOf("generate_handler![");
  assert.ok(start > 0, "lib.rs 里找不到 generate_handler! —— 注册方式变了要同步这条守卫");
  const seg = src.slice(start, src.indexOf("\n        ]", start));
  assert.ok(seg.length > 500, "generate_handler! 的切片过短，边界没找对");
  return [...seg.matchAll(/commands::([a-z0-9_]+)/g)].map((m) => m[1]);
}

/** 契约图 `const CMDS = [ ["名字", "领域", "入参", "返回"], … ]` 的第一列。 */
function mappedCommands(): string[] {
  const html = readFileSync(MAP_FILE, "utf8");
  const at = html.indexOf("const CMDS = [");
  assert.ok(at > 0, "契约图里找不到 `const CMDS = [` —— 图的形状变了要同步这条守卫");
  const seg = html.slice(at, html.indexOf("\n];", at));
  return [...seg.matchAll(/\["([a-z0-9_]+)"/g)].map((m) => m[1]);
}

/** 前端 api 层真正 invoke 的那一份（`invoke("x")` / `invoke<T>("x", {…})`）。 */
function invokedCommands(): string[] {
  const src = readFileSync(API_FILE, "utf8");
  return [...src.matchAll(/invoke(?:<[^>]*>)?\(\s*"([a-z0-9_]+)"/g)].map((m) => m[1]);
}

function dupes(names: string[]): string[] {
  return [...new Set(names.filter((n, i) => names.indexOf(n) !== i))];
}

test("前端 invoke 的每个命令名都必须在后端注册过（拼错只会运行时炸）", () => {
  const registered = new Set(registeredCommands());
  const invoked = invokedCommands();
  assert.ok(invoked.length > 100, `api 层只解析出 ${invoked.length} 条 invoke，判据前提塌了`);
  const missing = [...new Set(invoked.filter((n) => !registered.has(n)))];
  assert.deepEqual(
    missing,
    [],
    `这些命令前端在调、后端没注册 ⇒ 调用必然 reject（用户看到"点了没反应"）：${missing.join(", ")}`,
  );
});

test("注册命令与契约图 IPC 表双向一致，且两侧都没有重复条目", () => {
  const registered = registeredCommands();
  const mapped = mappedCommands();
  assert.deepEqual(dupes(registered), [], "generate_handler! 里重复注册同一条命令");
  assert.deepEqual(dupes(mapped), [], "契约图 IPC 表里同一命令上了两行（统计与检索都会_double_）");
  const notOnMap = registered.filter((n) => !mapped.includes(n));
  const notRegistered = mapped.filter((n) => !registered.includes(n));
  assert.deepEqual(
    notOnMap,
    [],
    `新增了注册但契约图没登记（图是改码前看影响面的那份依据，漏一条就漏看一整片）：${notOnMap.join(", ")}`,
  );
  assert.deepEqual(
    notRegistered,
    [],
    `契约图上有、后端没注册（图在骗人，而且骗的是最容易相信它的地方）：${notRegistered.join(", ")}`,
  );
});
