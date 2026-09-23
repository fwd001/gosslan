import { test } from "node:test";
import assert from "node:assert/strict";
import { StaleGuard } from "./staleGuard.ts";

test("StaleGuard：同一 key 连发两次，只有最新那次能落地", () => {
  const g = new StaleGuard();
  const a = g.begin("conversations");
  const b = g.begin("conversations");
  assert.equal(g.isCurrent("conversations", a), false, "更早的令牌必须失效");
  assert.equal(g.isCurrent("conversations", b), true);
});

test("StaleGuard：不同 key 互不干扰", () => {
  const g = new StaleGuard();
  const peers = g.begin("peers");
  const groups = g.begin("groups");
  assert.ok(g.isCurrent("peers", peers) && g.isCurrent("groups", groups));
  g.begin("peers");
  assert.ok(!g.isCurrent("peers", peers), "peers 被新请求作废");
  assert.ok(g.isCurrent("groups", groups), "groups 不受影响");
});

test("StaleGuard：未 begin 过的 key 一律不算最新", () => {
  const g = new StaleGuard();
  assert.equal(g.isCurrent("nope", 1), false);
  assert.equal(g.isCurrent("nope", 0), false);
});

/** 手工可控的 promise，用来精确制造"后发先至"。 */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const p = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { p, resolve, reject };
}

test("StaleGuard 实战：先发起的请求**后**回来，也不许覆盖新快照", async () => {
  const g = new StaleGuard();
  let value = "init";
  const d1 = deferred<string>();
  const d2 = deferred<string>();

  const run = async (d: Promise<string>) => {
    const tok = g.begin("list");
    const r = await d;
    if (!g.isCurrent("list", tok)) return; // 旧快照整个丢弃
    value = r;
  };
  const first = run(d1.p);
  const second = run(d2.p);

  d2.resolve("new"); // 后发起的先回来
  await second;
  assert.equal(value, "new");
  d1.resolve("stale"); // 先发起的后回来
  await first;
  assert.equal(value, "new", "旧请求后到绝不能把新快照写回去");
});

test("StaleGuard 实战：被丢弃的那次连副作用（error/loading）都不该执行", async () => {
  const g = new StaleGuard();
  const sideEffects: string[] = [];
  const call = async (fail: boolean, gate: Promise<void>) => {
    const tok = g.begin("list");
    await gate;
    try {
      if (fail) throw new Error("ipc");
    } catch {
      if (!g.isCurrent("list", tok)) return; // ⚠️ catch/finally 也要过闸
      sideEffects.push("error");
    }
    if (!g.isCurrent("list", tok)) return;
    sideEffects.push("done");
  };
  const a = new Promise<void>((r) => setTimeout(r, 0));
  const b = new Promise<void>((r) => setImmediate(r));
  const p1 = call(true, a);
  const p2 = call(false, b);
  await Promise.all([p1, p2]);
  assert.deepEqual(sideEffects, ["done"], "只有最新一次的副作用允许发生；旧请求报错也不该弹 error");
});
