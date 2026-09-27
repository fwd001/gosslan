#!/usr/bin/env node
/**
 * UI 运行时探针（零依赖）—— 把"只能落在运行时层"的那几格判据变成可重跑的东西。
 *
 * ## 为什么要有这个文件
 * 仓里没有 DOM 测试地基（没有 jsdom / vitest / @vue/test-utils / playwright 依赖），
 * 而有一类缺陷**只有**在真浏览器输入管线里才看得见：焦点落点、方向键走格子、
 * Esc 关完之后焦点去哪了、回车打开的是高亮那条还是第一条。
 * 这些以前每次都是"人眼看一次"，而 §30 的规矩是**每个真实缺陷都要换成一把永久锁**
 * （#92 那两条键盘出口就是这么发现、又这么差点丢失的）。
 *
 * ## 为什么不引依赖
 * `vite dev` 能把前端起来（Tauri 调用失败只弹一条 toast，界面壳照常渲染），
 * Node 24 有全局 `WebSocket` ⇒ 手写几十行就是 CDP 客户端。给 package.json 加一层
 * playwright/puppeteer 是"为了测试改动项目外的东西"，这条不做。
 * 浏览器二进制取本机 playwright 缓存里的 **Chrome for Testing**（没有系统 Chrome 也能跑），
 * 或者用 `GOSSLAN_CHROME` 指定。
 *
 * ## 三条把自己坑过的探针纪律（都写进判据了，别改回去）
 * 1. 一次按键的 `rawKeyDown / char / keyUp` 必须**同帧发出**（不逐条 await）。
 *    逐条 await 会被浏览器读成"按住不放"⇒ 自动重复：实测一次 Enter 打出 10194 次 keydown / 5074 次 click。
 * 2. Enter / 空格的 keyDown **必须带 `text`**。不带就没有"激活被聚焦的 button"这条默认动作，
 *    于是读到"回车不选中"——那是**假红**。本文件把"不带 text ⇒ 零次激活"固定成一条反向对照判据。
 * 3. 同一个页面里别叠两个开着的浮层（曾经把 renderer 主线程搞死，连 reload 的 eval 都发不进去）。
 *    ⇒ 每一段跑完都重新加载一次页面，段与段之间不共享浮层状态。
 *
 * ## 与真 app 的差别（报结论必须带上）
 * 组件逻辑是同一份代码，但按键走的是**浏览器**输入管线 ⇒ 这不等于 WKWebView / WebView2 里验过。
 * 那一半仍是人工（见验收矩阵 Smoke-11 的边界行）。
 *
 * ## 用法
 *     node scripts/check-ui-runtime.mjs              # 跑全部段
 *     node scripts/check-ui-runtime.mjs --only=emoji # 只跑某段
 *     node scripts/check-ui-runtime.mjs --self-check # 跑反面对照（判据必须报红，不是恒过）
 * 退出码：0 = 全绿；1 = 有判据红 / 环境缺件（**缺件是红，不是跳过**，§十 不许把没跑写成 PASS）。
 *
 * ## 改这个文件前先读三条（都是本轮实际踩过的）
 * - 页面侧那段是**模板字符串里的源码** ⇒ 里面不许出现反引号（会把模板提前结束），
 *   正则里的反斜杠也会被吃掉一层 —— 所以那段用 includes 而不是正则。
 * - 资源 URL 在 `entry.name` 上，不在 `entry.url`（写成 .url 会得到 undefined，
 *   报错却长得像"vite 没起来"）。
 * - `Page.navigate` 之后 vite 还在现编译依赖，拿 deps/vue.js 之前必须**轮询等**，
 *   等短了会把"还没编译完"读成"构建产物变了"。
 */
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const argv = process.argv.slice(2);
const ONLY = argv.find((a) => a.startsWith("--only="))?.slice("--only=".length) ?? "";
const SELF_CHECK = argv.includes("--self-check");
const VITE_PORT = Number(process.env.GOSSLAN_PROBE_PORT || 5199);
const CDP_PORT = Number(process.env.GOSSLAN_PROBE_CDP || 9444);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const nowMs = () => Date.now();

// ── 断言账本（终局由台账推导，不看"有没有抛异常"）──────────────────
const results = [];
function check(name, pass, expect, actual) {
  const ok = pass === true;
  results.push({ name, ok, expect, actual });
  console.log(`  ${ok ? "✅" : "❌"} ${name}`);
  if (!ok) console.log(`      预期 ${JSON.stringify(expect)} / 实际 ${JSON.stringify(actual)}`);
  return ok;
}

/** 找 Chrome for Testing（playwright 缓存）；`GOSSLAN_CHROME` 可覆盖。找不到返回 null。 */
function findChrome() {
  if (process.env.GOSSLAN_CHROME && fs.existsSync(process.env.GOSSLAN_CHROME)) {
    return process.env.GOSSLAN_CHROME;
  }
  const base = process.platform === "darwin"
    ? path.join(os.homedir(), "Library", "Caches", "ms-playwright")
    : path.join(process.env.HOME || os.homedir(), ".cache", "ms-playwright");
  if (!fs.existsSync(base)) return null;
  const cands = [];
  for (const dir of fs.readdirSync(base)) {
    if (!/^chromium-/.test(dir)) continue;
    cands.push(path.join(base, dir, "chrome-mac-arm64", "Google Chrome for Testing.app",
      "Contents", "MacOS", "Google Chrome for Testing"));
    cands.push(path.join(base, dir, "chrome-linux64", "chrome"));
  }
  return cands.find((p) => fs.existsSync(p)) ?? null;
}

/** 极简 CDP 客户端：id 配对、一次 eval 一个往返。 */
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.seq = 0;
    this.waiters = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(typeof ev.data === "string" ? ev.data : ev.data.toString());
      const w = this.waiters.get(msg.id);
      if (!w) return;
      this.waiters.delete(msg.id);
      if (msg.error) w.reject(new Error(`${msg.error.message} (${JSON.stringify(msg.error.data ?? "")})`));
      else w.resolve(msg.result);
    });
  }
  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((res, rej) => {
      ws.addEventListener("open", res, { once: true });
      ws.addEventListener("error", rej, { once: true });
      setTimeout(() => rej(new Error("CDP 握手超时")), 10_000);
    });
    return new Cdp(ws);
  }
  send(method, params = {}, awaitReply = true) {
    const id = ++this.seq;
    const p = awaitReply
      ? new Promise((res, rej) => this.waiters.set(id, { resolve: res, reject: rej }))
      : Promise.resolve(null);
    this.ws.send(JSON.stringify({ id, method, params }));
    return p;
  }
  async eval(expression) {
    const r = await this.send("Runtime.evaluate", {
      expression, awaitPromise: true, returnByValue: true, onlyPromise: false,
    });
    if (r.exceptionDetails) {
      throw new Error(`页面里抛错：${r.exceptionDetails.exception?.description
        ?? r.exceptionDetails.text}`);
    }
    return r.result?.value;
  }
  /**
   * 一次真按键。`chars` 决定要不要发 char（Enter 是 "\r"、空格是 " "）；
   * ★ 三条消息**不逐条 await** —— 见文件头纪律 1（逐条 await = 自动重复风暴）。
   */
  async key(key, code, text, keyCode) {
    const base = { key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
    this.ws.send(JSON.stringify({
      id: ++this.seq, method: "Input.dispatchKeyEvent",
      params: { type: "rawKeyDown", ...base, ...(text ? { text } : {}) },
    }));
    if (text) {
      this.ws.send(JSON.stringify({
        id: ++this.seq, method: "Input.dispatchKeyEvent",
        params: { type: "char", ...base, text, unmodifiedText: text },
      }));
    }
    this.ws.send(JSON.stringify({
      id: ++this.seq, method: "Input.dispatchKeyEvent",
      params: { type: "keyUp", ...base },
    }));
    await sleep(90);
  }
}

// ── 页面侧装进 window 的夹具（一次装好，之后每步只调函数）────────────
const PAGE_FIXTURE = `
window.__probe = (() => {
  const events = [];
  let clickCount = 0;
  const H = { events, get clicks() { return clickCount; } };
  H.reset = () => { events.length = 0; clickCount = 0; };
  H.install = async (cmpPath, mountHtml, propsJson) => {
    // 必须拿"页面已经在用的那一份 vue"，自己 import('vue') 会得到第二份（ref/reactive 互不相认）。
    // 而这条要**等**：Page.navigate 之后 vite 还在现编译依赖，resources 里出现 deps/vue.js 之前
    // 拿到的是空列表 —— 拿不到就当"构建产物变了"是误判。
    // （这里刻意用 includes 而不是正则：这段是**模板字符串里的源码**，反斜杠会被模板吃掉一层。）
    const VUE_MARK = '/node_modules/.vite/deps/vue.js?v=';
    let vueUrl = null;
    const resUrls = () => performance.getEntriesByType('resource').map((e) => e.name || '');
    for (let i = 0; i < 120 && !vueUrl; i += 1) {
      // 注意是 entry.name —— 资源计时条目里 URL 不在 .url 上（写成 e.url 会得到 undefined）
      vueUrl = resUrls().find((u) => u.includes(VUE_MARK)) ?? null;
      if (!vueUrl) await new Promise((r) => setTimeout(r, 500));
    }
    if (!vueUrl) {
      const seen = resUrls().map((u) => u.split('/').slice(-2).join('/'));
      throw new Error('60s 内没等到 deps/vue.js。已加载的资源尾巴：' + JSON.stringify(seen.slice(0, 12)));
    }
    const Vue = await import(vueUrl);
    const mod = await import(cmpPath);
    // 组件里用 useAppStore() ⇒ 必须给这一个 app 挂上 pinia，而且要用**页面已经在用的那份 pinia**
    // （自己 import('pinia') 会拿到第二份实例，store 互不相认，报的错还长得像"store 坏了"）。
    let pinia = null;
    const piniaUrl = performance.getEntriesByType('resource')
      .map((e) => e.name || '').find((u) => u.includes('/node_modules/.vite/deps/pinia.js?v='));
    if (piniaUrl) {
      const P = await import(piniaUrl);
      pinia = P.createPinia();
    }
    document.body.innerHTML = mountHtml;
    const host = document.createElement('div');
    document.body.appendChild(host);
    const state = Vue.reactive({ open: false, initialKeyword: '' });
    window.__st = state;
    // 夹具原先把 props 写死成 open/initialKeyword + 三个回调；任务卡那一族要的是
    // message / liveStatus / mentionNames / selfMention ⇒ 第三参收一份 JSON props。
    // liveStatus 是 Map，JSON 传不了 ⇒ 约定用 liveStatusPairs（[[id,status],…]）在这里还原。
    const extra = propsJson ? JSON.parse(propsJson) : {};
    if (Array.isArray(extra.liveStatusPairs)) {
      extra.liveStatus = new Map(extra.liveStatusPairs);
      delete extra.liveStatusPairs;
    }
    // ★ 这个对象必须**建在渲染函数里面**：open: state.open 是对 reactive 状态的读，
    //   提到外面求值一次就把 props 冻在 false 上 —— 表情面板那段当场整块不渲染（格子数 0），
    //   是隔壁那两段把它照出来的（**夹具的重构也要跑全量，别只跑新写的那一段**）。
    //   另：这一段是模板字符串里的源码，注释里也不许出现反引号（会提前结束模板）。
    const mkProps = () => ({
      open: state.open,
      initialKeyword: state.initialKeyword,
      onSelect: (v) => events.push(['select', v]),
      onClose: () => events.push(['close']),
      onOpenConversation: (p) => events.push(['open', JSON.stringify(p)]),
      // 任务卡底部那个入口 emit 的是 "open"（载荷是 todo_id）。tag 刻意叫 open-id：
      // 与上面那条 'open'（搜索弹窗的 open-conversation）区分开，免得两段互相污染判据。
      onOpen: (v) => events.push(['open-id', String(v)]),
    });
    const app = Vue.createApp({
      setup() {
        return () => Vue.h(mod.default, Object.assign(mkProps(), extra));
      },
    });
    if (pinia) app.use(pinia);
    window.__pinia = pinia;
    app.mount(host);
    window.__probe.vue = Vue;
    window.__probe.state = state;
    await Vue.nextTick(); await Vue.nextTick();
    document.addEventListener('click', () => { clickCount++; }, true);
    return { ok: true, cmp: cmpPath, pinia: !!pinia };
  };
  /** 替掉一条 api 命令。api 是普通对象 ⇒ 按名字替换，比伪造整个 __TAURI__ 便宜得多。 */
  H.stubApi = async (name, payload) => {
    const m = await import('/src/api/index.ts');
    if (!m.api || typeof m.api[name] !== 'function') {
      throw new Error('api.' + name + ' 不存在 —— 命令改名的话这条夹具要同步');
    }
    m.api[name] = async () => payload;
    return true;
  };
  /** 往搜索框里打字：走 v-model 需要 input 事件，不能直接改 .value 就完事。 */
  H.typeKeyword = async (text) => {
    const input = document.querySelector('input[type="search"], input[role="searchbox"], input');
    if (!input) throw new Error('找不到搜索输入框');
    input.focus();
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 900)); // 组件里是 150ms 防抖 + 一次请求
    return { focused: document.activeElement === input, hits: document.querySelectorAll('[data-hit]').length };
  };
  /**
   * 读统一预览 store 的当下状态。要点：**先 setActivePinia 到页面里那一份**，
   * 自己 import('pinia') 会得到第二个实例 ⇒ 读到的永远是空 store，
   * 看起来像"预览没开"（假红）。
   */
  H.previewState = async () => {
    const piniaUrl = performance.getEntriesByType('resource')
      .map((e) => e.name || '').find((u) => u.includes('/node_modules/.vite/deps/pinia.js?v='));
    if (!piniaUrl) throw new Error('页面里没有 pinia 资源 —— 挂载那一步没成功');
    const P = await import(piniaUrl);
    P.setActivePinia(window.__pinia);
    const m = await import('/src/stores/useImagePreview.ts');
    const st = m.useImagePreviewStore();
    return { open: st.open, source: st.source, index: st.index, n: (st.images || []).length };
  };
  H.text = () => document.body.innerText || '';
  H.el = (sel) => {
    const e = document.querySelector(sel);
    if (!e) return null;
    const r = e.getBoundingClientRect();
    return { role: e.getAttribute('role'), tabindex: e.getAttribute('tabindex'),
      aria: e.getAttribute('aria-label'), w: Math.round(r.width), h: Math.round(r.height) };
  };
  H.focusThumb = () => {
    const t = document.querySelector('[role="button"]');
    if (!t) return false;
    t.focus();
    return document.activeElement === t;
  };
  H.setOpen = async (v) => {
    window.__st.open = v;
    const t = window.__probe.vue;
    await t.nextTick(); await t.nextTick();
  };
  /**
   * 会话行这一刻「读屏得到的名字」与两枚徽标（#88 / #117 剩下的那一半）。
   *
   * 两条要点：
   *  · **先 setActivePinia 到页面里那一份**再去改 store —— 自己 import 一份 pinia 会得到
   *    第二个实例，改的是一份没人看的空 store（报出来的错长得像「徽标不亮」）。
   *  · 期望文案**由页面用应用同一份 i18n 现算**。这条判的不是「文案对不对」，
   *    而是「这三句在不在这一行的 accessible name 里」—— role=button 一旦带 aria-label，
   *    后代全被折叠掉，不在名字里就等于读屏用户得不到（2026-09-26 用系统控件树实测到的）。
   */
  H.convRow = async (arg) => {
    const piniaUrl = performance.getEntriesByType('resource')
      .map((e) => e.name || '').find((u) => u.includes('/node_modules/.vite/deps/pinia.js?v='));
    if (!piniaUrl) throw new Error('页面里没有 pinia 资源 —— 挂载那一步没成功');
    const P = await import(piniaUrl);
    P.setActivePinia(window.__pinia);
    const m = await import('/src/stores/useChatStore.ts');
    const chat = m.useChatStore();
    chat.mentionedConvs = new Set(arg.mention ? [arg.convId] : []);
    chat.openTodoByConv = arg.todos ? { [arg.convId]: arg.todos } : {};
    const written = { mention: chat.mentionedConvs.size, todo: JSON.stringify(chat.openTodoByConv) };
    const t = window.__probe.vue;
    await t.nextTick(); await t.nextTick();
    const I = await import('/src/i18n/index.ts');
    const row = document.querySelector('[role="button"][aria-label]');
    return {
      label: row ? (row.getAttribute('aria-label') || '') : null,
      text: row ? (row.innerText || '') : '',
      badges: Array.from(document.querySelectorAll('[class*="min-w-4"]'))
        .map((e) => (e.textContent || '').trim()),
      wantMention: I.t('msg.mentioned'),
      wantUnread: I.t('conv.unread', { name: arg.name, n: arg.unread }),
      wantTodo: I.t('todo.openForMe', { n: arg.todos }),
      dbg: { written: written, seen: Array.from(chat.mentionedConvs).join('|'),
        todos: JSON.stringify(chat.openTodoByConv), sid: chat.$id,
        registered: (window.__pinia && window.__pinia._s && window.__pinia._s.size) || -1 },
    };
  };
  H.grid = () => Array.from(document.querySelectorAll('button:not(#trigger)'));
  H.gridInfo = () => {
    const btns = H.grid();
    const el = document.activeElement;
    const r = el && el.getBoundingClientRect ? el.getBoundingClientRect() : null;
    return {
      count: btns.length,
      focusIdx: btns.indexOf(el),
      focusId: el ? (el.id || el.tagName) : 'none',
      x: r ? Math.round(r.left) : -1,
      y: r ? Math.round(r.top) : -1,
      triggerActive: el === document.getElementById('trigger'),
    };
  };
  // 列数**不写死也不读 class**：按第一行里不同的 x 数出来。
  // 组件里 ↑↓ 的步长是一个写死的 COLS，而真正的列数由布局决定 —— 两者不同步的表现就是跳错行，
  // 所以判据的分母必须来自布局本身，不能来自我对 class 的解读。
  H.cols = () => {
    const btns = H.grid();
    if (!btns.length) return -1;
    const geo = btns.map((b) => { const r = b.getBoundingClientRect(); return { x: Math.round(r.left), y: Math.round(r.top) }; });
    const top = Math.min(...geo.map((g) => g.y));
    const firstRow = geo.filter((g) => Math.abs(g.y - top) <= 2);
    return new Set(firstRow.map((g) => g.x)).size;
  };
  H.focusTrigger = () => { document.getElementById('trigger').focus(); };
  /** 命中列表的当前高亮：组件用 data-hit + aria-current 两个钩子表达，读的就是这两个钩子。 */
  H.hits = () => {
    const list = Array.from(document.querySelectorAll('[data-hit]'));
    return {
      n: list.length,
      at: list.findIndex((el) => el.getAttribute('aria-current') === 'true'),
      ids: list.map((el) => el.getAttribute('data-msg') || el.getAttribute('data-hit')),
    };
  };
  return H;
})();
true;
`;

async function waitForVite(child, timeoutMs = 90_000) {
  const t0 = nowMs();
  let buf = "";
  const p = new Promise((res, rej) => {
    child.stdout.on("data", (d) => {
      buf += d.toString();
      if (/Local:/.test(buf)) res(true);
    });
    child.stderr.on("data", (d) => { buf += d.toString(); });
    child.on("exit", (c) => rej(new Error(`vite 提前退出 ${c}：\n${buf.slice(-800)}`)));
  });
  await Promise.race([p, sleep(timeoutMs).then(() => { throw new Error(`vite ${timeoutMs}ms 没起来：\n${buf.slice(-800)}`); })]);
  return `http://127.0.0.1:${VITE_PORT}/`;
}

/**
 * 会话行那两枚徽标（#88 那四处读屏缺陷与 #117 剩下的那一半）。
 *
 * 这一格以前只有两种证据：源码守卫（禁手写徽标副本）+ 我拿系统控件树**手工**看过一次。
 * 而 #88 的真实形状是「屏幕上看得见、树里读不到」—— 只有把事实**并进这一行自己的
 * accessible name** 才算修好。所以这段判四件事各自的条件：未读数、有人@我、与我相关的
 * 开放任务数三句都得在名字里，**且各自不亮的时候不许混进来**（只钉「都在」会被
 * 「永远把三句都拼进去」那种写法混过去 = 半个守卫）。
 * ⚠️ 层次边界：徽标「该不该亮」的判定在 Rust 与 store 侧，另有单元与跨进程判据；
 *   这一段只判**亮起来之后界面与读屏得不到得到它**。真 WKWebView / WebView2 仍归 Smoke-11。
 */
async function runConvBadge(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  const GROUP = {
    id: "group:probe-badge", kind: "group", name: "验收群", avatar: null,
    unread: 3, last_msg: "收到一条", last_ts: 1, pinned: false,
  };
  // 未读是**prop**（挂载时刻冻住），徽标态是 **store**（随时可变）⇒ 想要"没有未读"那一档，
  // 只能换一个 conv 重新挂一次，不能靠改探针参数。
  const GROUP0 = { ...GROUP, unread: 0 };
  const PRIVATE = { ...GROUP0, id: "p:probe-badge", kind: "private", name: "验收人" };
  const mount = async (conv) => {
    await cdp.eval("window.__probe.install('/src/components/conversation/ConversationListItem.vue', '', "
      + JSON.stringify(JSON.stringify({ conv, active: false, online: null })) + ")");
    await sleep(900);
  };
  // ⚠️ 这里只做**一层** stringify：convRow 收的是对象，而 install 的第三参本身是字符串
  //   （那一句才要双层）。写成双层会让页面拿到一个字符串、arg.mention 恒为 undefined
  //   ⇒ 徽标永远不亮，而红得完全像"产品没渲染出来"（本轮就这么红过一次，6 条全红）。
  const probe = async (conv, mention, todos) => cdp.eval(
    "window.__probe.convRow(" + JSON.stringify({
      convId: conv.id, name: conv.name, unread: conv.unread, mention, todos }) + ")");

  // —— ① 三态齐：未读 + @我 + 与我相关的开放任务 ——
  await mount(GROUP);
  const a = await probe(GROUP, true, 2);
  check("三态齐时这一行的 accessible name 同时含未读那句、有人@我、与我相关的任务数",
    !!a.label && a.label.includes(a.wantUnread) && a.label.includes(a.wantMention)
      && a.label.includes(a.wantTodo), "三句都在名字里", JSON.stringify(a.label));
  check("三态齐时两枚数字徽标各自可见（未读 3 与任务 2）",
    a.badges.includes("3") && a.badges.includes("2"), "徽标含 3 与 2", JSON.stringify(a.badges));
  check("有人@我那句要在行内可见文本里（不能只挂在名字上，鼠标用户也得看得见）",
    a.text.includes(a.wantMention), "可见文本含这句", JSON.stringify(a.text.slice(0, 60)));

  // —— ② 有未读、有 @我，但没有待办：任务那句不许混进来 ——
  const b = await probe(GROUP, true, 0);
  check("没有与我相关的任务时，名字里不许出现那句任务数（三句各自有条件）",
    b.label.includes(b.wantMention) && b.label.includes(b.wantUnread)
      && !b.label.includes(b.wantTodo), "含@我与未读、不含任务句", JSON.stringify(b.label));

  // —— ③ 对照：三态全清 ⇒ 名字恰好等于群名、一枚徽标都没有 ——
  await mount(GROUP0);
  const c = await probe(GROUP0, false, 0);
  check("对照：三态全清时名字恰好等于群名、零枚徽标（证明①不是恒过）",
    c.label === GROUP0.name && c.badges.length === 0 && !c.text.includes(c.wantMention),
    "名字 = 验收群 且 0 枚徽标", `${JSON.stringify(c.label)} / 徽标=${JSON.stringify(c.badges)}`);

  // —— ④ 只有 @我（未读 0）：另两句都不该出现 ——
  const d = await probe(GROUP0, true, 0);
  check("只有@我时名字里只有群名与那句@我（未读与任务两句没资格出现）",
    d.label === `${GROUP0.name}，${d.wantMention}`, "验收群，<@我那句>", JSON.stringify(d.label));

  // —— ⑤ 单聊没有群任务：store 里有数也不许念出来 ——
  await mount(PRIVATE);
  const e = await probe(PRIVATE, false, 2);
  check("单聊行不许出现「与我相关的任务数」（那一族只属于群聊，口径由组件把住）",
    !e.label.includes(e.wantTodo) && e.label === PRIVATE.name, "名字 = 验收人",
    JSON.stringify(e.label));
}

async function main() {
  const chrome = findChrome();
  if (!chrome) {
    // 缺件必须红，不能"少一条 ✅"就当过了（§十）
    check("环境：找得到 Chrome for Testing（playwright 缓存或 GOSSLAN_CHROME）", false,
      "一个可执行文件", "没找到 —— 这一格就没跑");
    console.log("\n✗ UI 运行时探针：环境缺件，判红（不是跳过）");
    process.exit(1);
  }
  console.log(`· 浏览器：${chrome}`);
  const viteBin = path.join(ROOT, "node_modules", "vite", "bin", "vite.js");
  if (!fs.existsSync(viteBin)) {
    check("环境：仓里有 vite 可执行（node_modules/vite/bin/vite.js）", false, "存在", "不存在");
    process.exit(1);
  }
  const vite = spawn(process.execPath, [viteBin, "dev", "--port", String(VITE_PORT), "--strictPort"],
    { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "gosslan-probe-"));
  const chromeProc = (() => {
    const p = spawn(chrome, [
      "--headless=new", `--remote-debugging-port=${CDP_PORT}`, `--user-data-dir=${profile}`,
      "--no-first-run", "--no-default-browser-check", "--disable-gpu", "about:blank",
    ], { stdio: "ignore" });
    return p;
  })();
  let exitCode = 0;
  try {
    const url = await waitForVite(vite);
    console.log(`· dev server：${url}`);
    let wsUrl = null;
    for (let i = 0; i < 60 && !wsUrl; i += 1) {
      await sleep(500);
      try {
        const r = await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`);
        const list = await r.json();
        wsUrl = (list.find((t) => t.type === "page") || {}).webSocketDebuggerUrl ?? null;
      } catch { /* 浏览器还在起 */ }
    }
    if (!wsUrl) throw new Error("CDP /json/list 里一直没有 page target");
    const cdp = await Cdp.connect(wsUrl);
    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    await cdp.send("Page.navigate", { url });
    // 冷启动要等 vite 现编译依赖，实测 20~30s（等短了会把"还没起来"读成"组件坏了"）
    await sleep(4_000);
    for (let i = 0; i < 40; i += 1) {
      const ready = await cdp.eval(`(() => {
        const el = document.querySelector('#app');
        return !!(el && (el.children.length > 0 || document.body.innerText.length > 0));
      })()`);
      if (ready) break;
      await sleep(1_000);
    }
    await cdp.eval(PAGE_FIXTURE);

    const emoji = !ONLY || ONLY === "emoji";
    const search = !ONLY || ONLY === "search";
    const task = !ONLY || ONLY === "task";
    const convbadge = !ONLY || ONLY === "convbadge";
    if (emoji) await runEmoji(cdp, url);
    if (search) await runSearch(cdp, url);
    if (task) await runTaskCard(cdp, url);
    if (convbadge) await runConvBadge(cdp, url);
    process.exitCode = results.every((r) => r.ok) ? 0 : 1;
    exitCode = process.exitCode;
  } catch (e) {
    check("探针自己跑完了（没在半路抛错）", false, "正常结束", String(e && e.message || e).slice(0, 300));
    exitCode = 1;
  } finally {
    const tally = results.filter((r) => r.ok).length;
    console.log(`\n${exitCode === 0 ? "✅" : "✗"} UI 运行时探针：${tally}/${results.length} 条判据绿`
      + (SELF_CHECK ? "（这一段是反面对照：判据**必须**报红）" : ""));
    vite.kill("SIGTERM");
    chromeProc.kill("SIGTERM");
    await sleep(500);
    try { fs.rmSync(profile, { recursive: true, force: true }); } catch { /* 交给系统 */ }
    process.exit(exitCode);
  }
}

/** 表情面板那一段：键盘出口（N1）+ 反向对照（不带 text 的 Enter 必须零次激活）。 */
async function runEmoji(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  await cdp.eval(`window.__probe.install('/src/components/EmojiPicker.vue',
    '<button id="trigger" data-probe-panel>触发按钮</button>')`);
  const info = () => cdp.eval("window.__probe.gridInfo()");

  // 面板里格子的列数由布局决定，而 ↑↓ 的步长写的是组件里的 COLS。
  // 这两处不同步的表现就是"上下键跳错行"，所以列数从布局现读、且**不许回退成猜的常数**
  // （回退了就等于拿我自己写的 8 去验它写的 8）。
  await cdp.eval("window.__probe.focusTrigger()");
  await cdp.eval("window.__probe.setOpen(true)");
  const cols = await cdp.eval("window.__probe.cols()");
  const first = await info();
  check("打开面板：焦点落在第一格（键盘用户不该从头再 Tab 一遍）",
    first.focusIdx === 0 && first.count > 0, "焦点=第 0 格且格子数 >0",
    `focusIdx=${first.focusIdx} count=${first.count}`);
  check("列数量得出来（现读布局；量不出来说明面板没真渲染出网格）",
    cols > 1, ">1", cols);

  await cdp.key("ArrowRight", "ArrowRight", "", 39);
  const right = await info();
  check("→ 让焦点右移恰好一格", right.focusIdx === 1, "focusIdx=1", right.focusIdx);
  await cdp.key("ArrowLeft", "ArrowLeft", "", 37);
  const left = await info();
  check("← 让焦点左移恰好一格", left.focusIdx === 0, "focusIdx=0", left.focusIdx);

  await cdp.key("ArrowDown", "ArrowDown", "", 40);
  const down = await info();
  check("↓ 走的是**整行**：位移等于现读的列数（COLS 与真实布局一旦不同步这里就红）",
    cols > 1 && down.focusIdx === cols, `focusIdx=${cols}`,
    `${down.focusIdx} / 列数=${cols}`);
  check("↓ 之后横向位置没变（跳错行的真实表现就是 x 偏移）",
    down.x === first.x && down.y > first.y, `x=${first.x} 且 y 变大`,
    `x=${down.x} y=${down.y}（原 y=${first.y}）`);
  await cdp.key("ArrowUp", "ArrowUp", "", 38);
  const up = await info();
  check("↑ 走回同一格", up.focusIdx === 0 && up.x === first.x, "focusIdx=0 且 x 不变",
    `${up.focusIdx} / x=${up.x}`);

  await cdp.key("ArrowLeft", "ArrowLeft", "", 37);
  const clamp = await info();
  check("首格再按 ← 不越界、也不丢焦点（不钳住的话焦点掉回 body，读屏用户丢了位置）",
    clamp.focusIdx === 0, "focusIdx=0", `${clamp.focusIdx} (focusId=${clamp.focusId})`);

  // 反向对照：Enter **不带 text** 时浏览器不会激活被聚焦的 button ⇒ 必须零次 select。
  // 这条不是装饰：没有它，"Enter 选中"那格可能只是探针自己造出来的假绿。
  await cdp.eval("window.__probe.reset()");
  await cdp.key("Enter", "Enter", "", 13);
  const noText = await cdp.eval("window.__probe.events.length");
  check("对照：Enter 不带 text ⇒ 零次激活（证明下面那条不是恒过）",
    noText === 0, "0 次", `${noText} 次`);

  await cdp.eval("window.__probe.reset()");
  await cdp.key("Enter", "Enter", "\r", 13);
  const enterEv = await cdp.eval("window.__probe.events");
  const enterClicks = await cdp.eval("window.__probe.clicks");
  check("Enter 选中当前格：恰好一次 select，且 click 与 emit 一一对应",
    enterEv.length === 1 && enterEv[0][0] === "select" && enterClicks === 1,
    "1 次 select / 1 次 click", `${JSON.stringify(enterEv)} / clicks=${enterClicks}`);

  await cdp.eval("window.__probe.reset()");
  await cdp.key(" ", "Space", " ", 32);
  const spaceEv = await cdp.eval("window.__probe.events");
  check("空格同样选中一次（不该出现「一次按下塞两份」）",
    spaceEv.length === 1 && spaceEv[0][0] === "select", "1 次 select", JSON.stringify(spaceEv));

  await cdp.eval("window.__probe.reset()");
  await cdp.key("Escape", "Escape", "", 27);
  const escEv = await cdp.eval("window.__probe.events");
  check("Esc 关面板：emit 一次 close", escEv.length === 1 && escEv[0][0] === "close",
    "1 次 close", JSON.stringify(escEv));
  // 父组件（这里就是夹具）收到 close 才把 open 翻回 false —— 与真实调用方一致
  await cdp.eval("window.__probe.setOpen(false)");
  const after = await info();
  check("关掉之后焦点**还给触发按钮**（不还就等于键盘用户要从页头重走一遍）",
    after.triggerActive === true, "activeElement 是那个按钮", JSON.stringify(after));
}

/**
 * 搜索弹窗那一段（原生感走查 N2）。这一格里最要紧的不是"方向键能动"，而是
 * **回车打开的必须是高亮那一条** —— 旧行为是鼠标与回车都固定开 `messages[0]`，
 * 也就是"看起来在选、实际打开的是第一条"。所以判据钉的是 emit 出去的 msgId 本身。
 */
async function runSearch(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  const GROUP = {
    conv_id: "conv-probe", name: "探针会话", kind: "private", avatar: null,
    total: 3, latest_ts: 1,
    messages: [0, 1, 2].map((i) => ({
      msg_id: `m${i}`, sender_id: "p1", sender_name: "小布",
      kind: "text", content: `命中 ${i}`, ts: 1 + i,
    })),
  };
  await cdp.eval(`window.__probe.install('/src/components/search/ChatSearchDialog.vue', '')`);
  await cdp.eval(`window.__probe.stubApi('searchChatHistory', ${JSON.stringify([GROUP])})`);
  await cdp.eval("window.__probe.setOpen(true)");
  const typed = await cdp.eval(`window.__probe.typeKeyword('命中')`);
  const hits = () => cdp.eval("window.__probe.hits()");
  const h0 = await hits();

  check("输入关键词后命中列表真渲染出 3 行（api 替身被真调用、结果真进了组件）",
    h0.n === 3, "3 行", `${h0.n} 行 / 输入框聚焦=${typed.focused}`);
  check("默认高亮落在第一条（不是没有高亮，也不是越界）",
    h0.at === 0, "at=0", h0.at);

  await cdp.key("ArrowDown", "ArrowDown", "", 40);
  const h1 = await hits();
  check("↓ 把高亮移到第二条", h1.at === 1, "at=1", h1.at);
  await cdp.key("ArrowDown", "ArrowDown", "", 40);
  const h2 = await hits();
  check("再按 ↓ 移到第三条", h2.at === 2, "at=2", h2.at);
  await cdp.key("ArrowDown", "ArrowDown", "", 40);
  const hClamp = await hits();
  check("最后一条再按 ↓ 钳住不动（越界会让 aria-current 消失，读屏就丢了当前位置）",
    hClamp.at === 2, "at=2", hClamp.at);
  await cdp.key("ArrowUp", "ArrowUp", "", 38);
  const hUp = await hits();
  check("↑ 把高亮移回第二条", hUp.at === 1, "at=1", hUp.at);

  await cdp.key("ArrowDown", "ArrowDown", "", 40);
  await cdp.eval("window.__probe.reset()");
  const beforeEnter = await hits();
  await cdp.key("Enter", "Enter", "\r", 13);
  const ev = await cdp.eval("window.__probe.events");
  const opened = ev.filter((e) => e[0] === "open").map((e) => JSON.parse(e[1]));
  check("在高亮那条上按回车：emit 的 open-conversation 必须是**高亮那条**（旧行为固定开第一条）",
    beforeEnter.at === 2 && opened.length === 1 && opened[0].msgId === "m2"
    && opened[0].convId === "conv-probe",
    "1 次 open 且 msgId=m2", `at=${beforeEnter.at} / ${JSON.stringify(opened)}`);
  check("跟着必须关一次弹窗（打开与关闭是同一次动作，不能只 emit 不关）",
    ev.filter((e) => e[0] === "close").length === 1, "1 次 close", JSON.stringify(ev));
}

/**
 * 任务卡片那一段（#97：把「那张卡真出现在时间线里」从"没有断言"换成运行时断言）。
 *
 * 上游本来就有三道锁，但它们判的都不是界面：`storeContract` 判"记录进没进 store"、
 * `messageKinds` 判"会不会被时间线过滤掉 / 有没有人渲染它"、`todos.test.ts` 判数据层。
 * 用户最初报的是**看不见** —— 那件事只有真渲染一次才判得了，所以这一段挂的是
 * `TodoCardBubble` 本身（不是 MessageItem：气泡容器要 Tauri 事件与一堆 store，
 * 在探针里挂它会让人怀疑红的是夹具而不是产品）。
 *
 * 顺带把 #112（卡片图走统一预览）与 §9（描述里的 @）这两条在运行时层各判一次 ——
 * 结构级判据说"调用点给了 clickable"，这里判"按 Enter 之后那份预览真的开了"。
 */
async function runTaskCard(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);

  const TODO_ID = "todo-probe-1";
  const CID = "a".repeat(64);
  const payload = {
    todo_id: TODO_ID,
    title: "探针任务：把周报发出去",
    assignees: ["dev-other"],
    status: "todo",
    creator: "dev-me",
    deleted: false,
    description: "@小布 记得带上周报",
    images: [{ id: CID, name: "白板照片.jpg", size: 2048, sha256: CID, subtype: "image" }],
    archived: false,
    done_at: null,
  };
  const MSG = JSON.stringify({
    msg_id: "m-probe-1", sender_id: "dev-me", kind: "todo",
    content: JSON.stringify(payload), ts: 1700000000000,
  });
  // 自己视角的标签从 i18n 取，不在这里抄「@你」字面量（本仓刚给这条加了禁令）
  const labels = await cdp.eval(`(async () => {
    const u = await import('/src/utils/todos.ts');
    const i = await import('/src/i18n');
    return { todo: i.t(u.TODO_STATUS_LABEL_KEY.todo), done: i.t(u.TODO_STATUS_LABEL_KEY.done),
      selfTag: i.t('mention.self') };
  })()`);

  const mount = async (props) => {
    await cdp.eval(`window.__probe.install('/src/components/TodoCardBubble.vue', '', ${JSON.stringify(JSON.stringify(props))})`);
    await sleep(1_200); // 缩略图那一族是"字节后到"的：先 mount 再等一次取字节的 promise
  };
  const text = () => cdp.eval("window.__probe.text()");

  // —— 先按"对端视角"挂一次（没有 self 标签、没有实时状态表）——
  await cdp.eval(`window.__probe.stubApi('readContentPreview', [137,80,78,71,13,10,26,10])`);
  await mount({
    message: JSON.parse(MSG),
    mentionNames: ["小布", "我"],
    selfMention: null,
  });
  const t0 = await text();
  check("卡片渲染出这条任务的标题（用户报的是「看不见」，那就先判标题真在 DOM 里）",
    t0.includes("把周报发出去"), "含标题", t0.slice(0, 80));
  check("卡片不许把载荷当原始 JSON 画出来（内部字段名一个都不该露出）",
    !t0.includes("todo_id") && !t0.includes("done_at"), "不出现内部字段名", t0.slice(0, 80));
  check("状态胶囊显示载荷快照那一档（没传实时表 ⇒ 退回创建时）",
    t0.includes(labels.todo) && !t0.includes(labels.done), labels.todo, t0.slice(0, 80));
  check("对端视角里 @ 到的名字仍是原文（不是「@你」）",
    t0.includes("@小布") && !t0.includes(labels.selfTag), "@小布", t0.slice(0, 80));

  // —— 同一个数据换"我自己"的视角：同一段文本必须换标签 ——
  await mount({
    message: JSON.parse(MSG),
    mentionNames: ["小布", "我"],
    selfMention: { name: "小布", label: labels.selfTag },
  });
  const t1 = await text();
  const selfEls = await cdp.eval(`document.querySelectorAll('.mention-token--self').length`);
  check("换成我的视角：同一段 @ 渲染成 i18n 里那个自我标签（§9 的运行时那一半）",
    t1.includes(labels.selfTag) && selfEls === 1, "标签出现且 --self 恰好 1 个",
    `${JSON.stringify(t1.slice(0, 60))} / --self=${selfEls}`);

  // —— 实时状态表那一环（#23：载荷是创建时快照，卡片必须查父层那张表）——
  await mount({
    message: JSON.parse(MSG),
    mentionNames: ["小布"],
    selfMention: null,
    liveStatusPairs: [[TODO_ID, "done"]],
  });
  const t2 = await text();
  check("传了实时表说这条已完成 ⇒ 胶囊换成已完成（查不到才退回快照，查到就不许再显示旧值）",
    t2.includes(labels.done) && !t2.includes(labels.todo), labels.done, t2.slice(0, 80));

  // —— 键盘可达 + 按 Enter 真的开了那份统一预览 ——
  const thumb = await cdp.eval(`window.__probe.el('[role="button"]')`);
  check("卡片里的缩略图是键盘可达的（role=button + tabindex=0 + 有可访问名）",
    !!thumb && thumb.tabindex === "0" && !!thumb.aria, "role=button/tabindex=0/aria-label",
    JSON.stringify(thumb));
  const focused = await cdp.eval("window.__probe.focusThumb()");
  await cdp.eval("window.__probe.reset()");
  await cdp.key("Enter", "Enter", "\r", 13);
  const pv = await cdp.eval("window.__probe.previewState()");
  check("焦点在缩略图上按 Enter：统一预览 store 被打开，且来源标记是这张卡的 task-card:<todoId>",
    focused === true && pv.open === true && pv.source === "task-card:" + TODO_ID
    && pv.n === 1 && pv.index === 0,
    "open=true source=task-card:" + TODO_ID + " n=1 index=0", JSON.stringify(pv));

  // —— #116：@ 色块落在**最后一行可见行**时，会不会被卡片那段固定行高的裁切盒削掉 ——
  // `.mention-token` 有上下各 1px 内衬（`style.css`），而任务卡描述是 `max-h-[4.2em] overflow-hidden`。
  // 判的是矩形，不是肉眼：色块底边越过裁切盒 = 削。
  const clipMsg = JSON.parse(MSG);
  clipMsg.content = JSON.stringify({
    ...JSON.parse(clipMsg.content),
    description: "第一行 @小布\n第二行\n第三行结尾 @小布",
  });
  await mount({
    message: clipMsg,
    mentionNames: ["小布"],
    selfMention: { name: "小布", label: labels.selfTag },
  });
  const geo = await cdp.eval(`(() => {
    const toks = document.querySelectorAll('.mention-token--self');
    const last = toks[toks.length - 1];
    if (!last) return { found: false };
    const box = last.closest('p');
    if (!box) return { found: false, reason: '色块不在 <p> 里' };
    const t = last.getBoundingClientRect(), b = box.getBoundingClientRect();
    return { found: true, count: toks.length,
      bottomOver: +(t.bottom - b.bottom).toFixed(1), topUnder: +(b.top - t.top).toFixed(1),
      boxH: +b.height.toFixed(1), tokH: +t.height.toFixed(1),
      clipped: box.scrollHeight > box.clientHeight + 1 };
  })()`);
  check("#116：描述裁切盒落在整行上（最后一行的 @ 色块不被削半截；色块自带上下各 1px 内衬 ⇒ 容差 1px）",
    geo.found === true && geo.bottomOver <= 1 && geo.topUnder <= 1,
    "bottomOver<=1 且 topUnder<=1", JSON.stringify(geo));

  // —— 点「查看任务」必须带**这条**的 id（带错/带空就是开别人的任务）——
  await cdp.eval("window.__probe.reset()");
  const clicked = await cdp.eval(`(() => {
    const btns = Array.from(document.querySelectorAll('button'));
    const b = btns[btns.length - 1];
    if (!b) return false;
    b.click();
    return true;
  })()`);
  const ev = await cdp.eval("window.__probe.events");
  const opened = ev.filter((e) => e[0] === "open-id").map((e) => e[1]);
  check("卡片底部那个入口 emit 的是**这条**的 todo_id（不是 undefined、也不是别条）",
    clicked === true && opened.length === 1 && opened[0] === TODO_ID,
    "1 次 open 且带 " + TODO_ID, JSON.stringify(ev));
}

await main();
