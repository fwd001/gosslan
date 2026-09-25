/**
 * `mergePeerList` 的行为判据（架构复审 P11 里唯一被实测证明有代价的那一条）。
 *
 * ## 为什么"复用引用"是有意义的，而不是审美问题
 * `peers-updated` 事件最多每秒 3 次，而三条写入点都是整表直写 `peers.value = list`
 * ⇒ 每一拍都换掉**数组本身和里面每一个对象**。凡是在渲染里读过 `peers` 的
 * （`nicknameOf` 就在消息行的模板里被逐行调用）依赖都被作废 ⇒
 * 每 333ms 把整屏消息行重画一遍，与"有没有真的发生变化"无关。
 * 直连时看不出问题，挂机半小时、周围节点上下线时就是持续的无谓重渲染。
 *
 * 所以这里钉的不是"返回什么"，而是**引用身份**：没变的那台设备必须还是同一个对象，
 * 全表没变时必须返回 `null` 让调用方**根本不赋值**。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mergePeerList } from "./peerMerge.ts";
import type { Peer } from "../types.ts";

function peer(id: string, over: Partial<Peer> = {}): Peer {
  return {
    device_id: id,
    nickname: `节点${id}`,
    avatar: null,
    device_type: "desktop",
    ip: "192.168.1.10",
    tcp_port: 9000,
    last_seen: 1_000,
    rtt_ms: 12,
    x25519_pubkey: "k",
    ed25519_pubkey: "e",
    first_seen: 900,
    link: "lan",
    ...over,
  };
}

test("整表内容一字未改时返回 null —— 调用方因此一次赋值都不做", () => {
  const prev = [peer("a"), peer("b")];
  const next = [peer("a"), peer("b")]; // 同字段、不同对象（每次 IPC 都是新反序列化出来的）
  assert.equal(mergePeerList(prev, next), null, "没变化还换数组 = 每拍白重画一屏");
});

test("只有一台设备变化时，其余设备必须沿用旧对象引用", () => {
  const a = peer("a");
  const b = peer("b");
  const prev = [a, b];
  const next = [peer("a"), peer("b", { link: "relay", rtt_ms: 300 })];
  const merged = mergePeerList(prev, next);
  assert.ok(merged, "b 变了，必须给出新数组");
  assert.equal(merged![0], a, "a 一个字段都没变 ⇒ 必须是同一个对象（行级才能跳过）");
  assert.equal(merged![1].link, "relay", "变了的那台要拿到新值");
  assert.notEqual(merged![1], b);
});

test("设备集合变化（新增 / 消失）必须给出新数组", () => {
  const a = peer("a");
  assert.deepEqual(mergePeerList([a], [a, peer("b")])?.map((p) => p.device_id), ["a", "b"]);
  assert.deepEqual(mergePeerList([a, peer("b")], [a])?.map((p) => p.device_id), ["a"]);
});

test("顺序变化要反映出来：它本身就是附近设备列表的呈现顺序", () => {
  const a = peer("a");
  const b = peer("b");
  const merged = mergePeerList([a, b], [b, a]);
  assert.ok(merged, "顺序变了不算「没变化」");
  assert.deepEqual(merged!.map((p) => p.device_id), ["b", "a"]);
  assert.equal(merged![0], b, "换位但仍要复用旧对象");
});

test("link 从有到无（链路断开）算变化 —— 可选字段不许被当成相等", () => {
  const prev = [peer("a", { link: "bluetooth" })];
  const next = [peer("a", { link: null })];
  const merged = mergePeerList(prev, next);
  assert.ok(merged, "link 由 bluetooth 变 null 是一次真实的状态变化");
  assert.equal(merged![0].link, null);
});
