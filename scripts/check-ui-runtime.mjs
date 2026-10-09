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
  /**
   * 真的按下一次左键（mouseMoved → mousePressed → mouseReleased）。
   * 和 hover 同理：DOM 上 `el.click()` 不经过命中测试，也就照不出"这颗按钮其实被谁盖住了"。
   */
  async click(x, y) {
    const base = { x, y, button: "left", clickCount: 1 };
    this.ws.send(JSON.stringify({
      id: ++this.seq, method: "Input.dispatchMouseEvent", params: { type: "mouseMoved", ...base },
    }));
    await sleep(40);
    this.ws.send(JSON.stringify({
      id: ++this.seq, method: "Input.dispatchMouseEvent", params: { type: "mousePressed", ...base },
    }));
    await sleep(60);
    this.ws.send(JSON.stringify({
      id: ++this.seq, method: "Input.dispatchMouseEvent", params: { type: "mouseReleased", ...base },
    }));
    await sleep(160);
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
  /** 网格里**非按钮**的那些子元素（= 两段的小标题），按 DOM 顺序。 */
  H.gridLabels = () => {
    const first = H.grid()[0];
    const grid = first ? first.parentElement : null;
    if (!grid) return [];
    return Array.from(grid.children)
      .filter((c) => c.tagName !== 'BUTTON')
      .map((c) => ({
        tag: c.tagName,
        text: String(c.textContent || '').trim(),
        y: Math.round(c.getBoundingClientRect().top),
      }));
  };
  /** 网格第一个子元素的标签名（判"常用那一节消失后剩的是谁"）。 */
  H.gridFirstChildTag = () => {
    const first = H.grid()[0];
    const grid = first ? first.parentElement : null;
    const c = grid ? grid.firstElementChild : null;
    return c ? c.tagName : 'none';
  };
  H.focusTrigger = () => { document.getElementById('trigger').focus(); };
  /** 面板里每一格：alt（表情名）、常用标记、**计算后的背景色**、落点。按 DOM 顺序，不按组件内部下标。 */
  H.emojiCells = () => H.grid().map((b) => {
    const img = b.querySelector('img');
    const r = b.getBoundingClientRect();
    return {
      alt: img ? img.alt : '',
      freq: b.getAttribute('data-emoji-freq') === '1',
      bg: getComputedStyle(b).backgroundColor,
      x: Math.round(r.left),
      y: Math.round(r.top),
    };
  });
  /** 网格的**第一个子元素**是什么 —— 「常用」小标题必须是它，而且必须是**非按钮**（按钮会挤进步长）。 */
  H.gridHead = () => {
    const first = H.grid()[0];
    const grid = first ? first.parentElement : null;
    if (!grid) return { tag: 'none', text: '' };
    const c = grid.firstElementChild;
    return { tag: c ? c.tagName : 'none', text: c ? String(c.textContent || '').trim() : '' };
  };
  /** 命中列表的当前高亮：组件用 data-hit + aria-current 两个钩子表达，读的就是这两个钩子。 */
  H.hits = () => {
    const list = Array.from(document.querySelectorAll('[data-hit]'));
    return {
      n: list.length,
      at: list.findIndex((el) => el.getAttribute('aria-current') === 'true'),
      ids: list.map((el) => el.getAttribute('data-msg') || el.getAttribute('data-hit')),
    };
  };
  /** 「发送人」那枚筛选入口的中心坐标（给真点击用）。 */
  H.filterChip = () => {
    const b = document.querySelector("button[aria-haspopup='menu']");
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
  };
  /** 菜单自己**计算后**的背景 —— 透明就是用户那句「筛选下拉都是透明的」。 */
  H.menuStyle = () => {
    const m = document.querySelector('.gosslan-menu');
    if (!m) return { present: false, bg: '', blur: '' };
    const cs = getComputedStyle(m);
    return {
      present: true,
      bg: cs.backgroundColor,
      blur: cs.backdropFilter || cs.webkitBackdropFilter || '',
    };
  };
  /** 方向键与回车都挂在搜索框上 ⇒ 点过菜单之后必须把焦点还回去，不然下面的键盘判据红在夹具。 */
  H.focusInput = () => {
    const i = document.querySelector('input');
    if (!i) return false;
    i.focus();
    return document.activeElement === i;
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
    const overlay = want("overlay");
    if (emoji) await runEmoji(cdp, url);
    if (search) await runSearch(cdp, url);
    if (task) await runTaskCard(cdp, url);
    if (convbadge) await runConvBadge(cdp, url);
    if (caption) await runCaption(cdp, url);
    if (reaction) await runReaction(cdp, url);
    if (roster) await runReactionRoster(cdp, url);
    if (overlay) await runOverlay(cdp, url);
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
  const mutVp = await cdp.eval("window.__probe.floatLayer('[data-reaction-roster]')");
  check("对照：同一支推坐标出视口的变异下，inViewport 也必须翻假（证明那条视口判据会咬）",
    mutVp.ok === true && mutVp.inViewport === false,
    "inViewport === false",
    "rect=" + JSON.stringify(mutVp.rect) + " 视口=" + mutVp.vw + "x" + mutVp.vh);
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

  // ============ 隔壁那支表情选择器：同一个坑的第二次现场 ============
  // 这块面板宽 min(360px, 视口-32)，是这一类里最容易被窗口边缘切到的一支。
  // 4.33.25 那轮它只有静态那一半守着（没有"点开"的通路），这里补上：真点一下入口按钮，
  // 再对**那层壳**判 escape、对**里面的面板**判在不在视口内。
  //   ⚠️ 两个对象不能混：壳是零高度的（它的孩子全 absolute），量壳的矩形证明不了"看得见"；
  //      而里面那块面板是 absolute —— 对它只许读 inViewport，escapes 那一栏在这形状下本来就是假的
  //      （absolute 的祖先里当然有 overflow，它的包含块是这层 fixed 壳，不该拿去判裁切）。
  const eb = await cdp.eval("(function(){var b=document.querySelector('[data-reaction-entry]');"
    + "if(!b)return null;var r=b.getBoundingClientRect();"
    + "return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2),"
    + " display: getComputedStyle(b).display};})()");
  check("选择器：拿得到表情入口那颗按钮的落点（点开的坐标来源）", !!eb, "entry 非空", JSON.stringify(eb));
  let pickerOpen = false;
  for (let k = 0; k < 5 && !pickerOpen; k += 1) {
    await cdp.hover(Math.max(2, eb.x - 80), eb.y);
    await cdp.click(eb.x, eb.y);
    pickerOpen = !!(await cdp.eval("!!document.querySelector('[data-reaction-picker]')"));
  }
  const fp = await cdp.eval("window.__probe.floatLayer('[data-reaction-picker]')");
  const pp = await cdp.eval("window.__probe.floatLayer('[data-reaction-picker] [role=dialog]')");
  check("选择器：点开之后那层壳 escape 出裁切容器（挂在 body 上 + 没有祖先裁得到它）",
    pickerOpen === true && fp.ok === true && fp.parentIsBody === true && fp.escapes === true,
    "开得出、挂 body、且 escapes",
    "开=" + pickerOpen + " 挂body=" + fp.parentIsBody + " position=" + fp.position
      + " 切得到它的祖先=" + JSON.stringify(fp.clipped)
      + " 变成包含块的祖先=" + JSON.stringify(fp.breakers));
  check("选择器：那块面板整个在视口内（量的是里面的 dialog，不是零高度的壳）",
    pp.ok === true && pp.inViewport === true,
    "面板四边都在视口内",
    "rect=" + JSON.stringify(pp.rect) + " 视口=" + pp.vw + "x" + pp.vh
      + " 宽=" + (pp.rect.r - pp.rect.l) + " 高=" + (pp.rect.bo - pp.rect.t));
  // 反面对照：把壳的 fixed 坐标推到视口右缘外 ⇒ 里面那块面板必须跟着出去
  await cdp.eval("(function(){var el=document.querySelector('[data-reaction-picker]');"
    + "if(!el)return false;el._o={left: el.style.left, right: el.style.right};"
    + "el.style.right='auto';el.style.left=(window.innerWidth - 20) + 'px';return true;})()");
  const ppOut = await cdp.eval("window.__probe.floatLayer('[data-reaction-picker] [role=dialog]')");
  check("对照：把选择器那层壳推到视口右缘外 ⇒ 面板必须落到视口外（证明那条视口判据会咬）",
    ppOut.ok === true && ppOut.inViewport === false,
    "inViewport === false",
    "rect=" + JSON.stringify(ppOut.rect) + " 视口=" + ppOut.vw);
  await cdp.eval("(function(){var el=document.querySelector('[data-reaction-picker]');"
    + "if(!el||!el._o)return false;el.style.left=el._o.left;el.style.right=el._o.right;"
    + "delete el._o;return true;})()");
  const ppBack = await cdp.eval("window.__probe.floatLayer('[data-reaction-picker] [role=dialog]')");
  check("对照可逆：壳换回原来的锚点之后，面板又整个回到视口内（对照自己不留残留）",
    ppBack.ok === true && ppBack.inViewport === true,
    "inViewport === true", "rect=" + JSON.stringify(ppBack.rect));

  // ============ 已读成员弹层：这一支就是"真机：已读列表靠右被裁一半"那一支 ============
  // 挂的是 MessageReceipt 本体（它那一层浮层就是**带 fixed 的面板自己**，不像选择器那样壳/面板要分开量）。
  // ⚠️ 这一支的量法是**独立挂载**：escape 与在不在视口都判得动（同一份 placeCard、同一个 Teleport），
  //    但它不在真实消息行的几何里 ⇒ 别把它读成"真机那条列表的位置也被判过了"。
  const rmount = await cdp.eval("window.__probe.install('/src/components/message/MessageReceipt.vue', '', "
    + JSON.stringify(JSON.stringify({
      state: "read", title: "已读", isGroup: true, msgKey: "m-receipt-probe-1",
      readerIds: ["r-1", "r-2", "r-3", "r-4", "r-5"],
    })) + ")");
  check("已读弹层：独立挂出一条带 5 个已读成员的回执", rmount && rmount.ok === true, "install 成功", JSON.stringify(rmount).slice(0, 160));
  await cdp.eval("window.__probe.seedDevice('me-1')");
  await new Promise((r) => setTimeout(r, 300));
  const rb = await cdp.eval("(function(){var b=document.querySelector('button');"
    + "if(!b)return null;var r=b.getBoundingClientRect();"
    + "return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2),"
    + " label: b.getAttribute('aria-label')};})()");
  check("已读弹层：拿得到那枚已读入口的落点", !!rb, "button 非空", JSON.stringify(rb));
  let readersOpen = false;
  for (let k = 0; k < 5 && !readersOpen; k += 1) {
    await cdp.hover(Math.max(2, rb.x - 80), rb.y);
    await cdp.click(rb.x, rb.y);
    readersOpen = !!(await cdp.eval("!!document.querySelector('[data-readers-popover]')"));
  }
  const fr = await cdp.eval("window.__probe.floatLayer('[data-readers-popover]')");
  check("已读弹层：点开后挂在 body 上、没有任何祖先裁得到它，且整块在视口内",
    readersOpen === true && fr.ok === true && fr.parentIsBody === true
      && fr.escapes === true && fr.inViewport === true,
    "开得出 + 挂 body + escapes + 在视口内",
    "开=" + readersOpen + " 挂body=" + fr.parentIsBody + " position=" + fr.position
      + " 切得到它的祖先=" + JSON.stringify(fr.clipped)
      + " 包含块破坏者=" + JSON.stringify(fr.breakers)
      + " rect=" + JSON.stringify(fr.rect) + " 视口=" + fr.vw + "x" + fr.vh);
  await cdp.eval("(function(){var el=document.querySelector('[data-readers-popover]');"
    + "if(!el)return false;el._o={left: el.style.left, right: el.style.right};"
    + "el.style.right='auto';el.style.left=(window.innerWidth - 20) + 'px';return true;})()");
  const frOut = await cdp.eval("window.__probe.floatLayer('[data-readers-popover]')");
  check("对照：把已读弹层的 fixed 坐标推到视口右缘外 ⇒ 它必须落到视口外（证明那条判据会咬）",
    frOut.ok === true && frOut.inViewport === false,
    "inViewport === false", "rect=" + JSON.stringify(frOut.rect) + " 视口=" + frOut.vw);
  await cdp.eval("(function(){var el=document.querySelector('[data-readers-popover]');"
    + "if(!el||!el._o)return false;el.style.left=el._o.left;el.style.right=el._o.right;"
    + "delete el._o;return true;})()");
  const frBack = await cdp.eval("window.__probe.floatLayer('[data-readers-popover]')");
  check("对照可逆：换回原锚点后整块又回到视口内",
    frBack.ok === true && frBack.inViewport === true, "inViewport === true",
    "rect=" + JSON.stringify(frBack.rect));
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

/**
 * 弹窗 / 浮层那一段（原生体验审计的领域 F：打开与关闭、遮罩与命中、Esc、焦点陷阱与恢复、
 * 快速重复开关之后留下的残骸）。
 *
 * 为什么值得单开一段：领域 F 原先只有**静态**判据 —— designGuards 的「菜单没有遮罩」「浮层没有 Esc
 * 出口」那一族，加层级阶梯 ㉕。静态那半边能证明"源码里写了 Esc"，证不了这四件行为：
 *   ① 遮罩真挡得住点击（穿透 = 用户在弹窗背后误触，还看不见自己点着了什么）；
 *   ② Esc 按下去真关得掉，且关掉之后 DOM 里不剩东西（§八「无遗留遮罩 / 不可交互区域」）；
 *   ③ 关闭后焦点回到打开它的那颗按钮（§B「交互完成后焦点是否正确恢复」）；
 *   ④ 连开关几轮之后不往历史里漏条目（Android 返回键"要按好几下"那一族，
 *      `backStack.test.ts` 用的是**假端口**，真 `history` 上的接得对不对这一格它量不到）。
 * 四条读的都是真命中测试 / 真按键 / 真鼠标 / 真 history，不是 class 名。
 *
 * ⚠️ 口径边界（报结论必须带上）：这里挂的是**弹窗外壳本身**（BaseModal + Headless UI，slot 为空，
 *   `open` 由夹具代管、收到 close 后代关 —— 与每个调用方的写法一致）。所以量到的是模态外壳的行为，
 *   不量某个具体业务弹窗（创建群聊 / 转发…）的表单校验与提交；
 *   且按键走的是**浏览器**输入管线 ⇒ WKWebView / WebView2 那一半仍未证（归 Smoke-11）。
 */
async function runOverlay(cdp, url) {
  // 背景给两样东西：一屏垫高（"滚动锁定"这一格到底有没有作用点，由读数自己说）
  // + 一颗 fixed 的按钮（坐标固定 ⇒ "这一点命中的是谁"才有唯一答案；它自带 click 计数 ⇒ 判穿透）。
  const MOUNT_HTML =
    '<div id="spacer" style="height:300vh"></div>'
    + '<button id="opener" type="button" style="position:fixed;left:24px;bottom:24px;z-index:10">开弹窗</button>';
  const PROPS = JSON.stringify(JSON.stringify({ title: "探针弹窗", width: "max-w-sm" }));

  // 页面侧读数器 + "像真宿主一样接 close"的那 25ms 轮询。
  // ★ 为什么必须有轮询：install 的 onClose 只登记事件（固定写法），而每个真实调用方收到 close
  //   都会把 open 关掉。手工 settle 只在"我以为的那一步"补这一下 ⇒ 晚到的 close（比如 popstate
  //   引起的）没人接，量出来的"残留"其实是弹窗**正常开着**。轮询比两处过渡（150/200ms）快一个量级。
  const READER = `(() => {
    window.__openerHits = 0;
    document.getElementById('opener').addEventListener('click', () => { window.__openerHits += 1; });
    window.__ovOpen = (on) => { window.__probe.state.open = !!on; };
    window.__ovFocusOpener = () => { document.getElementById('opener').focus(); };
    // 出帧计数器（start / stop 两段，不返回 Promise ⇒ 页面冻住时也绝不把探针挂死）：
    // Headless UI 的过渡**收尾**靠的是 disposables.nextFrame = 双层 requestAnimationFrame
    // ⇒ 页面不出帧，leave 就停在第一步（类名停在 leave-from），壳永远挂在 DOM 上。
    // 所以"关掉之后还剩东西"这类读数必须先自证帧在出：帧在出 ⇒ 读数算在量界面；
    // 帧没出 ⇒ 读数只算在量量具。（2026-10-10 就是这条把一版假 P1 戳掉的。）
    window.__ovFrameStart = () => {
      window.__ovFrameN = 0;
      window.__ovFrameRun = true;
      const step = () => {
        if (!window.__ovFrameRun) return;
        window.__ovFrameN += 1;
        requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
      return true;
    };
    window.__ovFrameStop = () => { window.__ovFrameRun = false; return window.__ovFrameN; };
    window.__ovParentAuto = () => {
      let seen = window.__probe.events.filter((el) => el[0] === 'close').length;
      const tick = () => {
        const n = window.__probe.events.filter((el) => el[0] === 'close').length;
        if (n > seen) { seen = n; window.__probe.state.open = false; }
      };
      window.__ovAutoTimer = setInterval(tick, 25);
      return true;
    };
    window.__ovPoint = (which) => {
      // ✕ 按 **aria-label** 选：BaseModal 卡片形态里只有它带 aria-label（common.close）。
      // 原先取"弹窗里最后一个 button"——空 slot 时那正好是 ✕，但 D 组弹窗里有内容，
      // 于是量到的是内容里那颗（点了个没接任何东西的按钮，close 计数 0，整段结论作废）。
      let el = null;
      if (which === 'close') {
        el = document.querySelector('[role="dialog"] button[aria-label]');
        if (!el) {
          const inPanel = document.querySelectorAll('[role="dialog"] .elevated button');
          el = inPanel.length ? inPanel[inPanel.length - 1] : null;
        }
      } else {
        el = document.getElementById('opener');
      }
      if (!el) return { found: false, why: which + ' 没找到' };
      const r = el.getBoundingClientRect();
      return { found: true, x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
    };
    window.__ovRead = () => {
      const dlgs = document.querySelectorAll('[role="dialog"]');
      const dlg = dlgs[0] || null;
      const panel = dlg ? dlg.querySelector('.elevated') : null;
      const portal = document.getElementById('headlessui-portal-root');
      const op = document.getElementById('opener');
      const rr = op.getBoundingClientRect();
      const hit = document.elementFromPoint(Math.round(rr.left + rr.width / 2), Math.round(rr.top + rr.height / 2));
      const ae = document.activeElement;
      const pr = panel ? panel.getBoundingClientRect() : null;
      const name = (m) => (m ? (m.id || m.tagName + '.' + String(m.className).slice(0, 22)) : 'null');
      return {
        openFlag: (window.__mstate || window.__probe.state).open === true,
        dlgCount: dlgs.length,
        // portal 根是**惰性创建**的（从没开过弹窗时它压根不存在）⇒ 不存在就等于 0 份
        portalDlgCount: portal ? portal.querySelectorAll('[role="dialog"]').length : 0,
        panelW: pr ? Math.round(pr.width) : 0,
        panelTop: pr ? Math.round(pr.top) : -1,
        panelBottom: pr ? Math.round(pr.bottom) : -1,
        vh: window.innerHeight,
        hitId: name(hit),
        panelOpacity: panel ? getComputedStyle(panel).opacity : '-',
        panelCls: panel ? String(panel.className) : '-',
        // 卡在哪一相位只能看类名，而 leave 那几串类在**尾部**（58 个字符正好把它们截掉）；
        // 一并读 transition-duration：F() 是按计算后的时长 setTimeout 的，时长与"没清理"同框才有解释力。
        panelTrans: panel
          ? getComputedStyle(panel).transitionProperty + " / " + getComputedStyle(panel).transitionDuration
            + " / " + getComputedStyle(panel).transitionDelay
          : '-',
        backdropOpacity: (() => {
          const g = dlg ? dlg.querySelector('.glass') : null;
          return g ? getComputedStyle(g).opacity : '(无遮罩节点)';
        })(),
        wrapperPE: (() => {
          const w = dlg ? dlg.querySelector('.overflow-y-auto') : null;
          return w ? getComputedStyle(w).pointerEvents : '-';
        })(),
        focusInDlg: !!dlg && !!ae && (dlg === ae || dlg.contains(ae)),
        focusId: name(ae),
        openerHits: window.__openerHits,
        closeEvents: window.__probe.events.filter((e) => e[0] === 'close').length,
        histLen: history.length,
        htmlOv: getComputedStyle(document.documentElement).overflow,
        bodyStyle: document.body.getAttribute('style') || '(无)',
        scrollY: Math.round(window.scrollY),
        docScrollable: document.documentElement.scrollHeight > document.documentElement.clientHeight,
      };
    };
    return true;
  })()`;

  /**
   * 出帧泵：`Page.captureScreenshot` 会强制走一遍"更新渲染"，rAF 回调在那一步里跑。
   * 为什么这段非用不可（2026-10-10 实测）：headless-new 起的探针页实测 `visibilityState=hidden`
   * （文件里第 1340 行那条纪律就是为它写的），页面**静止时根本不出帧**；而 Headless UI 的
   * 过渡收尾要两帧。不泵帧就读"关掉之后"，读到的是停在 leave-from 的壳 ⇒ 那是量具的形状。
   * ⚠️ 只在本段用：不动全局启动参数（`--disable-renderer-backgrounding` 之类会把
   *   `hasFocus()`/visibilityState 一起翻掉，而 emoji 那段有一条断言正按"页面没焦点"写）。
   */
  const pump = async (times = 6) => {
    for (let i = 0; i < times; i += 1) {
      await cdp.send("Page.captureScreenshot", { format: "png" }).catch(() => {});
      await sleep(50);
    }
  };
  /** 关掉之后的一次读数：先泵帧让过渡收尾，再读 DOM。 */
  const readAfterClose = async () => {
    await pump(10);
    return await cdp.eval("window.__ovRead()");
  };
  /** 泵帧这段时间里 rAF 实际跑了几次 —— 量具自证用的就是它。 */
  const framesDuringPump = async (times = 6) => {
    await cdp.eval("window.__ovFrameStart()");
    await pump(times);
    return await cdp.eval("window.__ovFrameStop()");
  };

  /**
   * 每段自己起一次页面：一处红不许污染下一处（文件头纪律 3，同一段内串过三轮开关之后
   * DOM 里可能有东西，那时候再判"打开态只有一份"就不是在判它自己了）。
   */
  async function boot(tag) {
    await cdp.send("Page.navigate", { url });
    await sleep(3_000);
    await cdp.eval(PAGE_FIXTURE);
    const mounted = await cdp.eval(
      "window.__probe.install('/src/components/BaseModal.vue', " + JSON.stringify(MOUNT_HTML) + ", " + PROPS + ")",
    );
    await cdp.eval(READER);
    await cdp.eval("window.__ovParentAuto()");
    await sleep(80);
    const base = await cdp.eval("window.__ovRead()");
    const ok = !!mounted && mounted.ok === true && mounted.pinia === true
      && base.dlgCount === 0 && base.openFlag === false;
    check("[" + tag + "] 干净页起起来了（BaseModal + 一颗真按钮同页、pinia 用的是页面那一份、开局没有 dialog）",
      ok, "install ok 且 pinia=true 且 dlgCount=0",
      JSON.stringify({ m: mounted, open: base.openFlag, dlg: base.dlgCount }));
    // 量具自证：这一段后面每一条"关掉之后还剩什么"都站在"页面能出帧"上。
    // ⚠️ 这条红**不是**产品红，是量具红（这台机器 / 这个 headless 页此刻不出帧）；
    // 但它必须报出来 —— 没有它，下一轮又会把停在 leave-from 的壳读成"界面上有残留层"。
    const frames = await framesDuringPump(6);
    check("[" + tag + "] 量具在出帧（泵 6 次截图这段时间里 rAF ≥ 2 次）——"
      + "Headless UI 的过渡收尾要两帧；页面静止不出帧时读到的是停在 leave-from 的壳，那是量具的形状不是界面的形状",
      typeof frames === "number" && frames >= 2, "rAF 次数 >= 2", "实际 " + JSON.stringify(frames));
    return base;
  }

  const openModal = async () => {
    await cdp.eval("window.__ovFocusOpener()");
    await cdp.eval("window.__ovOpen(true)");
    await sleep(500); // 进入过渡 200ms + 两帧 ⇒ 量的是动画结束后的稳定几何
    await pump(4);
  };

  // ══ A 组：打开态本身（几何 / 焦点 / 遮罩命中 / 历史条目 / Tab 陷阱 / 点外部 / Esc）══
  const base = await boot("A");
  await openModal();
  const o = await cdp.eval("window.__ovRead()");
  check("A 打开后弹窗挂在**文档根**的 portal 上，且全页只有一份 dialog",
    o.openFlag === true && o.dlgCount === 1 && o.portalDlgCount === 1, "open 且两处都=1",
    "open=" + o.openFlag + " dlg=" + o.dlgCount + " portal=" + o.portalDlgCount);
  check("A 面板矩形落在视口内（桌面端窗口边缘 / 可视区域：上下都不越界）",
    o.panelW > 0 && o.panelTop >= 0 && o.panelBottom <= o.vh + 1,
    "panelTop>=0 且 panelBottom<=vh(" + o.vh + ")",
    "top=" + o.panelTop + " bottom=" + o.panelBottom + " w=" + o.panelW);
  check("A 焦点落进弹窗内部（不是留在 body 或触发按钮上）",
    o.focusInDlg === true, "focusInDlg=true", "activeElement=" + o.focusId);
  check("A 弹窗开着时，那颗背景按钮的坐标命中的**不是它**（遮罩真在前面挡着）",
    o.hitId !== 'opener', "hitId != opener", "命中 " + o.hitId);
  check("A 打开确实往真 history 里压了一条条目（分层返回不是只有单测里的假端口成立）",
    o.histLen > base.histLen, "histLen > " + base.histLen, "histLen=" + o.histLen);

  const trapIds = [];
  let trapOk = true;
  for (let i = 0; i < 5; i += 1) {
    await cdp.key("Tab", "Tab", "", 9);
    const r = await cdp.eval("window.__ovRead()");
    trapIds.push(r.focusId);
    if (r.focusInDlg !== true) trapOk = false;
  }
  check("A 连按 5 次 Tab 焦点始终没跑出弹窗（焦点陷阱由 Headless UI 提供，没被我们自己写漏）",
    trapOk === true, "五次都 focusInDlg=true", JSON.stringify(trapIds));

  // 真鼠标按下背景按钮那一点：一个动作同时判「不穿透」与「点外部关闭」
  const pt = await cdp.eval("window.__ovPoint('opener')");
  const beforeOutside = await cdp.eval("window.__ovRead()");
  await cdp.click(pt.x, pt.y);
  await sleep(500);
  const c = await readAfterClose();
  check("A 真按下遮罩那一点 ⇒ 背景按钮的 click 监听零次触发（点击不穿透到弹窗背后）",
    c.openerHits === beforeOutside.openerHits, "openerHits 不涨", "hits=" + c.openerHits);
  check("A 同一个动作就把它关掉了（点外部关闭：close 恰好一次，且 DOM 里不剩 dialog）",
    c.closeEvents === beforeOutside.closeEvents + 1 && c.openFlag === false && c.dlgCount === 0,
    "close+1 且 open=false 且 dlgCount=0",
    JSON.stringify({ cl: c.closeEvents, open: c.openFlag, dlg: c.dlgCount }));

  // 反面对照（少了它上一条"零次"可能只是坐标选错）：关掉之后同一个坐标再按 ⇒ 必须真打到按钮
  await cdp.click(pt.x, pt.y);
  await sleep(300);
  const k = await cdp.eval("window.__ovRead()");
  check("A 对照：弹窗关了以后同一个坐标再按 ⇒ 按钮收到一次真 click（证明上一条零次不是坐标选错）",
    k.openerHits === beforeOutside.openerHits + 1, "openerHits +1",
    "hits=" + k.openerHits + " 命中 " + k.hitId);

  // Esc（真按键）：关得掉、DOM 撤干净、样式回到基线；焦点必须还给打开它的那颗按钮
  await openModal();
  const beforeEsc = await cdp.eval("window.__ovRead()");
  await cdp.key("Escape", "Escape", "", 27);
  await sleep(500);
  const e = await readAfterClose();
  check("A Esc（真按键）关得掉，且关掉后 DOM 与 body/html 样式都回到基线（§八「无遗留遮罩」）",
    e.closeEvents === beforeEsc.closeEvents + 1 && e.openFlag === false && e.dlgCount === 0
      && e.bodyStyle === base.bodyStyle && e.htmlOv === base.htmlOv,
    "close+1 且 open=false 且 dlgCount=0 且样式==基线",
    JSON.stringify({ cl: e.closeEvents, open: e.openFlag, dlg: e.dlgCount, bodyStyle: e.bodyStyle, htmlOv: e.htmlOv }));
  check("A Esc 关掉之后焦点回到打开它的那颗按钮（N14：Headless UI 只管把焦点收进弹窗，"
    + "还回去这一下得有人做 —— 而那一刻 DOM 已经撤干净，所以这不是残留壳造成的）",
    e.focusId === "opener" && e.dlgCount === 0, "activeElement==opener",
    "实际 " + e.focusId + " dlg=" + e.dlgCount);

  // 反面对照：已经关了再按一次 Esc ⇒ close 计数不许再涨（证明上面两条读的是真事件）
  await cdp.key("Escape", "Escape", "", 27);
  await sleep(250);
  const e2 = await cdp.eval("window.__ovRead()");
  check("A 对照：弹窗已关时再按 Esc 不多冒一条 close（判据读的是真事件，不是任何按键都记一笔）",
    e2.closeEvents === e.closeEvents, "close 计数不变(" + e.closeEvents + ")", "实际 " + e2.closeEvents);

  // 系统返回键那一层：真 history.back() ⇒ 弹窗收到 close 并被宿主关干净（按一次就有反应）
  await openModal();
  const beforeBack = await cdp.eval("window.__ovRead()");
  await cdp.eval("history.back()");
  await sleep(1_400); // 与 B 组同一口径：close 落地 → 宿主置假 → 离开过渡跑完，三件事排完才叫"关掉之后" 
  const bk = await readAfterClose();
  // 按一次后退 ⇒ 恰好一条 close；紧接着 DOM 撤干净（下面第二条）。
  // ⚠️ 这里以前钉着"只敢判 close 那一半"，因为另一半年年读到 dlg=1 —— 后来查明那是
  // **页面不出帧**时停在 leave-from 的壳（量具形状），不是界面的形状。见 boot() 里那条出帧判据。
  check("A 真按一次后退 ⇒ 恰好收到一条 close（Android 系统返回键走的就是这条 popstate）",
    bk.closeEvents === beforeBack.closeEvents + 1, "close 恰好 +1",
    JSON.stringify({ before: beforeBack.closeEvents, after: bk.closeEvents }));
  check("A 后退那一层关掉之后 DOM 撤干净（open=false 且 portal 里没有 dialog）——"
    + "先前那版\"留着一层看不见的壳\"是不出帧页面的形状，泵帧后这里必须是 0（§八「无遗留遮罩」）",
    bk.openFlag === false && bk.dlgCount === 0 && bk.portalDlgCount === 0,
    "open=false 且 dlg=0 且 portal=0",
    JSON.stringify({ open: bk.openFlag, dlg: bk.dlgCount, portal: bk.portalDlgCount, hit: bk.hitId }));

  // ══ B 组：最常用那条关闭路径单独量（右上角 ✕，真鼠标）——干净页，前面什么都不干 ══
  await boot("B");
  await openModal();
  const xb = await cdp.eval("window.__ovPoint('close')");
  check("B 弹窗里有可点的关闭按钮（✕ 那条路径有作用点）",
    xb.found === true, "found=true", JSON.stringify(xb));
  await cdp.click(xb.x, xb.y);
  await sleep(1_400); // 关 → 宿主把 open 置假 → 离开过渡 150ms，三件事得排完才叫"关掉之后"
  const x0 = await cdp.eval("window.__ovRead()"); // 只等、不泵：这一份读的是量具停在哪
  const x = await readAfterClose();               // 泵帧后读同一次关闭：这一份才叫界面
  const ptB = await cdp.eval("window.__ovPoint('opener')");
  console.log("  · 读数（不判绿红）：B 点 ✕ 后【只等不泵】dlg=" + x0.dlgCount
    + " 面板class 尾部=" + x0.panelCls.slice(-40)
    + "；【泵帧后】dlg " + x0.dlgCount + "→" + x.dlgCount + " close=" + x.closeEvents
    + " open=" + x.openFlag + " 遮罩opacity=" + x.backdropOpacity + " 命中=" + x.hitId);
  check("B 右上角 ✕ 关掉之后 DOM 撤干净（open=false 且 portal 里没有 dialog）——"
    + "§八「动画结束后不许遗留遮罩 / 不可交互区域」那一格",
    x.openFlag === false && x.dlgCount === 0 && x.portalDlgCount === 0,
    "open=false 且 dlg=0 且 portal=0",
    JSON.stringify({ open: x.openFlag, dlg: x.dlgCount, portal: x.portalDlgCount }));
  check("B 右上角 ✕ 关掉之后焦点也回到那颗按钮（两条关闭路径都要量：Esc 走 Headless UI 自己那条，"
    + "✕ 走宿主 emit close，发起方不一样）",
    x.focusId === "opener" && x.openFlag === false, "activeElement==opener",
    "实际 " + x.focusId + " open=" + x.openFlag);
  await cdp.click(ptB.x, ptB.y); // 关掉之后同一个坐标再真点一次：那层壳不许吃掉点击
  await sleep(300);
  const x4 = await readAfterClose();
  // 这里只判"点击落没落到按钮上"：A/B/C 的 opener 由探针直接翻 prop 打开、**不接 click**
  // （只有 D 那种生产形状的宿主才写 onClick ⇒ open）。所以"open=true"在这两组不成立，
  // 判它就是我编的规矩；命中 = opener 本身才是"那层壳没吃掉点击"的证据。
  check("B 关掉之后同一个坐标再真点 ⇒ 打得着那颗按钮（hits 恰好 +1、命中的就是 opener）",
    x4.openerHits === x.openerHits + 1 && x4.hitId === "opener",
    "hits+1 且 hitId=opener",
    JSON.stringify({ hits: x4.openerHits, base: x.openerHits, hit: x4.hitId }));

  // ══ C 组：快速连开关（每步 120ms，短于两处过渡 ⇒ 动画真被打断）——也是干净页 ══
  const cbase = await boot("C");
  for (let i = 0; i < 3; i += 1) {
    await cdp.eval("window.__ovOpen(true)");
    await sleep(120);
    await cdp.eval("window.__ovOpen(false)");
    await sleep(120);
  }
  await sleep(700);
  const f = await readAfterClose();
  check("C 连开关 3 轮（每步 120ms，短于两处过渡 ⇒ 动画真被打断）之后不剩壳："
    + "open=false 且 dlg=0 且 portal=0",
    f.openFlag === false && f.dlgCount === 0 && f.portalDlgCount === 0,
    "open=false 且 dlg=0 且 portal=0",
    JSON.stringify({ open: f.openFlag, dlg: f.dlgCount, portal: f.portalDlgCount }));
  check("C 动画被打断之后样式与滚动位回到基线（body[style]、html overflow、scrollY 三项都 == 干净页那份）",
    f.bodyStyle === cbase.bodyStyle && f.htmlOv === cbase.htmlOv && f.scrollY === cbase.scrollY,
    "三项都等于基线",
    JSON.stringify({ body: f.bodyStyle, htmlOv: f.htmlOv, y: f.scrollY }));
  const ptC = await cdp.eval("window.__ovPoint('opener')");
  await cdp.click(ptC.x, ptC.y);
  await sleep(300);
  const f2 = await readAfterClose();
  check("C 打断之后同一个坐标真点触发按钮 ⇒ 打得着那颗按钮（hits +1 且命中的就是 opener）",
    f2.openerHits === cbase.openerHits + 1 && f2.hitId === "opener",
    "hits+1 且 hitId=opener",
    JSON.stringify({ hits: f2.openerHits, base: cbase.openerHits, hit: f2.hitId }));
  check("C 连开关 3 轮没往历史里漏条目（漏了的话 Android 返回键要按好几下才有反应）",
    f.histLen <= cbase.histLen + 1, "histLen <= 基线+1(" + (cbase.histLen + 1) + ")",
    "histLen=" + f.histLen);

  // ══ D 组：**生产形状**的宿主（真点击开、弹窗里有 slot 内容、关闭走真实的 onClose→open=false）══
  // A/B/C 三组的 open 都是夹具直接翻 prop（`state.open = true`）、slot 是空的。
  // 那两处红到底是产品的还是夹具的，只有照 26 个调用方都用的那个形状再量一遍才知道。
  const MODAL_APP = `(() => {
    const V = window.__probe.vue;
    document.body.innerHTML = '<div id="mhost"></div>';
    const state = V.reactive({ open: false });
    window.__mstate = state;
    window.__mhits = 0;
    import('/src/components/BaseModal.vue').then((m) => {
      const BaseModal = m.default;
      const app = V.createApp({
        setup() {
          return () => V.h('div', {}, [
            V.h('div', { style: 'height:300vh' }),
            V.h('button', {
              id: 'opener', type: 'button',
              style: 'position:fixed;left:24px;bottom:24px;z-index:10',
              onClick: () => { window.__mhits += 1; state.open = true; },
            }, '开弹窗'),
            V.h(BaseModal, {
              open: state.open, title: '探针弹窗', width: 'max-w-sm',
              onClose: () => { state.open = false; },
            }, {
              default: () => V.h('div', {}, [
                V.h('input', { id: 'field', placeholder: '群名' }),
                V.h('button', { id: 'ok', type: 'button' }, '确定'),
              ]),
            }),
          ]);
        },
      });
      if (window.__pinia) app.use(window.__pinia);
      app.mount(document.getElementById('mhost'));
      window.__mmounted = true;
    });
    return true;
  })()`;
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  await cdp.eval("window.__probe.install('/src/components/TitleBar.vue', '', '')");
  await cdp.eval(MODAL_APP);
  let ready = false;
  for (let i = 0; i < 40 && !ready; i += 1) {
    ready = await cdp.eval("window.__mmounted === true");
    if (!ready) await sleep(250);
  }
  // 挂载完了才装读数器：READER 里那句 `#opener` 的 click 计数就是这么挂上的（挂早了元素还不存在）
  await cdp.eval(READER);
  check("D 生产形状的宿主挂起来了（真按钮 + 弹窗里带 input 与确定那颗）",
    ready === true, "__mmounted=true", JSON.stringify(await cdp.eval("window.__ovRead()")));
  const d0 = await cdp.eval("window.__ovRead()");
  const dpt = await cdp.eval("window.__ovPoint('opener')");

  await cdp.click(dpt.x, dpt.y); // 真的点触发按钮（26 个调用方都是这个形状）
  await sleep(600);
  const d1 = await cdp.eval("window.__ovRead()");
  check("D 真点触发按钮 ⇒ 弹窗打开且全页只有一份（slot 里的内容也在）",
    d1.openFlag === true && d1.dlgCount === 1 && d1.focusInDlg === true,
    "open=true 且 dlg=1 且焦点在弹窗里",
    JSON.stringify({ open: d1.openFlag, dlg: d1.dlgCount, focus: d1.focusId, hits: d1.openerHits }));

  const dxb = await cdp.eval("window.__ovPoint('close')");
  await cdp.click(dxb.x, dxb.y); // 真点右上角 ✕
  await sleep(700);
  const d2 = await readAfterClose();
  check("D 生产形状（真点按钮开、真点 ✕ 关、宿主 open 变假）也撤干净且焦点回到那颗按钮："
    + "open=false 且 dlg=0 且 portal=0 且 activeElement==opener —— 26 个调用方都是这个形状，这条才是定性依据",
    d2.openFlag === false && d2.dlgCount === 0 && d2.portalDlgCount === 0 && d2.focusId === "opener",
    "open=false 且 dlg=0 且 portal=0 且焦点==opener",
    JSON.stringify({ open: d2.openFlag, dlg: d2.dlgCount, hit: d2.hitId, focus: d2.focusId }));
  console.log("  · 读数（不判绿红）：D 生产形状关掉之后焦点=" + d2.focusId + " 命中=" + d2.hitId
    + "（这一组的按钮真的接 onClick ⇒ 判完焦点，同坐标那一发已经另有一条判据在打）");

  await cdp.click(dpt.x, dpt.y); // 关掉之后同一坐标再真点一次
  await sleep(400);
  const d3 = await readAfterClose();
  check("D 生产形状：关掉之后同一个坐标再真点 ⇒ 打得开（hits 恰好 +1、弹窗又只有一份）",
    d3.openerHits === d2.openerHits + 1 && d3.openFlag === true && d3.dlgCount === 1,
    "hits+1 且 open=true 且 dlg=1",
    JSON.stringify({ hits: d3.openerHits, base: d2.openerHits, open: d3.openFlag, dlg: d3.dlgCount }));

  await cdp.eval("window.__ovOpen(false)");
  await sleep(400);

  // 读数，不判据：这一格在本应用里到底有没有作用点，只有实测说得出（写进文档，别凭猜）
  console.log("  · 读数（不判绿红）：文档层可滚=" + base.docScrollable
    + " / 打开时 html overflow=" + o.htmlOv + " body[style]=" + o.bodyStyle
    + " / 关闭后 html overflow=" + base.htmlOv + " ⇒ 「滚动锁定」判的是「关闭后必须回到基线」那一半");
}

/** 表情面板那一段：键盘出口（N1）+ 反向对照（不带 text 的 Enter 必须零次激活）。 */
async function runEmoji(cdp, url) {
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  // ★ 先往本机账里种 8 条常用，**再**装组件（`usage` 是在 setup 里读的，装完再写就晚了）。
  //   不种这一步，`frequentCount` 恒为 0 ⇒ 「常用」那一行连它上面的小标题根本不渲染，
  //   于是下面那批键盘判据量不到本轮新加的那个占一整行的标题元素 —— 而它正是最可能
  //   把"整行步长"弄错的东西（标题不是按钮，但它在按钮之前占了一行）。
  //   token 从组件自己那份全表现读，不在这份脚本里手抄表情名。
  const table = await cdp.eval(
    "(async () => { const m = await import('/src/utils/emoji.ts');" +
      " return { tokens: m.EMOJIS.slice(0, 8).map((e) => e.displayName), total: m.EMOJIS.length }; })()",
  );
  const seed = {};
  table.tokens.forEach((tk, i) => { seed[tk] = { n: 90 - i, t: 1 }; });
  await cdp.eval(
    `localStorage.setItem('gosslan.emojiUsage.v1', ${JSON.stringify(JSON.stringify(seed))})`,
  );
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

  // ---------------- 本轮新增：「常用」那一行的形状（用户 2026-10-09） ----------------
  const head = await cdp.eval("window.__probe.gridHead()");
  check("「常用」那一行上面有小标题，且它是网格第一个子元素、且不是 button",
    head.tag !== 'none' && head.tag !== 'BUTTON' && head.text.length > 0,
    "第一个子元素有文字且不是按钮（是按钮就会挤进键盘步长）", JSON.stringify(head));

  const cells = await cdp.eval("window.__probe.emojiCells()");
  const freq = cells.filter((c) => c.freq);
  const matrix = cells.slice(table.tokens.length);
  const matrixAlts = matrix.map((c) => c.alt);

  const labels = await cdp.eval("window.__probe.gridLabels()");
  const labelTexts = labels.map((l) => l.text);
  check("两段各有一行小标题，顺序是「常用」在前、「全部表情」在后（用户 2026-10-09 参照图）",
    labelTexts.length === 2 && labelTexts[0].length > 0 && labelTexts[1].length > 0,
    "两个非按钮子元素、都有文字", JSON.stringify(labels));
  check("对照：两行标题都不是 button（是按钮就会挤进键盘整行步长）",
    labels.every((l) => l.tag !== 'BUTTON'), "全是 div", JSON.stringify(labels.map((l) => l.tag)));
  check("「全部表情」那一行确实落在两段交界处（在最后一格常用之后、第一格矩阵之前）",
    labels.length === 2 && freq.length > 0
      && labels[1].y > freq[freq.length - 1].y && labels[1].y < matrix[0].y,
    "标题 y 夹在两段之间", JSON.stringify({
      标题y: labels[1] && labels[1].y,
      末格常用y: freq[freq.length - 1] && freq[freq.length - 1].y,
      首格矩阵y: matrix[0] && matrix[0].y,
    }));

  check("常用那一行渲染出来了（种了几条就该有几格带常用标记）",
    freq.length === table.tokens.length, `${table.tokens.length} 格`, freq.length);
  check("格子总数 = 常用那一行 + **完整**一张表（矩阵没被摘走几格）",
    cells.length === table.tokens.length + table.total,
    `${table.tokens.length}+${table.total}`, cells.length);
  check("矩阵段自己零重复（它是原样那张表，不是常用段的复制）",
    new Set(matrixAlts).size === matrixAlts.length, "无重复",
    `去重后 ${new Set(matrixAlts).size} / 共 ${matrixAlts.length}`);
  check("每个常用表情仍留在矩阵段里（「下面不随上面变化」的正面判据）",
    freq.every((c) => matrixAlts.includes(c.alt)), "常用那几个在原序里都还在",
    JSON.stringify(freq.map((c) => c.alt).filter((a) => !matrixAlts.includes(a))));
  check("对照：同一表情两格，只有常用那一格**真的被染色**（读计算后的背景色，不是标记属性）",
    cells[0].freq === true && matrix[0].freq === false
      && cells[0].bg !== matrix[0].bg && cells[0].alt === matrix[0].alt,
    "第 0 格与矩阵同一格：alt 相同、背景色不同", JSON.stringify([cells[0], matrix[0]]));
  check("对照：小标题那一行没把常用行挤成不满一行（现读列数仍是 8 的整行）",
    cols > 1 && freq.every((c) => c.y === freq[0].y) && new Set(freq.map((c) => c.x)).size === freq.length,
    "常用那一行的格子同一 y、x 各不相同", `${cols} 列 / ${freq.length} 格`);

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

  // ---------------- 第二趟：**账是空的**（新设备）那一档长什么样 ----------------
  // 用户 2026-10-09 明确要的那半句："在没有常用数据的时候，常用表情模块消失只有全部表情"。
  // 必须重新 navigate 再装：`usage` 是在组件 setup 里一次性读 localStorage 的，
  // 装完再清账不会让已经挂起来的那块面板变回去；而在同一页里再装一次会留下第二个面板，
  // `buttons()` 会把两块的格子混着数（探针纪律 3：段与段之间不共享浮层状态）。
  await cdp.send("Page.navigate", { url });
  await sleep(3_000);
  await cdp.eval(PAGE_FIXTURE);
  await cdp.eval("localStorage.removeItem('gosslan.emojiUsage.v1')");
  await cdp.eval(`window.__probe.install('/src/components/EmojiPicker.vue',
    '<button id="trigger" data-probe-panel>触发按钮</button>')`);
  await cdp.eval("window.__probe.focusTrigger()");
  await cdp.eval("window.__probe.setOpen(true)");
  const emptyLabels = await cdp.eval("window.__probe.gridLabels()");
  const emptyCells = await cdp.eval("window.__probe.emojiCells()");
  check("没有常用数据时：只剩「全部表情」一个标题，「常用」那一节整个消失",
    emptyLabels.length === 1 && emptyLabels[0].text.length > 0
      && emptyLabels[0].tag !== 'BUTTON',
    "恰好一个非按钮标题", JSON.stringify(emptyLabels));
  check("没有常用数据时：格子就是完整那张表，一格不多（常用段没留空位）",
    emptyCells.length === table.total && emptyCells.every((c) => c.freq === false),
    `${table.total} 格且无常用标记`, `${emptyCells.length} 格`);
  check("没有常用数据时：网格第一个子元素就是「全部表情」标题（不是某格按钮）",
    (await cdp.eval("window.__probe.gridFirstChildTag()")) === 'DIV',
    "DIV", await cdp.eval("window.__probe.gridFirstChildTag()"));
  const emptyCols = await cdp.eval("window.__probe.cols()");
  check("没有常用数据时列数仍是 8（少一节标题不该把布局换成另一档）",
    emptyCols > 1 && emptyCols === cols, `与有账那一趟同为 ${cols}`, `${emptyCols}`);
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
    // ⚠️ 三条**不能都是 text**：这一段的夹具以前全是 `kind: "text"`，于是它和
    //   `chatSearch.test.ts` 犯的是同一个盲点 —— 载荷型 kind 渲染成人话这件事从没被真渲染过
    //   （用户 2026-10-09：「搜索列表显示的都是 json」）。这里刻意混进两种卡片 kind：
    //   任务与图片的载荷都是 JSON，漏回旧写法就会在页面上出现花括号。
    messages: [
      { msg_id: "m0", sender_id: "p1", sender_name: "小布", kind: "text", content: "命中 0", ts: 1 },
      { msg_id: "m1", sender_id: "p1", sender_name: "小布", kind: "todo",
        content: '{"title":"命中 待办","assignees":["p1"],"status":"todo","priority":"high"}', ts: 2 },
      { msg_id: "m2", sender_id: "p1", sender_name: "小布", kind: "image",
        content: '{"name":"命中.png","path":"/x/命中.png","size":2048,"sha256":"ab12"}', ts: 3 },
    ],
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

  // ---------------- 本轮补：两条搜索缺陷在**真渲染**层各判一次 ----------------
  // 静态判据与单测判的是"cellText 有没有过 previewBody"，判不到"页面上到底长什么样"。
  // 这里读的是渲染完的整页文字：漏回旧写法就会露出 `{"title":` / `"sha256":` 这种东西。
  const pageText = await cdp.eval("window.__probe.text()");
  const leaked = ['{"', '"title":', '"sha256":', '"assignees":'].filter((s) => pageText.includes(s));
  check("命中行与左栏摘要里不许出现载荷原文（六种认识的卡片 kind 那一半）",
    h0.n === 3 && leaked.length === 0, "整页文字里没有 JSON 片段",
    `泄漏片段 ${JSON.stringify(leaked)} / 行数=${h0.n}`);
  check("对照：卡片 kind 确实渲染成了人话（不是整行空掉，也不是退化成未知 kind 占位）",
    pageText.includes("[任务] 命中 待办") && pageText.includes("[图片]")
      && !pageText.includes("不支持的消息"),
    "出现「[任务] 命中 待办」与「[图片]」", JSON.stringify(pageText.slice(0, 260)));

  // 筛选菜单的底：`.gosslan-menu` 自己不带背景，底在 `.frost` 上 —— 少挂一个类零报错，
  // 而坏法是"文字叠在底下的结果行上"。这里读计算后的 background-color，不读 class 列表
  // （读 class 就又回到"标着不等于有"那一类错）。
  const chip = await cdp.eval("window.__probe.filterChip()");
  check("对照：拿得到「发送人」筛选入口的落点（拿不到下面那条就是空跑）",
    !!chip && chip.x > 0 && chip.y > 0, "坐标 >0", JSON.stringify(chip));
  if (chip) {
    await cdp.click(chip.x, chip.y);
    const st = await cdp.eval("window.__probe.menuStyle()");
    check("点开筛选菜单：它自己必须真有底（透明 = 用户那句「筛选下拉都是透明的」）",
      st.present === true && st.bg !== "rgba(0, 0, 0, 0)" && st.bg !== "transparent",
      "计算后的背景色不是透明", JSON.stringify(st));
    // 菜单与键盘判据共用这个弹窗，而方向键挂在**输入框**上：点过菜单必须把焦点还回去，
    // 否则下面那批 ↑↓ 会红在夹具上而不是产品上。
    const refocus = await cdp.eval("window.__probe.focusInput()");
    check("对照可逆：关掉菜单、焦点回到输入框（下面那批键盘判据的前提）",
      refocus === true, "activeElement 又是输入框", refocus);
  }

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
