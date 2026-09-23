/**
 * Store 契约守卫：**界面里用到的 store 成员，必须在 store 里真的导出**。
 *
 * ## 为什么需要它
 * 真实事故（2026-09-12 用户实测）：给 `useAppStore` 新增了 `channels`/`refreshChannels`
 * 之后，长时间运行的 dev 会话里 Pinia 还是**旧实例**（当时没接 HMR），
 * 于是设置页里 `app.refreshChannels is not a function`、`channels.value.find` 抛错 ⇒
 * **一个分区渲染抛错，整个设置页再也 patch 不动**（用户看到的是"点设置卡死、
 * 过一会儿弹出好几个设置、主题延迟切换"）。
 *
 * 那一半靠 store 接 HMR 修掉了；这一半是**编译期/测试期就能拦住**的部分：
 * 只要 `.vue`/`.ts` 里出现 `app.xxx` / `chat.xxx`，而 store 的 `return { … }` 里没有 `xxx`，
 * 就直接报出来（并指到文件与行号），不必等运行时炸。
 *
 * 判据刻意收紧：只认 `app.` / `chat.` 两个已知 store 的引用（局部变量重名时可能误报，
 * 所以要求 store 的导出名集合里**同时**存在若干个"核心名字"才认定这个文件是 store；
 * 误报的逃生阀是文件级 `store-contract-ok` 注释）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dirname, "..");
const STORES = [
  {
    /** 页内引用前缀 */
    prefix: "app",
    file: join(ROOT, "stores", "useAppStore.ts"),
    /** 用这几个名字确认"这个文件确实是那个 store"，避免把任意 `.ts` 当成 store 解析 */
    sanity: ["device", "toast", "themeColor"],
  },
  {
    prefix: "chat",
    file: join(ROOT, "stores", "useChatStore.ts"),
    sanity: ["messages", "conversations", "send"],
  },
];

/** 去掉字符串字面量：i18n key 形如 `"chat.toast.xxx"`，不剥掉会被当成 store 引用（误报）。 */
function stripStrings(src: string): string {
  return src
    .replace(/"(?:[^"\\]|\\.)*"/g, '""')
    .replace(/'(?:[^'\\]|\\.)*'/g, "''")
    .replace(/`(?:[^`\\]|\\.)*`/gs, "``");
}

/** Vue 应用实例（`createApp()` 的返回值）不是 store，这几个成员要放行。 */
const NOT_STORE_MEMBERS = new Set([
  "config",
  "use",
  "mount",
  "unmount",
  "provide",
  "component",
  "directive",
  "mixin",
  "version",
]);

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else if (entry.name.endsWith(".vue") || entry.name.endsWith(".ts")) out.push(full);
  }
  return out;
}

/**
 * 从 store 源码里取 `return { … }` 的**顶层标识符**集合。
 *
 * 只做浅层扫描（到与 `return {` 配对的 `}` 为止），够用且不会误判——
 * store 的返回是扁平的 `name,` / `name: alias,` 列表。
 */
function storeExports(file: string, sanity: string[]): Set<string> {
  const src = readFileSync(file, "utf8");
  const at = src.lastIndexOf("return {");
  assert.ok(at > 0, `${file} 里找不到 return {`);
  let depth = 0;
  let end = at + "return {".length - 1;
  for (let i = end; i < src.length; i++) {
    if (src[i] === "{") depth += 1;
    else if (src[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  const body = src.slice(at, end);
  const names = new Set<string>();
  for (const m of body.matchAll(/(^|[\s{,])([A-Za-z_$][\w$]*)\s*(:|,)/g)) {
    names.add(m[2]);
  }
  for (const s of sanity) {
    assert.ok(names.has(s), `${file} 的 return 里没有 ${s} —— 解析逻辑可能需要更新`);
  }
  return names;
}

for (const store of STORES) {
  test(`${store.prefix}.* 用到的每个成员都在 store 里导出`, () => {
    const exported = storeExports(store.file, store.sanity);
    const bad: string[] = [];
    for (const f of walk(ROOT)) {
      if (f.startsWith(join(ROOT, "stores"))) continue; // store 内部互调不算
      const raw = readFileSync(f, "utf8");
      if (raw.includes("store-contract-ok")) continue;
      const src = stripStrings(raw);
      // 只查"像 store 引用"的用法：`app.foo` / `app.foo(` —— 排除 `app.foo = ` 这类局部改写
      const re = new RegExp(`\\b${store.prefix}\\.([A-Za-z_$][\\w$]*)`, "g");
      for (const m of src.matchAll(re)) {
        const name = m[1];
        if (exported.has(name) || NOT_STORE_MEMBERS.has(name)) continue;
        const line = src.slice(0, m.index ?? 0).split("\n").length;
        bad.push(`${f.replace(ROOT + "/", "")}:${line}  ${store.prefix}.${name}`);
      }
    }
    // 去重（同一名字在多个文件里出现时各自报一条，便于定位）
    assert.deepEqual(
      [...new Set(bad)],
      [],
      `以下 store 成员在界面里被使用、但 store 并没有导出 —— 运行时会 undefined/不是函数，` +
        `并会让所在页面渲染卡死：\\n${[...new Set(bad)].join("\\n")}`,
    );
  });
}

// ---------------- 会话级 UI 收尾与慢响应回填（审计阶段 4 · 4.1-3 / 4.1-4 / 4.1-7） ----------

/**
 * 这三处都是"少写一行不会报错、只会让用户看见**别的会话**的状态"的缺陷，
 * 编译器与 `vue-tsc` 都管不着 ⇒ 只能按源码结构钉。
 *
 * ⚠️ 判据一律取**代码形状**（`x.value = false`、`if (id !== chat.activeConv) return`），
 * 不取中文描述 —— 本文件读的是原始源码（含注释），拿文案当判据会变成"注释替代码通过"。
 */
test("切会话必须收尾会话级浮层；离开聊天视图必须退多选", () => {
  const cw = readFileSync(join(ROOT, "components", "ChatWindow.vue"), "utf8");
  const from = cw.indexOf("() => chat.activeConv");
  const to = cw.indexOf("() => app.mobileView");
  assert.ok(from >= 0, "找不到 activeConv 的 watcher");
  assert.ok(to > from, "找不到 mobileView 的 watcher —— 4.1-4 会回归（TabBar 被永久藏掉）");
  const convWatch = cw.slice(from, to);
  for (const flag of [
    "membersOpen",
    "filesOpen",
    "tasksOpen",
    "lightboxOpen",
    "announceViewOpen",
  ]) {
    assert.ok(
      convWatch.includes(`${flag}.value = false`),
      `切会话时没清 ${flag}：面板会带着上一个会话的内容继续显示`,
    );
  }
  assert.ok(
    cw.slice(to).includes("exitMultiSelect()"),
    "mobileView watcher 里没退多选：返回会话列表后 multiSelectActive 会永久挂着",
  );
});

test("跨 IPC 的回填必须核对「数据还是不是当前会话/群」", () => {
  const cw = readFileSync(join(ROOT, "components", "ChatWindow.vue"), "utf8");
  const start = cw.indexOf("async function refreshLinkState");
  assert.ok(start >= 0, "找不到 refreshLinkState —— 改名要同步这条守卫");
  const end = cw.indexOf("\n}", start);
  assert.ok(end > start, "refreshLinkState 的函数体边界没找到");
  assert.ok(
    cw.slice(start, end).includes("if (id !== chat.activeConv) return"),
    "refreshLinkState 缺过期守卫：上一个对端的链路会写进当前聊天头（4.1-3）",
  );

  const gfp = readFileSync(join(ROOT, "components", "GroupFilesPanel.vue"), "utf8");
  assert.ok(
    gfp.includes("() => props.groupId"),
    "GroupFilesPanel 不 watch groupId：换群后面板仍是上一个群的清单（4.1-7）",
  );
  assert.ok(
    gfp.includes("gid !== props.groupId"),
    "GroupFilesPanel.load() 缺过期守卫：切群瞬间的旧响应仍会把 A 群清单回填进 B 群",
  );
});

// ---------------- 历史翻页的两道闸与未读位移（审计阶段 4 · 4.1-2 + 2026-09-23 评审回修） ----

/**
 * 这三条都是"删掉一行不会编译错、只会让用户翻不动历史或定位错"的形状，
 * 而本仓没有能驱动 store 异步竞态的运行时夹具（与 `loadSeqs` 同处境）⇒ 按代码结构钉。
 * 判据取代码形状，不取注释文案。
 */
test("翻页单飞必须「并入在飞那一次」，不许把后来者直接弹回", () => {
  const st = readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8");
  const fn = st.slice(st.indexOf("if (historyTops.has(convId)) return;"));
  const head = fn.slice(0, fn.indexOf("async function loadMorePage"));
  assert.ok(
    head.includes("if (running) return running;"),
    "locateMessage / locateMessageInConv 靠「await 后长度没变」判断『已翻到头』，" +
      "单飞闸若直接 return 就会把「别人正在翻」误报成「没有更早历史」",
  );
  assert.ok(
    head.includes("page.finally("),
    "在飞记录必须自己摘除（finally），否则一次 IPC 抛错就把该会话的翻页永久锁死",
  );
});

test("「已翻到顶」结论必须在每次重新加载时作废，且作废点在任何 await 之前", () => {
  const st = readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8");
  const fn = st.slice(st.indexOf("async function loadMessages("));
  const body = fn.slice(0, fn.indexOf("\n  }"));
  const del = body.indexOf("historyTops.delete(convId)");
  assert.ok(del >= 0, "loadMessages 不作废 historyTops ⇒ IPC 失败/seq 被抢的早退路径会永久挡死翻页");
  assert.ok(
    del < body.indexOf("await api.getMessageCount"),
    "作废点必须排在 await 之前：放在成功路径末尾时，catch 与过期早退都到不了那里",
  );
});

test("prepend 后的未读位移必须按「真正新插入且会渲染」的行数算", () => {
  const st = readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8");
  const fn = st.slice(st.indexOf("async function loadMorePage("));
  const body = fn.slice(0, fn.indexOf("\n  }\n"));
  assert.ok(
    body.includes("!known.has(m.msg_id) && isRenderedInTimeline(m.kind)"),
    "mergeMessages 会按 msg_id 去重（会话总数落在 101~199 时第二页 offset 仍是 0，整页大面积重叠）；" +
      "按整页条数平移会把分割线推到真锚点下方",
  );
});
