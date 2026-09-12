/**
 * `defer.ts` 的单测 + 「输入框优先」接线守卫。
 *
 * 前半是去抖语义本身的单测；后半是**静态接线守卫** —— 它读各输入框组件的源码，
 * 确认"原值绑输入框、延迟镜像喂派生渲染"这个结构没有被改回去。
 * 加这层守卫的原因：接线退化后界面**看起来完全正常**（功能都对），
 * 只有按住 Ctrl+V 连发粘贴时才卡；靠人眼回归是发现不了的。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";
import { debounce, shouldRunThrottled } from "./defer.ts";

const srcDir = join(import.meta.dirname, "..");

/** 用假定时器驱动：`setTimeout` 只在测试里被替换，去抖实现本身不做特判。 */
function withFakeTimers<T>(fn: (clock: { tick(ms: number): void }) => T): T {
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  let now = 0;
  let seq = 0;
  const timers = new Map<number, { at: number; cb: () => void }>();
  globalThis.setTimeout = ((cb: () => void, ms?: number) => {
    const id = ++seq;
    timers.set(id, { at: now + (ms ?? 0), cb });
    return id as unknown as ReturnType<typeof setTimeout>;
  }) as typeof setTimeout;
  globalThis.clearTimeout = ((id: number) => {
    timers.delete(id);
  }) as typeof clearTimeout;
  try {
    return fn({
      tick(ms: number) {
        now += ms;
        for (const [id, t] of [...timers]) {
          if (t.at <= now) {
            timers.delete(id);
            t.cb();
          }
        }
      },
    });
  } finally {
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
  }
}

test("连发调用只在停手后执行一次，且用最后一次的参数", () => {
  withFakeTimers((clock) => {
    const seen: string[] = [];
    const d = debounce((v: string) => seen.push(v), 100);
    // 模拟按住 Ctrl+V：10 次连发，间隔 20ms
    for (let i = 0; i < 10; i++) {
      d(`paste-${i}`);
      clock.tick(20);
    }
    assert.deepEqual(seen, [], "连发期间不应执行（否则每次粘贴都会重算派生渲染）");
    // 最后一次调用发生在 now=180（窗口在 280 到点）；此刻 now=200
    clock.tick(79);
    assert.deepEqual(seen, [], "未到窗口不应执行");
    clock.tick(1);
    assert.deepEqual(seen, ["paste-9"], "窗口结束只执行一次，且是最后一次的参数");
  });
});

test("间隔大于窗口时每次都执行（不会吞掉正常打字）", () => {
  withFakeTimers((clock) => {
    let n = 0;
    const d = debounce(() => n++, 50);
    d();
    clock.tick(60);
    d();
    clock.tick(60);
    assert.equal(n, 2);
  });
});

test("cancel 丢弃待执行；flush 立刻执行且之后不重复", () => {
  withFakeTimers((clock) => {
    const seen: string[] = [];
    const d = debounce((v: string) => seen.push(v), 100);
    d("a");
    d("b");
    d.cancel();
    clock.tick(500);
    assert.deepEqual(seen, [], "cancel 后连定时器都不该留下");
    d("c");
    d.flush();
    assert.deepEqual(seen, ["c"], "flush 立刻执行待执行的那一次");
    clock.tick(500);
    assert.deepEqual(seen, ["c"], "flush 之后原定时器必须已被清掉，不能重复执行");
  });
});

test("无待执行时 flush 是空操作（不会凭空多写一次库）", () => {
  withFakeTimers((clock) => {
    let n = 0;
    const d = debounce(() => n++, 100);
    d.flush();
    clock.tick(500);
    assert.equal(n, 0);
    d();
    clock.tick(100);
    assert.equal(n, 1);
    d.flush();
    assert.equal(n, 1, "执行完再 flush 不该再来一次");
  });
});

// ---------------- 接线守卫：输入框必须"原值即时、派生延迟" ----------------

function read(rel: string): string {
  return readFileSync(join(srcDir, rel), "utf8");
}

/**
 * 断言：输入框绑原值 + 建了延迟镜像 + **派生渲染不再读原值**。
 *
 * 最后一条是重点：只检查"有没有调 useDeferredRef"是不够的（那样建了镜像却仍读原值
 * 也能骗过测试）。这里把原值的**写入**（`ref.value = ...`，如"打开弹窗时清空关键词"）
 * 排除掉，剩下的读引用一律不允许出现。
 */
function assertDeferredInput(rel: string, rawRef: string) {
  const src = read(rel);
  assert.match(
    src,
    new RegExp(`v-model="${rawRef}"`),
    `${rel}: 输入框应绑原值 \`${rawRef}\`（DOM 立即更新，零响应式成本）`,
  );
  assert.match(
    src,
    new RegExp(`useDeferredRef\\(\\s*${rawRef}\\b`),
    `${rel}: 必须为 \`${rawRef}\` 建延迟镜像并把派生渲染接过去 —— ` +
      `否则连发粘贴时每个字符都会重算/重渲染，输入框会一顿一顿`,
  );
  // 去掉赋值语句后，组件里不应再有对原值的读取
  const withoutWrites = src.replace(new RegExp(`${rawRef}\\.value\\s*=[^=]`, "g"), "WRITE");
  const reads = withoutWrites.match(new RegExp(`${rawRef}\\.value`, "g")) ?? [];
  assert.equal(
    reads.length,
    0,
    `${rel}: 派生渲染仍直接读 \`${rawRef}.value\`（${reads.length} 处）—— 应改读延迟镜像，` +
      `否则延迟镜像等于没建（功能不变，但连发粘贴依旧卡）`,
  );
}

test("过滤型输入框都接了延迟镜像（连发粘贴不卡）", () => {
  // 加好友搜索：过滤 peers 列表
  assertDeferredInput("components/AddFriendModal.vue", "keyword");
  // 转发搜索：过滤会话列表
  assertDeferredInput("components/message/ForwardModal.vue", "keyword");
  // 日志过滤：每次重算命中行 + 每行 4 处高亮（上千行 v-html）
  assertDeferredInput("components/LogViewer.vue", "filter");
});

test("会话列表：输入不改列表，回车才搜索（用户 2026-09-12 明确要求）", () => {
  const list = read("components/ConversationList.vue");
  assert.match(list, /v-model="keyword"/, "搜索框绑原值");
  // ① 输入不再驱动会话列表过滤：列表恒为全量
  assert.match(
    list,
    /const listConversations = computed\(\(\) => chat\.conversations\)/,
    "会话列表必须是全量、不随输入变化（用户：『在上面输入，列表就不要有变化了』）",
  );
  assert.ok(!list.includes('v-for="c in filtered"'), "不得再用过滤结果渲染会话列表");
  assert.ok(!list.includes("api.searchMessages"), "列表不再查消息内容（搜消息全在弹窗里做）");
  assert.match(list, /@keydown\.enter\.prevent="onSearchEnter"/, "回车仍然要打开搜索弹窗");
  // ② 联系人页的姓名过滤仍走延迟镜像（连发粘贴不卡）
  assert.match(list, /const \{ keyword, query \} = useSearchKeyword\(\)/, "输入状态来自 useSearchKeyword");
  const composable = read("composables/useSearchKeyword.ts");
  assert.match(
    composable,
    /useDeferredRef\(\s*keyword\b/,
    "仍要为联系人姓名过滤建延迟镜像（否则粘贴时整列重渲染）",
  );
  assert.match(composable, /return \{ keyword, query \}/, "必须把延迟值暴露出来");
});

test("搜索弹窗：有 loading 态；清空关键词回到初始空态", () => {
  const dlg = read("components/search/ChatSearchDialog.vue");
  assert.match(
    dlg,
    /v-(?:else-)?if="searching"/,
    "搜索在途必须有可见的 loading（用户：『感觉显示得比较慢，当前状态没有提示』）",
  );
  assert.match(
    dlg,
    /v-if="!keyword\.trim\(\)"/,
    "关键词为空时要回到初始空态（而不是留着上一次的结果）",
  );
  assert.match(dlg, /useImeEnterGuard/, "回车要过输入法守卫（拼音候选里回车是选字）");
});

test("日志过滤的派生渲染读延迟值，不读原值", () => {
  const src = read("components/LogViewer.vue");
  const derived = src.slice(src.indexOf("const matched = computed"));
  assert.ok(
    derived.includes("deferredFilter.value") || derived.includes("query.value"),
    "LogViewer 的 matched/trimmedFilter 必须读延迟值；改成 filter.value 就会每字符重建上千行",
  );
  assert.match(
    src,
    /const trimmedFilter = computed\(\(\) => deferredFilter\.value/,
    "高亮用的 trimmedFilter 也必须读延迟值（否则高亮与命中集合不一致）",
  );
});

test("高频写入的设置项不每次都落库（IPC 风暴守卫）", () => {
  const src = read("stores/useAppStore.ts");
  assert.match(
    src,
    /persistSoon/,
    "useAppStore 需要为高频写入提供去抖持久化（persistSoon）",
  );
  const setter = src.slice(src.indexOf("function setThemeColor"));
  assert.ok(
    setter.slice(0, 400).includes("persistSoon"),
    "setThemeColor 由颜色选择器连续触发，必须走去抖持久化而不是每次都 persistSettings()",
  );
});

// ---------------- `shouldRunThrottled`（事件驱动的刷新节流） ----------------
//
// 用途：`peers-updated` 最多 3/s，而它触发的拓扑刷新变化很慢。每个事件都发一次 IPC
// 就是白白的 IPC 风暴（每次都要跨进程、过主线程消息循环），攒起来就是"顿"。

test("从未执行过（last = 0）一律放行", () => {
  assert.equal(shouldRunThrottled(1000, 0, 1000), true);
});

test("距上次不足间隔就不执行，够间隔才执行", () => {
  assert.equal(shouldRunThrottled(1500, 1000, 1000), false, "差 500ms 应被节流");
  assert.equal(shouldRunThrottled(1999, 1000, 1000), false, "差 999ms 仍被节流");
  assert.equal(shouldRunThrottled(2000, 1000, 1000), true, "差 1000ms 放行");
  assert.equal(shouldRunThrottled(9999, 1000, 1000), true, "差得越多越放行");
});
