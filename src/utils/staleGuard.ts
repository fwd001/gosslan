/**
 * 「按 key 只让最新一次发起的请求落地」的守卫 —— 异步回填竞态的最小实现。
 *
 * ## 为什么需要
 * 本仓有一整族同形缺陷：`refresh*`、预览解析、投递摘要、共享目录树……它们都是
 * `x.value = await api.foo()`。IPC 没有顺序保证，**后发起的请求可能先回来**，
 * 于是旧快照覆盖掉新快照。表现都很隐蔽且不报错：未读回退、列表下坠、
 * "刚发的文件从传输列表里消失、进度条永久钉在 0%"（`refreshTransfers` 的真事故，
 * 见 `useChatStore` 里那段注释）、聊天头显示别的对端的链路状态。
 *
 * ## 为什么是"计数"而不是"比较 id"
 * 比较目标 id 只能挡住"切到了别的会话"这一种；挡不住**同一个目标被连发两次**
 * （例如两次 `void refreshConversations()` 交错，或滚动时反复 `resolveCurrent`）。
 * 计数天然覆盖两种情形，且不需要调用方把自己的目标传回来。
 *
 * ## 语义边界
 * 令牌只回答"我是不是最新的一次"，不回答"数据是否变化"。因此调用点必须把
 * **所有副作用**（写结果、写 error、清 loading）都放在 `isCurrent` 之后 —— 漏一个
 * 就等于那个副作用仍会被旧请求执行。`finally` 里的收尾尤其容易漏。
 */
export class StaleGuard {
  private readonly seq = new Map<string, number>();

  /**
   * 为 `key` 开启一次新请求，返回本次令牌。
   * 同 key 的更早令牌会立刻失效 —— 即使那次请求还在飞。
   */
  begin(key: string): number {
    const n = (this.seq.get(key) ?? 0) + 1;
    this.seq.set(key, n);
    return n;
  }

  /** 这次请求是否仍是该 key 上最新的一次（`false` ⇒ 结果必须整个丢弃）。 */
  isCurrent(key: string, token: number): boolean {
    return this.seq.get(key) === token;
  }

  // 刻意**不提供** forget(key)：删掉计数会让下一次 begin 从 1 重新开始，而一个仍在飞的
  // 旧请求手里的令牌可能恰好就是 1 ⇒ 它反而被判成"最新的一次"，把旧快照写回来。
  // 本工具的 key 是**固定的小集合**（peers/conversations/groups/friends/pending/transfers），
  // 不清理也不会增长 —— 这正是它适合放在这里的理由。
}
