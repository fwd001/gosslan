/**
 * 附近设备列表的**引用稳定合并**（架构复审 P11）。
 *
 * 解决的问题不是"少算几个字段"，而是**每 333ms 把整屏重画一遍**：
 * `peers-updated` 最多每秒 3 次，三条写入点原先都是 `peers.value = list` ——
 * 一次 IPC 回来就是"数组换了、里面每个对象也换了"，于是任何读过 `peers` 的渲染
 * （`nicknameOf` 就在消息行的模板里逐行调用）全部失效，与这一拍**到底有没有东西变了**
 * 毫无关系。
 *
 * 两条约定：
 * · 内容一字未改 ⇒ 返回 `null`，调用方**根本不赋值**（Vue 也就什么都不通知）；
 * · 有变化 ⇒ 逐台比对，**没变的那台沿用旧对象**，只有真变的那台是新引用。
 *
 * 为什么按 `device_id` 复用、却仍然把"顺序变化"算作变化：顺序本身就是
 * "附近设备"列表的呈现顺序，压掉它会真画出不一样的东西 —— 那不是可以省的。
 */
import type { Peer } from "../types.ts";

/**
 * 逐字段浅比对。字段表**不手写**：取两边 own keys 的并集，
 * 于是 `Peer` 以后加字段会自动进比对 —— 手写清单迟早漏一个，
 * 而漏掉的那个字段会变成"值变了但界面不更新"（比多比对难得多的 bug）。
 */
function samePeer(a: Peer, b: Peer): boolean {
  if (a === b) return true;
  const ra = a as unknown as Record<string, unknown>;
  const rb = b as unknown as Record<string, unknown>;
  for (const key of new Set([...Object.keys(ra), ...Object.keys(rb)])) {
    if (ra[key] !== rb[key]) return false;
  }
  return true;
}

/** `null` = 与 `prev` 完全等价，调用方什么都不必做。 */
export function mergePeerList(prev: readonly Peer[], next: readonly Peer[]): Peer[] | null {
  const byId = new Map(prev.map((p) => [p.device_id, p]));
  const merged = next.map((p) => {
    const old = byId.get(p.device_id);
    return old && samePeer(old, p) ? old : p;
  });
  if (merged.length === prev.length && merged.every((p, i) => p === prev[i])) return null;
  return merged;
}
