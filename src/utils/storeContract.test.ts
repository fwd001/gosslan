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
import { stripJsLiterals } from "../../scripts/jsScan.ts";

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

/**
 * 去掉注释与字符串字面量：i18n key 形如 `"chat.toast.xxx"`，不剥掉会被当成 store 引用（误报）。
 *
 * ⚠️ 这里**以前**是三段按引号配对的正则，被实测到两种错法（正则字面量里的引号把全文件的
 * "字符串内/外"状态整体错位 ⇒ 假红；模板串 `${app.percent}` 被整段抹掉 ⇒ 假绿）。
 * 现在走 `scripts/jsScan.ts` 那份词法扫描（下面四格自证就是它的判据）。
 */
function stripStrings(src: string): string {
  return stripJsLiterals(src);
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

/**
 * ★ 扫描器自己的非空转用例（2026-09-28 加，起因是实测到的假红）。
 *
 * 这条护栏的判据形状是「把字符串抠掉，再找 `app.xxx` / `chat.xxx`」，
 * 而第一版抠法是按引号配对的正则三段替换 —— 它**分不清正则字面量里的引号**：
 * 一句 `/^r#*"/.test(x)` 里那个引号会被当成"字符串开始"，把后面**真正的代码**与
 * 后面**真正的字符串**配对错，于是整份文件的"里/外"状态翻转 —— 表现就是
 * `src/api/events.test.ts` 里那些**故意写来喂扫描器的 Rust 夹具**（`'app.emit("x", &p)'`）
 * 被读成了真实 store 用法 ⇒ 护栏红，而红得没有道理。
 * 同一种错法反过来也会**假绿**：真正该被抓的 `app.xxx` 若正好落在被误配的那一段里，就被抠掉了。
 *
 * 所以这里钉四件事：① 正则字面量不许把后面的状态带偏；② 转义引号不许提前结束字符串；
 * ③ 模板串里的 `${…}` **是代码**，占位里的真实引用必须还能被看见（原来的整段抹掉会瞎）；
 * ④ 注释里的 `app.xxx` 不算用法（第一版连注释都不抠，任何解释性注释都能造出假红）。
 */
test("扫描器不被引号型正则带偏（同一份文件里前后两段都得判对）", () => {
  const src = [
    "const re = /^r#*\"/.test(x);", // 正则字面量里带引号：以前的配对此刻就翻车
    "const fixture = 'app.emit(\"real-line\", &p);';", // 字符串里的用法：必须被抠掉
    "app.toastError(e, t('msg.fail'));", // 真实用法：必须留下来
  ].join("\n");
  const kept = [...stripStrings(src).matchAll(/\bapp\.([A-Za-z_$][\w$]*)/g)].map((m) => m[1]);
  assert.deepEqual(kept, ["toastError"], `该留下的真实用法必须唯一，实测留下：${JSON.stringify(kept)}`);
});

test("扫描器不被转义引号骗，也不吃穿下一行", () => {
  const src = 'const s = "a\\"b";\nconst t = app.channels;\n';
  const kept = [...stripStrings(src).matchAll(/\bapp\.([A-Za-z_$][\w$]*)/g)].map((m) => m[1]);
  assert.deepEqual(kept, ["channels"], `转义引号不该把下一行的真实用法一起吞掉：${JSON.stringify(kept)}`);
});

test("模板串的 ${…} 是代码：占位里的真实 store 引用不许被抹掉", () => {
  const src = "notify(`进度 ${app.percent}%`);\n";
  const kept = [...stripStrings(src).matchAll(/\bapp\.([A-Za-z_$][\w$]*)/g)].map((m) => m[1]);
  assert.deepEqual(kept, ["percent"], `模板占位里的用法是真用法（旧抠法整段抹 ⇒ 假绿）：${JSON.stringify(kept)}`);
});

test("注释里的 app.xxx 不算界面用法", () => {
  const src = "// 这里解释 app.refreshChannels 为什么不存在\n/* app.alsoFake */\nconst ok = 1;\n";
  const kept = [...stripStrings(src).matchAll(/\bapp\.([A-Za-z_$][\w$]*)/g)].map((m) => m[1]);
  assert.deepEqual(kept, [], `注释里的名字不该造出假红：${JSON.stringify(kept)}`);
});

/// 扫描范围 = **会进产物的代码**：`.vue` 与 `.ts`，但**不含 `*.test.ts`**。
///
/// 为什么不扫测试文件（2026-09-28 现算撞出来的）：这条判据的成立前提是"界面里用了没导出的成员
/// ⇒ 那一页运行时炸"，而测试文件根本不进包。更要紧的是它天然造出误报 ——
/// `utils/windowEntries.test.ts` 里有个**局部变量**就叫 `chat`，`chat.slice(...)` 被读成了
/// "界面用了 store 里不存在的成员"。这种局部重名的误报，文件的 doc 注释里本来就承认
/// （逃生阀是整文件 `store-contract-ok` 注释），但对测试文件来说正确的处理不是加白名单，
/// 而是把它请出分母 —— 与 `scripts/semver.mjs` 的 `isAppCodePath` 同一条口径：
/// **测试文件不算应用码**，所以也不该算"界面用法"。
function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else if ((entry.name.endsWith(".vue") || entry.name.endsWith(".ts")) && !entry.name.endsWith(".test.ts")) out.push(full);
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
  for (const flag of ["membersOpen", "filesOpen", "tasksOpen", "announceViewOpen"]) {
    assert.ok(
      convWatch.includes(`${flag}.value = false`),
      `切会话时没清 ${flag}：面板会带着上一个会话的内容继续显示`,
    );
  }
  // 图片预览从批次 x 起是**全局那一份实例**（#40），不再有个本地 `lightboxOpen`。
  // 收尾要求没变、只是换了形状：按**来源**收 —— 无条件 close() 会把
  // "从任务详情点开的图"跟着切会话一起弄没，不收则会留着上一个会话的相册。
  assert.match(
    convWatch,
    /preview\.closeIfFrom\(`conv:\$\{prev\}`\)/,
    "切会话时没按来源收掉上一个会话给出的图片预览",
  );
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
  const st = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const fn = st.slice(st.indexOf("async function loadMessages("));
  const body = fn.slice(0, fn.indexOf("\n  }"));
  const del = body.indexOf("historyTops.delete(convId)");
  assert.ok(del >= 0, "loadMessages 不作废 historyTops ⇒ IPC 失败/seq 被抢的早退路径会永久挡死翻页");
  // ⚠️ 判据钉在**第一个 await**上，不钉在某条具体调用名上：以前写的是
  // `del < body.indexOf("await api.getMessageCount")`，把冷加载换成一条新命令之后
  // 那个字面量就消失了，这条红线会**永远绿灯** —— 而它守的是"早退路径漏清 historyTops"。
  const firstAwait = body.indexOf("await");
  assert.ok(firstAwait > 0, "loadMessages 里没有 await：判据前提塌了，改名要同步这条守卫");
  assert.ok(
    del < firstAwait,
    "作废点必须排在任何 await 之前：放在成功路径末尾时，catch 与过期早退都到不了那里",
  );
});

test("peers 只许经 mergePeerList 写入：每拍换掉整表引用 = 每 333ms 重画一屏", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const sites = [...st.matchAll(/peers\.value\s*=\s*([A-Za-z_$][\w$]*)/g)];
  assert.equal(
    sites.length,
    3,
    `peers 的写入点应为 3 处（refreshPeers / searchNearbyPeers / onPeers），实际 ${sites.length} 处：` +
      "多了就是有人又开始整表直写",
  );
  const bad = sites.filter((m) => m[1] !== "merged");
  assert.deepEqual(
    bad.map((m) => m[0]),
    [],
    "peers 必须写 mergePeerList 的结果：内容没变就不赋值，变了也只换真变的那台。" +
      "直接 `peers.value = <整个列表>` 会让所有读过 peers 的渲染（消息行模板里的 nicknameOf 就是）" +
      "每 333ms 全部失效一次",
  );
  assert.ok(st.includes('from "@/utils/peerMerge"'), "必须真的用那份合并函数，而不是就地再拼一遍");
});

test("isMobile 必须在 init 的第一个 await 之前定好，并同步写 html.is-mobile", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useAppStore.ts"), "utf8"));
  const at = st.indexOf("async function init()");
  assert.ok(at > 0, "找不到 app.init() —— 改名要同步这条守卫");
  const body = st.slice(at, st.indexOf("\n  }", at));
  const call = body.indexOf("applyIsMobile();");
  const firstAwait = body.indexOf("await ");
  assert.ok(call > 0, "init() 里没有 applyIsMobile()：判据前提变了");
  assert.ok(firstAwait > 0, "init() 里没有 await：判据前提变了");
  // 为什么钉"排在第一个 await 之前"：isMobile 只依赖 UA 与 matchMedia，**根本不需要等 IPC**。
  // 真机现场（#27）：`getSettings()` 之前那几步任一 reject，`App.vue` 的 `finally` 与
  // `boot.ts` 的 5s 硬定时器照样把骨架撤掉 ⇒ 手机上露出来的是 isMobile 仍是默认 false
  // 的那一帧起就一直挂着的**桌面三栏**。顺序错了不是"晚一点好"，是"永久错"。
  assert.ok(
    call < firstAwait,
    "applyIsMobile() 必须排在任何 await 之前：排在 IPC 之后，init 半途抛错就永远停在桌面布局",
  );
  assert.ok(
    body.includes('classList.toggle("is-mobile"'),
    "必须把同一个结论写进 html.is-mobile —— CSS 侧的布局级断点要用它挡掉" +
      "「WebView 首帧读到兜底视口宽度」那一档，否则 JS 说移动、CSS 说桌面",
  );
});

test("冷加载必须一次 IPC 取到最新一页，不许退回「先问总数再按 offset 取」两轮串行", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const at = st.indexOf("async function loadMessages(");
  assert.ok(at >= 0, "找不到 loadMessages —— 改名要同步这条守卫");
  const body = st.slice(at, st.indexOf("\n  }", at));
  const calls = [...body.matchAll(/\bapi\.(getMessageCount|getMessages|getLatestMessages)\b/g)];
  // 为什么钉"次数"而不是钉"没出现 getMessageCount"：切会话的冷加载正中间夹一次
  // `COUNT(*)` 查询，两轮都要排队过后端那把全局 `Mutex<Connection>` —— 多的一轮不是
  // 快一点慢一点的问题，而是用户切到一个冷会话时**先看到骨架、后看到内容**的那半拍。
  // 一个 await 的往返次数是本刀唯一可回归的东西，所以按调用点计数。
  assert.equal(
    calls.length,
    1,
    `冷加载发了 ${calls.length} 次消息页 IPC（${calls.map((c) => c[1]).join(" → ")}）：` +
      "必须一次拿完，count 那一轮是给翻页用的，不参与冷加载",
  );
  assert.equal(
    calls[0]?.[1],
    "getLatestMessages",
    "冷加载必须走「取尾部一页」那一条命令；退回 getMessages 需要 total 才能算 offset，等于把两轮串行带回来",
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

// ---------------- 「后发先至」一族（审计阶段 4 · 4.2，utils/staleGuard.ts） ----------------

/** 粗粒度剥掉 TS/Vue 的注释：形状判据必须只看代码，否则"注释里写了这个形状"会让守卫自己变红。 */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^[ \t]*\/\/.*$/gm, "");
}

test("store 里不许再有「x.value = await api.foo()」这种直写（后发先至的根源形状）", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const bad = st
    .split("\n")
    .map((l, i) => ({ l, n: i + 1 }))
    .filter(({ l }) => /\.value\s*=\s*await\s+(api|invoke)\./.test(l));
  assert.deepEqual(
    bad.map(({ l, n }) => `${n}: ${l.trim()}`),
    [],
    "IPC 无顺序保证：先发起的请求后回来就会用旧快照覆盖新状态（未读回退、红点亮回、\n" +
      "     刚收藏的条目从面板消失、进度条钉在 0%）。改走 utils/staleGuard 的 begin/isCurrent。\n" +
      "     注意本仓 useAppStore 还有几处同形状（device/shareDir/interfaces），多数是一次性初始化写、\n" +
      "     风险面不同，尚未按这条收掉 —— 但面积已被下一条'只许变小'的冻面判据钉住，长出新的即红。",
  );
});

/**
 * B-5 的"冻结半"（2026-09-26）。上一条约的是 useChatStore，而 `useAppStore` 今天仍有四处直写
 * （`device` 两处 / `shareDir` / `interfaces`，多为一次性初始化写，风险面与 chat 侧不同），
 * 按§十八 不在稳定任务里顺手改热路径 —— 于是先把**面积冻住**：
 * 修掉一处仍然绿（那是收敛），**长出第五处或换成新的 ref 就红**。
 * 两条断言各管一种 lie：同 ref 再加一处 ⇒ 计数红；引入新 ref ⇒ 名单红。
 */
test("useAppStore 的直写面冻在已知四处：长出第五处或新 ref 即红（B-5 冻结半）", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useAppStore.ts"), "utf8"));
  const hits = st
    .split("\n")
    .map((l, i) => ({ l: l.trim(), n: i + 1 }))
    .filter(({ l }) => /\.value\s*=\s*await\s+(api|invoke)\./.test(l));
  const FROZEN = ["device", "shareDir", "interfaces"];
  const novel = hits.filter(({ l }) => !FROZEN.some((ref) => l.startsWith(`${ref}.value`)));
  assert.deepEqual(
    novel.map(({ l, n }) => `${n}: ${l}`),
    [],
    `useAppStore 出现了冻结面之外的直写（冻结名单＝${FROZEN.join(" / ")}，当前共 ${hits.length} 处）。\n` +
      "     IPC 无顺序保证，旧响应会用旧快照盖掉新状态：新点要么走 utils/staleGuard 的 begin/isCurrent，\n" +
      "     要么按 useChatStore 那条判据收掉（那条不允许任何直写）。",
  );
  assert.ok(
    hits.length <= 4,
    `useAppStore 的直写点已从 4 处长到 ${hits.length} 处：\n` +
      hits.map(({ l, n }) => `     ${n}: ${l}`).join("\n"),
  );
});

test("组件侧的四个回填点必须各自过闸（旧形状一旦复现即红）", () => {
  const cases: [string, string, string][] = [
    // [文件, 必须出现的闸, 必须不出现的旧形状]
    ["components/message/ImageLightbox.vue", "resolveGuard.isCurrent", "apply(await "],
    ["components/MessageItem.vue", "summaryGuard.isCurrent", "deliverySummary.value = await invoke("],
    ["components/ShareDirectory.vue", "loadGuard.isCurrent", "downloadSharedFile(friendId()"],
  ];
  for (const [rel, need, forbidden] of cases) {
    const src = stripComments(readFileSync(join(ROOT, rel), "utf8"));
    assert.ok(src.includes(need), `${rel} 缺「${need}」：跨 IPC 的旧响应可以覆盖当前状态`);
    assert.ok(
      !src.includes(forbidden),
      `${rel} 又出现了「${forbidden}」：这是修之前的形状（旧请求直接落地 / 点击时才读当前会话）`,
    );
  }
});

// ---------------- 通知链路与图片重试（审计阶段 4 · 批次 g） ----------------

test("通知批次：先出队再查权限的那条链必须有「退回队列」的 catch", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const from = st.indexOf("function flushNotifications(");
  assert.ok(from >= 0, "找不到 flushNotifications");
  const fn = st.slice(from, st.indexOf("\n  }\n", from) + 4);
  assert.ok(fn.includes("notifyQueue.clear()"), "确认它仍是「先出队」的形状（判据的前提）");
  assert.ok(
    fn.includes("mergeNoticesInto(notifyQueue, entries)"),
    "出队与发出之间任何一步 reject 都会让整批通知凭空消失 —— 必须有退回队列的 catch",
  );
});

test("通知权限：必须区分「没问过」与「问过并被拒」，且用户点开关时强制重问", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useAppStore.ts"), "utf8"));
  assert.ok(
    /let notifyPermission: boolean \| null = null;/.test(st),
    "三态缓存：两个值会把「被拒」和「没问过」混为一谈 ⇒ 每个通知批次都重跑两次权限 IPC",
  );
  assert.ok(st.includes("if (!force && notifyPermission !== null) return notifyPermission;"));
  assert.ok(
    st.includes("await ensureNotifyPermission(true)"),
    "设置页开关是用户动作上下文：不能拿缓存的「上次被拒」挡死「后来在系统设置里放开」",
  );
});

test("start/stopNetwork 必须消费后端返回的快照（发起窗口收不到 runtime-changed）", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useAppStore.ts"), "utf8"));
  for (const call of ["api.startNetwork(bindIp)", "api.stopNetwork()"]) {
    const at = st.indexOf(`await ${call}`);
    assert.ok(at >= 0, `找不到 ${call} 的调用点`);
    const line = st.slice(st.lastIndexOf("\n", at - 1) + 1, st.indexOf("\n", at + call.length));
    assert.ok(
      line.includes("applyRuntimeSnapshot("),
      `${call} 的返回值被丢弃 ⇒ 本窗口 present/runtime 停更（后端刻意不回发给发起窗口）：${line.trim()}`,
    );
  }
});

test("图片重试不许再用查询串（blob:/data: 都不接受 query ⇒ 重试必然全败）", () => {
  const src = stripComments(readFileSync(join(ROOT, "components", "message", "MessageImageBubble.vue"), "utf8"));
  assert.ok(!src.includes("effectiveSrc"), "查询串版的 effectiveSrc 回来了：blob:/data: 上加 ?r=N 是无效地址");
  assert.ok(/:key="loadKey"/.test(src), "强制重取必须靠换 <img> 元素");
  const watchSrc = src.slice(src.indexOf("() => props.src"));
  assert.ok(watchSrc.includes("loadKey.value = 0"), "换图时必须把上一代的换代计数归零");
});

// ---------------- 重复初始化守卫（审计阶段 4 · 4.2，utils/initScope.ts） ----------------

test("chat.init() 必须先拆掉上一轮注册，且每条注册都要配对卸载", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const storeStart = st.indexOf("export const useChatStore");
  assert.ok(storeStart > 0, "找不到 useChatStore 定义");
  // 句柄必须在**模块作用域**：`acceptHMRUpdate` 换的是整个 store 实例，
  // 放在 setup 里的状态对"下一轮 init"就是一片空白，拆不到上一轮的东西。
  assert.ok(
    /let chatInitScope: InitScope \| null = null;/.test(st.slice(0, storeStart)),
    "chatInitScope 必须声明在 defineStore 之外（模块作用域）",
  );

  const initStart = st.indexOf("async function init()");
  const initBody = st.slice(initStart, st.indexOf("\n  return {", initStart));
  assert.ok(initStart > 0 && initBody.length > 500, "找不到 init() 函数体");
  assert.ok(
    initBody.indexOf("chatInitScope?.dispose()") > -1 &&
      initBody.indexOf("chatInitScope?.dispose()") < initBody.indexOf("bindEvents({"),
    "必须在任何注册之前拆掉上一轮（否则本轮注册会被自己拆掉）",
  );

  // 四类注册逐一配对（少一个 = 第二轮 init 起该事件跑两遍）
  assert.ok(
    /for \(const f of fns\) scope\.onDispose\(f\)/.test(initBody),
    "bindEvents 返回的 unlisten 必须逐个交给 scope 保管",
  );
  assert.ok(/scope\.onDispose\(\(\) => clearInterval\(/.test(initBody), "拓扑定时器必须有句柄");
  assert.ok(
    /scope\.onDispose\(\(\) => document\.removeEventListener\("visibilitychange", onVisibility\)\)/.test(
      initBody,
    ),
    "visibilitychange 必须用具名 handler 才能成对摘除",
  );
  assert.ok(/listener\.unregister\(\)/.test(initBody), "onAction 的 PluginListener 必须显式注销");
  assert.ok(
    !/void onAction\(/.test(initBody),
    "`void onAction(...)` 会丢掉返回的 PluginListener ⇒ 回调永久挂在插件上",
  );

  const adds = (initBody.match(/\.addEventListener\(/g) ?? []).length;
  const removes = (initBody.match(/\.removeEventListener\(/g) ?? []).length;
  assert.equal(adds, removes, "init 里的 addEventListener 必须与 removeEventListener 一一对应");
});

test("app.init() 的注册必须全部配对（不许退回「只解绑 settings-changed」那半套守卫）", () => {
  const st = stripComments(readFileSync(join(ROOT, "stores", "useAppStore.ts"), "utf8"));
  const storeStart = st.indexOf("export const useAppStore");
  assert.ok(storeStart > 0, "找不到 useAppStore 定义");
  assert.ok(
    /let appInitScope: InitScope \| null = null;/.test(st.slice(0, storeStart)),
    "appInitScope 必须声明在 defineStore 之外（模块作用域），否则换实例后拆不到上一轮",
  );
  // 旧的那半套守卫（两个 setup 级变量）不得复活：它们随实例一起被 HMR 丢掉
  assert.ok(!st.includes("settingsUnlisten"), "settingsUnlisten 回来了 = 又变成「按变量各自解绑」");
  assert.ok(!st.includes("runtimeUnlisten"), "runtimeUnlisten 同上");

  const initStart = st.indexOf("async function init()");
  // 注意：st 已经剥过注释，切片只能切**代码**（切 `/** …` 里的文案会得到空函数体）
  const initBody = st.slice(initStart, st.indexOf("async function resetDefaults()", initStart));
  assert.ok(initStart > 0 && initBody.length > 500, "找不到 init() 函数体");
  assert.ok(
    initBody.indexOf("appInitScope?.dispose()") > -1 &&
      initBody.indexOf("appInitScope?.dispose()") < initBody.indexOf("await api.getSettings()"),
    "必须在任何注册之前拆掉上一轮",
  );
  // 六类注册逐一配对
  assert.ok(/scope\.onDispose\(\s*await api\.onSettingsChanged/.test(initBody), "settings-changed");
  assert.ok(/scope\.onDispose\(\s*await api\.onRuntimeChanged/.test(initBody), "runtime-changed");
  assert.ok(/scope\.onDispose\(watchSystemAppearance\(\)\)/.test(initBody), "系统外观监听");
  assert.ok(/scope\.onDispose\(watchKeyboard\(\)\)/.test(initBody), "键盘高度监听");
  assert.ok(/clearTimeout\(bluetoothTimer\)/.test(initBody), "2s 蓝牙兜底定时器");
  assert.ok(/clearTimeout\(runtimeTimer\)/.test(initBody), "500ms 运行状态兜底定时器");
  const adds = (initBody.match(/\.addEventListener\(/g) ?? []).length;
  const removes = (initBody.match(/\.removeEventListener\(/g) ?? []).length;
  assert.equal(adds, removes, "init 里的 addEventListener 必须与 removeEventListener 一一对应");

  // 两个 watch* 必须真的返回卸载函数（返回 void 的话上面那两条断言是空的）。
  // 切片必须**止于下一个 function**：给固定长度会溢到相邻函数身上，白拿一个 removeEventListener。
  for (const fn of ["watchSystemAppearance", "watchKeyboard"]) {
    const at = st.indexOf(`function ${fn}`);
    const body = st.slice(at, st.indexOf("function ", at + fn.length + 10));
    assert.ok(at > 0 && body.length > 50 && body.includes("removeEventListener"), `${fn} 必须返回真正的卸载函数`);
  }
});

test("「不打扰」判据只许有一份：未读记账与通知闸门都必须走 countsTowardUnread", () => {
  // 后端 `protocol.rs::is_non_notifying_kind` = 静默类 + system。前端此前两处各自用
  // `!isSilentKind` ⇒ 经广播来的系统消息（如「X 加入了群聊」）会 +1 未读并弹一条系统通知。
  const msgs = stripComments(readFileSync(join(ROOT, "utils", "messages.ts"), "utf8"));
  const store = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const apply = msgs.slice(
    msgs.indexOf("export function applyIncomingToConversations"),
    msgs.indexOf("export function unreadAnchorIndex"),
  );
  assert.ok(apply.length > 100, "找不到 applyIncomingToConversations 函数体");
  assert.ok(
    apply.includes("countsTowardUnread(m.kind)"),
    "未读记账与预览必须用 countsTowardUnread（system 不记账），不许退回 !isSilentKind",
  );
  // 反向也钉：`countsTowardUnread(x) || x.kind === "system"` 这种"补一个 or"的写法
  // 看着保留了新判据、实际把 system 又放回记账集合，所以整函数体里不许再出现旧判据。
  assert.ok(!apply.includes("isSilentKind"), "记账路径上不许再出现第二份「静默」判据");
  assert.ok(
    store.includes("if (!countsTowardUnread(rec.kind)) return;"),
    "通知闸门必须与未读同一份判据，否则「后端不打扰、手机照样弹通知」",
  );
  assert.ok(
    !store.includes("if (isSilentKind(rec.kind)) return;"),
    "通知闸门不许退回旧的 isSilentKind 单判据",
  );
});

test("「停滞」必须带得上原因：file-stalled 的 reason 要一路走到气泡文案", () => {
  // 只有蓝牙链路时大文件"保持 pending 等 LAN"，那句原因必须落到界面上 ——
  // 否则用户看到的是一句"网络停滞"，会去查网络，而网络没问题。
  const types = readFileSync(join(ROOT, "types.ts"), "utf8");
  const store = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const msg = stripComments(readFileSync(join(ROOT, "composables", "useMessageFile.ts"), "utf8"));
  assert.ok(/reason\?:\s*string/.test(types), "FileStalledInfo 必须带可选 reason（与 Rust 同形）");
  assert.ok(
    store.includes("setTransferStalled(p.transfer_id, p.stalled, p.reason"),
    "事件里的 reason 不许在半路被丢掉",
  );
  assert.ok(
    msg.includes("transferStallReason(t.id)"),
    "气泡必须优先显示原因、没有原因才回退通用「网络停滞」",
  );
  // 单一集合：停滞标记与原因必须存在同一个 Map 里（分两处存就一定有一边忘清）
  assert.ok(
    /const stalledTransfers = ref<Map<string, string \| undefined>>/.test(store),
    "stalledTransfers 必须是「id → 原因」的 Map，不许退回 Set + 另一张原因表",
  );
});

/**
 * §30 第二条回归（#82「自己创建的群任务在聊天里看不见」的**形状**）。
 *
 * 那条 bug 的机制只有一句话：命令返回的那条消息记录**没被塞进 store**，
 * 于是时间线（`ChatWindow` 过滤 `chat.messages`）里自然没有它 —— 后端一切正常、界面无症状。
 *
 * 判据不靠手抄名单，靠**两份权威事实源对账**：
 * - 哪些命令"返回一条消息记录"？由 `src/api/index.ts` 自己声明：包装体里写的是
 *   `invoke<MessageRecord>("cmd", …)`。加一个新命令 ⇒ 自动进名单，不需要有人记得改这里。
 * - 它在 store 里的每个调用点都必须把结果 `enqueueMessage(...)`。少一行 ⇒ 红。
 *
 * 覆盖面自证（不这么写就会"少扫一处照样绿"）：`api.<key>(` 在全文件的出现次数必须等于
 * 落在被扫描函数体里的次数 —— 调用点藏在函数体外（顶层、对象字面量、回调）会直接红，
 * 而不是安静地被跳过。
 */
test("返回 MessageRecord 的每条命令，调用点必须把结果 enqueueMessage（#82 的形状）", () => {
  const apiSrc = stripComments(readFileSync(join(ROOT, "api", "index.ts"), "utf8"));
  const apiLines = apiSrc.split("\n");
  const recKeys: string[] = [];
  for (let i = 0; i < apiLines.length; i += 1) {
    const head = apiLines[i].match(/^\s{2}(\w+):\s*\(/);
    if (!head) continue;
    for (let j = i; j < apiLines.length; j += 1) {
      if (j > i && /^\s{2}\w+:\s*\(/.test(apiLines[j])) break; // 走到下一个包装 ⇒ 这个不返回记录
      if (/invoke<\s*MessageRecord\s*>\(/.test(apiLines[j])) { recKeys.push(head[1]); break; }
    }
  }
  // 前提变了（改名、换了泛型写法）时报的是这条，而不是"扫到 0 个 ⇒ 于是没有任何可判的 ⇒ 绿"
  assert.ok(
    recKeys.length >= 8,
    `只认出 ${recKeys.length} 个返回 MessageRecord 的 api 包装（今天至少 8 个）`
    + " —— 解析前提变了，先修这条守卫再谈别的",
  );

  const storeSrc = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const storeLines = storeSrc.split("\n");
  // 函数体范围：`  function name(` / `  async function name(` 起，到下一条顶格两空格的 `}` 止
  const fns: Array<{ name: string; from: number; to: number }> = [];
  for (let i = 0; i < storeLines.length; i += 1) {
    const m = storeLines[i].match(/^\s{2}(?:async )?function (\w+)/);
    if (!m) continue;
    let end = storeLines.length - 1;
    for (let j = i + 1; j < storeLines.length; j += 1) {
      if (/^\s{2}\}/.test(storeLines[j])) { end = j; break; }
    }
    fns.push({ name: m[1], from: i, to: end });
  }
  assert.ok(fns.length > 20, `只切出 ${fns.length} 个函数体 —— 切分前提变了`);

  const inRange = new Map<string, number>();
  const missing: string[] = [];
  /**
   * 纯 invoke 转发层：调命令但**刻意不** enqueue —— 由调用方决定记录去哪儿。
   *
   * 2026-10-03 引入 `sendToBackend`（单聊/群聊分流共用）时撞上本守卫。它不是漏 enqueue：
   * `send()` 的两条路径（新建乐观气泡 / 重发时把旧 failed 原地转 sending）各自决定
   * 记录怎么处理，转发层若擅自 enqueue 会把重发路径那条旧气泡顶掉。
   * 白名单**按函数名**列，且必须写明理由 —— 加进来的人要能回答"凭什么"，
   * 而不是为了让门禁变绿随手加一行。
   */
  const FORWARD_ONLY = new Map<string, string>([
    ["sendToBackend", "单聊/群聊 invoke 分流的共用实现；记录去哪儿由调用方（新建 vs 重发）决定"],
  ]);
  for (const fn of fns) {
    const body = storeLines.slice(fn.from, fn.to + 1).join("\n");
    for (const key of recKeys) {
      const hits = (body.match(new RegExp(`api\\.${key}\\(`, "g")) || []).length;
      if (!hits) continue;
      inRange.set(key, (inRange.get(key) ?? 0) + hits);
      if (FORWARD_ONLY.has(fn.name)) continue;
      if (!/enqueueMessage\(/.test(body)) missing.push(`${fn.name}() 调 api.${key} 却没 enqueueMessage`);
    }
  }
  const total = recKeys.reduce((n, k) => n + (storeSrc.match(new RegExp(`api\\.${k}\\(`, "g")) || []).length, 0);
  const scanned = [...inRange.values()].reduce((a, b) => a + b, 0);
  assert.equal(
    scanned, total,
    `有 ${total - scanned} 个调用点落在被扫描的函数体之外 ⇒ 这条守卫会静默漏判（覆盖面自证）`,
  );
  assert.ok(
    scanned >= recKeys.length,
    `今天判的调用点只有 ${scanned} 处，比返回记录的命令数 ${recKeys.length} 还少 ⇒ 名单在变大而没人调？先看上面那条`,
  );
  assert.deepEqual(missing, [], "这些调用点必须把返回的消息记录塞进 store：\n" + missing.join("\n"));

  // 白名单自证：每一条都必须**真的**还调着返回记录的命令，否则就是一条没用的豁免
  // （函数改名/重构后白名单会静默留着，"看起来有豁免、实际不豁免任何东西"）。
  for (const [name] of FORWARD_ONLY) {
    const at = storeSrc.search(new RegExp(`\\b${name}\\s*\\(`));
    assert.ok(at > 0, `白名单里的 ${name}() 在 store 里已经不存在了 —— 删掉白名单条目，别留一条空豁免`);
    assert.ok(
      recKeys.some((k) => new RegExp(`api\\.${k}\\(`).test(storeSrc)),
      `白名单里的 ${name}() 已不再调任何返回记录的命令 —— 同样该删掉这条豁免`,
    );
  }
  // 反向自证：白名单不许超过 3 条。膨胀到一半就说明它在被当成万能洞用。
  assert.ok(
    FORWARD_ONLY.size <= 3,
    `白名单已 ${FORWARD_ONLY.size} 条（上限 3）。若确实需要更多，先问「能不能不抽这一层」——` +
      `每一层转发都在削弱这条守卫的覆盖面。`,
  );
});
/**
 * 结构判据：store 的 `updateTodo` 转给命令的键集合，必须覆盖它自己 `patch` 声明的每一个键。
 *
 * 为什么必须机器钉（真实事故形状）：`category` 在 patch 类型里声明了、`api.updateGroupTodo`
 * 的入参里也有、后端命令与载荷都已支持，唯独 store 转发的那个对象字面量**少写一行** ⇒
 * 界面弹「已更新」的成功提示，而库里一字未动，重开弹窗就弹回原值。
 * 这条链上每一层单独看都"对"：TS 不会红（Partial 的键本来就可传可不传）、
 * 命令层收不到值就沿用库里那份（这是**设计**）、后端测试全绿 ⇒ 只有"键集合对账"看得见它。
 *
 * ⚠️ 判据故意**只核这一对**（`updateTodo` 的声明块 vs 它的转发块）：
 * 想把这套规则推广到所有 api 调用点，得先解决"怎么在不用 AST 的前提下不写出自造解析器的假阳性"
 * —— 这一版不做那件事，别把这条读成"所有 patch 都对过账"。
 */
test('store 的 updateTodo 不许漏转 patch 里声明过的键（category 那次事故的锁）', () => {
  // 用这份文件自己的 stripComments：注释里提到过那个键名，不剥就会自己命中自己
  const storeSrc = stripComments(readFileSync(join(ROOT, "stores", "useChatStore.ts"), "utf8"));
  const fnStart = storeSrc.indexOf("async function updateTodo(");
  assert.ok(fnStart >= 0, "找不到 updateTodo ⇒ 这条判据失去落点（改名要同步改这里）");
  const body = storeSrc.slice(fnStart, storeSrc.indexOf("enqueueMessage(rec)", fnStart) + 20);

  // 声明块：patch: Partial<{ ... }> = {}
  const declBlock = body.slice(body.indexOf("Partial<{"), body.indexOf("}> = {}"));
  assert.ok(declBlock.length > 20, "Partial 声明块没抠出来 ⇒ 形状变了，判据会空转");
  const declared = [...declBlock.matchAll(/^\s{6}(\w+):/gm)].map((m) => m[1]);

  // 转发块：api.updateGroupTodo(gid..., { key: ..., ... })
  const callAt = body.indexOf("api.updateGroupTodo(");
  assert.ok(callAt >= 0, "转发调用点没找到 ⇒ 同上");
  const argBlock = body.slice(body.indexOf("{", callAt), body.indexOf("});", callAt) + 2);
  const forwarded = [...argBlock.matchAll(/^\s{6}(\w+):/gm)].map((m) => m[1]);

  assert.ok(declared.length >= 7, `声明的键太少（${declared.length}）⇒ 抠到的不是那一块`);
  for (const k of declared) {
    assert.ok(forwarded.includes(k), `patch 声明了 ${k} 但没转发给命令 ⇒ 这一格改了等于没改`);
  }
  // 阳性对照：把 category 那一行从源码文本里摘掉，判据必须报缺
  const mutated = body.replace(/^\s*category: patch\.category,\n/gm, "");
  assert.notEqual(mutated, body, "对照那条替换没生效 ⇒ 这条对照什么都没测");
  const fwdMutated = [...mutated.slice(body.indexOf("{", mutated.indexOf("api.updateGroupTodo("))).matchAll(/^\s{6}(\w+):/gm)].map((m) => m[1]);
  assert.ok(!fwdMutated.includes("category"), "摘掉之后必须看不见 category ⇒ 否则判据是恒过的");
  assert.ok(!declared.every((k) => fwdMutated.includes(k)), "摘掉之后 declared ⊆ forwarded 必须被打破");
});
