/**
 * 群公告横幅的「这台设备已经看过这一条」记录（用户 2026-09-29 需求汇总八：
 * 「其他成员看完可临时关闭；关闭后不再强制弹出，直到公告更新 / 更换」）。
 *
 * ## 为什么记的是 `msgId` 而不是"已读时间"
 * 公告是覆盖式的：群主改一次就发**一条新消息**，`msgId` 跟着变。
 * 于是「更新过」与「这条没看过」是同一件事的两种说法，不需要时间戳、不需要比对墙钟
 * （本仓库那条"跨文件比时间戳得到废数"的教训记的就是这类判断）。
 * 收起 ⇒ 记下这一条的 `msgId`；横幅只在**当前那条 ≠ 记下的那条**时出现。
 *
 * ## 为什么是**本机**记录，不跟群同步
 * "我看过了"是每台设备自己的状态：同一人在手机上看过了，不该让桌面端也不再提醒；
 * 反过来也不该为这一个偏好去动协议（本阶段原则：不为原生感扩协议）。
 *
 * ## 读失败一律退回空表
 * 这条记录丢了最坏是"横幅再出现一次"（顶多多看一眼），而解析失败如果抛错会让整个
 * 聊天面板挂载失败 —— 失败方向必须是**多提醒**，不许是白屏。
 */

export const ANNOUNCE_SEEN_KEY = "gosslan.announceSeen.v1";

/** `groupId -> 已收起的那条公告 msgId`。 */
export type AnnounceSeenMap = Record<string, string>;

type KVStore = { getItem(key: string): string | null; setItem(key: string, value: string): void };

/** 读整份记录：脏数据 / 非对象 / 存储不可用 ⇒ 空表（见文件头那条失败方向）。 */
export function readAnnounceSeen(store: KVStore | null): AnnounceSeenMap {
  if (!store) return {};
  let raw: string | null = null;
  try {
    raw = store.getItem(ANNOUNCE_SEEN_KEY);
  } catch {
    return {};
  }
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    const out: AnnounceSeenMap = {};
    for (const [gid, msgId] of Object.entries(parsed as Record<string, unknown>)) {
      // 只收字符串值：脏数据里混进数字/null 时，把它剔掉比让它顶着"相等判断"更诚实
      // （否则 `map[gid] === msgId` 会拿着 `[object Object]` 之类的东西比）。
      if (typeof msgId === "string" && msgId) out[gid] = msgId;
    }
    return out;
  } catch {
    return {};
  }
}

export function writeAnnounceSeen(store: KVStore | null, map: AnnounceSeenMap): void {
  if (!store) return;
  try {
    store.setItem(ANNOUNCE_SEEN_KEY, JSON.stringify(map));
  } catch {
    /* 存储被禁用/写满：收起只是锦上添花，下次再出现一条横幅可以接受 */
  }
}

/** 这条公告在本机是否已被收起（**按群分开**：A 群的收起不许影响 B 群）。 */
export function isAnnounceDismissed(map: AnnounceSeenMap, groupId: string, msgId: string): boolean {
  return map[groupId] === msgId;
}

/** 收起某群当前这条公告，返回新表（不改入参 —— 调用点是 Vue 的 ref，就地改会让旧值跟着变）。 */
export function withAnnounceDismissed(
  map: AnnounceSeenMap,
  groupId: string,
  msgId: string,
): AnnounceSeenMap {
  return { ...map, [groupId]: msgId };
}
