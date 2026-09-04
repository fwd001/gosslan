import type { Conversation, MessageRecord } from "@/types";

/**
 * 合并去重并排序消息列表 —— Gossip 密集广播防重复的核心纯函数。
 * 去重键：msg_id（后端以 SHA-256 生成的 message_id 保证全网唯一）。
 * 排序：时间戳升序，同时间戳按 id 升序，保证稳定。
 */
export function mergeMessages(
  existing: MessageRecord[],
  incoming: MessageRecord[],
): MessageRecord[] {
  const seen = new Set<string>(existing.map((m) => m.msg_id));
  const merged: MessageRecord[] = [...existing];
  for (const m of incoming) {
    if (!seen.has(m.msg_id)) {
      seen.add(m.msg_id);
      merged.push(m);
    }
  }
  merged.sort((a, b) => a.ts - b.ts || a.id - b.id);
  return merged;
}

/** 消息摘要（会话列表展示）。 */
export function previewText(rec: MessageRecord): string {
  switch (rec.kind) {
    case "file":
      return "[文件]";
    case "image":
      return "[图片]";
    case "code":
      return "[代码]";
    default:
      return rec.content.slice(0, 30);
  }
}

/**
 * 将一批新消息应用到会话列表：更新 last_msg/last_ts、累计未读（活跃会话不计）、
 * 按 last_ts 降序重排。纯函数，便于测试与复用。
 */
export function applyIncomingToConversations(
  conversations: Conversation[],
  activeConvId: string | null,
  incomingByConv: Map<string, MessageRecord[]>,
): Conversation[] {
  const next: Conversation[] = conversations.map((c) => ({ ...c }));
  for (const [convId, msgs] of incomingByConv) {
    const last = msgs[msgs.length - 1];
    const conv = next.find((c) => c.id === convId);
    if (!conv) continue;
    conv.last_msg = previewText(last);
    conv.last_ts = last.ts;
    if (convId !== activeConvId) conv.unread += msgs.length;
  }
  next.sort((a, b) => (b.last_ts ?? 0) - (a.last_ts ?? 0));
  return next;
}
