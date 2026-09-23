/**
 * 系统通知正文（纯函数，便于单测）。
 *
 * 抽出来是因为它同时受「条数」和「显示正文隐私开关」两个维度影响，
 * 分支一旦写错（例如隐私开关只挡了单条、没挡多条）会在锁屏上泄内容。
 */

import type { MessageRecord } from "@/types";

/** 通知去抖队列里的一项：同一会话在窗口期内合并成一条。 */
export interface QueuedNotice {
  count: number;
  last: MessageRecord;
}

/**
 * 把一批"已经取出、这一轮却没能发出"的通知**放回**队列（权限查询失败时的回滚）。
 *
 * 为什么必须有（审计阶段 4 · 4.2）：`flushNotifications` 是**先 `clear()` 再 await 权限**，
 * 那条 IPC 一旦 reject，原先没有任何 catch ⇒ 这批通知直接从内存里消失，用户少收一条通知
 * 且毫无线索。放回是安全的：失败点在任何通知**发出之前**，所以不存在重复提醒。
 *
 * 合并规则与入队时一致（同会话累加条数、`last` 取更新的那条），否则放回会把窗口期内
 * 新到的一条覆盖成旧的，通知正文与未读跳转都会指错消息。
 */
export function mergeNoticesInto(
  queue: Map<string, QueuedNotice>,
  entries: readonly QueuedNotice[],
): void {
  for (const en of entries) {
    const cid = en.last.conv_id;
    const cur = queue.get(cid);
    if (!cur) {
      queue.set(cid, { count: en.count, last: en.last });
      continue;
    }
    cur.count += en.count;
    if (en.last.ts > cur.last.ts) cur.last = en.last;
  }
}

export interface NotificationBodyInput {
  /** 通知是否显示消息正文（隐私开关）。 */
  showContent: boolean;
  /** 本次要通知的消息条数（>1 表示合并提示）。 */
  count: number;
  /** 发送者昵称（仅 showContent 时用到）。 */
  sender: string;
  /** 单条消息的正文预览（仅 showContent && count === 1 时用到）。 */
  preview: string;
}

export function notificationBody(input: NotificationBodyInput): string {
  if (input.showContent) {
    return input.count > 1
      ? `${input.sender} 等 ${input.count} 条新消息`
      : input.preview;
  }
  // 隐私：关掉正文后只提示「收到新消息」，锁屏 / 通知中心不泄内容
  return input.count > 1 ? `你收到 ${input.count} 条新消息` : "你收到一条新消息";
}
