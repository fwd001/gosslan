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
/** `--shot=/tmp/x.png` 时顺手存一张会话行的真图（§10 的后半句要肉眼判）。不给就一字不动。 */
const SHOT_PATH = argv.find((a) => a.startsWith("--shot="))?.slice("--shot=".length) ?? "";
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

/**
 * 找可用浏览器。优先级（2026-10-07 加了第 3 档，负责人点头"用我自己的浏览器跑"）：
 *   ① `GOSSLAN_CHROME` 指定的那一个（显式覆盖永远第一）；
 *   ② playwright 缓存里的 Chrome for Testing —— **但"文件存在"不等于"起得来"**：
 *      本机实测缓存那台丢了 Framework 二进制（`Versions/<ver>/… Framework` 不见了），
 *      进程一触即溃 ⇒ 探针只会报 "CDP /json/list 里一直没有 page target"，看着像代码问题。
 *      所以这里做一次性**可加载性**核对（.app 就核对那份 Framework 文件在不在）。
 *   ③ 系统里已装的 Chromium 系浏览器（Brave / Chrome / Edge / Chromium）。
 * ⚠️ 三档全空 ⇒ **判红，不是跳过**（§十：没跑不许写成 PASS）。
 *    走 ③ 时会把"实际用的是谁"打在 `BROWSER_SOURCE` 里并由调用方印出来 ——
 *    换内核跑属于**可追溯的降级**，不许静默：同一条判据在别的内核上测到的东西名义上不同。
 */
let BROWSER_SOURCE = "";

/**
 * .app 包里的框架二进制是否真在（不在 = 这台装坏了）。
 *
 * 判据取自 macOS 的 bundle 布局：`Versions/Current` 是指向真版本目录的符号链接，
 * 加载器要的是 `Versions/Current/<Bundle 同名> Framework` 那个文件。本机实测那台缓存浏览器
 * 只剩 `Helpers/Libraries/Resources` 三个目录、这个文件没了 ⇒ dlopen 失败、进程一触即溃，
 * 而探针只会报"没有 page target"（看着像代码坏了）。所以**存在 ≠ 起得来**。
 */
function appBundleLoadable(binPath) {
  const fwDir = path.join(path.dirname(binPath), "..", "Frameworks");
  let frameworks;
  try {
    frameworks = fs.readdirSync(fwDir);
  } catch {
    return true; // 不是 .app 布局（Linux 那种散装），只做可执行核对
  }
  const bundle = frameworks.find((n) => /Framework\.framework$/.test(n));
  if (!bundle) return false;
  const core = bundle.slice(0, -".framework".length);
  return fs.existsSync(path.join(fwDir, bundle, "Versions", "Current", core));
}

function usable(p) {
  try {
    fs.accessSync(p, fs.constants.X_OK);
  } catch {
    return false;
  }
  return appBundleLoadable(p);
}

function findChrome() {
  const envBin = process.env.GOSSLAN_CHROME;
  if (envBin && fs.existsSync(envBin)) {
    BROWSER_SOURCE = "GOSSLAN_CHROME 指定";
    return envBin;
  }
  const base = process.platform === "darwin"
    ? path.join(os.homedir(), "Library", "Caches", "ms-playwright")
    : path.join(process.env.HOME || os.homedir(), ".cache", "ms-playwright");
  const cands = [];
  if (fs.existsSync(base)) {
    for (const dir of fs.readdirSync(base)) {
      if (!/^chromium-/.test(dir)) continue;
      cands.push(path.join(base, dir, "chrome-mac-arm64", "Google Chrome for Testing.app",
        "Contents", "MacOS", "Google Chrome for Testing"));
      cands.push(path.join(base, dir, "chrome-linux64", "chrome"));
    }
  }
  const pinned = cands.find(usable);
  if (pinned) {
    BROWSER_SOURCE = "playwright 缓存（Chrome for Testing）";
    return pinned;
  }
  const system = [
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ];
  const fallback = system.find(usable);
  if (fallback) {
    BROWSER_SOURCE = cands.some((p) => fs.existsSync(p))
      ? "系统浏览器回退（playwright 缓存里那台存在但装坏了：框架二进制缺失）"
      : "系统浏览器回退（没装 playwright 那份缓存）";
    return fallback;
  }
  return null;
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
  /**
   * 把指针真的移到某个坐标上（`Input.dispatchMouseEvent`）。
   * 为什么要两条：CSS 的 `:hover` 只在浏览器真的移动过指针时才成立，
   * 用 `elementFromPoint` 或加 class 都替代不了 —— 而那颗表情入口恰恰是 `hidden group-hover/msg:flex`
   * （不悬停就根本不存在于布局里，量不到任何几何）。先移到别处再移到目标点，逼一次重新命中。
   */
  async hover(x, y) {
    const send = (cx, cy) => this.ws.send(JSON.stringify({
      id: ++this.seq, method: "Input.dispatchMouseEvent",
      params: { type: "mouseMoved", x: cx, y: cy },
    }));
    send(Math.max(0, x - 120), Math.max(0, y - 60));
    await sleep(30);
    send(x, y);
    await sleep(120);
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
    // 样式表**也要等**，而且是同一类竞态的另一半：vite dev 把 style.css 当模块注入，
    // navigate 之后它可能还在飞。没穿上衣服的页面里所有计算样式都是 0px / clip，
    // 于是「两枚徽标的半径不同」「名字必须是省略号」这类判据会报成设计回归 —— 而真相只是时机。
    // （2026-09-30 本地实测：同一份内容两次跑，第一次这两条红、第二次全绿 ⇒ 不是回归，是没等。）
    // 判据现读 :root 上那个 token：它由 style.css 定义，取到非空值就等于那份样式表已经生效。
    let cssReady = false;
    for (let i = 0; i < 60 && !cssReady; i += 1) {
      const tok = String(getComputedStyle(document.documentElement).getPropertyValue('--gosslan-radius-pill') || '').trim();
      cssReady = tok !== '';
      if (!cssReady) await new Promise((r) => setTimeout(r, 250));
    }
    if (!cssReady) {
      throw new Error('15s 内样式表没生效（--gosslan-radius-pill 取不到值）⇒ 计算样式全是 0，下面的判据只能假红');
    }
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
    // message / liveTodo / mentionNames / selfMention ⇒ 第三参收一份 JSON props。
    // liveTodo 是 Map，JSON 传不了 ⇒ 约定用 liveTodoPairs（[[id,status,category,number],…]）在这里还原，
    // 而且**必须走应用自己那份 todoLiveMap**（import 到的还是组件用的同一个模块实例）：
    // 在这里手写一遍"字母怎么拼、几位要缩短"就是给编号口径开第二个家，探针会先骗过自己。
    const extra = propsJson ? JSON.parse(propsJson) : {};
    if (Array.isArray(extra.liveTodoPairs)) {
      const T = await import('/src/utils/todos.ts');
      extra.liveTodo = T.todoLiveMap(extra.liveTodoPairs.map((p) => ({
        todoId: p[0], status: p[1], category: p[2], number: p[3],
      })));
      delete extra.liveTodoPairs;
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
  /**
   * 标题栏 × 内容浮层那一路的**合成**夹具（2026-09-30 用户：「所有独立窗口标题栏都不会被内容栏遮住」）。
   * 单挂一个组件量不出这件事：要判的是**绘制顺序 + 命中测试**，必须让 caption 与遮罩真的在同一个
   * 根层叠上下文里共存。这里按辅助窗口的真实组成挂：TitleBar + 内容 + ImageLightbox（teleport 到 body）。
   * topInset 给值 = 独立预览窗口那一路；给空串 = 主窗口那一路（铺满整窗，只靠层级让开）。
   */
  H.composeStack = async (topInset) => {
    const V = window.__probe.vue;
    if (!V) return { ok: false, why: '先跑一次 install 把页面那份 vue 拿进来' };
    const [tbM, lbM] = await Promise.all([
      import('/src/components/TitleBar.vue'),
      import('/src/components/message/ImageLightbox.vue'),
    ]);
    const TitleBar = tbM.default;
    const Lightbox = lbM.default;
    document.body.innerHTML = '';
    const host = document.createElement('div');
    host.id = 'stack-host';
    host.className = 'flex h-screen flex-col overflow-hidden';
    document.body.appendChild(host);
    const Root = {
      render() {
        return V.h('div', { class: 'flex h-full flex-col' }, [
          V.h(TitleBar, { title: '相闻 · 图片预览' }),
          V.h('div', { class: 'min-h-0 flex-1 bg-white' }, '内容区'),
          V.h(Lightbox, {
            images: [{ dataSrc: 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7', name: 'probe.gif' }],
            index: 0,
            open: true,
            topInset: topInset || undefined,
            hideClose: true,
            onClose: () => { window.__probe.events.push(['close']); },
            'onUpdate:index': () => {},
          }),
        ]);
      },
    };
    const app = V.createApp(Root);
    if (window.__pinia) app.use(window.__pinia);
    app.mount(host);
    await V.nextTick(); await V.nextTick();
    return { ok: true, topInset: topInset || '' };
  };
  /** 现读那一横条与那张遮罩的矩形、层级，以及 caption 中点**实际命中的是谁**。 */
  H.stackRead = () => {
    const cap = document.querySelector('[data-caption]');
    const ov = document.querySelector('[role="dialog"]');
    if (!cap || !ov) return { ok: false, why: '没同时挂出 caption 与遮罩' };
    const cr = cap.getBoundingClientRect();
    const or = ov.getBoundingClientRect();
    const hit = document.elementFromPoint(Math.round(cr.left + cr.width / 2), Math.round(cr.top + cr.height / 2));
    return {
      ok: true,
      capTop: Math.round(cr.top), capBottom: Math.round(cr.bottom),
      capZ: getComputedStyle(cap).zIndex, ovZ: getComputedStyle(ov).zIndex,
      ovTop: Math.round(or.top),
      hitInCaption: !!hit && (hit === cap || cap.contains(hit)),
      hitTag: hit ? (hit.tagName + '.' + String(hit.className).slice(0, 40)) : 'null',
    };
  };
  /** 反面对照用：把 caption 的层级按回修之前（非定位、无 z-index）⇒ 同一点必须不再命中标题栏。 */
  H.setCaptionStack = (mode) => {
    const cap = document.querySelector('[data-caption]');
    if (!cap) return false;
    if (mode === 'off') { cap.style.zIndex = '0'; cap.style.position = 'static'; }
    else { cap.style.zIndex = ''; cap.style.position = ''; }
    return true;
  };

  /**
   * 量那颗"添加表情回复"入口（用户 2026-09-30：「按钮位置在视觉观感上应该与聊天内容最下边对齐，
   * 按钮再紧凑一些，现在有点大」）。
   * 列（=按钮的定位父元素，也就是气泡那一列）不从外面找：按钮就挂在那一列的直接子级上，
   * 它的 parentElement 就是那一列 —— 少一个选择器就少一处会漂的锚。
   */
  H.reactionBtn = () => {
    const btn = document.querySelector('[data-reaction-entry]');
    if (!btn) return { ok: false, why: '没找到那颗入口（canReact 没给？还是这一条根本没挂出来）' };
    const cs = getComputedStyle(btn);
    const b = btn.getBoundingClientRect();
    const col = btn.parentElement;
    const cr = col ? col.getBoundingClientRect() : null;
    return {
      ok: true,
      display: cs.display,
      w: Math.round(b.width), h: Math.round(b.height), bottom: Math.round(b.bottom),
      colBottom: cr ? Math.round(cr.bottom) : null,
      colClass: col ? String(col.className).slice(0, 46) : null,
      title: btn.getAttribute('title'),
      aria: btn.getAttribute('aria-label'),
      // classList 而不是正则啃 className：这段在模板字符串里，反斜杠会被模板吃掉一层
      // （实测写 \s 到了页面里就变成 s ⇒ 那条 tap-safe 判据永远假红）
      tapSafe: btn.classList.contains('tap-safe'),
    };
  };
  /** 悬停目标：那颗入口没悬停时是 display:none（量不到矩形），所以指针要移到它**那一列**上。 */
  H.colRect = () => {
    const btn = document.querySelector('[data-reaction-entry]');
    const col = btn ? btn.parentElement : null;
    if (!col) return null;
    const r = col.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2), bottom: Math.round(r.bottom) };
  };

  /** 对照：把入口按回"旧的中线居中"（top-1/2 + translateY(-50%)）⇒ 底边对齐差必须变大。 */
  H.forceLegacyReaction = (on) => {
    const btn = document.querySelector('[data-reaction-entry]');
    if (!btn) return false;
    if (on) {
      btn.style.top = '50%';
      btn.style.bottom = 'auto';
      btn.style.transform = 'translateY(-50%)';
    } else {
      btn.style.top = '';
      btn.style.bottom = '';
      btn.style.transform = '';
    }
    return true;
  };

  /**
   * 量**表情回应条**与气泡那一列的几何关系（用户 2026-10-07：「左右两条表情回复边缘都与上面气泡对齐，
   * 并且间距紧凑点」）。两侧各量一次：左列看左边缘、右列看右边缘，容差 1px。
   *
   * ⚠️ 量的是**胶囊自己的边**，不是那条容器的边：回应条是通栏的块级容器，padding 长在盒子**里面**
   * ⇒ 拿 getBoundingClientRect() 量那条容器去比气泡，两侧都会恒差一个"条的宽度差"（第一版就这么读出
   * 左右各 60px，差点照着这个假数去改补偿值）。用户那句"边缘对齐"判的是看得到的那两枚胶囊。
   * 裁切者不写死选择器：从条往上找第一个 overflow 不是 visible 的祖先（消息列表那个滚动容器），
   * 因为"谁在裁"这件事本身就是这条判据要回答的一半。
   */
  H.reactionGeom = () => {
    const bar = document.querySelector('[data-reaction-bar]');
    const col = document.querySelector('[data-msg-col]');
    if (!bar || !col) return { ok: false, why: '缺 data-reaction-bar 或 data-msg-col（钩子被改名了？）' };
    const chips = Array.from(bar.querySelectorAll('[data-reaction-chip]'));
    if (chips.length === 0) return { ok: false, why: '条里一颗胶囊都没有 ⇒ 没有可量的边' };
    const rs = chips.map((b) => b.getBoundingClientRect());
    const chipLeft = Math.min(...rs.map((r) => r.left));
    const chipRight = Math.max(...rs.map((r) => r.right));
    const chipTop = Math.min(...rs.map((r) => r.top));
    const b = { left: chipLeft, right: chipRight, top: chipTop };
    const c = col.getBoundingClientRect();
    let clip = null;
    for (let n = bar.parentElement; n; n = n.parentElement) {
      const s = getComputedStyle(n);
      if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { clip = n; break; }
    }
    const cr = clip ? clip.getBoundingClientRect() : null;
    return {
      ok: true,
      deltaLeft: Math.round(b.left - c.left),
      deltaRight: Math.round(b.right - c.right),
      gapToBubble: Math.round(b.top - c.bottom),
      chipLeft: Math.round(b.left), chipRight: Math.round(b.right),
      colLeft: Math.round(c.left), colRight: Math.round(c.right),
      padLeft: getComputedStyle(bar).paddingLeft, padRight: getComputedStyle(bar).paddingRight,
      // 那截竖向间距的构成也要带回来：条自己的 mt 之外，剩下的是**上面那一行**（消息行 = 前一个兄弟节点）
      // 的下内边距 —— 不知道 6px 从哪来就没法判断"再收紧"该动哪一层（动错层会碰到选中底色）。
      barMarginTop: getComputedStyle(bar).marginTop,
      rowPadBottom: bar.previousElementSibling
        ? getComputedStyle(bar.previousElementSibling).paddingBottom : null,
      barClass: String(bar.className).slice(0, 120),
      clipTag: clip ? String(clip.className).slice(0, 36) : 'none',
      clipLeft: cr ? Math.round(cr.left) : null,
      clipRight: cr ? Math.round(cr.right) : null,
      vw: window.innerWidth,
    };
  };

  /** 那颗胶囊的中心点（给 CDP 移指针用）；名单要靠真 hover 才出现，直接改状态就判不到 canHover 那一支。 */
  H.reactionChipPoint = (i) => {
    const all = document.querySelectorAll('[data-reaction-chip]');
    const b = all.length ? all[Math.min(all.length - 1, Math.max(0, i | 0))] : null;
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, n: all.length };
  };

  H.allChipPoints = () => Array.from(document.querySelectorAll('[data-reaction-chip]')).map((el, i) => {
    const r = el.getBoundingClientRect();
    return { i, x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2),
      left: Math.round(r.left), right: Math.round(r.right) };
  });

  /**
   * 通用「浮层会不会被裁」量具 —— 量的是整类缺陷的共同不变量（表情名单、已读列表、
   * 表情选择器是同一个坑的三次现场：absolute 浮层挂在带 overflow 的滚动容器里就一定被切）。
   *
   * 按 CSS 实际语义合成**一条**判据，不钉三条其中两条只是另一条的推论：
   *  1) position:fixed 的包含块是视口 ⇒ 中间祖先的 overflow 裁不到它，body/html 的
   *     overflow 也裁不到它（实测这个夹具页 body 就是 hidden/hidden ⇒ 把 body 算进链
   *     就永远是假红）。真能裁到它的只有「祖先变成了包含块」⇒ transform/filter/
   *     perspective/will-change/contain:paint 这一类连 body/html 一起查，记进 breakers。
   *  2) 非 fixed ⇒ 往上（含 body/html）任何 overflow 不是 visible 的祖先都切它，记进 clipped。
   * 两者都空 = escapes（没有任何东西裁得到它）。
   */
  H.floatLayer = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return { ok: false, why: 'not-mounted' };
    const AXES = ['transform', 'filter', 'perspective'];
    const isBreaker = (st) => (st.transform !== 'none' || st.filter !== 'none'
      || st.perspective !== 'none' || AXES.some((k) => st.willChange.indexOf(k) >= 0)
      || st.contain.indexOf('paint') >= 0);
    const isClipper = (st) => (st.overflowX !== 'visible' || st.overflowY !== 'visible');
    const name = (x) => (x === document.body ? 'body'
      : x === document.documentElement ? 'html' : String(x.className || x.tagName).slice(0, 40));
    const position = getComputedStyle(el).position;
    const anc = [];
    for (let n = el.parentElement; n; n = n.parentElement) anc.push(n);
    const breakers = [];
    const clipped = [];
    anc.forEach((n) => {
      const st = getComputedStyle(n);
      if (isBreaker(st)) breakers.push(name(n));
      if (position !== 'fixed' && isClipper(st)) clipped.push(name(n) + ':' + st.overflowY);
    });
    const b = el.getBoundingClientRect();
    const chain = [];
    for (let n = el; n; n = n.parentElement) {
      chain.push(name(n));
      if (n === document.body) break;
    }
    return {
      ok: true,
      position: position,
      parentIsBody: el.parentElement === document.body,
      clipped: clipped,
      breakers: breakers,
      escapes: clipped.length === 0 && breakers.length === 0,
      inViewport: b.left >= -1 && b.right <= window.innerWidth + 1
        && b.top >= -1 && b.bottom <= window.innerHeight + 1,
      rect: { l: Math.round(b.left), r: Math.round(b.right), t: Math.round(b.top), bo: Math.round(b.bottom) },
      vw: window.innerWidth,
      vh: window.innerHeight,
      chain: chain,
      bodyOverflow: getComputedStyle(document.body).overflowY,
    };
  };

  /** 名单里每一行：长名字必须**真的被省略**（截断 + 有 title 拿到全名），不能撑破面板。 */
  H.reactionRows = () => {
    const r = document.querySelector('[data-reaction-roster]');
    if (!r) return { ok: false };
    const rows = Array.from(r.querySelectorAll('[data-roster-row]'));
    return {
      ok: true,
      panelW: Math.round(r.getBoundingClientRect().width),
      rows: rows.map((el) => ({
        clipped: el.scrollWidth > el.clientWidth + 1,
        ellipsis: getComputedStyle(el).textOverflow === 'ellipsis',
        hasTitle: !!el.getAttribute('title'),
        text: String(el.textContent || '').trim().slice(0, 18),
      })),
    };
  };

  /**
   * 量 hover 展开的「谁点的」名单浮层（用户 2026-10-07：「右边聊天 hover 表情 人员列表 展开遮挡」）。
   * 两问分开答：① 有没有被滚动容器/视口**裁掉**（右缘越界）；② 露出来的那部分**是不是它自己**
   * （elementFromPoint 命中的必须落在名单里 —— 被别的元素盖住时命中的会是别人）。
   */
  H.reactionRoster = () => {
    const r = document.querySelector('[data-reaction-roster]');
    if (!r) {
      // 别只回一句"没弹出来"：这一条在门禁里红过而单独跑是绿的，成因有三种形状
      // （canHover 判假 / 根本没找到胶囊 / 指针没落上去）—— 三种的修法完全不同，一次把三样都带回来。
      const chip = document.querySelector('[data-reaction-chip]');
      const cr0 = chip ? chip.getBoundingClientRect() : null;
      return {
        ok: false,
        why: '名单没弹出来',
        diag: {
          hoverMedia: window.matchMedia('(hover: hover)').matches,
          // 组件里那份 canHover 是**模块加载时**算的，与此刻的 matchMedia 可以不同；
          // 而 hover 通路只绑在它上面 ⇒ 两者不一致就是"事件到了、面板不弹"的那种形状
          hoverCapable: chip ? chip.getAttribute('data-hover-capable') : null,
          chipDisabled: chip ? String(chip.disabled) : null,
          // 组件内部状态标在条上：分清「事件没触发处理器」与「处理器跑了但没渲染」
          openEmoji: (document.querySelector('[data-reaction-bar]') || { getAttribute: () => null })
            .getAttribute('data-open-emoji'),
          chips: document.querySelectorAll('[data-reaction-chip]').length,
          chipRect: cr0
            ? { x: Math.round(cr0.x), y: Math.round(cr0.y), w: Math.round(cr0.width), h: Math.round(cr0.height) }
            : null,
          inViewport: cr0 ? cr0.y >= 0 && cr0.y < window.innerHeight : null,
          // 指针该落在胶囊中心 —— 那里此刻压着谁，直接分开"根本没落上去"与"落上了但被别人抢走"。
          ownerAtChip: (() => {
            if (!cr0) return 'no-chip';
            const el = document.elementFromPoint(
              Math.round(cr0.x + cr0.width / 2), Math.round(cr0.y + cr0.height / 2));
            return el ? el.tagName + '.' + String(el.className || '').slice(0, 40) : 'null';
          })(),
          vw: window.innerWidth, vh: window.innerHeight,
        },
      };
    }
    const b = r.getBoundingClientRect();
    let clip = null;
    for (let n = r.parentElement; n; n = n.parentElement) {
      const s = getComputedStyle(n);
      if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { clip = n; break; }
    }
    const cr = clip ? clip.getBoundingClientRect() : null;
    const vw = window.innerWidth;
    const overRight = cr ? Math.round(b.right - cr.right) : Math.round(b.right - vw);
    const overLeft = cr ? Math.round(cr.left - b.left) : Math.round(0 - b.left);
    const probeX = Math.round(b.left + b.width / 2);
    const probeY = Math.round(b.top + b.height / 2);
    const hit = document.elementFromPoint(probeX, probeY);
    return {
      ok: true,
      left: Math.round(b.left), right: Math.round(b.right), width: Math.round(b.width),
      clipTag: clip ? String(clip.className).slice(0, 36) : 'viewport',
      clipLeft: cr ? Math.round(cr.left) : 0,
      clipRight: cr ? Math.round(cr.right) : vw,
      overRight,
      overLeft,
      hitSelf: !!hit && (hit === r || r.contains(hit)),
      hitTag: hit ? hit.tagName + '.' + String(hit.className).slice(0, 30) : 'null',
      vw,
    };
  };

  /**
   * 反面对照（对齐）：把回应条朝外侧**平移 8px** —— 8px 正是修之前那个补偿值差出来的量
   * （旧写法 4 + 48 = 52，而气泡边缘在 16 + 36 + 8 = 60）。用 margin 而不是改 padding：
   * padding 上有 px-4 与 pl-* 互相覆盖的顺序问题，拿它做对照会把"对照生效了吗"变成另一个未知。
   */
  H.forceAlignSkew = (on) => {
    const bar = document.querySelector('[data-reaction-bar]');
    const col = document.querySelector('[data-msg-col]');
    if (!bar || !col) return false;
    if (on) {
      if (col.classList.contains('items-end')) bar.style.marginRight = '8px';
      else bar.style.marginLeft = '-8px';
    } else {
      bar.style.marginLeft = '';
      bar.style.marginRight = '';
    }
    return true;
  };

  /** 反面对照（名单）：把 fixed 锚点强推到视口右缘外 ⇒ 右溢必须重新出现（证明判据不空转）。 */
  /**
   * 判据的反面对照：把名单**搬回**消息那一侧第一个带 overflow 的祖先里
   * （= 修之前它待的地方）。搬完 floatLayer 必须报出裁切祖先 / 不再挂 body,
   * 否则那条 escape 判据就是恒真。搬回去时按原位置插回，不动 Vue 的记账。
   */
  /**
   * escape 判据的反面对照：把名单搬回「absolute 挂在裁切容器里」= 修之前它的形状。
   * 真实应用里切它的是消息列表那个 overflow 滚动容器，这个夹具页里那一层不存在
   * （名单到 body 之间是空的 ⇒ 上一版对照因此根本搬不动、白报红），所以这里给气泡
   * 那一列补上 overflow 再把名单塞进去。撤的时候按原位置插回、inline 样式逐样还原。
   */
  H.reparentRoster = (on) => {
    const r = document.querySelector('[data-reaction-roster]');
    if (!r) return { ok: false, why: 'no-roster' };
    if (on) {
      const bar = document.querySelector('[data-reaction-bar]');
      const cell = bar ? bar.parentElement : null;
      if (!cell) return { ok: false, why: 'no-column' };
      window.__rosterHome = {
        parent: r.parentElement, next: r.nextSibling,
        cell: cell, cellOverflow: cell.style.overflow, pos: r.style.position,
      };
      cell.style.overflow = 'hidden';
      r.style.position = 'absolute';
      cell.appendChild(r);
      return { ok: true, into: String(cell.className || cell.tagName).slice(0, 40) };
    }
    const h = window.__rosterHome;
    if (!h || !h.parent) return { ok: false, why: 'no-home-recorded' };
    h.parent.insertBefore(r, h.next);
    h.cell.style.overflow = h.cellOverflow || '';
    r.style.position = h.pos || '';
    delete window.__rosterHome;
    return { ok: true };
  };

  H.forceLegacyRoster = (on) => {
    const r = document.querySelector('[data-reaction-roster]');
    if (!r) return false;
    if (on) {
      r._origPos = { left: r.style.left, right: r.style.right };
      // 往左长那一支发的是 CSS right ⇒ 不一起清掉，left/right 双锚会把盒子拉成
      // "按可用宽度撑开"，测出来的就不是"越界"而是"被压窄"了。
      r.style.right = 'auto';
      r.style.left = (window.innerWidth - 20) + 'px';
    } else {
      r.style.left = r._origPos?.left ?? '';
      r.style.right = r._origPos?.right ?? '';
      delete r._origPos;
    }
    return true;
  };

  /** 事件探针：hover 到底有没有落到那颗胶囊上（"名单没弹"有三种成因，只有监听能分开）。 */
  H.watchChip = () => {
    const chip = document.querySelector('[data-reaction-chip]');
    if (!chip) return false;
    window.__chipLog = [];
    window.__docLog = [];
    ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'mousemove', 'mouseleave'].forEach((t) => {
      chip.addEventListener(t, () => { window.__chipLog.push('chip:' + t); });
    });
    // document 捕获层：分清"事件根本没发"、"发到了别的元素"、"发到了胶囊但我的监听挂在被
    // Vue 换掉的旧节点上"这三种 —— 三者的修法完全不同，只回一句"没弹出来"就分不开。
    ['pointerover', 'mouseover', 'mousemove'].forEach((t) => {
      document.addEventListener(t, (e) => {
        const el = e.target;
        window.__docLog.push(t + ':' + el.tagName + '.' + String(el.className || '').slice(0, 18));
      }, true);
    });
    return true;
  };
  H.chipLog = () => (window.__chipLog || []).join(' | ') + ' ## doc: ' + (window.__docLog || []).slice(0, 8).join(' | ');
  /** 指针此刻真正压在谁身上（CDP 说"我移过去了"不等于浏览器认为指针在那儿）。 */
  H.pointOwner = (x, y) => {
    const el = document.elementFromPoint(Math.round(x), Math.round(y));
    if (!el) return 'null';
    const chip = document.querySelector('[data-reaction-chip]');
    return (chip && (el === chip || chip.contains(el)) ? 'chip:' : '')
      + el.tagName + '.' + String(el.className).slice(0, 28);
  };

  /** 给挂出来的那条消息换一个人：mine 是由 app.device.device_id 现推的，不种身份就永远量不到右列。 */
  H.seedDevice = async (id) => {
    const piniaUrl = performance.getEntriesByType('resource')
      .map((e) => e.name || '').find((u) => u.includes('/node_modules/.vite/deps/pinia.js?v='));
    if (!piniaUrl || !window.__pinia) return { ok: false, why: '没等到 pinia 资源或 __pinia 不在（先跑 install）' };
    const P = await import(piniaUrl);
    const A = await import('/src/stores/useAppStore.ts');
    P.setActivePinia(window.__pinia);
    const s = A.useAppStore();
    s.device = { device_id: id, nickname: '探针我', avatar: null };
    await new Promise((r) => setTimeout(r, 200));
    return { ok: true, at: s.device && s.device.device_id };
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
    // title / text 是 2026-09-30 那轮加的（编号那一格要同时量"可见的后四位"和"title/aria 里的全码"）：
    // 只加不减 ⇒ 现有消费者（读 tabindex / aria / 宽高）行为一字不变。
    return { role: e.getAttribute('role'), tabindex: e.getAttribute('tabindex'),
      aria: e.getAttribute('aria-label'), title: e.getAttribute('title'),
      text: (e.textContent || '').trim(), w: Math.round(r.width), h: Math.round(r.height) };
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
      /**
       * §10「群任务数量与群未读数量必须具有明显不同的视觉语义」—— 这条**量得出**：
       * 比两枚的计算形状与有没有图标，而不是比颜色
       * （改之前两枚是同一个 UnreadBadge、只差底色 ⇒ 这条必须红）。
       * ⚠️ 这段注释在一个**模板字符串**里 ⇒ 不能再出现反引号（会把外层模板提前关掉，
       *    本轮就这么把整个脚本改挂过一次）。
       * 未读那枚仍由 [class*="min-w-4"] 认（它是徽标家族的唯一实现，有 designGuards 钉着）；
       * 任务那枚由 data-todo-chip 认 —— 一个为「这一族只长这样」留的稳定钩子。
       */
      unreadShape: (() => {
        const e = document.querySelector('[class*="min-w-4"]');
        if (!e) return null;
        const r = e.getBoundingClientRect();
        return { text: (e.textContent || '').trim(), radius: getComputedStyle(e).borderRadius,
          h: Math.round(r.height), icon: !!e.querySelector('svg') };
      })(),
      todoShape: (() => {
        const e = document.querySelector('[data-todo-chip]');
        if (!e) return null;
        const r = e.getBoundingClientRect();
        return { text: (e.textContent || '').trim(), radius: getComputedStyle(e).borderRadius,
          h: Math.round(r.height), icon: !!e.querySelector('svg') };
      })(),
      /**
       * §10「不影响消息标题」—— 这半条也量得出，条件是量**布局结果**而不是量 class：
       * 名字那格必须仍然是「单行 + 省略号」，时间那格必须仍然完整落在行的右边界内。
       * 加了图标数字（比裸徽标宽约 15px）之后最容易坏的就是这两处：
       * 要么名字不再截断而把时间挤出行外，要么整行被撑成两行。
       * ⚠️ 名字**必须**用 data-conv-name 而不是 [class*="truncate"] 抓：组件里有两个带
       *    truncate 的 span（名字与摘要），把名字那个的 truncate 删掉时"按 class 找第一个"
       *    会静默改量摘要那格 ⇒ 假绿（lie 跑之前先想到这一层，跑出来就看不出来了）。
       */
      layout: (() => {
        const name = document.querySelector('[data-conv-name]');
        const time = document.querySelector('[class*="whitespace-nowrap"]');
        if (!name || !time || !row) return null;
        const cs = getComputedStyle(name);
        const nr = name.getBoundingClientRect();
        const tr = time.getBoundingClientRect();
        const rr = row.getBoundingClientRect();
        return {
          ellipsis: cs.textOverflow, overflowX: cs.overflow,
          nameH: Math.round(nr.height * 10) / 10, rowH: Math.round(rr.height * 10) / 10,
          timeInside: tr.right <= rr.right + 0.5 && tr.left >= rr.left - 0.5,
        };
      })(),
      wantMention: I.t('msg.mentioned'),
      wantUnread: I.t('conv.unread', { name: arg.name, n: arg.unread }),
      wantTodo: I.t('todo.openForMe', { n: arg.todos }),
      dbg: { written: written, seen: Array.from(chat.mentionedConvs).join('|'),
        todos: JSON.stringify(chat.openTodoByConv), sid: chat.$id,
        registered: (window.__pinia && window.__pinia._s && window.__pinia._s.size) || -1 },
    };
  };
  /**
   * 量「描述里那枚 @ 高亮」与「描述那个带 max-h + overflow-hidden 的裁切框」两个矩形。
   * 框用 class 片段找（Tailwind 那个 max-h-[…] 写成属性选择器最稳，不去猜具体值），
   * 行高**由 computed style 现读**——写死 19.5px 就是下一个漂移点。
   */
  H.mentionClip = async () => {
    const box = document.querySelector('[class*="max-h-"][class*="overflow-hidden"]');
    if (!box) throw new Error('找不到描述那个裁切框（class 口径变了？）');
    const tok = box.querySelector('.mention-token--self') || box.querySelector('.mention-token');
    if (!tok) throw new Error('描述里找不到 @ 高亮那一段（渲染没发生）');
    const b = box.getBoundingClientRect(), t = tok.getBoundingClientRect();
    const cs = window.getComputedStyle(box);
    return {
      boxTop: Math.round(b.top * 100) / 100, boxBottom: Math.round(b.bottom * 100) / 100,
      boxH: Math.round(b.height * 100) / 100,
      scrollH: Math.round(box.scrollHeight), clientH: Math.round(box.clientHeight),
      tokTop: Math.round(t.top * 100) / 100, tokBottom: Math.round(t.bottom * 100) / 100,
      tokH: Math.round(t.height * 100) / 100,
      lineHeight: cs.lineHeight, fontSize: cs.fontSize, selfToken: !!tok.classList.contains('mention-token--self'),
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
  check("三态齐时两枚数字各自可见（未读 3 与任务 2）",
    a.badges.includes("3") && !!a.todoShape && a.todoShape.text === "2",
    "未读=3、任务=2", `badges=${JSON.stringify(a.badges)} todo=${JSON.stringify(a.todoShape)}`);
  check("两枚的视觉语义必须不同：未读＝圆形无图标，任务＝图标＋数字（§10「不要两个数字都长得一样」）",
    !!a.unreadShape && !!a.todoShape
      && a.unreadShape.icon === false && a.todoShape.icon === true
      && a.unreadShape.radius !== a.todoShape.radius,
    "未读无图标且计算半径不同 / 任务带图标",
    `未读=${JSON.stringify(a.unreadShape)} 任务=${JSON.stringify(a.todoShape)}`);
  check("带任务那枚时名字仍是单行省略号、时间那格完整落在行内（§10「不影响消息标题」）",
    !!a.layout && a.layout.ellipsis === "ellipsis" && a.layout.overflowX === "hidden"
      && a.layout.nameH <= 22 && a.layout.timeInside === true,
    "text-overflow=ellipsis · overflow=hidden · 名字≤22px · 时间在行内",
    JSON.stringify(a.layout));
  if (SHOT_PATH) {
    // §10 的后半句是「位置由实际视觉效果决定」—— 光有数字判不出"两枚会不会互相遮"，
    // 所以这里可选地存一张真图（只在给了 --shot 时跑；不给就一字不动，条数不变）。
    const shot = await cdp.send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(SHOT_PATH, Buffer.from(shot.data, "base64"));
    console.log(`  📷 已存会话行截图 → ${SHOT_PATH}`);
  }
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
    c.label === GROUP0.name && c.badges.length === 0 && !c.text.includes(c.wantMention)
      && !c.todoShape,
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
    check("环境：找得到可用浏览器（GOSSLAN_CHROME / playwright 缓存 / 系统 Chromium 系）", false,
      "一个起得来的 Chromium 系浏览器", "三档都没有（或缓存那台装坏了且系统无同类）—— 这一格就没跑");
    console.log("\n✗ UI 运行时探针：环境缺件，判红（不是跳过）");
    process.exit(1);
  }
  console.log(`· 浏览器：${chrome}`);
  console.log(`· 来源：${BROWSER_SOURCE}`);
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

    // `--only=roster` 跑一段，`--only=reaction,roster` 跑**相邻几段** —— 用来把
    // "单独跑是绿的、整段顺序跑是红的"这类串味红二分出到底是谁污染了谁。
    const want = (name) => !ONLY || ONLY.split(",").includes(name);
    const emoji = want("emoji");
    const search = want("search");
    const task = want("task");
    const convbadge = want("convbadge");
    const caption = want("caption");
    const reaction = want("reaction");
    const roster = want("roster");
    if (emoji) await runEmoji(cdp, url);
    if (search) await runSearch(cdp, url);
    if (task) await runTaskCard(cdp, url);
    if (convbadge) await runConvBadge(cdp, url);
    if (caption) await runCaption(cdp, url);
    if (reaction) await runReaction(cdp, url);
    if (roster) await runReactionRoster(cdp, url);
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

/**
 * 那颗「添加表情回复」入口的几何（用户 2026-09-30：「按钮位置在视觉观感上应该与聊天内容最下边对齐。
 * 按钮再紧凑一些，现在有点大」）。
 * 为什么这段值得真悬停一次：入口是 `hidden group-hover/msg:flex` —— 不悬停时它**不在布局里**，
 * 任何静态读法都量不到尺寸与底边；而"跟气泡底边对齐"恰恰只能量出来。
 */
async function runReaction(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);

  const MSG = {
    msg_id: "m-react-probe-1", sender_id: "dev-me", kind: "text",
    // 刻意用一条会折成多行的长内容：单行时那一列本来就矮，"中线居中"与"贴底"只差 2~3px，
    // 那条对照就变成没判（第一版就是这么红的）。用户那句观感意见针对的正是多行消息。
    content: "这条要量那颗表情入口的底边对齐，所以写得长一点，让它折成好几行：一、二、三、四、五、六、七、八、九、十，后面再补一些字让宽度确实放不下。",
    ts: 1700000000000, conv_id: "group:g1", seq: 1,
  };
  const mounted = await cdp.eval("window.__probe.install('/src/components/MessageItem.vue', '', "
    + JSON.stringify(JSON.stringify({ message: MSG, canReact: true, isGroup: true, senderName: "测试者" })) + ")");
  check("消息行挂出来了（表情入口那颗才有得量）", !!mounted && mounted.ok === true,
    "install ok", JSON.stringify(mounted));

  const hidden = await cdp.eval("window.__probe.reactionBtn()");
  check("不悬停时那颗入口确实不在布局里（后面的读数才不是量了个隐形的东西）",
    hidden.ok === true && hidden.display === "none" && hidden.h === 0,
    "display=none 且高 0", JSON.stringify(hidden));

  const col = await cdp.eval("window.__probe.colRect()");
  check("那一列有可悬停的面积（拿不到矩形就没法移指针）", !!col && col.bottom > 0, "colRect 非空", JSON.stringify(col));
  await cdp.hover(col.x, col.y);

  const a = await cdp.eval("window.__probe.reactionBtn()");
  check("悬停后入口出现，且是紧凑的正方形小按钮（高=宽，且不大于 28px）",
    a.ok === true && a.display !== "none" && a.h > 0 && a.h === a.w && a.h <= 28,
    "h===w 且 h<=28", JSON.stringify(a));
  check("入口底边与气泡那一列的底边对齐（用户那句「与聊天内容最下边对齐」）",
    a.ok === true && a.colBottom !== null && Math.abs(a.bottom - a.colBottom) <= 1,
    "|btn.bottom - col.bottom| <= 1", "btn=" + a.bottom + " col=" + a.colBottom);
  check("触屏热区没被这次收窄弄丢（tap-safe 仍在：h-6 靠它扩到 40px）",
    a.ok === true && a.tapSafe === true, "class 里有 tap-safe", JSON.stringify(a));

  // tooltip / 可访问名：换成自己那句"添加表情回复"，不再是借输入框那颗的「表情」。
  const names = await cdp.eval(`(async () => {
    const i = await import('/src/i18n');
    return { own: i.t('msg.reactionAddEntry'), composer: i.t('chat.composer.emoji') };
  })()`);
  check("它的 title 与 aria-label 都是「添加表情回复」那句（不是借输入框那颗的文案）",
    !!a.title && a.title === names.own && a.aria === names.own && names.own !== names.composer,
    names.own, "title=" + a.title + " aria=" + a.aria + " 而输入框那颗=" + names.composer);

  // 反面对照：按回旧的"中线居中"写法 ⇒ 底边对齐差必须变大（不然上面那句就是恒真）。
  await cdp.eval("window.__probe.forceLegacyReaction(true)");
  const b = await cdp.eval("window.__probe.reactionBtn()");
  check("对照：改回旧的中线居中 ⇒ 多行气泡上底边就飘起来（证明那条对齐判据会咬）",
    // 阈值不是拍一个数：正向那格用的容差是 ±1px，这里要求**至少飘出容差的 4 倍**（实测 13px，
    // 那一列多高就飘多少 —— 写死 24 是我第一次的猜测，它在两行气泡上根本不成立，红得没道理）。
    b.ok === true && Math.abs(b.bottom - b.colBottom) > 4,
    "|差| > 4（正向那格容差 ±1 的 4 倍）",
    "btn=" + b.bottom + " col=" + b.colBottom + " 差=" + Math.abs(b.bottom - b.colBottom));
  await cdp.eval("window.__probe.forceLegacyReaction(false)");
  const c = await cdp.eval("window.__probe.reactionBtn()");
  check("对照可逆：恢复后底边又对齐（不是把页面改坏了一次）",
    c.ok === true && Math.abs(c.bottom - c.colBottom) <= 1, "|差| <= 1",
    "btn=" + c.bottom + " col=" + c.colBottom);
}

/**
 * 表情回应条的**边缘对齐 + 名单浮层越界**那一段（用户 2026-10-07 报的两条：
 * ①「电脑端 右边聊天 hover 表情 人员列表 展开遮挡」；②「表情和气泡对齐：左右两条表情回复边缘
 * 都与上面气泡对齐，并且间距紧凑点」）。
 *
 * 为什么必须走真浏览器量：这两条判的全是**布局几何**，而静态 class 判据只能证明"某个数被写进去了"，
 * 证不了"那个数等于气泡的真实边缘"。这次的根因正是补偿值算错 —— 册里注释写的是"头像 40px + 行间距 8px"，
 * 而头像是 `h-9 w-9` = **36px**，还漏了消息行自己的 `px-4` = 16px ⇒ 两侧各差 8px。
 * 那 8px 只有浏览器知道，注释和 class 都不会告诉我。
 */
/**
 * 外层只干一件事：**把"页面有焦点/可见"这件事圈在本段内**。
 *
 * headless 起的探针页实测是 visibilityState="hidden" + hasFocus()=false，
 * 而 mouseenter/mouseleave 是浏览器按它内部的 hover 追踪算出来的
 * ⇒ 真鼠标投递间歇性丢（曾表现为"名单一次都没弹出、而单独跑 --only=roster 全绿"）。
 * 开焦点模拟后，左右两列都在**第 0 次尝试**走通真鼠标。
 *
 * ⚠️ 刻意不外溢：emoji 那一段的 Enter 断言正是按"页面没焦点 ⇒ Enter 不激活被聚焦的格子"
 * 写的（那条"Enter 不带 text ⇒ 零次激活"的对照就是它的前提）。全局开焦点模拟会让 Enter
 * 真的激活格子，实测 select 事件刷到 8MB 日志。所以进来开、出去关，并用 finally 兜住。
 */
async function runReactionRoster(cdp, url) {
  await cdp.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  await cdp.send("Page.bringToFront").catch(() => {});
  try {
    await rosterChecks(cdp, url);
  } finally {
    await cdp.send("Emulation.setFocusEmulationEnabled", { enabled: false }).catch(() => {});
  }
}

async function rosterChecks(cdp, url) {
  /** 取证帧：`--shot=/tmp/x.png` 时两侧各存一张（左列 / 右列名单展开态）。 */
  const shot = async (name) => {
    if (!SHOT_PATH) return;
    const p = SHOT_PATH.replace(/\.png$/, `-${name}.png`);
    const s = await cdp.send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(p, Buffer.from(s.data, "base64"));
    console.log("  📷 回应条取证帧 → " + p);
  };
  const LONG = "一个会撑破整块面板的超长昵称-ABCDEFGHIJKLMNOP-QRSTUVWXYZ";
  const CHIPS = [{
    emoji: "[赞]", count: 4, mine: true,
    actors: ["peer-a", "peer-b", "peer-c", "me-1"], latest: "me-1",
  }];
  const mount = async (sender, mineSide, chips) => {
    await cdp.send("Page.navigate", { url });
    await sleep(3_000);
    await cdp.eval(PAGE_FIXTURE);
    const props = {
      message: {
        msg_id: "m-roster-probe-1", sender_id: sender, kind: "text",
        content: "这条要量回应条与气泡的边缘关系，所以内容写得长一些，让它折成两行以上。",
        ts: 1700000000000, conv_id: "group:g1", seq: 1,
      },
      canReact: true, isGroup: true, senderName: "测试者", reactions: chips ?? CHIPS,
    };
    const r = await cdp.eval("window.__probe.install('/src/components/MessageItem.vue', '', "
      + JSON.stringify(JSON.stringify(props)) + ")");
    if (!r || r.ok !== true) return false;
    // mine 是由 app.device.device_id 现推的：不种身份就永远量不到右列那一支。
    if (mineSide) await cdp.eval("window.__probe.seedDevice('me-1')");
    await sleep(400);
    return true;
  };
  /**
   * 把指针真的移进第 i 颗胶囊，重试到名单出现。走的是**真鼠标**：
   * 先退到一个明确不在胶囊上的点再回来（同一点重复 mouseMoved 会被 Chrome 判成
   * "位置没变"丢掉），送回去之后等一帧再判（摆位在 mouseenter 那一拍算完，
   * 但 DOM 要等 Vue 冲刷才出现）。
   *
   * ⚠️ 这里刻意**不再**留"派发合成 mouseenter"那条兜底。它是探针页没焦点时的救急，
   * 但留着就等于"hover 真不工作也能判绿"。焦点模拟（见 runReactionRoster 外层）
   * 已经把真鼠标通路修稳，所以兜底删掉 —— 现在 hover 坏了一定是红的。
   */
  async function hoverInto(i) {
    const pt = await cdp.eval("window.__probe.reactionChipPoint(" + i + ")");
    if (!pt) return { pt: null, tries: -1, via: "no-chip", log: "" };
    await cdp.eval("window.__probe.watchChip()");
    for (let k = 0; k < 5; k += 1) {
      await cdp.hover(Math.max(2, pt.x - 60), pt.y);
      await sleep(50);
      await cdp.hover(pt.x, pt.y);
      await sleep(150);
      if (await cdp.eval("!!document.querySelector('[data-reaction-roster]')")) {
        return { pt, tries: k, via: "mouse", log: await cdp.eval("window.__probe.chipLog()") };
      }
    }
    return { pt, tries: -1, via: "none", log: await cdp.eval("window.__probe.chipLog()") };
  }

  // ================= 左列（别人发的）：看左边缘 =================
  check("左列：挂出一条带回应的消息", await mount("peer-a", false), "install 成功");
  const gl = await cdp.eval("window.__probe.reactionGeom()");
  check("左列：胶囊左边缘与气泡那一列左边缘对齐（|差| <= 1px）",
    gl.ok === true && Math.abs(gl.deltaLeft) <= 1,
    "|chip.left - col.left| <= 1",
    "差 " + gl.deltaLeft + "px；chip.left=" + gl.chipLeft + " col.left=" + gl.colLeft
      + "，条 padding-left=" + gl.padLeft);
  check("左列：与气泡的竖向间距收到紧凑档（0 <= 差 <= 6px，改之前是 10px）",
    gl.ok === true && gl.gapToBubble >= 0 && gl.gapToBubble <= 6,
    "0 <= 间距 <= 6",
    "间距 " + gl.gapToBubble + "px = 条 mt " + gl.barMarginTop + " + 消息行 pb " + gl.rowPadBottom);

  const hl = await hoverInto(0);
  check("左列：拿得到胶囊的落点（hover 的坐标来源）", !!hl.pt, "chipPoint 非空", JSON.stringify(hl.pt));
  const rl = await cdp.eval("window.__probe.reactionRoster()");
  check("左列：hover 胶囊真的展开名单（" + hl.via + " 通路，第 " + hl.tries + " 次命中；事件 " + hl.log.slice(0, 170) + "）",
    rl.ok === true, "名单在布局里", JSON.stringify(rl).slice(0, 900));
  check("左列：名单不越出滚动容器（左右都不许）",
    rl.ok === true && rl.overLeft <= 0 && rl.overRight <= 0,
    "overLeft <= 0 且 overRight <= 0",
    "左溢 " + rl.overLeft + " / 右溢 " + rl.overRight + "（容器 " + rl.clipLeft + "…" + rl.clipRight + "）");
  // ★ 名单的**自带契约**（组件头注释 + chipTitle 都是这一条）：列到 ROSTER_VISIBLE 人为止，
  //   其余折成「+N」。夹具这里给的是 4 个 actor ⇒ 必须恰好 3 行 + 一条 +1。
  //   模板曾经 v-for 全部 actors **又**渲染 +N ⇒ 4 行名字 + "还有 1 人"，自己跟自己矛盾
  //   （用户 2026-10-07：「表情很多的时候 查看这个列表也是有问题的」）。
  const rowsL = await cdp.eval("window.__probe.reactionRows()");
  const plusL = await cdp.eval("(function(){var r=document.querySelector('[data-reaction-roster]');"
    + "if(!r)return null;var t=r.lastElementChild;"
    + "return t && !t.hasAttribute('data-roster-row') ? t.textContent.trim() : null;})()");
  const fl_l = await cdp.eval("window.__probe.floatLayer('[data-reaction-roster]')");
  check("左列：名单 escape 出裁切容器（挂在 body 上 + 没有任何祖先裁得到它）",
    fl_l.ok === true && fl_l.parentIsBody === true && fl_l.escapes === true,
    "parentIsBody 且 escapes",
    "挂body=" + fl_l.parentIsBody + " position=" + fl_l.position
      + " 切得到它的祖先=" + JSON.stringify(fl_l.clipped)
      + " 变成包含块的祖先=" + JSON.stringify(fl_l.breakers)
      + "（body overflow=" + fl_l.bodyOverflow + "：fixed 不被它裁 ⇒ 只有祖先变成包含块才裁得到）");
  check("左列：名单整个在视口内（escape 出去 ≠ 看得见，坐标算错会飞出屏幕）",
    fl_l.ok === true && fl_l.inViewport === true,
    "四边都在视口内", "rect=" + JSON.stringify(fl_l.rect) + " 视口=" + fl_l.vw + "x" + fl_l.vh);
  check("左列：4 人名单恰好折成 3 行 + 一条 +N（不许把所有人列出来又说还有 N 人）",
    rowsL.ok === true && rowsL.rows.length === 3 && /^\+|\d|还有/.test(String(plusL)),
    "行数 === 3 且末条是 +N",
    "现读 " + (rowsL.ok === true ? rowsL.rows.length : "?") + " 行，末条 = " + JSON.stringify(plusL));
  await shot("left");

  // ================= 右列（我自己发的）：看右边缘 + 那条越界 =================
  check("右列：挂出一条自己发的带回应消息", await mount("me-1", true), "install + seed 成功");
  const gm = await cdp.eval("window.__probe.reactionGeom()");
  check("右列：胶囊右边缘与气泡那一列右边缘对齐（|差| <= 1px）",
    gm.ok === true && Math.abs(gm.deltaRight) <= 1,
    "|chip.right - col.right| <= 1",
    "差 " + gm.deltaRight + "px；chip.right=" + gm.chipRight + " col.right=" + gm.colRight
      + "，条 padding-right=" + gm.padRight);
  check("右列：与气泡的竖向间距同样是紧凑档（0 <= 差 <= 6px）",
    gm.ok === true && gm.gapToBubble >= 0 && gm.gapToBubble <= 6,
    "0 <= 间距 <= 6", "间距 " + gm.gapToBubble + "px = 条 mt " + gm.barMarginTop + " + 消息行 pb " + gm.rowPadBottom);

  const hm = await hoverInto(0);
  check("右列：拿得到胶囊的落点（hover 的坐标来源）", !!hm.pt, "chipPoint 非空", JSON.stringify(hm.pt));
  const rm = await cdp.eval("window.__probe.reactionRoster()");
  check("右列：hover 展开名单（用户报的那一支；" + hm.via + " 通路，第 " + hm.tries + " 次命中；事件 " + hm.log.slice(0, 170) + "）",
    rm.ok === true, "名单在布局里", JSON.stringify(rm).slice(0, 900));
  check("右列：名单右缘不越出滚动容器（用户那句「展开遮挡」就是这一格）",
    rm.ok === true && rm.overRight <= 0 && rm.overLeft <= 0,
    "右溢 <= 0 且 左溢 <= 0",
    "右溢 " + rm.overRight + "px / 左溢 " + rm.overLeft + "px，名单宽 " + rm.width
      + "px，容器 " + rm.clipLeft + "…" + rm.clipRight + "，视口 " + rm.vw);
  const fl_r = await cdp.eval("window.__probe.floatLayer('[data-reaction-roster]')");
  check("右列：名单同样 escape 出裁切容器（这一支用 CSS right 钉，链路上必须一样干净）",
    fl_r.ok === true && fl_r.parentIsBody === true && fl_r.escapes === true,
    "parentIsBody 且 escapes",
    "挂body=" + fl_r.parentIsBody + " position=" + fl_r.position
      + " 切得到它的祖先=" + JSON.stringify(fl_r.clipped)
      + " 变成包含块的祖先=" + JSON.stringify(fl_r.breakers) + " rect=" + JSON.stringify(fl_r.rect));
  check("右列：名单整个在视口内",
    fl_r.ok === true && fl_r.inViewport === true,
    "四边都在视口内", "rect=" + JSON.stringify(fl_r.rect) + " 视口=" + fl_r.vw + "x" + fl_r.vh);
  check("右列：名单露出来的那部分确实是它自己（没被别的元素盖住）",
    rm.ok === true && rm.hitSelf === true,
    "elementFromPoint 命中名单自身", "命中 " + rm.hitTag);
  await shot("right");

  // ================= 两把反面对照：证明上面那些数不是恒真 =================
  await cdp.eval("window.__probe.forceAlignSkew(true)");
  const skewed = await cdp.eval("window.__probe.reactionGeom()");
  check("对照：把回应条朝外侧挪 8px（= 修之前那个补偿差的量）⇒ 对齐判据必须不再成立",
    skewed.ok === true && Math.abs(skewed.deltaRight) >= 4,
    "|差| >= 4", "差 " + skewed.deltaRight + "px");
  await cdp.eval("window.__probe.forceAlignSkew(false)");
  const back = await cdp.eval("window.__probe.reactionGeom()");
  check("对照可逆：撤掉平移后右缘又对齐",
    back.ok === true && Math.abs(back.deltaRight) <= 1, "|差| <= 1", "差 " + back.deltaRight + "px");

  await cdp.eval("window.__probe.forceLegacyRoster(true)");
  const legacy = await cdp.eval("window.__probe.reactionRoster()");
  check("对照：把名单 fixed left 推到视口右缘外 ⇒ 右溢必须重新出现（证明判据不空转）",
    legacy.ok === true && legacy.overRight > 0,
    "overRight > 0", "右溢 " + legacy.overRight + "px");
  await cdp.eval("window.__probe.forceLegacyRoster(false)");
  const fixed = await cdp.eval("window.__probe.reactionRoster()");
  check("对照可逆：换回新锚点后右溢归零",
    fixed.ok === true && fixed.overRight <= 0, "overRight <= 0", "右溢 " + fixed.overRight + "px");
  const rep = await cdp.eval("window.__probe.reparentRoster(true)");
  const mut = await cdp.eval("window.__probe.floatLayer('[data-reaction-roster]')");
  check("对照：把名单搬回「absolute 挂在裁切容器里」（= 修之前的形状）⇒ escape 判据必须翻假",
    rep.ok === true && mut.ok === true && mut.escapes === false,
    "escapes === false",
    "搬进 " + JSON.stringify(rep.into) + " position=" + mut.position
      + " 切得到它的祖先=" + JSON.stringify(mut.clipped) + " 链=" + JSON.stringify(mut.chain));
  await cdp.eval("window.__probe.reparentRoster(false)");
  const undone = await cdp.eval("window.__probe.floatLayer('[data-reaction-roster]')");
  check("对照可逆：搬回 body、position 又回到 fixed ⇒ escapes 恢复（对照自己不留残留）",
    undone.ok === true && undone.escapes === true && undone.parentIsBody === true
      && undone.position === "fixed",
    "escapes 且 parentIsBody 且 position=fixed",
    "escapes=" + undone.escapes + " 挂body=" + undone.parentIsBody + " position=" + undone.position
      + " 切得到它的祖先=" + JSON.stringify(back.clipped));
  // ============ 一排十几颗（用户第二轮）：方向按**这颗的位置**翻，长名字要真省略 ============
  // 用户那句："不能特别固定的就往左偏或者往右偏，而是看这个图标的位置靠近哪边就往反方向偏"
  // ⇒ 判据必须同时钉"贴左那颗"和"贴右那颗"，而且**同一条消息上**（消息朝向相同、该偏的方向相反）。
  const EMO = ["[赞]", "[捂脸]", "[笑哭]", "[火]", "[心]", "[666]", "[狗头]", "[打脸]", "[皱眉]", "[耶]", "[吃瓜]", "[偷笑]"];
  const MANY = EMO.map((e, i) => ({
    emoji: e, count: i + 1, mine: i === 0,
    actors: [i === 0 ? LONG : "同事甲", "同事乙"], latest: "同事乙",
  }));
  check("多颗：挂出一条带 12 颗回应的消息", await mount("peer-a", false, MANY), "install 成功");
  const pts = await cdp.eval("window.__probe.allChipPoints()");
  check("多颗：拿得到每一颗胶囊的落点（>= 8 颗才谈得上「贴哪一边」）",
    Array.isArray(pts) && pts.length >= 8, "至少 8 颗", "现读 " + (pts ? pts.length : 0) + " 颗");
  const leftmost = pts.reduce((x, y) => (y.left < x.left ? y : x));
  const rightmost = pts.reduce((x, y) => (y.right > x.right ? y : x));

  await hoverInto(leftmost.i);
  const r1 = await cdp.eval("window.__probe.reactionRoster()");
  check("多颗：贴左那颗的名单**往右长**（名单左缘就是胶囊左缘，左缘不越界）",
    r1.ok === true && Math.abs(r1.left - leftmost.left) <= 2 && r1.overLeft <= 0,
    "|名单.left - 胶囊.left| <= 2 且 左溢 <= 0",
    "名单 left=" + r1.left + " 胶囊 left=" + leftmost.left + " 左溢 " + r1.overLeft);
  const rows1 = await cdp.eval("window.__probe.reactionRows()");
  // 判"长名字那一行"必须真被省略号截断（短行不该截 —— 第一版我写成"每行都要 clipped"，
  // 那是判据自己太严：同事乙 那行本来就短，红得没道理）
  const longRow = rows1.ok === true ? rows1.rows.find((x) => x.text.startsWith("一个会撑破")) : null;
  check("多颗：超长昵称那一行被省略号真截断（clipped + ellipsis），且 title 里留了全名",
    !!longRow && longRow.clipped === true && longRow.ellipsis === true && longRow.hasTitle === true,
    "长名字行 clipped 且 ellipsis 且带 title",
    JSON.stringify(longRow));
  check("多颗：面板不被长名字撑破（宽 <= 16rem = 256px，每行都带 title 兜住全名）",
    rows1.ok === true && rows1.panelW <= 256 && rows1.rows.every((x) => x.hasTitle === true),
    "面板 <= 256px 且每行有 title",
    "面板宽 " + rows1.panelW + "px，行数 " + (rows1.rows ? rows1.rows.length : 0));
  // 长名字那一颗要**顶到上限**：撑开不是"越短越好"，超过上限才交给行内 truncate。
  check("多颗：长名字那一颗顶到最大档（面板宽 >= 250，说明 max-width 真的是那条上限）",
    rows1.ok === true && rows1.panelW >= 250,
    "长名字面板 >= 250", "面板宽 " + (rows1.ok === true ? rows1.panelW : "?") + "px");

  await hoverInto(rightmost.i);
  const r2 = await cdp.eval("window.__probe.reactionRoster()");
  check("多颗：贴右那颗的名单**往左长**（名单右缘就是胶囊右缘，右缘不越界）",
    r2.ok === true && Math.abs(r2.right - rightmost.right) <= 2 && r2.overRight <= 0,
    "|名单.right - 胶囊.right| <= 2 且 右溢 <= 0",
    "名单 right=" + r2.right + " 胶囊 right=" + rightmost.right + " 右溢 " + r2.overRight);
  // 宽度按名字长短**伸缩**（用户 2026-10-07：「框框是固定长度，名字不够长后面有空白」）。
  // 这一条是**两颗互证**的：同一块面板、短名字那颗必须明显窄于长名字那颗 ——
  // 只要宽度是写死的，两个读数就相等，这条当场红。所以它不需要额外的反面对照。
  check("多颗：短名字那一颗按内容撑开（比长名字那颗窄、且没顶到上限 ⇒ 不写死宽度）",
    r2.ok === true && r2.width < r1.width && r2.width < 200,
    "短 " + r2.width + " < 长 " + r1.width + " 且 短 < 200",
    "短名字面板 " + r2.width + "px / 长名字面板 " + r1.width + "px");
  await shot("many");

}

/**
 * 标题栏 × 内容浮层那一段（用户 2026-09-30：「图片预览独立窗口会遮住头部标题栏关闭最小化那块，
 * 应该只在下面内容区。所有独立窗口标题栏都不会被内容栏遮住，标题栏优先度最高」）。
 *
 * 为什么值得单开一段：这条修的是**绘制顺序 + 命中测试**，静态判据（层级现算）只能证明
 * "数字比大小成立"，证不了"那横条上的 ✕/− 与拖拽区实际还点得着"。而 `elementFromPoint`
 * 走的正是浏览器真实的命中顺序 —— 它命中标题栏，用户的鼠标才点得到。
 */
async function runCaption(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  // 先借 install 把"页面自己在用的那份 vue/pinia"接进 __probe（composeStack 依赖它们）
  await cdp.eval("window.__probe.install('/src/components/TitleBar.vue', '', '')");

  const inset = "var(--gosslan-title-h)";
  const comp = await cdp.eval('window.__probe.composeStack(' + JSON.stringify(inset) + ')');
  check("辅助窗口那一路：TitleBar 与图片遮罩真的同时挂在页面上", !!comp && comp.ok === true,
    "composeStack ok", JSON.stringify(comp));

  const a = await cdp.eval("window.__probe.stackRead()");
  check("遮罩把标题栏那一条让开了（overlay 顶边不低于 caption 底边）",
    a.ok === true && a.ovTop >= a.capBottom,
    "ovTop >= capBottom(" + (a.capBottom ?? "?") + ")", JSON.stringify(a));
  check("标题栏那一条仍高于遮罩（现算两个计算样式，不抄数字）",
    a.ok === true && Number(a.capZ) > Number(a.ovZ),
    "capZ > ovZ", "capZ=" + a.capZ + " ovZ=" + a.ovZ);
  check("caption 中点实际命中的是标题栏 ⇒ ✕/− 与整窗唯一拖拽区点得着",
    a.ok === true && a.hitInCaption === true, "elementFromPoint 落在 [data-caption] 内",
    "命中 " + a.hitTag);

  // 主窗口那一路：**没有** top-inset（遮罩铺到 ovTop=0，几何上真的压在标题栏上），
  // 于是"点得到 ✕/−"完全靠层级 —— 也正是在这一路，反面对照才有意义。
  // （辅助窗口那一路让位是几何给的，把层级按回去照样命中 ⇒ 在那一路做对照是恒真，
  //   第一版就栽在这里：那一条红得很有道理，因为它测的是不存在的关系。）
  const comp2 = await cdp.eval('window.__probe.composeStack("")');
  const d = await cdp.eval("window.__probe.stackRead()");
  check("主窗口那一路：遮罩确实铺到顶（ovTop=0）—— 所以下一条不是靠让位蒙对",
    !!comp2 && comp2.ok === true && d.ok === true && d.ovTop <= 1, "ovTop<=1", JSON.stringify(d));
  check("即便如此，caption 中点仍命中标题栏（层级独立于让位，两处宿主都成立）",
    d.ok === true && d.hitInCaption === true, "hitInCaption=true", "命中 " + d.hitTag);

  // 反面对照：按回"修之前"的形状（非定位、无层级）⇒ 同一个点必须被遮罩吃掉。
  // 少了这一条，上面那句命中判据就是恒真（遮罩没压上来时它也成立）。
  await cdp.eval("window.__probe.setCaptionStack('off')");
  const b = await cdp.eval("window.__probe.stackRead()");
  check("对照：把标题栏层级按回修之前 ⇒ 同一个点被遮罩吃掉（证明那条命中判据会咬）",
    b.ok === true && b.hitInCaption === false, "hitInCaption=false", JSON.stringify(b));
  await cdp.eval("window.__probe.setCaptionStack('on')");
  const c = await cdp.eval("window.__probe.stackRead()");
  check("对照可逆：恢复后同一个点又回到标题栏（不是「测一次就把页面弄坏了」）",
    c.ok === true && c.hitInCaption === true, "hitInCaption=true", JSON.stringify(c));
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
  // #116 用的第二份：描述长到必然超过那个框的三行 ⇒ 能证明"这层确实在裁"
  const MSG_LONG = JSON.stringify({
    msg_id: "m-probe-clip", sender_id: "dev-me", kind: "todo",
    content: JSON.stringify({ ...payload, description: "@小布 " + "补齐到四行之多的填充文字。".repeat(24) }),
    ts: 1700000000000,
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

  // —— #116：描述里的 @ 高亮会不会被那层 `max-h + overflow-hidden` 裁一半 ——
  // ⚠️ 只钉「高亮完整可见」会变成恒真（这层根本没裁的时候它也成立）⇒ 必须配一条
  //    「裁切机制确实活着」的对照（③），两者同时成立才算判到了风险本身。
  await mount({
    message: JSON.parse(MSG), mentionNames: ["小布", "我"],
    selfMention: { name: "小布", label: labels.selfTag },
  });
  const clip = await cdp.eval("window.__probe.mentionClip()");
  const lhPx = Number(String(clip.lineHeight).replace(/[^0-9.]/g, "")) || 0;
  check("描述里那枚 @ 高亮完整落在裁切框内（上缘与下缘都不被切）",
    clip.tokTop >= clip.boxTop - 1 && clip.tokBottom <= clip.boxBottom + 1,
    "token 上下缘都在框内", `tok=${clip.tokTop}..${clip.tokBottom} box=${clip.boxTop}..${clip.boxBottom}`);
  check("高亮没把行盒撑破（token 高 ≤ computed line-height × 1.05；行高由样式现读不写死）",
    lhPx > 0 && clip.tokH <= lhPx * 1.05, `≤ ${(lhPx * 1.05).toFixed(2)}（行高 ${clip.lineHeight}）`,
    `${clip.tokH} / fontSize=${clip.fontSize}`);
  const longDesc = await (async () => {
    await mount({
      message: JSON.parse(MSG_LONG), mentionNames: ["小布", "我"],
      selfMention: { name: "小布", label: labels.selfTag },
    });
    return cdp.eval("window.__probe.mentionClip()");
  })();
  check("对照：把描述写到 4 行 ⇒ 那个框确实在裁（scrollHeight > clientHeight）—— 证明上面两条不是恒真",
    longDesc.scrollH > longDesc.clientH, "scrollHeight > clientHeight",
    `${longDesc.scrollH} > ${longDesc.clientH}? 高亮=${longDesc.tokH}px 行高=${longDesc.lineHeight}`);

  // —— 实时状态表那一环（#23：载荷是创建时快照，卡片必须查父层那张表）——
  // 同一张表现在还管着编号：类型字母由**当前**类型现推（用户 2026-09-30 规则 3），
  // 而"超过 4 位只显后四位 + 全码要能查到"（规则 5/6）只有真渲染出来才量得动。
  await mount({
    message: JSON.parse(MSG),
    mentionNames: ["小布"],
    selfMention: null,
    liveTodoPairs: [[TODO_ID, "done", "bug", 12345]],
  });
  const t2 = await text();
  check("传了实时表说这条已完成 ⇒ 胶囊换成已完成（查不到才退回快照，查到就不许再显示旧值）",
    t2.includes(labels.done) && !t2.includes(labels.todo), labels.done, t2.slice(0, 80));
  const codeCell = await cdp.eval("window.__probe.el('.font-mono')");
  const cText = codeCell ? codeCell.text : "";
  const cTitle = codeCell ? String(codeCell.title || "") : "";
  const cAria = codeCell ? String(codeCell.aria || "") : "";
  check("编号那格的字母来自**实时表里的当前类型**（夹具的创建快照既没类型也没号 ⇒ 不查表一个字都出不来）",
    cText === "B2345", "可见文本 B2345", JSON.stringify(codeCell));
  check("五位数字 ⇒ 可见只有后四位，而 title 与 aria 都带完整编号 B12345（短码不许被当成全码）",
    cTitle.includes("B12345") && cAria.includes("B12345") && !cText.includes("12345"),
    "title/aria 含 B12345、可见文本不含", JSON.stringify(codeCell));

  // 反面对照：不传实时表 ⇒ 卡片只剩那份"既没类型也没号"的创建快照，那一格必须整个不出现。
  // 少了这一条，上面两句就成了探针自己造出来的号。
  await mount({ message: JSON.parse(MSG), mentionNames: ["小布"], selfMention: null });
  const codeOff = await cdp.eval("window.__probe.el('.font-mono')");
  check("对照：不传实时表 ⇒ 那格没有任何补出来的号（宁可空白，也不按顺序数一个会随集合漂移的号）",
    codeOff === null, "找不到 .font-mono", JSON.stringify(codeOff));

  // —— 键盘可达 + 按 Enter 真的开了那份统一预览 ——
  await mount({
    message: JSON.parse(MSG),
    mentionNames: ["小布"],
    selfMention: null,
    liveTodoPairs: [[TODO_ID, "done", "bug", 12345]],
  });
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
