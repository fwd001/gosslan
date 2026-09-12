/**
 * 「新朋友」（好友申请列表）的过滤规则 —— 纯函数，便于单测。
 *
 * ## 规则（用户 2026-09-12 真机实测要求）
 * 「如果两个人已经互相加上好友了（可能双方都给对方发送了加好友申请），其中一个人点了确定，
 * 另一个人点进『新朋友』列表，**如果该好友已在好友列表的话，那条好友申请就应该自动清除掉**」。
 *
 * 为什么做成"渲染时过滤"而不是只靠各条路径手动删：申请被"解决"的方式有好几种
 * （我同意、对方同意、重启后重新拉取、对方走别的消息把我加上的），
 * 只要**事实**是"他已经是我的好友"，那一行就不该出现 —— 用事实判定，不依赖某条回执送达。
 */
export function actionableRequests<T extends { from: string }>(
  requests: readonly T[],
  friendIds: Iterable<string>,
): T[] {
  const ids = new Set(friendIds);
  return requests.filter((r) => !ids.has(r.from));
}
