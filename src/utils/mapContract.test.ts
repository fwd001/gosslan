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
import { readdirSync, readFileSync } from "node:fs";
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

/** 门面包装的名字（`sendFile: (…)` 这种键）。 */
function facadeWrappers(): { cmd: string; key: string }[] {
  const src = readFileSync(API_FILE, "utf8");
  const out: { cmd: string; key: string }[] = [];
  // 只认 `key: (…) => invoke<T>("cmd", …)` 这一种形状（门面里全部 134 条都是这个形状）
  for (const m of src.matchAll(/(\w+):\s*\([^)]*\)[^]*?invoke(?:<[^>]*>)?\(\s*"([a-z0-9_]+)"/g)) {
    out.push({ key: m[1], cmd: m[2] });
  }
  return out;
}

/** 除 api 层自己以外，全仓 `src/` 下的 ts/vue 文件。 */
function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, e.name);
    if (e.isDirectory()) {
      if (full === join(ROOT, "src", "api")) continue;
      sourceFiles(full, acc);
    } else if (/\.(ts|vue)$/.test(e.name) && !e.name.endsWith(".test.ts")) {
      acc.push(full);
    }
  }
  return acc;
}

/**
 * 第三段对账：门面里的每条包装，界面（或 store）里必须真的有人引用。
 *
 * 为什么值得单独钉（0-A3 那批的遗留）：注册表 ↔ 门面这一层当时已经收口了，
 * 但**门面之下**还漏着五个"谁都不调"的包装 —— 它们守住的判据看不见这一层，
 * 于是这些命令变成了"注册着、能 IPC、永远没人按"的第三条入口。
 * 真正的危险不是白占几十行，而是**它们不会跟着主路径一起被改**：
 * 主路径后来加的 attempt epoch、终态契约、消毒口径，那五条一个都没吃到，
 * 哪天有人照着它们抄一份，就抄到一份旧的、少了几道闸的实现。
 *
 * ⚠️ 已知边界：这一层判的是"`api.K` 有没有被引用"，不是"有没有 UI 可达" ——
 * 一条 store 方法只被另一条没人调的 store 方法引用，仍会在这里被判成活的。
 * 要真判可达得建调用图，收益不值这片代码；需要时按具体案例 grep。
 */
test("门面之下不许留没人引用的包装（第三条入口会停在与主路径不同的位置）", () => {
  const ALLOWED_UNREFERENCED: Record<string, string> = {
    // 例外必须写理由，空表是默认状态。
  };
  const files = sourceFiles(join(ROOT, "src"));
  // 必须先把空白压平再匹配：真调用点常常写成 `api` 换行 `.openImagePreview(...)`，
  // 按行匹配的守卫会把它们误报成死代码（这个坑本仓已经记过一次，见契约图 0-A3 那条漂移行）。
  const bodies = files.map((f) => readFileSync(f, "utf8").replace(/\s+/g, " "));
  const dead: string[] = [];
  for (const { key, cmd } of facadeWrappers()) {
    const re = new RegExp(`api\\s*\\.\\s*${key}\\b`);
    if (bodies.some((b) => re.test(b))) continue;
    if (key in ALLOWED_UNREFERENCED) continue;
    dead.push(`${key}(${cmd})`);
  }
  assert.deepEqual(
    dead,
    [],
    `这些包装全仓没人引用（既不在界面上，也没在 store 里）：${dead.join(" · ")}`,
  );
});
