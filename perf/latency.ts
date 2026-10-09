// 输入→首帧可见反馈 量具页（手动/自动化皆可）。
//
// 它测的是 §八 那句「记录可重复测量的输入到反馈延迟」，而且只测这一段：
//   真实鼠标按下 → 浏览器把 click 交给**真实 BaseModal 的宿主** → Vue 把状态改到 DOM 上 →
//   下一帧里那个弹窗面板第一次带着可计算的样式出现在页面上。
// 它**不含**网络、数据库、IPC：那几个字若慢，量到的是别的层，别把这条当「消息发出去要多久」。
//
// 用法：npm run dev 后打开 /perf/latency.html，再按 perf/latency.mjs 的说明用 CDP 投真鼠标事件。
// 与 perf/vlist.html 同一条纪律：必须显式加载 @/style.css，否则 Tailwind 的类不存在 ⇒ 量具自己坏。

import { createApp, h, reactive, ref } from "vue";
import { createPinia } from "pinia";
import BaseModal from "@/components/BaseModal.vue";
import "@/style.css";

interface Sample {
  eventT: number | null;
  firstChangeT: number | null;
  frames: number;
  baseline: string;
  dialogSeen: number;
}

const state = reactive({ open: false, clicks: 0 });

/** 灵敏度对照用：在翻状态之前先原地空转这么多毫秒，模拟"处理里压了个长任务"。 */
let busyMs = 0;

let current: Sample | null = null;
let rafFrames = 0;
let frameRun = false;

/** 被观察的那一样东西：弹窗面板的计算样式。没挂上时是 "-"。 */
function panelStyle(): string {
  const el = document.querySelector('[role="dialog"]') as HTMLElement | null;
  if (!el) return "-";
  const cs = getComputedStyle(el);
  return `${cs.opacity}|${cs.visibility}|${cs.display}`;
}

function tick() {
  const s = current;
  if (!s) return;
  s.frames += 1;
  const now = performance.now();
  if (s.dialogSeen === 0 && document.querySelector('[role="dialog"]')) s.dialogSeen = now;
  if (s.firstChangeT === null && panelStyle() !== s.baseline) s.firstChangeT = now;
  requestAnimationFrame(tick);
}

// capture 阶段记 click 自己的 timeStamp —— 它和 performance.now() 同一个时基，
// 相减才是"从输入被交付"到"画面真的变了"的那段。
document.addEventListener(
  "click",
  (e) => {
    const s = current;
    if (s && s.eventT === null) s.eventT = e.timeStamp;
  },
  true,
);

const Host = {
  setup() {
    const opener = ref<HTMLElement | null>(null);
    return () =>
      h("div", {}, [
        h("div", { style: "height:200vh" }),
        h(
          "button",
          {
            id: "opener",
            ref: opener,
            type: "button",
            style:
              "position:fixed;left:24px;bottom:24px;z-index:10;padding:8px 14px;border:1px solid #94a3b8;border-radius:6px;background:#fff",
            onClick: () => {
              state.clicks += 1;
              if (busyMs > 0) {
                const end = performance.now() + busyMs;
                while (performance.now() < end) {
                  /* 空转到点：这一档是给"处理里有长任务"做对照的 */
                }
              }
              state.open = true;
            },
          },
          "打开弹窗",
        ),
        h(
          BaseModal,
          {
            open: state.open,
            title: "延迟量具",
            width: "max-w-sm",
            onClose: () => {
              state.open = false;
            },
          },
          {
            default: () =>
              h("div", {}, [
                h("input", { id: "field", placeholder: "群名", style: "border:1px solid #94a3b8;padding:4px" }),
                h("button", { id: "ok", type: "button" }, "确定"),
              ]),
          },
        ),
      ]);
  },
};

let loadError: string | null = null;
window.addEventListener("error", (e) => {
  loadError = String(e.message);
});

// ⚠️ 顺序要紧：api 必须**先挂到 window 再挂载组件**。反过来写的话，一旦真实组件在
// 挂载时抛错，window.__lat 就是 undefined，驱动只会报"页面没起来"，把真因（一次渲染异常）
// 说成加载问题 —— 这一版就是这么被骗过一次。
/**
 * 仿真对照要一起报的媒体特性：只报这一条会看不出"仿真有没有顺手改到别的档"。
 * ⚠️ 存的是**查询串**，`matches` 必须在读数那一刻现取 —— 在这里把布尔值存成常量，
 * 模块加载时就冻结了，之后 `Emulation.setEmulatedMedia` 改成什么它都照原样报回去
 * （那正是"探针看起来在工作、其实一条也没变"的形状）。
 */
const MEDIA_PROBES = [
  "(prefers-reduced-motion: reduce)",
  "(prefers-reduced-transparency: reduce)",
  "(hover: hover)",
  "(pointer: coarse)",
];

const api = {
  mounted: () => !!document.getElementById("opener"),
  error: () => loadError,
  point: () => {
    const el = document.getElementById("opener") as HTMLElement;
    const r = el.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
  },
  begin: () => {
    current = { eventT: null, firstChangeT: null, frames: 0, baseline: panelStyle(), dialogSeen: 0 };
    requestAnimationFrame(tick);
    return true;
  },
  end: () => {
    const s = current;
    current = null;
    if (!s) return null;
    return {
      eventT: s.eventT,
      firstChangeT: s.firstChangeT,
      dialogSeen: s.dialogSeen,
      frames: s.frames,
      delta: s.firstChangeT !== null && s.eventT !== null ? s.firstChangeT - s.eventT : null,
    };
  },
  setBusy: (ms: number) => {
    busyMs = ms;
    return busyMs;
  },
  close: () => {
    state.open = false;
    return true;
  },
  isOpen: () => state.open === true,
  clicks: () => state.clicks,
  panel: panelStyle,
  /**
   * 出帧自证：**持续**请求帧的计数器。
   * 教训（同一条坑第二次）：单次 rafPing 只能证明"那一刻有一帧"，证明不了"这段时间里一直在出帧" ——
   * 泵帧窗口里没人排队 rAF 的话，读回来的差值恒等于 0，量具会把自己"没人排队"误报成"页面冻住"。
   */
  frameStart: () => {
    rafFrames = 0;
    frameRun = true;
    const step = () => {
      if (!frameRun) return;
      rafFrames += 1;
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
    return true;
  },
  /** 返回的是**这一段窗口里**的帧数（frameStart 把计数清零），所以它自己就是判据，不需要再相减。 */
  frameStop: () => {
    frameRun = false;
    return rafFrames;
  },
  /**
   * 仿真对照用的读数：**页面自己**报它现在活在哪一档。
   * 只有 `matchMedia` 那一路不够 —— 它说"我匹配上了"并不等于样式真的变了，
   * 所以同时报面板此刻计算出来的过渡时长：reduce 档应该明显变小，否则仿真是空转。
   */
  /**
   * 仿真对照用的读数：**页面自己**报它现在活在哪一档。
   * 只有 `matchMedia` 那一路不够 —— 它说"我匹配上了"并不等于样式真的变了，
   * 所以同时报面板此刻计算出来的过渡时长：reduce 档应该明显变小，否则仿真是空转。
   * ⚠️ 面板只在开着的时候存在 ⇒ 要在**打开状态**下读（驱动里先点一下再读）。
   */
  media: () => {
    const el = document.querySelector('[role="dialog"]') as HTMLElement | null;
    const cs = el ? getComputedStyle(el) : null;
    return {
      probes: MEDIA_PROBES.map((m) => `${m} = ${matchMedia(m).matches}`).join(" ; "),
      transition: cs ? cs.transitionDuration : "-",
      animation: cs ? cs.animationDuration : "-",
      panel: panelStyle(),
      open: state.open,
    };
  },
  visibility: () => document.visibilityState,
};

(window as unknown as Record<string, unknown>).__lat = api;

// 挂载放进 try：真实组件在挂载期抛错时，页面至少还留着 __lat.error()，
// 驱动报得出"是哪一次渲染异常"，而不是只剩一句"页面没起来"。
try {
  const app = createApp(Host);
  // BaseModal 的 setup 里就调 useAppStore() ⇒ 没有活动 pinia 会当场抛，
  // 表现是"#opener 没渲染出来"。这一句是它能不能挂起来的前提，不是装饰。
  app.use(createPinia());
  app.mount(document.getElementById("lat-root") as Element);
} catch (e) {
  loadError = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
}
