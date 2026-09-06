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

/** 送达状态推进序：只会前进，不会后退。 */
const DELIVERY_ORDER = ["sent", "delivered", "read"];

/** 两个送达状态中更靠后的一个；不在推进序上的状态（sending / failed）保持原样。 */
export function furthestStatus(status: string, ahead: string): string {
  return DELIVERY_ORDER.indexOf(ahead) > DELIVERY_ORDER.indexOf(status) ? ahead : status;
}

/**
 * 会话重查（getMessages）的快照可能取自「对方已读 / Ack 落库之前」，直接覆盖会把
 * 界面上已推进的状态退回「发送中」→ 合并时保留两者中更靠后的状态。
 */
export function preserveDeliveryStatus(
  fresh: MessageRecord[],
  local: MessageRecord[],
): MessageRecord[] {
  if (local.length === 0) return fresh;
  const known = new Map(local.map((m) => [m.msg_id, m.status]));
  return fresh.map((m) => {
    const prev = known.get(m.msg_id);
    const best = prev ? furthestStatus(m.status, prev) : m.status;
    return best === m.status ? m : { ...m, status: best };
  });
}

/**
 * 乐观记录（`tmp-*` msg_id）经 rAF 批量队列落地，而 invoke 可能先返回真实记录；
 * 此时按 msg_id 就地替换会落空，真实记录一旦被丢弃气泡就永久停在「发送中」。
 * 挂起的替换在批次落地这一唯一入口处完成。
 *
 * `currentStatuses` 是批次落地前各 msg_id 的状态快照；替换时用 furthestStatus
 * 与之比较，防止已推进到 read 的记录被一个 stale 的 pendingReplace 退回 delivered。
 */
export function applyReplacements(
  batch: MessageRecord[],
  replacements: Map<string, MessageRecord>,
  currentStatuses?: Map<string, string>,
): MessageRecord[] {
  if (replacements.size === 0) return batch;
  return batch.map((m) => {
    const next = replacements.get(m.msg_id);
    if (!next) return m;
    replacements.delete(m.msg_id);
    if (currentStatuses) {
      const cur = currentStatuses.get(m.msg_id);
      if (cur) return { ...next, status: furthestStatus(cur, next.status) };
    }
    return next;
  });
}

/**
 * 从 peers 列表同步好友昵称/头像到 friends 和单聊 conversations。
 * 纯函数，便于测试；调用方为 useChatStore.onPeers。
 */
export function syncProfileFromPeers(
  friends: { device_id: string; nickname: string; avatar: string | null }[],
  conversations: { id: string; kind: string; name: string; avatar: string | null }[],
  peers: { device_id: string; nickname: string; avatar: string | null }[],
): void {
  const peerMap = new Map(peers.map((p) => [p.device_id, p]));
  for (const f of friends) {
    const peer = peerMap.get(f.device_id);
    if (peer) {
      f.nickname = peer.nickname;
      f.avatar = peer.avatar;
    }
  }
  for (const c of conversations) {
    if (c.kind !== "single") continue;
    const peer = peerMap.get(c.id);
    if (peer) {
      c.name = peer.nickname;
      c.avatar = peer.avatar;
    }
  }
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
