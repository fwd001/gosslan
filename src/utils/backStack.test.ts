/**
 * 分层返回的纯逻辑单测。
 *
 * 为什么值得测：真机上的 Android 返回键行为**没有自动化验证手段**（本项目也没有
 * 设备测试环境），而这块逻辑一旦出错，症状是"返回键要按好几下才有反应"或
 * "按返回把不该关的层关了" —— 都属于用户会立刻骂、但代码看不太出来的问题。
 * 这里用假 HistoryPort 把每条边界钉住。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { createBackStack, type HistoryPort } from "./backStack.ts";

/** 假端口：记录压入的条目，`back()` 立刻回调（真实浏览器是异步 popstate）。 */
function makePort() {
  const entries: unknown[] = [];
  const port: HistoryPort = {
    push(state) {
      entries.push(state);
      return true;
    },
    back() {
      entries.pop();
    },
  };
  return { port, entries };
}

test("压一层 / 返回一次：只关最上面那层", () => {
  const { port, entries } = makePort();
  const back = createBackStack(port);
  const closed: string[] = [];
  back.push(() => closed.push("a"));
  back.push(() => closed.push("b"));
  assert.equal(back.depth(), 2);
  assert.equal(entries.length, 2, "每层压一条历史条目");
  assert.equal(back.pushedCount(), 2);

  assert.equal(back.onPop(), true);
  assert.deepEqual(closed, ["b"], "先关最上面那层");
  assert.equal(back.depth(), 1);
  assert.equal(back.pushedCount(), 1, "返回键消费掉一条条目（历史条目本身由浏览器回退）");

  assert.equal(back.onPop(), true);
  assert.deepEqual(closed, ["b", "a"]);
  assert.equal(back.depth(), 0);
});

test("没有层时返回键交给系统（Android 才应该退出应用）", () => {
  const { port } = makePort();
  const back = createBackStack(port);
  assert.equal(back.onPop(), false, "没有层 ⇒ 不处理，交给系统退出");
});

test("UI 主动关闭栈顶层：把历史条目退回去（不能让返回键多按几下）", () => {
  const { port, entries } = makePort();
  const back = createBackStack(port);
  const release = back.push(() => {});
  assert.equal(entries.length, 1);

  release();
  assert.equal(entries.length, 0, "UI 关闭后历史条目也要回退");
  assert.equal(back.depth(), 0);
  // 我们自己 back() 触发的 popstate 必须被吃掉，不能误关下一层
  const closed: string[] = [];
  back.push(() => closed.push("under"));
  assert.equal(back.onPop(), true, "（吃掉自己触发的那次）");
  assert.deepEqual(closed, [], "不能让下层被误关");
  assert.equal(back.depth(), 1, "下层仍在");
});

test("释放是幂等的：重复释放不多退历史条目", () => {
  const { port, entries } = makePort();
  const back = createBackStack(port);
  const release = back.push(() => {});
  release();
  release();
  release();
  assert.equal(entries.length, 0);
});

test("关闭非栈顶层：只摘登记，不关错层、也不乱退条目", () => {
  const { port, entries } = makePort();
  const back = createBackStack(port);
  const closed: string[] = [];
  const releaseBottom = back.push(() => closed.push("bottom"));
  back.push(() => closed.push("top"));
  assert.equal(entries.length, 2);

  releaseBottom();
  assert.equal(back.depth(), 1, "只摘掉下面那层");
  assert.equal(entries.length, 2, "非栈顶关闭不动历史条目（留一条空条目，后续静默消费）");
  assert.equal(back.onPop(), true);
  assert.deepEqual(closed, ["top"], "返回键仍关的是栈顶那层");
});

test("端口压条目失败时：层照常能关，只是没有历史条目（退化为现状）", () => {
  const entries: unknown[] = [];
  const port: HistoryPort = {
    push: () => false,
    back() {
      entries.pop();
    },
  };
  const back = createBackStack(port);
  const closed: string[] = [];
  back.push(() => closed.push("a"));
  assert.equal(back.pushedCount(), 0);
  // 这种环境下 popstate 不会由我们触发；直接调用时也应关掉层
  assert.equal(back.onPop(), true);
  assert.deepEqual(closed, ["a"]);
});
