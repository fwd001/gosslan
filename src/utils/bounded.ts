/**
 * 按插入序淘汰的容量闸 —— 给"只加不删"的模块级集合兜底。
 *
 * ## 为什么需要它
 * store 与组件里有多张**模块生命周期**的 `Set` / `Map`：`pendingAcks`（等不到实体的 Ack
 * 就永久留着）、`notifMap`（用户直接划掉的通知永不回收）、`heightOverride`（每实测一条
 * 消息高度留一项）、预览缓存（值里还握着 objectURL）。它们的共同形状是"跟着使用时长单调
 * 增长"，而这类泄漏的表现非常晚才看得见 —— 不是崩，是几小时之后越用越卡、内存越吃越多。
 *
 * ## 为什么不各写一份
 * 仓里已经有正确先例（`utils/messageHeight.ts` 的 `*_MAX` + FIFO 淘汰、`useChatStore` 的
 * `recentInboundIds`），但每个新集合都得重写一遍淘汰循环 —— 而漏写的那一个正是本次审计
 * 抓到的那些。收在这里，加一道闸就是一次调用。
 *
 * ## 语义
 * `Map` / `Set` 的迭代顺序**就是插入顺序**（ECMAScript 规范保证），所以"从头部删到不超过
 * max"= FIFO 淘汰。注意这只对"插入后不再改动"的用法成立：`map.set(k, v)` 命中已有键
 * **不会**把它移到队尾，热用的老条目会先被淘汰。本文件的所有调用点都是"一次性写入、
 * 读到即丢"的形态，可接受；将来若有人拿它保护会被反复覆盖的缓存，需要换成 LRU（`get` 时重插）。
 */

/**
 * 把集合裁到 `max` 条以内（超出部分按插入序淘汰最旧的），返回被淘汰的条数。
 *
 * `max <= 0` 视为"不该留任何东西"，全部清空 —— 不静默忽略，因为传 0 通常是配置写错了，
 * 而静默不清等于这道闸根本不存在。
 */
export function trimOldest(
  coll: Map<unknown, unknown> | Set<unknown>,
  max: number,
): number {
  if (coll.size <= max) return 0;
  let dropped = 0;
  if (coll instanceof Map) {
    for (const k of coll.keys()) {
      if (coll.size <= Math.max(0, max)) break;
      coll.delete(k);
      dropped += 1;
    }
    return dropped;
  }
  for (const v of coll.values()) {
    if (coll.size <= Math.max(0, max)) break;
    coll.delete(v);
    dropped += 1;
  }
  return dropped;
}
