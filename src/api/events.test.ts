/**
 * IPC 事件契约守卫：**Rust 发的事件必须有人听，前端听的事件必须有人发**。
 *
 * ## 为什么需要它
 * 真实缺陷（用户实测）：在独立「设置」窗口里改语言/主题，主窗口**一点变化都没有** ——
 * 因为设置窗口与主窗口是两个 WebView、各有自己的 store，改完之后**没有任何事件通知对方**。
 * 同一类问题还有：`group-message-acked` 一直有 Rust 侧在发，前端却从没监听
 * （群消息气泡的"已送达"只能等别的刷新才更新）。
 *
 * 这类缺陷**编译通过、测试全绿、界面看着正常**，只有真的去点才发现 ——
 * 正是最该由机器盯住的一类。反向也要查：前端监听了一个永远不发的事件，
 * 说明某处功能被悄悄摘掉了（或事件名拼错）。
 *
 * 事件名可以是**字符串字面量**（`emit("x", …)`）或**常量**（`emit(MENU_SETTINGS, …)`），
 * 两种都要认 —— 只认字面量的话，菜单事件会被误判成"没人发"。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { readCommandsSrc } from "../../scripts/rustSrc.ts";

const ROOT = join(import.meta.dirname, "..", "..");
const RUST_SRC = join(ROOT, "src-tauri", "src");
const API_FILE = join(ROOT, "src", "api", "index.ts");

/**
 * 已知的"故意不接"的事件：**每条都必须写清理由**，否则就是漏接。
 * 加新条目时请先问自己：真的不需要 UI 反应，还是只是还没接？
 */
const ALLOWED_EMIT_WITHOUT_LISTENER: Record<string, string> = {
  "group-key-received":
    "紧邻 `groups-updated` 一起发（同一条路径），前端只处理后者 —— 冗余事件，可后续删除",
  "group-file-log":
    "DevDiag 面板目前走 `get_discovery_diag` 轮询拿数据，该事件暂无消费者（保留待接）",
};

/** 只发给特定窗口、由该窗口自己监听的事件不算漏接（这里是菜单事件，前端已监听）。 */
const IGNORED_PREFIXES = ["menu://"];

/**
 * 只保留 Rust 源码里的**代码**，把注释换成等长空白（保留换行，行号不偏移）。
 *
 * 为什么必须有：这条护栏扫的是 `emit(` 这个形态，而形态出现在注释里并不等于"后端在发事件"。
 * v4.24.0 现场被咬两次 —— 先在断言文本里写全那三个字符加左括号，再在注释里解释
 * "为什么不能写全"，第二次照样被扫成一个没人听的孤儿事件；当时的处理是**改写文案绕开**，
 * 那是在躲症状。真正要补的是"扫描前先分清水份"，所以这里按语法边界剥注释。
 *
 * ⚠️ 已知残留，刻意不在本次顺手做：**字符串字面量里**出现完整的 `emit("x"` 形态仍会被扫到。
 * 不能把字符串一起抹掉 —— 我们要找的恰恰就是 `emit("x")` 里那个字符串本身。要做对得先把
 * 事件名收进常量表、再按名比对（那属 #33③ 的另一片），而不是在这里加一条"看到引号就砍"。
 */
function stripRustComments(src: string): string {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const d = src[i + 1] ?? "";
    // 字符串 / raw 字符串 / byte 字符串：整段原样跳过，否则串里的 `//` 会被当成注释吃掉后半文件
    if (c === '"' || (c === "b" && d === '"') || /^r#*"/.test(src.slice(i, i + 6))) {
      const end = skipString(src, i);
      out += src.slice(i, end);
      i = end;
      continue;
    }
    // 字符字面量（`'a'` / `'\n'`）整段跳过；生命周期 `'a` 后面没有闭合引号 ⇒ 不会被误吃
    const charLit = /^'(?:\\.|[^'\\])'/.exec(src.slice(i));
    if (charLit) {
      out += charLit[0];
      i += charLit[0].length;
      continue;
    }
    if (c === "/" && d === "/") {
      let j = src.indexOf("\n", i);
      if (j === -1) j = src.length;
      out += " ".repeat(j - i);
      i = j;
      continue;
    }
    if (c === "/" && d === "*") {
      // 块注释可嵌套（Rust 允许），深度归零前一路都当注释；换行保留以稳住行号
      let depth = 0;
      let j = i;
      while (j < src.length) {
        if (src[j] === "/" && src[j + 1] === "*") {
          depth += 1;
          j += 2;
          continue;
        }
        if (src[j] === "*" && src[j + 1] === "/") {
          depth -= 1;
          j += 2;
          if (depth === 0) break;
          continue;
        }
        j += 1;
      }
      out += src.slice(i, j).replace(/[^\n]/g, " ");
      i = j;
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
  function skipString(s: string, from: number): number {
    const raw = /^r#*"/.exec(s.slice(from));
    if (raw) {
      const closer = `"${"#".repeat(raw[0].length - 2)}`;
      const hit = s.indexOf(closer, from + raw[0].length);
      return hit === -1 ? s.length : hit + closer.length;
    }
    let j = from + 1;
    while (j < s.length) {
      if (s[j] === "\\") {
        j += 2;
        continue;
      }
      if (s[j] === '"') return j + 1;
      j += 1;
    }
    return s.length;
  }
}

/** 从一份 Rust 源码里取出「后端真的在发」的事件名（纯函数 ⇒ 能被 fixtures 直接喂）。 */
function emittedNamesFrom(src: string, consts: Map<string, string>): Set<string> {
  const out = new Set<string>();
  // emit("name"            / emit(CONST
  for (const m of stripRustComments(src).matchAll(/\bemit\w*\(\s*(?:"([^"]+)"|([A-Z][A-Z0-9_]*))/g)) {
    const name = m[1] ?? consts.get(m[2] ?? "");
    if (name) out.add(name);
  }
  return out;
}

function collectRustFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) collectRustFiles(full, out);
    else if (entry.name.endsWith(".rs")) out.push(full);
  }
  return out;
}

/** Rust 侧 `const NAME: &str = "value";` 的映射（事件名常用常量）。 */
function collectStringConsts(files: string[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const f of files) {
    const src = stripRustComments(readFileSync(f, "utf8"));
    for (const m of src.matchAll(/const\s+([A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*"([^"]+)"/g)) {
      map.set(m[1], m[2]);
    }
  }
  return map;
}

/** Rust 侧实际发出的事件名（`emit` / `emit_to`，字面量或常量）。 */
function rustEmittedEvents(): Set<string> {
  const files = collectRustFiles(RUST_SRC);
  const consts = collectStringConsts(files);
  const out = new Set<string>();
  for (const f of files) {
    for (const name of emittedNamesFrom(readFileSync(f, "utf8"), consts)) out.add(name);
  }
  return out;
}

/** 前端 `listen<…>("name", …)` 监听的事件名。 */
function frontendListenedEvents(): Set<string> {
  const src = readFileSync(API_FILE, "utf8");
  const out = new Set<string>();
  for (const m of src.matchAll(/listen(?:<[^>]*>)?\(\s*"([^"]+)"/g)) out.add(m[1]);
  return out;
}

/// 扫描器的"水分判据"：一份喂料同时含真发送与各种注释里的假形态，
/// 结果必须**只**有真的那几个 —— 单向断言（只查"注释不算"）会被"整段都被抹掉"
/// 这种过度剥离糊过去，所以正反两面都钉在这里。
test("事件扫描器不被注释骗，也不误伤代码（含串内 // 与生命周期、嵌套块注释）", () => {
  const sample = [
    'app.emit("real-line", &p); // 行尾注释里写 emit("phantom-trailing") 也不算',
    '// emit("phantom-line") —— 解释"为什么不能把那三个字符连左括号写全"的那条注释',
    '/// 文档里提 emit("phantom-doc") 一样不是发送',
    '/* emit("phantom-block") */',
    '/* 外层 /* 内层 emit("phantom-nested") */ 还在注释里 */',
    'let s = "字符串里的 // 不是注释"; app.emit("real-after-string", &p);',
    'let r = r#"原始串里的 // 和 " 都不算注释"#; app.emit("real-after-raw", &p);',
    // 原始串里引号数**为奇数**：不认识 `r#"` 的扫描器会从第一个 `"` 起两两配对配错相位，
    // 于是串里那个 `//` 落到"代码"里，把整行（含真 emit）当行注释吃掉 ⇒ **少报真事件**。
    // ⚠️ 这条的变异点在 `skipString` 里那一句 `const raw = /^r#*"/`，**不在**外层那段：
    // 外层 `/^r#*"/.test(...)` 与 `skipString` 内部的 raw 识别是**冗余的两处**，
    // 单独改坏外层那处它会被 `skipString` 的回落救回来（实测整组仍绿）——
    // 所以"改了外层会不会红"这种直觉在这里是错的，别拿它当护栏存在与否的证据。
    'let q = r#"引号 " 和斜杠 // 都在原始串里"#; app.emit("real-after-raw-odd-quote", &p);',
    "fn probe<'a>(x: &'a str) { app.emit(\"real-with-lifetime\", x) }",
  ].join("\n");
  const found = [...emittedNamesFrom(sample, new Map())].sort();
  assert.deepEqual(found, [
    "real-after-raw",
    "real-after-raw-odd-quote",
    "real-after-string",
    "real-line",
    "real-with-lifetime",
  ]);
  // ⚠️ 这条顺带钉住扫描器的**已知边界**：只认"事件名是第一个实参"的形态
  // （`emit("x")` / `emit_filter("x")`）。本仓 `emit_to` 用了 0 次 ⇒ 不为此扩正则。
  // 注意：**下面那条 `[]` 断言自己不会响**（它断言的就是"扫不到"）—— 会响的是紧随其后的绊线。
  assert.deepEqual([...emittedNamesFrom('app.emit_to(w, "not-scanned", p);', new Map())], []);
  // 绊线：把"边界只写在注释里"换成"边界会红"。哪天 Rust 侧真的开始用 emit_to(target, "x")，
  // 扫描器会**静默少报**事件 ⇒ 契约测试把真实事件当成"前端独有的监听"放过。
  // 这条断言存在的意义就是让那种改动必须显式处理（扩正则 + 同步改上面那条 `[]` 夹具）。
  const emitToSites = collectRustFiles(RUST_SRC).filter((f) =>
    /\bemit_to\s*\(/.test(stripRustComments(readFileSync(f, "utf8"))),
  );
  assert.deepEqual(
    emitToSites,
    [],
    "Rust 侧出现了 emit_to(...)：扫描器只认「事件名是第一个实参」，会静默少报。" +
      "要么扩 `emittedNamesFrom` 的正则并同步改上面那条 `[]` 夹具，要么改回 `emit`",
  );
  // 反向对照：样例里必须**仍然含有**假形态 —— 否则哪天有人把 `stripRustComments` 的调用删掉，
  // 这条测试也不会红（"证明有效的夹具退化成不证明"是本项目反复付过钱的形状）。
  const raw = [
    ...sample.matchAll(/\bemit\w*\(\s*(?:"([^"]+)"|([A-Z][A-Z0-9_]*))/g),
  ].map((m) => m[1]);
  assert.ok(
    raw.some((x) => x !== undefined && x.startsWith("phantom-")),
    "夹具失效：不剥注释时扫不到任何 phantom-* ⇒ 样例里的假形态该补回来了",
  );
});

test("每个 Rust 事件都必须有前端消费者，或写在例外清单里并说明理由", () => {
  const emitted = rustEmittedEvents();
  const listened = frontendListenedEvents();
  assert.ok(emitted.size > 10, `应该扫到后端事件，实际 ${emitted.size} 个`);

  const orphans = [...emitted].filter(
    (name) =>
      !listened.has(name) &&
      !IGNORED_PREFIXES.some((p) => name.startsWith(p)) &&
      !(name in ALLOWED_EMIT_WITHOUT_LISTENER),
  );
  assert.deepEqual(
    orphans,
    [],
    `以下事件后端在发、前端却没人听（界面不会更新）：${orphans.join(", ")}\n` +
      `要么接上消费者，要么加进 ALLOWED_EMIT_WITHOUT_LISTENER 并写清理由。`,
  );
});

test("前端监听的事件必须真的有人发（避免死监听/拼错事件名）", () => {
  const emitted = rustEmittedEvents();
  const listened = frontendListenedEvents();
  assert.ok(listened.size > 10, `应该扫到前端监听，实际 ${listened.size} 个`);

  const dead = [...listened].filter(
    (name) => !emitted.has(name) && !IGNORED_PREFIXES.some((p) => name.startsWith(p)),
  );
  assert.deepEqual(
    dead,
    [],
    `以下事件前端在听、后端却从来不发（多半是拼错或功能被摘掉了）：${dead.join(", ")}`,
  );
});

test("例外清单不得包含已被监听的事件（挂账说谎会让真漏接隐身）", () => {
  // 真实教训（2026-09-19 审计）：`file-failed`/`file-cancelled` 早就在 bindEvents 里
  // 接了，例外表却还挂着「待接」——白名单失修会让 review 的人对整张表失去信任，
  // 真漏接混在里面也查不出来。接上消费者的那一刻必须同时把条目删掉。
  const listened = frontendListenedEvents();
  const stale = Object.keys(ALLOWED_EMIT_WITHOUT_LISTENER).filter((name) => listened.has(name));
  assert.deepEqual(
    stale,
    [],
    `以下事件已在 bindEvents 监听，却仍挂在例外清单里：${stale.join(", ")} —— 请删掉条目`,
  );
});

test("设置变更必须带补丁 + 不回发给发起窗口（否则每窗口全量重拉 + 事件乒乓）", () => {
  const state = readFileSync(join(RUST_SRC, "state.rs"), "utf8");
  // ① 必须是"按窗口过滤"的发送：emit_filter 才能排除发起窗口
  assert.match(
    state,
    /emit_filter\(\s*EVENT_SETTINGS_CHANGED/,
    "settings-changed 必须用 emit_filter 发送（emit 是无差别广播，会把事件回发给发起窗口）",
  );
  assert.match(
    state,
    /event_target_label\(target\)\s*!=\s*origin/,
    "过滤条件必须是『目标窗口 ≠ 发起窗口』（否则发起窗口会被自己的旧快照回灌）",
  );
  // ② 载荷必须带"变了哪些键 + 那些键的新值"
  assert.match(state, /pub struct SettingsPatch/, "必须有带载荷的 SettingsPatch");
  assert.match(state, /pub changed: Vec<String>/, "载荷必须说明变了哪些键");
  assert.match(state, /pub settings: serde_json::Value/, "载荷必须带上新值（否则接收方还是要全量重拉）");
});

test("每个改设置的后端命令都必须传 origin（否则发起窗口收到自己的事件）", () => {
  const calls: string[] = [];
  for (const f of collectRustFiles(RUST_SRC)) {
    const src = readFileSync(f, "utf8");
    // 只认 `state.notify_settings_changed(...)` 这种真实调用（注释里的 `AppState::…` 不算）
    for (const m of src.matchAll(/state\.notify_settings_changed\(([^;]*?)\);/gs)) calls.push(m[1]);
  }
  assert.ok(calls.length >= 6, `应至少找到 6 个调用点，实际 ${calls.length} —— 解析器失效了？`);
  const bad = calls.filter((c) => !/Some\(/.test(c));
  assert.deepEqual(
    bad,
    [],
    `以下调用没传 origin（会把 settings-changed 回发给发起窗口）：${bad.join(" | ")}`,
  );
});

test("我的在线状态 = 任一通道在跑（两个都关才是离线，用户规则）", () => {
  const commands = readCommandsSrc();
  assert.match(
    commands,
    /let present = list\.iter\(\)\.any\(\|c\| c\.running\)/,
    "present 必须由『任一通道在跑』算出（用 running 而不是 enabled —— 开关开了但起不来不该算在线）",
  );
  const state = readFileSync(join(RUST_SRC, "state.rs"), "utf8");
  assert.match(state, /pub present: bool/, "运行状态快照必须带 present");
  const profile = readFileSync(
    join(ROOT, "src", "components", "settings", "ProfileSection.vue"),
    "utf8",
  );
  assert.ok(profile.includes("app.present"), "设置里『我的状态』必须读 present（不是 online）");
  const rail = readFileSync(join(ROOT, "src", "components", "NavRail.vue"), "utf8");
  assert.ok(rail.includes("app.present"), "侧栏头像的状态点必须读 present");
  const net = readFileSync(
    join(ROOT, "src", "components", "settings", "NetworkSection.vue"),
    "utf8",
  );
  assert.ok(
    net.includes("app.online"),
    "局域网节点数那类 LAN 专属文案仍应读 online（它是『局域网在跑』，两件事不能混）",
  );
});

test("运行状态只能有一个快照 + 一个带载荷的事件（②）", () => {
  const state = readFileSync(join(RUST_SRC, "state.rs"), "utf8");
  assert.match(
    state,
    /emit_filter\(\s*EVENT_RUNTIME_CHANGED/,
    "runtime-changed 必须用 emit_filter 排除发起窗口（它从命令返回值里已经拿到了快照）",
  );
  assert.match(state, /pub struct RuntimeSnapshot/, "必须有唯一的运行状态快照结构");
  assert.match(
    state,
    /snapshot: RuntimeSnapshot/,
    "事件必须**带快照**：无载荷的话接收方只能再全量重拉一遍（就是『两份状态』的温床）",
  );
  const commands = readCommandsSrc();
  assert.ok(
    !commands.includes("pub async fn get_channel_status(") &&
      !commands.includes("pub fn get_network_status("),
    "半份状态的命令（get_channel_status / get_network_status）必须已删除",
  );
  assert.match(commands, /pub async fn build_runtime_snapshot\(/, "必须有唯一的采集点");
  const apiSrc = readFileSync(API_FILE, "utf8");
  // 只认"真的在 invoke 它们"（注释里提旧名字是为了说明为什么删，不算调用）
  assert.ok(
    !apiSrc.includes('"get_channel_status"') && !apiSrc.includes('"get_network_status"'),
    "前端不得再调那两个半份命令",
  );
  assert.match(
    apiSrc,
    /listen<RuntimeSnapshot>\("runtime-changed"/,
    "前端必须按快照载荷监听 runtime-changed",
  );
});

test("清空数据必须广播（回归：设置里清了聊天记录，主界面毫无反应）", () => {
  const src = readCommandsSrc();
  const at = src.indexOf("pub async fn clear_all_data");
  assert.ok(at > 0, "找不到 clear_all_data");
  assert.match(
    src.slice(at),
    /notify_data_cleared\(/,
    "clear_all_data 末尾必须广播 data-cleared（否则另一个窗口的主界面不会变）",
  );
  assert.ok(
    frontendListenedEvents().has("data-cleared"),
    "前端必须监听 data-cleared（主窗口据此重建会话/消息/申请列表）",
  );
});

test("settingsDirty 那套回灌守卫不得复活（发起窗口已收不到自己的事件）", () => {
  const store = readFileSync(join(ROOT, "src", "stores", "useAppStore.ts"), "utf8");
  // 只认"真的用了"（赋值/调用），注释里提到名字不算 —— 说明为什么删掉的那段注释本身也有价值
  assert.ok(
    !/\bsettingsDirty\s*=/.test(store) && !store.includes("shouldResyncFromBackend"),
    "发起窗口不再收到自己的设置事件 ⇒ settingsDirty/shouldResyncFromBackend 应保持删除状态；" +
      "若确实要复活，必须先解释为什么 emit_filter 的排除不够用",
  );
});

test("设置变更事件两端都在（回归：改语言/主题后另一个窗口不刷新）", () => {
  const emitted = rustEmittedEvents();
  const listened = frontendListenedEvents();
  assert.ok(
    emitted.has("settings-changed"),
    "后端必须广播 settings-changed（否则设置窗口改完，主窗口不会变）",
  );
  assert.ok(listened.has("settings-changed"), "前端必须监听 settings-changed");
});
