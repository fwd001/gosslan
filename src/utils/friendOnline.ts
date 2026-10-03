/**
 * `friends[].online` 的判定口径 —— **唯一的一份实现**。
 *
 * ## 为什么单独成文件而不是留在 store 里
 * 这段逻辑曾经被抄过两份（`refreshPeers` 那条链与 `searchNearbyPeers`），
 * 其中一份漏了「持有活跃链路也算在线」这一层，于是：用户点开"添加好友"弹窗、
 * 一次 `who_has` 探测回来的瞬间，正在保持 TCP 链路的好友被判成离线 ——
 * 界面表现是 `ChatHeader` 的 `v-if="!isGroup && online && linkState"` 整块链路信息
 * 凭空消失。**同一份语义的两个家必然漂移**（AI_RULES §32），所以收在这里，
 * 由 `useChatStore` 的两个调用点共用。
 *
 * 抽成纯函数而不是 store 私有函数，是为了**能测**：藏在 store 里的口径只能靠
 * 运行时观察，护栏无从下手（这正是它漂了没人发现的原因）。
 *
 * ## 口径与后端对齐
 * 后端 `commands/friends.rs::friend_is_online` 有**三**层：
 *
 * ```rust
 * has_active_link || last_seen >= now - FRIEND_ONLINE_GRACE_MS
 * ```
 *
 * ① 探测列表里出现过；② 持有活跃链路；③ 15 秒宽限。
 * 前端拿不到 `last_seen` 时间戳，**算不了第三层**，因此只实现前两层 ——
 * 而少了第二层会立刻穿帮：广播被 sweep / 局域网丢包时，明明 TCP 还连着却显示离线。
 * 前两层已覆盖用户能感知的场景，15 秒宽限是"心跳刚停但还没超时"那 15 秒的余量。
 * ## ★ 关于 `linkedIds` 那一层：它当前是**冗余**的，别当它在兜什么
 * 后端给 `link` 填值的两个入口形状完全相同，**都只给「已经在 peers 表里」的节点填**：
 *
 * · `state.rs::emit_peers_now`（`peers-updated` 事件来源）：先从 `self.peers` 收集
 *   `peers`，再 `for p in peers.iter_mut()` 按 `p.device_id` 查 `links` 填 `p.link`；
 * · `commands/network.rs::fill_peer_links`（`search_nearby_peers` / `get_peers` 来源）：
 *   同样先收 `s.peers`，再 `for p in peers.iter_mut()` 填。
 *
 * 恒有 `link` 非空 ⇒ 该节点在同一个 list 里 ⇒ `linkedIds ⊆ onlineIds`。
 *
 * 审计曾把「`searchNearbyPeers` 漏了 linkedIds」报成缺陷（好友会在建链时被闪成离线）。
 * **该结论已核查并排除**：修好它需要「link 能给一个不在 peers 表里的节点」，
 * 而上面两条路径都做不到 —— 节点不在表里，后端不会给它 `link` 字段。
 * `state.rs:1770-1774` 那段注释描述的痛点是真的，但**不是前端能修的**：
 * 该修的是后端让 peers 表覆盖"有链路但广播没到"（或让 `fill_peer_links` 额外接受
 * 一组待填 id），不在本文件范围。
 *
 * 为什么**保留**这一层而不是删掉：它**无害**（子集关系使它恒为冗余，不会算错），
 * 且后端哪天真的支持了"表外节点带 link"，这一行就自动开始生效。
 * 但上面这段推导必须留着 —— 否则下一个人会误以为它在兜什么，
 * 或反过来以为"删掉会引入 bug"而不敢简化。
 * `friendOnline.test.ts` 末尾有一条判据钉着这个数据形状假设。
 */

/** 探测/节点快照里我们用得到的最小形状（与 `types.ts` 的 `Peer` 对齐）。 */
export interface PeerOnlineInput {
  device_id: string;
  /** 有值 = 当前持有活跃链路（`PathKind` 的字符串形式）。 */
  link?: string | null;
}

/** 只需 `device_id` 与 `online` 的最小形状（与 `types.ts` 的 `Friend` 对齐）。 */
export interface FriendOnlineTarget {
  device_id: string;
  online: boolean;
}

/**
 * 就地标注 `friends[].online` 并返回同一个数组（便于链式调用与测试断言）。
 *
 * 判定：在节点表里 **或** 持有活跃链路 ⇒ 在线。
 *
 * 与后端 `commands/friends.rs::friend_is_online` 的差别：后端还有第三层
 * 「15 秒宽限」（`FRIEND_ONLINE_GRACE_MS`，按 `last_seen` 判断），前端拿不到时间戳、
 * 算不了；前两层已覆盖用户能感知的场景。
 */
export function markFriendsOnlineFrom<T extends FriendOnlineTarget>(
  friends: T[],
  list: readonly PeerOnlineInput[],
): T[] {
  const onlineIds = new Set(list.map((x) => x.device_id));
  const linkedIds = new Set(list.filter((x) => x.link).map((x) => x.device_id));
  friends.forEach((f) => (f.online = onlineIds.has(f.device_id) || linkedIds.has(f.device_id)));
  return friends;
}
