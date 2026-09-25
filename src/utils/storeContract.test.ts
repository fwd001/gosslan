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
      "     注意本仓 useAppStore 还有 4 处同形状（device/shareDir/interfaces/updateProfile），\n" +
      "     多数是一次性初始化写、风险面不同，尚未纳入本判据 —— 收敛它们时要一起把范围扩过去。",
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
