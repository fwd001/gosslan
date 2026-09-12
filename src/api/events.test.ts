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
    const src = readFileSync(f, "utf8");
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
    const src = readFileSync(f, "utf8");
    // emit("name"            / emit(CONST
    for (const m of src.matchAll(/\bemit\w*\(\s*(?:"([^"]+)"|([A-Z][A-Z0-9_]*))/g)) {
      const name = m[1] ?? consts.get(m[2] ?? "");
      if (name) out.add(name);
    }
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

test("设置变更事件两端都在（回归：改语言/主题后另一个窗口不刷新）", () => {
  const emitted = rustEmittedEvents();
  const listened = frontendListenedEvents();
  assert.ok(
    emitted.has("settings-changed"),
    "后端必须广播 settings-changed（否则设置窗口改完，主窗口不会变）",
  );
  assert.ok(listened.has("settings-changed"), "前端必须监听 settings-changed");
});
